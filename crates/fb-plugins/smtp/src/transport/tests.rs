use super::*;
use fb_plugin_api::{HttpPolicy, Resolver};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{net::SocketAddr, sync::Arc};
struct FakeResolver {
    result: PluginResult<Vec<SocketAddr>>,
    calls: AtomicUsize,
}
impl FakeResolver {
    fn new(result: PluginResult<Vec<SocketAddr>>) -> Self {
        Self {
            result,
            calls: AtomicUsize::new(0),
        }
    }
}
impl Resolver for FakeResolver {
    fn resolve(&self, host: &str, port: u16, timeout: Duration) -> PluginResult<Vec<SocketAddr>> {
        assert_eq!(host, "smtp.example.test");
        assert_eq!(port, 587);
        assert!(timeout <= crate::DEFAULT_TIMEOUT);
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.result.clone()
    }
}
fn config() -> crate::SmtpConfig {
    crate::SmtpConfig::new(
        "smtp.example.test".into(),
        587,
        Security::StartTls,
        "sender@example.test".into(),
    )
    .unwrap()
}
#[test]
fn dns_answers_are_validated_then_returned_unchanged_for_numeric_pinning() {
    let answers = vec![
        "8.8.8.8:587".parse().unwrap(),
        "[2606:4700:4700::1111]:587".parse().unwrap(),
    ];
    let resolver = Arc::new(FakeResolver::new(Ok(answers.clone())));
    let policy = HttpPolicy::new(&["smtp.example.test"]).unwrap();
    let pinned = resolve_addresses(
        &config(),
        &PluginHttp::with_resolver(policy.clone(), resolver.clone()),
        Instant::now(),
    )
    .unwrap();
    assert_eq!(pinned.addresses, answers);
    assert!(pinned.remaining <= crate::DEFAULT_TIMEOUT);
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
    // No connect occurs in this test; the transport takes this Vec<SocketAddr>, not a hostname.
}
#[test]
fn one_private_dns_answer_refuses_the_entire_resolution() {
    let resolver = Arc::new(FakeResolver::new(Ok(vec![
        "8.8.8.8:587".parse().unwrap(),
        "127.0.0.1:587".parse().unwrap(),
    ])));
    let policy = HttpPolicy::new(&["smtp.example.test"]).unwrap();
    assert_eq!(
        resolve_addresses(
            &config(),
            &PluginHttp::with_resolver(policy.clone(), resolver.clone()),
            Instant::now()
        )
        .err(),
        Some(PluginError::PrivateAddress)
    );
}
#[test]
fn denied_hosts_never_invoke_dns() {
    let resolver = Arc::new(FakeResolver::new(Err(PluginError::Timeout)));
    assert_eq!(
        resolve_addresses(
            &config(),
            &PluginHttp::with_resolver(HttpPolicy::new(&[]).unwrap(), resolver.clone()),
            Instant::now()
        )
        .err(),
        Some(PluginError::HostNotAllowed)
    );
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 0);
}
#[test]
fn dns_timeout_failure_and_empty_answers_stay_fixed_errors() {
    let policy = HttpPolicy::new(&["smtp.example.test"]).unwrap();
    for result in [
        Err(PluginError::Timeout),
        Err(PluginError::Network),
        Ok(vec![]),
    ] {
        let expected = match result {
            Err(error) => error,
            Ok(_) => PluginError::Network,
        };
        let resolver = Arc::new(FakeResolver::new(result));
        assert_eq!(
            resolve_addresses(
                &config(),
                &PluginHttp::with_resolver(policy.clone(), resolver.clone()),
                Instant::now()
            )
            .err(),
            Some(expected)
        );
    }
}
