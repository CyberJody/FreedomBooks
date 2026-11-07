//! Central bank credential policy. Raw guard installation and pending-mask
//! acknowledgement are private to fb-db; callers can only install this validated policy.
mod secrets;
mod html_tables;
mod py_unicode;
pub mod text;
pub use secrets::{SecretSet, MASK, MIN_CREDENTIAL_CHARS, TOO_SHORT, CREDENTIAL_ALPHABET, UNSAFE_CREDENTIAL, is_blank};
use serde_json::Value;
use zeroize::Zeroizing;
use std::sync::{Arc, atomic::AtomicU8};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce, aead::{Aead, Payload}};
use sha2::{Digest, Sha256};

/// Only the authenticated load path below can construct a production capability.
/// It is bound to this Book instance and its stored company/connection envelope.
/// No Debug or Serialize: the access URL remains a zeroizing secret.
/// ```compile_fail
/// let _ = fb_db::credential_mask::BankfeedRestore {
///     book_identity: Default::default(), company: String::new(),
///     connection: String::new(), access_url: Default::default(), envelope_hash: None,
/// };
/// ```
pub struct BankfeedRestore {
    book_identity: Arc<AtomicU8>,
    company: String,
    connection: String,
    access_url: Zeroizing<String>,
    envelope_hash: Option<[u8; 32]>,
}
impl BankfeedRestore {
    pub fn access_url(&self) -> &str { &self.access_url }
}
fn envelope_hash(parts: &[&[u8]]) -> [u8; 32] {
    let mut hash = Sha256::new();
    for part in parts { hash.update((part.len() as u64).to_be_bytes()); hash.update(part); }
    hash.finalize().into()
}

pub fn credential_aad(company: &str, plugin: &str, connection: &str, purpose: &[u8]) -> Vec<u8> {
    let mut out = b"FreedomBooks credential envelope v1".to_vec();
    for component in [purpose, company.as_bytes(), plugin.as_bytes(), connection.as_bytes()] {
        out.extend_from_slice(&(component.len() as u64).to_be_bytes());
        out.extend_from_slice(component);
    }
    out
}

#[derive(Debug)]
pub struct MaskError(&'static str);
impl MaskError { pub fn message(&self) -> String { self.0.into() } }
impl std::fmt::Display for MaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "ValidationError: {}", self.0) }
}
impl std::error::Error for MaskError {}
type MaskResult<T> = Result<T, MaskError>;
fn err(message: &'static str) -> MaskError { MaskError(message) }
const UNSAFE_JSON: &str = "Bank-feed text cannot be masked while preserving its JSON schema; the transaction was refused.";
pub fn final_text(secrets: &SecretSet, text: &str) -> MaskResult<String> {
    let masked = mask_final_text(secrets, text)?;
    #[cfg(any(test, feature = "test-support"))]
    FINAL_TEXTS.with_borrow_mut(|log| {
        if let Some(log) = log { log.push((text.to_owned(), masked.clone())); }
    });
    Ok(masked)
}
#[cfg(any(test, feature = "test-support"))]
thread_local! {
    pub static FINAL_TEXTS: std::cell::RefCell<Option<Vec<(String, String)>>> = const { std::cell::RefCell::new(None) };
}
pub fn final_scalar(secrets: &SecretSet, text: &str) -> String { safe_final_scalar(secrets, text) }
fn safe_final_scalar(secrets: &SecretSet, text: &str) -> String {
    let masked = secrets.mask(text);
    let encoded = serde_json::to_string(&masked).expect("string serialization");
    if secrets.sensitive_ranges(&encoded).is_empty() { masked } else { MASK.to_owned() }
}

fn mask_final_text(secrets: &SecretSet, text: &str) -> MaskResult<String> {
    if secrets.is_empty() { return Ok(text.to_owned()); }
    let Ok(original) = serde_json::from_str::<Value>(text) else { return Ok(safe_final_scalar(secrets, text)); };
    // A syntactically valid JSON input with duplicate keys must never be normalized into valid evidence.
    crate::parse_unique_json(text).map_err(|_| err(UNSAFE_JSON))?;
    // JSON serialization adds an escaping layer. Mask decoded leaves as well, so a supported
    // three-deep provider encoding remains covered after being placed in a JSON string token.
    let mut leaves = original.clone();
    protect_json_leaves(secrets, &mut leaves)?;
    let mut current = if leaves == original { text.to_owned() } else { leaves.to_string() };
    loop {
        let ranges = secrets.sensitive_ranges(&current);
        if ranges.is_empty() {
            let parsed = crate::parse_unique_json(&current).map_err(|_| err(UNSAFE_JSON))?;
            if !same_shape(&original, &parsed) { return Err(err(UNSAFE_JSON)); }
            return Ok(current);
        }
        let bytes = current.as_bytes();
        let mut i = 0;
        let mut from = 0;
        let mut next = String::new();
        while i < bytes.len() {
            if bytes[i] != b'"' { i += 1; continue; }
            let start = i;
            i += 1;
            while i < bytes.len() {
                match bytes[i] {
                    b'\\' => i += 2,
                    b'"' => { i += 1; break; }
                    _ => i += 1,
                }
            }
            let is_key = bytes[i..].iter().find(|b| !b.is_ascii_whitespace()) == Some(&b':');
            if !is_key && ranges.iter().any(|r| r.start < i && start < r.end) {
                next.push_str(&current[from..start]);
                next.push_str("\"***\"");
                from = i;
            }
        }
        next.push_str(&current[from..]);
        // A match wholly in keys, syntax or typed data cannot be masked without changing the schema.
        if next == current { return Err(err(UNSAFE_JSON)); }
        current = next;
    }
}

fn protect_json_leaves(secrets: &SecretSet, value: &mut Value) -> MaskResult<()> {
    match value {
        Value::String(text) => *text = safe_final_scalar(secrets, text),
        Value::Array(values) => for value in values { protect_json_leaves(secrets, value)?; },
        Value::Object(values) => for (key, value) in values {
            if secrets.mask(key) != *key || !secrets.sensitive_ranges(&serde_json::to_string(key).expect("string serialization")).is_empty() {
                return Err(err(UNSAFE_JSON));
            }
            protect_json_leaves(secrets, value)?;
        },
        _ => {}
    }
    Ok(())
}

fn same_shape(before: &Value, after: &Value) -> bool {
    match (before, after) {
        (Value::String(_), Value::String(_)) => true,
        (Value::Array(a), Value::Array(b)) => a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_shape(a, b)),
        (Value::Object(a), Value::Object(b)) => a.len() == b.len() && a.iter().all(|(k, a)| b.get(k).is_some_and(|b| same_shape(a, b))),
        _ => before == after,
    }
}


