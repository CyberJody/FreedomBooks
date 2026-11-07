use fb_plugin_api::{HttpPolicy, PluginError, PluginHttp, PluginResult, SecretBytes};
use std::{
    collections::VecDeque,
    fmt,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

/// No Debug: targets, headers and bodies can contain credentials.
pub struct FakeRequest {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: SecretBytes,
}
impl FakeRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}
pub struct FakeResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: SecretBytes,
    pub delay: Duration,
    /// Exercise streaming caps with no Content-Length (connection-close framing).
    pub omit_content_length: bool,
}
impl FakeResponse {
    pub fn new(status: u16, body: impl AsRef<[u8]>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: SecretBytes::new(body.as_ref().to_vec()),
            delay: Duration::ZERO,
            omit_content_length: false,
        }
    }
    pub fn header(mut self, key: &str, value: &str) -> Self {
        self.headers.push((key.to_owned(), value.to_owned()));
        self
    }
}
impl fmt::Debug for FakeResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeResponse")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// Synthetic-data-only numeric-loopback server. Drops stop and join the worker.
pub struct FakeServer {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl FakeServer {
    pub fn start(
        handler: impl Fn(FakeRequest) -> FakeResponse + Send + Sync + 'static,
    ) -> PluginResult<Self> {
        let listener =
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|_| PluginError::Network)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| PluginError::Network)?;
        let address = listener.local_addr().map_err(|_| PluginError::Network)?;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::spawn(move || {
            while !worker_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, peer)) if peer.ip() == Ipv4Addr::LOCALHOST => {
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                        let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                        if let Ok(request) = read_request(&mut stream) {
                            let response = handler(request);
                            let until = Instant::now() + response.delay;
                            while Instant::now() < until && !worker_stop.load(Ordering::Relaxed) {
                                thread::sleep(Duration::from_millis(2));
                            }
                            if !worker_stop.load(Ordering::Relaxed) {
                                let _ = write_response(&mut stream, response);
                            }
                        }
                    }
                    Ok(_) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            address,
            stop,
            worker: Some(worker),
        })
    }
    pub fn address(&self) -> SocketAddr {
        self.address
    }
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.address, path)
    }
    pub fn http(&self) -> PluginHttp {
        PluginHttp::new(
            HttpPolicy::loopback_for_tests(self.address.port()).expect("bound test port"),
        )
    }
}
impl Drop for FakeServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.join().expect("fake server worker panicked");
        }
    }
}
fn read_request(stream: &mut TcpStream) -> PluginResult<FakeRequest> {
    let mut raw = Zeroizing::new(Vec::new());
    while !raw.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        if raw.len() >= 64 * 1024 || stream.read(&mut byte).map_err(|_| PluginError::Network)? == 0
        {
            return Err(PluginError::InvalidResponse);
        }
        raw.push(byte[0]);
    }
    let text = std::str::from_utf8(&raw).map_err(|_| PluginError::InvalidResponse)?;
    let mut lines = text.split("\r\n");
    let mut first = lines.next().ok_or(PluginError::InvalidResponse)?.split(' ');
    let method = first.next().ok_or(PluginError::InvalidResponse)?.to_owned();
    let target = first.next().ok_or(PluginError::InvalidResponse)?.to_owned();
    let headers: Vec<_> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.to_owned(), value.trim().to_owned()))
        .collect();
    let size: usize = headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.parse())
        .transpose()
        .map_err(|_| PluginError::InvalidResponse)?
        .unwrap_or(0);
    if size > 20 * 1024 * 1024 {
        return Err(PluginError::TooLarge);
    }
    let mut body = Zeroizing::new(vec![0; size]);
    stream
        .read_exact(&mut body)
        .map_err(|_| PluginError::Network)?;
    Ok(FakeRequest {
        method,
        target,
        headers,
        body: SecretBytes::new(std::mem::take(&mut *body)),
    })
}
fn write_response(stream: &mut TcpStream, response: FakeResponse) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {} Test\r\nConnection: close\r\n",
        response.status
    )?;
    if !response.omit_content_length {
        write!(
            stream,
            "Content-Length: {}\r\n",
            response.body.expose().len()
        )?;
    }
    for (key, value) in response.headers {
        write!(stream, "{key}: {value}\r\n")?;
    }
    stream.write_all(b"\r\n")?;
    stream.write_all(response.body.expose())
}

pub struct ScriptedExchange {
    pub method: String,
    pub target: String,
    pub response: FakeResponse,
}
struct ScriptState {
    exchanges: VecDeque<ScriptedExchange>,
    unexpected: usize,
}
pub struct ScriptedServer {
    server: FakeServer,
    state: Arc<Mutex<ScriptState>>,
}
impl ScriptedServer {
    pub fn start(exchanges: Vec<ScriptedExchange>) -> PluginResult<Self> {
        let state = Arc::new(Mutex::new(ScriptState {
            exchanges: exchanges.into(),
            unexpected: 0,
        }));
        let captured = state.clone();
        let server = FakeServer::start(move |request| {
            let mut state = captured.lock().expect("test script mutex poisoned");
            match state.exchanges.pop_front() {
                Some(exchange)
                    if request.method == exchange.method && request.target == exchange.target =>
                {
                    exchange.response
                }
                _ => {
                    state.unexpected += 1;
                    FakeResponse::new(500, "Unexpected test request")
                }
            }
        })?;
        Ok(Self { server, state })
    }
    pub fn server(&self) -> &FakeServer {
        &self.server
    }
    pub fn assert_finished(&self) {
        let state = self.state.lock().expect("test script mutex poisoned");
        assert_eq!(
            state.unexpected, 0,
            "unexpected requests (contents intentionally hidden)"
        );
        assert_eq!(
            state.exchanges.len(),
            0,
            "not all scripted requests were made"
        );
    }
}
