use super::*;

struct BrokenCredentials;
impl CredentialStore for BrokenCredentials {
    fn load(&self, _: &CredentialScope) -> PluginResult<Option<SecretBytes>> {
        Err(PluginError::CredentialStore)
    }
    fn save(&self, _: &CredentialScope, _: SecretBytes) -> PluginResult<()> {
        Err(PluginError::CredentialStore)
    }
    fn forget(&self, _: &CredentialScope) -> PluginResult<()> {
        Err(PluginError::CredentialStore)
    }
}
struct BrokenAudit;
impl AuditSink for BrokenAudit {
    fn record(&self, _: AuditEvent) -> PluginResult<()> {
        Err(PluginError::Audit)
    }
}
fn custom_context(
    h: &Harness,
    policy: HttpPolicy,
    credentials: Arc<dyn CredentialStore>,
    audit: Arc<dyn AuditSink>,
    now: Timestamp,
) -> PluginContext {
    PluginContext::new(ContextServices {
        scope: h.cx.scope().clone(),
        actor: h.cx.actor(),
        credentials: ScopedCredentials::new(h.cx.scope().clone(), credentials),
        audit,
        http: PluginHttp::new(policy),
        clock: Arc::new(FixedClock(now)),
        core: h.core.clone(),
    })
    .unwrap()
}
#[test]
fn credential_store_failure_is_audited_and_never_opens_a_socket() {
    let server = FakeSmtp::start(Security::StartTls, Behavior::Accept);
    let (sender, h) = fake_sender(&server, Security::StartTls);
    let cx = custom_context(
        &h,
        HttpPolicy::loopback_for_tests(server.address.port()).unwrap(),
        Arc::new(BrokenCredentials),
        h.audit.clone(),
        Timestamp(1_700_000_000),
    );
    assert!(matches!(
        sender.send(&cx, document()),
        Err(PluginError::CredentialStore)
    ));
    h.event(AuditOutcome::Failed);
    assert_eq!(server.seen.lock().unwrap().connections, 0);
}
#[test]
fn audit_failure_after_acceptance_is_returned_without_retrying_the_message() {
    let server = FakeSmtp::start(Security::ImplicitTls, Behavior::Accept);
    let (sender, h) = fake_sender(&server, Security::ImplicitTls);
    let cx = custom_context(
        &h,
        HttpPolicy::loopback_for_tests(server.address.port()).unwrap(),
        h.store.clone(),
        Arc::new(BrokenAudit),
        Timestamp(1_700_000_000),
    );
    assert!(matches!(
        sender.send(&cx, document()),
        Err(PluginError::Audit)
    ));
    let seen = server.seen.lock().unwrap();
    assert_eq!(seen.mail, 1);
    assert!(seen.message.is_some());
    // This is an explicitly documented host gap: an audit error is NOT permission to retry.
}
#[test]
fn tls_handshake_has_the_same_total_deadline_in_both_modes() {
    for security in [Security::StartTls, Security::ImplicitTls] {
        let server = FakeSmtp::start(security, Behavior::StallTls);
        let (mut sender, h) = fake_sender(&server, security);
        sender.config.timeout = Duration::from_millis(160);
        failure(&sender, &h, document(), PluginError::Timeout);
        h.event(AuditOutcome::Failed);
        assert_eq!(server.seen.lock().unwrap().auth, 0);
    }
}
#[test]
fn only_auth_235_and_final_data_250_count_as_success() {
    for (behavior, error) in [
        (Behavior::AuthWrongCode, PluginError::NeedsSignIn),
        (Behavior::FinalWrongCode, PluginError::InvalidResponse),
    ] {
        let server = FakeSmtp::start(Security::ImplicitTls, behavior);
        let (sender, h) = fake_sender(&server, Security::ImplicitTls);
        failure(&sender, &h, document(), error);
        assert_eq!(server.seen.lock().unwrap().connections, 1);
        if behavior == Behavior::AuthWrongCode {
            assert_eq!(server.seen.lock().unwrap().mail, 0);
        }
    }
}
#[test]
fn insane_host_clock_is_an_audited_error_not_a_formatter_panic() {
    let h = Harness::production();
    h.credentials();
    for timestamp in [253_402_300_800, u64::MAX] {
        let cx = custom_context(
            &h,
            HttpPolicy::new(&[]).unwrap(),
            h.store.clone(),
            h.audit.clone(),
            Timestamp(timestamp),
        );
        assert!(matches!(
            SmtpSender::new(config(Security::StartTls)).send(&cx, document()),
            Err(PluginError::InvalidConfiguration)
        ));
    }
    assert_eq!(h.audit.events().len(), 2);
}
#[test]
fn test_endpoint_cannot_escape_the_exact_loopback_port() {
    let server = FakeSmtp::start(Security::StartTls, Behavior::Accept);
    let (sender, h) = fake_sender(&server, Security::StartTls);
    let cx = custom_context(
        &h,
        HttpPolicy::loopback_for_tests(if server.address.port() == 1 { 2 } else { 1 }).unwrap(),
        h.store.clone(),
        h.audit.clone(),
        Timestamp(1_700_000_000),
    );
    assert!(matches!(
        sender.send(&cx, document()),
        Err(PluginError::PrivateAddress)
    ));
    assert_eq!(server.seen.lock().unwrap().connections, 0);
}
#[test]
fn expired_deadline_does_not_send_to_the_fake_provider() {
    let server = FakeSmtp::start(Security::ImplicitTls, Behavior::Accept);
    let (mut sender, h) = fake_sender(&server, Security::ImplicitTls);
    sender.config.timeout = Duration::from_nanos(1);
    failure(&sender, &h, document(), PluginError::Timeout);
    h.event(AuditOutcome::Failed);
    assert_eq!(server.seen.lock().unwrap().connections, 0);
}
#[test]
fn credential_store_isolation_includes_company_and_connection() {
    let h = Harness::production();
    h.credentials();
    let mut other_company = h.cx.scope().clone();
    other_company.company_id = Uuid::from_u128(101);
    let mut other_connection = h.cx.scope().clone();
    other_connection.connection_id = Uuid::from_u128(201);
    assert!(h.store.load(&other_company).unwrap().is_none());
    assert!(h.store.load(&other_connection).unwrap().is_none());
}
#[test]
fn body_attachment_and_recipient_metadata_each_count_toward_the_cap() {
    let mutations: Vec<fn(&mut OutgoingDocument)> = vec![
        |d| d.body_text = SecretString::from("x".repeat(5000)),
        |d| d.to[0].display_name = Some("x".repeat(5000)),
        |d| d.subject = "x".repeat(5000),
        |d| {
            d.attachments.push(Attachment {
                filename: "blob.bin".into(),
                media_type: "application/octet-stream".into(),
                bytes: SecretBytes::new(vec![0; 5000]),
            })
        },
    ];
    for mutate in mutations {
        let h = Harness::production();
        h.credentials();
        let mut doc = document();
        mutate(&mut doc);
        let sender = SmtpSender::new(
            config(Security::ImplicitTls)
                .with_limits(4096, DEFAULT_TIMEOUT)
                .unwrap(),
        );
        failure(&sender, &h, doc, PluginError::OutboundTooLarge);
        h.event(AuditOutcome::Refused);
    }
}

