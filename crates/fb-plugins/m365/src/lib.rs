//! Delegated Microsoft Graph document delivery. No core/database dependency.
#![forbid(unsafe_code)]

mod lifecycle;
mod mail;
mod tokens;

use fb_plugin_api::oauth::{OAuthConfig, OAuthEndpoints, OAuthLoopback};
use fb_plugin_api::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub use mail::{MAX_ATTACHMENT_BYTES, MAX_MESSAGE_BYTES};

const AUTHORIZE: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/authorize";
const TOKEN: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/token";
const SEND: &str = "https://graph.microsoft.com/v1.0/me/sendMail";
const SCOPES: &[&str] = &["Mail.Send", "offline_access"];

pub static MANIFEST: PluginManifest = PluginManifest {
    id: "m365",
    name: "Microsoft mail",
    version: env!("CARGO_PKG_VERSION"),
    slots: &[Slot::DocumentSender],
    auth: AuthKind::OAuthLoopback {
        provider: "Microsoft identity platform",
    },
    outside_account: "Microsoft personal or work/school account with an Outlook mailbox",
    scopes: SCOPES,
    data_reads: &[DataKind::DocumentContents, DataKind::RecipientAddresses],
    data_sends: &[DataKind::DocumentContents, DataKind::RecipientAddresses],
    settings_fields: &[SettingsField {
        key: "client_id",
        label: "Microsoft Entra application (client) ID",
        kind: FieldKind::Text,
        required: true,
    }],
    network_hosts: &["login.microsoftonline.com", "graph.microsoft.com"],
};

/// Keep one instance per configured application in the host registry. The lock
/// serializes credential rotation, disconnect and send across its connections.
/// Network calls must be run off the UI thread, like all framework slot calls.
#[derive(Clone)]
pub struct M365Sender {
    client_id: String,
    authorize_endpoint: String,
    token_endpoint: String,
    send_endpoint: String,
    operations: Arc<Mutex<OperationState>>,
}

/// A one-shot callback listener bound to the company/connection that started it.
/// Dropping it cancels sign-in and closes the listener. No Debug exposes its URL.
pub struct PendingAuthorization {
    generation: u64,
    flow: OAuthLoopback,
    scope: CredentialScope,
    client_id: String,
    cancellation: CancellationToken,
}
#[derive(Default)]
struct OperationState {
    next_generation: u64,
    attempts: HashMap<CredentialScope, u64>,
}

impl PendingAuthorization {
    /// Give only to the system-browser opener, never logs or an embedded webview.
    pub fn authorization_url(&self) -> &SecretString {
        self.flow.authorization_url()
    }
}

impl M365Sender {
    pub fn new(client_id: &str) -> PluginResult<Self> {
        // Entra application IDs are UUIDs, not secrets. No embedded registration.
        if Uuid::parse_str(client_id).is_err() || client_id.len() != 36 {
            return Err(PluginError::InvalidConfiguration);
        }
        if Uuid::parse_str(client_id).is_ok_and(|id| id.is_nil()) {
            return Err(PluginError::InvalidConfiguration);
        }
        Ok(Self {
            client_id: client_id.into(),
            authorize_endpoint: AUTHORIZE.into(),
            token_endpoint: TOKEN.into(),
            send_endpoint: SEND.into(),
            operations: Arc::new(Mutex::new(OperationState::default())),
        })
    }

    pub fn start_authorization(&self, cx: &PluginContext) -> PluginResult<PendingAuthorization> {
        self.check_scope(cx)?;
        let endpoints = self.oauth_endpoints()?;
        let mut config = OAuthConfig::new(
            endpoints,
            self.client_id.clone(),
            SCOPES.iter().map(|s| (*s).into()).collect(),
            "/callback".into(),
        );
        config.extra_authorization_parameters = vec![
            ("response_mode".into(), "query".into()),
            ("prompt".into(), "select_account".into()),
        ];
        let flow = OAuthLoopback::start(config)?;
        let mut state = self
            .operations
            .lock()
            .map_err(|_| PluginError::CredentialStore)?;
        state.next_generation = state
            .next_generation
            .checked_add(1)
            .ok_or(PluginError::InvalidConfiguration)?;
        let generation = state.next_generation;
        state.attempts.insert(cx.scope().clone(), generation);
        Ok(PendingAuthorization {
            generation,
            flow,
            scope: cx.scope().clone(),
            client_id: self.client_id.clone(),
            cancellation: cx.cancellation().clone(),
        })
    }

