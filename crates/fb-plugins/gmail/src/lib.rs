//! Send-only Gmail adapter. The host supplies an unlocked, enabled connection,
//! opens authorization URLs in the system browser, and persists credentials encrypted.
#![forbid(unsafe_code)]

mod lifecycle;
mod mime;
#[cfg(test)]
mod tests;
mod tokens;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use fb_plugin_api::{oauth::*, *};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokens::Tokens;
use zeroize::Zeroizing;

pub const SEND_SCOPE: &str = "https://www.googleapis.com/auth/gmail.send";
pub static MANIFEST: PluginManifest = PluginManifest {
    id: "gmail",
    name: "Gmail document sender",
    version: env!("CARGO_PKG_VERSION"),
    slots: &[Slot::DocumentSender],
    auth: AuthKind::OAuthLoopback { provider: "Google" },
    outside_account: "A Gmail or Google Workspace mailbox",
    scopes: &[SEND_SCOPE],
    data_reads: &[DataKind::DocumentContents, DataKind::RecipientAddresses],
    data_sends: &[DataKind::DocumentContents, DataKind::RecipientAddresses],
    settings_fields: &[
        SettingsField {
            key: "client_id",
            label: "Google Desktop app client id",
            kind: FieldKind::Text,
            required: true,
        },
        SettingsField {
            key: "client_secret",
            label: "Desktop app client secret (if issued)",
            kind: FieldKind::Secret,
            required: false,
        },
        SettingsField {
            key: "from_address",
            label: "Gmail mailbox or authorized send-as address",
            kind: FieldKind::Text,
            required: true,
        },
    ],
    network_hosts: &[
        "accounts.google.com",
        "oauth2.googleapis.com",
        "gmail.googleapis.com",
    ],
};

#[derive(Clone)]
struct Endpoints {
    authorize: String,
    token: String,
    revoke: String,
    send: String,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            authorize: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token: "https://oauth2.googleapis.com/token".into(),
            revoke: "https://oauth2.googleapis.com/revoke".into(),
            send: "https://gmail.googleapis.com/gmail/v1/users/me/messages/send".into(),
        }
    }
}
#[derive(Default)]
struct State {
    epochs: HashMap<CredentialScope, u64>,
}

/// Share one instance via the registry. Its lock serializes refresh/send/forget,
/// including rotating refresh-token persistence. It never exposes Debug secrets.
#[derive(Clone)]
pub struct GmailSender {
    client_id: String,
    client_secret: Option<SecretString>,
    endpoints: Endpoints,
    state: Arc<Mutex<State>>,
}

