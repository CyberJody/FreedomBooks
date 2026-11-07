use fb_plugin_api::oauth::OAuthGrant;
use fb_plugin_api::*;
use serde::{Deserialize, Deserializer, Serialize};
use zeroize::Zeroizing;

const MAX_TOKEN_RESPONSE: usize = 64 * 1024;
const MAX_TOKEN: usize = 16 * 1024;

// No Debug implementations. Deserialize secrets directly into secrecy wrappers;
// serde/HTTP may still make transient copies (not a locked-memory guarantee).
#[derive(Deserialize)]
pub(crate) struct Credentials {
    client_id: String,
    #[serde(deserialize_with = "read_secret")]
    access_token: SecretString,
    #[serde(deserialize_with = "read_secret")]
    refresh_token: SecretString,
    expires_at: u64,
}
fn read_secret<'de, D: Deserializer<'de>>(d: D) -> Result<SecretString, D::Error> {
    String::deserialize(d).map(SecretString::from)
}
fn read_optional_secret<'de, D: Deserializer<'de>>(d: D) -> Result<Option<SecretString>, D::Error> {
    Option::<String>::deserialize(d).map(|v| v.map(SecretString::from))
}
#[derive(Deserialize)]
struct TokenResponse {
    #[serde(deserialize_with = "read_secret")]
    access_token: SecretString,
    #[serde(default, deserialize_with = "read_optional_secret")]
    refresh_token: Option<SecretString>,
    token_type: String,
    expires_in: u64,
    scope: Option<String>,
    #[serde(default, deserialize_with = "read_optional_secret")]
    id_token: Option<SecretString>,
}
#[derive(Deserialize)]
struct TokenError<'a> {
    #[serde(borrow)]
    error: &'a str,
}

pub(crate) fn save(cx: &PluginContext, record: &Credentials) -> PluginResult<()> {
    #[derive(Serialize)]
    struct Store<'a> {
        client_id: &'a str, access_token: &'a str, refresh_token: &'a str, expires_at: u64,
    }
    let mut bytes = Zeroizing::new(Vec::new());
    serde_json::to_writer(&mut *bytes, &Store {
        client_id: &record.client_id, access_token: record.access_token.expose_secret(),
        refresh_token: record.refresh_token.expose_secret(), expires_at: record.expires_at,
    }).map_err(|_| PluginError::CredentialStore)?;
    cx.credentials()
        .save(SecretBytes::new(std::mem::take(&mut *bytes)))
}

pub(crate) fn exchange(
    cx: &PluginContext,
    client_id: &str,
    grant: &OAuthGrant,
    counts: &mut (u64, u64),
) -> PluginResult<Credentials> {
    let response = request(cx, grant.token_endpoint.clone(), grant.token_form(), counts)?;
    parse(response, client_id, cx.clock().now(), None)
}

pub(crate) fn access_token(
    cx: &PluginContext,
    client_id: &str,
    endpoint: &str,
    counts: &mut (u64, u64),
) -> PluginResult<SecretString> {
    let bytes = cx.credentials().load()?.ok_or(PluginError::NeedsSignIn)?;
    if bytes.expose().len() > MAX_TOKEN_RESPONSE {
        return Err(PluginError::CredentialStore);
    }
    let old: Credentials =
        serde_json::from_slice(bytes.expose()).map_err(|_| PluginError::CredentialStore)?;
    if old.client_id != client_id {
        return Err(PluginError::NeedsSignIn);
    }
    if !valid_token(&old.access_token) || !valid_token(&old.refresh_token) {
        return Err(PluginError::CredentialStore);
    }
    if old.expires_at > cx.clock().now().0.saturating_add(60) {
        return Ok(old.access_token);
    }
    let form = SecretString::from(
        url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs([
                ("grant_type", "refresh_token"),
                ("client_id", client_id),
                ("refresh_token", old.refresh_token.expose_secret()),
                ("scope", "Mail.Send offline_access"),
            ])
            .finish(),
    );
    let result = request(cx, SecretString::from(endpoint), form, counts).and_then(|response| {
        parse(
            response,
            client_id,
            cx.clock().now(),
            Some(old.refresh_token),
        )
    });
    match result {
        Ok(record) => {
            // Persist rotation BEFORE sending. Never fall back to an expired token.
            save(cx, &record)?;
            Ok(record.access_token)
        }
        Err(PluginError::NeedsSignIn) => {
            cx.credentials().forget()?;
            Err(PluginError::NeedsSignIn)
        }
        other => other.map(|record| record.access_token),
    }
}

