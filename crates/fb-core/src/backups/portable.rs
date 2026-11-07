//! Portable book keys. The built-in key is public and provides no confidentiality.
use super::{BackupError, Result, recovery::WrappedKey};
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// Published raw SQLCipher key, shared by every install. This is intentionally not a secret.
pub const BUILTIN_KEY: [u8; 32] = *b"FreedomBooks public book key v1!";
pub const BUILTIN_WARNING: &str = "Anyone who gets a copy of this file can open it.";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    BuiltIn,
    OwnPassword,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sealed {
    nonce: String,
    ciphertext: String,
}

impl Sealed {
    pub fn new(key: &[u8; 32], label: &[u8], plain: &[u8]) -> Result<Self> {
        let mut nonce = [0; 12];
        OsRng.fill_bytes(&mut nonce);
        let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| invalid())?;
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plain,
                    aad: label,
                },
            )
            .map_err(|_| invalid())?;
        Ok(Self {
            nonce: STANDARD.encode(nonce),
            ciphertext: STANDARD.encode(ciphertext),
        })
    }
    pub fn open(&self, key: &[u8; 32], label: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        let nonce = STANDARD.decode(&self.nonce).map_err(|_| invalid())?;
        if nonce.len() != 12 || self.ciphertext.len() > 4096 {
            return Err(invalid());
        }
        let ciphertext = STANDARD.decode(&self.ciphertext).map_err(|_| invalid())?;
        Aes256Gcm::new_from_slice(key)
            .map_err(|_| invalid())?
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &ciphertext,
                    aad: label,
                },
            )
            .map(Zeroizing::new)
            .map_err(|_| invalid())
    }
}

const RECEIPTS: &[u8] = b"FreedomBooks portable receipt key v1";
const RECOVERY: &[u8] = b"FreedomBooks recovery display v1";
fn invalid() -> BackupError {
    BackupError::refused("The book key record is damaged or does not match these books.")
}

/// Only wrapped secrets are serialized. Local auto-open and saved-kit settings are not portable.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyEnvelope {
    pub version: u32,
    pub mode: Mode,
    pub password: Option<WrappedKey>,
    pub recovery: Option<WrappedKey>,
    receipt_key: Sealed,
    recovery_text: Option<Sealed>,
}

impl KeyEnvelope {
    pub fn new(master: &[u8; 32], receipts: &[u8; 32], password: Option<&str>) -> Result<Self> {
        let receipt_key = Sealed::new(master, RECEIPTS, receipts)?;
        if let Some(password) = password {
            if master == &BUILTIN_KEY {
                return Err(invalid());
            }
            let text = generate_recovery();
            Ok(Self {
                version: 1,
                mode: Mode::OwnPassword,
                password: Some(WrappedKey::for_password(master, password)?),
                recovery: Some(WrappedKey::new(master, &text)?),
                receipt_key,
                recovery_text: Some(Sealed::new(master, RECOVERY, text.as_bytes())?),
            })
        } else {
            if master != &BUILTIN_KEY {
                return Err(invalid());
            }
            Ok(Self {
                version: 1,
                mode: Mode::BuiltIn,
                password: None,
                recovery: None,
                receipt_key,
                recovery_text: None,
            })
        }
    }
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            return Err(invalid());
        }
        let own = self.mode == Mode::OwnPassword;
        if self.password.is_some() != own
            || self.recovery.is_some() != own
            || self.recovery_text.is_some() != own
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn unlock(&self, supplied: &str) -> Result<Zeroizing<[u8; 32]>> {
        self.validate()?;
        let key = match self.mode {
            Mode::BuiltIn => Zeroizing::new(BUILTIN_KEY),
            Mode::OwnPassword => {
                let password = self.password.as_ref().ok_or_else(invalid)?;
                let recovery = self.recovery.as_ref().ok_or_else(invalid)?;
                let wrong = || BackupError::refused(WRONG_SECRET);
                // A well-formed recovery key is tried first; a mistyped one is named before any guess at it.
                match recovery_input(supplied) {
                    RecoveryInput::Checked(text) => recovery
                        .unlock(&text)
                        .or_else(|_| password.unlock(supplied))
                        .map_err(|_| wrong())?,
                    RecoveryInput::Unchecked(text) => password
                        .unlock(supplied)
                        .or_else(|_| recovery.unlock(&text))
                        .map_err(|_| wrong())?,
                    RecoveryInput::Typo => password.unlock(supplied).map_err(|_| {
                        BackupError::refused(RECOVERY_TYPO)
                    })?,
                    RecoveryInput::NotKey => password.unlock(supplied).map_err(|_| wrong())?,
                }
            }
        };
        self.receipts(&key)?;
        Ok(key)
    }
    pub fn receipts(&self, master: &[u8; 32]) -> Result<Zeroizing<[u8; 32]>> {
        self.validate()?;
        let plain = self.receipt_key.open(master, RECEIPTS)?;
        let bytes: [u8; 32] = plain.as_slice().try_into().map_err(|_| invalid())?;
        Ok(Zeroizing::new(bytes))
    }
    pub fn recovery_text(&self, master: &[u8; 32]) -> Result<Zeroizing<String>> {
        self.validate()?;
        let plain = self
            .recovery_text
            .as_ref()
            .ok_or_else(|| BackupError::refused("Built-in-key books have no recovery key."))?
            .open(master, RECOVERY)?;
        String::from_utf8(plain.to_vec())
            .map(Zeroizing::new)
            .map_err(|_| invalid())
    }
    pub fn verify_password(&self, supplied: &str, master: &[u8; 32]) -> Result<()> {
        self.validate()?;
        let key = self
            .password
            .as_ref()
            .ok_or_else(invalid)?
            .unlock(supplied)?;
        if &*key != master {
            return Err(invalid());
        }
        Ok(())
    }
}

