//! Fake providers for the desktop plugin tests and the Electron smoke (numeric loopback only):
//! the SimpleFIN Bridge (claim + accounts) and an OpenAI issuer (authorize, token with a signed
//! id_token, discovery, JWKS, models, revoke). Never a real network, sign-in or bank.
#![allow(dead_code)]
use base64::{Engine, engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD}};
use fb_plugin_api::oauth::verify_pkce;
use fb_plugin_testkit::{FakeResponse, FakeServer};
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

// --- the fake SimpleFIN Bridge ---------------------------------------------------------------------

pub struct Bridge {
    pub user: String,
    pub pass: String,
    pub accounts: Value,
    pub claimed: bool,
    pub requests: Vec<String>,
}
pub struct FakeBridge {
    pub server: FakeServer,
    pub state: Arc<Mutex<Bridge>>,
}
impl FakeBridge {
    pub fn start(user: &str, pass: &str, accounts: Value) -> Self {
        let state = Arc::new(Mutex::new(Bridge { user: user.into(), pass: pass.into(), accounts, claimed: false, requests: vec![] }));
        let captured = state.clone();
        let server = FakeServer::start(move |request| {
            let mut s = captured.lock().unwrap();
            s.requests.push(format!("{} {}", request.method, request.target.split('?').next().unwrap_or("")));
            let host = request.header("Host").unwrap_or("").to_owned();
            if request.method == "POST" && request.target.starts_with("/claim/") {
                if s.claimed {
                    return FakeResponse::new(403, "");
                }
                s.claimed = true;
                let enc = |v: &str| v.replace('/', "%2F");
                return FakeResponse::new(200, format!("http://{}:{}@{host}/simplefin", enc(&s.user), enc(&s.pass)));
            }
            if request.method == "GET" && request.target.starts_with("/simplefin/accounts") {
                let expected = format!("Basic {}", STANDARD.encode(format!("{}:{}", s.user, s.pass)));
                if request.header("Authorization") != Some(expected.as_str()) {
                    return FakeResponse::new(403, "");
                }
                return FakeResponse::new(200, s.accounts.to_string()).header("Content-Type", "application/json");
            }
            FakeResponse::new(404, "")
        })
        .unwrap();
        Self { server, state }
    }
    pub fn port(&self) -> String {
        self.server.address().port().to_string()
    }
    pub fn setup_token(&self) -> String {
        STANDARD.encode(self.server.url("/claim/SYNTHETIC-ONE-TIME-CLAIM"))
    }
    pub fn reset(&self, user: &str, pass: &str, accounts: Value) {
        let mut s = self.state.lock().unwrap();
        *s = Bridge { user: user.into(), pass: pass.into(), accounts, claimed: false, requests: vec![] };
    }
}

pub fn sample_accounts() -> Value {
    let posted = now() - 2 * 86_400;
    json!({"accounts":[{"id":"ACT-checking","name":"Synthetic Checking","conn_id":"CON-1","currency":"USD","balance":"1250.40","balance-date":posted,
        "transactions":[
            {"id":"TX-1","posted":posted,"transacted_at":posted - 3600,"amount":"-42.75","description":"FUEL STOP"},
            {"id":"TX-2","posted":posted,"amount":"125.00","description":"DEPOSIT MOBILE"}]}],
        "connections":[{"conn_id":"CON-1","name":"Synthetic Bank"}]})
}

// --- the fake OpenAI provider ------------------------------------------------------------------------

