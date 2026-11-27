//! SQLCipher book + encrypted receipts. Format 3 deliberately cannot be confused
//! with Python's PostgreSQL formats 1/2. Optional portable-export encryption keeps
//! Python's exact stream cipher; ordinary local backups need no extra passphrase.
pub mod audit;
pub mod cleanup;
pub mod crypto;
pub mod manifest;
mod paths;
pub use paths::Directory;
mod restore_limits;
pub mod recovery;
pub mod portable;
pub mod retention;
pub mod scheduled;

use fb_db::{Book, Key, Query};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use tempfile::TempDir;

pub const FORMAT: &str = "freedombooks-backup";
pub const FORMAT_VERSION: u64 = 4;
pub type Result<T> = std::result::Result<T, BackupError>;
#[derive(Debug)]
pub struct BackupError {
    message: String,
    absent: Vec<Absent>,
    receipt_problem: Option<&'static str>,
    failure: Failure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Failure {
    Validation,
    Retryable,
    Conflict,
}
impl BackupError {
    fn new(message: impl Into<String>, failure: Failure) -> Self {
        Self { message: message.into(), absent: Vec::new(), receipt_problem: None, failure }
    }
    pub fn refused(message: impl Into<String>) -> Self {
        Self::new(message, Failure::Validation)
    }
    fn retryable(message: impl Into<String>) -> Self {
        Self::new(message, Failure::Retryable)
    }
    fn conflict(message: impl Into<String>) -> Self {
        Self::new(message, Failure::Conflict)
    }
    fn archive_conflict(mut self) -> Self {
        if self.failure == Failure::Validation {
            self.failure = Failure::Conflict;
        }
        self
    }
    pub fn is_conflict(&self) -> bool {
        self.failure == Failure::Conflict
    }
    /// A receipt verifier's verdict on one attachment file: [`DAMAGED`] or [`NO_KEY`]. Any other verifier
    /// error is about this computer, not the file, and stops a backup.
    pub fn receipt(message: impl Into<String>, problem: &'static str) -> Self {
        let mut error = Self::refused(message);
        error.receipt_problem = Some(problem);
        error
    }
    /// The attachment files that stopped a complete backup. Empty for every other refusal.
    pub fn absent(&self) -> &[Absent] {
        &self.absent
    }
    /// For a verifier's verdict on a file: [`DAMAGED`] or [`NO_KEY`].
    pub fn problem(&self) -> Option<&'static str> {
        self.receipt_problem
    }
}
impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for BackupError {}
#[cfg(unix)]
impl From<rustix::io::Errno> for BackupError {
    fn from(e: rustix::io::Errno) -> Self {
        Self::retryable(format!("Backup path refused: {e}"))
    }
}
impl From<std::io::Error> for BackupError {
    fn from(e: std::io::Error) -> Self {
        Self::retryable(format!("Backup I/O failed: {e}"))
    }
}
impl From<fb_db::DbError> for BackupError {
    fn from(e: fb_db::DbError) -> Self {
        let message = format!("Book verification failed: {e}");
        if e.is_damage() || matches!(e, fb_db::DbError::WrongKeyOrNotABook | fb_db::DbError::TooNew { .. }) {
            Self::refused(message)
        } else {
            Self::retryable(message)
        }
    }
}
impl From<fb_db::rusqlite::Error> for BackupError {
    fn from(e: fb_db::rusqlite::Error) -> Self {
        Self::retryable(format!("Manifest query failed: {e}"))
    }
}
impl From<serde_json::Error> for BackupError {
    fn from(_: serde_json::Error) -> Self {
        Self::refused("Malformed backup manifest.")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReceiptRef {
    pub id: String,
    pub company: String,
    pub stored_name: String,
    pub plain_bytes: u64,
    pub plain_sha256: String,
    pub original_sha256: String,
    pub content_type: String,
    /// The attachment row's own wrap of the file key.
    pub wrapped_key: String,
    pub wrap_nonce: String,
    pub nonce: String,
    /// Further wraps recorded in the book (table rows and historical audit candidates). Absent
    /// from the manifest when there are none, exactly as before such wraps existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rewraps: Vec<crate::attachments::storage::Wrap>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub entry: String,
    pub bytes: u64,
    pub sha256: String,
    pub owners: Vec<ReceiptRef>,
}
/// An attachment whose file was not in the backup when it was made: its row is in the book, its file is
/// not. `problem` is [`MISSING`] (no file), [`DAMAGED`] (wrong size, not a regular file, or content that
/// fails authentication under a key that opens its wrap) or [`NO_KEY`] (no key the book holds opens any
/// wrap of its key: the file may be intact, the key is what is missing). `name` and `document` are for
/// people: the file's name and its owner ("Invoice INV-0003", "Journal entry J000004").
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Absent {
    pub owner: ReceiptRef,
    pub name: String,
    pub document: String,
    pub removed: bool,
    pub problem: String,
}
pub const MISSING: &str = "missing";
pub const DAMAGED: &str = "damaged";
pub const NO_KEY: &str = "no key";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: String,
    pub format_version: u64,
    pub created_at: String,
    pub app_version: String,
    pub migrations: Vec<String>,
    pub book_bytes: u64,
    pub book_sha256: String,
    pub contents: manifest::Contents,
    pub attachments: Vec<Receipt>,
    pub not_included: Vec<String>,
    /// Non-empty only for an incomplete backup (see [`BackupOptions::allow_incomplete`]). A complete
    /// backup's manifest has no such key, exactly as before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub absent: Vec<Absent>,
}
impl Manifest {
    /// A backup holds every attachment file its book refers to.
    pub fn complete(&self) -> bool {
        self.absent.is_empty()
    }
}

/// Supplied by the attachment module. It must authenticate the staged ciphertext
/// against its owning row (AEAD, plaintext hash and size). No filesystem writes.
pub type ReceiptVerifier<'a> = dyn Fn(&ReceiptRef, &Path) -> Result<()> + 'a;
/// The attachment module's [`ReceiptVerifier`] (L4-BACKUPS integration request 1), for
/// `BackupOptions`, `RestoreOptions` and `verify_backup_with_receipts`: each staged ciphertext is
/// authenticated against its owning row's immutable snapshot under the book's receipt key, without the live book.
pub fn attachment_receipt_verifier(key: &Key) -> impl Fn(&ReceiptRef, &Path) -> Result<()> + '_ {
    attachment_receipt_verifier_with(key, None)
}

/// [`attachment_receipt_verifier`] that also accepts a file key wrapped under the book's master key
/// (files attached through the desktop chooser before the receipt-key fix). Contents are checked in full
/// either way; a file that fails under both keys is damaged.
pub fn attachment_receipt_verifier_with<'a>(receipt_key: &'a Key, master_key: Option<&'a Key>) -> impl Fn(&ReceiptRef, &Path) -> Result<()> + 'a {
    use crate::attachments::storage::{self, ReadError};
    move |r, staged| {
        let damaged = || BackupError::receipt("A receipt file does not match its attachment record.", DAMAGED);
        let company = crate::CompanyId::parse(&r.company).ok_or_else(damaged)?;
        let stored = storage::Stored {
            stored_name: r.stored_name.clone(),
            content_type: r.content_type.clone(),
            size: i64::try_from(r.plain_bytes).map_err(|_| damaged())?,
            sha256: r.plain_sha256.clone(),
            original_sha256: r.original_sha256.clone(),
            wrapped_key: r.wrapped_key.clone(),
            wrap_nonce: r.wrap_nonce.clone(),
            nonce: r.nonce.clone(),
            rewraps: r.rewraps.clone(),
        };
        let keys: Vec<&Key> = std::iter::once(receipt_key).chain(master_key).collect();
        match storage::verify_staged_checked(&keys, company, &stored, staged) {
            Ok(()) => Ok(()),
            Err(ReadError::NoKey) => Err(BackupError::receipt("No key these books hold opens a receipt file.", NO_KEY)),
            Err(ReadError::System(e)) => Err(BackupError::retryable(format!("A receipt file could not be read ({e})."))),
            Err(_) => Err(damaged()),
        }
    }
}

