#![cfg(all(unix, feature = "key-test-hooks"))]
//! From the second review of the keyring fixes (2026-11-11). "Use this as my books" killed (SIGKILL) between every pair of steps, and with every single file
//! operation failing in turn. After each: startup recovery settles, the saved choice of books agrees
//! with what is on disk, both books open with their correct secrets from wherever they are, the old
//! book keeps a transaction that was only in its write-ahead log, and the audit event exists exactly
//! when (and once) the change happened.
use fb_app::{App, Command, Outcome, books, keys::MasterKey, protection, screens};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::time::{Duration, Instant};

const OWNER: &str = "Riley Sample";
const BOOK_PASSWORD: &str = "Synthetic book password";
const USER_PASSWORD: &str = "Synthetic user password";
const MARKER: &str = "synthetic.wal_only_marker";

fn app_at(root: &Path) -> App {
    let mut app = App::new();
    app.backups.configure(root).unwrap();
    app.device_key = Some(MasterKey::new([77; 32]));
    app
}
fn main_book(root: &Path) -> PathBuf { root.join("books/default.fbbook") }
fn copy_book(root: &Path) -> PathBuf { root.join("books/restored-bakery-restored/restored-bakery-restored.fbbook") }

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap().flatten() {
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() { copy_tree(&e.path(), &target); } else { fs::copy(e.path(), &target).unwrap(); }
    }
}

/// Child processes: write a transaction that stays in the old book's WAL, or make the change.
#[test]
fn change_child() {
    let Ok(root) = std::env::var("FB_CHANGE_ROOT") else { return };
    let root = PathBuf::from(root);
    let mut app = app_at(&root);
    match std::env::var("FB_CHANGE_OPERATION").unwrap().as_str() {
        "wal" => {
            app.open_with_secret(&main_book(&root), BOOK_PASSWORD).unwrap();
            app.sign_in(OWNER, USER_PASSWORD).unwrap();
            let actor = app.authenticated_session().unwrap().actor;
            let company = app.first_company().unwrap().unwrap();
            app.open_mut().unwrap().book.write(|tx| {
                let user = fb_core::services::access(tx, actor, company)?;
                fb_core::services::audit(tx, &user, company, MARKER, "Book file".into(), serde_json::json!({}))
            }).unwrap();
            // Exit without closing: no checkpoint, so the marker exists only in default.fbbook-wal.
            std::process::exit(0);
        }
        "recover" => {
            let r = books::recover_book_change(&root);
            fs::write(root.join("recovered"), format!("{:?}", r.map_err(|e| e.to_string()))).unwrap();
        }
        _ => {
            app.open_with_secret(&copy_book(&root), BOOK_PASSWORD).unwrap();
            app.sign_in(OWNER, USER_PASSWORD).unwrap();
            app.mark_copy(true);
            let result = app.make_open_book_main(Some(&main_book(&root)));
            let text = match result { Ok(made) => format!("ok settled={}", made.settled), Err(e) => format!("err {e}") };
            fs::write(root.join("result"), text).unwrap();
        }
    }
}

fn child(root: &Path, operation: &str) -> Process {
    let mut command = Process::new(std::env::current_exe().unwrap());
    command.args(["--exact", "change_child", "--nocapture"])
        .env("FB_CHANGE_ROOT", root).env("FB_CHANGE_OPERATION", operation)
        .stdout(Stdio::null()).stderr(Stdio::null());
    command
}

/// One synthetic installation: a protected main book with receipts, a saved key and a WAL holding a
/// transaction, and a restored copy beside it. Built once, then copied for every case.
fn template() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let main = main_book(root);
    let mut app = app_at(root);
    app.create_protected_book(&main, OWNER, "Harbor Street Bakery", Some(BOOK_PASSWORD)).unwrap();
    app.enroll_owner(USER_PASSWORD).unwrap();
    let company = app.first_company().unwrap().unwrap();
    let path = format!("/c/{}/backups/create/", company.to_db());
    let token = app.tokens.mint(&path);
    let Outcome::Redirect { location } = app.run_renderer(Command::SubmitForm { path, token, fields: vec![] }).unwrap() else { panic!("backup") };
    let archive = app.backups.folder().unwrap().join(location.split("created=").nth(1).unwrap());
    app.lock();
    let copy = screens::backups::restore_from_gate(&mut app, &archive, "Bakery restored", BOOK_PASSWORD).unwrap();
    assert_eq!(copy, copy_book(root));
    app.lock();
    fs::create_dir_all(root.join("books/receipts")).unwrap();
    fs::write(root.join("books/receipts/synthetic-receipt.bin"), b"synthetic receipt bytes").unwrap();
    fs::write(protection::sidecar(&main, ".os-key"), b"synthetic sealed key").unwrap();
    assert!(child(root, "wal").status().unwrap().success());
    assert!(fs::metadata(protection::sidecar(&main, "-wal")).unwrap().len() > 0, "the marker waits in the WAL");
    d
}

