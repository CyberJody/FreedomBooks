//! `fb-net`: the desktop's network process. The book and its key stay in the Electron main process
//! (fb-node / fb-app, which link no HTTP or TLS stack); everything that talks to SimpleFIN or OpenAI
//! runs here, through the plugins' own code paths (claim, HTTP policy and SSRF guard, OAuth state,
//! PKCE and nonce, the loopback that ignores stray requests). This process never sees the book, its
//! key or any other connection: it gets exactly one connection's stored credential record per
//! request, keeps it in an in-memory [`CredentialStore`] for that request, and hands the updated
//! record back over the pipe.
//!
//! ## Protocol (JSON lines on stdin / stdout)
//! One request line per process run; fb-node spawns one process per operation (no state survives an
//! operation, a crash or hang affects only that operation, and a stray process can't be reused by
//! anything else). Every reply line is an object with `"ok"`; `ok:false` carries `"error"`, a fixed
//! user-safe sentence (never a URL, token or provider body).
//!
//! - `{"op":"simplefin_claim","setup_token":"…"}` → `{"ok":true,"record":"<stored credential JSON>","host":"…"}`
//! - `{"op":"simplefin_fetch","record":"…","start_epoch":N,"zone":"America/New_York"}` →
//!   `{"ok":true,"pull":{accounts,skipped,errors}}` in `fb_core::bankfeeds::StagedPull`'s shape (cents,
//!   dates in the display zone like Python's `_epoch_to_date`, Python's skip wording).
//! - `{"op":"chatgpt_begin","record":"…"|null}` → `{"ok":true,"stage":"authorize","authorization_url":"…","registration":bool}`,
//!   then, after the browser hit the loopback callback (or the 10-minute deadline),
//!   `{"ok":true,"stage":"done","record":"…","account_email":"…","model_choices":[{slug,display_name}],"models_error":code|null,"registration":bool}`
//!   or `{"ok":false,"stage":"done","code":"<fixed failure code>","error":"…"}`.
//! - `{"op":"chatgpt_disconnect","record":"…"}` → `{"ok":true,"revoked":true|false|null}`.
//!
//! The production build only reaches the plugins' manifest hosts over HTTPS. The `fake-network`
//! feature (tests and the smoke only) instead reads `FB_NET_FAKE_SIMPLEFIN_PORT` and
//! `FB_NET_FAKE_OPENAI` (`http://127.0.0.1:<port>`) and allows exactly that numeric loopback port.
#![forbid(unsafe_code)]
use fb_plugin_api::*;
use fb_plugin_openai::OpenAi;
use fb_plugin_simplefin::{client::FetchOptions, parse, SimpleFin};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Read, Write};
use std::sync::{Arc, Mutex};
use zeroize::{Zeroize, Zeroizing};

const MAX_REQUEST: u64 = 256 * 1024;

/// One scope's credential, in memory for one request. Wiped on drop (SecretBytes).
struct OneRecord {
    scope: CredentialScope,
    value: Mutex<Option<SecretBytes>>,
}
impl OneRecord {
    fn take(&self) -> PluginResult<Option<SecretBytes>> {
        Ok(self.value.lock().map_err(|_| PluginError::CredentialStore)?.take())
    }
}
impl CredentialStore for OneRecord {
    fn load(&self, scope: &CredentialScope) -> PluginResult<Option<SecretBytes>> {
        if *scope != self.scope {
            return Err(PluginError::WrongScope);
        }
        let value = self.value.lock().map_err(|_| PluginError::CredentialStore)?;
        Ok(value.as_ref().map(|v| SecretBytes::new(v.expose().to_vec())))
    }
    fn save(&self, scope: &CredentialScope, value: SecretBytes) -> PluginResult<()> {
        if *scope != self.scope {
            return Err(PluginError::WrongScope);
        }
        *self.value.lock().map_err(|_| PluginError::CredentialStore)? = Some(value);
        Ok(())
    }
    fn forget(&self, scope: &CredentialScope) -> PluginResult<()> {
        if *scope != self.scope {
            return Err(PluginError::WrongScope);
        }
        *self.value.lock().map_err(|_| PluginError::CredentialStore)? = None;
        Ok(())
    }
}

