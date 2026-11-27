//! The panic boundary. Every call from the shell into Rust runs inside [`guarded`], [`within`] or
//! [`free`], so a panic anywhere below becomes a typed error (`FB_PANIC {...}`) instead of unwinding
//! into Node-API, where Rust aborts the whole process (an `extern "C"` frame cannot be unwound).
//!
//! What a caught panic leaves behind, and why it is safe:
//! - **The book transaction** is rolled back. `Book::write` and `Book::read` hold their SQLite
//!   transaction in a rusqlite `Transaction`, whose `Drop` rolls back while unwinding, and reset
//!   their callback, owner and `query_only` scopes through `Drop` guards first (fb-db tests
//!   `callback_error_and_panic_reset_authorization_before_library_rollback`,
//!   `read_callback_panic_restores_query_only_...`). After the catch,
//!   `Book::idle_after_unwind` checks it: no transaction open, scopes clear, PRAGMAs in place.
//! - **Earlier transactions of the same request** stay committed, exactly as when a handler returns
//!   an error after a first write. The book's commit counter tells which case it was, and the
//!   owner is told ("nothing was saved" or "part of this may have been saved"). The count is
//!   paired with the connection's identity: when the call replaced the open book (a restore that
//!   opened the restored copy), the counts cannot be compared and the answer is "unknown".
//! - **The `APP` lock** is never poisoned: the guard is taken outside the catch and is still held
//!   while recovering. A poisoned lock (a panic outside every boundary) is recovered by closing the
//!   books.
//! - **In-memory session state** (form tokens, reviews, flash messages, sign-in, protection and
//!   backup state) may be half-updated by a handler that panicked. After any call that can change
//!   the books or the session ([`Class::Write`]) the books are closed with `App::lock`, which drops
//!   all of it and wipes the key; the shell then reopens them the way startup does, or shows the
//!   unlock screen. Reads ([`Class::Read`]) and journaled writes that leave session state unchanged
//!   ([`Class::RetryableWrite`]) keep the books open if the book is idle; flash messages are dropped
//!   either way, so no "Saved" from the failed call can show later.
use fb_app::App;
use napi::{Error, Result, Status};
use serde_json::json;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

/// What a call may have changed if it panics halfway.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Class {
    /// Renders and queries: the books stay open when the connection is idle again.
    Read,
    /// Idempotent journal completion with atomic writes and no session changes: keep idle books open.
    RetryableWrite,
    /// Anything that can write the books or change the session: the books are closed.
    Write,
}

/// The sentinel the shell recognises (`electron/main/diagnostics.cjs`).
pub(crate) const PANIC: &str = "FB_PANIC";

thread_local! {
    static CURRENT: RefCell<Vec<(&'static str, Option<String>)>> = const { RefCell::new(Vec::new()) };
}

struct Current;
impl Current {
    fn enter(op: &'static str) -> Current {
        CURRENT.with(|c| c.borrow_mut().push((op, None)));
        Current
    }
}
impl Drop for Current {
    fn drop(&mut self) {
        CURRENT.with(|c| {
            c.borrow_mut().pop();
        });
    }
}

/// The route the current call is about (a route *name*, never a path), for its log entries.
pub(crate) fn set_route(route: String) {
    CURRENT.with(|c| {
        if let Some(last) = c.borrow_mut().last_mut() {
            last.1 = Some(route);
        }
    });
}

fn current() -> (&'static str, Option<String>) {
    CURRENT.with(|c| c.borrow().last().cloned()).unwrap_or(("unknown", None))
}

/// Once per process: the panic hook, this build's identity, (test builds only) injected panics,
/// and the books' `App` itself. Building `App` compiles the screens' templates; if that panics it
/// is caught here, recorded, and every later call answers with a panic error instead of touching
/// the half-built value (a `LazyLock` whose builder panicked panics again on every use).
pub(crate) fn init() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        fb_diag::install_hook();
        fb_diag::set_build(crate::build_identity());
        #[cfg(feature = "panic-test")]
        fb_app::test_panic::from_env();
        if let Err(caught) = fb_diag::catch(crate::build_app) {
            START_FAILED.store(true, Ordering::SeqCst);
            fb_diag::record(fb_diag::Entry {
                op: "start".into(), kind: "panic".into(), detail: Some(caught.category), location: Some(caught.location),
                backtrace: caught.backtrace, outcome: vec!["FreedomBooks could not start its books module"], book_open: Some(false), ..Default::default()
            });
        }
    });
    fb_diag::drain_other_threads();
}