fn case(template: &Path) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    copy_tree(template, d.path());
    d
}

/// The previous book's own files beside it (the book, its key record, saved key and SQLite sidecars;
/// never the lock sidecar), as they are in the template.
fn own(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(root.join("books")).unwrap().flatten()
        .filter(|e| e.file_type().unwrap().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("default.fbbook") && !n.ends_with(".lock")).collect();
    names.sort();
    names
}

fn aside_folders(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root.join("books/set-aside")).map(|r| r.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default()
}

fn marker_present(app: &mut App) -> bool {
    app.open.as_mut().unwrap().book.read(|q| q.query_row(
        "SELECT EXISTS(SELECT 1 FROM ledger_auditevent WHERE action=?1)", [MARKER], |r| r.get::<_, bool>(0))).unwrap()
}
fn made_active(app: &mut App) -> i64 {
    app.open.as_mut().unwrap().book.read(|q| q.query_row(
        "SELECT count(DISTINCT json_extract(detail,'$.change')) FROM ledger_auditevent WHERE action='book.restored_made_active'", [], |r| r.get(0))).unwrap()
}

#[derive(Debug, PartialEq)]
enum Where { Old, New }

/// Startup as the shell runs it, then the invariant: each book wholly in one place with all its
/// files, the saved choice naming the book that is in place, both books opening with their secret.
fn settle(root: &Path, own: &[String]) -> Where {
    let first = books::recover_book_change(root).unwrap();
    // Recovery is idempotent: a second start changes nothing.
    let again = books::recover_book_change(root).unwrap();
    assert!(again == books::Recovered::None || (again == books::Recovered::Finished && first == books::Recovered::Finished), "{first:?} then {again:?}");
    let main = main_book(root);
    let copy = copy_book(root);
    let choice = fs::read(books::choice_file(root)).ok().map(|b| serde_json::from_slice::<serde_json::Value>(&b).unwrap()["path"].as_str().unwrap().to_owned());
    let folders = aside_folders(root);
    let place = if main.exists() {
        for f in own { assert!(root.join("books").join(f).is_file(), "{f} is home with its book"); }
        assert!(root.join("books/receipts/synthetic-receipt.bin").is_file());
        for folder in &folders { assert!(!folder.join("default.fbbook").exists(), "no second copy of the book"); assert!(fs::read_dir(folder).unwrap().flatten().all(|e| e.file_name().to_string_lossy().ends_with(".lock")), "nothing of the book left in {}", folder.display()); }
        assert!(choice.is_none() || choice.as_deref() == Some(main.to_str().unwrap()), "the saved choice names the book in place: {choice:?}");
        assert!(!books::change_journal(&root.join("books")).exists());
        Where::Old
    } else {
        assert_eq!(folders.len(), 1, "{folders:?}");
        for f in own { assert!(folders[0].join(f).is_file(), "{f} moved with its book"); assert!(!root.join("books").join(f).exists()); }
        assert_eq!(fs::read(folders[0].join("receipts/synthetic-receipt.bin")).unwrap(), b"synthetic receipt bytes");
        assert!(!root.join("books/receipts").exists());
        assert_eq!(choice.as_deref(), Some(copy.to_str().unwrap()), "the saved choice names the new main books");
        Where::New
    };
    assert!(protection::sidecar(&main, ".lock").exists(), "the lock sidecar never moves");
    // The previous book opens with its password from wherever it is, with its WAL-only transaction.
    let old_at = if place == Where::Old { main.clone() } else { folders[0].join("default.fbbook") };
    let mut app = app_at(root);
    app.open_with_secret(&old_at, BOOK_PASSWORD).unwrap();
    assert!(marker_present(&mut app), "the write-ahead log stayed with its book");
    assert_eq!(made_active(&mut app), 0);
    app.lock();
    // The copy opens with its password; after an open the owed audit event is written, once.
    app.open_with_secret(&copy, BOOK_PASSWORD).unwrap();
    app.finish_book_change().unwrap();
    assert_eq!(made_active(&mut app), if place == Where::New { 1 } else { 0 }, "the audit event exists exactly when the change happened");
    app.finish_book_change().unwrap();
    assert_eq!(made_active(&mut app), if place == Where::New { 1 } else { 0 }, "never twice");
    assert!(!books::change_journal(&root.join("books")).exists(), "the journal is gone once settled");
    app.lock();
    place
}


