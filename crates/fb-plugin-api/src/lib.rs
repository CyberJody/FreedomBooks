//! First-party plugin boundary. No database handles, posting methods, or provider identities.
//!
//! Calls are synchronous and object-safe. The host runs network calls off the UI thread;
//! no async runtime is imposed on the offline accounting core.
//! See `WRITING-A-PLUGIN.md` for the host and plugin obligations.
#![forbid(unsafe_code)]

mod contracts;
mod credentials;
mod error;
mod http;
mod lifecycle;
mod manifest;
pub mod oauth;
mod registry;
mod slots;
mod types;

pub use contracts::*;
pub use credentials::*;
pub use error::*;
pub use http::*;
pub use lifecycle::*;
pub use manifest::*;
pub use registry::*;
pub use secrecy::{ExposeSecret, SecretString};
pub use slots::*;
pub use types::*;
pub use uuid::Uuid;
