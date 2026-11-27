//! "Host these books" (docs/hosting.md, docs/remote-access-design.md section 10).
//!
//! Off by default. While an Administrator has hosting on, the desktop's `App` moves into a
//! multi-user `SharedHost` and the desktop window becomes that host's **local session**: every
//! desktop call takes the host's lease and runs with the local session swapped in (renders, form
//! posts and queries through `SessionHost::dispatch`, so the session layer's stale-edit and replay
//! rules cover the desktop and remote people alike). Remote people reach the same host through the
//! `fb-remote gateway` child (TLS, pairing; it never sees the book) and `fb_host::SessionBackend`.
//! When hosting stops, the `App` comes back with the local person's state in it and the desktop is
//! exactly the standalone desktop again.
//!
//! The host's sealed record (`fb_app::hosting::HostRecord`, in the encrypted book) keeps what the
//! files beside the book cannot be trusted with: whether this book has been hosted, the audit key
//! (made here, never given to the gateway) and the audit head, the paired-computer list's key and
//! counter, every revoked computer, and the host's TLS identity. Every start opens the audit log
//! against the kept head (`open_with_head`) and checks the list of paired computers against the kept
//! counter; an older copy of either is refused with a repair an Administrator can choose.
//!
//! Lock order: the desktop door (`APP`) first, then `HOSTED`, then the host's lease. Nothing here
//! talks to the gateway while holding the lease (the gateway's reader thread needs it to end
//! sessions), and the threads the gateway link and the record writer run never take the door.
use crate::{app_error, fail};
use fb_app::App;
use fb_app::hosting::{Head as KeptHead, HostRecord, Identity};
use fb_app::multiuser::{ClientInfo, HostError, Policy, Request, Response, SessionHost, SessionId, SharedHost};
use fb_host::SessionBackend;
use fb_remote_proto::audit::{self, AuditLog, Event, Head, KeyStore, Sink};
use fb_remote_proto::link::{HostLink, Start};
use fb_remote_proto::{Config, Dispatcher, registry_file, scope};
use napi::Result;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

/// How the desktop window appears in the host's session list and queue.
const LOCAL_LABEL: &str = "This computer (the host)";
/// The main thread never waits longer than this for a remote request to finish.
pub(crate) const DOOR_WAIT: Duration = Duration::from_secs(2);
/// A desktop task (backup, restore, sign-in) waits up to this for its turn.
pub(crate) const TASK_WAIT: Duration = Duration::from_secs(60);
/// While hosting stops, connected computers are told why for up to this long.
// The connected client probes every second; a vanished client cannot extend this wait.
const STOP_GRACE: Duration = Duration::from_millis(1500);
pub(crate) const STOPPED: &str = "The host stopped sharing these books.";
const AUDIT_FILE: &str = "remote-audit.jsonl";

static HELPER: OnceLock<PathBuf> = OnceLock::new();
/// Only touched while the desktop door is held.
static HOSTED: Mutex<Option<Hosted>> = Mutex::new(None);
/// Why hosting last stopped, and the problem that stopped the last start (for the screen).
static LAST: Mutex<Last> = Mutex::new(Last { stopped: None, problem: None });
/// Whether hosting is on, readable without the door or `HOSTED` (`app_state`, Lock, the watch).
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// The host person's Lock, not applied yet. Setting it needs neither the door nor the host's lease,
/// so Lock never fails because a remote request holds the books; it is applied before anything
/// else at the next desktop call (`ensure_local`, `settle`) and when hosting ends.
static LOCAL_LOCK: AtomicBool = AtomicBool::new(false);
/// What the trusted main process needs with nobody signed in at the host: the listening address
/// and the computers connected (the network watch and the window title). No secret.
static STATUS: Mutex<Option<(String, Weak<Dispatcher<SessionBackend>>)>> = Mutex::new(None);

struct Last {
    stopped: Option<String>,
    problem: Option<Problem>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct Problem {
    /// `audit-lost-key`, `audit-rolled-back`, `audit-damaged`, `devices-rolled-back`, `devices-damaged`.
    pub kind: &'static str,
    pub message: String,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

pub(crate) struct Hosted {
    shared: SharedHost,
    local: SessionId,
    link: Option<HostLink<SessionBackend>>,
    runtime: Arc<Runtime>,
    flusher: Option<JoinHandle<()>>,
    book_path: PathBuf,
    bind: String,
}

/// What the link's threads, the audit sink and the record writer share.
struct Runtime {
    shared: SharedHost,
    registry_dir: PathBuf,
    registry_key: [u8; 32],
    audit_path: PathBuf,
    pending: Mutex<Pending>,
    wake: Condvar,
    stopping: AtomicBool,
    /// Why the sealed record could not be written (hosting is paused when this is set).
    record_error: Mutex<Option<String>>,
    dispatcher: OnceLock<Weak<Dispatcher<SessionBackend>>>,
    /// The pairing code on show and when it expires.
    pairing: Mutex<Option<(String, Instant)>>,
    audit_lines: AtomicU64,
    counter: AtomicU64,
}

#[derive(Default)]
struct Pending {
    head: Option<Head>,
    counter: Option<u64>,
    seen: Vec<(String, u64)>,
    revoked: Vec<String>,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.head.is_none() && self.counter.is_none() && self.seen.is_empty() && self.revoked.is_empty()
    }
    fn merge(&mut self, later: Pending) {
        if later.head.is_some() {
            self.head = later.head;
        }
        if later.counter.is_some() {
            self.counter = later.counter;
        }
        self.seen.extend(later.seen);
        self.revoked.extend(later.revoked);
    }
    /// Apply to the record: the head and counter only ever move forward.
    fn apply(&self, r: &mut HostRecord) {
        if let Some(h) = &self.head
            && r.audit_head.as_ref().is_none_or(|k| h.seq >= k.seq)
        {
            r.audit_head = Some(KeptHead { seq: h.seq, hash: h.hash.clone() });
        }
        if let Some(c) = self.counter {
            r.device_counter = r.device_counter.max(c);
        }
        for id in &self.revoked {
            r.note_revoked(id);
        }
        for (id, at) in &self.seen {
            if !r.revoked.contains(id) {
                r.note_seen(id, *at);
            }
        }
    }
}

impl Runtime {
    fn push(&self, f: impl FnOnce(&mut Pending)) {
        f(&mut lock(&self.pending));
        self.wake.notify_all();
    }

