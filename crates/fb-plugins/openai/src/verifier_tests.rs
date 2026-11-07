use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use fb_plugin_testkit::*;
use serde_json::{json, Value};
use std::sync::Arc;

fn verifier_context(server: &FakeServer) -> PluginContext {
    let scope = test_scope("openai");
    test_context(
        scope.clone(),
        server.http(),
        Arc::new(MemoryCredentialStore::new()),
        Arc::new(FakeCoreBridge::new(scope)),
        Arc::new(MemoryAuditSink::default()),
    )
}
#[test]
fn rs256_signature_is_verified_against_testkit_jwks_with_injected_time() {
    let fixture: Value = serde_json::from_str(include_str!("../tests/rsa-identity.json")).unwrap();
    let jwks = fixture["jwks"].clone();
    let server = FakeServer::start(move |request| match request.target.as_str() {
        "/discovery" => FakeResponse::new(200, serde_json::to_vec(&json!({"issuer":"https://auth.openai.com","jwks_uri":"https://unlisted.example.invalid/jwks"})).unwrap()),
        "/jwks" => FakeResponse::new(200, serde_json::to_vec(&jwks).unwrap()),
        _ => FakeResponse::new(404, ""),
    }).unwrap();
    let fixture_jwks = fixture["jwks"].clone();
    let uri = Arc::new(std::sync::Mutex::new(String::new()));
    let u = uri.clone();
    let metadata = FakeServer::start(move |request| {
        let response = if request.target == "/discovery" {
            json!({"issuer":"https://auth.openai.com","jwks_uri":u.lock().unwrap().clone()})
        } else {
            fixture_jwks.clone()
        };
        FakeResponse::new(200, serde_json::to_vec(&response).unwrap())
    })
    .unwrap();
    *uri.lock().unwrap() = metadata.url("/jwks");
    let cx = verifier_context(&metadata);
    let mut plugin = OpenAi::new(Uuid::from_u128(77), vec![]).unwrap();
    plugin.endpoints.discovery = metadata.url("/discovery");
    let token = SecretString::from(fixture["token"].as_str().unwrap().to_string());
    let claims = plugin
        .verify_identity(&cx, &token, "oaiapp_fixture")
        .unwrap();
    assert_eq!(claims["sub"], "synthetic-person");
    assert!(plugin.verify_identity(&cx, &token, "oaiapp_wrong").is_err());
    let mut pieces: Vec<_> = token.expose_secret().split('.').map(String::from).collect();
    let mut sig = URL_SAFE_NO_PAD.decode(&pieces[2]).unwrap();
    sig[0] ^= 1;
    pieces[2] = URL_SAFE_NO_PAD.encode(sig);
    assert!(plugin
        .verify_identity(&cx, &SecretString::from(pieces.join(".")), "oaiapp_fixture")
        .is_err());
    // Discovery cannot smuggle a new allowlisted host into the context.
    plugin.endpoints.discovery = server.url("/discovery");
    assert!(plugin
        .verify_identity(&verifier_context(&server), &token, "oaiapp_fixture")
        .is_err());
}

#[test]
fn extreme_line_products_are_refused_without_panicking_or_wrapping() {
    assert!(!receipts::line_matches(i64::MAX, i64::MAX, 0));
    assert!(!receipts::line_matches(-1, 100, 0));
    assert!(!receipts::line_matches(10_000, -100, -100));
    assert!(!receipts::line_matches(0, 100, 0));
}

#[test]
fn sse_does_not_accept_empty_malformed_or_refused_completion() {
    for body in [
        "", "data: [DONE]\n\n", "data: not-json\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{}}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"refusal\"}]}]}}\n\n",
    ] { assert!(protocol::reply_text(body.as_bytes()).is_err()); }
}
