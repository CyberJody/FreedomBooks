use crate::*;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use uuid::Uuid;

/// Compiled-in implementations only. Populate every slot declared by the manifest.
/// Multi-slot implementations can supply multiple Arc coercions of the same plugin.
pub struct PluginRegistration {
    pub manifest: &'static PluginManifest,
    pub document_sender: Option<Arc<dyn DocumentSender>>,
    pub bank_feed: Option<Arc<dyn BankFeed>>,
    pub ai_helper: Option<Arc<dyn AiHelper>>,
    pub sales_importer: Option<Arc<dyn SalesImporter>>,
    pub remote_access: Option<Arc<dyn RemoteAccess>>,
}
impl PluginRegistration {
    pub fn new(manifest: &'static PluginManifest) -> Self {
        Self {
            manifest,
            document_sender: None,
            bank_feed: None,
            ai_helper: None,
            sales_importer: None,
            remote_access: None,
        }
    }
    fn validate(&self) -> PluginResult<()> {
        self.manifest.validate()?;
        let implementations = [
            (
                Slot::DocumentSender,
                self.document_sender.as_ref().map(|p| p.manifest()),
            ),
            (
                Slot::BankFeed,
                self.bank_feed.as_ref().map(|p| p.manifest()),
            ),
            (
                Slot::AiHelper,
                self.ai_helper.as_ref().map(|p| p.manifest()),
            ),
            (
                Slot::SalesImporter,
                self.sales_importer.as_ref().map(|p| p.manifest()),
            ),
            (
                Slot::RemoteAccess,
                self.remote_access.as_ref().map(|p| p.manifest()),
            ),
        ];
        for (slot, manifest) in implementations {
            if manifest.is_some() != self.manifest.slots.contains(&slot)
                || manifest.is_some_and(|m| !std::ptr::eq(m, self.manifest))
            {
                return Err(PluginError::InvalidConfiguration);
            }
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionStatus {
    Connected,
    Disconnected,
    NeedsSignIn,
    /// Authorization expired and could not be refreshed.
    Expired,
    /// Provider has positively reported revoked authorization.
    Revoked,
}
#[derive(Clone, Debug)]
pub struct CompanyPluginState {
    pub company_id: Uuid,
    pub connection_id: Uuid,
    pub plugin_id: String,
    pub enabled: bool,
    pub status: ConnectionStatus,
}
#[derive(Default)]
pub struct PluginRegistry {
    plugins: HashMap<&'static str, PluginRegistration>,
    states: HashMap<(Uuid, Uuid), CompanyPluginState>,
}
impl PluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn register(&mut self, plugin: PluginRegistration) -> PluginResult<()> {
        plugin.validate()?;
        if self.plugins.contains_key(plugin.manifest.id) {
            return Err(PluginError::DuplicatePlugin);
        }
        self.plugins.insert(plugin.manifest.id, plugin);
        Ok(())
    }
    /// Atomic replacement from host persistence; never enables plugins implicitly.
    pub fn set_company_states(&mut self, states: Vec<CompanyPluginState>) -> PluginResult<()> {
        let mut next = HashMap::new();
        let mut singleton = HashSet::new();
        for state in states {
            let plugin = self
                .plugins
                .get(state.plugin_id.as_str())
                .ok_or(PluginError::InvalidConfiguration)?;
            if state.company_id.is_nil()
                || state.connection_id.is_nil()
                || next.contains_key(&(state.company_id, state.connection_id))
            {
                return Err(PluginError::InvalidConfiguration);
            }
            if state.enabled {
                for slot in plugin.manifest.slots {
                    if matches!(slot, Slot::AiHelper | Slot::RemoteAccess)
                        && !singleton.insert((state.company_id, *slot))
                    {
                        return Err(PluginError::InvalidConfiguration);
                    }
                }
            }
            next.insert((state.company_id, state.connection_id), state);
        }
        self.states = next;
        Ok(())
    }
    pub fn manifests(&self) -> Vec<&'static PluginManifest> {
        let mut manifests: Vec<_> = self.plugins.values().map(|p| p.manifest).collect();
        manifests.sort_by_key(|m| m.id);
        manifests
    }
    /// Allows connect/disconnect on an enabled connection even when sign-in is needed.
    pub fn enabled_connection(
        &self,
        scope: &CredentialScope,
        slot: Slot,
    ) -> PluginResult<&PluginRegistration> {
        let state = self
            .states
            .get(&(scope.company_id, scope.connection_id))
            .ok_or(PluginError::Disabled)?;
        if !state.enabled || state.plugin_id != scope.plugin_id {
            return Err(PluginError::Disabled);
        }
        let plugin = self
            .plugins
            .get(scope.plugin_id.as_str())
            .ok_or(PluginError::Disabled)?;
        if !plugin.manifest.slots.contains(&slot) {
            return Err(PluginError::Disabled);
        }
        Ok(plugin)
    }
    pub fn connected(
        &self,
        scope: &CredentialScope,
        slot: Slot,
    ) -> PluginResult<&PluginRegistration> {
        let plugin = self.enabled_connection(scope, slot)?;
        let state = &self.states[&(scope.company_id, scope.connection_id)];
        if state.status != ConnectionStatus::Connected {
            return Err(PluginError::NeedsSignIn);
        }
        Ok(plugin)
    }
}
