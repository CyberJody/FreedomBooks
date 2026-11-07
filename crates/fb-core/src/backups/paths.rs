use super::{BackupError, Result};
use std::{
    fs::{self, File, OpenOptions},
    path::{Component, Path, PathBuf},
};

/// Fail closed on symlinks (including dangling links/loops) and ambiguous parents.
pub fn resolve(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut out = PathBuf::new();
    for part in absolute.components() {
        if matches!(part, Component::ParentDir) {
            return Err(BackupError::refused(
                "Use a plain path without parent traversal.",
            ));
        }
        out.push(part);
        match fs::symlink_metadata(&out) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(BackupError::refused(
                    "Could not safely resolve path: symbolic links are refused.",
                ));
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(out)
}
pub fn overlap(a: &Path, b: &Path) -> Result<bool> {
    let a = resolve(a)?;
    let b = resolve(b)?;
    Ok(a.starts_with(&b) || b.starts_with(&a))
}
pub fn private_dir(path: &Path) -> Result<PathBuf> {
    let path = resolve(path)?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(&path) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e.into()),
    }
    let meta = fs::symlink_metadata(&path)?;
    if !meta.is_dir() {
        return Err(BackupError::refused("A private work folder is required."));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // Metadata from a newly created private file is an unprivileged uid oracle.
        let probe = tempfile::NamedTempFile::new_in(&path)?;
        if meta.uid() != probe.as_file().metadata()?.uid() {
            return Err(BackupError::refused("Work folder belongs to another user."));
        }
        if meta.mode() & 0o022 != 0 {
            return Err(BackupError::refused(
                "A shared folder cannot hold private work files.",
            ));
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(path)
}
pub fn new_file(path: &Path) -> Result<File> {
    resolve(path)?;
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    Ok(opts.open(path)?)
}
pub fn regular(path: &Path) -> Result<File> {
    resolve(path)?;
    #[cfg(unix)]
    let f = {
        use rustix::fs::{Mode, OFlags, open, openat};
        let path = resolve(path)?;
        let parts: Vec<_> = path
            .components()
            .filter_map(|c| {
                if let Component::Normal(p) = c {
                    Some(p)
                } else {
                    None
                }
            })
            .collect();
        let mut directory = File::from(open(
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        for (i, part) in parts.iter().enumerate() {
            let last = i + 1 == parts.len();
            let flags = OFlags::RDONLY
                | OFlags::NOFOLLOW
                | OFlags::CLOEXEC
                | OFlags::NONBLOCK
                | if last {
                    OFlags::empty()
                } else {
                    OFlags::DIRECTORY
                };
            directory = File::from(openat(&directory, *part, flags, Mode::empty())?);
        }
        directory
    };
    #[cfg(not(unix))]
    let f = File::open(path)?;
    if !f.metadata()?.is_file() {
        return Err(BackupError::refused("Expected a regular file."));
    }
    Ok(f)
}
pub fn lock_file(path: &Path) -> Result<File> {
    resolve(path)?;
    #[cfg(unix)]
    let f = File::from(rustix::fs::open(
        path,
        rustix::fs::OFlags::RDWR
            | rustix::fs::OFlags::CREATE
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::from_bits_truncate(0o600),
    )?);
    #[cfg(not(unix))]
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    if !f.metadata()?.is_file() {
        return Err(BackupError::refused(
            "The backup lock is not a regular file.",
        ));
    }
    Ok(f)
}
pub fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}
pub fn require_space(path: &Path, bytes: u64) -> Result<()> {
    let mut parent = resolve(path)?;
    while !parent.exists() {
        if !parent.pop() {
            return Err(BackupError::refused("No writable parent folder."));
        }
    }
    if !parent.is_dir() {
        parent.pop();
    }
    if fs2::available_space(&parent)? < bytes {
        return Err(BackupError::refused(
            "Not enough free space to safely complete this operation.",
        ));
    }
    Ok(())
}
pub fn writable_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| BackupError::refused("A target folder is required."))?;
    if !parent.is_dir() {
        return Err(BackupError::refused(
            "Create the target parent folder first.",
        ));
    }
    let probe = tempfile::NamedTempFile::new_in(parent)?;
    probe.as_file().sync_all()?;
    Ok(())
}

pub fn new_receipts_dir(path: &Path) -> Result<()> {
    resolve(path)?;
    if !path.try_exists()? {
        return Ok(());
    }
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() || fs::read_dir(path)?.next().is_some() {
        return Err(BackupError::refused(
            "Restore receipts require a NEW missing or empty folder.",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != rustix::process::getuid().as_raw() {
            return Err(BackupError::refused(
                "Restore receipts folder belongs to another user.",
            ));
        }
    }
    let probe = tempfile::NamedTempFile::new_in(path)?;
    probe.as_file().sync_all()?;
    Ok(())
}
pub fn refuse_sidecars(path: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut side = path.as_os_str().to_owned();
        side.push(suffix);
        match fs::symlink_metadata(PathBuf::from(side)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            _ => {
                return Err(BackupError::refused(
                    "Restore target has existing SQLite sidecars. Give a NEW target.",
                ));
            }
        }
    }
    Ok(())
}
