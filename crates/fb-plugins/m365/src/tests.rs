use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use fb_plugin_testkit::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::{thread, time::Duration};
use url::Url;

const CLIENT: &str = "12345678-1234-1234-1234-123456789abc";
const NOW: u64 = 1_700_000_000;

#[path = "failure_tests.rs"]
mod failures;

fn sender(server: &FakeServer) -> M365Sender {
    let mut sender = M365Sender::new(CLIENT).unwrap();
    sender.token_endpoint = server.url("/token");
    sender.send_endpoint = server.url("/v1.0/me/sendMail");
    sender
}
fn context(server: &FakeServer) -> (PluginContext, Arc<MemoryAuditSink>, Arc<FakeCoreBridge>) {
    let scope = test_scope("m365");
    let audit = Arc::new(MemoryAuditSink::default());
    let core = Arc::new(FakeCoreBridge::new(scope.clone()));
    (
        test_context(
            scope,
            server.http(),
            Arc::new(MemoryCredentialStore::new()),
            core.clone(),
            audit.clone(),
        ),
        audit,
        core,
    )
}
fn seed(cx: &PluginContext, expires_at: u64) {
    cx.credentials()
        .save(SecretBytes::new(
            serde_json::to_vec(&json!({
                "client_id": CLIENT,
                "access_token": "SYNTHETIC_ACCESS",
                "refresh_token": "SYNTHETIC_REFRESH",
                "expires_at": expires_at,
            }))
            .unwrap(),
        ))
        .unwrap();
}
fn stored(cx: &PluginContext) -> Value {
    serde_json::from_slice(cx.credentials().load().unwrap().unwrap().expose()).unwrap()
}
fn token_json() -> Value {
    json!({"access_token":"NEW_ACCESS", "refresh_token":"ROTATED_REFRESH",
        "token_type":"Bearer", "expires_in":3600, "scope":"Mail.Send"})
}
fn token_response() -> FakeResponse {
    FakeResponse::new(200, token_json().to_string())
}
fn document() -> OutgoingDocument {
    OutgoingDocument {
        document: DocumentRef {
            id: Uuid::from_u128(400),
            kind: DocumentKind::Invoice,
        },
        to: vec![Mailbox {
            address: "customer@example.test".into(),
            display_name: Some("Synthetic Customer".into()),
        }],
        subject: "Synthetic invoice \"one\" — test".into(),
        body_text: SecretString::from("PRIVATE_BODY\nPlain text <not HTML>"),
        pdf: SecretBytes::new(b"%PDF-synthetic\0\xff".to_vec()),
        attachments: vec![Attachment {
            filename: "terms.txt".into(),
            media_type: "text/plain".into(),
            bytes: SecretBytes::new(b"SYNTHETIC_TERMS".to_vec()),
        }],
    }
}
fn form(request: &FakeRequest) -> HashMap<String, String> {
    url::form_urlencoded::parse(request.body.expose())
        .into_owned()
        .collect()
}
fn sign_in(
    sender: &mut M365Sender,
    cx: &PluginContext,
    provider: Arc<FakeOAuthProvider>,
) -> PluginResult<()> {
    sender.authorize_endpoint = provider.server().url("/authorize");
    let pending = sender.start_authorization(cx)?;
    let url = pending.authorization_url().clone();
    let browser = thread::spawn(move || provider.drive_browser(&url));
    let result = sender.complete_authorization(cx, pending);
    browser.join().unwrap().unwrap();
    result
}

