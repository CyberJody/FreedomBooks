//! Desktop credential persistence. Owns the SAME Book writer while a plugin runs;
//! there is no second connection, plaintext sidecar, or alternate mutation path.
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce, aead::{Aead, Payload}};
use fb_core::{Book, CompanyId, UserId};
use fb_db::{credential_mask::{BankfeedRestore, UnreadableBankfeed}, plugin_credentials::CredentialEnvelope};
use fb_plugin_api::{CredentialScope, CredentialStore, PluginError, PluginResult, SecretBytes};
use hkdf::Hkdf;
use sha2::Sha256;
use std::sync::Mutex;
use zeroize::Zeroizing;

pub struct BookCredentials {
    book: Mutex<Book>,
    actor: UserId,
    wrapping_key: Zeroizing<[u8; 32]>,
    /// Other keys a saved credential may have been written under (the old book-derived vault, or a
    /// superseded connection key). Read with, then re-saved under `wrapping_key`; never written with.
    legacy_keys: Vec<Zeroizing<[u8; 32]>>,
}

#[derive(Default)]
struct BankfeedMasks {
    loaded: Vec<(String, String, BankfeedRestore)>,
    unreadable: Vec<(String, String, UnreadableBankfeed)>,
}

impl BookCredentials {
    pub fn new(book: Book, actor: UserId, master: &[u8; 32]) -> PluginResult<Self> {
        Ok(Self::with_wrapping_key(book, actor, wrapping_key(master)?))
    }

    /// The vault over an open book, from the session's already-derived subkey.
    pub(crate) fn with_wrapping_key(book: Book, actor: UserId, wrapping_key: Zeroizing<[u8; 32]>) -> Self {
        Self { book: Mutex::new(book), actor, wrapping_key, legacy_keys: vec![] }
    }

    /// Only an old book without a portable key record may migrate its former book-derived vault.
    /// `alternates` are superseded connection keys this computer kept aside.
    pub(crate) fn with_device_key(book: Book, actor: UserId, device: &[u8; 32], legacy: Option<&[u8; 32]>, alternates: &[&[u8; 32]]) -> PluginResult<Self> {
        let mut store = Self::new(book, actor, device)?;
        store.legacy_keys = Self::legacy_wrapping_keys(device, legacy, alternates)?;
        Ok(store)
    }

    fn legacy_wrapping_keys(device: &[u8; 32], legacy: Option<&[u8; 32]>, alternates: &[&[u8; 32]]) -> PluginResult<Vec<Zeroizing<[u8; 32]>>> {
        legacy.into_iter().chain(alternates.iter().copied()).filter(|k| *k != device).map(wrapping_key).collect()
    }

    pub fn into_book(self) -> PluginResult<Book> {
        self.book.into_inner().map_err(|_| PluginError::CredentialStore)
    }

