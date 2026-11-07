//! fb-plugin-api OAuth loopback: the HTTP callback request a browser sends to the local listener
//! (`OAuthLoopback::wait` -> private `read_callback`: request line, Host header, query with
//! `state`, `code`, `error`, `client_id`).
//!
//! Each iteration starts a real loopback listener, connects to it, writes the fuzzer bytes (with a
//! terminating blank line so the reader finishes) and waits for the verdict. Asserts: no panic;
//! the flow never grants a code without the exact state (the fuzzer cannot know the random state,
//! so every result must be an error).
#![no_main]
use libfuzzer_sys::fuzz_target;
use fb_plugin_api::oauth::{OAuthConfig, OAuthEndpoints, OAuthLoopback};
use std::io::Write;
use std::net::TcpStream;
use std::time::Duration;

fuzz_target!(|data: &[u8]| {
    let endpoints = OAuthEndpoints::new("https://auth.example.com/authorize", "https://auth.example.com/token").unwrap();
    let mut config = OAuthConfig::new(endpoints, "client-id".into(), vec!["openid".into()], "/callback".into());
    config.timeout = Duration::from_secs(5);
    let flow = OAuthLoopback::start(config).expect("loopback starts");
    let addr = flow.local_addr().unwrap();
    let mut request = data.to_vec();
    request.extend_from_slice(b"\r\n\r\n");
    let writer = std::thread::spawn(move || {
        if let Ok(mut s) = TcpStream::connect(addr) {
            let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
            let _ = s.write_all(&request);
            let _ = s.flush();
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    let outcome = flow.wait();
    assert!(outcome.is_err(), "a callback without the real state was granted");
    let _ = writer.join();
});