/// Independent raw-SQL lifecycle verifier. It sees only the staged, migrated
/// book through a read-only handle; Err must abort verification and note writing.
pub type MoneyCheck<'a> = dyn Fn(&fb_db::Reader<'_>, &Manifest) -> Result<Value> + 'a;
pub struct VerifyOptions<'a> {
    pub work_root: &'a Path,
    pub passphrase: Option<&'a str>,
    pub record: bool,
    pub verify_receipt: Option<&'a ReceiptVerifier<'a>>,
    pub money_check: Option<&'a MoneyCheck<'a>>,
}
pub struct BackupOptions<'a> {
    pub output: &'a Path,
    pub work_root: &'a Path,
    pub receipts_root: Option<&'a Path>,
    pub verify_receipt: Option<&'a ReceiptVerifier<'a>>,
    pub label: &'a str,
    pub app_version: &'a str,
    pub created_at: &'a str,
    pub passphrase: Option<&'a str>,
    pub keep: Option<usize>,
    /// Make the backup even when attachment files are missing or damaged: the book and every authentic
    /// attachment file go in, and the others are listed in the manifest (`Manifest::absent`) and flagged
    /// in the header (`"incomplete": true`). An incomplete backup never prunes older backups. When false,
    /// such a backup is refused and the error lists the files (`BackupError::absent`).
    pub allow_incomplete: bool,
}
/// Desktop copy plumbing shared by manual, pre-upgrade and automatic backups.
pub struct CopyWorker<'a> {
    pub key: &'a Key,
    pub receipt_key: &'a Key,
    pub receipts_root: Option<&'a Path>,
    pub envelope: Option<&'a portable::KeyEnvelope>,
    pub app_version: &'a str,
}

pub struct CopyTarget<'a> {
    pub output: &'a Path,
    pub work_root: &'a Path,
    pub label: &'a str,
    pub created_at: &'a str,
    pub allow_incomplete: bool,
}

impl CopyWorker<'_> {
    pub fn write(&self, source: &dyn fb_db::SnapshotSource, target: &CopyTarget<'_>) -> Result<BackupResult> {
        self.write_slot(source, target, None)
    }

    fn write_slot(&self, source: &dyn fb_db::SnapshotSource, target: &CopyTarget<'_>, slot: Option<&scheduled::Slot>) -> Result<BackupResult> {
        let verifier = attachment_receipt_verifier_with(self.receipt_key, Some(self.key));
        let options = BackupOptions {
            output: target.output, work_root: target.work_root, label: target.label, created_at: target.created_at,
            allow_incomplete: target.allow_incomplete, receipts_root: self.receipts_root, app_version: self.app_version,
            verify_receipt: Some(&verifier), passphrase: None, keep: None,
        };
        create_with_keys(source, self.key, &options, self.envelope, slot)
    }

    pub fn verify(&self, file: &Path, work: &Path) -> Result<Manifest> {
        let verified = self.check_copy(file, work, None)?;
        if let Some(summary) = verified.summary {
            retention::record_verified_matching(file, &verified.fingerprint, summary)?;
        }
        Ok(verified.manifest)
    }

    fn check_copy(&self, file: &Path, work: &Path, slot: Option<(&scheduled::Slot, &[String])>) -> Result<CheckedBackup> {
        let verifier = attachment_receipt_verifier_with(self.receipt_key, Some(self.key));
        let options = VerifyOptions {
            work_root: work, passphrase: None, record: true,
            verify_receipt: Some(&verifier), money_check: None,
        };
        check_backup(file, self.key, &options, slot)
    }
}

pub struct RestoreOptions<'a> {
    pub verify_receipt: Option<&'a ReceiptVerifier<'a>>,
    pub target: &'a Path,
    pub receipts_target: Option<&'a Path>,
    pub work_root: &'a Path,
    /// All configured live book, receipt, backup and work paths. The bridge must
    /// supply them; the core never guesses process environment settings.
    pub protected: &'a [PathBuf],
    pub passphrase: Option<&'a str>,
    pub require_encrypted: bool,
}
pub struct BackupResult {
    pub path: PathBuf,
    pub manifest: Manifest,
    pub pruned: Vec<PathBuf>,
}
pub struct RestoreResult {
    pub path: PathBuf,
    pub manifest: Manifest,
    pub upgraded: bool,
}

fn hash_file(path: &Path) -> Result<(u64, String)> {
    hash_open_file(paths::regular(path)?)
}
pub(super) fn hash_open_file(mut f: File) -> Result<(u64, String)> {
    let mut sha = Sha256::new();
    let mut bytes = 0u64;
    let mut buf = [0; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        sha.update(&buf[..n]);
        bytes = bytes
            .checked_add(n as u64)
            .ok_or_else(|| BackupError::refused("File too large."))?;
    }
    Ok((bytes, format!("{:x}", sha.finalize())))
}
fn json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut b = serde_json::to_vec_pretty(value)?;
    b.push(b'\n');
    Ok(b)
}
fn parse_json(raw: &[u8]) -> Result<Value> {
    fb_db::parse_unique_json(
        std::str::from_utf8(raw).map_err(|_| BackupError::refused("Backup JSON is not UTF-8."))?,
    )
    .map_err(|_| BackupError::refused("Backup JSON is malformed or contains duplicate keys."))
}
/// BACKUPS-POLISH: Django's `core.read_header` for the file list: the header of a format-3 backup, read
/// cheaply (archive headers only, entry data is skipped by seeking). A file that is not a FreedomBooks
/// archive, has no valid header, uses another format, or is cut short (an entry runs past the end of
/// the file) is an error, so the Backups page can mark it Unreadable before anyone tries to restore it.
pub fn read_header(path: &Path) -> Result<Value> {
    read_header_file(open_backup(path)?)
}
pub(super) fn read_header_file(file: File) -> Result<Value> {
    let len = file.metadata()?.len();
    let mut tar = tar::Archive::new(file);
    let mut header = None;
    for item in tar.entries_with_seek().map_err(archive_io)?.raw(true) {
        let mut item = item.map_err(archive_io)?;
        let end = item.raw_file_position().checked_add(item.size());
        if end.is_none_or(|end| end > len) {
            return Err(BackupError::refused("This backup file is incomplete (cut short)."));
        }
        if item.path_bytes().as_ref() == b"header.json" {
            if header.is_some() || !item.header().entry_type().is_file() || item.size() > 65536 {
                return Err(BackupError::refused("Not a FreedomBooks backup: invalid header.json."));
            }
            let mut raw = Vec::new();
            item.read_to_end(&mut raw).map_err(archive_io)?;
            header = Some(raw);
        }
    }
    let raw = header.ok_or_else(|| BackupError::refused("Not a FreedomBooks backup: no header.json."))?;
    let header = parse_json(&raw)?;
    if header["format"] != FORMAT {
        return Err(BackupError::refused("Not a FreedomBooks backup file."));
    }
    if !matches!(header["format_version"].as_u64(), Some(3 | 4))
        || header["database"] != "SQLCipher"
        || !header["encrypted"].is_boolean()
    {
        return Err(BackupError::refused(
            "Unsupported FreedomBooks backup format. PostgreSQL formats 1/2 need the reference importer.",
        ));
    }
    Ok(header)
}
/// Open a local backup for export without following links or opening special files.
pub fn open_backup(path: &Path) -> Result<File> {
    paths::regular(path)
}

