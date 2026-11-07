//! fb-node: the Electron main process's only door into the books (ARCHITECTURE 8).
//!
//! A fixed list of named functions; every request is JSON parsed by fb-app with unknown types, unknown
//! fields and duplicate keys refused before anything touches the book. There is no generic dispatch:
//! adding a command means adding a variant to `fb_app::Command`, not a function here. The module keeps
//! one `fb_app::App` per process; locking drops the open book (and its key) and every form token.
//!
//! BACKUPS-POLISH: a backup or restore runs as an async task on the libuv pool (`run_async`,
//! `backup_dialog_async`), so the Electron main thread never waits for it. Every synchronous call only
//! *tries* the lock: while a task holds the books it fails at once with `BUSY` instead of blocking the
//! main thread, so nothing else can run against the books meanwhile (no second command, no lock).
use fb_app::{App, AppError, Outcome, keys, screens};
use napi::bindgen_prelude::AsyncTask;
use napi::{Env, Error, Result, Status, Task};
use napi_derive::napi;
use std::path::Path;
use std::sync::{LazyLock, Mutex, MutexGuard, TryLockError};
use zeroize::Zeroizing;

static APP: LazyLock<Mutex<App>> = LazyLock::new(|| Mutex::new(App::new()));

fn poisoned() -> Error {
    Error::new(Status::GenericFailure, "The books are unavailable after an earlier failure.")
}

/// The main thread's door: never waits. `BUSY` while a backup or restore task holds the books.
fn app() -> Result<MutexGuard<'static, App>> {
    APP.try_lock().map_err(|e| match e {
        TryLockError::WouldBlock => Error::new(Status::GenericFailure, "BUSY"),
        TryLockError::Poisoned(_) => poisoned(),
    })
}

/// A pool thread's door: waits for the books (only tasks call this, never the main thread).
fn app_blocking() -> Result<MutexGuard<'static, App>> {
    APP.lock().map_err(|_| poisoned())
}

fn fail(message: impl Into<String>) -> Error {
    Error::new(Status::GenericFailure, message.into())
}

fn app_error(e: AppError) -> Error {
    match e {
        AppError::Locked => Error::new(Status::GenericFailure, "LOCKED"),
        other => fail(other.to_string()),
    }
}

fn json(v: serde_json::Value) -> String {
    v.to_string()
}

fn master(key_hex: &str) -> Result<keys::MasterKey> {
    keys::master_from_hex(key_hex).map_err(|e| fail(e.to_string()))
}

/// The app version (from the shell's build metadata) and the host OS name/version, shown on
/// "Report a problem" for copying into an issue. Plain text only; fb-app cleans and caps both.
#[napi]
pub fn set_host(version: String, os: String) -> Result<()> {
    app()?.host = fb_app::Host::new(&version, &os);
    Ok(())
}

/// `{"open": bool}`.
#[napi]
pub fn app_state() -> Result<String> {
    let app = app()?;
    let path = app.book_path().map(|p| p.display().to_string());
    Ok(json(serde_json::json!({"open": app.is_open(), "path": path, "can_lock":app.can_lock(), "builtin":app.builtin(), "viewing_restored":app.backups.viewing_restored().is_some()})))
}

/// 32 random bytes as hex, for the shell to seal with the OS store (`safeStorage`).
#[napi]
pub fn new_master_key() -> Result<String> {
    let key = keys::new_master_key().map_err(|e| fail(e.to_string()))?;
    Ok(keys::to_hex(key.as_ref()))
}

/// First run: create the encrypted book, owner and first company. Returns `{company_id, home}`.
#[napi]
pub fn create_book(path: String, key_hex: String, owner_name: String, company_name: String, starter_chart: Option<String>) -> Result<String> {
    let key_hex = Zeroizing::new(key_hex);
    let key = master(&key_hex)?;
    let chart = chart_choice(starter_chart)?;
    let created = app()?.create_book_with_chart(Path::new(&path), &key, &owner_name, &company_name, chart).map_err(app_error)?;
    let c = created.company_id.to_db();
    Ok(json(serde_json::json!({"company_id": c, "home": format!("/c/{c}/home/")})))
}