#[test]
fn manifest_and_production_endpoints_are_minimal_and_fixed() {
    MANIFEST.validate().unwrap();
    assert_eq!(MANIFEST.scopes, &["Mail.Send", "offline_access"]);
    assert_eq!(MANIFEST.slots, &[Slot::DocumentSender]);
    let sender = M365Sender::new(CLIENT).unwrap();
    assert_eq!(sender.token_endpoint, TOKEN);
    assert_eq!(sender.send_endpoint, SEND);
    assert!(sender.authorize_endpoint.contains("/common/"));
    let plugin: Arc<dyn DocumentSender> = Arc::new(sender);
    let mut registration = PluginRegistration::new(&MANIFEST);
    registration.document_sender = Some(plugin);
    PluginRegistry::new().register(registration).unwrap();
    for bad in [
        "",
        "not-an-id",
        "00000000-0000-0000-0000-000000000000",
        " 12345678-1234-1234-1234-123456789abc",
    ] {
        assert_eq!(
            M365Sender::new(bad).err(),
            Some(PluginError::InvalidConfiguration)
        );
    }
}

#[test]
fn oauth_pkce_loopback_connect_send_disconnect_end_to_end() {
    let provider = Arc::new(FakeOAuthProvider::start().unwrap());
    let token_provider = provider.clone();
    let sends = Arc::new(AtomicUsize::new(0));
    let seen = sends.clone();
    let server = FakeServer::start(move |request| {
        assert_eq!(request.method, "POST");
        if request.target == "/token" {
            let form = form(&request);
            assert_eq!(form["client_id"], CLIENT);
            assert_eq!(form["grant_type"], "authorization_code");
            assert!(!form.contains_key("client_secret"));
            assert!(form["redirect_uri"].starts_with("http://127.0.0.1:"));
            // Testkit's provider verifies code binding, PKCE S256, redirect,
            // client id and single-use code, then this fake adds Microsoft fields.
            let response = token_provider
                .server()
                .http()
                .execute(HttpRequest {
                    method: HttpMethod::Post,
                    url: SecretString::from(token_provider.server().url("/token")),
                    headers: vec![],
                    body: request.body,
                })
                .unwrap();
            assert_eq!(response.status, 200);
            return token_response();
        }
        assert_eq!(request.target, "/v1.0/me/sendMail");
        assert_eq!(request.header("Authorization"), Some("Bearer NEW_ACCESS"));
        assert_eq!(request.header("Content-Type"), Some("application/json"));
        let body: Value = serde_json::from_slice(request.body.expose()).unwrap();
        assert_eq!(body["saveToSentItems"], true);
        assert_eq!(body["message"]["subject"], document().subject);
        assert_eq!(body["message"]["body"]["contentType"], "Text");
        assert_eq!(
            body["message"]["body"]["content"],
            document().body_text.expose_secret()
        );
        assert_eq!(
            body["message"]["toRecipients"][0]["emailAddress"]["address"],
            "customer@example.test"
        );
        assert!(body["message"].get("from").is_none());
        let attachments = body["message"]["attachments"].as_array().unwrap();
        assert_eq!(attachments.len(), 2);
        assert_eq!(
            attachments[0]["@odata.type"],
            "#microsoft.graph.fileAttachment"
        );
        assert_eq!(attachments[0]["name"], "document.pdf");
        assert_eq!(attachments[0]["contentType"], "application/pdf");
        assert_eq!(
            STANDARD
                .decode(attachments[0]["contentBytes"].as_str().unwrap())
                .unwrap(),
            document().pdf.expose()
        );
        assert_eq!(
            STANDARD
                .decode(attachments[1]["contentBytes"].as_str().unwrap())
                .unwrap(),
            b"SYNTHETIC_TERMS"
        );
        seen.fetch_add(1, Ordering::SeqCst);
        FakeResponse::new(202, "")
    })
    .unwrap();
    let (cx, audit, core) = context(&server);
    let mut sender = sender(&server);
    sender.authorize_endpoint = provider.server().url("/authorize");
    let pending = sender.start_authorization(&cx).unwrap();
    let url = Url::parse(pending.authorization_url().expose_secret()).unwrap();
    let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(params["scope"], "Mail.Send offline_access");
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(params["response_mode"], "query");
    assert_eq!(params["prompt"], "select_account");
    assert!(params.contains_key("nonce"));
    drop(pending);
    sign_in(&mut sender, &cx, provider.clone()).unwrap();
    assert_eq!(provider.exchanges(), 1);
    assert_eq!(stored(&cx)["refresh_token"], "ROTATED_REFRESH");
    let receipt = sender.send(&cx, document()).unwrap();
    assert!(receipt.provider_message_id.is_none());
    assert_eq!(receipt.sent_at, Timestamp(NOW));
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    assert!(core.calls().is_empty()); // Host records delivery; never writes books itself.
    sender.disconnect(&cx).unwrap();
    assert!(cx.credentials().load().unwrap().is_none());
    let events = audit.events();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|e| e.outcome == AuditOutcome::Success));
    assert!(events[0].bytes_sent > 0 && events[0].bytes_received > 0);
    let sent = &audit.document_sends()[0];
    assert!(sent.event.bytes_sent > 0);
    assert_eq!(sent.receipt, receipt);
    assert_eq!(sent.document, document().document);
    let text = format!("{events:?}");
    for secret in [
        "NEW_ACCESS",
        "ROTATED_REFRESH",
        "PRIVATE_BODY",
        "customer@example.test",
        "SYNTHETIC_TERMS",
    ] {
        assert!(!text.contains(secret));
    }
}