/// Refuse ambiguous, linked or overlapping backup folders before any writes.
pub fn check_backup_folder<'a>(root: &Path, protected: impl IntoIterator<Item = &'a Path>) -> Result<()> {
    paths::resolve(root)?;
    for path in protected {
        if paths::overlap(root, path)? {
            return Err(BackupError::refused("The backup folder overlaps a live book, receipts or work folder."));
        }
    }
    Ok(())
}

fn hex_valid(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn digest_valid(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn valid_stored(s: &str) -> bool {
    s.len() == 36
        && s.ends_with(".bin")
        && s.as_bytes()[..32]
            .iter()
            .copied()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn receipt_refs(q: &(impl Query + ?Sized)) -> Result<Vec<ReceiptRef>> {
    let has: i64 = q.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='attachments_attachment'",
        [],
        |r| r.get(0),
    )?;
    if has == 0 {
        return Ok(Vec::new());
    }
    let sql = "SELECT id,company_id,stored_name,size,sha256,original_sha256,content_type,wrapped_key,wrap_nonce,nonce FROM attachments_attachment ORDER BY id";
    let mut refs: Vec<ReceiptRef> = q.prepare(sql)?
    .query_map([], |r| {
        Ok(ReceiptRef {
            id: r.get(0)?,
            company: r.get(1)?,
            stored_name: r.get(2)?,
            plain_bytes: u64::try_from(r.get::<_, i64>(3)?)
                .map_err(|_| fb_db::rusqlite::Error::InvalidQuery)?,
            plain_sha256: r.get(4)?,
            original_sha256: r.get(5)?,
            content_type: r.get(6)?,
            wrapped_key: r.get(7)?,
            wrap_nonce: r.get(8)?,
            nonce: r.get(9)?,
            rewraps: Vec::new(),
        })
    })?
    .collect::<std::result::Result<_, _>>()?;
    // Every further wrap of each file's key travels with each row that shares the file, so verification
    // can try them all. Creation and restore read the same snapshot the same way.
    let mut wraps: BTreeMap<(String, String), Vec<crate::attachments::storage::Wrap>> = BTreeMap::new();
    for r in &mut refs {
        let company = crate::CompanyId::parse(&r.company).ok_or_else(|| BackupError::refused("Invalid attachment manifest reference."))?;
        let entry = match wraps.entry((r.company.clone(), r.stored_name.clone())) {
            std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::btree_map::Entry::Vacant(e) => e.insert(
                crate::attachments::recorded_wraps(q, company, &r.stored_name)
                    .map_err(|e| BackupError::refused(format!("Manifest query failed: {e}")))?,
            ),
        };
        r.rewraps = entry.clone();
    }
    Ok(refs)
}
/// Who owns each attachment, for people: the original file name, its document ("Invoice INV-0003") and
/// whether it was removed. Best effort: an older or unusual layout yields no labels, never an error.
fn receipt_labels(q: &(impl Query + ?Sized)) -> BTreeMap<String, (String, String, bool)> {
    let read = || -> std::result::Result<BTreeMap<String, (String, String, bool)>, fb_db::rusqlite::Error> {
        q.prepare(
            "SELECT a.id, a.original_name, a.deleted, CASE WHEN a.journal_id IS NOT NULL THEN 'JOURNAL' ELSE d.kind END, o.number
             FROM attachments_attachment a LEFT JOIN documents_documentidentity d ON d.id=a.document_id
             LEFT JOIN attachment_owner_details o ON o.id=a.id",
        )?
        .query_map([], |r| {
            let kind: Option<String> = r.get(3)?;
            let number: Option<String> = r.get(4)?;
            let word = match kind.as_deref() {
                Some("EST") => "Estimate", Some("INV") => "Invoice", Some("RCP") => "Payment received",
                Some("SR") => "Sales receipt", Some("BILL") => "Bill", Some("BPAY") => "Bill payment",
                Some("DEP") => "Deposit", Some("CHK") => "Check", Some("TRF") => "Transfer",
                Some("CM") => "Credit memo", Some("RFD") => "Credit refund", Some("CDP") => "Customer deposit",
                Some("CDR") => "Customer deposit refund", Some("JOURNAL") => "Journal entry", _ => "Document",
            };
            let document = match number.filter(|n| !n.is_empty()) { Some(n) => format!("{word} {n}"), None => word.to_owned() };
            Ok((r.get::<_, String>(0)?, (r.get::<_, String>(1)?, document, r.get::<_, i64>(2)? != 0)))
        })?
        .collect()
    };
    read().unwrap_or_default()
}
fn validate_ref(row: &ReceiptRef) -> Result<()> {
    if uuid::Uuid::parse_str(&row.id).is_err()
        || uuid::Uuid::parse_str(&row.company).is_err()
        || !valid_stored(&row.stored_name)
        || row.plain_bytes == 0
        || row.plain_bytes > 104857600
        || !digest_valid(&row.plain_sha256)
        || !digest_valid(&row.original_sha256)
        || !matches!(
            row.content_type.as_str(),
            "application/pdf" | "image/png" | "image/jpeg" | "image/webp"
        )
        || !hex_valid(&row.wrapped_key, 96)
        || !hex_valid(&row.wrap_nonce, 24)
        || !hex_valid(&row.nonce, 14)
        || row.rewraps.len() > 1000
        || row.rewraps.iter().any(|w| !hex_valid(&w.wrapped_key, 96) || !hex_valid(&w.wrap_nonce, 24))
    {
        return Err(BackupError::refused(
            "Invalid attachment manifest reference.",
        ));
    }
    Ok(())
}
fn stage(root: &Path, avoid: &[&Path]) -> Result<TempDir> {
    for other in avoid {
        if paths::overlap(root, other)? {
            return Err(BackupError::refused(
                "The private backup work folder overlaps a backup, live or restore folder.",
            ));
        }
    }
    let root = paths::private_dir(root)?;
    stage_in(&root)
}

fn stage_in(root: &Path) -> Result<TempDir> {
    cleanup::cleanup_stale(root)?;
    paths::temp_directory(root, &cleanup::prefix())
}
fn append_bytes<W: Write>(tar: &mut tar::Builder<W>, name: &str, bytes: &[u8]) -> Result<()> {
    let mut h = tar::Header::new_ustar();
    h.set_size(bytes.len() as u64);
    h.set_mode(0o600);
    h.set_mtime(0);
    h.set_cksum();
    tar.append_data(&mut h, name, bytes)?;
    Ok(())
}
fn append_file<W: Write>(tar: &mut tar::Builder<W>, name: &str, path: &Path) -> Result<()> {
    let mut f = paths::regular(path)?;
    let mut h = tar::Header::new_ustar();
    h.set_size(f.metadata()?.len());
    h.set_mode(0o600);
    h.set_mtime(0);
    h.set_cksum();
    tar.append_data(&mut h, name, &mut f)?;
    Ok(())
}
fn payload(path: &Path, work: &Path, data: &Manifest) -> Result<()> {
    let mut tar = tar::Builder::new(paths::new_file(path)?);
    append_bytes(&mut tar, "manifest.json", &json_bytes(data)?)?;
    append_file(&mut tar, "book.fbbook", &work.join("book.fbbook"))?;
    for receipt in &data.attachments {
        append_file(&mut tar, &receipt.entry, &work.join(&receipt.entry))?;
    }
    let f = tar.into_inner()?;
    f.sync_all()?;
    Ok(())
}
fn publish(source: &Path, target: &Path) -> Result<()> {
    // Same-folder hard link is atomic and never clobbers. Fail closed on a
    // filesystem without hard links; a check-then-rename can overwrite.
    fs::hard_link(source, target).map_err(|e| {
        BackupError::refused(format!(
            "Refusing to overwrite or non-atomic backup placement: {e}"
        ))
    })?;
    if let Err(e) = paths::sync_dir(
        target
            .parent()
            .ok_or_else(|| BackupError::refused("Missing parent."))?,
    ) {
        let _ = fs::remove_file(target);
        return Err(e);
    }
    Ok(())
}

/// Copy one stored file into the work folder and authenticate it for every row that refers to it.
/// `Ok(Some(problem))` when the file itself is missing ([`MISSING`]), is not the attached file
/// ([`DAMAGED`]: not a regular file, wrong size, or content failing authentication) or opens under no key
/// these books hold ([`NO_KEY`]): the backup can go on without it. `Err` for anything else, such as a
/// permission error, too many open files, a disk error or a full disk: that says nothing about the file
/// and stops the backup, naming the cause.
fn stage_receipt(root: Option<&Path>, rows: &[ReceiptRef], staged: &Path, verifier: &ReceiptVerifier<'_>) -> Result<Option<&'static str>> {
    let Some(root) = root else { return Ok(Some(MISSING)) };
    let row = &rows[0];
    // The folder itself must be a plain path (no link along the way), as before.
    let source = paths::resolve(root)?.join(&row.stored_name);
    let stopped = |e: std::io::Error| BackupError::refused(format!("the attached file {} could not be read ({e}). Nothing was backed up. Check that FreedomBooks can read the book's “receipts” folder, then try again.", row.stored_name));
    match fs::symlink_metadata(&source) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Some(MISSING)),
        Err(e) => return Err(stopped(e)),
        // A link, folder or device in the file's place: what is there is not the attached file.
        Ok(m) if !m.file_type().is_file() => return Ok(Some(DAMAGED)),
        Ok(_) => {}
    }
    let expected = row
        .plain_bytes
        .checked_add(16 * row.plain_bytes.div_ceil(crypto::CHUNK as u64))
        .ok_or_else(|| BackupError::refused("Receipt size overflow."))?;
    // Never follow a link put in the file's place after the check above.
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32);
    }
    let mut src = match options.open(&source) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Some(MISSING)),
        #[cfg(unix)]
        Err(e) if e.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error()) => return Ok(Some(DAMAGED)),
        Err(e) => return Err(stopped(e)),
    };
    let meta = src.metadata().map_err(stopped)?;
    if !meta.is_file() {
        return Ok(Some(DAMAGED));
    }
    if meta.len() != expected {
        return Ok(Some(DAMAGED));
    }
    let mut dst = paths::new_file(staged)?;
    let mut buf = vec![0; 65536];
    let mut copied = 0u64;
    loop {
        let n = match src.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(stopped(e)),
        };
        copied += n as u64;
        if copied > expected {
            return Ok(Some(DAMAGED));
        }
        dst.write_all(&buf[..n])?;
    }
    dst.sync_all()?;
    drop(dst);
    if copied != expected {
        return Ok(Some(DAMAGED));
    }
    for r in rows {
        if let Err(e) = verifier(r, staged) {
            return match e.problem() {
                Some(problem) => Ok(Some(problem)),
                None => Err(e),
            };
        }
    }
    Ok(None)
}

