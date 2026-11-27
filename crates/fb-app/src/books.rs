//! Book files on this computer: what the unlock screen shows about them, where a restored copy came
//! from, and changing which books are the main ones (setting the previous ones aside). Nothing here
//! deletes a book or key file.
use crate::protection::{error, present_path, private_dir, sidecar, sync_dir, write_private};
use crate::{App, AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

/// Where a restored book came from, written beside it when it is restored (`<book>.restored.json`).
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Restored {
    pub name: String,
    /// The backup file's name (not its folder, which may name a person or a drive).
    pub backup: String,
    /// When it was restored (UTC, RFC 3339).
    pub restored_at: String,
}

pub fn restored_marker(book: &Path) -> PathBuf {
    sidecar(book, ".restored.json")
}

/// Best effort: a restore that worked is never undone because its note could not be written.
pub fn note_restored(book: &Path, name: &str, backup: &Path) {
    let note = Restored {
        name: name.to_owned(),
        backup: backup.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        restored_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    };
    if let Ok(bytes) = serde_json::to_vec(&note) {
        let _ = write_private(&restored_marker(book), &bytes);
    }
}

pub fn read_restored(book: &Path) -> Option<Restored> {
    let path = restored_marker(book);
    let meta = fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > 4096 {
        return None;
    }
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_value(fb_db::parse_unique_json(&raw).ok()?).ok()
}

/// The name a copy is shown under: the name chosen when it was restored, else a plain description
/// from where the file is (the main books file, a set-aside book, or its folder's name).
pub fn copy_name(book: &Path) -> String {
    if let Some(r) = read_restored(book) {
        return r.name;
    }
    let parent = book.parent();
    let folder = parent.and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let set_aside = parent.and_then(Path::parent).and_then(Path::file_name).is_some_and(|n| n == "set-aside");
    let main_file = book.file_name().is_some_and(|f| f == "default.fbbook");
    match (set_aside, main_file) {
        (true, true) => "Previous main books (set aside)".into(),
        (true, false) => format!("Set aside: {}", folder.trim_start_matches(|c: char| c.is_ascii_alphanumeric() && c != '-').trim_start_matches('-')),
        (false, true) if folder == "books" => "Main books file".into(),
        _ => Some(folder.trim_start_matches("restored-").to_owned()).filter(|n| !n.is_empty()).unwrap_or_else(|| "Restored books".into()),
    }
}

/// The "ask for the book password when opening" setting a restored copy inherits: that of the book
/// on this computer with the same key record (the book the backup was made from: the open book,
/// the saved main book or the main books file), else asking. Built-in-key books never ask.
pub fn inherited_ask(root: &Path, envelope: &fb_core::backups::portable::KeyEnvelope, open: Option<&crate::protection::Record>) -> bool {
    if envelope.mode == fb_core::backups::portable::Mode::BuiltIn {
        return false;
    }
    let wanted = serde_json::to_value(envelope).ok();
    let same = |r: &crate::protection::Record| wanted.is_some() && serde_json::to_value(&r.keys).ok() == wanted;
    if let Some(r) = open.filter(|r| same(r)) {
        return r.ask_password;
    }
    let books = root.join("books");
    let mut candidates = vec![];
    if let Ok(Choice::Names(p)) = read_choice(root) && is_book(&books, &p) { candidates.push(p); }
    candidates.push(books.join("default.fbbook"));
    candidates.iter()
        .find_map(|p| crate::protection::Record::read(p).ok().flatten().filter(|r| same(r)))
        .is_none_or(|r| r.ask_password)
}

/// Whether a restore note sits beside this book (it was restored from a backup).
pub fn is_restored(book: &Path) -> bool {
    read_restored(book).is_some()
}

// ---------------------------------------------------------------------------------------------
// Changing which books are the main ones ("Use this as my books").
//
// The previous main book moves, with every file that belongs to it, into
// `<books>/set-aside/<time>-<name>/`, and `<root>/active-book.json` is pointed at the new main book.
// Several renames cannot be one atomic step, so the whole change is driven by a journal,
// `<books>/set-aside/changing-books.json`, written and synced before anything moves:
//
//   prepared      moves may have started. A failure or a restart puts every moved file back
//                 (nothing names the new book yet), and the journal goes.
//   rolling-back  the same, written when a failure might have left a committed journal behind.
//   committed     every move was durable before this was written. A restart finishes the moves,
//                 points the saved choice at the new book, and keeps the journal until the audit
//                 event is in the new book (written then by the actor recorded here).
//
// So at any instant the old book, its key record, saved key, write-ahead files and receipts are
// either all in the old place or are put back there, or are all in the new place, and the saved
// choice is changed only after the move is committed, by the same step that finishes it. The move
// happens BEFORE the choice is switched: until it is committed nothing names the new book, so a
// stop at any point undoes cleanly; after it, nothing is left to undo. The lock sidecar
// (`<book>.lock`) never moves and is never removed. Renames stay inside the books folder, on one
// file system (checked). Nothing here deletes a book or key file; at most an empty folder this
// change created is removed when the change is undone.
//
// The old book's `-wal` is moved with it rather than checkpointed: opening the old book would need
// its key (not always at hand) and the normal open migrates the file, which must not happen to a
// book being retired. Moved together under the journal, the write-ahead log stays with its book.
// ---------------------------------------------------------------------------------------------

/// The journal of an unfinished change of main books.
pub fn change_journal(books: &Path) -> PathBuf {
    books.join("set-aside").join("changing-books.json")
}

/// The shell's saved choice of which books open at start (`{"path": ...}`), written by the shell
/// for an ordinary open and by the change of main books here. Same format in both places.
pub fn choice_file(root: &Path) -> PathBuf {
    root.join("active-book.json")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Phase {
    Prepared,
    RollingBack,
    Committed,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Move {
    from: PathBuf,
    to: PathBuf,
}

/// The previous book file's size and modified time when the change started.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stamp {
    len: u64,
    modified_ns: u64,
}

fn stamp_of(book: &Path) -> AppResult<Stamp> {
    let meta = fs::symlink_metadata(book).map_err(error)?;
    let modified = meta.modified().map_err(error)?.duration_since(std::time::UNIX_EPOCH).map_err(error)?;
    Ok(Stamp { len: meta.len(), modified_ns: u64::try_from(modified.as_nanos()).map_err(error)? })
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    format: u32,
    id: String,
    phase: Phase,
    /// The book that becomes the main one.
    main: PathBuf,
    /// The previous main book and where it goes; both absent when nothing is set aside.
    previous: Option<PathBuf>,
    set_aside: Option<PathBuf>,
    /// A folder this change creates for the previous book (removed again, if empty, on undo).
    folder: Option<PathBuf>,
    moves: Vec<Move>,
    /// Who made the change, for the audit event if a restart has to write it.
    actor: String,
    detail: serde_json::Value,
    /// The previous book as it was when the change started (absent in journals written before this
    /// was recorded). Its side files (WAL, key record) are only moved back or on beside a book that
    /// still matches: beside a book changed since, an old WAL would damage it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    book_stamp: Option<Stamp>,
}

/// What the shell is told after the owner made the open copy the main books.
#[derive(Debug, Serialize)]
pub struct MadeMain {
    pub main: PathBuf,
    /// Where the previous main book now is, when one was set aside.
    pub set_aside: Option<PathBuf>,
    /// The saved choice on disk now names `main`. When false the change is committed and the next
    /// start finishes saving the choice.
    pub settled: bool,
}

/// What startup recovery did with an interrupted change of main books.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Recovered {
    /// Nothing was interrupted.
    None,
    /// The change had not been committed: everything was put back where it was.
    Undone,
    /// The change was committed: it was finished and the saved choice names the new book.
    Finished,
}

/// A test-only stop or failure between two steps. Release builds compile this to nothing.
fn step(name: &str) -> AppResult<()> {
    crate::protection::checkpoint(name);
    if crate::protection::fault(name) {
        return Err(error(format!("Injected failure at {name}.")));
    }
    Ok(())
}

fn in_books(books: &Path, file: &Path) -> bool {
    file.is_absolute()
        && file.starts_with(books)
        && file != books
        && file.components().all(|c| matches!(c, std::path::Component::RootDir | std::path::Component::Normal(_)))
}

fn is_book(books: &Path, file: &Path) -> bool {
    in_books(books, file) && file.extension().is_some_and(|e| e == "fbbook")
}

enum Choice {
    Absent,
    Names(PathBuf),
    Unreadable,
}

fn read_choice(root: &Path) -> AppResult<Choice> {
    let file = choice_file(root);
    let meta = match fs::symlink_metadata(&file) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Choice::Absent),
        Err(e) => return Err(error(e)),
        Ok(m) => m,
    };
    if !meta.is_file() || meta.len() > 65536 {
        return Ok(Choice::Unreadable);
    }
    let raw = fs::read_to_string(&file).map_err(error)?;
    Ok(match fb_db::parse_unique_json(&raw).ok().and_then(|v: serde_json::Value| v["path"].as_str().map(PathBuf::from)) {
        Some(path) => Choice::Names(path),
        None => Choice::Unreadable,
    })
}

