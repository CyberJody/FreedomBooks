use crate::*;

pub trait Plugin: Send + Sync {
    fn manifest(&self) -> &'static PluginManifest;
    fn connection_lifecycle(&self) -> Option<&dyn ConnectionLifecycle> {
        None
    }
}
pub trait DocumentSender: Plugin {
    fn send(&self, cx: &PluginContext, document: OutgoingDocument) -> PluginResult<SendReceipt>;
    /// Legacy errors cannot prove no submission occurred. Override to retain an
    /// accepted receipt even when local audit/history fails; never auto-retry.
    fn send_outcome(&self, cx: &PluginContext, document: OutgoingDocument) -> SendOutcome {
        if let Err(error) = cx.check_active() {
            return SendOutcome::NotSubmitted { error };
        }
        match self.send(cx, document) {
            Ok(receipt) => SendOutcome::Accepted {
                receipt,
                recording_error: None,
            },
            Err(error) => SendOutcome::Uncertain { error },
        }
    }
}
pub trait BankFeed: Plugin {
    fn connect(&self, cx: &PluginContext, input: ConnectInput) -> PluginResult<FeedConnection>;
    fn list_accounts(
        &self,
        cx: &PluginContext,
        connection: &FeedConnection,
    ) -> PluginResult<Vec<FeedAccount>>;
    fn fetch(
        &self,
        cx: &PluginContext,
        connection: &FeedConnection,
        window: DateWindow,
    ) -> PluginResult<Vec<FeedTransaction>>;
    fn forget(&self, cx: &PluginContext, connection: &FeedConnection) -> PluginResult<()>;
    fn list_accounts_result(
        &self,
        cx: &PluginContext,
        connection: &FeedConnection,
    ) -> PluginResult<FeedResult<FeedAccount>> {
        self.list_accounts(cx, connection).map(FeedResult::legacy)
    }
    fn fetch_result(
        &self,
        cx: &PluginContext,
        connection: &FeedConnection,
        window: DateWindow,
    ) -> PluginResult<FeedResult<FeedTransaction>> {
        self.fetch(cx, connection, window).map(FeedResult::legacy)
    }
}
pub trait AiHelper: Plugin {
    fn models(&self, cx: &PluginContext) -> PluginResult<Vec<ModelChoice>>;
    fn respond(&self, cx: &PluginContext, request: AiRequest) -> PluginResult<AiResponse>;
}
pub trait SalesImporter: Plugin {
    fn connect(&self, cx: &PluginContext, input: ConnectInput) -> PluginResult<ImportConnection>;
    fn fetch_orders(
        &self,
        cx: &PluginContext,
        connection: &ImportConnection,
        window: DateWindow,
    ) -> PluginResult<Vec<ImportedOrder>>;
    fn fetch_payouts(
        &self,
        cx: &PluginContext,
        connection: &ImportConnection,
        window: DateWindow,
    ) -> PluginResult<Vec<ImportedPayout>>;
}
pub trait RemoteAccess: Plugin {
    fn status(
        &self,
        cx: &PluginContext,
        request: RemoteStatusRequest,
    ) -> PluginResult<RemoteStatus>;
}
