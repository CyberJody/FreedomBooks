use crate::{protocol, OpenAi, PLAN_SCOPE, RESOURCE, SCOPES};
use fb_plugin_api::{oauth::*, *};
use jsonwebtoken::{decode, decode_header, jwk::JwkSet, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod secret {
    use fb_plugin_api::SecretString;
    use serde::{Deserialize, Deserializer};
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SecretString, D::Error> {
        Ok(SecretString::from(String::deserialize(d)?))
    }
}
fn empty_secret() -> SecretString {
    SecretString::from(String::new())
}
/// No Debug, no public serialization surface. Only the scoped credential store receives it.
#[derive(Deserialize)]
pub(crate) struct Record {
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub ext_agent_host_id: String,
    #[serde(default)]
    pub issuer: String,
    #[serde(with = "secret", default = "empty_secret")]
    pub subject: SecretString,
    #[serde(with = "secret", default = "empty_secret")]
    pub email: SecretString,
    #[serde(with = "secret", default = "empty_secret")]
    pub id_token: SecretString,
    #[serde(with = "secret", default = "empty_secret")]
    pub access_token: SecretString,
    #[serde(with = "secret", default = "empty_secret")]
    pub refresh_token: SecretString,
    #[serde(with = "secret", default = "empty_secret")]
    pub api_key: SecretString,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub expires_at: u64,
}
impl Default for Record {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            ext_agent_host_id: String::new(),
            issuer: String::new(),
            subject: empty_secret(),
            email: empty_secret(),
            id_token: empty_secret(),
            access_token: empty_secret(),
            refresh_token: empty_secret(),
            api_key: empty_secret(),
            scopes: vec![],
            expires_at: 0,
        }
    }
}
impl Record {
    pub(crate) fn load(cx: &PluginContext) -> PluginResult<Self> {
        let bytes = cx.credentials().load()?.ok_or(PluginError::NeedsSignIn)?;
        let record: Self =
            serde_json::from_slice(bytes.expose()).map_err(|_| PluginError::CredentialStore)?;
        if !record.api_key.expose_secret().is_empty() && !valid_key(record.api_key.expose_secret())
        {
            return Err(PluginError::CredentialStore);
        }
        Ok(record)
    }
    pub(crate) fn save(&self, cx: &PluginContext) -> PluginResult<()> {
        // This private borrowed wire type exists only at the encrypted-store boundary.
        // The token owner deliberately has neither Serialize nor Debug.
        #[derive(Serialize)]
        struct Store<'a> {
            client_id: &'a str,
            ext_agent_host_id: &'a str,
            issuer: &'a str,
            subject: &'a str,
            email: &'a str,
            id_token: &'a str,
            access_token: &'a str,
            refresh_token: &'a str,
            api_key: &'a str,
            scopes: &'a [String],
            expires_at: u64,
        }
        let wire = Store {
            client_id: &self.client_id, ext_agent_host_id: &self.ext_agent_host_id,
            issuer: &self.issuer, subject: self.subject.expose_secret(),
            email: self.email.expose_secret(), id_token: self.id_token.expose_secret(),
            access_token: self.access_token.expose_secret(), refresh_token: self.refresh_token.expose_secret(),
            api_key: self.api_key.expose_secret(), scopes: &self.scopes, expires_at: self.expires_at,
        };
        let mut bytes = zeroize::Zeroizing::new(Vec::new());
        serde_json::to_writer(&mut *bytes, &wire).map_err(|_| PluginError::CredentialStore)?;
        cx.credentials().save(SecretBytes::new(std::mem::take(&mut *bytes)))
    }
    fn plan(&self) -> bool {
        self.scopes.iter().any(|s| s == PLAN_SCOPE)
    }
}

/// Holds the already-bound framework listener. Drop cancels; complete consumes it.
/// The system-browser opener receives only `authorization_url()` (a secret).
pub struct PendingAuthorization {
    flow: OAuthLoopback,
    scope: CredentialScope,
    client_id: String,
    generation: u64,
    cancellation: CancellationToken,
}
impl PendingAuthorization {
    pub fn authorization_url(&self) -> &SecretString {
        self.flow.authorization_url()
    }
    pub fn redirect_uri(&self) -> &str {
        self.flow.redirect_uri()
    }
}
impl std::fmt::Debug for PendingAuthorization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PendingAuthorization([REDACTED])")
    }
}