    fn pause(&self, reason: &str) {
        if let Some(d) = self.dispatcher.get().and_then(Weak::upgrade) {
            d.pause(reason);
        }
    }

    /// The gateway wrote the list of paired computers: read it back with the key the gateway was
    /// given, check it is the list it says it wrote, and record its counter and revocations.
    fn registry_written(&self, counter: u64) {
        match registry_file::read(&self.registry_dir, &self.registry_key) {
            Ok(Some(c)) if c.counter >= counter && c.counter >= self.counter.load(Ordering::SeqCst) => {
                self.counter.store(c.counter, Ordering::SeqCst);
                let revoked: Vec<String> = c.devices.iter().filter(|d| d.revoked_at.is_some()).map(|d| d.id.clone()).collect();
                self.push(|p| {
                    p.counter = Some(c.counter);
                    p.revoked.extend(revoked);
                });
            }
            _ => self.pause("The list of paired computers on disk does not match what the remote-access listener reported. Remote access is paused; restart hosting on the Hosting screen."),
        }
    }
}

/// The audit log as the dispatcher's sink: every line also moves the head the sealed record keeps.
struct Recorder {
    log: AuditLog,
    runtime: Arc<Runtime>,
}

impl Sink for Recorder {
    fn record(&mut self, event: &Event) -> std::io::Result<()> {
        self.log.append(event)?;
        let head = self.log.head();
        self.runtime.audit_lines.store(head.seq, Ordering::SeqCst);
        let seen = (event.kind == "connect").then(|| event.device_id.clone()).flatten();
        if event.kind == "pair.succeeded" {
            *lock(&self.runtime.pairing) = None;
        }
        self.runtime.push(|p| {
            p.head = Some(head);
            if let Some(id) = seen {
                p.seen.push((id, now_secs()));
            }
        });
        Ok(())
    }
}

/// The audit key, kept in the sealed record (`audit::KeyStore` over the record being prepared).
struct RecordKeys(RefCell<Option<[u8; 32]>>);

impl KeyStore for RecordKeys {
    fn load(&self) -> std::io::Result<Option<[u8; 32]>> {
        Ok(*self.0.borrow())
    }
    fn store(&self, key: &[u8; 32]) -> std::io::Result<()> {
        *self.0.borrow_mut() = Some(*key);
        Ok(())
    }
}

fn hex(bytes: &[u8]) -> String {
    fb_remote_proto::hex(bytes)
}

/// The record writer: brings the sealed record up to the audit head, the list counter, revocations
/// and last-seen times. It waits for the host's lease like any request; a write that fails stops
/// remote access (fail closed), because the record could no longer catch an older file put back.
fn flush_loop(rt: Arc<Runtime>) {
    loop {
        let pending = {
            let mut p = lock(&rt.pending);
            while p.is_empty() && !rt.stopping.load(Ordering::SeqCst) {
                p = rt.wake.wait_timeout(p, Duration::from_secs(5)).unwrap_or_else(PoisonError::into_inner).0;
            }
            if p.is_empty() {
                return;
            }
            std::mem::take(&mut *p)
        };
        let written = match rt.shared.lease_within("hosting: records", Duration::from_secs(30), &mut |_| {}) {
            Ok(mut lease) => Some(write_pending(lease.host_app(), &pending)),
            Err(_) => None,
        };
        match written {
            Some(Ok(())) => {}
            Some(Err(e)) => {
                let reason = format!("The host could not update its hosting record in the books ({e}). Remote access is paused; restart hosting on the Hosting screen.");
                *lock(&rt.record_error) = Some(reason.clone());
                rt.pause(&reason);
                return;
            }
            None => {
                // The books are busy (a long backup): keep it for the next turn.
                lock(&rt.pending).merge(pending);
                if rt.stopping.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

fn write_pending(app: &mut App, pending: &Pending) -> fb_app::AppResult<()> {
    let mut r = app.host_record()?.ok_or_else(|| fb_app::AppError::Invalid("the hosting record is missing".into()))?;
    pending.apply(&mut r);
    app.save_host_record(&r)
}

// ---- The desktop door while hosting ----

pub(crate) fn active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}

/// The host person's Lock while hosting (S1): recorded at once, without the books' lease. False
/// when not hosting (the caller then signs out the standalone desktop as usual).
pub(crate) fn request_local_lock() -> bool {
    if !active() {
        return false;
    }
    LOCAL_LOCK.store(true, Ordering::SeqCst);
    true
}

/// A Lock recorded while hosting that hosting ended before applying: the door's person signs out.
fn settle(door: &mut App) {
    if LOCAL_LOCK.swap(false, Ordering::SeqCst) {
        door.sign_out();
    }
}

/// `{running, bind, connected}` for the trusted main process; no Administrator is needed (S2, S5).
pub(crate) fn status() -> serde_json::Value {
    let status = lock(&STATUS).as_ref().map(|(bind, d)| (bind.clone(), d.upgrade()));
    match status {
        Some((bind, d)) => {
            let mut devices: Vec<String> = d.map(|d| d.connections().into_iter().map(|(dev, _, _)| dev.id).collect()).unwrap_or_default();
            devices.sort();
            devices.dedup();
            serde_json::json!({"running": true, "bind": bind, "connected": devices.len()})
        }
        None => serde_json::json!({"running": false, "bind": "", "connected": 0}),
    }
}

fn host_error(e: HostError) -> napi::Error {
    match e {
        HostError::App(e) => app_error(e),
        HostError::NotOpen => napi::Error::new(napi::Status::GenericFailure, "LOCKED"),
        HostError::Busy { .. } => napi::Error::new(napi::Status::GenericFailure, "BUSY"),
        other => fail(other.to_string()),
    }
}

fn lease(shared: &SharedHost, wait: Duration) -> Result<fb_app::multiuser::Lease<'_>> {
    shared.lease_within("This computer", wait, &mut |_| {}).map_err(host_error)
}

/// The local session, opened again if the host ended it (it used up its generations), with a Lock
/// recorded since the last call applied first.
fn ensure_local(host: &mut SessionHost, local: &mut SessionId) -> Result<()> {
    if host.with_session(local, |_| ()).is_err() {
        *local = host.open_session(ClientInfo::local(LOCAL_LABEL)).map_err(host_error)?;
    }
    if LOCAL_LOCK.swap(false, Ordering::SeqCst)
        && let Err(e) = host.dispatch(local, Request::Lock)
    {
        // Still pending: nothing runs for the local session until it is applied.
        LOCAL_LOCK.store(true, Ordering::SeqCst);
        return Err(host_error(e));
    }
    Ok(())
}

/// Why hosting must end after a desktop call: the books were closed or other books opened.
fn book_changed(host: &SessionHost, book: &Path) -> Option<&'static str> {
    let app = host.app();
    if !app.is_open() {
        Some("The host closed these books.")
    } else if app.book_path() != Some(book) {
        Some("The host switched to other books.")
    } else {
        None
    }
}

/// Run a desktop call: on the standalone desktop `alone(door)`; while hosting `hosted(host, local,
/// alone)` under the host's lease (waiting at most `wait`). Hosting ends afterwards if the call
/// closed or replaced the books.
pub(crate) fn route<T, F: FnOnce(&mut App) -> Result<T>>(door: &mut App, wait: Duration, alone: F, hosted: impl FnOnce(&mut SessionHost, &SessionId, F) -> Result<T>) -> Result<T> {
    let mut slot = lock(&HOSTED);
    let Some(h) = slot.as_mut() else {
        drop(slot);
        settle(door);
        return alone(door);
    };
    let (result, ended) = {
        let mut lease = lease(&h.shared, wait)?;
        ensure_local(&mut lease, &mut h.local)?;
        let result = hosted(&mut lease, &h.local, alone);
        (result, book_changed(&lease, &h.book_path))
    };
    if let Some(reason) = ended {
        end(door, &mut slot, reason);
    }
    result
}

/// [`route`] with the local session swapped into the app (`SessionHost::with_session`).
pub(crate) fn with_desk<T>(door: &mut App, wait: Duration, f: impl FnOnce(&mut App) -> Result<T>) -> Result<T> {
    route(door, wait, f, |host, local, f| host.with_session(local, f).map_err(host_error)?)
}

/// One request from the desktop window as the host's local session (`SessionHost::dispatch`).
pub(crate) fn dispatch(host: &mut SessionHost, local: &SessionId, request: Request) -> Result<Response> {
    host.dispatch(local, request).map_err(host_error)
}

/// Stop hosting (whatever the reason): connected computers are told, every remote session ends,
/// the record is brought up to date, and the app comes back to the door with the local person's
/// state in it.
fn end(door: &mut App, slot: &mut Option<Hosted>, reason: &str) {
    let Some(mut h) = slot.take() else { return };
    ACTIVE.store(false, Ordering::SeqCst);
    *lock(&STATUS) = None;
    if let Some(link) = h.link.take() {
        link.stop_with(reason, STOP_GRACE);
    }
    h.runtime.stopping.store(true, Ordering::SeqCst);
    h.runtime.wake.notify_all();
    if let Some(t) = h.flusher.take() {
        let _ = t.join();
    }
    let mut host = loop {
        // Nothing else holds the books now (the listener is gone); this does not wait in practice.
        if let Ok(mut lease) = h.shared.lease_within("hosting: stop", Duration::from_secs(120), &mut |_| {}) {
            break std::mem::replace(&mut *lease, SessionHost::new(App::new(), Policy::default()));
        }
    };
    let pending = std::mem::take(&mut *lock(&h.runtime.pending));
    if !pending.is_empty() && host.app().is_open() {
        let _ = write_pending(host.host_app(), &pending);
    }
    *door = host.into_app(&h.local);
    // A Lock recorded while the books were busy: the person it was for does not come back signed in.
    settle(door);
    lock(&LAST).stopped = Some(reason.to_owned());
}

/// Stop hosting from the door (quit, lock that closes the books, network change). No-op if off.
pub(crate) fn end_from(door: &mut App, reason: &str) {
    let mut slot = lock(&HOSTED);
    end(door, &mut slot, reason);
}

// ---- Hosting controls ----

pub(crate) fn configure(path: PathBuf) -> Result<()> {
    if !path.is_absolute() || !path.is_file() {
        return Err(fail("The remote-access helper was not found."));
    }
    HELPER.set(path).map_err(|_| fail("The remote-access helper is already configured."))
}

/// The signed-in Administrator at this desktop (hosting or not).
fn administrator(door: &mut App, slot: &mut Option<Hosted>) -> Result<String> {
    match slot.as_mut() {
        None => {
            settle(door);
            door.hosting_administrator().map_err(app_error)
        }
        Some(h) => {
            let mut lease = lease(&h.shared, TASK_WAIT)?;
            ensure_local(&mut lease, &mut h.local)?;
            lease.with_session(&h.local, |app| app.hosting_administrator()).map_err(host_error)?.map_err(app_error)
        }
    }
}

/// The open book's record, read at the door or under the lease.
fn with_book<T>(door: &mut App, slot: &mut Option<Hosted>, f: impl FnOnce(&mut App) -> Result<T>) -> Result<T> {
    match slot.as_mut() {
        None => f(door),
        Some(h) => {
            let mut lease = lease(&h.shared, TASK_WAIT)?;
            f(lease.host_app())
        }
    }
}

/// This book's hosting folder; `hosting/` itself is made private too (N5).
fn folder(app: &App, record: &HostRecord) -> Result<PathBuf> {
    let root = app.backups.root().map_err(app_error)?.join("hosting");
    private_dir(&root)?;
    Ok(root.join(&record.host_id))
}

fn private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(|_| fail("The hosting folder could not be made."))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|_| fail("The hosting folder could not be made private."))?;
    }
    Ok(())
}

