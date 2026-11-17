//! A book restored from the unlock screen beside the main one is a labelled copy until the owner
//! confirms "Use this as my books"; then the previous main book is set aside, never deleted, and the
//! saved choice of books names the copy. Refusals and failures change nothing and audit nothing.
use fb_app::{App, Command, Outcome, books, keys::MasterKey, protection, screens};
use std::fs;
use std::path::{Path, PathBuf};

const OWNER: &str = "Riley Sample";
const BOOK_PASSWORD: &str = "Synthetic book password";
const USER_PASSWORD: &str = "Synthetic user password";

fn app_at(root: &Path) -> App {
    let mut app = App::new();
    app.backups.configure(root).unwrap();
    app.device_key = Some(MasterKey::new([77; 32]));
    app
}

fn backup(app: &mut App) -> PathBuf {
    let company = app.first_company().unwrap().unwrap();
    let path = format!("/c/{}/backups/create/", company.to_db());
    let token = app.tokens.mint(&path);
    let Outcome::Redirect { location } = app.run_renderer(Command::SubmitForm { path, token, fields: vec![] }).unwrap() else { panic!("backup") };
    app.backups.folder().unwrap().join(location.split("created=").nth(1).unwrap())
}

fn submit(app: &mut App, path: &str, fields: &[(&str, &str)]) -> Outcome {
    let token = app.tokens.mint(path);
    app.run_renderer(Command::SubmitForm { path: path.into(), token, fields: fields.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }).unwrap()
}

fn events(app: &mut App, action: &str) -> Vec<serde_json::Value> {
    let rows: Vec<String> = app.open_mut().unwrap().book.read(|q| q.prepare("SELECT detail FROM ledger_auditevent WHERE action=?1")?
        .query_map([action], |r| r.get(0))?.collect::<Result<_, _>>()).unwrap();
    rows.iter().map(|d| serde_json::from_str(d).unwrap()).collect()
}

/// The main book, a backup of it, and a restored copy opened from the unlock screen.
fn restored(root: &Path) -> (App, PathBuf, PathBuf, PathBuf) {
    let main = root.join("books/default.fbbook");
    let mut app = app_at(root);
    app.create_protected_book(&main, OWNER, "Harbor Street Bakery", Some(BOOK_PASSWORD)).unwrap();
    app.enroll_owner(USER_PASSWORD).unwrap();
    let archive = backup(&mut app);
    fs::create_dir_all(root.join("books/receipts")).unwrap();
    fs::write(root.join("books/receipts/synthetic-receipt.bin"), b"synthetic receipt bytes").unwrap();
    fs::write(root.join("books/default.fbbook.os-key"), b"synthetic sealed key").unwrap();
    app.lock();
    let copy = screens::backups::restore_from_gate(&mut app, &archive, "Bakery restored", BOOK_PASSWORD).unwrap();
    app.sign_in(OWNER, USER_PASSWORD).unwrap();
    (app, main, copy, archive)
}

/// Everything under the books folder except lock sidecars, which never move and are never removed.
fn tree(books: &Path) -> Vec<String> {
    let mut out = vec![];
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        for e in fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() { walk(base, &p, out); }
            else if !p.to_string_lossy().ends_with(".lock") { out.push(p.strip_prefix(base).unwrap().display().to_string()); }
        }
    }
    walk(books, books, &mut out);
    out.sort();
    out
}

#[test]
fn a_restored_copy_is_labelled_and_records_where_it_came_from() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, _main, copy, archive) = restored(d.path());
    let note = books::read_restored(&copy).expect("restore note beside the copy");
    assert_eq!(note.name, "Bakery restored");
    assert_eq!(note.backup, archive.file_name().unwrap().to_string_lossy());
    assert!(!note.backup.contains('/'), "only the file name, never its folder");
    assert_eq!(app.backups.viewing_restored(), Some("Bakery restored"));
    let c = app.first_company().unwrap().unwrap();
    let home = screens::render(&mut app, &format!("/c/{}/home/", c.to_db())).unwrap().html;
    assert!(home.contains("This is a restored copy, “Bakery restored”, not your main books."), "{home}");
    assert!(home.contains("File › Use this copy as my books"));
    let page = screens::render(&mut app, "/settings/restored-book/").unwrap().html;
    assert!(page.contains("You’re working in a restored copy") && page.contains("Use this as my books"), "{page}");
    assert!(page.contains(&*note.backup));
    assert!(page.contains("last changed") && page.contains("name=\"confirm\""), "the step shows when the book last changed and needs confirming: {page}");
    // The shell says this open book is the main one after all: the label goes.
    app.mark_copy(false);
    assert_eq!(app.backups.viewing_restored(), None);
    app.mark_copy(true);
    assert_eq!(app.backups.viewing_restored(), Some("Bakery restored"));
}

