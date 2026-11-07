use crate::{
    check_scope, CredentialScope, PluginError, PluginHttp, PluginResult, ScopedCredentials,
    SecretBytes, SecretString,
};
use serde::{Deserialize, Serialize};
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};
use uuid::Uuid;

/// UTC seconds since the Unix epoch. Inject a Clock; never read wall time in a plugin.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Timestamp(pub u64);
pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;
}
pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        )
    }
}

/// Valid Gregorian accounting date, independent of UTC audit timestamps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Date {
    year: u16,
    month: u8,
    day: u8,
}
impl Date {
    pub fn new(year: u16, month: u8, day: u8) -> PluginResult<Self> {
        let leap =
            year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
        let days = match month {
            2 if leap => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            _ => 0,
        };
        if year == 0 || day == 0 || day > days {
            return Err(PluginError::InvalidResponse);
        }
        Ok(Self { year, month, day })
    }
    pub fn parts(self) -> (u16, u8, u8) {
        (self.year, self.month, self.day)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DateWindow {
    pub start: Date,
    pub end: Date,
}
/// Boundary representation only; core validates currency and bounds, never floats or SQL arithmetic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Amount {
    pub cents: i64,
    pub currency: String,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Quantity {
    pub ten_thousandths: i64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentKind {
    Invoice,
    Estimate,
    SalesReceipt,
    Statement,
    Bill,
    Deposit,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DocumentRef {
    pub kind: DocumentKind,
    pub id: Uuid,
}
pub struct Mailbox {
    pub address: String,
    pub display_name: Option<String>,
}
pub struct Attachment {
    pub filename: String,
    pub media_type: String,
    pub bytes: SecretBytes,
}
pub struct OutgoingDocument {
    pub document: DocumentRef,
    pub to: Vec<Mailbox>,
    pub subject: String,
    pub body_text: SecretString,
    pub pdf: SecretBytes,
    pub attachments: Vec<Attachment>,
}
/// Provider submission acceptance only; never proof of recipient delivery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SendReceipt {
    pub provider_message_id: Option<String>,
    pub sent_at: Timestamp,
}
pub struct ConnectInput {
    pub fields: SecretBytes,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FeedConnection {
    pub id: Uuid,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImportConnection {
    pub id: Uuid,
}
pub struct FeedAccount {
    pub remote_id: String,
    pub name: String,
    pub currency: String,
    pub balance: Option<Amount>,
}
pub struct FeedTransaction {
    pub account_remote_id: String,
    pub remote_id: String,
    pub posted_on: Option<Date>,
    pub transacted_on: Option<Date>,
    pub amount: Amount,
    pub description: String,
    pub pending: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AiPurpose {
    ReadReceipt,
    SuggestCategory,
    ExplainReport,
}
pub struct ModelChoice {
    pub id: String,
    pub label: String,
}
pub enum AiInput {
    Text(SecretString),
    File(Attachment),
}
pub struct AiRequest {
    pub purpose: AiPurpose,
    pub instructions: SecretString,
    pub input: Vec<AiInput>,
    pub text_format: Option<String>,
    pub model: Option<String>,
}
pub struct AiResponse {
    pub output: SecretString,
    pub model: String,
}
pub struct ImportedLine {
    pub description: String,
    pub quantity: Quantity,
    pub unit_price: Amount,
    pub tax_collected: Amount,
}
pub struct ImportedOrder {
    pub remote_id: String,
    pub customer: String,
    pub date: Date,
    pub lines: Vec<ImportedLine>,
    pub fees: Amount,
}
pub struct ImportedPayout {
    pub remote_id: String,
    pub date: Date,
    pub gross: Amount,
    pub fees: Amount,
    pub net: Amount,
}
pub struct DraftBill {
    pub source_remote_id: String,
    pub vendor: String,
    pub date: Date,
    pub lines: Vec<ImportedLine>,
}
pub struct DraftSale {
    pub order: ImportedOrder,
}
pub struct DraftDeposit {
    pub payout: ImportedPayout,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DraftRef {
    pub id: Uuid,
}
pub struct StagedFeed {
    pub transaction_ids: Vec<Uuid>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoteStatusRequest;
pub struct RemoteStatus {
    pub trusted_network_up: bool,
    pub private_addresses: Vec<std::net::IpAddr>,
}

/// Closed codes keep audit metadata free of provider text, email bodies and credentials.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditAction {
    Connect,
    Disconnect,
    Send,
    Fetch,
    ListAccounts,
    Models,
    Respond,
    Stage,
    CreateDraft,
    Attach,
    State,
    Status,
}
impl AuditAction {
    pub fn code(self) -> &'static str {
        match self {
            Self::Connect => "connected",
            Self::Disconnect => "disconnected",
            Self::Send => "send",
            Self::Fetch => "fetch",
            Self::ListAccounts => "list_accounts",
            Self::Models => "models",
            Self::Respond => "respond",
            Self::Stage => "stage",
            Self::CreateDraft => "create_draft",
            Self::Attach => "attach",
            Self::State => "state",
            Self::Status => "status",
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditPurpose {
    Connection,
    DocumentDelivery,
    BankSync,
    ReceiptReading,
    CategorySuggestion,
    ReportExplanation,
    SalesSync,
    NetworkStatus,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditOutcome {
    Success,
    Refused,
    NeedsSignIn,
    Failed,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditEvent {
    pub scope: CredentialScope,
    pub actor: Uuid,
    pub at: Timestamp,
    pub action: AuditAction,
    pub purpose: AuditPurpose,
    pub outcome: AuditOutcome,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub duration: Duration,
}
impl AuditEvent {
    pub fn event_name(&self) -> String {
        format!("plugin.{}.{}", self.scope.plugin_id, self.action.code())
    }
}
pub trait AuditSink: Send + Sync {
    fn record(&self, event: AuditEvent) -> PluginResult<()>;
    /// Fail closed when the host has not implemented typed send auditing.
    fn record_document_send(&self, _event: crate::DocumentSendAudit) -> PluginResult<()> {
        Err(PluginError::Audit)
    }
}

/// Host implements this *scoped* draft/read-only surface in wave 4.
/// All mutations use normal core services, enforce dedupe/caps/consent, and recheck
/// the book-unlocked lease. No post, journal, SQL, or ledger account access exists here.
pub trait CoreBridge: Send + Sync {
    fn scope(&self) -> &CredentialScope;
    fn rendered_document(&self, document: DocumentRef) -> PluginResult<OutgoingDocument>;
    fn stage_bank_transactions(
        &self,
        transactions: Vec<FeedTransaction>,
    ) -> PluginResult<StagedFeed>;
    fn create_draft_bill(&self, bill: DraftBill) -> PluginResult<DraftRef>;
    fn create_draft_sale(&self, sale: DraftSale) -> PluginResult<DraftRef>;
    fn create_draft_deposit(&self, deposit: DraftDeposit) -> PluginResult<DraftRef>;
    fn attach_to_draft(&self, draft: DraftRef, attachment: Attachment) -> PluginResult<()>;
    fn record_document_send(&self, document: DocumentRef, receipt: SendReceipt)
        -> PluginResult<()>;
    fn load_connection_state(&self) -> PluginResult<Option<SecretBytes>>;
    /// Noncredential cursors/preferences only. Tokens and access URLs belong in credentials.
    fn save_connection_state(&self, state: SecretBytes) -> PluginResult<()>;
    fn record_audit(&self, event: AuditEvent) -> PluginResult<()>;
}

/// Construct only while the host owns an unlocked-book lease and an enabled connection.
pub struct ContextServices {
    pub scope: CredentialScope,
    pub actor: Uuid,
    pub credentials: ScopedCredentials,
    pub audit: Arc<dyn AuditSink>,
    pub http: PluginHttp,
    pub clock: Arc<dyn Clock>,
    pub core: Arc<dyn CoreBridge>,
}
pub struct PluginContext {
    services: ContextServices,
    cancellation: crate::CancellationToken,
}
impl PluginContext {
    pub fn new(services: ContextServices) -> PluginResult<Self> {
        let lease = crate::OperationLease::new(services.scope.clone());
        Self::new_cancellable(services, lease)
    }
    /// Host owns the matching book/connection lease. Cancellation is cooperative,
    /// not a substitute for transactional lease checks inside CoreBridge/store.
    pub fn new_cancellable(
        mut services: ContextServices,
        lease: crate::OperationLease,
    ) -> PluginResult<Self> {
        check_scope(&services.scope, lease.scope())?;
        check_scope(&services.scope, services.credentials.scope())?;
        check_scope(&services.scope, services.core.scope())?;
        if services.scope.plugin_id.is_empty()
            || services.scope.company_id.is_nil()
            || services.scope.connection_id.is_nil()
            || services.actor.is_nil()
        {
            return Err(PluginError::InvalidConfiguration);
        }
        lease.check()?;
        services.http = services
            .http
            .with_cancellation(lease.cancellation().clone());
        services.credentials = services
            .credentials
            .with_cancellation(lease.cancellation().clone());
        Ok(Self {
            services,
            cancellation: lease.cancellation().clone(),
        })
    }
    pub fn check_active(&self) -> PluginResult<()> {
        self.cancellation.check()
    }
    pub fn cancellation(&self) -> &crate::CancellationToken {
        &self.cancellation
    }
    pub fn record_document_send_audit(&self, event: crate::DocumentSendAudit) -> PluginResult<()> {
        check_scope(self.scope(), &event.event.scope)?;
        if event.event.actor != self.actor() {
            return Err(PluginError::WrongScope);
        }
        event.validate()?;
        self.services.audit.record_document_send(event)
    }
    pub fn scope(&self) -> &CredentialScope {
        &self.services.scope
    }
    pub fn company_id(&self) -> Uuid {
        self.scope().company_id
    }
    pub fn connection_id(&self) -> Uuid {
        self.scope().connection_id
    }
    pub fn actor(&self) -> Uuid {
        self.services.actor
    }
    pub fn credentials(&self) -> &ScopedCredentials {
        &self.services.credentials
    }
    pub fn http(&self) -> &PluginHttp {
        &self.services.http
    }
    pub fn clock(&self) -> &dyn Clock {
        self.services.clock.as_ref()
    }
    pub fn core(&self) -> &dyn CoreBridge {
        self.services.core.as_ref()
    }
    pub fn record_audit(
        &self,
        action: AuditAction,
        purpose: AuditPurpose,
        outcome: AuditOutcome,
        bytes: (u64, u64),
        duration: Duration,
    ) -> PluginResult<()> {
        self.services.audit.record(AuditEvent {
            scope: self.scope().clone(),
            actor: self.actor(),
            at: self.clock().now(),
            action,
            purpose,
            outcome,
            bytes_sent: bytes.0,
            bytes_received: bytes.1,
            duration,
        })
    }
}
