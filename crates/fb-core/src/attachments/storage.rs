//! Book-local encrypted receipt store. Directory handles and relative no-follow opens prevent traversal.
//! Keys never implement Debug or Serialize.
//!
//! Each file has its own random content key; the attachment row keeps that key wrapped under a key derived
//! from the book's receipt key (`KeyEnvelope::receipts`), which survives password changes. Files attached
//! through the desktop chooser before November 2026 were wrapped under the book's master key instead.
//! Those wraps are still accepted for reading and verification ([`Store::with_legacy`],
//! [`verify_staged_any`]), and [`rewrap`] makes a receipt-key wrap of the same content key, kept as a
//! further wrap of that stored file (`attachments_keywrap`, see `super::rewrap_legacy`), before the master
//! key can change.
//!
//! A file can therefore have several wraps: its row's own, and each recorded further wrap
//! ([`Stored::rewraps`], newest first). Readers try every wrap under every key the book holds; the file
//! is unreadable only if none opens. Why it is unreadable is kept apart ([`ReadError`]): no file, a file
//! whose content is damaged (wrong size, or the content fails authentication under a key that did open
//! its wrap), no key these books hold opens any of its wraps (the file may be fine; the key is missing),
//! or a system error reading it.
use super::*;
use chacha20poly1305::aead::stream::{DecryptorBE32, EncryptorBE32};
use chacha20poly1305::{
    ChaCha20Poly1305, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use hkdf::Hkdf;
#[cfg(unix)]
use rustix::fs::{Mode, OFlags, open, openat};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

#[cfg(not(any(unix, windows)))]
compile_error!("The attachment store needs a Unix or Windows backend.");
/// Windows backend: handle-relative NT opens, reparse-point refusal, owner-only DACLs. See the module docs.
#[cfg(windows)]
mod windows;

pub struct Store {
    folder: PathBuf,
    directory: File,
    wrapping_key: Zeroizing<[u8; 32]>,
    /// Accepted for reading only: the book's master key, which wrapped files attached before the
    /// receipt-key fix. New files are always wrapped under `wrapping_key`.
    legacy_key: Option<Zeroizing<[u8; 32]>>,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Stored {
    pub stored_name: String,
    pub content_type: String,
    pub size: i64,
    pub sha256: String,
    pub original_sha256: String,
    /// The attachment row's own (immutable) wrap of the content key.
    pub wrapped_key: String,
    pub wrap_nonce: String,
    pub nonce: String,
    /// Further wraps of the same content key recorded for this stored file, newest first. Never part of
    /// the row; filled in by the attachment module when it reads one.
    #[serde(skip)]
    pub rewraps: Vec<Wrap>,
}
/// One wrap of a file's content key (hex, as stored).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wrap {
    pub wrapped_key: String,
    pub wrap_nonce: String,
}
impl Stored {
    /// Every wrap of this file's content key, in the order readers try them: each further wrap newest
    /// first, then the row's own.
    pub fn wraps(&self) -> impl Iterator<Item = (&str, &str)> {
        self.rewraps
            .iter()
            .map(|w| (w.wrapped_key.as_str(), w.wrap_nonce.as_str()))
            .chain(std::iter::once((self.wrapped_key.as_str(), self.wrap_nonce.as_str())))
    }
}

/// Why an attached file could not be read. Only [`ReadError::Missing`], [`ReadError::Damaged`] and
/// [`ReadError::NoKey`] are about the file; [`ReadError::System`] is about this computer (permissions,
/// too many open files, a disk error) and says nothing about the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    /// No file of that name in the attachment folder.
    Missing,
    /// The file is there but its content is not the attached file: wrong size, not a regular file, or
    /// the content fails authentication under a key that does open its wrap.
    Damaged,
    /// None of the keys these books hold opens any wrap of the file's key. The file itself may be intact.
    NoKey,
    /// The stored metadata is not a valid attachment reference.
    Invalid,
    /// Reading failed for a reason that is not about the file.
    System(String),
}
/// The words for each [`ReadError`], as `FbError::Refused` messages (all start "Attachment store:").
pub const MISSING_MESSAGE: &str = "Attachment store: this attached file is missing from the book's attachment folder.";
pub const DAMAGED_MESSAGE: &str = "Attachment store: this attached file is damaged: its content no longer matches what was attached.";
pub const NO_KEY_MESSAGE: &str = "Attachment store: none of the keys these books hold opens this attached file. The file itself may be intact; the key that opens it is missing from these books.";
impl ReadError {
    pub fn message(&self) -> String {
        match self {
            Self::Missing => MISSING_MESSAGE.into(),
            Self::Damaged => DAMAGED_MESSAGE.into(),
            Self::NoKey => NO_KEY_MESSAGE.into(),
            Self::Invalid => "Attachment store: Invalid stored file metadata".into(),
            Self::System(e) => format!("Attachment store: the attached file could not be read ({e})."),
        }
    }
    /// The reading problem an attachment `FbError` stands for, if it is one of these.
    pub fn of(e: &FbError) -> Option<Self> {
        match e {
            FbError::Refused(m) if m == MISSING_MESSAGE => Some(Self::Missing),
            FbError::Refused(m) if m == DAMAGED_MESSAGE => Some(Self::Damaged),
            FbError::Refused(m) if m == NO_KEY_MESSAGE => Some(Self::NoKey),
            _ => None,
        }
    }
}
impl From<ReadError> for FbError {
    fn from(e: ReadError) -> Self {
        FbError::refused(e.message())
    }
}
fn io(e: impl std::fmt::Display) -> FbError {
    FbError::refused(format!("Attachment store: {e}"))
}
pub fn valid_name(name: &str) -> bool {
    name.len() == 36
        && name.ends_with(".bin")
        && name.as_bytes()[..32]
            .iter()
            .all(|b| b.is_ascii_digit() || matches!(*b, b'a'..=b'f'))
}
pub(super) fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(s: &str) -> FbResult<Vec<u8>> {
    if !s.len().is_multiple_of(2)
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
    {
        return Err(io("Invalid encrypted metadata"));
    }
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| {
            let d = |v: u8| if v <= b'9' { v - b'0' } else { v - b'a' + 10 };
            Ok(d(p[0]) * 16 + d(p[1]))
        })
        .collect()
}
fn random<const N: usize>() -> FbResult<[u8; N]> {
    let mut b = [0; N];
    getrandom::fill(&mut b).map_err(io)?;
    Ok(b)
}
fn cipher(key: &[u8]) -> FbResult<ChaCha20Poly1305> {
    ChaCha20Poly1305::new_from_slice(key).map_err(|_| io("Invalid encryption key"))
}
impl Store {
    /// Each book lives in its own folder, with the receipts sibling specified in ARCHITECTURE §6.
    /// `receipt_key` is the book's receipt key (for a book without a key record, its master key).
    pub fn for_book(book_path: &Path, receipt_key: &[u8; 32]) -> FbResult<Self> {
        let parent = book_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let directory = open_receipts(parent)?;
        Ok(Self {
            folder: parent.join("receipts"),
            directory,
            wrapping_key: wrapping_key(receipt_key)?,
            legacy_key: None,
        })
    }
    /// Also read files whose key is wrapped under the book's current master key (attached before the
    /// receipt-key fix). Writing is unaffected. Equal keys add nothing.
    pub fn with_legacy(mut self, master_key: &[u8; 32]) -> FbResult<Self> {
        let legacy = wrapping_key(master_key)?;
        if *legacy != *self.wrapping_key {
            self.legacy_key = Some(legacy);
        }
        Ok(self)
    }
    fn keys(&self) -> impl Iterator<Item = &[u8; 32]> {
        std::iter::once(&*self.wrapping_key).chain(self.legacy_key.as_deref())
    }
    pub fn folder(&self) -> &Path {
        &self.folder
    }
    fn aad(co: CompanyId, name: &str) -> String {
        format!("FreedomBooks receipt v1:{}:{name}", co.to_db())
    }
    pub fn write_atomic(&self, co: CompanyId, data: &[u8], content_type: &str) -> FbResult<Stored> {
        let name = format!("{}.bin", hex(&random::<16>()?));
        let aad = Self::aad(co, &name);
        if data.is_empty() || data.len() > 100 * 1024 * 1024 {
            return Err(io("Invalid receipt size"));
        }
        let key = Zeroizing::new(random::<32>()?);
        let nonce = random::<7>()?;
        let wrap_nonce = random::<12>()?;
        let wrapped = cipher(self.wrapping_key.as_ref())?
            .encrypt(
                Nonce::from_slice(&wrap_nonce),
                Payload {
                    msg: key.as_ref(),
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| io("Key wrapping failed"))?;
        let mut stream = Some(EncryptorBE32::from_aead(
            cipher(key.as_ref())?,
            (&nonce).into(),
        ));
        let mut digest = Sha256::new();
        let mut chunks = data.chunks(1024 * 1024).peekable();
        self.place_chunks(
            &name,
            std::iter::from_fn(|| {
                let chunk = chunks.next()?;
                digest.update(chunk);
                let payload = Payload {
                    msg: chunk,
                    aad: aad.as_bytes(),
                };
                let result = if chunks.peek().is_none() {
                    stream
                        .take()
                        .ok_or_else(|| io("Encryption stream ended"))
                        .and_then(|s| s.encrypt_last(payload).map_err(|_| io("Encryption failed")))
                } else {
                    stream
                        .as_mut()
                        .ok_or_else(|| io("Encryption stream ended"))
                        .and_then(|s| s.encrypt_next(payload).map_err(|_| io("Encryption failed")))
                };
                Some(result)
            }),
        )?;
        let sha256 = hex(&digest.finalize());
        Ok(Stored {
            stored_name: name,
            content_type: content_type.into(),
            size: i64::try_from(data.len()).map_err(io)?,
            original_sha256: sha256.clone(),
            sha256,
            wrapped_key: hex(&wrapped),
            wrap_nonce: hex(&wrap_nonce),
            nonce: hex(&nonce),
            rewraps: Vec::new(),
        })
    }
    /// Never overwrite an existing file, even when hard links are unavailable. Fail closed in that case.
    #[cfg(unix)]
    fn place_chunks(
        &self,
        name: &str,
        chunks: impl Iterator<Item = FbResult<Vec<u8>>>,
    ) -> FbResult<()> {
        if !valid_name(name) {
            return Err(io("Bad stored name."));
        }
        let tmp = format!(".fb-upload-{}", hex(&random::<16>()?));
        let mut placed = false;
        let result = (|| {
            let fd = openat(
                &self.directory,
                tmp.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_bits_truncate(0o600),
            )
            .map_err(io)?;
            let mut f = File::from(fd);
            for chunk in chunks {
                f.write_all(&chunk?).map_err(io)?;
            }
            f.sync_all().map_err(io)?;
            rustix::fs::linkat(
                &self.directory,
                tmp.as_str(),
                &self.directory,
                name,
                rustix::fs::AtFlags::empty(),
            )
            .map_err(io)?;
            placed = true;
            self.directory.sync_all().map_err(io)?;
            Ok(())
        })();
        let _ = rustix::fs::unlinkat(&self.directory, tmp.as_str(), rustix::fs::AtFlags::empty());
        if placed && result.is_err() {
            self.remove_unreferenced(name);
        }
        result
    }
    /// Windows: the temporary file is created with `CREATE_NEW` relative to the directory handle, flushed, then
    /// renamed into place by handle with `ReplaceIfExists = FALSE`, so an existing entry (file, link or junction) is
    /// never overwritten. The directory is then flushed; if that fails the new file is removed (fail closed).
    #[cfg(windows)]
    fn place_chunks(
        &self,
        name: &str,
        chunks: impl Iterator<Item = FbResult<Vec<u8>>>,
    ) -> FbResult<()> {
        if !valid_name(name) {
            return Err(io("Bad stored name."));
        }
        let tmp = format!(".fb-upload-{}", hex(&random::<16>()?));
        let mut f = windows::create_new(&self.directory, &tmp)?;
        let mut placed = false;
        let result = (|| {
            for chunk in chunks {
                f.write_all(&chunk?).map_err(io)?;
            }
            f.sync_all().map_err(io)?;
            windows::rename_no_replace(&f, &self.directory, name)?;
            placed = true;
            self.directory.sync_all().map_err(io)?;
            Ok(())
        })();
        if !placed {
            let _ = windows::delete_on_close(&f);
        }
        drop(f);
        if placed && result.is_err() {
            self.remove_unreferenced(name);
        }
        result
    }
    pub fn open_read(&self, co: CompanyId, r: &Stored) -> FbResult<Vec<u8>> {
        self.read_checked(co, r).map_err(FbError::from)
    }
    /// [`Store::open_read`] with the reason a file cannot be read kept apart (see [`ReadError`]).
    pub fn read_checked(&self, co: CompanyId, r: &Stored) -> Result<Vec<u8>, ReadError> {
        if !valid_name(&r.stored_name) || r.size < 1 || r.size > 100 * 1024 * 1024 {
            return Err(ReadError::Invalid);
        }
        #[cfg(unix)]
        let f = match openat(
            &self.directory,
            r.stored_name.as_str(),
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => File::from(fd),
            Err(rustix::io::Errno::NOENT) => return Err(ReadError::Missing),
            // Not a regular file in the file's place (a link): what is there is not the attached file.
            Err(rustix::io::Errno::LOOP) => return Err(ReadError::Damaged),
            Err(e) => return Err(ReadError::System(e.to_string())),
        };
        #[cfg(windows)]
        let f = windows::open_read_at(&self.directory, &r.stored_name).map_err(|e| ReadError::System(e.message()))?;
        let meta = f.metadata().map_err(|e| ReadError::System(e.to_string()))?;
        let encrypted_size = r.size as u64 + 16 * (r.size as u64).div_ceil(1024 * 1024);
        if !meta.is_file() || meta.len() != encrypted_size {
            return Err(ReadError::Damaged);
        }
        let mut encrypted = vec![];
        f.take(encrypted_size + 1)
            .read_to_end(&mut encrypted)
            .map_err(|e| ReadError::System(e.to_string()))?;
        decrypt_any(self.keys(), co, r, &encrypted)
    }
    /// Only for a newly written file whose database insert failed. Never for an existing row.
    pub(super) fn remove_unreferenced(&self, name: &str) {
        if valid_name(name) {
            #[cfg(unix)]
            let _ = rustix::fs::unlinkat(&self.directory, name, rustix::fs::AtFlags::empty());
            #[cfg(windows)]
            let _ = windows::unlink_at(&self.directory, name);
            let _ = self.directory.sync_all();
        }
    }
}

#[cfg(unix)]
fn open_receipts(parent: &Path) -> FbResult<File> {
    let parent_fd = open(
        parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io)?;
    match rustix::fs::mkdirat(&parent_fd, "receipts", Mode::from_bits_truncate(0o700)) {
        Ok(()) => {}
        Err(rustix::io::Errno::EXIST) => {}
        Err(e) => return Err(io(e)),
    }
    let fd = openat(
        &parent_fd,
        "receipts",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io)?;
    let st = rustix::fs::fstat(&fd).map_err(io)?;
    if st.st_uid != rustix::process::getuid().as_raw() {
        return Err(io("The receipts folder belongs to another user."));
    }
    rustix::fs::fchmod(&fd, Mode::from_bits_truncate(0o700)).map_err(io)?;
    Ok(fd.into())
}

/// Windows: same guarantees via a handle-relative `NtCreateFile`, reparse-point refusal and an owner-only DACL.
#[cfg(windows)]
fn open_receipts(parent: &Path) -> FbResult<File> {
    windows::open_receipts(parent)
}

fn wrapping_key(master_key: &[u8; 32]) -> FbResult<Zeroizing<[u8; 32]>> {
    let mut wrapping_key = Zeroizing::new([0; 32]);
    Hkdf::<Sha256>::new(None, master_key)
        .expand(b"FreedomBooks receipt key wrapping v1", wrapping_key.as_mut())
        .map_err(|_| io("Key derivation failed"))?;
    Ok(wrapping_key)
}

/// Backups integration (L4-BACKUPS request 1): authenticate a staged receipt ciphertext, outside any store
/// folder, against its owning row's immutable metadata: both AEAD layers under the book's receipt key, exact
/// size, plaintext SHA-256, sniffed type and container rules. Read-only: no directory is created or changed.
pub fn verify_staged(receipt_key: &[u8; 32], co: CompanyId, r: &Stored, staged: &Path) -> FbResult<()> {
    verify_staged_any(&[receipt_key], co, r, staged)
}

/// [`verify_staged`] accepting any wrap of the file's key ([`Stored::wraps`]) under any of `keys` (the
/// receipt key, then the master key that wrapped files attached before the receipt-key fix).
pub fn verify_staged_any(keys: &[&[u8; 32]], co: CompanyId, r: &Stored, staged: &Path) -> FbResult<()> {
    verify_staged_checked(keys, co, r, staged).map_err(FbError::from)
}

/// [`verify_staged_any`] with the reason kept apart: [`ReadError::NoKey`] when no wrap opens under any
/// key, [`ReadError::Damaged`] for a wrong size or content that fails under an opened key, and
/// [`ReadError::System`] when the staged copy itself cannot be read.
pub fn verify_staged_checked(keys: &[&[u8; 32]], co: CompanyId, r: &Stored, staged: &Path) -> Result<(), ReadError> {
    if !valid_name(&r.stored_name) || r.size < 1 || r.size > 100 * 1024 * 1024 {
        return Err(ReadError::Invalid);
    }
    let system = |e: &dyn std::fmt::Display| ReadError::System(e.to_string());
    #[cfg(unix)]
    let f = File::from(open(staged, OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC, Mode::empty()).map_err(|e| system(&e))?);
    #[cfg(windows)]
    let f = windows::open_path_read(staged).map_err(|e| ReadError::System(e.message()))?;
    let encrypted_size = r.size as u64 + 16 * (r.size as u64).div_ceil(1024 * 1024);
    let meta = f.metadata().map_err(|e| system(&e))?;
    if !meta.is_file() || meta.len() != encrypted_size {
        return Err(ReadError::Damaged);
    }
    let mut encrypted = vec![];
    f.take(encrypted_size + 1).read_to_end(&mut encrypted).map_err(|e| system(&e))?;
    let derived = keys.iter().map(|k| wrapping_key(k)).collect::<FbResult<Vec<_>>>().map_err(|e| ReadError::System(e.message()))?;
    decrypt_any(derived.iter().map(|k| &**k), co, r, &encrypted).map(|_| ())
}

/// Every wrap under every key, receipt key first: the first content key whose contents authenticate wins.
/// A file is [`ReadError::NoKey`] only when no wrap opens under any key, and [`ReadError::Damaged`] when
/// some wrap opened but no opened content key authenticates the contents.
fn decrypt_any<'k>(keys: impl Iterator<Item = &'k [u8; 32]>, co: CompanyId, r: &Stored, encrypted: &[u8]) -> Result<Vec<u8>, ReadError> {
    let aad = Store::aad(co, &r.stored_name);
    let mut tried: Vec<Zeroizing<Vec<u8>>> = Vec::new();
    for key in keys {
        for (wrapped, nonce) in r.wraps() {
            let Ok(content) = unwrap_content_key(key, &aad, wrapped, nonce) else { continue };
            if tried.iter().any(|t| **t == *content) {
                continue;
            }
            if let Ok(data) = decrypt_with(&content, &aad, r, encrypted) {
                return Ok(data);
            }
            tried.push(content);
        }
    }
    Err(if tried.is_empty() { ReadError::NoKey } else { ReadError::Damaged })
}

