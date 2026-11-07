use crate::SEND_SCOPE;
use fb_plugin_api::*;
use serde::{Deserialize, Serialize};

// No Debug or Serialize on plaintext token owners. Secret fields deserialize
// straight into zeroizing owners, including valid JSON-escaped token strings.
pub(crate) struct Tokens {
    pub client_id: String,
    pub from_address: String,
    pub access: SecretString,
    pub refresh: SecretString,
    pub expires_at: Timestamp,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u8,
    client_id: String,
    from_address: String,
    #[serde(deserialize_with = "secret")]
    access_token: SecretString,
    #[serde(deserialize_with = "secret")]
    refresh_token: SecretString,
    expires_at: u64,
}
#[derive(Deserialize)]
struct Response {
    #[serde(deserialize_with = "secret")]
    access_token: SecretString,
    token_type: String,
    expires_in: u64,
    #[serde(default, deserialize_with = "optional_secret")]
    refresh_token: Option<SecretString>,
    scope: Option<String>,
}
fn secret<'de, D: serde::Deserializer<'de>>(de: D) -> Result<SecretString, D::Error> {
    String::deserialize(de).map(SecretString::from)
}
fn optional_secret<'de, D: serde::Deserializer<'de>>(
    de: D,
) -> Result<Option<SecretString>, D::Error> {
    Option::<String>::deserialize(de).map(|s| s.map(SecretString::from))
}
fn valid_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= 16 * 1024 && token.bytes().all(|b| b.is_ascii_graphic())
}
impl Tokens {
    pub fn decode(raw: &SecretBytes) -> PluginResult<Self> {
        let stored: Stored =
            serde_json::from_slice(raw.expose()).map_err(|_| PluginError::NeedsSignIn)?;
        if stored.version != 1
            || stored.client_id.is_empty()
            || crate::mime::mailbox(&stored.from_address, None).is_err()
            || !valid_token(stored.access_token.expose_secret())
            || !valid_token(stored.refresh_token.expose_secret())
        {
            return Err(PluginError::NeedsSignIn);
        }
        Ok(Self {
            client_id: stored.client_id,
            from_address: stored.from_address,
            access: stored.access_token,
            refresh: stored.refresh_token,
            expires_at: Timestamp(stored.expires_at),
        })
    }
    pub fn encode(&self) -> PluginResult<SecretBytes> {
        #[derive(Serialize)]
        struct Store<'a> {
            version: u8,
            client_id: &'a str,
            from_address: &'a str,
            access_token: &'a str,
            refresh_token: &'a str,
            expires_at: u64,
        }
        serde_json::to_vec(&Store {
            version: 1,
            client_id: &self.client_id,
            from_address: &self.from_address,
            access_token: self.access.expose_secret(),
            refresh_token: self.refresh.expose_secret(),
            expires_at: self.expires_at.0,
        })
        .map(SecretBytes::new)
        .map_err(|_| PluginError::CredentialStore)
    }
    pub fn from_response(
        response: &HttpResponse,
        now: Timestamp,
        client_id: &str,
        from_address: &str,
        old: Option<&Self>,
    ) -> PluginResult<Self> {
        if response.status != 200 {
            #[derive(Deserialize)]
            struct Failure {
                error: String,
            }
            let invalid_grant = serde_json::from_slice::<Failure>(response.body.expose())
                .is_ok_and(|e| e.error == "invalid_grant");
            return Err(match response.status {
                400 if invalid_grant => PluginError::NeedsSignIn,
                400 | 401 => PluginError::InvalidConfiguration,
                403 => PluginError::OAuthDenied,
                429 | 500..=599 => PluginError::Network,
                _ => PluginError::InvalidResponse,
            });
        }
        let wire: Response = serde_json::from_slice(response.body.expose())
            .map_err(|_| PluginError::InvalidResponse)?;
        if !wire.token_type.eq_ignore_ascii_case("Bearer")
            || !valid_token(wire.access_token.expose_secret())
            || wire.expires_in == 0
            || wire.expires_in > 7 * 24 * 60 * 60
            || wire
                .scope
                .as_deref()
                .is_some_and(|s| s.split_ascii_whitespace().collect::<Vec<_>>() != [SEND_SCOPE])
        {
            return Err(PluginError::InvalidResponse);
        }
        // A malformed replacement is not evidence that the existing grant was
        // revoked. Preserve the stored refresh token on malformed responses.
        if wire
            .refresh_token
            .as_ref()
            .is_some_and(|s| !valid_token(s.expose_secret()))
        {
            return Err(PluginError::InvalidResponse);
        }
        let refresh = wire
            .refresh_token
            .or_else(|| old.map(|t| t.refresh.clone()))
            .ok_or(PluginError::NeedsSignIn)?;
        let expires_at = now
            .0
            .checked_add(wire.expires_in)
            .ok_or(PluginError::InvalidResponse)?;
        Ok(Self {
            client_id: client_id.into(),
            from_address: from_address.into(),
            access: wire.access_token,
            refresh,
            expires_at: Timestamp(expires_at),
        })
    }
}

#[cfg(test)]
mod token_trait_guard {
    use super::*;
    #[test]
    fn token_owners_cannot_be_serialized_or_debugged() {
        trait AmbiguousIfSerialize<A> { fn check() {} }
        impl<T: ?Sized> AmbiguousIfSerialize<()> for T {}
        struct Serializes;
        impl<T: ?Sized + serde::Serialize> AmbiguousIfSerialize<Serializes> for T {}
        trait AmbiguousIfDebug<A> { fn check() {} }
        impl<T: ?Sized> AmbiguousIfDebug<()> for T {}
        struct Debugs;
        impl<T: ?Sized + std::fmt::Debug> AmbiguousIfDebug<Debugs> for T {}
        let _ = <Tokens as AmbiguousIfSerialize<_>>::check;
        let _ = <Tokens as AmbiguousIfDebug<_>>::check;
        let _ = <Stored as AmbiguousIfSerialize<_>>::check;
        let _ = <Stored as AmbiguousIfDebug<_>>::check;
        let _ = <Response as AmbiguousIfSerialize<_>>::check;
        let _ = <Response as AmbiguousIfDebug<_>>::check;
    }
}
