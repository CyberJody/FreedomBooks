//! The book side of the plugin network boundary. fb-app holds the book and its key and links no HTTP
//! client; [`NetHost`] is the one way out, implemented with the separate `fb-net` process
//! ([`ProcessNet`], std::process only; fb-node configures its path once).
//!
//! These bodies translate the frozen Python services: `bankfeeds.services.connect`, `sync` (with
//! `_sync_locked`, `_record_sync` and `_store` already in the core), `forget`, and
//! `assistant.services.start_authorization`, `complete_callback` and `disconnect`. Every network leg
//! runs outside any book write. A connection flagged "reconnect needed" never has its credential
//! loaded or sent (the core's `begin_sync` refuses it before the vault is touched).
use crate::credentials::BookCredentials;
use crate::plugins::Refusal;
use crate::session::{App, AppError, AppResult, Open};
use fb_core::bankfeeds::{self, SecretSet, StagedPull};
use fb_core::{CompanyId, FbError};
use fb_plugin_api::{CredentialScope, CredentialStore, PluginError, SecretBytes, Uuid};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::time::Duration;
use zeroize::{Zeroize, Zeroizing};

/// One network request to the fb-net process and its reply, as JSON. Secrets inside travel only
/// between the two Rust processes over a private pipe, never through JavaScript or the renderer.
pub trait NetHost: Send {
    fn call(&mut self, request: &serde_json::Value) -> Result<serde_json::Value, String>;
}

const UNAVAILABLE: &str = "Bank feeds and ChatGPT aren't available in this build.";
const HELPER_FAILED: &str = "The network helper stopped unexpectedly. Try again.";
const FORGOTTEN: &str = "This connection was forgotten. Connect again with a new setup token.";
const UNREADABLE: &str = "The stored access for this connection cannot be read. Forget the connection and connect again.";
pub(crate) const BANK_KEYRING_REQUIRED: &str = "Bank connections need your OS keyring. Unlock or enable it, then restart FreedomBooks. Your saved connections and downloaded transactions are kept.";
const SIMPLEFIN: &str = "simplefin";
const OPENAI: &str = "openai";

// --- the fb-net process ----------------------------------------------------------------------------

/// Spawns `fb-net` once per operation: nothing outlives an operation except a running ChatGPT
/// sign-in, whose process waits on its loopback listener until the browser returns (or its
/// 10-minute deadline) and is polled by the assistant page. One process per operation keeps no
/// credential in a long-lived second process, confines a crash or hang to one operation, and needs
/// no request multiplexing. The child gets an empty environment except the platform's essentials and
/// (read only by a `fake-network` build) the fake-provider addresses.
pub struct ProcessNet {
    exe: PathBuf,
    signins: HashMap<String, Signin>,
    fake: Vec<(String, String)>,
}
struct Signin {
    child: Child,
    lines: Receiver<Zeroizing<String>>,
}
const ENV_PASSED: [&str; 5] = ["SystemRoot", "SYSTEMROOT", "WINDIR", "FB_NET_FAKE_SIMPLEFIN_PORT", "FB_NET_FAKE_OPENAI"];
const REPLY_TIMEOUT: Duration = Duration::from_secs(150);
const MAX_REPLY: u64 = 64 * 1024 * 1024;

