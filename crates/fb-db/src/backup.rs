//! Consistent encrypted snapshots. No callback receives the raw connection or
//! ATTACH/PRAGMA privileges. Manifest readers open the completed snapshot.
use super::*;
use std::fs::OpenOptions;

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
        let mut book = Book {
            conn,
            callback_mode,
            owner_empty_command,
            pending_credential_masks: Default::default(),
            retained_text_guards: Default::default(),
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
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut side = path.as_os_str().to_owned();
            side.push(suffix);
            match std::fs::symlink_metadata(std::path::PathBuf::from(side)) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                _ => {
                    return Err(DbError::Pragma(
                        "Snapshot target has existing SQLite sidecars".into(),
                    ));
                }
            }
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(path)
            .map_err(|_| DbError::Pragma("Snapshot requires a NEW writable file".into()))?;
        let result = (|| {
            let mut dest = Connection::open(path)?;
            dest.execute_batch(&format!(
                "PRAGMA key=\"x'{}'\";PRAGMA synchronous=FULL",
                hex(key)
            ))?;
            let backup = rusqlite::backup::Backup::new(&self.conn, &mut dest)?;
            backup.run_to_completion(256, Duration::from_millis(10), None)?;
            drop(backup);
            dest.close().map_err(|(_, e)| e)?;
            file.sync_all()
                .map_err(|_| DbError::Pragma("Could not sync snapshot".into()))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(path);
        }
        result
    }
}
