use crate::{FakeRequest, FakeResponse, FakeServer};
use fb_plugin_api::{
    oauth::{verify_pkce, OAuthConfig, OAuthEndpoints},
    ExposeSecret, PluginError, PluginResult, SecretString,
};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{Ipv4Addr, TcpStream},
    sync::{Arc, Mutex},
};
use url::Url;
use zeroize::Zeroizing;

struct CodeRecord {
    client_id: String,
    redirect_uri: String,
    challenge: String,
    nonce: SecretString,
}
#[derive(Default)]
struct ProviderState {
    codes: HashMap<String, CodeRecord>,
    next: u64,
    exchanges: usize,
}
/// A synthetic public-client OAuth provider. Tokens are fake JSON, NOT signed ID
/// tokens; provider plugin tests must separately exercise their real JWT verifier.
pub struct FakeOAuthProvider {
    server: FakeServer,
    state: Arc<Mutex<ProviderState>>,
}
impl FakeOAuthProvider {
    pub fn start() -> PluginResult<Self> {
        let state = Arc::new(Mutex::new(ProviderState::default()));
        let captured = state.clone();
        let server = FakeServer::start(move |request| {
            let mut state = captured.lock().expect("fake OAuth mutex poisoned");
            match (request.method.as_str(), request.target.split('?').next()) {
                ("GET", Some("/authorize")) => authorize(&mut state, request),
                ("POST", Some("/token")) => token(&mut state, request),
                _ => FakeResponse::new(404, "Not found"),
            }
        })?;
        Ok(Self { server, state })
    }
    pub fn config(&self, client_id: &str, path: &str) -> OAuthConfig {
        let endpoints = OAuthEndpoints::loopback_for_tests(
            &self.server.url("/authorize"),
            &self.server.url("/token"),
        )
        .expect("fake endpoints");
        OAuthConfig::new(
            endpoints,
            client_id.to_owned(),
            vec!["openid".to_owned()],
            path.to_owned(),
        )
    }
    pub fn server(&self) -> &FakeServer {
        &self.server
    }
    pub fn exchanges(&self) -> usize {
        self.state
            .lock()
            .expect("fake OAuth mutex poisoned")
            .exchanges
    }
    /// Simulate approval at the provider using real loopback HTTP. Does not yet
    /// deliver the callback, allowing tests to tamper with state/path first.
    pub fn authorization_redirect(
        &self,
        authorization_url: &SecretString,
    ) -> PluginResult<SecretString> {
        let url = Url::parse(authorization_url.expose_secret())
            .map_err(|_| PluginError::InvalidConfiguration)?;
        if url.host_str() != Some("127.0.0.1") || url.port() != Some(self.server.address().port()) {
            return Err(PluginError::WrongScope);
        }
        let response = browser_get(authorization_url)?;
        if response.status != 302 {
            return Err(PluginError::OAuthDenied);
        }
        response.location.ok_or(PluginError::InvalidResponse)
    }
    pub fn drive_browser(&self, authorization_url: &SecretString) -> PluginResult<()> {
        let redirect = self.authorization_redirect(authorization_url)?;
        if browser_get(&redirect)?.status != 200 {
            return Err(PluginError::OAuthDenied);
        }
        Ok(())
    }
}
fn parameters(raw: &[u8]) -> Option<HashMap<String, String>> {
    let mut result = HashMap::new();
    for (key, value) in url::form_urlencoded::parse(raw) {
        if result
            .insert(key.into_owned(), value.into_owned())
            .is_some()
        {
            return None;
        }
    }
    Some(result)
}
fn authorize(state: &mut ProviderState, request: FakeRequest) -> FakeResponse {
    let Some((_, query)) = request.target.split_once('?') else {
        return FakeResponse::new(400, "Missing query");
    };
    let Some(mut p) = parameters(query.as_bytes()) else {
        return FakeResponse::new(400, "Duplicate parameter");
    };
    if p.get("response_type").map(String::as_str) != Some("code")
        || p.get("code_challenge_method").map(String::as_str) != Some("S256")
    {
        return FakeResponse::new(400, "PKCE S256 required");
    }
    let (Some(client_id), Some(redirect_uri), Some(challenge), Some(nonce), Some(csrf)) = (
        p.remove("client_id"),
        p.remove("redirect_uri"),
        p.remove("code_challenge"),
        p.remove("nonce"),
        p.remove("state"),
    ) else {
        return FakeResponse::new(400, "Missing parameter");
    };
    let Ok(mut redirect) = Url::parse(&redirect_uri) else {
        return FakeResponse::new(400, "Invalid callback");
    };
    if redirect.scheme() != "http"
        || redirect.host_str() != Some("127.0.0.1")
        || redirect.port().is_none()
        || redirect.query().is_some()
        || redirect.fragment().is_some()
    {
        return FakeResponse::new(400, "Invalid callback");
    }
    state.next += 1;
    let code = format!("synthetic-code-{}", state.next);
    state.codes.insert(
        code.clone(),
        CodeRecord {
            client_id,
            redirect_uri,
            challenge,
            nonce: SecretString::from(nonce),
        },
    );
    redirect
        .query_pairs_mut()
        .append_pair("code", &code)
        .append_pair("state", &csrf);
    FakeResponse::new(302, "").header("Location", redirect.as_str())
}
fn token(state: &mut ProviderState, request: FakeRequest) -> FakeResponse {
    let Some(mut p) = parameters(request.body.expose()) else {
        return FakeResponse::new(400, r#"{"error":"invalid_request"}"#);
    };
    let (Some(code), Some(mut verifier)) = (p.remove("code"), p.remove("code_verifier")) else {
        return FakeResponse::new(400, r#"{"error":"invalid_grant"}"#);
    };
    let verifier = Zeroizing::new(std::mem::take(&mut verifier));
    let Some(record) = state.codes.remove(&code) else {
        return FakeResponse::new(400, r#"{"error":"invalid_grant"}"#);
    };
    if p.get("grant_type").map(String::as_str) != Some("authorization_code")
        || p.get("client_id") != Some(&record.client_id)
        || p.get("redirect_uri") != Some(&record.redirect_uri)
        || verify_pkce(&verifier, &record.challenge).is_err()
    {
        return FakeResponse::new(400, r#"{"error":"invalid_grant"}"#);
    }
    state.exchanges += 1;
    let bytes = Zeroizing::new(serde_json::to_vec(&serde_json::json!({"access_token": "FAKE_ACCESS_TOKEN", "token_type": "Bearer", "nonce": record.nonce.expose_secret()})).expect("fake token JSON"));
    FakeResponse::new(200, &*bytes).header("Content-Type", "application/json")
}

pub struct BrowserResponse {
    pub status: u16,
    pub location: Option<SecretString>,
}
/// Test browser only: numeric 127.0.0.1, no proxies, no automatic redirects.
pub fn browser_get(url: &SecretString) -> PluginResult<BrowserResponse> {
    let url = Url::parse(url.expose_secret()).map_err(|_| PluginError::InvalidConfiguration)?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.username() != ""
        || url.password().is_some()
    {
        return Err(PluginError::PrivateAddress);
    }
    let port = url.port().ok_or(PluginError::InvalidConfiguration)?;
    let mut stream = TcpStream::connect_timeout(
        &(Ipv4Addr::LOCALHOST, port).into(),
        std::time::Duration::from_secs(2),
    )
    .map_err(|_| PluginError::Network)?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .map_err(|_| PluginError::Network)?;
    stream
        .set_write_timeout(Some(std::time::Duration::from_secs(2)))
        .map_err(|_| PluginError::Network)?;
    let mut target = Zeroizing::new(url.path().to_owned());
    if let Some(query) = url.query() {
        target.push('?');
        target.push_str(query);
    }
    write!(
        stream,
        "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n",
        target.as_str()
    )
    .map_err(|_| PluginError::Network)?;
    let mut raw = Zeroizing::new(Vec::new());
    stream
        .take(64 * 1024)
        .read_to_end(&mut raw)
        .map_err(|_| PluginError::Network)?;
    let text = std::str::from_utf8(&raw).map_err(|_| PluginError::InvalidResponse)?;
    let header = text
        .split("\r\n\r\n")
        .next()
        .ok_or(PluginError::InvalidResponse)?;
    let mut lines = header.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .ok_or(PluginError::InvalidResponse)?
        .parse()
        .map_err(|_| PluginError::InvalidResponse)?;
    let location = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("location"))
        .map(|(_, value)| SecretString::from(value.trim().to_owned()));
    Ok(BrowserResponse { status, location })
}
