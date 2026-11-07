//! The context carries the scoped, host-issued OperationLease cancellation.
//! Keep provider generations/mutexes until wave-4 host-wide coordination exists.
use crate::{tokens, M365Sender, PendingAuthorization};
use fb_plugin_api::*;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {}
struct Pending {
    sender: M365Sender,
    authorization: PendingAuthorization,
}
impl ConnectionLifecycle for M365Sender {
    fn begin(
        &self,
        cx: &PluginContext,
        input: ConnectInput,
    ) -> PluginResult<Box<dyn PendingConnection>> {
        let _: Input = serde_json::from_slice(input.fields.expose())
            .map_err(|_| PluginError::InvalidConfiguration)?;
        Ok(Box::new(Pending {
            sender: self.clone(),
            authorization: self.start_authorization(cx)?,
        }))
    }
    fn disconnect(&self, cx: &PluginContext) -> PluginResult<ConnectionUpdate> {
        M365Sender::disconnect(self, cx)?;
        Ok(ConnectionUpdate {
            scope: cx.scope().clone(),
            status: ConnectionStatus::Disconnected,
            expires_at: None,
            revocation: RevocationStatus::Unknown,
        })
    }
}
impl PendingConnection for Pending {
    fn authorization_url(&self) -> &SecretString {
        self.authorization.authorization_url()
    }
    fn complete(self: Box<Self>, cx: &PluginContext) -> PluginResult<ConnectionUpdate> {
        self.sender.complete_authorization(cx, self.authorization)?;
        Ok(ConnectionUpdate {
            scope: cx.scope().clone(),
            status: ConnectionStatus::Connected,
            expires_at: Some(tokens::connection_expiry(cx)?),
            revocation: RevocationStatus::NotApplicable,
        })
    }
}