pub(crate) fn is_main_book(root: &Path, book: &Path) -> AppResult<bool> {
    Ok(match read_choice(root)? {
        Choice::Absent => book == root.join("books/default.fbbook"),
        Choice::Names(path) => path == book,
        Choice::Unreadable => false,
    })
}

/// Point the saved choice at `main`. A damaged choice file is kept under a new name first.
fn write_choice(root: &Path, main: &Path) -> AppResult<()> {
    let file = choice_file(root);
    if let Choice::Unreadable = read_choice(root)? {
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
        let mut kept = false;
        for n in 0..100 {
            let aside = sidecar(&file, &format!(".unreadable-{stamp}{}", if n == 0 { String::new() } else { format!("-{n}") }));
            match fs::hard_link(&file, &aside) {
                Ok(()) => { kept = true; break; }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(error(e)),
            }
        }
        if !kept { return Err(error("The damaged note of which books to open could not be kept.")); }
    }
    write_private(&file, &serde_json::to_vec(&json!({"path": main})).map_err(error)?)
}

fn present(path: &Path) -> AppResult<bool> {
    present_path(path)
}

/// The previous main book's moves, with the guard that keeps it closed everywhere meanwhile.
struct Plan {
    moves: Vec<Move>,
    folder: Option<PathBuf>,
    set_aside: PathBuf,
    _guard: fb_db::MaintenanceGuard,
}