static START_FAILED: AtomicBool = AtomicBool::new(false);

/// Whether building `App` panicked at start (see [`init`]).
pub(crate) fn start_failed() -> Option<Error> {
    START_FAILED.load(Ordering::SeqCst).then(|| panic_error(false, false, "none", "start"))
}

/// A call with the books: takes the main thread's lease (never waits), then [`within`].
pub(crate) fn guarded<T>(op: &'static str, class: Class, f: impl FnOnce(&mut App) -> Result<T>) -> Result<T> {
    at_door(op, class, crate::app, |door| {
        crate::hosting::with_desk(door, crate::hosting::DOOR_WAIT, |app| within(op, class, app, f))
    })
}

/// Keep the mutex guard outside the book-aware catch, and protect acquisition and routing as well.
pub(crate) fn at_door<T>(op: &'static str, class: Class,
    acquire: fn() -> Result<std::sync::MutexGuard<'static, App>>,
    f: impl FnOnce(&mut App) -> Result<T>) -> Result<T> {
    free(op, || {
        let mut door = acquire()?;
        within(op, class, &mut *door, f)
    })
}

pub(crate) trait Books {
    fn books(&mut self) -> &mut App;
}

impl Books for App {
    fn books(&mut self) -> &mut App { self }
}

impl Books for fb_app::multiuser::SessionHost {
    fn books(&mut self) -> &mut App { self.host_app() }
}

/// Which connection was open, and how many write transactions it had committed.
fn commits(app: &App) -> Option<(u64, u64)> {
    app.open.as_ref().map(|o| (o.book.connection_id(), o.book.commits()))
}

/// Run `f` with the books already leased (the guard stays outside the catch, see the module notes).
pub(crate) fn within<T, B: Books>(op: &'static str, class: Class, books: &mut B, f: impl FnOnce(&mut B) -> Result<T>) -> Result<T> {
    init();
    let _current = Current::enter(op);
    let was_open = books.books().is_open();
    let before = commits(books.books());
    match fb_diag::catch(|| f(books)) {
        Ok(result) => result,
        Err(caught) => {
            let (op, route) = current();
            let app = books.books();
            // Recovery is itself guarded: if it panics too, the books are dropped directly.
            Err(match fb_diag::quietly(|| recover(class, app, was_open, before, caught, op, route)) {
                Some(error) => error,
                None => {
                    let _ = fb_diag::quietly(|| close_hard(app));
                    panic_error(was_open, was_open, "unknown", op)
                }
            })
        }
    }
}

/// A call that does not touch the books (key wrapping, file checks, build facts).
pub(crate) fn free<T>(op: &'static str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    match fb_diag::catch(|| {
        init();
        let _current = Current::enter(op);
        f()
    }) {
        Ok(result) => result,
        Err(caught) => {
            fb_diag::quietly(|| fb_diag::record(fb_diag::Entry {
                op: op.into(), kind: "panic".into(), detail: Some(caught.category), location: Some(caught.location),
                backtrace: caught.backtrace, outcome: vec!["no books involved"], ..Default::default()
            }));
            Err(panic_error(false, false, "none", op))
        }
    }
}

/// What a commit count read before the call says now: nothing saved, something saved, or unknown
/// (the books were closed, or a different connection is open: its count cannot be compared).
fn saved(class: Class, before: Option<(u64, u64)>, after: Option<(u64, u64)>) -> &'static str {
    match (before, after) {
        (Some((was, b)), Some((now, a))) if was == now && a == b => "none",
        (Some((was, _)), Some((now, _))) if was == now => "partial",
        (Some(_), _) => "unknown",
        (None, _) if class == Class::Write => "unknown",
        (None, _) => "none",
    }
}

