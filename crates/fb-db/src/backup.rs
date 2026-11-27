//! Consistent encrypted snapshots. No callback receives the raw connection or
//! ATTACH/PRAGMA privileges. Manifest readers open the completed snapshot.
use super::*;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::PathBuf;

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

fn new_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

pub(super) fn require_owned(path: &Path, file: &File) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_file()
        && same_file::Handle::from_file(file.try_clone()?)? == same_file::Handle::from_path(path)? {
        Ok(())
    } else {
        Err(io::Error::other("The destination file changed. It was kept for recovery."))
    }
}

fn sqlite_sidecars(path: &Path, remove: bool) -> io::Result<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let side = sidecar(path, suffix);
        match fs::symlink_metadata(&side) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
            Ok(meta) if remove && meta.is_file() => fs::remove_file(side)?,
            Ok(_) => return Err(io::Error::other("Snapshot target has existing SQLite sidecars")),
        }
    }
    Ok(())
}

/// Only SQLite can create journals for these new books while the caller holds the inode
/// lock. Clean them immediately after connection teardown, before releasing that lock.
fn with_new_sqlite_files(path: &Path, file: &File, operation: impl FnOnce() -> Result<(), DbError>) -> Result<(), DbError> {
    require_owned(path, file).map_err(DbError::Io)?;
    sqlite_sidecars(path, false).map_err(DbError::Io)?;
    let result = operation().and_then(|()| sqlite_sidecars(path, false).map_err(DbError::Io));
    if let Err(error) = result {
        require_owned(path, file).and_then(|()| sqlite_sidecars(path, true))
            .map_err(|cleanup| DbError::Pragma(format!("{error}. SQLite cleanup needs attention: {cleanup}")))?;
        return Err(error);
    }
    Ok(())
}

/// Reserve the new book's pathname and inode across publication, reopen and rollback.
/// Other FreedomBooks openers cannot acquire either lock during this operation.
pub struct RestoreTarget {
    guard: Arc<ProcessBookGuard>,
    lock_path: PathBuf,
    created_lock: bool,
}

impl RestoreTarget {
    pub fn reserve(path: &Path, staged: File) -> Result<Self, DbError> {
        let lock_path = sidecar(path, ".lock");
        let (lock, created_lock) = open_lock_sidecar(&lock_path).map_err(DbError::Io)?;
        lock.try_lock().map_err(|e| DbError::Pragma(format!("Restore target is busy: {e}")))?;
        require_owned(&lock_path, &lock).map_err(DbError::Io)?;
        if lock.metadata().map_err(DbError::Io)?.len() != 0 {
            return Err(DbError::Pragma("Existing book-lock record requires the matching book-lock version".into()));
        }
        let inode = match lock_book_file(&staged, path) {
            Ok(inode) => inode,
            Err(error) => {
                if created_lock {
                    require_owned(&lock_path, &lock).and_then(|()| fs::remove_file(&lock_path))
                        .map_err(|cleanup| DbError::Pragma(format!("{error}. Restore cleanup needs attention: {cleanup}")))?;
                }
                return Err(error);
            }
        };
        Ok(Self {
            guard: Arc::new(ProcessBookGuard { _sidecar: lock, _inode: inode, book: staged,
                path: path.to_owned(), admission: Default::default(), maintenance: true.into() }),
            lock_path, created_lock,
        })
    }

    pub fn check(&self, key: &Key) -> Result<(), DbError> {
        let path = &self.guard.path;
        require_owned(path, &self.guard.book).map_err(DbError::Io)?;
        require_owned(&self.lock_path, &self.guard._sidecar).map_err(DbError::Io)?;
        self.guard.ensure_single_link()?;
        check_recovery_marker(path)?;
        with_new_sqlite_files(path, &self.guard.book, || {
            Book::open_inner(path, key, false, self.guard.clone()).map(drop)
        })
    }

    pub fn rollback(self, published: bool) -> Result<(), DbError> {
        require_owned(&self.lock_path, &self.guard._sidecar).map_err(DbError::Io)?;
        if published {
            require_owned(&self.guard.path, &self.guard.book).map_err(DbError::Io)?;
            fs::remove_file(&self.guard.path).map_err(DbError::Io)?;
        }
        self.guard._inode.remove_owned().map_err(DbError::Io)?;
        if self.created_lock { fs::remove_file(self.lock_path).map_err(DbError::Io)?; }
        Ok(())
    }
}