fn plan_set_aside(books: &Path, previous: &Path, open: &Path) -> AppResult<Plan> {
    let refuse = |m: &str| Err(AppError::Invalid(format!("{m} Nothing was moved.")));
    if !is_book(books, previous) {
        return refuse("Only a book in this computer's books folder can be set aside.");
    }
    if previous == open {
        return refuse("The open book can't be set aside.");
    }
    if !fs::symlink_metadata(previous).map_err(error)?.is_file() {
        return refuse("The previous book is not a regular file.");
    }
    let guard = fb_db::MaintenanceGuard::acquire(previous)
        .map_err(|_| AppError::Invalid("The previous book is open in another window. Nothing was moved.".into()))?;
    if present(&sidecar(previous, ".rekey.json"))? || present(&sidecar(previous, ".rekey-before"))? {
        return refuse("The previous book has an unfinished password change. Open it once to finish that change, then try again.");
    }
    let parent = previous.parent().ok_or_else(|| error("Missing book folder."))?;
    let file_name = previous.file_name().and_then(|n| n.to_str()).ok_or_else(|| error("Invalid book name."))?;
    let in_root = parent == books;
    if in_root && !present(&crate::protection::record_path(previous))?
        && (present(&books.with_file_name("book-key.lock.json"))? || present(&books.with_file_name("book-key.sealed"))?)
    {
        return refuse("The previous book still uses this computer's older key files, so it stays where it is.");
    }
    if !in_root && (parent.parent() != Some(books) || open.starts_with(parent)) {
        return refuse("The previous book is not in a folder of its own.");
    }
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let label = if in_root { file_name.trim_end_matches(".fbbook").to_owned() } else {
        parent.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "book".into())
    };
    let aside_root = books.join("set-aside");
    private_dir(&aside_root)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // Renames never cross file systems: a set-aside folder mounted elsewhere is refused up front.
        let dev = |p: &Path| fs::metadata(p).map(|m| m.dev()).map_err(error);
        if dev(books)? != dev(&aside_root)? || dev(parent)? != dev(&aside_root)? {
            return refuse("The set-aside folder is on a different drive from your books.");
        }
    }
    let mut target = aside_root.join(format!("{stamp}-{label}"));
    for n in 2..100 {
        if !present(&target)? { break; }
        target = aside_root.join(format!("{stamp}-{label}-{n}"));
    }
    if present(&target)? {
        return refuse("There is no free name to set the previous book aside.");
    }
    if !in_root {
        // A restored book has its own folder: the whole folder moves in one rename.
        return Ok(Plan { moves: vec![Move { from: parent.to_owned(), to: target.clone() }], folder: None,
            set_aside: target.join(file_name), _guard: guard });
    }
    // The main book shares the books folder: its own files, its receipts folder, the book last.
    let mut moves = vec![];
    let mut names = vec![];
    for entry in fs::read_dir(parent).map_err(error)? {
        let entry = entry.map_err(error)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name == file_name || !name.starts_with(file_name) || name == format!("{file_name}.lock") {
            continue;
        }
        // Anything that is not a plain file (a link, a folder) is not guessed at.
        if !entry.file_type().map_err(error)?.is_file() {
            return refuse("The previous book has a file beside it that is not a regular file.");
        }
        names.push(name.to_owned());
    }
    names.sort();
    for name in names {
        moves.push(Move { from: parent.join(&name), to: target.join(&name) });
    }
    let receipts = parent.join("receipts");
    match fs::symlink_metadata(&receipts) {
        Ok(m) if m.is_dir() => moves.push(Move { from: receipts, to: target.join("receipts") }),
        Ok(_) => return refuse("The previous book's receipts folder is not a regular folder."),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(error(e)),
    }
    moves.push(Move { from: previous.to_owned(), to: target.join(file_name) });
    Ok(Plan { moves, folder: Some(target.clone()), set_aside: target.join(file_name), _guard: guard })
}

