//! Exact labels, filename stamps (not mtimes), verified-note and clock-rollback protection.
use super::{BackupError, Result, hash_file, parse_json, paths};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

/// A verification note's company binding, without exposing company identifiers or keys.
/// A changed master key requires a fresh verified backup before Home can count it.
pub fn company_tag(key: &fb_db::Key, company: &str) -> Result<String> {
    let mut tag = [0u8; 32];
    hkdf::Hkdf::<Sha256>::new(Some(company.as_bytes()), key)
        .expand(b"FreedomBooks verified backup company v1", &mut tag)
        .map_err(|_| BackupError::refused("Backup status binding failed."))?;
    Ok(tag.iter().map(|b| format!("{b:02x}")).collect())
}

pub fn safe_db_label(name: &str) -> String {
    if !name.is_empty()
        && name.len() <= 63
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return name.into();
    }
    let prefix: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "_-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .take(54)
        .collect();
    format!(
        "{}.{hash}",
        if prefix.is_empty() { "db" } else { &prefix },
        hash = &format!("{:x}", Sha256::digest(name.as_bytes()))[..8]
    )
}
pub fn filename(label: &str, time: &str) -> Result<String> {
    if label.is_empty() {
        return Err(BackupError::refused("A backup label is required."));
    }
    let time = chrono::DateTime::parse_from_rfc3339(time)
        .map_err(|_| BackupError::refused("Invalid backup time."))?;
    Ok(format!(
        "freedombooks-{}-{}.fbbackup",
        safe_db_label(label),
        time.with_timezone(&chrono::Utc).format("%Y%m%dT%H%M%SZ")
    ))
}
fn label_of(name: &str) -> Option<&str> {
    let core = name
        .strip_prefix("freedombooks-")?
        .strip_suffix(".fbbackup")?;
    let (label, stamp) = core.rsplit_once('-')?;
    if stamp.len() != 16
        || stamp.as_bytes()[8] != b'T'
        || stamp.as_bytes()[15] != b'Z'
        || !stamp
            .bytes()
            .enumerate()
            .all(|(i, b)| i == 8 || i == 15 || b.is_ascii_digit())
    {
        return None;
    }
    let valid = |s: &str| {
        !s.is_empty()
            && s.len() <= 63
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    };
    if valid(label)
        || label.rsplit_once('.').is_some_and(|(p, h)| {
            valid(p)
                && h.len() == 8
                && h.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
    {
        Some(label)
    } else {
        None
    }
}
pub fn backup_files(dir: &Path, label: Option<&str>) -> Result<Vec<PathBuf>> {
    paths::resolve(dir)?;
    if !dir.try_exists()? {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        if let Some(name) = entry.file_name().to_str()
            && let Some(found) = label_of(name)
            && label.is_none_or(|l| l == found)
        {
            files.push(entry.path());
        }
    }
    // The stamp is the final 16 characters before suffix; with exact label all
    // files share the prefix, so filename order is timestamp order.
    let stamp = |p: &PathBuf| {
        p.file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.rsplit_once('-').map(|(_, stamp)| stamp))
            .unwrap_or("")
            .to_owned()
    };
    files.sort_by(|a, b| {
        stamp(b)
            .cmp(&stamp(a))
            .then_with(|| b.file_name().cmp(&a.file_name()))
    });
    Ok(files)
}
pub fn sidecar(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".verified.json");
    PathBuf::from(name)
}
pub fn record_verified(path: &Path) -> Result<()> {
    let fingerprint = hash_file(path)?;
    record_verified_matching(path, &fingerprint, json!({}))
}
pub(super) fn record_verified_matching(
    path: &Path,
    fingerprint: &(u64, String),
    note: serde_json::Value,
) -> Result<()> {
    let dir = paths::Directory::open(paths::parent_name(path)?.0)?;
    record_verified_inner(&dir, path, fingerprint, note, false)
}

pub(super) fn record_verified_noclobber(
    dir: &paths::Directory, path: &Path, fingerprint: &(u64, String), note: serde_json::Value,
) -> Result<()> {
    record_verified_inner(dir, path, fingerprint, note, true)
}