/// Whether any wrap of `r`'s content key opens under `key` (no file is read).
pub fn opens_under(key: &[u8; 32], co: CompanyId, r: &Stored) -> FbResult<bool> {
    if !valid_name(&r.stored_name) {
        return Err(io("Invalid stored file metadata"));
    }
    let aad = Store::aad(co, &r.stored_name);
    let derived = wrapping_key(key)?;
    Ok(r.wraps().any(|(wrapped, nonce)| unwrap_content_key(derived.as_ref(), &aad, wrapped, nonce).is_ok()))
}

fn unwrap_content_key(wrapping_key: &[u8], aad: &str, wrapped_key: &str, wrap_nonce: &str) -> FbResult<Zeroizing<Vec<u8>>> {
    let wn = unhex(wrap_nonce)?;
    if wn.len() != 12 {
        return Err(io("Invalid nonce"));
    }
    Ok(Zeroizing::new(
        cipher(wrapping_key)?
            .decrypt(
                Nonce::from_slice(&wn),
                Payload {
                    msg: &unhex(wrapped_key)?,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| io("Receipt key authentication failed"))?,
    ))
}

/// A further wrap of one stored file's content key, under the receipt key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rewrap {
    pub company: CompanyId,
    pub stored_name: String,
    pub wrap: Wrap,
}