#[test]
fn expired_token_refreshes_rotates_and_persists_before_send() {
    let server = FakeServer::start(|request| {
        if request.target == "/token" {
            let form = form(&request);
            assert_eq!(form["grant_type"], "refresh_token");
            assert_eq!(form["refresh_token"], "SYNTHETIC_REFRESH");
            assert_eq!(form["scope"], "Mail.Send offline_access");
            assert_eq!(form["client_id"], CLIENT);
            assert!(!form.contains_key("client_secret"));
            token_response()
        } else {
            assert_eq!(request.header("Authorization"), Some("Bearer NEW_ACCESS"));
            FakeResponse::new(202, "")
        }
    })
    .unwrap();
    let (cx, audit, _) = context(&server);
    seed(&cx, NOW + 60);
    sender(&server).send(&cx, document()).unwrap();
    assert_eq!(stored(&cx)["expires_at"], NOW + 3600);
    assert_eq!(stored(&cx)["refresh_token"], "ROTATED_REFRESH");
    assert_eq!(audit.document_sends().len(), 1);
    assert!(audit.events().is_empty());
}

#[test]
fn refresh_without_replacement_keeps_existing_refresh_token() {
    let server = FakeServer::start(|r| {
        if r.target == "/token" {
            let mut body = token_json();
            body.as_object_mut().unwrap().remove("refresh_token");
            FakeResponse::new(200, body.to_string())
        } else {
            FakeResponse::new(202, "")
        }
    })
    .unwrap();
    let (cx, _, _) = context(&server);
    seed(&cx, 0);
    sender(&server).send(&cx, document()).unwrap();
    assert_eq!(stored(&cx)["refresh_token"], "SYNTHETIC_REFRESH");
}

#[test]
fn simultaneous_sends_refresh_only_once() {
    let refreshes = Arc::new(AtomicUsize::new(0));
    let seen = refreshes.clone();
    let server = FakeServer::start(move |r| {
        if r.target == "/token" {
            seen.fetch_add(1, Ordering::SeqCst);
            token_response()
        } else {
            FakeResponse::new(202, "")
        }
    })
    .unwrap();
    let (cx, _, _) = context(&server);
    let cx = Arc::new(cx);
    seed(&cx, 0);
    let sender = Arc::new(sender(&server));
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let cx = cx.clone();
            let sender = sender.clone();
            thread::spawn(move || sender.send(&cx, document()).is_ok())
        })
        .collect();
    for worker in workers {
        assert!(worker.join().unwrap());
    }
    assert_eq!(refreshes.load(Ordering::SeqCst), 1);
}

