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
    record_verified_inner(path, fingerprint, note, false)
}

pub(super) fn record_verified_noclobber(
    path: &Path,
    fingerprint: &(u64, String),
    note: serde_json::Value,
) -> Result<()> {
    record_verified_inner(path, fingerprint, note, true)
}

fn record_verified_inner(
    path: &Path,
    fingerprint: &(u64, String),
    mut note: serde_json::Value,
    noclobber: bool,
) -> Result<()> {
    if &hash_file(path)? != fingerprint {
        return Err(BackupError::conflict(
            "The backup changed while verification ran; no verification was recorded.",
        ));
    }
    let (bytes, digest) = fingerprint;
    let meta = fs::metadata(path)?;
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
    let parent = path
        .parent()
        .ok_or_else(|| BackupError::refused("Missing backup folder."))?;
    paths::resolve(parent)?;
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
    let mut tmp = tempfile::Builder::new()
        .prefix(&super::cleanup::prefix())
        .suffix(".partial")
        .tempfile_in(parent)?;
    tmp.write_all(&serde_json::to_vec(&note)?)?;
    tmp.as_file().sync_all()?;
    if noclobber {
        match tmp.persist_noclobber(&target) {
            Ok(_) => (),
            Err(e) if e.error.kind() == io::ErrorKind::AlreadyExists => {
                if matching_note(path).is_some_and(|existing| {
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
                return Err(BackupError::conflict(
                    "The backup verification file path is already occupied.",
                ));
            }
            Err(e) => {
                return Err(BackupError::refused(format!(
                    "Could not record verification: {}",
                    e.error
                )));
            }
        }
    } else {
        tmp.persist(&target).map_err(|e| {
            if fs::symlink_metadata(&target).is_ok_and(|meta| !meta.file_type().is_file()) {
                BackupError::conflict(
                    "The backup verification file path is occupied by a non-regular entry.",
                )
            } else {
                BackupError::refused(format!("Could not record verification: {}", e.error))
            }
        })?;
    }
    paths::sync_dir(parent)?;
    Ok(())
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
    let note = read_note(path).ok()?;
    let meta = fs::symlink_metadata(path).ok()?;
    let (bytes, sha) = hash_file(path).ok()?;
    note_matches(&note, &meta, bytes, &sha)
        .ok()?
        .then_some(note)
}

/// Quarantine both names relative to the pinned schedule directory handle before verifying.
/// Once populated, the quarantine directory survives errors and mismatches so an unexpected
/// file is never auto-deleted, and Unix moves/reads/unlinks never re-resolve ancestor paths.
pub(super) fn quarantine_remove(
    dir: &paths::Directory,
    archive_name: &str,
    eligible: impl FnOnce(&paths::Directory, &str, &str) -> bool,
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
    let moved = match dir.rename_to(&note_name, &q_dir, &note_name) {
        Ok(()) => true,
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => {
            put_back_in(&q_dir, archive_name, dir, archive_name)?;
            dir.remove_empty_subdir(&q_name)?;
            return Err(e.into());
        }
    };
    let remove = moved && eligible(&q_dir, archive_name, &note_name);
    if remove {
        q_dir.unlink_file(archive_name)?;
        q_dir.unlink_file(&note_name)?;
        q_dir.sync()?;
    } else {
        put_back_in(&q_dir, archive_name, dir, archive_name)?;
        if moved {
            put_back_in(&q_dir, &note_name, dir, &note_name)?;
        }
    }
    // If a name was reused or cannot be restored, retain the quarantined copy.
    dir.remove_empty_subdir(&q_name)?;
    dir.sync()?;
    Ok(remove)
}

fn put_back_in(
    from_dir: &paths::Directory,
    from_name: &str,
    to_dir: &paths::Directory,
    to_name: &str,
) -> Result<()> {
    if from_dir.link_to(from_name, to_dir, to_name).is_ok() {
        from_dir.unlink_file(from_name)?;
    }
    Ok(())
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
    let verified = mine.iter().find(|p| verified_note(p) && complete(p));
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
