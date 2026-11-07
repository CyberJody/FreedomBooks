//! Object-safe OAuth adapter; the API-key entry point remains explicit.
use crate::{auth::Record, OpenAi, PendingAuthorization};
use fb_plugin_api::*;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OAuthInput {}

struct Pending {
    plugin: OpenAi,
    authorization: PendingAuthorization,
}
impl ConnectionLifecycle for OpenAi {
    fn begin(
        &self,
        cx: &PluginContext,
        input: ConnectInput,
    ) -> PluginResult<Box<dyn PendingConnection>> {
        let _: OAuthInput = serde_json::from_slice(input.fields.expose())
            .map_err(|_| PluginError::InvalidConfiguration)?;
        Ok(Box::new(Pending {
            plugin: self.clone(),
            authorization: self.start_authorization(cx)?,
        }))
    }
    fn disconnect(&self, cx: &PluginContext) -> PluginResult<ConnectionUpdate> {
        let revocation = match OpenAi::disconnect(self, cx)? {
            None => RevocationStatus::NotApplicable,
            Some(true) => RevocationStatus::Confirmed,
            Some(false) => RevocationStatus::Unknown,
        };
        Ok(ConnectionUpdate {
            scope: cx.scope().clone(),
            status: ConnectionStatus::Disconnected,
            expires_at: None,
            revocation,
        })
    }
}
impl PendingConnection for Pending {
    fn authorization_url(&self) -> &SecretString {
        self.authorization.authorization_url()
    }
    fn complete(self: Box<Self>, cx: &PluginContext) -> PluginResult<ConnectionUpdate> {
        self.plugin.complete_authorization(cx, self.authorization)?;
        let record = Record::load(cx)?;
        Ok(ConnectionUpdate {
            scope: cx.scope().clone(),
            status: ConnectionStatus::Connected,
            expires_at: Some(Timestamp(record.expires_at)),
            revocation: RevocationStatus::NotApplicable,
        })
    }
}
