//! Provider-neutral desktop authorization. The shell opens `authorization_url()` in
//! the system browser. This helper validates callback state; provider crates must
//! cryptographically verify ID tokens (signature/issuer/audience/expiry), then call
//! `verify_nonce` on the verified claim. Never trust a callback query-string nonce.
use crate::{CancellationToken, PluginError, PluginResult};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{rngs::OsRng, RngCore};
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fmt,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    thread,
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use url::Url;
use zeroize::Zeroizing;

#[cfg(test)]
mod tests;

fn random_secret() -> PluginResult<SecretString> {
    let mut bytes = Zeroizing::new([0u8; 32]);
    OsRng
        .try_fill_bytes(&mut *bytes)
        .map_err(|_| PluginError::InvalidConfiguration)?;
    Ok(SecretString::from(URL_SAFE_NO_PAD.encode(*bytes)))
}
fn equal(left: &[u8], right: &[u8]) -> bool {
    bool::from(left.ct_eq(right))
}

#[derive(Debug)]
pub struct PkcePair {
    pub verifier: SecretString,
    pub challenge: String,
}
impl PkcePair {
    pub fn generate() -> PluginResult<Self> {
        let verifier = random_secret()?;
        let challenge = pkce_challenge(verifier.expose_secret())?;
        Ok(Self {
            verifier,
            challenge,
        })
    }
}
pub fn pkce_challenge(verifier: &str) -> PluginResult<String> {
    if !(43..=128).contains(&verifier.len())
        || !verifier
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-._~".contains(&c))
    {
        return Err(PluginError::OAuthPkce);
    }
    Ok(URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())))
}
pub fn verify_pkce(verifier: &str, challenge: &str) -> PluginResult<()> {
    if !equal(pkce_challenge(verifier)?.as_bytes(), challenge.as_bytes()) {
        return Err(PluginError::OAuthPkce);
    }
    Ok(())
}

pub struct OAuthEndpoints {
    authorization: Url,
    token: Url,
}
impl OAuthEndpoints {
    pub fn new(authorization: &str, token: &str) -> PluginResult<Self> {
        Self::parse(authorization, token, false)
    }
    #[cfg(feature = "test-support")]
    pub fn loopback_for_tests(authorization: &str, token: &str) -> PluginResult<Self> {
        Self::parse(authorization, token, true)
    }
    fn parse(authorization: &str, token: &str, test: bool) -> PluginResult<Self> {
        fn endpoint(raw: &str, test: bool) -> PluginResult<Url> {
            let url = Url::parse(raw).map_err(|_| PluginError::InvalidConfiguration)?;
            let test = test
                && url.scheme() == "http"
                && url.host_str() == Some("127.0.0.1")
                && url.port().is_some_and(|p| p != 0);
            if (!test && url.scheme() != "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
                || url.query().is_some()
            {
                return Err(PluginError::InvalidConfiguration);
            }
            Ok(url)
        }
        Ok(Self {
            authorization: endpoint(authorization, test)?,
            token: endpoint(token, test)?,
        })
    }
}
impl fmt::Debug for OAuthEndpoints {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OAuthEndpoints([REDACTED])")
    }
}
pub struct OAuthConfig {
    pub endpoints: OAuthEndpoints,
    pub client_id: String,
    pub scopes: Vec<String>,
    pub redirect_path: String,
    pub timeout: Duration,
    /// Non-reserved provider parameters, e.g. OpenAI's resource and agent_name_hint.
    /// `OAuthLoopback::start` takes zeroizing ownership before validation and
    /// discards these after URL construction, including on failure. Until then,
    /// callers are responsible for these plain strings and any copies.
    pub extra_authorization_parameters: Vec<(String, String)>,
}
impl OAuthConfig {
    pub fn new(
        endpoints: OAuthEndpoints,
        client_id: String,
        scopes: Vec<String>,
        redirect_path: String,
    ) -> Self {
        Self {
            endpoints,
            client_id,
            scopes,
            redirect_path,
            timeout: Duration::from_secs(600),
            extra_authorization_parameters: Vec::new(),
        }
    }
}