#[test]
fn revoked_refresh_is_forgotten_and_requires_sign_in() {
    for code in [
        "invalid_grant",
        "interaction_required",
        "login_required",
        "consent_required",
    ] {
        let server = ScriptedServer::start(vec![ScriptedExchange {
            method: "POST".into(),
            target: "/token".into(),
            response: FakeResponse::new(
                400,
                json!({"error":code,"error_description":"PRIVATE_BODY"}).to_string(),
            ),
        }])
        .unwrap();
        let (cx, audit, _) = context(server.server());
        seed(&cx, 0);
        let error = sender(server.server()).send(&cx, document()).err().unwrap();
        assert_eq!(error, PluginError::NeedsSignIn);
        assert!(!error.to_string().contains("PRIVATE_BODY"));
        assert!(cx.credentials().load().unwrap().is_none());
        assert_eq!(audit.events()[0].outcome, AuditOutcome::NeedsSignIn);
        server.assert_finished();
    }
}

#[test]
fn temporary_refresh_errors_keep_credentials_and_never_send() {
    for status in [429, 500, 503] {
        let server = ScriptedServer::start(vec![ScriptedExchange {
            method: "POST".into(),
            target: "/token".into(),
            response: FakeResponse::new(status, "PRIVATE_PROVIDER_BODY"),
        }])
        .unwrap();
        let (cx, _, _) = context(server.server());
        seed(&cx, 0);
        assert_eq!(
            sender(server.server()).send(&cx, document()).err(),
            Some(PluginError::Network)
        );
        assert_eq!(stored(&cx)["refresh_token"], "SYNTHETIC_REFRESH");
        server.assert_finished();
    }
}

#[test]
fn malformed_or_wrong_scope_tokens_are_not_saved_or_used() {
    let mut bodies = vec!["not-json".into(), "{}".into()];
    for (key, bad) in [
        ("access_token", json!("")),
        ("access_token", json!("bad\r\ntoken")),
        ("refresh_token", json!("")),
        ("token_type", json!("mac")),
        ("expires_in", json!(0)),
        ("expires_in", json!(u64::MAX)),
        ("expires_in", json!(-1)),
        ("expires_in", json!("3600")),
        ("scope", json!("User.Read")),
        ("id_token", json!("UNVERIFIED_JWT")),
    ] {
        let mut value = token_json();
        value[key] = bad;
        bodies.push(value.to_string());
    }
    for body in bodies {
        let server = ScriptedServer::start(vec![ScriptedExchange {
            method: "POST".into(),
            target: "/token".into(),
            response: FakeResponse::new(200, body),
        }])
        .unwrap();
        let (cx, _, _) = context(server.server());
        seed(&cx, 0);
        assert!(sender(server.server()).send(&cx, document()).is_err());
        assert_eq!(stored(&cx)["refresh_token"], "SYNTHETIC_REFRESH");
        server.assert_finished();
    }
}

#[test]
fn initial_sign_in_requires_refresh_token_and_preserves_prior_connection_on_failure() {
    let provider = Arc::new(FakeOAuthProvider::start().unwrap());
    let server = FakeServer::start(|_| {
        let mut body = token_json();
        body.as_object_mut().unwrap().remove("refresh_token");
        FakeResponse::new(200, body.to_string())
    })
    .unwrap();
    let (cx, audit, _) = context(&server);
    seed(&cx, NOW + 3600);
    assert_eq!(
        sign_in(&mut sender(&server), &cx, provider),
        Err(PluginError::InvalidResponse)
    );
    assert_eq!(stored(&cx)["access_token"], "SYNTHETIC_ACCESS");
    assert_eq!(audit.events()[0].outcome, AuditOutcome::Failed);
}

