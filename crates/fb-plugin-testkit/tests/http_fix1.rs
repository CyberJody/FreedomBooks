use fb_plugin_api::*;
use fb_plugin_testkit::*;
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

fn get(url: &str) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Get,
        url: SecretString::from(url),
        headers: vec![],
        body: SecretBytes::new(vec![]),
    }
}
struct SequenceResolver {
    calls: AtomicUsize,
    answers: Vec<Vec<SocketAddr>>,
    timeouts: Mutex<Vec<Duration>>,
}
impl SequenceResolver {
    fn new(answers: Vec<Vec<SocketAddr>>) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            answers,
            timeouts: Mutex::new(vec![]),
        }
    }
}
impl Resolver for SequenceResolver {
    fn resolve(&self, host: &str, port: u16, timeout: Duration) -> PluginResult<Vec<SocketAddr>> {
        assert_eq!(host, "provider.example");
        assert_ne!(port, 0);
        self.timeouts.lock().unwrap().push(timeout);
        self.answers
            .get(self.calls.fetch_add(1, Ordering::SeqCst))
            .cloned()
            .ok_or(PluginError::Network)
    }
}
#[test]
fn constrained_http_intersects_limits_and_hosts_without_losing_resolver() {
    let resolver = Arc::new(SequenceResolver::new(vec![vec!["10.0.0.1:443"
        .parse()
        .unwrap()]]));
    let policy = HttpPolicy::new(&["provider.example", "other.example"])
        .unwrap()
        .with_limits(Duration::from_secs(2), 20, 30)
        .unwrap();
    let original = PluginHttp::with_resolver(policy, resolver.clone());
    let constrained = original
        .constrained(Duration::from_secs(90), 10, 100)
        .unwrap()
        .restricted_hosts(&["provider.example", "unconsented.example"])
        .unwrap();
    assert_eq!(
        constrained.policy().limits(),
        (Duration::from_secs(2), 10, 30)
    );
    assert_eq!(original.policy().limits(), (Duration::from_secs(2), 20, 30));
    for host in ["other.example", "unconsented.example"] {
        assert_eq!(
            constrained
                .policy()
                .validate_url(&format!("https://{host}")),
            Err(PluginError::HostNotAllowed)
        );
    }
    assert!(matches!(
        constrained.execute(get("https://provider.example")),
        Err(PluginError::PrivateAddress)
    ));
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
    assert_eq!(*resolver.timeouts.lock().unwrap(), [Duration::from_secs(2)]);
    assert!(original.constrained(Duration::ZERO, 1, 1).is_err());
    assert!(original.constrained(Duration::from_secs(1), 0, 1).is_err());
    assert!(original
        .constrained(Duration::from_secs(1), 1, usize::MAX)
        .is_err());
}
#[test]
fn constraints_preserve_loopback_seam_and_enforce_smaller_transfer_caps() {
    for streamed in [false, true] {
        let server = FakeServer::start(move |_| {
            let mut response = FakeResponse::new(200, "123456789");
            response.omit_content_length = streamed;
            response
        })
        .unwrap();
        let http = server
            .http()
            .constrained(Duration::from_secs(1), 2, 8)
            .unwrap();
        assert!(matches!(
            http.execute(get(&server.url("/"))),
            Err(PluginError::TooLarge)
        ));
        let mut request = get(&server.url("/"));
        request.body = SecretBytes::new(vec![1, 2, 3]);
        assert!(matches!(http.execute(request), Err(PluginError::TooLarge)));
    }
}
#[test]
fn protocol_neutral_resolution_preserves_injected_resolver_and_deadline() {
    let resolver = Arc::new(SequenceResolver::new(vec![
        vec!["8.8.8.8:587".parse().unwrap()],
        vec!["127.0.0.1:587".parse().unwrap()],
    ]));
    let http = PluginHttp::with_resolver(
        HttpPolicy::new(&["provider.example"]).unwrap(),
        resolver.clone(),
    );
    assert!(http
        .policy()
        .validate_endpoint("provider.example", 587)
        .is_ok());
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 0);
    assert!(http
        .resolve_endpoint("unconsented.example", 587, Duration::from_secs(1))
        .is_err());
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 0);
    let pinned = http
        .resolve_endpoint("provider.example", 587, Duration::from_secs(1))
        .unwrap();
    assert_eq!(pinned.host, "provider.example");
    assert_eq!(
        pinned.addresses,
        ["8.8.8.8:587".parse::<SocketAddr>().unwrap()]
    );
    assert!(pinned.remaining <= Duration::from_secs(1));
    assert!(matches!(
        http.resolve_endpoint("provider.example", 587, Duration::from_secs(1)),
        Err(PluginError::PrivateAddress)
    ));
}
#[test]
fn tls_pinning_keeps_sni_host_and_refuses_second_request_rebinding() {
    let server = FakeTlsServer::start("provider.example").unwrap();
    let port = server.address().port();
    let public = SocketAddr::new("8.8.8.8".parse().unwrap(), port);
    let private = server.address();
    let resolver = Arc::new(SequenceResolver::new(vec![
        vec![public],
        vec![private],
        vec![public, private],
    ]));
    let http = PluginHttp::with_resolver(
        HttpPolicy::new(&["provider.example"]).unwrap(),
        resolver.clone(),
    );
    let http = server
        .route(http, "provider.example", public.ip())
        .unwrap()
        .constrained(Duration::from_secs(2), 100, 100)
        .unwrap();
    let url = format!("https://provider.example:{port}/");
    assert_eq!(http.execute(get(&url)).unwrap().body.expose(), b"ok");
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        server.observations(),
        [TlsObservation {
            sni: Some("provider.example".into()),
            host: format!("provider.example:{port}")
        }]
    );
    for count in [2, 3] {
        assert!(matches!(
            http.execute(get(&url)),
            Err(PluginError::PrivateAddress)
        ));
        assert_eq!(resolver.calls.load(Ordering::SeqCst), count);
        assert_eq!(server.connections(), 1);
    }
}
#[test]
fn tls_name_and_trust_are_actually_verified() {
    for wrong_name in [true, false] {
        let server = FakeTlsServer::start(if wrong_name {
            "wrong.example"
        } else {
            "provider.example"
        })
        .unwrap();
        let unrelated = FakeTlsServer::start("provider.example").unwrap();
        let port = server.address().port();
        let public = SocketAddr::new("8.8.8.8".parse().unwrap(), port);
        let resolver = Arc::new(SequenceResolver::new(vec![vec![public]]));
        let http =
            PluginHttp::with_resolver(HttpPolicy::new(&["provider.example"]).unwrap(), resolver);
        let root = if wrong_name {
            server.root_der()
        } else {
            unrelated.root_der()
        };
        let http = http
            .with_tls_test_route("provider.example", public, server.address(), root)
            .unwrap();
        assert!(matches!(
            http.execute(get(&format!("https://provider.example:{port}/"))),
            Err(PluginError::Network)
        ));
        assert_eq!(server.connections(), 1);
        assert!(server.observations().is_empty());
    }
}
#[test]
fn tls_test_route_cannot_grant_unlisted_hosts_or_private_dns_answers() {
    let server = FakeTlsServer::start("provider.example").unwrap();
    let http = PluginHttp::new(HttpPolicy::new(&["provider.example"]).unwrap());
    assert!(server
        .route(http.clone(), "evil.example", "8.8.8.8".parse().unwrap())
        .is_err());
    assert!(server
        .route(http, "provider.example", "127.0.0.1".parse().unwrap())
        .is_err());
    assert_eq!(server.connections(), 0);
}
