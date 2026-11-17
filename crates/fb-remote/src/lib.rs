//! Remote access, network side (design: `docs/remote-access-design.md`).
//!
//! The `fb-remote` executable has two modes, each a child process that talks to its parent only
//! over stdio frames:
//! - `gateway` ([`gateway`]): on the host, a child of the book process. Terminates mutual TLS,
//!   pairs computers, keeps the device registry, limits connections, and forwards authenticated
//!   requests to the book process. It never opens the book.
//! - `client` ([`client_main`]): on a connected computer, a child of the desktop's main process.
//!   Owns the TLS connection to the host with the sealed device identity the parent hands it.
//!
//! This crate links rustls and never fb-app or fb-db (the gate checks both directions).
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]
#![forbid(unsafe_code)]

pub mod client;
pub mod client_main;
pub mod gateway;
pub mod limits;
pub mod pairing;
pub mod registry;
pub mod scope;
pub mod tls;
