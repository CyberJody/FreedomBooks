//! Retry deduplication. A connected computer sends each command with a request id it generated.
//! If the connection drops after the host committed but before the answer arrived, the client sends
//! the same request again; the host answers it from here instead of running it twice.
//!
//! Keys are (device, request id). The entry also remembers a fingerprint of the session, the
//! signed-in user and the command: a reused id with a different command, from a different session
//! or under a different user is a conflict and runs nothing. Entries belong to the session that
//! made them and are dropped when that session's sign-in changes or it ends, so a stored answer is
//! never handed to anyone else. A retry that arrives while the first is still running waits for it.
//!
//! Stored answers are bounded per device by count and by bytes, and in total by bytes.
use crate::wire::HostResponse;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

pub const TTL: Duration = Duration::from_secs(15 * 60);
pub const PER_DEVICE: usize = 1024;
/// Bytes of stored answers one device may hold.
pub const PER_DEVICE_BYTES: usize = 16 * 1024 * 1024;
/// Bytes of stored answers all devices together may hold.
pub const TOTAL_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum Begin {
    /// New request: run it, then call `finish`.
    Run,
    /// Already answered: return this, run nothing.
    Done(HostResponse),
    /// The id was used for a different request, session or user.
    Conflict,
    /// The first attempt is still running after the wait.
    StillRunning,
}

enum State {
    Running,
    Done(HostResponse, usize),
}

struct Entry {
    fingerprint: [u8; 32],
    session: String,
    state: State,
    at: Instant,
}

type Key = (String, String);

pub struct Dedupe {
    entries: Mutex<HashMap<Key, Entry>>,
    changed: Condvar,
    ttl: Duration,
    per_device: usize,
    per_device_bytes: usize,
    total_bytes: usize,
}

impl Default for Dedupe {
    fn default() -> Self {
        Dedupe::new(TTL, PER_DEVICE)
    }
}

/// `user` is the signed-in user's id, or empty when nobody is signed in.
pub fn fingerprint(session: &str, user: &str, command: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    for part in [session, user] {
        h.update((part.len() as u64).to_be_bytes());
        h.update(part.as_bytes());
    }
    h.update(command.as_bytes());
    h.finalize().into()
}

/// Roughly how much memory a stored answer holds.
fn size(response: &HostResponse) -> usize {
    64 + match response {
        HostResponse::Session { session, host_name, .. } => session.len() + host_name.len(),
        HostResponse::Page { html, route, .. } => html.len() + route.len(),
        HostResponse::Outcome { outcome } => outcome.len(),
        HostResponse::Data { data } => data.len(),
        HostResponse::Refused { message, .. } => message.len(),
        HostResponse::SignedOut | HostResponse::Pong => 0,
    }
}

fn stored(e: &Entry) -> usize {
    match e.state {
        State::Done(_, n) => n,
        State::Running => 0,
    }
}

impl Dedupe {
    pub fn new(ttl: Duration, per_device: usize) -> Self {
        Dedupe::with_bytes(ttl, per_device, PER_DEVICE_BYTES, TOTAL_BYTES)
    }

