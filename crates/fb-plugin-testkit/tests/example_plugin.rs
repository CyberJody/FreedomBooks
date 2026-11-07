//! A complete toy sender: first-party manifest, scoped credentials, the real
//! loopback flow against a fake provider, hardened HTTP, and secret-free audit.
use fb_plugin_api::oauth::*;
use fb_plugin_api::*;
use fb_plugin_testkit::*;
use std::{sync::Arc, thread, time::Duration};
use url::Url;

static TOY: PluginManifest = PluginManifest {
    id: "toy-sender",
    name: "Toy sender (test only)",
    version: "0.1.0",
    slots: &[Slot::DocumentSender],
    auth: AuthKind::OAuthLoopback {
        provider: "Fake provider",
    },
    outside_account: "Synthetic mail account",
    scopes: &["send"],
    data_reads: &[DataKind::DocumentContents, DataKind::RecipientAddresses],
    data_sends: &[DataKind::DocumentContents, DataKind::RecipientAddresses],
    settings_fields: &[],
    network_hosts: &["127.0.0.1"],
};
struct ToySender {
    endpoint: SecretString,
}
impl Plugin for ToySender {
    fn manifest(&self) -> &'static PluginManifest {
        &TOY
    }
}
impl DocumentSender for ToySender {
    fn send(&self, cx: &PluginContext, document: OutgoingDocument) -> PluginResult<SendReceipt> {
        let operation = || {
            let credential = cx.credentials().load()?.ok_or(PluginError::NeedsSignIn)?;
            let token =
                std::str::from_utf8(credential.expose()).map_err(|_| PluginError::NeedsSignIn)?;
            let response = cx.http().execute(HttpRequest {
                method: HttpMethod::Post,
                url: self.endpoint.clone(),
                headers: vec![(
                    "Authorization".into(),
                    SecretString::from(format!("Bearer {token}")),
                )],
                body: document.pdf,
            })?;
            if response.status != 202 {
                return Err(PluginError::InvalidResponse);
            }
            Ok(SendReceipt {
                provider_message_id: Some("synthetic-message-id".into()),
                sent_at: cx.clock().now(),
            })
        };
        let result = operation();
        cx.record_audit(
            AuditAction::Send,
            AuditPurpose::DocumentDelivery,
            if result.is_ok() {
                AuditOutcome::Success
            } else {
                AuditOutcome::Failed
            },
            (0, 0),
            Duration::ZERO,
        )?;
        result
    }
}
fn exchange(provider: &FakeOAuthProvider, grant: &OAuthGrant, form: SecretString) -> HttpResponse {
    provider
        .server()
        .http()
        .execute(HttpRequest {
            method: HttpMethod::Post,
            url: grant.token_endpoint.clone(),
            headers: vec![(
                "Content-Type".into(),
                SecretString::from("application/x-www-form-urlencoded"),
            )],
            body: SecretBytes::new(form.expose_secret().as_bytes().to_vec()),
        })
        .unwrap()
}
fn signed_in_grant(provider: &FakeOAuthProvider) -> OAuthGrant {
    let flow = OAuthLoopback::start(provider.config("toy-public-client", "/callback")).unwrap();
    let authorization = flow.authorization_url().clone();
    let waiter = thread::spawn(move || flow.wait());
    provider.drive_browser(&authorization).unwrap();
    waiter.join().unwrap().unwrap()
}
fn document() -> OutgoingDocument {
    OutgoingDocument {
        document: DocumentRef {
            kind: DocumentKind::Invoice,
            id: Uuid::from_u128(400),
        },
        to: vec![Mailbox {
            address: "customer@example.test".into(),
            display_name: None,
        }],
        subject: "Synthetic invoice".into(),
        body_text: SecretString::from("PRIVATE_EMAIL_BODY_SENTINEL"),
        pdf: SecretBytes::new(b"SYNTHETIC_PDF".to_vec()),
        attachments: vec![],
    }
}
#[test]
fn example_document_sender_passes_the_testkit() {
    let provider = FakeOAuthProvider::start().unwrap();
    let grant = signed_in_grant(&provider);
    let tokens = exchange(&provider, &grant, grant.token_form());
    assert_eq!(tokens.status, 200);
    let tokens: serde_json::Value = serde_json::from_slice(tokens.body.expose()).unwrap();
    // The test provider supplies synthetic claims. Real provider plugins MUST verify
    // the ID-token signature, issuer, audience and expiry before this nonce check.
    grant
        .verify_nonce(tokens["nonce"].as_str().unwrap())
        .unwrap();
    assert_eq!(provider.exchanges(), 1);

    let server = FakeServer::start(|request| {
        assert_eq!(request.method, "POST");
        assert_eq!(request.target, "/send");
        assert_eq!(
            request.header("Authorization"),
            Some("Bearer FAKE_ACCESS_TOKEN")
        );
        assert_eq!(request.body.expose(), b"SYNTHETIC_PDF");
        FakeResponse::new(202, "Accepted")
    })
    .unwrap();
    let sender = Arc::new(ToySender {
        endpoint: SecretString::from(server.url("/send")),
    });
    let mut registration = PluginRegistration::new(sender.manifest());
    registration.document_sender = Some(sender);
    let mut registry = PluginRegistry::new();
    registry.register(registration).unwrap();
    assert_eq!(
        registry.manifests()[0].outside_account,
        "Synthetic mail account"
    );
    assert_eq!(
        registry.manifests()[0].data_sends,
        &[DataKind::DocumentContents, DataKind::RecipientAddresses]
    );
    let scope = test_scope(TOY.id);
    registry
        .set_company_states(vec![CompanyPluginState {
            company_id: scope.company_id,
            connection_id: scope.connection_id,
            plugin_id: scope.plugin_id.clone(),
            enabled: true,
            status: ConnectionStatus::Connected,
        }])
        .unwrap();
    let credentials = Arc::new(MemoryCredentialStore::new());
    let core = Arc::new(FakeCoreBridge::new(scope.clone()));
    let audit = Arc::new(MemoryAuditSink::default());
    let cx = test_context(
        scope.clone(),
        server.http(),
        credentials.clone(),
        core.clone(),
        audit.clone(),
    );
    cx.credentials()
        .save(SecretBytes::new(
            tokens["access_token"].as_str().unwrap().as_bytes().to_vec(),
        ))
        .unwrap();
    core.queue_document(document());
    let document_ref = document().document;
    let rendered = cx.core().rendered_document(document_ref).unwrap();
    let sender = registry
        .connected(&scope, Slot::DocumentSender)
        .unwrap()
        .document_sender
        .as_ref()
        .unwrap();
    let receipt = sender.send(&cx, rendered).unwrap();
    cx.core()
        .record_document_send(document_ref, receipt)
        .unwrap();
    assert_eq!(
        core.calls(),
        vec![
            CoreCall::Render(document_ref),
            CoreCall::DocumentSent(document_ref)
        ]
    );
    let events = audit.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_name(), "plugin.toy-sender.send");
    assert_eq!(events[0].outcome, AuditOutcome::Success);
    for sentinel in [
        "FAKE_ACCESS_TOKEN",
        "SYNTHETIC_PDF",
        "PRIVATE_EMAIL_BODY_SENTINEL",
    ] {
        assert!(!format!("{events:?}").contains(sentinel));
    }
    // A fake-server opt-in never changes the default production address guard.
    assert!(matches!(
        PluginHttp::new(HttpPolicy::new(&["127.0.0.1"]).unwrap())
            .execute(get(&server.url("/send"))),
        Err(PluginError::HttpsRequired)
    ));
    assert!(matches!(
        PluginHttp::new(HttpPolicy::new(&["127.0.0.1"]).unwrap()).execute(get(&format!(
            "https://127.0.0.1:{}/send",
            server.address().port()
        ))),
        Err(PluginError::PrivateAddress)
    ));
    cx.credentials().forget().unwrap();
    assert!(cx.credentials().load().unwrap().is_none());
    assert!(!format!("{credentials:?}").contains("FAKE_ACCESS_TOKEN"));
}