/// The distinct content keys held only by the outgoing master. This compares the unwrapped keys,
/// not merely whether some receipt wrap authenticates: another authentic wrap may hold the wrong key.
fn uncovered_content_keys(receipt_key: &[u8; 32], master_key: &[u8; 32], co: CompanyId, r: &Stored) -> FbResult<Vec<Zeroizing<Vec<u8>>>> {
    if !valid_name(&r.stored_name) { return Err(io("Invalid stored file metadata")); }
    let aad = Store::aad(co, &r.stored_name);
    let contents = |key: &[u8; 32]| -> FbResult<Vec<Zeroizing<Vec<u8>>>> {
        let derived = wrapping_key(key)?;
        let mut found: Vec<Zeroizing<Vec<u8>>> = Vec::new();
        for (wrapped, nonce) in r.wraps() {
            if let Ok(content) = unwrap_content_key(derived.as_ref(), &aad, wrapped, nonce)
                && content.len() == 32 && !found.iter().any(|old| **old == *content)
            { found.push(content); }
        }
        Ok(found)
    };
    let receipts = contents(receipt_key)?;
    Ok(contents(master_key)?.into_iter().filter(|content| !receipts.iter().any(|held| **held == **content)).collect())
}

/// Whether removing the master would discard any candidate content key. No file is read, so this
/// protection also applies while the receipts folder is temporarily absent.
pub fn needs_rewrap(receipt_key: &[u8; 32], master_key: &[u8; 32], co: CompanyId, r: &Stored) -> FbResult<bool> {
    Ok(!uncovered_content_keys(receipt_key, master_key, co, r)?.is_empty())
}

