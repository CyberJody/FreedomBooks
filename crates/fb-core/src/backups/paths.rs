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
        let dir = directory_handle(&path)?;
        let meta = dir.metadata()?;
        if meta.uid() != rustix::process::getuid().as_raw() {
            return Err(BackupError::refused("Work folder belongs to another user."));
        }
        if meta.mode() & 0o022 != 0 {
            return Err(BackupError::refused(
                "A shared folder cannot hold private work files.",
            ));
        }
        dir.set_permissions(fs::Permissions::from_mode(0o700))?;
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
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
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
        self.read_named(Self::filename(name)?)
    }

    pub(super) fn read_named(&self, name: &std::ffi::OsStr) -> Result<Option<File>> {
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
            #[cfg(unix)]
            Err(e) if e.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error()) =>
                Err(BackupError::refused("Symbolic links are refused.")),
            Err(e) => Err(e.into()),
        }
    }

    pub fn replace_private_file(&self, name: &str, bytes: &[u8]) -> Result<()> {
        self.write_private(Self::filename(name)?, false, |file| Ok(file.write_all(bytes)?))
    }

    pub(super) fn write_private<T>(&self, name: &std::ffi::OsStr, noclobber: bool,
        write: impl FnOnce(&mut File) -> Result<T>) -> Result<T> {
        #[cfg(unix)]
        {
            use rustix::fs::{AtFlags, FileType, Mode, OFlags, linkat, openat, renameat, statat, unlinkat};
            if !noclobber {
                match statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
                    Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile => (),
                    Ok(_) => return Err(BackupError::refused("Expected a regular file.")),
                    Err(rustix::io::Errno::NOENT) => (),
                    Err(e) => return Err(e.into()),
                }
            }
            let temp = format!("{}{}.partial", super::cleanup::prefix(), fb_ids::new_v7().simple());
            let mut file = File::from(openat(&self.fd, temp.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
                Mode::from_bits_truncate(0o600))?);
            let result = (|| {
                let value = write(&mut file)?;
                file.sync_all()?;
                if noclobber {
                    linkat(&self.fd, temp.as_str(), &self.fd, name, AtFlags::empty())
                        .map_err(io::Error::from).map_err(placement_error)?;
                    unlinkat(&self.fd, temp.as_str(), AtFlags::empty())?;
                } else {
                    renameat(&self.fd, temp.as_str(), &self.fd, name)?;
                }
                self.sync()?;
                Ok(value)
            })();
            if result.is_err() {
                match unlinkat(&self.fd, temp.as_str(), AtFlags::empty()) {
                    Ok(()) | Err(rustix::io::Errno::NOENT) => (),
                    Err(e) => return Err(e.into()),
                }
            }
            result
        }
        #[cfg(not(unix))]
        {
            let path = self.path.join(name);
            if !noclobber {
                resolve(&path)?;
                match fs::symlink_metadata(&path) {
                    Ok(meta) if meta.is_file() => (),
                    Ok(_) => return Err(BackupError::refused("Expected a regular file.")),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                    Err(e) => return Err(e.into()),
                }
            }
            let mut temp = tempfile::NamedTempFile::new_in(&self.path)?;
            let value = write(temp.as_file_mut())?;
            temp.as_file().sync_all()?;
            if noclobber { temp.persist_noclobber(path) } else { temp.persist(path) }
                .map_err(|e| placement_error(e.error))?;
            self.sync()?;
            Ok(value)
        }
    }

    /// Create a key record for a new book and retain its identity for rollback.
    pub fn create_private_file(&self, name: &str, bytes: &[u8]) -> Result<File> {
        self.write_private(Self::filename(name)?, true, |file| {
            file.write_all(bytes)?;
            Ok(file.try_clone()?)
        })
    }

    pub fn remove_created_file(&self, name: &str, owned: File) -> Result<()> {
        let Some(current) = self.read_file(name)? else { return Ok(()) };
        if same_file::Handle::from_file(owned)? != same_file::Handle::from_file(current)? {
            return Err(BackupError::retryable("The file changed during restore. It was kept for recovery."));
        }
        self.unlink_file(name)?;
        self.sync()
    }

    /// Publish a streamed export without exposing partial bytes at the selected name.
    pub fn copy_new(source: &mut impl io::Read, path: &Path) -> Result<u64> {
        let path = resolve(path)?;
        let (parent, name) = parent_name(&path)?;
        let dir = Self::open(parent)?;
        let bytes = dir.write_private(name, true, |out| Ok(io::copy(source, out)?))?;
        dir.confirm(parent)?;
        Ok(bytes)
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
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
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

    /// Recover a quarantined entry without overwriting a name another process has reused.
    pub(super) fn restore_to(&self, name: &str, dest: &Self) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        {
            use rustix::fs::{RenameFlags, renameat_with};
            match renameat_with(&self.fd, name, &dest.fd, name, RenameFlags::NOREPLACE) {
                Ok(()) => return Ok(()),
                Err(rustix::io::Errno::NOSYS | rustix::io::Errno::INVAL | rustix::io::Errno::OPNOTSUPP) => (),
                Err(e) => return Err(e.into()),
            }
        }
        self.link_to(name, dest, name)?;
        self.unlink_file(name)
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
                Err(rustix::io::Errno::NOENT) => Ok(()),
                Err(e) => Err(e.into()),
            }
        }
        #[cfg(not(unix))]
        {
            match fs::remove_dir(self.path.join(name)) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            }
        }
    }

    pub(super) fn make_private(&self) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            self.fd.set_permissions(fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    pub(super) fn sync(&self) -> Result<()> {
        #[cfg(unix)]
        {
            self.fd.sync_all()?;
        }
        Ok(())
    }
}
impl Directory {
    pub(super) fn lock_file(&self, name: &std::ffi::OsStr) -> Result<File> {
        #[cfg(unix)]
        let f = {
            use rustix::fs::{Mode, OFlags, openat};
            File::from(openat(&self.fd, name, OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW
                | OFlags::NONBLOCK | OFlags::CLOEXEC, Mode::from_bits_truncate(0o600))?)
        };
        #[cfg(not(unix))]
        let f = OpenOptions::new().read(true).write(true).create(true).truncate(false)
            .open(resolve(&self.path.join(name))?)?;
        if !f.metadata()?.is_file() {
            return Err(BackupError::refused("The backup lock is not a regular file."));
        }
        Ok(f)
    }

