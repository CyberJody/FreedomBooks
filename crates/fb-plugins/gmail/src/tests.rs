use super::*;
#[path = "failure_tests.rs"]
mod failure_tests;
use base64::engine::general_purpose::STANDARD;
use fb_plugin_testkit::*;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};
use url::Url;

const NOW: u64 = 1_700_000_000;
const CLIENT: &str = "synthetic-desktop-client.apps.googleusercontent.com";
const ACCESS: &str = "SYNTHETIC_ACCESS_SENTINEL";
const REFRESH: &str = "SYNTHETIC_REFRESH_SENTINEL";
const BODY: &str = "PRIVATE_BODY_SENTINEL — café";
const PDF: &[u8] = b"%PDF-1.7\nSYNTHETIC_PDF_SENTINEL\x00\xff\n%%EOF";

fn document() -> OutgoingDocument {
    OutgoingDocument {
        document: DocumentRef {
            kind: DocumentKind::Invoice,
            id: Uuid::from_u128(400),
        },
        to: vec![Mailbox {
            address: "customer@example.test".into(),
            display_name: Some("Café customer".into()),
        }],
        subject: "Synthetic invoice — café".into(),
        body_text: SecretString::from(BODY),
        pdf: SecretBytes::new(PDF.to_vec()),
        attachments: vec![Attachment {
            filename: "notes.txt".into(),
            media_type: "text/plain".into(),
            bytes: SecretBytes::new(b"SYNTHETIC_NOTES".to_vec()),
        }],
    }
}
struct FaultCore {
    fake: FakeCoreBridge,
}
impl CoreBridge for FaultCore {
    fn scope(&self) -> &CredentialScope {
        self.fake.scope()
    }
    fn rendered_document(&self, d: DocumentRef) -> PluginResult<OutgoingDocument> {
        self.fake.rendered_document(d)
    }
    fn stage_bank_transactions(&self, t: Vec<FeedTransaction>) -> PluginResult<StagedFeed> {
        self.fake.stage_bank_transactions(t)
    }
    fn create_draft_bill(&self, b: DraftBill) -> PluginResult<DraftRef> {
        self.fake.create_draft_bill(b)
    }
    fn create_draft_sale(&self, s: DraftSale) -> PluginResult<DraftRef> {
        self.fake.create_draft_sale(s)
    }
    fn create_draft_deposit(&self, d: DraftDeposit) -> PluginResult<DraftRef> {
        self.fake.create_draft_deposit(d)
    }
    fn attach_to_draft(&self, d: DraftRef, a: Attachment) -> PluginResult<()> {
        self.fake.attach_to_draft(d, a)
    }
    fn record_document_send(&self, d: DocumentRef, r: SendReceipt) -> PluginResult<()> {
        let _ = (d, r);
        Err(PluginError::Core)
    }
    fn load_connection_state(&self) -> PluginResult<Option<SecretBytes>> {
        self.fake.load_connection_state()
    }
    fn save_connection_state(&self, s: SecretBytes) -> PluginResult<()> {
        self.fake.save_connection_state(s)
    }
    fn record_audit(&self, e: AuditEvent) -> PluginResult<()> {
        self.fake.record_audit(e)
    }
}
struct Harness {
    cx: PluginContext,
    core: Arc<FakeCoreBridge>,
    audit: Arc<MemoryAuditSink>,
    credentials: Arc<MemoryCredentialStore>,
}
impl Harness {
    fn new(http: PluginHttp) -> Self {
        Self::scoped(http, test_scope("gmail"), false)
    }
    fn scoped(http: PluginHttp, scope: CredentialScope, fail_send: bool) -> Self {
        let core = Arc::new(FakeCoreBridge::new(scope.clone()));
        let bridge: Arc<dyn CoreBridge> = if fail_send {
            Arc::new(FaultCore {
                fake: FakeCoreBridge::new(scope.clone()),
            })
        } else {
            core.clone()
        };
        let audit = Arc::new(MemoryAuditSink::default());
        let credentials = Arc::new(MemoryCredentialStore::new());
        let cx = PluginContext::new(ContextServices {
            credentials: ScopedCredentials::new(scope.clone(), credentials.clone()),
            scope,
            actor: Uuid::from_u128(300),
            audit: audit.clone(),
            http,
            clock: Arc::new(FixedClock(Timestamp(NOW))),
            core: bridge,
        })
        .unwrap();
        Self {
            cx,
            core,
            audit,
            credentials,
        }
    }
    fn seed(&self, expiry: u64) {
        self.cx
            .credentials()
            .save(
                Tokens {
                    client_id: CLIENT.into(),
                    from_address: "sender@example.test".into(),
                    access: SecretString::from(ACCESS),
                    refresh: SecretString::from(REFRESH),
                    expires_at: Timestamp(expiry),
                }
                .encode()
                .unwrap(),
            )
            .unwrap();
    }
    fn tokens(&self) -> Tokens {
        Tokens::decode(&self.cx.credentials().load().unwrap().unwrap()).unwrap()
    }
    fn no_secrets(&self) {
        let text = format!(
            "{:?} {:?} {:?}",
            self.audit.document_sends(),
            self.credentials,
            self.core.calls()
        );
        for secret in [
            ACCESS,
            REFRESH,
            BODY,
            "SYNTHETIC_PDF_SENTINEL",
            "synthetic-client-secret",
        ] {
            assert!(!text.contains(secret));
        }
    }
}
fn sender(server: &FakeServer) -> GmailSender {
    let mut sender = GmailSender::new(CLIENT.into(), None).unwrap();
    sender.endpoints = Endpoints {
        authorize: server.url("/authorize"),
        token: server.url("/token"),
        revoke: server.url("/revoke"),
        send: server.url("/send"),
    };
    sender
}
fn token_reply(access: &str, refresh: Option<&str>) -> FakeResponse {
    let mut value = serde_json::json!({"access_token": access, "token_type": "Bearer", "expires_in": 3600, "scope": SEND_SCOPE});
    if let Some(refresh) = refresh {
        value["refresh_token"] = serde_json::json!(refresh);
    }
    FakeResponse::new(200, serde_json::to_vec(&value).unwrap())
}
fn form(request: &FakeRequest) -> HashMap<String, String> {
    assert_eq!(
        request.header("Content-Type"),
        Some("application/x-www-form-urlencoded")
    );
    url::form_urlencoded::parse(request.body.expose())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}