/// One-shot attempt, bound to company, connection, client and cancellation epoch.
/// Dropping it closes the loopback socket. No grant can be injected by the host.
pub struct GmailAuthorization {
    flow: OAuthLoopback,
    scope: CredentialScope,
    client_id: String,
    epoch: u64,
    from_address: String,
    cancellation: CancellationToken,
}
impl GmailAuthorization {
    pub fn authorization_url(&self) -> &SecretString {
        self.flow.authorization_url()
    }
    pub fn redirect_uri(&self) -> &str {
        self.flow.redirect_uri()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DisconnectReceipt {
    /// Local credentials are forgotten even when the provider is unavailable.
    pub revocation_confirmed: bool,
}

#[derive(Default)]
struct IoStats {
    sent: u64,
    received: u64,
}
impl GmailSender {
    /// The OAuth registration is a Google Desktop app, not a web application.
    /// A shipped desktop client secret is not confidential; wrap it nonetheless.
    pub fn new(client_id: String, client_secret: Option<SecretString>) -> PluginResult<Self> {
        if client_id.is_empty()
            || client_id.len() > 512
            || client_id.chars().any(char::is_control)
            || client_secret
                .as_ref()
                .is_some_and(|s| s.expose_secret().is_empty())
        {
            return Err(PluginError::InvalidConfiguration);
        }
        Ok(Self {
            client_id,
            client_secret,
            endpoints: Endpoints::default(),
            state: Arc::new(Mutex::new(State::default())),
        })
    }

    fn check_scope(&self, cx: &PluginContext) -> PluginResult<()> {
        if cx.scope().plugin_id != MANIFEST.id {
            return Err(PluginError::WrongScope);
        }
        cx.check_active()
    }

    pub fn begin_authorization(
        &self,
        cx: &PluginContext,
        from_address: String,
    ) -> PluginResult<GmailAuthorization> {
        self.check_scope(cx)?;
        mime::mailbox(&from_address, None)?;
        let mut state = self.state.lock().map_err(|_| PluginError::Core)?;
        let epoch = state.epochs.entry(cx.scope().clone()).or_default();
        *epoch = epoch.checked_add(1).ok_or(PluginError::Core)?;
        let endpoints = self.oauth_endpoints()?;
        let mut config = OAuthConfig::new(
            endpoints,
            self.client_id.clone(),
            vec![SEND_SCOPE.into()],
            "/callback".into(),
        );
        // Offline grants are needed for refresh. Explicit consent makes reconnect
        // capable of issuing a new refresh token after local forget.
        config.extra_authorization_parameters = vec![
            ("access_type".into(), "offline".into()),
            ("prompt".into(), "consent".into()),
        ];
        Ok(GmailAuthorization {
            flow: OAuthLoopback::start(config)?,
            scope: cx.scope().clone(),
            client_id: self.client_id.clone(),
            epoch: *epoch,
            from_address,
            cancellation: cx.cancellation().clone(),
        })
    }

    fn oauth_endpoints(&self) -> PluginResult<OAuthEndpoints> {
        #[cfg(test)]
        if self.endpoints.authorize.starts_with("http://127.0.0.1:") {
            return OAuthEndpoints::loopback_for_tests(
                &self.endpoints.authorize,
                &self.endpoints.token,
            );
        }
        OAuthEndpoints::new(&self.endpoints.authorize, &self.endpoints.token)
    }

    /// Wait off the UI thread; only save after the helper has validated callback
    /// state and Google has validated the PKCE proof. This requests no OIDC scopes
    /// and trusts no ID token or identity claims, so there is no ID-token nonce to verify.
    pub fn complete_authorization(
        &self,
        cx: &PluginContext,
        attempt: GmailAuthorization,
    ) -> PluginResult<()> {
        self.check_scope(cx)?;
        self.audited(
            cx,
            AuditAction::Connect,
            AuditPurpose::Connection,
            |stats| {
                self.check_scope(cx)?;
                if attempt.scope != *cx.scope() || attempt.client_id != self.client_id {
                    return Err(PluginError::WrongScope);
                }
                attempt.cancellation.check()?;
                let grant = attempt.flow.wait_cancellable(cx.cancellation())?;
                attempt.cancellation.check()?;
                cx.check_active()?;
                let state = self.state.lock().map_err(|_| PluginError::Core)?;
                if state.epochs.get(cx.scope()).copied() != Some(attempt.epoch) {
                    return Err(PluginError::OAuthState);
                }
                if grant.token_endpoint.expose_secret() != self.endpoints.token {
                    return Err(PluginError::WrongScope);
                }
                let form = self.with_client_secret(grant.token_form());
                let response = self.request(
                    cx,
                    &self.endpoints.token,
                    "application/x-www-form-urlencoded",
                    SecretBytes::new(form.expose_secret().as_bytes().to_vec()),
                    None,
                    stats,
                )?;
                let tokens = Tokens::from_response(
                    &response,
                    cx.clock().now(),
                    &self.client_id,
                    &attempt.from_address,
                    None,
                )?;
                attempt.cancellation.check()?;
                cx.check_active()?;
                cx.credentials().save(tokens.encode()?)
            },
        )
    }

    /// Revoke using a form body (never put a token in a URL), then wipe locally
    /// whether or not Google's response confirms revocation. No real-account work
    /// occurs unless the host explicitly calls this method.
    pub fn disconnect(&self, cx: &PluginContext) -> PluginResult<DisconnectReceipt> {
        self.check_scope(cx)?;
        let start = Instant::now();
        let mut stats = IoStats::default();
        let mut state = self.state.lock().map_err(|_| PluginError::Core)?;
        let epoch = state.epochs.entry(cx.scope().clone()).or_default();
        *epoch = epoch.checked_add(1).ok_or(PluginError::Core)?;
        let loaded = cx.credentials().load();
        let confirmed = loaded
            .as_ref()
            .ok()
            .and_then(|s| s.as_ref())
            .and_then(|s| Tokens::decode(s).ok())
            .is_some_and(|tokens| {
                let form = SecretString::from(
                    url::form_urlencoded::Serializer::new(String::new())
                        .append_pair("token", tokens.refresh.expose_secret())
                        .finish(),
                );
                self.request(
                    cx,
                    &self.endpoints.revoke,
                    "application/x-www-form-urlencoded",
                    SecretBytes::new(form.expose_secret().as_bytes().to_vec()),
                    None,
                    &mut stats,
                )
                .is_ok_and(|r| r.status == 200)
            });
        let forgotten = cx.credentials().forget();
        let result = forgotten.and(loaded.map(|_| DisconnectReceipt {
            revocation_confirmed: confirmed,
        }));
        cx.record_audit(
            AuditAction::Disconnect,
            AuditPurpose::Connection,
            if result.is_ok() && confirmed {
                AuditOutcome::Success
            } else {
                AuditOutcome::Failed
            },
            (stats.sent, stats.received),
            start.elapsed(),
        )?;
        result
    }

    fn load_tokens(&self, cx: &PluginContext) -> PluginResult<Tokens> {
        let raw = cx.credentials().load()?.ok_or(PluginError::NeedsSignIn)?;
        let old = match Tokens::decode(&raw) {
            Ok(tokens) => tokens,
            Err(_) => {
                cx.credentials().forget()?;
                return Err(PluginError::NeedsSignIn);
            }
        };
        if old.client_id != self.client_id {
            return Err(PluginError::NeedsSignIn);
        }
        Ok(old)
    }

    fn access_token(
        &self,
        cx: &PluginContext,
        old: Tokens,
        stats: &mut IoStats,
    ) -> PluginResult<Tokens> {
        if old.expires_at.0 > cx.clock().now().0.saturating_add(60) {
            return Ok(old);
        }
        let form = SecretString::from(
            url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs([
                    ("grant_type", "refresh_token"),
                    ("client_id", self.client_id.as_str()),
                    ("refresh_token", old.refresh.expose_secret()),
                ])
                .finish(),
        );
        let form = self.with_client_secret(form);
        let response = self.request(
            cx,
            &self.endpoints.token,
            "application/x-www-form-urlencoded",
            SecretBytes::new(form.expose_secret().as_bytes().to_vec()),
            None,
            stats,
        )?;
        match Tokens::from_response(
            &response,
            cx.clock().now(),
            &self.client_id,
            &old.from_address,
            Some(&old),
        ) {
            Ok(tokens) => {
                cx.credentials().save(tokens.encode()?)?;
                Ok(tokens)
            }
            Err(PluginError::NeedsSignIn) => {
                cx.credentials().forget()?;
                Err(PluginError::NeedsSignIn)
            }
            Err(error) => Err(error), // Temporary or malformed failures retain refresh credentials.
        }
    }

    fn with_client_secret(&self, form: SecretString) -> SecretString {
        match &self.client_secret {
            Some(secret) => SecretString::from(
                url::form_urlencoded::Serializer::new(form.expose_secret().to_owned())
                    .append_pair("client_secret", secret.expose_secret())
                    .finish(),
            ),
            None => form,
        }
    }

    fn request(
        &self,
        cx: &PluginContext,
        endpoint: &str,
        content_type: &str,
        body: SecretBytes,
        bearer: Option<&SecretString>,
        stats: &mut IoStats,
    ) -> PluginResult<HttpResponse> {
        let mut headers = vec![("Content-Type".into(), SecretString::from(content_type))];
        if let Some(token) = bearer {
            headers.push((
                "Authorization".into(),
                SecretString::from(format!("Bearer {}", token.expose_secret())),
            ));
        }
        stats.sent = stats.sent.saturating_add(body.expose().len() as u64);
        let response = cx.http().execute(HttpRequest {
            method: HttpMethod::Post,
            url: SecretString::from(endpoint),
            headers,
            body,
        })?;
        stats.received = stats
            .received
            .saturating_add(response.body.expose().len() as u64);
        Ok(response)
    }

    fn audited<T>(
        &self,
        cx: &PluginContext,
        action: AuditAction,
        purpose: AuditPurpose,
        operation: impl FnOnce(&mut IoStats) -> PluginResult<T>,
    ) -> PluginResult<T> {
        let start = Instant::now();
        let mut stats = IoStats::default();
        let result = operation(&mut stats);
        let outcome = match &result {
            Ok(_) => AuditOutcome::Success,
            Err(PluginError::NeedsSignIn) => AuditOutcome::NeedsSignIn,
            Err(
                PluginError::WrongScope
                | PluginError::OAuthDenied
                | PluginError::InvalidConfiguration
                | PluginError::TooLarge,
            ) => AuditOutcome::Refused,
            Err(_) => AuditOutcome::Failed,
        };
        cx.record_audit(
            action,
            purpose,
            outcome,
            (stats.sent, stats.received),
            start.elapsed(),
        )?;
        result
    }
}
impl Plugin for GmailSender {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }
    fn connection_lifecycle(&self) -> Option<&dyn ConnectionLifecycle> {
        Some(self)
    }
}
impl DocumentSender for GmailSender {
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
        let mut stats = IoStats::default();
        let mut submitted = false;
        let mut accepted = None;
        let result = (|| -> PluginResult<()> {
            self.check_scope(cx)?;
            let _lock = self.state.lock().map_err(|_| PluginError::Core)?;
            let old = self.load_tokens(cx)?;
            // The sender mailbox is scoped to this credential record, not global
            // plugin settings. Validate inputs before any refresh or send.
            let mime = mime::build(&old.from_address, &document, cx.clock().now())?;
            #[derive(Serialize)]
            struct RawMessage<'a> {
                raw: &'a str,
            }
            let raw = Zeroizing::new(URL_SAFE_NO_PAD.encode(mime.expose()));
            let body = SecretBytes::new(
                serde_json::to_vec(&RawMessage { raw: &raw })
                    .map_err(|_| PluginError::InvalidConfiguration)?,
            );
            let tokens = self.access_token(cx, old, &mut stats)?;
            cx.check_active()?;
            submitted = true;
            let response = self.request(
                cx,
                &self.endpoints.send,
                "application/json",
                body,
                Some(&tokens.access),
                &mut stats,
            )?;
            match response.status {
                200 => (),
                401 => {
                    submitted = false;
                    cx.credentials().forget()?;
                    return Err(PluginError::NeedsSignIn);
                }
                400 | 403 | 404 | 413 | 429 => {
                    submitted = false;
                    return Err(match response.status {
                        403 => PluginError::OAuthDenied,
                        429 => PluginError::Network,
                        _ => PluginError::InvalidResponse,
                    });
                }
                500..=599 => return Err(PluginError::Network),
                _ => return Err(PluginError::InvalidResponse),
            }
            #[derive(Deserialize)]
            struct Sent<'a> {
                id: &'a str,
            }
            let sent: Sent<'_> = serde_json::from_slice(response.body.expose())
                .map_err(|_| PluginError::InvalidResponse)?;
            if sent.id.is_empty()
                || sent.id.len() > 256
                || !sent
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
            {
                return Err(PluginError::InvalidResponse);
            }