fn save_change(journal: &Path, change: &Change) -> AppResult<()> {
    write_private(journal, &serde_json::to_vec(change).map_err(error)?)
}

fn read_change(journal: &Path) -> AppResult<Change> {
    let damaged = || error("The note of an interrupted change of your main books is damaged, so FreedomBooks can't tell where your books should be. Nothing was changed or deleted.");
    let meta = fs::symlink_metadata(journal).map_err(error)?;
    if !meta.is_file() || meta.len() > 65536 {
        return Err(damaged());
    }
    let raw = fs::read_to_string(journal).map_err(|_| damaged())?;
    let change: Change = serde_json::from_value(fb_db::parse_unique_json(&raw).map_err(|_| damaged())?).map_err(|_| damaged())?;
    if change.format != 1 { return Err(damaged()); }
    Ok(change)
}

fn parents(paths: impl Iterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = vec![];
    for p in paths {
        if let Some(parent) = p.parent().map(Path::to_owned) && !out.contains(&parent) {
            out.push(parent);
        }
    }
    out
}

/// Sync each folder that exists. A folder this change had not created yet (a stop or failure before
/// it) has nothing to make durable.
fn sync_all(dirs: &[PathBuf]) -> AppResult<()> {
    for d in dirs {
        if present(d)? { sync_dir(d)?; }
    }
    Ok(())
}

fn remove_journal(journal: &Path) -> AppResult<()> {
    step("change-journal-remove")?;
    fs::remove_file(journal).map_err(error)?;
    sync_dir(journal.parent().ok_or_else(|| error("Missing set-aside folder."))?)
}

/// Before any file is moved back or on: the previous book must be the file it was when the change
/// started. If it changed (something wrote to it meanwhile), nothing is moved and nothing opens:
/// putting its old WAL beside it would damage it.
fn book_unchanged(change: &Change, pending: bool) -> AppResult<()> {
    let (true, Some(stamp), Some(from), Some(to)) = (pending, change.book_stamp, &change.previous, &change.set_aside) else { return Ok(()) };
    let at = if present(from)? { from } else if present(to)? { to } else { return Ok(()) };
    if stamp_of(at)? != stamp {
        return Err(error("The previous books file changed after the change started, so FreedomBooks did not put its other files back beside it."));
    }
    Ok(())
}

/// Put every moved file back. Only files this change moved (absent at home, present at the new
/// place) are touched; anything else is refused rather than guessed at.
fn undo_moves(change: &Change) -> AppResult<()> {
    let mut pending = false;
    for m in &change.moves { pending |= !present(&m.from)? && present(&m.to)?; }
    book_unchanged(change, pending)?;
    for (i, m) in change.moves.iter().enumerate().rev() {
        match (present(&m.from)?, present(&m.to)?) {
            (true, false) => {}
            (false, true) => {
                step(&format!("change-undo-{i}"))?;
                fs::rename(&m.to, &m.from).map_err(error)?;
            }
            (true, true) => return Err(error("A file of the previous books is in both places, so it was left as it is.")),
            (false, false) => return Err(error("A file of the previous books is missing from both places.")),
        }
    }
    step("change-undo-sync")?;
    sync_all(&parents(change.moves.iter().flat_map(|m| [m.from.clone(), m.to.clone()])))?;
    if let Some(folder) = &change.folder {
        // Only an empty folder goes (a lock sidecar made there keeps it, harmlessly).
        if fs::remove_dir(folder).is_ok() {
            sync_dir(folder.parent().ok_or_else(|| error("Missing set-aside folder."))?)?;
        }
    }
    Ok(())
}

fn finish_moves(change: &Change) -> AppResult<()> {
    // Only when something still has to move: a finished change's set-aside book may since have
    // been opened from the chooser, which is fine.
    let mut pending = false;
    for m in &change.moves { pending |= present(&m.from)? && !present(&m.to)?; }
    book_unchanged(change, pending)?;
    for (i, m) in change.moves.iter().enumerate() {
        match (present(&m.from)?, present(&m.to)?) {
            (false, true) => {}
            (true, false) => {
                step(&format!("change-redo-{i}"))?;
                fs::rename(&m.from, &m.to).map_err(error)?;
            }
            (true, true) => return Err(error("A file of the previous books is in both places, so it was left as it is.")),
            (false, false) => return Err(error("A file of the previous books is missing from both places.")),
        }
    }
    sync_all(&parents(change.moves.iter().flat_map(|m| [m.from.clone(), m.to.clone()])))
}