/// Crockford base32: no I, L, O or U. Typed look-alikes fold in (O→0, I/L→1, U→V).
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// 30 random symbols (150 bits) and one check group of 5 symbols (25 bits of SHA-256 over them).
const DATA_SYMBOLS: usize = 30;
const CHECK_SYMBOLS: usize = 5;
/// Every wrong password or recovery key gets these exact words, whichever wrap was tried.
pub const WRONG_SECRET: &str = "That password or key doesn't open these books. Check it and try again, or use your recovery key.";
pub const RECOVERY_TYPO: &str = "That recovery key has a typing mistake: it does not match its last (check) group. Compare each group with your Emergency Kit.";

fn check_group(data: &[u8]) -> [u8; CHECK_SYMBOLS] {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"FreedomBooks recovery check v1:");
    hash.update(data);
    let digest = hash.finalize();
    let bits = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) >> 7;
    std::array::from_fn(|i| ALPHABET[((bits >> (5 * (CHECK_SYMBOLS - 1 - i))) & 31) as usize])
}
fn grouped(symbols: &[u8]) -> Zeroizing<String> {
    let mut out = Zeroizing::new(String::with_capacity(symbols.len() + symbols.len() / 5));
    for (i, c) in symbols.iter().enumerate() {
        if i > 0 && i % 5 == 0 {
            out.push('-');
        }
        out.push(*c as char);
    }
    out
}
fn generate_recovery() -> Zeroizing<String> {
    let mut bytes = Zeroizing::new([0; DATA_SYMBOLS]);
    OsRng.fill_bytes(&mut *bytes);
    let mut symbols = Zeroizing::new(Vec::with_capacity(DATA_SYMBOLS + CHECK_SYMBOLS));
    symbols.extend(bytes.iter().map(|b| ALPHABET[(b & 31) as usize]));
    let check = check_group(&symbols);
    symbols.extend_from_slice(&check);
    grouped(&symbols)
}

enum RecoveryInput {
    /// 35 symbols whose check group matches: the wrapped text.
    Checked(Zeroizing<String>),
    /// 30 symbols: a key made before the check group existed.
    Unchecked(Zeroizing<String>),
    /// 35 symbols whose check group does not match.
    Typo,
    /// Not shaped like a recovery key (a password).
    NotKey,
}
fn recovery_input(text: &str) -> RecoveryInput {
    let mut symbols = Zeroizing::new(Vec::with_capacity(text.len()));
    for c in text.chars().filter(|c| !c.is_whitespace() && *c != '-') {
        let c = match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            'U' => 'V',
            c => c,
        };
        if !c.is_ascii() || !ALPHABET.contains(&(c as u8)) {
            return RecoveryInput::NotKey;
        }
        symbols.push(c as u8);
    }
    match symbols.len() {
        DATA_SYMBOLS => RecoveryInput::Unchecked(grouped(&symbols)),
        n if n == DATA_SYMBOLS + CHECK_SYMBOLS => {
            if check_group(&symbols[..DATA_SYMBOLS]) == symbols[DATA_SYMBOLS..] {
                RecoveryInput::Checked(grouped(&symbols))
            } else {
                RecoveryInput::Typo
            }
        }
        _ => RecoveryInput::NotKey,
    }
}

