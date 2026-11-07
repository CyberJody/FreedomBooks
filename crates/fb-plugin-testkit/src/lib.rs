//! Test-only tools. Never link this crate or api/test-support into a production host.
//! The memory store is deliberately not encrypted and must never persist real credentials.
#![forbid(unsafe_code)]
mod core;
mod credentials;
mod oauth;
mod server;
mod tls;

pub use core::*;
pub use credentials::*;
pub use oauth::*;
pub use server::*;
pub use tls::*;