#[test]
fn rich_acceptance_survives_local_audit_failure() {
    let server = FakeSmtp::start(Security::ImplicitTls, Behavior::Accept);
    let (sender, h) = fake_sender(&server, Security::ImplicitTls);
    let cx = custom_context(
        &h,
        HttpPolicy::loopback_for_tests(server.address.port()).unwrap(),
        h.store.clone(),
        Arc::new(BrokenAudit),
        Timestamp(1_700_000_000),
    );
    assert!(matches!(
        sender.send_outcome(&cx, document()),
        SendOutcome::Accepted {
            receipt: SendReceipt {
                provider_message_id: None,
                sent_at: Timestamp(1_700_000_000)
            },
            recording_error: Some(PluginError::Audit)
        }
    ));
    assert_eq!(server.seen.lock().unwrap().mail, 1);
}

#[test]
fn rich_outcomes_distinguish_refusal_from_lost_final_data_response() {
    for (behavior, uncertain) in [
        (Behavior::RejectRecipient, false),
        (Behavior::RejectData, false),
        (Behavior::StallFinal, true),
    ] {
        let server = FakeSmtp::start(Security::ImplicitTls, behavior);
        let (mut sender, h) = fake_sender(&server, Security::ImplicitTls);
        sender.config.timeout = Duration::from_millis(160);
        let result = sender.send_outcome(&h.cx, document());
        if uncertain {
            assert!(matches!(
                result,
                SendOutcome::Uncertain {
                    error: PluginError::Timeout
                }
            ));
        } else {
            assert!(matches!(
                result,
                SendOutcome::NotSubmitted {
                    error: PluginError::InvalidResponse
                }
            ));
        }
        assert_eq!(server.seen.lock().unwrap().connections, 1);
    }
}

#[test]
fn cancelled_before_send_is_not_submitted_and_after_data_is_uncertain() {
    let server = FakeSmtp::start(Security::ImplicitTls, Behavior::StallFinal);
    let (sender, h) = fake_sender(&server, Security::ImplicitTls);
    std::thread::scope(|scope| {
        let send = scope.spawn(|| sender.send_outcome(&h.cx, document()));
        let start = Instant::now();
        while server.seen.lock().unwrap().message.is_none() {
            assert!(start.elapsed() < Duration::from_secs(2));
            std::thread::sleep(Duration::from_millis(5));
        }
        h.cx.cancellation().cancel();
        assert!(matches!(
            send.join().unwrap(),
            SendOutcome::Uncertain {
                error: PluginError::Cancelled
            }
        ));
    });
    assert!(matches!(
        sender.send_outcome(&h.cx, document()),
        SendOutcome::NotSubmitted {
            error: PluginError::Cancelled
        }
    ));
    assert_eq!(server.seen.lock().unwrap().connections, 1);
}

#[test]
fn host_deadline_ceiling_is_used_after_pinned_resolution() {
    let server = FakeSmtp::start(Security::ImplicitTls, Behavior::StallGreeting);
    let (sender, h) = fake_sender(&server, Security::ImplicitTls);
    let policy = HttpPolicy::loopback_for_tests(server.address.port())
        .unwrap()
        .with_limits(Duration::from_millis(80), 1_000_000, 1_000_000)
        .unwrap();
    let cx = custom_context(
        &h,
        policy,
        h.store.clone(),
        h.audit.clone(),
        Timestamp(1_700_000_000),
    );
    let start = Instant::now();
    assert!(matches!(
        sender.send_outcome(&cx, document()),
        SendOutcome::NotSubmitted {
            error: PluginError::Timeout
        }
    ));
    assert!(start.elapsed() < Duration::from_millis(400));
}
