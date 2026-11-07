use crate::{CancellationToken, PluginError, PluginResult, SecretBytes};
#[cfg(feature = "http-client")]
use reqwest::{
    blocking::Client,
    header::{HeaderMap, HeaderName, HeaderValue},
    Method,
};
#[cfg(feature = "http-client")]
use secrecy::ExposeSecret;
use secrecy::SecretString;
use std::{
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs},
    sync::{mpsc, Arc},
    time::Duration,
};
#[cfg(feature = "http-client")]
use std::{io::Read, time::Instant};
use url::{Host, Url};
#[cfg(feature = "http-client")]
use zeroize::Zeroizing;

/// The host supplies this immutable, per-plugin policy. Exact hostnames, no wildcards.
#[derive(Clone, Debug)]
pub struct HttpPolicy {
    hosts: Vec<String>,
    timeout: Duration,
    max_response_bytes: usize,
    max_request_bytes: usize,
    test_loopback_port: Option<u16>,
}
impl HttpPolicy {
    pub fn new(hosts: &[&str]) -> PluginResult<Self> {
        let mut normalized = Vec::new();
        for host in hosts {
            let parsed = Host::parse(host).map_err(|_| PluginError::InvalidConfiguration)?;
            if host.is_empty()
                || host.contains(['/', '*', '@', '?', '#'])
                || (!matches!(parsed, Host::Ipv6(_)) && host.contains(':'))
            {
                return Err(PluginError::InvalidConfiguration);
            }
            normalized.push(parsed.to_string());
        }
        Ok(Self {
            hosts: normalized,
            timeout: Duration::from_secs(30),
            max_response_bytes: 20 * 1024 * 1024,
            max_request_bytes: 20 * 1024 * 1024,
            test_loopback_port: None,
        })
    }
    pub fn with_limits(
        mut self,
        timeout: Duration,
        request_bytes: usize,
        response_bytes: usize,
    ) -> PluginResult<Self> {
        if timeout.is_zero()
            || request_bytes == 0
            || response_bytes == 0
            || response_bytes == usize::MAX
        {
            return Err(PluginError::InvalidConfiguration);
        }
        self.timeout = timeout;
        self.max_request_bytes = request_bytes;
        self.max_response_bytes = response_bytes;
        Ok(self)
    }
    /// Explicit, compile-time test seam. Only this numeric loopback address and port;
    /// never permits localhost, another interface, a DNS alias, or private networks.
    #[cfg(feature = "test-support")]
    pub fn loopback_for_tests(port: u16) -> PluginResult<Self> {
        if port == 0 {
            return Err(PluginError::InvalidConfiguration);
        }
        let mut policy = Self::new(&["127.0.0.1"])?;
        policy.test_loopback_port = Some(port);
        Ok(policy)
    }
    /// Non-I/O preflight only. execute/resolve_endpoint still validate DNS answers.
    pub fn validate_url(&self, raw: &str) -> PluginResult<()> {
        self.check_url(raw).map(|_| ())
    }
    pub fn limits(&self) -> (Duration, usize, usize) {
        (
            self.timeout,
            self.max_request_bytes,
            self.max_response_bytes,
        )
    }
    fn endpoint_url(&self, host: &str, port: u16) -> PluginResult<String> {
        let normalized = Self::new(&[host])?;
        if port == 0 {
            return Err(PluginError::InvalidConfiguration);
        }
        let scheme = if host == "127.0.0.1" && self.test_loopback_port == Some(port) {
            "http"
        } else {
            "https"
        };
        Ok(format!("{scheme}://{}:{port}/", normalized.hosts[0]))
    }
    /// Protocol-neutral host/port preflight; does not establish TLS or connect.
    pub fn validate_endpoint(&self, host: &str, port: u16) -> PluginResult<()> {
        self.validate_url(&self.endpoint_url(host, port)?)
    }
    fn check_url(&self, raw: &str) -> PluginResult<Url> {
        let url = Url::parse(raw).map_err(|_| PluginError::InvalidConfiguration)?;
        if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
            return Err(PluginError::InvalidConfiguration);
        }
        let host = url.host_str().ok_or(PluginError::InvalidConfiguration)?;
        let test = url.scheme() == "http"
            && host == "127.0.0.1"
            && url.port() == self.test_loopback_port
            && self.test_loopback_port.is_some();
        if !test && url.scheme() != "https" {
            return Err(PluginError::HttpsRequired);
        }
        if !self.hosts.iter().any(|allowed| allowed == host) {
            return Err(PluginError::HostNotAllowed);
        }
        match url.host() {
            Some(Host::Ipv4(ip)) if !test && address_blocked(IpAddr::V4(ip)) => {
                return Err(PluginError::PrivateAddress)
            }
            Some(Host::Ipv6(ip)) if address_blocked(IpAddr::V6(ip)) => {
                return Err(PluginError::PrivateAddress)
            }
            Some(Host::Domain(name)) if name == "localhost" || name.ends_with(".localhost") => {
                return Err(PluginError::PrivateAddress)
            }
            _ => (),
        }
        Ok(url)
    }
    pub fn validate_resolved(&self, url: &str, addresses: &[SocketAddr]) -> PluginResult<()> {
        let url = self.check_url(url)?;
        let port = url
            .port_or_known_default()
            .ok_or(PluginError::InvalidConfiguration)?;
        if addresses.is_empty() {
            return Err(PluginError::Network);
        }
        for address in addresses {
            let test = url.scheme() == "http"
                && url.host_str() == Some("127.0.0.1")
                && self.test_loopback_port == Some(port)
                && address.ip() == IpAddr::V4(Ipv4Addr::LOCALHOST);
            if address.port() != port
                || (!test && address_blocked(address.ip()))
                || matches!(address, SocketAddr::V6(v) if v.scope_id() != 0)
            {
                return Err(PluginError::PrivateAddress);
            }
        }
        Ok(())
    }
}