impl ProcessNet {
    pub fn new(exe: PathBuf) -> Self {
        Self { exe, signins: HashMap::new(), fake: vec![] }
    }
    /// Tests: a fake provider address for the child (`FB_NET_FAKE_*` only). Only a `fake-network`
    /// build of fb-net reads these; the production build has no loopback path at all.
    pub fn with_fake(mut self, name: &str, value: &str) -> Self {
        if name.starts_with("FB_NET_FAKE_") {
            self.fake.push((name.to_owned(), value.to_owned()));
        }
        self
    }
    fn spawn(&self, request: &Value) -> Result<(Child, Receiver<Zeroizing<String>>), String> {
        let mut command = Command::new(&self.exe);
        command.env_clear().stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        for name in ENV_PASSED {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        for (name, value) in &self.fake {
            command.env(name, value);
        }
        let mut child = command.spawn().map_err(|_| UNAVAILABLE.to_owned())?;
        let mut line = Zeroizing::new(serde_json::to_vec(request).map_err(|_| HELPER_FAILED.to_owned())?);
        line.push(b'\n');
        let written = child.stdin.take().map(|mut stdin| stdin.write_all(&line).and_then(|()| stdin.flush()));
        if !matches!(written, Some(Ok(()))) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(HELPER_FAILED.into());
        }
        let stdout = child.stdout.take().ok_or_else(|| HELPER_FAILED.to_owned())?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout.take(MAX_REPLY));
            loop {
                let mut line = Zeroizing::new(String::new());
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok((child, rx))
    }
    fn parse(line: &str) -> Result<Value, String> {
        serde_json::from_str(line).map_err(|_| HELPER_FAILED.to_owned())
    }
    fn cancel(&mut self, key: &str) {
        if let Some(mut s) = self.signins.remove(key) {
            let _ = s.child.kill();
            let _ = s.child.wait();
        }
    }
}
impl Drop for ProcessNet {
    fn drop(&mut self) {
        let keys: Vec<String> = self.signins.keys().cloned().collect();
        for key in keys {
            self.cancel(&key);
        }
    }
}
impl NetHost for ProcessNet {
    fn call(&mut self, request: &Value) -> Result<Value, String> {
        let op = request["op"].as_str().unwrap_or("");
        let key = request["company"].as_str().unwrap_or("").to_owned();
        let mut forwarded = request.clone();
        if let Some(map) = forwarded.as_object_mut() {
            map.remove("company");
        }
        match op {
            // Local: the sign-in process's second line, if it has arrived.
            "chatgpt_poll" => {
                let Some(signin) = self.signins.get_mut(&key) else { return Ok(json!({"none": true})) };
                match signin.lines.try_recv() {
                    Ok(line) => {
                        self.cancel(&key);
                        Self::parse(&line)
                    }
                    Err(TryRecvError::Empty) => Ok(json!({"pending": true})),
                    Err(TryRecvError::Disconnected) => {
                        self.cancel(&key);
                        Err(HELPER_FAILED.into())
                    }
                }
            }
            "chatgpt_cancel" => {
                self.cancel(&key);
                Ok(json!({"ok": true}))
            }
            "chatgpt_begin" => {
                self.cancel(&key);
                let (mut child, lines) = self.spawn(&forwarded)?;
                forwarded.zeroize_strings();
                let first = match lines.recv_timeout(REPLY_TIMEOUT) {
                    Ok(line) => Self::parse(&line),
                    Err(_) => Err(HELPER_FAILED.into()),
                };
                if first.as_ref().is_ok_and(|v| v["ok"] == true) {
                    self.signins.insert(key, Signin { child, lines });
                } else {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                first
            }
            _ => {
                let (mut child, lines) = self.spawn(&forwarded)?;
                forwarded.zeroize_strings();
                let reply = match lines.recv_timeout(REPLY_TIMEOUT) {
                    Ok(line) => Self::parse(&line),
                    Err(RecvTimeoutError::Timeout) => Err("The connection took too long. Check its outcome before trying again.".into()),
                    Err(RecvTimeoutError::Disconnected) => Err(HELPER_FAILED.into()),
                };
                let _ = child.kill();
                let _ = child.wait();
                reply
            }
        }
    }
}
trait ZeroizeStrings {
    fn zeroize_strings(&mut self);
}
impl ZeroizeStrings for Value {
    fn zeroize_strings(&mut self) {
        match self {
            Value::String(s) => s.zeroize(),
            Value::Array(a) => a.iter_mut().for_each(ZeroizeStrings::zeroize_strings),
            Value::Object(m) => m.values_mut().for_each(ZeroizeStrings::zeroize_strings),
            _ => {}
        }
    }
}

// --- helpers ------------------------------------------------------------------------------------------

/// Django shows a `ValidationError`'s messages; anything else is an application error.
fn refusal(e: FbError) -> AppResult<Refusal> {
    match e {
        FbError::Validation(_) | FbError::Refused(_) | FbError::PermissionDenied(_) | FbError::DoesNotExist(_) => Ok(e.message()),
        other => Err(AppError::Core(other)),
    }
}
fn uuid(text: &str) -> AppResult<Uuid> {
    Uuid::parse_str(text).map_err(|_| AppError::Invalid("A stored id is damaged.".into()))
}
fn scope(plugin: &str, co: CompanyId, connection: &str) -> AppResult<CredentialScope> {
    Ok(CredentialScope { plugin_id: plugin.into(), company_id: uuid(&co.to_db())?, connection_id: uuid(connection)? })
}
/// The ChatGPT sign-in is one per company (`assistant_connection` is unique per company), so its vault
/// row is scoped to the company id as its connection id.
fn openai_scope(co: CompanyId) -> AppResult<CredentialScope> {
    let id = co.to_db();
    scope(OPENAI, co, &id)
}

/// Run `f` with the vault over the open book (the same writer; no second connection), then put the
/// book back. The vault holds only the session's HKDF subkey.
fn with_vault<T>(app: &mut App, f: impl FnOnce(&BookCredentials) -> T) -> AppResult<T> {
    let key = app.vault_key.clone().ok_or(AppError::Locked)?;
    let Open { book, owner, owner_name, staff } = app.open.take().ok_or(AppError::Locked)?;
    let vault = BookCredentials::with_wrapping_key(book, owner, key);
    let out = f(&vault);
    let book = vault.into_book().map_err(|_| AppError::Invalid("The stored connection credentials could not be saved.".into()))?;
    app.open = Some(Open { book, owner, owner_name, staff });
    Ok(out)
}
fn load(app: &mut App, scope: &CredentialScope) -> AppResult<Result<Option<SecretBytes>, PluginError>> {
    with_vault(app, |v| v.load(scope))
}

fn net_call(app: &mut App, request: &Value) -> Result<Value, String> {
    match app.net.as_mut() {
        None => Err(UNAVAILABLE.into()),
        Some(net) => net.call(request),
    }
}
fn net_error(reply: &Value) -> String {
    reply["error"].as_str().unwrap_or(HELPER_FAILED).to_owned()
}

/// The access URL inside the stored SimpleFIN record (the plugin's `StoredConnection` JSON).
fn access_url(record: &[u8]) -> Option<Zeroizing<String>> {
    #[derive(serde::Deserialize)]
    struct Stored {
        access_url: String,
    }
    let mut stored: Stored = serde_json::from_slice(record).ok()?;
    let url = Zeroizing::new(std::mem::take(&mut stored.access_url));
    Some(url)
}
fn now_utc() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}
/// The book's display zone (Python `settings.DISPLAY_TIME_ZONE`), as the D6 screens read it.
fn display_zone() -> String {
    std::env::var("FB_TIME_ZONE").unwrap_or_else(|_| "America/New_York".into())
}

