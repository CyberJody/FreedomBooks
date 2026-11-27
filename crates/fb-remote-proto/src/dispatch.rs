//! The host dispatcher: one remote request in, one response out. It binds connections to
//! sessions (each session belongs to the device that opened it), expires idle sessions, applies the
//! route policy before the session layer sees anything, deduplicates retried commands, throttles and
//! audits sign-in, and filters outcomes that only make sense on the host computer.
use crate::audit::{Event, Sink};
use crate::backend::{Backend, BackendError, SessionRef, SignInState};
use crate::dedupe::{Begin, Dedupe, fingerprint};
use crate::policy;
use crate::throttle::Throttle;
use crate::wire::{ClientRequest, DeviceRecord, DeviceRef, HostResponse, RefusalCode, SignedInVm};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

pub struct Config {
    /// Shown on the client's connected-host indicator.
    pub host_name: String,
    /// A remote session with no request for this long is signed out (the desktop's idle lock).
    pub idle: Duration,
    /// How long a retried command waits for its first attempt to finish.
    pub retry_wait: Duration,
    /// Sessions one device may hold at once; the least recently used is ended beyond this.
    pub sessions_per_device: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config { host_name: "FreedomBooks host".into(), idle: Duration::from_secs(15 * 60), retry_wait: Duration::from_secs(60), sessions_per_device: 4 }
    }
}

struct Session {
    device: DeviceRef,
    backend: SessionRef,
    last: Instant,
    user: Option<String>,
    signed_out: bool,
    identity_revision: u64,
    identity_changes: usize,
}

struct Conn {
    device: DeviceRef,
    peer: String,
    session: Option<String>,
}

#[derive(Default)]
struct State {
    sessions: HashMap<String, Session>,
    conns: HashMap<u64, Conn>,
    throttle: Throttle,
    /// Devices with a sign-in being checked right now: one at a time per device, so the throttle's
    /// check and its failure count cannot be raced by parallel connections.
    signing_in: HashSet<String>,
    /// Connections the gateway closed, and when: a request of theirs still being answered must not
    /// open them again. Pruned by `sweep`.
    closed: HashMap<u64, Instant>,
    /// Devices revoked (in this run, or listed as revoked by the gateway): nothing of theirs is
    /// served again. The value is what is still outstanding for the host's screen to show (the
    /// listener has not confirmed it, or could not save it); None once it is saved.
    revoked: HashMap<String, Option<String>>,
    /// Paired devices the book process has seen (their Hello, or the gateway's device list), so a
    /// revocation can be applied and recorded here without waiting for the gateway.
    known: HashMap<String, DeviceRef>,
}

pub struct Dispatcher<B: Backend> {
    backend: B,
    config: Config,
    state: Mutex<State>,
    dedupe: Dedupe,
    audit: Mutex<Box<dyn Sink>>,
    audit_failed: AtomicBool,
    audit_error: Mutex<Option<String>>,
    /// What every request is told while hosting stops ("The host stopped sharing these books.").
    ending: Mutex<Option<String>>,
}

/// How long a closed connection's number is remembered (longer than the gateway waits for an answer).
const CLOSED_MEMORY: Duration = Duration::from_secs(10 * 60);
const EXPIRED: &str = "Your session on the host ended. Sign in again.";
const PAUSED: &str = "Remote access is paused because the host could not keep its records (its audit log or its list of paired computers). Ask the host's administrator to check it.";
/// What a revocation still waits for until the gateway answers it.
pub const NOT_CONFIRMED: &str = "The remote-access listener has not confirmed this revocation yet. The computer is refused by the books now; the listener cuts it off and saves the list of paired computers when it answers.";
const DEVICES_KNOWN: usize = 1024;

fn vm(state: &SignInState) -> Option<SignedInVm> {
    match state {
        SignInState::SignedIn(s) => Some(SignedInVm { user: s.user.clone(), role: s.role.clone() }),
        _ => None,
    }
}

fn from_backend(e: BackendError, signed_in: bool) -> HostResponse {
    match e {
        BackendError::Locked => HostResponse::refused(RefusalCode::HostLocked, "The books are locked on the host computer. Ask someone there to unlock them, then try again."),
        BackendError::Busy => HostResponse::refused(RefusalCode::Busy, "The books are busy on the host (for example a backup is running). Wait a moment, then try again."),
        BackendError::NoSession => HostResponse::refused(RefusalCode::Unauthenticated, EXPIRED),
        // The app's own words are shown only to a signed-in person: before sign-in a refusal must
        // not describe anything in the books.
        BackendError::Refused(message) if signed_in => HostResponse::refused(RefusalCode::Refused, message),
        BackendError::Refused(_) => HostResponse::refused(RefusalCode::Refused, "The host refused this request."),
    }
}

