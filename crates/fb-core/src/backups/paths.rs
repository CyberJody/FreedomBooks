use super::{BackupError, Result};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
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
#[cfg(unix)]
fn directory_handle(path: &Path) -> Result<File> {
    use rustix::fs::{Mode, OFlags, open, openat};
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut dir = File::from(open("/", flags, Mode::empty())?);
    for component in path.components() {
        if let Component::Normal(part) = component {
            dir = File::from(openat(&dir, part, flags, Mode::empty())?);
        }
    }
    Ok(dir)
}

pub fn regular(path: &Path) -> Result<File> {
    let path = resolve(path)?;
    #[cfg(unix)]
    let f = {
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return Err(BackupError::refused("Expected a regular file."));
        };
        open_read_at(&directory_handle(parent)?, name)?
    };
    #[cfg(not(unix))]
    let f = File::open(path)?;
    if !f.metadata()?.is_file() {
        return Err(BackupError::refused("Expected a regular file."));
    }
    Ok(f)
}

#[cfg(unix)]
fn open_read_at(dir: &File, name: &std::ffi::OsStr) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags, openat};
    Ok(File::from(
        openat(
            dir,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(io::Error::from)?,
    ))
}

/// A symlink-free directory whose file operations are relative to its pinned
/// descriptor on Unix. Elsewhere each operation revalidates the directory path.
pub struct Directory {
    #[cfg(unix)]
    fd: File,
    #[cfg(not(unix))]
    path: PathBuf,
}

impl Directory {
    pub fn open(path: &Path) -> Result<Self> {
        let path = resolve(path)?;
        #[cfg(unix)]
        return Ok(Self {
            fd: directory_handle(&path)?,
        });
        #[cfg(not(unix))]
        if fs::symlink_metadata(&path)?.is_dir() {
            Ok(Self { path })
        } else {
            Err(BackupError::refused("Expected a directory."))
        }
    }

    fn filename(name: &str) -> Result<&std::ffi::OsStr> {
        let name = Path::new(name);
        match (name.components().next(), name.file_name()) {
            (Some(Component::Normal(part)), Some(file)) if part == name.as_os_str() => Ok(file),
            _ => Err(BackupError::refused("Expected a filename.")),
        }
    }

    pub fn read_file(&self, name: &str) -> Result<Option<File>> {
        let name = Self::filename(name)?;
        #[cfg(unix)]
        let file = open_read_at(&self.fd, name);
        #[cfg(not(unix))]
        let file = {
            let path = resolve(&self.path.join(name))?;
            match fs::symlink_metadata(&path) {
                Ok(meta) if !meta.is_file() => {
                    return Err(BackupError::refused("Expected a regular file."));
                }
                Ok(_) => File::open(path),
                Err(e) => Err(e),
            }
        };
        match file {
            Ok(f) if f.metadata()?.is_file() => Ok(Some(f)),
            Ok(_) => Err(BackupError::refused("Expected a regular file.")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn replace_private_file(&self, name: &str, bytes: &[u8]) -> Result<()> {
        let name = Self::filename(name)?;
        #[cfg(unix)]
        {
            use rustix::fs::{AtFlags, FileType, Mode, OFlags, openat, renameat, statat, unlinkat};
            match statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile => (),
                Ok(_) => return Err(BackupError::refused("Expected a regular file.")),
                Err(rustix::io::Errno::NOENT) => (),
                Err(e) => return Err(e.into()),
            }
            let temp = format!(".tmp-{}", fb_ids::new_v7());
            let mut file = File::from(openat(
                &self.fd,
                temp.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
                Mode::from_bits_truncate(0o600),
            )?);
            let result = (|| {
                file.write_all(bytes)?;
                file.sync_all()?;
                renameat(&self.fd, temp.as_str(), &self.fd, name)?;
                self.fd.sync_all()?;
                Ok(())
            })();
            if result.is_err() {
                let _ = unlinkat(&self.fd, temp.as_str(), AtFlags::empty());
            }
            result
        }
        #[cfg(not(unix))]
        {
            let path = resolve(&self.path.join(name))?;
            match fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_file() => (),
                Ok(_) => return Err(BackupError::refused("Expected a regular file.")),
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
            let mut temp = tempfile::NamedTempFile::new_in(&self.path)?;
            temp.write_all(bytes)?;
            temp.as_file().sync_all()?;
            temp.persist(path).map_err(|e| e.error)?;
            Ok(())
        }
    }

    pub(super) fn create_private_subdir(&self, prefix: &str) -> Result<(String, Self)> {
        let name = format!("{prefix}{}", fb_ids::new_v7());
        let file = Self::filename(&name)?;
        #[cfg(unix)]
        {
            use rustix::fs::{Mode, OFlags, mkdirat, openat};
            mkdirat(&self.fd, file, Mode::from_bits_truncate(0o700))?;
            let fd = openat(
                &self.fd,
                file,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )?;
            self.sync()?;
            Ok((name, Self { fd: File::from(fd) }))
        }
        #[cfg(not(unix))]
        {
            let path = resolve(&self.path.join(file))?;
            fs::create_dir(&path)?;
            Ok((name, Self { path }))
        }
    }

    pub(super) fn rename_to(&self, from: &str, to_dir: &Self, to: &str) -> io::Result<()> {
        let from = Self::filename(from).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
        let to = Self::filename(to).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
        #[cfg(unix)]
        {
            rustix::fs::renameat(&self.fd, from, &to_dir.fd, to).map_err(io::Error::from)
        }
        #[cfg(not(unix))]
        {
            fs::rename(self.path.join(from), to_dir.path.join(to))
        }
    }

    pub(super) fn link_to(&self, from: &str, to_dir: &Self, to: &str) -> io::Result<()> {
        let from = Self::filename(from).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
        let to = Self::filename(to).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
        #[cfg(unix)]
        {
            rustix::fs::linkat(&self.fd, from, &to_dir.fd, to, rustix::fs::AtFlags::empty())
                .map_err(io::Error::from)
        }
        #[cfg(not(unix))]
        {
            fs::hard_link(self.path.join(from), to_dir.path.join(to))
        }
    }

    pub(super) fn unlink_file(&self, name: &str) -> io::Result<()> {
        let name = Self::filename(name).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
        #[cfg(unix)]
        {
            rustix::fs::unlinkat(&self.fd, name, rustix::fs::AtFlags::empty()).map_err(io::Error::from)
        }
        #[cfg(not(unix))]
        {
            fs::remove_file(self.path.join(name))
        }
    }

    pub(super) fn remove_empty_subdir(&self, name: &str) -> Result<()> {
        let name = Self::filename(name)?;
        #[cfg(unix)]
        {
            match rustix::fs::unlinkat(&self.fd, name, rustix::fs::AtFlags::REMOVEDIR) {
                Ok(()) => Ok(()),
                Err(rustix::io::Errno::NOTEMPTY | rustix::io::Errno::EXIST | rustix::io::Errno::NOENT) => Ok(()),
                Err(e) => Err(e.into()),
            }
        }
        #[cfg(not(unix))]
        {
            let path = self.path.join(name);
            if fs::read_dir(&path)?.next().is_none() {
                fs::remove_dir(&path)?;
            }
            Ok(())
        }
    }

    pub(super) fn sync(&self) -> Result<()> {
        #[cfg(unix)]
        {
            self.fd.sync_all()?;
        }
        Ok(())
    }
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
pub(super) fn temp_directory(root: &Path, prefix: &str) -> Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(prefix);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    Ok(builder.tempdir_in(root)?)
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
        return Err(BackupError::retryable(
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