            let receipt = SendReceipt {
                provider_message_id: Some(sent.id.to_owned()),
                sent_at: cx.clock().now(),
            };
            accepted = Some(receipt.clone());
            // Provider-owned history until wave 4 chooses one atomic owner.
            // Attempt rich audit even if the separate history bridge fails.
            let history = cx
                .core()
                .record_document_send(document.document, receipt.clone());
            let audit = cx.record_document_send_audit(DocumentSendAudit {
                event: AuditEvent {
                    scope: cx.scope().clone(),
                    actor: cx.actor(),
                    at: receipt.sent_at,
                    action: AuditAction::Send,
                    purpose: AuditPurpose::DocumentDelivery,
                    outcome: AuditOutcome::Success,
                    bytes_sent: stats.sent,
                    bytes_received: stats.received,
                    duration: start.elapsed(),
                },
                document: document.document,
                receipt,
            });
            history.and(audit)
        })();
        if let Some(receipt) = accepted {
            return SendOutcome::Accepted {
                receipt,
                recording_error: result.err(),
            };
        }
        let error = result.expect_err("non-accepted send must fail");
        let outcome = match error {
            PluginError::NeedsSignIn => AuditOutcome::NeedsSignIn,
            PluginError::WrongScope
            | PluginError::OAuthDenied
            | PluginError::InvalidConfiguration
            | PluginError::TooLarge => AuditOutcome::Refused,
            _ => AuditOutcome::Failed,
        };
        // An audit failure never changes what happened on the network.
        let _ = cx.record_audit(
            AuditAction::Send,
            AuditPurpose::DocumentDelivery,
            outcome,
            (stats.sent, stats.received),
            start.elapsed(),
        );
        if submitted {
            SendOutcome::Uncertain { error }
        } else {
            SendOutcome::NotSubmitted { error }
        }
    }
}
