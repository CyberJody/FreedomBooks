use fb_plugin_api::*;
use std::{collections::HashMap, fmt, sync::Mutex};

/// TEST ONLY: memory-backed, zeroizing secret storage; not encrypted persistence.
#[derive(Default)]
pub struct MemoryCredentialStore {
    values: Mutex<HashMap<CredentialScope, SecretBytes>>,
}
impl MemoryCredentialStore {
    pub fn new() -> Self {
        Self::default()
    }
}
impl CredentialStore for MemoryCredentialStore {
    fn load(&self, scope: &CredentialScope) -> PluginResult<Option<SecretBytes>> {
        let values = self
            .values
            .lock()
            .map_err(|_| PluginError::CredentialStore)?;
        Ok(values
            .get(scope)
            .map(|value| SecretBytes::new(value.expose().to_vec())))
    }
    fn save(&self, scope: &CredentialScope, value: SecretBytes) -> PluginResult<()> {
        self.values
            .lock()
            .map_err(|_| PluginError::CredentialStore)?
            .insert(scope.clone(), value);
        Ok(())
    }
    fn forget(&self, scope: &CredentialScope) -> PluginResult<()> {
        self.values
            .lock()
            .map_err(|_| PluginError::CredentialStore)?
            .remove(scope);
        Ok(())
    }
}
impl fmt::Debug for MemoryCredentialStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MemoryCredentialStore([REDACTED])")
    }
}