/// The app-lock copy of the key (Argon2id + ChaCha20-Poly1305), as JSON for the shell to store.
#[napi]
pub fn wrap_key(key_hex: String, password: String) -> Result<String> {
    let key_hex = Zeroizing::new(key_hex);
    let password = Zeroizing::new(password);
    let blob = keys::wrap(&master(&key_hex)?, &password).map_err(|e| fail(e.to_string()))?;
    serde_json::to_string(&blob).map_err(|e| fail(e.to_string()))
}

/// Unlock with the OS-store key. Returns `{home}` (the first company's Home, or "").
#[napi]
pub fn open_book(path: String, key_hex: String) -> Result<String> {
    let key_hex = Zeroizing::new(key_hex);
    let key = master(&key_hex)?;
    open_with(&path, &key)
}

/// Unlock with the app-lock password: the key is unwrapped here and never reaches JavaScript.
#[napi]
pub fn open_book_with_password(path: String, wrapped_json: String, password: String) -> Result<String> {
    let password = Zeroizing::new(password);
    let blob: keys::WrappedKey = serde_json::from_str(&wrapped_json).map_err(|_| fail("The saved key is damaged."))?;
    // A damaged file is named before the password is tried.
    fb_app::opening::check_book_file(Path::new(&path)).map_err(app_error)?;
    let key = keys::unwrap(&blob, &password).map_err(|e| fail(e.to_string()))?;
    open_with(&path, &key)?;
    let mut app = app()?;
    app.adopt_legacy_keys(&password).map_err(app_error)?;
    opened_location(&mut app)
}

fn open_with(path: &str, key: &keys::MasterKey) -> Result<String> {
    let mut app = app()?;
    app.open_book(Path::new(path), key).map_err(app_error)?;
    opened_location(&mut app)
}

fn opened_location(app: &mut App) -> Result<String> {
    let home = if app.protection.is_none() {
        fb_app::protection::SETTINGS.to_owned()
    } else if app.recovery_authorized && app.protection.as_ref().is_some_and(|r| !r.recovery_saved) {
        fb_app::protection::RECOVERY.to_owned()
    } else {
        app.first_company().map_err(app_error)?.map(|c| format!("/c/{}/home/", c.to_db())).unwrap_or_default()
    };
    Ok(json(serde_json::json!({"home":home})))
}

/// `app.lock()`: close the book; its key leaves memory with the connection.
#[napi]
pub fn lock() -> Result<()> {
    app()?.lock();
    Ok(())
}

/// `books.run(command)`: one `fb_app::Command` as JSON. Returns the `Outcome` as JSON.
#[napi]
pub fn run(command_json: String) -> Result<String> {
    run_in(&mut *app()?, &command_json)
}

/// `books.query(query)`: one `fb_app::Query` as JSON.
#[napi]
pub fn query(query_json: String) -> Result<String> {
    let query = fb_app::commands::parse_query(&query_json).map_err(fail)?;
    Ok(json(app()?.query(query).map_err(app_error)?))
}

/// `screens.render(target)`: the HTML of the screen at a path (`/c/<id>/invoices/?state=DRAFT`).
#[napi]
pub fn render(target: String) -> Result<String> {
    render_in(&mut *app()?, &target)
}

fn screen_target(target: &str) -> Result<()> {
    if target.len() > 4096 || !target.starts_with('/') {
        return Err(fail("Invalid screen address."));
    }
    Ok(())
}

fn render_in(app: &mut App, target: &str) -> Result<String> {
    screen_target(target)?;
    let r = screens::render(app, target).map_err(app_error)?;
    Ok(json(serde_json::json!({"html": r.html, "status": r.status, "route": r.route})))
}

/// Backups GET includes file inspection and full verification hashes. Keep exactly the
/// same authorization, rendering and token handling, under the worker's app lease.
#[napi]
pub fn render_async(target: String) -> Result<AsyncTask<Work>> {
    screen_target(&target)?;
    Ok(AsyncTask::new(Work::Render(target)))
}

/// First run or lock screen (works while the book is closed).
#[napi]
pub fn render_gate(mode: String, needs_password: bool, password_reason: String, errors_json: String, starter_chart: Option<String>) -> Result<String> {
    let errors: Vec<String> = serde_json::from_str(&errors_json).map_err(|_| fail("Invalid gate errors."))?;
    let app = app()?;
    screens::render_gate_with_chart(&app, &mode, needs_password, &password_reason, &errors, starter_chart.as_deref().unwrap_or("standard")).map_err(app_error)
}

