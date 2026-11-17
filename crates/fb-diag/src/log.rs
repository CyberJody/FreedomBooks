//! The problem log: JSON lines in `<dir>/freedombooks.log`, rotated at 1 MiB with two older files
//! kept (`.1`, `.2`). Each file starts with a header line naming the build; every entry carries
//! the time, the build, the schema version and what failed (see [`crate::scrub`] for what may be
//! written: no free text). Entries made before the shell names the folder are held (up to 64)
//! and written once it does.
//!
//! The files are handled as someone else may have tampered with the folder: the folder must be a
//! real directory owned by this user (it is created 0700, never chmodded); a log file is opened
//! without following links and used only when it is a regular, single-link file owned by this
//! user and readable by nobody else. A new file is created exclusively with mode 0600. Nothing
//! here changes the permissions of a file it did not create. Writing never fails loudly: a log
//! that cannot be written (full disk, unwritable folder) is skipped.
use crate::build;
use serde_json::{Map, Value, json};
use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

pub const FILE: &str = "freedombooks.log";
const MAX_BYTES: u64 = 1024 * 1024;
const KEEP: usize = 3;
const HELD: usize = 64;
const RECENT: usize = 20;

/// One problem. Every field is code-owned: identifiers, code coordinates and fixed phrases. There
/// is deliberately no field for a message.
#[derive(Clone, Debug, Default)]
pub struct Entry {
    /// "rust" (the default) or "main".
    pub source: &'static str,
    /// The bridge call or main-process step (`render`, `run`, `sync-keys`).
    pub op: String,
    /// The screen's route name (`invoices:detail`), never its path or query.
    pub route: Option<String>,
    /// `panic`, `error:Render`, `caught:TypeError:ENOENT`, ...
    pub kind: String,
    /// A fixed phrase chosen by the code (a panic's category), never derived from a value.
    pub detail: Option<&'static str>,
    /// Where in the code: `crates/fb-app/src/x.rs:12:5`.
    pub location: Option<String>,
    /// Code-only frames.
    pub backtrace: Vec<String>,
    /// What the app did about it, as fixed phrases (`nothing saved by this call`, `books closed`).
    pub outcome: Vec<&'static str>,
    pub book_open: Option<bool>,
}

struct State {
    dir: Option<PathBuf>,
    file: Option<File>,
    size: u64,
    held: Vec<String>,
    recent: VecDeque<(String, String, String)>,
}

static STATE: Mutex<State> = Mutex::new(State { dir: None, file: None, size: 0, held: vec![], recent: VecDeque::new() });

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Write the log into `dir` from now on (created 0700 if missing). Earlier held entries follow the
/// session header. The newest earlier problems in an existing file seed the summary's list.
pub fn configure(dir: &Path) -> std::io::Result<()> {
    if !dir.is_absolute() {
        return Err(std::io::Error::other("The log folder must be an absolute path."));
    }
    crate::panic::quietly(|| configure_inner(dir)).unwrap_or_else(|| Err(std::io::Error::other("The log folder could not be prepared.")))
}

fn configure_inner(dir: &Path) -> std::io::Result<()> {
    private_dir(dir)?;
    let mut s = state();
    s.dir = Some(dir.to_owned());
    s.file = None;
    for item in read_recent(&dir.join(FILE)) {
        push_recent(&mut s, item);
    }
    // A file that cannot be opened now (full disk, read-only) is retried on each entry.
    let _ = open_current(&mut s);
    let held = std::mem::take(&mut s.held);
    for line in held {
        write_line(&mut s, &line);
    }
    drop(s);
    crate::panic::drain_other_threads();
    Ok(())
}

pub fn dir() -> Option<PathBuf> {
    state().dir.clone()
}

/// Record one problem. Never fails and never panics: a log that cannot be written is skipped.
pub fn record(entry: Entry) {
    let _ = crate::panic::quietly(move || record_inner(entry));
}

fn record_inner(entry: Entry) {
    use crate::scrub;
    let t = now();
    let kind = scrub::ident(&entry.kind);
    let op = scrub::ident(&entry.op);
    let mut line = Map::new();
    line.insert("t".into(), json!(t));
    line.insert("build".into(), json!(crate::short_version()));
    line.insert("schema".into(), json!(build().migrations));
    line.insert("source".into(), json!(if entry.source == "main" { "main" } else { "rust" }));
    line.insert("op".into(), json!(op));
    if let Some(route) = &entry.route {
        line.insert("route".into(), json!(scrub::ident(route)));
    }
    line.insert("kind".into(), json!(kind));
    if let Some(detail) = entry.detail {
        line.insert("detail".into(), json!(detail));
    }
    if let Some(l) = &entry.location {
        line.insert("location".into(), json!(scrub::code(l, 200)));
    }
    if !entry.backtrace.is_empty() {
        let frames: Vec<String> = entry.backtrace.iter().take(48).map(|f| scrub::code(f, 260)).collect();
        line.insert("backtrace".into(), json!(frames));
    }
    if !entry.outcome.is_empty() {
        line.insert("outcome".into(), json!(entry.outcome.join("; ")));
    }
    if let Some(open) = entry.book_open {
        line.insert("book".into(), json!(if open { "open" } else { "closed" }));
    }
    let text = Value::Object(line).to_string();
    let mut s = state();
    push_recent(&mut s, (t, kind, op));
    if s.dir.is_none() {
        if s.held.len() < HELD {
            s.held.push(text);
        }
        return;
    }
    write_line(&mut s, &text);
}

