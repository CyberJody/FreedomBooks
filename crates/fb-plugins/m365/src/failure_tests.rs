use super::*;

fn custom_context(
    http: PluginHttp,
    credentials: Arc<dyn CredentialStore>,
    audit: Arc<dyn AuditSink>,
) -> PluginContext {
    let scope = test_scope("m365");
    PluginContext::new(ContextServices {
        credentials: ScopedCredentials::new(scope.clone(), credentials),
        core: Arc::new(FakeCoreBridge::new(scope.clone())),
        scope,
        actor: Uuid::from_u128(300),
        http,
        audit,
        clock: Arc::new(FixedClock(Timestamp(NOW))),
    })
    .unwrap()
}

#[test]
fn http_timeout_does_not_retry_or_forget_good_credentials() {
    let requests = Arc::new(AtomicUsize::new(0));
    let count = requests.clone();
    let server = FakeServer::start(move |_| {
        count.fetch_add(1, Ordering::SeqCst);
        let mut response = FakeResponse::new(202, "");
        response.delay = Duration::from_millis(400);
        response
    })
    .unwrap();
    let policy = HttpPolicy::loopback_for_tests(server.address().port())
        .unwrap()
        .with_limits(Duration::from_millis(100), MAX_MESSAGE_BYTES, 64 * 1024)
        .unwrap();
    let audit = Arc::new(MemoryAuditSink::default());
    let cx = custom_context(
        PluginHttp::new(policy),
        Arc::new(MemoryCredentialStore::new()),
        audit.clone(),
    );
    seed(&cx, NOW + 3600);
    assert!(matches!(
        sender(&server).send_outcome(&cx, document()),
        SendOutcome::Uncertain {
            error: PluginError::Timeout
        }
    ));
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    assert_eq!(stored(&cx)["access_token"], "SYNTHETIC_ACCESS");
    assert_eq!(audit.events()[0].outcome, AuditOutcome::Failed);
}

#[test]
fn http_caps_refuse_oversized_response_and_request() {
    let server = FakeServer::start(|_| FakeResponse::new(202, vec![b'x'; 1025])).unwrap();
    let policy = HttpPolicy::loopback_for_tests(server.address().port())
        .unwrap()
        .with_limits(Duration::from_secs(1), MAX_MESSAGE_BYTES, 1024)
        .unwrap();
    let cx = custom_context(
        PluginHttp::new(policy),
        Arc::new(MemoryCredentialStore::new()),
        Arc::new(MemoryAuditSink::default()),
    );
    seed(&cx, NOW + 3600);
    assert_eq!(
        sender(&server).send(&cx, document()).err(),
        Some(PluginError::TooLarge)
    );
    let server = ScriptedServer::start(vec![]).unwrap();
    let policy = HttpPolicy::loopback_for_tests(server.server().address().port())
        .unwrap()
        .with_limits(Duration::from_secs(1), 1, 1024)
        .unwrap();
    let cx = custom_context(
        PluginHttp::new(policy),
        Arc::new(MemoryCredentialStore::new()),
        Arc::new(MemoryAuditSink::default()),
    );
    seed(&cx, NOW + 3600);
    assert_eq!(
        sender(server.server()).send(&cx, document()).err(),
        Some(PluginError::TooLarge)
    );
    server.assert_finished();
}

#[test]
fn production_policy_does_not_inherit_test_http_permissions() {
    let server = ScriptedServer::start(vec![]).unwrap();
    let cx = custom_context(
        PluginHttp::new(HttpPolicy::new(MANIFEST.network_hosts).unwrap()),
        Arc::new(MemoryCredentialStore::new()),
        Arc::new(MemoryAuditSink::default()),
    );
    seed(&cx, NOW + 3600);
    let mut sender = sender(server.server());
    assert_eq!(
        sender.send(&cx, document()).err(),
        Some(PluginError::HttpsRequired)
    );
    sender.send_endpoint = "https://example.test/send".into();
    assert_eq!(
        sender.send(&cx, document()).err(),
        Some(PluginError::HostNotAllowed)
    );
    server.assert_finished();
}

struct FailingStore {
    memory: MemoryCredentialStore,
    fail_load: bool,
    fail_save: bool,
    fail_forget: bool,
}
impl CredentialStore for FailingStore {
    fn load(&self, scope: &CredentialScope) -> PluginResult<Option<SecretBytes>> {
        if self.fail_load {
            Err(PluginError::CredentialStore)
        } else {
            self.memory.load(scope)
        }
    }
    fn save(&self, scope: &CredentialScope, value: SecretBytes) -> PluginResult<()> {
        if self.fail_save {
            Err(PluginError::CredentialStore)
        } else {
            self.memory.save(scope, value)
        }
    }
    fn forget(&self, scope: &CredentialScope) -> PluginResult<()> {
        if self.fail_forget {
            Err(PluginError::CredentialStore)
        } else {
            self.memory.forget(scope)
        }
    }
}