/// DESKTOP-BACKUPS: the app data folder (Electron userData); books are in `books/`, backups default to `backups/`.
#[napi]
pub fn configure_backups(root: String) -> Result<()> {
    app()?.backups.configure(Path::new(&root)).map_err(app_error)
}

/// DESKTOP-BACKUPS: the path the user picked in the dialog a Backups action asked for ("" = cancelled).
/// The one-time ticket came from `run`'s outcome and never reaches the page.
#[napi]
pub fn backup_dialog(ticket: String, chosen: String) -> Result<String> {
    dialog_in(&mut *app()?, &ticket, &chosen)
}

/// BACKUPS-POLISH: the long Backups work, off the main thread.
pub enum Work {
    Render(String),
    Run(Zeroizing<String>),
    Dialog(String, Zeroizing<String>),
    Create { path: String, owner: String, company: String, password: Zeroizing<String>, protect: bool, chart: fb_app::commands::StarterChart },
    Restore { file: String, name: String, secret: Zeroizing<String> },
    RecoverySaved(Zeroizing<String>),
}

impl Task for Work {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> Result<String> {
        match self {
            Work::Render(target) => render_in(&mut *app_blocking()?, target),
            Work::Run(command_json) => run_in(&mut *app_blocking()?, command_json),
            Work::Dialog(chosen, ticket) => dialog_in(&mut *app_blocking()?, ticket, chosen),
            Work::RecoverySaved(ticket) => {
                app_blocking()?.recovery_saved_export(ticket).map_err(app_error)?;
                Ok(String::new())
            }
            Work::Create { path, owner, company, password, protect, chart } => {
                let mut app = app_blocking()?;
                app.create_protected_book_with_chart(Path::new(path), owner, company, protect.then_some(password.as_str()), *chart).map_err(app_error)?;
                opened_location(&mut app)
            }
            Work::Restore { file, name, secret } => {
                let mut app = app_blocking()?;
                screens::backups::restore_from_gate(&mut app, Path::new(file), name, secret).map_err(app_error)?;
                opened_location(&mut app)
            }
        }
    }

    fn resolve(&mut self, _env: Env, output: String) -> Result<String> {
        Ok(output)
    }
}

fn run_in(app: &mut App, command_json: &str) -> Result<String> {
    let command = fb_app::commands::parse_command(command_json).map_err(fail)?;
    let outcome: Outcome = app.run_renderer(command).map_err(app_error)?;
    serde_json::to_string(&outcome).map_err(|e| fail(e.to_string()))
}

fn dialog_in(app: &mut App, ticket: &str, chosen: &str) -> Result<String> {
    let outcome = screens::backups::dialog_result(app, ticket, chosen).map_err(app_error)?;
    serde_json::to_string(&outcome).map_err(|e| fail(e.to_string()))
}

/// `books.run` for a command that may take long ("Create backup now"): the same parsing and dispatch as
/// `run`, on a pool thread. Resolves to the `Outcome` JSON.
#[napi]
pub fn run_async(command_json: String) -> AsyncTask<Work> {
    AsyncTask::new(Work::Run(Zeroizing::new(command_json)))
}

/// `backup_dialog` on a pool thread: the restore, copy or folder step after the native dialog.
#[napi]
pub fn backup_dialog_async(ticket: String, chosen: String) -> AsyncTask<Work> {
    AsyncTask::new(Work::Dialog(chosen, Zeroizing::new(ticket)))
}

/// The command/query JSON Schema.
#[napi]
pub fn schema() -> Result<String> {
    Ok(json(fb_app::commands::schema()))
}

/// Where the fb-net executable lives, set once by the main process at startup and never changed:
/// a later call (another path, or the same one) is refused, so nothing that gains access to the
/// module afterwards can point plugin credentials at another program.
static NET_PATH: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// `configureNetwork(path)`: the absolute path of the fb-net executable (packaged next to this
/// module). Secrets cross only between this process and fb-net over a private pipe, never to JS.
/// `pluginsEnabled()`: whether this build shows the plugin surfaces (Cargo feature `plugins`). The
/// main process reads it once; release builds are false and never start the network helper.
#[napi]
pub fn plugins_enabled() -> bool {
    cfg!(feature = "plugins")
}

