//! Desktop plugin host operations (WAVE4 item 3). The D9 screens call only these functions; every
//! network leg (SimpleFIN claim and pull, Sign in with ChatGPT) runs in the separate `fb-net` process
//! through [`crate::net::NetHost`], never in the renderer and never in this crate (fb-app links no HTTP
//! client). Errors a screen shows are returned as `Ok(Err(message))`, the text Django's views add to the
//! form or notice; `Err(AppError)` is for a locked book or a damaged store, as everywhere else.
use crate::session::{App, AppResult};
use fb_core::CompanyId;

/// What a screen shows for a refused plugin action (Django's `'; '.join(exc.messages)`).
pub type Refusal = String;

/// `bankfeeds.services.connect`: claim the access URL from a one-time setup token (network, outside
/// any write), register the connection in the core, and keep the claimed credential in the book's
/// vault. Returns the new connection id. The token is never stored, echoed or returned.
pub fn bankfeed_connect(app: &mut App, co: CompanyId, setup_token: &str, label: &str) -> AppResult<Result<String, Refusal>> {
    crate::net::bankfeed_connect(app, co, setup_token, label)
}

/// `bankfeeds.services.sync` with the manual minimum interval: begin, pull (network), then store or
/// record the failure. Returns the summary text the list page shows after `"<label>: "`.
pub fn bankfeed_sync(app: &mut App, co: CompanyId, connection_id: &str) -> AppResult<Result<String, Refusal>> {
    crate::net::bankfeed_sync(app, co, connection_id)
}

/// `bankfeeds.services.forget`: deactivate in the core, then wipe the vault envelope.
pub fn bankfeed_forget(app: &mut App, co: CompanyId, connection_id: &str) -> AppResult<()> {
    crate::net::bankfeed_forget(app, co, connection_id)
}

/// "Continue with ChatGPT": start a sign-in in the fb-net process (system browser plus loopback).
/// Returns the authorization URL the main process opens in the system browser; the screen then shows
/// the assistant page, which finishes the sign-in when the callback has arrived.
pub fn chatgpt_start(app: &mut App, co: CompanyId) -> AppResult<Result<String, Refusal>> {
    crate::net::chatgpt_start(app, co)
}

/// Finish a sign-in whose callback has arrived (called by the assistant page render). `Ok(None)`
/// while still waiting or when none is running.
pub fn chatgpt_poll(app: &mut App, co: CompanyId) -> AppResult<Option<Result<(), Refusal>>> {
    crate::net::chatgpt_poll(app, co)
}

/// `assistant.services.disconnect`: wipe the stored tokens and record the disconnect.
pub fn chatgpt_disconnect(app: &mut App, co: CompanyId) -> AppResult<Result<(), Refusal>> {
    crate::net::chatgpt_disconnect(app, co)
}
