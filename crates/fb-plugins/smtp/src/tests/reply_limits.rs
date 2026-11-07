use super::{fake::ReplyStage, *};

fn rejects_unterminated(stage: ReplyStage, security: Security) {
    rejects_oversized(stage, security, false);
}

fn rejects_oversized(stage: ReplyStage, security: Security, multiline: bool) {
    let behavior = if multiline {
        Behavior::OversizedMultiline(stage)
    } else {
        Behavior::Oversized(stage)
    };
    let server = FakeSmtp::start(security, behavior);
    let (mut sender, h) = fake_sender(&server, security);
    sender.config.timeout = Duration::from_secs(5);
    let start = Instant::now();
    let result = sender.send(&h.cx, document());
    let elapsed = start.elapsed();
    // Join the fake before inspecting closure, without counting join time as
    // client rejection time. Its read timeout is longer than the client deadline.
    let observed = server.seen.clone();
    drop(server);
    let seen = observed.lock().unwrap();
    eprintln!(
        "{stage:?} {security:?}: rejection={elapsed:?}, peer_closed={}, bytes={}",
        seen.overflow_peer_closed, seen.overflow_bytes
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "oversize reply waited for deadline: {elapsed:?}"
    );
    assert!(
        matches!(result, Err(PluginError::InvalidResponse)),
        "expected a fixed invalid-response error"
    );
    assert!(seen.overflow_peer_closed, "client must close on overflow");
    assert_eq!(seen.overflow_bytes, if multiline { 100_004 } else { 1001 });
    assert_eq!(seen.connections, 1);
    assert_eq!(seen.auth, seen.encrypted_auth);
    if matches!(
        stage,
        ReplyStage::Greeting | ReplyStage::Ehlo | ReplyStage::StartTls | ReplyStage::EncryptedEhlo
    ) {
        assert_eq!(seen.auth, 0);
    }
    h.event(AuditOutcome::Failed);
}

#[test]
fn unterminated_oversized_greeting_is_rejected_early_and_closed() {
    for security in [Security::StartTls, Security::ImplicitTls] {
        rejects_unterminated(ReplyStage::Greeting, security);
    }
}
#[test]
fn unterminated_oversized_ehlo_is_rejected_early_and_closed() {
    rejects_unterminated(ReplyStage::Ehlo, Security::StartTls);
}
#[test]
fn unterminated_oversized_starttls_reply_is_rejected_early_and_closed() {
    rejects_unterminated(ReplyStage::StartTls, Security::StartTls);
}
#[test]
fn unterminated_oversized_post_tls_ehlo_is_rejected_early_and_closed() {
    for security in [Security::StartTls, Security::ImplicitTls] {
        rejects_unterminated(ReplyStage::EncryptedEhlo, security);
    }
}
#[test]
fn unterminated_oversized_auth_is_rejected_early_and_closed() {
    for security in [Security::StartTls, Security::ImplicitTls] {
        rejects_unterminated(ReplyStage::Auth, security);
    }
}
#[test]
fn unterminated_oversized_data_is_rejected_early_and_closed() {
    for security in [Security::StartTls, Security::ImplicitTls] {
        rejects_unterminated(ReplyStage::Data, security);
    }
}
#[test]
fn unterminated_oversized_final_reply_is_rejected_early_and_closed() {
    for security in [Security::StartTls, Security::ImplicitTls] {
        rejects_unterminated(ReplyStage::Final, security);
    }
}

#[test]
fn unterminated_multiline_greeting_obeys_total_response_ceiling() {
    rejects_oversized(ReplyStage::Greeting, Security::StartTls, true);
}
#[test]
fn unterminated_multiline_post_tls_ehlo_obeys_total_response_ceiling() {
    for security in [Security::StartTls, Security::ImplicitTls] {
        rejects_oversized(ReplyStage::EncryptedEhlo, security, true);
    }
}
#[test]
fn unterminated_multiline_final_reply_obeys_total_response_ceiling() {
    for security in [Security::StartTls, Security::ImplicitTls] {
        rejects_oversized(ReplyStage::Final, security, true);
    }
}
