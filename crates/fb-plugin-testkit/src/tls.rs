//! TLS fake with ephemeral synthetic trust anchor. Never production credentials.
use fb_plugin_api::*;
use rustls::{pki_types::PrivatePkcs8KeyDer, ServerConfig, ServerConnection, StreamOwned};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TlsObservation {
    pub sni: Option<String>,
    pub host: String,
}
pub struct FakeTlsServer {
    address: SocketAddr,
    root_der: Vec<u8>,
    stop: Arc<AtomicBool>,
    connections: Arc<AtomicUsize>,
    observations: Arc<Mutex<Vec<TlsObservation>>>,
    thread: Option<JoinHandle<()>>,
}
impl FakeTlsServer {
    /// The self-signed leaf is an ephemeral test trust anchor, with exactly the
    /// supplied DNS name. Real rustls verification still checks validity and name.
    pub fn start(certificate_name: &str) -> PluginResult<Self> {
        let certified = rcgen::generate_simple_self_signed(vec![certificate_name.to_owned()])
            .map_err(|_| PluginError::InvalidConfiguration)?;
        let root_der = certified.cert.der().to_vec();
        let config =
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|_| PluginError::Network)?
                .with_no_client_auth()
                .with_single_cert(
                    vec![certified.cert.der().clone()],
                    PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der()).into(),
                )
                .map_err(|_| PluginError::Network)?;
        let config = Arc::new(config);
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|_| PluginError::Network)?;
        let address = listener.local_addr().map_err(|_| PluginError::Network)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| PluginError::Network)?;
        let stop = Arc::new(AtomicBool::new(false));
        let connections = Arc::new(AtomicUsize::new(0));
        let observations = Arc::new(Mutex::new(Vec::new()));
        let (stopping, connected, observed) =
            (stop.clone(), connections.clone(), observations.clone());
        let thread = thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((socket, _)) => {
                        connected.fetch_add(1, Ordering::SeqCst);
                        let _ = socket.set_read_timeout(Some(Duration::from_millis(500)));
                        let _ = socket.set_write_timeout(Some(Duration::from_millis(500)));
                        let Ok(connection) = ServerConnection::new(config.clone()) else {
                            continue;
                        };
                        let mut stream = StreamOwned::new(connection, socket);
                        let mut request = Vec::new();
                        let mut byte = [0u8; 1];
                        while request.len() < 8192 && !request.ends_with(b"\r\n\r\n") {
                            if stream.read_exact(&mut byte).is_err() {
                                break;
                            }
                            request.push(byte[0]);
                        }
                        if !request.ends_with(b"\r\n\r\n") {
                            continue;
                        }
                        let text = String::from_utf8_lossy(&request);
                        let host = text
                            .lines()
                            .filter_map(|l| l.split_once(':'))
                            .find(|(k, _)| k.eq_ignore_ascii_case("host"))
                            .map(|(_, v)| v.trim().to_owned())
                            .unwrap_or_default();
                        observed
                            .lock()
                            .expect("test TLS observations")
                            .push(TlsObservation {
                                sni: stream.conn.server_name().map(str::to_owned),
                                host,
                            });
                        let _ = stream.write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                        );
                        let _ = stream.flush();
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            address,
            root_der,
            stop,
            connections,
            observations,
            thread: Some(thread),
        })
    }
    pub fn address(&self) -> SocketAddr {
        self.address
    }
    pub fn root_der(&self) -> &[u8] {
        &self.root_der
    }
    pub fn observations(&self) -> Vec<TlsObservation> {
        self.observations
            .lock()
            .expect("test TLS observations")
            .clone()
    }
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
    /// Preserve the caller's policy/resolver; only redirect the checked synthetic IP.
    pub fn route(
        &self,
        http: PluginHttp,
        host: &str,
        checked_ip: std::net::IpAddr,
    ) -> PluginResult<PluginHttp> {
        http.with_tls_test_route(
            host,
            SocketAddr::new(checked_ip, self.address.port()),
            self.address,
            &self.root_der,
        )
    }
}
impl Drop for FakeTlsServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.join().expect("TLS fake panicked");
        }
    }
}
