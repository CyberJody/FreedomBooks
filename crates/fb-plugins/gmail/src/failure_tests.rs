use super::*;

struct FaultStore {
    memory: Arc<MemoryCredentialStore>,
    // 1 = load, 2 = save, 3 = forget
    mode: usize,
}
impl CredentialStore for FaultStore {
    fn load(&self, scope: &CredentialScope) -> PluginResult<Option<SecretBytes>> {
        if self.mode == 1 {
            return Err(PluginError::CredentialStore);
        }
        self.memory.load(scope)
    }
    fn save(&self, scope: &CredentialScope, value: SecretBytes) -> PluginResult<()> {
        if self.mode == 2 {
            return Err(PluginError::CredentialStore);
        }
        self.memory.save(scope, value)
    }
    fn forget(&self, scope: &CredentialScope) -> PluginResult<()> {
        if self.mode == 3 {
            return Err(PluginError::CredentialStore);
        }
        self.memory.forget(scope)
    }
}
struct FaultAudit;
impl AuditSink for FaultAudit {
    fn record(&self, _: AuditEvent) -> PluginResult<()> {
        Err(PluginError::Audit)
    }
}
fn context(
    h: &Harness,
    http: PluginHttp,
    store: Arc<dyn CredentialStore>,
    audit: Arc<dyn AuditSink>,
) -> PluginContext {
    PluginContext::new(ContextServices {
        scope: h.cx.scope().clone(),
        actor: h.cx.actor(),
        credentials: ScopedCredentials::new(h.cx.scope().clone(), store),
        audit,
        http,
        clock: Arc::new(FixedClock(Timestamp(NOW))),
        core: h.core.clone(),
    })
    .unwrap()
}

#[test]
fn credential_load_failure_has_no_provider_request() {
    let server = ScriptedServer::start(vec![]).unwrap();
    let h = Harness::new(server.server().http());
    h.seed(NOW + 3600);
    let store = Arc::new(FaultStore {
        memory: h.credentials.clone(),
        mode: 1,
    });
    let cx = context(&h, server.server().http(), store, h.audit.clone());
    assert_error(
        sender(server.server()).send(&cx, document()),
        PluginError::CredentialStore,
    );
    assert_eq!(h.audit.events()[0].outcome, AuditOutcome::Failed);
    server.assert_finished();
}

#[test]
fn refreshed_credentials_must_save_before_any_message_is_sent() {
    let server = ScriptedServer::start(vec![ScriptedExchange {
        method: "POST".into(),
        target: "/token".into(),
        response: token_reply("NEW_ACCESS", Some("ROTATED_REFRESH")),
    }])
    .unwrap();
    let h = Harness::new(server.server().http());
    h.seed(NOW);
    let store = Arc::new(FaultStore {
        memory: h.credentials.clone(),
        mode: 2,
    });
    let cx = context(&h, server.server().http(), store, h.audit.clone());
    assert_error(
        sender(server.server()).send(&cx, document()),
        PluginError::CredentialStore,
    );
    assert!(h.core.sent_receipts().is_empty());
    assert_eq!(h.tokens().refresh.expose_secret(), REFRESH);
    server.assert_finished();
}