fn valid_change(books: &Path, change: &Change) -> bool {
    let aside = books.join("set-aside");
    is_book(books, &change.main)
        && change.previous.as_deref().is_none_or(|p| is_book(books, p) && !p.starts_with(&aside))
        && change.set_aside.as_deref().is_none_or(|p| is_book(books, p) && p.starts_with(&aside))
        && change.folder.as_deref().is_none_or(|p| in_books(books, p) && p.parent() == Some(&*aside))
        && change.moves.iter().all(|m| in_books(books, &m.from) && !m.from.starts_with(&aside) && in_books(books, &m.to) && m.to.starts_with(&aside))
        && change.previous.is_some() == change.set_aside.is_some()
}

/// Before any book opens: settle an unfinished change of main books. A committed change whose files are
/// all in their new places is finished on disk and only waits for its audit event (finish_book_change),
/// so it is left alone; anything else is finished or undone first, and an error means nothing opens.
pub fn recover_before_open(root: &Path) -> AppResult<()> {
    let books = root.join("books");
    let journal = change_journal(&books);
    if !present(&journal)? {
        return Ok(());
    }
    let change = read_change(&journal)?;
    if change.phase == Phase::Committed {
        let mut left_behind = false;
        for m in &change.moves {
            left_behind |= present(&m.from)?;
        }
        if !left_behind {
            return Ok(());
        }
    }
    recover_book_change(root).map(|_| ())
}

/// Startup, before the shell reads which books to open: finish a committed change of main books or
/// undo one that was not committed. An error means nothing could be decided; the caller must not
/// open any book on a guess.
pub fn recover_book_change(root: &Path) -> AppResult<Recovered> {
    let books = root.join("books");
    let journal = change_journal(&books);
    if !present(&journal)? {
        return Ok(Recovered::None);
    }
    let change = read_change(&journal)?;
    if !valid_change(&books, &change) {
        return Err(error("The note of an interrupted change of your main books names files outside your books folder. Nothing was changed or deleted."));
    }
    // The previous book stays closed everywhere while its files move (wherever it is right now).
    let _guard = match (&change.previous, &change.set_aside) {
        (Some(from), Some(to)) => {
            let at = if present(from)? { from } else if present(to)? { to } else {
                return Err(error("The books named in an interrupted change of your main books are missing. Nothing was changed or deleted."));
            };
            Some(fb_db::MaintenanceGuard::acquire(at).map_err(|_| error("The previous books are open in another window. Close it, then start FreedomBooks again."))?)
        }
        _ => None,
    };
    let explain = |e: AppError| error(format!("FreedomBooks could not finish an interrupted change of your main books: {e} Nothing was deleted. Quit and start FreedomBooks again; if this keeps happening use “Report a problem”."));
    match change.phase {
        Phase::Prepared | Phase::RollingBack => {
            undo_moves(&change).map_err(explain)?;
            remove_journal(&journal).map_err(explain)?;
            Ok(Recovered::Undone)
        }
        Phase::Committed => {
            finish_moves(&change).map_err(explain)?;
            if !matches!(read_choice(root)?, Choice::Names(ref p) if *p == change.main) {
                step("change-choice-write").map_err(explain)?;
                write_choice(root, &change.main).map_err(explain)?;
            }
            // The journal stays until the audit event is in the new main book (finish_book_change).
            Ok(Recovered::Finished)
        }
    }
}

/// Write `book.restored_made_active` in every company of the open book the actor can access,
/// once per change (a restart never writes it twice).
fn audit_made_main(book: &mut fb_db::Book, actor: fb_core::UserId, id: &str, detail: &serde_json::Value) -> AppResult<bool> {
    let wrote = book.write(|tx| {
        let already: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM ledger_auditevent WHERE action='book.restored_made_active' AND json_extract(detail,'$.change')=?1)",
            [id], |r| r.get(0))?;
        if already { return Ok::<_, fb_core::FbError>(false); }
        let companies: Vec<String> = tx.prepare("SELECT id FROM ledger_company ORDER BY id")?
            .query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        let mut written = 0;
        for c in companies {
            let c = fb_core::CompanyId::parse(&c).ok_or_else(|| fb_core::FbError::validation("Invalid company."))?;
            let Ok(user) = fb_core::services::access(tx, actor, c) else { continue };
            fb_core::services::audit(tx, &user, c, "book.restored_made_active", "Book file".into(), detail.clone())?;
            written += 1;
        }
        if written == 0 { return Err(fb_core::FbError::denied("You need access to a company in these books.")); }
        Ok(true)
    })?;
    Ok(wrote)
}

