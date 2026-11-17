//! `fb-remote gateway`: the host's TLS listener. A child of the book process, talking to it only
//! over stdio frames ([`ToGateway`] in, [`FromGateway`] out). It authenticates paired computers by
//! their certificate pin, runs pairing, enforces connection limits, and forwards each authenticated
//! request to the book process with the device identity. It never sees the book or its key.
use crate::limits::{Admitted, Limits};
use crate::pairing::{self, PairReply, PairRequest, Slot};
use crate::registry::{self, Registry, Revoked};
use crate::tls::{self, DeviceGate, Identity, Pin};
use fb_remote_proto::frame::{self, FrameError, MAX_PIPE, MAX_REQUEST, MAX_RESPONSE};
use fb_remote_proto::wire::{ClientRequest, DeviceRef, FromGateway, HostResponse, RefusalCode, ToGateway};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// A connection must finish its TLS handshake within this (a silent one holds a slot until then).
pub const HANDSHAKE: Duration = Duration::from_secs(3);
/// A connection with no request for this long is closed (clients reconnect as needed).
pub const IDLE: Duration = Duration::from_secs(10 * 60);
/// How long a connection waits for the book process's answer before telling the client to retry.
pub const REPLY: Duration = Duration::from_secs(120);
/// How long a pairing connection may take to send its pairing frame after the handshake.
pub const PAIR_FRAME: Duration = Duration::from_secs(5);

struct ConnEntry {
    device_id: String,
    socket: TcpStream,
    replies: Sender<(u64, HostResponse)>,
}

struct Shared {
    registry: Mutex<Registry>,
    pairing: Mutex<Option<Slot>>,
    conns: Mutex<HashMap<u64, ConnEntry>>,
    limits: Mutex<Limits>,
    /// Connections still pending (handshaking or pairing), so one can be closed to make room.
    pending: Mutex<HashMap<u64, TcpStream>>,
    parent: Mutex<Box<dyn Write + Send>>,
    host_pin: Pin,
    host_name: String,
    next: AtomicU64,
    stopping: AtomicBool,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Shared { .. }")
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl DeviceGate for Shared {
    fn known(&self, pin: &Pin) -> bool {
        lock(&self.registry).active(pin).is_some()
    }
    fn pairing_open(&self) -> bool {
        lock(&self.pairing).as_ref().is_some_and(|s| s.live(Instant::now()))
    }
}

impl Shared {
    fn tell(&self, message: &FromGateway) {
        if frame::send(&mut *lock(&self.parent), message, MAX_PIPE).is_err() {
            // The book process is gone: nothing may be served without it.
            self.stopping.store(true, Ordering::SeqCst);
        }
    }

    fn event(&self, kind: &str, peer: IpAddr, device: Option<&DeviceRef>, detail: &str) {
        self.tell(&FromGateway::Event { kind: kind.into(), peer: peer.to_string(), device: device.cloned(), detail: detail.into() });
    }