#[test]
fn failed_token_forget_is_not_reported_as_success() {
    let server = ScriptedServer::start(vec![ScriptedExchange {
        method: "POST".into(),
        target: "/token".into(),
        response: FakeResponse::new(400, r#"{"error":"invalid_grant"}"#),
    }])
    .unwrap();
    let h = Harness::new(server.server().http());
    h.seed(NOW);
    let store = Arc::new(FaultStore {
        memory: h.credentials.clone(),
        mode: 3,
    });
    let cx = context(&h, server.server().http(), store, h.audit.clone());
    assert_error(
        sender(server.server()).send(&cx, document()),
        PluginError::CredentialStore,
    );
    assert!(h.cx.credentials().load().unwrap().is_some());
    assert_eq!(h.audit.events()[0].outcome, AuditOutcome::Failed);
    server.assert_finished();
}

#[test]
fn disconnect_still_attempts_local_forget_after_load_failure() {
    let server = ScriptedServer::start(vec![]).unwrap();
    let h = Harness::new(server.server().http());
    h.seed(NOW);
    let store = Arc::new(FaultStore {
        memory: h.credentials.clone(),
        mode: 1,
    });
    let cx = context(&h, server.server().http(), store, h.audit.clone());
    assert_error(
        sender(server.server()).disconnect(&cx),
        PluginError::CredentialStore,
    );
    assert!(h.cx.credentials().load().unwrap().is_none());
    server.assert_finished();
}

#[test]
fn local_forget_failure_is_reported_even_when_google_revokes() {
    let server = ScriptedServer::start(vec![ScriptedExchange {
        method: "POST".into(),
        target: "/revoke".into(),
        response: FakeResponse::new(200, ""),
    }])
    .unwrap();
    let h = Harness::new(server.server().http());
    h.seed(NOW);
    let store = Arc::new(FaultStore {
        memory: h.credentials.clone(),
        mode: 3,
    });
    let cx = context(&h, server.server().http(), store, h.audit.clone());
    assert_error(
        sender(server.server()).disconnect(&cx),
        PluginError::CredentialStore,
    );
    assert!(h.cx.credentials().load().unwrap().is_some());
    assert_eq!(h.audit.events()[0].outcome, AuditOutcome::Failed);
    server.assert_finished();
}

#[test]
fn authorization_save_failure_is_audited_without_retaining_tokens() {
    let provider = FakeOAuthProvider::start().unwrap();
    let server = ScriptedServer::start(vec![ScriptedExchange {
        method: "POST".into(),
        target: "/token".into(),
        response: token_reply(ACCESS, Some(REFRESH)),
    }])
    .unwrap();
    let h = Harness::new(server.server().http());
    let store = Arc::new(FaultStore {
        memory: h.credentials.clone(),
        mode: 2,
    });
    let cx = context(&h, server.server().http(), store, h.audit.clone());
    let mut s = sender(server.server());
    s.endpoints.authorize = provider.server().url("/authorize");
    let attempt = s
        .begin_authorization(&cx, "sender@example.test".into())
        .unwrap();
    let auth = attempt.authorization_url().clone();
    thread::scope(|scope| {
        let worker = scope.spawn(|| s.complete_authorization(&cx, attempt));
        provider.drive_browser(&auth).unwrap();
        assert_error(worker.join().unwrap(), PluginError::CredentialStore);
    });
    assert!(h.cx.credentials().load().unwrap().is_none());
    assert_eq!(h.audit.events()[0].outcome, AuditOutcome::Failed);
    server.assert_finished();
}

#[test]
fn audit_failure_does_not_cause_a_second_message_send() {
    let server = ScriptedServer::start(vec![ScriptedExchange {
        method: "POST".into(),
        target: "/send".into(),
        response: FakeResponse::new(200, r#"{"id":"abc123"}"#),
    }])
    .unwrap();
    let h = Harness::new(server.server().http());
    h.seed(NOW + 3600);
    let cx = context(
        &h,
        server.server().http(),
        h.credentials.clone(),
        Arc::new(FaultAudit),
    );
    assert!(
        matches!(sender(server.server()).send_outcome(&cx, document()),
        SendOutcome::Accepted { ref receipt, recording_error: Some(PluginError::Audit) }
        if receipt.provider_message_id.as_deref() == Some("abc123"))
    );
    assert_eq!(
        h.core.sent_receipts()[0].1.provider_message_id.as_deref(),
        Some("abc123")
    );
    server.assert_finished();
}

#[test]
fn audit_failure_cannot_prevent_local_forget() {
    let server = ScriptedServer::start(vec![ScriptedExchange {
        method: "POST".into(),
        target: "/revoke".into(),
        response: FakeResponse::new(200, ""),
    }])
    .unwrap();
    let h = Harness::new(server.server().http());
    h.seed(NOW);
    let cx = context(
        &h,
        server.server().http(),
        h.credentials.clone(),
        Arc::new(FaultAudit),
    );
    assert_error(sender(server.server()).disconnect(&cx), PluginError::Audit);
    assert!(h.cx.credentials().load().unwrap().is_none());
    server.assert_finished();
}

#[test]
fn configured_mailboxes_are_per_connection_even_with_one_shared_plugin_and_store() {
    let sends = Arc::new(AtomicUsize::new(0));
    let captured = sends.clone();
    let server = FakeServer::start(move |request| {
        assert_eq!(request.target, "/send");
        let value: serde_json::Value = serde_json::from_slice(request.body.expose()).unwrap();
        let mime = String::from_utf8(
            URL_SAFE_NO_PAD
                .decode(value["raw"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        let n = captured.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            assert!(mime.contains("From: sender@example.test\r\n"));
            assert_eq!(
                request.header("Authorization"),
                Some(format!("Bearer {ACCESS}").as_str())
            );
        } else {
            assert!(mime.contains("From: second@example.test\r\n"));
            assert_eq!(
                request.header("Authorization"),
                Some("Bearer SECOND_ACCESS")
            );
        }
        FakeResponse::new(200, r#"{"id":"abc123"}"#)
    })
    .unwrap();
    let first = Harness::new(server.http());
    first.seed(NOW + 3600);
    let mut other_scope = first.cx.scope().clone();
    other_scope.company_id = Uuid::from_u128(900);
    other_scope.connection_id = Uuid::from_u128(901);
    let mut second = Harness::scoped(server.http(), other_scope.clone(), false);
    second.credentials = first.credentials.clone();
    second.cx = context(
        &second,
        server.http(),
        first.credentials.clone(),
        second.audit.clone(),
    );
    second
        .cx
        .credentials()
        .save(
            Tokens {
                client_id: CLIENT.into(),
                from_address: "second@example.test".into(),
                access: SecretString::from("SECOND_ACCESS"),
                refresh: SecretString::from("SECOND_REFRESH"),
                expires_at: Timestamp(NOW + 3600),
            }
            .encode()
            .unwrap(),
        )
        .unwrap();
    let s = sender(&server);
    s.send(&first.cx, document()).unwrap();
    s.send(&second.cx, document()).unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 2);
    assert_eq!(first.tokens().access.expose_secret(), ACCESS);
    assert_eq!(second.tokens().access.expose_secret(), "SECOND_ACCESS");
    assert_eq!(
        first.audit.document_sends()[0].event.scope,
        *first.cx.scope()
    );
    assert_eq!(second.audit.document_sends()[0].event.scope, other_scope);
}

#[test]
fn escaped_json_tokens_roundtrip_without_losing_secret_wrapping() {
    let response = HttpResponse { status: 200, body: SecretBytes::new(br#"{"access_token":"escaped\u005faccess","refresh_token":"escaped\u005frefresh","expires_in":3600,"token_type":"Bearer"}"#.to_vec()) };
    let tokens = Tokens::from_response(
        &response,
        Timestamp(NOW),
        CLIENT,
        "sender@example.test",
        None,
    )
    .unwrap();
    assert_eq!(tokens.access.expose_secret(), "escaped_access");
    assert_eq!(tokens.refresh.expose_secret(), "escaped_refresh");
    let encoded = tokens.encode().unwrap();
    let decoded = Tokens::decode(&encoded).unwrap();
    assert_eq!(decoded.refresh.expose_secret(), "escaped_refresh");
    assert!(!format!("{:?}", encoded).contains("escaped_refresh"));
}

#[test]
fn invalid_clock_range_is_an_error_not_a_mime_builder_panic() {
    for at in [253_402_300_800, u64::MAX] {
        assert_error(
            mime::build("sender@example.test", &document(), Timestamp(at)),
            PluginError::InvalidConfiguration,
        );
    }
}

#[test]
fn input_cardinality_limits_are_checked_before_refresh() {
    let server = ScriptedServer::start(vec![]).unwrap();
    let h = Harness::new(server.server().http());
    h.seed(NOW);
    let mut recipients = document();
    for _ in 0..100 {
        recipients.to.push(Mailbox {
            address: "more@example.test".into(),
            display_name: None,
        });
    }
    assert_error(
        sender(server.server()).send(&h.cx, recipients),
        PluginError::InvalidConfiguration,
    );
    let mut files = document();
    for _ in 0..20 {
        files.attachments.push(Attachment {
            filename: "extra.txt".into(),
            media_type: "text/plain".into(),
            bytes: SecretBytes::new(vec![]),
        });
    }
    assert_error(
        sender(server.server()).send(&h.cx, files),
        PluginError::InvalidConfiguration,
    );
    server.assert_finished();
}

#[test]
fn oauth_denial_closes_attempt_and_records_no_secret_provider_text() {
    let provider = FakeOAuthProvider::start().unwrap();
    let server = ScriptedServer::start(vec![]).unwrap();
    let h = Harness::new(server.server().http());
    let mut s = sender(server.server());
    s.endpoints.authorize = provider.server().url("/authorize");
    let attempt = s
        .begin_authorization(&h.cx, "sender@example.test".into())
        .unwrap();
    let redirect = provider
        .authorization_redirect(attempt.authorization_url())
        .unwrap();
    let mut redirect = Url::parse(redirect.expose_secret()).unwrap();
    let state = redirect
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    redirect.set_query(None);
    redirect.query_pairs_mut().extend_pairs([
        ("state", state.as_str()),
        ("error", "access_denied"),
        ("error_description", "PRIVATE_PROVIDER_SENTINEL"),
    ]);
    thread::scope(|scope| {
        let worker = scope.spawn(|| s.complete_authorization(&h.cx, attempt));
        assert_eq!(
            browser_get(&SecretString::from(redirect.to_string()))
                .unwrap()
                .status,
            400
        );
        assert_error(worker.join().unwrap(), PluginError::OAuthDenied);
    });
    assert_eq!(h.audit.events()[0].outcome, AuditOutcome::Refused);
    assert!(h.cx.credentials().load().unwrap().is_none());
    server.assert_finished();
}