impl Book {
    /// Purge obsolete live-file pages after replacing legacy credential envelopes.
    pub fn compact_secret_pages(&mut self) -> Result<(), DbError> {
        if !self.conn.is_autocommit() { return Err(DbError::Pragma("Compaction needs an idle book".into())); }
        self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    /// Change the SQLCipher key in place; caller owns the verified rollback backup and key journal.
    pub fn rekey(&mut self, key: &Key) -> Result<(), DbError> {
        if !self.conn.is_autocommit() { return Err(DbError::Pragma("Rekey needs an idle book".into())); }
        self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE; VACUUM;")?;
        #[cfg(feature = "key-test-hooks")]
        if std::env::var("FB_KEY_STOP_AT").ok().as_deref() == Some("rekey-in-progress")
            && let Ok(marker) = std::env::var("FB_KEY_STOP_MARKER") {
            std::fs::write(marker, "started").expect("test marker");
        }
        self.conn.execute_batch(&format!("PRAGMA rekey=\"x'{}'\"", hex(key)))?;
        #[cfg(feature = "key-test-hooks")]
        if std::env::var("FB_KEY_STOP_AT").ok().as_deref() == Some("rekey-in-progress")
            && let Ok(marker) = std::env::var("FB_KEY_STOP_MARKER") {
            std::fs::write(marker, "finished").expect("test marker");
        }
        let quick: String = self.conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if quick != "ok" { return Err(DbError::Integrity(vec!["Rekey check failed".into()])); }
        Self::configure(&self.conn)?;
        Ok(())
    }

    /// Inspect a staged snapshot BEFORE any migration. Exact prior schema hashes
    /// are authenticated; callbacks have the same read-only boundary as Book::read.
    /// Never use this on the live file: SQLCipher may create WAL sidecars.
    pub fn inspect_snapshot<T, E>(
        path: &Path,
        key: &Key,
        f: impl FnOnce(&Reader<'_>) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<rusqlite::Error> + From<DbError>,
    {
        Self::inspect_snapshot_with_history(path, key, |q, _| f(q))
    }

    pub fn inspect_snapshot_with_history<T, E>(
        path: &Path, key: &Key,
        f: impl FnOnce(&Reader<'_>, &[(&'static str, &'static str)]) -> Result<T, E>,
    ) -> Result<T, E>
    where E: From<rusqlite::Error> + From<DbError>,
    {
        let mut conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.execute_batch(&format!("PRAGMA key=\"x'{}'\"", hex(key)))?;
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|_| E::from(DbError::WrongKeyOrNotABook))?;
        if version <= 0 || version > MIGRATIONS.len() as i64 {
            return Err(DbError::TooNew {
                found: version,
                known: MIGRATIONS.len() as i64,
            }
            .into());
        }
        Self::configure(&conn).map_err(E::from)?;
        let installed = authenticated_migrations(&conn, version as usize).map_err(E::from)?;
        let quick: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        let foreign: i64 =
            conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })?;
        if quick != "ok" || foreign != 0 {
            return Err(DbError::Integrity(vec!["Invalid snapshot".into()]).into());
        }
        let callback_mode = Arc::new(AtomicU8::new(0));
        let mode = callback_mode.clone();
        conn.authorizer(Some(move |ctx: AuthContext<'_>| {
            authorize(ctx, mode.load(Ordering::Relaxed))
        }))?;
        let owner_empty_command = register_owner_command(&conn)?;
        let company_period_command = company_periods::register(&conn)?;
        let mut book = Book {
            conn,
            callback_mode,
            owner_empty_command,
            company_period_command,
            pending_credential_masks: Default::default(),
            retained_text_guards: Default::default(),
            commits: 0,
            connection: crate::next_connection(),
            retained_text_state: Default::default(),
        };
        book.read(|q| f(q, &installed))
    }

    /// Upper bound for snapshot preflight, from SQLite page counts (no SQL money math).
    pub fn snapshot_size(&self) -> Result<u64, DbError> {
        let pages: i64 = self.conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let size: i64 = self
            .conn
            .query_row("PRAGMA page_size", [], |r| match r.get_ref(0)? {
                rusqlite::types::ValueRef::Integer(n) => Ok(n),
                rusqlite::types::ValueRef::Text(s) => std::str::from_utf8(s)
                    .ok()
                    .and_then(|s| s.parse::<i64>().ok())
                    .ok_or(rusqlite::Error::InvalidQuery),
                _ => Err(rusqlite::Error::InvalidQuery),
            })?;
        pages
            .checked_mul(size)
            .and_then(|v| u64::try_from(v).ok())
            .ok_or_else(|| DbError::Pragma("Snapshot size overflow".into()))
    }

    /// Write a NEW SQLCipher file using SQLite's online backup API. The caller
    /// supplies the destination key; no master key is stored in a manifest.
    pub fn snapshot(&self, path: &Path, key: &Key) -> Result<(), DbError> {
        let problems = self.check()?;
        if !problems.is_empty() {
            return Err(DbError::Integrity(problems));
        }
        self.write_snapshot(path, key)
    }