// --- bank feeds -------------------------------------------------------------------------------------

/// `payables.services._text(label, 120)`, checked BEFORE the one-time token is spent (Python reads the
/// label first; a refused label must not consume the token).
fn check_label(label: &str) -> Result<(), Refusal> {
    fb_core::py::clean_free_text(label).map_err(|e| e.message())?;
    if fb_core::py::py_len(fb_core::py::py_strip(label)) > 120 {
        return Err("Enter text up to 120 characters.".into());
    }
    Ok(())
}

pub(crate) fn bankfeed_connect(app: &mut App, co: CompanyId, setup_token: &str, label: &str) -> AppResult<Result<String, Refusal>> {
    {
        let open = app.open_mut()?;
        let owner = open.owner;
        if let Err(e) = open.book.read(|q| fb_core::services::access(q, owner, co)) {
            return Ok(Err(refusal(e)?));
        }
    }
    if let Err(message) = check_label(label) {
        return Ok(Err(message));
    }
    if app.vault_key.is_none() {
        return Err(AppError::Locked);
    }
    if !app.os_store_usable {
        return Ok(Err(BANK_KEYRING_REQUIRED.into()));
    }
    if app.protection.as_ref().is_some_and(|r| r.keys.mode == fb_core::backups::portable::Mode::BuiltIn || !r.ask_password) {
        return Ok(Err("Add your own password and ask for it when opening these books before saving a connection.".into()));
    }
    // Network, outside any write: claim the access URL.
    let mut request = json!({"op": "simplefin_claim", "setup_token": setup_token});
    let reply = net_call(app, &request);
    request.zeroize_strings();
    let mut reply = match reply {
        Ok(v) if v["ok"] == true => v,
        Ok(v) => return Ok(Err(net_error(&v))),
        Err(message) => return Ok(Err(message)),
    };
    let record = SecretBytes::new(reply["record"].as_str().unwrap_or("").as_bytes().to_vec());
    let host = reply["host"].as_str().unwrap_or("").to_owned();
    reply.zeroize_strings();
    let Some(url) = access_url(record.expose()) else { return Ok(Err(HELPER_FAILED.into())) };
    // FIX7: a credential too short to protect, or with unsafe characters, is refused with that message.
    let secrets = match SecretSet::from_access_url(&url, Vec::<String>::new()) {
        Ok(s) => s,
        Err(e) => return Ok(Err(refusal(e)?)),
    };
    drop(url);
    let (owner, created) = {
        let open = app.open_mut()?;
        let owner = open.owner;
        (owner, bankfeeds::connect(&mut open.book, owner, co, &json!(label), &host, &secrets))
    };
    let connection = match created {
        Ok(row) => row["id"].as_str().unwrap_or("").to_owned(),
        Err(e) => return Ok(Err(refusal(e)?)),
    };
    // The core's write retained this connection's mask on the open Book for every later write of
    // the session; the vault keeps the record for the next open (restore-on-open reinstalls it).
    let scope = scope(SIMPLEFIN, co, &connection)?;
    let saved = with_vault(app, |v| v.save(&scope, record))?;
    if saved.is_err() {
        let open = app.open_mut()?;
        let _ = bankfeeds::forget(&mut open.book, owner, co, &connection, &secrets);
        return Err(AppError::Invalid("The connection credentials could not be saved in these books.".into()));
    }
    Ok(Ok(connection))
}

