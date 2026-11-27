//! Interrupted staging cleanup: this host, dead pids, exact names only.
use super::{Result, paths};
#[cfg(unix)]
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[cfg(unix)]
fn host_hash(name: &[u8]) -> String {
    format!("{:x}", Sha256::digest(name))[..16].into()
}

#[cfg(unix)]
fn host() -> Option<String> {
    let mut name = [0_u8; 256];
    // SAFETY: `name` is writable for the entire length passed to gethostname.
    if unsafe { libc::gethostname(name.as_mut_ptr().cast(), name.len()) } != 0 {
        return None;
    }
    let end = name.iter().position(|&byte| byte == 0)?;
    (end != 0).then(|| host_hash(&name[..end]))
}

#[cfg(not(unix))]
fn host() -> Option<String> {
    None
}

/// 16-hex-character tag for the current host used in backup staging names, if available.
pub fn host_tag() -> Option<String> {
    host()
}

#[cfg(target_os = "linux")]
fn legacy_host() -> Option<String> {
    let name = fs::read_to_string("/etc/hostname").ok()?;
    let name = name.trim();
    (!name.is_empty()).then(|| host_hash(name.as_bytes()))
}

#[cfg(unix)]
fn pid_is_dead_with(pid: u32, probe: impl FnOnce(libc::pid_t) -> std::io::Result<()>) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    pid > 0 && probe(pid).is_err_and(|err| err.raw_os_error() == Some(libc::ESRCH))
}

