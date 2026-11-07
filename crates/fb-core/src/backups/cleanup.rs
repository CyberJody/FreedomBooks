//! Interrupted staging cleanup: this host, dead pids, exact names only.
use super::{Result, paths};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn host() -> Option<String> {
    let name = fs::read_to_string("/etc/hostname").ok()?;
    Some(format!("{:x}", Sha256::digest(name.trim().as_bytes()))[..16].into())
}
pub(super) fn prefix() -> String {
    format!(
        ".fb-backup.{}.{}.",
        host().unwrap_or_else(|| "unknown".into()),
        std::process::id()
    )
}
pub fn cleanup_stale(directory: &Path) -> Result<Vec<PathBuf>> {
    paths::resolve(directory)?;
    let Some(host) = host() else {
        return Ok(Vec::new());
    };
    if !directory.try_exists()? {
        return Ok(Vec::new());
    }
    let mut removed = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(tail) = name.strip_prefix(&format!(".fb-backup.{host}.")) else {
            continue;
        };
        let Some((pid, random)) = tail.split_once('.') else {
            continue;
        };
        let Ok(pid) = pid.parse::<u32>() else {
            continue;
        };
        let random = random.strip_suffix(".partial").unwrap_or(random);
        if pid == 0
            || random.len() < 6
            || random.len() > 32
            || !random.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            continue;
        }
        // No reliable portable liveness oracle in std. On unsupported systems
        // keep the file; inability to check must never mean "dead".
        #[cfg(target_os = "linux")]
        let dead = fs::metadata(format!("/proc/{pid}"))
            .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound);
        #[cfg(not(target_os = "linux"))]
        let dead = false;
        if !dead {
            continue;
        }
        let kind = entry.file_type()?;
        if !kind.is_file() && !kind.is_dir() {
            continue;
        }
        #[cfg_attr(not(unix), allow(unused_variables))]
        let meta = entry.metadata()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.uid() != fs::metadata(directory)?.uid() {
                continue;
            }
        }
        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(entry.path())?;
        } else if entry.file_type()?.is_file() {
            fs::remove_file(entry.path())?;
        } else {
            continue;
        }
        removed.push(entry.path());
    }
    Ok(removed)
}

/// BACKUPS-POLISH: a restore target folder (`books/restored-<name>/`) left behind by a crash. The dead-pid
/// partial copies go first (`cleanup_stale`, this host, owner only). The folder is then residue only when it
/// is empty, or when a dead-pid partial was removed and the one thing left is the half-written `receipts/`
/// folder (restore writes receipts before it publishes the book, and the partial lives until publication).
/// Only then is the folder removed (`Ok(true)`). A book file, a live-pid partial, a symlink or anything else
/// keeps the folder (`Ok(false)`): the caller still refuses the name.
pub fn clear_interrupted_restore(folder: &Path) -> Result<bool> {
    let Ok(meta) = fs::symlink_metadata(folder) else {
        return Ok(false);
    };
    if !meta.is_dir() {
        return Ok(false);
    }
    let removed = cleanup_stale(folder)?;
    let mut rest = Vec::new();
    for entry in fs::read_dir(folder)? {
        rest.push(entry?);
    }
    match rest.as_slice() {
        [] => {}
        [only] if !removed.is_empty() && only.file_name() == "receipts" && only.file_type()?.is_dir() => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if only.metadata()?.uid() != meta.uid() {
                    return Ok(false);
                }
            }
            fs::remove_dir_all(only.path())?;
        }
        _ => return Ok(false),
    }
    fs::remove_dir(folder)?;
    Ok(true)
}

#[cfg(test)]
mod interrupted_restore_tests {
    use super::*;

    fn residue(dir: &Path, pid: u32) -> PathBuf {
        let p = dir.join(format!(".fb-backup.{}.{pid}.AbC123xyz.partial", host().unwrap()));
        fs::write(&p, b"half a book").unwrap();
        p
    }

    #[test]
    fn dead_pid_residue_is_cleared_and_anything_else_is_kept() {
        let d = tempfile::tempdir().unwrap();
        // Dead pid partial plus a half-written receipts folder: residue, removed.
        let a = d.path().join("restored-a");
        fs::create_dir_all(a.join("receipts")).unwrap();
        fs::write(a.join("receipts").join("x.bin"), b"r").unwrap();
        residue(&a, 4_294_967_295);
        assert!(clear_interrupted_restore(&a).unwrap());
        assert!(!a.exists());
        // An empty folder (the host made it, the restore never started): removed.
        let e = d.path().join("restored-e");
        fs::create_dir(&e).unwrap();
        assert!(clear_interrupted_restore(&e).unwrap());
        assert!(!e.exists());
        // A live pid (this process): a restore may still be running; kept.
        let b = d.path().join("restored-b");
        fs::create_dir(&b).unwrap();
        let live = residue(&b, std::process::id());
        assert!(!clear_interrupted_restore(&b).unwrap());
        assert!(live.exists());
        // A finished book (with receipts), even next to a dead partial: kept; the partial alone goes.
        let c = d.path().join("restored-c");
        fs::create_dir_all(c.join("receipts")).unwrap();
        fs::write(c.join("restored-c.fbbook"), b"book").unwrap();
        let dead = residue(&c, 4_294_967_295);
        assert!(!clear_interrupted_restore(&c).unwrap());
        assert!(c.join("restored-c.fbbook").exists() && c.join("receipts").exists() && !dead.exists());
        // receipts/ without any dead partial is not proof of a crash: kept.
        let r = d.path().join("restored-r");
        fs::create_dir_all(r.join("receipts")).unwrap();
        assert!(!clear_interrupted_restore(&r).unwrap());
        assert!(r.join("receipts").exists());
        // Not a folder, or missing: nothing to do.
        fs::write(d.path().join("file"), b"x").unwrap();
        assert!(!clear_interrupted_restore(&d.path().join("file")).unwrap());
        assert!(!clear_interrupted_restore(&d.path().join("missing")).unwrap());
        #[cfg(unix)]
        {
            let target = d.path().join("elsewhere");
            fs::create_dir(&target).unwrap();
            std::os::unix::fs::symlink(&target, d.path().join("restored-link")).unwrap();
            assert!(!clear_interrupted_restore(&d.path().join("restored-link")).unwrap());
            assert!(target.exists());
        }
    }
}