#[test]
fn missing_or_corrupt_credentials_fail_without_http() {
    let server = ScriptedServer::start(vec![]).unwrap();
    let (cx, audit, _) = context(server.server());
    let sender = sender(server.server());
    assert_eq!(
        sender.send(&cx, document()).err(),
        Some(PluginError::NeedsSignIn)
    );
    cx.credentials()
        .save(SecretBytes::new(b"corrupt".to_vec()))
        .unwrap();
    assert_eq!(
        sender.send(&cx, document()).err(),
        Some(PluginError::CredentialStore)
    );
    seed(&cx, NOW + 3600);
    let mut record = stored(&cx);
    record["client_id"] = json!("different-registration");
    cx.credentials()
        .save(SecretBytes::new(record.to_string().into_bytes()))
        .unwrap();
    assert_eq!(
        sender.send(&cx, document()).err(),
        Some(PluginError::NeedsSignIn)
    );
    assert_eq!(audit.events().len(), 3);
    server.assert_finished();
}

#[test]
fn graph_errors_are_fixed_audited_and_never_retried() {
    for (status, error) in [
        (200, PluginError::InvalidResponse),
        (400, PluginError::InvalidResponse),
        (401, PluginError::NeedsSignIn),
        (403, PluginError::OAuthDenied),
        (413, PluginError::TooLarge),
        (429, PluginError::Network),
        (500, PluginError::Network),
        (503, PluginError::Network),
    ] {
        let server = ScriptedServer::start(vec![ScriptedExchange {
            method: "POST".into(),
            target: "/v1.0/me/sendMail".into(),
            response: FakeResponse::new(status, "SECRET_PROVIDER_ERROR"),
        }])
        .unwrap();
        let (cx, audit, _) = context(server.server());
        seed(&cx, NOW + 3600);
        assert_eq!(
            sender(server.server()).send(&cx, document()).err(),
            Some(error)
        );
        assert_eq!(cx.credentials().load().unwrap().is_none(), status == 401);
        assert_eq!(audit.events().len(), 1);
        assert!(!format!("{:?}", audit.events()).contains("SECRET_PROVIDER_ERROR"));
        server.assert_finished();
    }
}

#[test]
fn redirects_are_not_followed_for_graph_or_tokens() {
    for path in ["/token", "/v1.0/me/sendMail"] {
        let server = ScriptedServer::start(vec![ScriptedExchange {
            method: "POST".into(),
            target: path.into(),
            response: FakeResponse::new(302, "")
                .header("Location", "http://169.254.169.254/credentials"),
        }])
        .unwrap();
        let (cx, _, _) = context(server.server());
        seed(&cx, if path == "/token" { 0 } else { NOW + 3600 });
        assert_eq!(
            sender(server.server()).send(&cx, document()).err(),
            Some(PluginError::Redirect)
        );
        assert!(cx.credentials().load().unwrap().is_some());
        server.assert_finished();
    }
}

#[test]
fn invalid_document_is_rejected_before_credentials_or_network() {
    let server = ScriptedServer::start(vec![]).unwrap();
    let (cx, audit, _) = context(server.server());
    let sender = sender(server.server());
    let mut docs = Vec::new();
    let mut d = document();
    d.to.clear();
    docs.push(d);
    let mut d = document();
    d.to[0].address = "bad\r\nbcc:other@example.test".into();
    docs.push(d);
    let mut d = document();
    d.subject.push('\n');
    docs.push(d);
    let mut d = document();
    d.pdf = SecretBytes::new(vec![]);
    docs.push(d);
    let mut d = document();
    d.attachments[0].filename = "../private".into();
    docs.push(d);
    let mut d = document();
    d.attachments[0].media_type = "text/plain\r\n".into();
    docs.push(d);
    let mut d = document();
    d.to[0].display_name = Some("name\0".into());
    docs.push(d);
    for d in docs {
        assert_eq!(
            sender.send(&cx, d).err(),
            Some(PluginError::InvalidConfiguration)
        );
    }
    assert!(audit
        .events()
        .iter()
        .all(|e| e.outcome == AuditOutcome::Refused));
    server.assert_finished();
}