#[test]
fn use_this_as_my_books_sets_the_previous_book_aside_then_audits_in_the_copy() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, main, copy, archive) = restored(d.path());
    // Without the confirmation box nothing happens.
    let refused = submit(&mut app, "/settings/restored-book/", &[("action", "use")]);
    assert!(matches!(refused, Outcome::Invalid { ref html, .. } if html.contains("Tick the box")), "{refused:?}");
    let outcome = submit(&mut app, "/settings/restored-book/", &[("action", "use"), ("confirm", "on")]);
    assert!(matches!(outcome, Outcome::Done { ref data } if data["desktop_keys"] == "use-restored"), "{outcome:?}");
    assert!(events(&mut app, "book.restored_made_active").is_empty(), "the form only checks; the change is audited when it happens");

    let lock = protection::sidecar(&main, ".lock");
    let lock_existed = lock.exists();
    let made = app.make_open_book_main(Some(&main)).unwrap();
    assert!(made.settled);
    assert_eq!(made.main, copy);
    let moved = made.set_aside.unwrap();
    assert!(!main.exists());
    assert!(moved.starts_with(d.path().join("books/set-aside")) && moved.is_file(), "{}", moved.display());
    let folder = moved.parent().unwrap();
    assert!(folder.join("default.fbbook.keys.json").is_file(), "its key record moves with it");
    assert!(folder.join("default.fbbook.os-key").is_file());
    assert_eq!(fs::read(folder.join("receipts/synthetic-receipt.bin")).unwrap(), b"synthetic receipt bytes");
    assert_eq!(lock.exists(), lock_existed, "the lock sidecar is never moved or removed");
    assert!(!books::change_journal(&d.path().join("books")).exists(), "a finished change leaves no journal");
    let choice: serde_json::Value = serde_json::from_slice(&fs::read(books::choice_file(d.path())).unwrap()).unwrap();
    assert_eq!(choice["path"], copy.display().to_string(), "the copy is now the book that opens at start");
    let detail = events(&mut app, "book.restored_made_active");
    assert_eq!(detail.len(), 1);
    assert_eq!(detail[0]["backup"], archive.file_name().unwrap().to_string_lossy().as_ref());
    assert_eq!(detail[0]["book"], "Bakery restored");
    assert_eq!(detail[0]["made_active"], true);
    assert_eq!(detail[0]["previous"], "default.fbbook");
    assert!(detail[0]["set_aside"].as_str().unwrap().starts_with("set-aside/"), "{detail:?}");
    assert_eq!(app.backups.viewing_restored(), None);
    let page = screens::render(&mut app, "/settings/restored-book/").unwrap().html;
    assert!(page.contains("These are now your books") && page.contains("set aside, not deleted"), "{page}");
    // Finishing again records nothing more.
    assert!(!app.finish_book_change().unwrap());
    assert_eq!(events(&mut app, "book.restored_made_active").len(), 1);
    // The set-aside book still opens with its own password, from its new place.
    app.lock();
    app.open_with_secret(&moved, BOOK_PASSWORD).unwrap();
    assert_eq!(books::recover_book_change(d.path()).unwrap(), books::Recovered::None);
}

#[test]
fn a_restored_book_in_its_own_folder_moves_as_one_folder() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, main, first, archive) = restored(d.path());
    app.make_open_book_main(Some(&main)).unwrap();
    // A second restore becomes the books; the first one (now main, in its own folder) moves whole.
    app.lock();
    let second = screens::backups::restore_from_gate(&mut app, &archive, "Second copy", BOOK_PASSWORD).unwrap();
    app.sign_in(OWNER, USER_PASSWORD).unwrap();
    let made = app.make_open_book_main(Some(&first)).unwrap();
    let moved = made.set_aside.unwrap();
    assert!(!first.parent().unwrap().exists() && moved.is_file() && moved.parent().unwrap().join("receipts").is_dir());
    assert!(protection::record_path(&moved).is_file());
    assert_eq!(made.main, second);
    app.lock();
    app.open_with_secret(&moved, BOOK_PASSWORD).unwrap();
}

#[test]
fn only_a_copy_can_be_made_the_books_and_only_by_the_signed_in_owner() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, main, copy, _) = restored(d.path());
    app.mark_copy(false);
    let refused = submit(&mut app, "/settings/restored-book/", &[("action", "use"), ("confirm", "on")]);
    assert!(matches!(refused, Outcome::Invalid { ref html, .. } if html.contains("These are already your books.")), "{refused:?}");
    assert!(app.make_open_book_main(Some(&main)).is_err());
    app.mark_copy(true);
    app.sign_out();
    assert!(app.make_open_book_main(Some(&main)).is_err());
    assert!(main.is_file() && copy.is_file());
    assert!(!books::choice_file(d.path()).exists());
}

