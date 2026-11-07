use fb_plugin_api::oauth::*;
use fb_plugin_api::*;
use std::{
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

#[test]
fn pkce_matches_rfc_7636_vector_and_refuses_mismatch() {
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
    assert_eq!(pkce_challenge(verifier).unwrap(), challenge);
    assert_eq!(verify_pkce(verifier, "wrong"), Err(PluginError::OAuthPkce));
    for bad in ["short", &"a".repeat(129), &"/".repeat(43)] {
        assert_eq!(pkce_challenge(bad), Err(PluginError::OAuthPkce));
    }
    let generated = PkcePair::generate().unwrap();
    verify_pkce(generated.verifier.expose_secret(), &generated.challenge).unwrap();
    assert_ne!(
        generated.verifier.expose_secret(),
        PkcePair::generate().unwrap().verifier.expose_secret()
    );
}

#[test]
fn ssrf_blocks_reference_ranges_including_wrappers() {
    for address in [
        "0.0.0.0",
        "0.1.2.3",
        "10.255.255.255",
        "100.64.0.1",
        "100.127.255.255",
        "127.0.0.1",
        "127.255.255.255",
        "169.254.169.254",
        "169.254.170.2",
        "172.16.0.1",
        "172.31.255.255",
        "192.0.0.9",
        "192.0.0.10",
        "192.0.2.1",
        "192.88.99.1",
        "192.168.1.1",
        "198.18.0.1",
        "198.19.255.255",
        "198.51.100.1",
        "203.0.113.1",
        "224.0.0.1",
        "239.255.255.255",
        "240.0.0.1",
        "255.255.255.255",
        "::",
        "::1",
        "::ffff:127.0.0.1",
        "::ffff:169.254.169.254",
        "::ffff:10.0.0.1",
        "::ffff:8.8.8.8",
        "::127.0.0.1",
        "64:ff9b::a00:1",
        "64:ff9b:1::1",
        "100::1",
        "2001::1",
        "2001:db8::1",
        "2002:7f00:1::1",
        "3fff:0::1",
        "fc00::1",
        "fd00:ec2::254",
        "fe80::1",
        "fec0::1",
        "ff02::1",
    ] {
        assert!(
            address_blocked(address.parse::<IpAddr>().unwrap()),
            "must block {address}"
        );
    }
    for address in [
        "8.8.8.8",
        "1.1.1.1",
        "100.128.0.1",
        "172.32.0.1",
        "192.0.1.1",
        "198.20.0.1",
        "2001:4860:4860::8888",
        "2606:4700:4700::1111",
    ] {
        assert!(
            !address_blocked(address.parse().unwrap()),
            "public unicast {address}"
        );
    }
}

#[test]
fn every_dns_answer_is_checked_even_if_first_is_public() {
    let policy = HttpPolicy::new(&["provider.example"]).unwrap();
    let public: SocketAddr = "8.8.8.8:443".parse().unwrap();
    for bad in [
        "10.0.0.1:443",
        "169.254.169.254:443",
        "[::ffff:127.0.0.1]:443",
        "[fc00::1]:443",
        "8.8.8.8:444",
    ] {
        let bad = bad.parse().unwrap();
        assert_eq!(
            policy.validate_resolved("https://provider.example/api", &[public, bad]),
            Err(PluginError::PrivateAddress)
        );
        assert_eq!(
            policy.validate_resolved("https://provider.example/api", &[bad, public]),
            Err(PluginError::PrivateAddress)
        );
    }
    assert_eq!(
        policy.validate_resolved("https://provider.example", &[]),
        Err(PluginError::Network)
    );
    policy
        .validate_resolved("https://provider.example", &[public])
        .unwrap();
}
struct RefusedDns {
    calls: AtomicUsize,
}
impl Resolver for RefusedDns {
    fn resolve(
        &self,
        _host: &str,
        _port: u16,
        _timeout: Duration,
    ) -> PluginResult<Vec<SocketAddr>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec![
            "8.8.8.8:443".parse().unwrap(),
            "169.254.169.254:443".parse().unwrap(),
        ])
    }
}
#[test]
fn http_executes_dns_guard_before_opening_connection() {
    let resolver = Arc::new(RefusedDns {
        calls: AtomicUsize::new(0),
    });
    let http = PluginHttp::with_resolver(
        HttpPolicy::new(&["provider.example"]).unwrap(),
        resolver.clone(),
    );
    #[cfg(feature = "http-client")]
    assert!(matches!(
        http.execute(request("https://provider.example/api")),
        Err(PluginError::PrivateAddress)
    ));
    #[cfg(not(feature = "http-client"))]
    assert!(matches!(
        http.resolve_endpoint("provider.example", 443, Duration::from_secs(1)),
        Err(PluginError::PrivateAddress)
    ));
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
}
#[cfg(feature = "http-client")]
fn request(url: &str) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Get,
        url: SecretString::from(url),
        headers: Vec::new(),
        body: SecretBytes::new(Vec::new()),
    }
}
#[test]
fn http_rejects_insecure_unlisted_userinfo_and_local_urls() {
    let policy = HttpPolicy::new(&["provider.example", "127.0.0.1", "localhost"]).unwrap();
    let http = PluginHttp::new(policy);
    for (url, error) in [
        ("http://provider.example/api", PluginError::HttpsRequired),
        ("https://evil.example/api", PluginError::HostNotAllowed),
        (
            "https://provider.example.evil.example/api",
            PluginError::HostNotAllowed,
        ),
        (
            "https://user:password@provider.example/api",
            PluginError::InvalidConfiguration,
        ),
        (
            "https://provider.example/api#secret",
            PluginError::InvalidConfiguration,
        ),
        ("https://127.0.0.1/api", PluginError::PrivateAddress),
        ("https://localhost/api", PluginError::PrivateAddress),
    ] {
        #[cfg(feature = "http-client")]
        assert!(matches!(http.execute(request(url)), Err(e) if e == error));
        #[cfg(not(feature = "http-client"))]
        assert_eq!(http.policy().validate_url(url), Err(error));
    }
    assert!(HttpPolicy::new(&["*.example"]).is_err());
    assert!(HttpPolicy::new(&["provider.example:443"]).is_err());
    assert!(HttpPolicy::new(&["provider.example"])
        .unwrap()
        .with_limits(Duration::ZERO, 10, 10)
        .is_err());
}
#[test]
fn secrets_and_errors_are_redacted_in_debug() {
    let secret = "SENTINEL_TOP_SECRET";
    let data = SecretBytes::new(secret.as_bytes().to_vec());
    let value = SecretString::from(secret);
    let request = HttpRequest {
        method: HttpMethod::Post,
        url: SecretString::from(format!("https://provider.example/{secret}")),
        headers: vec![("Authorization".to_owned(), SecretString::from(secret))],
        body: SecretBytes::new(secret.as_bytes().to_vec()),
    };
    let response = HttpResponse {
        status: 200,
        body: SecretBytes::new(secret.as_bytes().to_vec()),
    };
    let envelope = EncryptedCredential {
        cipher_blob: SecretBytes::new(secret.as_bytes().to_vec()),
        nonce: vec![1, 2, 3],
        wrapped_key: SecretBytes::new(secret.as_bytes().to_vec()),
    };
    for output in [
        format!("{data:?}"),
        format!("{value:?}"),
        format!("{request:?}"),
        format!("{response:?}"),
        format!("{envelope:?}"),
        format!("{:?}", PluginError::InvalidResponse),
        PluginError::InvalidResponse.to_string(),
    ] {
        assert!(!output.contains(secret));
    }
}
#[test]
fn oauth_rejects_unsafe_endpoints_paths_and_reserved_parameters() {
    assert!(OAuthEndpoints::new(
        "http://provider.example/authorize",
        "https://provider.example/token"
    )
    .is_err());
    assert!(OAuthEndpoints::new(
        "https://user:pass@provider.example/authorize",
        "https://provider.example/token"
    )
    .is_err());
    for path in [
        "//callback",
        "/callback?x=1",
        "/foo/../callback",
        "/%63allback",
        "callback",
        "/callback#x",
        "/callback\\",
    ] {
        let endpoints = OAuthEndpoints::new(
            "https://provider.example/authorize",
            "https://provider.example/token",
        )
        .unwrap();
        let config = OAuthConfig::new(endpoints, "client".into(), vec![], path.into());
        assert!(matches!(
            OAuthLoopback::start(config),
            Err(PluginError::InvalidConfiguration)
        ));
    }
    let endpoints = OAuthEndpoints::new(
        "https://provider.example/authorize",
        "https://provider.example/token",
    )
    .unwrap();
    let mut config = OAuthConfig::new(endpoints, "client".into(), vec![], "/callback".into());
    config
        .extra_authorization_parameters
        .push(("state".into(), "override".into()));
    assert!(matches!(
        OAuthLoopback::start(config),
        Err(PluginError::InvalidConfiguration)
    ));
}
#[test]
fn oauth_listener_is_numeric_ipv4_loopback_and_times_out() {
    let endpoints = OAuthEndpoints::new(
        "https://provider.example/authorize",
        "https://provider.example/token",
    )
    .unwrap();
    let mut config = OAuthConfig::new(endpoints, "client".into(), vec![], "/callback".into());
    config.timeout = Duration::from_millis(20);
    let flow = OAuthLoopback::start(config).unwrap();
    let address = flow.local_addr().unwrap();
    assert_eq!(address.ip().to_string(), "127.0.0.1");
    assert_ne!(address.port(), 0);
    assert!(flow.redirect_uri().starts_with("http://127.0.0.1:"));
    let url = flow.authorization_url().expose_secret().to_owned();
    assert!(!format!("{flow:?}").contains(&url));
    assert!(matches!(flow.wait(), Err(PluginError::Timeout)));
    assert!(std::net::TcpStream::connect_timeout(&address, Duration::from_millis(50)).is_err());
}
#[test]
fn accounting_date_validates_calendar_without_floats() {
    assert!(Date::new(2024, 2, 29).is_ok());
    assert!(Date::new(2023, 2, 29).is_err());
    assert!(Date::new(1900, 2, 29).is_err());
    assert!(Date::new(2000, 2, 29).is_ok());
    for (year, month, day) in [
        (0, 1, 1),
        (2026, 0, 1),
        (2026, 13, 1),
        (2026, 4, 31),
        (2026, 1, 0),
    ] {
        assert!(Date::new(year, month, day).is_err());
    }
}