pub(crate) fn bankfeed_sync(app: &mut App, co: CompanyId, connection_id: &str) -> AppResult<Result<String, Refusal>> {
    app.open_mut()?;
    if !app.os_store_usable {
        return Ok(Err(BANK_KEYRING_REQUIRED.into()));
    }
    let now = now_utc();
    let owner = {
        let open = app.open_mut()?;
        let owner = open.owner;
        // `_sync_locked` preflight: this company's connection, not forgotten, not "reconnect needed",
        // outside the manual minimum interval; returns the window start.
        match bankfeeds::begin_sync(&mut open.book, owner, co, connection_id, &now, bankfeeds::MANUAL_MIN_INTERVAL_SECS, false) {
            Ok(start) => (owner, start),
            Err(e) => return Ok(Err(refusal(e)?)),
        }
    };
    let (owner, start) = owner;
    if app.net.is_none() {
        return Ok(Err(UNAVAILABLE.into()));
    }
    let scope = scope(SIMPLEFIN, co, connection_id)?;
    let record = match load(app, &scope)? {
        Ok(Some(record)) => record,
        Ok(None) => return Ok(Err(FORGOTTEN.into())),
        Err(_) => return Ok(Err(UNREADABLE.into())),
    };
    let Some(url) = access_url(record.expose()) else { return Ok(Err(UNREADABLE.into())) };
    let secrets = match SecretSet::from_access_url(&url, Vec::<String>::new()) {
        Ok(s) => s,
        Err(e) => return Ok(Err(refusal(e)?)),
    };
    drop(url);
    let record_text = std::str::from_utf8(record.expose()).map_err(|_| AppError::Invalid(UNREADABLE.into()))?;
    let mut request = json!({"op": "simplefin_fetch", "record": record_text, "start_epoch": start, "zone": display_zone()});
    let reply = net_call(app, &request);
    request.zeroize_strings();
    drop(record);
    let open = app.open_mut()?;
    let failure = match reply {
        Ok(v) if v["ok"] == true => match serde_json::from_value::<StagedPull>(v["pull"].clone()) {
            Ok(pull) => {
                return match bankfeeds::store_pull(&mut open.book, owner, co, connection_id, &pull, &now, false, Some(start), &secrets) {
                    Ok(result) => Ok(Ok(result["summary"].as_str().unwrap_or("").to_owned())),
                    Err(e) => Ok(Err(refusal(e)?)),
                };
            }
            Err(_) => "SimpleFIN returned something that is not an account list.".to_owned(),
        },
        Ok(v) => net_error(&v),
        Err(message) => message,
    };
    // `_record_sync(ok=False)` with the user-safe message, then the same message to the screen.
    bankfeeds::record_sync_failure(&mut open.book, owner, co, connection_id, &now, &failure, false, &secrets)?;
    Ok(Err(failure))
}