#[derive(Default)]
struct RetainedSecrets {
    identities: std::collections::HashSet<String>,
    combined: SecretSet,
}
impl crate::Book {
    /// Authenticate the actual stored envelope, never caller-provided plaintext.
    /// The host holds only the HKDF credential subkey, not an arbitrary policy.
    pub fn authenticate_bankfeed_restore(&mut self, connection_id: &str, wrapping_key: &[u8; 32]) -> crate::rusqlite::Result<BankfeedRestore> {
        self.authenticate_stored(connection_id, wrapping_key)?.map_err(|()| mask_unavailable())
    }

    /// The single reader of a stored envelope. `Ok(Err(()))` means the credential itself can't be
    /// read: no envelope, wrong lengths, failed authentication or an undecodable record. A database
    /// failure (anything else) stays an `Err`, so it can never be mistaken for an unreadable credential.
    fn authenticate_stored(&mut self, connection_id: &str, wrapping_key: &[u8; 32]) -> crate::rusqlite::Result<Result<BankfeedRestore, ()>> {
        use crate::rusqlite::OptionalExtension;
        let company: String = self.read(|tx| tx.query_row("SELECT company_id FROM bankfeeds_connection WHERE id=?1", [connection_id], |r| r.get(0)))?;
        let row = self.read(|tx| tx.query_row(
            "SELECT wrapped_key,wrap_nonce,cipher_blob,nonce FROM plugin_credential WHERE company_id=?1 AND plugin_id='simplefin' AND connection_id=?2",
            [company.as_str(), connection_id], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?, r.get::<_, Vec<u8>>(2)?, r.get::<_, Vec<u8>>(3)?)),
        ).optional())?;
        let Some((wrapped, wn, encrypted, n)) = row else { return Ok(Err(())) };
        Ok(self.decrypt_stored(connection_id, wrapping_key, company, wrapped, wn, encrypted, n))
    }

    #[allow(clippy::too_many_arguments)]
    fn decrypt_stored(&self, connection_id: &str, wrapping_key: &[u8; 32], company: String, wrapped: Vec<u8>, wn: Vec<u8>, encrypted: Vec<u8>, n: Vec<u8>) -> Result<BankfeedRestore, ()> {
        if wn.len() != 12 || n.len() != 12 { return Err(()); }
        let wrapper = ChaCha20Poly1305::new_from_slice(wrapping_key).map_err(|_| ())?;
        let key = Zeroizing::new(wrapper.decrypt(Nonce::from_slice(&wn), Payload {
            msg: &wrapped, aad: &credential_aad(&company, "simplefin", connection_id, b"key"),
        }).map_err(|_| ())?);
        let cipher = ChaCha20Poly1305::new_from_slice(&key).map_err(|_| ())?;
        let plain = Zeroizing::new(cipher.decrypt(Nonce::from_slice(&n), Payload {
            msg: &encrypted, aad: &credential_aad(&company, "simplefin", connection_id, b"data"),
        }).map_err(|_| ())?);
        #[derive(serde::Deserialize)]
        struct Stored {
            #[serde(deserialize_with = "secret_url")]
            access_url: Zeroizing<String>,
        }
        fn secret_url<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Zeroizing<String>, D::Error> {
            use serde::Deserialize;
            String::deserialize(d).map(Zeroizing::new)
        }
        let record: Stored = serde_json::from_slice(&plain).map_err(|_| ())?;
        Ok(BankfeedRestore { book_identity: self.callback_mode.clone(), company, connection: connection_id.into(), access_url: record.access_url, envelope_hash: Some(envelope_hash(&[&wrapped, &wn, &encrypted, &n])) })
    }

    #[cfg(feature = "test-support")]
    pub fn test_bankfeed_restore(&self, connection_id: &str, access_url: &str) -> BankfeedRestore {
        let company = self.conn.query_row("SELECT company_id FROM bankfeeds_connection WHERE id=?1", [connection_id], |r| r.get(0)).unwrap();
        BankfeedRestore { book_identity: self.callback_mode.clone(), company, connection: connection_id.into(), access_url: Zeroizing::new(access_url.into()), envelope_hash: None }
    }

    /// Only a real SecretSet (with private fields) can install a policy. The caller
    /// cannot supply an arbitrary closure, replace the policy, or clear pending IDs.
    pub fn retain_bankfeed_mask(&mut self, secrets: &SecretSet) -> crate::rusqlite::Result<()> {
        if secrets.is_empty() { return Ok(()); }
        let state = self.retained_text_state::<RetainedSecrets>("bankfeeds")?;
        {
            let mut locked = state.lock().map_err(|_| mask_unavailable())?;
            if locked.identities.insert(secrets.policy_identity()) { locked.combined.merge(secrets); }
        }
        self.retain_text_guard("bankfeeds".into(), move |text| {
            let locked = state.lock().map_err(|_| mask_unavailable())?;
            final_text(&locked.combined, text).map_err(|e| crate::rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
        });
        Ok(())
    }
    pub fn verify_bankfeed_restore(&mut self, connection_id: &str, restore: &BankfeedRestore) -> crate::rusqlite::Result<()> {
        if !Arc::ptr_eq(&self.callback_mode, &restore.book_identity) || restore.connection != connection_id {
            return Err(mask_unavailable());
        }
        let company: String = self.read(|tx| tx.query_row("SELECT company_id FROM bankfeeds_connection WHERE id=?1", [connection_id], |r| r.get(0)))?;
        if company != restore.company { return Err(mask_unavailable()); }
        if let Some(expected) = restore.envelope_hash {
            let (wrapped, wn, encrypted, n) = self.read(|tx| tx.query_row(
                "SELECT wrapped_key,wrap_nonce,cipher_blob,nonce FROM plugin_credential WHERE company_id=?1 AND plugin_id='simplefin' AND connection_id=?2",
                [company.as_str(), connection_id], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?, r.get::<_, Vec<u8>>(2)?, r.get::<_, Vec<u8>>(3)?)),
            ))?;
            if expected != envelope_hash(&[&wrapped, &wn, &encrypted, &n]) { return Err(mask_unavailable()); }
        }
        Ok(())
    }

    /// A capability is usable only on the Book instance, connection and envelope
    /// from which its credential was authenticated. No raw-URL unlock API exists.
    pub fn restore_bankfeed_mask<S: AsRef<str>>(&mut self, connection_id: &str, restore: &BankfeedRestore, extra: impl IntoIterator<Item=S>) -> crate::rusqlite::Result<()> {
        self.verify_bankfeed_restore(connection_id, restore)?;
        let secrets = SecretSet::masking_from_access_url(restore.access_url(), extra)
            .map_err(|e| crate::rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        if secrets.is_empty() { return Err(mask_unavailable()); }
        self.retain_bankfeed_mask(&secrets)?;
        self.credential_mask_restored(connection_id)
    }

    /// Release an ACTIVE connection whose stored credential can't be read with this book's own
    /// credential subkey (no envelope, a damaged one, or a value no mask can be built from), so the
    /// rest of the books can open. Nothing can unmask what nobody can read, and the returned proof only
    /// lets the core mark that connection "reconnect needed". A connection whose credential IS
    /// readable is refused here: it must go through `restore_bankfeed_mask`, so the write guard can
    /// never be lifted for a mask that is expected.
    pub fn release_unreadable_bankfeed(&mut self, connection_id: &str, wrapping_key: &[u8; 32]) -> crate::rusqlite::Result<UnreadableBankfeed> {
        let active: bool = self.read(|tx| tx.query_row("SELECT active FROM bankfeeds_connection WHERE id=?1", [connection_id], |r| r.get(0)))?;
        if !active { return Err(mask_unavailable()); }
        let company = match self.authenticate_stored(connection_id, wrapping_key)? {
            Ok(restore) => {
                if SecretSet::masking_from_access_url(restore.access_url(), Vec::<String>::new()).is_ok_and(|s| !s.is_empty()) {
                    return Err(crate::rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(
                        "This connection's credential is readable; restore it instead."))));
                }
                restore.company.clone()
            }
            Err(()) => self.read(|tx| tx.query_row("SELECT company_id FROM bankfeeds_connection WHERE id=?1", [connection_id], |r| r.get(0)))?,
        };
        self.credential_mask_unavailable(connection_id);
        Ok(UnreadableBankfeed { book_identity: self.callback_mode.clone(), company, connection: connection_id.into() })
    }

    pub fn verify_unreadable_bankfeed(&mut self, connection_id: &str, proof: &UnreadableBankfeed) -> crate::rusqlite::Result<()> {
        if !Arc::ptr_eq(&self.callback_mode, &proof.book_identity) || proof.connection != connection_id {
            return Err(mask_unavailable());
        }
        let company: String = self.read(|tx| tx.query_row("SELECT company_id FROM bankfeeds_connection WHERE id=?1", [connection_id], |r| r.get(0)))?;
        if company != proof.company { return Err(mask_unavailable()); }
        Ok(())
    }
}