fn record_verified_inner(
    dir: &paths::Directory, path: &Path, fingerprint: &(u64, String),
    mut note: serde_json::Value, noclobber: bool,
) -> Result<()> {
    let (parent, name) = paths::parent_name(path)?;
    let changed = || BackupError::conflict("The backup changed while verification ran; no verification was recorded.");
    let archive = dir.read_named(name)?.ok_or_else(changed)?;
    let meta = archive.metadata()?;
    if meta.len() != fingerprint.0
        || &super::copy_and_hash((&archive).take(fingerprint.0.saturating_add(1)), &mut io::sink(), u64::MAX)? != fingerprint {
        return Err(changed());
    }
    let (bytes, digest) = fingerprint;
    let mtime = meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| BackupError::refused("Invalid backup timestamp."))?
        .as_nanos()
        .to_string();
    note["ok"] = json!(true);
    note["bytes"] = json!(bytes);
    note["sha256"] = json!(digest);
    note["mtime_ns"] = json!(mtime);
    let target = sidecar(path);
    match fs::symlink_metadata(&target) {
        Ok(meta) if !meta.file_type().is_file() => {
            return Err(BackupError::conflict(
                "The backup verification file path is occupied by a non-regular entry.",
            ));
        }
        Ok(_) => (),
        Err(e) if e.kind() == io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    if !noclobber {
        paths::resolve(&target)?;
    }
    let bytes = serde_json::to_vec(&note)?;
    let result = dir.write_private(paths::parent_name(&target)?.1, noclobber, |file| Ok(file.write_all(&bytes)?));
    dir.confirm(parent)?;
    match result {
        Err(e) if noclobber && e.is_conflict() => {
            if matching_note_in(dir, path)?.is_some_and(|existing| {
                existing["complete"] == note["complete"]
                    && existing["scheduled"] == note["scheduled"]
                    && existing["file"] == note["file"]
                    && existing["book_sha256"] == note["book_sha256"]
                    && existing["bytes"] == note["bytes"]
                    && existing["sha256"] == note["sha256"]
                    && existing["verified_at"]
                        .as_str()
                        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                        .is_some()
            }) {
                return Ok(());
            }
            Err(BackupError::conflict("The backup verification file path is already occupied."))
        }
        other => other,
    }
}

pub fn verified_note(path: &Path) -> bool {
    matching_note(path).is_some()
}

pub(super) fn read_note(path: &Path) -> Result<serde_json::Value> {
    read_note_file(paths::regular(&sidecar(path))?)
}

pub(super) fn read_note_file(f: fs::File) -> Result<serde_json::Value> {
    let mut raw = Vec::new();
    f.take(1024 * 1024 + 1).read_to_end(&mut raw)?;
    if raw.len() > 1024 * 1024 {
        return Err(BackupError::refused(
            "Backup verification record is too large.",
        ));
    }
    parse_json(&raw)
}

pub(super) fn note_matches(
    note: &serde_json::Value,
    meta: &fs::Metadata,
    bytes: u64,
    sha: &str,
) -> Result<bool> {
    let mtime = meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| BackupError::refused("Invalid time."))?
        .as_nanos()
        .to_string();
    Ok(note["ok"] == true
        && note["bytes"].as_u64() == Some(bytes)
        && note["sha256"] == sha
        && note["mtime_ns"] == mtime)
}

pub(super) fn matching_note(path: &Path) -> Option<serde_json::Value> {
    let dir = paths::Directory::open(paths::parent_name(path).ok()?.0).ok()?;
    matching_note_in(&dir, path).ok().flatten()
}

fn matching_note_in(dir: &paths::Directory, path: &Path) -> Result<Option<serde_json::Value>> {
    let result = (|| {
        let Some(note) = dir.read_named(paths::parent_name(&sidecar(path))?.1)? else { return Ok(None) };
        let note = read_note_file(note)?;
        let Some(mut archive) = dir.read_named(paths::parent_name(path)?.1)? else { return Ok(None) };
        let meta = archive.metadata()?;
        if note["bytes"].as_u64() != Some(meta.len()) { return Ok(None); }
        let (bytes, sha) = super::hash_open_file(&mut archive)?;
        Ok(note_matches(&note, &meta, bytes, &sha)?.then_some(note))
    })();
    match result {
        Err(BackupError { failure: super::Failure::Validation, .. }) => Ok(None),
        other => other,
    }
}

/// Quarantine both names relative to the pinned schedule directory handle before verifying.
/// Once populated, the quarantine directory survives errors and mismatches so an unexpected
/// file is never auto-deleted, and Unix moves/reads/unlinks never re-resolve ancestor paths.
pub(super) fn quarantine_remove(
    dir: &paths::Directory,
    archive_name: &str,
    eligible: impl FnOnce(&paths::Directory, &str, &str) -> Result<bool>,
) -> Result<bool> {
    let note_name = format!("{archive_name}.verified.json");
    let (q_name, q_dir) = dir.create_private_subdir(".fb-retention-")?;
    match dir.rename_to(archive_name, &q_dir, archive_name) {
        Ok(()) => (),
        Err(e) => {
            dir.remove_empty_subdir(&q_name)?;
            if e.kind() == io::ErrorKind::NotFound {
                return Ok(false);
            }
            return Err(e.into());
        }
    }
    let restore = |name| q_dir.restore_to(name, dir).map_err(|e| {
        BackupError::retryable(format!("Retention could not restore {name} from {q_name}: {e}"))
    });
    let moved = match dir.rename_to(&note_name, &q_dir, &note_name) {
        Ok(()) => true,
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => {
            restore(archive_name)?;
            dir.remove_empty_subdir(&q_name)?;
            dir.sync()?;
            return Err(e.into());
        }
    };
    let decision = if moved { eligible(&q_dir, archive_name, &note_name) } else { Ok(false) };
    if matches!(decision, Ok(true)) {
        q_dir.unlink_file(archive_name)?;
        q_dir.unlink_file(&note_name)?;
    } else {
        // On collision keep the remaining pair together for manual recovery.
        restore(archive_name)?;
        if moved { restore(&note_name)?; }
    }
    q_dir.sync()?;
    dir.remove_empty_subdir(&q_name)?;
    dir.sync()?;
    decision
}