fn assert_error<T>(result: PluginResult<T>, expected: PluginError) {
    assert!(
        matches!(result, Err(e) if e == expected),
        "wrong fixed error (contents hidden)"
    );
}
fn authorize(sender: &GmailSender, h: &Harness, provider: &FakeOAuthProvider) -> PluginResult<()> {
    let attempt = sender.begin_authorization(&h.cx, "sender@example.test".into())?;
    let url = attempt.authorization_url().clone();
    thread::scope(|scope| {
        let complete = scope.spawn(|| sender.complete_authorization(&h.cx, attempt));
        provider.drive_browser(&url).unwrap();
        complete.join().unwrap()
    })
}

#[test]
fn manifest_registers_only_send_and_has_no_read_scope() {
    MANIFEST.validate().unwrap();
    assert_eq!(MANIFEST.scopes, &[SEND_SCOPE]);
    assert_eq!(MANIFEST.slots, &[Slot::DocumentSender]);
    assert_eq!(
        MANIFEST.network_hosts,
        &[
            "accounts.google.com",
            "oauth2.googleapis.com",
            "gmail.googleapis.com"
        ]
    );
    let sender = Arc::new(GmailSender::new(CLIENT.into(), None).unwrap());
    let mut entry = PluginRegistration::new(sender.manifest());
    entry.document_sender = Some(sender);
    let mut registry = PluginRegistry::new();
    registry.register(entry).unwrap();
    assert!(registry
        .connected(&test_scope("gmail"), Slot::DocumentSender)
        .is_err());
    registry
        .set_company_states(vec![CompanyPluginState {
            company_id: test_scope("gmail").company_id,
            connection_id: test_scope("gmail").connection_id,
            plugin_id: "gmail".into(),
            enabled: true,
            status: ConnectionStatus::Connected,
        }])
        .unwrap();
    assert!(registry
        .connected(&test_scope("gmail"), Slot::DocumentSender)
        .is_ok());
}

#[test]
fn sends_base64url_mime_and_preserves_message_id_at_document_audit_boundary() {
    let server = FakeServer::start(|request| {
        assert_eq!(request.method, "POST");
        assert_eq!(request.target, "/send");
        assert_eq!(
            request.header("Authorization"),
            Some(format!("Bearer {ACCESS}").as_str())
        );
        assert_eq!(request.header("Content-Type"), Some("application/json"));
        let value: serde_json::Value = serde_json::from_slice(request.body.expose()).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 1);
        let raw = value["raw"].as_str().unwrap();
        assert!(!raw.contains(['+', '/', '=']));
        let mime = String::from_utf8(URL_SAFE_NO_PAD.decode(raw).unwrap()).unwrap();
        assert!(mime.contains("From: sender@example.test\r\n"));
        assert!(mime.contains("customer@example.test"));
        assert!(mime.contains("Content-Type: multipart/mixed;"));
        assert!(mime.contains("Content-Type: application/pdf"));
        assert!(mime.contains("filename=\"document.pdf\""));
        assert!(mime.contains(&STANDARD.encode(PDF)));
        assert!(mime.contains(&STANDARD.encode("SYNTHETIC_NOTES")));
        FakeResponse::new(200, r#"{"id":"18eabc123","threadId":"18eabc123"}"#)
    })
    .unwrap();
    let h = Harness::new(server.http());
    h.seed(NOW + 3600);
    let receipt = sender(&server).send(&h.cx, document()).unwrap();
    assert_eq!(receipt.provider_message_id.as_deref(), Some("18eabc123"));
    assert_eq!(receipt.sent_at, Timestamp(NOW));
    assert_eq!(
        h.core.sent_receipts(),
        vec![(document().document, receipt.clone())]
    );
    assert_eq!(
        h.core.calls(),
        vec![CoreCall::DocumentSent(document().document)]
    );
    let sends = h.audit.document_sends();
    assert_eq!(sends.len(), 1);
    assert_eq!(sends[0].receipt, receipt);
    assert_eq!(sends[0].document, document().document);
    assert!(h.audit.events().is_empty());
    let events: Vec<_> = sends.into_iter().map(|s| s.event).collect();
    assert_eq!(events[0].event_name(), "plugin.gmail.send");
    assert_eq!(events[0].outcome, AuditOutcome::Success);
    assert!(events[0].bytes_sent > 0 && events[0].bytes_received > 0);
    h.no_secrets();
}

