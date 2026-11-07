//! SQLCipher book + encrypted receipts. Format 3 deliberately cannot be confused
//! with Python's PostgreSQL formats 1/2. Optional portable-export encryption keeps
//! Python's exact stream cipher; ordinary local backups need no extra passphrase.
pub mod audit;
pub mod cleanup;
pub mod crypto;
pub mod manifest;
mod paths;
mod restore_limits;
pub mod recovery;
pub mod portable;
pub mod retention;

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
pub struct BackupError(String);
impl BackupError {
    pub fn refused(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}
impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for BackupError {}
#[cfg(unix)]
impl From<rustix::io::Errno> for BackupError {
    fn from(e: rustix::io::Errno) -> Self {
        Self(format!("Backup path refused: {e}"))
    }
}
impl From<std::io::Error> for BackupError {
    fn from(e: std::io::Error) -> Self {
        Self(format!("Backup I/O failed: {e}"))
    }
}
impl From<fb_db::DbError> for BackupError {
    fn from(e: fb_db::DbError) -> Self {
        Self(format!("Book verification failed: {e}"))
    }
}
impl From<fb_db::rusqlite::Error> for BackupError {
    fn from(e: fb_db::rusqlite::Error) -> Self {
        Self(format!("Manifest query failed: {e}"))
    }
}
impl From<serde_json::Error> for BackupError {
    fn from(_: serde_json::Error) -> Self {
        Self("Malformed backup manifest.".into())
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
    pub wrapped_key: String,
    pub wrap_nonce: String,
    pub nonce: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub entry: String,
    pub bytes: u64,
    pub sha256: String,
    pub owners: Vec<ReceiptRef>,
}
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
}

/// Supplied by the attachment module. It must authenticate the staged ciphertext
/// against its owning row (AEAD, plaintext hash and size). No filesystem writes.
pub type ReceiptVerifier<'a> = dyn Fn(&ReceiptRef, &Path) -> Result<()> + 'a;
/// The attachment module's [`ReceiptVerifier`] (L4-BACKUPS integration request 1), for
/// `BackupOptions`, `RestoreOptions` and `verify_backup_with_receipts`: each staged ciphertext is
/// authenticated against its owning row's immutable snapshot under the book's master key, without the live book.
pub fn attachment_receipt_verifier(key: &Key) -> impl Fn(&ReceiptRef, &Path) -> Result<()> + '_ {
    move |r, staged| {
        let refused = || BackupError::refused("A receipt file does not match its attachment record.");
        let company = crate::CompanyId::parse(&r.company).ok_or_else(refused)?;
        let stored = crate::attachments::storage::Stored {
            stored_name: r.stored_name.clone(),
            content_type: r.content_type.clone(),
            size: i64::try_from(r.plain_bytes).map_err(|_| refused())?,
            sha256: r.plain_sha256.clone(),
            original_sha256: r.original_sha256.clone(),
            wrapped_key: r.wrapped_key.clone(),
            wrap_nonce: r.wrap_nonce.clone(),
            nonce: r.nonce.clone(),
        };
        crate::attachments::storage::verify_staged(key, company, &stored, staged).map_err(|_| refused())
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
    let mut f = paths::regular(path)?;
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
    let file = paths::regular(path)?;
    let len = file.metadata()?.len();
    let mut tar = tar::Archive::new(file);
    let mut header = None;
    let unreadable = |_| BackupError::refused("Not a FreedomBooks backup (not a readable archive).");
    for item in tar.entries_with_seek().map_err(unreadable)?.raw(true) {
        let mut item = item.map_err(unreadable)?;
        let end = item.raw_file_position().checked_add(item.size());
        if end.is_none_or(|end| end > len) {
            return Err(BackupError::refused("This backup file is incomplete (cut short)."));
        }
        if item.path_bytes().as_ref() == b"header.json" {
            if header.is_some() || !item.header().entry_type().is_file() || item.size() > 65536 {
                return Err(BackupError::refused("Not a FreedomBooks backup: invalid header.json."));
            }
            let mut raw = Vec::new();
            item.read_to_end(&mut raw).map_err(unreadable)?;
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
    Ok(q.prepare(
        "SELECT id,company_id,stored_name,size,sha256,original_sha256,content_type,wrapped_key,wrap_nonce,nonce FROM attachments_attachment ORDER BY id",
    )?
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
        })
    })?
    .collect::<std::result::Result<_, _>>()?)
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
    cleanup::cleanup_stale(&root)?;
    Ok(tempfile::Builder::new()
        .prefix(&cleanup::prefix())
        .tempdir_in(root)?)
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

pub fn create_backup(book: &Book, key: &Key, options: &BackupOptions<'_>) -> Result<BackupResult> {
    create_with_keys(book, key, options, None)
}

