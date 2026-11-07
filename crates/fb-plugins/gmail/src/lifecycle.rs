use crate::{GmailAuthorization, GmailSender};
use fb_plugin_api::*;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    from_address: String,
}
struct Pending {
    sender: GmailSender,
    attempt: GmailAuthorization,
}
impl ConnectionLifecycle for GmailSender {
    fn begin(
        &self,
        cx: &PluginContext,
        input: ConnectInput,
    ) -> PluginResult<Box<dyn PendingConnection>> {
        let input: Input = serde_json::from_slice(input.fields.expose())
            .map_err(|_| PluginError::InvalidConfiguration)?;
        Ok(Box::new(Pending {
            sender: self.clone(),
            attempt: self.begin_authorization(cx, input.from_address)?,
        }))
    }
    fn disconnect(&self, cx: &PluginContext) -> PluginResult<ConnectionUpdate> {
        let receipt = GmailSender::disconnect(self, cx)?;
        Ok(ConnectionUpdate {
            scope: cx.scope().clone(),
            status: ConnectionStatus::Disconnected,
            expires_at: None,
            revocation: if receipt.revocation_confirmed {
                RevocationStatus::Confirmed
            } else {
                RevocationStatus::Unknown
            },
        })
    }
}
impl PendingConnection for Pending {
    fn authorization_url(&self) -> &SecretString {
        self.attempt.authorization_url()
    }
    fn complete(self: Box<Self>, cx: &PluginContext) -> PluginResult<ConnectionUpdate> {
        self.sender.complete_authorization(cx, self.attempt)?;
        let tokens = self.sender.load_tokens(cx)?;
        Ok(ConnectionUpdate {
            scope: cx.scope().clone(),
            status: ConnectionStatus::Connected,
            expires_at: Some(tokens.expires_at),
            revocation: RevocationStatus::NotApplicable,
        })
    }
}
