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
//!
//! CRASHLOG: every entry point runs inside the panic boundary (`boundary.rs`): a panic becomes an
//! `FB_PANIC` error, is written to the problem log (fb-diag), and never aborts the process.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]
pub mod ai;
mod boundary;
mod hosting;
#[cfg(test)]
#[path = "tests/hosting.rs"]
mod hosting_tests;
#[cfg(all(test, feature = "panic-test"))]
mod panic_tests;

use boundary::{Class, free, guarded, within};

// The boundary needs unwinding: with `panic = "abort"` every panic would end the app again.
#[cfg(panic = "abort")]
compile_error!("fb-node's panic boundary needs `panic = \"unwind\"`.");
#[cfg(all(feature = "panic-test", not(debug_assertions)))]
compile_error!("`panic-test` is for test builds only and is refused in optimized (release) builds.");

/// This build, as stamped by build.rs (see there for where each value comes from).
pub(crate) fn build_identity() -> fb_diag::Build {
    let mut features = vec![];
    for (on, name) in [
        (cfg!(feature = "simplefin"), "simplefin"),
        (cfg!(feature = "invoice-delivery"), "invoice-delivery"),
        (cfg!(feature = "plugins"), "plugins"),
        (cfg!(feature = "fake-network"), "fake-network"),
        (cfg!(feature = "key-test-hooks"), "key-test-hooks"),
        (cfg!(feature = "panic-test"), "panic-test"),
    ] {
        if on {
            features.push(name);
        }
    }
    let dirty = match env!("FB_BUILD_DIRTY") {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    };
    fb_diag::Build {
        crate_version: env!("CARGO_PKG_VERSION"),
        commit: env!("FB_BUILD_COMMIT").to_owned(),
        dirty,
        date: env!("FB_BUILD_DATE").to_owned(),
        profile: if cfg!(debug_assertions) { "debug" } else { "release" },
        features,
        migrations: fb_diag::build::known_migrations(),
        rustc: env!("FB_BUILD_RUSTC").to_owned(),
    }
}

use fb_app::{App, AppError, Outcome, keys, screens};
use napi::bindgen_prelude::AsyncTask;
use napi::{Env, Error, Result, Status, Task};
use napi_derive::napi;
use std::path::Path;
use std::sync::{LazyLock, Mutex, MutexGuard, TryLockError};
use zeroize::Zeroizing;

// The desktop has no connection key until the shell supplies one (`configureKeys`): sign-in before
// that must never read or flag saved connections with a stand-in key.
static APP: LazyLock<Mutex<App>> = LazyLock::new(|| {
    let mut app = App::new();
    app.device_key = None;
    app.connection_keys = fb_app::ConnectionKeys::Deferred;
    Mutex::new(app)
});

/// Build `App` (its templates compile here). Called once, inside a catch, by `boundary::init`.
pub(crate) fn build_app() {
    LazyLock::force(&APP);
}

/// A lock poisoned by a panic that no boundary caught (there should be none): the session state
/// it guards may be half-updated, so the books are closed before anyone uses it again.
fn unpoison(poisoned: std::sync::PoisonError<MutexGuard<'static, App>>) -> MutexGuard<'static, App> {
    let mut app = poisoned.into_inner();
    APP.clear_poison();
    boundary::close(&mut app);
    fb_diag::record(fb_diag::Entry { source: "rust", op: "lock".into(), kind: "poisoned".into(), outcome: vec!["books closed"], book_open: Some(false), ..Default::default() });
    app
}

/// The main thread's door: never waits. `BUSY` while a backup or restore task holds the books.
fn app() -> Result<MutexGuard<'static, App>> {
    boundary::init();
    if let Some(failed) = boundary::start_failed() {
        return Err(failed);
    }
    match APP.try_lock() {
        Ok(app) => Ok(app),
        Err(TryLockError::WouldBlock) => Err(Error::new(Status::GenericFailure, "BUSY")),
        Err(TryLockError::Poisoned(p)) => Ok(unpoison(p)),
    }
}

/// A pool thread's door: waits for the books (only tasks call this, never the main thread).
fn app_blocking() -> Result<MutexGuard<'static, App>> {
    boundary::init();
    if let Some(failed) = boundary::start_failed() {
        return Err(failed);
    }
    Ok(APP.lock().unwrap_or_else(unpoison))
}

fn fail(message: impl Into<String>) -> Error {
    Error::new(Status::GenericFailure, message.into())
}

fn app_error(e: AppError) -> Error {
    boundary::note_error(&e);
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
    guarded("set_host", Class::Read, |app| {
        // The shell's version plus this module's commit: the unlock screen and Report a problem
        // show which build is running, not only the shell's package number.
        fb_diag::set_shell_version(&version);
        fb_diag::build::set_os(&os);
        app.host = fb_app::Host::new(&fb_diag::short_version(), &os);
        Ok(())
    })
}

/// `{"open": bool}`.
#[napi]
pub fn app_state() -> Result<String> {
    let hosting_now = hosting::active();
    guarded("app_state", Class::Read, |app| {
    let signed_in = app.authenticated_session().is_ok();
    let generation = app.session_generation();
    let path = app.book_path().map(|p| p.display().to_string());
    Ok(json(serde_json::json!({"open": app.is_open(), "signed_in":signed_in, "session_generation":generation, "path": path, "can_lock":app.can_lock(), "builtin":app.builtin(), "viewing_restored":app.backups.viewing_restored().is_some(), "hosting": hosting_now})))
    })
}