/// Injectable DNS seam for deterministic rebinding tests. The helper validates *all*
/// answers and reqwest connects only to these pinned addresses, preserving hostname/SNI.
pub trait Resolver: Send + Sync {
    fn resolve(&self, host: &str, port: u16, timeout: Duration) -> PluginResult<Vec<SocketAddr>>;
}
pub struct SystemResolver;
impl Resolver for SystemResolver {
    fn resolve(&self, host: &str, port: u16, timeout: Duration) -> PluginResult<Vec<SocketAddr>> {
        let host = host.to_owned();
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let result = (host.as_str(), port)
                .to_socket_addrs()
                .map(|answers| answers.collect())
                .map_err(|_| PluginError::Network);
            let _ = tx.send(result);
        });
        rx.recv_timeout(timeout).map_err(|_| PluginError::Timeout)?
    }
}

pub struct HttpRequest {
    pub method: HttpMethod,
    pub url: SecretString,
    pub headers: Vec<(String, SecretString)>,
    pub body: SecretBytes,
}
#[derive(Clone, Copy, Debug)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}
impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .finish_non_exhaustive()
    }
}
pub struct HttpResponse {
    pub status: u16,
    pub body: SecretBytes,
}
impl fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}
#[derive(Clone)]
pub struct PluginHttp {
    policy: HttpPolicy,
    #[cfg_attr(not(feature = "http-client"), allow(dead_code))]
    resolver: Arc<dyn Resolver>,
    cancellation: CancellationToken,
    #[cfg(all(feature = "test-support", feature = "http-client"))]
    tls_test_route: Option<TlsTestRoute>,
}
/// Checked addresses for a non-HTTP protocol. Use only these numeric addresses;
/// TLS must authenticate host, never an IP substituted for the original hostname.
pub struct PinnedEndpoint {
    pub host: String,
    pub addresses: Vec<SocketAddr>,
    pub remaining: Duration,
}
#[cfg(all(feature = "test-support", feature = "http-client"))]
#[derive(Clone)]
struct TlsTestRoute {
    host: String,
    checked: SocketAddr,
    loopback: SocketAddr,
    root: reqwest::Certificate,
}
impl PluginHttp {
    pub fn new(policy: HttpPolicy) -> Self {
        Self {
            policy,
            resolver: Arc::new(SystemResolver),
            cancellation: CancellationToken::new(),
            #[cfg(all(feature = "test-support", feature = "http-client"))]
            tls_test_route: None,
        }
    }
    /// Host/test injection only. Providers must not implement their own resolver or client.
    pub fn with_resolver(policy: HttpPolicy, resolver: Arc<dyn Resolver>) -> Self {
        Self {
            policy,
            resolver,
            cancellation: CancellationToken::new(),
            #[cfg(all(feature = "test-support", feature = "http-client"))]
            tls_test_route: None,
        }
    }
    pub(crate) fn with_cancellation(mut self, cancellation: CancellationToken) -> Self {
        self.cancellation = cancellation;
        self
    }
    /// Intersect operation limits with host ceilings, retaining resolver and policy.
    pub fn constrained(
        &self,
        timeout: Duration,
        request_bytes: usize,
        response_bytes: usize,
    ) -> PluginResult<Self> {
        // Validate requests before intersection; a zero/invalid cap is not a default.
        self.policy
            .clone()
            .with_limits(timeout, request_bytes, response_bytes)?;
        let mut next = self.clone();
        next.policy = self.policy.clone().with_limits(
            timeout.min(self.policy.timeout),
            request_bytes.min(self.policy.max_request_bytes),
            response_bytes.min(self.policy.max_response_bytes),
        )?;
        Ok(next)
    }
    /// Intersection, not replacement: a plugin cannot grant itself a new host.
    pub fn restricted_hosts(&self, hosts: &[&str]) -> PluginResult<Self> {
        let requested = HttpPolicy::new(hosts)?;
        let mut next = self.clone();
        next.policy.hosts.retain(|h| requested.hosts.contains(h));
        Ok(next)
    }
    pub fn resolve_endpoint(
        &self,
        host: &str,
        port: u16,
        timeout: Duration,
    ) -> PluginResult<PinnedEndpoint> {
        self.cancellation.check()?;
        if timeout.is_zero() {
            return Err(PluginError::InvalidConfiguration);
        }
        let start = std::time::Instant::now();
        let timeout = timeout.min(self.policy.timeout);
        let raw = self.policy.endpoint_url(host, port)?;
        let url = self.policy.check_url(&raw)?;
        let host = url.host_str().ok_or(PluginError::InvalidConfiguration)?;
        let addresses = self.resolve_url(&url, timeout)?;
        self.policy.validate_resolved(&raw, &addresses)?;
        self.cancellation.check()?;
        let remaining = timeout
            .checked_sub(start.elapsed())
            .filter(|d| !d.is_zero())
            .ok_or(PluginError::Timeout)?;
        Ok(PinnedEndpoint {
            host: host.to_owned(),
            addresses,
            remaining,
        })
    }
    fn resolve_url(&self, url: &Url, timeout: Duration) -> PluginResult<Vec<SocketAddr>> {
        let port = url
            .port_or_known_default()
            .ok_or(PluginError::InvalidConfiguration)?;
        match url.host() {
            Some(Host::Ipv4(ip)) => Ok(vec![SocketAddr::new(IpAddr::V4(ip), port)]),
            Some(Host::Ipv6(ip)) => Ok(vec![SocketAddr::new(IpAddr::V6(ip), port)]),
            _ => self.resolver.resolve(
                url.host_str().ok_or(PluginError::InvalidConfiguration)?,
                port,
                timeout,
            ),
        }
    }
    /// Test-only routing AFTER normal public DNS validation. Never permits a
    /// private resolver answer. Exact synthetic public IP -> exact loopback socket,
    /// retaining URL authority, SNI and certificate-name verification.
    #[cfg(all(feature = "test-support", feature = "http-client"))]
    pub fn with_tls_test_route(
        mut self,
        host: &str,
        checked: SocketAddr,
        loopback: SocketAddr,
        root_der: &[u8],
    ) -> PluginResult<Self> {
        if address_blocked(checked.ip())
            || checked.port() == 0
            || loopback.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST)
            || loopback.port() != checked.port()
            || !matches!(Host::parse(host), Ok(Host::Domain(_)))
        {
            return Err(PluginError::InvalidConfiguration);
        }
        self.policy.validate_endpoint(host, checked.port())?;
        self.tls_test_route = Some(TlsTestRoute {
            host: host.to_owned(),
            checked,
            loopback,
            root: reqwest::Certificate::from_der(root_der)
                .map_err(|_| PluginError::InvalidConfiguration)?,
        });
        Ok(self)
    }
    pub fn policy(&self) -> &HttpPolicy {
        &self.policy
    }
    #[cfg(feature = "http-client")]
    pub fn execute(&self, request: HttpRequest) -> PluginResult<HttpResponse> {
        self.cancellation.check()?;
        let start = Instant::now();
        let url = self.policy.check_url(request.url.expose_secret())?;
        if request.body.expose().len() > self.policy.max_request_bytes {
            return Err(PluginError::TooLarge);
        }
        let host = url.host_str().ok_or(PluginError::InvalidConfiguration)?;
        let addresses = self.resolve_url(&url, self.policy.timeout)?;
        self.policy
            .validate_resolved(request.url.expose_secret(), &addresses)?;
        let remaining = self
            .policy
            .timeout
            .checked_sub(start.elapsed())
            .filter(|d| !d.is_zero())
            .ok_or(PluginError::Timeout)?;
        self.cancellation.check()?;
        let builder = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(remaining)
            .connect_timeout(remaining)
            .resolve_to_addrs(host, &addresses);
        #[cfg(all(feature = "test-support", feature = "http-client"))]
        let builder = if let Some(route) = &self.tls_test_route {
            // No fallback to the internet if the test resolver changes unexpectedly.
            if host != route.host || addresses != [route.checked] || url.scheme() != "https" {
                return Err(PluginError::InvalidConfiguration);
            }
            builder
                .resolve_to_addrs(host, &[route.loopback])
                .add_root_certificate(route.root.clone())
        } else {
            builder
        };
        let client = builder.build().map_err(|_| PluginError::Network)?;
        let mut headers = HeaderMap::new();
        for (name, value) in &request.headers {
            // Do not let callers override the pinned authority or HTTP framing.
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| PluginError::InvalidConfiguration)?;
            if [
                "host",
                "content-length",
                "transfer-encoding",
                "connection",
                "proxy-authorization",
                "proxy-connection",
                "upgrade",
                "trailer",
            ]
            .contains(&name.as_str())
            {
                return Err(PluginError::InvalidConfiguration);
            }
            let mut value = HeaderValue::from_str(value.expose_secret())
                .map_err(|_| PluginError::InvalidConfiguration)?;
            value.set_sensitive(true);
            headers.append(name, value);
        }
        let method = match request.method {
            HttpMethod::Get => Method::GET,
            HttpMethod::Post => Method::POST,
            HttpMethod::Put => Method::PUT,
            HttpMethod::Delete => Method::DELETE,
        };
        self.cancellation.check()?;
        let mut response = client
            .request(method, url)
            .headers(headers)
            .body(request.body.expose().to_vec())
            .send()
            .map_err(transport_error)?;
        if response.status().is_redirection() {
            return Err(PluginError::Redirect);
        }
        if response
            .content_length()
            .is_some_and(|size| size > self.policy.max_response_bytes as u64)
        {
            return Err(PluginError::TooLarge);
        }
        let status = response.status().as_u16();
        let mut body = Zeroizing::new(Vec::new());
        let mut chunk = Zeroizing::new([0u8; 8192]);
        loop {
            self.cancellation.check()?;
            let read = response.read(&mut *chunk).map_err(|_| {
                if start.elapsed() >= self.policy.timeout {
                    PluginError::Timeout
                } else {
                    PluginError::Network
                }
            })?;
            if read == 0 {
                break;
            }
            if body.len().saturating_add(read) > self.policy.max_response_bytes {
                return Err(PluginError::TooLarge);
            }
            body.extend_from_slice(&chunk[..read]);
        }
        self.cancellation.check()?;
        Ok(HttpResponse {
            status,
            body: SecretBytes::new(std::mem::take(&mut *body)),
        })
    }
}
#[cfg(feature = "http-client")]
fn transport_error(error: reqwest::Error) -> PluginError {
    if error.is_timeout() {
        PluginError::Timeout
    } else {
        PluginError::Network
    }
}