    fn oauth_endpoints(&self) -> PluginResult<OAuthEndpoints> {
        // No public endpoint override or insecure production feature.
        #[cfg(test)]
        if self.authorize_endpoint.starts_with("http://127.0.0.1:") {
            return OAuthEndpoints::loopback_for_tests(
                &self.authorize_endpoint,
                &self.token_endpoint,
            );
        }
        OAuthEndpoints::new(&self.authorize_endpoint, &self.token_endpoint)
    }

    /// Wait for the callback, exchange the PKCE code, and save through the scoped
    /// credential store. OAuth authorization only: no openid/User.Read/profile,
    /// no identity claim, and no unverified ID token is ever accepted.
    pub fn complete_authorization(
        &self,
        cx: &PluginContext,
        pending: PendingAuthorization,
    ) -> PluginResult<()> {
        self.audited(
            cx,
            AuditAction::Connect,
            AuditPurpose::Connection,
            |counts| {
                self.check_scope(cx)?;
                if pending.scope != *cx.scope() || pending.client_id != self.client_id {
                    return Err(PluginError::WrongScope);
                }
                {
                    let state = self
                        .operations
                        .lock()
                        .map_err(|_| PluginError::CredentialStore)?;
                    if state.attempts.get(cx.scope()) != Some(&pending.generation) {
                        return Err(PluginError::OAuthState);
                    }
                }
                pending.cancellation.check()?;
                let grant = pending.flow.wait_cancellable(cx.cancellation())?;
                pending.cancellation.check()?;
                cx.check_active()?;
                let mut state = self
                    .operations
                    .lock()
                    .map_err(|_| PluginError::CredentialStore)?;
                // Disconnect or a newer Connect can invalidate a waiting callback.
                if state.attempts.get(cx.scope()) != Some(&pending.generation) {
                    return Err(PluginError::OAuthState);
                }
                state.attempts.remove(cx.scope());
                let record = tokens::exchange(cx, &self.client_id, &grant, counts)?;
                pending.cancellation.check()?;
                cx.check_active()?;
                tokens::save(cx, &record)
            },
        )
    }

    /// Local credential removal only, NOT upstream revocation. Microsoft account
    /// consent removal is a separate user/admin action; see README.
    pub fn disconnect(&self, cx: &PluginContext) -> PluginResult<()> {
        self.audited(
            cx,
            AuditAction::Disconnect,
            AuditPurpose::Connection,
            |_| {
                self.check_scope(cx)?;
                let mut state = self
                    .operations
                    .lock()
                    .map_err(|_| PluginError::CredentialStore)?;
                state.attempts.remove(cx.scope());
                cx.credentials().forget()
            },
        )
    }

    fn check_scope(&self, cx: &PluginContext) -> PluginResult<()> {
        if cx.scope().plugin_id != MANIFEST.id {
            return Err(PluginError::WrongScope);
        }
        cx.check_active()
    }

    fn audited<T>(
        &self,
        cx: &PluginContext,
        action: AuditAction,
        purpose: AuditPurpose,
        operation: impl FnOnce(&mut (u64, u64)) -> PluginResult<T>,
    ) -> PluginResult<T> {
        let start = Instant::now();
        let mut counts = (0, 0);
        let result = operation(&mut counts);
        let outcome = match &result {
            Ok(_) => AuditOutcome::Success,
            Err(PluginError::NeedsSignIn) => AuditOutcome::NeedsSignIn,
            Err(
                PluginError::OAuthDenied
                | PluginError::WrongScope
                | PluginError::InvalidConfiguration
                | PluginError::TooLarge,
            ) => AuditOutcome::Refused,
            Err(_) => AuditOutcome::Failed,
        };
        cx.record_audit(action, purpose, outcome, counts, start.elapsed())?;
        result
    }
}