fn stop_and_kill(mut command: Process, root: &Path, point: &str) {
    let marker = root.join(format!("stopped-{point}"));
    let mut process = command.env("FB_KEY_STOP_AT", point).env("FB_KEY_STOP_MARKER", &marker).spawn().unwrap();
    let start = Instant::now();
    while !marker.exists() {
        assert!(process.try_wait().unwrap().is_none(), "child finished before {point}");
        assert!(start.elapsed() < Duration::from_secs(300), "{point} not reached");
        std::thread::sleep(Duration::from_millis(5));
    }
    process.kill().unwrap();
    process.wait().unwrap();
}

/// R2-1: a move fails, then its in-process undo fails. The app keeps running (the shell does not
/// restart), and nothing stops the owner opening the half-moved previous book from the unlock screen.
#[test]
fn r2_double_fault_then_open_previous_without_restart() {
    let template = template();
    let own = own(template.path());
    eprintln!("own files in move order: {own:?}");
    // Move order: the book's sidecars sorted (the book itself excluded), receipts, the book last.
    let side: Vec<&String> = own.iter().filter(|n| n.as_str() != "default.fbbook").collect();
    let wal = side.iter().position(|n| n.ends_with("-wal")).unwrap();
    let keys = side.iter().position(|n| n.ends_with(".keys.json")).unwrap();
    assert!(wal < keys, "the WAL moves before the key record");
    for last in [wal, keys] {
        let d = case(template.path());
        let root = d.path();
        // Fail after the WAL or after the key record; fail undo of that same sidecar.
        let faults = format!("change-move-{},change-undo-{}", last + 1, last);
        let status = child(root, "change").env("FB_KEY_FAIL_AT", &faults).status().unwrap();
        assert!(status.success());
        let result = fs::read_to_string(root.join("result")).unwrap();
        eprintln!("result: {result}");
        assert!(books::change_journal(&root.join("books")).exists(), "prepared journal left for startup");
        assert!(!protection::sidecar(&main_book(root), "-wal").exists(), "the WAL is in set-aside");
        assert_eq!(protection::record_path(&main_book(root)).exists(), last < keys, "the second case also displaced the password record");
        // Same running app: the owner locks, picks Main books on the unlock screen, enters the password.
        let mut app = app_at(root);
        // Opening any book first finishes or undoes the unfinished change: the previous book is whole again,
        // with the transaction that was only in its write-ahead log.
        app.open_with_secret(&main_book(root), BOOK_PASSWORD).expect("the change is undone, then the book opens");
        assert!(!books::change_journal(&root.join("books")).exists(), "the unfinished change was settled before opening");
        assert!(marker_present(&mut app), "the WAL-only transaction is back");
        app.sign_in(OWNER, USER_PASSWORD).unwrap();
        let actor = app.authenticated_session().unwrap().actor;
        let company = app.first_company().unwrap().unwrap();
        for i in 0..200 {
            app.open_mut().unwrap().book.write(|tx| {
                let user = fb_core::services::access(tx, actor, company)?;
                fb_core::services::audit(tx, &user, company, "synthetic.during", "Book file".into(), serde_json::json!({"i": i, "pad": "x".repeat(500)}))
            }).unwrap();
        }
        app.lock();
        // The next start has nothing left to recover, and the book is whole with everything entered.
        assert!(matches!(books::recover_book_change(root).unwrap(), books::Recovered::None));
        let mut app = app_at(root);
        app.open_with_secret(&main_book(root), BOOK_PASSWORD).unwrap();
        let (ok, during, marker): (String, i64, bool) = app.open.as_mut().unwrap().book.read(|q| Ok::<_, fb_core::FbError>((
            q.query_row("PRAGMA integrity_check", [], |r| r.get(0))?,
            q.query_row("SELECT count(*) FROM ledger_auditevent WHERE action='synthetic.during'", [], |r| r.get(0))?,
            q.query_row("SELECT EXISTS(SELECT 1 FROM ledger_auditevent WHERE action=?1)", [MARKER], |r| r.get(0))?))).unwrap();
        assert_eq!((ok.as_str(), during, marker), ("ok", 200, true));
    }
}