fn problem(kind: &'static str, message: impl Into<String>) -> napi::Error {
    let message = message.into();
    lock(&LAST).problem = Some(Problem { kind, message: message.clone() });
    fail(message)
}

const AUDIT_REPAIR: &str = " An Administrator can set the old log aside on the Hosting screen and start a new one; the old log is kept, but can no longer be checked.";

/// Turn hosting on (by an Administrator `by`) or resume it (`by` None: the record says it was on).
pub(crate) fn start(door: &mut App, bind: &str, host_name: &str, by: Option<&str>) -> Result<()> {
    let mut slot = lock(&HOSTED);
    if slot.is_some() {
        return Err(fail("These books are already being hosted."));
    }
    let helper = HELPER.get().cloned().ok_or_else(|| fail("This copy of FreedomBooks cannot host: its remote-access helper is missing."))?;
    if !door.is_open() {
        return Err(fail("Unlock the books first."));
    }
    if door.backups.viewing_restored().is_some() {
        return Err(fail("A restored copy cannot be hosted. Host the books in use, or choose “Use this as my books” first."));
    }
    settle(door);
    let addr = scope::bind(bind).map_err(fail)?;
    if addr.port() == 0 && by.is_some() && !cfg!(debug_assertions) {
        return Err(fail("Choose a port, for example 47100."));
    }
    lock(&LAST).problem = None;
    let mut record = match door.host_record().map_err(app_error)? {
        Some(r) => r,
        None => HostRecord::fresh().map_err(app_error)?,
    };
    let dir = folder(door, &record)?;
    let gateway_dir = dir.join("gateway");
    private_dir(&dir)?;
    private_dir(&gateway_dir)?;
    let registry_key = record.registry_key_bytes().map_err(app_error)?;
    // The audit log first (opened against the head the record kept), so the list's repair below
    // can always record itself in it (N2).
    let audit_path = dir.join(AUDIT_FILE);
    let keys = RecordKeys(RefCell::new(record.audit_key_bytes()));
    let kept = record.audit_head.as_ref().map(|h| Head { seq: h.seq, hash: h.hash.clone() });
    let log = match audit::open_for_hosting(&audit_path, &keys, record.hosted_before, kept.as_ref()) {
        Ok(log) => log,
        Err(e) if e.to_string() == audit::LOST_KEY => return Err(problem("audit-lost-key", audit::LOST_KEY)),
        Err(e) if e.to_string().contains("not the one the host recorded") => {
            return Err(problem("audit-rolled-back", format!("The remote-access audit log is not the one these books recorded: an earlier copy was put back or it was replaced. Hosting did not start.{AUDIT_REPAIR}")));
        }
        Err(e) => return Err(problem("audit-damaged", format!("The remote-access audit log cannot be used ({e}). Hosting did not start.{AUDIT_REPAIR}"))),
    };
    // The list of paired computers: checked here against the sealed counter before anything starts.
    match registry_file::read(&gateway_dir, &registry_key) {
        Ok(Some(c)) if c.counter < record.device_counter => {
            return Err(problem("devices-rolled-back", format!("The list of paired computers is older than the one these books recorded (it is at change {}, the books recorded change {}): an earlier copy was put back, which could let a revoked computer in again. Hosting did not start. An Administrator can set the list aside on the Hosting screen; every computer then pairs again.", c.counter, record.device_counter)));
        }
        Ok(Some(c)) => {
            for d in c.devices.iter().filter(|d| d.revoked_at.is_some()) {
                record.note_revoked(&d.id);
            }
            record.device_counter = c.counter;
        }
        Ok(None) => {}
        Err(_) => return Err(problem("devices-damaged", "The list of paired computers was changed outside FreedomBooks or is damaged. Hosting did not start. An Administrator can set the list aside on the Hosting screen; every computer then pairs again.")),
    }
    record.audit_key = keys.0.borrow().map(|k| hex(&k));
    record.audit_head = Some(KeptHead { seq: log.head().seq, hash: log.head().hash });
    record.hosted_before = true;
    record.enabled = true;
    record.bind = addr.to_string();
    if let Some(by) = by {
        record.enabled_by = by.to_owned();
    }
    door.save_host_record(&record).map_err(app_error)?;
    let book_path = door.book_path().map(Path::to_path_buf).ok_or_else(|| fail("Unlock the books first."))?;

    // From here the app lives in the host; the door keeps a closed stand-in until hosting ends.
    let app = std::mem::replace(door, App::new());
    let (host, local) = SessionHost::host_with_local(app, Policy::default(), LOCAL_LABEL);
    let shared = SharedHost::new(host);
    let runtime = Arc::new(Runtime {
        shared: shared.clone(),
        registry_dir: gateway_dir.clone(),
        registry_key,
        audit_path: audit_path.clone(),
        pending: Mutex::new(Pending::default()),
        wake: Condvar::new(),
        stopping: AtomicBool::new(false),
        record_error: Mutex::new(None),
        dispatcher: OnceLock::new(),
        pairing: Mutex::new(None),
        audit_lines: AtomicU64::new(log.head().seq),
        counter: AtomicU64::new(record.device_counter),
    });
    let name = fb_remote_proto::audit::clean(host_name);
    let name = if name.trim().is_empty() { "FreedomBooks host".to_owned() } else { name.chars().take(60).collect() };
    let dispatcher = Arc::new(Dispatcher::new(SessionBackend::new(shared.clone()), Config { host_name: name.clone(), ..Config::default() }, Box::new(Recorder { log, runtime: runtime.clone() })));
    let _ = runtime.dispatcher.set(Arc::downgrade(&dispatcher));
    let hook = {
        let runtime = runtime.clone();
        move |counter| runtime.registry_written(counter)
    };
    let started = HostLink::start(
        dispatcher,
        Start {
            exe: helper,
            bind: addr.to_string(),
            identity: record.identity.as_ref().map(|i| fb_remote_proto::wire::IdentityHex { cert: i.cert.clone(), key: i.key.clone() }),
            state_dir: gateway_dir,
            host_name: name,
            registry_key,
            min_counter: record.device_counter,
            revoked: record.revoked.clone(),
            on_registry: Some(Box::new(hook)),
        },
    );
    let mut hosted = Hosted { shared, local, link: None, runtime: runtime.clone(), flusher: None, book_path, bind: addr.to_string() };
    let link = match started {
        Ok(link) => link,
        Err(e) => {
            // Give the app back exactly as it was.
            *slot = Some(hosted);
            end(door, &mut slot, "Hosting could not start.");
            lock(&LAST).stopped = None;
            return Err(fail(e.to_string()));
        }
    };
    // The identity the gateway made on the first start, and the list's counter as it loaded it.
    let saved = {
        let mut lease = lease(&hosted.shared, TASK_WAIT);
        match lease.as_mut() {
            Ok(lease) => (|| {
                let app = lease.host_app();
                let mut r = app.host_record()?.unwrap_or(record);
                if let Some(id) = &link.new_identity {
                    r.identity = Some(Identity { cert: id.cert.clone(), key: id.key.clone() });
                }
                r.device_counter = r.device_counter.max(link.counter);
                app.save_host_record(&r)
            })()
            .map_err(app_error),
            Err(_) => Err(fail("The books are busy.")),
        }
    };
    runtime.counter.fetch_max(link.counter, Ordering::SeqCst);
    *lock(&STATUS) = Some((hosted.bind.clone(), Arc::downgrade(link.dispatcher())));
    hosted.link = Some(link);
    hosted.flusher = Some(std::thread::spawn(move || flush_loop(runtime)));
    *slot = Some(hosted);
    ACTIVE.store(true, Ordering::SeqCst);
    if let Err(e) = saved {
        end(door, &mut slot, "Hosting could not record its identity in the books.");
        return Err(e);
    }
    lock(&LAST).stopped = None;
    Ok(())
}

