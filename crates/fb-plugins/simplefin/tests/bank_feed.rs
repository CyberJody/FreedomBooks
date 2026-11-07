use base64::{engine::general_purpose::STANDARD, Engine};
use fb_plugin_api::*;
use fb_plugin_simplefin::{client::FetchOptions, SimpleFin, MANIFEST};
use fb_plugin_testkit::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

struct ServerState {
    requests: Vec<FakeRequest>,
    claimed: bool,
    claim_status: u16,
    claim_body: Option<Vec<u8>>,
    status: u16,
    body: Vec<u8>,
    omit_length: bool,
}
struct Fixture {
    server: FakeServer,
    state: Arc<Mutex<ServerState>>,
    cx: PluginContext,
    store: Arc<MemoryCredentialStore>,
    core: Arc<FakeCoreBridge>,
    audit: Arc<MemoryAuditSink>,
    plugin: SimpleFin,
}
fn sample() -> Value {
    json!({"accounts":[{"id":"checking", "name":"Synthetic Checking", "currency":"USD", "balance":"12480.55",
        "transactions":[{"id":"TX-1", "posted":1792800000, "transacted_at":1792713600, "amount":"-42.75", "description":"FUEL STOP"},
        {"id":"TX-PENDING", "posted":0, "amount":"-5.005", "pending":false, "description":"Pending charge"}]}]})
}
impl Fixture {
    fn new() -> Self {
        let state = Arc::new(Mutex::new(ServerState {
            requests: vec![],
            claimed: false,
            claim_status: 200,
            claim_body: None,
            status: 200,
            body: serde_json::to_vec(&sample()).unwrap(),
            omit_length: false,
        }));
        let captured = state.clone();
        let server = FakeServer::start(move |request| {
            let mut state = captured.lock().unwrap();
            let mut response = if request.method == "POST" {
                let status = if state.claimed {
                    403
                } else {
                    state.claim_status
                };
                state.claimed = true;
                let raw = format!(
                    "http://{}:{}@{}/simplefin/",
                    "synthetic-user",
                    "synthetic%40pass%3Aword",
                    request.header("Host").unwrap()
                );
                FakeResponse::new(
                    status,
                    state.claim_body.as_deref().unwrap_or(raw.as_bytes()),
                )
            } else {
                FakeResponse::new(state.status, &state.body)
            };
            if (300..400).contains(&response.status) {
                response = response.header("Location", "http://169.254.169.254/latest/meta-data/");
            }
            response.omit_content_length = state.omit_length;
            state.requests.push(request);
            response
        })
        .unwrap();
        let scope = test_scope("simplefin");
        let store = Arc::new(MemoryCredentialStore::new());
        let core = Arc::new(FakeCoreBridge::new(scope.clone()));
        let audit = Arc::new(MemoryAuditSink::default());
        let cx = test_context(
            scope,
            server.http(),
            store.clone(),
            core.clone(),
            audit.clone(),
        );
        Self {
            server,
            state,
            cx,
            store,
            core,
            audit,
            plugin: SimpleFin::new("America/New_York").unwrap(),
        }
    }
    fn input(&self) -> ConnectInput {
        ConnectInput {
            fields: SecretBytes::new(
                STANDARD
                    .encode(self.server.url("/simplefin/claim/one-time-test-token"))
                    .into_bytes(),
            ),
        }
    }
    fn connect(&self) -> FeedConnection {
        self.plugin.connect(&self.cx, self.input()).unwrap()
    }
    fn connection(&self) -> FeedConnection {
        FeedConnection {
            id: self.cx.connection_id(),
        }
    }
    fn window(&self) -> DateWindow {
        DateWindow {
            start: Date::new(2026, 10, 1).unwrap(),
            end: Date::new(2026, 10, 30).unwrap(),
        }
    }
    fn count(&self) -> usize {
        self.state.lock().unwrap().requests.len()
    }
    fn set_body(&self, value: Value) {
        self.state.lock().unwrap().body = serde_json::to_vec(&value).unwrap();
    }
}

