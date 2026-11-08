//! fb-app: the transport-neutral application layer between a window (or, later, a network host) and
//! fb-core. It owns the session (open book, owner, app lock), the command and query vocabulary as plain
//! serializable data with a JSON schema, and the screens (route -> view model -> fb-templates HTML).
//! It never opens sockets, windows or dialogs; the Electron bridge (fb-node) is a thin adapter over it.
pub mod ai;
pub mod commands;
pub mod keys;
pub mod opening;
pub mod protection;
pub mod screens;
pub mod session;
pub mod identity;
pub mod mfa;
pub mod credentials;
pub mod net;
pub mod plugins;

pub use commands::{Command, Outcome, Query};
pub use session::{App, AppError, AppResult, Host, ISSUE_URL};
