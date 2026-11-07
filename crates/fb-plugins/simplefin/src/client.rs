//! SimpleFIN's outside half. All sockets, TLS, DNS and redirect decisions stay in PluginHttp.
use crate::parse::{parse_account_set_json, AccountSet};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD},
    Engine,
};
use fb_plugin_api::{
    ExposeSecret, HttpMethod, HttpPolicy, HttpRequest, PluginContext, PluginError, PluginResult,
    SecretBytes, SecretString,
};
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use url::Url;
use zeroize::Zeroizing;

pub const CLAIM_TIMEOUT: Duration = Duration::from_secs(30);
pub const ACCOUNTS_TIMEOUT: Duration = Duration::from_secs(90);
pub const CLAIM_MAX_BYTES: usize = 4096;
pub const ACCOUNTS_MAX_BYTES: usize = 20 * 1024 * 1024;

/// The host explicitly consents to any non-default SimpleFIN server before constructing its policy.
/// This performs no DNS or HTTP; every actual request gets the framework's full DNS guard.
pub fn decode_setup_token(token: &SecretString, policy: &HttpPolicy) -> PluginResult<SecretString> {
    // Bound raw pasted input as well as compact Base64; whitespace never bypasses allocation caps.
    if token.expose_secret().len() > 16 * 1024 {
        return Err(PluginError::InvalidConfiguration);
    }
    let compact = Zeroizing::new(token.expose_secret().split_whitespace().collect::<String>());
    if compact.is_empty() || compact.len() > 4096 {
        return Err(PluginError::InvalidConfiguration);
    }
    let decoded = Zeroizing::new(
        STANDARD
            .decode(compact.as_bytes())
            .or_else(|_| URL_SAFE.decode(compact.as_bytes()))
            .or_else(|_| URL_SAFE_NO_PAD.decode(compact.trim_end_matches('=').as_bytes()))
            .map_err(|_| PluginError::InvalidConfiguration)?,
    );
    let raw = std::str::from_utf8(&decoded)
        .map_err(|_| PluginError::InvalidConfiguration)?
        .trim();
    let url = parse_url(raw)?;
    if !url.username().is_empty()
        || url.password().is_some()
        || !url.path().contains("/claim/")
        || url.path().trim_end_matches('/').ends_with("/claim")
    {
        return Err(PluginError::InvalidConfiguration);
    }
    policy.validate_url(url.as_str())?;
    Ok(SecretString::from(raw))
}

fn parse_url(raw: &str) -> PluginResult<Url> {
    // URL parsers normalize control characters/backslashes; do not let ambiguous authorities through.
    if raw.chars().any(|c| c.is_control() || c == '\\') {
        return Err(PluginError::InvalidConfiguration);
    }
    let url = Url::parse(raw).map_err(|_| PluginError::InvalidConfiguration)?;
    if url.host_str().is_none() || url.fragment().is_some() {
        return Err(PluginError::InvalidConfiguration);
    }
    Ok(url)
}

#[derive(Deserialize)]
struct StoredConnection {
    version: u8,
    host: String,
    scheme: String,
    port: u16,
    #[serde(with = "secret_url")]
    access_url: SecretString,
}
// Explicit serialization is restricted to the encrypted CredentialStore record.
mod secret_url {
    use super::*;
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<SecretString, D::Error> {
        String::deserialize(deserializer).map(SecretString::from)
    }
}

/// Tighten the host transport, preserving its resolver, cancellation and TLS policy.
/// Protocol ceilings never expand a tighter host deadline or response cap.
fn execute(
    cx: &PluginContext,
    request: HttpRequest,
    claim: bool,
    received: &mut u64,
) -> PluginResult<fb_plugin_api::HttpResponse> {
    let (timeout, cap) = if claim {
        (CLAIM_TIMEOUT, CLAIM_MAX_BYTES)
    } else {
        (ACCOUNTS_TIMEOUT, ACCOUNTS_MAX_BYTES)
    };
    let response = cx.http().constrained(timeout, 1, cap)?.execute(request)?;
    *received += response.body.expose().len() as u64;
    Ok(response)
}

pub(crate) fn claim(
    cx: &PluginContext,
    token: &SecretString,
    received: &mut u64,
) -> PluginResult<()> {
    // Reconnecting requires explicit forget first. Never accidentally consume another one-time token.
    if cx.credentials().load()?.is_some() {
        return Err(PluginError::InvalidConfiguration);
    }
    let claim_url = decode_setup_token(token, cx.http().policy())?;
    let origin = parse_url(claim_url.expose_secret())?;
    let response = execute(
        cx,
        HttpRequest {
            method: HttpMethod::Post,
            url: claim_url,
            headers: vec![(
                "User-Agent".into(),
                SecretString::from("FreedomBooks (SimpleFIN client)"),
            )],
            body: SecretBytes::new(vec![]),
        },
        true,
        received,
    )?;
    match response.status {
        200 => (),
        403 => return Err(PluginError::SetupTokenRefused),
        402 => return Err(PluginError::ProviderSubscriptionRequired),
        _ => return Err(PluginError::Network),
    }
    let raw = std::str::from_utf8(response.body.expose())
        .map_err(|_| PluginError::InvalidResponse)?
        .trim();
    let access = parse_url(raw)?;
    if access.host_str() != origin.host_str()
        || access.port_or_known_default() != origin.port_or_known_default()
        || access.scheme() != origin.scheme()
    {
        return Err(PluginError::HostNotAllowed);
    }
    let (base, _auth) = split_access(&access)?;
    cx.http().policy().validate_url(base.expose_secret())?;
    let stored = StoredConnection {
        version: 1,
        host: origin
            .host_str()
            .ok_or(PluginError::InvalidConfiguration)?
            .to_owned(),
        scheme: origin.scheme().to_owned(),
        port: origin
            .port_or_known_default()
            .ok_or(PluginError::InvalidConfiguration)?,
        access_url: SecretString::from(raw),
    };
    #[derive(Serialize)]
    struct Store<'a> {
        version: u8, host: &'a str, scheme: &'a str, port: u16, access_url: &'a str,
    }
    let mut serialized = Zeroizing::new(Vec::new());
    serde_json::to_writer(&mut *serialized, &Store {
        version: stored.version, host: &stored.host, scheme: &stored.scheme,
        port: stored.port, access_url: stored.access_url.expose_secret(),
    }).map_err(|_| PluginError::CredentialStore)?;
    cx.credentials().save(SecretBytes::new(std::mem::take(&mut *serialized)))
}

