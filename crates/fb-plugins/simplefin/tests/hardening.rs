//! Ports bankfeeds/test_fix1.py and test_hardening2.py's outside-half cases.
//! No public sockets or live DNS: DNS answers are synthetic, fake sockets are loopback only.
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use fb_plugin_api::*;
use fb_plugin_simplefin::client::{decode_setup_token, ACCOUNTS_TIMEOUT, CLAIM_TIMEOUT};
use fb_plugin_testkit::*;
use std::{
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

fn token(url: &str) -> SecretString {
    SecretString::from(STANDARD.encode(url))
}
fn request(url: &str) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Post,
        url: SecretString::from(url),
        headers: vec![],
        body: SecretBytes::new(vec![]),
    }
}

#[test]
fn token_base64_standard_urlsafe_and_whitespace_are_supported() {
    let policy = HttpPolicy::new(&["bank.example"]).unwrap();
    let raw = "https://bank.example/simplefin/claim/test-token";
    let tokens = [
        STANDARD.encode(raw),
        URL_SAFE_NO_PAD.encode(raw),
        format!(" \n{}\r\n", STANDARD.encode(raw)),
    ];
    for encoded in tokens {
        let decoded = decode_setup_token(&SecretString::from(encoded), &policy).unwrap();
        assert!(decoded.expose_secret() == raw);
        assert!(!format!("{decoded:?}").contains("test-token"));
    }
}

#[test]
fn malformed_tokens_schemes_missing_claim_credentials_fragments_and_controls_fail() {
    let policy = HttpPolicy::new(&["bank.example"]).unwrap();
    for encoded in [
        "".into(),
        "not base64 !!".into(),
        "A".into(),
        "a".repeat(4097),
        " ".repeat(16385),
        STANDARD.encode([255]),
    ] {
        assert!(decode_setup_token(&SecretString::from(encoded), &policy).is_err());
    }
    for raw in [
        "ftp://bank.example/claim/t",
        "http://bank.example/claim/t",
        "https://bank.example/create",
        "https://u:p@bank.example/claim/t",
        "https://bank.example/claim/",
        "https://bank.example/claim//",
        "https://bank.example/claim/t#fragment",
        "https://bank.example:bad/claim/t",
        "https:///claim/t",
        "https://bank.example/claim/\nt",
        "https://bank.example\\@evil.example/claim/t",
    ] {
        assert!(decode_setup_token(&token(raw), &policy).is_err());
    }
}

#[test]
fn claim_host_must_have_explicit_consent_and_no_domain_suffix_or_userinfo_tricks() {
    let policy = HttpPolicy::new(&["bridge.simplefin.org"]).unwrap();
    for raw in [
        "https://bridge.simplefin.org.evil.example/claim/t",
        "https://evil.example/claim/t",
        "https://bridge.simplefin.org@evil.example/claim/t",
        "https://evil.example@bridge.simplefin.org/claim/t",
        "https://bridge.simplefin.org./claim/t",
    ] {
        assert!(decode_setup_token(&token(raw), &policy).is_err());
    }
    assert!(decode_setup_token(&token("https://BRIDGE.SIMPLEFIN.ORG/claim/t"), &policy).is_ok());
}

