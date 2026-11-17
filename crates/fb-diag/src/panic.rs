//! The panic hook and the catch used at the bridge.
//!
//! A panic inside the panic hook aborts the whole process, so the hook does the least it can and
//! nothing that can fail or wait:
//! - it never writes to stderr (a closed terminal, a full disk or a pipe nobody reads would make a
//!   write fail or block), never formats, never takes a lock and never touches the log;
//! - it copies the panic's location into fixed storage, picks a fixed category for it from a
//!   table (never the message), and takes a backtrace handle whose symbols are not resolved yet;
//! - it hands that to the [`catch`] around the current call (same thread), or, on a thread no
//!   catch guards, posts it to a small lock-free mailbox that the next bridge call writes out.
//!
//! Resolving the backtrace, reducing it to code coordinates and writing the log all happen after
//! `catch_unwind` has returned, on the normal path, inside [`quietly`]: if any of that fails, the
//! result is "logged nothing", never a second panic.
use crate::scrub;
use std::backtrace::Backtrace;
use std::cell::Cell;
use std::panic::{AssertUnwindSafe, PanicHookInfo};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};

/// A panic, reduced to what the log may hold: where in the code, a fixed category and code-only
/// frames. Nothing from the panic's message is kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Caught {
    /// `crates/fb-app/src/x.rs:12:5`, or `unknown`.
    pub location: String,
    /// A fixed phrase chosen by [`category`], never text from the panic.
    pub category: &'static str,
    /// Frames in this source tree: `fb_app::screens::render at crates/fb-app/src/screens.rs:150:9`.
    pub backtrace: Vec<String>,
}

const FILE_MAX: usize = 200;

/// What the hook stores: plain copies into fixed storage, no formatting, no text from the payload.
pub(crate) struct Raw {
    file: [u8; FILE_MAX],
    file_len: usize,
    line: u32,
    column: u32,
    category: &'static str,
    backtrace: Option<Backtrace>,
}

impl Raw {
    fn take(info: &PanicHookInfo<'_>) -> Raw {
        let mut raw = Raw { file: [0; FILE_MAX], file_len: 0, line: 0, column: 0, category: category(info.payload_as_str()), backtrace: None };
        if let Some(l) = info.location() {
            let bytes = l.file().as_bytes();
            let n = bytes.len().min(FILE_MAX);
            raw.file[..n].copy_from_slice(&bytes[..n]);
            raw.file_len = n;
            raw.line = l.line();
            raw.column = l.column();
        }
        // Capturing walks the stack; symbols are resolved later. Skipped if this thread panicked
        // while resolving a backtrace (std's backtrace lock may still be held).
        if !RESOLVING.try_with(Cell::get).unwrap_or(true) {
            raw.backtrace = Some(Backtrace::force_capture());
        }
        raw
    }

    /// Reduce to what the log may hold. Runs after the catch, never inside the hook.
    fn reduce(self) -> Caught {
        let category = self.category;
        let reduced = quietly(move || {
            let file = String::from_utf8_lossy(&self.file[..self.file_len]).into_owned();
            let location = if self.file_len == 0 { "unknown".to_owned() } else { scrub::location(&file, self.line, self.column) };
            let backtrace = match &self.backtrace {
                Some(bt) => {
                    let _resolving = Resolving::enter();
                    scrub::backtrace(&bt.to_string())
                }
                None => vec![],
            };
            Caught { location, category, backtrace }
        });
        reduced.unwrap_or(Caught { location: "unknown".into(), category, backtrace: vec![] })
    }
}