fn recover(class: Class, app: &mut App, was_open: bool, before: Option<(u64, u64)>, caught: fb_diag::Caught, op: &'static str, route: Option<String>) -> Error {
    let saved = saved(class, before, commits(app));
    let idle = app.open.as_ref().map(|o| o.book.idle_after_unwind());
    if app.is_open() && (class == Class::Write || matches!(idle, Some(Err(_)))) {
        close(app);
    }
    app.messages.clear();
    let closed = was_open && !app.is_open();
    let mut outcome = vec![match saved {
        "none" => "nothing saved by this call",
        "partial" => "an earlier step of this call was saved",
        _ => "saved state unknown",
    }];
    outcome.push(if closed { "books closed" } else if app.is_open() { "books kept open" } else { "books were not open" });
    if let Some(Err(reason)) = idle {
        outcome.push("connection not idle");
        outcome.push(reason);
    }
    fb_diag::record(fb_diag::Entry {
        op: op.into(), route, kind: "panic".into(), detail: Some(caught.category), location: Some(caught.location),
        backtrace: caught.backtrace, outcome, book_open: Some(app.is_open()), ..Default::default()
    });
    panic_error(closed, was_open, saved, op)
}

/// Close the books after a panic. `App::lock` is itself guarded: if it panics too, the book is
/// dropped directly (its connection closes, SQLite rolls back anything open) and the tokens go.
pub(crate) fn close(app: &mut App) {
    if fb_diag::catch(|| app.lock()).is_err() {
        close_hard(app);
    }
}

fn close_hard(app: &mut App) {
    if let Some(open) = app.open.take() {
        // A connection whose Drop panicked is forgotten rather than allowed to unwind out.
        let _ = fb_diag::quietly(move || drop(open));
    }
    app.tokens.clear();
    app.messages.clear();
}

fn panic_error(closed: bool, was_open: bool, saved: &str, op: &str) -> Error {
    let detail = json!({"closed": closed, "was_open": was_open, "saved": saved, "op": op});
    Error::new(Status::GenericFailure, format!("{PANIC} {detail}"))
}

/// Errors the owner cannot cause or fix (database, template, book-file problems) go to the log
/// with their kind only, never their text. Refusals, validation and "not found" are the app
/// working as designed.
pub(crate) fn note_error(e: &fb_app::AppError) {
    use fb_app::AppError;
    let kind: String = match e {
        AppError::Core(core) => match core.kind() {
            kind @ ("IntegrityError" | "DatabaseError") => format!("error:{kind}"),
            _ => return,
        },
        AppError::Render(_) => "error:Render".into(),
        AppError::Open(open) => {
            let name = match open {
                fb_app::opening::OpenError::WrongKey => return,
                fb_app::opening::OpenError::TooNew { .. } => "TooNew",
                fb_app::opening::OpenError::Damaged => "Damaged",
                fb_app::opening::OpenError::UpgradeCopy { .. } => "UpgradeCopy",
                fb_app::opening::OpenError::Unavailable(_) => "Unavailable",
                fb_app::opening::OpenError::KeyFileMissing { .. } => "KeyFileMissing",
            };
            format!("error:Open:{name}")
        }
        AppError::Locked | AppError::Ai(_) | AppError::Invalid(_) | AppError::NotFound(_) => return,
    };
    let (op, route) = current();
    fb_diag::record(fb_diag::Entry { op: op.into(), route, kind, ..Default::default() });
}

/// A screen path's route name for the log: the route table's name, one of the desktop's own
/// fixed pages, or "unresolved"; never ids, query text or any other part of the path.
pub(crate) fn route_of(target: &str) -> String {
    let path = target.split_once('?').map_or(target, |(p, _)| p);
    if let Some(route) = fb_app::screens::resolve(path) {
        return route.name;
    }
    fb_app::screens::desktop_route_name(path).map_or_else(|| "unresolved".into(), str::to_owned)
}