/// The backstop behind the check at every open: if the previous book changes while its WAL is set
/// aside (something outside FreedomBooks wrote to it), recovery refuses to put the old WAL back
/// beside it or finish moving it. Nothing moves, and no book opens until the owner gets help.
#[test]
fn recovery_refuses_to_move_side_files_beside_a_changed_book() {
    use std::io::Write;
    let template = template();
    let own = own(template.path());
    let side: Vec<&String> = own.iter().filter(|n| n.as_str() != "default.fbbook").collect();
    let wal = side.iter().position(|n| n.ends_with("-wal")).unwrap();
    for (phase, change_size) in [("prepared", false), ("prepared", true), ("committed", false), ("committed", true)] {
        let d = case(template.path());
        let root = d.path();
        let faults = format!("change-move-{},change-undo-{}", wal + 1, wal);
        assert!(child(root, "change").env("FB_KEY_FAIL_AT", &faults).status().unwrap().success());
        let journal = books::change_journal(&root.join("books"));
        let mut saved: serde_json::Value = serde_json::from_str(&fs::read_to_string(&journal).unwrap()).unwrap();
        assert!(saved["book_stamp"]["len"].as_u64().is_some(), "the journal records the book as it was: {saved}");
        // Also exercise redo of a committed move, before anything has been moved back.
        saved["phase"] = serde_json::json!(phase);
        fs::write(&journal, saved.to_string()).unwrap();
        // Change either the size alone (preserving mtime) or mtime alone.
        let mut book = fs::File::options().append(true).open(main_book(root)).unwrap();
        if change_size {
            let modified = book.metadata().unwrap().modified().unwrap();
            book.write_all(b"synthetic external write").unwrap();
            book.set_modified(modified).unwrap();
        } else {
            let later = std::time::SystemTime::now() + Duration::from_secs(3600);
            book.set_modified(later).unwrap();
        }
        drop(book);
        let before: Vec<PathBuf> = aside_folders(root).iter().flat_map(|f| fs::read_dir(f).unwrap().flatten().map(|e| e.path())).collect();
        let refused = books::recover_book_change(root).unwrap_err().to_string();
        assert!(refused.contains("changed after the change started") && refused.contains("Report a problem"), "{refused}");
        let after: Vec<PathBuf> = aside_folders(root).iter().flat_map(|f| fs::read_dir(f).unwrap().flatten().map(|e| e.path())).collect();
        assert_eq!(before, after, "nothing was moved back");
        assert!(!protection::sidecar(&main_book(root), "-wal").exists(), "the old WAL stays set aside");
        assert!(journal.exists(), "the journal stays");
        let mut app = app_at(root);
        assert!(app.open_with_secret(&main_book(root), BOOK_PASSWORD).is_err(), "no book opens on a guess");
        assert!(app.open_with_secret(&copy_book(root), BOOK_PASSWORD).is_err());
    }
}

/// An older journal without the book's stamp is still settled as before.
#[test]
fn a_journal_without_a_book_stamp_is_still_undone() {
    let template = template();
    let own = own(template.path());
    let d = case(template.path());
    let root = d.path();
    stop_and_kill(child(root, "change"), root, &format!("change-move-{}", own.len()));
    let journal = books::change_journal(&root.join("books"));
    let mut saved: serde_json::Value = serde_json::from_str(&fs::read_to_string(&journal).unwrap()).unwrap();
    assert!(saved.as_object_mut().unwrap().remove("book_stamp").is_some());
    fs::write(&journal, saved.to_string()).unwrap();
    assert_eq!(settle(root, &own), Where::Old);
}

/// R2-2: killed mid-move, then the startup recovery is itself killed mid-undo; next start settles.
#[test]
fn r2_kill_during_recovery_undo() {
    let template = template();
    let own = own(template.path());
    for (stop, undo) in [(own.len(), own.len() - 1), (2, 1), (own.len(), 0)] {
        let d = case(template.path());
        stop_and_kill(child(d.path(), "change"), d.path(), &format!("change-move-{stop}"));
        stop_and_kill(child(d.path(), "recover"), d.path(), &format!("change-undo-{undo}"));
        let place = settle(d.path(), &own);
        assert_eq!(place, Where::Old, "move {stop} undo {undo}");
    }
}

/// R2-3: committed, then killed; startup recovery killed while writing the choice; next start settles.
#[test]
fn r2_kill_during_recovery_after_commit() {
    let template = template();
    let own = own(template.path());
    let d = case(template.path());
    stop_and_kill(child(d.path(), "change"), d.path(), "change-committed");
    stop_and_kill(child(d.path(), "recover"), d.path(), "change-choice-write");
    assert_eq!(settle(d.path(), &own), Where::New);
}

/// R2-4: commit written but its sync "failed" and the rollback note also fails: the owner is told
/// to restart; the journal on disk says committed although the error says not moved?
#[test]
fn r2_commit_unsynced_and_rollback_note_fails() {
    let template = template();
    let own = own(template.path());
    let d = case(template.path());
    let status = child(d.path(), "change").env("FB_KEY_FAIL_AT", "change-journal-commit-unsynced,change-journal-rollback").status().unwrap();
    assert!(status.success());
    // The owner is told to restart; the journal says committed, and the next start finishes the change.
    assert!(fs::read_to_string(d.path().join("result")).unwrap().contains("Quit and start FreedomBooks again"));
    let j = fs::read_to_string(books::change_journal(&d.path().join("books"))).unwrap();
    assert_eq!(serde_json::from_str::<serde_json::Value>(&j).unwrap()["phase"], "committed");
    assert_eq!(settle(d.path(), &own), Where::New);
}