/// 32 random bytes as hex, for the shell to seal with the OS store (`safeStorage`).
#[napi]
pub fn new_master_key() -> Result<String> {
    free("new_master_key", || {
        let key = keys::new_master_key().map_err(|e| fail(e.to_string()))?;
        Ok(keys::to_hex(key.as_ref()))
    })
}

/// First run: create the encrypted book, owner and first company. Returns `{company_id, home}`.
#[napi]
pub fn create_book(path: String, key_hex: String, owner_name: String, company_name: String, starter_chart: Option<String>) -> Result<String> {
    let key_hex = Zeroizing::new(key_hex);
    let chart = chart_choice(starter_chart)?;
    guarded("create_book", Class::Write, |app| {
        let key = master(&key_hex)?;
        let created = app.create_book_with_chart(Path::new(&path), &key, &owner_name, &company_name, chart).map_err(app_error)?;
        let c = created.company_id.to_db();
        Ok(json(serde_json::json!({"company_id": c, "home": format!("/c/{c}/home/")})))
    })
}

/// The app-lock copy of the key (Argon2id + ChaCha20-Poly1305), as JSON for the shell to store.
#[napi]
pub fn wrap_key(key_hex: String, password: String) -> Result<String> {
    let key_hex = Zeroizing::new(key_hex);
    let password = Zeroizing::new(password);
    free("wrap_key", || {
        let blob = keys::wrap(&master(&key_hex)?, &password).map_err(|e| fail(e.to_string()))?;
        serde_json::to_string(&blob).map_err(|e| fail(e.to_string()))
    })
}

/// Unlock with the OS-store key. Returns `{home}` (the first company's Home, or "").
#[napi]
pub fn open_book(env: Env, path: String, key_hex: String) -> Result<String> {
    let key_hex = Zeroizing::new(key_hex);
    guarded("open_book", Class::Write, |app| {
        let key = master(&key_hex)?;
        if let Err(error) = app.open_book(Path::new(&path), &key) {
            // A saved key that no longer fits (the book was rekeyed elsewhere) is a reason to ask, not damage.
            if matches!(&error, AppError::Open(fb_app::opening::OpenError::WrongKey)) {
                let message = error.to_string();
                env.throw_error(&message, Some("FB_BOOK_WRONG_KEY"))?;
                return Err(Error::new(Status::PendingException, message));
            }
            return Err(app_error(error));
        }
        opened_location(app)
    })
}

/// Unlock with the app-lock password: the key is unwrapped here and never reaches JavaScript.
#[napi]
pub fn open_book_with_password(path: String, wrapped_json: String, password: String) -> Result<String> {
    let password = Zeroizing::new(password);
    let key = free("open_book_with_password", || {
        let blob: keys::WrappedKey = serde_json::from_str(&wrapped_json).map_err(|_| fail("The saved key is damaged."))?;
        // A damaged file is named before the password is tried.
        fb_app::opening::check_book_file(Path::new(&path)).map_err(app_error)?;
        keys::unwrap(&blob, &password).map_err(|e| fail(e.to_string()))
    })?;
    guarded("open_book_with_password", Class::Write, |app| {
        app.open_book(Path::new(&path), &key).map_err(app_error)?;
        app.adopt_legacy_keys(&password).map_err(app_error)?;
        opened_location(app)
    })
}