fn split_access(access: &Url) -> PluginResult<(SecretString, SecretString)> {
    if access.username().is_empty() || access.password().is_none() {
        return Err(PluginError::InvalidResponse);
    }
    let user = Zeroizing::new(
        percent_decode_str(access.username())
            .decode_utf8()
            .map_err(|_| PluginError::InvalidResponse)?
            .into_owned(),
    );
    let password = Zeroizing::new(
        percent_decode_str(access.password().unwrap_or_default())
            .decode_utf8()
            .map_err(|_| PluginError::InvalidResponse)?
            .into_owned(),
    );
    if user.contains(':') || user.chars().chain(password.chars()).any(char::is_control) {
        return Err(PluginError::InvalidResponse);
    }
    let credentials = Zeroizing::new(format!("{}:{}", user.as_str(), password.as_str()));
    let encoded = Zeroizing::new(STANDARD.encode(credentials.as_bytes()));
    let authorization = SecretString::from(format!("Basic {}", encoded.as_str()));
    let mut base = access.clone();
    base.set_username("")
        .map_err(|_| PluginError::InvalidResponse)?;
    base.set_password(None)
        .map_err(|_| PluginError::InvalidResponse)?;
    base.set_query(None);
    base.set_fragment(None);
    let base = SecretString::from(base.as_str().trim_end_matches('/'));
    Ok((base, authorization))
}

#[derive(Default)]
pub struct FetchOptions {
    pub start_epoch: Option<i64>,
    pub end_epoch: Option<i64>,
    pub pending: bool,
    pub balances_only: bool,
    pub account_ids: Vec<String>,
}
impl FetchOptions {
    fn validate(&self) -> PluginResult<()> {
        if [self.start_epoch, self.end_epoch]
            .into_iter()
            .flatten()
            .any(|n| !(0..=4_102_444_800).contains(&n))
            || matches!((self.start_epoch, self.end_epoch), (Some(s), Some(e)) if s > e)
            || self.account_ids.len() > 1000
            || self
                .account_ids
                .iter()
                .any(|s| s.len() > 800 || s.chars().any(char::is_control))
        {
            return Err(PluginError::InvalidConfiguration);
        }
        Ok(())
    }
}

pub(crate) fn fetch(
    cx: &PluginContext,
    options: &FetchOptions,
    received: &mut u64,
) -> PluginResult<AccountSet> {
    options.validate()?;
    let secret = cx.credentials().load()?.ok_or(PluginError::NeedsSignIn)?;
    if secret.expose().len() > 16 * 1024 {
        return Err(PluginError::CredentialStore);
    }
    let stored: StoredConnection =
        serde_json::from_slice(secret.expose()).map_err(|_| PluginError::CredentialStore)?;
    let access = parse_url(stored.access_url.expose_secret())?;
    if stored.version != 1
        || Some(stored.host.as_str()) != access.host_str()
        || stored.scheme != access.scheme()
        || Some(stored.port) != access.port_or_known_default()
    {
        return Err(PluginError::HostNotAllowed);
    }
    let (base, authorization) = split_access(&access)?;
    cx.http().policy().validate_url(base.expose_secret())?;
    let mut url = parse_url(&format!("{}/accounts", base.expose_secret()))?;
    {
        let mut query = url.query_pairs_mut();
        if let Some(n) = options.start_epoch {
            query.append_pair("start-date", &n.to_string());
        }
        if let Some(n) = options.end_epoch {
            query.append_pair("end-date", &n.to_string());
        }
        if options.pending {
            query.append_pair("pending", "1");
        }
        if options.balances_only {
            query.append_pair("balances-only", "1");
        }
        for id in &options.account_ids {
            query.append_pair("account", id);
        }
    }
    // Keep /accounts without a spurious '?' for an unfiltered pull.
    if url.query() == Some("") {
        url.set_query(None);
    }
    let response = execute(
        cx,
        HttpRequest {
            method: HttpMethod::Get,
            url: SecretString::from(url.as_str()),
            headers: vec![
                ("Authorization".into(), authorization),
                ("Accept".into(), SecretString::from("application/json")),
                (
                    "User-Agent".into(),
                    SecretString::from("FreedomBooks (SimpleFIN client)"),
                ),
            ],
            body: SecretBytes::new(vec![]),
        },
        false,
        received,
    )?;
    match response.status {
        200 => (),
        403 => return Err(PluginError::NeedsSignIn),
        402 => return Err(PluginError::ProviderSubscriptionRequired),
        _ => return Err(PluginError::Network),
    }
    parse_account_set_json(response.body.expose())
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
        let _ = <StoredConnection as AmbiguousIfSerialize<_>>::check;
        let _ = <StoredConnection as AmbiguousIfDebug<_>>::check;
    }
}
