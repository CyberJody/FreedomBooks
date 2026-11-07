//! Validated credential masks; implementation lives beside the central Book writer.
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
    #[cfg(test)]
    pub(super) fn masking_new<S: AsRef<str>>(values: impl IntoIterator<Item = S>) -> FbResult<Self> { Ok(Self(fb_db::credential_mask::SecretSet::test_masking_new(values)?)) }
    #[cfg(test)]
    pub(super) fn masking_from_credentials<S: AsRef<str>>(user: &str, password: &str, extra: impl IntoIterator<Item = S>) -> FbResult<Self> { Ok(Self(fb_db::credential_mask::SecretSet::test_masking_from_credentials(user, password, extra)?)) }
    #[cfg(test)]
    pub(super) fn masking_from_access_url<S: AsRef<str>>(url: &str, extra: impl IntoIterator<Item = S>) -> FbResult<Self> { Ok(Self(fb_db::credential_mask::SecretSet::test_masking_from_access_url(url, extra)?)) }
}