fn opened_location(app: &mut App) -> Result<String> {
    let home = if let Some(location) = app.identity_location() {
        location.to_owned()
    } else if app.protection.is_none() {
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
    // Closing the books ends hosting first: connected people are told and signed out.
    boundary::init();
    {
        let mut door = app()?;
        hosting::end_from(&mut door, "The host closed these books.");
    }
    guarded("lock", Class::Write, |app| {
        app.lock();
        Ok(())
    })
}

/// Named-user lock for books whose encryption policy keeps the file open on this desktop.
///
/// While hosting, this is the host person's lock: only the desktop's own session is signed out; the
/// books stay open for the people connected.
#[napi]
pub fn sign_out() -> Result<()> {
    boundary::init();
    if hosting::request_local_lock() {
        // The host person's Lock never waits for the books (a remote request may hold them): it is
        // recorded at once, applied now if the books are free, and otherwise applied first at the
        // next desktop call. The window shows the sign-in screen either way.
        if let Ok(mut door) = app() {
            let _ = within("sign_out", Class::Write, &mut door, |door| hosting::route(door, std::time::Duration::ZERO, |_| Ok(()), |_, _, _| Ok(())));
        }
        return Ok(());
    }
    desk("sign_out", Class::Write, |app| {
        app.sign_out();
        Ok(())
    }, |host, local, _| hosting::dispatch(host, local, fb_app::multiuser::Request::Lock).map(drop))
}

/// A desktop call that, while hosting, goes through the session layer's `dispatch` as the local
/// session (`hosted`), and otherwise runs on the desktop's own app inside the panic boundary.
fn desk<T>(op: &'static str, class: Class, alone: impl FnOnce(&mut App) -> Result<T>, hosted: impl FnOnce(&mut fb_app::multiuser::SessionHost, &fb_app::multiuser::SessionId, ()) -> Result<T>) -> Result<T> {
    boundary::init();
    let mut door = app()?;
    hosting::route(&mut door, hosting::DOOR_WAIT, move |app: &mut App| within(op, class, app, alone), |host, local, _alone| hosted(host, local, ()))
}

/// `books.run(command)`: one `fb_app::Command` as JSON. Returns the `Outcome` as JSON.
#[napi]
pub fn run(command_json: String) -> Result<String> {
    desk("run", Class::Write, |app| run_in(app, &command_json), |host, local, ()| {
        let command = fb_app::commands::parse_command(&command_json).map_err(fail)?;
        match hosting::dispatch(host, local, fb_app::multiuser::Request::Run { command })? {
            fb_app::multiuser::Response::Outcome { outcome, .. } => serde_json::to_string(&outcome).map_err(|e| fail(e.to_string())),
            _ => Err(fail("Unexpected answer.")),
        }
    })
}

/// `books.query(query)`: one `fb_app::Query` as JSON.
#[napi]
pub fn query(query_json: String) -> Result<String> {
    desk("query", Class::Read, |app| {
        let query = fb_app::commands::parse_query(&query_json).map_err(fail)?;
        Ok(json(app.query(query).map_err(app_error)?))
    }, |host, local, ()| {
        let query = fb_app::commands::parse_query(&query_json).map_err(fail)?;
        match hosting::dispatch(host, local, fb_app::multiuser::Request::Query { query })? {
            fb_app::multiuser::Response::Value(v) => Ok(json(v)),
            _ => Err(fail("Unexpected answer.")),
        }
    })
}

/// `screens.render(target)`: the HTML of the screen at a path (`/c/<id>/invoices/?state=DRAFT`).
#[napi]
pub fn render(target: String) -> Result<String> {
    desk("render", Class::Read, |app| render_in(app, &target), |host, local, ()| {
        screen_target(&target)?;
        match hosting::dispatch(host, local, fb_app::multiuser::Request::Render { target: target.clone() })? {
            fb_app::multiuser::Response::Page { html, status, route } => Ok(json(serde_json::json!({"html": html, "status": status, "route": route}))),
            _ => Err(fail("Unexpected answer.")),
        }
    })
}

fn screen_target(target: &str) -> Result<()> {
    if target.len() > 4096 || !target.starts_with('/') {
        return Err(fail("Invalid screen address."));
    }
    Ok(())
}

fn render_in(app: &mut App, target: &str) -> Result<String> {
    screen_target(target)?;
    boundary::set_route(boundary::route_of(target));
    let r = screens::render(app, target).map_err(app_error)?;
    Ok(json(serde_json::json!({"html": r.html, "status": r.status, "route": r.route})))
}

/// Backups GET includes file inspection and full verification hashes. Keep exactly the
/// same authorization, rendering and token handling, under the worker's app lease.
#[napi]
pub fn render_async(target: String) -> Result<AsyncTask<Work>> {
    free("render_async", || screen_target(&target))?;
    Work::new(Action::Render(target))
}

/// First run, the unlock screen or the book chooser (works while the book is closed). `context_json`
/// is the shell's view of this computer: why the screen shows, the current book and the books listed.
#[napi]
pub fn render_gate(mode: String, context_json: String, errors_json: String, starter_chart: Option<String>) -> Result<String> {
    guarded("render_gate", Class::Read, |app| {
        let errors: Vec<String> = serde_json::from_str(&errors_json).map_err(|_| fail("Invalid gate errors."))?;
        let context: fb_app::books::GateContext = serde_json::from_str(&context_json).map_err(|_| fail("Invalid gate context."))?;
        screens::render_gate_in(app, &mode, &context, &errors, starter_chart.as_deref().unwrap_or("standard")).map_err(app_error)
    })
}

/// DESKTOP-BACKUPS: the app data folder (Electron userData); books are in `books/`, backups default to `backups/`.
#[napi]
pub fn configure_backups(root: String) -> Result<()> {
    guarded("configure_backups", Class::Read, |app| app.backups.configure(Path::new(&root)).map_err(app_error))
}

/// DESKTOP-BACKUPS: the path the user picked in the dialog a Backups action asked for ("" = cancelled).
/// The one-time ticket came from `run`'s outcome and never reaches the page.
#[napi]
pub fn backup_dialog(ticket: String, chosen: String) -> Result<String> {
    guarded("backup_dialog", Class::Write, |app| dialog_in(app, &ticket, &chosen))
}

/// BACKUPS-POLISH: the long Backups work, off the main thread.
enum Action {
    InvoiceEmail(String, Zeroizing<Vec<u8>>),
    RecurringBills,
    StripeRefresh(String),
    Render(String),
    Run(Zeroizing<String>),
    Dialog(String, Zeroizing<String>),
    Create { path: String, owner: String, company: String, password: Zeroizing<String>, protect: bool, chart: fb_app::commands::StarterChart },
    Restore { file: String, name: String, secret: Zeroizing<String> },
    RecoverySaved(Zeroizing<String>),
    Hosting(HostingAction),
}

/// "Host these books" controls (hosting.rs), on the worker pool: they wait for the listener.
enum HostingAction {
    On { bind: String, host_name: String },
    Off { generation: String },
    End(String),
    Resume { host_name: String },
    Restart { host_name: String },
    Pair,
    CancelPair,
    Revoke { device: String, generation: String },
    Repair { kind: String, host_name: String },
}

/// Every queued task is bound when accepted, then checked under the same APP lease used to
/// execute it. Results are checked again before delivery so old-account pages cannot escape.
pub struct Work { generation: u64, action: Action }
impl Work {
    fn new(action: Action) -> Result<AsyncTask<Self>> {
        let generation = guarded("queue", Class::Read, |app| Ok(app.session_generation()))?;
        Ok(AsyncTask::new(Self { generation, action }))
    }
    fn op(&self) -> (&'static str, Class) {
        match self.action {
            Action::InvoiceEmail(..) => ("invoice_email_finish", Class::Write),
            Action::RecurringBills => ("recurring_bills", Class::Write),
            Action::StripeRefresh(_) => ("stripe_refresh", Class::Write),
            Action::Render(_) => ("render_async", Class::Read),
            Action::Run(_) => ("run_async", Class::Write),
            Action::Dialog(..) => ("backup_dialog_async", Class::Write),
            Action::Create { .. } => ("create_protected_book_async", Class::Write),
            Action::Restore { .. } => ("restore_book_from_gate_async", Class::Write),
            Action::RecoverySaved(_) => ("recovery_saved_async", Class::Write),
            Action::Hosting(_) => ("hosting", Class::Read),
        }
    }
}
impl Task for Work {
    type Output = (String,u64);
    type JsValue = String;

    fn compute(&mut self) -> Result<Self::Output> {
        // A pool thread: a panic here would abort the process from libuv's worker, so it gets the
        // same boundary as the main thread's calls.
        let mut app=app_blocking()?;
        let (op, class) = self.op();
        let generation = self.generation;
        let action = &mut self.action;
        if let Action::Hosting(h) = action {
            // The door itself: these move the books into or out of the host.
            return within(op, class, &mut app, |door| hosting_in(door, h)).map(|out| (out, 0));
        }
        hosting::with_desk(&mut app, hosting::TASK_WAIT, |app| within(op, class, app, |app| {
        if generation!=app.session_generation() { return Err(fail("The active user changed. Open the screen again.")); }
        let output=match action {
            Action::RecurringBills => {fb_app::recurring_bills::desktop_tick(app).map_err(app_error)?; Ok(String::new())},
            Action::StripeRefresh(company)=>{fb_app::invoice_stripe::desktop_tick(app,company).map_err(app_error)?;Ok(String::new())},
            Action::InvoiceEmail(ticket,pdf)=>serde_json::to_string(&fb_app::invoice_email::finish_pdf(app,ticket,pdf).map_err(app_error)?).map_err(|e|fail(e.to_string())),
            Action::Render(target) => render_in(app, target),
            Action::Run(command_json) => run_in(app, command_json),
            Action::Dialog(chosen,ticket) => dialog_in(app,ticket,chosen),
            Action::RecoverySaved(ticket) => { app.recovery_saved_export(ticket).map_err(app_error)?; Ok(String::new()) }
            Action::Create { path,owner,company,password,protect,chart } => {
                app.create_protected_book_with_chart(Path::new(path),owner,company,protect.then_some(password.as_str()),*chart).map_err(app_error)?;
                opened_location(app)
            }
            Action::Restore { file,name,secret } => {
                screens::backups::restore_from_gate(app,Path::new(file),name,secret).map_err(app_error)?;
                opened_location(app)
            }
            Action::Hosting(_) => Err(fail("unreachable")),
        }?;
        Ok((output,app.session_generation()))
        }))
    }
    fn resolve(&mut self, _env: Env, (output,generation): Self::Output) -> Result<String> {
        if matches!(self.action, Action::Hosting(_)) {
            return Ok(output);
        }
        guarded("resolve", Class::Read, |app| {
            if generation!=app.session_generation() { return Err(fail("The active user changed. Open the screen again.")); }
            Ok(output)
        })
    }
}

fn hosting_in(door: &mut App, action: &HostingAction) -> Result<String> {
    // The dialog belongs to the local Administrator who opened the Hosting screen.
    // Check on this worker, under the desktop door, after any queued remote work.
    if let HostingAction::Off { generation } | HostingAction::Revoke { generation, .. } = action {
        hosting::with_desk(door, hosting::TASK_WAIT, |app| {
            if app.session_generation().to_string() != *generation {
                return Err(fail("The active user changed. Open the screen again."));
            }
            Ok(())
        })?;
    }
    let done = || Ok(json(serde_json::json!({"ok": true})));
    match action {
        HostingAction::On { bind, host_name } => hosting::turn_on(door, bind, host_name).and_then(|()| done()),
        HostingAction::Off { .. } => hosting::stop(door).and_then(|()| done()),
        HostingAction::End(reason) => {
            hosting::end_from(door, reason);
            done()
        }
        HostingAction::Resume { host_name } => hosting::resume(door, host_name).map(|on| json(serde_json::json!({"ok": true, "running": on}))),
        HostingAction::Restart { host_name } => hosting::restart(door, host_name).and_then(|()| done()),
        HostingAction::Pair => hosting::pair(door).map(json),
        HostingAction::CancelPair => hosting::cancel_pair(door).and_then(|()| done()),
        HostingAction::Revoke { device, .. } => hosting::revoke(door, device).map(json),
        HostingAction::Repair { kind, host_name } => hosting::repair(door, kind, host_name).map(|aside| json(serde_json::json!({"ok": true, "aside": aside}))),
    }
}

/// `configureRemote(path)`: the absolute path of the `fb-remote` helper (packaged next to this
/// module), set once at startup. Hosting starts it as the TLS listener; it never sees the book.
#[napi]
pub fn configure_remote(path: String) -> Result<()> {
    free("configure_remote", || hosting::configure(std::path::PathBuf::from(path)))
}

/// Trusted main process, after the books open: whether hosting was left on (see `hostingResumeAsync`).
#[napi]
pub fn hosting_wanted() -> Result<bool> {
    boundary::init();
    let mut door = app()?;
    within("hosting_wanted", Class::Read, &mut door, |door| Ok(hosting::wanted(door)))
}

/// Trusted main process: whether these books are being hosted. Never waits (Lock uses it).
#[napi]
pub fn hosting_active() -> Result<bool> {
    free("hosting_active", || Ok(hosting::active()))
}

/// Trusted main process: `{running, bind, connected}` with nobody signed in (the network watch and
/// the window title). Never waits; no secret.
#[napi]
pub fn hosting_status() -> Result<String> {
    free("hosting_status", || Ok(json(hosting::status())))
}

/// What the Hosting screen shows (JSON; no secret). Administrators get the details.
#[napi]
pub fn hosting_state(skip_if_busy: Option<bool>) -> Result<String> {
    boundary::init();
    let mut door = app()?;
    let wait = if skip_if_busy == Some(true) { std::time::Duration::ZERO } else { hosting::DOOR_WAIT };
    within("hosting_state", Class::Read, &mut door, |door| hosting::state(door, wait).map(json))
}

fn hosting_task(action: HostingAction) -> Result<AsyncTask<Work>> {
    Ok(AsyncTask::new(Work { generation: 0, action: Action::Hosting(action) }))
}

/// Administrator: turn hosting on, listening on `bind` (one private address of this computer).
#[napi]
pub fn hosting_on_async(bind: String, host_name: String) -> Result<AsyncTask<Work>> { hosting_task(HostingAction::On { bind, host_name }) }
/// Administrator: turn hosting off.
#[napi]
pub fn hosting_off_async(generation: String) -> Result<AsyncTask<Work>> { hosting_task(HostingAction::Off { generation }) }
/// Trusted main process: stop hosting (quit, the network changed); the setting stays on.
#[napi]
pub fn hosting_end_async(reason: String) -> Result<AsyncTask<Work>> { hosting_task(HostingAction::End(reason)) }
/// Trusted main process, after the books open: start again if hosting was left on.
#[napi]
pub fn hosting_resume_async(host_name: String) -> Result<AsyncTask<Work>> { hosting_task(HostingAction::Resume { host_name }) }
/// Administrator: Restart hosting (checks the Administrator before anything stops).
#[napi]
pub fn hosting_restart_async(host_name: String) -> Result<AsyncTask<Work>> { hosting_task(HostingAction::Restart { host_name }) }
/// Administrator: a one-time pairing code.
#[napi]
pub fn hosting_pair_async() -> Result<AsyncTask<Work>> { hosting_task(HostingAction::Pair) }
#[napi]
pub fn hosting_cancel_pair_async() -> Result<AsyncTask<Work>> { hosting_task(HostingAction::CancelPair) }
/// Administrator: revoke a paired computer.
#[napi]
pub fn hosting_revoke_async(device: String, generation: String) -> Result<AsyncTask<Work>> { hosting_task(HostingAction::Revoke { device, generation }) }
/// Administrator: the repair the Hosting screen offers for a start that was refused, then start.
#[napi]
pub fn hosting_repair_async(kind: String, host_name: String) -> Result<AsyncTask<Work>> { hosting_task(HostingAction::Repair { kind, host_name }) }

fn run_in(app: &mut App, command_json: &str) -> Result<String> {
    let command = fb_app::commands::parse_command(command_json).map_err(fail)?;
    boundary::set_route(match &command {
        fb_app::Command::SubmitForm { path, .. } => boundary::route_of(path),
        other => serde_json::to_value(other).ok().and_then(|v| v["type"].as_str().map(str::to_owned)).unwrap_or_else(|| "command".into()),
    });
    // The desktop entry: a form whose single-use token this post spent comes back to the page
    // with a fresh token instead of an expired form (fb-app `App::run_desktop`).
    let outcome: Outcome = app.run_desktop(command).map_err(app_error)?;
    serde_json::to_string(&outcome).map_err(|e| fail(e.to_string()))
}

fn dialog_in(app: &mut App, ticket: &str, chosen: &str) -> Result<String> {
    let outcome = screens::backups::dialog_result(app, ticket, chosen).map_err(app_error)?;
    serde_json::to_string(&outcome).map_err(|e| fail(e.to_string()))
}

/// `books.run` for a command that may take long ("Create backup now"): the same parsing and dispatch as
/// `run`, on a pool thread. Resolves to the `Outcome` JSON.
#[napi]
pub fn run_async(command_json: String) -> Result<AsyncTask<Work>> {
    Work::new(Action::Run(Zeroizing::new(command_json)))
}

/// Trusted desktop catch-up; never exposed by preload.
#[napi]
pub fn stripe_refresh_async(company:String)->Result<AsyncTask<Work>> {Work::new(Action::StripeRefresh(company))}

/// Main-process-only PDF preparation; these are absent from the renderer bridge.
#[napi]
pub fn invoice_email_print(ticket:String)->Result<String> {
    guarded("invoice_email_print", Class::Write, |app| fb_app::invoice_email::print_document(app,&ticket).map_err(app_error))
}
#[napi]
pub fn invoice_email_finish_async(ticket:String,pdf:napi::bindgen_prelude::Buffer)->Result<AsyncTask<Work>> {
    if pdf.len()>8*1024*1024 {return Err(fail("Invoice PDF is too large."));}
    let pdf = free("invoice_email_finish_async", || Ok(Zeroizing::new(pdf.to_vec())))?;
    Work::new(Action::InvoiceEmail(ticket,pdf))
}

/// `backup_dialog` on a pool thread: the restore, copy or folder step after the native dialog.
#[napi]
pub fn backup_dialog_async(ticket: String, chosen: String) -> Result<AsyncTask<Work>> {
    Work::new(Action::Dialog(chosen, Zeroizing::new(ticket)))
}

/// The command/query JSON Schema.
#[napi]
pub fn schema() -> Result<String> {
    free("schema", || Ok(json(fb_app::commands::schema())))
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
pub fn invoice_delivery_enabled() -> bool { cfg!(feature="invoice-delivery") }

#[napi]
pub fn configure_network(path: String) -> Result<()> {
    if !bankfeeds_enabled() && !invoice_delivery_enabled() {
        return Err(fail("This build has no network helper."));
    }
    let path = std::path::PathBuf::from(path);
    if !path.is_absolute() || !path.is_file() {
        return Err(fail("The network helper was not found."));
    }
    NET_PATH.set(path.clone()).map_err(|_| fail("The network helper is already configured."))?;
    guarded("configure_network", Class::Read, |app| {
        app.net = Some(Box::new(fb_app::net::ProcessNet::new(path)));
        Ok(())
    })
}

/// The origins an `Outcome::External` URL may open in the system browser: the OpenAI issuer over
/// https (the only External the plugins produce). A `fake-network` build (smoke only) also allows
/// the fake OAuth provider's exact `http://127.0.0.1:<port>` origin from `FB_NET_FAKE_OPENAI`.
#[napi]
pub fn external_origins() -> Result<String> {
    free("external_origins", external_origins_in)
}

fn external_origins_in() -> Result<String> {
    #[allow(unused_mut)]
    let mut origins = vec![];
    if plugins_enabled() {origins.push("https://auth.openai.com".to_owned());}
    if invoice_delivery_enabled() {origins.push("https://accounts.google.com".to_owned());}
    #[cfg(feature = "fake-network")]
    if let Ok(fake) = std::env::var("FB_NET_FAKE_OPENAI") {
        let fake = fake.trim_end_matches('/');
        let port = fake.strip_prefix("http://127.0.0.1:").and_then(|p| p.parse::<u16>().ok());
        if let Some(port) = port.filter(|p| *p != 0) {
            origins.push(format!("http://127.0.0.1:{port}"));
        }
    }
    #[cfg(feature = "fake-network")]
    if let Ok(fake) = std::env::var("FB_NET_FAKE_GMAIL") {
        if let Some(port)=fake.trim_end_matches('/').strip_prefix("http://127.0.0.1:").and_then(|p|p.parse::<u16>().ok()).filter(|p|*p!=0) {
            origins.push(format!("http://127.0.0.1:{port}"));
        }
    }
    Ok(json(serde_json::json!(origins)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The books are process-wide; tests that use them take turns.
    pub(crate) fn turn() -> std::sync::MutexGuard<'static, ()> {
        static TURN: Mutex<()> = Mutex::new(());
        TURN.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn the_main_threads_door_never_waits_while_a_task_holds_the_books() {
        let _turn = turn();
        let held = app_blocking().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        // Another thread stands in for the main thread: it must get BUSY at once, not block.
        std::thread::spawn(move || tx.send(app().err().map(|e| e.reason.clone()))).join().unwrap().unwrap();
        assert_eq!(rx.recv().unwrap().as_deref(), Some("BUSY"));
        drop(held);
        assert!(app().is_ok());
        let mut work = Work { generation: app().unwrap().session_generation(), action: Action::Run(Zeroizing::new("{\"type\":\"Nope\"}".to_owned())) };
        assert!(work.compute().is_err(), "a task parses commands exactly like run");
        assert!(render_async("not a local path".to_owned()).is_err());
        assert!(render_async(format!("/{}", "x".repeat(4096))).is_err());
        let mut render = Work { generation: app().unwrap().session_generation(), action: Action::Render("/c/0195b4a4-55af-719e-96cb-a2f946b067bf/backups/".to_owned()) };
        assert!(render.compute().is_err(), "a worker cannot render a closed book");
        let generation=app().unwrap().session_generation();
        let mut stale=Work { generation, action: Action::Run(Zeroizing::new("invalid old request".to_owned())) };
        app().unwrap().sign_out();
        assert!(stale.compute().unwrap_err().reason.contains("active user changed"), "a stale worker is refused before parsing or executing its request");
    }
}


/// Trusted main process only, before sign-in. Older built-in books can have saved connections.
#[napi]
pub fn has_credentials() -> Result<bool> {
    guarded("has_credentials", Class::Read, |app| app.has_credentials().map_err(app_error))
}

/// OS-held connection secret, never derived from a book or included in its backups.
/// `state` is the shell's finding (`ready`, `created`, `replaced` or `unavailable`); an unavailable
/// key is never configured, so sign-in leaves saved connections exactly as they are.
#[napi]
pub fn configure_keys(device_hex: String, state: String, os_store_usable: bool, alternates_json: Option<String>) -> Result<()> {
    let device = Zeroizing::new(device_hex);
    // Superseded connection keys kept aside on this computer: read with, never written with.
    let alternates: Vec<Zeroizing<String>> = match alternates_json.map(Zeroizing::new) {
        Some(raw) if !raw.is_empty() => serde_json::from_str::<Vec<String>>(&raw).map_err(|_| fail("Invalid connection keys."))?.into_iter().map(Zeroizing::new).collect(),
        _ => vec![],
    };
    guarded("configure_keys", Class::Write, |app| {
        let state = fb_app::ConnectionKeys::parse(&state).ok_or_else(|| fail("Unknown connection key state."))?;
        let key = match state {
            fb_app::ConnectionKeys::Unavailable => None,
            _ => Some(master(&device)?),
        };
        app.device_alternates = match state {
            fb_app::ConnectionKeys::Unavailable => vec![],
            _ => alternates.iter().take(4).map(|k| master(k)).collect::<Result<_>>()?,
        };
        app.device_key = key;
        app.connection_keys = state;
        app.os_store_usable = os_store_usable && state != fb_app::ConnectionKeys::Unavailable;
        app.connection_keys_changed().map_err(app_error)
    })
}

/// Trusted main process: whether the open book is a restored copy (not the book that opens at start).
#[napi]
pub fn mark_copy(copy: bool) -> Result<()> {
    guarded("mark_copy", Class::Read, |app| {
        app.mark_copy(copy);
        Ok(())
    })
}

/// Trusted main process, after the owner confirmed "Use this as my books": make the open copy the
/// main books. `previous` is the main book the shell knows of ("" for none); it is set aside and the
/// saved choice is pointed at the open book, under a journal that startup finishes or undoes.
/// Returns `{main, set_aside, settled}`. Nothing is deleted.
#[napi]
pub fn use_restored_copy(previous: String) -> Result<String> {
    guarded("use_restored_copy", Class::Write, |app| {
        let previous = (!previous.is_empty()).then(|| std::path::PathBuf::from(previous));
        let made = app.make_open_book_main(previous.as_deref()).map_err(app_error)?;
        Ok(json(serde_json::to_value(made).map_err(|e| fail(e.to_string()))?))
    })
}

/// Trusted main process, at start before it reads which books to open: finish or undo an
/// interrupted change of main books. Returns "none", "undone" or "finished"; an error means the
/// shell must not open any book on a guess.
#[napi]
pub fn recover_book_change() -> Result<String> {
    guarded("recover_book_change", Class::Write, |app| {
        let root = app.backups.root().map_err(app_error)?.to_owned();
        if app.is_open() { return Err(fail("Close the books before recovering a change of books.")); }
        let done = fb_app::books::recover_book_change(&root).map_err(app_error)?;
        Ok(serde_json::to_value(done).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default())
    })
}

/// Trusted main process, after any open: record a committed change of main books whose audit
/// event a restart still owes, then clear its journal. Returns whether anything was finished.
///
/// A panic here keeps the books open (when the connection is idle again): the shell runs this after
/// every open and only logs its errors, so closing would send the owner back to the unlock screen
/// with no explanation, and every time if the panic repeats. Nothing here changes the session; the
/// audit event is written once per change in its own transaction, and the journal stays until
/// that is done, so the next try simply repeats it.
#[napi]
pub fn finish_book_change() -> Result<bool> {
    guarded("finish_book_change", Class::RetryableWrite, |app| app.finish_book_change().map_err(app_error))
}

#[napi]
pub fn protection_state(path: String) -> Result<String> {
    free("protection_state", || {
        let record = fb_app::protection::Record::read(Path::new(&path)).map_err(app_error)?;
        Ok(match record {
            Some(r) => json(serde_json::json!({"known":true,"builtin":r.keys.mode == fb_app::protection::builtin_mode(),"ask_password":r.ask_password})),
            None => json(serde_json::json!({"known":false,"builtin":false,"ask_password":true})),
        })
    })
}

#[napi]
pub fn recover_key_change(path: String) -> Result<bool> {
    // The recovery's own file steps are built to survive being killed at any point; a panic in
    // them is caught here and the next open runs the same recovery again.
    guarded("recover_key_change", Class::Write, |app| {
        if app.is_open() { return Err(fail("Close the books before recovering a key change.")); }
        fb_app::protection::recover_key_change(Path::new(&path)).map_err(app_error)
    })
}

#[napi]
pub fn create_protected_book(path: String, owner_name: String, company_name: String, password: String, protect: bool, starter_chart: Option<String>) -> Result<String> {
    let password = Zeroizing::new(password);
    let chart = chart_choice(starter_chart)?;
    guarded("create_protected_book", Class::Write, |app| {
        app.create_protected_book_with_chart(Path::new(&path), &owner_name, &company_name, protect.then_some(password.as_str()), chart).map_err(app_error)?;
        opened_location(app)
    })
}

#[napi]
pub fn open_protected_book(env: Env, path: String, secret: String) -> Result<String> {
    let secret = Zeroizing::new(secret);
    guarded("open_protected_book", Class::Write, |app| {
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
        opened_location(app)
    })
}

/// Trusted main-process use only: safeStorage sealing never crosses renderer IPC.
#[napi]
pub fn key_for_sealing() -> Result<String> {
    guarded("key_for_sealing", Class::Read, |app| {
        if !app.os_store_usable || app.builtin() { return Err(fail("No OS-sealed book key is needed.")); }
        Ok(keys::to_hex(app.master_key().map_err(app_error)?.as_ref()))
    })
}

#[napi]
pub fn recovery_kit() -> Result<String> {
    guarded("recovery_kit", Class::Read, recovery_kit_in)
}

fn recovery_kit_in(app: &mut App) -> Result<String> {
    let key = app.recovery_text().map_err(app_error)?;
    let name = app.company_name_for_kit().map_err(app_error)?;
    Ok(format!("FreedomBooks Emergency Kit\n{name}\nDate: {}\n\n{}\n\nKeep this key separate from your computer and backups.\nTo restore: install FreedomBooks, choose Restore a backup, select your backup file, and enter this recovery key.\nAnyone with this key and a backup can open your books.\n", (app.today)(), key.as_str()))
}

#[napi]
pub fn recovery_key() -> Result<String> {
    // Only called by the desktop's path-bound Copy action, never exposed as renderer IPC.
    guarded("recovery_key", Class::Read, |app| Ok(app.recovery_text().map_err(app_error)?.to_string()))
}

#[napi]
pub fn recovery_export() -> Result<String> {
    guarded("recovery_export", Class::Read, |app| {
        let kit = Zeroizing::new(recovery_kit_in(app)?);
        let ticket = app.recovery_export_ticket().map_err(app_error)?;
        Ok(json(serde_json::json!({"kit":kit.as_str(),"ticket":ticket})))
    })
}

#[napi]
pub fn validate_recovery_export(ticket: String) -> Result<()> {
    guarded("validate_recovery_export", Class::Read, |app| app.validate_recovery_export(&ticket).map_err(app_error))
}

#[napi]
pub fn recovery_saved_async(ticket: String) -> Result<AsyncTask<Work>> {
    Work::new(Action::RecoverySaved(Zeroizing::new(ticket)))
}

#[napi]
pub fn restore_book_from_gate(file: String, name: String, secret: String) -> Result<String> {
    let secret = Zeroizing::new(secret);
    guarded("restore_book_from_gate", Class::Write, |app| {
        fb_app::screens::backups::restore_from_gate(app, Path::new(&file), &name, &secret).map_err(app_error)?;
        opened_location(app)
    })
}


/// First-run creation uses the same exclusive worker as backup/restore.
#[napi]
pub fn create_protected_book_async(path: String, owner: String, company: String, password: String, protect: bool, starter_chart: Option<String>) -> Result<AsyncTask<Work>> {
    let password = Zeroizing::new(password);
    let chart = chart_choice(starter_chart)?;
    Work::new(Action::Create { path, owner, company, password, protect, chart })
}

#[napi]
pub fn restore_book_from_gate_async(file: String, name: String, secret: String) -> Result<AsyncTask<Work>> {
    Work::new(Action::Restore { file, name, secret: Zeroizing::new(secret) })
}

// Compatibility for trusted older callers; renderer setup always submits an explicit choice.
fn chart_choice(value: Option<String>) -> Result<fb_app::commands::StarterChart> {
    serde_json::from_value(serde_json::json!(value.as_deref().unwrap_or("empty")))
        .map_err(|_| fail("Choose a chart of accounts."))
}


/// CRASHLOG: this build's identity (commit, tree state, date, features, schema count) as JSON.
#[napi]
pub fn build_info() -> Result<String> {
    free("build_info", || Ok(json(fb_diag::build().json())))
}

/// CRASHLOG: write the problem log into `dir` (absolute; created owner-only). The main process
/// passes the state folder, or the isolated profile's `logs/` in development runs.
#[napi]
pub fn configure_log(dir: String) -> Result<()> {
    free("configure_log", || fb_diag::configure(Path::new(&dir)).map_err(|_| fail("The problem log folder could not be prepared.")))
}

/// CRASHLOG: one main-process problem, as `electron/main/diagnostics.cjs` reduces it:
/// `{op, kind, name?, code?, frames?: [{file, line, col}], route?, book_open?}`. No message is
/// accepted: the error's name and code come from fixed lists, `op` and `kind` must be code-shaped,
/// and only frames in the app's own files are kept (see `fb_diag::scrub`). Anything else in the
/// event, a `message` or `stack` included, is ignored.
#[napi]
pub fn log_event(event_json: String) -> Result<()> {
    free("log_event", || {
        if event_json.len() > 64 * 1024 {
            return Err(fail("Log event too large."));
        }
        let v: serde_json::Value = serde_json::from_str(&event_json).map_err(|_| fail("Invalid log event."))?;
        fb_diag::record(main_event(&v));
        Ok(())
    })
}

/// The log entry for one main-process event (see [`log_event`]).
pub(crate) fn main_event(v: &serde_json::Value) -> fb_diag::Entry {
    use fb_diag::scrub;
    let text = |k: &str| v.get(k).and_then(|x| x.as_str());
    // A main-process step: lowercase words joined by `-`, `.` or `:` (`sync-keys`, `books.run`).
    let op = text("op")
        .filter(|op| op.len() <= 40 && op.starts_with(|c: char| c.is_ascii_lowercase()) && op.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-.:".contains(c)))
        .unwrap_or("main");
    let kind = text("kind")
        .filter(|kind| matches!(*kind, "caught" | "error" | "uncaughtException" | "unhandledRejection"))
        .unwrap_or("main");
    let mut kind = kind.to_owned();
    if let Some(name) = text("name") {
        kind.push(':');
        kind.push_str(scrub::js_error_name(name));
        if let Some(code) = text("code") {
            kind.push(':');
            kind.push_str(&scrub::js_error_code(code));
        }
    }
    let frames = v.get("frames").and_then(|f| f.as_array()).map(|frames| {
        frames.iter().take(24).filter_map(|f| {
            scrub::js_frame(f.get("file")?.as_str()?, f.get("line")?.as_u64()?, f.get("col").and_then(|c| c.as_u64()).unwrap_or(0))
        }).collect()
    }).unwrap_or_default();
    fb_diag::Entry {
        source: "main",
        op: op.into(),
        route: text("route").map(boundary::route_of),
        kind,
        backtrace: frames,
        book_open: v.get("book_open").and_then(|b| b.as_bool()),
        ..Default::default()
    }
}

/// CRASHLOG: the text "Copy diagnostic summary" puts on the clipboard (no log bodies).
#[napi]
pub fn diagnostic_summary() -> Result<String> {
    free("diagnostic_summary", || Ok(fb_diag::summary()))
}

/// Trusted host timer only; no renderer channel exposes this method.
#[napi]
pub fn recurring_bills_async()->Result<AsyncTask<Work>> {Work::new(Action::RecurringBills)}
