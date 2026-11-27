//! Remote access in the book process: [`SessionBackend`] maps the remote dispatcher's
//! [`fb_remote_proto::Backend`] onto fb-app's multi-user session host
//! (`fb_app::multiuser::SharedHost`, see `docs/multi-user.md`).
//!
//! Every remote request goes through `SharedHost::dispatch`, never `with_session` or `host_app`, so
//! it gets the session layer's own checks (authorization under the lease, stale-edit refusal,
//! duplicate-post replay, maintenance refusal for remote sessions) in addition to the dispatcher's
//! route policy and request-id deduplication. This crate links no network or TLS stack: the TLS
//! listener is the separate `fb-remote` process, started by `fb_remote_proto::link::HostLink`.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]
#![forbid(unsafe_code)]

use fb_app::multiuser::{ClientInfo, EndReason, HostError, Request, Response, SessionId, SharedHost};
use fb_remote_proto::{Backend, BackendError, DeviceRef, Remote, Rendered, SessionRef, SignInState, SignedIn};

pub mod routes;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

pub struct SessionBackend {
    host: SharedHost,
    ids: Mutex<HashMap<u64, SessionId>>,
    /// Sessions the dispatcher closed that could not be ended yet (the books were busy or locked):
    /// ended at the next chance, so a closed remote session never lingers signed in.
    unended: Mutex<Vec<SessionId>>,
    next: AtomicU64,
}

fn error(e: HostError) -> BackendError {
    match e {
        HostError::UnknownSession | HostError::Ended(_) => BackendError::NoSession,
        HostError::Busy { .. } | HostError::TooManySessions => BackendError::Busy,
        HostError::NotOpen => BackendError::Locked,
        other @ (HostError::NotEnrolled | HostError::Maintenance(_) | HostError::App(_)) => BackendError::Refused(other.to_string()),
    }
}

impl SessionBackend {
    pub fn new(host: SharedHost) -> SessionBackend {
        SessionBackend { host, ids: Mutex::new(HashMap::new()), unended: Mutex::new(vec![]), next: AtomicU64::new(1) }
    }

    pub fn host(&self) -> &SharedHost {
        &self.host
    }

    fn id(&self, session: SessionRef) -> Result<SessionId, BackendError> {
        self.ids.lock().unwrap_or_else(PoisonError::into_inner).get(&session.0).cloned().ok_or(BackendError::NoSession)
    }

    fn end_unended(&self) {
        let ids: Vec<SessionId> = std::mem::take(&mut *self.unended.lock().unwrap_or_else(PoisonError::into_inner));
        if ids.is_empty() {
            return;
        }
        match self.host.lease("remote: disconnect", &mut |_| {}) {
            Ok(mut host) => {
                for id in &ids {
                    host.end_session(id, EndReason::Closed);
                }
            }
            Err(_) => self.unended.lock().unwrap_or_else(PoisonError::into_inner).extend(ids),
        }
    }

    /// Sessions closed but not yet ended (tests).
    pub fn unended(&self) -> usize {
        self.unended.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    fn dispatch(&self, session: SessionRef, request: Request) -> Result<Response, BackendError> {
        self.end_unended();
        let id = self.id(session)?;
        self.host.dispatch(&id, request, &mut |_| {}).map_err(error)
    }
}

impl Backend for SessionBackend {
    fn classify(&self, path: &str) -> Option<Remote> {
        routes::classify(path)
    }

    fn open_session(&self, device: &DeviceRef) -> Result<SessionRef, BackendError> {
        let id = {
            let mut host = self.host.lease("remote: connect", &mut |_| {}).map_err(error)?;
            host.open_transport_session(ClientInfo::remote(&device.name, Some(&device.id))).map_err(error)?
        };
        let n = self.next.fetch_add(1, Ordering::SeqCst);
        self.ids.lock().unwrap_or_else(PoisonError::into_inner).insert(n, id);
        Ok(SessionRef(n))
    }

