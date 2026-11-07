//! W1-FIX1 adoption exercised through the plugin, not only the HTTP helper.
use base64::{engine::general_purpose::STANDARD, Engine};
use fb_plugin_api::*;
use fb_plugin_simplefin::SimpleFin;
use fb_plugin_testkit::*;
use serde_json::{json, Value};
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

struct Fixture {
    server: FakeServer,
    body: Arc<Mutex<Value>>,
    requests: Arc<Mutex<Vec<FakeRequest>>>,
    cx: PluginContext,
    core: Arc<FakeCoreBridge>,
    audit: Arc<MemoryAuditSink>,
    plugin: SimpleFin,
}
fn context(http: PluginHttp) -> (PluginContext, Arc<FakeCoreBridge>, Arc<MemoryAuditSink>) {
    let scope = test_scope("simplefin");
    let core = Arc::new(FakeCoreBridge::new(scope.clone()));
    let audit = Arc::new(MemoryAuditSink::default());
    let cx = test_context(
        scope,
        http,
        Arc::new(MemoryCredentialStore::new()),
        core.clone(),
        audit.clone(),
    );
    (cx, core, audit)
}
fn input(url: &str) -> ConnectInput {
    ConnectInput {
        fields: SecretBytes::new(STANDARD.encode(url).into_bytes()),
    }
}
fn window() -> DateWindow {
    DateWindow {
        start: Date::new(2026, 10, 1).unwrap(),
        end: Date::new(2026, 10, 30).unwrap(),
    }
}
fn connection(cx: &PluginContext) -> FeedConnection {
    FeedConnection {
        id: cx.connection_id(),
    }
}
fn seed(cx: &PluginContext, host: &str, port: u16, scheme: &str) {
    // Synthetic scoped credential, not a real claim/token or persisted fixture.
    cx.credentials()
        .save(SecretBytes::new(
            serde_json::to_vec(&json!({
                "version":1, "host":host, "port":port, "scheme":scheme,
                "access_url":format!("{scheme}://synthetic:synthetic@{host}:{port}/simplefin")
            }))
            .unwrap(),
        ))
        .unwrap();
}
fn sample() -> Value {
    json!({"connections":[{"conn_id":"C", "org_name":"Synthetic bank"}], "accounts":[{
        "id":"A", "conn_id":"C", "org":{"id":"O", "name":"Old name"},
        "name":"Checking", "currency":"USD", "balance":"12.34", "available-balance":"10.005",
        "balance-date":1792800000,
        "transactions":[{"id":"T", "amount":"-1.005", "posted":1792800000}]
    }]})
}
impl Fixture {
    fn new(body: Value, timeout: Duration, cap: usize, delay: Duration, streamed: bool) -> Self {
        let body = Arc::new(Mutex::new(body));
        let requests = Arc::new(Mutex::new(vec![]));
        let (captured, calls) = (body.clone(), requests.clone());
        let server = FakeServer::start(move |r| {
            let bytes = if r.method == "POST" {
                format!(
                    "http://synthetic:synthetic@{}/simplefin",
                    r.header("Host").unwrap()
                )
                .into_bytes()
            } else {
                serde_json::to_vec(&*captured.lock().unwrap()).unwrap()
            };
            let mut response = FakeResponse::new(200, bytes);
            response.omit_content_length = streamed;
            response.delay = delay;
            calls.lock().unwrap().push(r);
            response
        })
        .unwrap();
        let policy = server
            .http()
            .policy()
            .clone()
            .with_limits(timeout, 1, cap)
            .unwrap();
        let (cx, core, audit) = context(PluginHttp::new(policy));
        Self {
            server,
            body,
            requests,
            cx,
            core,
            audit,
            plugin: SimpleFin::new("UTC").unwrap(),
        }
    }
    fn ordinary(body: Value) -> Self {
        Self::new(
            body,
            Duration::from_secs(2),
            1024 * 1024,
            Duration::ZERO,
            false,
        )
    }
    fn seed(&self) {
        seed(&self.cx, "127.0.0.1", self.server.address().port(), "http");
    }
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

#[test]
fn tighter_host_16_byte_cap_survives_claim_and_accounts_for_both_framings() {
    for streamed in [false, true] {
        for claim in [false, true] {
            let f = Fixture::new(
                sample(),
                Duration::from_secs(2),
                16,
                Duration::ZERO,
                streamed,
            );
            if claim {
                assert_eq!(
                    f.plugin
                        .connect(&f.cx, input(&f.server.url("/claim/test")))
                        .unwrap_err(),
                    PluginError::TooLarge
                );
                assert!(f.cx.credentials().load().unwrap().is_none());
            } else {
                f.seed();
                assert!(matches!(
                    f.plugin.fetch_result(&f.cx, &connection(&f.cx), window()),
                    Err(PluginError::TooLarge)
                ));
                assert!(f.cx.credentials().load().unwrap().is_some());
            }
            assert_eq!(f.count(), 1);
            assert!(f.core.calls().is_empty());
        }
    }
}

#[test]
fn tighter_host_25_ms_deadline_refuses_125_ms_claim_and_account_responses() {
    for claim in [false, true] {
        let f = Fixture::new(
            sample(),
            Duration::from_millis(25),
            4096,
            Duration::from_millis(125),
            false,
        );
        let started = Instant::now();
        if claim {
            assert_eq!(
                f.plugin
                    .connect(&f.cx, input(&f.server.url("/claim/test")))
                    .unwrap_err(),
                PluginError::Timeout
            );
            assert!(f.cx.credentials().load().unwrap().is_none());
        } else {
            f.seed();
            assert!(matches!(
                f.plugin.fetch_result(&f.cx, &connection(&f.cx), window()),
                Err(PluginError::Timeout)
            ));
        }
        eprintln!(
            "claim={claim} host_deadline_ms=25 reply_delay_ms=125 elapsed_ms={}",
            started.elapsed().as_millis()
        );
        assert!(f.count() <= 1); // no automatic retry, including timeout before peer accepts
        assert!(f.core.calls().is_empty());
    }
}

struct SequenceResolver {
    answers: Vec<Vec<SocketAddr>>,
    calls: AtomicUsize,
    budgets: Mutex<Vec<Duration>>,
}
impl SequenceResolver {
    fn new(answers: Vec<Vec<SocketAddr>>) -> Self {
        Self {
            answers,
            calls: AtomicUsize::new(0),
            budgets: Mutex::new(vec![]),
        }
    }
}
impl Resolver for SequenceResolver {
    fn resolve(&self, host: &str, _: u16, timeout: Duration) -> PluginResult<Vec<SocketAddr>> {
        assert_eq!(host, "bank.example");
        self.budgets.lock().unwrap().push(timeout);
        self.answers
            .get(self.calls.fetch_add(1, Ordering::SeqCst))
            .cloned()
            .ok_or(PluginError::Network)
    }
}
#[test]
fn injected_resolver_and_deadline_are_kept_through_claim() {
    let resolver = Arc::new(SequenceResolver::new(vec![vec![
        "8.8.8.8:443".parse().unwrap(),
        "169.254.169.254:443".parse().unwrap(),
    ]]));
    let policy = HttpPolicy::new(&["bank.example"])
        .unwrap()
        .with_limits(Duration::from_millis(25), 1, 16)
        .unwrap();
    let (cx, core, _) = context(PluginHttp::with_resolver(policy, resolver.clone()));
    let plugin = SimpleFin::new("UTC").unwrap();
    assert_eq!(
        plugin
            .connect(&cx, input("https://bank.example/claim/test"))
            .unwrap_err(),
        PluginError::PrivateAddress
    );
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        *resolver.budgets.lock().unwrap(),
        [Duration::from_millis(25)]
    );
    assert!(cx.credentials().load().unwrap().is_none());
    assert!(core.calls().is_empty());
    assert_eq!(
        plugin
            .connect(&cx, input("https://unconsented.example/claim/test"))
            .unwrap_err(),
        PluginError::HostNotAllowed
    );
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn plugin_uses_real_hostname_pinned_tls_then_refuses_private_and_mixed_rebinding() {
    let server = FakeTlsServer::start("bank.example").unwrap();
    let port = server.address().port();
    let public = SocketAddr::new("8.8.8.8".parse().unwrap(), port);
    let private = server.address();
    let resolver = Arc::new(SequenceResolver::new(vec![
        vec![public],
        vec![private],
        vec![public, private],
    ]));
    let http = PluginHttp::with_resolver(
        HttpPolicy::new(&["bank.example"]).unwrap(),
        resolver.clone(),
    );
    let http = server.route(http, "bank.example", public.ip()).unwrap();
    let (cx, core, audit) = context(http);
    seed(&cx, "bank.example", port, "https");
    let plugin = SimpleFin::new("UTC").unwrap();
    // FakeTlsServer deliberately replies "ok", not an Account Set. Prove transport
    // success from peer observation AND completed bytes before JSON parsing refuses it.
    assert!(matches!(
        plugin.fetch_result(&cx, &connection(&cx), window()),
        Err(PluginError::InvalidResponse)
    ));
    assert_eq!(audit.events()[0].bytes_received, 2);
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
    assert_eq!(server.connections(), 1);
    assert_eq!(
        server.observations(),
        [TlsObservation {
            sni: Some("bank.example".into()),
            host: format!("bank.example:{port}")
        }]
    );
    for count in [2, 3] {
        assert!(matches!(
            plugin.fetch_result(&cx, &connection(&cx), window()),
            Err(PluginError::PrivateAddress)
        ));
        assert_eq!(resolver.calls.load(Ordering::SeqCst), count);
        assert_eq!(server.connections(), 1);
        assert_eq!(server.observations().len(), 1);
    }
    assert!(core.calls().is_empty());
}

#[test]
fn plugin_tls_rejects_wrong_hostname_and_untrusted_certificate() {
    for wrong_name in [true, false] {
        let server = FakeTlsServer::start(if wrong_name {
            "wrong.example"
        } else {
            "bank.example"
        })
        .unwrap();
        let unrelated = FakeTlsServer::start("bank.example").unwrap();
        let port = server.address().port();
        let public = SocketAddr::new("8.8.8.8".parse().unwrap(), port);
        let resolver = Arc::new(SequenceResolver::new(vec![vec![public]]));
        let http = PluginHttp::with_resolver(
            HttpPolicy::new(&["bank.example"]).unwrap(),
            resolver.clone(),
        );
        let root = if wrong_name {
            server.root_der()
        } else {
            unrelated.root_der()
        };
        let http = http
            .with_tls_test_route("bank.example", public, server.address(), root)
            .unwrap();
        let (cx, _, audit) = context(http);
        seed(&cx, "bank.example", port, "https");
        assert!(matches!(
            SimpleFin::new("UTC")
                .unwrap()
                .fetch_result(&cx, &connection(&cx), window()),
            Err(PluginError::Network)
        ));
        assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
        assert_eq!(server.connections(), 1);
        assert!(server.observations().is_empty());
        assert_eq!(audit.events()[0].bytes_received, 0);
    }
}

#[test]
fn rich_slot_preserves_metadata_complete_inventory_and_exact_transactions() {
    let f = Fixture::ordinary(sample());
    f.seed();
    let slot: &dyn BankFeed = &f.plugin;
    let result = slot
        .list_accounts_result(&f.cx, &connection(&f.cx))
        .unwrap();
    assert_eq!(result.completeness, FeedCompleteness::Complete);
    assert!(result.has_complete_inventory());
    assert_eq!(result.present_account_ids, Some(vec!["A".into()]));
    assert_eq!(result.items[0].balance.as_ref().unwrap().cents, 1234);
    assert_eq!(
        result.account_metadata,
        [FeedAccountMetadata {
            account_remote_id: "A".into(),
            organization_remote_id: Some("O".into()),
            organization_name: Some("Synthetic bank".into()),
            connection_remote_id: Some("C".into()),
            available_balance: Some(Amount {
                cents: 1001,
                currency: "USD".into()
            }),
            balance_at: Some(Timestamp(1792800000)),
        }]
    );
    assert_eq!(f.count(), 1);
    assert_eq!(
        f.requests.lock().unwrap()[0].target,
        "/simplefin/accounts?balances-only=1"
    );
    assert_eq!(f.audit.events()[0].action, AuditAction::ListAccounts);
    let transactions = slot
        .fetch_result(&f.cx, &connection(&f.cx), window())
        .unwrap();
    assert_eq!(f.count(), 2);
    assert!(transactions.has_complete_inventory());
    assert_eq!(transactions.account_metadata, result.account_metadata);
    assert_eq!(transactions.items[0].amount.cents, -101);
    assert_eq!(transactions.items[0].remote_id, "T");
    assert_eq!(
        transactions.items[0].posted_on,
        Some(Date::new(2026, 10, 24).unwrap())
    );
    assert!(f.core.calls().is_empty());
}

#[test]
fn partial_slot_retains_usable_data_skipped_account_inventory_and_closed_warnings() {
    let mut data = sample();
    data["accounts"].as_array_mut().unwrap().extend([
        json!({"id":"<b>B</b>", "conn_id":"C", "currency":"USD", "balance":"NaN"}),
        json!({"id":"D", "conn_id":"C", "balance":"1"}),
        json!({"currency":"USD", "balance":"1"}),
        json!(false),
    ]);
    data["accounts"][0]["transactions"]
        .as_array_mut()
        .unwrap()
        .extend([
            json!({"id":"<b>bad</b>", "amount":"NaN"}),
            json!({"amount":"1"}),
            json!(false),
        ]);
    data["errlist"] = json!([null, {"code":"gen.auth.expired", "msg":"secret-provider-message", "conn_id":"<b>C</b>", "account_id":"<b>A</b>"}]);
    let f = Fixture::ordinary(data);
    f.seed();
    let slot: &dyn BankFeed = &f.plugin;
    let result = slot
        .fetch_result(&f.cx, &connection(&f.cx), window())
        .unwrap();
    assert_eq!(result.completeness, FeedCompleteness::Partial);
    assert!(!result.has_complete_inventory());
    assert_eq!(result.items.len(), 1);
    assert_eq!(
        result.present_account_ids,
        Some(vec!["A".into(), "B".into(), "D".into()])
    );
    assert_eq!(result.warnings.len(), 8);
    assert_eq!(
        result.warnings[0],
        FeedWarning {
            code: FeedWarningCode::AuthenticationRequired,
            account_remote_id: Some("A".into()),
            transaction_remote_id: None,
            connection_remote_id: Some("C".into()),
            source_index: Some(1),
        }
    );
    assert_eq!(
        result.warnings[1],
        FeedWarning {
            code: FeedWarningCode::InvalidAmount,
            account_remote_id: Some("A".into()),
            transaction_remote_id: Some("bad".into()),
            connection_remote_id: Some("C".into()),
            source_index: Some(1),
        }
    );
    assert_eq!(result.warnings[2].code, FeedWarningCode::MissingIdentifier);
    assert_eq!(result.warnings[2].source_index, Some(2));
    assert_eq!(result.warnings[3].code, FeedWarningCode::InvalidTransaction);
    assert_eq!(result.warnings[4].account_remote_id.as_deref(), Some("B"));
    assert_eq!(result.warnings[4].source_index, Some(1));
    assert_eq!(
        result.warnings[5].code,
        FeedWarningCode::UnsupportedCurrency
    );
    assert_eq!(result.warnings[6].code, FeedWarningCode::MissingIdentifier);
    assert_eq!(result.warnings[7].code, FeedWarningCode::InvalidAccount);
    assert!(!format!("{:?} {:?}", result.warnings, f.audit.events())
        .contains("secret-provider-message"));
    assert_eq!(f.count(), 1);
    let accounts = slot
        .list_accounts_result(&f.cx, &connection(&f.cx))
        .unwrap();
    assert_eq!(accounts.items.len(), 1);
    assert_eq!(accounts.warnings, result.warnings);
    assert!(matches!(
        slot.fetch(&f.cx, &connection(&f.cx), window()),
        Err(PluginError::NeedsSignIn)
    ));
    assert!(matches!(
        slot.list_accounts(&f.cx, &connection(&f.cx)),
        Err(PluginError::NeedsSignIn)
    ));
    assert!(f.core.calls().is_empty());
}

#[test]
fn empty_inventory_is_known_but_missing_inventory_is_not_invented() {
    let f = Fixture::ordinary(json!({"accounts":[]}));
    f.seed();
    let slot: &dyn BankFeed = &f.plugin;
    let result = slot
        .fetch_result(&f.cx, &connection(&f.cx), window())
        .unwrap();
    assert!(result.items.is_empty());
    assert_eq!(result.present_account_ids, Some(vec![]));
    assert!(result.has_complete_inventory());
    *f.body.lock().unwrap() = json!({"errors":["secret-provider-message"], "accounts":[]});
    let partial = slot
        .list_accounts_result(&f.cx, &connection(&f.cx))
        .unwrap();
    assert_eq!(partial.completeness, FeedCompleteness::Partial);
    assert!(!partial.has_complete_inventory());
    assert_eq!(partial.warnings[0].code, FeedWarningCode::Other);
    assert_eq!(partial.warnings[0].source_index, Some(0));
    assert!(matches!(
        slot.list_accounts(&f.cx, &connection(&f.cx)),
        Err(PluginError::InvalidResponse)
    ));
    *f.body.lock().unwrap() = json!({});
    assert!(matches!(
        slot.list_accounts_result(&f.cx, &connection(&f.cx)),
        Err(PluginError::InvalidResponse)
    ));
    assert!(matches!(
        slot.fetch_result(&f.cx, &connection(&f.cx), window()),
        Err(PluginError::InvalidResponse)
    ));
}

#[test]
fn rich_methods_keep_scope_and_window_checks_without_network_or_staging() {
    let f = Fixture::ordinary(sample());
    f.seed();
    let wrong = FeedConnection {
        id: Uuid::from_u128(999),
    };
    assert!(matches!(
        f.plugin.list_accounts_result(&f.cx, &wrong),
        Err(PluginError::WrongScope)
    ));
    assert!(matches!(
        f.plugin.fetch_result(&f.cx, &wrong, window()),
        Err(PluginError::WrongScope)
    ));
    let w = window();
    assert!(matches!(
        f.plugin.fetch_result(
            &f.cx,
            &connection(&f.cx),
            DateWindow {
                start: w.end,
                end: w.start
            }
        ),
        Err(PluginError::InvalidConfiguration)
    ));
    assert_eq!(f.count(), 0);
    assert!(f.core.calls().is_empty());
}

#[test]
fn v1_metadata_and_skipped_container_warnings_keep_the_same_connection_identity() {
    let f = Fixture::ordinary(json!({"accounts":[
        {"id":"A", "org":{"id":"<b>O</b>", "name":"Legacy bank"}, "currency":"USD",
         "balance":"1", "available-balance":"bad", "balance-date":0, "transactions":"bad"},
        {"id":"B", "org":{"id":"<b>O</b>"}, "currency":"USD", "balance":"bad"},
        {"id":"B", "org":{"id":"<b>O</b>"}, "currency":"USD", "balance":"bad"}
    ]}));
    f.seed();
    let result = f
        .plugin
        .list_accounts_result(&f.cx, &connection(&f.cx))
        .unwrap();
    assert_eq!(result.completeness, FeedCompleteness::Partial);
    assert_eq!(
        result.present_account_ids,
        Some(vec!["A".into(), "B".into()])
    );
    assert_eq!(
        result.account_metadata[0].connection_remote_id.as_deref(),
        Some("O")
    );
    assert_eq!(
        result.account_metadata[0].organization_remote_id.as_deref(),
        Some("O")
    );
    assert_eq!(
        result.account_metadata[0].organization_name.as_deref(),
        Some("Legacy bank")
    );
    assert!(result.account_metadata[0].available_balance.is_none());
    assert!(result.account_metadata[0].balance_at.is_none());
    assert_eq!(result.warnings.len(), 3);
    assert_eq!(result.warnings[0].code, FeedWarningCode::InvalidTransaction);
    for (index, warning) in result.warnings.iter().enumerate() {
        assert_eq!(warning.source_index, Some(index));
        assert_eq!(warning.connection_remote_id.as_deref(), Some("O"));
    }
    assert!(matches!(
        f.plugin.list_accounts(&f.cx, &connection(&f.cx)),
        Err(PluginError::InvalidResponse)
    ));
    assert!(matches!(
        f.plugin
            .fetch_and_stage(&f.cx, &connection(&f.cx), window()),
        Err(PluginError::InvalidResponse)
    ));
    assert!(f.core.calls().is_empty());
}

#[test]
fn subscription_status_is_distinct_on_claim_and_established_feed() {
    let server = FakeServer::start(|_| FakeResponse::new(402, "secret-provider-message")).unwrap();
    let (cx, _, audit) = context(server.http());
    let plugin = SimpleFin::new("UTC").unwrap();
    assert_eq!(
        plugin
            .connect(&cx, input(&server.url("/claim/test")))
            .unwrap_err(),
        PluginError::ProviderSubscriptionRequired
    );
    seed(&cx, "127.0.0.1", server.address().port(), "http");
    assert!(matches!(
        plugin.fetch_result(&cx, &connection(&cx), window()),
        Err(PluginError::ProviderSubscriptionRequired)
    ));
    assert!(cx.credentials().load().unwrap().is_some());
    assert!(!format!("{:?}", audit.events()).contains("secret-provider-message"));
}