/// The Administrator turns hosting off.
/// Hosting ends first, so a choice that cannot be saved (a full disk) never leaves it running (N4).
pub(crate) fn stop(door: &mut App) -> Result<()> {
    let mut slot = lock(&HOSTED);
    administrator(door, &mut slot)?;
    end(door, &mut slot, STOPPED);
    let saved = (|| {
        if let Some(mut r) = door.host_record()? {
            r.enabled = false;
            door.save_host_record(&r)?;
        }
        Ok::<_, fb_app::AppError>(())
    })();
    saved.map_err(|e| fail(format!("Hosting stopped, but the choice to keep it off could not be saved in the books ({e}). It would start again the next time the books open here; choose Stop hosting again.")))
}

/// The Administrator's "Restart hosting" (the repair for a paused audit log or record): one call
/// that checks the Administrator before anything stops (N3), then starts on the same address.
pub(crate) fn restart(door: &mut App, host_name: &str) -> Result<()> {
    let (by, bind) = {
        let mut slot = lock(&HOSTED);
        let by = administrator(door, &mut slot)?;
        let bind = match slot.as_ref() {
            Some(h) => h.bind.clone(),
            None => door.host_record().map_err(app_error)?.map(|r| r.bind).unwrap_or_default(),
        };
        if bind.is_empty() {
            return Err(fail("Hosting is not set up."));
        }
        end(door, &mut slot, "The host restarted hosting.");
        (by, bind)
    };
    start(door, &bind, host_name, Some(&by))
}