/// Preserve every distinct content key available under the outgoing master under the receipt key.
/// The file, its own immutable wrap and its contents stay untouched.
pub fn rewrap_all(receipt_key: &[u8; 32], master_key: &[u8; 32], co: CompanyId, r: &Stored) -> FbResult<Vec<Rewrap>> {
    let pending = uncovered_content_keys(receipt_key, master_key, co, r)?;
    let aad = Store::aad(co, &r.stored_name);
    let receipts = wrapping_key(receipt_key)?;
    let mut changes: Vec<Rewrap> = Vec::new();
    for content in pending {
        let (wrapped, wrap_nonce) = loop {
            let wrap_nonce = random::<12>()?;
            let nonce = hex(&wrap_nonce);
            if r.wraps().any(|(_, n)| n == nonce) || changes.iter().any(|w| w.wrap.wrap_nonce == nonce) { continue; }
            let wrapped = cipher(receipts.as_ref())?
                .encrypt(Nonce::from_slice(&wrap_nonce), Payload { msg: content.as_ref(), aad: aad.as_bytes() })
                .map_err(|_| io("Key wrapping failed"))?;
            break (hex(&wrapped), nonce);
        };
        if *unwrap_content_key(receipts.as_ref(), &aad, &wrapped, &wrap_nonce)? != *content { return Err(io("Key wrapping failed")); }
        changes.push(Rewrap { company: co, stored_name: r.stored_name.clone(), wrap: Wrap { wrapped_key: wrapped, wrap_nonce } });
    }
    Ok(changes)
}

