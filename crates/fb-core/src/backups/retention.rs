//! Exact labels, filename stamps (not mtimes), verified-note and clock-rollback protection.
use super::{BackupError, Result, hash_file, parse_json, paths};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

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
    mut note: serde_json::Value,
) -> Result<()> {
    if &hash_file(path)? != fingerprint {
        return Err(BackupError::refused(
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
    paths::resolve(&target)?;
    let parent = path
        .parent()
        .ok_or_else(|| BackupError::refused("Missing backup folder."))?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(&serde_json::to_vec(&note)?)?;
    tmp.as_file().sync_all()?;
    tmp.persist(&target)
        .map_err(|e| BackupError::refused(format!("Could not record verification: {}", e.error)))?;
    paths::sync_dir(parent)?;
    Ok(())
}
pub fn verified_note(path: &Path) -> bool {
    (|| -> Result<bool> {
        let side = sidecar(path);
        let mut f = paths::regular(&side)?;
        if f.metadata()?.len() > 1024 * 1024 {
            return Ok(false);
        }
        let mut raw = Vec::new();
        std::io::Read::read_to_end(&mut f, &mut raw)?;
        let note = parse_json(&raw)?;
        let meta = fs::metadata(path)?;
        let mtime = meta
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| BackupError::refused("Invalid time."))?
            .as_nanos()
            .to_string();
        let (bytes, sha) = hash_file(path)?;
        Ok(note["ok"] == true
            && note["bytes"].as_u64() == Some(bytes)
            && note["sha256"] == sha
            && note["mtime_ns"] == mtime)
    })()
    .unwrap_or(false)
}
fn same_file(a: &Path, b: &Path) -> bool {
    same_file::is_same_file(a, b).unwrap_or(true)
}
fn remove_regular(path: &Path) -> Result<bool> {
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

pub fn prune(dir: &Path, label: &str, keep: usize, protect: Option<&Path>) -> Result<Vec<PathBuf>> {
    if keep == 0 {
        return Err(BackupError::refused("--keep must be at least 1."));
    }
    let mine = backup_files(dir, Some(label))?;
    let verified = mine.iter().find(|p| verified_note(p));
    let mut removed = Vec::new();
    for path in mine.iter().skip(keep) {
        if protect.is_some_and(|p| same_file(path, p))
            || verified.is_some_and(|p| same_file(path, p))
        {
            continue;
        }
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