fn push_recent(s: &mut State, item: (String, String, String)) {
    if s.recent.len() == RECENT {
        s.recent.pop_front();
    }
    s.recent.push_back(item);
}

/// The last few problems: (time, kind, operation), oldest first.
pub fn recent(n: usize) -> Vec<(String, String, String)> {
    let s = state();
    s.recent.iter().skip(s.recent.len().saturating_sub(n)).cloned().collect()
}

fn header() -> String {
    json!({"t": now(), "header": "FreedomBooks problem log", "build": build().json(), "version": crate::short_version(),
           "os": crate::build::os(), "arch": std::env::consts::ARCH})
    .to_string()
}

#[cfg(unix)]
mod files {
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::os::unix::io::{AsRawFd, FromRawFd};
    use std::path::Path;

    fn refused(why: &'static str) -> io::Error {
        io::Error::new(io::ErrorKind::PermissionDenied, why)
    }

    fn euid() -> u32 {
        // SAFETY: geteuid has no preconditions and cannot fail.
        unsafe { libc::geteuid() }
    }

    /// The folder: created 0700 when missing (parents too); an existing one must be a real
    /// directory (not a link) owned by this user. Its permissions are never changed.
    pub(super) fn private_dir(dir: &Path) -> io::Result<()> {
        match std::fs::symlink_metadata(dir) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => match std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            },
            Err(e) => return Err(e),
        }
        let m = std::fs::symlink_metadata(dir)?;
        if !m.file_type().is_dir() {
            return Err(refused("The log folder is not a folder."));
        }
        if m.uid() != euid() {
            return Err(refused("The log folder belongs to someone else."));
        }
        Ok(())
    }

    /// Whether an opened file may be used: a regular file with one link, owned by this user,
    /// that nobody else can read or write.
    fn usable(file: &File, private: bool) -> io::Result<()> {
        let m = file.metadata()?;
        if !m.file_type().is_file() || m.nlink() != 1 || m.uid() != euid() {
            return Err(refused("The log file is not a private regular file."));
        }
        if private && m.mode() & 0o077 != 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "The log file is readable by others."));
        }
        Ok(())
    }

    /// Keep the log off descriptors 0 to 2: with stderr closed, a new file can be given fd 2, and
    /// anything else writing to stderr would then write into the log.
    fn above_stdio(file: File) -> io::Result<File> {
        let fd = file.as_raw_fd();
        if fd > 2 {
            return Ok(file);
        }
        // SAFETY: `fd` is a valid descriptor owned by `file`; F_DUPFD_CLOEXEC returns a new one.
        let moved = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
        if moved < 0 {
            return Err(io::Error::last_os_error());
        }
        drop(file);
        // SAFETY: `moved` is a fresh descriptor that nothing else owns.
        Ok(unsafe { File::from_raw_fd(moved) })
    }

    /// Open the log at `path` for appending. A missing file is created new (exclusively, 0600).
    /// An existing one is opened without following links and must pass [`usable`]; one that
    /// others can read is reported as `InvalidData` so the caller can set it aside.
    pub(super) fn open_append(path: &Path) -> io::Result<File> {
        let created = OpenOptions::new().append(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(path);
        let file = match created {
            Ok(file) => {
                // Ours: exactly 0600 whatever the umask.
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
                file
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let file = OpenOptions::new().append(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC).open(path)?;
                usable(&file, true)?;
                file
            }
            Err(e) => return Err(e),
        };
        above_stdio(file)
    }

    /// Open an existing log for reading, on the same terms (a FIFO or device never blocks this).
    pub(super) fn open_read(path: &Path) -> io::Result<File> {
        let file = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC).open(path)?;
        usable(&file, false)?;
        above_stdio(file)
    }

    /// A regular file of ours (rotation only moves our own files, never links or others' files).
    pub(super) fn ours(path: &Path) -> bool {
        std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file() && m.uid() == euid())
    }
}

#[cfg(not(unix))]
mod files {
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::path::Path;

    pub(super) fn private_dir(dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(dir)
    }
    pub(super) fn open_append(path: &Path) -> io::Result<File> {
        if std::fs::symlink_metadata(path).is_ok_and(|m| !m.file_type().is_file()) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "The log file is not a regular file."));
        }
        OpenOptions::new().create(true).append(true).open(path)
    }
    pub(super) fn open_read(path: &Path) -> io::Result<File> {
        if !std::fs::symlink_metadata(path)?.file_type().is_file() {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "The log file is not a regular file."));
        }
        File::open(path)
    }
    pub(super) fn ours(path: &Path) -> bool {
        std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
    }
}