pub struct OAuthLoopback {
    listener: TcpListener,
    config: OAuthConfig,
    state: SecretString,
    nonce: SecretString,
    pkce: PkcePair,
    authorization_url: SecretString,
    redirect_uri: String,
    deadline: Instant,
}
impl fmt::Debug for OAuthLoopback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OAuthLoopback([REDACTED])")
    }
}
impl OAuthLoopback {
    pub fn start(mut config: OAuthConfig) -> PluginResult<Self> {
        // Take the existing allocations, without cloning hint values. The guard
        // wipes keys and values on every early return (and unwind); the pending
        // config never retains them. Keep this before any fallible operation.
        let extra_parameters =
            Zeroizing::new(std::mem::take(&mut config.extra_authorization_parameters));
        if config.client_id.is_empty()
            || config.timeout.is_zero()
            || config.timeout > Duration::from_secs(600)
            || !valid_path(&config.redirect_path)
        {
            return Err(PluginError::InvalidConfiguration);
        }
        const RESERVED: &[&str] = &[
            "response_type",
            "client_id",
            "redirect_uri",
            "scope",
            "state",
            "nonce",
            "code_challenge",
            "code_challenge_method",
        ];
        let mut extras = std::collections::HashSet::new();
        if extra_parameters
            .iter()
            .any(|(key, _)| RESERVED.contains(&key.as_str()) || !extras.insert(key))
        {
            return Err(PluginError::InvalidConfiguration);
        }
        let listener =
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|_| PluginError::Network)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| PluginError::Network)?;
        let port = listener
            .local_addr()
            .map_err(|_| PluginError::Network)?
            .port();
        let redirect_uri = format!("http://127.0.0.1:{port}{}", config.redirect_path);
        let pkce = PkcePair::generate()?;
        let state = random_secret()?;
        let nonce = random_secret()?;
        let mut url = config.endpoints.authorization.clone();
        url.query_pairs_mut()
            .extend_pairs([
                ("response_type", "code"),
                ("client_id", config.client_id.as_str()),
                ("redirect_uri", redirect_uri.as_str()),
                ("scope", config.scopes.join(" ").as_str()),
                ("state", state.expose_secret()),
                ("nonce", nonce.expose_secret()),
                ("code_challenge", pkce.challenge.as_str()),
                ("code_challenge_method", "S256"),
            ])
            .extend_pairs(
                extra_parameters
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str())),
            );
        drop(extra_parameters);
        // Move the URL serialization rather than leaving a plain-string copy
        // behind when the temporary Url is dropped.
        let authorization_url = SecretString::from(String::from(url));
        let deadline = Instant::now() + config.timeout;
        Ok(Self {
            listener,
            config,
            state,
            nonce,
            pkce,
            authorization_url,
            redirect_uri,
            deadline,
        })
    }
    pub fn local_addr(&self) -> PluginResult<SocketAddr> {
        self.listener.local_addr().map_err(|_| PluginError::Network)
    }
    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }
    pub fn authorization_url(&self) -> &SecretString {
        &self.authorization_url
    }
    /// Consumes and closes the listener on a valid callback, cancellation or timeout.
    /// Unrelated local requests (wrong path or missing state) do not end sign-in.
    pub fn wait(self) -> PluginResult<OAuthGrant> {
        self.wait_cancellable(&CancellationToken::new())
    }
    /// Cooperative host cancellation closes even a partially received callback.
    pub fn wait_cancellable(self, cancellation: &CancellationToken) -> PluginResult<OAuthGrant> {
        loop {
            cancellation.check()?;
            if Instant::now() >= self.deadline {
                return Err(PluginError::Timeout);
            }
            match self.listener.accept() {
                Ok((mut stream, peer)) => {
                    if peer.ip() != Ipv4Addr::LOCALHOST {
                        continue;
                    }
                    let result = self.read_callback(&mut stream, cancellation);
                    let (status, message) = if matches!(result, Ok(Some(_))) {
                        ("200 OK", "Sign-in received. You can close this tab.")
                    } else {
                        (
                            "400 Bad Request",
                            "Sign-in could not be completed. Return to FreedomBooks and try again.",
                        )
                    };
                    let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{message}", message.len());
                    let _ = stream.write_all(response.as_bytes());
                    let Some((code, returned_client_id)) = result? else { continue };
                    cancellation.check()?;
                    return Ok(OAuthGrant {
                        code,
                        verifier: self.pkce.verifier,
                        nonce: self.nonce,
                        redirect_uri: self.redirect_uri,
                        token_endpoint: SecretString::from(self.config.endpoints.token.to_string()),
                        client_id: self.config.client_id,
                        returned_client_id,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(_) => return Err(PluginError::Network),
            }
        }
    }
    fn read_callback(
        &self,
        stream: &mut TcpStream,
        cancellation: &CancellationToken,
    ) -> PluginResult<Option<(SecretString, Option<String>)>> {
        let remaining = self
            .deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or(PluginError::Timeout)?;
        stream
            .set_write_timeout(Some(remaining.min(Duration::from_secs(2))))
            .map_err(|_| PluginError::Network)?;
        let mut raw = Zeroizing::new(Vec::new());
        while !raw.ends_with(b"\r\n\r\n") {
            cancellation.check()?;
            let remaining = self
                .deadline
                .checked_duration_since(Instant::now())
                .filter(|d| !d.is_zero())
                .ok_or(PluginError::Timeout)?;
            stream
                .set_read_timeout(Some(remaining.min(Duration::from_millis(25))))
                .map_err(|_| PluginError::Network)?;
            let mut byte = [0u8];
            let size = match stream.read(&mut byte) {
                Ok(size) => size,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    continue
                }
                Err(_) => return Err(PluginError::OAuthCallback),
            };
            if size == 0 || raw.len() >= 16 * 1024 {
                return Err(PluginError::OAuthCallback);
            }
            raw.push(byte[0]);
        }
        let raw = std::str::from_utf8(&raw).map_err(|_| PluginError::OAuthCallback)?;
        let mut lines = raw.split("\r\n");
        let first: Vec<_> = lines.next().unwrap_or_default().split(' ').collect();
        if first.len() != 3
            || first[0] != "GET"
            || first[2] != "HTTP/1.1"
        {
            return Err(PluginError::OAuthCallback);
        }
        if first[1].split('?').next() != Some(self.config.redirect_path.as_str()) {
            return Ok(None);
        }
        let authority = format!("127.0.0.1:{}", self.local_addr()?.port());
        let hosts: Vec<_> = lines
            .filter_map(|line| line.split_once(':'))
            .filter(|(key, _)| key.eq_ignore_ascii_case("host"))
            .map(|(_, value)| value.trim())
            .collect();
        if hosts != [authority.as_str()] {
            return Err(PluginError::OAuthCallback);
        }
        let Some((_, query)) = first[1].split_once('?') else { return Ok(None) };
        let mut params = HashMap::new();
        for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
            if params
                .insert(key.into_owned(), Zeroizing::new(value.into_owned()))
                .is_some()
            {
                return Err(PluginError::OAuthCallback);
            }
        }
        let Some(state) = params.get("state") else { return Ok(None) };
        if !equal(state.as_bytes(), self.state.expose_secret().as_bytes()) {
            return Err(PluginError::OAuthState);
        }
        if params.contains_key("error") {
            return Err(PluginError::OAuthDenied);
        }
        let mut code = params
            .remove("code")
            .filter(|s| !s.is_empty())
            .ok_or(PluginError::OAuthCallback)?;
        let returned_client_id = params
            .remove("client_id")
            .map(|mut id| {
                if id.is_empty() || id.len() > 512 || id.chars().any(char::is_control) {
                    return Err(PluginError::OAuthCallback);
                }
                Ok(std::mem::take(&mut *id))
            })
            .transpose()?;
        Ok(Some((
            SecretString::from(std::mem::take(&mut *code)),
            returned_client_id,
        )))
    }
}
fn valid_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && !path.contains(['?', '#', '%', '\\'])
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-_".contains(&b))
        && !path.split('/').any(|part| part == "." || part == "..")
}