#[test]
fn manifest_declares_bankfeed_read_only_consent_and_secret_input() {
    MANIFEST.validate().unwrap();
    assert_eq!(MANIFEST.slots, [Slot::BankFeed]);
    assert_eq!(MANIFEST.auth, AuthKind::SetupToken);
    assert_eq!(MANIFEST.settings_fields[0].kind, FieldKind::Secret);
    assert_eq!(MANIFEST.network_hosts, ["bridge.simplefin.org", "beta-bridge.simplefin.org"]);
    assert!(MANIFEST.scopes[0].contains("no payment"));
    assert!(MANIFEST.outside_account.contains("disable"));
    assert!(SimpleFin::new("not-a-zone").is_err());
}

#[test]
fn claim_is_one_empty_post_and_credentials_are_stored_only_in_scoped_store() {
    let f = Fixture::new();
    let connection = f.connect();
    assert_eq!(connection.id, f.cx.connection_id());
    let state = f.state.lock().unwrap();
    assert_eq!(state.requests.len(), 1);
    let r = &state.requests[0];
    assert_eq!(r.method, "POST");
    assert!(r.target.starts_with("/simplefin/claim/"));
    assert!(r.header("Authorization").is_none());
    assert!(r.body.expose().is_empty());
    let saved = f.cx.credentials().load().unwrap().unwrap();
    let secret: Value = serde_json::from_slice(saved.expose()).unwrap();
    assert!(secret["access_url"]
        .as_str()
        .unwrap()
        .contains("synthetic-user"));
    assert!(!std::str::from_utf8(saved.expose())
        .unwrap()
        .contains("one-time-test-token"));
    assert_eq!(secret["host"], "127.0.0.1");
    assert!(f.core.calls().is_empty());
    let events = f.audit.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].action, AuditAction::Connect);
    assert_eq!(events[0].outcome, AuditOutcome::Success);
    assert!(events[0].bytes_received > 0);
    for leaked in ["synthetic-user", "synthetic%40pass", "one-time-test-token"] {
        assert!(!format!("{:?} {:?} {:?}", f.store, events, saved).contains(leaked));
    }
}

#[test]
fn reconnect_never_consumes_a_token_or_overwrites_existing_credentials() {
    let f = Fixture::new();
    f.connect();
    assert!(matches!(
        f.plugin.connect(&f.cx, f.input()),
        Err(PluginError::InvalidConfiguration)
    ));
    assert_eq!(f.count(), 1);
    assert_eq!(f.audit.events()[1].outcome, AuditOutcome::Refused);
}

#[test]
fn used_claim_returns_setup_token_refused_without_retry_or_secret_leak() {
    let f = Fixture::new();
    f.state.lock().unwrap().claim_status = 403;
    let error = f.plugin.connect(&f.cx, f.input()).unwrap_err();
    assert_eq!(error, PluginError::SetupTokenRefused);
    assert_eq!(f.count(), 1);
    assert!(f.cx.credentials().load().unwrap().is_none());
    assert_eq!(f.audit.events()[0].outcome, AuditOutcome::Refused);
    assert!(!format!("{error:?} {error}").contains("synthetic"));
}

#[test]
fn claim_bad_status_redirect_and_utf8_fail_closed() {
    for (status, body, expected) in [
        (500, vec![], PluginError::Network),
        (302, vec![], PluginError::Redirect),
        (200, vec![255], PluginError::InvalidResponse),
        (201, vec![], PluginError::Network),
    ] {
        let f = Fixture::new();
        {
            let mut s = f.state.lock().unwrap();
            s.claim_status = status;
            s.claim_body = Some(body);
        }
        assert_eq!(f.plugin.connect(&f.cx, f.input()).unwrap_err(), expected);
        assert_eq!(f.count(), 1);
        assert!(f.cx.credentials().load().unwrap().is_none());
    }
}

#[test]
fn returned_access_origin_must_match_claim_host_scheme_and_port() {
    for raw in [
        "https://u:p@elsewhere.example/simplefin",
        "http://u:p@127.0.0.1:1/simplefin",
        "http://127.0.0.1/simplefin",
        "http://u:p@169.254.169.254/simplefin",
    ] {
        let f = Fixture::new();
        f.state.lock().unwrap().claim_body = Some(raw.as_bytes().to_vec());
        assert!(f.plugin.connect(&f.cx, f.input()).is_err());
        assert_eq!(f.count(), 1);
        assert!(f.cx.credentials().load().unwrap().is_none());
    }
}