/// Whether [`resume`] would start hosting: the open books (not a copy) were left hosted and this
/// build can host. Reads the record only.
pub(crate) fn wanted(door: &mut App) -> bool {
    !active() && door.is_open() && door.backups.viewing_restored().is_none() && HELPER.get().is_some() && door.host_record().ok().flatten().is_some_and(|r| r.enabled)
}

/// Start again if the record says hosting is on (after the books open). Never asks anyone.
pub(crate) fn resume(door: &mut App, host_name: &str) -> Result<bool> {
    if active() || !door.is_open() || door.backups.viewing_restored().is_some() || HELPER.get().is_none() {
        return Ok(false);
    }
    let Some(record) = door.host_record().map_err(app_error)? else { return Ok(false) };
    if !record.enabled {
        return Ok(false);
    }
    start(door, &record.bind, host_name, None).map(|()| true)
}

pub(crate) fn turn_on(door: &mut App, bind: &str, host_name: &str) -> Result<()> {
    let by = {
        let mut slot = lock(&HOSTED);
        if slot.is_some() {
            return Err(fail("These books are already being hosted."));
        }
        administrator(door, &mut slot)?
    };
    start(door, bind, host_name, Some(&by))
}

fn running<T>(slot: &mut Option<Hosted>, f: impl FnOnce(&mut Hosted) -> Result<T>) -> Result<T> {
    match slot.as_mut() {
        Some(h) => f(h),
        None => Err(fail("Hosting is off. Turn it on first.")),
    }
}