impl OpenAi {
    pub fn start_authorization(&self, cx: &PluginContext) -> PluginResult<PendingAuthorization> {
        self.audited(cx, AuditAction::Connect, AuditPurpose::Connection, || {
            let lock = self.connection_lock(cx.scope())?;
            let mut generations = lock.lock().map_err(|_| PluginError::CredentialStore)?;
            let generation = advance_generation(&mut generations)?;
            let previous = if cx.credentials().load()?.is_some() {
                Some(Record::load(cx)?)
            } else {
                None
            };
            let client_id = previous
                .as_ref()
                .filter(|r| valid_client(&r.client_id))
                .map(|r| r.client_id.clone())
                .unwrap_or_else(|| "dynamic_agent_client".into());
            let endpoints = self.oauth_endpoints()?;
            let mut config = OAuthConfig::new(
                endpoints,
                client_id.clone(),
                SCOPES.iter().map(|s| (*s).into()).collect(),
                "/auth/openai/callback".into(),
            );
            config.extra_authorization_parameters = vec![
                ("resource".into(), RESOURCE.into()),
                ("ext_agent_host_id".into(), self.host_id.clone()),
            ];
            if client_id == "dynamic_agent_client" {
                config
                    .extra_authorization_parameters
                    .push(("agent_name_hint".into(), "FreedomBooks".into()));
            } else if let Some(record) = previous {
                // No ID token may reach browser history or opener arguments.
                if !record.email.expose_secret().is_empty() {
                    config
                        .extra_authorization_parameters
                        .push(("login_hint".into(), record.email.expose_secret().into()));
                }
            }
            Ok(PendingAuthorization {
                flow: OAuthLoopback::start(config)?,
                scope: cx.scope().clone(),
                client_id,
                generation,
                cancellation: cx.cancellation().clone(),
            })
        })
    }
    fn oauth_endpoints(&self) -> PluginResult<OAuthEndpoints> {
        #[cfg(any(test, feature = "test-support"))]
        if self.endpoints.authorize.starts_with("http://127.0.0.1:") {
            return OAuthEndpoints::loopback_for_tests(
                &self.endpoints.authorize,
                &self.endpoints.token,
            );
        }
        OAuthEndpoints::new(&self.endpoints.authorize, &self.endpoints.token)
    }
    /// Consume the state-validated callback. The issued client id is bound again
    /// by the code exchange and signed ID-token audience before credentials save.
    pub fn complete_authorization(
        &self,
        cx: &PluginContext,
        pending: PendingAuthorization,
    ) -> PluginResult<()> {
        self.audited(cx, AuditAction::Connect, AuditPurpose::Connection, || {
            if pending.scope != *cx.scope() {
                return Err(PluginError::WrongScope);
            }
            // Never hold the provider mutex while the browser is waiting.
            pending.cancellation.check()?;
            let grant = pending.flow.wait_cancellable(cx.cancellation())?;
            pending.cancellation.check()?;
            cx.check_active()?;
            let callback_client_id = grant.returned_client_id.as_deref();
            let id = if pending.client_id == "dynamic_agent_client" {
                callback_client_id
                    .filter(|s| valid_client(s))
                    .ok_or(PluginError::OAuthCallback)?
                    .to_string()
            } else {
                if callback_client_id.is_some_and(|id| id != pending.client_id) {
                    return Err(PluginError::OAuthCallback);
                }
                pending.client_id.clone()
            };
            let lock = self.connection_lock(cx.scope())?;
            let generations = lock.lock().map_err(|_| PluginError::CredentialStore)?;
            check_generation(*generations, pending.generation)?;
            let fields = [
                ("grant_type", "authorization_code"),
                ("client_id", id.as_str()),
                ("code", grant.code.expose_secret()),
                ("code_verifier", grant.verifier.expose_secret()),
                ("redirect_uri", grant.redirect_uri.as_str()),
                ("resource", RESOURCE),
            ];
            let response = protocol::form(cx, &self.endpoints.token, &fields)?;
            let tokens = parse_tokens(response)?;
            if !tokens.scopes.iter().any(|s| s == PLAN_SCOPE) {
                return Err(PluginError::OAuthDenied);
            }
            let claims = self.verify_identity(cx, &tokens.id_token, &id)?;
            grant.verify_nonce(claims["nonce"].as_str().ok_or(PluginError::OAuthNonce)?)?;
            if cx.credentials().load()?.is_some() {
                let previous = Record::load(cx)?;
                if !previous.subject.expose_secret().is_empty()
                    && previous.subject.expose_secret() != claims["sub"].as_str().unwrap_or("")
                {
                    return Err(PluginError::OAuthDenied);
                }
            }
            // Keep the mutex through exchange, identity checks and persistence.
            // Recheck immediately before saving; cancellation cannot slip between
            // this check and the credential-store write.
            check_generation(*generations, pending.generation)?;
            cx.check_active()?;
            pending.cancellation.check()?;
            Record {
                client_id: id,
                ext_agent_host_id: self.host_id.clone(),
                issuer: self.endpoints.issuer.clone(),
                subject: SecretString::from(claims["sub"].as_str().unwrap_or("").to_string()),
                email: SecretString::from(claims["email"].as_str().unwrap_or("").to_string()),
                id_token: tokens.id_token,
                access_token: tokens.access_token,
                refresh_token: tokens.refresh_token,
                scopes: tokens.scopes,
                expires_at: cx.clock().now().0.saturating_add(tokens.expires_in),
                ..Record::default()
            }
            .save(cx)
        })
    }
    pub fn connect_with_key(&self, cx: &PluginContext, key: SecretString) -> PluginResult<String> {
        self.audited(cx, AuditAction::Connect, AuditPurpose::Connection, || {
            let lock = self.connection_lock(cx.scope())?;
            let mut generations = lock.lock().map_err(|_| PluginError::CredentialStore)?;
            let raw = key.expose_secret().trim();
            if !valid_key(raw) {
                return Err(PluginError::InvalidConfiguration);
            }
            advance_generation(&mut generations)?;
            let key = SecretString::from(raw.to_string());
            protocol::models(cx, &self.endpoints.api, &key)?;
            let last4 = raw
                .get(raw.len().saturating_sub(4)..)
                .ok_or(PluginError::InvalidConfiguration)?
                .to_string();
            Record {
                api_key: key,
                ..Record::default()
            }
            .save(cx)?;
            Ok(last4)
        })
    }
    pub fn key_last4(&self, cx: &PluginContext) -> PluginResult<Option<String>> {
        self.scope(cx)?;
        let record = Record::load(cx)?;
        let key = record.api_key.expose_secret();
        if key.is_empty() {
            return Ok(None);
        }
        key.len()
            .checked_sub(4)
            .and_then(|start| key.get(start..))
            .map(|last| Some(last.to_string()))
            .ok_or(PluginError::CredentialStore)
    }
    /// None for an API key, Some(true/false) for confirmed/unconfirmed OAuth revocation.
    /// Always forget locally, including unreadable credentials or transport failures.
    pub fn disconnect(&self, cx: &PluginContext) -> PluginResult<Option<bool>> {
        self.audited(
            cx,
            AuditAction::Disconnect,
            AuditPurpose::Connection,
            || {
                let lock = self.connection_lock(cx.scope())?;
            let mut generations = lock.lock().map_err(|_| PluginError::CredentialStore)?;
                advance_generation(&mut generations)?;
                let revoked = match Record::load(cx) {
                    Ok(r) if !r.refresh_token.expose_secret().is_empty() => Some(
                        protocol::form(
                            cx,
                            &self.endpoints.revoke,
                            &[
                                ("token", r.refresh_token.expose_secret()),
                                ("token_type_hint", "refresh_token"),
                                ("client_id", &r.client_id),
                            ],
                        )
                        .is_ok_and(|r| r.status == 200),
                    ),
                    Ok(r) if !r.client_id.is_empty() => Some(false),
                    _ => None,
                };
                cx.credentials().forget()?;
                Ok(revoked)
            },
        )
    }
    // Caller owns the shared instance mutex throughout use of the bearer, so forget
    // cannot race an in-flight call and refresh can never reuse a rotating token.
    pub(crate) fn bearer(&self, cx: &PluginContext) -> PluginResult<Record> {
        let mut record = Record::load(cx)?;
        if !record.api_key.expose_secret().is_empty() {
            return Ok(record);
        }
        if !record.plan() {
            invalidate(cx, record)?;
            return Err(PluginError::NeedsSignIn);
        }
        if record.expires_at > cx.clock().now().0.saturating_add(120)
            && !record.access_token.expose_secret().is_empty()
        {
            return Ok(record);
        }
        if record.refresh_token.expose_secret().is_empty() || !valid_client(&record.client_id) {
            return Err(PluginError::NeedsSignIn);
        }
        let response = protocol::form(
            cx,
            &self.endpoints.token,
            &[
                ("grant_type", "refresh_token"),
                ("client_id", &record.client_id),
                ("refresh_token", record.refresh_token.expose_secret()),
                ("resource", RESOURCE),
            ],
        )?;
        if response.status != 200 {
            // Failure responses alone are inspected for an OAuth error code.
            // Success is deserialized exactly once into secret-owning fields.
            #[derive(Deserialize)]
            struct Failure {
                error: String,
            }
            let invalid = response.status == 400
                || response.status == 401
                || serde_json::from_slice::<Failure>(response.body.expose()).is_ok_and(|data| {
                    matches!(
                        data.error.as_str(),
                        "invalid_grant"
                            | "invalid_refresh_token"
                            | "token_expired"
                            | "refresh_token_expired"
                            | "refresh_token_invalidated"
                            | "refresh_token_reused"
                            | "invalid_client"
                    )
                });
            if invalid {
                invalidate(cx, record)?;
                return Err(PluginError::NeedsSignIn);
            }
        }
        let tokens = parse_tokens(response)?;
        record.access_token = tokens.access_token;
        if !tokens.refresh_token.expose_secret().is_empty() {
            record.refresh_token = tokens.refresh_token;
        }
        if !tokens.id_token.expose_secret().is_empty() {
            record.id_token = tokens.id_token;
        }
        if !tokens.scopes.is_empty() {
            record.scopes = tokens.scopes;
        }
        record.expires_at = cx.clock().now().0.saturating_add(tokens.expires_in);
        if !record.plan() {
            invalidate(cx, record)?;
            return Err(PluginError::NeedsSignIn);
        }
        record.save(cx)?;
        Ok(record)
    }
    pub(crate) fn verify_identity(
        &self,
        cx: &PluginContext,
        token: &SecretString,
        client_id: &str,
    ) -> PluginResult<Value> {
        if token.expose_secret().len() > 16 * 1024 {
            return Err(PluginError::InvalidResponse);
        }
        let header =
            decode_header(token.expose_secret()).map_err(|_| PluginError::InvalidResponse)?;
        if !matches!(header.alg, Algorithm::RS256 | Algorithm::ES256) {
            return Err(PluginError::InvalidResponse);
        }
        let discovery = protocol::get_json(cx, &self.endpoints.discovery)?;
        if discovery["issuer"].as_str() != Some(self.endpoints.issuer.as_str()) {
            return Err(PluginError::InvalidResponse);
        }
        let uri = discovery["jwks_uri"]
            .as_str()
            .ok_or(PluginError::InvalidResponse)?;
        // Still subject to the same exact-host policy and SSRF guard.
        let keys: JwkSet = serde_json::from_value(protocol::get_json(cx, uri)?)
            .map_err(|_| PluginError::InvalidResponse)?;
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[&self.endpoints.issuer]);
        validation.set_audience(&[client_id]);
        validation.validate_exp = false; // injected clock, never library wall time
        validation.validate_nbf = false;
        validation.required_spec_claims = ["iss", "aud", "exp", "sub"]
            .iter()
            .map(|s| (*s).into())
            .collect();
        for key in keys.keys.iter().filter(|k| {
            header
                .kid
                .as_ref()
                .is_none_or(|kid| k.common.key_id.as_ref() == Some(kid))
        }) {
            let Ok(key) = DecodingKey::from_jwk(key) else {
                continue;
            };
            let Ok(data) = decode::<Value>(token.expose_secret(), &key, &validation) else {
                continue;
            };
            let c = data.claims;
            let now = cx.clock().now().0;
            if c["exp"]
                .as_u64()
                .is_none_or(|exp| exp.saturating_add(60) < now)
                || c["iat"]
                    .as_u64()
                    .is_none_or(|iat| iat > now.saturating_add(60))
                || c["sub"].as_str().is_none_or(|s| s.trim().is_empty())
            {
                return Err(PluginError::InvalidResponse);
            }
            return Ok(c);
        }
        Err(PluginError::InvalidResponse)
    }
}
pub(crate) fn invalidate(cx: &PluginContext, mut record: Record) -> PluginResult<()> {
    if !record.api_key.expose_secret().is_empty() {
        return cx.credentials().forget();
    }
    record.access_token = empty_secret();
    record.refresh_token = empty_secret();
    record.scopes.clear();
    record.expires_at = 0;
    record.save(cx)
}
fn valid_client(s: &str) -> bool {
    s.starts_with("oaiapp_")
        && s.len() > 7
        && s.len() < 256
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn valid_key(raw: &str) -> bool {
    (20..=512).contains(&raw.len()) && raw.bytes().all(|b| b > 32 && b < 127)
}
fn advance_generation(generation: &mut u64) -> PluginResult<u64> {
    *generation = generation.checked_add(1).ok_or(PluginError::CredentialStore)?;
    Ok(*generation)
}
fn check_generation(current: u64, generation: u64) -> PluginResult<()> {
    if current != generation { return Err(PluginError::OAuthState); }
    Ok(())
}
struct Tokens {
    access_token: SecretString,
    refresh_token: SecretString,
    id_token: SecretString,
    scopes: Vec<String>,
    expires_in: u64,
}
// No Debug/Serialize or intermediate Value: token allocations move immediately
// into zeroizing owners, including JSON-escaped token values. Derived decoding
// also rejects duplicate token fields instead of silently choosing the last one.
#[derive(Deserialize)]
struct TokenResponse {
    #[serde(with = "secret")]
    access_token: SecretString,
    #[serde(with = "secret", default = "empty_secret")]
    refresh_token: SecretString,
    #[serde(with = "secret", default = "empty_secret")]
    id_token: SecretString,
    token_type: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    expires_in: u64,
}
fn parse_tokens(response: HttpResponse) -> PluginResult<Tokens> {
    let wire: TokenResponse = protocol::decode_ok(response)?;
    if wire.access_token.expose_secret().is_empty() || wire.token_type != "Bearer" {
        return Err(PluginError::InvalidResponse);
    }
    Ok(Tokens {
        access_token: wire.access_token,
        refresh_token: wire.refresh_token,
        id_token: wire.id_token,
        scopes: wire.scope.split_whitespace().map(String::from).collect(),
        expires_in: wire.expires_in,
    })
}

#[cfg(test)]
mod token_tests {
    use super::*;

    #[test]
    fn token_owners_cannot_be_serialized_or_debugged() {
        trait AmbiguousIfSerialize<A> { fn check() {} }
        impl<T: ?Sized> AmbiguousIfSerialize<()> for T {}
        struct Serializes;
        impl<T: ?Sized + serde::Serialize> AmbiguousIfSerialize<Serializes> for T {}
        let _ = <Record as AmbiguousIfSerialize<_>>::check;
        let _ = <Tokens as AmbiguousIfSerialize<_>>::check;
        let _ = <TokenResponse as AmbiguousIfSerialize<_>>::check;
        trait AmbiguousIfDebug<A> { fn check() {} }
        impl<T: ?Sized> AmbiguousIfDebug<()> for T {}
        struct Debugs;
        impl<T: ?Sized + std::fmt::Debug> AmbiguousIfDebug<Debugs> for T {}
        let _ = <Record as AmbiguousIfDebug<_>>::check;
        let _ = <Tokens as AmbiguousIfDebug<_>>::check;
        let _ = <TokenResponse as AmbiguousIfDebug<_>>::check;
    }

    fn response(body: &[u8]) -> HttpResponse {
        HttpResponse {
            status: 200,
            body: SecretBytes::new(body.to_vec()),
        }
    }

    #[test]
    fn escaped_token_fields_deserialize_into_secret_owners() {
        let tokens = parse_tokens(response(br#"{"access_token":"SYNTHETIC\u005fACCESS","refresh_token":"SYNTHETIC\u005fREFRESH","id_token":"SYNTHETIC\u005fID","token_type":"Bearer","scope":"openid chatgpt.tokens.use.direct","expires_in":3600}"#)).unwrap();
        assert!(tokens.access_token.expose_secret() == "SYNTHETIC_ACCESS");
        assert!(tokens.refresh_token.expose_secret() == "SYNTHETIC_REFRESH");
        assert!(tokens.id_token.expose_secret() == "SYNTHETIC_ID");
        assert_eq!(tokens.scopes, ["openid", PLAN_SCOPE]);
    }

    #[test]
    fn duplicate_refresh_and_id_token_fields_and_bad_types_are_refused() {
        for body in [
            r#"{"access_token":"SYNTHETIC","token_type":"Bearer","refresh_token":"ONE","refresh_token":"TWO"}"#,
            r#"{"access_token":"SYNTHETIC","token_type":"Bearer","id_token":"ONE","id_token":"TWO"}"#,
            r#"{"access_token":"SYNTHETIC","token_type":"Bearer","refresh_token":{}}"#,
            r#"{"access_token":"SYNTHETIC","token_type":"Bearer","id_token":[]}"#,
        ] {
            assert!(matches!(
                parse_tokens(response(body.as_bytes())),
                Err(PluginError::InvalidResponse)
            ));
        }
    }
}
