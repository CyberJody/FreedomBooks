use super::*;

fn compose(h: &Harness) {
    h.seed(NOW + 3600);
    let mut tokens = h.tokens();
    tokens.scope = COMPOSE_SCOPE;
    h.cx.credentials().save(tokens.encode().unwrap()).unwrap();
}
fn rich() -> DeliveryContent {
    DeliveryContent {
        html: Some(SecretString::from(
            "<p>PRIVATE_HTML_SENTINEL — café</p><img src=\"cid:freedombooks-logo\">",
        )),
        reply_to: Some(fb_plugin_api::Mailbox {
            address: "reply@example.test".into(),
            display_name: None,
        }),
        inline_images: vec![],
        inline_logo: Some(Attachment {
            filename: "logo.png".into(),
            media_type: "image/png".into(),
            bytes: SecretBytes::new(b"SYNTHETIC_LOGO".to_vec()),
        }),
        message_id: Some("fb-synthetic-attempt@freedombooks.local".into()),
    }
}

#[test]
fn drafts_and_sends_are_distinct_single_submissions_and_history_remains_host_owned() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    let server = FakeServer::start(move |request| {
        captured.lock().unwrap().push(request.target.clone());
        let value: serde_json::Value = serde_json::from_slice(request.body.expose()).unwrap();
        let (raw, response) = if request.target == "/drafts" {
            assert!(value.get("raw").is_none());
            (
                value["message"]["raw"].as_str().unwrap(),
                r#"{"id":"draft123","message":{"id":"message123"}}"#,
            )
        } else {
            assert_eq!(request.target, "/send");
            (value["raw"].as_str().unwrap(), r#"{"id":"sent123"}"#)
        };
        let mime = String::from_utf8(URL_SAFE_NO_PAD.decode(raw).unwrap()).unwrap();
        assert!(mime.contains("multipart/alternative") && mime.contains("multipart/related"));
        assert!(mime.contains("Reply-To: reply@example.test"));
        assert!(mime.contains("Message-ID: <fb-synthetic-attempt@freedombooks.local>"));
        assert!(mime.contains("Content-ID: <freedombooks-logo>"));
        assert!(mime.contains(&STANDARD.encode(PDF)));
        FakeResponse::new(200, response)
    })
    .unwrap();
    let h = Harness::new(server.http());
    compose(&h);
    let s = sender(&server).with_drafts();
    assert_eq!(s.manifest().scopes, &[COMPOSE_SCOPE]);
    let draft = s.deliver_outcome(&h.cx, document(), rich(), DeliveryMode::Draft);
    assert!(
        matches!(draft,DeliveryOutcome::Accepted{receipt:DeliveryReceipt{draft_id:Some(id),message_id,..},recording_error:None} if id=="draft123" && message_id=="message123")
    );
    assert!(h.core.sent_receipts().is_empty());
    assert_eq!(h.audit.events()[0].action, AuditAction::CreateDraft);
    let sent = s.deliver_outcome(&h.cx, document(), rich(), DeliveryMode::Send);
    assert!(
        matches!(sent,DeliveryOutcome::Accepted{receipt:DeliveryReceipt{draft_id:None,message_id,..},recording_error:None} if message_id=="sent123")
    );
    assert_eq!(*requests.lock().unwrap(), vec!["/drafts", "/send"]);
    assert!(
        h.core.sent_receipts().is_empty(),
        "desktop host records the durable attempt atomically"
    );
    h.no_secrets();
}

#[test]
fn legacy_send_only_tokens_cannot_save_drafts_or_be_silently_upgraded() {
    let server = ScriptedServer::start(vec![]).unwrap();
    let h = Harness::new(server.server().http());
    h.seed(NOW + 3600);
    for s in [
        sender(server.server()),
        sender(server.server()).with_drafts(),
    ] {
        assert!(matches!(
            s.deliver_outcome(&h.cx, document(), rich(), DeliveryMode::Draft),
            DeliveryOutcome::NotSubmitted {
                error: PluginError::NeedsSignIn
            }
        ));
    }
    assert_eq!(h.tokens().scope, SEND_SCOPE);
    server.assert_finished();
    let old=SecretBytes::new(serde_json::to_vec(&serde_json::json!({"version":1,"client_id":CLIENT,
        "from_address":"sender@example.test","access_token":ACCESS,"refresh_token":REFRESH,"expires_at":NOW+3600})).unwrap());
    assert_eq!(Tokens::decode(&old).unwrap().scope, SEND_SCOPE);
}

