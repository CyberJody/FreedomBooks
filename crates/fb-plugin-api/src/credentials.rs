use crate::{PluginError, PluginResult};
use secrecy::{ExposeSecret, SecretBox};
use std::{fmt, sync::Arc};
use uuid::Uuid;

/// Secret bytes are wiped on drop, never serialized, and redacted by Debug.
/// Callers must not log values explicitly exposed for protocol operations.
pub struct SecretBytes(SecretBox<Vec<u8>>);
impl SecretBytes {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(SecretBox::new(Box::new(bytes)))
    }
    pub fn expose(&self) -> &[u8] {
        self.0.expose_secret()
    }
}
impl fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretBytes([REDACTED])")
    }
}

/// Credential and key associated-data identity. Connection ids alone are not sufficient.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CredentialScope {
    pub plugin_id: String,
    pub company_id: Uuid,
    pub connection_id: Uuid,
}

/// Host implementation encrypts at rest with a random per-connection key.
/// No plaintext persistence or unscoped API is permitted. `forget` must wipe the
/// cipher blob AND wrapped key while retaining host-side disconnect metadata.
pub trait CredentialStore: Send + Sync {
    fn load(&self, scope: &CredentialScope) -> PluginResult<Option<SecretBytes>>;
    fn save(&self, scope: &CredentialScope, value: SecretBytes) -> PluginResult<()>;
    fn forget(&self, scope: &CredentialScope) -> PluginResult<()>;
}

/// Plugins receive only this bound handle, never the host's unscoped store.
#[derive(Clone)]
pub struct ScopedCredentials {
    scope: CredentialScope,
    store: Arc<dyn CredentialStore>,
    cancellation: crate::CancellationToken,
}
impl ScopedCredentials {
    pub fn new(scope: CredentialScope, store: Arc<dyn CredentialStore>) -> Self {
        Self {
            scope,
            store,
            cancellation: crate::CancellationToken::new(),
        }
    }
    pub(crate) fn with_cancellation(mut self, cancellation: crate::CancellationToken) -> Self {
        self.cancellation = cancellation;
        self
    }
    pub fn scope(&self) -> &CredentialScope {
        &self.scope
    }
    pub fn load(&self) -> PluginResult<Option<SecretBytes>> {
        self.cancellation.check()?;
        self.store.load(&self.scope)
    }
    pub fn save(&self, value: SecretBytes) -> PluginResult<()> {
        self.cancellation.check()?;
        self.store.save(&self.scope, value)
    }
    pub fn forget(&self) -> PluginResult<()> {
        self.cancellation.check()?;
        self.store.forget(&self.scope)
    }
}
impl fmt::Debug for ScopedCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScopedCredentials")
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

/// Opaque encrypted persistence envelope, not a cryptographic implementation.
/// Host must authenticate the scope, label, nonce and ciphertext as associated data.
/// The master-key HKDF label is fixed by the design, not chosen by a plugin.
pub const CREDENTIAL_KEY_LABEL: &[u8] = b"plugin-credentials";
#[derive(Debug)]
pub struct EncryptedCredential {
    pub cipher_blob: SecretBytes,
    pub nonce: Vec<u8>,
    pub wrapped_key: SecretBytes,
}

/// Wave 4 implements HKDF + authenticated key wrapping under the book master key.
/// A separate random 32-byte key encrypts each connection. Never return the master key.
pub trait ConnectionKeyWrapper: Send + Sync {
    fn wrap(
        &self,
        scope: &CredentialScope,
        connection_key: &SecretBytes,
    ) -> PluginResult<SecretBytes>;
    fn unwrap(
        &self,
        scope: &CredentialScope,
        wrapped_key: &SecretBytes,
    ) -> PluginResult<SecretBytes>;
}

pub(crate) fn check_scope(
    expected: &CredentialScope,
    actual: &CredentialScope,
) -> PluginResult<()> {
    if expected != actual {
        return Err(PluginError::WrongScope);
    }
    Ok(())
}