/// Proof, issued only by `Book::release_unreadable_bankfeed`, that this Book could not read one
/// connection's stored credential. It carries no secret and unlocks nothing but the reconnect flag.
/// ```compile_fail
/// let _ = fb_db::credential_mask::UnreadableBankfeed {
///     book_identity: Default::default(), company: String::new(), connection: String::new(),
/// };
/// ```
pub struct UnreadableBankfeed {
    book_identity: Arc<AtomicU8>,
    company: String,
    connection: String,
}
impl UnreadableBankfeed {
    pub fn company(&self) -> &str { &self.company }
    pub fn connection(&self) -> &str { &self.connection }
}
fn mask_unavailable() -> crate::rusqlite::Error {
    crate::rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other("The bank credential mask is unavailable; the transaction was refused.")))
}

#[cfg(test)]
mod capability_traits {
    use super::BankfeedRestore;
    #[test]
    fn authenticated_credentials_have_no_debug_or_serialization_surface() {
        trait AmbiguousIfSerialize<A> { fn check() {} }
        impl<T: ?Sized> AmbiguousIfSerialize<()> for T {}
        struct Serializes;
        impl<T: ?Sized + serde::Serialize> AmbiguousIfSerialize<Serializes> for T {}
        let _ = <BankfeedRestore as AmbiguousIfSerialize<_>>::check;
        trait AmbiguousIfDebug<A> { fn check() {} }
        impl<T: ?Sized> AmbiguousIfDebug<()> for T {}
        struct Debugs;
        impl<T: ?Sized + std::fmt::Debug> AmbiguousIfDebug<Debugs> for T {}
        let _ = <BankfeedRestore as AmbiguousIfDebug<_>>::check;
    }

    #[test]
    fn final_writer_error_keeps_the_existing_validation_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let mut book = crate::Book::create(&dir.path().join("mask-error.fbbook"), &[61; 32]).unwrap();
        let secrets = super::SecretSet::new(["SyntheticCredential1234"]).unwrap();
        book.retain_bankfeed_mask(&secrets).unwrap();
        let error = book.write(|tx| tx.execute(
            "INSERT INTO ledger_company(id,name,currency) VALUES(?1,?2,'USD')",
            ["00000000-0000-7000-8000-000000000001", r#"{"SyntheticCredential1234":"value"}"#],
        )).unwrap_err();
        assert!(error.to_string().contains("ValidationError: Bank-feed text cannot be masked while preserving its JSON schema; the transaction was refused."));
        let count: i64 = book.read(|tx| tx.query_row("SELECT count(*) FROM ledger_company", [], |r| r.get(0))).unwrap();
        assert_eq!(count, 0);
    }
}