/// Plain words for the owner: which files, and what to do.
pub fn incomplete_message(absent: &[Absent], folder_missing: bool) -> String {
    let label = |a: &Absent| {
        let problem = if a.problem == NO_KEY { "no key opens it" } else { a.problem.as_str() };
        format!("“{}” on {}{} ({problem})", a.name, a.document, if a.removed { ", already removed" } else { "" })
    };
    let files = absent.iter().map(label).collect::<Vec<_>>().join("; ");
    let count = absent.len();
    let keyless = absent.iter().filter(|a| a.problem == NO_KEY).count();
    let one = |n: usize| n == 1;
    let them = if one(count) { "it" } else { "them" };
    let older = format!("An older backup may still have {them}: restore that backup as a new book to get {them} back.");
    let key_file = "That happens when the books' key file (the “.keys.json” file next to the book file) was replaced or lost: copy it back from wherever these books came from, or restore a recent backup as a new book.";
    let records = "Your accounting records are not affected.";
    if folder_missing {
        let what = if one(count) { "1 attached file is".to_owned() } else { format!("{count} attached files are") };
        return format!("{what} missing or damaged in this book's attachment folder: {files}. The “receipts” folder that should sit next to the book file is missing. If these books were copied from somewhere else, copy that folder too, then back up again.");
    }
    if keyless == 0 {
        let what = if one(count) { "1 attached file is".to_owned() } else { format!("{count} attached files are") };
        return format!("{what} missing or damaged in this book's attachment folder: {files}. {older} {records}");
    }
    let what = if one(count) { "1 attached file".to_owned() } else { format!("{count} attached files") };
    if keyless == count {
        let (they, is) = if one(count) { ("It", "is") } else { ("They", "are") };
        return format!("{what} could not be backed up because none of the keys these books hold opens {them}: {files}. {they} {is} still in the attachment folder, unchanged. {key_file} {records}");
    }
    format!("{what} could not be backed up: {files}. A file marked missing or damaged is gone from, or changed in, this book's attachment folder; an older backup may still have it. A file marked “no key opens it” is still in the attachment folder, unchanged, but none of the keys these books hold opens it. {key_file} {records}")
}

pub fn create_backup(book: &Book, key: &Key, options: &BackupOptions<'_>) -> Result<BackupResult> {
    create_backup_from(book, key, options, None)
}

/// Desktop archives carry authenticated password/recovery wraps in their public header.
pub fn create_portable_backup(book: &Book, key: &Key, options: &BackupOptions<'_>, envelope: &portable::KeyEnvelope) -> Result<BackupResult> {
    create_backup_from(book, key, options, Some(envelope))
}

/// A backup of any snapshot source, such as a book opened before its upgrade. The manifest lists
/// the migrations the snapshot carries, so restore upgrades it like any older backup.
pub fn create_backup_from(source: &dyn fb_db::SnapshotSource, key: &Key, options: &BackupOptions<'_>, envelope: Option<&portable::KeyEnvelope>) -> Result<BackupResult> {
    create_with_keys(source, key, options, envelope, None)
}

