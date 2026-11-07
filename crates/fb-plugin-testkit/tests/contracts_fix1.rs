use fb_plugin_api::*;
use fb_plugin_testkit::*;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

static MANIFEST: PluginManifest = PluginManifest {
    id: "sender",
    name: "Synthetic sender",
    version: "0.1.0",
    slots: &[Slot::DocumentSender],
    auth: AuthKind::OAuthLoopback {
        provider: "synthetic",
    },
    outside_account: "Synthetic account",
    scopes: &["send"],
    data_reads: &[DataKind::DocumentContents],
    data_sends: &[DataKind::DocumentContents],
    settings_fields: &[],
    network_hosts: &["provider.example"],
};
fn services(scope: CredentialScope, audit: Arc<dyn AuditSink>) -> ContextServices {
    ContextServices {
        credentials: ScopedCredentials::new(scope.clone(), Arc::new(MemoryCredentialStore::new())),
        core: Arc::new(FakeCoreBridge::new(scope.clone())),
        scope,
        actor: Uuid::from_u128(300),
        audit,
        http: PluginHttp::new(HttpPolicy::new(&["provider.example"]).unwrap()),
        clock: Arc::new(FixedClock(Timestamp(123))),
    }
}
fn document() -> OutgoingDocument {
    OutgoingDocument {
        document: DocumentRef {
            kind: DocumentKind::Invoice,
            id: Uuid::from_u128(400),
        },
        to: vec![],
        subject: "synthetic".into(),
        body_text: SecretString::from("not audited"),
        pdf: SecretBytes::new(vec![]),
        attachments: vec![],
    }
}
fn receipt() -> SendReceipt {
    SendReceipt {
        provider_message_id: Some("synthetic-message-42".into()),
        sent_at: Timestamp(123),
    }
}
fn audit(cx: &PluginContext) -> DocumentSendAudit {
    DocumentSendAudit {
        event: AuditEvent {
            scope: cx.scope().clone(),
            actor: cx.actor(),
            at: cx.clock().now(),
            action: AuditAction::Send,
            purpose: AuditPurpose::DocumentDelivery,
            outcome: AuditOutcome::Success,
            bytes_sent: 100,
            bytes_received: 10,
            duration: Duration::from_millis(1),
        },
        document: document().document,
        receipt: receipt(),
    }
}
#[test]
fn typed_send_audit_and_fake_bridge_preserve_exact_document_receipt_scope() {
    let sink = Arc::new(MemoryAuditSink::default());
    let scope = test_scope("sender");
    let core = Arc::new(FakeCoreBridge::new(scope.clone()));
    let mut svc = services(scope, sink.clone());
    svc.core = core.clone();
    let cx = PluginContext::new(svc).unwrap();
    let event = audit(&cx);
    cx.record_document_send_audit(event.clone()).unwrap();
    assert_eq!(sink.document_sends(), std::slice::from_ref(&event));
    core.record_document_send(event.document, event.receipt.clone())
        .unwrap();
    assert_eq!(core.sent_receipts(), [(event.document, event.receipt)]);
    let mut wrong = audit(&cx);
    wrong.event.scope.connection_id = Uuid::from_u128(999);
    assert_eq!(
        cx.record_document_send_audit(wrong),
        Err(PluginError::WrongScope)
    );
    let mut wrong = audit(&cx);
    wrong.event.actor = Uuid::from_u128(999);
    assert_eq!(
        cx.record_document_send_audit(wrong),
        Err(PluginError::WrongScope)
    );
    let mut malformed = audit(&cx);
    malformed.receipt.provider_message_id = Some("id\r\nsecret".into());
    assert!(cx.record_document_send_audit(malformed).is_err());
    let mut malformed = audit(&cx);
    malformed.event.action = AuditAction::Fetch;
    assert!(cx.record_document_send_audit(malformed).is_err());
    assert_eq!(sink.document_sends().len(), 1);
}
struct OldSink;
impl AuditSink for OldSink {
    fn record(&self, _: AuditEvent) -> PluginResult<()> {
        Ok(())
    }
}
#[test]
fn legacy_audit_sink_refuses_instead_of_silently_dropping_metadata() {
    let cx = PluginContext::new(services(test_scope("sender"), Arc::new(OldSink))).unwrap();
    assert_eq!(
        cx.record_document_send_audit(audit(&cx)),
        Err(PluginError::Audit)
    );
}
struct LegacySender(AtomicUsize);
impl Plugin for LegacySender {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }
}
impl DocumentSender for LegacySender {
    fn send(&self, _: &PluginContext, _: OutgoingDocument) -> PluginResult<SendReceipt> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(PluginError::Audit)
    }
}
#[test]
fn legacy_sender_errors_are_uncertain_and_not_retried() {
    let cx = PluginContext::new(services(test_scope("sender"), Arc::new(OldSink))).unwrap();
    let sender: Box<dyn DocumentSender> = Box::new(LegacySender(AtomicUsize::new(0)));
    assert_eq!(
        sender.send_outcome(&cx, document()),
        SendOutcome::Uncertain {
            error: PluginError::Audit
        }
    );
    let sender = LegacySender(AtomicUsize::new(0));
    assert!(matches!(
        sender.send_outcome(&cx, document()),
        SendOutcome::Uncertain { .. }
    ));
    assert_eq!(sender.0.load(Ordering::SeqCst), 1);
}
struct RichSender;
impl Plugin for RichSender {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }
}
impl DocumentSender for RichSender {
    fn send(&self, _: &PluginContext, _: OutgoingDocument) -> PluginResult<SendReceipt> {
        Err(PluginError::DeliveryUnknown)
    }
    fn send_outcome(&self, cx: &PluginContext, _: OutgoingDocument) -> SendOutcome {
        // Model the provider commit point, then an unsupported/failing local sink.
        let receipt = receipt();
        let recording_error = cx.record_document_send_audit(audit(cx)).err();
        SendOutcome::Accepted {
            receipt,
            recording_error,
        }
    }
}
#[test]
fn accepted_receipt_survives_local_recording_failure_without_claiming_delivery() {
    let cx = PluginContext::new(services(test_scope("sender"), Arc::new(OldSink))).unwrap();
    let sender: &dyn DocumentSender = &RichSender;
    assert_eq!(
        sender.send_outcome(&cx, document()),
        SendOutcome::Accepted {
            receipt: receipt(),
            recording_error: Some(PluginError::Audit)
        }
    );
    assert!(!PluginError::Timeout
        .to_string()
        .contains("Please try again"));
}
#[test]
fn cancellation_gates_credentials_http_and_legacy_send_without_affecting_new_lease() {
    let scope = test_scope("sender");
    let lease = OperationLease::new(scope.clone());
    let cx =
        PluginContext::new_cancellable(services(scope.clone(), Arc::new(OldSink)), lease.clone())
            .unwrap();
    cx.credentials().save(SecretBytes::new(vec![1])).unwrap();
    let retained = cx.credentials().clone();
    let http = cx.http().constrained(Duration::from_secs(1), 1, 1).unwrap();
    lease.cancel();
    assert_eq!(cx.check_active(), Err(PluginError::Cancelled));
    assert!(matches!(retained.load(), Err(PluginError::Cancelled)));
    assert_eq!(
        retained.save(SecretBytes::new(vec![])),
        Err(PluginError::Cancelled)
    );
    assert_eq!(retained.forget(), Err(PluginError::Cancelled));
    assert!(matches!(
        http.execute(HttpRequest {
            method: HttpMethod::Get,
            url: SecretString::from("https://provider.example"),
            headers: vec![],
            body: SecretBytes::new(vec![])
        }),
        Err(PluginError::Cancelled)
    ));
    let sender = LegacySender(AtomicUsize::new(0));
    assert_eq!(
        sender.send_outcome(&cx, document()),
        SendOutcome::NotSubmitted {
            error: PluginError::Cancelled
        }
    );
    assert_eq!(sender.0.load(Ordering::SeqCst), 0);
    assert!(
        PluginContext::new_cancellable(services(scope.clone(), Arc::new(OldSink)), lease).is_err()
    );
    let new_lease = OperationLease::new(scope.clone());
    assert!(PluginContext::new_cancellable(services(scope, Arc::new(OldSink)), new_lease).is_ok());
    assert!(PluginContext::new_cancellable(
        services(test_scope("sender"), Arc::new(OldSink)),
        OperationLease::new(test_scope("other"))
    )
    .is_err());
    // Audit remains possible after cancellation to record the cancelled attempt.
    assert_eq!(
        cx.record_audit(
            AuditAction::Send,
            AuditPurpose::DocumentDelivery,
            AuditOutcome::Failed,
            (0, 0),
            Duration::ZERO
        ),
        Ok(())
    );
}
struct Lifecycle;
struct Pending {
    url: SecretString,
    scope: CredentialScope,
    cancellation: CancellationToken,
}
impl PendingConnection for Pending {
    fn authorization_url(&self) -> &SecretString {
        &self.url
    }
    fn complete(self: Box<Self>, cx: &PluginContext) -> PluginResult<ConnectionUpdate> {
        self.cancellation.check()?;
        let update = ConnectionUpdate {
            scope: self.scope,
            status: ConnectionStatus::Connected,
            expires_at: Some(Timestamp(456)),
            revocation: RevocationStatus::NotApplicable,
        };
        update.validate_for(cx)?;
        Ok(update)
    }
}
impl ConnectionLifecycle for Lifecycle {
    fn begin(
        &self,
        cx: &PluginContext,
        _: ConnectInput,
    ) -> PluginResult<Box<dyn PendingConnection>> {
        cx.check_active()?;
        Ok(Box::new(Pending {
            url: SecretString::from("https://provider.example/authorize"),
            scope: cx.scope().clone(),
            cancellation: cx.cancellation().clone(),
        }))
    }
    fn disconnect(&self, cx: &PluginContext) -> PluginResult<ConnectionUpdate> {
        cx.credentials().forget()?;
        Ok(ConnectionUpdate {
            scope: cx.scope().clone(),
            status: ConnectionStatus::Disconnected,
            expires_at: None,
            revocation: RevocationStatus::Unknown,
        })
    }
}
impl Plugin for Lifecycle {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }
    fn connection_lifecycle(&self) -> Option<&dyn ConnectionLifecycle> {
        Some(self)
    }
}
#[test]
fn object_safe_lifecycle_retains_unknown_revocation_and_refuses_stale_attempt() {
    let cx = PluginContext::new(services(test_scope("sender"), Arc::new(OldSink))).unwrap();
    let plugin: &dyn Plugin = &Lifecycle;
    let lifecycle = plugin.connection_lifecycle().unwrap();
    let start = || {
        lifecycle
            .begin(
                &cx,
                ConnectInput {
                    fields: SecretBytes::new(vec![]),
                },
            )
            .unwrap()
    };
    let pending = start();
    assert!(pending
        .authorization_url()
        .expose_secret()
        .starts_with("https://"));
    let update = pending.complete(&cx).unwrap();
    assert_eq!(update.status, ConnectionStatus::Connected);
    assert_eq!(update.expires_at, Some(Timestamp(456)));
    let wrong = PluginContext::new(services(test_scope("other"), Arc::new(OldSink))).unwrap();
    assert!(matches!(
        start().complete(&wrong),
        Err(PluginError::WrongScope)
    ));
    assert_eq!(
        lifecycle.disconnect(&cx).unwrap().revocation,
        RevocationStatus::Unknown
    );
    let pending = start();
    cx.cancellation().cancel();
    assert!(matches!(pending.complete(&cx), Err(PluginError::Cancelled)));
    // The compatibility constructor uses the same token for all scoped handles.
    assert!(matches!(
        cx.credentials().load(),
        Err(PluginError::Cancelled)
    ));
    assert!(matches!(
        cx.http()
            .resolve_endpoint("provider.example", 443, Duration::from_secs(1)),
        Err(PluginError::Cancelled)
    ));
}
#[test]
fn expired_and_revoked_statuses_cannot_acquire_connected_slot() {
    let scope = test_scope("sender");
    let mut registry = PluginRegistry::new();
    let mut registration = PluginRegistration::new(&MANIFEST);
    registration.document_sender = Some(Arc::new(RichSender));
    registry.register(registration).unwrap();
    for status in [
        ConnectionStatus::Connected,
        ConnectionStatus::Expired,
        ConnectionStatus::Revoked,
        ConnectionStatus::Disconnected,
    ] {
        registry
            .set_company_states(vec![CompanyPluginState {
                company_id: scope.company_id,
                connection_id: scope.connection_id,
                plugin_id: scope.plugin_id.clone(),
                enabled: true,
                status,
            }])
            .unwrap();
        assert_eq!(
            registry.connected(&scope, Slot::DocumentSender).is_ok(),
            status == ConnectionStatus::Connected
        );
        assert!(registry
            .enabled_connection(&scope, Slot::DocumentSender)
            .is_ok());
    }
}
#[test]
fn partial_feed_retains_items_warnings_and_inventory_is_not_assumed() {
    let mut result = FeedResult::legacy(vec!["synthetic transaction"]);
    assert_eq!(result.completeness, FeedCompleteness::Unknown);
    assert!(!result.has_complete_inventory());
    result.completeness = FeedCompleteness::Partial;
    result.present_account_ids = Some(vec!["account-a".into()]);
    result.warnings.push(FeedWarning {
        code: FeedWarningCode::InvalidAmount,
        account_remote_id: Some("account-b".into()),
        transaction_remote_id: Some("skipped-tx".into()),
        connection_remote_id: None,
        source_index: Some(4),
    });
    assert_eq!(result.items, ["synthetic transaction"]);
    assert_eq!(result.warnings[0].source_index, Some(4));
    assert!(!result.has_complete_inventory());
    result.completeness = FeedCompleteness::Complete;
    assert!(!result.has_complete_inventory());
    result.warnings.clear();
    assert!(result.has_complete_inventory());
    result.present_account_ids = None;
    assert!(!result.has_complete_inventory());
    result.present_account_ids = Some(vec![]);
    assert!(result.has_complete_inventory());
}
#[test]
fn new_error_codes_have_fixed_messages_not_provider_text() {
    for error in [
        PluginError::UsageLimit,
        PluginError::SetupTokenRefused,
        PluginError::ProviderSubscriptionRequired,
        PluginError::TlsRequired,
        PluginError::CertificateRejected,
        PluginError::OutboundTooLarge,
        PluginError::DeliveryUnknown,
        PluginError::Cancelled,
    ] {
        assert!(!error.to_string().contains("https://"));
        assert!(!error.to_string().contains("Please try again"));
    }
}
