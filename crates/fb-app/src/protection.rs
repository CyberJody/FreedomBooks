//! Durable book-key records and first-run publication. No unwrapped key is persisted here.
use crate::{
    App, AppError, AppResult,
    keys::{self, MasterKey},
};
use fb_core::backups::portable::{BUILTIN_KEY, KeyEnvelope, Mode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub const SETTINGS: &str = "/settings/book-protection/";
pub const RECOVERY: &str = "/settings/recovery-key/";
pub const KIT: &str = "/settings/emergency-kit/";
// Not a renderer route: these one-use tickets stay in the trusted native/main boundary.
const RECOVERY_EXPORT: &str = "native:recovery-export";
/// Shown on the switch forms while the backup folder holds backups without a key envelope.
pub const OLDER_BACKUPS: &str = "Your backup folder has backups made before recovery keys were added. They hold no password or recovery key and open only with the key these books use now, so after this change they cannot be restored. The verified backup taken now can be.";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub format: u32,
    pub keys: KeyEnvelope,
    pub ask_password: bool,
    // Read existing records/journals without retaining or acting on this retired marker.
    #[serde(default, skip_serializing, rename = "desktop_signin_configured")]
    _retired_signin_marker: Option<bool>,
    pub recovery_saved: bool,
}

pub fn error(e: impl std::fmt::Display) -> AppError {
    AppError::Invalid(e.to_string())
}
pub fn sidecar(book: &Path, suffix: &str) -> PathBuf {
    let mut p = book.as_os_str().to_owned();
    p.push(suffix);
    p.into()
}
pub fn record_path(book: &Path) -> PathBuf {
    sidecar(book, ".keys.json")
}
pub fn private_dir(path: &Path) -> AppResult<()> {
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(path).map_err(error)
}
pub fn sync_dir(path: &Path) -> AppResult<()> {
    #[cfg(unix)]
    {
        fs::File::open(path)
            .and_then(|f| f.sync_all())
            .map_err(error)?;
    }
    Ok(())
}
pub fn write_private(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let parent = path.parent().ok_or_else(|| error("Missing key folder."))?;
    private_dir(parent)?;
    match fs::symlink_metadata(path) {
        Ok(m) if !m.is_file() => return Err(error("Use a regular key file.")),
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(error(e)),
        _ => (),
    }
    let tmp = sidecar(path, &format!(".tmp-{}", uuid::Uuid::new_v4()));
    let mut opts = fs::OpenOptions::new();
    // Exclusive creation guarantees an empty file; append mode cannot replace existing bytes.
    opts.append(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = (|| {
        let mut file = opts.open(&tmp).map_err(error)?;
        journal_fault(path, bytes, "write")?;
        file.write_all(bytes).map_err(error)?;
        journal_fault(path, bytes, "file-sync")?;
        file.sync_all().map_err(error)?;
        journal_fault(path, bytes, "rename")?;
        fs::rename(&tmp, path).map_err(error)?;
        journal_fault(path, bytes, "dir-sync")?;
        sync_dir(parent)?;
        journal_fault(path, bytes, "published")
    })();
    let _ = fs::remove_file(tmp);
    result
}
fn journal_fault(_path: &Path, _bytes: &[u8], _stage: &str) -> AppResult<()> {
    #[cfg(feature = "key-test-hooks")]
    if _path.as_os_str().as_encoded_bytes().ends_with(b".rekey.json") {
        let value: serde_json::Value = serde_json::from_slice(_bytes).map_err(error)?;
        let phase = if value["committed"] == true { "committed" } else { "prepared" };
        if fault(&format!("journal-{phase}-{_stage}")) {
            return Err(error("Injected protection journal failure."));
        }
    }
    Ok(())
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> AppResult<T> {
    let m = fs::symlink_metadata(path).map_err(error)?;
    if !m.is_file() || m.len() > 65536 {
        return Err(error("The key record is not a regular, bounded file."));
    }
    let raw = fs::read_to_string(path).map_err(error)?;
    serde_json::from_value(fb_db::parse_unique_json(&raw).map_err(error)?).map_err(error)
}
impl Record {
    pub fn new(keys: KeyEnvelope) -> Self {
        Self {
            format: 1,
            ask_password: false,
            _retired_signin_marker: None,
            keys,
            recovery_saved: false,
        }
    }
    pub fn read(book: &Path) -> AppResult<Option<Self>> {
        let path = record_path(book);
        if !path.try_exists().map_err(error)? {
            return Ok(None);
        }
        let record: Self = read_json(&path)?;
        record.keys.validate().map_err(error)?;
        if record.format != 1 || (record.keys.mode == Mode::BuiltIn && record.ask_password) {
            return Err(error("The book protection settings are damaged."));
        }
        Ok(Some(record))
    }
    pub fn save(&self, book: &Path) -> AppResult<()> {
        write_private(
            &record_path(book),
            &serde_json::to_vec(self).map_err(error)?,
        )
    }
}

/// Leftovers of an interrupted run: `<book>.*.tmp-<uuid>` files from an atomic write that never renamed,
/// and the first-run stage once the book is published (then only a second link to the same file).
pub fn clean_leftovers(book: &Path) -> AppResult<()> {
    let (Some(parent), Some(name)) = (book.parent(), book.file_name().and_then(|n| n.to_str())) else {
        return Ok(());
    };
    let prefix = format!("{name}.");
    let Ok(entries) = fs::read_dir(parent) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let file = entry.file_name();
        let Some(file) = file.to_str() else { continue };
        let temporary = file.starts_with(&prefix)
            && file
                .rsplit_once(".tmp-")
                .is_some_and(|(_, id)| uuid::Uuid::parse_str(id).is_ok());
        if temporary && entry.file_type().is_ok_and(|t| t.is_file()) {
            fs::remove_file(entry.path()).map_err(error)?;
        }
    }
    let stage = sidecar(book, ".creating");
    if fs::symlink_metadata(book).is_ok_and(|m| m.is_file())
        && fs::symlink_metadata(&stage).is_ok_and(|m| m.is_dir())
    {
        fs::remove_dir_all(&stage).map_err(error)?;
    }
    Ok(())
}

/// Fault points exist only in explicitly built test binaries, never release packages.
pub fn checkpoint(_name: &str) {
    #[cfg(feature = "key-test-hooks")]
    if std::env::var("FB_KEY_STOP_AT").ok().as_deref() == Some(_name) {
        if let Ok(path) = std::env::var("FB_KEY_STOP_MARKER") {
            fs::write(path, _name).expect("test marker");
        }
        loop {
            if std::env::var_os("FB_KEY_RESUME_MARKER").is_some_and(|path| Path::new(&path).exists()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

/// A failure injected at `name` (FB_KEY_FAIL_AT), only in explicitly built test binaries.
pub fn fault(_name: &str) -> bool {
    #[cfg(feature = "key-test-hooks")]
    if std::env::var("FB_KEY_FAIL_AT").ok().as_deref() == Some(_name) {
        return true;
    }
    false
}

impl App {
    /// Publish key material first and a completed, closed book last. An interrupted stage is disposable.
    pub fn create_protected_book(
        &mut self,
        path: &Path,
        owner: &str,
        company: &str,
        password: Option<&str>,
    ) -> AppResult<crate::session::Created> {
        self.create_protected_book_with_chart(path, owner, company, password, crate::commands::StarterChart::Empty)
    }

    /// Pass the chosen chart through the existing protected first-run publication flow.
    pub fn create_protected_book_with_chart(
        &mut self,
        path: &Path,
        owner: &str,
        company: &str,
        password: Option<&str>,
        chart: crate::commands::StarterChart,
    ) -> AppResult<crate::session::Created> {
        let result = (|| {
            if fs::symlink_metadata(path).is_ok() {
                return Err(error("A book already exists at that location."));
            }
            let parent = path.parent().ok_or_else(|| error("Missing book folder."))?;
            private_dir(parent)?;
            clean_leftovers(path)?;
            let stage = sidecar(path, ".creating");
            if fs::symlink_metadata(&stage).is_ok() {
                if !fs::symlink_metadata(&stage).map_err(error)?.is_dir() {
                    return Err(error("Invalid first-run stage."));
                }
                fs::remove_dir_all(&stage).map_err(error)?;
            }
            private_dir(&stage)?;
            let master = if password.is_some() {
                keys::new_master_key().map_err(error)?
            } else {
                MasterKey::new(BUILTIN_KEY)
            };
            let receipts = keys::new_master_key().map_err(error)?;
            let record =
                Record::new(KeyEnvelope::new(&master, &receipts, password).map_err(error)?);
            record.save(path)?;
            checkpoint("create-keys-saved");
            let staged = stage.join("book.fbbook");
            let created = self.create_book_with_chart(&staged, &master, owner, company, chart)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&staged, fs::Permissions::from_mode(0o600)).map_err(error)?;
            }
            checkpoint("create-book-written");
            self.lock();
            fs::File::open(&staged)
                .and_then(|f| f.sync_all())
                .map_err(error)?;
            // Refuse a raced destination; a hard link publishes without replacement on the same volume.
            fs::hard_link(&staged, path).map_err(error)?;
            sync_dir(parent)?;
            checkpoint("create-book-published");
            fs::remove_dir_all(stage).map_err(error)?;
            self.open_book(path, &master)?;
            self.recovery_authorized = password.is_some();
            Ok(created)
        })();
        if result.is_err() {
            self.lock();
        }
        result
    }

    pub fn load_protection(&mut self, path: &Path, key: &MasterKey) -> AppResult<()> {
        let record = Record::read(path)?;
        if let Some(ref r) = record {
            r.keys.receipts(key).map_err(error)?;
        }
        self.protection = record;
        Ok(())
    }

    /// Older password wraps keep working. After their first successful unlock, add portable wraps.
    pub fn adopt_legacy_keys(&mut self, password: &str) -> AppResult<()> {
        if self.protection.is_some() {
            return Ok(());
        }
        let (path, key) = self.backups.open_book()?;
        let record = Record::new(KeyEnvelope::new(key, key, Some(password)).map_err(error)?);
        record.save(path)?;
        self.protection = Some(record);
        self.recovery_authorized = true;
        Ok(())
    }

    pub fn receipt_key(&self) -> AppResult<MasterKey> {
        let (_, key) = self.backups.open_book()?;
        match &self.protection {
            Some(r) => r.keys.receipts(key).map_err(error),
            None => Ok(MasterKey::new(**key)),
        }
    }
    pub fn builtin(&self) -> bool {
        self.protection
            .as_ref()
            .is_some_and(|r| r.keys.mode == Mode::BuiltIn)
    }
    pub fn can_lock(&self) -> bool {
        self.is_open()
            && !self.builtin()
            && self
                .protection
                .as_ref()
                .is_none_or(|r| r.ask_password || !self.os_store_usable)
    }
    pub fn authenticate_owner(&mut self, password: &str) -> AppResult<()> {
        self.authorize(None, crate::identity::Permission::Administer)?;
        let record = self
            .protection
            .as_ref()
            .ok_or_else(|| error("Set up recovery for these books first."))?;
        record
            .keys
            .verify_password(password, self.backups.open_book()?.1)
            .map_err(|_| error("That password does not unlock these books."))
    }
    pub fn has_credentials(&mut self) -> AppResult<bool> {
        Ok(self.open_mut()?.book.read(|q| {
            q.query_row("SELECT EXISTS(SELECT 1 FROM plugin_credential)", [], |r| {
                r.get(0)
            })
        })?)
    }
    pub fn change_ask_password(&mut self, password: &str, ask: bool) -> AppResult<()> {
        self.authenticate_owner(password)?;
        if !ask && !self.os_store_usable {
            return Err(error(
                "This computer has no system keyring. Keep asking for your password when opening these books.",
            ));
        }
        let mut record = self.protection.clone().ok_or(AppError::Locked)?;
        record.ask_password = ask;
        self.save_record_audited(record, "book.password_prompt", serde_json::json!({"ask_password":ask}))
    }
    pub fn authorize_recovery(&mut self, password: &str) -> AppResult<()> {
        self.authenticate_owner(password)?;
        self.tokens.clear();
        self.recovery_authorized = true;
        Ok(())
    }
    pub fn recovery_text(&mut self) -> AppResult<zeroize::Zeroizing<String>> {
        self.authorize(None, crate::identity::Permission::Administer)?;
        if !self.recovery_authorized {
            return Err(error("Enter your password to show the recovery key."));
        }
        self.protection
            .as_ref()
            .ok_or(AppError::Locked)?
            .keys
            .recovery_text(self.backups.open_book()?.1)
            .map_err(error)
    }
    pub fn recovery_saved(&mut self) -> AppResult<()> {
        self.recovery_text()?;
        let mut record = self.protection.clone().ok_or(AppError::Locked)?;
        record.recovery_saved = true;
        self.save_record_audited(record, "book.recovery_saved", serde_json::json!({}))
    }
    pub fn recovery_export_ticket(&mut self) -> AppResult<String> {
        self.recovery_text()?;
        Ok(self.tokens.mint(RECOVERY_EXPORT))
    }
    pub fn validate_recovery_export(&mut self, ticket: &str) -> AppResult<()> {
        self.recovery_text()?;
        if !self.tokens.valid(ticket, RECOVERY_EXPORT) {
            return Err(error("The recovery export expired. Open the recovery key again."));
        }
        Ok(())
    }
    pub fn recovery_saved_export(&mut self, ticket: &str) -> AppResult<()> {
        self.validate_recovery_export(ticket)?;
        self.tokens.consume(ticket, RECOVERY_EXPORT);
        self.recovery_saved()
    }
    fn protection_preflight(&mut self) -> AppResult<fb_db::ProtectionLease> {
        self.authorize(None, crate::identity::Permission::Administer)?;
        if !self.open_mut()?.staff { return Err(error("Book protection is limited to the owner.")); }
        self.first_company()?.ok_or_else(|| error("No company in these books."))?;
        let lease = self.open_mut()?.book.protection_lease().map_err(error)?;
        let clear = (|| {
            let (path, _) = self.backups.open_book()?;
            if present(&sidecar(path, ".rekey.json"))? || present(&sidecar(path, ".rekey-before"))? {
                return Err(error("Restart FreedomBooks to recover the previous protection change first."));
            }
            Ok(())
        })();
        if let Err(e) = clear {
            self.lock();
            drop(lease);
            return Err(e);
        }
        Ok(lease)
    }

    // Filesystem publication and the actor event share the same rollback snapshot. Until the
    // audit is durable the marker remains, so startup restores BOTH old settings and old audit.
    fn save_record_audited(&mut self, record: Record, action: &str, detail: serde_json::Value) -> AppResult<()> {
        let lease = self.protection_preflight()?;
        self.with_protection_change(lease, |app, path| {
            record.save(path)?;
            app.protection = Some(record);
            checkpoint("protection-record-published");
            app.audit_protection(action, detail)
        })
    }

    // One owner for preparation, publication, audit, commit and cleanup. Any failure after
    // preparation begins closes the live connection BEFORE releasing the reservation. Startup
    // recovery inspects the actual journal under an exclusive guard: an error from rename/fsync
    // cannot tell us whether the old or new marker is durable, so never guess and reopen here.
    fn with_protection_change(
        &mut self,
        lease: fb_db::ProtectionLease,
        change: impl FnOnce(&mut Self, &Path) -> AppResult<()>,
    ) -> AppResult<()> {
        let old = self.protection.clone().ok_or(AppError::Locked)?;
        let (path, key) = self.backups.open_book()?;
        let (path, key) = (path.to_owned(), MasterKey::new(**key));
        let result = (|| {
            let snapshot = sidecar(&path, ".rekey-before");
            let marker = sidecar(&path, ".rekey.json");
            if present(&marker)? || present(&snapshot)? {
                return Err(error("Recover the previous protection change first."));
            }
            self.tokens.clear();
            checkpoint("protection-before-snapshot");
            self.open_mut()?.book.snapshot(&snapshot, &key).map_err(error)?;
            let journal = RekeyJournal { old, snapshot_hash: file_hash(&snapshot)?, committed: false };
            write_private(&marker, &serde_json::to_vec(&journal).map_err(error)?)?;
            checkpoint("rekey-journal-saved");
            change(self, &path)?;
            checkpoint("protection-audit-written");
            journal.finish(&path)
        })();
        if result.is_err() { self.lock(); }
        drop(lease);
        result.map_err(|e| error(format!("These books were closed after an incomplete protection change. Restart FreedomBooks to recover them before continuing. {e}")))
    }

    pub fn audit_protection(&mut self, action: &str, detail: serde_json::Value) -> AppResult<()> {
        checkpoint("protection-before-audit");
        #[cfg(feature = "key-test-hooks")]
        if std::env::var("FB_KEY_FAIL_AUDIT").ok().as_deref() == Some(action) {
            return Err(error("Injected protection audit failure."));
        }
        let c = self
            .first_company()?
            .ok_or_else(|| error("No company in these books."))?;
        let open = self.open_mut()?;
        fb_core::backups::portable::audit_event(&mut open.book, open.owner, c, action, detail)?;
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RekeyJournal {
    old: Record,
    snapshot_hash: String,
    #[serde(default)]
    committed: bool,
}
impl RekeyJournal {
    fn finish(mut self, path: &Path) -> AppResult<()> {
        self.committed = true;
        write_private(&sidecar(path, ".rekey.json"), &serde_json::to_vec(&self).map_err(error)?)?;
        checkpoint("protection-committed");
        cleanup_committed(path)
    }
}
fn cleanup_committed(path: &Path) -> AppResult<()> {
    let snapshot = sidecar(path, ".rekey-before");
    cleanup_fault("snapshot-remove")?;
    match fs::remove_file(snapshot) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(error(e)),
    }
    let parent = path.parent().ok_or_else(|| error("Missing book folder."))?;
    cleanup_fault("snapshot-sync")?;
    sync_dir(parent)?;
    cleanup_fault("marker-remove")?;
    fs::remove_file(sidecar(path, ".rekey.json")).map_err(error)?;
    cleanup_fault("marker-sync")?;
    sync_dir(parent)
}

fn cleanup_fault(stage: &str) -> AppResult<()> {
    if fault(&format!("journal-committed-cleanup-{stage}")) {
        return Err(error("Injected protection journal failure."));
    }
    Ok(())
}

fn present(path: &Path) -> AppResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(error(e)),
    }
}

fn file_hash(path: &Path) -> AppResult<String> {
    use std::io::Read;
    let mut f = fs::File::open(path).map_err(error)?;
    let mut hash = Sha256::new();
    let mut buf = [0; 65536];
    loop {
        let n = f.read(&mut buf).map_err(error)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

/// Before any open, roll back an uncommitted switch or finish cleanup after a durable actor audit.
pub fn recover_rekey(path: &Path) -> AppResult<bool> {
    let guard = fb_db::MaintenanceGuard::acquire(path).map_err(error)?;
    recover_rekey_guarded(path, &guard)
}

/// Native startup cleanup and recovery share one exclusive maintenance lifetime.
pub fn recover_key_change(path: &Path) -> AppResult<bool> {
    let guard = fb_db::MaintenanceGuard::acquire(path).map_err(error)?;
    clean_leftovers(path)?;
    recover_rekey_guarded(path, &guard)
}

fn recover_rekey_guarded(path: &Path, guard: &fb_db::MaintenanceGuard) -> AppResult<bool> {
    guard.ensure_single_link().map_err(error)?;
    let marker = sidecar(path, ".rekey.json");
    if !present(&marker)? {
        // A snapshot can be left by failed preparation. No live mutation begins until the
        // prepared journal has been synced. Under this maintenance guard its absence proves
        // that this unreferenced preparation copy cannot be used for a later rollback.
        let snapshot = sidecar(path, ".rekey-before");
        if !present(&snapshot)? { return Ok(false); }
        if !fs::symlink_metadata(&snapshot).map_err(error)?.is_file() {
            return Err(error("The protection snapshot is not a regular file."));
        }
        fs::remove_file(snapshot).map_err(error)?;
        sync_dir(path.parent().ok_or_else(|| error("Missing book folder."))?)?;
        return Ok(true);
    }
    let journal: RekeyJournal = read_json(&marker)?;
    journal.old.keys.validate().map_err(error)?;
    if journal.committed { cleanup_committed(path)?; return Ok(true); }
    let snapshot = sidecar(path, ".rekey-before");
    if file_hash(&snapshot)? != journal.snapshot_hash {
        return Err(error(
            "The interrupted password change needs its verified backup. Restore that backup as a new book.",
        ));
    }
    let temporary = sidecar(path, ".rekey-rollback");
    if temporary.exists() {
        fs::remove_file(&temporary).map_err(error)?;
    }
    fs::copy(&snapshot, &temporary).map_err(error)?;
    fs::File::open(&temporary)
        .and_then(|f| f.sync_all())
        .map_err(error)?;
    let _replacement = guard.lock_replacement(&temporary).map_err(error)?;
    for suffix in ["-wal", "-shm", "-journal"] {
        let p = sidecar(path, suffix);
        if p.exists() {
            fs::remove_file(p).map_err(error)?;
        }
    }
    fs::rename(&temporary, path).map_err(error)?;
    checkpoint("rollback-book-published");
    journal.old.save(path)?;
    checkpoint("rollback-record-published");
    sync_dir(path.parent().ok_or_else(|| error("Missing book folder."))?)?;
    journal.finish(path)?;
    Ok(true)
}

impl App {
    pub fn change_protection(
        &mut self,
        current: &str,
        new_password: Option<&str>,
    ) -> AppResult<()> {
        let lease = self.protection_preflight()?;
        let old = self
            .protection
            .clone()
            .ok_or_else(|| error("Set up recovery for these books first."))?;
        if old.keys.mode == Mode::OwnPassword {
            self.authenticate_owner(current)?;
        }
        if new_password.is_none() && self.has_credentials()? {
            return Err(error(
                "Disconnect the saved bank-feed and AI connections before removing your password.",
            ));
        }
        let receipts = self.receipt_key()?;
        let next = if new_password.is_some() {
            keys::new_master_key().map_err(error)?
        } else {
            MasterKey::new(BUILTIN_KEY)
        };
        let mut record = Record::new(KeyEnvelope::new(&next, &receipts, new_password).map_err(error)?);
        if old.keys.mode == Mode::OwnPassword && new_password.is_some() {
            // Rotating the encryption key does not change this computer's sign-in preference.
            record.ask_password = old.ask_password;
        }
        let company = self
            .first_company()?
            .ok_or_else(|| error("No company in these books."))?;
        // This returns only after opening and checking the complete archive and all receipts.
        crate::screens::backups::verified_backup(self, company)?;
        checkpoint("rekey-backup-verified");
        self.with_protection_change(lease, |app, path| {
            app.open_mut()?.book.rekey(&next).map_err(error)?;
            checkpoint("rekey-pages-written");
            record.save(path)?;
            checkpoint("rekey-record-saved");
            // Keep the same live connection and both OS guards through the final audit.
            app.backups.opened(path, &next);
            app.protection = Some(record.clone());
            app.audit_protection("book.protection_changed", serde_json::json!({"mode":record.keys.mode}))
        })?;
        self.recovery_authorized = new_password.is_some();
        Ok(())
    }
}

pub fn builtin_mode() -> Mode {
    Mode::BuiltIn
}
pub fn builtin_key() -> MasterKey {
    MasterKey::new(BUILTIN_KEY)
}

impl App {
    pub fn book_path(&self) -> Option<&Path> {
        self.backups.open_book().ok().map(|(path, _)| path)
    }
    pub fn master_key(&self) -> AppResult<&MasterKey> {
        Ok(self.backups.open_book()?.1)
    }
    pub fn adopt_builtin_keys(&mut self) -> AppResult<()> {
        let (path, key) = self.backups.open_book()?;
        let record = Record::new(KeyEnvelope::new(key, key, None).map_err(error)?);
        record.save(path)?;
        self.protection = Some(record);
        Ok(())
    }
    pub fn company_name_for_kit(&mut self) -> AppResult<String> {
        let id = self
            .first_company()?
            .ok_or_else(|| error("No company in these books."))?;
        Ok(self.open_mut()?.book.read(|q| {
            q.query_row(
                "SELECT name FROM ledger_company WHERE id=?1",
                [id.to_db()],
                |r| r.get(0),
            )
        })?)
    }
}
