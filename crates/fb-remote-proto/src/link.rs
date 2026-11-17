//! The book process's side of the gateway: start the `fb-remote gateway` child, answer its
//! requests through the [`Dispatcher`], record its events in the audit, and send it the
//! Administrator's controls (pairing codes, revocation, device list, stop).
//!
//! The child is started from a fixed path with an empty environment and talks only over its stdio
//! pipes. If it exits, every remote session ends.
use crate::audit::Event;
use crate::backend::Backend;
use crate::dispatch::Dispatcher;
use crate::frame::{self, MAX_PIPE};
use crate::wire::{DeviceRecord, FromGateway, HostResponse, IdentityHex, RefusalCode, ToGateway};
use std::io;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

/// Requests answered at once; beyond this the gateway's callers are told the host is busy.
pub const MAX_WORKERS: usize = 32;
const CONTROL_TIMEOUT: Duration = Duration::from_secs(15);

pub struct Start {
    /// The `fb-remote` executable (a fixed path beside the app, never searched for on PATH).
    pub exe: PathBuf,
    /// Numeric address and port to listen on (`192.168.1.20:47100`, `100.101.102.103:0`).
    pub bind: String,
    /// The host's TLS identity, or None to have the gateway make one (returned in `new_identity`).
    pub identity: Option<IdentityHex>,
    /// Where the gateway keeps its device registry.
    pub state_dir: PathBuf,
    pub host_name: String,
    /// The list of paired computers' MAC key: random, kept in the host's sealed record.
    pub registry_key: [u8; 32],
    /// The list's counter as the sealed record has it: an older list on disk is refused.
    pub min_counter: u64,
    /// Computers the sealed record says were revoked: refused whatever the list on disk says.
    pub revoked: Vec<String>,
    /// Called (on the link's reader thread) after every write of the list, with its new counter.
    pub on_registry: Option<Box<dyn Fn(u64) + Send + Sync>>,
}

impl Start {
    /// A start with no sealed record (tests and the development host): a fresh random list key,
    /// counter 0 and nothing revoked.
    pub fn new(exe: PathBuf, bind: &str, identity: Option<IdentityHex>, state_dir: PathBuf, host_name: &str) -> io::Result<Start> {
        let registry_key = crate::random::<32>().map_err(|_| io::Error::other("no random source"))?;
        Ok(Start { exe, bind: bind.into(), identity, state_dir, host_name: host_name.into(), registry_key, min_counter: 0, revoked: vec![], on_registry: None })
    }
}

/// What [`HostLink::revoke`] did.
#[derive(Debug, PartialEq, Eq)]
pub struct Revocation {
    /// False: no such paired computer.
    pub revoked: bool,
    /// It was already revoked (a retry, or revoked before): nothing new was done or recorded.
    pub already: bool,
    /// Revoked now, but not yet saved (why): the listener has not answered yet, or could not
    /// write the list of paired computers. Until it is saved, a restart of hosting would let the
    /// computer back in; the host's screen keeps showing this ([`Dispatcher::revocation`]).
    pub unsaved: Option<String>,
}

pub struct HostLink<B: Backend> {
    dispatcher: Arc<Dispatcher<B>>,
    control_timeout: Duration,
    writer: Arc<Mutex<ChildStdin>>,
    child: Mutex<Child>,
    control: Mutex<Receiver<FromGateway>>,
    stopping: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    pub addr: String,
    pub fingerprint: String,
    /// Set when the gateway generated the identity: the caller must seal and keep it.
    pub new_identity: Option<IdentityHex>,
    /// The list of paired computers' counter when the gateway started.
    pub counter: u64,
}

/// Whether `reply` is an answer to control message `message` (a late answer to an earlier one is
/// not). The gateway sends `Failed` after start only for a pairing code it cannot use.
fn answers(message: &ToGateway, reply: &FromGateway) -> bool {
    match (message, reply) {
        (ToGateway::ArmPairing { .. }, FromGateway::Armed | FromGateway::Failed { .. }) | (ToGateway::CancelPairing, FromGateway::Cancelled) | (ToGateway::ListDevices, FromGateway::Devices { .. }) => true,
        (ToGateway::Revoke { device_id }, FromGateway::Revoked { device_id: answered, .. }) => answered == device_id,
        _ => false,
    }
}

struct WorkerSlot(Arc<AtomicUsize>);