fn create_with_keys(book: &dyn fb_db::SnapshotSource, key: &Key, options: &BackupOptions<'_>, envelope: Option<&portable::KeyEnvelope>, scheduled: Option<&scheduled::Slot>) -> Result<BackupResult> {
    if let Some(envelope) = envelope {
        envelope.validate()?;
        envelope.receipts(key)?;
    }
    if options.label.is_empty() {
        return Err(BackupError::refused("A backup label is required."));
    }
    if let Some(p) = options.passphrase {
        crypto::check_new_passphrase(p)?;
    }
    if options.keep == Some(0) {
        return Err(BackupError::refused("--keep must be at least 1."));
    }
    chrono::DateTime::parse_from_rfc3339(options.created_at)
        .map_err(|_| BackupError::refused("Invalid backup timestamp."))?;
    let target = paths::resolve(options.output)?;
    if target.try_exists()? {
        return Err(BackupError::refused(
            "Refusing to overwrite an existing backup.",
        ));
    }
    paths::writable_parent(&target)?;
    let parent = target
        .parent()
        .ok_or_else(|| BackupError::refused("Missing output folder."))?;
    let mut avoid = vec![parent];
    if let Some(r) = options.receipts_root {
        avoid.push(r);
    }
    let work = stage(options.work_root, &avoid)?;
    cleanup::cleanup_stale(parent)?;
    let lock_path = parent.join(".fb-backup.lock");
    paths::resolve(&lock_path)?;
    let lock = paths::lock_file(&lock_path)?;
    fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| {
        BackupError::refused("Another backup is running right now; try again when it finishes.")
    })?;
    let estimate = book
        .snapshot_size()?
        .saturating_mul(4)
        .saturating_add(1024 * 1024);
    paths::require_space(work.path(), estimate)?;
    paths::require_space(parent, estimate)?;
    let snap = work.path().join("book.fbbook");
    book.snapshot(&snap, key)?;
    let (contents, refs, labels) = Book::inspect_snapshot(&snap, key, |q| -> Result<_> {
        Ok((manifest::contents(q)?, receipt_refs(q)?, receipt_labels(q)))
    })?;
    let mut receipts: BTreeMap<String, Receipt> = BTreeMap::new();
    let mut absent: Vec<Absent> = Vec::new();
    if !refs.is_empty() {
        let verifier = options.verify_receipt.ok_or_else(|| {
            BackupError::refused(
                "The attachment module must authenticate receipt files before backup.",
            )
        })?;
        fs::create_dir(work.path().join("receipts"))?;
        // Rows sharing one stored file are authenticated together: the file goes in only when every row
        // that refers to it authenticates it.
        let mut files: BTreeMap<String, Vec<ReceiptRef>> = BTreeMap::new();
        for row in refs {
            validate_ref(&row)?;
            files.entry(row.stored_name.clone()).or_default().push(row);
        }
        for (stored, rows) in files {
            let entry = format!("receipts/{stored}");
            let staged = work.path().join(&entry);
            match stage_receipt(options.receipts_root, &rows, &staged, verifier)? {
                None => {
                    let (bytes, sha256) = hash_file(&staged)?;
                    receipts.insert(entry.clone(), Receipt { entry, bytes, sha256, owners: rows });
                }
                Some(problem) => {
                    if staged.try_exists()? {
                        fs::remove_file(&staged)?;
                    }
                    for row in rows {
                        let (name, document, removed) = labels.get(&row.id).cloned().unwrap_or_else(|| ("attachment".into(), "Document".into(), false));
                        absent.push(Absent { owner: row, name, document, removed, problem: problem.into() });
                    }
                }
            }
        }
        absent.sort_by(|a, b| a.owner.id.cmp(&b.owner.id));
        if !absent.is_empty() && !options.allow_incomplete {
            let mut e = BackupError::refused(incomplete_message(&absent, options.receipts_root.is_none()));
            e.absent = absent;
            return Err(e);
        }
    }
    // Inspection may enable WAL but does not modify data. Closing it leaves a
    // self-contained snapshot; hash only after the handle is closed.
    let (book_bytes, book_sha256) = hash_file(&snap)?;
    paths::require_space(
        parent,
        book_bytes.saturating_mul(3).saturating_add(1024 * 1024),
    )?;
    let version = if envelope.is_some() { FORMAT_VERSION } else { 3 };
    let data = Manifest { format: FORMAT.into(), format_version: version, created_at: options.created_at.into(), app_version: options.app_version.into(),
        migrations: book.migrations().iter().map(|(n, _)| (*n).into()).collect(), book_bytes, book_sha256, contents, attachments: receipts.into_values().collect(),
        not_included: vec!["OS key store, connection-sealing key, local app settings, environment files, and unwrapped passwords or recovery keys.".into()], absent };
    let inner = work.path().join("payload.tar");
    if options.passphrase.is_some() {
        payload(&inner, work.path(), &data)?;
    }
    let mut header = json!({"format":FORMAT,"format_version":version,"encrypted":options.passphrase.is_some(),"database":"SQLCipher"});
    if options.passphrase.is_some() {
        for (k, v) in crypto::new_parameters()
            .as_object()
            .ok_or_else(|| BackupError::refused("Invalid cipher parameters."))?
        {
            header[k] = v.clone();
        }
    }
    if options.passphrase.is_none() {
        header["manifest_nonce"] = json!(manifest::new_nonce());
    }
    // Public and authenticated (the header is the manifest's associated data): the Backups list can say
    // "incomplete" without opening the backup, and a restore checks it against the manifest.
    if !data.complete() {
        header["incomplete"] = json!(true);
    }
    if let Some(slot) = scheduled {
        // The encrypted manifest authenticates the scheduled identity along with the rest of the header.
        header["scheduled"] = slot.identity();
    }
    if let Some(envelope) = envelope {
        header["book_keys"] = serde_json::to_value(envelope)?;
    }
    let raw = json_bytes(&header)?;
    let encrypted = work.path().join("payload.enc");
    if let Some(pass) = options.passphrase {
        crypto::encrypt_stream(
            &mut File::open(&inner)?,
            &mut paths::new_file(&encrypted)?,
            pass,
            &header,
            &raw,
        )?;
    }
    let partial = tempfile::Builder::new()
        .prefix(&cleanup::prefix())
        .suffix(".partial")
        .tempfile_in(parent)?;
    let mut tar = tar::Builder::new(partial.as_file());
    append_bytes(&mut tar, "header.json", &raw)?;
    if options.passphrase.is_some() {
        append_file(&mut tar, "payload.enc", &encrypted)?;
    } else {
        let sealed = manifest::encrypt(&json_bytes(&data)?, key, &header, &raw)?;
        append_bytes(&mut tar, "manifest.enc", &sealed)?;
        append_file(&mut tar, "book.fbbook", &snap)?;
        for r in &data.attachments {
            append_file(&mut tar, &r.entry, &work.path().join(&r.entry))?;
        }
    }
    tar.into_inner()?.sync_all()?;
    publish(partial.path(), &target)?;
    // An incomplete backup never replaces or prunes anything.
    let pruned = if let Some(keep) = options.keep.filter(|_| data.complete()) {
        retention::prune(
            parent,
            &retention::safe_db_label(options.label),
            keep,
            Some(&target),
        )?
    } else {
        Vec::new()
    };
    Ok(BackupResult {
        path: target,
        manifest: data,
        pruned,
    })
}

