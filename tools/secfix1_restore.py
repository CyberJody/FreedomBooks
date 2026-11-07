"""Apply the bounded two-pass restore extraction changes."""
from pathlib import Path
p = Path(__file__).resolve().parents[1] / 'crates/fb-core/src/backups/mod.rs'
t = p.read_text()
t = t.replace('mod paths;', 'mod paths;\nmod restore_limits;')
t = t.replace('fn extract(archive: &Path, work: &Path, outer: bool)', 'fn extract(archive: &Path, work: &Path, outer: bool, data: Option<&Manifest>)')
t = t.replace('''    let mut tar = tar::Archive::new(paths::regular(archive)?);
    let mut seen''', '''    let file = paths::regular(archive)?;
    if file.metadata()?.len() > restore_limits::ABSOLUTE {
        return Err(BackupError::refused("Backup size exceeds the restore limit."));
    }
    let mut tar = tar::Archive::new(file);
    let mut total = 0u64;
    let mut seen''')
t = t.replace('for item in tar.entries()?.raw(true)', 'for item in tar.entries_with_seek()?.raw(true)')
t = t.replace('''            _ => u64::MAX,''', '''            _ => restore_limits::ABSOLUTE,''')
old = '''        paths::require_space(work, size.saturating_add(1024 * 1024))?;'''
new = '''        total = total.checked_add(size).filter(|n| *n <= restore_limits::ABSOLUTE)
            .ok_or_else(|| BackupError::refused("Backup size exceeds the restore limit."))?;
        let metadata = matches!(name.as_str(), "header.json" | "manifest.json" | "manifest.enc");
        if let Some(data) = data {
            if total > restore_limits::budget(data)? { return Err(BackupError::refused("Backup size exceeds the restore limit.")); }
            if metadata { continue; }
            let expected = if name == "book.fbbook" { Some(data.book_bytes) }
                else { data.attachments.iter().find(|r| r.entry == name).map(|r| r.bytes) };
            if expected != Some(size) { return Err(BackupError::refused("Backup entry size does not match its manifest.")); }
        } else if !metadata { continue; }
        paths::require_space(work, size.saturating_add(1024 * 1024))?;'''
assert old in t
t=t.replace(old,new)
t=t.replace('let members = extract(path, work, true)?;', 'let members = extract(path, work, true, None)?;')
start=t.index('''        crypto::decrypt_stream(''',t.index('fn unpack('))
end=t.index('''    } else {''', start)
t=t[:start]+'''        // Stream ciphertext directly from the archive; only bounded, authenticated
        // plaintext reaches a temporary file, removed automatically on any error.
        let mut tar = tar::Archive::new(paths::regular(path)?);
        let mut dst = restore_limits::Payload::new(work)?;
        for entry in tar.entries_with_seek()?.raw(true) {
            let mut entry = entry?;
            if entry.path_bytes().as_ref() == b"payload.enc" {
                crypto::decrypt_stream(&mut entry, &mut dst, pass, &header, &raw)?;
            }
        }
        dst.publish(&inner)?;
        extract(&inner, work, false, None)?
''' + t[end:]
marker='''    validate_manifest(&data)?;
    let expected'''
assert marker in t
t=t.replace(marker,'''    validate_manifest(&data)?;
    restore_limits::budget(&data)?;
    extract(if encrypted { &work.join("payload.tar") } else { path }, work, !encrypted, Some(&data))?;
    let expected''')
p.write_text(t)