fn refused(ips: &[&str]) {
    let policy = HttpPolicy::new(&["bank.example"]).unwrap();
    for raw in ips {
        let ip: IpAddr = raw.parse().unwrap();
        assert!(address_blocked(ip), "unblocked {ip}");
        let authority = if ip.is_ipv6() {
            format!("[{ip}]")
        } else {
            ip.to_string()
        };
        let literal_policy = HttpPolicy::new(&[&authority]).unwrap();
        assert!(matches!(
            decode_setup_token(
                &token(&format!("https://{authority}/claim/t")),
                &literal_policy
            ),
            Err(PluginError::PrivateAddress)
        ));
        assert_eq!(
            policy.validate_resolved("https://bank.example/claim/t", &[SocketAddr::new(ip, 443)]),
            Err(PluginError::PrivateAddress)
        );
        assert_eq!(
            policy.validate_resolved(
                "https://bank.example/claim/t",
                &[
                    "93.184.216.34:443".parse().unwrap(),
                    SocketAddr::new(ip, 443)
                ]
            ),
            Err(PluginError::PrivateAddress)
        );
    }
}
#[test]
fn private_metadata_cgnat_loopback_and_multicast_ranges_are_refused() {
    refused(&[
        "127.0.0.1",
        "127.8.9.10",
        "10.0.0.1",
        "172.16.5.4",
        "192.168.1.1",
        "169.254.169.254",
        "100.64.0.1",
        "100.127.255.254",
        "0.0.0.0",
        "224.0.0.1",
        "239.255.255.250",
        "240.0.0.1",
        "255.255.255.255",
        "::1",
        "::",
        "fe80::1",
        "fc00::1",
        "fd12:3456::1",
        "ff02::1",
        "ff0e::1",
        "::127.0.0.1",
    ]);
}
#[test]
fn hardening2_site_local_entire_ietf_block_benchmark_and_documentation() {
    refused(&[
        "fec0::1",
        "fec0:1234::5",
        "feff:ffff:ffff:ffff::1",
        "192.0.0.0",
        "192.0.0.1",
        "192.0.0.9",
        "192.0.0.10",
        "192.0.0.170",
        "192.0.0.255",
        "198.18.0.0",
        "198.18.0.1",
        "198.19.255.254",
        "192.0.2.1",
        "198.51.100.1",
        "203.0.113.1",
        "2001:db8::1",
        "2001:db8:ffff:ffff::1",
        "3fff::1",
    ]);
}
#[test]
fn hardening2_mapped_nat64_six_to_four_teredo_and_local_use_nat64() {
    refused(&[
        "::ffff:127.0.0.1",
        "::ffff:10.0.0.1",
        "::ffff:169.254.169.254",
        "::ffff:100.64.0.1",
        "::ffff:192.0.0.9",
        "::ffff:198.19.0.1",
        "64:ff9b::a00:1",
        "64:ff9b::7f00:1",
        "64:ff9b::c0a8:101",
        "64:ff9b::a9fe:a9fe",
        "64:ff9b::6440:1",
        "64:ff9b::c000:9",
        "64:ff9b::c612:1",
        "64:ff9b:1::1",
        "2002:a00:1::1",
        "2002:c0a8:101::1",
        "2002:a9fe:a9fe::1",
        "2002:7f00:1::",
        "2002:c000:a::1",
        "2001::1",
        "2001:0:4136:e378:8000:63bf:3fff:fdd2",
        "2001:0:ffff:ffff::1",
    ]);
    // W1 is intentionally stricter than Python: NO transition/mapped IPv6, even of public v4.
    refused(&["::ffff:8.8.8.8", "64:ff9b::808:808", "2001:1::1"]);
}
#[test]
fn every_python_blocked_network_is_blocked_at_both_endpoints() {
    for (raw, prefix) in [
        ("0.0.0.0", 8),
        ("10.0.0.0", 8),
        ("100.64.0.0", 10),
        ("127.0.0.0", 8),
        ("169.254.0.0", 16),
        ("172.16.0.0", 12),
        ("192.0.0.0", 24),
        ("192.0.2.0", 24),
        ("192.88.99.0", 24),
        ("192.168.0.0", 16),
        ("198.18.0.0", 15),
        ("198.51.100.0", 24),
        ("203.0.113.0", 24),
        ("224.0.0.0", 4),
        ("240.0.0.0", 4),
    ] {
        let first = u32::from(raw.parse::<std::net::Ipv4Addr>().unwrap());
        let last = first | (u32::MAX >> prefix);
        refused(&[
            &std::net::Ipv4Addr::from(first).to_string(),
            &std::net::Ipv4Addr::from(last).to_string(),
        ]);
    }
    for (raw, prefix) in [
        ("::", 128),
        ("::1", 128),
        ("64:ff9b:1::", 48),
        ("100::", 64),
        ("2001::", 32),
        ("2001:db8::", 32),
        ("2002::", 16),
        ("3fff::", 20),
        ("fc00::", 7),
        ("fe80::", 10),
        ("fec0::", 10),
        ("ff00::", 8),
    ] {
        let first = u128::from(raw.parse::<std::net::Ipv6Addr>().unwrap());
        let last = first
            | if prefix == 128 {
                0
            } else {
                u128::MAX >> prefix
            };
        refused(&[
            &std::net::Ipv6Addr::from(first).to_string(),
            &std::net::Ipv6Addr::from(last).to_string(),
        ]);
    }
}
#[test]
fn public_neighbors_pass_without_making_any_request() {
    for raw in [
        "93.184.216.34",
        "8.8.8.8",
        "1.1.1.1",
        "2606:4700:4700::1111",
        "192.0.1.1",
        "191.255.255.254",
        "198.17.255.254",
        "198.20.0.1",
        "2001:db9::1",
        "2001:200::1",
    ] {
        assert!(!address_blocked(raw.parse().unwrap()));
    }
}
#[test]
fn numeric_legacy_forms_local_names_and_https_loopback_never_use_test_exception() {
    for name in [
        "2130706433",
        "0x7f000001",
        "017700000001",
        "localhost",
        "api.localhost",
        "127.0.0.1",
    ] {
        let policy = HttpPolicy::new(&[name]).unwrap();
        assert!(matches!(
            decode_setup_token(&token(&format!("https://{name}/claim/t")), &policy),
            Err(PluginError::PrivateAddress)
        ));
    }
    let server = FakeServer::start(|_| panic!("must not contact socket")).unwrap();
    assert!(matches!(
        decode_setup_token(
            &token(&server.url("/claim/t")),
            &HttpPolicy::new(&["127.0.0.1"]).unwrap()
        ),
        Err(PluginError::HttpsRequired)
    ));
    assert!(decode_setup_token(&token(&server.url("/claim/t")), server.http().policy()).is_ok());
    let another = FakeServer::start(|_| panic!("wrong socket")).unwrap();
    assert!(decode_setup_token(&token(&another.url("/claim/t")), server.http().policy()).is_err());
}