    pub fn confirm(&self, path: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let current = Self::open(path)?.fd.metadata()?;
            let pinned = self.fd.metadata()?;
            if (current.dev(), current.ino()) != (pinned.dev(), pinned.ino()) {
                return Err(BackupError::retryable("The backup folder changed during the operation."));
            }
        }
        #[cfg(not(unix))]
        { resolve(path)?; }
        Ok(())
    }

    /// Link into the pinned destination without overwriting. The caller syncs this descriptor.
    pub(super) fn link_new(&self, source: &Path, name: &std::ffi::OsStr) -> Result<()> {
        #[cfg(unix)]
        let linked = {
            let source = resolve(source)?;
            let (parent, file) = parent_name(&source)?;
            let source_dir = Self::open(parent)?;
            rustix::fs::linkat(&source_dir.fd, file, &self.fd, name, rustix::fs::AtFlags::empty())
                .map_err(io::Error::from)
        };
        #[cfg(not(unix))]
        let linked = fs::hard_link(resolve(source)?, resolve(&self.path.join(name))?);
        linked.map_err(|e| BackupError::refused(format!("Refusing to overwrite or non-atomic backup placement: {e}")))?;
        Ok(())
    }
}

fn placement_error(e: io::Error) -> BackupError {
    if e.kind() == io::ErrorKind::AlreadyExists {
        BackupError::conflict("A file with that name already exists there. Choose a new name.")
    } else { e.into() }
}
pub(super) fn parent_name(path: &Path) -> Result<(&Path, &std::ffi::OsStr)> {
    path.parent().zip(path.file_name()).ok_or_else(|| BackupError::refused("Missing file name."))
}
pub fn lock_file(path: &Path) -> Result<File> {
    let path = resolve(path)?;
    let (parent, name) = parent_name(&path)?;
    Directory::open(parent)?.lock_file(name)
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
    Directory::open(path)?.sync()
}
#[cfg(test)]
thread_local! { pub(super) static TEST_SPACE: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) }; }

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
    let available = fs2::available_space(&parent)?;
    #[cfg(test)]
    let available = TEST_SPACE.get().unwrap_or(available);
    if available < bytes {
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
