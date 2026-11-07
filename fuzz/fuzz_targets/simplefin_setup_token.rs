//! SimpleFIN plugin: the setup token a user pastes (`client::decode_setup_token`: base64 of the
//! claim URL, checked against the host policy) and the plugin-api URL policy
//! (`HttpPolicy::validate_url`, `validate_endpoint`) on arbitrary text.
//!
//! Asserts: no panic; an accepted claim URL is https on the allowed host.
#![no_main]
use libfuzzer_sys::fuzz_target;
use fb_plugin_api::{ExposeSecret, HttpPolicy, SecretString};
use fb_plugin_simplefin::client::decode_setup_token;
use std::sync::LazyLock;

static POLICY: LazyLock<HttpPolicy> = LazyLock::new(|| HttpPolicy::new(&["bridge.simplefin.org", "beta-bridge.simplefin.org"]).unwrap());

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data).into_owned();
    if let Ok(url) = decode_setup_token(&SecretString::from(text.clone()), &POLICY) {
        let u = url::Url::parse(url.expose_secret()).expect("accepted claim URL must parse");
        assert_eq!(u.scheme(), "https");
        assert!(POLICY.validate_url(url.expose_secret()).is_ok());
    }
    let _ = POLICY.validate_url(&text);
    let _ = POLICY.validate_endpoint(&text, 443);
    let _ = HttpPolicy::new(&[text.as_str()]);
    if let Ok(host) = url::Host::parse(&text) {
        if let url::Host::Ipv4(ip) = host {
            let _ = fb_plugin_api::address_blocked(ip.into());
        }
    }
    let _ = fb_plugin_api::oauth::pkce_challenge(&text);
    let _ = fb_plugin_api::oauth::verify_pkce(&text, &text);
    let _ = fb_plugin_api::oauth::OAuthEndpoints::new(&text, &text);
});