#[test]
fn oauth_refuses_pkce_nonce_and_replayed_authorization_codes() {
    let provider = FakeOAuthProvider::start().unwrap();
    let grant = signed_in_grant(&provider);
    assert_eq!(
        grant.verify_nonce("wrong-nonce"),
        Err(PluginError::OAuthNonce)
    );
    assert_eq!(grant.verify_nonce(""), Err(PluginError::OAuthNonce));
    let response = exchange(&provider, &grant, grant.token_form());
    let body: serde_json::Value = serde_json::from_slice(response.body.expose()).unwrap();
    grant.verify_nonce(body["nonce"].as_str().unwrap()).unwrap();
    assert_eq!(exchange(&provider, &grant, grant.token_form()).status, 400);
    let second = signed_in_grant(&provider);
    assert_eq!(
        second.verify_nonce(body["nonce"].as_str().unwrap()),
        Err(PluginError::OAuthNonce)
    );
    let tampered: Vec<_> =
        url::form_urlencoded::parse(second.token_form().expose_secret().as_bytes())
            .map(|(key, value)| {
                let value = if key == "code_verifier" {
                    "x".repeat(43)
                } else {
                    value.into_owned()
                };
                (key.into_owned(), value)
            })
            .collect();
    let form = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(tampered)
        .finish();
    assert_eq!(
        exchange(&provider, &second, SecretString::from(form)).status,
        400
    );
    assert_eq!(provider.exchanges(), 1);
    assert!(!format!("{grant:?}").contains(grant.code.expose_secret()));
    assert!(!format!("{grant:?}").contains(grant.verifier.expose_secret()));
}

