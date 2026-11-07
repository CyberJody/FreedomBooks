mod edge;
mod fake;
mod reply_limits;
use super::*;
use fake::{Behavior, FakeSmtp};
use fb_plugin_testkit::*;
use mailparse::MailHeaderMap;
use std::{net::SocketAddr, sync::Arc};

fn config(security: Security) -> SmtpConfig {
    SmtpConfig::new(
        "smtp.example.test".into(),
        587,
        security,
        "sender@example.test".into(),
    )
    .unwrap()
}
fn document() -> OutgoingDocument {
    OutgoingDocument {
        document: DocumentRef {
            kind: DocumentKind::Invoice,
            id: Uuid::from_u128(400),
        },
        to: vec![Mailbox {
            address: "recipient@example.test".into(),
            display_name: Some("Synthetic Customer".into()),
        }],
        subject: "Synthetic invoice résumé".into(),
        body_text: SecretString::from("PRIVATE_BODY_SENTINEL\r\n.leading dot\r\nThank you."),
        pdf: SecretBytes::new(b"%PDF-1.7\nSYNTHETIC_PDF_SENTINEL\n%%EOF".to_vec()),
        attachments: vec![],
    }
}
struct Harness {
    cx: PluginContext,
    core: Arc<FakeCoreBridge>,
    audit: Arc<MemoryAuditSink>,
    store: Arc<MemoryCredentialStore>,
}
impl Harness {
    fn new(policy: HttpPolicy, plugin: &str) -> Self {
        let scope = test_scope(plugin);
        let core = Arc::new(FakeCoreBridge::new(scope.clone()));
        let audit = Arc::new(MemoryAuditSink::default());
        let store = Arc::new(MemoryCredentialStore::new());
        let cx = test_context(
            scope,
            PluginHttp::new(policy),
            store.clone(),
            core.clone(),
            audit.clone(),
        );
        Self {
            cx,
            core,
            audit,
            store,
        }
    }
    fn production() -> Self {
        Self::new(HttpPolicy::new(&["smtp.example.test"]).unwrap(), "smtp")
    }
    fn credentials(&self) {
        SmtpSender::save_credentials(
            &self.cx,
            SecretString::from("FAKE_USERNAME"),
            SecretString::from("FAKE_PASSWORD"),
        )
        .unwrap();
    }
    fn event(&self, outcome: AuditOutcome) {
        let events = if outcome == AuditOutcome::Success {
            self.audit
                .document_sends()
                .into_iter()
                .map(|s| s.event)
                .collect()
        } else {
            self.audit.events()
        };
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_name(), "plugin.smtp.send");
        assert_eq!(events[0].outcome, outcome);
        assert_eq!(events[0].purpose, AuditPurpose::DocumentDelivery);
        assert_eq!(events[0].scope, *self.cx.scope());
        assert_eq!(events[0].actor, self.cx.actor());
        for sentinel in [
            "FAKE_USERNAME",
            "FAKE_PASSWORD",
            "PRIVATE_BODY_SENTINEL",
            "SYNTHETIC_PDF_SENTINEL",
            "recipient@example.test",
            "provider-private-id",
            "Synthetic invoice",
        ] {
            assert!(!format!("{events:?}").contains(sentinel));
        }
    }
}
fn fake_sender(server: &FakeSmtp, security: Security) -> (SmtpSender, Harness) {
    let mut sender = SmtpSender::new(config(security));
    sender.test_endpoint = Some(transport::TestEndpoint {
        address: server.address,
        certificate: server.certificate.clone(),
        tls_name: "smtp.example.test".into(),
    });
    let harness = Harness::new(
        HttpPolicy::loopback_for_tests(server.address.port()).unwrap(),
        "smtp",
    );
    harness.credentials();
    (sender, harness)
}
fn failure(
    sender: &SmtpSender,
    harness: &Harness,
    document: OutgoingDocument,
    expected: PluginError,
) {
    match sender.send(&harness.cx, document) {
        Err(error) => assert_eq!(error, expected),
        Ok(_) => panic!("unexpected SMTP success"),
    }
}