fn archive_io(e: std::io::Error) -> BackupError {
    use std::io::ErrorKind;
    // tar reports malformed headers and truncated blocks as synthetic I/O errors.
    if e.raw_os_error().is_none() && matches!(e.kind(), ErrorKind::Other | ErrorKind::InvalidData | ErrorKind::UnexpectedEof) {
        BackupError::refused("Not a FreedomBooks backup (not a readable archive).")
    } else {
        e.into()
    }
}

/// Strict flat extraction, never tar::unpack. Entry names, types, duplicates,
/// counts and bounded metadata are checked before any path is constructed.
fn extract(archive: &Path, work: &Path, outer: bool, data: Option<&Manifest>) -> Result<BTreeSet<String>> {
    let file = paths::regular(archive)?;
    if file.metadata()?.len() > restore_limits::ABSOLUTE {
        return Err(BackupError::refused("Backup size exceeds the restore limit."));
    }
    let mut tar = tar::Archive::new(file);
    let mut total = 0u64;
    let mut seen = BTreeSet::new();
    for item in tar.entries_with_seek().map_err(archive_io)?.raw(true) {
        let mut item = item.map_err(archive_io)?;
        let raw = item.path_bytes();
        let name = std::str::from_utf8(&raw)
            .map_err(|_| BackupError::refused("Invalid archive entry."))?
            .to_string();
        let receipt = name.strip_prefix("receipts/").is_some_and(valid_stored);
        if !item.header().entry_type().is_file()
            || !(matches!(
                name.as_str(),
                "manifest.json" | "manifest.enc" | "book.fbbook"
            ) || receipt
                || (outer && matches!(name.as_str(), "header.json" | "payload.enc")))
            || !seen.insert(name.clone())
        {
            return Err(BackupError::refused(
                "Not a FreedomBooks backup: unexpected or duplicate archive entry.",
            ));
        }
        if seen.len() > 1_000_000 {
            return Err(BackupError::refused("Too many archive entries."));
        }
        let size = item.size();
        let cap = match name.as_str() {
            "header.json" => 65536,
            "manifest.json" | "manifest.enc" => 64 * 1024 * 1024,
            _ => restore_limits::ABSOLUTE,
        };
        if size > cap {
            return Err(BackupError::refused("Backup metadata is too large."));
        }
        total = total.checked_add(size).filter(|n| *n <= restore_limits::ABSOLUTE)
            .ok_or_else(|| BackupError::refused("Backup size exceeds the restore limit."))?;
        let metadata = matches!(name.as_str(), "header.json" | "manifest.json" | "manifest.enc");
        if let Some(data) = data {
            if total > restore_limits::budget(data)? { return Err(BackupError::refused("Backup size exceeds the restore limit.")); }
            if metadata { continue; }
            let expected = if name == "book.fbbook" { Some(data.book_bytes) }
                else { data.attachments.iter().find(|r| r.entry == name).map(|r| r.bytes) };
            if expected != Some(size) { return Err(BackupError::refused("Backup entry size does not match its manifest.")); }
        } else if !metadata { continue; }
        paths::require_space(work, size.saturating_add(1024 * 1024))?;
        if receipt {
            fs::create_dir_all(work.join("receipts"))?;
        }
        let mut dst = paths::new_file(&work.join(name))?;
        if std::io::copy(&mut item, &mut dst)? != size {
            return Err(BackupError::refused("Truncated backup entry."));
        }
        dst.sync_all()?;
    }
    Ok(seen)
}

fn validate_manifest(data: &Manifest) -> Result<()> {
    if data.format != FORMAT
        || !matches!(data.format_version, 3 | 4)
        || data.migrations.is_empty()
        || !fb_db::supported_migrations(&data.migrations)
    {
        return Err(BackupError::refused(
            "This backup was made by a NEWER or incompatible FreedomBooks. Upgrade FreedomBooks first. Nothing was restored.",
        ));
    }
    if data.book_bytes == 0
        || !digest_valid(&data.book_sha256)
        || data.contents.tables.is_empty()
        || data.contents.schema.is_empty()
    {
        return Err(BackupError::refused("Invalid backup manifest."));
    }
    let mut entries = BTreeSet::new();
    let mut owners = BTreeSet::new();
    for r in &data.attachments {
        if !r.entry.strip_prefix("receipts/").is_some_and(valid_stored)
            || !entries.insert(&r.entry)
            || r.bytes == 0
            || !digest_valid(&r.sha256)
            || r.owners.is_empty()
        {
            return Err(BackupError::refused("Invalid attachment manifest."));
        }
        for row in &r.owners {
            validate_ref(row)?;
            if r.entry != format!("receipts/{}", row.stored_name) || !owners.insert(&row.id) {
                return Err(BackupError::refused(
                    "Duplicate or mismatched attachment owner.",
                ));
            }
        }
    }
    for a in &data.absent {
        validate_ref(&a.owner)?;
        if !matches!(a.problem.as_str(), MISSING | DAMAGED | NO_KEY)
            || a.name.is_empty() || a.name.len() > 1024 || a.document.is_empty() || a.document.len() > 1024
            || entries.contains(&format!("receipts/{}", a.owner.stored_name))
            || !owners.insert(&a.owner.id)
        {
            return Err(BackupError::refused("Invalid list of missing attachments."));
        }
    }
    Ok(())
}