#[test]
fn message_size_caps_include_aggregate_files_and_json_expansion() {
    let mut d = document();
    d.pdf = SecretBytes::new(vec![0; MAX_ATTACHMENT_BYTES]);
    assert_eq!(mail::encode(&d).err(), Some(PluginError::TooLarge));
    d.attachments.clear();
    assert!(mail::encode(&d).is_ok());
    d.body_text = SecretString::from("\0".repeat(64 * 1024));
    assert_eq!(mail::encode(&d).err(), Some(PluginError::TooLarge));
    let mut d = document();
    d.body_text = SecretString::from("x".repeat(64 * 1024 + 1));
    assert_eq!(mail::encode(&d).err(), Some(PluginError::TooLarge));
}

#[test]
fn no_display_name_and_multiple_recipients_are_encoded() {
    let mut d = document();
    d.to[0].display_name = None;
    d.to.push(Mailbox {
        address: "second@example.test".into(),
        display_name: None,
    });
    let encoded = mail::encode(&d).unwrap();
    let body: Value = serde_json::from_slice(encoded.expose()).unwrap();
    assert!(body["message"]["toRecipients"][0]["emailAddress"]
        .get("name")
        .is_none());
    assert_eq!(body["message"]["toRecipients"].as_array().unwrap().len(), 2);
}

#[test]
fn oauth_refuses_bad_state_denial_and_duplicate_callback_parameters() {
    for (query, expected) in [
        ("code=fake&state=wrong", PluginError::OAuthState),
        ("code=fake&state=x&state=x", PluginError::OAuthCallback),
    ] {
        let provider = FakeOAuthProvider::start().unwrap();
        let server = ScriptedServer::start(vec![]).unwrap();
        let (cx, audit, _) = context(server.server());
        let mut sender = sender(server.server());
        sender.authorize_endpoint = provider.server().url("/authorize");
        let pending = sender.start_authorization(&cx).unwrap();
        let mut callback = Url::parse(pending.flow.redirect_uri()).unwrap();
        callback.set_query(Some(query));
        let address = pending.flow.local_addr().unwrap();
        let mut stray = callback.clone();
        stray.set_query(Some("code=fake"));
        let browser = thread::spawn(move || {
            // A request without state is unrelated local traffic: refuse that
            // request while retaining the listener for the actual callback.
            assert_eq!(browser_get(&SecretString::from(stray.to_string())).unwrap().status, 400);
            browser_get(&SecretString::from(callback.to_string()))
        });
        assert_eq!(sender.complete_authorization(&cx, pending), Err(expected));
        assert_eq!(browser.join().unwrap().unwrap().status, 400);
        assert!(std::net::TcpStream::connect_timeout(&address, Duration::from_millis(50)).is_err());
        assert!(cx.credentials().load().unwrap().is_none());
        assert_eq!(audit.events().len(), 1);
        server.assert_finished();
    }
    let provider = FakeOAuthProvider::start().unwrap();
    let server = ScriptedServer::start(vec![]).unwrap();
    let (cx, _, _) = context(server.server());
    let mut sender = sender(server.server());
    sender.authorize_endpoint = provider.server().url("/authorize");
    let pending = sender.start_authorization(&cx).unwrap();
    let mut callback = Url::parse(
        provider
            .authorization_redirect(pending.authorization_url())
            .unwrap()
            .expose_secret(),
    )
    .unwrap();
    callback
        .query_pairs_mut()
        .append_pair("error", "access_denied")
        .append_pair("error_description", "PRIVATE_ERROR");
    let browser = thread::spawn(move || browser_get(&SecretString::from(callback.to_string())));
    assert_eq!(
        sender.complete_authorization(&cx, pending),
        Err(PluginError::OAuthDenied)
    );
    assert_eq!(browser.join().unwrap().unwrap().status, 400);
    server.assert_finished();
}