/// Owner-only metadata for protection changes; no secret-bearing fields are accepted.
pub fn audit_event(book: &mut fb_db::Book, actor: crate::UserId, company: crate::CompanyId, action: &str, detail: serde_json::Value) -> crate::FbResult<()> {
    let valid = match action {
        "book.password_prompt" => detail.as_object().is_some_and(|v| v.len() == 1 && v.get("ask_password").is_some_and(serde_json::Value::is_boolean)),
        "book.recovery_saved" => detail.as_object().is_some_and(|v| v.is_empty()),
        "book.protection_changed" => detail.as_object().is_some_and(|v| v.len() == 1 && matches!(v.get("mode").and_then(serde_json::Value::as_str), Some("built-in" | "own-password"))),
        _ => false,
    };
    if !valid { return Err(crate::FbError::refused("Invalid book protection audit event.")); }
    book.write(|tx| {
        let user = crate::services::access(tx, actor, company)?;
        if !(user.is_staff || user.is_superuser) { return Err(crate::FbError::denied("Book protection is limited to the owner.")); }
        crate::services::audit(tx, &user, company, action, "Book protection".into(), detail)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_modes_are_portable_and_own_keys_need_a_secret() {
        let own = KeyEnvelope::new(&[23; 32], &[27; 32], Some("test password")).unwrap();
        let text = own.recovery_text(&[23; 32]).unwrap();
        assert_eq!(text.len(), 41);
        let serialized = serde_json::to_string(&own).unwrap();
        assert!(!serialized.contains(text.as_str()));
        let other: KeyEnvelope = serde_json::from_str(&serialized).unwrap();
        assert_eq!(*other.unlock("test password").unwrap(), [23; 32]);
        assert_eq!(
            *other
                .unlock(&text.to_lowercase().replace('-', " "))
                .unwrap(),
            [23; 32]
        );
        assert!(other.unlock("wrong password").is_err());
        assert!(other.receipts(&BUILTIN_KEY).is_err());
        let builtin = KeyEnvelope::new(&BUILTIN_KEY, &[31; 32], None).unwrap();
        assert_eq!(*builtin.unlock("").unwrap(), BUILTIN_KEY);
        assert_eq!(*builtin.receipts(&BUILTIN_KEY).unwrap(), [31; 32]);
        assert!(builtin.recovery_text(&BUILTIN_KEY).is_err());
        assert!(builtin.password.is_none() && builtin.recovery.is_none());
    }
    #[test]
    fn recovery_keys_fold_look_alikes_and_their_check_group_names_a_typo() {
        let own = KeyEnvelope::new(&[41; 32], &[43; 32], Some("test password")).unwrap();
        let text = own.recovery_text(&[41; 32]).unwrap();
        assert!(matches!(recovery_input(&text), RecoveryInput::Checked(_)));
        // Seven groups of five; the alphabet has no I, L, O or U, so typed look-alikes fold in.
        assert_eq!(text.split('-').count(), 7);
        let typed: String = text
            .chars()
            .map(|c| match c {
                '0' => 'o',
                '1' => 'l',
                'V' => 'u',
                c => c.to_ascii_lowercase(),
            })
            .collect();
        assert_eq!(*own.unlock(&typed.replace('-', " ")).unwrap(), [41; 32]);
        // One wrong symbol is caught by the check group with a plain message.
        let mut wrong: Vec<char> = text.chars().collect();
        wrong[3] = if wrong[3] == 'A' { 'B' } else { 'A' };
        let wrong: String = wrong.into_iter().collect();
        assert!(matches!(recovery_input(&wrong), RecoveryInput::Typo));
        assert_eq!(
            own.unlock(&wrong).err().unwrap().to_string(),
            BackupError::refused(RECOVERY_TYPO).to_string()
        );
        // So is a swap of two neighbouring symbols, and a missing check group still reads as a password.
        let mut swapped: Vec<char> = text.replace('-', "").chars().collect();
        let i = (0..29).find(|&i| swapped[i] != swapped[i + 1]).unwrap();
        swapped.swap(i, i + 1);
        assert!(matches!(recovery_input(&swapped.into_iter().collect::<String>()), RecoveryInput::Typo));
        assert!(matches!(recovery_input("test password"), RecoveryInput::NotKey));
        assert_eq!(*own.unlock("test password").unwrap(), [41; 32]);
    }
    #[test]
    fn keys_from_before_the_check_group_still_open() {
        let master = [47; 32];
        let legacy = "ABCDE-FGHJK-MNPQR-STVWX-YZ012-34567";
        let mut own = KeyEnvelope::new(&master, &[53; 32], Some("test password")).unwrap();
        own.recovery = Some(WrappedKey::new(&master, legacy).unwrap());
        assert!(matches!(recovery_input("abcdefghjkmnpqrstvwxyzo1234567"), RecoveryInput::Unchecked(_)));
        assert_eq!(*own.unlock("abcde fghjk mnpqr stvwx yzo12 34567").unwrap(), master);
    }
}