/// Conservative public-unicast policy; mirrors the Python guard and additionally
/// refuses transition/mapped IPv6 forms wholesale. Check after DNS, never just a URL.
pub fn address_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let n = u32::from(ip);
            const BLOCKS: &[(u32, u32)] = &[
                (0x00000000, 0xff000000),
                (0x0a000000, 0xff000000),
                (0x64400000, 0xffc00000),
                (0x7f000000, 0xff000000),
                (0xa9fe0000, 0xffff0000),
                (0xac100000, 0xfff00000),
                (0xc0000000, 0xffffff00),
                (0xc0000200, 0xffffff00),
                (0xc0586300, 0xffffff00),
                (0xc0a80000, 0xffff0000),
                (0xc6120000, 0xfffe0000),
                (0xc6336400, 0xffffff00),
                (0xcb007100, 0xffffff00),
                (0xe0000000, 0xf0000000),
                (0xf0000000, 0xf0000000),
            ];
            BLOCKS.iter().any(|(network, mask)| n & mask == *network)
        }
        IpAddr::V6(ip) => {
            let s = ip.segments();
            // Only ordinary global unicast (2000::/3), excluding assignments,
            // Teredo, documentation, 6to4, and the RFC 9637 documentation range.
            s[0] & 0xe000 != 0x2000
                || (s[0] == 0x2001 && (s[1] < 0x0200 || s[1] == 0x0db8))
                || s[0] == 0x2002
                || (s[0] == 0x3fff && s[1] & 0xf000 == 0)
                || ip == Ipv6Addr::UNSPECIFIED
        }
    }
}
