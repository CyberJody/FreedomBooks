//! fb-app: the transport-neutral application layer between a window (or, later, a network host) and
//! fb-core. It owns the session (open book, owner, app lock), the command and query vocabulary as plain
//! serializable data with a JSON schema, and the screens (route -> view model -> fb-templates HTML).
//! It never opens sockets, windows or dialogs; the Electron bridge (fb-node) is a thin adapter over it.
pub mod ai;
pub mod books;
pub mod commands;
pub mod keys;
pub mod opening;
pub mod protection;
pub mod screens;
pub mod session;
pub mod identity;
pub mod hosting;
pub mod mfa;
pub mod multiuser;
pub mod credentials;
pub mod net;
pub mod plugins;

pub use commands::{Command, Outcome, Query};
pub use session::{App, AppError, AppResult, ConnectionKeys, Host, ISSUE_URL};
pub mod invoice_email;
pub mod invoice_stripe;
#[cfg(feature = "panic-test")]
pub mod test_panic;

pub mod recurring_bills;

pub mod scheduled_backups;