#[test]
fn failed_rotation_save_prevents_email_submission() {
    let server = ScriptedServer::start(vec![ScriptedExchange {
        method: "POST".into(),
        target: "/token".into(),
        response: token_response(),
    }])
    .unwrap();
    let (initial, _, _) = context(server.server());
    seed(&initial, 0);
    let store = Arc::new(FailingStore {
        memory: MemoryCredentialStore::new(),
        fail_load: false,
        fail_save: true,
        fail_forget: false,
    });
    store
        .memory
        .save(
            initial.scope(),
            initial.credentials().load().unwrap().unwrap(),
        )
        .unwrap();
    let cx = custom_context(
        server.server().http(),
        store.clone(),
        Arc::new(MemoryAuditSink::default()),
    );
    assert_eq!(
        sender(server.server()).send(&cx, document()).err(),
        Some(PluginError::CredentialStore)
    );
    assert_eq!(stored(&cx)["refresh_token"], "SYNTHETIC_REFRESH");
    server.assert_finished();
}

#[test]
fn failed_load_and_forget_are_not_reported_as_success() {
    let server = ScriptedServer::start(vec![]).unwrap();
    let audit = Arc::new(MemoryAuditSink::default());
    let store = Arc::new(FailingStore {
        memory: MemoryCredentialStore::new(),
        fail_load: true,
        fail_save: false,
        fail_forget: true,
    });
    let cx = custom_context(server.server().http(), store, audit.clone());
    let sender = sender(server.server());
    assert_eq!(
        sender.send(&cx, document()).err(),
        Some(PluginError::CredentialStore)
    );
    assert_eq!(sender.disconnect(&cx), Err(PluginError::CredentialStore));
    assert!(audit
        .events()
        .iter()
        .all(|e| e.outcome == AuditOutcome::Failed));
    server.assert_finished();
}

struct FailingAudit;
impl AuditSink for FailingAudit {
    fn record(&self, _event: AuditEvent) -> PluginResult<()> {
        Err(PluginError::Audit)
    }
}

#[test]
fn audit_failure_after_acceptance_is_reported_without_a_second_send() {
    // The rich result retains provider acceptance even when local recording fails.
    let server = ScriptedServer::start(vec![ScriptedExchange {
        method: "POST".into(),
        target: "/v1.0/me/sendMail".into(),
        response: FakeResponse::new(202, ""),
    }])
    .unwrap();
    let cx = custom_context(
        server.server().http(),
        Arc::new(MemoryCredentialStore::new()),
        Arc::new(FailingAudit),
    );
    seed(&cx, NOW + 3600);
    assert!(matches!(
        sender(server.server()).send_outcome(&cx, document()),
        SendOutcome::Accepted {
            receipt: SendReceipt {
                provider_message_id: None,
                sent_at: Timestamp(NOW)
            },
            recording_error: Some(PluginError::Audit)
        }
    ));
    server.assert_finished();
}

#[test]
fn provider_token_configuration_errors_are_not_mistaken_for_revocation() {
    for (code, expected) in [
        ("invalid_client", PluginError::InvalidConfiguration),
        ("unauthorized_client", PluginError::InvalidConfiguration),
        ("invalid_scope", PluginError::InvalidConfiguration),
        ("access_denied", PluginError::OAuthDenied),
        ("invalid_request", PluginError::InvalidResponse),
    ] {
        let server = ScriptedServer::start(vec![ScriptedExchange {
            method: "POST".into(),
            target: "/token".into(),
            response: FakeResponse::new(
                400,
                json!({"error":code,"error_description":"SENSITIVE"}).to_string(),
            ),
        }])
        .unwrap();
        let (cx, _, _) = context(server.server());
        seed(&cx, 0);
        assert_eq!(
            sender(server.server()).send(&cx, document()).err(),
            Some(expected)
        );
        assert!(cx.credentials().load().unwrap().is_some());
        server.assert_finished();
    }
}

#[test]
fn absent_scope_and_resource_qualified_mail_scope_are_accepted() {
    for scope in [
        None,
        Some("https://graph.microsoft.com/Mail.Send offline_access"),
    ] {
        let server = FakeServer::start(move |r| {
            if r.target == "/token" {
                let mut body = token_json();
                if let Some(scope) = scope {
                    body["scope"] = json!(scope);
                } else {
                    body.as_object_mut().unwrap().remove("scope");
                }
                FakeResponse::new(200, body.to_string())
            } else {
                FakeResponse::new(202, "")
            }
        })
        .unwrap();
        let (cx, _, _) = context(&server);
        seed(&cx, 0);
        sender(&server).send(&cx, document()).unwrap();
    }
}

#[test]
fn rich_known_refusal_and_invalid_document_are_not_submitted() {
    let server = ScriptedServer::start(vec![ScriptedExchange {
        method: "POST".into(),
        target: "/v1.0/me/sendMail".into(),
        response: FakeResponse::new(403, "SYNTHETIC_PRIVATE_ERROR"),
    }])
    .unwrap();
    let (cx, audit, _) = context(server.server());
    seed(&cx, NOW + 3600);
    let s = sender(server.server());
    assert!(matches!(
        s.send_outcome(&cx, document()),
        SendOutcome::NotSubmitted {
            error: PluginError::OAuthDenied
        }
    ));
    let mut invalid = document();
    invalid.document.id = Uuid::nil();
    assert!(matches!(
        s.send_outcome(&cx, invalid),
        SendOutcome::NotSubmitted {
            error: PluginError::InvalidConfiguration
        }
    ));
    assert!(audit.document_sends().is_empty());
    server.assert_finished();
}
