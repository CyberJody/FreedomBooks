//! Why a book did not open, and the automatic copy before an upgrade.
//!
//! Damaged versus wrong key is decided without the secret: `fb_db::check_file` runs before any
//! password or recovery key is tried, and a later failure counts as damage only once the key is
//! proven (by the key record's authenticated receipt seal, or by SQLCipher reading page 1). Every
//! wrong secret gets the same words and does the same work.
use crate::keys::MasterKey;
use crate::protection::Record;
use crate::session::{App, AppError, AppResult};
use fb_core::backups::portable::{BUILTIN_KEY, KeyEnvelope, WRONG_SECRET};
use fb_db::{Book, DbError, Opened, Unmigrated};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum OpenError {
    /// The book's schema is newer than this app's.
    TooNew { found: i64, known: i64 },
    /// The file fails a check, is cut short or is not a FreedomBooks book.
    Damaged,
    /// The password, recovery key or saved key does not open the books.
    WrongKey,
    /// The book needs an upgrade and its pre-upgrade copy could not be written or verified.
    UpgradeCopy { from: usize, to: usize, folder: String, reason: String },
    /// The file could not be reached (missing, no permission, disk error).
    Unavailable(String),
    /// The book has no key record beside it, yet holds attached files that its own key does not open:
    /// the record holding their key was left behind or lost when the book was copied or moved. Opening
    /// it would replace that record with one that can never open those files.
    KeyFileMissing { record: String, files: usize },
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::TooNew { found, known } => write!(f, "These books were saved by a newer version of FreedomBooks (book version {found}; this app reads up to {known}). Install the latest version to open them. Nothing was changed."),
            OpenError::Damaged => f.write_str("This file is damaged or isn't a FreedomBooks book. Nothing was changed. Restore it from a backup: open “Restore a backup” below and choose your most recent backup."),
            OpenError::WrongKey => f.write_str(WRONG_SECRET),
            OpenError::UpgradeCopy { from, to, folder, reason } => write!(f, "These books need a one-time update for this version of FreedomBooks (book version {from} to {to}). A checked backup copy is made before every update, and it could not be made: {}. Nothing was changed: the books were not updated or opened. Free some disk space in the backup folder ({folder}) and try again. To keep backups in another folder, open these books in the version of FreedomBooks that saved them and choose another folder on the Backups page.", reason.trim_end_matches('.')),
            OpenError::Unavailable(detail) => write!(f, "These books couldn't be opened ({detail}). Nothing was changed. Check that the file is still there and that you can write to its folder, then try again."),
            OpenError::KeyFileMissing { record, files } => {
                let (what, them) = if *files == 1 { ("1 attached file".to_owned(), "it") } else { (format!("{files} attached files"), "them") };
                write!(f, "These books' key file is missing. It is called “{record}” and belongs in the same folder as the book file. Without it, {what} in these books cannot be opened, so the books were not opened and nothing was changed. If you copied or moved these books, copy that key file from where they came from into the same folder, then open them again. If you no longer have it, choose “Restore a backup” and pick your most recent backup: your backups carry the key, and the restored books will open {them}.")
            }
        }
    }
}

/// `key_proven`: the key record already authenticated this key, so a first page that will not
/// decrypt is damage, not a wrong key.
pub(crate) fn open_error(e: DbError, key_proven: bool) -> AppError {
    AppError::Open(match e {
        DbError::TooNew { found, known } => OpenError::TooNew { found, known },
        DbError::WrongKeyOrNotABook if !key_proven => OpenError::WrongKey,
        DbError::WrongKeyOrNotABook => OpenError::Damaged,
        e if e.is_damage() => OpenError::Damaged,
        e => OpenError::Unavailable(e.to_string()),
    })
}

/// The key-free file checks, as the unlock screen words them.
pub fn check_book_file(path: &Path) -> AppResult<()> {
    fb_db::check_file(path).map_err(|e| open_error(e, false))
}

impl App {
    /// Unlock from the gate with a password or recovery key (a built-in-key book needs neither).
    pub fn open_with_secret(&mut self, path: &Path, secret: &str) -> AppResult<()> {
        // An interrupted move may have set the key record aside. Put it back before deciding
        // whether this book needs a secret or uses the built-in key.
        self.prepare_book_open()?;
        check_book_file(path)?;
        let record = Record::read(path)?;
        let key = match &record {
            Some(r) => r.keys.unlock(secret).map_err(|e| {
                if e.to_string() == WRONG_SECRET { AppError::Open(OpenError::WrongKey) } else { crate::protection::error(e) }
            })?,
            None => crate::protection::builtin_key(),
        };
        self.open_book(path, &key)?;
        if record.is_none() && let Err(e) = self.adopt_builtin_keys() {
            self.lock();
            return Err(e);
        }
        // Existing books show their key only through the password-gated recovery action.
        self.recovery_authorized = false;
        Ok(())
    }