/// This process has no book: every core call is refused. Staging happens in fb-app with the pull.
struct NoCore(CredentialScope);
impl CoreBridge for NoCore {
    fn scope(&self) -> &CredentialScope {
        &self.0
    }
    fn rendered_document(&self, _: DocumentRef) -> PluginResult<OutgoingDocument> {
        Err(PluginError::Core)
    }
    fn stage_bank_transactions(&self, _: Vec<FeedTransaction>) -> PluginResult<StagedFeed> {
        Err(PluginError::Core)
    }
    fn create_draft_bill(&self, _: DraftBill) -> PluginResult<DraftRef> {
        Err(PluginError::Core)
    }
    fn create_draft_sale(&self, _: DraftSale) -> PluginResult<DraftRef> {
        Err(PluginError::Core)
    }
    fn create_draft_deposit(&self, _: DraftDeposit) -> PluginResult<DraftRef> {
        Err(PluginError::Core)
    }
    fn attach_to_draft(&self, _: DraftRef, _: Attachment) -> PluginResult<()> {
        Err(PluginError::Core)
    }
    fn record_document_send(&self, _: DocumentRef, _: SendReceipt) -> PluginResult<()> {
        Err(PluginError::Core)
    }
    fn load_connection_state(&self) -> PluginResult<Option<SecretBytes>> {
        Ok(None)
    }
    fn save_connection_state(&self, _: SecretBytes) -> PluginResult<()> {
        Err(PluginError::Core)
    }
    fn record_audit(&self, _: AuditEvent) -> PluginResult<()> {
        Ok(())
    }
}
/// The book side writes the user-facing audit (`bankfeed.*`, `ai.*`); this process keeps no log.
struct NoAudit;
impl AuditSink for NoAudit {
    fn record(&self, _: AuditEvent) -> PluginResult<()> {
        Ok(())
    }
}

fn context(plugin_id: &str, http: PluginHttp, record: Option<&str>) -> PluginResult<(PluginContext, Arc<OneRecord>)> {
    let scope = CredentialScope { plugin_id: plugin_id.into(), company_id: Uuid::new_v4(), connection_id: Uuid::new_v4() };
    let store = Arc::new(OneRecord { scope: scope.clone(), value: Mutex::new(record.map(|r| SecretBytes::new(r.as_bytes().to_vec()))) });
    let cx = PluginContext::new(ContextServices {
        credentials: ScopedCredentials::new(scope.clone(), store.clone()),
        core: Arc::new(NoCore(scope.clone())),
        scope,
        actor: Uuid::new_v4(),
        audit: Arc::new(NoAudit),
        http,
        clock: Arc::new(SystemClock),
    })?;
    Ok((cx, store))
}

#[cfg(not(feature = "fake-network"))]
fn simplefin_http() -> PluginResult<PluginHttp> {
    Ok(PluginHttp::new(HttpPolicy::new(fb_plugin_simplefin::MANIFEST.network_hosts)?))
}
#[cfg(feature = "fake-network")]
fn simplefin_http() -> PluginResult<PluginHttp> {
    let port = std::env::var("FB_NET_FAKE_SIMPLEFIN_PORT").ok().and_then(|p| p.parse().ok()).ok_or(PluginError::InvalidConfiguration)?;
    Ok(PluginHttp::new(HttpPolicy::loopback_for_tests(port)?))
}
#[cfg(not(feature = "fake-network"))]
fn openai(host_id: Uuid) -> PluginResult<(OpenAi, PluginHttp)> {
    Ok((OpenAi::new(host_id, vec![])?, PluginHttp::new(HttpPolicy::new(fb_plugin_openai::MANIFEST.network_hosts)?)))
}
#[cfg(feature = "fake-network")]
fn openai(host_id: Uuid) -> PluginResult<(OpenAi, PluginHttp)> {
    let base = std::env::var("FB_NET_FAKE_OPENAI").map_err(|_| PluginError::InvalidConfiguration)?;
    let port = base.rsplit(':').next().and_then(|p| p.trim_end_matches('/').parse().ok()).ok_or(PluginError::InvalidConfiguration)?;
    let plugin = OpenAi::new(host_id, vec![])?.with_loopback_endpoints(&base)?;
    Ok((plugin, PluginHttp::new(HttpPolicy::loopback_for_tests(port)?)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    op: String,
    #[serde(default)]
    setup_token: Option<String>,
    #[serde(default)]
    record: Option<String>,
    #[serde(default)]
    start_epoch: Option<i64>,
    #[serde(default)]
    zone: Option<String>,
}
impl Drop for Request {
    fn drop(&mut self) {
        self.setup_token.zeroize();
        self.record.zeroize();
    }
}

fn reply(value: &impl Serialize) {
    let mut line = Zeroizing::new(Vec::new());
    if serde_json::to_writer(&mut *line, value).is_err() {
        line.clear();
        line.extend_from_slice(br#"{"ok":false,"error":"The network helper could not answer."}"#);
    }
    line.push(b'\n');
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(&line);
    let _ = out.flush();
}
#[derive(Serialize)]
struct Failure<'a> {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stage: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'a str>,
    error: String,
}
fn fail(stage: Option<&str>, error: PluginError) {
    // A finished sign-in's failure also carries a fixed code for the book's `ai.signin_failed` audit.
    let code = stage.map(|_| signin_failure_code(error));
    reply(&Failure { ok: false, stage, code, error: error.to_string() });
}
/// `fb_core::assistant::SIGNIN_FAILURE_CODES`.
fn signin_failure_code(error: PluginError) -> &'static str {
    match error {
        PluginError::OAuthState => "state_mismatch",
        PluginError::OAuthNonce => "nonce_mismatch",
        PluginError::OAuthPkce => "pkce_mismatch",
        PluginError::OAuthCallback => "callback",
        PluginError::OAuthDenied => "denied",
        PluginError::Timeout | PluginError::Cancelled => "expired",
        PluginError::Network => "unreachable",
        PluginError::CertificateRejected => "certificate",
        PluginError::InvalidResponse => "response_unreadable",
        _ => "failed",
    }
}

fn exposed(record: &SecretBytes) -> PluginResult<&str> {
    std::str::from_utf8(record.expose()).map_err(|_| PluginError::CredentialStore)
}

fn simplefin_claim(request: &Request) -> PluginResult<()> {
    let token = request.setup_token.as_deref().ok_or(PluginError::InvalidConfiguration)?;
    let (cx, store) = context(fb_plugin_simplefin::MANIFEST.id, simplefin_http()?, None)?;
    let plugin = SimpleFin::new("UTC")?;
    plugin.connect(&cx, ConnectInput { fields: SecretBytes::new(token.as_bytes().to_vec()) })?;
    let record = store.take()?.ok_or(PluginError::CredentialStore)?;
    #[derive(Deserialize)]
    struct Host {
        host: String,
    }
    let host: Host = serde_json::from_slice(record.expose()).map_err(|_| PluginError::CredentialStore)?;
    #[derive(Serialize)]
    struct Claimed<'a> {
        ok: bool,
        record: &'a str,
        host: &'a str,
    }
    reply(&Claimed { ok: true, record: exposed(&record)?, host: &host.host });
    Ok(())
}