#[test]
fn uncertain_submissions_never_retry_or_claim_a_draft_was_saved() {
    for (status, body, uncertain) in [
        (503, "{}", true),
        (200, r#"{"id":"draft123"}"#, true),
        (403, "{}", false),
        (429, "{}", false),
    ] {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let server = FakeServer::start(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            FakeResponse::new(status, body)
        })
        .unwrap();
        let h = Harness::new(server.http());
        compose(&h);
        let result = sender(&server).with_drafts().deliver_outcome(
            &h.cx,
            document(),
            rich(),
            DeliveryMode::Draft,
        );
        assert_eq!(
            matches!(result, DeliveryOutcome::Uncertain { .. }),
            uncertain
        );
        assert!(!matches!(result, DeliveryOutcome::Accepted { .. }));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        h.no_secrets();
    }
}

#[test]
fn rich_headers_and_content_limits_are_validated_before_network_access() {
    let server = ScriptedServer::start(vec![]).unwrap();
    let h = Harness::new(server.server().http());
    compose(&h);
    let s = sender(server.server()).with_drafts();
    for content in [
        DeliveryContent {
            message_id: Some("bad\r\nBcc: hidden@example.test".into()),
            ..rich()
        },
        DeliveryContent {
            reply_to: Some(fb_plugin_api::Mailbox {
                address: "bad\r\n@example.test".into(),
                display_name: None,
            }),
            ..rich()
        },
        DeliveryContent {
            html: None,
            ..rich()
        },
        DeliveryContent {
            html: Some(SecretString::from("x".repeat(mime::MAX_CONTENT_BYTES))),
            ..rich()
        },
    ] {
        assert!(matches!(
            s.deliver_outcome(&h.cx, document(), content, DeliveryMode::Draft),
            DeliveryOutcome::NotSubmitted { .. }
        ));
    }
    server.assert_finished();
}

#[test]
fn rich_mime_is_readable_by_an_independent_email_parser() {
    use std::io::Write;
    let raw = mime::build_with_content("sender@example.test", &document(), &rich(), Timestamp(NOW))
        .unwrap();
    let mut child = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/verify_invoice_mime.py"
        ))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(raw.expose()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn inline_payment_images_are_related_and_identifiers_are_bounded() {
    use crate::InlineImage;
    let image=|id:&str,bytes:usize|InlineImage {content_id:id.into(),file:Attachment {filename:"payment.png".into(),media_type:"image/png".into(),bytes:SecretBytes::new(vec![1;bytes])}};
    let mut content=rich();content.inline_images=vec![image("freedombooks-pay-zelle",10),image("freedombooks-pay-visa",10)];
    let raw=mime::build_with_content("sender@example.test",&document(),&content,Timestamp(NOW)).unwrap();
    let text=String::from_utf8(raw.expose().to_vec()).unwrap();assert!(text.contains("Content-ID: <freedombooks-pay-zelle>"));assert!(text.contains("Content-ID: <freedombooks-pay-visa>"));assert!(text.contains("multipart/related"));
    for images in [vec![image("bad\r\nBcc-injected",10)],vec![image("freedombooks-logo",10)],vec![image("duplicate",10),image("duplicate",10)],vec![image("oversized",128*1024+1)],(0..9).map(|i|image(&format!("image-{i}"),1)).collect()] {
        let mut content=rich();content.inline_images=images;
        assert!(mime::build_with_content("sender@example.test",&document(),&content,Timestamp(NOW)).is_err());
    }
}