#[test]
fn pending_authorization_cannot_cross_company_or_registration() {
    let provider = FakeOAuthProvider::start().unwrap();
    let server = ScriptedServer::start(vec![]).unwrap();
    let (cx, _, _) = context(server.server());
    let mut sender = sender(server.server());
    sender.authorize_endpoint = provider.server().url("/authorize");
    let pending = sender.start_authorization(&cx).unwrap();
    sender.client_id = "87654321-4321-4321-4321-cba987654321".into();
    assert_eq!(
        sender.complete_authorization(&cx, pending),
        Err(PluginError::WrongScope)
    );
    let pending = sender.start_authorization(&cx).unwrap();
    let mut scope = test_scope("m365");
    scope.company_id = Uuid::from_u128(777);
    let other = test_context(
        scope.clone(),
        server.server().http(),
        Arc::new(MemoryCredentialStore::new()),
        Arc::new(FakeCoreBridge::new(scope)),
        Arc::new(MemoryAuditSink::default()),
    );
    assert_eq!(
        sender.complete_authorization(&other, pending),
        Err(PluginError::WrongScope)
    );
    server.assert_finished();
}

#[test]
fn foreign_plugin_context_refuses_send_connect_and_disconnect() {
    let server = ScriptedServer::start(vec![]).unwrap();
    let scope = test_scope("other");
    let cx = test_context(
        scope.clone(),
        server.server().http(),
        Arc::new(MemoryCredentialStore::new()),
        Arc::new(FakeCoreBridge::new(scope)),
        Arc::new(MemoryAuditSink::default()),
    );
    let sender = sender(server.server());
    assert_eq!(
        sender.send(&cx, document()).err(),
        Some(PluginError::WrongScope)
    );
    assert_eq!(
        sender.start_authorization(&cx).err(),
        Some(PluginError::WrongScope)
    );
    assert_eq!(sender.disconnect(&cx), Err(PluginError::WrongScope));
    server.assert_finished();
}

#[test]
fn disconnect_invalidates_a_pending_authorization_before_exchange() {
    let provider = FakeOAuthProvider::start().unwrap();
    let server = ScriptedServer::start(vec![]).unwrap();
    let (cx, audit, _) = context(server.server());
    let mut sender = sender(server.server());
    sender.authorize_endpoint = provider.server().url("/authorize");
    let pending = sender.start_authorization(&cx).unwrap();
    let address = pending.flow.local_addr().unwrap();
    sender.disconnect(&cx).unwrap();
    assert_eq!(
        sender.complete_authorization(&cx, pending),
        Err(PluginError::OAuthState)
    );
    assert!(cx.credentials().load().unwrap().is_none());
    assert!(std::net::TcpStream::connect_timeout(&address, Duration::from_millis(50)).is_err());
    assert_eq!(audit.events().len(), 2);
    server.assert_finished();
}

#[test]
fn a_new_connect_invalidates_the_previous_attempt() {
    let provider = FakeOAuthProvider::start().unwrap();
    let server = ScriptedServer::start(vec![]).unwrap();
    let (cx, _, _) = context(server.server());
    let mut sender = sender(server.server());
    sender.authorize_endpoint = provider.server().url("/authorize");
    let previous = sender.start_authorization(&cx).unwrap();
    let current = sender.start_authorization(&cx).unwrap();
    assert_eq!(
        sender.complete_authorization(&cx, previous),
        Err(PluginError::OAuthState)
    );
    assert!(
        sender.operations.lock().unwrap().attempts.get(cx.scope()) == Some(&current.generation)
    );
    drop(current);
    server.assert_finished();
}

#[test]
fn token_response_has_a_small_plugin_limit() {
    let server = ScriptedServer::start(vec![ScriptedExchange {
        method: "POST".into(),
        target: "/token".into(),
        response: FakeResponse::new(200, vec![b'x'; 64 * 1024 + 1]),
    }])
    .unwrap();
    let (cx, _, _) = context(server.server());
    seed(&cx, 0);
    assert_eq!(
        sender(server.server()).send(&cx, document()).err(),
        Some(PluginError::TooLarge)
    );
    assert!(cx.credentials().load().unwrap().is_some());
    server.assert_finished();
}