fn relative(books: &Path, path: &Path) -> String {
    path.strip_prefix(books).unwrap_or(path).display().to_string()
}

/// One book the unlock screen can offer, as the shell lists it.
#[derive(Deserialize)]
pub struct Listed {
    pub path: String,
    pub kind: String,
    pub modified: f64,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub current: bool,
}

/// What the unlock screen says about the books on this computer.
#[derive(Deserialize, Default)]
pub struct GateContext {
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub book: String,
    #[serde(default)]
    pub books: Vec<Listed>,
    #[serde(default)]
    pub home: String,
}

pub(crate) fn shown_path(path: &str, home: &str) -> String {
    let home = home.trim_end_matches('/');
    match path.strip_prefix(home) {
        Some(rest) if !home.is_empty() && rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_owned(),
    }
}

pub(crate) fn shown_time(ms: f64) -> String {
    use chrono::TimeZone;
    if !ms.is_finite() || ms <= 0.0 { return String::new(); }
    chrono::Local.timestamp_millis_opt(ms as i64).single().map(|t| t.format("%-d %b %Y, %-I:%M %p").to_string()).unwrap_or_default()
}

impl GateContext {
    pub(crate) fn view(&self) -> serde_json::Value {
        let current = self.books.iter().find(|b| b.path == self.book);
        let books: Vec<_> = self.books.iter().map(|b| {
            let path = Path::new(&b.path);
            let named = read_restored(path).map(|r| r.name);
            let label = match (b.active, b.kind.as_str()) {
                (true, _) => match named { Some(n) => format!("Your books (restored as “{n}”)"), None => "Your books".to_owned() },
                // The main books file while the saved choice names another book (after "Use this as
                // my books" with no saved choice, or a book that had to stay where it is).
                (false, "main") if self.books.iter().any(|o| o.active) => "Previous main books file".to_owned(),
                (false, "main") => "Main books".to_owned(),
                (false, "set-aside") => match named {
                    Some(n) => format!("Set aside: “{n}”"),
                    None if path.file_name().is_some_and(|f| f == "default.fbbook") => "Set aside: previous main books".to_owned(),
                    None => format!("Set aside: {}", copy_name(path)),
                },
                _ => format!("Restored copy: “{}”", copy_name(path)),
            };
            json!({"path": b.path, "shown": shown_path(&b.path, &self.home), "label": label, "changed": shown_time(b.modified),
                "active": b.active, "current": b.current, "kind": b.kind})
        }).collect();
        json!({
            "reason": self.reason,
            "book": current.map(|b| json!({"shown": shown_path(&b.path, &self.home), "changed": shown_time(b.modified),
                "copy": b.kind != "main" && !b.active, "name": copy_name(Path::new(&b.path))})),
            "books": books,
            "other_books": self.books.len() > 1,
        })
    }
}


impl App {
    /// The open book is (`copy`) or is not a copy that someone is only looking at.
    pub fn mark_copy(&mut self, copy: bool) {
        let name = self.book_path().filter(|_| copy).map(copy_name);
        self.backups.set_viewing(name);
    }

    /// The restored-book form's check before the shell is asked to make the change: the signed-in
    /// owner, looking at a copy. Nothing is written.
    pub fn check_use_restored(&mut self) -> AppResult<()> {
        self.authorize(None, crate::identity::Permission::Administer)?;
        if !self.open_mut()?.staff { return Err(AppError::Invalid("Only the owner can choose which books are used.".into())); }
        if self.backups.viewing_restored().is_none() {
            return Err(AppError::Invalid("These are already your books.".into()));
        }
        Ok(())
    }