fn tampered_callback(transform: impl FnOnce(&mut Url), expected: PluginError) {
    let provider = FakeOAuthProvider::start().unwrap();
    let flow = OAuthLoopback::start(provider.config("toy-client", "/callback")).unwrap();
    let address = flow.local_addr().unwrap();
    let redirect = provider
        .authorization_redirect(flow.authorization_url())
        .unwrap();
    let mut redirect = Url::parse(redirect.expose_secret()).unwrap();
    transform(&mut redirect);
    let waiter = thread::spawn(move || flow.wait());
    assert_eq!(
        browser_get(&SecretString::from(redirect.to_string()))
            .unwrap()
            .status,
        400
    );
    assert!(matches!(waiter.join().unwrap(), Err(error) if error == expected));
    assert!(std::net::TcpStream::connect_timeout(&address, Duration::from_millis(50)).is_err());
}

fn stray_then_valid_callback(transform: impl FnOnce(&mut Url)) {
    let provider = FakeOAuthProvider::start().unwrap();
    let flow = OAuthLoopback::start(provider.config("toy-client", "/callback")).unwrap();
    let address = flow.local_addr().unwrap();
    let redirect = provider.authorization_redirect(flow.authorization_url()).unwrap();
    let mut stray = Url::parse(redirect.expose_secret()).unwrap();
    transform(&mut stray);
    let waiter = thread::spawn(move || flow.wait());
    assert_eq!(browser_get(&SecretString::from(stray.to_string())).unwrap().status, 400);
    // The exact original callback must remain usable after the stray request;
    // neither normalization nor an aborted listener can satisfy this assertion.
    assert_eq!(browser_get(&redirect).unwrap().status, 200);
    let grant = waiter.join().unwrap().unwrap();
    assert_eq!(exchange(&provider, &grant, grant.token_form()).status, 200);
    assert_eq!(provider.exchanges(), 1);
    assert!(std::net::TcpStream::connect_timeout(&address, Duration::from_millis(50)).is_err());
}
#[test]
fn oauth_refuses_state_mismatch_and_duplicates_but_ignores_missing_state() {
    tampered_callback(
        |url| {
            url.set_query(Some("code=synthetic-code-1&state=wrong"));
        },
        PluginError::OAuthState,
    );
    stray_then_valid_callback(
        |url| {
            url.set_query(Some("code=synthetic-code-1"));
        },
    );
    tampered_callback(
        |url| {
            url.query_pairs_mut().append_pair("state", "duplicate");
        },
        PluginError::OAuthCallback,
    );
    tampered_callback(
        |url| {
            url.query_pairs_mut().append_pair("code", "duplicate");
        },
        PluginError::OAuthCallback,
    );
}
#[test]
fn oauth_ignores_wrong_path_without_normalizing_or_consuming_callback() {
    for path in ["/wrong", "/callback/", "/%63allback", "/CALLBACK"] {
        stray_then_valid_callback(
            |url| {
                url.set_path(path);
            },
        );
    }
}
#[test]
fn oauth_refuses_provider_denial_without_echoing_description() {
    tampered_callback(
        |url| {
            url.query_pairs_mut()
                .append_pair("error", "access_denied")
                .append_pair("error_description", "TOKEN_SENTINEL");
        },
        PluginError::OAuthDenied,
    );
}
#[test]
fn loopback_is_bound_before_browser_opens_and_closes_on_drop() {
    let provider = FakeOAuthProvider::start().unwrap();
    let flow = OAuthLoopback::start(provider.config("toy-client", "/callback")).unwrap();
    let address = flow.local_addr().unwrap();
    assert_eq!(address.ip().to_string(), "127.0.0.1");
    assert!(!address.ip().is_unspecified());
    assert!(std::net::TcpListener::bind(address).is_err());
    drop(flow);
    assert!(std::net::TcpListener::bind(address).is_ok());
}
fn get(url: &str) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Get,
        url: SecretString::from(url),
        headers: vec![],
        body: SecretBytes::new(vec![]),
    }
}