#[test]
fn access_needs_credentials_and_refuses_fragment_or_ambiguous_basic_username() {
    for userinfo in ["", "u@", "u%3Aname:p@"] {
        let f = Fixture::new();
        let raw = format!("http://{userinfo}{}/simplefin", f.server.address());
        f.state.lock().unwrap().claim_body = Some(raw.into_bytes());
        assert!(f.plugin.connect(&f.cx, f.input()).is_err());
        assert!(f.cx.credentials().load().unwrap().is_none());
    }
    let f = Fixture::new();
    let raw = format!("http://u:p@{}/simplefin#fragment", f.server.address());
    f.state.lock().unwrap().claim_body = Some(raw.into_bytes());
    assert!(f.plugin.connect(&f.cx, f.input()).is_err());
}

#[test]
fn claim_cap_is_enforced_for_content_length_and_streamed_response() {
    for streamed in [false, true] {
        let f = Fixture::new();
        {
            let mut s = f.state.lock().unwrap();
            s.claim_body = Some(vec![b'x'; 4097]);
            s.omit_length = streamed;
        }
        assert_eq!(
            f.plugin.connect(&f.cx, f.input()).unwrap_err(),
            PluginError::TooLarge
        );
        assert!(f.cx.credentials().load().unwrap().is_none());
    }
}

#[test]
fn fetch_maps_exact_money_pending_and_local_dates_without_posting() {
    let f = Fixture::new();
    let c = f.connect();
    let transactions = f.plugin.fetch(&f.cx, &c, f.window()).unwrap();
    assert_eq!(transactions.len(), 2);
    assert_eq!(transactions[0].amount.cents, -4275);
    assert_eq!(transactions[0].amount.currency, "USD");
    assert_eq!(
        transactions[0].posted_on,
        Some(Date::new(2026, 10, 23).unwrap())
    );
    assert_eq!(
        transactions[0].transacted_on,
        Some(Date::new(2026, 10, 22).unwrap())
    );
    assert_eq!(transactions[0].account_remote_id, "checking");
    assert!(!transactions[0].pending);
    assert!(transactions[1].pending);
    assert_eq!(transactions[1].posted_on, None);
    assert_eq!(transactions[1].amount.cents, -501);
    assert!(f.core.calls().is_empty());
    let state = f.state.lock().unwrap();
    let r = &state.requests[1];
    assert_eq!(r.method, "GET");
    let url = url::Url::parse(&format!("http://example.invalid{}", r.target)).unwrap();
    assert_eq!(url.path(), "/simplefin/accounts");
    let query: Vec<_> = url.query_pairs().collect();
    assert!(query
        .iter()
        .any(|(k, v)| k == "start-date" && v == "1790827200"));
    assert!(query
        .iter()
        .any(|(k, v)| k == "end-date" && v == "1793419199"));
    assert!(!query.iter().any(|(k, _)| k == "pending"));
    let authorization = r.header("Authorization").unwrap();
    assert!(authorization.starts_with("Basic "));
    let decoded = STANDARD
        .decode(authorization.trim_start_matches("Basic "))
        .unwrap();
    assert!(decoded == b"synthetic-user:synthetic@pass:word");
    assert!(!r.target.contains("synthetic"));
    assert!(r.body.expose().is_empty());
    assert_eq!(r.header("Accept"), Some("application/json"));
    assert_eq!(f.audit.events()[1].action, AuditAction::Fetch);
    assert_eq!(f.audit.events()[1].bytes_received, state.body.len() as u64);
}

#[test]
fn list_accounts_uses_balances_only_and_preserves_currency() {
    let f = Fixture::new();
    let c = f.connect();
    let accounts = f.plugin.list_accounts(&f.cx, &c).unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].remote_id, "checking");
    assert_eq!(accounts[0].balance.as_ref().unwrap().cents, 1248055);
    assert_eq!(
        f.state.lock().unwrap().requests[1].target,
        "/simplefin/accounts?balances-only=1"
    );
}

#[test]
fn explicit_fetch_and_stage_only_calls_draft_bridge_and_forget_keeps_history() {
    let f = Fixture::new();
    let c = f.connect();
    let staged = f.plugin.fetch_and_stage(&f.cx, &c, f.window()).unwrap();
    assert_eq!(staged.transaction_ids.len(), 2);
    assert_eq!(f.core.calls(), [CoreCall::Stage { count: 2 }]);
    f.plugin.forget(&f.cx, &c).unwrap();
    f.plugin.forget(&f.cx, &c).unwrap();
    assert!(f.cx.credentials().load().unwrap().is_none());
    assert!(matches!(
        f.plugin.fetch(&f.cx, &c, f.window()),
        Err(PluginError::NeedsSignIn)
    ));
    assert_eq!(f.count(), 2);
    assert_eq!(f.core.calls(), [CoreCall::Stage { count: 2 }]);
}