#[test]
fn loopback_authorization_exchanges_pkce_and_stores_refresh_credentials() {
    let provider = Arc::new(FakeOAuthProvider::start().unwrap());
    let proxy = provider.clone();
    let server = FakeServer::start(move |request| {
        assert_eq!(request.target, "/token");
        let p = form(&request);
        assert_eq!(p["client_id"], CLIENT);
        assert_eq!(p["client_secret"], "synthetic-client-secret");
        assert_eq!(p["grant_type"], "authorization_code");
        assert!(!p["code_verifier"].is_empty());
        let checked = proxy
            .server()
            .http()
            .execute(HttpRequest {
                method: HttpMethod::Post,
                url: SecretString::from(proxy.server().url("/token")),
                headers: vec![(
                    "Content-Type".into(),
                    SecretString::from("application/x-www-form-urlencoded"),
                )],
                body: request.body,
            })
            .unwrap();
        assert_eq!(checked.status, 200); // the testkit validates code, PKCE, client and redirect
        token_reply(ACCESS, Some(REFRESH))
    })
    .unwrap();
    let mut sender = sender(&server);
    sender.client_secret = Some(SecretString::from("synthetic-client-secret"));
    sender.endpoints.authorize = provider.server().url("/authorize");
    let h = Harness::new(server.http());
    let attempt = sender
        .begin_authorization(&h.cx, "sender@example.test".into())
        .unwrap();
    let url = Url::parse(attempt.authorization_url().expose_secret()).unwrap();
    let p: HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(p["scope"], SEND_SCOPE);
    assert_eq!(p["access_type"], "offline");
    assert_eq!(p["prompt"], "consent");
    assert_eq!(p["code_challenge_method"], "S256");
    assert!(!p["nonce"].is_empty());
    assert!(attempt.redirect_uri().starts_with("http://127.0.0.1:"));
    let auth = attempt.authorization_url().clone();
    thread::scope(|scope| {
        let worker = scope.spawn(|| sender.complete_authorization(&h.cx, attempt));
        provider.drive_browser(&auth).unwrap();
        worker.join().unwrap().unwrap();
    });
    assert_eq!(provider.exchanges(), 1);
    let t = h.tokens();
    assert_eq!(t.access.expose_secret(), ACCESS);
    assert_eq!(t.refresh.expose_secret(), REFRESH);
    assert_eq!(t.expires_at, Timestamp(NOW + 3600));
    assert_eq!(h.audit.events()[0].event_name(), "plugin.gmail.connected");
    h.no_secrets();
}

#[test]
fn rejects_wrong_state_before_token_exchange() {
    let provider = FakeOAuthProvider::start().unwrap();
    let server = ScriptedServer::start(vec![]).unwrap();
    let mut sender = sender(server.server());
    sender.endpoints.authorize = provider.server().url("/authorize");
    let h = Harness::new(server.server().http());
    let attempt = sender
        .begin_authorization(&h.cx, "sender@example.test".into())
        .unwrap();
    let redirect = provider
        .authorization_redirect(attempt.authorization_url())
        .unwrap();
    let mut redirect = Url::parse(redirect.expose_secret()).unwrap();
    redirect.set_query(Some("code=fake&state=wrong"));
    thread::scope(|scope| {
        let worker = scope.spawn(|| sender.complete_authorization(&h.cx, attempt));
        assert_eq!(
            browser_get(&SecretString::from(redirect.to_string()))
                .unwrap()
                .status,
            400
        );
        assert_error(worker.join().unwrap(), PluginError::OAuthState);
    });
    assert!(h.cx.credentials().load().unwrap().is_none());
    server.assert_finished();
    assert_eq!(h.audit.events()[0].outcome, AuditOutcome::Failed);
}

