//! The connecting computer's side: pairing, and a client that keeps a session with the host
//! across dropped connections. Reads and sign-out are simply sent again on a new connection; a
//! command is sent again with the **same request id**, so the host answers a command it already
//! committed instead of running it twice.
use crate::pairing::{self, PairReply, PairRequest};
use crate::tls::{self, Identity, Pin};
use fb_remote_proto::frame::{self, FrameError, MAX_REQUEST, MAX_RESPONSE};
use fb_remote_proto::wire::{ClientRequest, HostResponse, RefusalCode};
use rustls::{ClientConfig, ClientConnection, StreamOwned};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

pub const CONNECT: Duration = Duration::from_secs(5);
pub const HANDSHAKE: Duration = Duration::from_secs(10);
/// Longest wait for one answer (a long report or a bank-feed sync on the host).
pub const ANSWER: Duration = Duration::from_secs(180);
/// How long a request keeps trying to reach the host after the connection drops.
pub const RETRY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    /// The address is outside the private-network scope or malformed (the words say which).
    Address(&'static str),
    /// No connection could be made (host off, wrong address, network down).
    Unreachable,
    /// The host's certificate is not the paired one: possibly a different computer or an attack.
    HostMismatch,
    /// The host refused this computer's certificate (not paired, or revoked).
    NotPaired,
    /// Pairing was refused; the words are the host's.
    Pairing(String),
    /// The connection kept dropping for the whole retry window.
    Lost,
    Protocol(String),
}

impl ClientError {
    pub fn code(&self) -> &'static str {
        match self {
            ClientError::Address(_) => "address",
            ClientError::Unreachable => "unreachable",
            ClientError::HostMismatch => "host_mismatch",
            ClientError::NotPaired => "not_paired",
            ClientError::Pairing(_) => "pairing",
            ClientError::Lost => "lost",
            ClientError::Protocol(_) => "protocol",
        }
    }
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Address(m) => f.write_str(m),
            ClientError::Unreachable => f.write_str("The host could not be reached. Check that FreedomBooks is hosting on that computer, that it is on, and that this computer is on the same network or tailnet."),
            ClientError::HostMismatch => f.write_str("The computer at that address is not the host this computer was paired with. Nothing was sent. If the host was reinstalled, pair again; otherwise ask your administrator, because someone may be intercepting the connection."),
            ClientError::NotPaired => f.write_str("The host did not accept this computer. It may not be paired or may have been removed. Ask the host's administrator for a pairing code."),
            ClientError::Pairing(m) => f.write_str(m),
            ClientError::Lost => f.write_str("The connection to the host was lost. Your last action may or may not have reached it; when the connection is back, open the page again and check."),
            ClientError::Protocol(_) => f.write_str("The host answered in a way this version of FreedomBooks does not understand. Check that both computers run the same version."),
        }
    }
}

type Stream = StreamOwned<ClientConnection, TcpStream>;

/// A read or write that failed. In TLS 1.3 the client's handshake ends before the host has checked
/// its certificate, so the host's refusal (an AccessDenied alert) arrives on the first read.
fn dropped(e: &FrameError) -> ClientError {
    match e {
        FrameError::Io(io) if io.to_string().contains("AccessDenied") => ClientError::NotPaired,
        FrameError::Io(_) => ClientError::Unreachable,
        other => ClientError::Protocol(other.to_string()),
    }
}

fn open(addrs: &[SocketAddr], config: Arc<ClientConfig>, verifier: &tls::HostVerifier, expected: Option<Pin>) -> Result<Stream, ClientError> {
    let mut socket = addrs.iter().find_map(|a| TcpStream::connect_timeout(a, CONNECT).ok()).ok_or(ClientError::Unreachable)?;
    socket.set_write_timeout(Some(Duration::from_secs(30))).map_err(|_| ClientError::Unreachable)?;
    let _ = socket.set_nodelay(true);
    let mut tls = ClientConnection::new(config, tls::server_name()).map_err(|e| ClientError::Protocol(e.to_string()))?;
    if let Err(e) = tls::handshake(&mut tls, &mut socket, Instant::now() + HANDSHAKE) {
        if expected.is_some() && verifier.seen().is_some_and(|seen| Some(seen) != expected) {
            return Err(ClientError::HostMismatch);
        }
        let text = e.to_string();
        if text.contains("AccessDenied") {
            return Err(ClientError::NotPaired);
        }
        return Err(ClientError::Unreachable);
    }
    if tls.alpn_protocol() != Some(tls::ALPN) {
        return Err(ClientError::Protocol("no protocol agreement".into()));
    }
    socket.set_read_timeout(Some(ANSWER)).map_err(|_| ClientError::Unreachable)?;
    Ok(StreamOwned::new(tls, socket))
}