#[test]
fn manifest_registers_document_sender_with_explicit_data_and_no_wildcard() {
    MANIFEST.validate().unwrap();
    assert_eq!(MANIFEST.auth, AuthKind::Password);
    assert_eq!(MANIFEST.scopes, &["smtp.send"]);
    assert!(MANIFEST.network_hosts.is_empty());
    assert_eq!(
        MANIFEST.data_sends,
        &[DataKind::DocumentContents, DataKind::RecipientAddresses]
    );
    let mut registration = PluginRegistration::new(&MANIFEST);
    registration.document_sender = Some(Arc::new(SmtpSender::new(config(Security::StartTls))));
    PluginRegistry::new().register(registration).unwrap();
}
fn successful_send(security: Security, behavior: Behavior) {
    let server = FakeSmtp::start(security, behavior);
    let (sender, h) = fake_sender(&server, security);
    let mut doc = document();
    doc.to.push(Mailbox {
        address: "second@example.test".into(),
        display_name: None,
    });
    doc.attachments.push(Attachment {
        filename: "terms.txt".into(),
        media_type: "text/plain".into(),
        bytes: SecretBytes::new(b"Synthetic terms".to_vec()),
    });
    let reference = doc.document;
    h.core.queue_document(doc);
    let rendered = h.cx.core().rendered_document(reference).unwrap();
    let receipt = sender.send(&h.cx, rendered).unwrap();
    assert!(receipt.provider_message_id.is_none());
    assert_eq!(receipt.sent_at, Timestamp(1_700_000_000));
    h.cx.core()
        .record_document_send(reference, receipt)
        .unwrap();
    assert_eq!(
        h.core.calls(),
        vec![
            CoreCall::Render(reference),
            CoreCall::DocumentSent(reference)
        ]
    );
    h.event(AuditOutcome::Success);
    let audit = &h.audit.document_sends()[0];
    assert_eq!(audit.document, reference);
    assert_eq!(audit.receipt, h.core.sent_receipts()[0].1);
    assert!(h.audit.events().is_empty());
    let seen = server.seen.lock().unwrap();
    assert_eq!(seen.auth, 1);
    assert_eq!(seen.encrypted_auth, 1);
    assert!(seen.valid_credentials);
    assert_eq!(seen.starttls, usize::from(security == Security::StartTls));
    assert_eq!(seen.recipients, 2);
    let raw = seen.message.as_ref().unwrap().expose();
    assert!(!String::from_utf8_lossy(raw).contains("FAKE_PASSWORD"));
    let parsed = mailparse::parse_mail(raw).unwrap();
    assert_eq!(
        parsed.headers.get_first_value("Subject").unwrap(),
        "Synthetic invoice résumé"
    );
    assert_eq!(
        parsed.headers.get_first_value("From").unwrap(),
        "sender@example.test"
    );
    assert!(parsed
        .headers
        .get_first_value("To")
        .unwrap()
        .contains("second@example.test"));
    assert_eq!(parsed.subparts.len(), 3);
    assert_eq!(
        parsed.subparts[0].get_body().unwrap(),
        document().body_text.expose_secret()
    );
    assert_eq!(parsed.subparts[1].ctype.mimetype, "application/pdf");
    assert_eq!(
        parsed.subparts[1].get_body_raw().unwrap(),
        document().pdf.expose()
    );
    assert_eq!(
        parsed.subparts[1].get_content_disposition().params["filename"],
        "document.pdf"
    );
    assert_eq!(
        parsed.subparts[2].get_body_raw().unwrap(),
        b"Synthetic terms"
    );
    assert!(h.audit.document_sends()[0].event.bytes_sent > document().pdf.expose().len() as u64);
}
#[test]
fn starttls_sends_mime_and_records_a_secret_free_audit() {
    successful_send(Security::StartTls, Behavior::Accept);
}
#[test]
fn implicit_tls_sends_mime_and_records_a_secret_free_audit() {
    successful_send(Security::ImplicitTls, Behavior::Accept);
}
#[test]
fn login_mechanism_is_supported_but_only_inside_tls() {
    successful_send(Security::StartTls, Behavior::LoginOnly);
}