fn unpack(
    path: &Path,
    work: &Path,
    key: &Key,
    passphrase: Option<&str>,
    require_encrypted: bool,
    verify_receipt: Option<&ReceiptVerifier<'_>>,
) -> Result<Manifest> {
    let members = extract(path, work, true, None)?;
    if !members.contains("header.json") {
        return Err(BackupError::refused("Not a FreedomBooks backup: no header.json."));
    }
    let raw = fs::read(work.join("header.json"))?;
    let header = parse_json(&raw)?;
    if header["format"] != FORMAT
        || !matches!(header["format_version"].as_u64(), Some(3 | 4))
        || header["database"] != "SQLCipher"
        || !header["encrypted"].is_boolean()
    {
        return Err(BackupError::refused(
            "Unsupported FreedomBooks backup format. PostgreSQL formats 1/2 need the reference importer.",
        ));
    }
    let encrypted = header["encrypted"] == true;
    if require_encrypted && !encrypted {
        return Err(BackupError::refused(
            "An encrypted portable backup is required.",
        ));
    }
    let active = if encrypted {
        if members != BTreeSet::from(["header.json".into(), "payload.enc".into()]) {
            return Err(BackupError::refused("Unexpected encrypted backup entry."));
        }
        let pass = passphrase.ok_or_else(|| {
            BackupError::refused("This backup is encrypted: a passphrase is required.")
        })?;
        let inner = work.join("payload.tar");
        // Stream ciphertext directly from the archive; only bounded, authenticated
        // plaintext reaches a temporary file, removed automatically on any error.
        let mut tar = tar::Archive::new(paths::regular(path)?);
        let mut dst = restore_limits::Payload::new(work)?;
        for entry in tar.entries_with_seek().map_err(archive_io)?.raw(true) {
            let mut entry = entry.map_err(archive_io)?;
            if entry.path_bytes().as_ref() == b"payload.enc" {
                crypto::decrypt_stream(&mut entry, &mut dst, pass, &header, &raw)?;
            }
        }
        dst.publish(&inner)?;
        extract(&inner, work, false, None)?
    } else {
        let mut p = members;
        if p.contains("manifest.json") || !p.remove("manifest.enc") {
            return Err(BackupError::refused(
                "A local backup requires an encrypted manifest.",
            ));
        }
        let encrypted = fs::read(work.join("manifest.enc"))?;
        let plain = manifest::decrypt(&encrypted, key, &header, &raw)?;
        paths::new_file(&work.join("manifest.json"))?.write_all(&plain)?;
        p.insert("manifest.json".into());
        p.remove("header.json");
        p
    };
    let raw = fs::read(work.join("manifest.json"))?;
    let data: Manifest = serde_json::from_value(parse_json(&raw)?)?;
    validate_manifest(&data)?;
    if !(header.get("incomplete").is_none() && data.complete() || header["incomplete"] == true && !data.complete()) {
        return Err(BackupError::refused("The backup's incomplete flag does not match its manifest."));
    }
    restore_limits::budget(&data)?;
    let inner_path = work.join("payload.tar");
    extract(if encrypted { &inner_path } else { path }, work, !encrypted, Some(&data))?;
    let expected: BTreeSet<String> = ["manifest.json".to_string(), "book.fbbook".into()]
        .into_iter()
        .chain(data.attachments.iter().map(|r| r.entry.clone()))
        .collect();
    if active != expected {
        return Err(BackupError::refused(
            "The attachment files in the backup do not match its manifest.",
        ));
    }
    if hash_file(&work.join("book.fbbook"))? != (data.book_bytes, data.book_sha256.clone()) {
        return Err(BackupError::refused(
            "The book does not match the SHA-256 recorded in the manifest. Nothing was restored.",
        ));
    }
    for r in &data.attachments {
        if hash_file(&work.join(&r.entry))? != (r.bytes, r.sha256.clone()) {
            return Err(BackupError::refused(
                "Attachment SHA-256 or size mismatch. Nothing was restored.",
            ));
        }
    }
    let (actual, refs, legacy_refs, version, installed) =
        Book::inspect_snapshot_with_history(&work.join("book.fbbook"), key, |q, history| -> Result<_> {
            let refs = receipt_refs(q)?;
            // The first attachment-backup implementation replaced the own-wrap fields with the newest
            // audit wrap, without a rewraps array. Only an authenticated pre-table history admits that
            // exact projection; all owner metadata and every other manifest field still compare exactly.
            let legacy = if history.iter().any(|(name, _)| *name == "0197_attachment_key_wraps") { None } else {
                let mut old = refs.clone();
                for row in &mut old {
                    if let Some(wrap) = crate::attachments::legacy_wraps(q, &row.stored_name).map_err(|e| BackupError::refused(e.to_string()))?.into_iter().next() {
                        row.wrapped_key = wrap.wrapped_key;
                        row.wrap_nonce = wrap.wrap_nonce;
                    }
                    row.rewraps.clear();
                }
                Some(old)
            };
            Ok((
                manifest::contents(q)?,
                refs,
                legacy,
                q.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
                history.iter().map(|(name,_)| (*name).to_owned()).collect::<Vec<_>>(),
            ))
        })?;
    let mut listed: Vec<_> = data
        .attachments
        .iter()
        .flat_map(|r| r.owners.clone())
        .chain(data.absent.iter().map(|a| a.owner.clone()))
        .collect();
    listed.sort_by(|a, b| a.id.cmp(&b.id));
    if actual != data.contents || (refs != listed && legacy_refs.as_ref() != Some(&listed)) || version != data.migrations.len() as i64 || installed != data.migrations {
        return Err(BackupError::refused(
            "The restored database does NOT match the backup manifest. Nothing was restored.",
        ));
    }
    if !data.attachments.is_empty() {
        let verifier = verify_receipt.ok_or_else(|| {
            BackupError::refused("The attachment module must authenticate restored receipt files.")
        })?;
        for file in &data.attachments {
            for row in &file.owners {
                verifier(row, &work.join(&file.entry))?;
            }
        }
    }
    Ok(data)
}

pub fn restore_backup(
    path: &Path,
    key: &Key,
    options: &RestoreOptions<'_>,
) -> Result<RestoreResult> {
    let target = paths::resolve(options.target)?;
    if target.try_exists()? {
        return Err(BackupError::refused(
            "Refusing to restore into an existing book. Restore only goes into a NEW file.",
        ));
    }
    paths::writable_parent(&target)?;
    paths::refuse_sidecars(&target)?;
    let mut protected = options.protected.to_vec();
    protected.push(path.to_owned());
    protected.push(options.work_root.to_owned());
    for p in &protected {
        if paths::overlap(&target, p)? {
            return Err(BackupError::refused(
                "Restore target overlaps a configured or protected path.",
            ));
        }
    }
    if let Some(receipts) = options.receipts_target {
        let r = paths::resolve(receipts)?;
        paths::new_receipts_dir(&r)?;
        paths::writable_parent(&r)?;
        for p in protected.iter().chain(std::iter::once(&target)) {
            if paths::overlap(&r, p)? {
                return Err(BackupError::refused(
                    "Restore receipts overlap a protected path.",
                ));
            }
        }
    }
    let mut avoid = vec![target.as_path(), path];
    if let Some(r) = options.receipts_target {
        avoid.push(r);
    }
    let work = stage(options.work_root, &avoid)?;
    let data = unpack(
        path,
        work.path(),
        key,
        options.passphrase,
        options.require_encrypted,
        options.verify_receipt,
    )?;
    if !data.attachments.is_empty() && options.receipts_target.is_none() {
        return Err(BackupError::refused(
            "This backup holds attachment files. Give a NEW folder for them. Nothing was restored.",
        ));
    }
    let total = data
        .attachments
        .iter()
        .try_fold(data.book_bytes, |a, r| a.checked_add(r.bytes))
        .ok_or_else(|| BackupError::refused("Restore size overflow."))?;
    paths::require_space(&target, total.saturating_mul(2))?;
    if let Some(r) = options.receipts_target {
        paths::require_space(r, total)?;
    }
    // Verify current data invariants after authenticating the old manifest; any
    // upgrade happens only in the private staged copy.
    let upgraded = data.migrations.len() != fb_db::MIGRATIONS.len();
    let parent = target
        .parent()
        .ok_or_else(|| BackupError::refused("Missing parent."))?;
    let restored = Book::open(&work.path().join("book.fbbook"), key)?;
    let partial = restore_partial(&restored, key, parent)?;
    drop(restored);
    let mut receipts_created = false;
    let mut receipts_written = Vec::new();
    let mut target_created = false;
    let result = (|| {
        if let Some(folder) = options.receipts_target {
            #[cfg_attr(not(unix), allow(unused_mut))]
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            paths::new_receipts_dir(folder)?;
            if !folder.try_exists()? {
                builder.create(folder)?;
                receipts_created = true;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(folder, fs::Permissions::from_mode(0o700))?;
            }
            for r in &data.attachments {
                let stored = r
                    .entry
                    .strip_prefix("receipts/")
                    .ok_or_else(|| BackupError::refused("Invalid receipt entry."))?;
                let dest = folder.join(stored);
                let mut dst = paths::new_file(&dest)?;
                receipts_written.push(dest.clone());
                let mut src = File::open(work.path().join(&r.entry))?;
                std::io::copy(&mut src, &mut dst)?;
                dst.sync_all()?;
                if hash_file(&dest)? != (r.bytes, r.sha256.clone()) {
                    return Err(BackupError::refused("Attachment did not copy correctly."));
                }
            }
            paths::sync_dir(folder)?;
        }
        publish(partial.path(), &target)?;
        target_created = true;
        // Publication must not retain a second filename while the book is admitted:
        // recovery journals belong to one pathname. The target is already durably linked,
        // so retire only this operation's staging name before the normal open checks.
        partial.close()?;
        paths::sync_dir(parent)?;
        let reopened = Book::open(&target, key)?;
        drop(reopened);
        paths::sync_dir(parent)?;
        Ok(())
    })();
    if let Err(e) = result {
        if target_created {
            let _ = fs::remove_file(&target);
        }
        for path in receipts_written {
            let _ = fs::remove_file(path);
        }
        if receipts_created && let Some(folder) = options.receipts_target {
            let _ = fs::remove_dir(folder);
        }
        return Err(e);
    }
    Ok(RestoreResult {
        path: target,
        manifest: data,
        upgraded,
    })
}