impl Drop for WorkerSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn send(writer: &Mutex<ChildStdin>, message: &ToGateway) -> io::Result<()> {
    let mut w = writer.lock().unwrap_or_else(PoisonError::into_inner);
    frame::send(&mut *w, message, MAX_PIPE).map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e.to_string()))
}

impl<B: Backend> HostLink<B> {
    pub fn start(dispatcher: Arc<Dispatcher<B>>, start: Start) -> io::Result<HostLink<B>> {
        let mut child = Command::new(&start.exe)
            .arg("gateway")
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| io::Error::other("gateway stdin"))?;
        let mut stdout = child.stdout.take().ok_or_else(|| io::Error::other("gateway stdout"))?;
        let writer = Arc::new(Mutex::new(stdin));
        dispatcher.learn_revoked(&start.revoked);
        let on_registry = start.on_registry;
        let begin = ToGateway::Start {
            bind: start.bind.clone(),
            identity: start.identity,
            state_dir: start.state_dir.to_string_lossy().into_owned(),
            host_name: start.host_name,
            registry_key: crate::hex(&start.registry_key),
            min_counter: start.min_counter,
            revoked: start.revoked,
        };
        if let Err(e) = send(&writer, &begin) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        let ready = frame::recv::<_, FromGateway>(&mut stdout, MAX_PIPE);
        let (addr, fingerprint, new_identity, counter) = match ready {
            Ok(Some(FromGateway::Ready { addr, fingerprint, new_identity, counter })) => (addr, fingerprint, new_identity, counter),
            other => {
                let _ = child.kill();
                let _ = child.wait();
                let message = match other {
                    Ok(Some(FromGateway::Failed { message })) => message,
                    _ => "The remote-access listener did not start.".into(),
                };
                return Err(io::Error::other(message));
            }
        };
        dispatcher.audit(Event { kind: "host.started".into(), peer: addr.clone(), detail: fingerprint.clone(), ..Default::default() });
        let (control_tx, control_rx) = mpsc::channel();
        let stopping = Arc::new(AtomicBool::new(false));
        let reader = {
            let dispatcher = dispatcher.clone();
            let writer = writer.clone();
            let stopping = stopping.clone();
            std::thread::spawn(move || {
                let workers = Arc::new(AtomicUsize::new(0));
                while let Ok(Some(message)) = frame::recv::<_, FromGateway>(&mut stdout, MAX_PIPE) {
                    match message {
                        FromGateway::Request { conn, seq, device, peer, request } => {
                            if workers.fetch_add(1, Ordering::SeqCst) >= MAX_WORKERS {
                                workers.fetch_sub(1, Ordering::SeqCst);
                                let response = HostResponse::refused(RefusalCode::Busy, "The host is busy. Try again in a moment.");
                                let _ = send(&writer, &ToGateway::Reply { conn, seq, response });
                                continue;
                            }
                            let (dispatcher, writer) = (dispatcher.clone(), writer.clone());
                            // Released when the worker ends, even if handling the request panics.
                            let slot = WorkerSlot(workers.clone());
                            std::thread::spawn(move || {
                                let _slot = slot;
                                let response = dispatcher.handle(conn, &device, &peer, request);
                                let _ = send(&writer, &ToGateway::Reply { conn, seq, response });
                            });
                        }
                        FromGateway::Disconnected { conn } => dispatcher.disconnected(conn),
                        FromGateway::Registry { counter } => {
                            if let Some(hook) = &on_registry {
                                hook(counter);
                            }
                        }
                        FromGateway::Event { kind, peer, device, detail } => {
                            if kind == "device.revocation_saved" {
                                dispatcher.revocations_saved();
                            }
                            dispatcher.audit(Event { kind, peer, device_id: device.as_ref().map(|d| d.id.clone()), device_name: device.map(|d| d.name), user: None, detail });
                        }
                        // A revocation's answer is handled whenever it arrives, also after
                        // `revoke` stopped waiting for it, then passed on in case it still waits.
                        FromGateway::Revoked { device_id, device, unsaved } => {
                            dispatcher.revocation_answered(&device_id, device.as_ref(), unsaved.as_deref());
                            if control_tx.send(FromGateway::Revoked { device_id, device, unsaved }).is_err() {
                                break;
                            }
                        }
                        other => {
                            if control_tx.send(other).is_err() {
                                break;
                            }
                        }
                    }
                }
                // The gateway is gone: nobody can reach these sessions any more.
                let reason = if stopping.load(Ordering::SeqCst) { "hosting stopped" } else { "listener exited" };
                dispatcher.end_all(reason);
                dispatcher.audit(Event { kind: "host.stopped".into(), detail: reason.into(), ..Default::default() });
            })
        };
        let sweeper = {
            let dispatcher = dispatcher.clone();
            let stopping = stopping.clone();
            std::thread::spawn(move || {
                let mut ticks = 0u32;
                while !stopping.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(250));
                    ticks += 1;
                    if ticks.is_multiple_of(40) {
                        dispatcher.sweep();
                    }
                }
            })
        };
        Ok(HostLink { dispatcher, control_timeout: CONTROL_TIMEOUT, writer, child: Mutex::new(child), control: Mutex::new(control_rx), stopping, threads: vec![reader, sweeper], addr, fingerprint, new_identity, counter })
    }

    pub fn dispatcher(&self) -> &Arc<Dispatcher<B>> {
        &self.dispatcher
    }

    /// How long a control message waits for the gateway (15 s; tests shorten it).
    pub fn set_control_timeout(&mut self, timeout: Duration) {
        self.control_timeout = timeout;
    }

    /// The gateway's process id (diagnostics; tests freeze it).
    pub fn gateway_pid(&self) -> u32 {
        self.child.lock().unwrap_or_else(PoisonError::into_inner).id()
    }

    /// Why remote access stopped, if the audit log could not be written (see
    /// [`Dispatcher::audit_error`]). Hosting must be restarted, which reopens and repairs the log.
    pub fn audit_error(&self) -> Option<String> {
        self.dispatcher.audit_error()
    }

    /// Send one control message and wait for its answer. A late answer to an earlier message that
    /// timed out is a different kind of reply, and is skipped rather than taken for this one.
    fn control(&self, message: ToGateway) -> io::Result<FromGateway> {
        let expected = |reply: &FromGateway| answers(&message, reply);
        let control = self.control.lock().unwrap_or_else(PoisonError::into_inner);
        while control.try_recv().is_ok() {}
        send(&self.writer, &message)?;
        let deadline = std::time::Instant::now() + self.control_timeout;
        loop {
            match control.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) {
                Ok(reply) if !expected(&reply) => continue,
                Ok(FromGateway::Failed { message }) => return Err(io::Error::other(message)),
                Ok(reply) => return Ok(reply),
                Err(RecvTimeoutError::Timeout) => return Err(io::Error::new(io::ErrorKind::TimedOut, "The remote-access listener did not answer.")),
                Err(RecvTimeoutError::Disconnected) => return Err(io::Error::new(io::ErrorKind::BrokenPipe, "The remote-access listener stopped.")),
            }
        }
    }

    /// An Administrator (`minted_by`, already authorized by the caller) asks to pair a computer.
    /// Returns the one-time code to show; it admits one device for ten minutes.
    pub fn arm_pairing(&self, minted_by: &str) -> io::Result<String> {
        self.arm_pairing_for(minted_by, crate::code::LIFETIME_SECONDS)
    }

    /// As [`HostLink::arm_pairing`] with a shorter life (at most the standard ten minutes).
    pub fn arm_pairing_for(&self, minted_by: &str, seconds: u64) -> io::Result<String> {
        let code = crate::code::generate().map_err(|_| io::Error::other("no random source"))?;
        match self.control(ToGateway::ArmPairing { code: code.clone(), seconds, minted_by: minted_by.into() })? {
            FromGateway::Armed => {
                self.dispatcher.audit(Event { kind: "pair.armed".into(), user: Some(minted_by.into()), ..Default::default() });
                Ok(code)
            }
            _ => Err(io::Error::other("unexpected answer")),
        }
    }

    pub fn cancel_pairing(&self) -> io::Result<()> {
        self.control(ToGateway::CancelPairing).map(drop)
    }

    pub fn devices(&self) -> io::Result<Vec<DeviceRecord>> {
        match self.control(ToGateway::ListDevices)? {
            FromGateway::Devices { devices } => {
                self.dispatcher.learn_devices(&devices);
                Ok(devices)
            }
            _ => Err(io::Error::other("unexpected answer")),
        }
    }

    /// Revoke a device; `by` is the Administrator (already authorized by the caller).
    ///
    /// The book process applies it first, without waiting for the gateway: nothing of the device's
    /// is served from now on, its sessions end and `device.revoked` is recorded. Then the gateway
    /// is told to refuse its certificate, close its connections and save the list of paired
    /// computers; its answer is recorded whenever it arrives (`device.revocation_confirmed`, or
    /// `device.revocation_unsaved` with why). If it does not answer in time, or could not save,
    /// `unsaved` says so and the host's screen keeps showing it. A repeat answers `already`.
    pub fn revoke(&self, device_id: &str, by: &str) -> io::Result<Revocation> {
        if let Some(outstanding) = self.dispatcher.revocation(device_id) {
            return Ok(Revocation { revoked: true, already: true, unsaved: outstanding });
        }
        if self.dispatcher.known_device(device_id).is_none() {
            // Not seen in this run: learn the list of paired computers first.
            self.devices()?;
            if let Some(outstanding) = self.dispatcher.revocation(device_id) {
                return Ok(Revocation { revoked: true, already: true, unsaved: outstanding });
            }
        }
        let Some(device) = self.dispatcher.known_device(device_id) else {
            return Ok(Revocation { revoked: false, already: false, unsaved: None });
        };
        if !self.dispatcher.revoke_device(&device, by) {
            return Ok(Revocation { revoked: true, already: true, unsaved: self.dispatcher.revocation(device_id).flatten() });
        }
        let answered = self.control(ToGateway::Revoke { device_id: device_id.into() });
        let unsaved = match answered {
            // The reader thread has recorded the answer already.
            Ok(FromGateway::Revoked { unsaved, .. }) => unsaved,
            Ok(_) => Some(crate::dispatch::NOT_CONFIRMED.to_owned()),
            Err(e) => {
                self.dispatcher.audit(Event { kind: "device.revocation_unconfirmed".into(), device_id: Some(device.id.clone()), device_name: Some(device.name.clone()), detail: e.to_string(), ..Default::default() });
                Some(crate::dispatch::NOT_CONFIRMED.to_owned())
            }
        };
        Ok(Revocation { revoked: true, already: false, unsaved })
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    /// Stop hosting and tell whoever is connected: for up to `grace`, while computers are still
    /// connected, every request is answered "`reason`" (refusal code `host_locked`), then the
    /// listener stops and every remote session ends.
    pub fn stop_with(mut self, reason: &str, grace: Duration) {
        self.dispatcher.set_ending(reason);
        let deadline = std::time::Instant::now() + grace;
        while std::time::Instant::now() < deadline && self.dispatcher.connected() > 0 {
            std::thread::sleep(Duration::from_millis(50));
        }
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if self.stopping.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = send(&self.writer, &ToGateway::Stop);
        let mut child = self.child.lock().unwrap_or_else(PoisonError::into_inner);
        for _ in 0..40 {
            if matches!(child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
        drop(child);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

impl<B: Backend> Drop for HostLink<B> {
    fn drop(&mut self) {
        self.shutdown();
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::DeviceRef;

    #[test]
    fn a_late_answer_to_an_earlier_control_message_is_not_taken_for_the_next() {
        let revoke = ToGateway::Revoke { device_id: "a".into() };
        let list = ToGateway::ListDevices;
        assert!(answers(&list, &FromGateway::Devices { devices: vec![] }));
        assert!(!answers(&list, &FromGateway::Revoked { device_id: "a".into(), device: None, unsaved: None }), "a late revocation answer");
        assert!(!answers(&revoke, &FromGateway::Devices { devices: vec![] }), "a late device list");
        assert!(!answers(&revoke, &FromGateway::Revoked { device_id: "b".into(), device: Some(DeviceRef { id: "b".into(), name: "x".into() }), unsaved: None }), "another device's revocation");
        assert!(!answers(&revoke, &FromGateway::Revoked { device_id: "b".into(), device: None, unsaved: None }), "another id's 'no such device' (B6)");
        assert!(answers(&revoke, &FromGateway::Revoked { device_id: "a".into(), device: Some(DeviceRef { id: "a".into(), name: "x".into() }), unsaved: None }));
        assert!(!answers(&ToGateway::CancelPairing, &FromGateway::Armed));
        assert!(!answers(&ToGateway::CancelPairing, &FromGateway::Failed { message: "x".into() }), "a Failed answers only a pairing code (B6)");
        assert!(!answers(&revoke, &FromGateway::Failed { message: "x".into() }));
        assert!(answers(&ToGateway::ArmPairing { code: "c".into(), seconds: 1, minted_by: "a".into() }, &FromGateway::Failed { message: "x".into() }));
    }
}