pub struct Paired {
    pub identity: Identity,
    pub host_pin: Pin,
    pub device_id: String,
    pub host_name: String,
}

/// Pair this computer with the host at `addr` using the one-time code shown there.
pub fn pair(addr: &str, typed_code: &str, device_name: &str) -> Result<Paired, ClientError> {
    let code = fb_remote_proto::code::normalize(typed_code).ok_or_else(|| ClientError::Pairing("Type the 16-character pairing code shown on the host, for example ABCD-EFGH-JKMN-PQRS.".into()))?;
    let name = crate::registry::clean_name(device_name).ok_or_else(|| ClientError::Pairing("Give this computer a name of up to 60 characters.".into()))?;
    let addrs = crate::scope::resolve(addr).map_err(ClientError::Address)?;
    let identity = Identity::generate(&name).map_err(ClientError::Protocol)?;
    let (config, verifier) = tls::client_config(&identity, None).map_err(ClientError::Protocol)?;
    let mut stream = open(&addrs, config, &verifier, None)?;
    let host_pin = verifier.seen().ok_or_else(|| ClientError::Protocol("no host certificate".into()))?;
    let device_pin = identity.pin();
    let request = PairRequest::Pair { device_name: name, proof: fb_remote_proto::hex(&pairing::client_proof(&code, &host_pin, &device_pin)) };
    frame::send(&mut stream, &request, MAX_REQUEST).map_err(|_| ClientError::Unreachable)?;
    let reply: PairReply = match frame::recv(&mut stream, MAX_REQUEST) {
        Ok(Some(r)) => r,
        Ok(None) | Err(FrameError::Io(_)) => return Err(ClientError::NotPaired),
        Err(e) => return Err(ClientError::Protocol(e.to_string())),
    };
    match reply {
        PairReply::Paired { device_id, proof, host_name } => {
            // The host must prove it knew the code and saw this computer's certificate: otherwise
            // something in between answered, and nothing is pinned.
            let expected = pairing::host_proof(&code, &host_pin, &device_pin);
            if !fb_remote_proto::unhex(&proof).is_some_and(|p| pairing::equal(&p, &expected)) {
                return Err(ClientError::HostMismatch);
            }
            Ok(Paired { identity, host_pin, device_id, host_name })
        }
        PairReply::Refused { message } => Err(ClientError::Pairing(message)),
    }
}

/// Connection state changes, for the connected-host indicator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    Connected,
    Reconnecting,
    Lost,
}

pub struct Client {
    addrs: Vec<SocketAddr>,
    host_pin: Pin,
    config: Arc<ClientConfig>,
    verifier: Arc<tls::HostVerifier>,
    idle: Mutex<Vec<Stream>>,
    session: Mutex<Option<String>>,
    notify: Box<dyn Fn(Link) + Send + Sync>,
    retry: Duration,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// With a pairing code active, a revoked certificate can reach only the pairing
// endpoint. Its reply to Hello is a pairing refusal (without a session refusal code).
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum HelloReply {
    Host(HostResponse),
    Pair(PairReply),
}

enum Attempt {
    Answer(HostResponse),
    /// The connection failed before an answer arrived; the request may or may not have arrived.
    Dropped,
    Fatal(ClientError),
}

impl Client {
    pub fn new(addr: &str, identity: &Identity, host_pin: Pin, notify: Box<dyn Fn(Link) + Send + Sync>) -> Result<Client, ClientError> {
        let addrs = crate::scope::resolve(addr).map_err(ClientError::Address)?;
        let (config, verifier) = tls::client_config(identity, Some(host_pin)).map_err(ClientError::Protocol)?;
        Ok(Client { addrs, host_pin, config, verifier, idle: Mutex::new(vec![]), session: Mutex::new(None), notify, retry: RETRY })
    }

    /// Shorter retry window (tests).
    pub fn with_retry(mut self, retry: Duration) -> Self {
        self.retry = retry;
        self
    }