#[test]
fn rich_fetch_preserves_partial_warnings_but_slot_refuses_to_hide_them() {
    let f = Fixture::new();
    let c = f.connect();
    let mut value = sample();
    value["errors"] = json!(["<b>Bank login expired</b>"]);
    f.set_body(value);
    let result = f
        .plugin
        .fetch_account_set(&f.cx, &c, FetchOptions::default())
        .unwrap();
    assert_eq!(result.accounts.len(), 1);
    assert_eq!(result.errors[0].message, "Bank login expired");
    assert!(matches!(
        f.plugin.fetch_and_stage(&f.cx, &c, f.window()),
        Err(PluginError::InvalidResponse)
    ));
    assert!(f.core.calls().is_empty());
    let mut value = sample();
    value["errlist"] = json!([{"code":"gen.auth", "msg":"expired"}]);
    f.set_body(value);
    assert!(matches!(
        f.plugin.list_accounts(&f.cx, &c),
        Err(PluginError::NeedsSignIn)
    ));
    let mut value = sample();
    value["accounts"][0]["transactions"][0]["amount"] = json!("NaN");
    f.set_body(value);
    assert!(matches!(
        f.plugin.fetch(&f.cx, &c, f.window()),
        Err(PluginError::InvalidResponse)
    ));
}

#[test]
fn rich_fetch_supports_epoch_windows_pending_and_repeated_account_filters() {
    let f = Fixture::new();
    let c = f.connect();
    let result = f
        .plugin
        .fetch_account_set(
            &f.cx,
            &c,
            FetchOptions {
                start_epoch: Some(10),
                end_epoch: Some(20),
                pending: true,
                balances_only: true,
                account_ids: vec!["A/B & C".into(), "D".into()],
            },
        )
        .unwrap();
    assert_eq!(result.accounts.len(), 1);
    let state = f.state.lock().unwrap();
    let url = url::Url::parse(&format!(
        "http://example.invalid{}",
        state.requests[1].target
    ))
    .unwrap();
    let query: Vec<_> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    assert_eq!(
        query,
        [
            ("start-date", "10"),
            ("end-date", "20"),
            ("pending", "1"),
            ("balances-only", "1"),
            ("account", "A/B & C"),
            ("account", "D")
        ]
        .map(|(k, v)| (k.into(), v.into()))
    );
}

#[test]
fn malformed_json_missing_accounts_and_bad_statuses_are_fixed_errors_no_retry() {
    for (status, body, expected) in [
        (403, "access-url-never-echo", PluginError::NeedsSignIn),
        (
            402,
            "subscription",
            PluginError::ProviderSubscriptionRequired,
        ),
        (500, "secret-provider-diagnostic", PluginError::Network),
        (302, "", PluginError::Redirect),
        (200, "broken-json", PluginError::InvalidResponse),
        (200, "[]", PluginError::InvalidResponse),
        (200, "{}", PluginError::InvalidResponse),
    ] {
        let f = Fixture::new();
        let c = f.connect();
        {
            let mut s = f.state.lock().unwrap();
            s.status = status;
            s.body = body.as_bytes().to_vec();
        }
        assert!(matches!(f.plugin.fetch(&f.cx,&c,f.window()),Err(e) if e==expected));
        assert_eq!(f.count(), 2);
        assert!(f.core.calls().is_empty());
        assert!(f.cx.credentials().load().unwrap().is_some()); // no silent credential destruction
    }
}

#[test]
fn accounts_cap_is_enforced_for_length_and_streaming() {
    for streamed in [false, true] {
        let f = Fixture::new();
        let c = f.connect();
        {
            let mut s = f.state.lock().unwrap();
            s.body = vec![b'x'; 20 * 1024 * 1024 + 1];
            s.omit_length = streamed;
        }
        assert!(matches!(
            f.plugin.fetch(&f.cx, &c, f.window()),
            Err(PluginError::TooLarge)
        ));
    }
}

