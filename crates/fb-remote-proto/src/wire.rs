//! The typed protocol. A connected computer sends [`ClientRequest`] frames over its TLS connection
//! and gets one [`HostResponse`] per request. The gateway and the book process exchange
//! [`ToGateway`] / [`FromGateway`] frames over stdio. Every type refuses unknown fields.
use serde::{Deserialize, Serialize};

/// What a paired, connected computer may ask. Commands and queries travel as JSON *text* so that
/// fb-app's own strict parser sees exactly the bytes the client sent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum ClientRequest {
    /// Start a session for this device, or resume one this device already holds.
    Hello { resume: Option<String> },
    /// A screen, by path and query (`/c/<id>/invoices/?state=DRAFT`).
    Render { target: String },
    /// One `fb_app::Command` (JSON text). `request_id` is a client-generated UUID: a retry with the
    /// same id returns the first answer instead of running the command again.
    Run { request_id: String, command: String },
    /// One `fb_app::Query` (JSON text).
    Query { query: String },
    SignOut,
    Ping,
}

/// Why a request was not carried out. Each code has one fixed meaning for the client.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalCode {
    /// No session on this connection, or it expired: send Hello again and sign in.
    Unauthenticated,
    /// Signed-in work asked before signing in.
    SignInRequired,
    /// Only possible on the host computer itself.
    HostOnly,
    /// Too many unsuccessful sign-ins; wait.
    Throttled,
    /// The host is busy (maintenance, or the same request is still running); try again.
    Busy,
    /// The host's books are locked or closed.
    HostLocked,
    /// The request id was already used for a different request or session.
    Conflict,
    /// Malformed request.
    Invalid,
    /// The host refused the work; `message` says why in the app's own words.
    Refused,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedInVm {
    pub user: String,
    pub role: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum HostResponse {
    Session { session: String, signed_in: Option<SignedInVm>, host_name: String, resumed: bool },
    /// A rendered screen. For a 3xx status `html` is the redirect location.
    Page { html: String, status: u16, route: String },
    /// An `fb_app::Outcome` as JSON text, after the outcome filter.
    Outcome { outcome: String },
    /// A query result as JSON text.
    Data { data: String },
    SignedOut,
    Pong,
    Refused { code: RefusalCode, message: String },
}

impl HostResponse {
    pub fn refused(code: RefusalCode, message: impl Into<String>) -> Self {
        HostResponse::Refused { code, message: message.into() }
    }
}

/// A paired computer as the gateway authenticated it (certificate pin matched a live registry
/// entry). The book process trusts this only because the gateway is its own child process.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceRef {
    pub id: String,
    pub name: String,
}

/// A device as the registry holds it (public pin only; no secret).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceRecord {
    pub id: String,
    pub name: String,
    /// SHA-256 of the device's certificate, hex.
    pub pin: String,
    pub paired_at: u64,
    pub paired_by: String,
    pub revoked_at: Option<u64>,
}

/// The host's TLS identity: certificate and PKCS#8 private key, DER as hex. The book process
/// keeps it sealed (OS store) and hands it to the gateway at start; the gateway never writes it.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityHex {
    pub cert: String,
    pub key: String,
}

impl std::fmt::Debug for IdentityHex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IdentityHex { .. }")
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum ToGateway {
    /// First frame. `identity` None: generate one and return it in `Ready.new_identity`.
    /// `registry_key` (hex, 32 bytes) authenticates the list of paired computers: made at random
    /// and kept by the book process, never derived from the TLS key. The list on disk must have at
    /// least `min_counter` (the counter the book process recorded) or the gateway refuses to start;
    /// a missing list starts there. Devices in `revoked` are refused whatever the file says.
    Start { bind: String, identity: Option<IdentityHex>, state_dir: String, host_name: String, registry_key: String, min_counter: u64, revoked: Vec<String> },
    Reply { conn: u64, seq: u64, response: HostResponse },
    /// An Administrator on the host made a pairing code (the approval). One device may use it.
    ArmPairing { code: String, seconds: u64, minted_by: String },
    CancelPairing,
    Revoke { device_id: String },
    ListDevices,
    Stop,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum FromGateway {
    /// `counter`: the list of paired computers' counter as loaded (after applying `revoked`).
    Ready { addr: String, fingerprint: String, new_identity: Option<IdentityHex>, counter: u64 },
    Failed { message: String },
    Request { conn: u64, seq: u64, device: DeviceRef, peer: String, request: ClientRequest },
    Disconnected { conn: u64 },
    Event { kind: String, peer: String, device: Option<DeviceRef>, detail: String },
    Devices { devices: Vec<DeviceRecord> },
    /// The answer to `Revoke { device_id }`. `device` None: no such active device. `unsaved`:
    /// revoked now, but the registry could not be written (the reason); the gateway keeps trying
    /// on every later change.
    Revoked { device_id: String, device: Option<DeviceRef>, unsaved: Option<String> },
    /// The list of paired computers was written; its counter is now `counter`. The book process
    /// reads the file back, checks it and records the counter in its sealed record.
    Registry { counter: u64 },
    Armed,
    Cancelled,
    Stopped,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_refuse_unknown_fields_and_types() {
        let ok: ClientRequest = crate::strict::parse(r#"{"type":"Render","target":"/"}"#).unwrap();
        assert_eq!(ok, ClientRequest::Render { target: "/".into() });
        assert!(crate::strict::parse::<ClientRequest>(r#"{"type":"Render","target":"/","actor":"x"}"#).is_err());
        assert!(crate::strict::parse::<ClientRequest>(r#"{"type":"Unlock","password":"x"}"#).is_err());
        assert!(crate::strict::parse::<ClientRequest>(r#"{"type":"Ping","type":"SignOut"}"#).is_err());
    }

    #[test]
    fn identity_debug_hides_the_key() {
        let id = IdentityHex { cert: "aa".into(), key: "secret".into() };
        assert!(!format!("{id:?}").contains("secret"));
    }
}
