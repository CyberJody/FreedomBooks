#![cfg(all(unix, feature = "key-test-hooks"))]
//! "Use this as my books" killed (SIGKILL) between every pair of steps, and with every single file
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

/// Every stop point in order, with the index from which the commit is durable.
fn points(own: &[String]) -> (Vec<String>, usize) {
    let mut out: Vec<String> = ["change-journal-prepared", "change-journal-saved", "change-folder-create", "change-folder-sync"].map(String::from).to_vec();
    // The book's own files other than itself, its receipts folder, then the book.
    for i in 0..own.len() + 1 { out.push(format!("change-move-{i}")); }
    out.extend(["change-sync-target", "change-sync-source", "change-journal-commit"].map(String::from));
    let committed = out.len();
    out.extend(["change-committed", "change-choice-write", "change-choice-written", "change-audit", "change-audited"].map(String::from));
    (out, committed)
}

#[test]
fn a_kill_between_any_two_steps_settles_into_one_place_or_the_other() {
    let template = template();
    let own = own(template.path());
    assert!(own.iter().any(|n| n.ends_with("-wal")) && own.iter().any(|n| n.ends_with(".keys.json")) && own.iter().any(|n| n.ends_with(".os-key")), "{own:?}");
    let (points, committed_from) = points(&own);
    for (n, point) in points.iter().enumerate() {
        let d = case(template.path());
        let marker = d.path().join("stopped-at");
        let mut process = child(d.path(), "change").env("FB_KEY_STOP_AT", point).env("FB_KEY_STOP_MARKER", &marker).spawn().unwrap();
        let start = Instant::now();
        while !marker.exists() {
            assert!(process.try_wait().unwrap().is_none(), "child finished before {point}");
            assert!(start.elapsed() < Duration::from_secs(300), "{point} not reached");
            std::thread::sleep(Duration::from_millis(5));
        }
        process.kill().unwrap();
        process.wait().unwrap();
        let place = settle(d.path(), &own);
        let expected = if n >= committed_from { Where::New } else { Where::Old };
        assert_eq!(place, expected, "killed at {point}");
    }
}

#[test]
fn any_single_failing_file_operation_leaves_both_books_whole() {
    let template = template();
    let own = own(template.path());
    let (points, _) = points(&own);
    let mut faults: Vec<&str> = points.iter().map(String::as_str).filter(|p| !["change-committed", "change-choice-written", "change-audited"].contains(p)).collect();
    faults.extend(["change-journal-commit-unsynced", "change-journal-remove"]);
    for fault in faults {
        let d = case(template.path());
        let status = child(d.path(), "change").env("FB_KEY_FAIL_AT", fault).status().unwrap();
        assert!(status.success(), "{fault}");
        let result = fs::read_to_string(d.path().join("result")).unwrap();
        let committed = ["change-choice-write", "change-audit", "change-journal-remove"].contains(&fault);
        if committed {
            assert!(result.starts_with("ok"), "{fault}: {result}");
            assert_eq!(result.contains("settled=false"), fault == "change-choice-write", "{fault}: {result}");
        } else {
            // The message is true: nothing is out of place after an undone failure.
            assert!(result.starts_with("err") && result.contains("Nothing was moved and these books are still a copy"), "{fault}: {result}");
            assert!(main_book(d.path()).is_file() && !books::change_journal(&d.path().join("books")).exists(), "{fault}: undone in place");
        }
        let place = settle(d.path(), &own);
        assert_eq!(place, if committed { Where::New } else { Where::Old }, "{fault}");
    }
}