pub(crate) fn bankfeed_forget(app: &mut App, co: CompanyId, connection_id: &str) -> AppResult<()> {
    {
        let open = app.open_mut()?;
        bankfeeds::forget(&mut open.book, open.owner, co, connection_id, &SecretSet::none())?;
    }
    if app.vault_key.is_some() {
        let scope = scope(SIMPLEFIN, co, connection_id)?;
        with_vault(app, |v| v.forget(&scope))?
            .map_err(|_| AppError::Invalid("The stored connection credentials could not be wiped.".into()))?;
    }
    Ok(())
}

// --- ChatGPT ------------------------------------------------------------------------------------------

fn staff_only(app: &mut App) -> AppResult<Option<Refusal>> {
    Ok((!app.open_mut()?.staff).then(|| "The AI assistant settings are limited to staff operators.".to_owned()))
}

pub(crate) fn chatgpt_start(app: &mut App, co: CompanyId) -> AppResult<Result<String, Refusal>> {
    if let Some(refused) = staff_only(app)? {
        return Ok(Err(refused));
    }
    {
        let open = app.open_mut()?;
        let owner = open.owner;
        if let Err(e) = open.book.read(|q| fb_core::services::access(q, owner, co)) {
            return Ok(Err(refusal(e)?));
        }
    }
    if app.net.is_none() {
        return Ok(Err(UNAVAILABLE.into()));
    }
    if app.protection.as_ref().is_some_and(|r| r.keys.mode == fb_core::backups::portable::Mode::BuiltIn || !r.ask_password) {
        return Ok(Err("Add your own password and ask for it when opening these books before saving a connection.".into()));
    }
    // An earlier sign-in's record lets the plugin reuse its registered client and `login_hint`
    // (never `id_token_hint`, F-M6). An unreadable record counts as none, like Python's `_read`.
    let scope = openai_scope(co)?;
    let previous = load(app, &scope)?.ok().flatten();
    let record = previous.as_ref().and_then(|r| std::str::from_utf8(r.expose()).ok());
    let mut request = json!({"op": "chatgpt_begin", "record": record, "company": co.to_db()});
    let reply = net_call(app, &request);
    request.zeroize_strings();
    match reply {
        Ok(v) if v["ok"] == true => match v["authorization_url"].as_str() {
            Some(url) => {
                // Python `_audit(..., 'ai.signin_started', {'mode': 'siwc', 'registration': …})`.
                let open = app.open_mut()?;
                let owner = open.owner;
                if let Err(e) = fb_core::assistant::signin_started(&mut open.book, owner, co, v["registration"] == true) {
                    let _ = net_call(app, &json!({"op": "chatgpt_cancel", "company": co.to_db()}));
                    return Ok(Err(refusal(e)?));
                }
                Ok(Ok(url.to_owned()))
            }
            None => Ok(Err(HELPER_FAILED.into())),
        },
        Ok(v) => Ok(Err(net_error(&v))),
        Err(message) => Ok(Err(message)),
    }
}