fn valid_token(value: &SecretString) -> bool {
    let value = value.expose_secret();
    !value.is_empty() && value.len() <= MAX_TOKEN && value.bytes().all(|b| b.is_ascii_graphic())
}

fn request(
    cx: &PluginContext,
    endpoint: SecretString,
    form: SecretString,
    counts: &mut (u64, u64),
) -> PluginResult<HttpResponse> {
    counts.0 += form.expose_secret().len() as u64;
    let response = cx.http().execute(HttpRequest {
        method: HttpMethod::Post,
        url: endpoint,
        headers: vec![(
            "Content-Type".into(),
            SecretString::from("application/x-www-form-urlencoded"),
        )],
        body: SecretBytes::new(form.expose_secret().as_bytes().to_vec()),
    })?;
    counts.1 += response.body.expose().len() as u64;
    if response.body.expose().len() > MAX_TOKEN_RESPONSE {
        return Err(PluginError::TooLarge);
    }
    Ok(response)
}

fn parse(
    response: HttpResponse,
    client_id: &str,
    now: Timestamp,
    old_refresh: Option<SecretString>,
) -> PluginResult<Credentials> {
    if response.status != 200 {
        if response.status == 429 || response.status >= 500 {
            return Err(PluginError::Network);
        }
        if response.status == 400 {
            if let Ok(error) = serde_json::from_slice::<TokenError<'_>>(response.body.expose()) {
                return Err(match error.error {
                    "invalid_grant"
                    | "interaction_required"
                    | "login_required"
                    | "consent_required" => PluginError::NeedsSignIn,
                    "invalid_client" | "unauthorized_client" | "invalid_scope" => {
                        PluginError::InvalidConfiguration
                    }
                    "access_denied" => PluginError::OAuthDenied,
                    _ => PluginError::InvalidResponse,
                });
            }
        }
        return Err(PluginError::InvalidResponse);
    }
    let token: TokenResponse =
        serde_json::from_slice(response.body.expose()).map_err(|_| PluginError::InvalidResponse)?;
    // No openid scope requested. Reject rather than trust any unsolicited ID token.
    if token.id_token.is_some()
        || !token.token_type.eq_ignore_ascii_case("Bearer")
        || token.expires_in == 0
        || !valid_token(&token.access_token)
    {
        return Err(PluginError::InvalidResponse);
    }
    if let Some(scope) = &token.scope {
        if !scope.split_ascii_whitespace().any(|s| {
            s.eq_ignore_ascii_case("Mail.Send")
                || s.eq_ignore_ascii_case("https://graph.microsoft.com/Mail.Send")
        }) {
            return Err(PluginError::OAuthDenied);
        }
    }
    let refresh_token = token
        .refresh_token
        .or(old_refresh)
        .ok_or(PluginError::InvalidResponse)?;
    if !valid_token(&refresh_token) {
        return Err(PluginError::InvalidResponse);
    }
    Ok(Credentials {
        client_id: client_id.into(),
        access_token: token.access_token,
        refresh_token,
        expires_at: now
            .0
            .checked_add(token.expires_in)
            .ok_or(PluginError::InvalidResponse)?,
    })
}

pub(crate) fn connection_expiry(cx: &PluginContext) -> PluginResult<Timestamp> {
    let bytes = cx.credentials().load()?.ok_or(PluginError::NeedsSignIn)?;
    if bytes.expose().len() > MAX_TOKEN_RESPONSE {
        return Err(PluginError::CredentialStore);
    }
    let record: Credentials =
        serde_json::from_slice(bytes.expose()).map_err(|_| PluginError::CredentialStore)?;
    Ok(Timestamp(record.expires_at))
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
        let _ = <Credentials as AmbiguousIfSerialize<_>>::check;
        let _ = <Credentials as AmbiguousIfDebug<_>>::check;
        let _ = <TokenResponse as AmbiguousIfSerialize<_>>::check;
        let _ = <TokenResponse as AmbiguousIfDebug<_>>::check;
    }
}