    pub fn session(&self) -> Option<String> {
        lock(&self.session).clone()
    }

    /// A fresh connection, bound to this client's session (resumed) or to a new one.
    fn connection(&self) -> Result<(Stream, Option<HostResponse>), ClientError> {
        if let Some(s) = lock(&self.idle).pop() {
            return Ok((s, None));
        }
        let mut stream = open(&self.addrs, self.config.clone(), &self.verifier, Some(self.host_pin))?;
        // A pooled connection may have been closed by the host (idle, revocation); a fresh one
        // reports a refusal properly.
        let had = self.session();
        frame::send(&mut stream, &ClientRequest::Hello { resume: had.clone() }, MAX_REQUEST).map_err(|e| dropped(&e))?;
        match frame::recv::<_, HelloReply>(&mut stream, MAX_RESPONSE) {
            Ok(Some(HelloReply::Host(HostResponse::Session { session, signed_in, host_name, resumed }))) => {
                *lock(&self.session) = Some(session.clone());
                // The host no longer has our session (idle expiry, host restart): whoever was
                // signed in is not any more. Say so instead of acting in the new session.
                if had.is_some() && !resumed {
                    return Ok((stream, Some(HostResponse::refused(RefusalCode::Unauthenticated, "Your session on the host ended. Sign in again."))));
                }
                Ok((stream, Some(HostResponse::Session { session, signed_in, host_name, resumed })))
            }
            Ok(Some(HelloReply::Host(HostResponse::Refused { code, message }))) => Ok((stream, Some(HostResponse::Refused { code, message }))),
            Ok(Some(HelloReply::Pair(PairReply::Refused { .. }))) => Err(ClientError::NotPaired),
            Ok(Some(_)) => Err(ClientError::Protocol("unexpected hello reply".into())),
            Ok(None) => Err(ClientError::Unreachable),
            Err(e) => Err(dropped(&e)),
        }
    }

    /// Connect (or reconnect) and return the host's Session answer.
    pub fn hello(&self) -> Result<HostResponse, ClientError> {
        lock(&self.idle).clear();
        let (stream, answer) = self.connection()?;
        lock(&self.idle).push(stream);
        (self.notify)(Link::Connected);
        answer.ok_or_else(|| ClientError::Protocol("no session".into()))
    }

    fn attempt(&self, request: &ClientRequest) -> Attempt {
        let (mut stream, hello) = match self.connection() {
            Ok(c) => c,
            Err(ClientError::Unreachable) => return Attempt::Dropped,
            Err(e) => return Attempt::Fatal(e),
        };
        if let Some(r @ HostResponse::Refused { .. }) = hello {
            lock(&self.idle).push(stream);
            return Attempt::Answer(r);
        }
        let answer = frame::send(&mut stream, request, MAX_REQUEST).and_then(|()| frame::recv::<_, HostResponse>(&mut stream, MAX_RESPONSE));
        match answer {
            Ok(Some(r)) => {
                lock(&self.idle).push(stream);
                Attempt::Answer(r)
            }
            Ok(None) => Attempt::Dropped,
            Err(e) => match dropped(&e) {
                ClientError::Unreachable => Attempt::Dropped,
                fatal => Attempt::Fatal(fatal),
            },
        }
    }

    /// Send one request and wait for its answer, reconnecting while the retry window lasts.
    pub fn call(&self, request: &ClientRequest) -> Result<HostResponse, ClientError> {
        let deadline = Instant::now() + self.retry;
        // Resume promptly on a brief drop, then leave headroom under the host’s one-token-per-three-seconds limit.
        let mut backoff = Duration::from_millis(500);
        let mut dropped = false;
        loop {
            match self.attempt(request) {
                Attempt::Answer(r) => {
                    if dropped {
                        (self.notify)(Link::Connected);
                    }
                    return Ok(r);
                }
                Attempt::Fatal(e) => {
                    (self.notify)(Link::Lost);
                    return Err(e);
                }
                Attempt::Dropped => {
                    lock(&self.idle).clear();
                    if !dropped {
                        dropped = true;
                        (self.notify)(Link::Reconnecting);
                    }
                    if Instant::now() + backoff >= deadline {
                        (self.notify)(Link::Lost);
                        return Err(ClientError::Lost);
                    }
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(Duration::from_secs(5));
                }
            }
        }
    }

    pub fn close(&self) {
        lock(&self.idle).clear();
    }
}
