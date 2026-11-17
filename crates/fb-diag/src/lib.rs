//! fb-diag: what FreedomBooks records about its own failures, on this computer only.
//!
//! - [`build`]: which build is running (commit, tree state, date, features, schema count).
//! - [`log`]: the rotating problem log (JSON lines, owner-only files) and the diagnostic summary.
//! - [`panic`]: the panic hook and the catch the native bridge puts around every call.
//! - [`scrub`]: what may be written. The log holds no free text at all: no message from a panic,
//!   a Rust error or a JavaScript exception, so it cannot hold book content (names, amounts, memos,
//!   documents), secrets (passwords, keys, recovery keys, tokens) or personal paths. It holds code
//!   coordinates, code-owned identifiers and fixed phrases chosen by the code.
pub mod build;
pub mod log;
pub mod panic;
pub mod scrub;

pub use build::{Build, build, record_label, set_build, set_shell_version, short_version};
pub use log::{Entry, configure, record, recent, summary};
pub use panic::{Caught, catch, drain_other_threads, install_hook, quietly};

#[cfg(test)]
mod tests;