#[test]
fn refused_changes_move_nothing_and_audit_nothing() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, main, copy, _) = restored(d.path());
    let books_dir = d.path().join("books");
    let before = tree(&books_dir);
    type Arrange = Box<dyn Fn()>;
    let refusals: Vec<(&str, Arrange)> = vec![
        // The saved choice on disk names some other book than the shell's previous one.
        ("changed meanwhile", Box::new({ let root = d.path().to_owned(); move || fs::write(books::choice_file(&root), br#"{"path":"/elsewhere.fbbook"}"#).unwrap() })),
        ("unfinished password change", Box::new({ let m = main.clone(); move || fs::write(protection::sidecar(&m, ".rekey.json"), b"{}").unwrap() })),
        ("hasn't finished", Box::new({ let b = books_dir.clone(); move || { fs::create_dir_all(b.join("set-aside")).unwrap(); fs::write(books::change_journal(&b), b"{}").unwrap(); } })),
    ];
    for (expected, arrange) in refusals {
        arrange();
        let error = app.make_open_book_main(Some(&main)).unwrap_err().to_string();
        assert!(error.contains(expected) && error.contains("Nothing was moved"), "{expected}: {error}");
        let _ = fs::remove_file(books::choice_file(d.path()));
        let _ = fs::remove_file(protection::sidecar(&main, ".rekey.json"));
        let _ = fs::remove_file(books::change_journal(&books_dir));
        assert_eq!(tree(&books_dir), before, "{expected}: nothing moved");
        assert!(events(&mut app, "book.restored_made_active").is_empty(), "{expected}: no audit event for a change that did not happen");
    }
    // Never the open book itself, and never a file outside the books folder.
    let outside = d.path().join("outside.fbbook");
    fs::write(&outside, b"synthetic").unwrap();
    fs::write(books::choice_file(d.path()), serde_json::to_vec(&serde_json::json!({"path": outside})).unwrap()).unwrap();
    let error = app.make_open_book_main(Some(&outside)).unwrap_err().to_string();
    assert!(error.contains("Nothing was moved"), "{error}");
    assert!(outside.is_file() && copy.is_file());
    assert!(events(&mut app, "book.restored_made_active").is_empty());
}

#[test]
fn with_no_main_book_the_copy_becomes_the_books_and_nothing_is_set_aside() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, main, copy, _) = restored(d.path());
    // The saved choice is damaged: the shell has no main book to offer as previous.
    fs::write(books::choice_file(d.path()), b"{not json").unwrap();
    let made = app.make_open_book_main(None).unwrap();
    assert!(made.set_aside.is_none() && made.settled);
    assert!(main.is_file(), "a book the shell did not name is never moved");
    let kept: Vec<_> = fs::read_dir(d.path()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("active-book.json.unreadable-")).collect();
    assert_eq!(kept.len(), 1, "the damaged note is kept, not overwritten");
    let choice: serde_json::Value = serde_json::from_slice(&fs::read(books::choice_file(d.path())).unwrap()).unwrap();
    assert_eq!(choice["path"], copy.display().to_string());
    let detail = events(&mut app, "book.restored_made_active");
    assert_eq!(detail.len(), 1);
    assert!(detail[0]["previous"].is_null() && detail[0]["set_aside"].is_null());
}

#[test]
fn a_restored_copy_inherits_whether_its_source_asks_for_the_book_password() {
    for ask in [true, false] {
        let d = tempfile::tempdir().unwrap();
        let main = d.path().join("books/default.fbbook");
        let mut app = app_at(d.path());
        app.create_protected_book(&main, OWNER, "Harbor Street Bakery", Some(BOOK_PASSWORD)).unwrap();
        app.enroll_owner(USER_PASSWORD).unwrap();
        app.os_store_usable = true;
        app.change_ask_password(BOOK_PASSWORD, ask).unwrap();
        let archive = backup(&mut app);
        // From the unlock screen: the main book on this computer has the same key record.
        app.lock();
        let copy = screens::backups::restore_from_gate(&mut app, &archive, "Gate copy", BOOK_PASSWORD).unwrap();
        assert_eq!(protection::Record::read(&copy).unwrap().unwrap().ask_password, ask, "gate restore, source ask={ask}");
        // From the Backups page of the open main book.
        app.lock();
        app.open_with_secret(&main, BOOK_PASSWORD).unwrap();
        app.sign_in(OWNER, USER_PASSWORD).unwrap();
        let company = app.first_company().unwrap().unwrap();
        let page = format!("/c/{}/backups/", company.to_db());
        let Outcome::Done { data } = submit(&mut app, &page, &[("action", "restore"), ("new_book", "Page copy")]) else { panic!("restore dialog") };
        assert_eq!(data["desktop"], "pick-backup");
        screens::backups::dialog_result(&mut app, data["ticket"].as_str().unwrap(), archive.to_str().unwrap()).unwrap();
        let page_copy = d.path().join("books/restored-page-copy/restored-page-copy.fbbook");
        assert_eq!(protection::Record::read(&page_copy).unwrap().unwrap().ask_password, ask, "page restore, source ask={ask}");
    }
    // A backup whose key record matches no book here (another computer's books): the copy asks.
    let other = tempfile::tempdir().unwrap();
    let mut source = app_at(other.path());
    source.create_protected_book(&other.path().join("books/default.fbbook"), OWNER, "Elsewhere", Some(BOOK_PASSWORD)).unwrap();
    source.enroll_owner(USER_PASSWORD).unwrap();
    source.os_store_usable = true;
    source.change_ask_password(BOOK_PASSWORD, false).unwrap();
    let archive = backup(&mut source);
    let here = tempfile::tempdir().unwrap();
    let mut app = app_at(here.path());
    let copy = screens::backups::restore_from_gate(&mut app, &archive, "From elsewhere", BOOK_PASSWORD).unwrap();
    assert!(protection::Record::read(&copy).unwrap().unwrap().ask_password, "unknown source: the copy asks");
}

/// A second process that holds the previous main book open.
#[test]
fn holder_child() {
    let Some(book) = std::env::var_os("FB_HOLD_BOOK") else { return };
    let root = PathBuf::from(std::env::var_os("FB_HOLD_ROOT").unwrap());
    let mut app = app_at(&root);
    app.open_with_secret(Path::new(&book), BOOK_PASSWORD).unwrap();
    fs::write(root.join("holding"), b"open").unwrap();
    while !root.join("release").exists() { std::thread::sleep(std::time::Duration::from_millis(20)); }
}

#[test]
fn a_book_open_in_another_process_is_not_set_aside() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, main, copy, _) = restored(d.path());
    let before = tree(&d.path().join("books"));
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "holder_child", "--nocapture"])
        .env("FB_HOLD_BOOK", &main).env("FB_HOLD_ROOT", d.path())
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .spawn().unwrap();
    let start = std::time::Instant::now();
    while !d.path().join("holding").exists() {
        assert!(child.try_wait().unwrap().is_none(), "the holder exited early");
        assert!(start.elapsed().as_secs() < 300, "the holder never opened the book");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let error = app.make_open_book_main(Some(&main)).unwrap_err().to_string();
    assert!(error.contains("open in another window") && error.contains("Nothing was moved"), "{error}");
    // And the other process's book can't be opened here either.
    let mut other = app_at(d.path());
    assert!(other.open_with_secret(&main, BOOK_PASSWORD).is_err(), "a second opener is refused");
    fs::write(d.path().join("release"), b"go").unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(tree(&d.path().join("books")), before);
    assert!(events(&mut app, "book.restored_made_active").is_empty());
    assert!(main.is_file() && copy.is_file());
    // Once it is closed there, the change goes ahead.
    app.make_open_book_main(Some(&main)).unwrap();
}

