//! Cooperative lifecycle contracts; durable state/locking is implemented by the host.
use crate::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// One-way cancellation: never reset a cancelled token for a later operation.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);
impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn check(&self) -> PluginResult<()> {
        if self.0.load(Ordering::SeqCst) {
            Err(PluginError::Cancelled)
        } else {
            Ok(())
        }
    }
}
/// A host-issued scoped operation handle, NOT proof that the book is unlocked.
/// The host must cancel all outstanding handles on lock/close/disable/config change,
/// keep its own unlock lease, and serialize state/credential commits with lock/forget.
#[derive(Clone, Debug)]
pub struct OperationLease {
    scope: CredentialScope,
    cancellation: CancellationToken,
}
impl OperationLease {
    pub fn new(scope: CredentialScope) -> Self {
        Self {
            scope,
            cancellation: CancellationToken::new(),
        }
    }
    pub fn scope(&self) -> &CredentialScope {
        &self.scope
    }
    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }
    pub fn check(&self) -> PluginResult<()> {
        self.cancellation.check()
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RevocationStatus {
    NotApplicable,
    Confirmed,
    /// Forgotten locally, but upstream revocation was not confirmed (including
    /// providers with no supported revocation endpoint). Never label this revoked.
    Unknown,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectionUpdate {
    pub scope: CredentialScope,
    pub status: ConnectionStatus,
    /// Access-token expiry alone need not mean sign-in is required: refresh first.
    pub expires_at: Option<Timestamp>,
    pub revocation: RevocationStatus,
}
impl ConnectionUpdate {
    pub fn validate_for(&self, cx: &PluginContext) -> PluginResult<()> {
        crate::check_scope(cx.scope(), &self.scope)?;
        cx.check_active()
    }
}
/// Optional object-safe lifecycle, obtained through Plugin::connection_lifecycle.
/// Concrete providers decode ConnectInput, retain provider-specific validation and
/// serialize rotating-token use. The host persists returned status atomically with
/// credentials/audit in wave 4; this trait performs no automatic registry changes.
pub trait ConnectionLifecycle: Send + Sync {
    fn begin(
        &self,
        cx: &PluginContext,
        input: ConnectInput,
    ) -> PluginResult<Box<dyn PendingConnection>>;
    fn disconnect(&self, cx: &PluginContext) -> PluginResult<ConnectionUpdate>;
}
/// Provider-owned attempt. Drop closes its listener. Implementations must capture
/// scope, config generation and cancellation and recheck before saving credentials.
pub trait PendingConnection: Send {
    fn authorization_url(&self) -> &SecretString;
    fn complete(self: Box<Self>, cx: &PluginContext) -> PluginResult<ConnectionUpdate>;
}