impl Plugin for M365Sender {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }
    fn connection_lifecycle(&self) -> Option<&dyn ConnectionLifecycle> {
        Some(self)
    }
}
impl DocumentSender for M365Sender {
    fn send(&self, cx: &PluginContext, document: OutgoingDocument) -> PluginResult<SendReceipt> {
        match self.send_outcome(cx, document) {
            SendOutcome::Accepted {
                receipt,
                recording_error: None,
            } => Ok(receipt),
            SendOutcome::Accepted {
                recording_error: Some(error),
                ..
            }
            | SendOutcome::NotSubmitted { error }
            | SendOutcome::Uncertain { error } => Err(error),
        }
    }
    fn send_outcome(&self, cx: &PluginContext, document: OutgoingDocument) -> SendOutcome {
        let start = Instant::now();
        let mut counts = (0, 0);
        let mut submitted = false;
        let result = (|| -> PluginResult<SendReceipt> {
            self.check_scope(cx)?;
            // Reject malformed/oversized documents before credentials/network.
            if document.document.id.is_nil() {
                return Err(PluginError::InvalidConfiguration);
            }
            let body = mail::encode(&document)?;
            let _guard = self
                .operations
                .lock()
                .map_err(|_| PluginError::CredentialStore)?;
            let access =
                tokens::access_token(cx, &self.client_id, &self.token_endpoint, &mut counts)?;
            counts.0 += body.expose().len() as u64;
            cx.check_active()?;
            submitted = true;
            let response = cx.http().execute(HttpRequest {
                method: HttpMethod::Post,
                url: SecretString::from(self.send_endpoint.clone()),
                headers: vec![
                    (
                        "Content-Type".into(),
                        SecretString::from("application/json"),
                    ),
                    (
                        "Authorization".into(),
                        SecretString::from(format!("Bearer {}", access.expose_secret())),
                    ),
                ],
                body,
            })?;
            counts.1 += response.body.expose().len() as u64;
            match response.status {
                // Graph acceptance never supplies a message id or delivery proof.
                202 => Ok(SendReceipt {
                    provider_message_id: None,
                    sent_at: cx.clock().now(),
                }),
                400 | 401 | 403 | 404 | 413 | 429 => {
                    submitted = false;
                    match response.status {
                        401 => {
                            cx.credentials().forget()?;
                            Err(PluginError::NeedsSignIn)
                        }
                        403 => Err(PluginError::OAuthDenied),
                        413 => Err(PluginError::TooLarge),
                        429 => Err(PluginError::Network),
                        _ => Err(PluginError::InvalidResponse),
                    }
                }
                500..=599 => Err(PluginError::Network),
                _ => Err(PluginError::InvalidResponse),
            }
        })();
        match result {
            Ok(receipt) => {
                let recording_error = cx
                    .record_document_send_audit(DocumentSendAudit {
                        event: AuditEvent {
                            scope: cx.scope().clone(),
                            actor: cx.actor(),
                            at: receipt.sent_at,
                            action: AuditAction::Send,
                            purpose: AuditPurpose::DocumentDelivery,
                            outcome: AuditOutcome::Success,
                            bytes_sent: counts.0,
                            bytes_received: counts.1,
                            duration: start.elapsed(),
                        },
                        document: document.document,
                        receipt: receipt.clone(),
                    })
                    .err();
                SendOutcome::Accepted {
                    receipt,
                    recording_error,
                }
            }
            Err(error) => {
                let outcome = match error {
                    PluginError::NeedsSignIn => AuditOutcome::NeedsSignIn,
                    PluginError::OAuthDenied
                    | PluginError::WrongScope
                    | PluginError::InvalidConfiguration
                    | PluginError::TooLarge => AuditOutcome::Refused,
                    _ => AuditOutcome::Failed,
                };
                let _ = cx.record_audit(
                    AuditAction::Send,
                    AuditPurpose::DocumentDelivery,
                    outcome,
                    counts,
                    start.elapsed(),
                );
                if submitted {
                    SendOutcome::Uncertain { error }
                } else {
                    SendOutcome::NotSubmitted { error }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