use files::private_dir;

fn name(dir: &Path, i: usize) -> PathBuf {
    if i == 0 { dir.join(FILE) } else { dir.join(format!("{FILE}.{i}")) }
}

/// Move the files along (`.1` to `.2`, current to `.1`). Only regular files of ours are moved or
/// removed; anything else in the way stops the rotation.
fn shift(dir: &Path) -> std::io::Result<()> {
    let last = name(dir, KEEP - 1);
    if std::fs::symlink_metadata(&last).is_ok() {
        if !files::ours(&last) {
            return Err(std::io::Error::other("A log file is not ours."));
        }
        std::fs::remove_file(&last)?;
    }
    for i in (0..KEEP - 1).rev() {
        let from = name(dir, i);
        if std::fs::symlink_metadata(&from).is_ok() {
            if !files::ours(&from) {
                return Err(std::io::Error::other("A log file is not ours."));
            }
            std::fs::rename(&from, name(dir, i + 1))?;
        }
    }
    Ok(())
}

fn open_current(s: &mut State) -> std::io::Result<()> {
    let Some(dir) = s.dir.clone() else { return Err(std::io::Error::other("no log folder")) };
    let file = match files::open_append(&dir.join(FILE)) {
        // A file others can read (made by something else) is set aside, never chmodded.
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            shift(&dir)?;
            files::open_append(&dir.join(FILE))?
        }
        other => other?,
    };
    s.size = file.metadata()?.len();
    s.file = Some(file);
    // A header starts every file and every session, so each entry's build is never in doubt.
    let h = header();
    append(s, &h);
    Ok(())
}

fn rotate(s: &mut State) -> std::io::Result<()> {
    let Some(dir) = s.dir.clone() else { return Ok(()) };
    s.file = None;
    shift(&dir)?;
    let file = files::open_append(&name(&dir, 0))?;
    s.size = 0;
    s.file = Some(file);
    let h = header();
    append(s, &h);
    Ok(())
}

fn write_line(s: &mut State, line: &str) {
    if s.file.is_none() && open_current(s).is_err() {
        return;
    }
    if s.size + line.len() as u64 + 1 > MAX_BYTES && s.size > 0 && rotate(s).is_err() {
        return;
    }
    append(s, line);
}

fn append(s: &mut State, line: &str) {
    let Some(file) = s.file.as_mut() else { return };
    let mut bytes = Vec::with_capacity(line.len() + 1);
    bytes.extend_from_slice(line.as_bytes());
    bytes.push(b'\n');
    if file.write_all(&bytes).is_ok() {
        s.size += bytes.len() as u64;
    } else {
        // Full disk or similar: drop the handle; the next entry tries again.
        s.file = None;
    }
}

fn read_recent(path: &Path) -> Vec<(String, String, String)> {
    let Ok(mut file) = files::open_read(path) else { return vec![] };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(64 * 1024);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return vec![];
    }
    let mut bytes = vec![];
    if file.take(64 * 1024).read_to_end(&mut bytes).is_err() {
        return vec![];
    }
    let text = String::from_utf8_lossy(&bytes);
    let time = |t: &str| t.len() <= 32 && t.chars().all(|c| c.is_ascii_digit() || "-:.TZ+".contains(c));
    let mut out: Vec<(String, String, String)> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v.get("header").is_none())
        .filter_map(|v| {
            let t = v["t"].as_str().filter(|t| time(t))?.to_owned();
            Some((t, crate::scrub::ident(v["kind"].as_str()?), crate::scrub::ident(v["op"].as_str().unwrap_or(""))))
        })
        .collect();
    let keep = out.len().saturating_sub(RECENT);
    out.drain(..keep);
    out
}

/// Plain text for "Copy diagnostic summary": the version and build, the book format, the
/// operating system, and the time, kind and step of the last five problems. Nothing else from
/// the log, no paths, no messages.
pub fn summary() -> String {
    let mut lines = vec!["FreedomBooks diagnostic summary".to_owned(), format!("Version: {}", crate::short_version())];
    lines.extend(crate::build::details());
    lines.push(format!("Operating system: {}", crate::build::os()));
    let recent = recent(5);
    if recent.is_empty() {
        lines.push("Recent problems: none recorded".into());
    } else {
        lines.push("Recent problems (oldest first):".into());
        for (t, kind, op) in recent {
            lines.push(format!("  {t}  {kind}  {op}"));
        }
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    let mut s = state();
    *s = State { dir: None, file: None, size: 0, held: vec![], recent: VecDeque::new() };
}

#[cfg(test)]
pub(crate) const TEST_MAX_BYTES: u64 = MAX_BYTES;