    /// Open the book at `path`; an older book is copied to a verified backup before it is upgraded.
    /// Returns the book and, after an upgrade, the notice to show once.
    pub(crate) fn open_and_upgrade(&self, path: &Path, key: &MasterKey, record: Option<&Record>) -> AppResult<(Book, Option<String>)> {
        check_book_file(path)?;
        let proven = match record {
            Some(r) => {
                r.keys.receipts(key).map_err(|_| AppError::Open(OpenError::WrongKey))?;
                true
            }
            None => false,
        };
        let mut old = match Book::open_staged(path, key).map_err(|e| open_error(e, proven))? {
            Opened::Current(mut book) => {
                if record.is_none() { refuse_lost_key_record(&mut book, path, key)?; }
                return Ok((book, None));
            }
            Opened::Behind(old) => old,
        };
        // A lost receipt-key record is a refusal before any safety-copy rehearsal or schema change.
        // Very old schemas with no attachment table cannot hold stranded attachment keys.
        if record.is_none() {
            let stranded = old.read(|q| -> fb_core::FbResult<_> {
                let has: bool = q.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='attachments_attachment')", [], |r| r.get(0))?;
                if has { fb_core::attachments::unopenable(q, key) } else { Ok(Vec::new()) }
            })?;
            refuse_stranded(path, stranded.len())?;
        }
        let (from, to) = (old.from(), old.to());
        let (copy, incomplete) = self.upgrade_copy(&old, path, key, record).map_err(|reason| {
            let folder = self.backups.folder().map(|f| f.display().to_string()).unwrap_or_else(|_| "not set up".into());
            AppError::Open(OpenError::UpgradeCopy { from, to, folder, reason })
        })?;
        crate::protection::checkpoint("upgrade-copy-verified");
        // Page 1 decrypted, so the key is right: any failure from here is the file's.
        let mut book = old.upgrade().map_err(|e| open_error(e, true))?;
        if record.is_none() { refuse_lost_key_record(&mut book, path, key)?; }
        let mut notice = format!(
            "These books were updated for this version of FreedomBooks (book version {from} to {to}). A checked backup copy from before the update is in your backup folder: {}",
            copy.display()
        );
        if let Some(incomplete) = incomplete {
            notice.push_str(&format!(" {incomplete}"));
        }
        Ok((book, Some(notice)))
    }

    /// The adopt paths' own check, on the open book: refuse to write a key record for these books when
    /// their key would strand attached files (see [`OpenError::KeyFileMissing`]).
    pub(crate) fn refuse_adopting_keys(&mut self) -> AppResult<()> {
        let (path, key) = self.backups.open_book()?;
        let (path, key) = (path.to_owned(), MasterKey::new(**key));
        let open = self.open.as_mut().ok_or(AppError::Locked)?;
        refuse_lost_key_record(&mut open.book, &path, &key)
    }

    fn upgrade_copy(&self, old: &Unmigrated, path: &Path, key: &MasterKey, record: Option<&Record>) -> Result<(PathBuf, Option<String>), String> {
        let receipt_key = match record {
            Some(r) => r.keys.receipts(key).map_err(|e| e.to_string())?,
            None => MasterKey::new(**key),
        };
        // A copied built-in book has not adopted its local record yet. Its safety copy
        // still needs the normal portable envelope; creating it here does not save a sidecar.
        let copied_builtin = if record.is_none() && **key == BUILTIN_KEY {
            Some(KeyEnvelope::new(key, &receipt_key, None).map_err(|e| e.to_string())?)
        } else { None };
        let envelope = record.map(|r| &r.keys).or(copied_builtin.as_ref());
        let suffix = format!("before-upgrade-{}-to-{}", old.from(), old.to());
        crate::screens::backups::verified_copy(&self.backups, old, path, key, &receipt_key, envelope, &suffix)
    }
}

/// A book without a key record is opened with its own key as its receipt key. If any attached file opens
/// under no wrap with that key, the real receipt key is in a record that is not here (the book was copied
/// or moved without it): refuse, so no new record replaces it. Only key wraps are checked; no file is read.
fn refuse_lost_key_record(book: &mut Book, path: &Path, key: &MasterKey) -> AppResult<()> {
    let stranded = book.read(|q| fb_core::attachments::unopenable(q, key))?;
    refuse_stranded(path, stranded.len())
}

fn refuse_stranded(path: &Path, files: usize) -> AppResult<()> {
    if files == 0 { return Ok(()); }
    let record = crate::protection::record_path(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Err(AppError::Open(OpenError::KeyFileMissing { record, files }))
}
