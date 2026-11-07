use super::*;
use crate::auth::Record;
use crate::receipts::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use fb_plugin_api::oauth::verify_pkce;
use fb_plugin_testkit::*;
use ring::{
    rand::SystemRandom,
    signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING},
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    thread,
};
use url::Url;

const NOW: u64 = 1_700_000_000;
const KEY: &str = "sk-synthetic-only-not-a-real-key-1234";
#[derive(Default)]
struct State {
    issuer: String,
    codes: HashMap<String, HashMap<String, String>>,
    next: u64,
    refreshes: usize,
    token_calls: usize,
    duplicate_tokens: bool,
    current_refresh: String,
    token_status: u16,
    models_status: u16,
    revoke_status: u16,
    drop_scope: bool,
    ttl: u64,
    claims_fault: String,
    subject: String,
    response_mode: String,
    response_calls: usize,
    last_response: Option<Value>,
}
struct Provider {
    server: FakeServer,
    state: Arc<Mutex<State>>,
}
fn params(raw: &[u8]) -> HashMap<String, String> {
    url::form_urlencoded::parse(raw)
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}
fn demo() -> Value {
    json!({"vendor_name":"Demo Supply", "vendor_address":["1 Example Way"], "document_number":"SYN-1",
        "document_date":"2023-11-14", "due_date":"2023-12-14", "currency":"USD", "subtotal":"40.00", "tax":"3.20", "total":"43.20",
        "lines":[{"description":"Synthetic supplies", "quantity":"2", "unit_price":"20.00", "amount":"40.00"}]})
}
fn signed_token(key: &EcdsaKeyPair, claims: &Value, fault: &str) -> String {
    let alg = if fault == "alg" { "HS256" } else { "ES256" };
    let header = json!({"alg":alg, "kid":if fault=="kid" {"unknown"} else {"synthetic-key"}});
    let input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap()),
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap())
    );
    let signature = key.sign(&SystemRandom::new(), input.as_bytes()).unwrap();
    let mut bytes = signature.as_ref().to_vec();
    if fault == "signature" {
        bytes[0] ^= 1;
    }
    format!("{input}.{}", URL_SAFE_NO_PAD.encode(bytes))
}
impl Provider {
    fn start() -> Self {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        let public = key.public_key().as_ref();
        let jwks = json!({"keys":[{"kty":"EC","crv":"P-256", "kid":"synthetic-key","use":"sig","alg":"ES256",
            "x":URL_SAFE_NO_PAD.encode(&public[1..33]), "y":URL_SAFE_NO_PAD.encode(&public[33..65])}]});
        let state = Arc::new(Mutex::new(State {
            token_status: 200,
            models_status: 200,
            revoke_status: 200,
            ttl: 3600,
            subject: "synthetic-person".into(),
            ..State::default()
        }));
        let captured = state.clone();
        let server = FakeServer::start(move |req| {
            let mut s = captured.lock().unwrap();
            match (req.method.as_str(), req.target.split('?').next().unwrap()) {
                ("GET", "/authorize") => {
                    let p = params(req.target.split_once('?').unwrap().1.as_bytes());
                    assert_eq!(p.get("resource").map(String::as_str), Some(RESOURCE));
                    assert_eq!(p.get("code_challenge_method").map(String::as_str), Some("S256"));
                    assert!(p["ext_agent_host_id"].starts_with("urn:uuid:"));
                    assert!(Uuid::parse_str(&p["ext_agent_host_id"]).is_ok());
                    let mut p = p;
                    if p["client_id"] == "dynamic_agent_client" {
                        assert_eq!(p.get("agent_name_hint").map(String::as_str),Some("FreedomBooks"));
                        p.insert("client_id".into(),"oaiapp_synthetic".into());
                    }
                    let mut redirect = Url::parse(&p["redirect_uri"]).unwrap();
                    assert_eq!(redirect.host_str(),Some("127.0.0.1"));
                    let code = format!("synthetic-code-{}",s.next);
                    s.next+=1;
                    redirect.query_pairs_mut().append_pair("code",&code).append_pair("state",&p["state"]).append_pair("client_id",&p["client_id"]);
                    s.codes.insert(code,p);
                    FakeResponse::new(302,"").header("Location", redirect.as_str())
                }
                ("POST", "/token") => {
                    s.token_calls += 1;
                    if s.token_status !=200 { return FakeResponse::new(s.token_status,if s.token_status>=500 {r#"{"error":"server_error"}"#} else {r#"{"error":"invalid_grant"}"#}); }
                    let p = params(req.body.expose());
                    assert_eq!(p.get("resource").map(String::as_str),Some(RESOURCE));
                    let nonce = if p.get("grant_type").map(String::as_str)==Some("authorization_code") {
                        let Some(code) = s.codes.remove(&p["code"]) else { return FakeResponse::new(400,r#"{"error":"invalid_grant"}"#); };
                        if p.get("client_id")!=code.get("client_id") || p.get("redirect_uri")!=code.get("redirect_uri") || verify_pkce(&p["code_verifier"],&code["code_challenge"]).is_err() {
                            return FakeResponse::new(400,r#"{"error":"invalid_grant"}"#);
                        }
                        code["nonce"].clone()
                    } else {
                        s.refreshes+=1;
                        if p.get("refresh_token")!=Some(&s.current_refresh) { return FakeResponse::new(400,r#"{"error":"refresh_token_reused"}"#); }
                        "refresh-nonce".into()
                    };
                    let mut claims = json!({"iss":s.issuer,"aud":[p["client_id"]], "sub":s.subject, "email":"synthetic@example.invalid", "nonce":nonce, "exp":NOW+3600, "iat":NOW});
                    match s.claims_fault.as_str() {
                        "issuer" => claims["iss"]=json!("https://wrong.example.invalid"),
                        "audience" => claims["aud"]=json!(["oaiapp_wrong"]),
                        "nonce" => claims["nonce"]=json!("wrong-nonce"),
                        "expired" => claims["exp"]=json!(NOW-61),
                        "future" => claims["iat"]=json!(NOW+61),
                        "subject" => claims["sub"]=json!(""),
                        "missing_iat" => { claims.as_object_mut().unwrap().remove("iat"); },
                        _ => (),
                    }
                    s.current_refresh=format!("FAKE_REFRESH_{}",s.next); s.next+=1;
                    let scopes = if s.drop_scope { "openid email".to_string() } else { SCOPES.join(" ") };
                    let body = json!({"access_token":"FAKE_ACCESS_TOKEN", "refresh_token":s.current_refresh,
                        "token_type":"Bearer", "scope":scopes, "expires_in":s.ttl,
                        "id_token":signed_token(&key,&claims,&s.claims_fault)}).to_string();
                    let body = if s.duplicate_tokens {
                        format!("{{\"access_token\":\"DUPLICATE_SYNTHETIC_TOKEN\",{}", &body[1..])
                    } else { body };
                    FakeResponse::new(200, body)
                }
                ("GET", "/discovery") => FakeResponse::new(200,serde_json::to_vec(&json!({"issuer":s.issuer,"jwks_uri":format!("{}/jwks",s.issuer)})).unwrap()),
                ("GET", "/jwks") => FakeResponse::new(200,serde_json::to_vec(&jwks).unwrap()),
                ("POST", "/revoke") => {
                    let p=params(req.body.expose()); assert_eq!(p.get("token_type_hint").map(String::as_str),Some("refresh_token"));
                    FakeResponse::new(s.revoke_status,"")
                }
                ("GET", "/v1/models") => {
                    if s.models_status!=200 {return FakeResponse::new(s.models_status,r#"{"error":{"message":"SECRET_PROVIDER_MESSAGE"}}"#);}
                    let body = if req.header("Authorization")==Some(&format!("Bearer {KEY}")) {
                        json!({"data":[{"id":"synthetic-key-model"}]})
                    } else if req.header("Authorization")==Some("Bearer FAKE_ACCESS_TOKEN") {
                        json!({"models":[{"slug":"synthetic-model","display_name":"<b>Synthetic</b>\nModel"},{"slug":"hidden","visibility":"hidden"},{"slug":"bad slug"}]})
                    } else { return FakeResponse::new(401,r#"{"error":"unauthorized"}"#); };
                    FakeResponse::new(200,serde_json::to_vec(&body).unwrap())
                }
                ("POST", "/v1/responses") => {
                    s.response_calls+=1;
                    let body:Value=serde_json::from_slice(req.body.expose()).unwrap();
                    assert_eq!(body["store"],false); assert_eq!(body["stream"],true);
                    s.last_response=Some(body);
                    match s.response_mode.as_str() {
                        "401" => FakeResponse::new(401,r#"{"error":"unauthorized"}"#),
                        "429" => FakeResponse::new(429,r#"{"error":{"code":"subscription_sharing_usage_limit_exceeded"}}"#),
                        "failed" => FakeResponse::new(200,"data: {\"type\":\"response.failed\"}\n\n"),
                        "incomplete" => FakeResponse::new(200,"data: {\"type\":\"response.incomplete\"}\n\n"),
                        "truncated" => FakeResponse::new(200,"data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n"),
                        _ => FakeResponse::new(200,format!("data: {}\n\n",json!({"type":"response.completed","response":{"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":demo().to_string()}]}]}}))),
                    }
                }
                _ => FakeResponse::new(404,"")
            }
        }).unwrap();
        state.lock().unwrap().issuer = server.url("");
        Self { server, state }
    }
    fn plugin(&self) -> OpenAi {
        let mut plugin =
            OpenAi::new(Uuid::from_u128(77), vec!["synthetic-key-model".into()]).unwrap();
        plugin.endpoints = Endpoints {
            issuer: self.server.url(""),
            authorize: self.server.url("/authorize"),
            token: self.server.url("/token"),
            revoke: self.server.url("/revoke"),
            discovery: self.server.url("/discovery"),
            api: self.server.url("/v1"),
        };
        plugin
    }
    fn context(&self) -> (PluginContext, Arc<FakeCoreBridge>, Arc<MemoryAuditSink>) {
        let scope = test_scope("openai");
        let core = Arc::new(FakeCoreBridge::new(scope.clone()));
        let audit = Arc::new(MemoryAuditSink::default());
        let cx = test_context(
            scope,
            self.server.http(),
            Arc::new(MemoryCredentialStore::new()),
            core.clone(),
            audit.clone(),
        );
        (cx, core, audit)
    }
    fn approve(&self, pending: &PendingAuthorization) -> SecretString {
        browser_get(pending.authorization_url())
            .unwrap()
            .location
            .unwrap()
    }
    fn connect(&self, plugin: &OpenAi, cx: &PluginContext) -> PluginResult<()> {
        let pending = plugin.start_authorization(cx)?;
        let callback = self.approve(&pending);
        thread::scope(|scope| {
            let waiter = scope.spawn(|| plugin.complete_authorization(cx, pending));
            let response = browser_get(&callback).unwrap();
            assert_eq!(response.status, 200);
            waiter.join().unwrap()
        })
    }
}
fn request(media: &str) -> ReceiptRequest {
    ReceiptRequest {
        attachment: Attachment {
            filename: "PRIVATE-CUSTOMER-FILENAME.pdf".into(),
            media_type: media.into(),
            bytes: SecretBytes::new(b"SYNTHETIC_RECEIPT".to_vec()),
        },
        model: "synthetic-key-model".into(),
        currency: "USD".into(),
        today: Date::new(2023, 11, 14).unwrap(),
    }
}
fn allow(_: &PluginContext) -> PluginResult<()> {
    Ok(())
}

#[test]
fn manifest_and_registration() {
    MANIFEST.validate().unwrap();
    let plugin = Arc::new(OpenAi::new(Uuid::from_u128(77), vec![]).unwrap());
    let mut reg = PluginRegistration::new(plugin.manifest());
    reg.ai_helper = Some(plugin);
    PluginRegistry::new().register(reg).unwrap();
}

#[test]
fn another_scope_can_start_sign_in_while_one_connection_is_busy() {
    let p = Provider::start();
    let plugin = p.plugin();
    let (cx, _, _) = p.context();
    let first_lock = plugin.connection_lock(cx.scope()).unwrap();
    let busy = first_lock.lock().unwrap();
    // This is the same lock a token exchange or refresh holds across network I/O.
    // The provider map lock must have been dropped, and another scope must progress.
    assert!(plugin.lock.try_lock().is_ok());
    let mut other = cx.scope().clone();
    other.company_id = Uuid::from_u128(982);
    let core = Arc::new(FakeCoreBridge::new(other.clone()));
    let other_cx = test_context(other, p.server.http(), Arc::new(MemoryCredentialStore::new()), core, Arc::new(MemoryAuditSink::default()));
    let pending = plugin.start_authorization(&other_cx).unwrap();
    assert!(!pending.authorization_url().expose_secret().is_empty());
    drop(busy);
}
#[test]
fn sign_in_uses_dynamic_registration_and_retains_host_and_client_for_reauth() {
    let p = Provider::start();
    let plugin = p.plugin();
    let (cx, _, audit) = p.context();
    p.connect(&plugin, &cx).unwrap();
    let record = Record::load(&cx).unwrap();
    assert_eq!(record.client_id, "oaiapp_synthetic");
    assert_eq!(record.ext_agent_host_id, plugin.host_id);
    assert!(!record.access_token.expose_secret().is_empty());
    let pending = plugin.start_authorization(&cx).unwrap();
    let u = Url::parse(pending.authorization_url().expose_secret()).unwrap();
    let params = params(u.query().unwrap().as_bytes());
    assert_eq!(params["client_id"], "oaiapp_synthetic");
    assert!(params.contains_key("login_hint"));
    assert!(!params.contains_key("id_token_hint"));
    assert!(!pending.authorization_url().expose_secret().contains(record.id_token.expose_secret()));
    assert!(!params.contains_key("agent_name_hint"));
    assert!(!format!("{pending:?}").contains("synthetic@example.invalid"));
    drop(pending);
    p.connect(&plugin, &cx).unwrap();
    let models = plugin.models(&cx).unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].label, "Synthetic Model");
    let audit = format!("{:?}", audit.events());
    for secret in [
        "FAKE_ACCESS_TOKEN",
        "FAKE_REFRESH_",
        "synthetic@example.invalid",
        "synthetic-person",
        KEY,
    ] {
        assert!(!audit.contains(secret));
    }
}
#[test]
fn missing_and_wrong_registration_ids_fail_closed() {
    let p = Provider::start();
    let plugin = p.plugin();
    let (cx, _, _) = p.context();
    for id in [None, Some("bad-id"), Some("oaiapp_wrong")] {
        let pending = plugin.start_authorization(&cx).unwrap();
        let callback = p.approve(&pending);
        let mut url = Url::parse(callback.expose_secret()).unwrap();
        let pairs: Vec<_> = url
            .query_pairs()
            .filter(|(k, _)| k != "client_id")
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        url.set_query(None);
        url.query_pairs_mut().extend_pairs(pairs);
        if let Some(id) = id {
            url.query_pairs_mut().append_pair("client_id", id);
        }
        thread::scope(|s| {
            let w = s.spawn(|| plugin.complete_authorization(&cx, pending));
            browser_get(&SecretString::from(url.to_string())).unwrap();
            assert!(w.join().unwrap().is_err());
        });
    }
    assert!(cx.credentials().load().unwrap().is_none());
}
#[test]
fn state_nonce_signature_issuer_audience_expiry_iat_subject_and_algorithm_are_checked() {
    for fault in [
        "nonce",
        "signature",
        "issuer",
        "audience",
        "expired",
        "future",
        "subject",
        "missing_iat",
        "alg",
        "kid",
    ] {
        let p = Provider::start();
        let plugin = p.plugin();
        let (cx, _, _) = p.context();
        p.state.lock().unwrap().claims_fault = fault.into();
        assert!(
            p.connect(&plugin, &cx).is_err(),
            "fault {fault} was accepted"
        );
        assert!(cx.credentials().load().unwrap().is_none());
    }
    let p = Provider::start();
    let plugin = p.plugin();
    let (cx, _, _) = p.context();
    let pending = plugin.start_authorization(&cx).unwrap();
    let callback = p.approve(&pending);
    let mut url = Url::parse(callback.expose_secret()).unwrap();
    url.query_pairs_mut().append_pair("state", "duplicate");
    thread::scope(|s| {
        let w = s.spawn(|| plugin.complete_authorization(&cx, pending));
        assert_eq!(
            browser_get(&SecretString::from(url.to_string()))
                .unwrap()
                .status,
            400
        );
        assert_eq!(w.join().unwrap(), Err(PluginError::OAuthCallback));
    });
}
#[test]
fn scope_and_subject_must_match_before_saving() {
    let p = Provider::start();
    let plugin = p.plugin();
    let (cx, _, _) = p.context();
    p.state.lock().unwrap().drop_scope = true;
    assert_eq!(p.connect(&plugin, &cx), Err(PluginError::OAuthDenied));
    assert!(cx.credentials().load().unwrap().is_none());
    p.state.lock().unwrap().drop_scope = false;
    p.connect(&plugin, &cx).unwrap();
    p.state.lock().unwrap().subject = "different-person".into();
    assert_eq!(p.connect(&plugin, &cx), Err(PluginError::OAuthDenied));
    assert_eq!(
        Record::load(&cx).unwrap().subject.expose_secret(),
        "synthetic-person"
    );
}
#[test]
fn key_is_verified_once_and_only_last_four_are_returned() {
    let p = Provider::start();
    let plugin = p.plugin();
    let (cx, _, _) = p.context();
    assert_eq!(
        plugin
            .connect_with_key(&cx, SecretString::from(KEY))
            .unwrap(),
        "1234"
    );
    assert_eq!(plugin.key_last4(&cx).unwrap().as_deref(), Some("1234"));
    assert_eq!(plugin.models(&cx).unwrap()[0].id, "synthetic-key-model");
    for key in [
        "short",
        "sk-key-with space-inside-it-1234",
        "sk-key-with\nnewline-1234",
    ] {
        assert!(plugin
            .connect_with_key(&cx, SecretString::from(key))
            .is_err());
    }
    assert!(plugin
        .connect_with_key(&cx, SecretString::from("sk-incorrect-synthetic-key-6789"))
        .is_err());
    assert_eq!(plugin.key_last4(&cx).unwrap().as_deref(), Some("1234"));
}
#[test]
fn refresh_rotates_and_concurrent_calls_do_not_reuse_it() {
    let p = Provider::start();
    let plugin = Arc::new(p.plugin());
    let (cx, _, _) = p.context();
    p.connect(&plugin, &cx).unwrap();
    let mut record = Record::load(&cx).unwrap();
    record.expires_at = NOW;
    let old = record.refresh_token.clone();
    record.save(&cx).unwrap();
    let cx = Arc::new(cx);
    thread::scope(|s| {
        let mut handles = vec![];
        for _ in 0..8 {
            handles.push(s.spawn(|| plugin.models(&cx).unwrap()));
        }
        for h in handles {
            assert_eq!(h.join().unwrap().len(), 1);
        }
    });
    assert_eq!(p.state.lock().unwrap().refreshes, 1);
    assert!(Record::load(&cx).unwrap().refresh_token.expose_secret() != old.expose_secret());
}
#[test]
fn invalid_refresh_drops_tokens_but_temporary_failure_keeps_them() {
    for status in [400, 401, 503] {
        let p = Provider::start();
        let plugin = p.plugin();
        let (cx, _, _) = p.context();
        p.connect(&plugin, &cx).unwrap();
        let mut record = Record::load(&cx).unwrap();
        record.expires_at = NOW;
        record.save(&cx).unwrap();
        p.state.lock().unwrap().token_status = status;
        assert!(plugin.models(&cx).is_err());
        let after = Record::load(&cx).unwrap();
        assert_eq!(after.client_id, "oaiapp_synthetic");
        assert_eq!(after.access_token.expose_secret().is_empty(), status != 503);
        assert_eq!(
            after.refresh_token.expose_secret().is_empty(),
            status != 503
        );
    }
}
#[test]
fn dropped_plan_scope_after_refresh_sends_nothing() {
    let p = Provider::start();
    let plugin = p.plugin();
    let (cx, _, _) = p.context();
    p.connect(&plugin, &cx).unwrap();
    let mut r = Record::load(&cx).unwrap();
    r.expires_at = NOW;
    r.save(&cx).unwrap();
    p.state.lock().unwrap().drop_scope = true;
    assert!(matches!(
        plugin.read_receipt(&cx, request("image/png"), &allow),
        Err(PluginError::NeedsSignIn)
    ));
    assert_eq!(p.state.lock().unwrap().response_calls, 0);
}
#[test]
fn receipt_consent_is_rechecked_after_refresh() {
    let p = Provider::start();
    let plugin = p.plugin();
    let (cx, _, _) = p.context();
    p.connect(&plugin, &cx).unwrap();
    let mut r = Record::load(&cx).unwrap();
    r.expires_at = NOW;
    r.save(&cx).unwrap();
    let check = |_: &PluginContext| {
        if p.state.lock().unwrap().refreshes == 0 {
            Ok(())
        } else {
            Err(PluginError::Disabled)
        }
    };
    assert!(matches!(
        plugin.read_receipt(&cx, request("image/png"), &check),
        Err(PluginError::Disabled)
    ));
    assert_eq!(p.state.lock().unwrap().response_calls, 0);
}
#[test]
fn disconnect_wipes_even_if_revoke_fails_or_credentials_are_unreadable() {
    for status in [200, 503] {
        let p = Provider::start();
        let plugin = p.plugin();
        let (cx, _, _) = p.context();
        p.connect(&plugin, &cx).unwrap();
        p.state.lock().unwrap().revoke_status = status;
        assert_eq!(plugin.disconnect(&cx).unwrap(), Some(status == 200));
        assert!(cx.credentials().load().unwrap().is_none());
    }
    let p = Provider::start();
    let plugin = p.plugin();
    let (cx, _, _) = p.context();
    cx.credentials()
        .save(SecretBytes::new(b"invalid-json".to_vec()))
        .unwrap();
    plugin.disconnect(&cx).unwrap();
    assert!(cx.credentials().load().unwrap().is_none());
}
#[test]
fn receipt_sends_only_neutral_inline_file_fixed_instruction_and_schema_then_stages_draft() {
    for media in ["application/pdf", "image/png", "image/jpeg", "image/webp"] {
        let p = Provider::start();
        let plugin = p.plugin();
        let (cx, core, audit) = p.context();
        plugin
            .connect_with_key(&cx, SecretString::from(KEY))
            .unwrap();
        let reading = plugin.read_receipt(&cx, request(media), &allow).unwrap();
        assert_eq!(reading.total_cents, 4320);
        assert_eq!(reading.lines.len(), 2);
        let body = p.state.lock().unwrap().last_response.take().unwrap();
        assert_eq!(body["instructions"], INSTRUCTION);
        assert_eq!(
            body["text"]["format"],
            serde_json::from_str::<Value>(TEXT_FORMAT).unwrap()
        );
        assert!(!body.to_string().contains("PRIVATE-CUSTOMER-FILENAME"));
        assert_eq!(body["input"][0]["content"].as_array().unwrap().len(), 2);
        if media == "application/pdf" {
            assert_eq!(body["input"][0]["content"][1]["filename"], "receipt.pdf");
        }
        let draft = plugin
            .stage_receipt_draft(&cx, Uuid::from_u128(500), &reading)
            .unwrap();
        assert!(core
            .calls()
            .iter()
            .any(|c| matches!(c,CoreCall::DraftBill{id} if *id==draft.id)));
        let audit = format!("{:?}", audit.events());
        assert!(!audit.contains("SYNTHETIC_RECEIPT"));
        assert!(!audit.contains(KEY));
    }
}
#[test]
fn failed_incomplete_truncated_limited_and_unauthorized_calls_are_safe() {
    for mode in ["failed", "incomplete", "truncated", "429", "401"] {
        let p = Provider::start();
        let plugin = p.plugin();
        let (cx, core, _) = p.context();
        plugin
            .connect_with_key(&cx, SecretString::from(KEY))
            .unwrap();
        p.state.lock().unwrap().response_mode = mode.into();
        assert!(plugin
            .read_receipt(&cx, request("image/png"), &allow)
            .is_err());
        assert!(!core
            .calls()
            .iter()
            .any(|c| matches!(c, CoreCall::DraftBill { .. })));
        if mode == "401" {
            assert!(cx.credentials().load().unwrap().is_none());
        }
    }
}
#[test]
fn malformed_schema_negative_currency_and_excessive_lines_are_refused() {
    let today = Date::new(2023, 11, 14).unwrap();
    for field in ["total", "tax", "subtotal"] {
        for bad in ["-1", "NaN", "Infinity", "9999999999999999999999"] {
            let mut d = demo();
            d[field] = json!(bad);
            assert!(parse_reply(&d.to_string(), "USD", today).is_err());
        }
    }
    let mut d = demo();
    d["currency"] = json!("EUR");
    assert!(parse_reply(&d.to_string(), "USD", today).is_err());
    let mut d = demo();
    d.as_object_mut().unwrap().remove("total");
    assert!(parse_reply(&d.to_string(), "USD", today).is_err());
    let mut d = demo();
    d["unexpected"] = json!("extra");
    assert!(parse_reply(&d.to_string(), "USD", today).is_err());
    let mut d = demo();
    d["lines"] = json!(vec![demo()["lines"][0].clone(); 50]);
    assert!(parse_reply(&d.to_string(), "USD", today).is_err());
    assert!(parse_reply("not json", "USD", today).is_err());
}
#[test]
fn line_math_is_kept_as_read_and_blocks_draft_staging() {
    let p = Provider::start();
    let plugin = p.plugin();
    let (cx, core, _) = p.context();
    let mut d = demo();
    d["lines"][0]["amount"] = json!("39.00");
    let reading = parse_reply(&d.to_string(), "USD", Date::new(2023, 11, 14).unwrap()).unwrap();
    assert_eq!(reading.lines[0].printed_amount_cents, 3900);
    assert_eq!(reading.lines[0].product_cents, 4000);
    assert!(reading.lines[0].mismatch);
    assert!(matches!(
        plugin.stage_receipt_draft(&cx, Uuid::from_u128(500), &reading),
        Err(PluginError::Core)
    ));
    assert!(core.calls().is_empty());
    assert!(line_matches(10_000, 3900, 3900));
    assert!(!line_matches(20_000, 2000, 3900));
    assert!(line_matches(5_000, 1, 0));
    assert!(line_matches(15_000, 1, 2));
}
#[test]
fn fallback_lines_tax_missing_currency_and_dates_match_reference() {
    let today = Date::new(2023, 11, 14).unwrap();
    let mut d = demo();
    d["lines"] = json!([]);
    d["currency"] = json!("");
    d["document_date"] = json!("1800-01-01");
    let r = parse_reply(&d.to_string(), "USD", today).unwrap();
    assert_eq!(r.lines[0].printed_amount_cents, 4000);
    assert!(r.notes.contains(&ReceiptNote::NoLines));
    assert!(r.notes.contains(&ReceiptNote::CurrencyAssumed));
    assert!(r.document_date.is_none());
    assert!(r.due_date.is_none());
}
#[test]
fn sse_supports_comments_crlf_multiline_data_final_override_and_refusals() {
    let text=b":comment\r\nevent: response.output_text.delta\r\ndata: {\"delta\":\"x\"}\r\n\r\nevent: response.completed\r\ndata: {\"response\":\r\ndata: {\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"final\"}]}]}}\r\n\r\n";
    assert_eq!(protocol::reply_text(text).unwrap().expose_secret(), "final");
    assert!(protocol::reply_text(b"data: {\"type\":\"response.refusal.done\"}\n\n").is_err());
}
#[test]
fn production_http_cannot_contact_loopback_and_scope_mismatch_is_refused() {
    let p = Provider::start();
    let plugin = OpenAi::new(Uuid::from_u128(1), vec![]).unwrap();
    let (cx, _, _) = p.context();
    // A test HTTP policy does not change the plugin's fixed production hosts.
    assert!(matches!(
        plugin.connect_with_key(&cx, SecretString::from(KEY)),
        Err(PluginError::HostNotAllowed)
    ));
    let scope = test_scope("other");
    let core = Arc::new(FakeCoreBridge::new(scope.clone()));
    let cx = test_context(
        scope,
        p.server.http(),
        Arc::new(MemoryCredentialStore::new()),
        core,
        Arc::new(MemoryAuditSink::default()),
    );
    assert!(matches!(plugin.models(&cx), Err(PluginError::WrongScope)));
}

fn refuse_stale_callback(
    p: &Provider,
    plugin: &OpenAi,
    cx: &PluginContext,
    pending: PendingAuthorization,
) {
    let callback = p.approve(&pending);
    thread::scope(|s| {
        let waiter = s.spawn(|| plugin.complete_authorization(cx, pending));
        assert_eq!(browser_get(&callback).unwrap().status, 200);
        assert_eq!(waiter.join().unwrap(), Err(PluginError::OAuthState));
    });
    assert_eq!(
        p.state.lock().unwrap().token_calls,
        0,
        "stale attempt must not exchange its code"
    );
}

#[test]
fn pending_authorization_cannot_reconnect_after_disconnect() {
    let p = Provider::start();
    let plugin = p.plugin();
    let (cx, _, _) = p.context();
    let pending = plugin.start_authorization(&cx).unwrap();
    plugin.disconnect(&cx).unwrap();
    refuse_stale_callback(&p, &plugin, &cx, pending);
    assert!(cx.credentials().load().unwrap().is_none());
}

#[test]
fn newer_authorization_invalidates_previous_callback() {
    let p = Provider::start();
    let plugin = p.plugin();
    let (cx, _, _) = p.context();
    let first = plugin.start_authorization(&cx).unwrap();
    let second = plugin.start_authorization(&cx).unwrap();
    refuse_stale_callback(&p, &plugin, &cx, first);
    assert!(cx.credentials().load().unwrap().is_none());
    let callback = p.approve(&second);
    thread::scope(|s| {
        let waiter = s.spawn(|| plugin.complete_authorization(&cx, second));
        assert_eq!(browser_get(&callback).unwrap().status, 200);
        waiter.join().unwrap().unwrap();
    });
    assert_eq!(p.state.lock().unwrap().token_calls, 1);
    assert_eq!(Record::load(&cx).unwrap().client_id, "oaiapp_synthetic");
}

#[test]
fn api_key_replacement_invalidates_pending_callback() {
    let p = Provider::start();
    let plugin = p.plugin();
    let (cx, _, _) = p.context();
    let pending = plugin.start_authorization(&cx).unwrap();
    plugin
        .connect_with_key(&cx, SecretString::from(KEY))
        .unwrap();
    refuse_stale_callback(&p, &plugin, &cx, pending);
    assert_eq!(plugin.key_last4(&cx).unwrap().as_deref(), Some("1234"));
    assert!(Record::load(&cx).unwrap().client_id.is_empty());
}

#[test]
fn malformed_stored_keys_return_fixed_error_without_panicking_or_network() {
    let p = Provider::start();
    let plugin = p.plugin();
    let (cx, _, _) = p.context();
    for key in [
        "synthetic-malformed-éabc",
        "short",
        "synthetic key with spaces 1234",
        "synthetic-key\n1234",
    ] {
        Record {
            api_key: SecretString::from(key),
            ..Record::default()
        }
        .save(&cx)
        .unwrap();
        assert_eq!(plugin.key_last4(&cx), Err(PluginError::CredentialStore));
        assert!(matches!(
            plugin.models(&cx),
            Err(PluginError::CredentialStore)
        ));
    }
    assert_eq!(p.state.lock().unwrap().token_calls, 0);
}

#[test]
fn duplicate_secret_fields_in_token_exchange_are_refused() {
    let p = Provider::start();
    let plugin = p.plugin();
    let (cx, _, _) = p.context();
    p.state.lock().unwrap().duplicate_tokens = true;
    assert_eq!(p.connect(&plugin, &cx), Err(PluginError::InvalidResponse));
    assert!(cx.credentials().load().unwrap().is_none());
}

#[test]
fn duplicate_secret_fields_in_refresh_are_refused_without_replacing_tokens() {
    let p = Provider::start();
    let plugin = p.plugin();
    let (cx, _, _) = p.context();
    p.connect(&plugin, &cx).unwrap();
    let mut record = Record::load(&cx).unwrap();
    record.expires_at = NOW;
    record.save(&cx).unwrap();
    let old_refresh = record.refresh_token;
    p.state.lock().unwrap().duplicate_tokens = true;
    assert!(matches!(
        plugin.models(&cx),
        Err(PluginError::InvalidResponse)
    ));
    assert!(
        Record::load(&cx).unwrap().refresh_token.expose_secret() == old_refresh.expose_secret()
    );
}

#[test]
fn authorization_generations_are_isolated_by_credential_scope() {
    let p = Provider::start();
    let plugin = p.plugin();
    let (cx, _, _) = p.context();
    let first = plugin.start_authorization(&cx).unwrap();
    let mut other_scope = cx.scope().clone();
    other_scope.company_id = Uuid::from_u128(9191);
    other_scope.connection_id = Uuid::from_u128(9292);
    let other = test_context(
        other_scope.clone(),
        p.server.http(),
        Arc::new(MemoryCredentialStore::new()),
        Arc::new(FakeCoreBridge::new(other_scope)),
        Arc::new(MemoryAuditSink::default()),
    );
    let _other_pending = plugin.start_authorization(&other).unwrap();
    plugin.disconnect(&other).unwrap();
    plugin
        .connect_with_key(&other, SecretString::from(KEY))
        .unwrap();
    let callback = p.approve(&first);
    thread::scope(|s| {
        let waiter = s.spawn(|| plugin.complete_authorization(&cx, first));
        assert_eq!(browser_get(&callback).unwrap().status, 200);
        waiter.join().unwrap().unwrap();
    });
    assert_eq!(p.state.lock().unwrap().token_calls, 1);
    assert_eq!(Record::load(&cx).unwrap().client_id, "oaiapp_synthetic");
    assert_eq!(plugin.key_last4(&other).unwrap().as_deref(), Some("1234"));
}

#[test]
fn lifecycle_uses_callback_grant_and_reports_scoped_status() {
    let p = Provider::start();
    let plugin = p.plugin();
    let (cx, _, _) = p.context();
    let lifecycle = plugin.connection_lifecycle().unwrap();
    let pending = lifecycle
        .begin(
            &cx,
            ConnectInput {
                fields: SecretBytes::new(b"{}".to_vec()),
            },
        )
        .unwrap();
    let callback = browser_get(pending.authorization_url())
        .unwrap()
        .location
        .unwrap();
    let update = thread::scope(|s| {
        let waiter = s.spawn(|| pending.complete(&cx));
        assert_eq!(browser_get(&callback).unwrap().status, 200);
        waiter.join().unwrap().unwrap()
    });
    update.validate_for(&cx).unwrap();
    assert_eq!(update.status, ConnectionStatus::Connected);
    assert_eq!(update.expires_at, Some(Timestamp(NOW + 3600)));
    assert_eq!(Record::load(&cx).unwrap().client_id, "oaiapp_synthetic");
    p.state.lock().unwrap().revoke_status = 500;
    let update = lifecycle.disconnect(&cx).unwrap();
    assert_eq!(update.status, ConnectionStatus::Disconnected);
    assert_eq!(update.revocation, RevocationStatus::Unknown);
    assert!(cx.credentials().load().unwrap().is_none());
}

#[test]
fn cancelled_callback_wait_does_not_exchange_or_save() {
    let p = Provider::start();
    let plugin = p.plugin();
    let (cx, _, _) = p.context();
    let pending = plugin.start_authorization(&cx).unwrap();
    thread::scope(|s| {
        let waiter = s.spawn(|| plugin.complete_authorization(&cx, pending));
        cx.cancellation().cancel();
        assert_eq!(waiter.join().unwrap(), Err(PluginError::Cancelled));
    });
    assert_eq!(p.state.lock().unwrap().token_calls, 0);
}

#[test]
fn usage_limit_is_typed_and_keeps_credentials() {
    let p = Provider::start();
    let plugin = p.plugin();
    let (cx, _, _) = p.context();
    p.connect(&plugin, &cx).unwrap();
    p.state.lock().unwrap().models_status = 429;
    assert!(matches!(plugin.models(&cx), Err(PluginError::UsageLimit)));
    assert!(!Record::load(&cx)
        .unwrap()
        .refresh_token
        .expose_secret()
        .is_empty());
}

#[test]
fn changed_reauthorization_client_id_is_refused_before_exchange() {
    let p = Provider::start();
    let plugin = p.plugin();
    let (cx, _, _) = p.context();
    p.connect(&plugin, &cx).unwrap();
    let pending = plugin.start_authorization(&cx).unwrap();
    let callback = p.approve(&pending);
    let mut url = Url::parse(callback.expose_secret()).unwrap();
    let pairs: Vec<_> = url
        .query_pairs()
        .map(|(k, v)| {
            let v = if k == "client_id" {
                "oaiapp_changed".to_string()
            } else {
                v.into_owned()
            };
            (k.into_owned(), v)
        })
        .collect();
    url.set_query(None);
    url.query_pairs_mut().extend_pairs(pairs);
    thread::scope(|s| {
        let waiter = s.spawn(|| plugin.complete_authorization(&cx, pending));
        browser_get(&SecretString::from(url.to_string())).unwrap();
        assert_eq!(waiter.join().unwrap(), Err(PluginError::OAuthCallback));
    });
    assert_eq!(p.state.lock().unwrap().token_calls, 1);
    assert_eq!(Record::load(&cx).unwrap().client_id, "oaiapp_synthetic");
}