/// Desktop archives carry authenticated password/recovery wraps in their public header.
pub fn create_portable_backup(book: &Book, key: &Key, options: &BackupOptions<'_>, envelope: &portable::KeyEnvelope) -> Result<BackupResult> {
    create_backup_from(book, key, options, Some(envelope))
}

/// A backup of any snapshot source, such as a book opened before its upgrade. The manifest lists
/// the migrations the snapshot carries, so restore upgrades it like any older backup.
pub fn create_backup_from(source: &dyn fb_db::SnapshotSource, key: &Key, options: &BackupOptions<'_>, envelope: Option<&portable::KeyEnvelope>) -> Result<BackupResult> {
    if let Some(envelope) = envelope {
        envelope.validate()?;
        envelope.receipts(key)?;
    }
    create_with_keys(source, key, options, envelope)
}

fn create_with_keys(book: &dyn fb_db::SnapshotSource, key: &Key, options: &BackupOptions<'_>, envelope: Option<&portable::KeyEnvelope>) -> Result<BackupResult> {
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
    let (contents, refs) = Book::inspect_snapshot(&snap, key, |q| -> Result<_> {
        Ok((manifest::contents(q)?, receipt_refs(q)?))
    })?;
    let mut receipts: BTreeMap<String, Receipt> = BTreeMap::new();
    if !refs.is_empty() {
        let root = options.receipts_root.ok_or_else(|| {
            BackupError::refused("A receipts folder is required for this backup.")
        })?;
        let verifier = options.verify_receipt.ok_or_else(|| {
            BackupError::refused(
                "The attachment module must authenticate receipt files before backup.",
            )
        })?;
        fs::create_dir(work.path().join("receipts"))?;
        for row in refs {
            validate_ref(&row)?;
            let entry = format!("receipts/{}", row.stored_name);
            if let Some(existing) = receipts.get_mut(&entry) {
                verifier(&row, &work.path().join(&entry))?;
                existing.owners.push(row);
                continue;
            }
            let source = root.join(&row.stored_name);
            let staged = work.path().join(&entry);
            let mut src = paths::regular(&source)?;
            let mut dst = paths::new_file(&staged)?;
            let expected = row
                .plain_bytes
                .checked_add(16 * row.plain_bytes.div_ceil(crypto::CHUNK as u64))
                .ok_or_else(|| BackupError::refused("Receipt size overflow."))?;
            if src.metadata()?.len() != expected {
                return Err(BackupError::refused(
                    "Receipt file size does not match its owning row.",
                ));
            }
            std::io::copy(&mut src, &mut dst)?;
            dst.sync_all()?;
            verifier(&row, &staged)?;
            let (bytes, sha256) = hash_file(&staged)?;
            receipts.insert(
                entry.clone(),
                Receipt {
                    entry,
                    bytes,
                    sha256,
                    owners: vec![row],
                },
            );
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
        not_included: vec!["OS key store, connection-sealing key, local app settings, environment files, and unwrapped passwords or recovery keys.".into()] };
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
    let pruned = if let Some(keep) = options.keep {
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
    for item in tar.entries_with_seek()?.raw(true) {
        let mut item = item?;
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
        for entry in tar.entries_with_seek()?.raw(true) {
            let mut entry = entry?;
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
    let (actual, refs, version, installed) =
        Book::inspect_snapshot_with_history(&work.join("book.fbbook"), key, |q, history| -> Result<_> {
            Ok((
                manifest::contents(q)?,
                receipt_refs(q)?,
                q.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
                history.iter().map(|(name,_)| (*name).to_owned()).collect::<Vec<_>>(),
            ))
        })?;
    let mut listed: Vec<_> = data
        .attachments
        .iter()
        .flat_map(|r| r.owners.clone())
        .collect();
    listed.sort_by(|a, b| a.id.cmp(&b.id));
    if actual != data.contents || refs != listed || version != data.migrations.len() as i64 || installed != data.migrations {
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
    if options.record {
        let header = parse_json(&fs::read(work.path().join("header.json"))?)?;
        let attachment_bytes = data
            .attachments
            .iter()
            .try_fold(0u64, |n, a| n.checked_add(a.bytes))
            .ok_or_else(|| BackupError::refused("Attachment size overflow."))?;
        let summary = json!({
            "verified_at":chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "file":path.file_name().and_then(|n|n.to_str()),
            "encrypted":header["encrypted"],
            "companies":data.contents.companies.len(),
            "tables":data.contents.tables.len(),
            "needs_migrate":data.migrations.len() != fb_db::MIGRATIONS.len(),
            "app_version":data.app_version,
            "lifecycle":lifecycle,
            "attachments":data.attachments.len(),
            "attachment_bytes":attachment_bytes,
        });
        retention::record_verified_matching(path, &fingerprint, summary)?;
    }
    Ok(data)
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
        let dead =
            d.path()
                .join(name.replacen(&format!(".{}.", std::process::id()), ".4294967295.", 1));
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
