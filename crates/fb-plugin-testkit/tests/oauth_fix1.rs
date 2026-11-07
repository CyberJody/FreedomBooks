use fb_plugin_api::{oauth::*, *};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::{Duration, Instant},
};
use url::Url;

fn flow() -> OAuthLoopback {
    let mut config = OAuthConfig::new(
        OAuthEndpoints::new(
            "https://provider.example/authorize",
            "https://provider.example/token",
        )
        .unwrap(),
        "dynamic_agent_client".into(),
        vec!["openid".into()],
        "/callback".into(),
    );
    config.timeout = Duration::from_secs(2);
    OAuthLoopback::start(config).unwrap()
}
fn callback(extra: &str, valid_state: bool) -> PluginResult<OAuthGrant> {
    let flow = flow();
    let address = flow.local_addr().unwrap();
    let url = Url::parse(flow.authorization_url().expose_secret()).unwrap();
    let state = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    let state = if valid_state { state } else { "wrong".into() };
    let request = format!("GET /callback?code=synthetic-code&state={state}{extra} HTTP/1.1\r\nHost: {address}\r\n\r\n");
    let browser = thread::spawn(move || {
        let mut socket = TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        socket.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        let _ = socket.read_to_string(&mut response);
        assert!(!response.contains("synthetic-code"));
    });
    let result = flow.wait();
    browser.join().unwrap();
    assert!(
        TcpListener::bind(address).is_ok(),
        "listener must close on every path"
    );
    result
}
#[test]
fn dynamic_client_id_is_state_bound_and_requested_id_is_not_overwritten() {
    let grant = callback("&client_id=oaiapp_synthetic", true).unwrap();
    assert_eq!(grant.client_id, "dynamic_agent_client");
    assert_eq!(
        grant.returned_client_id.as_deref(),
        Some("oaiapp_synthetic")
    );
    assert!(grant
        .token_form()
        .expose_secret()
        .contains("client_id=dynamic_agent_client"));
    assert_eq!(callback("", true).unwrap().returned_client_id, None);
    assert!(matches!(
        callback("&client_id=oaiapp_synthetic", false),
        Err(PluginError::OAuthState)
    ));
}
#[test]
fn returned_id_rejects_duplicates_empty_controls_and_denial() {
    for suffix in [
        "&client_id=oaiapp_a&client_id=oaiapp_b",
        "&client_id=",
        "&client_id=oaiapp_%0A",
        "&client%5Fid=oaiapp_a&client_id=oaiapp_b",
    ] {
        assert!(matches!(
            callback(suffix, true),
            Err(PluginError::OAuthCallback)
        ));
    }
    assert!(matches!(
        callback(&format!("&client_id={}", "x".repeat(513)), true),
        Err(PluginError::OAuthCallback)
    ));
    assert!(matches!(
        callback("&client_id=oaiapp_synthetic&error=denied", true),
        Err(PluginError::OAuthDenied)
    ));
}
#[test]
fn cancellation_closes_waiting_and_partial_callback_listeners() {
    for partial in [false, true] {
        let flow = flow();
        let address = flow.local_addr().unwrap();
        let mut socket = partial.then(|| TcpStream::connect(address).unwrap());
        if let Some(socket) = &mut socket {
            socket.write_all(b"GET /callback?").unwrap();
        }
        let cancellation = CancellationToken::new();
        let other = cancellation.clone();
        let waiter = thread::spawn(move || flow.wait_cancellable(&other));
        thread::sleep(Duration::from_millis(40));
        let start = Instant::now();
        cancellation.cancel();
        assert!(matches!(
            waiter.join().unwrap(),
            Err(PluginError::Cancelled)
        ));
        assert!(start.elapsed() < Duration::from_secs(1));
        drop(socket);
        assert!(TcpListener::bind(address).is_ok());
    }
}
#[test]
fn dynamic_port_registration_contract_is_numeric_exact_and_ephemeral() {
    let first = flow();
    let second = flow();
    let a = Url::parse(first.redirect_uri()).unwrap();
    let b = Url::parse(second.redirect_uri()).unwrap();
    assert_eq!(a.scheme(), "http");
    assert_eq!(a.host_str(), Some("127.0.0.1"));
    assert_ne!(a.port(), Some(0));
    assert_ne!(a.port(), b.port());
    assert_eq!(a.path(), "/callback");
    let auth = Url::parse(first.authorization_url().expose_secret()).unwrap();
    assert_eq!(
        auth.query_pairs()
            .find(|(k, _)| k == "redirect_uri")
            .unwrap()
            .1,
        first.redirect_uri()
    );
}