    fn close_device(&self, device_id: &str) {
        for c in lock(&self.conns).values().filter(|c| c.device_id == device_id) {
            let _ = c.socket.shutdown(Shutdown::Both);
        }
    }
}

/// Run the gateway until the parent says Stop or goes away. Returns the process exit code.
pub fn run(mut input: impl Read, output: impl Write + Send + 'static) -> i32 {
    let parent: Mutex<Box<dyn Write + Send>> = Mutex::new(Box::new(output));
    let fail = |message: &str| {
        let _ = frame::send(&mut *lock(&parent), &FromGateway::Failed { message: message.into() }, MAX_PIPE);
        2
    };
    let Ok(Some(ToGateway::Start { bind, identity, state_dir, host_name, registry_key, min_counter, revoked })) = frame::recv::<_, ToGateway>(&mut input, MAX_PIPE) else {
        return fail("The remote-access listener was started without its settings.");
    };
    let addr = match crate::scope::bind(&bind) {
        Ok(a) => a,
        Err(message) => return fail(message),
    };
    let (identity, new_identity) = match identity {
        Some(hex) => match Identity::from_hex(&hex) {
            Ok(id) => (id, None),
            Err(_) => return fail("The host's saved connection identity could not be read."),
        },
        None => match Identity::generate(&host_name) {
            Ok(id) => {
                let hex = id.to_hex();
                (id, Some(hex))
            }
            Err(_) => return fail("The host could not make its connection identity."),
        },
    };
    let state_dir = std::path::PathBuf::from(state_dir);
    if Registry::private_dir(&state_dir).is_err() {
        return fail("The remote-access folder could not be made private to this user.");
    }
    let Some(registry_key) = fb_remote_proto::unhex(&registry_key).and_then(|k| <[u8; 32]>::try_from(k).ok()) else {
        return fail("The remote-access listener was started without the key of its list of paired computers.");
    };
    let mut registry = match Registry::load(&state_dir, registry_key, min_counter) {
        Ok(r) => r,
        Err(e) if e.to_string() == registry::ROLLED_BACK => return fail(registry::ROLLED_BACK),
        Err(e) => return fail(&format!("The list of paired computers could not be used: {e}.")),
    };
    if let Err(e) = registry.enforce(&revoked) {
        return fail(&format!("The list of paired computers could not be saved: {e}."));
    }
    let listener = match TcpListener::bind(addr) {
        Ok(l) => l,
        Err(_) => return fail("FreedomBooks could not listen on that address. Check that it belongs to this computer and the port is free."),
    };
    let Ok(local) = listener.local_addr() else { return fail("The listener has no address.") };
    let shared = Arc::new(Shared {
        registry: Mutex::new(registry),
        pairing: Mutex::new(None),
        conns: Mutex::new(HashMap::new()),
        limits: Mutex::new(Limits::default()),
        pending: Mutex::new(HashMap::new()),
        parent,
        host_pin: identity.pin(),
        host_name: registry::clean_name(&host_name).unwrap_or_else(|| "FreedomBooks host".into()),
        next: AtomicU64::new(1),
        stopping: AtomicBool::new(false),
    });
    let config = match tls::server_config(&identity, shared.clone()) {
        Ok(c) => c,
        Err(_) => {
            shared.tell(&FromGateway::Failed { message: "The host's connection settings could not be prepared.".into() });
            return 2;
        }
    };
    drop(identity);
    let counter = lock(&shared.registry).counter();
    shared.tell(&FromGateway::Ready { addr: local.to_string(), fingerprint: tls::fingerprint(&shared.host_pin), new_identity, counter });
    let acceptor = {
        let shared = shared.clone();
        std::thread::spawn(move || accept(listener, shared, config))
    };
    while let Ok(Some(message)) = frame::recv::<_, ToGateway>(&mut input, MAX_PIPE) {
        // A revocation that could not be written is tried again on every message from the host.
        {
            let mut registry = lock(&shared.registry);
            if registry.unsaved() && registry.retry_save().is_ok() {
                let counter = registry.counter();
                drop(registry);
                shared.tell(&FromGateway::Event { kind: "device.revocation_saved".into(), peer: String::new(), device: None, detail: String::new() });
                shared.tell(&FromGateway::Registry { counter });
            }
        }
        match message {
            ToGateway::Reply { conn, seq, response } => {
                if let Some(c) = lock(&shared.conns).get(&conn) {
                    let _ = c.replies.send((seq, response));
                }
            }
            ToGateway::ArmPairing { code, seconds, minted_by } => match fb_remote_proto::code::normalize(&code) {
                Some(code) if seconds > 0 && seconds <= fb_remote_proto::code::LIFETIME_SECONDS => {
                    *lock(&shared.pairing) = Some(Slot::new(code, Instant::now() + Duration::from_secs(seconds), minted_by));
                    shared.tell(&FromGateway::Armed);
                }
                _ => shared.tell(&FromGateway::Failed { message: "Invalid pairing code.".into() }),
            },
            ToGateway::CancelPairing => {
                *lock(&shared.pairing) = None;
                shared.tell(&FromGateway::Cancelled);
            }
            ToGateway::Revoke { device_id } => {
                // In force before anything else, whether or not the file can be written: the
                // registry refuses the device from now on and its connections close.
                let (revoked, counter) = {
                    let mut registry = lock(&shared.registry);
                    let revoked = registry.revoke(&device_id);
                    (revoked, registry.counter())
                };
                if matches!(revoked, Revoked::Saved(_)) {
                    shared.tell(&FromGateway::Registry { counter });
                }
                let (device, unsaved) = match revoked {
                    Revoked::Unknown => (None, None),
                    Revoked::Saved(d) => (Some(d), None),
                    Revoked::NotSaved(d, why) => (Some(d), Some(format!("The list of paired computers could not be saved ({why}). The computer is disconnected and refused now, but would be allowed again if hosting restarted before the file can be written."))),
                };
                if device.is_some() {
                    shared.close_device(&device_id);
                }
                shared.tell(&FromGateway::Revoked { device_id, device, unsaved });
            }
            ToGateway::ListDevices => {
                let devices = lock(&shared.registry).list();
                shared.tell(&FromGateway::Devices { devices });
            }
            ToGateway::Stop => break,
            ToGateway::Start { .. } => {}
        }
        if shared.stopping.load(Ordering::SeqCst) {
            break;
        }
    }
    shared.stopping.store(true, Ordering::SeqCst);
    for c in lock(&shared.conns).values() {
        let _ = c.socket.shutdown(Shutdown::Both);
    }
    for s in lock(&shared.pending).values() {
        let _ = s.shutdown(Shutdown::Both);
    }
    let _ = acceptor.join();
    shared.tell(&FromGateway::Stopped);
    0
}

fn accept(listener: TcpListener, shared: Arc<Shared>, config: Arc<ServerConfig>) {
    if listener.set_nonblocking(true).is_err() {
        shared.stopping.store(true, Ordering::SeqCst);
        return;
    }
    while !shared.stopping.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((socket, peer)) => {
                let ip = peer.ip();
                // Never serve a public source, whatever the routing or port forwarding.
                if !crate::scope::private(ip) {
                    if lock(&shared.limits).report(ip, Instant::now()) {
                        shared.event("connection.refused", ip, None, "public address");
                    }
                    continue;
                }
                let admission = lock(&shared.limits).admit(ip, Instant::now());
                let Some(admission) = admission else {
                    if lock(&shared.limits).report(ip, Instant::now()) {
                        shared.event("connection.limited", ip, None, "too many connections from this address");
                    }
                    continue;
                };
                let Ok(handle) = socket.try_clone() else { continue };
                let mut admitted = admission.admitted;
                {
                    // Make room: the pending connections the limits chose are closed now.
                    let mut pending = lock(&shared.pending);
                    pending.insert(admitted.id, handle);
                    for id in admission.evict {
                        if let Some(s) = pending.remove(&id) {
                            let _ = s.shutdown(Shutdown::Both);
                        }
                    }
                }
                let (shared, config) = (shared.clone(), config.clone());
                std::thread::spawn(move || {
                    let id = admitted.id;
                    serve(&shared, config, socket, ip, &mut admitted);
                    lock(&shared.pending).remove(&id);
                    lock(&shared.limits).closed(admitted);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn serve(shared: &Arc<Shared>, config: Arc<ServerConfig>, mut socket: TcpStream, ip: IpAddr, admitted: &mut Admitted) {
    if socket.set_nonblocking(false).is_err() || socket.set_write_timeout(Some(Duration::from_secs(30))).is_err() {
        return;
    }
    let _ = socket.set_nodelay(true);
    let Ok(mut tls) = ServerConnection::new(config) else { return };
    if let Err(e) = tls::handshake(&mut tls, &mut socket, Instant::now() + HANDSHAKE) {
        if lock(&shared.limits).report(ip, Instant::now()) {
            let detail = if e.to_string().contains("ApplicationVerificationFailure") { "computer not paired or revoked" } else { "TLS handshake failed" };
            shared.event("handshake.refused", ip, None, detail);
        }
        return;
    }
    if tls.alpn_protocol() != Some(tls::ALPN) {
        return;
    }
    let Some(pin) = tls.peer_certificates().and_then(|c| c.first()).map(|c| tls::pin(c)) else { return };
    let device = lock(&shared.registry).active(&pin).map(|d| DeviceRef { id: d.id.clone(), name: d.name.clone() });
    let Ok(control) = socket.try_clone() else { return };
    let stream = StreamOwned::new(tls, socket);
    match device {
        Some(device) => {
            // A paired computer: it leaves the pending pool (nothing can close it to make room)
            // and counts against the caps for authenticated connections.
            let ok = lock(&shared.limits).authenticated(admitted, Instant::now());
            lock(&shared.pending).remove(&admitted.id);
            if !ok {
                if lock(&shared.limits).report(ip, Instant::now()) {
                    shared.event("connection.limited", ip, Some(&device), "too many connections from this computer");
                }
                return;
            }
            connected(shared, stream, control, ip, device)
        }
        // A pairing connection stays pending, so it can be closed to make room.
        None => pair(shared, stream, control, ip, pin),
    }
}

type Tls = StreamOwned<ServerConnection, TcpStream>;

fn connected(shared: &Arc<Shared>, mut stream: Tls, control: TcpStream, ip: IpAddr, device: DeviceRef) {
    let conn = shared.next.fetch_add(1, Ordering::SeqCst);
    let (tx, rx) = mpsc::channel();
    let Ok(socket) = control.try_clone() else { return };
    if control.set_read_timeout(Some(IDLE)).is_err() {
        return;
    }
    lock(&shared.conns).insert(conn, ConnEntry { device_id: device.id.clone(), socket, replies: tx });
    shared.event("connect", ip, Some(&device), "");
    let mut seq = 0;
    loop {
        let request = match frame::recv::<_, ClientRequest>(&mut stream, MAX_REQUEST) {
            Ok(Some(r)) => r,
            Err(FrameError::Json(_) | FrameError::TooLarge(_)) => {
                let _ = frame::send(&mut stream, &HostResponse::refused(RefusalCode::Invalid, "The host could not read that request."), MAX_RESPONSE);
                break;
            }
            _ => break,
        };
        // A revocation takes effect before the next request, even on a connection already open.
        if shared.stopping.load(Ordering::SeqCst) || !lock(&shared.registry).is_active_id(&device.id) {
            break;
        }
        seq += 1;
        shared.tell(&FromGateway::Request { conn, seq, device: device.clone(), peer: ip.to_string(), request });
        let deadline = Instant::now() + REPLY;
        let response = loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok((s, r)) if s == seq => break r,
                Ok(_) => continue,
                Err(RecvTimeoutError::Timeout) => break HostResponse::refused(RefusalCode::Busy, "The host did not answer in time. Try again; nothing will be done twice."),
                Err(RecvTimeoutError::Disconnected) => break HostResponse::refused(RefusalCode::Busy, "The host stopped."),
            }
        };
        let sent = match frame::send(&mut stream, &response, MAX_RESPONSE) {
            Err(FrameError::TooLarge(_)) => frame::send(&mut stream, &HostResponse::refused(RefusalCode::Refused, "That page is too large to send to another computer."), MAX_RESPONSE),
            other => other,
        };
        if sent.is_err() {
            break;
        }
    }
    let _ = control.shutdown(Shutdown::Both);
    lock(&shared.conns).remove(&conn);
    shared.tell(&FromGateway::Disconnected { conn });
    shared.event("disconnect", ip, Some(&device), "");
}

fn pair(shared: &Arc<Shared>, mut stream: Tls, control: TcpStream, ip: IpAddr, device_pin: Pin) {
    if control.set_read_timeout(Some(PAIR_FRAME)).is_err() {
        return;
    }
    let refuse = |stream: &mut Tls, message: &str| {
        let _ = frame::send(stream, &PairReply::Refused { message: message.into() }, MAX_REQUEST);
        let _ = control.shutdown(Shutdown::Both);
    };
    let Ok(Some(PairRequest::Pair { device_name, proof })) = frame::recv::<_, PairRequest>(&mut stream, 4096) else {
        return refuse(&mut stream, pairing::REFUSED);
    };
    let Some(name) = registry::clean_name(&device_name) else {
        return refuse(&mut stream, "Give this computer a name of up to 60 characters.");
    };
    let proof = fb_remote_proto::unhex(&proof).unwrap_or_default();
    let (accepted, destroyed) = {
        let mut slot = lock(&shared.pairing);
        let now = Instant::now();
        match slot.as_mut() {
            Some(s) if s.allows(ip, now) => {
                if proof.len() == 32 && pairing::equal(&proof, &pairing::client_proof(&s.code, &shared.host_pin, &device_pin)) {
                    (slot.take(), false)
                } else {
                    s.wrong(ip);
                    let destroyed = !s.live(now);
                    if destroyed {
                        *slot = None;
                    }
                    (None, destroyed)
                }
            }
            _ => (None, false),
        }
    };
    let Some(slot) = accepted else {
        shared.event("pair.refused", ip, None, if destroyed { "wrong code; the code was destroyed after too many attempts" } else { "wrong, expired or used code, or a certificate mismatch" });
        return refuse(&mut stream, pairing::REFUSED);
    };
    let added = {
        let mut registry = lock(&shared.registry);
        registry.add(&name, &device_pin, &slot.minted_by).map(|r| (r, registry.counter()))
    };
    let record = match added {
        Ok((r, counter)) => {
            shared.tell(&FromGateway::Registry { counter });
            r
        }
        Err(_) => {
            shared.event("pair.refused", ip, None, "the registry could not be saved");
            return refuse(&mut stream, "The host could not save this computer. Ask the host's administrator to check the host, then try again with a new code.");
        }
    };
    let device = DeviceRef { id: record.id.clone(), name: record.name.clone() };
    shared.event("pair.succeeded", ip, Some(&device), &format!("approved by {}", slot.minted_by));
    let reply = PairReply::Paired { device_id: record.id, proof: fb_remote_proto::hex(&pairing::host_proof(&slot.code, &shared.host_pin, &device_pin)), host_name: shared.host_name.clone() };
    let _ = frame::send(&mut stream, &reply, MAX_REQUEST);
    let _ = stream.flush();
    let _ = control.shutdown(Shutdown::Both);
}