#[test]
fn authorization_cannot_cross_connection_or_client() {
    let server = ScriptedServer::start(vec![]).unwrap();
    let s = sender(server.server());
    let h = Harness::new(server.server().http());
    let attempt = s
        .begin_authorization(&h.cx, "sender@example.test".into())
        .unwrap();
    let mut other_scope = test_scope("gmail");
    other_scope.connection_id = Uuid::from_u128(777);
    let other = Harness::scoped(server.server().http(), other_scope, false);
    assert_error(
        s.complete_authorization(&other.cx, attempt),
        PluginError::WrongScope,
    );
    let attempt = s
        .begin_authorization(&h.cx, "sender@example.test".into())
        .unwrap();
    let mut other_sender = sender(server.server());
    other_sender.client_id = "different-client".into();
    assert_error(
        other_sender.complete_authorization(&h.cx, attempt),
        PluginError::WrongScope,
    );
    server.assert_finished();
}

#[test]
fn disconnect_invalidates_pending_authorization() {
    let provider = FakeOAuthProvider::start().unwrap();
    let server = ScriptedServer::start(vec![]).unwrap();
    let mut s = sender(server.server());
    s.endpoints.authorize = provider.server().url("/authorize");
    let h = Harness::new(server.server().http());
    let attempt = s
        .begin_authorization(&h.cx, "sender@example.test".into())
        .unwrap();
    let auth = attempt.authorization_url().clone();
    s.disconnect(&h.cx).unwrap();
    thread::scope(|scope| {
        let worker = scope.spawn(|| s.complete_authorization(&h.cx, attempt));
        provider.drive_browser(&auth).unwrap();
        assert_error(worker.join().unwrap(), PluginError::OAuthState);
    });
    assert!(h.cx.credentials().load().unwrap().is_none());
    server.assert_finished();
}

#[test]
fn newer_authorization_invalidates_previous_attempt() {
    let provider = FakeOAuthProvider::start().unwrap();
    let server = ScriptedServer::start(vec![]).unwrap();
    let mut s = sender(server.server());
    s.endpoints.authorize = provider.server().url("/authorize");
    let h = Harness::new(server.server().http());
    let old = s
        .begin_authorization(&h.cx, "sender@example.test".into())
        .unwrap();
    let auth = old.authorization_url().clone();
    let _new = s
        .begin_authorization(&h.cx, "sender@example.test".into())
        .unwrap();
    thread::scope(|scope| {
        let worker = scope.spawn(|| s.complete_authorization(&h.cx, old));
        provider.drive_browser(&auth).unwrap();
        assert_error(worker.join().unwrap(), PluginError::OAuthState);
    });
    server.assert_finished();
}

#[test]
fn initial_authorization_requires_refresh_token_and_valid_expiry() {
    for reply in [
        r#"{"access_token":"fake","token_type":"Bearer","expires_in":3600}"#,
        r#"{"access_token":"fake","refresh_token":"refresh","token_type":"Bearer"}"#,
        r#"{"access_token":"fake","refresh_token":"refresh","token_type":"Bearer","expires_in":0}"#,
    ] {
        let provider = FakeOAuthProvider::start().unwrap();
        let reply = reply.to_owned();
        let server = FakeServer::start(move |_| FakeResponse::new(200, &reply)).unwrap();
        let mut s = sender(&server);
        s.endpoints.authorize = provider.server().url("/authorize");
        let h = Harness::new(server.http());
        assert!(authorize(&s, &h, &provider).is_err());
        assert!(h.cx.credentials().load().unwrap().is_none());
        assert_eq!(h.audit.events().len(), 1);
    }
}

#[test]
fn refresh_preserves_old_refresh_token_when_omitted_and_rotates_when_supplied() {
    for rotate in [false, true] {
        let requests = Arc::new(AtomicUsize::new(0));
        let captured = requests.clone();
        let server = FakeServer::start(move |request| {
            let n = captured.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                assert_eq!(request.target, "/token");
                let p = form(&request);
                assert_eq!(p["grant_type"], "refresh_token");
                assert_eq!(p["client_id"], CLIENT);
                assert_eq!(p["refresh_token"], REFRESH);
                assert!(!p.contains_key("code"));
                token_reply("NEW_ACCESS", rotate.then_some("ROTATED_REFRESH"))
            } else {
                assert_eq!(request.target, "/send");
                assert_eq!(request.header("Authorization"), Some("Bearer NEW_ACCESS"));
                FakeResponse::new(200, r#"{"id":"abc123"}"#)
            }
        })
        .unwrap();
        let h = Harness::new(server.http());
        h.seed(NOW + 60);
        sender(&server).send(&h.cx, document()).unwrap();
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        let t = h.tokens();
        assert_eq!(
            t.refresh.expose_secret(),
            if rotate { "ROTATED_REFRESH" } else { REFRESH }
        );
        assert_eq!(t.expires_at, Timestamp(NOW + 3600));
    }
}