/// A one-time pairing code (the Administrator's approval). Returns `{code, seconds}`.
pub(crate) fn pair(door: &mut App) -> Result<serde_json::Value> {
    let mut slot = lock(&HOSTED);
    let by = administrator(door, &mut slot)?;
    running(&mut slot, |h| {
        let link = h.link.as_ref().ok_or_else(|| fail("Hosting is stopping."))?;
        let code = link.arm_pairing(&by).map_err(|e| fail(e.to_string()))?;
        let shown = fb_remote_proto::code::display(&code);
        *lock(&h.runtime.pairing) = Some((shown.clone(), Instant::now() + Duration::from_secs(fb_remote_proto::code::LIFETIME_SECONDS)));
        Ok(serde_json::json!({"code": shown, "seconds": fb_remote_proto::code::LIFETIME_SECONDS}))
    })
}

pub(crate) fn cancel_pair(door: &mut App) -> Result<()> {
    let mut slot = lock(&HOSTED);
    administrator(door, &mut slot)?;
    running(&mut slot, |h| {
        *lock(&h.runtime.pairing) = None;
        h.link.as_ref().ok_or_else(|| fail("Hosting is stopping."))?.cancel_pairing().map_err(|e| fail(e.to_string()))
    })
}

/// Revoke a paired computer. Recorded in the sealed record first (so a list put back later cannot
/// re-admit it), then in the book process, then by the listener. Returns what is still outstanding.
pub(crate) fn revoke(door: &mut App, device_id: &str) -> Result<serde_json::Value> {
    if device_id.len() != 32 || !device_id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(fail("No such paired computer."));
    }
    let mut slot = lock(&HOSTED);
    let by = administrator(door, &mut slot)?;
    let recorded = with_book(door, &mut slot, |app| {
        let mut r = app.host_record().map_err(app_error)?.ok_or_else(|| fail("These books have never been hosted."))?;
        r.note_revoked(device_id);
        Ok(app.save_host_record(&r).map_err(|e| e.to_string()))
    })?;
    let record_unsaved = recorded.err().map(|e| format!("The revocation could not be saved in the books ({e}). It is in force now, but would not survive an older list of paired computers being put back. Try again."));
    let Some(h) = slot.as_mut() else {
        // Not hosting: the list of paired computers and the audit log are brought up to date here
        // (S3), so a backup restored later cannot bring the computer back, and the audit log names
        // who revoked it. The sealed record refuses it at the next start in any case.
        let problems = revoke_offline(door, device_id, &by);
        let unsaved = (!problems.is_empty()).then(|| format!("{}. It is recorded in the books and refused when hosting starts, but restoring an older backup before then could let it in again. Try again.", problems.join("; ")));
        return Ok(serde_json::json!({"revoked": record_unsaved.is_none() && unsaved.is_none(), "already": false, "unsaved": unsaved, "record_unsaved": record_unsaved}));
    };
    let link = h.link.as_ref().ok_or_else(|| fail("Hosting is stopping."))?;
    let r = link.revoke(device_id, &by).map_err(|e| fail(e.to_string()))?;
    Ok(serde_json::json!({"revoked": r.revoked, "already": r.already, "unsaved": r.unsaved, "record_unsaved": record_unsaved}))
}