/// The fixed category for a panic message: a phrase from this table, chosen by comparing the
/// start of the message with the standard library's own fixed texts. Nothing from the message is
/// returned; a message that matches none of them is just "panic with a message".
pub fn category(message: Option<&str>) -> &'static str {
    const KNOWN: &[(&str, &str)] = &[
        ("called `Option::unwrap()` on a `None` value", "unwrap on None"),
        ("called `Result::unwrap()` on an `Err` value", "unwrap on Err"),
        ("called `Result::unwrap_err()` on an `Ok` value", "unwrap_err on Ok"),
        ("index out of bounds", "index out of bounds"),
        ("range start index", "slice range out of bounds"),
        ("range end index", "slice range out of bounds"),
        ("slice index starts at", "slice range out of bounds"),
        ("byte index", "string index not on a character boundary"),
        ("start byte index", "string index not on a character boundary"),
        ("end byte index", "string index not on a character boundary"),
        ("begin <= end", "string range out of order"),
        ("attempt to add with overflow", "arithmetic overflow"),
        ("attempt to subtract with overflow", "arithmetic overflow"),
        ("attempt to multiply with overflow", "arithmetic overflow"),
        ("attempt to negate with overflow", "arithmetic overflow"),
        ("attempt to shift", "arithmetic overflow"),
        ("attempt to divide by zero", "division by zero"),
        ("attempt to calculate the remainder with a divisor of zero", "division by zero"),
        ("attempt to divide with overflow", "arithmetic overflow"),
        ("internal error: entered unreachable code", "unreachable code"),
        ("not yet implemented", "not yet implemented"),
        ("not implemented", "not implemented"),
        ("already borrowed", "RefCell already borrowed"),
        ("already mutably borrowed", "RefCell already borrowed"),
        ("assertion failed", "assertion failed"),
        ("assertion `left", "assertion failed"),
        ("explicit panic", "explicit panic"),
        ("cannot access a Thread Local Storage value", "thread-local storage gone"),
    ];
    match message {
        None => "panic with a non-text payload",
        Some(m) => KNOWN.iter().find(|(start, _)| m.starts_with(start)).map_or("panic with a message", |(_, phrase)| phrase),
    }
}

thread_local! {
    /// How many [`catch`] or [`quietly`] frames are around this thread's current code.
    static GUARDED: Cell<u32> = const { Cell::new(0) };
    /// The hook's capture for the innermost catch on this thread.
    static SLOT: Cell<Option<Raw>> = const { Cell::new(None) };
    /// This thread is resolving a backtrace (holding std's backtrace lock).
    static RESOLVING: Cell<bool> = const { Cell::new(false) };
}

struct Resolving;
impl Resolving {
    fn enter() -> Resolving {
        let _ = RESOLVING.try_with(|r| r.set(true));
        Resolving
    }
}
impl Drop for Resolving {
    fn drop(&mut self) {
        let _ = RESOLVING.try_with(|r| r.set(false));
    }
}

struct Guarded;
impl Guarded {
    fn enter() -> Guarded {
        let _ = GUARDED.try_with(|g| g.set(g.get().saturating_add(1)));
        Guarded
    }
}
impl Drop for Guarded {
    fn drop(&mut self) {
        let _ = GUARDED.try_with(|g| g.set(g.get().saturating_sub(1)));
    }
}

/// Panics on threads that no catch guards, waiting for the next bridge call to write them.
const MAILBOX: usize = 8;
static OTHER_THREADS: [AtomicPtr<Raw>; MAILBOX] = [const { AtomicPtr::new(ptr::null_mut()) }; MAILBOX];
static MISSED: AtomicU32 = AtomicU32::new(0);

fn post(raw: Raw) {
    let boxed = Box::into_raw(Box::new(raw));
    for slot in &OTHER_THREADS {
        if slot.compare_exchange(ptr::null_mut(), boxed, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
            return;
        }
    }
    // SAFETY: `boxed` came from Box::into_raw above and was not stored anywhere.
    drop(unsafe { Box::from_raw(boxed) });
    MISSED.fetch_add(1, Ordering::Relaxed);
}