#[test]
fn lifecycle_reports_scoped_status_and_unknown_local_only_revocation() {
    let provider = FakeOAuthProvider::start().unwrap();
    let server = ScriptedServer::start(vec![ScriptedExchange {
        method: "POST".into(),
        target: "/token".into(),
        response: token_response(),
    }])
    .unwrap();
    let (cx, _, core) = context(server.server());
    let mut sender = sender(server.server());
    sender.authorize_endpoint = provider.server().url("/authorize");
    let lifecycle = sender.connection_lifecycle().unwrap();
    let pending = lifecycle
        .begin(
            &cx,
            ConnectInput {
                fields: SecretBytes::new(b"{}".to_vec()),
            },
        )
        .unwrap();
    let url = pending.authorization_url().clone();
    let update = thread::scope(|scope| {
        let worker = scope.spawn(|| pending.complete(&cx));
        provider.drive_browser(&url).unwrap();
        worker.join().unwrap().unwrap()
    });
    update.validate_for(&cx).unwrap();
    assert_eq!(update.status, ConnectionStatus::Connected);
    assert_eq!(update.expires_at, Some(Timestamp(NOW + 3600)));
    let update = lifecycle.disconnect(&cx).unwrap();
    assert_eq!(update.status, ConnectionStatus::Disconnected);
    assert_eq!(update.revocation, RevocationStatus::Unknown);
    assert!(cx.credentials().load().unwrap().is_none());
    assert!(core.calls().is_empty());
    server.assert_finished();
}

#[test]
fn host_operation_lease_cancels_wait_and_prevents_submission() {
    let server = ScriptedServer::start(vec![]).unwrap();
    let scope = test_scope("m365");
    let lease = OperationLease::new(scope.clone());
    let cx = PluginContext::new_cancellable(
        ContextServices {
            credentials: ScopedCredentials::new(
                scope.clone(),
                Arc::new(MemoryCredentialStore::new()),
            ),
            core: Arc::new(FakeCoreBridge::new(scope.clone())),
            scope,
            actor: Uuid::from_u128(300),
            audit: Arc::new(MemoryAuditSink::default()),
            http: server.server().http(),
            clock: Arc::new(FixedClock(Timestamp(NOW))),
        },
        lease.clone(),
    )
    .unwrap();
    let mut sender = sender(server.server());
    sender.authorize_endpoint = server.server().url("/authorize");
    let pending = sender.start_authorization(&cx).unwrap();
    let address = pending.flow.local_addr().unwrap();
    thread::scope(|scope| {
        let worker = scope.spawn(|| sender.complete_authorization(&cx, pending));
        lease.cancel();
        assert_eq!(worker.join().unwrap(), Err(PluginError::Cancelled));
    });
    assert!(std::net::TcpStream::connect_timeout(&address, Duration::from_millis(50)).is_err());
    assert!(matches!(
        sender.send_outcome(&cx, document()),
        SendOutcome::NotSubmitted {
            error: PluginError::Cancelled
        }
    ));
    server.assert_finished();
}

#[test]
fn lifecycle_clone_shares_generation_invalidation() {
    let server = ScriptedServer::start(vec![]).unwrap();
    let (cx, _, _) = context(server.server());
    let mut sender = sender(server.server());
    sender.authorize_endpoint = server.server().url("/authorize");
    let pending = sender
        .connection_lifecycle()
        .unwrap()
        .begin(
            &cx,
            ConnectInput {
                fields: SecretBytes::new(b"{}".to_vec()),
            },
        )
        .unwrap();
    sender.disconnect(&cx).unwrap();
    assert_eq!(pending.complete(&cx).err(), Some(PluginError::OAuthState));
    assert!(cx.credentials().load().unwrap().is_none());
    server.assert_finished();
}
