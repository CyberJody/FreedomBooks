"""Relocate the existing mask intact so raw Book policy setters can be crate-private."""
from pathlib import Path
import shutil

root = Path(__file__).resolve().parents[1]
core = root / "crates/fb-core/src/bankfeeds"
db = root / "crates/fb-db/src/credential_mask"
db.mkdir()
for name in ("secrets.rs", "text.rs", "html_tables.rs", "py_unicode.rs"):
    shutil.move(core / name, db / name)
shutil.copytree(core / "testdata", db / "testdata")
p = db / "secrets.rs"
s = p.read_text().replace("use crate::FbResult;", "use super::MaskResult as FbResult;")
assert s != p.read_text()
# Public, feature-gated attack helpers preserve the existing independent decoder
# and legacy tests; production retains only validated constructors.
at = "    /// Validate a typed secret component."
helpers = ""
for name, signature, args in [
    ("masking_new", "values: impl IntoIterator<Item = S>", "values"),
    ("masking_from_credentials", "user: &str, password: &str, extra: impl IntoIterator<Item = S>", "user, password, extra"),
    ("masking_from_access_url", "url: &str, extra: impl IntoIterator<Item = S>", "url, extra"),
]:
    helpers += f"    #[cfg(feature = \"test-support\")]\n    pub fn test_{name}<S: AsRef<str>>({signature}) -> FbResult<Self> {{ Self::{name}({args}) }}\n"
assert at in s
s = s.replace(at, helpers + at, 1)
p.write_text(s)
p = db / "text.rs"
s = p.read_text()
assert "crate::py::py_strip(" in s
s = s.replace('crate::py::py_strip(&redact(&TAGS.replace_all(&text, "")))', 'redact(&TAGS.replace_all(&text, "")).trim_matches(fb_money::py_isspace)')
p.write_text(s)

old = (core / "write.rs").read_text()
final_scalar = old[old.index("fn safe_final_scalar"):old.index("/// Final SQL-bound")]
masker = old[old.index("fn mask_final_text"):old.index("/// One bank-feeds write transaction")]
masker = masker.replace("fb_db::parse_unique_json", "crate::parse_unique_json").replace("FbResult", "MaskResult").replace("FbError::validation", "err")
module = '''//! Central bank credential policy. Raw guard installation and pending-mask
//! acknowledgement are private to fb-db; callers can only install this validated policy.
mod secrets;
mod html_tables;
mod py_unicode;
pub mod text;
pub use secrets::{SecretSet, MASK, MIN_CREDENTIAL_CHARS, TOO_SHORT, CREDENTIAL_ALPHABET, UNSAFE_CREDENTIAL, is_blank};
use serde_json::Value;

#[derive(Debug)]
pub struct MaskError(&'static str);
impl MaskError { pub fn message(&self) -> String { self.0.into() } }
impl std::fmt::Display for MaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.0) }
}
impl std::error::Error for MaskError {}
type MaskResult<T> = Result<T, MaskError>;
fn err(message: &'static str) -> MaskError { MaskError(message) }
const UNSAFE_JSON: &str = "Bank-feed text cannot be masked while preserving its JSON schema; the transaction was refused.";
pub fn final_text(secrets: &SecretSet, text: &str) -> MaskResult<String> { mask_final_text(secrets, text) }
pub fn final_scalar(secrets: &SecretSet, text: &str) -> String { safe_final_scalar(secrets, text) }
'''
module += final_scalar + masker
module += '''
#[derive(Default)]
struct RetainedSecrets {
    identities: std::collections::HashSet<String>,
    combined: SecretSet,
}
impl crate::Book {
    /// Only a real SecretSet (with private fields) can install a policy. The caller
    /// cannot supply an arbitrary closure, replace the policy, or clear pending IDs.
    pub fn retain_bankfeed_mask(&mut self, secrets: &SecretSet) -> crate::rusqlite::Result<()> {
        if secrets.is_empty() { return Ok(()); }
        let state = self.retained_text_state::<RetainedSecrets>("bankfeeds")?;
        {
            let mut locked = state.lock().map_err(|_| mask_unavailable())?;
            if locked.identities.insert(secrets.policy_identity()) { locked.combined.merge(secrets); }
        }
        self.retain_text_guard("bankfeeds".into(), move |text| {
            let locked = state.lock().map_err(|_| mask_unavailable())?;
            final_text(&locked.combined, text).map_err(|e| crate::rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
        });
        Ok(())
    }
    /// Host restore boundary: parse the actual stored URL before acknowledging its
    /// connection. Legacy credentials get masking only; admission stays strict.
    pub fn restore_bankfeed_mask<S: AsRef<str>>(&mut self, connection_id: &str, access_url: &str, extra: impl IntoIterator<Item=S>) -> crate::rusqlite::Result<()> {
        let secrets = SecretSet::masking_from_access_url(access_url, extra)
            .map_err(|e| crate::rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        if secrets.is_empty() { return Err(mask_unavailable()); }
        self.retain_bankfeed_mask(&secrets)?;
        self.credential_mask_restored(connection_id)
    }
}
fn mask_unavailable() -> crate::rusqlite::Error {
    crate::rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other("The bank credential mask is unavailable; the transaction was refused.")))
}
'''
(db / "mod.rs").write_text(module)