#[derive(Serialize)]
struct PullTransaction<'a> {
    remote_id: &'a str,
    posted_on: Option<String>,
    transacted_on: Option<String>,
    amount: i64,
    description: &'a str,
    pending: bool,
}
#[derive(Serialize)]
struct PullAccount<'a> {
    conn_key: &'a str,
    remote_id: &'a str,
    name: &'a str,
    org_name: &'a str,
    currency: &'a str,
    balance: Option<i64>,
    available: Option<i64>,
    balance_epoch: Option<i64>,
    skipped: &'a [String],
    transactions: Vec<PullTransaction<'a>>,
}
#[derive(Serialize)]
struct Pull<'a> {
    accounts: Vec<PullAccount<'a>>,
    skipped: &'a [String],
    errors: Vec<&'a str>,
}

fn simplefin_fetch(request: &Request) -> PluginResult<()> {
    let record = request.record.as_deref().ok_or(PluginError::NeedsSignIn)?;
    let start = request.start_epoch.ok_or(PluginError::InvalidConfiguration)?;
    let plugin = SimpleFin::new(request.zone.as_deref().unwrap_or("UTC"))?;
    let zone = plugin.zone();
    let (cx, _store) = context(fb_plugin_simplefin::MANIFEST.id, simplefin_http()?, Some(record))?;
    let connection = FeedConnection { id: cx.connection_id() };
    // Python `_sync_locked`: start date only, no end date, no pending items (the protocol default).
    let set = plugin.fetch_account_set(&cx, &connection, FetchOptions { start_epoch: Some(start), ..Default::default() })?;
    let pull = Pull {
        accounts: set
            .accounts
            .iter()
            .map(|a| PullAccount {
                conn_key: &a.connection_key,
                remote_id: &a.remote_id,
                name: &a.name,
                org_name: &a.organization_name,
                currency: &a.currency,
                balance: Some(a.balance_cents),
                available: a.available_cents,
                balance_epoch: a.balance_epoch,
                skipped: &a.skipped_text,
                transactions: a
                    .transactions
                    .iter()
                    .map(|t| PullTransaction {
                        remote_id: &t.remote_id,
                        posted_on: parse::local_date(t.posted_epoch, zone),
                        transacted_on: parse::local_date(t.transacted_epoch, zone),
                        amount: t.cents,
                        description: &t.description,
                        pending: t.pending,
                    })
                    .collect(),
            })
            .collect(),
        skipped: &set.skipped_text,
        errors: set.errors.iter().map(|e| e.message.as_str()).collect(),
    };
    #[derive(Serialize)]
    struct Fetched<'a> {
        ok: bool,
        pull: Pull<'a>,
    }
    reply(&Fetched { ok: true, pull });
    Ok(())
}

