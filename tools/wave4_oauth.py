"""F-M6/F-L8: no ID token browser hints; scope-local provider serialization."""
from pathlib import Path
root = Path(__file__).resolve().parents[1]
base = root / "crates/fb-plugins/openai/src"
p = base / "lib.rs"
s = p.read_text()
old = "lock: Arc<Mutex<HashMap<CredentialScope, u64>>>,"
assert old in s
s = s.replace(old, "lock: Arc<Mutex<HashMap<CredentialScope, Arc<Mutex<u64>>>>>,")
at = "    fn scope(&self, cx: &PluginContext) -> PluginResult<()> {"
assert at in s
s = s.replace(at, '''    // Hold the provider-wide map only while cloning this connection's lock.
    // Network calls serialize rotating tokens within a scope, never across books.
    fn connection_lock(&self, scope: &CredentialScope) -> PluginResult<Arc<Mutex<u64>>> {
        let mut locks = self.lock.lock().map_err(|_| PluginError::CredentialStore)?;
        Ok(locks.entry(scope.clone()).or_insert_with(|| Arc::new(Mutex::new(0))).clone())
    }
''' + at)
old = "let _guard = self.lock.lock().map_err(|_| PluginError::CredentialStore)?;"
assert s.count(old) == 2
s = s.replace(old, "let lock = self.connection_lock(cx.scope())?;\n            let _guard = lock.lock().map_err(|_| PluginError::CredentialStore)?;")
p.write_text(s)
p = base / "receipts.rs"
s = p.read_text()
assert s.count(old) == 1
s = s.replace(old, "let lock = self.connection_lock(cx.scope())?;\n                let _guard = lock.lock().map_err(|_| PluginError::CredentialStore)?;")
p.write_text(s)
p = base / "auth.rs"
s = p.read_text()
old = '''                for (name, value) in [
                    ("login_hint", record.email),
                    ("id_token_hint", record.id_token),
                ] {'''
assert old in s
s = s.replace(old, '''                // No ID token may reach browser history or opener arguments.
                for (name, value) in [("login_hint", record.email)] {''')
old = "let mut generations = self.lock.lock().map_err(|_| PluginError::CredentialStore)?;"
assert s.count(old) == 3
s = s.replace(old, "let lock = self.connection_lock(cx.scope())?;\n            let mut generations = lock.lock().map_err(|_| PluginError::CredentialStore)?;")
old = "let generations = self.lock.lock().map_err(|_| PluginError::CredentialStore)?;"
assert s.count(old) == 1
s = s.replace(old, "let lock = self.connection_lock(cx.scope())?;\n            let generations = lock.lock().map_err(|_| PluginError::CredentialStore)?;")
s = s.replace("advance_generation(&mut generations, cx.scope())", "advance_generation(&mut generations)")
s = s.replace("check_generation(&generations, cx.scope(), pending.generation)", "check_generation(*generations, pending.generation)")
start = s.index("fn advance_generation(")
end = s.index("struct Tokens {", start)
s = s[:start] + '''fn advance_generation(generation: &mut u64) -> PluginResult<u64> {
    *generation = generation.checked_add(1).ok_or(PluginError::CredentialStore)?;
    Ok(*generation)
}
fn check_generation(current: u64, generation: u64) -> PluginResult<()> {
    if current != generation { return Err(PluginError::OAuthState); }
    Ok(())
}
''' + s[end:]
p.write_text(s)
p = base / "tests.rs"
s = p.read_text()
old = '    assert!(params.contains_key("id_token_hint"));'
assert old in s
s = s.replace(old, '''    assert!(!params.contains_key("id_token_hint"));
    assert!(!pending.authorization_url().expose_secret().contains(record.id_token.expose_secret()));''')
p.write_text(s)
