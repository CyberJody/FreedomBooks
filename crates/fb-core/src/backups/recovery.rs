//! Argon2id-wrapped master key (ARCHITECTURE J-6). Separate from book archives.
//! Fixed KDF parameters per format prevent untrusted sidecars choosing resource costs; the AAD pins them.
//! - v1: 19,456 KiB, 2 passes, 1 lane (OWASP floor): backup recovery sidecars and the 150-bit recovery key.
//! - v2: 65,536 KiB, 3 passes, 1 lane, the cost of the earlier app-lock wrap: owner passwords. v1 still opens.
use super::{BackupError, Result, paths};
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use std::{io::Write, path::Path};
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

struct Cost {
    format: &'static str,
    m_kib: u32,
    t: u32,
    p: u32,
    aad: &'static [u8],
}
const V1: Cost = Cost {
    format: "freedombooks-recovery-v1",
    m_kib: 19456,
    t: 2,
    p: 1,
    aad: b"FreedomBooks recovery key v1:Argon2id:19456:2:1:AES-256-GCM",
};
const V2: Cost = Cost {
    format: "freedombooks-recovery-v2",
    m_kib: 65536,
    t: 3,
    p: 1,
    aad: b"FreedomBooks recovery key v2:Argon2id:65536:3:1:AES-256-GCM",
};
// Never weaker than the earlier app-lock wrap (fb-app keys.rs: 64 MiB, 3 passes).
const _: () = assert!(V2.m_kib >= 65536 && V2.t >= 3 && V2.p == 1);
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WrappedKey {
    format: String,
    salt: String,
    nonce: String,
    wrapped_key: String,
}
fn derive(pass: &str, salt: &[u8], cost: &Cost) -> Result<Zeroizing<[u8; 32]>> {
    if pass.is_empty() {
        return Err(BackupError::refused(
            "A lock/recovery passphrase is required.",
        ));
    }
    let pass = Zeroizing::new(pass.nfc().collect::<String>());
    let params = argon2::Params::new(cost.m_kib, cost.t, cost.p, Some(32))
        .map_err(|_| BackupError::refused("Invalid recovery KDF parameters."))?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = Zeroizing::new([0; 32]);
    argon
        .hash_password_into(pass.as_bytes(), salt, &mut *key)
        .map_err(|_| BackupError::refused("Could not derive recovery key."))?;
    Ok(key)
}
impl WrappedKey {
    pub fn new(key: &[u8; 32], passphrase: &str) -> Result<Self> {
        super::crypto::check_new_passphrase(passphrase)?;
        Self::wrap(key, passphrase, &V1)
    }
    /// Desktop passwords preserve the existing eight-character minimum.
    pub fn for_password(key: &[u8; 32], password: &str) -> Result<Self> {
        if password.chars().count() < 8 || password.len() > 4096 {
            return Err(BackupError::refused("Use a password of at least 8 characters."));
        }
        Self::wrap(key, password, &V2)
    }
    fn wrap(key: &[u8; 32], passphrase: &str, cost: &Cost) -> Result<Self> {
        let mut salt = [0; 16];
        let mut nonce = [0; 12];
        OsRng.fill_bytes(&mut salt);
        OsRng.fill_bytes(&mut nonce);
        let wrapping = derive(passphrase, &salt, cost)?;
        let aead = Aes256Gcm::new_from_slice(&*wrapping)
            .map_err(|_| BackupError::refused("Invalid recovery key."))?;
        let encrypted = aead
            .encrypt(Nonce::from_slice(&nonce), Payload { msg: key, aad: cost.aad })
            .map_err(|_| BackupError::refused("Recovery key wrapping failed."))?;
        Ok(Self {
            format: cost.format.into(),
            salt: STANDARD.encode(salt),
            nonce: STANDARD.encode(nonce),
            wrapped_key: STANDARD.encode(encrypted),
        })
    }
    pub fn unlock(&self, passphrase: &str) -> Result<Zeroizing<[u8; 32]>> {
        let decode = |text: &str, size: usize| {
            STANDARD
                .decode(text)
                .ok()
                .filter(|b| b.len() == size)
                .ok_or_else(|| BackupError::refused("Malformed recovery key."))
        };
        let cost = [&V1, &V2]
            .into_iter()
            .find(|c| c.format == self.format)
            .ok_or_else(|| BackupError::refused("Unsupported recovery key."))?;
        let salt = decode(&self.salt, 16)?;
        let nonce = decode(&self.nonce, 12)?;
        let wrapped = decode(&self.wrapped_key, 48)?;
        let wrapping = derive(passphrase, &salt, cost)?;
        let aead = Aes256Gcm::new_from_slice(&*wrapping)
            .map_err(|_| BackupError::refused("Invalid recovery key."))?;
        let plain = Zeroizing::new(
            aead.decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &wrapped,
                    aad: cost.aad,
                },
            )
            .map_err(|_| BackupError::refused("Wrong passphrase or changed recovery key."))?,
        );
        let mut key = Zeroizing::new([0; 32]);
        key.copy_from_slice(&plain);
        Ok(key)
    }
    /// Create once; never overwrite the user's only recovery sidecar.
    pub fn write_once(&self, path: &Path) -> Result<()> {
        let mut f = paths::new_file(path)?;
        f.write_all(&serde_json::to_vec_pretty(self)?)?;
        f.sync_all()?;
        paths::sync_dir(
            path.parent()
                .ok_or_else(|| BackupError::refused("Missing recovery folder."))?,
        )?;
        Ok(())
    }
    pub fn read(path: &Path) -> Result<Self> {
        let mut f = paths::regular(path)?;
        if f.metadata()?.len() > 4096 {
            return Err(BackupError::refused("Recovery key file too large."));
        }
        let mut raw = Vec::new();
        std::io::Read::read_to_end(&mut f, &mut raw)?;
        Ok(serde_json::from_value(super::parse_json(&raw)?)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owner_passwords_use_the_app_lock_cost_and_v1_records_still_open() {
        let password = WrappedKey::for_password(&[7; 32], "synthetic password").unwrap();
        assert_eq!(password.format, "freedombooks-recovery-v2");
        assert_eq!(*password.unlock("synthetic password").unwrap(), [7; 32]);
        assert!(password.unlock("other password").is_err());
        // A v1 record (this branch's first password wraps, and every recovery wrap) keeps opening.
        let old = WrappedKey::wrap(&[9; 32], "synthetic password", &V1).unwrap();
        let old: WrappedKey = serde_json::from_str(&serde_json::to_string(&old).unwrap()).unwrap();
        assert_eq!(old.format, "freedombooks-recovery-v1");
        assert_eq!(*old.unlock("synthetic password").unwrap(), [9; 32]);
        // The format names its cost: relabelling a v1 wrap as v2 does not open it.
        let mut relabelled = old.clone();
        relabelled.format = V2.format.into();
        assert!(relabelled.unlock("synthetic password").is_err());
    }
}