# A small error-converting facade keeps every existing core public signature.
facade = '''//! Validated credential masks; implementation lives beside the central Book writer.
pub use fb_db::credential_mask::{MASK, MIN_CREDENTIAL_CHARS, TOO_SHORT, CREDENTIAL_ALPHABET, UNSAFE_CREDENTIAL};
#[cfg(test)]
pub(super) use fb_db::credential_mask::is_blank;
use crate::FbResult;
#[derive(Clone, Debug, Default)]
pub struct SecretSet(pub(super) fb_db::credential_mask::SecretSet);
impl SecretSet {
    pub fn new<S: AsRef<str>>(values: impl IntoIterator<Item=S>) -> FbResult<Self> { Ok(Self(fb_db::credential_mask::SecretSet::new(values)?)) }
    pub fn from_credentials<S: AsRef<str>>(user: &str, password: &str, extra: impl IntoIterator<Item=S>) -> FbResult<Self> { Ok(Self(fb_db::credential_mask::SecretSet::from_credentials(user,password,extra)?)) }
    pub fn from_access_url<S: AsRef<str>>(url: &str, extra: impl IntoIterator<Item=S>) -> FbResult<Self> { Ok(Self(fb_db::credential_mask::SecretSet::from_access_url(url,extra)?)) }
    pub fn none() -> Self { Self::default() }
    pub fn is_empty(&self) -> bool { self.0.is_empty() }
    pub fn mask(&self, text: &str) -> String { self.0.mask(text) }
'''
for name, signature, args in [
    ("masking_new", "values: impl IntoIterator<Item = S>", "values"),
    ("masking_from_credentials", "user: &str, password: &str, extra: impl IntoIterator<Item = S>", "user, password, extra"),
    ("masking_from_access_url", "url: &str, extra: impl IntoIterator<Item = S>", "url, extra"),
]:
    facade += f"    #[cfg(test)]\n    pub(super) fn {name}<S: AsRef<str>>({signature}) -> FbResult<Self> {{ Ok(Self(fb_db::credential_mask::SecretSet::test_{name}({args})?)) }}\n"
(core / "secrets.rs").write_text(facade + "}\n")
(core / "text.rs").write_text('//! Shared byte-exact provider text policy.\npub use fb_db::credential_mask::text::*;\n')
p = core / "mod.rs"
s = p.read_text().replace("mod html_tables;\n", "").replace("mod py_unicode;\n", "")
p.write_text(s)
p = core / "write.rs"
prefix = '''//! Bank-feed entry masking plus the sealed central Book policy.
use super::{SecretSet, staging};
use crate::store::User;
use crate::{CompanyId, FbError, FbResult, services};
use fb_db::Writer;
use fb_db::rusqlite::types::{ToSql, ToSqlOutput, Value as SqlValue, ValueRef};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
pub(super) fn retain_secrets(book: &mut crate::Book, secrets: &SecretSet) -> FbResult<()> {
    Ok(book.retain_bankfeed_mask(&secrets.0)?)
}
fn safe_scalar(secrets: &SecretSet, text: &str) -> String {
    fb_db::credential_mask::final_scalar(&secrets.0, &staging::provider_text(secrets, text))
}
#[cfg(test)]
pub(crate) fn final_text(secrets: &SecretSet, text: &str) -> FbResult<String> {
    let masked = fb_db::credential_mask::final_text(&secrets.0, text)?;
    FINAL_TEXTS.with_borrow_mut(|log| {
        if let Some(log) = log { log.push((text.to_owned(), masked.clone())); }
    });
    Ok(masked)
}
#[cfg(test)]
thread_local! {
    pub(super) static FINAL_TEXTS: RefCell<Option<Vec<(String, String)>>> = const { RefCell::new(None) };
}
'''
p.write_text(prefix + old[old.index("/// One bank-feeds write transaction"):])
p = core / "credentials.rs"
s = p.read_text()
before = '''    if let Ok(masking) = SecretSet::masking_from_access_url(access_url, &extra) {
        write::retain_secrets(book, &masking)?;
        book.credential_mask_restored(connection_id)?;
    }'''
assert before in s
s = s.replace(before, '    let _ = book.restore_bankfeed_mask(connection_id, access_url, &extra);')
p.write_text(s)

p = root / "crates/fb-db/src/lib.rs"
s = p.read_text()
for name in ("retain_text_guard", "retained_text_state", "credential_mask_restored"):
    assert f"pub fn {name}" in s
    s = s.replace(f"pub fn {name}", f"pub(crate) fn {name}")
s = s.replace("pub mod bankfeeds_text;", "pub mod bankfeeds_text;\npub mod credential_mask;")
p.write_text(s)
p = root / "crates/fb-db/Cargo.toml"
s = p.read_text().replace('sha2 = "0.10"', 'sha2 = "0.10"\nregex = "1"')
p.write_text(s)
p = root / "crates/fb-core/src/error.rs"
p.write_text(p.read_text() + '''
impl From<fb_db::credential_mask::MaskError> for FbError {
    fn from(e: fb_db::credential_mask::MaskError) -> Self { Self::validation(e.message()) }
}
''')

# Keep the invariant test checking the relocated real implementation, not a stale file.
p = core / "tests/fix8.rs"
s = p.read_text()
before = '''    assert!(write.contains("book.retain_text_guard(\\"bankfeeds\\".into(), move |text|"));
    assert!(write.contains("final_text(&state.combined, text)"));
    assert!(write.contains("state.combined.merge(secrets)"));'''
assert before in s
s = s.replace(before, '''    assert!(write.contains("book.retain_bankfeed_mask(&secrets.0)"));
    let policy = include_str!("../../../../fb-db/src/credential_mask/mod.rs");
    assert!(policy.contains("self.retain_text_guard(\\"bankfeeds\\".into(), move |text|"));
    assert!(policy.contains("final_text(&locked.combined, text)"));
    assert!(policy.contains("locked.combined.merge(secrets)"));''')
p.write_text(s)