fn same_file(a: &Path, b: &Path) -> bool {
    same_file::is_same_file(a, b).unwrap_or(true)
}
fn remove_regular(path: &Path) -> Result<bool> {
    if let Some(parent) = path.parent() {
        paths::resolve(parent)?;
    }
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.file_type().is_file() => return Ok(false),
        Ok(_) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Whether a backup is complete: it is not flagged incomplete (missing attachment files). The flag is in
/// the public header, which the manifest authenticates. A file whose header cannot be read is not
/// flagged, so retention errs on the side of keeping it.
pub fn complete(path: &Path) -> bool {
    !super::read_header(path).is_ok_and(|h| h["incomplete"] == true)
}

/// Remove this label's backups beyond the newest `keep`, never `protect` and never the newest verified
/// complete backup (an incomplete one, however new, does not stand in for it).
pub fn prune(dir: &Path, label: &str, keep: usize, protect: Option<&Path>) -> Result<Vec<PathBuf>> {
    if keep == 0 {
        return Err(BackupError::refused("--keep must be at least 1."));
    }
    let mine = backup_files(dir, Some(label))?;
    if mine.is_empty() { return Ok(Vec::new()); }
    let pinned = paths::Directory::open(dir)?;
    let mut verified = None;
    for path in &mine {
        if matching_note_in(&pinned, path)?.is_some() && complete(path) {
            verified = Some(path);
            break;
        }
    }
    remove_backups(mine.iter().skip(keep).filter(|path| {
        !protect.is_some_and(|p| same_file(path, p))
            && !verified.is_some_and(|p| same_file(path, p))
    }))
}

pub(super) fn remove_backups<'a>(
    paths: impl IntoIterator<Item = &'a PathBuf>,
) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    for path in paths {
        // Refuse if ownership/type changed since the inventory; never follow a symlink.
        if !remove_regular(path)? {
            continue;
        }
        let side = sidecar(path);
        remove_regular(&side)?;
        removed.push(path.clone());
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quarantine_io_error_restores_the_pair_and_a_collision_names_the_retained_copy() {
        let temp = tempfile::tempdir().unwrap();
        let dir = paths::Directory::open(temp.path()).unwrap();
        let file = temp.path().join("archive");
        let note = sidecar(&file);
        fs::write(&file, b"original").unwrap();
        fs::write(&note, b"note").unwrap();
        let error = quarantine_remove(&dir, "archive", |_, _, _| Err(io::Error::other("read failure").into())).unwrap_err();
        assert!(error.to_string().contains("read failure"));
        assert_eq!(fs::read(&file).unwrap(), b"original");
        assert_eq!(fs::read(&note).unwrap(), b"note");

        let error = quarantine_remove(&dir, "archive", |_, _, _| {
            fs::write(&file, b"replacement")?;
            Ok(false)
        }).unwrap_err();
        let quarantine = fs::read_dir(temp.path()).unwrap().map(|e| e.unwrap().path())
            .find(|p| p.is_dir()).unwrap();
        assert!(error.to_string().contains(quarantine.file_name().unwrap().to_str().unwrap()));
        assert_eq!(fs::read(&file).unwrap(), b"replacement");
        assert_eq!(fs::read(quarantine.join("archive")).unwrap(), b"original");
        assert_eq!(fs::read(quarantine.join("archive.verified.json")).unwrap(), b"note");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn quarantine_can_restore_a_directory_without_hard_linking_it() {
        let temp = tempfile::tempdir().unwrap();
        let dir = paths::Directory::open(temp.path()).unwrap();
        fs::create_dir(temp.path().join("archive")).unwrap();
        fs::write(temp.path().join("archive/owned"), b"preserve").unwrap();
        assert!(!quarantine_remove(&dir, "archive", |_, _, _| panic!("missing note")).unwrap());
        assert_eq!(fs::read(temp.path().join("archive/owned")).unwrap(), b"preserve");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn vanished_retention_candidate_and_sidecar_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vanished.fbbackup");
        assert!(!remove_regular(&path).unwrap());
        fs::write(&path, b"synthetic").unwrap();
        assert!(remove_regular(&path).unwrap());
        assert!(!remove_regular(&path).unwrap());
        assert!(!remove_regular(&sidecar(&path)).unwrap());
    }
}
