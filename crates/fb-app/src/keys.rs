//! The book master key and its app-lock wrap (ARCHITECTURE 5.2 and 7).
//!
//! - The master key is 32 random bytes made at first run; it is never derived from a password. The
//!   desktop shell stores one copy encrypted by the OS (Electron `safeStorage`).
//! - When the user sets an app-lock password (required when the OS store is `basic_text`, i.e. Linux
//!   without a secret service), a second copy is wrapped here: Argon2id(password, random salt) -> a
//!   256-bit key-encryption key -> ChaCha20-Poly1305 over the master key with a random nonce. The blob
//!   is versioned JSON with hex fields and holds nothing secret in the clear.
//! - Keys live in `Zeroizing` buffers and are wiped on drop (lock).
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub const KEY_LEN: usize = 32;
/// Argon2id cost: OWASP's 19 MiB / 2 passes / 1 lane baseline, raised to 64 MiB for a desktop.
const M_COST_KIB: u32 = 65536;
const T_COST: u32 = 3;
const P_COST: u32 = 1;
const MIN_PASSWORD_CHARS: usize = 8;

pub type MasterKey = Zeroizing<[u8; KEY_LEN]>;

#[derive(Debug, PartialEq)]
pub enum KeyError {
    Random,
    WeakPassword,
    WrongPassword,
    Corrupt(&'static str),
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyError::Random => f.write_str("The system random source failed."),
            KeyError::WeakPassword => write!(f, "Use an app-lock password of at least {MIN_PASSWORD_CHARS} characters."),
            KeyError::WrongPassword => f.write_str(fb_core::backups::portable::WRONG_SECRET),
            KeyError::Corrupt(what) => write!(f, "The saved key is damaged ({what})."),
        }
    }
}

pub fn random_bytes<const N: usize>() -> Result<[u8; N], KeyError> {
    let mut out = [0u8; N];
    getrandom::fill(&mut out).map_err(|_| KeyError::Random)?;
    Ok(out)
}

pub fn new_master_key() -> Result<MasterKey, KeyError> {
    Ok(Zeroizing::new(random_bytes::<KEY_LEN>()?))
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn from_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok()).collect()
}

pub fn master_from_hex(text: &str) -> Result<MasterKey, KeyError> {
    let bytes = Zeroizing::new(from_hex(text).ok_or(KeyError::Corrupt("not hex"))?);
    let array: [u8; KEY_LEN] = bytes.as_slice().try_into().map_err(|_| KeyError::Corrupt("wrong length"))?;
    Ok(Zeroizing::new(array))
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WrappedKey {
    pub v: u32,
    pub kdf: String,
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
    pub salt: String,
    pub nonce: String,
    pub ciphertext: String,
}

fn kek(password: &str, salt: &[u8], m: u32, t: u32, p: u32) -> Result<Zeroizing<[u8; 32]>, KeyError> {
    let params = argon2::Params::new(m, t, p, Some(32)).map_err(|_| KeyError::Corrupt("kdf parameters"))?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut out = Zeroizing::new([0u8; 32]);
    argon.hash_password_into(password.as_bytes(), salt, out.as_mut()).map_err(|_| KeyError::Corrupt("kdf"))?;
    Ok(out)
}

pub fn check_password(password: &str) -> Result<(), KeyError> {
    if password.chars().count() < MIN_PASSWORD_CHARS { Err(KeyError::WeakPassword) } else { Ok(()) }
}

pub fn wrap(master: &MasterKey, password: &str) -> Result<WrappedKey, KeyError> {
    wrap_with_cost(master, password, M_COST_KIB, T_COST, P_COST)
}

/// `wrap` with explicit Argon2 costs (tests use small ones; the stored blob records what was used).
pub fn wrap_with_cost(master: &MasterKey, password: &str, m: u32, t: u32, p: u32) -> Result<WrappedKey, KeyError> {
    check_password(password)?;
    let salt = random_bytes::<16>()?;
    let nonce = random_bytes::<12>()?;
    let key = kek(password, &salt, m, t, p)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key.as_ref()));
    let ciphertext = cipher.encrypt(Nonce::from_slice(&nonce), master.as_ref()).map_err(|_| KeyError::Corrupt("encrypt"))?;
    Ok(WrappedKey { v: 1, kdf: "argon2id".into(), m_cost: m, t_cost: t, p_cost: p, salt: to_hex(&salt), nonce: to_hex(&nonce), ciphertext: to_hex(&ciphertext) })
}

pub fn unwrap(blob: &WrappedKey, password: &str) -> Result<MasterKey, KeyError> {
    if blob.v != 1 || blob.kdf != "argon2id" {
        return Err(KeyError::Corrupt("unknown format"));
    }
    // Refuse costs an attacker-edited blob could use to make unlock trivially cheap or hang the app.
    if !(8192..=1_048_576).contains(&blob.m_cost) || !(1..=10).contains(&blob.t_cost) || !(1..=4).contains(&blob.p_cost) {
        return Err(KeyError::Corrupt("kdf cost out of range"));
    }
    let salt = from_hex(&blob.salt).filter(|s| s.len() == 16).ok_or(KeyError::Corrupt("salt"))?;
    let nonce = from_hex(&blob.nonce).filter(|n| n.len() == 12).ok_or(KeyError::Corrupt("nonce"))?;
    let ciphertext = from_hex(&blob.ciphertext).filter(|c| c.len() == KEY_LEN + 16).ok_or(KeyError::Corrupt("ciphertext"))?;
    let key = kek(password, &salt, blob.m_cost, blob.t_cost, blob.p_cost)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key.as_ref()));
    let plain = Zeroizing::new(cipher.decrypt(Nonce::from_slice(&nonce), ciphertext.as_ref()).map_err(|_| KeyError::WrongPassword)?);
    let array: [u8; KEY_LEN] = plain.as_slice().try_into().map_err(|_| KeyError::Corrupt("length"))?;
    Ok(Zeroizing::new(array))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_round_trips_and_refuses_wrong_password() {
        let master = new_master_key().unwrap();
        let blob = wrap_with_cost(&master, "correct horse", 8192, 1, 1).unwrap();
        assert_eq!(*unwrap(&blob, "correct horse").unwrap(), *master);
        assert_eq!(unwrap(&blob, "wrong horse!").unwrap_err(), KeyError::WrongPassword);
        assert!(!blob.ciphertext.contains(&to_hex(master.as_ref())));
    }

    #[test]
    fn short_passwords_and_tampered_blobs_are_refused() {
        let master = new_master_key().unwrap();
        assert_eq!(wrap(&master, "short").unwrap_err(), KeyError::WeakPassword);
        let mut blob = wrap_with_cost(&master, "long enough", 8192, 1, 1).unwrap();
        blob.m_cost = 8;
        assert_eq!(unwrap(&blob, "long enough").unwrap_err(), KeyError::Corrupt("kdf cost out of range"));
        blob.m_cost = 8192;
        let mut ct = from_hex(&blob.ciphertext).unwrap();
        ct[0] ^= 1;
        blob.ciphertext = to_hex(&ct);
        assert_eq!(unwrap(&blob, "long enough").unwrap_err(), KeyError::WrongPassword);
    }

    #[test]
    fn hex_parsing_is_strict() {
        assert!(master_from_hex(&"ab".repeat(32)).is_ok());
        assert!(master_from_hex(&"ab".repeat(31)).is_err());
        assert!(master_from_hex(&"zz".repeat(32)).is_err());
        assert!(from_hex("é1").is_none());
    }
}
