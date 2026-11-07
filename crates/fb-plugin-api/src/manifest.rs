use crate::{HttpPolicy, PluginError, PluginResult};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum Slot {
    DocumentSender,
    BankFeed,
    AiHelper,
    SalesImporter,
    RemoteAccess,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthKind {
    OAuthLoopback { provider: &'static str },
    ApiKey,
    SetupToken,
    Password,
    None,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataKind {
    DocumentContents,
    RecipientAddresses,
    ReceiptImages,
    BankData,
    SalesOrders,
    ReportSummary,
    NetworkStatus,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldKind {
    Text,
    Secret,
    Port,
    Choice(&'static [&'static str]),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SettingsField {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,
    pub required: bool,
}

/// Display this complete consent inventory before any outside connection is made.
#[derive(Debug)]
pub struct PluginManifest {
    pub id: &'static str,
    pub name: &'static str,
    pub version: &'static str,
    pub slots: &'static [Slot],
    pub auth: AuthKind,
    pub outside_account: &'static str,
    pub scopes: &'static [&'static str],
    pub data_reads: &'static [DataKind],
    pub data_sends: &'static [DataKind],
    pub settings_fields: &'static [SettingsField],
    pub network_hosts: &'static [&'static str],
}
impl PluginManifest {
    pub fn validate(&self) -> PluginResult<()> {
        if self.id.is_empty()
            || !self
                .id
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
            || self.name.is_empty()
            || self.version.is_empty()
            || self.slots.is_empty()
            || self
                .slots
                .iter()
                .enumerate()
                .any(|(i, s)| self.slots[..i].contains(s))
        {
            return Err(PluginError::InvalidConfiguration);
        }
        HttpPolicy::new(self.network_hosts)?;
        Ok(())
    }
}