pub struct Code {
    pub client_id: String,
    pub redirect_uri: String,
    pub challenge: String,
    pub nonce: String,
}
#[derive(Default)]
pub struct Provider {
    pub issuer: String,
    pub codes: HashMap<String, Code>,
    pub next: u64,
    pub revoked: usize,
}
pub fn params(raw: &str) -> HashMap<String, String> {
    url::form_urlencoded::parse(raw.as_bytes()).map(|(k, v)| (k.into_owned(), v.into_owned())).collect()
}
pub fn fake_openai() -> (FakeServer, Arc<Mutex<Provider>>) {
    let rng = SystemRandom::new();
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
    let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng).unwrap();
    let public = key.public_key().as_ref().to_vec();
    let jwks = json!({"keys":[{"kty":"EC","crv":"P-256","kid":"synthetic-key","use":"sig","alg":"ES256",
        "x":URL_SAFE_NO_PAD.encode(&public[1..33]),"y":URL_SAFE_NO_PAD.encode(&public[33..65])}]});
    let state = Arc::new(Mutex::new(Provider::default()));
    let captured = state.clone();
    let server = FakeServer::start(move |req| {
        let mut s = captured.lock().unwrap();
        let (path, query) = req.target.split_once('?').unwrap_or((req.target.as_str(), ""));
        match (req.method.as_str(), path) {
            ("GET", "/authorize") => {
                let p = params(query);
                assert!(!p.contains_key("id_token_hint"), "F-M6: no id_token_hint in the browser URL");
                let client = if p["client_id"] == "dynamic_agent_client" { "oaiapp_synthetic".to_owned() } else { p["client_id"].clone() };
                let code = format!("synthetic-code-{}", s.next);
                s.next += 1;
                let mut redirect = url::Url::parse(&p["redirect_uri"]).unwrap();
                redirect.query_pairs_mut().append_pair("code", &code).append_pair("state", &p["state"]).append_pair("client_id", &client);
                s.codes.insert(code, Code { client_id: client, redirect_uri: p["redirect_uri"].clone(), challenge: p["code_challenge"].clone(), nonce: p["nonce"].clone() });
                FakeResponse::new(302, "").header("Location", redirect.as_str())
            }
            ("POST", "/token") => {
                let p = params(std::str::from_utf8(req.body.expose()).unwrap());
                let Some(code) = s.codes.remove(&p["code"]) else { return FakeResponse::new(400, r#"{"error":"invalid_grant"}"#) };
                if p["client_id"] != code.client_id || p["redirect_uri"] != code.redirect_uri || verify_pkce(&p["code_verifier"], &code.challenge).is_err() {
                    return FakeResponse::new(400, r#"{"error":"invalid_grant"}"#);
                }
                let now = now();
                let claims = json!({"iss":s.issuer,"aud":[code.client_id],"sub":"synthetic-person","email":"synthetic@example.invalid","nonce":code.nonce,"exp":now+3600,"iat":now});
                let header = json!({"alg":"ES256","kid":"synthetic-key"});
                let input = format!("{}.{}", URL_SAFE_NO_PAD.encode(header.to_string()), URL_SAFE_NO_PAD.encode(claims.to_string()));
                let signature = key.sign(&SystemRandom::new(), input.as_bytes()).unwrap();
                let id_token = format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature.as_ref()));
                let body = json!({"access_token":"FAKE_ACCESS_TOKEN_SYNTHETIC","refresh_token":"FAKE_REFRESH_TOKEN_SYNTHETIC","token_type":"Bearer",
                    "scope":"openid profile email offline_access resource.invoke chatgpt.tokens.use.direct","expires_in":3600,"id_token":id_token});
                FakeResponse::new(200, body.to_string())
            }
            ("GET", "/discovery") => FakeResponse::new(200, json!({"issuer":s.issuer,"jwks_uri":format!("{}/jwks",s.issuer)}).to_string()),
            ("GET", "/jwks") => FakeResponse::new(200, jwks.to_string()),
            ("GET", "/v1/models") => {
                if req.header("Authorization") != Some("Bearer FAKE_ACCESS_TOKEN_SYNTHETIC") {
                    return FakeResponse::new(401, r#"{"error":"unauthorized"}"#);
                }
                FakeResponse::new(200, json!({"models":[{"slug":"synthetic-model","display_name":"Synthetic Model"}]}).to_string())
            }
            ("POST", "/revoke") => {
                s.revoked += 1;
                FakeResponse::new(200, "")
            }
            _ => FakeResponse::new(404, ""),
        }
    })
    .unwrap();
    state.lock().unwrap().issuer = server.url("");
    (server, state)
}