/// A revocation while hosting is off: written into the list of paired computers (counter + 1, with
/// the key the book process keeps), recorded in the audit log with the Administrator's name, and
/// the record moved to both. Returns what could not be written.
fn revoke_offline(door: &mut App, device_id: &str, by: &str) -> Vec<String> {
    let mut problems = vec![];
    let mut record = match door.host_record() {
        Ok(Some(r)) => r,
        _ => return vec!["The hosting record could not be read".into()],
    };
    let (dir, key) = match (folder(door, &record), record.registry_key_bytes()) {
        (Ok(dir), Ok(key)) => (dir, key),
        _ => return vec!["The hosting folder could not be used".into()],
    };
    let gateway = dir.join("gateway");
    let mut name = None;
    match registry_file::read(&gateway, &key) {
        Ok(Some(mut c)) if c.counter >= record.device_counter => {
            if let Some(d) = c.devices.iter_mut().find(|d| d.id == device_id) {
                name = Some(d.name.clone());
                if d.revoked_at.is_none() {
                    d.revoked_at = Some(now_secs());
                    c.counter += 1;
                    match registry_file::write(&gateway, &key, &c) {
                        Ok(()) => record.device_counter = record.device_counter.max(c.counter),
                        Err(e) => problems.push(format!("The list of paired computers could not be written ({e})")),
                    }
                }
            }
        }
        // No list (nothing paired here), or one the next start refuses anyway (older or damaged):
        // the sealed record alone keeps the revocation.
        Ok(_) | Err(_) => {}
    }
    let keys = RecordKeys(RefCell::new(record.audit_key_bytes()));
    let kept = record.audit_head.as_ref().map(|h| Head { seq: h.seq, hash: h.hash.clone() });
    match audit::open_for_hosting(&dir.join(AUDIT_FILE), &keys, record.hosted_before, kept.as_ref()) {
        Ok(mut log) => {
            let event = Event { kind: "device.revoked".into(), device_id: Some(device_id.to_owned()), device_name: name, user: Some(by.to_owned()), detail: "revoked while hosting was off".into(), ..Default::default() };
            match log.append(&event) {
                Ok(()) => {
                    record.audit_key = keys.0.borrow().map(|k| hex(&k));
                    record.audit_head = Some(KeptHead { seq: log.head().seq, hash: log.head().hash });
                }
                Err(e) => problems.push(format!("The remote-access audit log could not be written ({e})")),
            }
        }
        Err(e) => problems.push(format!("The remote-access audit log could not be opened ({e})")),
    }
    record.note_revoked(device_id);
    if let Err(e) = door.save_host_record(&record) {
        problems.push(format!("The hosting record could not be saved ({e})"));
    }
    problems
}

/// An Administrator's repair for a start that was refused, then a start. Returns the name the old
/// file was set aside under, for the Administrator to see.
pub(crate) fn repair(door: &mut App, kind: &str, host_name: &str) -> Result<String> {
    let aside_name;
    {
        let mut slot = lock(&HOSTED);
        if slot.is_some() {
            return Err(fail("Stop hosting first."));
        }
        let by = administrator(door, &mut slot)?;
        // Only the repair for the problem the last start found (N1): never on a healthy file.
        if lock(&LAST).problem.as_ref().map(|p| p.kind) != Some(kind) {
            return Err(fail("There is nothing to repair. Open the Hosting screen again."));
        }
        let mut record = door.host_record().map_err(app_error)?.ok_or_else(|| fail("These books have never been hosted."))?;
        let dir = folder(door, &record)?;
        private_dir(&dir)?;
        match kind {
            "audit-lost-key" | "audit-rolled-back" | "audit-damaged" => {
                let keys = RecordKeys(RefCell::new(None));
                let (label, why) = match kind {
                    "audit-lost-key" => ("lost-key", "the key was lost"),
                    "audit-rolled-back" => ("rolled-back", "the log was not the one the host recorded (an earlier copy was put back or it was replaced)"),
                    _ => ("damaged", "the log could not be used"),
                };
                let (log, aside) = audit::set_aside(&dir.join(AUDIT_FILE), &keys, &by, label, why).map_err(|e| fail(format!("The old audit log could not be set aside: {e}")))?;
                aside_name = aside.display().to_string();
                record.audit_key = keys.0.borrow().map(|k| hex(&k));
                record.audit_head = Some(KeptHead { seq: log.head().seq, hash: log.head().hash });
            }
            "devices-rolled-back" | "devices-damaged" => {
                let label = if kind == "devices-rolled-back" { "older" } else { "damaged" };
                // The audit log is opened first (a start checks it before the list), so setting the
                // list aside is always recorded (N2).
                let keys = RecordKeys(RefCell::new(record.audit_key_bytes()));
                let kept = record.audit_head.as_ref().map(|h| Head { seq: h.seq, hash: h.hash.clone() });
                let mut log = audit::open_for_hosting(&dir.join(AUDIT_FILE), &keys, record.hosted_before, kept.as_ref()).map_err(|e| fail(format!("The audit log cannot be used ({e}); nothing was set aside. Open the Hosting screen again.")))?;
                let aside = registry_file::set_aside(&dir.join("gateway"), label).map_err(|e| fail(format!("The old list of paired computers could not be set aside: {e}")))?;
                let file = aside.as_ref().and_then(|p| p.file_name()).map(|n| format!(" as {}", n.to_string_lossy())).unwrap_or_default();
                aside_name = aside.map(|p| p.display().to_string()).unwrap_or_default();
                log.append(&Event { kind: "devices.set_aside".into(), user: Some(by.clone()), detail: format!("the list of paired computers was set aside ({label}){file}; every computer pairs again"), ..Default::default() })
                    .map_err(|e| fail(format!("The list was set aside as {aside_name}, but the audit log could not be written: {e}")))?;
                record.audit_key = keys.0.borrow().map(|k| hex(&k));
                record.audit_head = Some(KeptHead { seq: log.head().seq, hash: log.head().hash });
            }
            _ => return Err(fail("Unknown repair.")),
        }
        door.save_host_record(&record).map_err(app_error)?;
        lock(&LAST).problem = None;
        let _ = by;
    }
    let bind = door.host_record().map_err(app_error)?.map(|r| r.bind).unwrap_or_default();
    if bind.is_empty() {
        return Ok(aside_name);
    }
    let by = {
        let mut slot = lock(&HOSTED);
        administrator(door, &mut slot)?
    };
    start(door, &bind, host_name, Some(&by)).map(|()| aside_name)
}