struct Answers {
    addresses: Vec<SocketAddr>,
    calls: AtomicUsize,
}
impl Resolver for Answers {
    fn resolve(&self, _: &str, _: u16, _: Duration) -> PluginResult<Vec<SocketAddr>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.addresses.clone())
    }
}
#[test]
fn framework_checks_every_dns_answer_before_any_connection() {
    let resolver = Arc::new(Answers {
        addresses: vec![
            "93.184.216.34:443".parse().unwrap(),
            "169.254.169.254:443".parse().unwrap(),
        ],
        calls: AtomicUsize::new(0),
    });
    let http = PluginHttp::with_resolver(
        HttpPolicy::new(&["bank.example"]).unwrap(),
        resolver.clone(),
    );
    assert!(matches!(
        http.execute(request("https://bank.example/claim/t")),
        Err(PluginError::PrivateAddress)
    ));
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        http.execute(request("https://unconsented.example/claim/t")),
        Err(PluginError::HostNotAllowed)
    ));
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
}
#[test]
fn empty_dns_and_scoped_ipv6_are_refused_and_resolver_failure_is_fixed() {
    let policy = HttpPolicy::new(&["bank.example"]).unwrap();
    assert_eq!(
        policy.validate_resolved("https://bank.example/", &[]),
        Err(PluginError::Network)
    );
    assert_eq!(
        policy.validate_resolved(
            "https://bank.example/",
            &["[2606:4700:4700::1111%1]:443".parse().unwrap()]
        ),
        Err(PluginError::PrivateAddress)
    );
    assert_eq!(
        policy.validate_resolved(
            "https://bank.example/",
            &["93.184.216.34:80".parse().unwrap()]
        ),
        Err(PluginError::PrivateAddress)
    );
    let resolver = Arc::new(Answers {
        addresses: vec![],
        calls: AtomicUsize::new(0),
    });
    assert!(matches!(
        PluginHttp::with_resolver(policy, resolver)
            .execute(request("https://bank.example/claim/t")),
        Err(PluginError::Network)
    ));
}
#[test]
fn framework_total_request_deadline_includes_slow_provider_response() {
    let server = FakeServer::start(|_| {
        let mut response = FakeResponse::new(200, "late");
        response.delay = Duration::from_millis(300);
        response
    })
    .unwrap();
    let policy = server
        .http()
        .policy()
        .clone()
        .with_limits(Duration::from_millis(30), 1, 4096)
        .unwrap();
    assert!(matches!(
        PluginHttp::new(policy).execute(request(&server.url("/claim/t"))),
        Err(PluginError::Timeout)
    ));
    assert_eq!(CLAIM_TIMEOUT, Duration::from_secs(30));
    assert_eq!(ACCOUNTS_TIMEOUT, Duration::from_secs(90));
}

#[test]
fn official_bridge_setup_tokens_use_the_production_manifest_without_broadening_trust() {
    let policy = HttpPolicy::new(fb_plugin_simplefin::MANIFEST.network_hosts).unwrap();
    for host in ["bridge.simplefin.org", "beta-bridge.simplefin.org"] {
        let raw = format!("https://{host}/simplefin/claim/synthetic-one-time-token");
        assert!(decode_setup_token(&token(&raw), &policy).is_ok(), "official host {host}");
        assert_eq!(policy.validate_resolved(&raw, &["8.8.8.8:443".parse().unwrap()]), Ok(()));
        assert_eq!(policy.validate_resolved(&raw, &["127.0.0.1:443".parse().unwrap()]), Err(PluginError::PrivateAddress));
        assert_eq!(policy.validate_resolved(&raw, &["8.8.8.8:443".parse().unwrap(), "169.254.169.254:443".parse().unwrap()]), Err(PluginError::PrivateAddress));
    }
    for raw in [
        "http://beta-bridge.simplefin.org/simplefin/claim/test",
        "https://beta-bridge.simplefin.org.evil.example/simplefin/claim/test",
        "https://evil.beta-bridge.simplefin.org/simplefin/claim/test",
        "https://evil.example@beta-bridge.simplefin.org/simplefin/claim/test",
        "https://beta-bridge.simplefin.org@evil.example/simplefin/claim/test",
        "https://beta-bridge.simplefin.org./simplefin/claim/test",
        "https://127.0.0.1/simplefin/claim/test",
    ] {
        assert!(decode_setup_token(&token(raw), &policy).is_err(), "must refuse {raw}");
    }
}