pub(crate) fn chatgpt_poll(app: &mut App, co: CompanyId) -> AppResult<Option<Result<(), Refusal>>> {
    app.open_mut()?;
    if app.protection.as_ref().is_some_and(|r| r.keys.mode == fb_core::backups::portable::Mode::BuiltIn || !r.ask_password) { return Ok(None); }
    if app.net.is_none() {
        return Ok(None);
    }
    let reply = net_call(app, &json!({"op": "chatgpt_poll", "company": co.to_db()}));
    let mut reply = match reply {
        Ok(v) if v["pending"] == true || v["none"] == true => return Ok(None),
        Ok(v) if v["ok"] == true => v,
        Ok(v) => {
            // Python `complete_callback`: `_audit(..., 'ai.signin_failed', {'mode': 'siwc', 'code': …})`.
            let code = v["code"].as_str().unwrap_or("failed");
            let open = app.open_mut()?;
            let owner = open.owner;
            if let Err(e) = fb_core::assistant::signin_failed(&mut open.book, owner, co, code) {
                refusal(e)?;
            }
            return Ok(Some(Err(net_error(&v))));
        }
        Err(message) => return Ok(Some(Err(message))),
    };
    let record = SecretBytes::new(reply["record"].as_str().unwrap_or("").as_bytes().to_vec());
    let state = json!({
        "mode": "siwc",
        "model_choices": reply["model_choices"].clone(),
        "account_email": reply["account_email"].as_str().unwrap_or(""),
        "models_error": reply["models_error"].clone(),
        "registration": reply["registration"] == true,
    });
    reply.zeroize_strings();
    let scope = openai_scope(co)?;
    let previous = load(app, &scope)?.ok().flatten();
    if with_vault(app, |v| v.save(&scope, record))?.is_err() {
        return Err(AppError::Invalid("The ChatGPT sign-in could not be saved in these books.".into()));
    }
    let connected = {
        let open = app.open_mut()?;
        let owner = open.owner;
        fb_core::assistant::connected(&mut open.book, owner, co, &state)
    };
    match connected {
        Ok(_) => Ok(Some(Ok(()))),
        Err(e) => {
            // Python writes the cipher and the row in one transaction: put the vault back.
            let _ = with_vault(app, |v| match previous {
                Some(old) => v.save(&scope, old),
                None => v.forget(&scope),
            });
            Ok(Some(Err(refusal(e)?)))
        }
    }
}

pub(crate) fn chatgpt_disconnect(app: &mut App, co: CompanyId) -> AppResult<Result<(), Refusal>> {
    let row = {
        let open = app.open_mut()?;
        let owner = open.owner;
        match fb_core::assistant::connection_for(&mut open.book, owner, co) {
            Ok(row) => row,
            Err(e) => return Ok(Err(refusal(e)?)),
        }
    };
    let connected = row.as_ref().is_some_and(|r| r["status"] != "disconnected" || r["credential_present"] == true);
    if !connected {
        return Ok(Err("Nothing is connected.".into()));
    }
    let _ = net_call(app, &json!({"op": "chatgpt_cancel", "company": co.to_db()}));
    let scope = openai_scope(co)?;
    let record = if app.vault_key.is_some() { load(app, &scope)?.ok().flatten() } else { None };
    // Revoke a ChatGPT sign-in (network, outside any write); the secret is wiped even when the
    // revocation is not confirmed.
    let siwc = row.as_ref().is_some_and(|r| r["mode"] == "siwc");
    let revoked = match (&record, siwc) {
        (Some(record), true) => {
            let text = std::str::from_utf8(record.expose()).unwrap_or("");
            let mut request = json!({"op": "chatgpt_disconnect", "record": text});
            let reply = net_call(app, &request);
            request.zeroize_strings();
            match reply {
                Ok(v) if v["ok"] == true => v["revoked"].as_bool(),
                _ => Some(false),
            }
        }
        _ => None,
    };
    drop(record);
    if app.vault_key.is_some() {
        with_vault(app, |v| v.forget(&scope))?
            .map_err(|_| AppError::Invalid("The stored ChatGPT sign-in could not be wiped.".into()))?;
    }
    let open = app.open_mut()?;
    let owner = open.owner;
    match fb_core::assistant::disconnect(&mut open.book, owner, co, revoked) {
        Ok(_) => Ok(Ok(())),
        Err(e) => Ok(Err(refusal(e)?)),
    }
}