#[test]
fn refresh_invalid_grant_drops_credentials_and_marks_needs_sign_in() {
    let server = ScriptedServer::start(vec![ScriptedExchange {
        method: "POST".into(),
        target: "/token".into(),
        response: FakeResponse::new(
            400,
            r#"{"error":"invalid_grant","error_description":"PRIVATE_PROVIDER_SENTINEL"}"#,
        ),
    }])
    .unwrap();
    let h = Harness::new(server.server().http());
    h.seed(NOW);
    assert_error(
        sender(server.server()).send(&h.cx, document()),
        PluginError::NeedsSignIn,
    );
    assert!(h.cx.credentials().load().unwrap().is_none());
    server.assert_finished();
    assert_eq!(h.audit.events()[0].outcome, AuditOutcome::NeedsSignIn);
    h.no_secrets();
}

#[test]
fn transient_and_invalid_refresh_responses_preserve_credentials_and_never_send() {
    for (status, body, expected) in [
        (500, "PRIVATE_PROVIDER_SENTINEL", PluginError::Network),
        (429, "", PluginError::Network),
        (
            400,
            r#"{"error":"invalid_client"}"#,
            PluginError::InvalidConfiguration,
        ),
        (200, "not json", PluginError::InvalidResponse),
        (
            200,
            r#"{"access_token":"new","refresh_token":"","token_type":"Bearer","expires_in":3600}"#,
            PluginError::InvalidResponse,
        ),
        (
            200,
            r#"{"access_token":"new","token_type":"MAC","expires_in":3600}"#,
            PluginError::InvalidResponse,
        ),
        (
            200,
            r#"{"access_token":"new","token_type":"Bearer","expires_in":3600,"scope":"https://mail.google.com/"}"#,
            PluginError::InvalidResponse,
        ),
    ] {
        let server = ScriptedServer::start(vec![ScriptedExchange {
            method: "POST".into(),
            target: "/token".into(),
            response: FakeResponse::new(status, body),
        }])
        .unwrap();
        let h = Harness::new(server.server().http());
        h.seed(NOW);
        assert_error(sender(server.server()).send(&h.cx, document()), expected);
        assert_eq!(h.tokens().refresh.expose_secret(), REFRESH);
        assert!(h.core.sent_receipts().is_empty());
        server.assert_finished();
        h.no_secrets();
    }
}