    /// "Use this as my books": the signed-in owner makes the open copy the book that opens from now
    /// on. `previous` is the main book the shell knows of (None when it has none, or its saved
    /// choice is missing or damaged); it is set aside, and the saved choice on disk must agree with
    /// it. The audit event is written only once the change is committed.
    pub fn make_open_book_main(&mut self, previous: Option<&Path>) -> AppResult<MadeMain> {
        let session = self.authorize(None, crate::identity::Permission::Administer)?;
        self.check_use_restored()?;
        let unchanged = |m: &str| AppError::Invalid(format!("{m} Nothing was moved and these books are still a copy."));
        let root = self.backups.root()?.to_owned();
        let books = root.join("books");
        let main = self.book_path().ok_or(AppError::Locked)?.to_owned();
        if !is_book(&books, &main) {
            return Err(unchanged("Only books in this computer's books folder can become your books."));
        }
        let journal = change_journal(&books);
        if present(&journal)? {
            return Err(unchanged("An earlier change of your main books hasn't finished. Quit and start FreedomBooks again to finish it."));
        }
        // The saved choice on disk must be the one the shell acted on.
        let default = books.join("default.fbbook");
        let agrees = match (read_choice(&root)?, previous) {
            (Choice::Absent, Some(p)) => p == default,
            (Choice::Names(n), Some(p)) => n == p,
            (Choice::Absent, None) => !present(&default)?,
            (Choice::Names(n), None) => !present(&n)?,
            (Choice::Unreadable, None) => true,
            (Choice::Unreadable, Some(_)) => false,
        };
        if !agrees {
            return Err(unchanged("The note of which books open at start changed meanwhile. Lock and try again."));
        }
        let previous = previous.filter(|p| *p != main.as_path() && !p.starts_with(books.join("set-aside")));
        let previous = match previous {
            Some(p) if present(p)? => Some(p.to_owned()),
            _ => None,
        };
        let note = read_restored(&main);
        let id = uuid::Uuid::new_v4().to_string();
        // Companies the owner can record this in are checked before anything changes.
        let actor = session.actor;
        let can_audit = self.open.as_mut().ok_or(AppError::Locked)?.book.read(|q| {
            let ids: Vec<String> = q.prepare("SELECT id FROM ledger_company ORDER BY id")?.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
            Ok::<_, fb_core::FbError>(ids.iter().filter_map(|c| fb_core::CompanyId::parse(c)).any(|c| fb_core::services::access(q, actor, c).is_ok()))
        })?;
        if !can_audit { return Err(unchanged("You need access to a company in these books.")); }
        // A restored copy becomes the books in use with hosting off, whatever the backup said: an
        // Administrator turns it on again knowingly on this computer (its keys came with the backup).
        self.hosting_off_for_restored().map_err(|e| unchanged(&format!("Hosting could not be switched off in this copy ({e}).")))?;
        let plan = previous.as_deref().map(|p| plan_set_aside(&books, p, &main)).transpose()?;
        let mut change = Change {
            format: 1,
            id: id.clone(),
            phase: Phase::Prepared,
            main: main.clone(),
            previous: previous.clone(),
            set_aside: plan.as_ref().map(|p| p.set_aside.clone()),
            folder: plan.as_ref().and_then(|p| p.folder.clone()),
            moves: plan.as_ref().map(|p| p.moves.clone()).unwrap_or_default(),
            actor: actor.to_db(),
            // Taken while the guard keeps the previous book closed everywhere.
            book_stamp: previous.as_deref().filter(|_| plan.is_some()).map(stamp_of).transpose().map_err(|e| unchanged(&e.to_string()))?,
            detail: json!({
                "book": copy_name(&main),
                "backup": note.as_ref().map(|n| n.backup.clone()),
                "restored_at": note.as_ref().map(|n| n.restored_at.clone()),
                "made_active": true,
                "previous": previous.as_deref().map(|p| relative(&books, p)),
                "set_aside": plan.as_ref().map(|p| relative(&books, &p.set_aside)),
                "change": id,
            }),
        };
        step("change-journal-prepared").map_err(|e| unchanged(&e.to_string()))?;
        private_dir(&books.join("set-aside")).map_err(|e| unchanged(&e.to_string()))?;
        if let Err(e) = save_change(&journal, &change) {
            // A prepared journal that did reach the disk names no moves yet; it is undone at start.
            let _ = fs::remove_file(&journal);
            return Err(unchanged(&format!("FreedomBooks could not prepare the change ({e}).")));
        }
        step("change-journal-saved").map_err(|e| self.undo_change(&journal, &change, false, e))?;
        // The moves, then their durability, then the commit. Any failure here is undone.
        let mut maybe_committed = false;
        let forward = (|| {
            if let Some(folder) = &change.folder {
                step("change-folder-create")?;
                private_dir(folder)?;
                step("change-folder-sync")?;
                sync_dir(&books.join("set-aside"))?;
            }
            for (i, m) in change.moves.iter().enumerate() {
                step(&format!("change-move-{i}"))?;
                fs::rename(&m.from, &m.to).map_err(error)?;
            }
            step("change-sync-target")?;
            sync_all(&parents(change.moves.iter().map(|m| m.to.clone())))?;
            step("change-sync-source")?;
            sync_all(&parents(change.moves.iter().map(|m| m.from.clone())))?;
            step("change-journal-commit")?;
            change.phase = Phase::Committed;
            maybe_committed = true;
            save_change(&journal, &change)?;
            // A commit whose rename happened but whose directory sync failed: on disk it may or may
            // not be committed. Tested as its own failure.
            if crate::protection::fault("change-journal-commit-unsynced") {
                return Err(error("Injected failure after the commit was written."));
            }
            Ok(())
        })();
        if let Err(e) = forward {
            change.phase = Phase::Prepared;
            return Err(self.undo_change(&journal, &change, maybe_committed, e));
        }
        step("change-committed").ok();
        // Committed: only forward from here; a restart finishes whatever is left.
        let settled = step("change-choice-write").and_then(|()| write_choice(&root, &main)).is_ok();
        step("change-choice-written").ok();
        let audited = step("change-audit").and_then(|()| {
            let open = self.open.as_mut().ok_or(AppError::Locked)?;
            audit_made_main(&mut open.book, actor, &id, &change.detail)
        });
        step("change-audited").ok();
        if audited.is_ok() && settled {
            let _ = crate::scheduled_backups::reset_for_main(&root, &main, (self.now)());
            let _ = remove_journal(&journal);
        }
        self.backups.set_viewing(None);
        self.set_aside_note = change.set_aside.as_ref().map(|p| p.display().to_string());
        self.book_change_pending = (!settled).then(|| "FreedomBooks could not yet save that these books open at start. The change itself is done and nothing is lost: FreedomBooks finishes saving it the next time it starts.".to_owned());
        Ok(MadeMain { main, set_aside: change.set_aside, settled })
    }