#[cfg(unix)]
fn pid_is_dead(pid: u32) -> bool {
    pid_is_dead_with(pid, |pid| {
        // SAFETY: `pid` is positive and signal 0 checks liveness without sending a signal.
        if unsafe { libc::kill(pid, 0) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    })
}

#[cfg(not(unix))]
fn pid_is_dead(_pid: u32) -> bool {
    false
}

#[cfg(unix)]
fn same_owner(entry: &fs::Metadata, directory: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    entry.uid() == directory.uid()
}

pub(super) fn prefix() -> String {
    format!(
        ".fb-backup.{}.{}.",
        host().unwrap_or_else(|| "unknown".into()),
        std::process::id()
    )
}
pub fn cleanup_stale(directory: &Path) -> Result<Vec<PathBuf>> {
    cleanup_stale_with(directory, pid_is_dead)
}

fn cleanup_stale_with(directory: &Path, is_dead: impl Fn(u32) -> bool) -> Result<Vec<PathBuf>> {
    paths::resolve(directory)?;
    let Some(host) = host() else {
        return Ok(Vec::new());
    };
    if !directory.try_exists()? {
        return Ok(Vec::new());
    }
    #[cfg(target_os = "linux")]
    let legacy_prefix = legacy_host()
        .filter(|legacy| *legacy != host)
        .map(|legacy| format!(".fb-backup.{legacy}."));
    let prefix = format!(".fb-backup.{host}.");
    let mut removed = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let tail = name.strip_prefix(&prefix);
        #[cfg(target_os = "linux")]
        let tail = tail.or_else(|| name.strip_prefix(legacy_prefix.as_deref()?));
        let Some(tail) = tail else {
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
        if !is_dead(pid) {
            continue;
        }
        let kind = entry.file_type()?;
        if !kind.is_file() && !kind.is_dir() {
            continue;
        }
        #[cfg(unix)]
        if !same_owner(&entry.metadata()?, &fs::metadata(directory)?) {
            continue;
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
    clear_interrupted_restore_with(folder, pid_is_dead)
}

fn clear_interrupted_restore_with(folder: &Path, is_dead: impl Fn(u32) -> bool) -> Result<bool> {
    let Ok(meta) = fs::symlink_metadata(folder) else {
        return Ok(false);
    };
    if !meta.is_dir() {
        return Ok(false);
    }
    let removed = cleanup_stale_with(folder, is_dead)?;
    let mut rest = Vec::new();
    for entry in fs::read_dir(folder)? {
        rest.push(entry?);
    }
    match rest.as_slice() {
        [] => {}
        [only]
            if !removed.is_empty()
                && only.file_name() == "receipts"
                && only.file_type()?.is_dir() =>
        {
            #[cfg(unix)]
            if !same_owner(&only.metadata()?, &meta) {
                return Ok(false);
            }
            fs::remove_dir_all(only.path())?;
        }
        _ => return Ok(false),
    }
    fs::remove_dir(folder)?;
    Ok(true)
}

#[cfg(all(test, unix))]
mod interrupted_restore_tests {
    use super::*;
    use std::{
        io::Error,
        os::unix::fs::{MetadataExt, chown, symlink},
    };

    fn partial_path(dir: &Path, host: &str, pid: u32) -> PathBuf {
        dir.join(format!(".fb-backup.{host}.{pid}.AbC123xyz.partial"))
    }

    fn residue(dir: &Path, pid: u32) -> PathBuf {
        let p = partial_path(dir, &host().unwrap(), pid);
        fs::write(&p, b"half a book").unwrap();
        p
    }

    fn dead_probe(pid: u32) -> bool {
        pid_is_dead_with(pid, |_| Err(Error::from_raw_os_error(libc::ESRCH)))
    }

    #[test]
    fn only_esrch_is_dead_and_only_positive_representable_pids_are_probed() {
        assert!(pid_is_dead_with(123, |pid| {
            assert_eq!(pid, 123);
            Err(Error::from_raw_os_error(libc::ESRCH))
        }));
        assert!(!pid_is_dead_with(123, |_| Ok(())));
        for error in [libc::EPERM, libc::EINVAL, libc::EINTR] {
            assert!(!pid_is_dead_with(123, |_| Err(Error::from_raw_os_error(
                error
            ))));
        }
        for pid in [0, i32::MAX as u32 + 1, u32::MAX] {
            assert!(!pid_is_dead_with(pid, |_| panic!(
                "unsafe PID was probed: {pid}"
            )));
        }
    }

    #[test]
    fn stale_cleanup_preserves_unconfirmed_pids_foreign_hosts_and_symlinks() {
        let d = tempfile::tempdir().unwrap();
        let live = residue(d.path(), std::process::id());
        let overflow = [i32::MAX as u32 + 1, u32::MAX].map(|pid| residue(d.path(), pid));
        let foreign = partial_path(d.path(), "other", 126);
        let unknown = partial_path(d.path(), "unknown", 126);
        fs::write(&foreign, b"keep").unwrap();
        fs::write(&unknown, b"keep").unwrap();
        let link = partial_path(d.path(), &host().unwrap(), 124);
        symlink(&foreign, &link).unwrap();
        assert!(cleanup_stale(d.path()).unwrap().is_empty());
        assert!(live.is_file());

        let denied = residue(d.path(), 125);
        let dead = residue(d.path(), 126);
        let removed = cleanup_stale_with(d.path(), |pid| {
            pid_is_dead_with(pid, |pid| match pid {
                125 => Err(Error::from_raw_os_error(libc::EPERM)),
                124 | 126 => Err(Error::from_raw_os_error(libc::ESRCH)),
                _ => Ok(()),
            })
        })
        .unwrap();
        assert_eq!(removed, vec![dead.clone()]);
        assert!(!dead.exists());
        for kept in [
            &live,
            &denied,
            &foreign,
            &unknown,
            &overflow[0],
            &overflow[1],
        ] {
            assert!(kept.is_file(), "{}", kept.display());
        }
        assert_eq!(fs::read_link(&link).unwrap(), foreign);
    }

    #[test]
    fn owner_check_rejects_different_uids() {
        let d = tempfile::tempdir().unwrap();
        let directory = fs::metadata(d.path()).unwrap();
        let root = fs::metadata("/").unwrap();
        if directory.uid() == root.uid() {
            let entry = residue(d.path(), 123);
            chown(&entry, Some(directory.uid() + 1), None).unwrap();
            assert!(cleanup_stale_with(d.path(), dead_probe).unwrap().is_empty());
            assert!(entry.is_file());
        } else {
            // A regular test user cannot change file owners. Use real metadata from another owner.
            assert!(!same_owner(&root, &directory));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn stale_cleanup_recognizes_the_legacy_linux_hostname() {
        let Some(legacy) = legacy_host() else { return };
        let d = tempfile::tempdir().unwrap();
        let partial = partial_path(d.path(), &legacy, 123);
        fs::create_dir(&partial).unwrap();
        fs::write(partial.join("staged"), b"keep until confirmed dead").unwrap();
        assert_eq!(
            cleanup_stale_with(d.path(), dead_probe).unwrap(),
            vec![partial.clone()]
        );
        assert!(!partial.exists());
    }

    #[test]
    fn dead_pid_residue_is_cleared_and_anything_else_is_kept() {
        let d = tempfile::tempdir().unwrap();
        // Dead pid partial plus a half-written receipts folder: residue, removed.
        let a = d.path().join("restored-a");
        fs::create_dir_all(a.join("receipts")).unwrap();
        fs::write(a.join("receipts").join("x.bin"), b"r").unwrap();
        residue(&a, 123);
        assert!(clear_interrupted_restore_with(&a, dead_probe).unwrap());
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
        let dead = residue(&c, 123);
        assert!(!clear_interrupted_restore_with(&c, dead_probe).unwrap());
        assert!(
            c.join("restored-c.fbbook").exists() && c.join("receipts").exists() && !dead.exists()
        );
        // receipts/ without any dead partial is not proof of a crash: kept.
        let r = d.path().join("restored-r");
        fs::create_dir_all(r.join("receipts")).unwrap();
        assert!(!clear_interrupted_restore(&r).unwrap());
        assert!(r.join("receipts").exists());
        // Not a folder, or missing: nothing to do.
        fs::write(d.path().join("file"), b"x").unwrap();
        assert!(!clear_interrupted_restore(&d.path().join("file")).unwrap());
        assert!(!clear_interrupted_restore(&d.path().join("missing")).unwrap());
        let target = d.path().join("elsewhere");
        fs::create_dir(&target).unwrap();
        symlink(&target, d.path().join("restored-link")).unwrap();
        assert!(!clear_interrupted_restore(&d.path().join("restored-link")).unwrap());
        assert!(target.exists());

        // A receipts symlink keeps the restore folder even after a dead partial was removed.
        let linked_receipts = d.path().join("restored-receipts-link");
        fs::create_dir(&linked_receipts).unwrap();
        symlink(&target, linked_receipts.join("receipts")).unwrap();
        let dead = residue(&linked_receipts, 123);
        assert!(!clear_interrupted_restore_with(&linked_receipts, dead_probe).unwrap());
        assert!(!dead.exists());
        assert_eq!(
            fs::read_link(linked_receipts.join("receipts")).unwrap(),
            target
        );
    }
}
