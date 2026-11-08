//! Desktop credential persistence. Owns the SAME Book writer while a plugin runs;
//! there is no second connection, plaintext sidecar, or alternate mutation path.
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce, aead::{Aead, Payload}};
use fb_core::{Book, CompanyId, UserId};
use fb_db::plugin_credentials::CredentialEnvelope;
use fb_plugin_api::{CredentialScope, CredentialStore, PluginError, PluginResult, SecretBytes};
use hkdf::Hkdf;
use sha2::Sha256;
use std::sync::Mutex;
use zeroize::Zeroizing;

pub struct BookCredentials {
    book: Mutex<Book>,
    actor: UserId,
    wrapping_key: Zeroizing<[u8; 32]>,
    legacy_key: Option<Zeroizing<[u8; 32]>>,
}

impl BookCredentials {
    pub fn new(book: Book, actor: UserId, master: &[u8; 32]) -> PluginResult<Self> {
        Ok(Self::with_wrapping_key(book, actor, wrapping_key(master)?))
    }

    /// The vault over an open book, from the session's already-derived subkey.
    pub(crate) fn with_wrapping_key(book: Book, actor: UserId, wrapping_key: Zeroizing<[u8; 32]>) -> Self {
        Self { book: Mutex::new(book), actor, wrapping_key, legacy_key: None }
    }

    /// Only an old book without a portable key record may migrate its former book-derived vault.
    pub(crate) fn with_device_key(book: Book, actor: UserId, device: &[u8; 32], legacy: Option<&[u8; 32]>) -> PluginResult<Self> {
        let mut store = Self::new(book, actor, device)?;
        store.legacy_key = legacy.map(wrapping_key).transpose()?;
        Ok(store)
    }

    pub fn into_book(self) -> PluginResult<Book> {
        self.book.into_inner().map_err(|_| PluginError::CredentialStore)
    }

    /// Complete restoration before the host publishes an Open session. First
    /// install ALL masks: legacy quarantine audit writes must also be protected
    /// by the masks of the company's other connections.
    ///
    /// A connection whose stored credential can't be read (missing or damaged envelope) never makes
    /// the books unopenable (lead decision, WAVE4 fix 1): fb-db releases only that connection, after
    /// proving with the book's own subkey that it is unreadable, and the core marks it "reconnect
    /// needed" (audited). Every readable connection keeps restore-before-any-command. Returns the
    /// labels of the connections newly or still waiting for a reconnect.
    pub(crate) fn restore_bankfeeds(&self) -> PluginResult<Vec<String>> {
        let scopes = {
            let mut book = self.book.lock().map_err(|_| PluginError::CredentialStore)?;
            book.read(|tx| {
                let mut st = tx.prepare("SELECT c.company_id,c.id,c.active FROM bankfeeds_connection c WHERE c.active=1 OR EXISTS(SELECT 1 FROM plugin_credential v WHERE v.company_id=c.company_id AND v.plugin_id='simplefin' AND v.connection_id=c.id) ORDER BY c.company_id,c.id")?;
                st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, bool>(2)?)))?
                    .collect::<Result<Vec<_>, _>>()
            }).map_err(|_| PluginError::CredentialStore)?
        };
        let mut loaded = Vec::new();
        let mut unreadable = Vec::new();
        for (company, connection, active) in scopes {
            // Install masks for the entire file before ANY Writer is allowed. This internal
            // restore never exposes credentials or grants company access to the signed-in user.
            let mut book = self.book.lock().map_err(|_| PluginError::CredentialStore)?;
            let restored = book.authenticate_bankfeed_restore(&connection, &self.wrapping_key)
                .or_else(|e| match &self.legacy_key {
                    Some(key) => book.authenticate_bankfeed_restore(&connection, key),
                    None => Err(e),
                })
                .and_then(|record| book.restore_bankfeed_mask(&connection, &record, Vec::<String>::new()).map(|()| record));
            match restored {
                Ok(record) => loaded.push((company, connection, record)),
                // A forgotten connection is not write-guarded and is never used again.
                Err(_) if !active => {}
                // Refused (Err) when the credential is in fact readable: then the open is refused too.
                Err(_) => {
                    let proof = book.release_unreadable_bankfeed(&connection, &self.wrapping_key)
                        .map_err(|_| PluginError::CredentialStore)?;
                    unreadable.push((company, connection, proof));
                }
            }
        }
        let mut book = self.book.lock().map_err(|_| PluginError::CredentialStore)?;
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
        let Some(legacy) = &self.legacy_key else { return Ok(()) };
        let scopes = self.book.lock().map_err(|_| PluginError::CredentialStore)?.read(|q| {
            q.prepare("SELECT company_id,plugin_id,connection_id FROM plugin_credential WHERE company_id IN (SELECT company_id FROM ledger_company_members WHERE user_id=?1) ORDER BY company_id,plugin_id,connection_id")?
                .query_map([self.actor.to_db()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?
                .collect::<Result<Vec<_>, _>>()
        }).map_err(|_| PluginError::CredentialStore)?;
        let mut migrated = false;
        for (company, plugin_id, connection) in scopes {
            let scope = CredentialScope { company_id: company.parse().map_err(|_| PluginError::WrongScope)?, plugin_id, connection_id: connection.parse().map_err(|_| PluginError::WrongScope)? };
            if self.load(&scope).is_ok() { continue; }
            if let Ok(Some(value)) = self.load_key(&scope, legacy) { self.save(&scope, value)?; migrated = true; }
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