    /// The copy itself; the caller has run the checks for the book's schema version.
    pub(crate) fn write_snapshot(&self, path: &Path, key: &Key) -> Result<(), DbError> {
        write_new_snapshot(path, || {
            let mut dest = Connection::open(path)?;
            dest.execute_batch(&format!(
                "PRAGMA key=\"x'{}'\";PRAGMA synchronous=FULL",
                hex(key)
            ))?;
            let backup = rusqlite::backup::Backup::new(&self.conn, &mut dest)?;
            // Test builds only: stop part-way through the pre-change copy, while the destination's rollback journal
            // exists, so a SIGKILL there leaves the snapshot and its `-journal` behind.
            #[cfg(feature = "key-test-hooks")]
            if std::env::var("FB_KEY_STOP_AT").ok().as_deref() == Some("snapshot-in-progress")
                && path.as_os_str().as_encoded_bytes().ends_with(b".rekey-before")
                && let Ok(marker) = std::env::var("FB_KEY_STOP_MARKER") {
                backup.step(1)?;
                std::fs::write(marker, "snapshot-in-progress").expect("test marker");
                loop {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            backup.run_to_completion(256, Duration::from_millis(10), None)?;
            drop(backup);
            dest.close().map_err(|(_, e)| e)?;
            Ok(())
        })
    }
}

fn write_new_snapshot(path: &Path, copy: impl FnOnce() -> Result<(), DbError>) -> Result<(), DbError> {
    sqlite_sidecars(path, false).map_err(DbError::Io)?;
    let file = new_file(path)
        .map_err(|_| DbError::Pragma("Snapshot requires a NEW writable file".into()))?;
    let inode = lock_book_file(&file, path)?;
    let result = with_new_sqlite_files(path, &file, copy).and_then(|()| file.sync_all().map_err(DbError::Io));
    if let Err(error) = result {
        require_owned(path, &file).and_then(|()| fs::remove_file(path))
            .and_then(|()| inode.remove_owned()).map_err(|cleanup| {
            DbError::Pragma(format!("{error}. Snapshot cleanup needs attention: {cleanup}"))
        })?;
        return Err(error);
    }
    // The completed copy may be renamed, removed, or reserved for restore next.
    // Retire this writer's sidecar while locked, before handing the snapshot back.
    inode.remove_owned().map_err(DbError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_snapshot_leaves_its_directory_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("copy.fbbook");
        let error = write_new_snapshot(&path, || {
            fs::write(sidecar(&path, "-journal"), b"test").unwrap();
            Err(DbError::Pragma("copy failed".into()))
        }).unwrap_err();
        assert!(error.to_string().contains("copy failed"));
        assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn rollback_preserves_an_independent_inode_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("restored.fbbook");
        let staged = new_file(&path).unwrap();
        let independent = inode_lock_path(&staged, &path).unwrap();
        fs::write(&independent, b"independent").unwrap();
        RestoreTarget::reserve(&path, staged).unwrap().rollback(true).unwrap();
        assert_eq!(fs::read(&independent).unwrap(), b"independent");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_snapshot_removes_its_sqlite_sidecars_and_preserves_an_independent_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("copy.fbbook");
        fs::write(sidecar(&path, ".lock"), b"independent").unwrap();
        let error = write_new_snapshot(&path, || {
            for suffix in ["-wal", "-shm", "-journal"] {
                fs::write(sidecar(&path, suffix), b"test").unwrap();
            }
            Err(DbError::Pragma("failed snapshot".into()))
        }).unwrap_err();
        assert!(error.to_string().contains("failed snapshot"));
        assert_eq!(fs::read(sidecar(&path, ".lock")).unwrap(), b"independent");
        sqlite_sidecars(&path, false).unwrap();
        assert!(!path.exists());
        let book = Book::create(&dir.path().join("source"), &[1; 32]).unwrap();
        book.snapshot(&path, &[2; 32]).unwrap();
    }

    #[test]
    fn restore_rollback_cleans_owned_sidecars_and_preserves_preexisting_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("restored.fbbook");
        let source = tempfile::tempdir().unwrap();
        let book = Book::create(&source.path().join("source"), &[1; 32]).unwrap();
        for existing_lock in [false, true] {
            if existing_lock { new_file(&sidecar(&path, ".lock")).unwrap(); }
            book.snapshot(&path, &[2; 32]).unwrap();
            let staged = File::open(&path).unwrap();
            let reservation = RestoreTarget::reserve(&path, staged).unwrap();
            assert!(Book::open(&path, &[2; 32]).is_err());
            assert!(reservation.check(&[3; 32]).is_err());

            reservation.rollback(true).unwrap();
            assert!(!path.exists());
            sqlite_sidecars(&path, false).unwrap();
            assert_eq!(sidecar(&path, ".lock").exists(), existing_lock);
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), usize::from(existing_lock));
        }
        book.snapshot(&path, &[2; 32]).unwrap();
        let reservation = RestoreTarget::reserve(&path, File::open(&path).unwrap()).unwrap();
        fs::write(sidecar(&path, "-wal"), b"foreign").unwrap();
        assert!(reservation.check(&[2; 32]).is_err());
        reservation.rollback(true).unwrap();
        assert_eq!(fs::read(sidecar(&path, "-wal")).unwrap(), b"foreign");
    }
}