/// Write out panics from threads no catch guarded (what the hook could keep of them). Called on
/// the normal path by every bridge call; never fails.
pub fn drain_other_threads() {
    for slot in &OTHER_THREADS {
        let taken = slot.swap(ptr::null_mut(), Ordering::AcqRel);
        if taken.is_null() {
            continue;
        }
        // SAFETY: only `post` stores pointers here (from Box::into_raw), and the swap gave this
        // call sole ownership.
        let raw = unsafe { Box::from_raw(taken) };
        let caught = raw.reduce();
        crate::log::record(crate::log::Entry {
            op: "thread".into(),
            kind: "panic".into(),
            detail: Some(caught.category),
            location: Some(caught.location),
            backtrace: caught.backtrace,
            outcome: vec!["the thread stopped"],
            ..Default::default()
        });
    }
    if MISSED.swap(0, Ordering::Relaxed) > 0 {
        crate::log::record(crate::log::Entry {
            op: "thread".into(),
            kind: "panic".into(),
            outcome: vec!["more panics on other threads were not recorded"],
            ..Default::default()
        });
    }
}

static RAW_STDERR: AtomicBool = AtomicBool::new(false);

/// Install the hook (once per process; later calls do nothing).
pub fn install_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // Development only: FB_DIAG_STDERR=raw also runs Rust's default hook (raw message on
        // stderr), so a test's own assertion text stays readable. Never set it where real books
        // are open: the default hook prints the message, and it can block on a full pipe.
        RAW_STDERR.store(std::env::var("FB_DIAG_STDERR").is_ok_and(|v| v == "raw"), Ordering::Relaxed);
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if RAW_STDERR.load(Ordering::Relaxed) {
                previous(info);
            }
            hook(info);
        }));
    });
}

fn hook(info: &PanicHookInfo<'_>) {
    let raw = Raw::take(info);
    if GUARDED.try_with(Cell::get).unwrap_or(0) > 0 {
        // `set` drops an earlier capture nobody took (an inner catch_unwind of other code). If
        // thread-local storage is already being torn down, nothing can be kept.
        let _ = SLOT.try_with(|s| s.set(Some(raw)));
    } else {
        post(raw);
    }
}

/// Run `f`, turning a panic into `Err` with what the hook captured, reduced to code coordinates.
/// Unwinding stops here: nothing above this frame sees it. The panic payload is never read for
/// text and is dropped under its own catch, so a payload whose `Drop` panics cannot unwind out.
pub fn catch<T>(f: impl FnOnce() -> T) -> Result<T, Caught> {
    let _ = SLOT.try_with(|s| s.take());
    let result = {
        let _guarded = Guarded::enter();
        std::panic::catch_unwind(AssertUnwindSafe(f))
    };
    match result {
        Ok(value) => Ok(value),
        Err(payload) => {
            let raw = SLOT.try_with(|s| s.take()).ok().flatten();
            drop_payload(payload);
            Err(match raw {
                Some(raw) => raw.reduce(),
                None => Caught { location: "unknown (no hook)".into(), category: "panic", backtrace: vec![] },
            })
        }
    }
}

/// Run `f` where a panic must only mean "nothing happened": the diagnostics' own work (resolving
/// backtraces, writing the log) and recovery steps. A panic is swallowed and returns `None`.
pub fn quietly<T>(f: impl FnOnce() -> T) -> Option<T> {
    let result = {
        let _guarded = Guarded::enter();
        std::panic::catch_unwind(AssertUnwindSafe(f))
    };
    match result {
        Ok(value) => Some(value),
        Err(payload) => {
            let _ = SLOT.try_with(|s| s.take());
            drop_payload(payload);
            None
        }
    }
}

fn drop_payload(payload: Box<dyn std::any::Any + Send>) {
    let _guarded = Guarded::enter();
    if let Err(again) = std::panic::catch_unwind(AssertUnwindSafe(move || drop(payload))) {
        std::mem::forget(again);
    }
    let _ = SLOT.try_with(|s| s.take());
}