    fn prepare_bankfeed_masks(book: &mut Book, primary: &[u8; 32], legacy: &[Zeroizing<[u8; 32]>]) -> PluginResult<BankfeedMasks> {
        let scopes = {
            book.read(|tx| {
                let mut st = tx.prepare("SELECT c.company_id,c.id,c.active FROM bankfeeds_connection c WHERE c.active=1 OR EXISTS(SELECT 1 FROM plugin_credential v WHERE v.company_id=c.company_id AND v.plugin_id='simplefin' AND v.connection_id=c.id) ORDER BY c.company_id,c.id")?;
                st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, bool>(2)?)))?
                    .collect::<Result<Vec<_>, _>>()
            }).map_err(|_| PluginError::CredentialStore)?
        };
        let mut masks = BankfeedMasks::default();
        for (company, connection, active) in scopes {
            // Install masks for the entire file before ANY Writer is allowed. This internal
            // restore never exposes credentials or grants company access to the signed-in user.
            let keys = std::iter::once(primary).chain(legacy.iter().map(|key| &**key));
            match book.find_bankfeed_restore(&connection, keys).map_err(|_| PluginError::CredentialStore)? {
                Some((record, key)) => {
                    if book.restore_bankfeed_mask(&connection, &record, Vec::<String>::new()).is_ok() {
                        masks.loaded.push((company, connection, record));
                    } else if active {
                        let proof = book.release_unreadable_bankfeed(&connection, key).map_err(|_| PluginError::CredentialStore)?;
                        masks.unreadable.push((company, connection, proof));
                    }
                }
                // A forgotten connection is not write-guarded and is never used again.
                None if !active => {}
                // Refused (Err) when the credential is in fact readable: then the open is refused too.
                None => {
                    let proof = book.release_unreadable_bankfeed(&connection, primary)
                        .map_err(|_| PluginError::CredentialStore)?;
                    masks.unreadable.push((company, connection, proof));
                }
            }
        }
        Ok(masks)
    }

    pub(crate) fn prepare_bankfeeds_for_write(book: &mut Book, device: &[u8; 32], legacy: Option<&[u8; 32]>, alternates: &[&[u8; 32]]) -> PluginResult<()> {
        let primary = wrapping_key(device)?;
        Self::prepare_bankfeed_masks(book, &primary, &Self::legacy_wrapping_keys(device, legacy, alternates)?).map(drop)
    }

    /// Prepare every mask before any durable repair. Returns the names awaiting reconnection.
    pub(crate) fn restore_bankfeeds(&self) -> PluginResult<Vec<String>> {
        let mut book = self.book.lock().map_err(|_| PluginError::CredentialStore)?;
        let BankfeedMasks { loaded, unreadable } = Self::prepare_bankfeed_masks(&mut book, &self.wrapping_key, &self.legacy_keys)?;
        for (company, connection, record) in loaded {
            // An incompatible legacy value remains masked, quarantined and unable
            // to sync.
            let co = CompanyId::parse(&company).ok_or(PluginError::CredentialStore)?;
            if book.read(|q| fb_core::services::access(q,self.actor,co)).is_err() { continue; }
            if let Err(error) = fb_core::bankfeeds::restore_connection_credentials(
                &mut book, self.actor, co, &connection, &record, Vec::<String>::new(),
            ) && error.message() != fb_core::bankfeeds::UNSAFE_CREDENTIAL {
                return Err(PluginError::CredentialStore);
            }
        }
        let mut labels = Vec::new();
        for (company, connection, proof) in unreadable {
            let co = CompanyId::parse(&company).ok_or(PluginError::CredentialStore)?;
            if book.read(|q| fb_core::services::access(q,self.actor,co)).is_err() { continue; }
            labels.push(fb_core::bankfeeds::flag_unreadable_credential(&mut book, self.actor, co, &connection, &proof)
                .map_err(|_| PluginError::CredentialStore)?);
        }
        drop(book);
        self.migrate_legacy_envelopes()?;
        self.restore_ai_credentials()?;
        Ok(labels)
    }

    /// The connection key is unavailable this session (no working keyring). Every active
    /// connection's write guard is released for this open Book only, with a key that cannot read it,
    /// so the books stay writable. Nothing durable changes: no "reconnect needed" flag, no audit,
    /// no AI status, no envelope migration. Returns how many connections are waiting.
    pub(crate) fn release_bankfeeds_unavailable(book: &mut Book, key: &[u8; 32]) -> PluginResult<usize> {
        let active = book.read(|tx| {
            tx.prepare("SELECT id FROM bankfeeds_connection WHERE active=1 ORDER BY company_id,id")?
                .query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()
        }).map_err(|_| PluginError::CredentialStore)?;
        for connection in &active {
            book.release_unreadable_bankfeed(connection, key).map_err(|_| PluginError::CredentialStore)?;
        }
        Ok(active.len())
    }

    fn restore_ai_credentials(&self) -> PluginResult<()> {
        let companies = self.book.lock().map_err(|_| PluginError::CredentialStore)?.read(|q| {
            q.prepare("SELECT company_id FROM assistant_connection WHERE status='connected' AND credential_present=1 AND company_id IN (SELECT company_id FROM ledger_company_members WHERE user_id=?1)")?
                .query_map([self.actor.to_db()], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()
        }).map_err(|_| PluginError::CredentialStore)?;
        for company in companies {
            let scope = CredentialScope { company_id:company.parse().map_err(|_| PluginError::WrongScope)?, plugin_id:"openai".into(), connection_id:company.parse().map_err(|_| PluginError::WrongScope)? };
            // Only a saved envelope this installation cannot read (another computer's key) needs a reconnect.
            if self.load(&scope).is_ok() { continue; }
            let co = CompanyId::parse(&company).ok_or(PluginError::WrongScope)?;
            let mut book = self.book.lock().map_err(|_| PluginError::CredentialStore)?;
            fb_core::assistant::local_credentials_unavailable(&mut book,self.actor,co).map_err(|_| PluginError::CredentialStore)?;
        }
        Ok(())
    }

    fn migrate_legacy_envelopes(&self) -> PluginResult<()> {
        if self.legacy_keys.is_empty() { return Ok(()) }
        let scopes = self.book.lock().map_err(|_| PluginError::CredentialStore)?.read(|q| {
            q.prepare("SELECT company_id,plugin_id,connection_id FROM plugin_credential WHERE company_id IN (SELECT company_id FROM ledger_company_members WHERE user_id=?1) ORDER BY company_id,plugin_id,connection_id")?
                .query_map([self.actor.to_db()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?
                .collect::<Result<Vec<_>, _>>()
        }).map_err(|_| PluginError::CredentialStore)?;
        let mut migrated = false;
        for (company, plugin_id, connection) in scopes {
            let scope = CredentialScope { company_id: company.parse().map_err(|_| PluginError::WrongScope)?, plugin_id, connection_id: connection.parse().map_err(|_| PluginError::WrongScope)? };
            if self.load(&scope).is_ok() { continue; }
            for legacy in &self.legacy_keys {
                if let Ok(Some(value)) = self.load_key(&scope, legacy) { self.save(&scope, value)?; migrated = true; break; }
            }
        }
        if migrated { self.book.lock().map_err(|_| PluginError::CredentialStore)?.compact_secret_pages().map_err(|_| PluginError::CredentialStore)?; }
        Ok(())
    }

    fn load_key(&self, scope: &CredentialScope, wrapping_key: &[u8; 32]) -> PluginResult<Option<SecretBytes>> {
        let mut book = self.book.lock().map_err(|_| PluginError::CredentialStore)?;
        self.check(&mut book, scope)?;
        let row = book.credential_envelope(&scope.company_id.to_string(), &scope.plugin_id, &scope.connection_id.to_string())
            .map_err(|_| PluginError::CredentialStore)?;
        let Some(CredentialEnvelope { wrapped_key: wrapped, wrap_nonce: wn, cipher_blob: encrypted, nonce: n }) = row else { return Ok(None) };
        if wn.len() != 12 || n.len() != 12 { return Err(PluginError::CredentialStore); }
        let key = Zeroizing::new(cipher(wrapping_key)?.decrypt(Nonce::from_slice(&wn), Payload { msg: &wrapped, aad: &aad(scope, b"key") })
            .map_err(|_| PluginError::CredentialStore)?);
        let plain = Zeroizing::new(cipher(&key)?.decrypt(Nonce::from_slice(&n), Payload { msg: &encrypted, aad: &aad(scope, b"data") })
            .map_err(|_| PluginError::CredentialStore)?);
        Ok(Some(SecretBytes::new(plain.to_vec())))
    }

    fn check(&self, book: &mut Book, scope: &CredentialScope) -> PluginResult<()> {
        if scope.plugin_id.is_empty() || scope.plugin_id.len() > 100 {
            return Err(PluginError::WrongScope);
        }
        let company = CompanyId::parse(&scope.company_id.to_string()).ok_or(PluginError::WrongScope)?;
        book.read(|tx| fb_core::services::access(tx, self.actor, company))
            .map_err(|_| PluginError::WrongScope)?;
        Ok(())
    }
}

/// Domain separation of the independent installation secret (never the book key).
pub(crate) fn wrapping_key(master: &[u8; 32]) -> PluginResult<Zeroizing<[u8; 32]>> {
    let mut key = Zeroizing::new([0; 32]);
    Hkdf::<Sha256>::new(None, master)
        .expand(fb_plugin_api::CREDENTIAL_KEY_LABEL, &mut *key)
        .map_err(|_| PluginError::CredentialStore)?;
    Ok(key)
}

// Length-prefixed components make scope encodings injective, even for unusual plugin IDs.
fn aad(scope: &CredentialScope, purpose: &[u8]) -> Vec<u8> {
    fb_db::credential_mask::credential_aad(&scope.company_id.to_string(), &scope.plugin_id, &scope.connection_id.to_string(), purpose)
}

fn cipher(key: &[u8]) -> PluginResult<ChaCha20Poly1305> {
    ChaCha20Poly1305::new_from_slice(key).map_err(|_| PluginError::CredentialStore)
}
fn random<const N: usize>() -> PluginResult<[u8; N]> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(|_| PluginError::CredentialStore)?;
    Ok(bytes)
}

impl CredentialStore for BookCredentials {
    fn load(&self, scope: &CredentialScope) -> PluginResult<Option<SecretBytes>> {
        self.load_key(scope, &self.wrapping_key)
    }

    fn save(&self, scope: &CredentialScope, value: SecretBytes) -> PluginResult<()> {
        if value.expose().len() > 1024 * 1024 { return Err(PluginError::CredentialStore); }
        let mut book = self.book.lock().map_err(|_| PluginError::CredentialStore)?;
        self.check(&mut book, scope)?;
        let mut key = Zeroizing::new([0; 32]);
        getrandom::fill(&mut *key).map_err(|_| PluginError::CredentialStore)?;
        let wn = random::<12>()?;
        let n = random::<12>()?;
        let wrapped = cipher(&*self.wrapping_key)?.encrypt(Nonce::from_slice(&wn), Payload { msg: &*key, aad: &aad(scope, b"key") })
            .map_err(|_| PluginError::CredentialStore)?;
        let encrypted = cipher(&*key)?.encrypt(Nonce::from_slice(&n), Payload { msg: value.expose(), aad: &aad(scope, b"data") })
            .map_err(|_| PluginError::CredentialStore)?;
        book.put_credential_envelope(&scope.company_id.to_string(), &scope.plugin_id, &scope.connection_id.to_string(),
            &CredentialEnvelope { wrapped_key: wrapped, wrap_nonce: wn.to_vec(), cipher_blob: encrypted, nonce: n.to_vec() })
            .map_err(|_| PluginError::CredentialStore)?;
        Ok(())
    }

    fn forget(&self, scope: &CredentialScope) -> PluginResult<()> {
        let mut book = self.book.lock().map_err(|_| PluginError::CredentialStore)?;
        self.check(&mut book, scope)?;
        book.delete_credential_envelope(&scope.company_id.to_string(), &scope.plugin_id, &scope.connection_id.to_string())
            .map_err(|_| PluginError::CredentialStore)?;
        Ok(())
    }
}