#[derive(Deserialize)]
struct RecordHead {
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    ext_agent_host_id: String,
}
fn head(record: Option<&str>) -> RecordHead {
    record.and_then(|r| serde_json::from_str(r).ok()).unwrap_or(RecordHead { client_id: String::new(), ext_agent_host_id: String::new() })
}
/// The install identity OpenAI registers the dynamic client for: kept in the connection's own stored
/// record (`ext_agent_host_id`), new on the first sign-in.
fn host_id(head: &RecordHead) -> Uuid {
    head.ext_agent_host_id.strip_prefix("urn:uuid:").and_then(|s| Uuid::parse_str(s).ok()).filter(|u| !u.is_nil()).unwrap_or_else(Uuid::new_v4)
}
/// `assistant.client` failure codes the core turns into its fixed sentences (`client_failure_message`).
fn models_error_code(error: PluginError) -> &'static str {
    match error {
        PluginError::Timeout => "timeout",
        PluginError::CertificateRejected => "certificate",
        PluginError::Redirect => "redirect",
        PluginError::TooLarge => "response_oversized",
        PluginError::InvalidResponse => "invalid_model_list",
        PluginError::HttpsRequired => "insecure_address",
        _ => "unreachable",
    }
}

fn chatgpt_begin(request: &Request) -> Result<(), (Option<&'static str>, PluginError)> {
    let previous = head(request.record.as_deref());
    let registration = !previous.client_id.starts_with("oaiapp_");
    let (plugin, http) = openai(host_id(&previous)).map_err(|e| (None, e))?;
    let (cx, store) = context(fb_plugin_openai::MANIFEST.id, http, request.record.as_deref()).map_err(|e| (None, e))?;
    let pending = plugin.start_authorization(&cx).map_err(|e| (None, e))?;
    #[derive(Serialize)]
    struct Authorize<'a> {
        ok: bool,
        stage: &'a str,
        authorization_url: &'a str,
        registration: bool,
    }
    reply(&Authorize { ok: true, stage: "authorize", authorization_url: pending.authorization_url().expose_secret(), registration });
    let done = Some("done");
    plugin.complete_authorization(&cx, pending).map_err(|e| (done, e))?;
    let (choices, models_error) = match AiHelper::models(&plugin, &cx) {
        Ok(models) => (models, None),
        Err(e) => (vec![], Some(models_error_code(e))),
    };
    let record = store.take().map_err(|e| (done, e))?.ok_or((done, PluginError::CredentialStore))?;
    #[derive(Deserialize)]
    struct Email {
        #[serde(default)]
        email: String,
    }
    let mut email: Email = serde_json::from_slice(record.expose()).map_err(|_| (done, PluginError::CredentialStore))?;
    #[derive(Serialize)]
    struct Choice<'a> {
        slug: &'a str,
        display_name: &'a str,
    }
    #[derive(Serialize)]
    struct Done<'a> {
        ok: bool,
        stage: &'a str,
        record: &'a str,
        account_email: &'a str,
        model_choices: Vec<Choice<'a>>,
        models_error: Option<&'a str>,
        registration: bool,
    }
    reply(&Done {
        ok: true,
        stage: "done",
        record: exposed(&record).map_err(|e| (done, e))?,
        account_email: &email.email,
        model_choices: choices.iter().map(|c| Choice { slug: &c.id, display_name: &c.label }).collect(),
        models_error,
        registration,
    });
    email.email.zeroize();
    Ok(())
}

fn chatgpt_disconnect(request: &Request) -> PluginResult<()> {
    let previous = head(request.record.as_deref());
    let (plugin, http) = openai(host_id(&previous))?;
    let (cx, _store) = context(fb_plugin_openai::MANIFEST.id, http, request.record.as_deref())?;
    let revoked = plugin.disconnect(&cx)?;
    #[derive(Serialize)]
    struct Disconnected {
        ok: bool,
        revoked: Option<bool>,
    }
    reply(&Disconnected { ok: true, revoked });
    Ok(())
}

fn main() {
    let mut line = Zeroizing::new(String::new());
    let read = std::io::stdin().lock().take(MAX_REQUEST).read_line(&mut line);
    let request: Request = match read.ok().and_then(|_| serde_json::from_str(&line).ok()) {
        Some(request) => request,
        None => return fail(None, PluginError::InvalidConfiguration),
    };
    line.zeroize();
    match request.op.as_str() {
        "simplefin_claim" => simplefin_claim(&request).unwrap_or_else(|e| fail(None, e)),
        "simplefin_fetch" => simplefin_fetch(&request).unwrap_or_else(|e| fail(None, e)),
        "chatgpt_begin" => chatgpt_begin(&request).unwrap_or_else(|(stage, e)| fail(stage, e)),
        "chatgpt_disconnect" => chatgpt_disconnect(&request).unwrap_or_else(|e| fail(None, e)),
        _ => fail(None, PluginError::InvalidConfiguration),
    }
}