    /// Undo a change that was not committed. Returns the error to show, which says truthfully
    /// whether anything is still out of place.
    fn undo_change(&mut self, journal: &Path, change: &Change, maybe_committed: bool, cause: AppError) -> AppError {
        let partly = |e: AppError| AppError::Invalid(format!(
            "FreedomBooks could not set your previous books aside ({cause}), and could not yet put back everything it had moved ({e}). Nothing was deleted. Quit and start FreedomBooks again: it puts your previous books back before opening anything. These books are still a copy."));
        if maybe_committed {
            // The journal on disk may say committed: mark it rolling back first, or leave it to startup.
            let mut back = clone_change(change);
            back.phase = Phase::RollingBack;
            if let Err(e) = step("change-journal-rollback").and_then(|()| save_change(journal, &back)) {
                return AppError::Invalid(format!(
                    "FreedomBooks could not finish setting your previous books aside ({cause}), and could not record that it is undoing it ({e}). Nothing was deleted. Quit and start FreedomBooks again before doing anything else: it finishes or undoes the change before opening any books."));
            }
        }
        if let Err(e) = undo_moves(change) { return partly(e); }
        // Everything is back in place; a journal that could not be removed only undoes nothing at
        // the next start.
        let _ = remove_journal(journal);
        AppError::Invalid(format!("FreedomBooks could not set your previous books aside ({cause}). Nothing was moved and these books are still a copy."))
    }

    /// After any open: an interrupted change whose audit event a restart still owes is recorded in
    /// the new main book, by the actor who made the change, and the journal goes.
    pub fn finish_book_change(&mut self) -> AppResult<bool> {
        let Ok(root) = self.backups.root().map(Path::to_owned) else { return Ok(false) };
        let books = root.join("books");
        let journal = change_journal(&books);
        if !present(&journal)? { return Ok(false); }
        let change = read_change(&journal)?;
        let Some(open) = self.book_path().map(Path::to_owned) else { return Ok(false) };
        if change.phase != Phase::Committed || change.main != open || !valid_change(&books, &change) {
            return Ok(false);
        }
        // The saved choice first: the journal is the only record of the change until it agrees.
        if !matches!(read_choice(&root)?, Choice::Names(ref p) if *p == change.main) {
            write_choice(&root, &change.main)?;
        }
        let actor = fb_core::UserId::parse(&change.actor).ok_or_else(|| error("The interrupted change names an unknown user."))?;
        let mut detail = change.detail.clone();
        detail["recorded_later"] = json!(true);
        let book = &mut self.open.as_mut().ok_or(AppError::Locked)?.book;
        audit_made_main(book, actor, &change.id, &detail)?;
        let _ = crate::scheduled_backups::reset_for_main(&root, &change.main, (self.now)());
        remove_journal(&journal)?;
        Ok(true)
    }
}

fn clone_change(c: &Change) -> Change {
    Change {
        format: c.format,
        id: c.id.clone(),
        phase: c.phase,
        main: c.main.clone(),
        previous: c.previous.clone(),
        set_aside: c.set_aside.clone(),
        folder: c.folder.clone(),
        moves: c.moves.clone(),
        actor: c.actor.clone(),
        detail: c.detail.clone(),
        book_stamp: c.book_stamp,
    }
}