/// The first missing wrap, for callers handling one content key. Book-level repairs use
/// [`rewrap_all`] so every outgoing-master candidate survives.
pub fn rewrap(receipt_key: &[u8; 32], master_key: &[u8; 32], co: CompanyId, r: &Stored) -> FbResult<Option<Rewrap>> {
    Ok(rewrap_all(receipt_key, master_key, co, r)?.into_iter().next())
}

fn decrypt_with(key: &[u8], aad: &str, r: &Stored, encrypted: &[u8]) -> FbResult<Vec<u8>> {
    let n = unhex(&r.nonce)?;
    if n.len() != 7 {
        return Err(io("Invalid nonce"));
    }
    let mut stream = Some(DecryptorBE32::from_aead(cipher(key)?, n.as_slice().into()));
    let mut data = Vec::with_capacity(r.size as usize);
    let mut chunks = encrypted.chunks(1024 * 1024 + 16).peekable();
    while let Some(chunk) = chunks.next() {
        let payload = Payload {
            msg: chunk,
            aad: aad.as_bytes(),
        };
        let plain = if chunks.peek().is_none() {
            stream
                .take()
                .ok_or_else(|| io("Decryption stream ended"))?
                .decrypt_last(payload)
        } else {
            stream
                .as_mut()
                .ok_or_else(|| io("Decryption stream ended"))?
                .decrypt_next(payload)
        }
        .map_err(|_| io("Receipt authentication failed"))?;
        data.extend(plain);
    }
    if data.len() != r.size as usize
        || hex(&Sha256::digest(&data)) != r.sha256
        || sniff::sniff(&data) != Some(r.content_type.as_str())
        || sniff::problem(&data, &r.content_type).is_some()
    {
        return Err(io("Receipt contents no longer match"));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn never_overwrites_even_an_existing_symlink() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::for_book(&d.path().join("b.fbbook"), &[1; 32]).unwrap();
        let name = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.bin";
        s.place_chunks(name, std::iter::once(Ok(b"first".to_vec())))
            .unwrap();
        assert!(
            s.place_chunks(name, std::iter::once(Ok(b"second".to_vec())))
                .is_err()
        );
        assert_eq!(std::fs::read(s.folder().join(name)).unwrap(), b"first");
        assert_eq!(std::fs::read_dir(s.folder()).unwrap().count(), 1);
    }
}
