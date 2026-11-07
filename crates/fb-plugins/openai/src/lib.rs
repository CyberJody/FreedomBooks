//! OpenAI's outside-system boundary. No database or posting dependency.
#![forbid(unsafe_code)]
mod auth;
mod lifecycle;
mod protocol;
pub mod receipts;

use fb_plugin_api::*;
use std::time::Instant;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub use auth::PendingAuthorization;
pub use receipts::{ReceiptReading, ReceiptRequest};

pub const SCOPES: &[&str] = &[
    "openid",
    "profile",
    "email",
    "offline_access",
    "resource.invoke",
    "chatgpt.tokens.use.direct",
];
pub const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
pub const RESOURCE: &str = "https://api.openai.com/v1";
pub static MANIFEST: PluginManifest = PluginManifest {
    id: "openai",
    name: "OpenAI / Sign in with ChatGPT",
    version: env!("CARGO_PKG_VERSION"),
    slots: &[Slot::AiHelper],
    auth: AuthKind::OAuthLoopback {
        provider: "OpenAI (API-key fallback available)",
    },
    outside_account: "Your ChatGPT account or your own OpenAI API key",
    scopes: SCOPES,
    data_reads: &[DataKind::ReceiptImages],
    data_sends: &[DataKind::ReceiptImages],
    settings_fields: &[
        SettingsField {
            key: "api_key",
            label: "API key (optional fallback)",
            kind: FieldKind::Secret,
            required: false,
        },
        SettingsField {
            key: "model",
            label: "Model",
            kind: FieldKind::Text,
            required: false,
        },
    ],
    network_hosts: &["auth.openai.com", "api.openai.com"],
};

/// One shared instance per host, serializing rotating refresh tokens and disconnects.
/// `host_id` must be a persisted, install-wide UUID from the host, not a user id.
/// CoreBridge only exposes per-connection state; see FRAMEWORK-REQUESTS.md.
#[derive(Clone)]
pub struct OpenAi {
    endpoints: Endpoints,
    host_id: String,
    key_models: Vec<String>,
    lock: Arc<Mutex<HashMap<CredentialScope, Arc<Mutex<u64>>>>>,
}
#[derive(Clone)]
struct Endpoints {
    issuer: String,
    authorize: String,
    token: String,
    revoke: String,
    discovery: String,
    api: String,
}
impl OpenAi {
    pub fn new(host_id: Uuid, key_models: Vec<String>) -> PluginResult<Self> {
        if host_id.is_nil() || key_models.iter().any(|s| !protocol::valid_slug(s)) {
            return Err(PluginError::InvalidConfiguration);
        }
        Ok(Self {
            endpoints: Endpoints {
                issuer: "https://auth.openai.com".into(),
                authorize: "https://auth.openai.com/api/accounts/authorize".into(),
                token: "https://auth.openai.com/api/accounts/oauth/token".into(),
                revoke: "https://auth.openai.com/api/accounts/oauth/revoke".into(),
                discovery: "https://auth.openai.com/.well-known/openid-configuration".into(),
                api: RESOURCE.into(),
            },
            host_id: host_id.urn().to_string(),
            key_models,
            lock: Arc::new(Mutex::new(HashMap::new())),
        })
    }
    /// Test seam (feature `test-support`, never in a production host): every endpoint on one
    /// numeric-loopback fake provider at `base` (`http://127.0.0.1:<port>`), with the same paths the
    /// plugin's own tests use. The HTTP policy must still allow exactly that port.
    #[cfg(feature = "test-support")]
    pub fn with_loopback_endpoints(mut self, base: &str) -> PluginResult<Self> {
        let url = url::Url::parse(base).map_err(|_| PluginError::InvalidConfiguration)?;
        if url.scheme() != "http" || url.host_str() != Some("127.0.0.1") || url.port().is_none() || url.path() != "/" {
            return Err(PluginError::InvalidConfiguration);
        }
        let base = base.trim_end_matches('/');
        self.endpoints = Endpoints {
            issuer: base.into(),
            authorize: format!("{base}/authorize"),
            token: format!("{base}/token"),
            revoke: format!("{base}/revoke"),
            discovery: format!("{base}/discovery"),
            api: format!("{base}/v1"),
        };
        Ok(self)
    }
    // Hold the provider-wide map only while cloning this connection's lock.
    // Network calls serialize rotating tokens within a scope, never across books.
    fn connection_lock(&self, scope: &CredentialScope) -> PluginResult<Arc<Mutex<u64>>> {
        let mut locks = self.lock.lock().map_err(|_| PluginError::CredentialStore)?;
        Ok(locks.entry(scope.clone()).or_insert_with(|| Arc::new(Mutex::new(0))).clone())
    }
    fn scope(&self, cx: &PluginContext) -> PluginResult<()> {
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
        operation: impl FnOnce() -> PluginResult<T>,
    ) -> PluginResult<T> {
        let start = Instant::now();
        let result = self.scope(cx).and_then(|()| operation());
        let outcome = match &result {
            Ok(_) => AuditOutcome::Success,
            Err(PluginError::NeedsSignIn) => AuditOutcome::NeedsSignIn,
            Err(PluginError::Disabled | PluginError::WrongScope | PluginError::OAuthDenied) => {
                AuditOutcome::Refused
            }
            Err(_) => AuditOutcome::Failed,
        };
        cx.record_audit(action, purpose, outcome, (0, 0), start.elapsed())?;
        result
    }
}
impl Plugin for OpenAi {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }
    fn connection_lifecycle(&self) -> Option<&dyn ConnectionLifecycle> {
        Some(self)
    }
}
impl AiHelper for OpenAi {
    fn models(&self, cx: &PluginContext) -> PluginResult<Vec<ModelChoice>> {
        self.audited(cx, AuditAction::Models, AuditPurpose::Connection, || {
            let lock = self.connection_lock(cx.scope())?;
            let _guard = lock.lock().map_err(|_| PluginError::CredentialStore)?;
            let record = self.bearer(cx)?;
            if !record.api_key.expose_secret().is_empty() {
                return Ok(self
                    .key_models
                    .iter()
                    .map(|id| ModelChoice {
                        id: id.clone(),
                        label: id.clone(),
                    })
                    .collect());
            }
            let result = protocol::models(cx, &self.endpoints.api, &record.access_token);
            if matches!(result, Err(PluginError::NeedsSignIn)) {
                auth::invalidate(cx, record)?;
            }
            result
        })
    }
    fn respond(&self, cx: &PluginContext, request: AiRequest) -> PluginResult<AiResponse> {
        let purpose = match request.purpose {
            AiPurpose::ReadReceipt => AuditPurpose::ReceiptReading,
            AiPurpose::SuggestCategory => AuditPurpose::CategorySuggestion,
            AiPurpose::ExplainReport => AuditPurpose::ReportExplanation,
        };
        self.audited(cx, AuditAction::Respond, purpose, || {
            // The only approved feature is receipt reading; no report/ledger export.
            if request.purpose != AiPurpose::ReadReceipt {
                return Err(PluginError::Disabled);
            }
            let lock = self.connection_lock(cx.scope())?;
            let _guard = lock.lock().map_err(|_| PluginError::CredentialStore)?;
            let record = self.bearer(cx)?;
            let bearer = if record.api_key.expose_secret().is_empty() {
                &record.access_token
            } else {
                &record.api_key
            };
            let result = protocol::respond(cx, &self.endpoints.api, bearer, request);
            if matches!(result, Err(PluginError::NeedsSignIn)) {
                auth::invalidate(cx, record)?;
            }
            result
        })
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod verifier_tests;