#[test]
fn wrong_connection_or_plugin_never_reads_credentials_or_calls_provider() {
    let f = Fixture::new();
    let c = f.connect();
    let wrong = FeedConnection {
        id: Uuid::from_u128(999),
    };
    assert!(matches!(
        f.plugin.fetch(&f.cx, &wrong, f.window()),
        Err(PluginError::WrongScope)
    ));
    assert!(matches!(
        f.plugin.list_accounts(&f.cx, &wrong),
        Err(PluginError::WrongScope)
    ));
    assert_eq!(f.plugin.forget(&f.cx, &wrong), Err(PluginError::WrongScope));
    assert!(f.cx.credentials().load().unwrap().is_some());
    let scope = test_scope("other");
    let cx = test_context(
        scope.clone(),
        f.server.http(),
        f.store.clone(),
        Arc::new(FakeCoreBridge::new(scope)),
        f.audit.clone(),
    );
    assert!(matches!(
        f.plugin.connect(&cx, f.input()),
        Err(PluginError::WrongScope)
    ));
    assert!(matches!(
        f.plugin.fetch(&cx, &c, f.window()),
        Err(PluginError::WrongScope)
    ));
    assert_eq!(f.count(), 1);
}

#[test]
fn credential_host_pin_is_checked_on_each_load_and_secrets_never_go_in_state() {
    let f = Fixture::new();
    let c = f.connect();
    let saved = f.cx.credentials().load().unwrap().unwrap();
    let mut value: Value = serde_json::from_slice(saved.expose()).unwrap();
    value["host"] = json!("attacker.example");
    f.cx.credentials()
        .save(SecretBytes::new(serde_json::to_vec(&value).unwrap()))
        .unwrap();
    assert!(matches!(
        f.plugin.fetch(&f.cx, &c, f.window()),
        Err(PluginError::HostNotAllowed)
    ));
    assert_eq!(f.count(), 1);
    assert!(f.core.calls().is_empty());
}

#[test]
fn invalid_window_input_and_credential_blob_fail_without_network() {
    let f = Fixture::new();
    let c = f.connect();
    let window = DateWindow {
        start: Date::new(2026, 10, 30).unwrap(),
        end: Date::new(2026, 10, 1).unwrap(),
    };
    assert!(matches!(
        f.plugin.fetch(&f.cx, &c, window),
        Err(PluginError::InvalidConfiguration)
    ));
    assert!(matches!(
        f.plugin.fetch_account_set(
            &f.cx,
            &c,
            FetchOptions {
                start_epoch: Some(20),
                end_epoch: Some(10),
                ..Default::default()
            }
        ),
        Err(PluginError::InvalidConfiguration)
    ));
    f.cx.credentials()
        .save(SecretBytes::new(b"corrupt secret blob".to_vec()))
        .unwrap();
    assert!(matches!(
        f.plugin.list_accounts(&f.cx, &c),
        Err(PluginError::CredentialStore)
    ));
    assert_eq!(f.count(), 1);
    assert!(matches!(
        f.plugin.connect(
            &f.cx,
            ConnectInput {
                fields: SecretBytes::new(vec![255])
            }
        ),
        Err(PluginError::InvalidConfiguration)
    ));
}

#[test]
fn missing_credentials_are_audited_without_contacting_simplefin() {
    let f = Fixture::new();
    assert!(matches!(
        f.plugin.fetch(&f.cx, &f.connection(), f.window()),
        Err(PluginError::NeedsSignIn)
    ));
    assert_eq!(f.count(), 0);
    assert_eq!(f.audit.events()[0].outcome, AuditOutcome::NeedsSignIn);
}

struct BrokenStore {
    inner: Arc<MemoryCredentialStore>,
    fail: &'static str,
}
impl CredentialStore for BrokenStore {
    fn load(&self, scope: &CredentialScope) -> PluginResult<Option<SecretBytes>> {
        if self.fail == "load" {
            Err(PluginError::CredentialStore)
        } else {
            self.inner.load(scope)
        }
    }
    fn save(&self, scope: &CredentialScope, value: SecretBytes) -> PluginResult<()> {
        if self.fail == "save" {
            Err(PluginError::CredentialStore)
        } else {
            self.inner.save(scope, value)
        }
    }
    fn forget(&self, scope: &CredentialScope) -> PluginResult<()> {
        if self.fail == "forget" {
            Err(PluginError::CredentialStore)
        } else {
            self.inner.forget(scope)
        }
    }
}
struct BrokenAudit;
impl AuditSink for BrokenAudit {
    fn record(&self, _: AuditEvent) -> PluginResult<()> {
        Err(PluginError::Audit)
    }
}
fn custom_context(
    f: &Fixture,
    store: Arc<dyn CredentialStore>,
    audit: Arc<dyn AuditSink>,
    core: Arc<dyn CoreBridge>,
) -> PluginContext {
    PluginContext::new(ContextServices {
        scope: f.cx.scope().clone(),
        actor: f.cx.actor(),
        credentials: ScopedCredentials::new(f.cx.scope().clone(), store),
        audit,
        http: f.server.http(),
        clock: Arc::new(FixedClock(Timestamp(1_700_000_000))),
        core,
    })
    .unwrap()
}