impl<B: Backend> Dispatcher<B> {
    pub fn new(backend: B, config: Config, audit: Box<dyn Sink>) -> Self {
        Dispatcher { backend, config, state: Mutex::new(State::default()), dedupe: Dedupe::default(), audit: Mutex::new(audit), audit_failed: AtomicBool::new(false), audit_error: Mutex::new(None), ending: Mutex::new(None) }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Record one event. The first failure to write stops remote access (fail closed): every
    /// request is refused, every remote session ends, nothing more is written, and
    /// [`Dispatcher::audit_error`] says why until hosting is started again on a repaired log.
    pub fn audit(&self, event: Event) {
        if self.audit_failed.load(Ordering::SeqCst) {
            return;
        }
        let failed = {
            let mut sink = self.audit.lock().unwrap_or_else(PoisonError::into_inner);
            sink.record(&event).err()
        };
        if let Some(e) = failed
            && !self.audit_failed.swap(true, Ordering::SeqCst)
        {
            *self.audit_error.lock().unwrap_or_else(PoisonError::into_inner) = Some(e.to_string());
            self.end_all("the audit log could not be written");
        }
    }

    /// Stop remote access for a reason other than the audit log (the host's sealed record could not
    /// be written, or the list of paired computers on disk no longer matches it): fail closed exactly
    /// as an audit write failure does, and say why in [`Dispatcher::audit_error`].
    pub fn pause(&self, reason: &str) {
        self.audit(Event { kind: "host.paused".into(), detail: reason.into(), ..Default::default() });
        if !self.audit_failed.swap(true, Ordering::SeqCst) {
            *self.audit_error.lock().unwrap_or_else(PoisonError::into_inner) = Some(reason.to_owned());
            self.end_all(reason);
        }
    }

    /// Hosting is stopping: every request from now on is refused with `reason`.
    pub fn set_ending(&self, reason: &str) {
        *self.ending.lock().unwrap_or_else(PoisonError::into_inner) = Some(reason.to_owned());
    }

    /// Computers the book process will refuse whatever the gateway says (the host's sealed record
    /// keeps them, so a list of paired computers put back from an older copy cannot re-admit them).
    pub fn learn_revoked(&self, ids: &[String]) {
        let mut state = self.state();
        for id in ids {
            state.revoked.entry(id.clone()).or_insert(None);
        }
    }

    /// Every device id revoked in the book process (for the host's sealed record).
    pub fn revoked_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.state().revoked.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Open connections (for stopping politely).
    pub fn connected(&self) -> usize {
        self.state().conns.len()
    }

    /// Why remote access stopped, if the audit log could not be written. The host's screen shows
    /// it; restarting hosting reopens (and repairs) the log.
    pub fn audit_error(&self) -> Option<String> {
        self.audit_error.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn event(&self, kind: &str, device: &DeviceRef, peer: &str, user: Option<&str>, detail: &str) {
        self.audit(Event { kind: kind.into(), peer: peer.into(), device_id: Some(device.id.clone()), device_name: Some(device.name.clone()), user: user.map(str::to_owned), detail: detail.into() });
    }

    /// Answer one request from connection `conn`, which the gateway authenticated as `device`.
    pub fn handle(&self, conn: u64, device: &DeviceRef, peer: &str, request: ClientRequest) -> HostResponse {
        if let Some(reason) = self.ending.lock().unwrap_or_else(PoisonError::into_inner).clone() {
            return HostResponse::refused(RefusalCode::HostLocked, reason);
        }
        if self.audit_failed.load(Ordering::SeqCst) {
            return HostResponse::refused(RefusalCode::Busy, PAUSED);
        }
        {
            let mut state = self.state();
            if state.revoked.contains_key(&device.id) || state.closed.contains_key(&conn) {
                return HostResponse::refused(RefusalCode::Unauthenticated, "This connection has ended.");
            }
            // A connection is registered by its Hello; nothing else opens one again.
            if matches!(request, ClientRequest::Hello { .. }) {
                state.conns.entry(conn).or_insert_with(|| Conn { device: device.clone(), peer: peer.to_owned(), session: None });
                if state.known.len() < DEVICES_KNOWN || state.known.contains_key(&device.id) {
                    state.known.insert(device.id.clone(), device.clone());
                }
            }
            if state.conns.get(&conn).is_some_and(|c| c.device.id != device.id) {
                return HostResponse::refused(RefusalCode::Invalid, "This connection belongs to another computer.");
            }
        }
        match request {
            ClientRequest::Ping => HostResponse::Pong,
            ClientRequest::Hello { resume } => self.hello(conn, device, peer, resume),
            other => {
                let (token, session) = match self.session_of(conn, device, peer) {
                    Ok(found) => found,
                    Err(response) => return response,
                };
                match other {
                    ClientRequest::Render { target } => self.render(&token, session, &target),
                    ClientRequest::Run { request_id, command } => self.run(&token, session, device, peer, &request_id, &command),
                    ClientRequest::Query { query } => self.query(&token, session, &query),
                    ClientRequest::SignOut => self.sign_out(&token, session, device, peer),
                    ClientRequest::Ping | ClientRequest::Hello { .. } => HostResponse::Pong,
                }
            }
        }
    }

    fn idle(&self, now: Instant, session: &Session, attached: bool) -> bool {
        now.saturating_duration_since(session.last) >= self.config.idle
            && !(attached && session.signed_out && session.identity_changes == 0)
    }

    fn hello(&self, conn: u64, device: &DeviceRef, peer: &str, resume: Option<String>) -> HostResponse {
        let now = Instant::now();
        let resumed = resume.and_then(|token| {
            let mut state = self.state();
            let live = state.sessions.get(&token).is_some_and(|s| {
                s.device.id == device.id && !self.idle(now, s, state.conns.values().any(|c| c.session.as_ref() == Some(&token)))
            });
            if !live {
                return None;
            }
            let session = state.sessions.get_mut(&token)?;
            session.last = now;
            let backend = session.backend;
            if let Some(c) = state.conns.get_mut(&conn) {
                c.session = Some(token.clone());
            }
            Some((token, backend))
        });
        let (token, backend, was_resumed) = match resumed {
            Some((token, backend)) => (token, backend, true),
            None => {
                let backend = match self.backend.open_session(device) {
                    Ok(s) => s,
                    Err(e) => return from_backend(e, false),
                };
                let Ok(bytes) = crate::random::<32>() else {
                    self.backend.close_session(backend);
                    return HostResponse::refused(RefusalCode::Busy, "The host could not start a session. Try again.");
                };
                let token = crate::hex(&bytes);
                let evicted = {
                    let mut state = self.state();
                    state.sessions.insert(token.clone(), Session { device: device.clone(), backend, last: now, user: None, signed_out: false, identity_revision: 0, identity_changes: 0 });
                    if let Some(c) = state.conns.get_mut(&conn) {
                        c.session = Some(token.clone());
                    }
                    // Keep this device under its session cap: end its least recently used others.
                    let mut mine: Vec<(Instant, String)> = state.sessions.iter().filter(|(t, s)| s.device.id == device.id && **t != token).map(|(t, s)| (s.last, t.clone())).collect();
                    mine.sort();
                    let excess = (mine.len() + 1).saturating_sub(self.config.sessions_per_device);
                    mine.into_iter().take(excess).filter_map(|(_, t)| state.sessions.remove(&t).map(|s| (t, s.backend))).collect::<Vec<_>>()
                };
                for (t, s) in evicted {
                    self.dedupe.forget_session(&device.id, &t, None);
                    self.backend.close_session(s);
                }
                self.event("session.opened", device, peer, None, "");
                (token, backend, false)
            }
        };
        let signed_in = self.observed_sign_in(&token, backend, false).ok().as_ref().and_then(vm);
        HostResponse::Session { session: token, signed_in, host_name: self.config.host_name.clone(), resumed: was_resumed }
    }

    fn session_of(&self, conn: u64, device: &DeviceRef, peer: &str) -> Result<(String, SessionRef), HostResponse> {
        let now = Instant::now();
        let mut state = self.state();
        let token = state.conns.get(&conn).and_then(|c| c.session.clone()).ok_or_else(|| HostResponse::refused(RefusalCode::Unauthenticated, "Connect to the host first."))?;
        let Some(session) = state.sessions.get_mut(&token) else {
            return Err(HostResponse::refused(RefusalCode::Unauthenticated, EXPIRED));
        };
        if session.device.id != device.id {
            return Err(HostResponse::refused(RefusalCode::Unauthenticated, EXPIRED));
        }
        if self.idle(now, session, true) {
            let expired = state.sessions.remove(&token);
            drop(state);
            if let Some(s) = expired {
                self.dedupe.forget_session(&device.id, &token, None);
                self.backend.close_session(s.backend);
                self.event("session.expired", device, peer, s.user.as_deref(), "idle");
            }
            return Err(HostResponse::refused(RefusalCode::Unauthenticated, EXPIRED));
        }
        session.last = now;
        Ok((token, session.backend))
    }

    fn begin_identity_change(&self, token: &str) {
        if let Some(s) = self.state().sessions.get_mut(token) {
            s.identity_changes += 1;
            s.identity_revision += 1;
            s.signed_out = false;
        }
    }

    fn observed_sign_in(&self, token: &str, session: SessionRef, finishing_change: bool) -> Result<SignInState, BackendError> {
        let expected_changes = usize::from(finishing_change);
        let revision = self.state().sessions.get(token).filter(|s| s.identity_changes == expected_changes).map(|s| s.identity_revision);
        let observed = self.backend.sign_in_state(session);
        if let Some(s) = self.state().sessions.get_mut(token) {
            if revision == Some(s.identity_revision) && s.identity_changes == expected_changes
                && let Ok(identity) = &observed
            {
                s.signed_out = matches!(identity, SignInState::SignedOut);
            }
            if finishing_change {
                s.identity_changes -= 1;
            }
        }
        observed
    }

    fn render(&self, token: &str, session: SessionRef, target: &str) -> HostResponse {
        let state = match self.observed_sign_in(token, session, false) {
            Ok(s) => s,
            Err(e) => return from_backend(e, false),
        };
        let signed_in = matches!(state, SignInState::SignedIn(_));
        match policy::render(target, signed_in, self.backend.classify(policy::path_of(target))) {
            Ok(()) => {}
            // Any page before sign-in goes to the sign-in screen, as on the desktop.
            Err((RefusalCode::SignInRequired, _)) => return HostResponse::Page { html: policy::SIGN_IN.into(), status: 303, route: "sign-in".into() },
            Err((code, message)) => return HostResponse::refused(code, message),
        }
        match self.backend.render(session, target) {
            Ok(r) => {
                if (300..400).contains(&r.status) && (!r.html.starts_with('/') || r.html.starts_with("//")) {
                    return HostResponse::refused(RefusalCode::Refused, "The host answered with an invalid address.");
                }
                HostResponse::Page { html: r.html, status: r.status, route: r.route }
            }
            Err(e) => from_backend(e, signed_in),
        }
    }

    fn run(&self, token: &str, session: SessionRef, device: &DeviceRef, peer: &str, request_id: &str, command: &str) -> HostResponse {
        if !crate::is_request_id(request_id) {
            return HostResponse::refused(RefusalCode::Invalid, "This request could not be read. Nothing was saved.");
        }
        let before = match self.observed_sign_in(token, session, false) {
            Ok(s) => s,
            Err(e) => return from_backend(e, false),
        };
        let user = match &before {
            SignInState::SignedIn(s) => Some(s),
            _ => None,
        };
        let info = match policy::command(command, user.is_some(), |path| self.backend.classify(path)) {
            Ok(info) => info,
            Err((code, message)) => return HostResponse::refused(code, message),
        };
        if info.sign_in {
            let mut state = self.state();
            if let Err(wait) = state.throttle.check(&device.id, Instant::now()) {
                drop(state);
                self.event("sign_in.throttled", device, peer, None, "");
                let minutes = wait.as_secs().div_ceil(60).max(1);
                return HostResponse::refused(RefusalCode::Throttled, format!("Too many unsuccessful sign-ins from this computer. Wait {minutes} minute{}, then try again.", if minutes == 1 { "" } else { "s" }));
            }
            if !state.signing_in.insert(device.id.clone()) {
                return HostResponse::refused(RefusalCode::Busy, "A sign-in from this computer is already being checked. Wait a moment, then try again.");
            }
        }
        let changes_identity = info.sign_in || info.path.as_deref() == Some(policy::SESSION);
        if changes_identity {
            self.begin_identity_change(token);
        }
        let response = self.run_once(token, session, device, peer, request_id, command, &info, user.map(|u| u.user_id.as_str()), user.map(|u| u.user.as_str()));
        let after = changes_identity.then(|| self.observed_sign_in(token, session, true));
        if info.sign_in {
            self.after_sign_in(token, device, peer, request_id, &before, after.and_then(Result::ok).unwrap_or(SignInState::SignedOut));
            self.state().signing_in.remove(&device.id);
        }
        response
    }

    #[allow(clippy::too_many_arguments)]
    fn run_once(&self, token: &str, session: SessionRef, device: &DeviceRef, peer: &str, request_id: &str, command: &str, info: &policy::CommandInfo, user_id: Option<&str>, user: Option<&str>) -> HostResponse {
        let signed_in = user_id.is_some();
        let execute = || match self.backend.run(session, command) {
            Ok(outcome) => match policy::outcome(&outcome) {
                Ok(()) => HostResponse::Outcome { outcome },
                Err((code, message)) => {
                    // The route policy should have refused this command before it ran.
                    self.event("policy.outcome_withheld", device, peer, user, message);
                    HostResponse::refused(code, message)
                }
            },
            Err(e) => from_backend(e, signed_in),
        };
        // A read-only export changes nothing when repeated: it is not kept for retries.
        if info.export {
            return execute();
        }
        // A sign-in is fingerprinted as sent (nobody signed in yet), so its retry after it
        // succeeded is answered; the change of user keeps only this entry (`after_sign_in`).
        let fingerprint_user = if info.sign_in { "" } else { user_id.unwrap_or("") };
        match self.dedupe.begin(&device.id, token, request_id, fingerprint(token, fingerprint_user, command), self.config.retry_wait) {
            Begin::Run => {}
            Begin::Done(response) => {
                self.event("request.replayed", device, peer, user, request_id);
                return response;
            }
            Begin::Conflict => return HostResponse::refused(RefusalCode::Conflict, "This request was already sent for something else. Open this page again and check before trying again."),
            Begin::StillRunning => return HostResponse::refused(RefusalCode::Busy, "The host is still working on this request. Wait a moment; it will not be done twice."),
        }
        let response = execute();
        self.dedupe.finish(&device.id, request_id, response.clone());
        response
    }

    fn after_sign_in(&self, token: &str, device: &DeviceRef, peer: &str, request_id: &str, before: &SignInState, after: SignInState) {
        if after != *before {
            // Whoever is signed in changed: no answer stored under the old sign-in may be replayed.
            self.dedupe.forget_session(&device.id, token, Some(request_id));
        }
        match (&before, &after) {
            (_, SignInState::SignedIn(user)) => {
                let mut state = self.state();
                if let Some(s) = state.sessions.get_mut(token) {
                    s.user = Some(user.user.clone());
                }
                if !matches!(before, SignInState::SignedIn(_)) {
                    state.throttle.success(&device.id);
                    drop(state);
                    self.event("sign_in.succeeded", device, peer, Some(&user.user), &user.role);
                }
            }
            (SignInState::SignedOut, SignInState::MfaPending) => self.event("sign_in.password_accepted", device, peer, None, "second factor required"),
            _ => {
                let waits = self.state().throttle.failure(&device.id, Instant::now());
                self.event("sign_in.failed", device, peer, None, if waits { "throttled" } else { "" });
            }
        }
    }

    fn query(&self, token: &str, session: SessionRef, query: &str) -> HostResponse {
        match self.observed_sign_in(token, session, false) {
            Ok(SignInState::SignedIn(_)) => {}
            Ok(_) => return HostResponse::refused(RefusalCode::SignInRequired, policy::SIGN_IN_TEXT),
            Err(e) => return from_backend(e, false),
        }
        if query.len() > 256 * 1024 || crate::strict::parse::<serde_json::Value>(query).is_err() {
            return HostResponse::refused(RefusalCode::Invalid, "This request could not be read.");
        }
        match self.backend.query(session, query) {
            Ok(data) => HostResponse::Data { data },
            Err(e) => from_backend(e, true),
        }
    }

    fn sign_out(&self, token: &str, session: SessionRef, device: &DeviceRef, peer: &str) -> HostResponse {
        self.begin_identity_change(token);
        let user = self.state().sessions.get_mut(token).and_then(|s| s.user.take());
        // Whatever happens next, nothing this session's user was answered can be replayed.
        self.dedupe.forget_session(&device.id, token, None);
        match self.backend.sign_out(session) {
            Ok(()) => {
                let _ = self.observed_sign_in(token, session, true);
                self.event("sign_out", device, peer, user.as_deref(), "");
                HostResponse::SignedOut
            }
            Err(e) => {
                // The books could not sign out now (busy, locked): end the whole session instead,
                // so nobody stays signed in on it (fb-host ends it once the books are free).
                let ended = self.state().sessions.remove(token);
                if let Some(s) = ended {
                    self.backend.close_session(s.backend);
                }
                let why = match e {
                    BackendError::Busy => "the books were busy",
                    BackendError::Locked => "the books were locked",
                    BackendError::NoSession => "the session had already ended",
                    BackendError::Refused(_) => "the sign-out was refused",
                };
                self.event("sign_out", device, peer, user.as_deref(), &format!("session ended ({why})"));
                HostResponse::SignedOut
            }
        }
    }

    /// The gateway closed connection `conn`. Its session stays resumable until it idles out.
    pub fn disconnected(&self, conn: u64) {
        let mut state = self.state();
        state.conns.remove(&conn);
        state.closed.insert(conn, Instant::now());
    }

    /// Revoke a device in the book process: from now on nothing of its is served, its sessions end
    /// and its answers are forgotten, and `device.revoked` is recorded with the Administrator
    /// `by`. This does not wait for the gateway (which is told next); until it confirms, the
    /// revocation is outstanding ([`NOT_CONFIRMED`]). False: it was already revoked here.
    pub fn revoke_device(&self, device: &DeviceRef, by: &str) -> bool {
        let ended: Vec<SessionRef> = {
            let mut state = self.state();
            if state.revoked.contains_key(&device.id) {
                return false;
            }
            state.revoked.insert(device.id.clone(), Some(NOT_CONFIRMED.into()));
            state.conns.retain(|_, c| c.device.id != device.id);
            let tokens: Vec<String> = state.sessions.iter().filter(|(_, s)| s.device.id == device.id).map(|(t, _)| t.clone()).collect();
            tokens.iter().filter_map(|t| state.sessions.remove(t)).map(|s| s.backend).collect()
        };
        for s in ended {
            self.backend.close_session(s);
        }
        self.dedupe.forget_device(&device.id);
        self.event("device.revoked", device, "", Some(by).filter(|b| !b.is_empty()), "");
        true
    }

    /// The gateway answered the revocation of `device_id` (whenever its answer arrives). `device`
    /// Some: it has cut the device off, and saved the list of paired computers unless `unsaved`
    /// says why not. None: it had no active device by that id (nothing to cut off or save).
    pub fn revocation_answered(&self, device_id: &str, device: Option<&DeviceRef>, unsaved: Option<&str>) {
        let Some(device) = device else {
            if let Some(r) = self.state().revoked.get_mut(device_id) {
                *r = None;
            }
            return;
        };
        // A revocation the book process did not start (none today) is applied here too.
        self.revoke_device(device, "");
        if let Some(r) = self.state().revoked.get_mut(&device.id) {
            *r = unsaved.map(str::to_owned);
        }
        match unsaved {
            Some(why) => self.event("device.revocation_unsaved", device, "", None, &format!("in force now but not saved: {why}")),
            None => self.event("device.revocation_confirmed", device, "", None, ""),
        }
    }

    /// The gateway saved the revocations it could not save before.
    pub fn revocations_saved(&self) {
        for r in self.state().revoked.values_mut() {
            if r.as_deref() != Some(NOT_CONFIRMED) {
                *r = None;
            }
        }
    }

    /// Whether `device_id` is revoked, and what is still outstanding (None: saved).
    pub fn revocation(&self, device_id: &str) -> Option<Option<String>> {
        self.state().revoked.get(device_id).cloned()
    }

    /// A paired device the book process has seen, by id.
    pub fn known_device(&self, device_id: &str) -> Option<DeviceRef> {
        self.state().known.get(device_id).cloned()
    }

    /// Learn the gateway's list of paired computers: active ones become known, revoked ones are
    /// refused here too (already saved, so nothing is outstanding for them).
    pub fn learn_devices(&self, devices: &[DeviceRecord]) {
        let mut state = self.state();
        for d in devices {
            if d.revoked_at.is_some() {
                state.revoked.entry(d.id.clone()).or_insert(None);
            } else if state.known.len() < DEVICES_KNOWN || state.known.contains_key(&d.id) {
                state.known.insert(d.id.clone(), DeviceRef { id: d.id.clone(), name: d.name.clone() });
            }
        }
    }

    /// End sessions that have idled out. Called periodically by the link.
    pub fn sweep(&self) {
        let now = Instant::now();
        let ended: Vec<(String, Session)> = {
            let mut state = self.state();
            state.closed.retain(|_, at| now.duration_since(*at) < CLOSED_MEMORY);
            let attached: HashSet<_> = state.conns.values().filter_map(|c| c.session.as_ref()).collect();
            let tokens: Vec<String> = state.sessions.iter().filter(|(t, s)| self.idle(now, s, attached.contains(t))).map(|(t, _)| t.clone()).collect();
            tokens.into_iter().filter_map(|t| state.sessions.remove(&t).map(|s| (t, s))).collect()
        };
        for (t, s) in ended {
            self.dedupe.forget_session(&s.device.id, &t, None);
            self.backend.close_session(s.backend);
            self.event("session.expired", &s.device, "", s.user.as_deref(), "idle");
        }
    }

    /// End every remote session (hosting stopped).
    pub fn end_all(&self, reason: &str) {
        let ended: Vec<(String, Session)> = {
            let mut state = self.state();
            state.conns.clear();
            state.sessions.drain().collect()
        };
        for (t, s) in ended {
            self.dedupe.forget_session(&s.device.id, &t, None);
            self.backend.close_session(s.backend);
            self.event("session.ended", &s.device, "", s.user.as_deref(), reason);
        }
    }

    pub fn session_count(&self) -> usize {
        self.state().sessions.len()
    }

    /// The peer address a connection came from (for the host's "who is connected" list).
    pub fn connections(&self) -> Vec<(DeviceRef, String, Option<String>)> {
        let state = self.state();
        state.conns.values().map(|c| (c.device.clone(), c.peer.clone(), c.session.as_ref().and_then(|t| state.sessions.get(t)).and_then(|s| s.user.clone()))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{Remote, Rendered, SignedIn};
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

    /// A minimal book: sessions with a signed-in user, a counter of commands that reached it.
    #[derive(Default)]
    struct Fake {
        users: Mutex<HashMap<u64, SignInState>>,
        next: AtomicU64,
        runs: AtomicU64,
        status_reads: AtomicU64,
        slow_sign_in: AtomicBool,
        busy_sign_out: AtomicBool,
    }

    impl Backend for Arc<Fake> {
        fn classify(&self, path: &str) -> Option<Remote> {
            match path {
                "/desktop/sign-in/" | "/desktop/session/" | "/desktop/users/" | "/c/x/home/" => Some(Remote::Allowed),
                "/c/x/backups/" => Some(Remote::HostOnly),
                _ => None,
            }
        }
        fn open_session(&self, _: &DeviceRef) -> Result<SessionRef, BackendError> {
            let n = self.next.fetch_add(1, Ordering::SeqCst);
            self.users.lock().unwrap().insert(n, SignInState::SignedOut);
            Ok(SessionRef(n))
        }
        fn close_session(&self, s: SessionRef) {
            self.users.lock().unwrap().remove(&s.0);
        }
        fn sign_in_state(&self, s: SessionRef) -> Result<SignInState, BackendError> {
            self.status_reads.fetch_add(1, Ordering::SeqCst);
            self.users.lock().unwrap().get(&s.0).cloned().ok_or(BackendError::NoSession)
        }
        fn render(&self, _: SessionRef, target: &str) -> Result<Rendered, BackendError> {
            Ok(Rendered { html: format!("page {target}"), status: 200, route: "r".into() })
        }
        fn run(&self, s: SessionRef, command: &str) -> Result<String, BackendError> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            let v: serde_json::Value = serde_json::from_str(command).unwrap();
            let field = |n: &str| v["fields"].as_array().into_iter().flatten().find(|f| f[0] == n).and_then(|f| f[1].as_str()).map(str::to_owned);
            if v["path"] == "/desktop/sign-in/" {
                if field("action").as_deref() == Some("cancel_mfa") {
                    self.sign_out(s)?;
                    return Ok(r#"{"outcome":"redirect","location":"/desktop/sign-in/"}"#.into());
                }
                if self.slow_sign_in.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(200));
                }
                let user = field("username").unwrap();
                if field("password").as_deref() == Some("right") {
                    if user == "mfa" {
                        self.users.lock().unwrap().insert(s.0, SignInState::MfaPending);
                        return Ok(r#"{"outcome":"redirect","location":"/desktop/sign-in/"}"#.into());
                    }
                    let role = if user == "admin" { "admin" } else { "bookkeeper" };
                    self.users.lock().unwrap().insert(s.0, SignInState::SignedIn(SignedIn { user_id: user.clone(), user, role: role.into() }));
                    return Ok(r#"{"outcome":"redirect","location":"/c/x/home/"}"#.into());
                }
                return Ok(r#"{"outcome":"invalid","html":"no"}"#.into());
            }
            if v["path"] == "/desktop/session/" {
                self.sign_out(s)?;
                return Ok(r#"{"outcome":"redirect","location":"/desktop/sign-in/"}"#.into());
            }
            if v["path"] == "/desktop/users/" {
                let users = self.users.lock().unwrap();
                return match users.get(&s.0) {
                    Some(SignInState::SignedIn(user)) if user.role == "admin" => Ok(r#"{"outcome":"page","html":"the administrator's user editor"}"#.into()),
                    _ => Err(BackendError::Refused("An administrator is required for this action.".into())),
                };
            }
            Ok(r#"{"outcome":"done","data":{}}"#.into())
        }
        fn query(&self, _: SessionRef, _: &str) -> Result<String, BackendError> {
            Ok("[]".into())
        }
        fn sign_out(&self, s: SessionRef) -> Result<(), BackendError> {
            if self.busy_sign_out.load(Ordering::SeqCst) {
                return Err(BackendError::Busy);
            }
            self.users.lock().unwrap().insert(s.0, SignInState::SignedOut);
            Ok(())
        }
    }

    const ID: &str = "211ce6fc-4559-46d4-91ea-c81b0b41ff93";

    type Setup = (Arc<Fake>, Dispatcher<Arc<Fake>>, DeviceRef, Arc<Mutex<Vec<Event>>>);

    fn setup() -> Setup {
        let fake = Arc::new(Fake::default());
        let events = Arc::new(Mutex::new(vec![]));
        let d = Dispatcher::new(fake.clone(), Config::default(), Box::new(crate::audit::Memory(events.clone())));
        (fake, d, DeviceRef { id: "dev".into(), name: "Shared PC".into() }, events)
    }

    fn post(path: &str, fields: &[(&str, &str)]) -> String {
        serde_json::json!({"type": "SubmitForm", "path": path, "token": "t", "fields": fields.iter().map(|(k, v)| [k, v]).collect::<Vec<_>>()}).to_string()
    }

    fn run(d: &Dispatcher<Arc<Fake>>, dev: &DeviceRef, id: &str, command: &str) -> HostResponse {
        d.handle(1, dev, "10.0.0.2", ClientRequest::Run { request_id: id.into(), command: command.into() })
    }

    fn sign_in(d: &Dispatcher<Arc<Fake>>, dev: &DeviceRef, user: &str) -> HostResponse {
        run(d, dev, &uuid(), &post("/desktop/sign-in/", &[("action", "signin"), ("username", user), ("password", "right")]))
    }

    fn uuid() -> String {
        let b = crate::random::<16>().unwrap();
        let h = crate::hex(&b);
        format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
    }

    fn hello(d: &Dispatcher<Arc<Fake>>, dev: &DeviceRef, conn: u64, resume: Option<String>) -> (String, bool) {
        let answer = d.handle(conn, dev, "p", ClientRequest::Hello { resume });
        let HostResponse::Session { session, resumed, .. } = answer else { panic!("{answer:?}") };
        (session, resumed)
    }

    fn backdate(d: &Dispatcher<Arc<Fake>>, token: &str) {
        d.state().sessions.get_mut(token).unwrap().last = Instant::now() - d.config.idle;
    }

    fn sign_in_page() -> ClientRequest {
        ClientRequest::Render { target: policy::SIGN_IN.into() }
    }

    #[test]
    fn attached_sign_in_page_survives_idle_and_first_password_reaches_book() {
        let (fake, d, dev, _) = setup();
        let (token, _) = hello(&d, &dev, 1, None);
        assert!(matches!(d.handle(1, &dev, "p", sign_in_page()), HostResponse::Page { status: 200, .. }));
        backdate(&d, &token);
        let reads = fake.status_reads.load(Ordering::SeqCst);
        d.sweep();
        assert_eq!(fake.status_reads.load(Ordering::SeqCst), reads);
        assert!(matches!(sign_in(&d, &dev, "keeper"), HostResponse::Outcome { outcome } if outcome.contains("/c/x/home/")));
        assert_eq!(fake.runs.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn attached_signed_out_resume_survives_until_last_connection_goes() {
        let (fake, d, dev, _) = setup();
        let (token, _) = hello(&d, &dev, 1, None);
        backdate(&d, &token);
        assert_eq!(hello(&d, &dev, 2, Some(token.clone())), (token.clone(), true));
        d.disconnected(1);
        backdate(&d, &token);
        d.sweep();
        assert_eq!(d.session_count(), 1);
        d.disconnected(2);
        let (fresh, resumed) = hello(&d, &dev, 3, Some(token.clone()));
        assert!(!resumed);
        assert_ne!(fresh, token);
        d.sweep();
        assert!(!d.state().sessions.contains_key(&token));
        assert_eq!(fake.users.lock().unwrap().len(), 1);
        d.disconnected(3);
        assert_eq!(hello(&d, &dev, 4, Some(fresh.clone())), (fresh, true));
    }

    #[test]
    fn signed_in_and_mfa_pending_sessions_still_expire_by_request_sweep_and_resume() {
        for user in ["keeper", "mfa"] {
            for path in ["request", "sweep", "resume"] {
                let (fake, d, dev, _) = setup();
                let (token, _) = hello(&d, &dev, 1, None);
                assert!(matches!(sign_in(&d, &dev, user), HostResponse::Outcome { .. }));
                backdate(&d, &token);
                if path == "resume" {
                    assert!(!hello(&d, &dev, 2, Some(token.clone())).1);
                    d.sweep();
                } else {
                    if path == "sweep" {
                        d.sweep();
                    }
                    assert!(matches!(d.handle(1, &dev, "p", sign_in_page()), HostResponse::Refused { code: RefusalCode::Unauthenticated, .. }));
                }
                assert!(!d.state().sessions.contains_key(&token), "{user} {path}");
                assert!(!fake.users.lock().unwrap().contains_key(&0));
            }
        }
    }

    #[test]
    fn sign_out_and_cancel_mfa_restore_attached_idle_exemption() {
        for (user, path, action) in [("keeper", "", ""), ("keeper", policy::SESSION, "signout"), ("mfa", policy::SIGN_IN, "cancel_mfa")] {
            let (_, d, dev, _) = setup();
            let (token, _) = hello(&d, &dev, 1, None);
            sign_in(&d, &dev, user);
            if path.is_empty() {
                assert_eq!(d.handle(1, &dev, "p", ClientRequest::SignOut), HostResponse::SignedOut);
            } else {
                assert!(matches!(run(&d, &dev, &uuid(), &post(path, &[("action", action)])), HostResponse::Outcome { .. }));
            }
            backdate(&d, &token);
            d.sweep();
            assert_eq!(d.session_count(), 1, "{action}");
        }
    }

    #[test]
    fn attached_signed_out_sessions_still_respect_cap_revocation_and_shutdown() {
        let (fake, d, dev, _) = setup();
        let (oldest, _) = hello(&d, &dev, 1, None);
        backdate(&d, &oldest);
        for conn in 2..=d.config.sessions_per_device as u64 + 1 {
            hello(&d, &dev, conn, None);
        }
        assert_eq!(d.session_count(), d.config.sessions_per_device);
        assert!(!d.state().sessions.contains_key(&oldest));
        let other = DeviceRef { id: "other".into(), name: "Other synthetic computer".into() };
        hello(&d, &other, 10, None);
        assert!(d.revoke_device(&dev, "admin"));
        assert_eq!(d.session_count(), 1);
        d.end_all("synthetic shutdown");
        assert_eq!(d.session_count(), 0);
        assert!(fake.users.lock().unwrap().is_empty());
    }

    /// The reviewer's [E] sequence (S1): an Administrator's stored answer must never be replayed
    /// to the next person signed in on the same computer and session.
    #[test]
    fn a_stored_answer_is_never_replayed_to_another_signed_in_user() {
        let (_fake, d, dev, events) = setup();
        assert!(matches!(d.handle(1, &dev, "p", ClientRequest::Hello { resume: None }), HostResponse::Session { .. }));
        assert!(matches!(sign_in(&d, &dev, "admin"), HostResponse::Outcome { .. }));
        let users = post("/desktop/users/", &[("action", "edit")]);
        let first = run(&d, &dev, ID, &users);
        assert!(matches!(&first, HostResponse::Outcome { outcome } if outcome.contains("user editor")));
        assert_eq!(run(&d, &dev, ID, &users), first, "the administrator's own retry is answered");
        assert!(matches!(d.handle(1, &dev, "p", ClientRequest::SignOut), HostResponse::SignedOut));
        assert!(matches!(run(&d, &dev, ID, &users), HostResponse::Refused { code: RefusalCode::SignInRequired, .. }));
        assert!(matches!(sign_in(&d, &dev, "keeper"), HostResponse::Outcome { .. }));
        assert!(matches!(run(&d, &dev, &uuid(), &users), HostResponse::Refused { code: RefusalCode::Refused, .. }), "a fresh request is refused");
        let replay = run(&d, &dev, ID, &users);
        assert_ne!(replay, first, "the administrator's answer is not replayed");
        assert!(matches!(replay, HostResponse::Refused { code: RefusalCode::Refused, .. }), "it runs fresh under the bookkeeper and is refused: {replay:?}");
        // Without a sign-out in between: a sign-in that changes the user also forgets the answers.
        assert!(matches!(sign_in(&d, &dev, "admin"), HostResponse::Outcome { .. }));
        let id2 = uuid();
        let admin_answer = run(&d, &dev, &id2, &users);
        assert!(matches!(&admin_answer, HostResponse::Outcome { .. }));
        assert!(matches!(sign_in(&d, &dev, "keeper"), HostResponse::Outcome { .. }));
        assert_ne!(run(&d, &dev, &id2, &users), admin_answer);
        let replayed: Vec<Event> = events.lock().unwrap().iter().filter(|e| e.kind == "request.replayed").cloned().collect();
        assert_eq!(replayed.len(), 1, "only the administrator's own retry was replayed");
        assert_eq!(replayed[0].user.as_deref(), Some("admin"), "the replay names its user");
    }

    #[test]
    fn exports_are_not_kept_for_retries() {
        let (fake, d, dev, _) = setup();
        d.handle(1, &dev, "p", ClientRequest::Hello { resume: None });
        sign_in(&d, &dev, "keeper");
        let before = fake.runs.load(Ordering::SeqCst);
        let stored = d.dedupe.len();
        let export = r#"{"type":"ExportHistory","company_id":"x"}"#;
        run(&d, &dev, ID, export);
        run(&d, &dev, ID, export);
        assert_eq!(fake.runs.load(Ordering::SeqCst), before + 2, "each export runs; none is stored");
        assert_eq!(d.dedupe.len(), stored, "nothing stored for the export");
    }

    #[test]
    fn unclassified_and_host_only_routes_never_reach_the_book() {
        let (fake, d, dev, _) = setup();
        d.handle(1, &dev, "p", ClientRequest::Hello { resume: None });
        sign_in(&d, &dev, "admin");
        let before = fake.runs.load(Ordering::SeqCst);
        for target in ["/c/x/new-screen/", "/c/x/backups/"] {
            assert!(matches!(d.handle(1, &dev, "p", ClientRequest::Render { target: target.into() }), HostResponse::Refused { code: RefusalCode::HostOnly, .. }), "{target}");
            assert!(matches!(run(&d, &dev, &uuid(), &post(target, &[])), HostResponse::Refused { code: RefusalCode::HostOnly, .. }), "{target}");
        }
        // S5: host-only actions are refused before the command runs.
        assert!(matches!(run(&d, &dev, &uuid(), &post("/c/x/home/", &[("action", "email_send")])), HostResponse::Refused { code: RefusalCode::HostOnly, .. }));
        assert_eq!(fake.runs.load(Ordering::SeqCst), before);
        assert!(matches!(d.handle(1, &dev, "p", ClientRequest::Render { target: "/c/x/home/".into() }), HostResponse::Page { .. }));
    }

    #[test]
    fn a_late_request_after_disconnect_or_revocation_opens_nothing() {
        let (_fake, d, dev, _) = setup();
        d.handle(7, &dev, "p", ClientRequest::Hello { resume: None });
        d.disconnected(7);
        assert!(matches!(d.handle(7, &dev, "p", ClientRequest::Hello { resume: None }), HostResponse::Refused { code: RefusalCode::Unauthenticated, .. }));
        assert!(d.connections().is_empty(), "no ghost connection");
        d.handle(8, &dev, "p", ClientRequest::Hello { resume: None });
        assert!(d.revoke_device(&dev, "admin"));
        assert!(!d.revoke_device(&dev, "admin"), "a second revocation is a no-op");
        assert_eq!(d.session_count(), 0);
        assert!(matches!(d.handle(9, &dev, "p", ClientRequest::Hello { resume: None }), HostResponse::Refused { code: RefusalCode::Unauthenticated, .. }));
        assert_eq!(d.session_count(), 0, "a late Hello from a revoked computer opens no session");
        assert!(d.connections().is_empty());
        // A request on a connection that never said Hello is not registered either.
        let other = DeviceRef { id: "other".into(), name: "x".into() };
        assert!(matches!(d.handle(10, &other, "p", ClientRequest::Render { target: "/c/x/home/".into() }), HostResponse::Refused { code: RefusalCode::Unauthenticated, .. }));
        assert!(d.connections().is_empty());
    }

    #[test]
    fn parallel_sign_ins_from_one_computer_are_checked_one_at_a_time() {
        let (fake, d, dev, _) = setup();
        fake.slow_sign_in.store(true, Ordering::SeqCst);
        let d = Arc::new(d);
        for conn in 0..6 {
            d.handle(conn, &dev, "p", ClientRequest::Hello { resume: None });
        }
        let attempts: Vec<_> = (0..6)
            .map(|conn| {
                let (d, dev) = (d.clone(), dev.clone());
                std::thread::spawn(move || d.handle(conn, &dev, "p", ClientRequest::Run { request_id: uuid(), command: post("/desktop/sign-in/", &[("action", "signin"), ("username", "x"), ("password", "wrong")]) }))
            })
            .collect();
        let answers: Vec<HostResponse> = attempts.into_iter().map(|t| t.join().unwrap()).collect();
        let checked = answers.iter().filter(|r| matches!(r, HostResponse::Outcome { .. })).count();
        assert!(checked < 6, "parallel attempts do not all get past the throttle: {answers:?}");
        assert!(answers.iter().any(|r| matches!(r, HostResponse::Refused { code: RefusalCode::Busy, .. })));
    }

    /// The re-check's B2: the retry of a sign-in that succeeded is answered, not a conflict.
    #[test]
    fn a_retried_sign_in_is_answered_after_it_succeeded() {
        let (_fake, d, dev, _) = setup();
        d.handle(1, &dev, "p", ClientRequest::Hello { resume: None });
        let command = post("/desktop/sign-in/", &[("action", "signin"), ("username", "keeper"), ("password", "right")]);
        let first = run(&d, &dev, ID, &command);
        assert!(matches!(&first, HostResponse::Outcome { .. }), "{first:?}");
        assert_eq!(run(&d, &dev, ID, &command), first);
    }

    /// The re-check's B3: a sign-out the books refuse as busy ends the session instead.
    #[test]
    fn a_sign_out_the_books_cannot_do_now_ends_the_session() {
        let (fake, d, dev, events) = setup();
        d.handle(1, &dev, "p", ClientRequest::Hello { resume: None });
        sign_in(&d, &dev, "keeper");
        fake.busy_sign_out.store(true, Ordering::SeqCst);
        assert_eq!(d.handle(1, &dev, "p", ClientRequest::SignOut), HostResponse::SignedOut);
        assert_eq!(d.session_count(), 0);
        assert!(fake.users.lock().unwrap().is_empty(), "the book session was closed");
        let e = events.lock().unwrap().iter().find(|e| e.kind == "sign_out").cloned().unwrap();
        assert_eq!(e.user.as_deref(), Some("keeper"));
        assert!(e.detail.contains("busy"));
        assert!(matches!(run(&d, &dev, &uuid(), &post("/c/x/home/", &[])), HostResponse::Refused { code: RefusalCode::Unauthenticated, .. }));
    }

    /// The re-check's A1: one failed audit write stops remote access (every request refused,
    /// every session ended, the reason kept for the host's screen), and nothing more is written,
    /// so the log opens again (and records the repair) once the disk allows.
    #[cfg(unix)]
    #[test]
    fn an_audit_log_that_cannot_be_written_stops_remote_access_until_it_is_reopened() {
        use crate::audit::{AuditKey, AuditLog, Expect};
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("remote");
        std::fs::create_dir(&folder).unwrap();
        let path = folder.join("remote-audit.jsonl");
        let key = AuditKey::generate().unwrap();
        let log = AuditLog::open(&path, &key, Expect::New).unwrap();
        let fake = Arc::new(Fake::default());
        let d = Dispatcher::new(fake.clone(), Config::default(), Box::new(log));
        d.handle(1, &dev(), "p", ClientRequest::Hello { resume: None });
        assert!(matches!(sign_in(&d, &dev(), "keeper"), HostResponse::Outcome { .. }));
        assert!(d.audit_error().is_none());
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o500)).unwrap();
        if std::fs::write(folder.join("probe"), b"x").is_ok() {
            return; // running as root: permissions do not stop writes
        }
        let lines_before = std::fs::read_to_string(&path).unwrap().lines().count();
        d.handle(1, &dev(), "p", ClientRequest::SignOut);
        assert!(d.audit_error().is_some(), "the reason is kept for the host's screen");
        assert_eq!(d.session_count(), 0, "every remote session ended");
        assert!(fake.users.lock().unwrap().is_empty());
        for request in [ClientRequest::Hello { resume: None }, ClientRequest::Ping, ClientRequest::Render { target: "/c/x/home/".into() }] {
            assert!(matches!(d.handle(2, &dev(), "p", request), HostResponse::Refused { code: RefusalCode::Busy, ref message } if message == PAUSED));
        }
        d.disconnected(1);
        d.sweep();
        d.end_all("hosting stopped");
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), lines_before + 1, "only the line whose checkpoint failed reached the file");
        drop(d);
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).unwrap();
        let log = AuditLog::open(&path, &key, Expect::Existing).unwrap();
        assert_eq!(log.head().seq as usize, lines_before + 2);
        drop(log);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.lines().nth(lines_before).unwrap().contains("sign_out"));
        assert!(text.lines().last().unwrap().contains("audit.repaired"));
    }

    fn dev() -> DeviceRef {
        DeviceRef { id: "dev".into(), name: "Shared PC".into() }
    }
}