/// Verify in a throwaway private folder; no live/configured paths are changed.
pub fn verify_backup(
    path: &Path,
    key: &Key,
    work_root: &Path,
    passphrase: Option<&str>,
    record: bool,
) -> Result<Manifest> {
    verify_backup_with_receipts(path, key, work_root, passphrase, record, None)
}

pub fn verify_backup_with_receipts(
    path: &Path,
    key: &Key,
    work_root: &Path,
    passphrase: Option<&str>,
    record: bool,
    verify_receipt: Option<&ReceiptVerifier<'_>>,
) -> Result<Manifest> {
    verify_backup_with_options(
        path,
        key,
        &VerifyOptions {
            work_root,
            passphrase,
            record,
            verify_receipt,
            money_check: None,
        },
    )
}

/// Run optional independent money checks after native validation and staged
/// migration, before recording success. The callback cannot modify either book.
pub fn verify_backup_with_options(
    path: &Path,
    key: &Key,
    options: &VerifyOptions<'_>,
) -> Result<Manifest> {
    let checked = check_backup(path, key, options, None)?;
    if let Some(summary) = checked.summary {
        retention::record_verified_matching(path, &checked.fingerprint, summary)?;
    }
    Ok(checked.manifest)
}

struct CheckedBackup {
    manifest: Manifest,
    fingerprint: (u64, String),
    summary: Option<Value>,
}

fn check_backup(
    path: &Path,
    key: &Key,
    options: &VerifyOptions<'_>,
    scheduled: Option<(&scheduled::Slot, &[String])>,
) -> Result<CheckedBackup> {
    let work = stage(options.work_root, &[path])?;
    // Verify a private, immutable copy. The eventual sidecar is bound to these
    // exact bytes, even if another process replaces the original during verify.
    let input = work.path().join("input.fbbackup");
    let mut source = paths::regular(path)?;
    paths::require_space(work.path(), source.metadata()?.len().saturating_mul(3))?;
    let mut copy = paths::new_file(&input)?;
    std::io::copy(&mut source, &mut copy)?;
    copy.sync_all()?;
    drop(copy);
    let fingerprint = hash_file(&input)?;
    let data = unpack(
        &input,
        work.path(),
        key,
        options.passphrase,
        false,
        options.verify_receipt,
    )?;
    let mut book = Book::open(&work.path().join("book.fbbook"), key)?;
    let lifecycle = if let Some(check) = options.money_check {
        book.read(|q| check(q, &data))?
    } else {
        json!({"run":false,"reason":"L4-LIFECYCLE-VERIFY-PORT: independent lifecycle verifier awaits integration"})
    };
    drop(book);
    let summary = if options.record || scheduled.is_some() {
        let header = parse_json(&fs::read(work.path().join("header.json"))?)?;
        if let Some((slot, companies)) = scheduled {
            slot.check_capture(&header, &data, companies)?;
        }
        let attachment_bytes = data
            .attachments
            .iter()
            .try_fold(0u64, |n, a| n.checked_add(a.bytes))
            .ok_or_else(|| BackupError::refused("Attachment size overflow."))?;
        Some(json!({
            "verified_at":chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "backup_created_at":data.created_at,
            "company_tags":data.contents.companies.iter().map(|c| retention::company_tag(key, &c.id)).collect::<Result<Vec<_>>>()?,
            "file":path.file_name().and_then(|n|n.to_str()),
            "encrypted":header["encrypted"],
            "companies":data.contents.companies.len(),
            "tables":data.contents.tables.len(),
            "needs_migrate":data.migrations.len() != fb_db::MIGRATIONS.len(),
            "app_version":data.app_version,
            "lifecycle":lifecycle,
            "attachments":data.attachments.len(),
            "attachment_bytes":attachment_bytes,
            "complete":data.complete(),
            "missing_attachments":data.absent.len(),
        }))
    } else {
        None
    };
    Ok(CheckedBackup { manifest: data, fingerprint, summary })
}

fn restore_partial(book: &Book, key: &Key, parent: &Path) -> Result<tempfile::NamedTempFile> {
    let partial = tempfile::Builder::new()
        .prefix(&cleanup::prefix())
        .suffix(".partial")
        .tempfile_in(parent)?;
    // snapshot requires an exclusive NEW destination. Keep the tempfile guard
    // while it writes so every failure still removes this operation's file.
    fs::remove_file(partial.path())?;
    book.snapshot(partial.path(), key)?;
    paths::refuse_sidecars(partial.path())?;
    // The original tempfile descriptor referred to the unlinked empty file;
    // snapshot fsyncs the replacement itself before returning.
    Ok(partial)
}

#[cfg(test)]
mod review_publication_tests {
    use super::*;
    #[test]
    fn restore_partial_uses_cleanup_name_and_dead_pid_copy_is_removed() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("source.fbbook");
        let book = Book::create(&src, &[42; 32]).unwrap();
        let partial = restore_partial(&book, &[42; 32], d.path()).unwrap();
        let name = partial.path().file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with(&cleanup::prefix()) && name.ends_with(".partial"),
            "{name}"
        );
        // Positive pid_t well above Linux/macOS PID_MAX so kill(pid, 0) returns ESRCH.
        let dead = d.path().join(name.replacen(
            &format!(".{}.", std::process::id()),
            &format!(".{}.", i32::MAX),
            1,
        ));
        fs::copy(partial.path(), &dead).unwrap();
        assert_eq!(
            cleanup::cleanup_stale(d.path()).unwrap(),
            vec![dead.clone()]
        );
        assert!(!dead.exists());
        assert!(partial.path().exists());
    }
    #[test]
    fn restore_publication_includes_committed_wal_and_is_self_contained() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("source.fbbook");
        let mut book = Book::create(&src, &[42; 32]).unwrap();
        book.connection()
            .execute_batch("PRAGMA wal_autocheckpoint=0")
            .unwrap();
        crate::users::create_user(&mut book, "In WAL", true).unwrap();
        let partial = restore_partial(&book, &[42; 32], d.path()).unwrap();
        paths::refuse_sidecars(partial.path()).unwrap();
        let restored = Book::open(partial.path(), &[42; 32]).unwrap();
        let n: i64 = restored
            .connection()
            .query_row(
                "SELECT count(*) FROM fb_user WHERE username='In WAL'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "published main file omitted committed WAL pages");
    }
}