#[derive(Debug)]
pub struct OAuthGrant {
    pub code: SecretString,
    pub verifier: SecretString,
    nonce: SecretString,
    pub redirect_uri: String,
    pub token_endpoint: SecretString,
    /// Originally requested client id; never silently replaced by callback input.
    pub client_id: String,
    /// State-bound callback metadata, NOT authenticated identity. Provider must
    /// validate its format/reauthorization binding and verify token audience.
    pub returned_client_id: Option<String>,
}
impl OAuthGrant {
    pub fn token_form(&self) -> SecretString {
        SecretString::from(
            url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs([
                    ("grant_type", "authorization_code"),
                    ("code", self.code.expose_secret()),
                    ("client_id", self.client_id.as_str()),
                    ("redirect_uri", self.redirect_uri.as_str()),
                    ("code_verifier", self.verifier.expose_secret()),
                ])
                .finish(),
        )
    }
    /// Only pass a nonce from a cryptographically verified ID token. This helper
    /// does not parse or authenticate JWTs. Missing/mismatched OIDC nonce is fatal.
    pub fn verify_nonce(&self, verified_nonce: &str) -> PluginResult<()> {
        if !equal(
            verified_nonce.as_bytes(),
            self.nonce.expose_secret().as_bytes(),
        ) {
            return Err(PluginError::OAuthNonce);
        }
        Ok(())
    }
}
