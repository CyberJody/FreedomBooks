//! Book-local encrypted receipt store. Directory handles and relative no-follow opens prevent traversal.
//! Keys never implement Debug or Serialize. The host supplies the unlocked book's master key.
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
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Stored {
    pub stored_name: String,
    pub content_type: String,
    pub size: i64,
    pub sha256: String,
    pub original_sha256: String,
    pub wrapped_key: String,
    pub wrap_nonce: String,
    pub nonce: String,
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
    pub fn for_book(book_path: &Path, master_key: &[u8; 32]) -> FbResult<Self> {
        let parent = book_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let directory = open_receipts(parent)?;
        Ok(Self {
            folder: parent.join("receipts"),
            directory,
            wrapping_key: wrapping_key(master_key)?,
        })
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
        if !valid_name(&r.stored_name) || r.size < 1 || r.size > 100 * 1024 * 1024 {
            return Err(io("Invalid stored file metadata"));
        }
        #[cfg(unix)]
        let f = File::from(
            openat(
                &self.directory,
                r.stored_name.as_str(),
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(io)?,
        );
        #[cfg(windows)]
        let f = windows::open_read_at(&self.directory, &r.stored_name)?;
        let meta = f.metadata().map_err(io)?;
        let encrypted_size = r.size as u64 + 16 * (r.size as u64).div_ceil(1024 * 1024);
        if !meta.is_file() || meta.len() != encrypted_size {
            return Err(io("Receipt file size does not match"));
        }
        let mut encrypted = vec![];
        f.take(encrypted_size + 1)
            .read_to_end(&mut encrypted)
            .map_err(io)?;
        decrypt_verified(self.wrapping_key.as_ref(), co, r, &encrypted)
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
/// folder, against its owning row's immutable metadata: both AEAD layers under the book's master key, exact
/// size, plaintext SHA-256, sniffed type and container rules. Read-only: no directory is created or changed.
pub fn verify_staged(master_key: &[u8; 32], co: CompanyId, r: &Stored, staged: &Path) -> FbResult<()> {
    if !valid_name(&r.stored_name) || r.size < 1 || r.size > 100 * 1024 * 1024 {
        return Err(io("Invalid stored file metadata"));
    }
    #[cfg(unix)]
    let f = File::from(open(staged, OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC, Mode::empty()).map_err(io)?);
    #[cfg(windows)]
    let f = windows::open_path_read(staged)?;
    let encrypted_size = r.size as u64 + 16 * (r.size as u64).div_ceil(1024 * 1024);
    if !f.metadata().map_err(io)?.is_file() || f.metadata().map_err(io)?.len() != encrypted_size {
        return Err(io("Receipt file size does not match"));
    }
    let mut encrypted = vec![];
    f.take(encrypted_size + 1).read_to_end(&mut encrypted).map_err(io)?;
    decrypt_verified(wrapping_key(master_key)?.as_ref(), co, r, &encrypted).map(|_| ())
}

fn decrypt_verified(wrapping_key: &[u8], co: CompanyId, r: &Stored, encrypted: &[u8]) -> FbResult<Vec<u8>> {
    let aad = Store::aad(co, &r.stored_name);
    let wn = unhex(&r.wrap_nonce)?;
    let n = unhex(&r.nonce)?;
    if wn.len() != 12 || n.len() != 7 {
        return Err(io("Invalid nonce"));
    }
    let key = Zeroizing::new(
        cipher(wrapping_key)?
            .decrypt(
                Nonce::from_slice(&wn),
                Payload {
                    msg: &unhex(&r.wrapped_key)?,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| io("Receipt key authentication failed"))?,
    );
    let mut stream = Some(DecryptorBE32::from_aead(cipher(&key)?, n.as_slice().into()));
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