/// What the Hosting screen shows. Never waits more than the door does; no secret is included.
pub(crate) fn state(door: &mut App, wait: Duration) -> Result<serde_json::Value> {
    let mut slot = lock(&HOSTED);
    let helper = HELPER.get().is_some();
    let last = {
        let l = lock(&LAST);
        (l.stopped.clone(), l.problem.clone())
    };
    let (admin, generation, company, record, open, copy) = match slot.as_mut() {
        None => {
            settle(door);
            let admin = door.hosting_administrator().ok();
            let record = if door.is_open() { door.host_record().ok().flatten() } else { None };
            (admin, door.session_generation(), fb_app::screens::frame_company(door), record, door.is_open(), door.backups.viewing_restored().is_some())
        }
        Some(h) => {
            let mut lease = lease(&h.shared, wait)?;
            ensure_local(&mut lease, &mut h.local)?;
            let (admin, generation, company) = lease.with_session(&h.local, |app| (app.hosting_administrator().ok(), app.session_generation(), fb_app::screens::frame_company(app))).unwrap_or_default();
            let record = lease.host_app().host_record().ok().flatten();
            (admin, generation, company, record, true, false)
        }
    };
    let Some(admin) = admin else {
        return Ok(serde_json::json!({"available": helper, "open": open, "admin": false}));
    };
    let record = record.unwrap_or_default();
    let registry = match (slot.as_ref(), door.backups.root().ok(), record.registry_key_bytes().ok()) {
        (Some(h), _, Some(key)) => registry_file::read(&h.runtime.registry_dir, &key).ok().flatten(),
        (None, Some(root), Some(key)) if !record.host_id.is_empty() => registry_file::read(&root.join("hosting").join(&record.host_id).join("gateway"), &key).ok().flatten(),
        _ => None,
    };
    let mut out = serde_json::json!({
        "available": helper, "open": open, "admin": true, "admin_name": admin, "generation": generation.to_string(), "company": company.map(|c| c.to_db()), "copy": copy,
        "running": slot.is_some(), "enabled": record.enabled, "bind": record.bind, "enabled_by": record.enabled_by,
        "hosted_before": record.hosted_before, "stopped": last.0, "problem": last.1,
        "audit": {"lines": record.audit_head.as_ref().map_or(0, |h| h.seq), "error": null},
    });
    let mut connected: Vec<String> = vec![];
    let mut dispatcher = None;
    if let Some(h) = slot.as_ref() {
        let link = h.link.as_ref();
        let d = link.map(|l| l.dispatcher().clone());
        let connections = d.as_ref().map(|d| d.connections()).unwrap_or_default();
        let people: std::collections::BTreeSet<_> = connections.iter().filter_map(|(dev, _, user)| user.as_ref().map(|user| (user, &dev.name))).collect();
        out["signed_in"] = serde_json::json!(people);
        connected = connections.into_iter().map(|(dev, _, _)| dev.id).collect();
        dispatcher = d.clone();
        out["addr"] = link.map(|l| l.addr.clone()).into();
        out["fingerprint"] = link.map(|l| l.fingerprint.clone()).into();
        out["bind"] = h.bind.clone().into();
        out["sessions"] = d.as_ref().map_or(0, |d| d.session_count()).into();
        out["audit"] = serde_json::json!({
            "lines": h.runtime.audit_lines.load(Ordering::SeqCst),
            "error": d.as_ref().and_then(|d| d.audit_error()),
            "record_error": lock(&h.runtime.record_error).clone(),
            "path": h.runtime.audit_path.display().to_string(),
        });
        let pairing = lock(&h.runtime.pairing).clone().filter(|(_, until)| *until > Instant::now());
        out["pairing"] = pairing.map(|(code, until)| serde_json::json!({"code": code, "seconds": until.saturating_duration_since(Instant::now()).as_secs()})).into();
    }
    let outstanding = |id: &str| dispatcher.as_ref().and_then(|d| d.revocation(id)).flatten();
    let devices: Vec<serde_json::Value> = registry
        .map(|c| c.devices)
        .unwrap_or_default()
        .into_iter()
        .map(|d| {
            let revoked = d.revoked_at.is_some() || record.revoked.contains(&d.id);
            serde_json::json!({
                "id": d.id, "name": d.name, "paired_at": d.paired_at, "paired_by": d.paired_by,
                "revoked": revoked, "revoked_at": d.revoked_at,
                // Revoked, but not yet in the sealed record: a list put back could re-admit it.
                "recorded": record.revoked.contains(&d.id),
                "last_seen": record.last_seen.get(&d.id),
                "connected": connected.contains(&d.id),
                "unsaved": outstanding(&d.id),
            })
        })
        .collect();
    out["devices"] = devices.into();
    Ok(out)
}

/// The running host's queue, so a test can hold the books as a slow remote request would.
#[cfg(test)]
pub(crate) fn test_shared() -> Option<SharedHost> {
    lock(&HOSTED).as_ref().map(|h| h.shared.clone())
}
