use crate::MemoryCredentialStore;
use fb_plugin_api::*;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

pub struct FixedClock(pub Timestamp);
impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        self.0
    }
}
#[derive(Default)]
pub struct MemoryAuditSink {
    events: Mutex<Vec<AuditEvent>>,
    document_sends: Mutex<Vec<DocumentSendAudit>>,
}
impl MemoryAuditSink {
    pub fn document_sends(&self) -> Vec<DocumentSendAudit> {
        self.document_sends
            .lock()
            .expect("test audit mutex poisoned")
            .clone()
    }
    pub fn events(&self) -> Vec<AuditEvent> {
        self.events
            .lock()
            .expect("test audit mutex poisoned")
            .clone()
    }
}
impl AuditSink for MemoryAuditSink {
    fn record_document_send(&self, event: DocumentSendAudit) -> PluginResult<()> {
        event.validate()?;
        self.document_sends
            .lock()
            .map_err(|_| PluginError::Audit)?
            .push(event);
        Ok(())
    }
    fn record(&self, event: AuditEvent) -> PluginResult<()> {
        self.events
            .lock()
            .map_err(|_| PluginError::Audit)?
            .push(event);
        Ok(())
    }
}
/// Only identifiers and counts are recorded, never document bodies or secret state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CoreCall {
    Render(DocumentRef),
    Stage { count: usize },
    DraftBill { id: Uuid },
    DraftSale { id: Uuid },
    DraftDeposit { id: Uuid },
    Attach { draft: DraftRef, bytes: usize },
    DocumentSent(DocumentRef),
    LoadState,
    SaveState,
    Audit,
}
pub struct FakeCoreBridge {
    scope: CredentialScope,
    calls: Mutex<Vec<CoreCall>>,
    state: Mutex<Option<SecretBytes>>,
    documents: Mutex<Vec<(DocumentRef, OutgoingDocument)>>,
    sent_receipts: Mutex<Vec<(DocumentRef, SendReceipt)>>,
    next: AtomicU64,
    pub audit: Arc<MemoryAuditSink>,
}
impl FakeCoreBridge {
    pub fn new(scope: CredentialScope) -> Self {
        Self {
            scope,
            calls: Mutex::new(Vec::new()),
            state: Mutex::new(None),
            documents: Mutex::new(Vec::new()),
            sent_receipts: Mutex::new(Vec::new()),
            next: AtomicU64::new(1),
            audit: Arc::new(MemoryAuditSink::default()),
        }
    }
    pub fn sent_receipts(&self) -> Vec<(DocumentRef, SendReceipt)> {
        self.sent_receipts
            .lock()
            .expect("test send mutex poisoned")
            .clone()
    }
    pub fn calls(&self) -> Vec<CoreCall> {
        self.calls.lock().expect("test core mutex poisoned").clone()
    }
    pub fn queue_document(&self, document: OutgoingDocument) {
        self.documents
            .lock()
            .expect("test document mutex poisoned")
            .push((document.document, document));
    }
    fn call(&self, call: CoreCall) -> PluginResult<()> {
        self.calls.lock().map_err(|_| PluginError::Core)?.push(call);
        Ok(())
    }
    fn id(&self) -> Uuid {
        Uuid::from_u128(self.next.fetch_add(1, Ordering::Relaxed) as u128)
    }
}
impl CoreBridge for FakeCoreBridge {
    fn scope(&self) -> &CredentialScope {
        &self.scope
    }
    fn rendered_document(&self, document: DocumentRef) -> PluginResult<OutgoingDocument> {
        self.call(CoreCall::Render(document))?;
        let mut docs = self.documents.lock().map_err(|_| PluginError::Core)?;
        let index = docs
            .iter()
            .position(|(id, _)| *id == document)
            .ok_or(PluginError::Core)?;
        Ok(docs.remove(index).1)
    }
    fn stage_bank_transactions(
        &self,
        transactions: Vec<FeedTransaction>,
    ) -> PluginResult<StagedFeed> {
        self.call(CoreCall::Stage {
            count: transactions.len(),
        })?;
        Ok(StagedFeed {
            transaction_ids: transactions.iter().map(|_| self.id()).collect(),
        })
    }
    fn create_draft_bill(&self, _bill: DraftBill) -> PluginResult<DraftRef> {
        let id = self.id();
        self.call(CoreCall::DraftBill { id })?;
        Ok(DraftRef { id })
    }
    fn create_draft_sale(&self, _sale: DraftSale) -> PluginResult<DraftRef> {
        let id = self.id();
        self.call(CoreCall::DraftSale { id })?;
        Ok(DraftRef { id })
    }
    fn create_draft_deposit(&self, _deposit: DraftDeposit) -> PluginResult<DraftRef> {
        let id = self.id();
        self.call(CoreCall::DraftDeposit { id })?;
        Ok(DraftRef { id })
    }
    fn attach_to_draft(&self, draft: DraftRef, attachment: Attachment) -> PluginResult<()> {
        self.call(CoreCall::Attach {
            draft,
            bytes: attachment.bytes.expose().len(),
        })
    }
    fn record_document_send(
        &self,
        document: DocumentRef,
        receipt: SendReceipt,
    ) -> PluginResult<()> {
        self.call(CoreCall::DocumentSent(document))?;
        self.sent_receipts
            .lock()
            .map_err(|_| PluginError::Core)?
            .push((document, receipt));
        Ok(())
    }
    fn load_connection_state(&self) -> PluginResult<Option<SecretBytes>> {
        self.call(CoreCall::LoadState)?;
        Ok(self
            .state
            .lock()
            .map_err(|_| PluginError::Core)?
            .as_ref()
            .map(|value| SecretBytes::new(value.expose().to_vec())))
    }
    fn save_connection_state(&self, state: SecretBytes) -> PluginResult<()> {
        self.call(CoreCall::SaveState)?;
        *self.state.lock().map_err(|_| PluginError::Core)? = Some(state);
        Ok(())
    }
    fn record_audit(&self, event: AuditEvent) -> PluginResult<()> {
        if event.scope != self.scope {
            return Err(PluginError::WrongScope);
        }
        self.call(CoreCall::Audit)?;
        self.audit.record(event)
    }
}

/// Test scope with deterministic UUIDs, not ids supplied by fb-ids or fb-core.
pub fn test_scope(plugin_id: &str) -> CredentialScope {
    CredentialScope {
        plugin_id: plugin_id.to_owned(),
        company_id: Uuid::from_u128(100),
        connection_id: Uuid::from_u128(200),
    }
}
pub fn test_context(
    scope: CredentialScope,
    http: PluginHttp,
    credentials: Arc<MemoryCredentialStore>,
    core: Arc<FakeCoreBridge>,
    audit: Arc<MemoryAuditSink>,
) -> PluginContext {
    PluginContext::new(ContextServices {
        credentials: ScopedCredentials::new(scope.clone(), credentials),
        scope,
        actor: Uuid::from_u128(300),
        audit,
        http,
        clock: Arc::new(FixedClock(Timestamp(1_700_000_000))),
        core,
    })
    .expect("valid test context")
}