    fn close_session(&self, session: SessionRef) {
        let Some(id) = self.ids.lock().unwrap_or_else(PoisonError::into_inner).remove(&session.0) else { return };
        self.unended.lock().unwrap_or_else(PoisonError::into_inner).push(id);
        self.end_unended();
    }

    fn sign_in_state(&self, session: SessionRef) -> Result<SignInState, BackendError> {
        match self.dispatch(session, Request::Status)? {
            Response::Status(s) => Ok(match (s.signed_in, s.user, s.role) {
                (true, Some((id, name)), Some(role)) => SignInState::SignedIn(SignedIn { user_id: id.to_db(), user: name, role: role.as_str().into() }),
                (true, _, _) => return Err(BackendError::Refused("Could not read the signed-in user.".into())),
                _ if s.mfa_pending => SignInState::MfaPending,
                _ => SignInState::SignedOut,
            }),
            _ => Err(BackendError::Refused("Unexpected answer.".into())),
        }
    }

    fn render(&self, session: SessionRef, target: &str) -> Result<Rendered, BackendError> {
        match self.dispatch(session, Request::Render { target: target.into() })? {
            Response::Page { html, status, route } => Ok(Rendered { html, status, route }),
            _ => Err(BackendError::Refused("Unexpected answer.".into())),
        }
    }

    fn run(&self, session: SessionRef, command: &str) -> Result<String, BackendError> {
        let command = fb_app::commands::parse_command(command).map_err(BackendError::Refused)?;
        match self.dispatch(session, Request::Run { command })? {
            Response::Outcome { outcome, .. } => serde_json::to_string(&outcome).map_err(|e| BackendError::Refused(e.to_string())),
            _ => Err(BackendError::Refused("Unexpected answer.".into())),
        }
    }

    fn query(&self, session: SessionRef, query: &str) -> Result<String, BackendError> {
        let query = fb_app::commands::parse_query(query).map_err(BackendError::Refused)?;
        match self.dispatch(session, Request::Query { query })? {
            Response::Value(v) => Ok(v.to_string()),
            _ => Err(BackendError::Refused("Unexpected answer.".into())),
        }
    }

    fn sign_out(&self, session: SessionRef) -> Result<(), BackendError> {
        self.dispatch(session, Request::SignOut).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fb_app::App;
    use fb_app::keys::MasterKey;
    use fb_app::multiuser::{Policy, SessionHost};
    use std::time::Duration;

    /// Review nit N4: a session the dispatcher closed while the books were busy is ended at the
    /// next chance, not forgotten while it may still be signed in.
    #[test]
    fn a_session_closed_while_the_books_are_busy_is_ended_later() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new();
        app.backups.configure(dir.path()).unwrap();
        app.device_key = Some(MasterKey::new([41; 32]));
        app.create_protected_book(&dir.path().join("books/default.fbbook"), "Synthetic owner", "Synthetic company", Some("Synthetic book password")).unwrap();
        app.enroll_owner("Synthetic owner user password").unwrap();
        let shared = SharedHost::new(SessionHost::new(app, Policy { queue_wait: Duration::from_millis(50), ..Policy::default() }));
        let backend = std::sync::Arc::new(SessionBackend::new(shared.clone()));
        let device = DeviceRef { id: "dev".into(), name: "Laptop".into() };
        let (a, b) = (backend.open_session(&device).unwrap(), backend.open_session(&device).unwrap());
        let a_id = backend.id(a).unwrap();
        {
            let _busy = shared.lease("backup", &mut |_| {}).unwrap();
            let closer = std::thread::spawn({
                let backend = backend.clone();
                move || backend.close_session(a)
            });
            closer.join().unwrap();
            assert_eq!(backend.unended(), 1, "kept to end later");
        }
        assert!(backend.sign_in_state(b).is_ok());
        assert_eq!(backend.unended(), 0, "ended at the next request");
        let mut host = shared.lease("test", &mut |_| {}).unwrap();
        assert!(host.dispatch(&a_id, Request::Status).is_err(), "the closed session is gone");
    }
}