#[napi]
pub fn bankfeeds_enabled() -> bool {
    cfg!(any(feature = "plugins", feature = "simplefin"))
}

#[napi]
pub fn configure_network(path: String) -> Result<()> {
    if !bankfeeds_enabled() {
        return Err(fail("This build has no network helper."));
    }
    let path = std::path::PathBuf::from(path);
    if !path.is_absolute() || !path.is_file() {
        return Err(fail("The network helper was not found."));
    }
    NET_PATH.set(path.clone()).map_err(|_| fail("The network helper is already configured."))?;
    app()?.net = Some(Box::new(fb_app::net::ProcessNet::new(path)));
    Ok(())
}

/// The origins an `Outcome::External` URL may open in the system browser: the OpenAI issuer over
/// https (the only External the plugins produce). A `fake-network` build (smoke only) also allows
/// the fake OAuth provider's exact `http://127.0.0.1:<port>` origin from `FB_NET_FAKE_OPENAI`.
#[napi]
pub fn external_origins() -> Result<String> {
    if !plugins_enabled() {
        return Ok(json(serde_json::json!([])));
    }
    #[allow(unused_mut)]
    let mut origins = vec!["https://auth.openai.com".to_owned()];
    #[cfg(feature = "fake-network")]
    if let Ok(fake) = std::env::var("FB_NET_FAKE_OPENAI") {
        let fake = fake.trim_end_matches('/');
        let port = fake.strip_prefix("http://127.0.0.1:").and_then(|p| p.parse::<u16>().ok());
        if let Some(port) = port.filter(|p| *p != 0) {
            origins.push(format!("http://127.0.0.1:{port}"));
        }
    }
    Ok(json(serde_json::json!(origins)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_main_threads_door_never_waits_while_a_task_holds_the_books() {
        let held = app_blocking().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        // Another thread stands in for the main thread: it must get BUSY at once, not block.
        std::thread::spawn(move || tx.send(app().err().map(|e| e.reason.clone()))).join().unwrap().unwrap();
        assert_eq!(rx.recv().unwrap().as_deref(), Some("BUSY"));
        drop(held);
        assert!(app().is_ok());
        let mut work = Work::Run(Zeroizing::new("{\"type\":\"Nope\"}".to_owned()));
        assert!(work.compute().is_err(), "a task parses commands exactly like run");
        assert!(render_async("not a local path".to_owned()).is_err());
        assert!(render_async(format!("/{}", "x".repeat(4096))).is_err());
        let mut render = Work::Render("/c/0195b4a4-55af-719e-96cb-a2f946b067bf/backups/".to_owned());
        assert!(render.compute().is_err(), "a worker cannot render a closed book");
    }
}


/// OS-held connection secret, never derived from a book or included in its backups.
#[napi]
pub fn configure_keys(device_hex: String, os_store_usable: bool) -> Result<()> {
    let device = Zeroizing::new(device_hex);
    let mut app = app()?;
    app.device_key = Some(master(&device)?);
    app.os_store_usable = os_store_usable;
    Ok(())
}

#[napi]
pub fn protection_state(path: String) -> Result<String> {
    let record = fb_app::protection::Record::read(Path::new(&path)).map_err(app_error)?;
    Ok(match record {
        Some(r) => json(serde_json::json!({"known":true,"builtin":r.keys.mode == fb_app::protection::builtin_mode(),"ask_password":r.ask_password})),
        None => json(serde_json::json!({"known":false,"builtin":false,"ask_password":true})),
    })
}

#[napi]
pub fn recover_key_change(path: String) -> Result<bool> {
    if app()?.is_open() { return Err(fail("Close the books before recovering a key change.")); }
    fb_app::protection::recover_key_change(Path::new(&path)).map_err(app_error)
}

#[napi]
pub fn create_protected_book(path: String, owner_name: String, company_name: String, password: String, protect: bool, starter_chart: Option<String>) -> Result<String> {
    let password = Zeroizing::new(password);
    let mut app = app()?;
    app.create_protected_book_with_chart(Path::new(&path), &owner_name, &company_name, protect.then_some(password.as_str()), chart_choice(starter_chart)?).map_err(app_error)?;
    opened_location(&mut app)
}

#[napi]
pub fn open_protected_book(env: Env, path: String, secret: String) -> Result<String> {
    let secret = Zeroizing::new(secret);
    let mut app = app()?;
    if let Err(error) = app.open_with_secret(Path::new(&path), &secret) {
        // Only authentication refusal permits the trusted legacy saved-key fallback.
        // Keep the same plain message; never classify by parsing human-facing text.
        if matches!(&error, AppError::Open(fb_app::opening::OpenError::WrongKey)) {
            let message = error.to_string();
            env.throw_error(&message, Some("FB_BOOK_WRONG_KEY"))?;
            return Err(Error::new(Status::PendingException, message));
        }
        return Err(app_error(error));
    }
    opened_location(&mut app)
}

/// Trusted main-process use only: safeStorage sealing never crosses renderer IPC.
#[napi]
pub fn key_for_sealing() -> Result<String> {
    let app = app()?;
    if !app.os_store_usable || app.builtin() { return Err(fail("No OS-sealed book key is needed.")); }
    Ok(keys::to_hex(app.master_key().map_err(app_error)?.as_ref()))
}

#[napi]
pub fn recovery_kit() -> Result<String> {
    recovery_kit_in(&mut *app()?)
}

fn recovery_kit_in(app: &mut App) -> Result<String> {
    let key = app.recovery_text().map_err(app_error)?;
    let name = app.company_name_for_kit().map_err(app_error)?;
    Ok(format!("FreedomBooks Emergency Kit\n{name}\nDate: {}\n\n{}\n\nKeep this key separate from your computer and backups.\nTo restore: install FreedomBooks, choose Restore a backup, select your backup file, and enter this recovery key.\nAnyone with this key and a backup can open your books.\n", (app.today)(), key.as_str()))
}

#[napi]
pub fn recovery_key() -> Result<String> {
    // Only called by the desktop's path-bound Copy action, never exposed as renderer IPC.
    Ok(app()?.recovery_text().map_err(app_error)?.to_string())
}

#[napi]
pub fn recovery_export() -> Result<String> {
    let mut app = app()?;
    let kit = Zeroizing::new(recovery_kit_in(&mut app)?);
    let ticket = app.recovery_export_ticket().map_err(app_error)?;
    Ok(json(serde_json::json!({"kit":kit.as_str(),"ticket":ticket})))
}

#[napi]
pub fn validate_recovery_export(ticket: String) -> Result<()> {
    app()?.validate_recovery_export(&ticket).map_err(app_error)
}

#[napi]
pub fn recovery_saved_async(ticket: String) -> AsyncTask<Work> {
    AsyncTask::new(Work::RecoverySaved(Zeroizing::new(ticket)))
}

#[napi]
pub fn restore_book_from_gate(file: String, name: String, secret: String) -> Result<String> {
    let secret = Zeroizing::new(secret);
    let mut app = app()?;
    fb_app::screens::backups::restore_from_gate(&mut app, Path::new(&file), &name, &secret).map_err(app_error)?;
    opened_location(&mut app)
}


/// First-run creation uses the same exclusive worker as backup/restore.
#[napi]
pub fn create_protected_book_async(path: String, owner: String, company: String, password: String, protect: bool, starter_chart: Option<String>) -> Result<AsyncTask<Work>> {
    let password = Zeroizing::new(password);
    let chart = chart_choice(starter_chart)?;
    Ok(AsyncTask::new(Work::Create { path, owner, company, password, protect, chart }))
}

#[napi]
pub fn restore_book_from_gate_async(file: String, name: String, secret: String) -> AsyncTask<Work> {
    AsyncTask::new(Work::Restore { file, name, secret: Zeroizing::new(secret) })
}

// Compatibility for trusted older callers; renderer setup always submits an explicit choice.
fn chart_choice(value: Option<String>) -> Result<fb_app::commands::StarterChart> {
    serde_json::from_value(serde_json::json!(value.as_deref().unwrap_or("empty")))
        .map_err(|_| fail("Choose a chart of accounts."))
}