    pub fn with_bytes(ttl: Duration, per_device: usize, per_device_bytes: usize, total_bytes: usize) -> Self {
        Dedupe { entries: Mutex::new(HashMap::new()), changed: Condvar::new(), ttl, per_device, per_device_bytes, total_bytes }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Key, Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `session` owns the entry: [`Dedupe::forget_session`] drops it.
    pub fn begin(&self, device: &str, session: &str, request_id: &str, fingerprint: [u8; 32], wait: Duration) -> Begin {
        let key = (device.to_owned(), request_id.to_owned());
        let deadline = Instant::now() + wait;
        let mut entries = self.lock();
        loop {
            let now = Instant::now();
            entries.retain(|_, e| matches!(e.state, State::Running) || now.duration_since(e.at) < self.ttl);
            match entries.get(&key) {
                None => break,
                Some(e) if e.fingerprint != fingerprint => return Begin::Conflict,
                Some(Entry { state: State::Done(r, _), .. }) => return Begin::Done(r.clone()),
                Some(_) if now >= deadline => return Begin::StillRunning,
                Some(_) => {
                    entries = self.changed.wait_timeout(entries, deadline - now).unwrap_or_else(PoisonError::into_inner).0;
                }
            }
        }
        // Keep each device under its count cap by dropping its oldest finished answers.
        let mine: Vec<(Instant, Key)> = entries.iter().filter(|(k, e)| k.0 == device && matches!(e.state, State::Done(..))).map(|(k, e)| (e.at, k.clone())).collect();
        if mine.len() >= self.per_device {
            let mut mine = mine;
            mine.sort();
            let excess = mine.len() + 1 - self.per_device;
            for (_, k) in mine.into_iter().take(excess) {
                entries.remove(&k);
            }
        }
        entries.insert(key, Entry { fingerprint, session: session.to_owned(), state: State::Running, at: Instant::now() });
        Begin::Run
    }

    /// Store the answer for retries, then trim the device's and everyone's stored bytes, oldest
    /// first. Does nothing if the entry was forgotten while the command ran.
    pub fn finish(&self, device: &str, request_id: &str, response: HostResponse) {
        let mut entries = self.lock();
        let n = size(&response);
        if let Some(e) = entries.get_mut(&(device.to_owned(), request_id.to_owned())) {
            e.state = State::Done(response, n);
            e.at = Instant::now();
        }
        let trim = |entries: &mut HashMap<Key, Entry>, mine: bool, cap: usize| {
            let mut used: usize = entries.iter().filter(|(k, _)| !mine || k.0 == device).map(|(_, e)| stored(e)).sum();
            if used <= cap {
                return;
            }
            let mut done: Vec<(Instant, Key, usize)> = entries.iter().filter(|(k, e)| (!mine || k.0 == device) && stored(e) > 0).map(|(k, e)| (e.at, k.clone(), stored(e))).collect();
            done.sort();
            for (_, k, bytes) in done {
                if used <= cap {
                    break;
                }
                entries.remove(&k);
                used -= bytes;
            }
        };
        trim(&mut entries, true, self.per_device_bytes);
        trim(&mut entries, false, self.total_bytes);
        drop(entries);
        self.changed.notify_all();
    }

    /// Run nothing more under this id: the command's answer is not kept (read-only exports).
    pub fn abandon(&self, device: &str, request_id: &str) {
        self.lock().remove(&(device.to_owned(), request_id.to_owned()));
        self.changed.notify_all();
    }

    /// Forget every answer a session made (its sign-in changed, it signed out or it ended).
    /// `except` keeps one request (the sign-in that caused the change, so its own retry is answered).
    pub fn forget_session(&self, device: &str, session: &str, except: Option<&str>) {
        self.lock().retain(|k, e| k.0 != device || e.session != session || except.is_some_and(|id| k.1 == id));
        self.changed.notify_all();
    }

    /// Forget a device's answers (revocation).
    pub fn forget_device(&self, device: &str) {
        self.lock().retain(|k, e| k.0 != device || matches!(e.state, State::Running));
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Bytes of stored answers (tests and the host's status).
    pub fn bytes(&self) -> usize {
        self.lock().values().map(stored).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const ID: &str = "211ce6fc-4559-46d4-91ea-c81b0b41ff93";

    #[test]
    fn a_retry_gets_the_first_answer_and_a_changed_request_conflicts() {
        let d = Dedupe::default();
        let fp = fingerprint("s", "u", "cmd");
        assert_eq!(d.begin("dev", "s", ID, fp, Duration::ZERO), Begin::Run);
        d.finish("dev", ID, HostResponse::Pong);
        assert_eq!(d.begin("dev", "s", ID, fp, Duration::ZERO), Begin::Done(HostResponse::Pong));
        assert_eq!(d.begin("dev", "s", ID, fingerprint("s", "u", "other"), Duration::ZERO), Begin::Conflict);
        assert_eq!(d.begin("dev", "s", ID, fingerprint("other-session", "u", "cmd"), Duration::ZERO), Begin::Conflict);
        assert_eq!(d.begin("dev", "s", ID, fingerprint("s", "another-user", "cmd"), Duration::ZERO), Begin::Conflict, "bound to the user");
        assert_eq!(d.begin("dev", "s", ID, fingerprint("s", "", "cmd"), Duration::ZERO), Begin::Conflict, "bound to being signed in");
        assert_eq!(d.begin("other-dev", "s", ID, fp, Duration::ZERO), Begin::Run, "ids are per device");
    }

    #[test]
    fn a_session_whose_sign_in_changes_loses_its_answers() {
        let d = Dedupe::default();
        let fp = fingerprint("s", "admin", "cmd");
        assert_eq!(d.begin("dev", "s", ID, fp, Duration::ZERO), Begin::Run);
        d.finish("dev", ID, HostResponse::Pong);
        let other = "311ce6fc-4559-46d4-91ea-c81b0b41ff93";
        assert_eq!(d.begin("dev", "s2", other, fingerprint("s2", "x", "c"), Duration::ZERO), Begin::Run);
        d.finish("dev", other, HostResponse::Pong);
        d.forget_session("dev", "s", None);
        assert_eq!(d.begin("dev", "s", ID, fp, Duration::ZERO), Begin::Run, "forgotten: runs fresh, under the backend's own checks");
        assert_eq!(d.begin("dev", "s2", other, fingerprint("s2", "x", "c"), Duration::ZERO), Begin::Done(HostResponse::Pong), "other sessions keep theirs");
        d.forget_session("dev", "s2", Some(other));
        assert_eq!(d.begin("dev", "s2", other, fingerprint("s2", "x", "c"), Duration::ZERO), Begin::Done(HostResponse::Pong), "the excepted request is kept");
    }

    #[test]
    fn a_retry_during_the_first_attempt_waits_for_its_answer() {
        let d = Arc::new(Dedupe::default());
        let fp = fingerprint("s", "u", "cmd");
        assert_eq!(d.begin("dev", "s", ID, fp, Duration::ZERO), Begin::Run);
        assert_eq!(d.begin("dev", "s", ID, fp, Duration::from_millis(20)), Begin::StillRunning);
        let waiter = {
            let d = d.clone();
            std::thread::spawn(move || d.begin("dev", "s", ID, fp, Duration::from_secs(10)))
        };
        std::thread::sleep(Duration::from_millis(50));
        d.finish("dev", ID, HostResponse::SignedOut);
        assert_eq!(waiter.join().unwrap(), Begin::Done(HostResponse::SignedOut));
    }

    #[test]
    fn answers_expire_and_each_device_is_capped() {
        let d = Dedupe::new(Duration::from_millis(30), 3);
        let fp = fingerprint("s", "u", "c");
        for i in 0..5 {
            let id = format!("{i:08}-0000-0000-0000-000000000000");
            assert_eq!(d.begin("dev", "s", &id, fp, Duration::ZERO), Begin::Run);
            d.finish("dev", &id, HostResponse::Pong);
        }
        assert!(d.len() <= 3, "capped: {}", d.len());
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(d.begin("dev", "s", ID, fp, Duration::ZERO), Begin::Run);
        assert_eq!(d.len(), 1, "expired answers are dropped");
    }

    #[test]
    fn stored_bytes_are_capped_per_device_and_in_total() {
        let d = Dedupe::with_bytes(TTL, 1024, 10_000, 25_000);
        let big = || HostResponse::Outcome { outcome: "x".repeat(4_000) };
        for dev in ["a", "b", "c", "d"] {
            for i in 0..50 {
                let id = format!("{i:08}-0000-0000-0000-000000000000");
                assert_eq!(d.begin(dev, "s", &id, fingerprint("s", "u", &id), Duration::ZERO), Begin::Run);
                d.finish(dev, &id, big());
            }
        }
        assert!(d.bytes() <= 25_000, "total: {}", d.bytes());
        // The newest answer of the last device is still there.
        let last = format!("{:08}-0000-0000-0000-000000000000", 49);
        assert!(matches!(d.begin("d", "s", &last, fingerprint("s", "u", &last), Duration::ZERO), Begin::Done(_)));
        let first = format!("{:08}-0000-0000-0000-000000000000", 0);
        assert_eq!(d.begin("d", "s", &first, fingerprint("s", "u", &first), Duration::ZERO), Begin::Run, "the oldest went first");
        d.abandon("d", &first);
        assert_eq!(d.begin("d", "s", &first, fingerprint("s", "u", &first), Duration::ZERO), Begin::Run, "abandoned: not kept");
    }
}
