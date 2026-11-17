//! The session API as the remote boundary uses it. The real implementation is the multi-user
//! session layer in fb-app (`docs/multi-user.md`); tests use a synthetic one.
//!
//! Contract every implementation must keep:
//! - Each [`SessionRef`] is an independent person's session: its own sign-in, MFA, company
//!   context, form tokens and reviews. Signing one out never signs out another.
//! - `render`, `run` and `query` take the actor from the session alone, never from the request.
//! - Every call re-checks the session's user, role and company membership against the book.
//! - `run` keeps fb-app's desktop semantics (`App::run_desktop`): single-use form tokens, and a
//!   failed post that may have saved something is answered "open this page again and check".
use serde::{Deserialize, Serialize};

/// Opaque handle for one backend session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionRef(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedIn {
    pub user_id: String,
    pub user: String,
    pub role: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignInState {
    SignedOut,
    /// The password was right; the second factor is still owed.
    MfaPending,
    SignedIn(SignedIn),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rendered {
    pub html: String,
    pub status: u16,
    pub route: String,
}

/// Whether a screen or form path may be used from a connected computer. Every route the host
/// serves must be classified by the backend; a path it does not classify is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Remote {
    /// Served to a connected computer (the session layer's role and company checks still apply).
    Allowed,
    /// Only on the host computer's own window.
    HostOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendError {
    /// The books are locked or closed on the host.
    Locked,
    /// Book-wide maintenance (backup, restore, upgrade) holds the books.
    Busy,
    /// No such session (ended on the host).
    NoSession,
    /// The app refused the request; the words are the app's own, written for the person.
    Refused(String),
}

pub trait Backend: Send + Sync + 'static {
    /// The remote classification of a path (no query string). `None` for a path the host does not
    /// serve or nobody classified: the dispatcher refuses it (deny by default).
    fn classify(&self, path: &str) -> Option<Remote>;
    /// A new, signed-out session for a connected device.
    fn open_session(&self, device: &crate::DeviceRef) -> Result<SessionRef, BackendError>;
    /// End a session (sign-out, idle expiry, revoked device, disconnect after expiry).
    fn close_session(&self, session: SessionRef);
    fn sign_in_state(&self, session: SessionRef) -> Result<SignInState, BackendError>;
    fn render(&self, session: SessionRef, target: &str) -> Result<Rendered, BackendError>;
    /// One `fb_app::Command` as JSON text; returns the `Outcome` as JSON text.
    fn run(&self, session: SessionRef, command: &str) -> Result<String, BackendError>;
    /// One `fb_app::Query` as JSON text; returns JSON text.
    fn query(&self, session: SessionRef, query: &str) -> Result<String, BackendError>;
    fn sign_out(&self, session: SessionRef) -> Result<(), BackendError>;
}