#[test]
fn credential_failures_are_audited_and_never_trigger_automatic_claim_retry() {
    for fail in ["load", "save", "forget"] {
        let f = Fixture::new();
        let cx = custom_context(
            &f,
            Arc::new(BrokenStore {
                inner: f.store.clone(),
                fail,
            }),
            f.audit.clone(),
            f.core.clone(),
        );
        if fail == "forget" {
            let c = f.connect();
            assert_eq!(f.plugin.forget(&cx, &c), Err(PluginError::CredentialStore));
            assert!(f.cx.credentials().load().unwrap().is_some());
        } else {
            assert_eq!(
                f.plugin.connect(&cx, f.input()).unwrap_err(),
                PluginError::CredentialStore
            );
            assert!(f.cx.credentials().load().unwrap().is_none());
        }
        assert_eq!(f.count(), if fail == "load" { 0 } else { 1 });
        assert_eq!(
            f.audit.events().last().unwrap().outcome,
            AuditOutcome::Failed
        );
    }
}

#[test]
fn audit_failure_is_returned_not_reported_as_success_and_does_not_reclaim() {
    let f = Fixture::new();
    let cx = custom_context(&f, f.store.clone(), Arc::new(BrokenAudit), f.core.clone());
    assert_eq!(
        f.plugin.connect(&cx, f.input()).unwrap_err(),
        PluginError::Audit
    );
    assert_eq!(f.count(), 1);
    // Persistence already succeeded: the host must coordinate audit/store atomicity in wave 4.
    assert!(f.cx.credentials().load().unwrap().is_some());
    assert!(f.core.calls().is_empty());
}

struct BrokenCore(CredentialScope);
impl CoreBridge for BrokenCore {
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
        Err(PluginError::Core)
    }
    fn save_connection_state(&self, _: SecretBytes) -> PluginResult<()> {
        Err(PluginError::Core)
    }
    fn record_audit(&self, _: AuditEvent) -> PluginResult<()> {
        Err(PluginError::Core)
    }
}
#[test]
fn failed_staging_does_not_retry_fetch_and_is_audited_as_failure() {
    let f = Fixture::new();
    let c = f.connect();
    let cx = custom_context(
        &f,
        f.store.clone(),
        f.audit.clone(),
        Arc::new(BrokenCore(f.cx.scope().clone())),
    );
    assert!(matches!(
        f.plugin.fetch_and_stage(&cx, &c, f.window()),
        Err(PluginError::Core)
    ));
    assert_eq!(f.count(), 2);
    assert_eq!(f.audit.events().last().unwrap().action, AuditAction::Stage);
    assert_eq!(
        f.audit.events().last().unwrap().outcome,
        AuditOutcome::Failed
    );
}

#[test]
fn fetch_preserves_string_and_numeric_amounts_before_value_conversion() {
    let f = Fixture::new();
    let c = f.connect();
    f.state.lock().unwrap().body = br#"{"accounts":[{"id":"A","currency":"USD",
        "balance":99999999999999.99,"available-balance":"1.005","transactions":[
            {"id":"T1","amount":0.00499999999999999999999999999999999},
            {"id":"T2","amount":"-1.005"}]}]}"#
        .to_vec();
    let accounts = f.plugin.list_accounts(&f.cx, &c).unwrap();
    assert_eq!(
        accounts[0].balance.as_ref().unwrap().cents,
        9_999_999_999_999_999
    );
    let transactions = f.plugin.fetch(&f.cx, &c, f.window()).unwrap();
    assert_eq!(
        transactions
            .iter()
            .map(|t| t.amount.cents)
            .collect::<Vec<_>>(),
        [0, -101]
    );
    assert!(f.core.calls().is_empty());
}