#[test]
fn parallel_sends_refresh_once_on_the_shared_plugin_instance() {
    let refreshes = Arc::new(AtomicUsize::new(0));
    let captured = refreshes.clone();
    let sends = Arc::new(AtomicUsize::new(0));
    let captured_sends = sends.clone();
    let server = FakeServer::start(move |request| {
        if request.target == "/token" {
            captured.fetch_add(1, Ordering::SeqCst);
            token_reply("NEW_ACCESS", Some("NEW_REFRESH"))
        } else {
            captured_sends.fetch_add(1, Ordering::SeqCst);
            FakeResponse::new(200, r#"{"id":"abc123"}"#)
        }
    })
    .unwrap();
    let h = Harness::new(server.http());
    h.seed(NOW);
    let s = sender(&server);
    thread::scope(|scope| {
        let a = scope.spawn(|| s.send(&h.cx, document()));
        let b = scope.spawn(|| s.send(&h.cx, document()));
        a.join().unwrap().unwrap();
        b.join().unwrap().unwrap();
    });
    assert_eq!(refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(sends.load(Ordering::SeqCst), 2);
    assert_eq!(h.audit.document_sends().len(), 2);
}

#[test]
fn missing_corrupt_and_wrong_client_credentials_do_not_send() {
    let server = ScriptedServer::start(vec![]).unwrap();
    let h = Harness::new(server.server().http());
    let s = sender(server.server());
    assert_error(s.send(&h.cx, document()), PluginError::NeedsSignIn);
    h.cx.credentials()
        .save(SecretBytes::new(b"INVALID_CREDENTIAL_SENTINEL".to_vec()))
        .unwrap();
    assert_error(s.send(&h.cx, document()), PluginError::NeedsSignIn);
    assert!(h.cx.credentials().load().unwrap().is_none());
    h.seed(NOW + 3600);
    let mut t = h.tokens();
    t.client_id = "wrong".into();
    h.cx.credentials().save(t.encode().unwrap()).unwrap();
    assert_error(s.send(&h.cx, document()), PluginError::NeedsSignIn);
    server.assert_finished();
}

#[test]
fn sending_cannot_use_another_plugins_scope() {
    let server = ScriptedServer::start(vec![]).unwrap();
    let h = Harness::scoped(server.server().http(), test_scope("smtp"), false);
    h.seed(NOW + 3600);
    assert_error(
        sender(server.server()).send(&h.cx, document()),
        PluginError::WrongScope,
    );
    assert_error(
        sender(server.server()).begin_authorization(&h.cx, "sender@example.test".into()),
        PluginError::WrongScope,
    );
    assert_error(
        sender(server.server()).disconnect(&h.cx),
        PluginError::WrongScope,
    );
    assert!(h.cx.credentials().load().unwrap().is_some());
    server.assert_finished();
}

#[test]
fn send_errors_are_fixed_and_never_automatically_retried() {
    for (status, body, expected, forgotten) in [
        (401, "TOKEN_SENTINEL", PluginError::NeedsSignIn, true),
        (
            403,
            "PRIVATE_PROVIDER_SENTINEL",
            PluginError::OAuthDenied,
            false,
        ),
        (429, "", PluginError::Network, false),
        (500, "", PluginError::Network, false),
        (400, "", PluginError::InvalidResponse, false),
        (200, "not json", PluginError::InvalidResponse, false),
        (200, r#"{"id":""}"#, PluginError::InvalidResponse, false),
        (
            200,
            r#"{"id":"evil\r\nTOKEN_SENTINEL"}"#,
            PluginError::InvalidResponse,
            false,
        ),
    ] {
        let server = ScriptedServer::start(vec![ScriptedExchange {
            method: "POST".into(),
            target: "/send".into(),
            response: FakeResponse::new(status, body),
        }])
        .unwrap();
        let h = Harness::new(server.server().http());
        h.seed(NOW + 3600);
        assert_error(sender(server.server()).send(&h.cx, document()), expected);
        assert_eq!(h.cx.credentials().load().unwrap().is_none(), forgotten);
        assert!(h.core.sent_receipts().is_empty());
        assert_eq!(h.audit.events().len(), 1);
        server.assert_finished();
        h.no_secrets();
    }
}

#[test]
fn revokes_refresh_token_in_body_and_wipes_credentials() {
    let server = FakeServer::start(|request| {
        assert_eq!(request.target, "/revoke");
        assert!(request.header("Authorization").is_none());
        let p = form(&request);
        assert_eq!(p.len(), 1);
        assert_eq!(p["token"], REFRESH);
        FakeResponse::new(200, "")
    })
    .unwrap();
    let h = Harness::new(server.http());
    h.seed(NOW + 3600);
    assert!(
        sender(&server)
            .disconnect(&h.cx)
            .unwrap()
            .revocation_confirmed
    );
    assert!(h.cx.credentials().load().unwrap().is_none());
    assert_eq!(
        h.audit.events()[0].event_name(),
        "plugin.gmail.disconnected"
    );
    h.no_secrets();
}

#[test]
fn revocation_failures_still_forget_but_do_not_claim_upstream_success() {
    for status in [400, 401, 500, 302] {
        let server = ScriptedServer::start(vec![ScriptedExchange {
            method: "POST".into(),
            target: "/revoke".into(),
            response: FakeResponse::new(status, "PRIVATE_PROVIDER_SENTINEL")
                .header("Location", "https://evil.example.test"),
        }])
        .unwrap();
        let h = Harness::new(server.server().http());
        h.seed(NOW + 3600);
        assert!(
            !sender(server.server())
                .disconnect(&h.cx)
                .unwrap()
                .revocation_confirmed
        );
        assert!(h.cx.credentials().load().unwrap().is_none());
        assert_eq!(h.audit.events()[0].outcome, AuditOutcome::Failed);
        server.assert_finished();
    }
}

#[test]
fn disconnect_absent_or_corrupt_credentials_has_no_outbound_request() {
    let server = ScriptedServer::start(vec![]).unwrap();
    let h = Harness::new(server.server().http());
    let s = sender(server.server());
    assert!(!s.disconnect(&h.cx).unwrap().revocation_confirmed);
    h.cx.credentials()
        .save(SecretBytes::new(b"invalid".to_vec()))
        .unwrap();
    assert!(!s.disconnect(&h.cx).unwrap().revocation_confirmed);
    assert!(h.cx.credentials().load().unwrap().is_none());
    server.assert_finished();
}

#[test]
fn http_redirect_timeout_and_response_caps_are_enforced() {
    for mode in [0, 1, 2] {
        let server = FakeServer::start(move |_| match mode {
            0 => FakeResponse::new(302, "").header("Location", "https://evil.example.test"),
            1 => {
                let mut r = FakeResponse::new(200, r#"{"id":"abc123"}"#);
                r.delay = Duration::from_millis(200);
                r
            }
            _ => FakeResponse::new(200, vec![b'x'; 1024]),
        })
        .unwrap();
        let http = PluginHttp::new(
            HttpPolicy::loopback_for_tests(server.address().port())
                .unwrap()
                .with_limits(Duration::from_millis(80), 20 * 1024 * 1024, 128)
                .unwrap(),
        );
        let h = Harness::new(http);
        h.seed(NOW + 3600);
        assert_error(
            sender(&server).send(&h.cx, document()),
            match mode {
                0 => PluginError::Redirect,
                1 => PluginError::Timeout,
                _ => PluginError::TooLarge,
            },
        );
        assert!(h.core.sent_receipts().is_empty());
    }
}

#[test]
fn production_http_policy_refuses_loopback_test_endpoints() {
    let server = ScriptedServer::start(vec![]).unwrap();
    let h = Harness::new(PluginHttp::new(
        HttpPolicy::new(MANIFEST.network_hosts).unwrap(),
    ));
    h.seed(NOW + 3600);
    assert_error(
        sender(server.server()).send(&h.cx, document()),
        PluginError::HttpsRequired,
    );
    server.assert_finished();
}

#[test]
fn malformed_document_headers_and_metadata_never_reach_http() {
    let server = ScriptedServer::start(vec![]).unwrap();
    let h = Harness::new(server.server().http());
    h.seed(NOW);
    let s = sender(server.server());
    for case in 0..11 {
        let mut d = document();
        match case {
            0 => d.subject = "Invoice\r\nBcc: attacker@example.test".into(),
            1 => d.to[0].address = "customer@example.test\nBcc: attacker@example.test".into(),
            2 => d.to[0].display_name = Some("customer\r\nBcc: attacker@example.test".into()),
            3 => d.attachments[0].filename = "a\r\nBcc: evil".into(),
            4 => d.attachments[0].media_type = "text/plain\r\nX: evil".into(),
            5 => d.to.clear(),
            6 => d.pdf = SecretBytes::new(vec![]),
            7 => d.attachments[0].filename = "../notes.txt".into(),
            8 => d.to[0].address = "not an address".into(),
            9 => d.document.id = Uuid::nil(),
            _ => d.to[0].address = "a@example.test, b@example.test".into(),
        }
        assert_error(s.send(&h.cx, d), PluginError::InvalidConfiguration);
    }
    server.assert_finished();
    assert_eq!(h.audit.events().len(), 11);
}

#[test]
fn oversized_documents_are_rejected_before_refresh_or_send() {
    let server = ScriptedServer::start(vec![]).unwrap();
    let h = Harness::new(server.server().http());
    h.seed(NOW);
    let mut d = document();
    d.pdf = SecretBytes::new(vec![0; mime::MAX_CONTENT_BYTES]);
    assert_error(
        sender(server.server()).send(&h.cx, d),
        PluginError::TooLarge,
    );
    server.assert_finished();
}

#[test]
fn mime_has_recoverable_binary_attachments_and_an_injected_date() {
    let d = document();
    let raw = mime::build("sender@example.test", &d, Timestamp(NOW)).unwrap();
    let text = std::str::from_utf8(raw.expose()).unwrap();
    assert!(text.contains("Date: Tue, 14 Nov 2023 22:13:20 +0000\r\n"));
    let boundary = text
        .split("boundary=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let parts: Vec<_> = text.split(&format!("--{boundary}")).collect();
    assert_eq!(parts.len(), 5); // preamble, text, PDF, extra attachment, terminator
    for (part, expected) in
        parts[1..4]
            .iter()
            .zip([BODY.as_bytes(), PDF, b"SYNTHETIC_NOTES".as_slice()])
    {
        let (_, body) = part.split_once("\r\n\r\n").unwrap();
        let encoded: String = body.split_ascii_whitespace().collect();
        assert_eq!(STANDARD.decode(encoded).unwrap(), expected);
    }
    assert!(text.lines().all(|line| line.len() <= 998));
    if let Ok(path) = std::env::var("FB_GMAIL_MIME_PROBE") {
        std::fs::write(path, raw.expose()).unwrap();
    }
}

#[test]
fn configuration_rejects_header_injection_and_invalid_mailboxes() {
    let server = ScriptedServer::start(vec![]).unwrap();
    let h = Harness::new(server.server().http());
    let s = sender(server.server());
    for from in [
        "",
        "sender@example.test\r\nBcc: evil",
        "bad address",
        "a@example.test, b@example.test",
    ] {
        assert_error(
            s.begin_authorization(&h.cx, from.into()),
            PluginError::InvalidConfiguration,
        );
    }
    assert_error(
        GmailSender::new(String::new(), None),
        PluginError::InvalidConfiguration,
    );
    server.assert_finished();
}

#[test]
fn bridge_failure_is_audited_without_retrying_the_send() {
    let server = ScriptedServer::start(vec![ScriptedExchange {
        method: "POST".into(),
        target: "/send".into(),
        response: FakeResponse::new(200, r#"{"id":"abc123"}"#),
    }])
    .unwrap();
    let h = Harness::scoped(server.server().http(), test_scope("gmail"), true);
    h.seed(NOW + 3600);
    assert_error(
        sender(server.server()).send(&h.cx, document()),
        PluginError::Core,
    );
    assert_eq!(
        h.audit.document_sends()[0].event.outcome,
        AuditOutcome::Success
    );
    assert!(h.audit.events().is_empty());
    server.assert_finished();
}

#[test]
fn token_parser_refuses_bad_types_expiry_tokens_and_scope() {
    for body in [
        r#"{}"#,
        r#"{"access_token":"a","refresh_token":"b","token_type":"Bearer","expires_in":-1}"#,
        r#"{"access_token":"a","refresh_token":"b","token_type":"Bearer","expires_in":999999999999999999}"#,
        r#"{"access_token":"a\r\nInjected","refresh_token":"b","token_type":"Bearer","expires_in":3600}"#,
        r#"{"access_token":"a","refresh_token":"b","token_type":"Bearer","expires_in":3600,"scope":"openid"}"#,
    ] {
        let r = HttpResponse {
            status: 200,
            body: SecretBytes::new(body.as_bytes().to_vec()),
        };
        assert!(
            Tokens::from_response(&r, Timestamp(NOW), CLIENT, "sender@example.test", None).is_err()
        );
    }
}

#[test]
fn accepted_receipt_survives_history_failure_and_still_audits() {
    let server = ScriptedServer::start(vec![ScriptedExchange {
        method: "POST".into(),
        target: "/send".into(),
        response: FakeResponse::new(200, r#"{"id":"accepted123"}"#),
    }])
    .unwrap();
    let h = Harness::scoped(server.server().http(), test_scope("gmail"), true);
    h.seed(NOW + 3600);
    let outcome = sender(server.server()).send_outcome(&h.cx, document());
    assert!(matches!(outcome, SendOutcome::Accepted { ref receipt,
        recording_error: Some(PluginError::Core) }
        if receipt.provider_message_id.as_deref() == Some("accepted123")));
    assert_eq!(
        h.audit.document_sends()[0]
            .receipt
            .provider_message_id
            .as_deref(),
        Some("accepted123")
    );
    assert!(h.audit.events().is_empty());
    server.assert_finished();
}

#[test]
fn lost_final_http_response_is_uncertain_never_retried() {
    let count = Arc::new(AtomicUsize::new(0));
    let captured = count.clone();
    let server = FakeServer::start(move |_| {
        captured.fetch_add(1, Ordering::SeqCst);
        // Claim a complete response length, then close with only a fragment.
        // This proves loss AFTER the fake received the POST, without a tiny
        // deadline racing machine scheduling before the request arrives.
        let mut response = FakeResponse::new(200, br#"{"id":"#).header("Content-Length", "100");
        response.omit_content_length = true;
        response
    })
    .unwrap();
    let h = Harness::new(server.http());
    h.seed(NOW + 3600);
    assert!(matches!(
        sender(&server).send_outcome(&h.cx, document()),
        SendOutcome::Uncertain {
            error: PluginError::Network
        }
    ));
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(h.core.sent_receipts().is_empty());
}

#[test]
fn lifecycle_adapter_preserves_mailbox_binding_and_cancellation() {
    let provider = FakeOAuthProvider::start().unwrap();
    let server = ScriptedServer::start(vec![
        ScriptedExchange {
            method: "POST".into(),
            target: "/token".into(),
            response: token_reply(ACCESS, Some(REFRESH)),
        },
        ScriptedExchange {
            method: "POST".into(),
            target: "/revoke".into(),
            response: FakeResponse::new(200, ""),
        },
    ])
    .unwrap();
    let h = Harness::new(server.server().http());
    let mut s = sender(server.server());
    s.endpoints.authorize = provider.server().url("/authorize");
    let lifecycle = s.connection_lifecycle().unwrap();
    let pending = lifecycle
        .begin(
            &h.cx,
            ConnectInput {
                fields: SecretBytes::new(br#"{"from_address":"sender@example.test"}"#.to_vec()),
            },
        )
        .unwrap();
    let url = pending.authorization_url().clone();
    let update = thread::scope(|scope| {
        let worker = scope.spawn(|| pending.complete(&h.cx));
        provider.drive_browser(&url).unwrap();
        worker.join().unwrap().unwrap()
    });
    update.validate_for(&h.cx).unwrap();
    assert_eq!(update.status, ConnectionStatus::Connected);
    assert_eq!(h.tokens().from_address, "sender@example.test");
    let update = lifecycle.disconnect(&h.cx).unwrap();
    assert_eq!(update.status, ConnectionStatus::Disconnected);
    assert_eq!(update.revocation, RevocationStatus::Confirmed);
    let pending = s
        .begin_authorization(&h.cx, "sender@example.test".into())
        .unwrap();
    h.cx.cancellation().cancel();
    assert_error(
        s.complete_authorization(&h.cx, pending),
        PluginError::Cancelled,
    );
    server.assert_finished();
}
