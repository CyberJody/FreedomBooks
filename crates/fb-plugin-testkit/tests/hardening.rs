use fb_plugin_api::*;
use fb_plugin_testkit::*;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
fn get(url: &str) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Get,
        url: SecretString::from(url),
        headers: vec![],
        body: SecretBytes::new(vec![]),
    }
}

#[test]
fn scripted_server_is_local_and_records_expected_exchanges() {
    let server = ScriptedServer::start(vec![ScriptedExchange {
        method: "GET".into(),
        target: "/data".into(),
        response: FakeResponse::new(200, "synthetic result"),
    }])
    .unwrap();
    assert_eq!(server.server().address().ip().to_string(), "127.0.0.1");
    let result = server
        .server()
        .http()
        .execute(get(&server.server().url("/data")))
        .unwrap();
    assert_eq!(result.status, 200);
    assert_eq!(result.body.expose(), b"synthetic result");
    server.assert_finished();
}
#[test]
fn redirects_to_same_or_different_host_are_never_followed() {
    for target in [
        "https://evil.example/collect",
        "/same-host-path",
        "http://169.254.169.254/latest/meta-data/",
    ] {
        let target = target.to_owned();
        let calls = Arc::new(AtomicUsize::new(0));
        let captured = calls.clone();
        let server = FakeServer::start(move |_| {
            captured.fetch_add(1, Ordering::SeqCst);
            FakeResponse::new(302, "").header("Location", &target)
        })
        .unwrap();
        assert!(matches!(
            server.http().execute(get(&server.url("/data"))),
            Err(PluginError::Redirect)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
#[test]
fn response_caps_cover_length_headers_and_streamed_bodies() {
    for omit_length in [false, true] {
        let server = FakeServer::start(move |_| {
            let mut response = FakeResponse::new(200, "123456789");
            response.omit_content_length = omit_length;
            response
        })
        .unwrap();
        let policy = HttpPolicy::loopback_for_tests(server.address().port())
            .unwrap()
            .with_limits(Duration::from_secs(1), 100, 8)
            .unwrap();
        assert!(matches!(
            PluginHttp::new(policy).execute(get(&server.url("/large"))),
            Err(PluginError::TooLarge)
        ));
    }
    let server = FakeServer::start(|_| FakeResponse::new(200, "12345678")).unwrap();
    let policy = HttpPolicy::loopback_for_tests(server.address().port())
        .unwrap()
        .with_limits(Duration::from_secs(1), 100, 8)
        .unwrap();
    assert_eq!(
        PluginHttp::new(policy)
            .execute(get(&server.url("/exact")))
            .unwrap()
            .body
            .expose(),
        b"12345678"
    );
}
#[test]
fn request_cap_fails_before_network_and_host_header_cannot_override_authority() {
    let server = FakeServer::start(|_| panic!("must refuse before network request")).unwrap();
    let policy = HttpPolicy::loopback_for_tests(server.address().port())
        .unwrap()
        .with_limits(Duration::from_secs(1), 2, 8)
        .unwrap();
    let mut request = get(&server.url("/data"));
    request.body = SecretBytes::new(vec![1, 2, 3]);
    assert!(matches!(
        PluginHttp::new(policy).execute(request),
        Err(PluginError::TooLarge)
    ));
    for header in [
        "Host",
        "Content-Length",
        "Transfer-Encoding",
        "Connection",
        "Proxy-Authorization",
    ] {
        let mut request = get(&server.url("/data"));
        request
            .headers
            .push((header.into(), SecretString::from("secret")));
        assert!(matches!(
            server.http().execute(request),
            Err(PluginError::InvalidConfiguration)
        ));
    }
}
#[test]
fn request_timeout_caps_the_whole_response() {
    let server = FakeServer::start(|_| {
        let mut response = FakeResponse::new(200, "late");
        response.delay = Duration::from_millis(300);
        response
    })
    .unwrap();
    let policy = HttpPolicy::loopback_for_tests(server.address().port())
        .unwrap()
        .with_limits(Duration::from_millis(30), 100, 100)
        .unwrap();
    let start = Instant::now();
    assert!(matches!(
        PluginHttp::new(policy).execute(get(&server.url("/slow"))),
        Err(PluginError::Timeout)
    ));
    assert!(start.elapsed() < Duration::from_secs(1));
}
#[test]
fn test_loopback_exception_is_exact_host_port_and_does_not_weaken_production() {
    let server = FakeServer::start(|_| FakeResponse::new(200, "ok")).unwrap();
    let policy = HttpPolicy::loopback_for_tests(server.address().port()).unwrap();
    assert_eq!(
        policy.validate_resolved(&server.url("/"), &[server.address()]),
        Ok(())
    );
    for url in [
        "http://localhost:1234/",
        "http://127.0.0.2:1234/",
        "http://10.0.0.1:1234/",
        "http://127.0.0.1:1234/",
    ] {
        assert!(matches!(
            PluginHttp::new(policy.clone()).execute(get(url)),
            Err(PluginError::HttpsRequired)
        ));
    }
    assert_eq!(
        policy.validate_resolved(&server.url("/"), &["10.0.0.1:1234".parse().unwrap()]),
        Err(PluginError::PrivateAddress)
    );
    assert!(matches!(
        PluginHttp::new(HttpPolicy::new(&["127.0.0.1"]).unwrap()).execute(get(&format!(
            "https://127.0.0.1:{}/",
            server.address().port()
        ))),
        Err(PluginError::PrivateAddress)
    ));
}
#[test]
fn credentials_are_scoped_by_plugin_company_and_connection_and_forget_only_one() {
    let store = Arc::new(MemoryCredentialStore::new());
    let a = test_scope("toy");
    let mut b = a.clone();
    b.plugin_id = "other".into();
    let mut c = a.clone();
    c.company_id = Uuid::from_u128(101);
    let mut d = a.clone();
    d.connection_id = Uuid::from_u128(201);
    for (scope, value) in [(&a, b"a"), (&b, b"b"), (&c, b"c"), (&d, b"d")] {
        store.save(scope, SecretBytes::new(value.to_vec())).unwrap();
    }
    let handle = ScopedCredentials::new(a.clone(), store.clone());
    assert_eq!(handle.load().unwrap().unwrap().expose(), b"a");
    handle
        .save(SecretBytes::new(b"replacement".to_vec()))
        .unwrap();
    assert_eq!(store.load(&a).unwrap().unwrap().expose(), b"replacement");
    handle.forget().unwrap();
    assert!(store.load(&a).unwrap().is_none());
    for (scope, value) in [(&b, b"b"), (&c, b"c"), (&d, b"d")] {
        assert_eq!(store.load(scope).unwrap().unwrap().expose(), value);
    }
    assert!(!format!("{handle:?} {store:?}").contains("replacement"));
}
#[test]
fn context_rejects_mismatched_credentials_and_core_scopes() {
    let scope = test_scope("toy");
    let mut other = scope.clone();
    other.company_id = Uuid::from_u128(999);
    for (credential_scope, core_scope) in [(other.clone(), scope.clone()), (scope.clone(), other)] {
        let result = PluginContext::new(ContextServices {
            scope: scope.clone(),
            actor: Uuid::from_u128(300),
            credentials: ScopedCredentials::new(
                credential_scope,
                Arc::new(MemoryCredentialStore::new()),
            ),
            core: Arc::new(FakeCoreBridge::new(core_scope)),
            audit: Arc::new(MemoryAuditSink::default()),
            http: PluginHttp::new(HttpPolicy::new(&[]).unwrap()),
            clock: Arc::new(FixedClock(Timestamp(1))),
        });
        assert!(matches!(result, Err(PluginError::WrongScope)));
    }
}
#[test]
fn fake_core_records_only_draft_operations_and_sanitized_metadata() {
    let scope = test_scope("toy");
    let core = FakeCoreBridge::new(scope);
    let date = Date::new(2026, 10, 30).unwrap();
    let draft = core
        .create_draft_bill(DraftBill {
            source_remote_id: "receipt-1".into(),
            vendor: "Synthetic vendor".into(),
            date,
            lines: vec![],
        })
        .unwrap();
    core.attach_to_draft(
        draft,
        Attachment {
            filename: "receipt.txt".into(),
            media_type: "text/plain".into(),
            bytes: SecretBytes::new(b"PRIVATE_DOCUMENT_SENTINEL".to_vec()),
        },
    )
    .unwrap();
    let staged = core
        .stage_bank_transactions(vec![FeedTransaction {
            account_remote_id: "bank-1".into(),
            remote_id: "txn-1".into(),
            posted_on: Some(date),
            transacted_on: None,
            amount: Amount {
                cents: 1234,
                currency: "USD".into(),
            },
            description: "PRIVATE_TRANSACTION_SENTINEL".into(),
            pending: true,
        }])
        .unwrap();
    assert_eq!(staged.transaction_ids.len(), 1);
    core.save_connection_state(SecretBytes::new(b"cursor-1".to_vec()))
        .unwrap();
    assert_eq!(
        core.load_connection_state().unwrap().unwrap().expose(),
        b"cursor-1"
    );
    let calls = core.calls();
    assert!(matches!(calls[0], CoreCall::DraftBill { .. }));
    assert!(matches!(calls[1], CoreCall::Attach { .. }));
    assert_eq!(calls[2], CoreCall::Stage { count: 1 });
    for sentinel in [
        "PRIVATE_DOCUMENT_SENTINEL",
        "PRIVATE_TRANSACTION_SENTINEL",
        "cursor-1",
    ] {
        assert!(!format!("{calls:?}").contains(sentinel));
    }
}