/// Review S4: a backup made while hosting was on restores as a copy that never hosts, and becomes
/// the books in use with hosting off; an Administrator turns it on again knowingly. The host's keys
/// come with the backup (owner decision), only the on/off choice is reset.
#[test]
fn a_restored_copy_becomes_the_books_in_use_with_hosting_off() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let main = root.join("books/default.fbbook");
    let mut app = app_at(root);
    app.create_protected_book(&main, OWNER, "Harbor Street Bakery", Some(BOOK_PASSWORD)).unwrap();
    app.enroll_owner(USER_PASSWORD).unwrap();
    let mut record = fb_app::hosting::HostRecord::fresh().unwrap();
    record.hosted_before = true;
    record.enabled = true;
    record.bind = "192.168.1.20:47100".into();
    record.identity = Some(fb_app::hosting::Identity { cert: "aa".into(), key: "bb".into() });
    app.save_host_record(&record).unwrap();
    let archive = backup(&mut app);
    app.lock();
    screens::backups::restore_from_gate(&mut app, &archive, "Bakery restored", BOOK_PASSWORD).unwrap();
    app.sign_in(OWNER, USER_PASSWORD).unwrap();
    assert!(app.host_record().unwrap().unwrap().enabled, "the copy as restored still says on (it is a copy, so it is never hosted)");
    app.make_open_book_main(Some(&main)).unwrap();
    let now = app.host_record().unwrap().unwrap();
    assert!(!now.enabled, "the books in use come back with hosting off");
    assert!(now.host_id == record.host_id && now.bind == record.bind && now.identity == record.identity, "everything else travels with the backup");
}
