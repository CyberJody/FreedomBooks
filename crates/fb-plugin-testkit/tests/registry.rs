use fb_plugin_api::*;
use fb_plugin_testkit::test_scope;
use std::sync::Arc;

static SENDER: PluginManifest = PluginManifest {
    id: "sender",
    name: "Sender",
    version: "0.1.0",
    slots: &[Slot::DocumentSender],
    auth: AuthKind::Password,
    outside_account: "Mail account",
    scopes: &["send"],
    data_reads: &[DataKind::DocumentContents],
    data_sends: &[DataKind::DocumentContents],
    settings_fields: &[],
    network_hosts: &["mail.example"],
};
static AI: PluginManifest = PluginManifest {
    id: "ai",
    name: "AI",
    version: "0.1.0",
    slots: &[Slot::AiHelper],
    auth: AuthKind::ApiKey,
    outside_account: "AI account",
    scopes: &["read-receipt"],
    data_reads: &[DataKind::ReceiptImages],
    data_sends: &[DataKind::ReceiptImages],
    settings_fields: &[],
    network_hosts: &["ai.example"],
};
struct Sender;
impl Plugin for Sender {
    fn manifest(&self) -> &'static PluginManifest {
        &SENDER
    }
}
impl DocumentSender for Sender {
    fn send(&self, _cx: &PluginContext, _document: OutgoingDocument) -> PluginResult<SendReceipt> {
        Err(PluginError::NeedsSignIn)
    }
}
struct Ai;
impl Plugin for Ai {
    fn manifest(&self) -> &'static PluginManifest {
        &AI
    }
}
impl AiHelper for Ai {
    fn models(&self, _cx: &PluginContext) -> PluginResult<Vec<ModelChoice>> {
        Ok(vec![])
    }
    fn respond(&self, _cx: &PluginContext, _request: AiRequest) -> PluginResult<AiResponse> {
        Err(PluginError::NeedsSignIn)
    }
}
fn sender() -> PluginRegistration {
    let mut registration = PluginRegistration::new(&SENDER);
    registration.document_sender = Some(Arc::new(Sender));
    registration
}
fn state(scope: &CredentialScope, enabled: bool, status: ConnectionStatus) -> CompanyPluginState {
    CompanyPluginState {
        company_id: scope.company_id,
        connection_id: scope.connection_id,
        plugin_id: scope.plugin_id.clone(),
        enabled,
        status,
    }
}
#[test]
fn registry_requires_explicit_company_enable_and_connected_status() {
    let mut registry = PluginRegistry::new();
    registry.register(sender()).unwrap();
    let scope = test_scope("sender");
    assert!(matches!(
        registry.connected(&scope, Slot::DocumentSender),
        Err(PluginError::Disabled)
    ));
    registry
        .set_company_states(vec![state(&scope, false, ConnectionStatus::Connected)])
        .unwrap();
    assert!(matches!(
        registry.connected(&scope, Slot::DocumentSender),
        Err(PluginError::Disabled)
    ));
    registry
        .set_company_states(vec![state(&scope, true, ConnectionStatus::NeedsSignIn)])
        .unwrap();
    assert!(registry
        .enabled_connection(&scope, Slot::DocumentSender)
        .is_ok());
    assert!(matches!(
        registry.connected(&scope, Slot::DocumentSender),
        Err(PluginError::NeedsSignIn)
    ));
    registry
        .set_company_states(vec![state(&scope, true, ConnectionStatus::Connected)])
        .unwrap();
    assert!(registry.connected(&scope, Slot::DocumentSender).is_ok());
    assert!(matches!(
        registry.connected(&scope, Slot::BankFeed),
        Err(PluginError::Disabled)
    ));
    let mut other = scope.clone();
    other.company_id = Uuid::from_u128(777);
    assert!(matches!(
        registry.connected(&other, Slot::DocumentSender),
        Err(PluginError::Disabled)
    ));
    other = scope.clone();
    other.plugin_id = "ai".into();
    assert!(matches!(
        registry.connected(&other, Slot::DocumentSender),
        Err(PluginError::Disabled)
    ));
}
#[test]
fn registry_refuses_duplicates_and_lying_slot_manifests() {
    let mut registry = PluginRegistry::new();
    assert_eq!(
        registry.register(PluginRegistration::new(&SENDER)),
        Err(PluginError::InvalidConfiguration)
    );
    registry.register(sender()).unwrap();
    assert_eq!(
        registry.register(sender()),
        Err(PluginError::DuplicatePlugin)
    );
    let mut wrong = PluginRegistration::new(&AI);
    wrong.document_sender = Some(Arc::new(Sender));
    assert_eq!(
        registry.register(wrong),
        Err(PluginError::InvalidConfiguration)
    );
}
#[test]
fn multiple_senders_allowed_ai_is_singleton_and_state_updates_are_atomic() {
    let mut registry = PluginRegistry::new();
    registry.register(sender()).unwrap();
    let mut ai = PluginRegistration::new(&AI);
    ai.ai_helper = Some(Arc::new(Ai));
    registry.register(ai).unwrap();
    let first_sender = test_scope("sender");
    let mut second_sender = first_sender.clone();
    second_sender.connection_id = Uuid::from_u128(201);
    let mut first_ai = test_scope("ai");
    first_ai.connection_id = Uuid::from_u128(202);
    let mut second_ai = first_ai.clone();
    second_ai.connection_id = Uuid::from_u128(203);
    let good = vec![
        state(&first_sender, true, ConnectionStatus::Connected),
        state(&second_sender, true, ConnectionStatus::Connected),
        state(&first_ai, true, ConnectionStatus::Connected),
    ];
    registry.set_company_states(good.clone()).unwrap();
    let mut bad = good.clone();
    bad.push(state(&second_ai, true, ConnectionStatus::Connected));
    assert_eq!(
        registry.set_company_states(bad),
        Err(PluginError::InvalidConfiguration)
    );
    assert!(registry.connected(&first_ai, Slot::AiHelper).is_ok());
    assert!(matches!(
        registry.connected(&second_ai, Slot::AiHelper),
        Err(PluginError::Disabled)
    ));
    let mut allowed = good.clone();
    allowed.push(state(&second_ai, false, ConnectionStatus::Disconnected));
    registry.set_company_states(allowed).unwrap();
    second_ai.company_id = Uuid::from_u128(999);
    let mut other_company = good;
    other_company.push(state(&second_ai, true, ConnectionStatus::Connected));
    registry.set_company_states(other_company).unwrap();
    assert!(registry.connected(&second_ai, Slot::AiHelper).is_ok());
}
#[test]
fn invalid_manifest_ids_and_duplicate_states_are_refused() {
    let bad = PluginManifest {
        id: "not.an.id",
        ..SENDER
    };
    assert_eq!(bad.validate(), Err(PluginError::InvalidConfiguration));
    let duplicate_slots = PluginManifest {
        slots: &[Slot::DocumentSender, Slot::DocumentSender],
        ..SENDER
    };
    assert_eq!(
        duplicate_slots.validate(),
        Err(PluginError::InvalidConfiguration)
    );
    let mut registry = PluginRegistry::new();
    registry.register(sender()).unwrap();
    let scope = test_scope("sender");
    assert_eq!(
        registry.set_company_states(vec![
            state(&scope, true, ConnectionStatus::Connected),
            state(&scope, false, ConnectionStatus::Disconnected)
        ]),
        Err(PluginError::InvalidConfiguration)
    );
}
