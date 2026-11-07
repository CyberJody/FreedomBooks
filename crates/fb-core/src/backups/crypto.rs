//! Byte-compatible port of backups.crypto: NFC, scrypt 2^17/8/1,
//! AES-256-GCM STREAM, 1 MiB records, exact header bytes as AAD.
use super::{BackupError, Result};
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use rand::{RngCore, rngs::OsRng};
use serde_json::{Value, json};
use std::io::{Read, Write};
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

pub const CHUNK: usize = 1024 * 1024;
pub const TAG: usize = 16;
pub const CIPHER_NAME: &str = "AES-256-GCM-STREAM";

pub fn check_new_passphrase(passphrase: &str) -> Result<()> {
    if passphrase.chars().count() < 12 {
        return Err(BackupError::refused(
            "The backup passphrase must be at least 12 characters.",
        ));
    }
    Ok(())
}

pub fn new_parameters() -> Value {
    let mut salt = [0; 16];
    let mut prefix = [0; 7];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut prefix);
    json!({"kdf":{"name":"scrypt","n":131072,"r":8,"p":1,"salt":STANDARD.encode(salt)},
           "cipher":{"name":CIPHER_NAME,"chunk_size":CHUNK,"nonce_prefix":STANDARD.encode(prefix)}})
}

fn unpack(header: &Value) -> Result<(Vec<u8>, Vec<u8>)> {
    if header["kdf"]["name"] != "scrypt"
        || header["cipher"]["name"] != CIPHER_NAME
        || header["cipher"]["chunk_size"].as_u64() != Some(CHUNK as u64)
    {
        return Err(BackupError::refused(
            "Unsupported encryption parameters in the backup header.",
        ));
    }
    if header["kdf"]["n"].as_u64() != Some(131072)
        || header["kdf"]["r"].as_u64() != Some(8)
        || header["kdf"]["p"].as_u64() != Some(1)
    {
        return Err(BackupError::refused(
            "Unsupported key-derivation parameters in the backup header; this FreedomBooks writes and reads scrypt n=2^17, r=8, p=1 only.",
        ));
    }
    let decode = |v: &Value, len| -> Result<Vec<u8>> {
        STANDARD
            .decode(
                v.as_str()
                    .ok_or_else(|| BackupError::refused("Malformed encryption header."))?,
            )
            .ok()
            .filter(|x| x.len() == len)
            .ok_or_else(|| BackupError::refused("Malformed encryption header."))
    };
    Ok((
        decode(&header["kdf"]["salt"], 16)?,
        decode(&header["cipher"]["nonce_prefix"], 7)?,
    ))
}

fn key(passphrase: &str, salt: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
    if passphrase.is_empty() {
        return Err(BackupError::refused("A passphrase is required."));
    }
    let secret = Zeroizing::new(passphrase.nfc().collect::<String>());
    let mut out = Zeroizing::new([0u8; 32]);
    let params = scrypt::Params::new(17, 8, 1, 32)
        .map_err(|_| BackupError::refused("Invalid scrypt parameters."))?;
    scrypt::scrypt(secret.as_bytes(), salt, &params, &mut *out)
        .map_err(|_| BackupError::refused("Could not derive backup key."))?;
    Ok(out)
}

fn nonce(prefix: &[u8], counter: u64, last: bool) -> Result<[u8; 12]> {
    let count = u32::try_from(counter)
        .map_err(|_| BackupError::refused("Payload too large for one backup file."))?;
    let mut out = [0; 12];
    out[..7].copy_from_slice(prefix);
    out[7..11].copy_from_slice(&count.to_be_bytes());
    out[11] = u8::from(last);
    Ok(out)
}

// Read is allowed to return short chunks without EOF (pipes/custom readers).
fn record(src: &mut impl Read, size: usize) -> Result<Zeroizing<Vec<u8>>> {
    let mut value = Zeroizing::new(vec![0; size]);
    let mut used = 0;
    while used < size {
        match src.read(&mut value[used..]) {
            Ok(0) => break,
            Ok(n) => used += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    value.truncate(used);
    Ok(value)
}

pub fn encrypt_stream(
    src: &mut impl Read,
    dst: &mut impl Write,
    passphrase: &str,
    header: &Value,
    aad: &[u8],
) -> Result<()> {
    let (salt, prefix) = unpack(header)?;
    let key = key(passphrase, &salt)?;
    let aead = Aes256Gcm::new_from_slice(&*key)
        .map_err(|_| BackupError::refused("Invalid backup key."))?;
    let mut current = record(src, CHUNK)?;
    let mut counter = 0;
    loop {
        let next = if current.len() == CHUNK {
            record(src, CHUNK)?
        } else {
            Zeroizing::new(Vec::new())
        };
        let last = next.is_empty();
        let iv = nonce(&prefix, counter, last)?;
        let sealed = aead
            .encrypt(Nonce::from_slice(&iv), Payload { msg: &current, aad })
            .map_err(|_| BackupError::refused("Could not encrypt backup."))?;
        dst.write_all(&sealed)?;
        if last {
            return Ok(());
        }
        current = next;
        counter += 1;
    }
}

/// Only writes authenticated records. The caller MUST keep dst private and
/// discard it if any later record fails; a prefix is not a complete backup.
pub fn decrypt_stream(
    src: &mut impl Read,
    dst: &mut impl Write,
    passphrase: &str,
    header: &Value,
    aad: &[u8],
) -> Result<()> {
    let (salt, prefix) = unpack(header)?;
    let key = key(passphrase, &salt)?;
    let aead = Aes256Gcm::new_from_slice(&*key)
        .map_err(|_| BackupError::refused("Invalid backup key."))?;
    let mut current = record(src, CHUNK + TAG)?;
    if current.len() < TAG {
        return Err(BackupError::refused(
            "The encrypted payload is empty or truncated.",
        ));
    }
    let mut counter = 0;
    loop {
        let next = if current.len() == CHUNK + TAG {
            record(src, CHUNK + TAG)?
        } else {
            Zeroizing::new(Vec::new())
        };
        let last = next.is_empty();
        let iv = nonce(&prefix, counter, last)?;
        let plain = Zeroizing::new(
            aead.decrypt(Nonce::from_slice(&iv), Payload { msg: &current, aad })
                .map_err(|_| {
                    BackupError::refused(
                        "Wrong passphrase, or the encrypted backup was changed or truncated.",
                    )
                })?,
        );
        dst.write_all(&plain)?;
        if last {
            return Ok(());
        }
        if next.len() < TAG {
            return Err(BackupError::refused("The encrypted payload is truncated."));
        }
        current = next;
        counter += 1;
    }
}
