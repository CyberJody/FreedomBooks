//! Minimal in-memory SMTP provider; TLS certificates are generated for this test only.
use crate::Security;
use base64::{engine::general_purpose::STANDARD, Engine};
use fb_plugin_api::SecretBytes;
use lettre::transport::smtp::client::Certificate;
use rustls::{
    pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
    ServerConfig, ServerConnection, StreamOwned,
};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Behavior {
    Accept,
    LoginOnly,
    NoStartTls,
    RejectStartTls,
    RejectAuth,
    TemporaryAuth,
    RejectRecipient,
    RejectData,
    MalformedGreeting,
    HugeGreeting,
    Oversized(ReplyStage),
    OversizedMultiline(ReplyStage),
    StallGreeting,
    StallTls,
    AuthWrongCode,
    FinalWrongCode,
    StallAuth,
    StallData,
    StallFinal,
    SlowGreeting,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReplyStage {
    Greeting,
    Ehlo,
    StartTls,
    EncryptedEhlo,
    Auth,
    Data,
    Final,
}
#[derive(Default)]
pub(super) struct Observed {
    pub connections: usize,
    pub overflow_peer_closed: bool,
    pub overflow_bytes: usize,
    pub auth: usize,
    pub encrypted_auth: usize,
    pub valid_credentials: bool,
    pub starttls: usize,
    pub mail: usize,
    pub recipients: usize,
    pub message: Option<SecretBytes>,
}
pub(super) struct FakeSmtp {
    pub address: SocketAddr,
    pub certificate: Certificate,
    pub seen: Arc<Mutex<Observed>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl FakeSmtp {
    pub fn start(security: Security, behavior: Behavior) -> Self {
        let certified =
            rcgen::generate_simple_self_signed(vec!["smtp.example.test".into()]).unwrap();
        let cert = certified.cert.der().clone();
        let certificate = Certificate::from_der(cert.to_vec()).unwrap();
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            certified.signing_key.serialize_der(),
        ));
        let provider = rustls::crypto::ring::default_provider();
        let tls = Arc::new(
            ServerConfig::builder_with_provider(Arc::new(provider))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(vec![cert], key)
                .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let seen = Arc::new(Mutex::new(Observed::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let observed = seen.clone();
        let stopping = stop.clone();
        let worker = thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((socket, _)) => {
                        observed.lock().unwrap().connections += 1;
                        socket
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        socket
                            .set_write_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        // Disconnects/certificate rejection/timeouts are expected test outcomes.
                        let _ = serve(socket, tls, security, behavior, &observed);
                        break;
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            address,
            certificate,
            seen,
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for FakeSmtp {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}
enum Wire {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ServerConnection, TcpStream>>),
}
impl Wire {
    fn upgrade(self, tls: Arc<ServerConfig>) -> io::Result<Self> {
        match self {
            Self::Plain(socket) => Ok(Self::Tls(Box::new(StreamOwned::new(
                ServerConnection::new(tls).map_err(io::Error::other)?,
                socket,
            )))),
            Self::Tls(_) => Err(io::Error::other("already encrypted")),
        }
    }
    fn encrypted(&self) -> bool {
        matches!(self, Self::Tls(_))
    }
}
impl Read for Wire {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(s) => s.read(buf),
            Self::Tls(s) => s.read(buf),
        }
    }
}
impl Write for Wire {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(s) => s.write(buf),
            Self::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(s) => s.flush(),
            Self::Tls(s) => s.flush(),
        }
    }
}
fn line(wire: &mut Wire) -> io::Result<String> {
    let mut value = Vec::new();
    loop {
        let mut b = [0];
        wire.read_exact(&mut b)?;
        value.push(b[0]);
        if value.ends_with(b"\r\n") {
            break;
        }
        if value.len() > 65536 {
            return Err(io::Error::other("test line too large"));
        }
    }
    String::from_utf8(value).map_err(io::Error::other)
}
fn reply(wire: &mut Wire, text: &str) -> io::Result<()> {
    wire.write_all(text.as_bytes())?;
    wire.flush()
}
// Send one byte beyond the intended 1000-byte ceiling, with NO newline,
// then keep the connection open. EOF/reset must come from the client, not us.
fn overflow(wire: &mut Wire, seen: &Mutex<Observed>, multiline: bool) -> io::Result<()> {
    let bytes = if multiline {
        let line = format!("250-{}\r\n", "x".repeat(994));
        // Exactly 100,000 legal continuation bytes, then a short unterminated
        // line: below the line cap, but already beyond the response cap.
        let mut bytes = line.repeat(100).into_bytes();
        bytes.extend_from_slice(b"250 ");
        bytes
    } else {
        vec![b'x'; 1001]
    };
    wire.write_all(&bytes)?;
    wire.flush()?;
    seen.lock().unwrap().overflow_bytes = bytes.len();
    let mut byte = [0];
    let closed = match wire.read(&mut byte) {
        Ok(0) => true,
        Err(error) => matches!(
            error.kind(),
            io::ErrorKind::ConnectionReset
                | io::ErrorKind::UnexpectedEof
                | io::ErrorKind::ConnectionAborted
        ),
        _ => false,
    };
    seen.lock().unwrap().overflow_peer_closed = closed;
    Ok(())
}
fn stall() {
    thread::sleep(Duration::from_millis(450));
}
fn serve(
    socket: TcpStream,
    tls: Arc<ServerConfig>,
    security: Security,
    behavior: Behavior,
    seen: &Mutex<Observed>,
) -> io::Result<()> {
    if behavior == Behavior::StallTls && security == Security::ImplicitTls {
        stall();
        return Ok(());
    }
    if matches!(
        behavior,
        Behavior::Oversized(_) | Behavior::OversizedMultiline(_)
    ) {
        socket.set_read_timeout(Some(Duration::from_secs(8)))?;
    }
    let mut wire = Wire::Plain(socket);
    if security == Security::ImplicitTls {
        wire = wire.upgrade(tls.clone())?;
    }
    match behavior {
        Behavior::Oversized(ReplyStage::Greeting)
        | Behavior::OversizedMultiline(ReplyStage::Greeting) => {
            return overflow(
                &mut wire,
                seen,
                matches!(behavior, Behavior::OversizedMultiline(_)),
            );
        }
        Behavior::StallGreeting => {
            stall();
            return Ok(());
        }
        Behavior::MalformedGreeting => {
            return reply(&mut wire, "NOT_SMTP_PROVIDER_SECRET\r\n");
        }
        Behavior::HugeGreeting => {
            return reply(&mut wire, &format!("220 {}\r\n", "x".repeat(70000)));
        }
        Behavior::SlowGreeting => {
            for b in b"220 slow greeting\r\n" {
                wire.write_all(&[*b])?;
                wire.flush()?;
                thread::sleep(Duration::from_millis(40));
            }
        }
        _ => reply(&mut wire, "220 synthetic smtp ready\r\n")?,
    }
    loop {
        let command = line(&mut wire)?;
        if command.starts_with("EHLO ") {
            let stage = if wire.encrypted() {
                ReplyStage::EncryptedEhlo
            } else {
                ReplyStage::Ehlo
            };
            if behavior == Behavior::Oversized(stage)
                || behavior == Behavior::OversizedMultiline(stage)
            {
                return overflow(
                    &mut wire,
                    seen,
                    matches!(behavior, Behavior::OversizedMultiline(_)),
                );
            }
            if !wire.encrypted() && behavior != Behavior::NoStartTls {
                reply(
                    &mut wire,
                    "250-smtp.example.test\r\n250-STARTTLS\r\n250 AUTH PLAIN LOGIN\r\n",
                )?;
            } else if behavior == Behavior::LoginOnly {
                reply(&mut wire, "250-smtp.example.test\r\n250 AUTH LOGIN\r\n")?;
            } else {
                reply(
                    &mut wire,
                    "250-smtp.example.test\r\n250 AUTH PLAIN LOGIN\r\n",
                )?;
            }
        } else if command == "STARTTLS\r\n" {
            seen.lock().unwrap().starttls += 1;
            if behavior == Behavior::Oversized(ReplyStage::StartTls)
                || behavior == Behavior::OversizedMultiline(ReplyStage::StartTls)
            {
                return overflow(
                    &mut wire,
                    seen,
                    matches!(behavior, Behavior::OversizedMultiline(_)),
                );
            }
            if behavior == Behavior::RejectStartTls {
                reply(&mut wire, "454 TLS unavailable\r\n")?;
                continue;
            }
            reply(&mut wire, "220 begin tls\r\n")?;
            if behavior == Behavior::StallTls {
                stall();
                return Ok(());
            }
            wire = wire.upgrade(tls.clone())?;
        } else if command.starts_with("AUTH ") {
            {
                let mut stats = seen.lock().unwrap();
                stats.auth += 1;
                if wire.encrypted() {
                    stats.encrypted_auth += 1;
                }
            }
            if behavior == Behavior::Oversized(ReplyStage::Auth)
                || behavior == Behavior::OversizedMultiline(ReplyStage::Auth)
            {
                return overflow(
                    &mut wire,
                    seen,
                    matches!(behavior, Behavior::OversizedMultiline(_)),
                );
            }
            if behavior == Behavior::StallAuth {
                stall();
                return Ok(());
            }
            if behavior == Behavior::AuthWrongCode {
                reply(&mut wire, "250 not an authentication success\r\n")?;
                continue;
            }
            if behavior == Behavior::RejectAuth {
                reply(&mut wire, "535 FAKE_PASSWORD must never be exposed\r\n")?;
                continue;
            }
            if behavior == Behavior::TemporaryAuth {
                reply(&mut wire, "454 temporary authentication failure\r\n")?;
                continue;
            }
            let valid = if command.starts_with("AUTH PLAIN ") {
                STANDARD
                    .decode(command.trim_end().trim_start_matches("AUTH PLAIN "))
                    .ok()
                    .as_deref()
                    == Some(b"\0FAKE_USERNAME\0FAKE_PASSWORD".as_slice())
            } else if command == "AUTH LOGIN\r\n" {
                reply(&mut wire, "334 VXNlcm5hbWU6\r\n")?;
                let user = STANDARD
                    .decode(line(&mut wire)?.trim_end())
                    .unwrap_or_default();
                reply(&mut wire, "334 UGFzc3dvcmQ6\r\n")?;
                let password = STANDARD
                    .decode(line(&mut wire)?.trim_end())
                    .unwrap_or_default();
                user == b"FAKE_USERNAME" && password == b"FAKE_PASSWORD"
            } else {
                false
            };
            seen.lock().unwrap().valid_credentials = valid;
            reply(
                &mut wire,
                if valid {
                    "235 authenticated\r\n"
                } else {
                    "535 invalid\r\n"
                },
            )?;
        } else if command.starts_with("MAIL FROM:") {
            seen.lock().unwrap().mail += 1;
            reply(&mut wire, "250 sender accepted\r\n")?;
        } else if command.starts_with("RCPT TO:") {
            seen.lock().unwrap().recipients += 1;
            reply(
                &mut wire,
                if behavior == Behavior::RejectRecipient {
                    "550 FAKE_PASSWORD no such mailbox\r\n"
                } else {
                    "250 recipient accepted\r\n"
                },
            )?;
        } else if command == "DATA\r\n" {
            if behavior == Behavior::Oversized(ReplyStage::Data)
                || behavior == Behavior::OversizedMultiline(ReplyStage::Data)
            {
                return overflow(
                    &mut wire,
                    seen,
                    matches!(behavior, Behavior::OversizedMultiline(_)),
                );
            }
            if behavior == Behavior::RejectData {
                reply(&mut wire, "554 message rejected\r\n")?;
                continue;
            }
            if behavior == Behavior::StallData {
                stall();
                return Ok(());
            }
            reply(&mut wire, "354 send message\r\n")?;
            let mut message = Vec::new();
            loop {
                let data = line(&mut wire)?;
                if data == ".\r\n" {
                    break;
                }
                message.extend_from_slice(data.strip_prefix('.').unwrap_or(&data).as_bytes());
                if message.len() > 1024 * 1024 {
                    return Err(io::Error::other("test message too large"));
                }
            }
            seen.lock().unwrap().message = Some(SecretBytes::new(message));
            if behavior == Behavior::Oversized(ReplyStage::Final)
                || behavior == Behavior::OversizedMultiline(ReplyStage::Final)
            {
                return overflow(
                    &mut wire,
                    seen,
                    matches!(behavior, Behavior::OversizedMultiline(_)),
                );
            }
            if behavior == Behavior::StallFinal {
                stall();
                return Ok(());
            }
            reply(
                &mut wire,
                if behavior == Behavior::FinalWrongCode {
                    "354 not an acceptance\r\n"
                } else {
                    "250 queued provider-private-id\r\n"
                },
            )?;
            // Deliberately do not reply to QUIT: acceptance must still mean success.
            return Ok(());
        } else if command == "QUIT\r\n" {
            reply(&mut wire, "221 bye\r\n")?;
            return Ok(());
        } else {
            return Err(io::Error::other("unexpected test SMTP command"));
        }
    }
}