#[test]
fn starttls_is_mandatory_even_when_plaintext_auth_is_advertised() {
    for behavior in [Behavior::NoStartTls, Behavior::RejectStartTls] {
        let server = FakeSmtp::start(Security::StartTls, behavior);
        let (sender, h) = fake_sender(&server, Security::StartTls);
        failure(
            &sender,
            &h,
            document(),
            if behavior == Behavior::NoStartTls {
                PluginError::TlsRequired
            } else {
                PluginError::Network
            },
        );
        h.event(AuditOutcome::Failed);
        assert_eq!(server.seen.lock().unwrap().auth, 0);
    }
}
#[test]
fn certificate_hostname_mismatch_never_authenticates() {
    for security in [Security::StartTls, Security::ImplicitTls] {
        let server = FakeSmtp::start(security, Behavior::Accept);
        let (mut sender, h) = fake_sender(&server, security);
        sender.test_endpoint.as_mut().unwrap().tls_name = "wrong.example.test".into();
        failure(&sender, &h, document(), PluginError::Network);
        h.event(AuditOutcome::Failed);
        assert_eq!(server.seen.lock().unwrap().auth, 0);
    }
}
#[test]
fn untrusted_certificate_never_authenticates() {
    let server = FakeSmtp::start(Security::ImplicitTls, Behavior::Accept);
    let (mut sender, h) = fake_sender(&server, Security::ImplicitTls);
    let unrelated = rcgen::generate_simple_self_signed(vec!["smtp.example.test".into()]).unwrap();
    sender.test_endpoint.as_mut().unwrap().certificate =
        lettre::transport::smtp::client::Certificate::from_der(unrelated.cert.der().to_vec())
            .unwrap();
    failure(&sender, &h, document(), PluginError::Network);
    assert_eq!(server.seen.lock().unwrap().auth, 0);
}
#[test]
fn auth_failure_is_redacted_and_preserves_stored_credentials() {
    let server = FakeSmtp::start(Security::ImplicitTls, Behavior::RejectAuth);
    let (sender, h) = fake_sender(&server, Security::ImplicitTls);
    failure(&sender, &h, document(), PluginError::NeedsSignIn);
    h.event(AuditOutcome::NeedsSignIn);
    assert!(h.cx.credentials().load().unwrap().is_some());
    assert!(!format!("{:?}", h.store).contains("FAKE_PASSWORD"));
    assert_eq!(server.seen.lock().unwrap().mail, 0);
}
#[test]
fn temporary_auth_failure_does_not_request_new_credentials() {
    let server = FakeSmtp::start(Security::StartTls, Behavior::TemporaryAuth);
    let (sender, h) = fake_sender(&server, Security::StartTls);
    failure(&sender, &h, document(), PluginError::Network);
    h.event(AuditOutcome::Failed);
    assert!(h.cx.credentials().load().unwrap().is_some());
}
#[test]
fn recipient_and_data_rejection_are_not_successes_and_do_not_retry() {
    for behavior in [Behavior::RejectRecipient, Behavior::RejectData] {
        let server = FakeSmtp::start(Security::ImplicitTls, behavior);
        let (sender, h) = fake_sender(&server, Security::ImplicitTls);
        failure(&sender, &h, document(), PluginError::InvalidResponse);
        h.event(AuditOutcome::Failed);
        let seen = server.seen.lock().unwrap();
        assert_eq!(seen.connections, 1);
        assert!(seen.message.is_none());
    }
}
#[test]
fn invalid_and_oversized_smtp_responses_fail_before_auth() {
    for behavior in [Behavior::MalformedGreeting, Behavior::HugeGreeting] {
        let server = FakeSmtp::start(Security::StartTls, behavior);
        let (sender, h) = fake_sender(&server, Security::StartTls);
        failure(&sender, &h, document(), PluginError::InvalidResponse);
        h.event(AuditOutcome::Failed);
        assert_eq!(server.seen.lock().unwrap().auth, 0);
    }
}
#[test]
fn single_deadline_cancels_greeting_auth_data_final_and_slow_drip() {
    for behavior in [
        Behavior::StallGreeting,
        Behavior::StallAuth,
        Behavior::StallData,
        Behavior::StallFinal,
        Behavior::SlowGreeting,
    ] {
        let server = FakeSmtp::start(Security::ImplicitTls, behavior);
        let (mut sender, h) = fake_sender(&server, Security::ImplicitTls);
        sender.config.timeout = Duration::from_millis(160);
        let start = Instant::now();
        failure(&sender, &h, document(), PluginError::Timeout);
        assert!(start.elapsed() < Duration::from_millis(400));
        h.event(AuditOutcome::Failed);
    }
}
#[test]
fn missing_corrupt_and_forgotten_credentials_fail_before_socket_open() {
    for value in [
        None,
        Some(b"wrong-format".to_vec()),
        Some(b"SMTP1\0user\0".to_vec()),
        Some(vec![255]),
        Some(b"SMTP1\0user\0pass\0extra".to_vec()),
    ] {
        let server = FakeSmtp::start(Security::StartTls, Behavior::Accept);
        let (sender, h) = fake_sender(&server, Security::StartTls);
        h.cx.credentials().forget().unwrap();
        if let Some(value) = value {
            h.cx.credentials().save(SecretBytes::new(value)).unwrap();
        }
        failure(&sender, &h, document(), PluginError::NeedsSignIn);
        h.event(AuditOutcome::NeedsSignIn);
        assert_eq!(server.seen.lock().unwrap().connections, 0);
    }
}
#[test]
fn credential_roundtrip_is_scoped_and_rejects_controls() {
    let h = Harness::production();
    h.credentials();
    let (user, pass) = SmtpSender::credentials(&h.cx).unwrap();
    assert!(user.expose_secret() == "FAKE_USERNAME" && pass.expose_secret() == "FAKE_PASSWORD");
    for value in ["", "bad\0password", "bad\r\npassword"] {
        assert_eq!(
            SmtpSender::save_credentials(
                &h.cx,
                SecretString::from("user"),
                SecretString::from(value)
            ),
            Err(PluginError::InvalidConfiguration)
        );
    }
    let other = test_scope("gmail");
    assert!(h.store.load(&other).unwrap().is_none());
}
#[test]
fn wrong_plugin_scope_cannot_send_or_store_credentials() {
    let h = Harness::new(HttpPolicy::new(&[]).unwrap(), "gmail");
    let sender = SmtpSender::new(config(Security::StartTls));
    failure(&sender, &h, document(), PluginError::WrongScope);
    assert_eq!(
        SmtpSender::save_credentials(
            &h.cx,
            SecretString::from("user"),
            SecretString::from("password")
        ),
        Err(PluginError::WrongScope)
    );
    assert!(h.audit.events().is_empty());
    assert!(h.cx.credentials().load().unwrap().is_none());
}
#[test]
fn production_host_requires_consent_before_dns_or_connect() {
    let h = Harness::new(HttpPolicy::new(&[]).unwrap(), "smtp");
    h.credentials();
    failure(
        &SmtpSender::new(config(Security::StartTls)),
        &h,
        document(),
        PluginError::HostNotAllowed,
    );
    h.event(AuditOutcome::Refused);
}
#[test]
fn production_refuses_private_numeric_hosts_even_when_allowlisted() {
    for host in [
        "127.0.0.1",
        "10.0.0.1",
        "169.254.169.254",
        "192.168.1.1",
        "0.0.0.0",
        "localhost",
    ] {
        let h = Harness::new(HttpPolicy::new(&[host]).unwrap(), "smtp");
        h.credentials();
        let config = SmtpConfig::new(
            host.into(),
            587,
            Security::StartTls,
            "sender@example.test".into(),
        )
        .unwrap();
        failure(
            &SmtpSender::new(config),
            &h,
            document(),
            PluginError::PrivateAddress,
        );
        h.event(AuditOutcome::Refused);
    }
}
#[test]
fn resolved_guard_rejects_mixed_public_private_and_ipv6_answers() {
    let policy = HttpPolicy::new(&["smtp.example.test"]).unwrap();
    let good: SocketAddr = "8.8.8.8:587".parse().unwrap();
    for bad in [
        "127.0.0.1:587",
        "10.0.0.1:587",
        "169.254.169.254:587",
        "[::1]:587",
        "[::ffff:127.0.0.1]:587",
        "[fc00::1]:587",
        "8.8.4.4:465",
    ] {
        assert_eq!(
            policy.validate_resolved(
                "https://smtp.example.test:587/",
                &[good, bad.parse().unwrap()]
            ),
            Err(PluginError::PrivateAddress)
        );
    }
    assert_eq!(
        policy.validate_resolved("https://smtp.example.test:587/", &[good]),
        Ok(())
    );
}
#[test]
fn invalid_configuration_has_no_plaintext_escape_hatch() {
    for host in [
        "",
        "https://example.test",
        "user@example.test",
        "smtp.example.test/path",
        "smtp.example.test:587",
        "*.example.test",
        "smtp..example.test",
        "-smtp.test",
        "smtp.test\r\n",
        "[::1]",
    ] {
        assert!(SmtpConfig::new(
            host.into(),
            587,
            Security::StartTls,
            "sender@example.test".into()
        )
        .is_err());
    }
    for port in [0, 25] {
        assert!(SmtpConfig::new(
            "smtp.example.test".into(),
            port,
            Security::StartTls,
            "sender@example.test".into()
        )
        .is_err());
    }
    for (bytes, time) in [
        (0, DEFAULT_TIMEOUT),
        (usize::MAX, DEFAULT_TIMEOUT),
        (1024, Duration::ZERO),
        (1024, Duration::from_secs(121)),
    ] {
        assert!(config(Security::StartTls).with_limits(bytes, time).is_err());
    }
}
#[test]
fn invalid_document_fields_fail_before_network_and_are_audited() {
    let mutations: Vec<fn(&mut OutgoingDocument)> = vec![
        |d| d.to.clear(),
        |d| d.to[0].address = "not a mailbox".into(),
        |d| d.to[0].address = "valid@example.test\r\nBcc: thief@example.test".into(),
        |d| d.to[0].display_name = Some("bad\nname".into()),
        |d| d.subject = "Invoice\r\nBcc: thief@example.test".into(),
        |d| d.pdf = SecretBytes::new(vec![]),
        |d| d.pdf = SecretBytes::new(b"not a pdf".to_vec()),
        |d| d.document.id = Uuid::nil(),
        |d| {
            d.attachments.push(Attachment {
                filename: "../secret.txt".into(),
                media_type: "text/plain".into(),
                bytes: SecretBytes::new(vec![1]),
            })
        },
        |d| {
            d.attachments.push(Attachment {
                filename: "safe.txt".into(),
                media_type: "text/plain\r\nBcc: thief@example.test".into(),
                bytes: SecretBytes::new(vec![1]),
            })
        },
        |d| {
            d.attachments.push(Attachment {
                filename: "safe.txt".into(),
                media_type: "not a media type".into(),
                bytes: SecretBytes::new(vec![1]),
            })
        },
    ];
    for mutate in mutations {
        let h = Harness::production();
        h.credentials();
        let mut doc = document();
        mutate(&mut doc);
        failure(
            &SmtpSender::new(config(Security::StartTls)),
            &h,
            doc,
            PluginError::InvalidConfiguration,
        );
        h.event(AuditOutcome::Refused);
    }
}
#[test]
fn recipient_and_attachment_counts_are_bounded() {
    for attachments in [false, true] {
        let h = Harness::production();
        h.credentials();
        let mut doc = document();
        if attachments {
            for _ in 0..11 {
                doc.attachments.push(Attachment {
                    filename: "safe.txt".into(),
                    media_type: "text/plain".into(),
                    bytes: SecretBytes::new(vec![1]),
                });
            }
        } else {
            for _ in 0..50 {
                doc.to.push(Mailbox {
                    address: "recipient@example.test".into(),
                    display_name: None,
                });
            }
        }
        failure(
            &SmtpSender::new(config(Security::StartTls)),
            &h,
            doc,
            PluginError::InvalidConfiguration,
        );
    }
}
#[test]
fn raw_content_and_encoded_mime_are_both_size_capped() {
    for size in [128, 900] {
        let h = Harness::production();
        h.credentials();
        let sender = SmtpSender::new(
            config(Security::StartTls)
                .with_limits(size, DEFAULT_TIMEOUT)
                .unwrap(),
        );
        let mut doc = document();
        doc.pdf = SecretBytes::new([b"%PDF-".as_slice(), &[b'x'; 500]].concat());
        failure(&sender, &h, doc, PluginError::OutboundTooLarge);
        h.event(AuditOutcome::Refused);
    }
}
#[test]
fn slot_is_usable_when_caller_already_has_a_tokio_runtime() {
    let server = FakeSmtp::start(Security::ImplicitTls, Behavior::Accept);
    let (sender, h) = fake_sender(&server, Security::ImplicitTls);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            assert!(sender.send(&h.cx, document()).is_ok());
        });
}
