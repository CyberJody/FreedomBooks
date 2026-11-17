//! CRASHLOG proof, through the real bridge entry points on a synthetic built-in-key book: a panic in
//! a read and a panic in the middle of a write (after the statements ran, before COMMIT) each come
//! back as an `FB_PANIC` error instead of aborting, leave the book consistent and the app usable,
//! and write a log entry with the expected fields and none of the forbidden content. So do panics
//! in a protection change, a migration at open, a backup and a restore, each followed by reopening
//! the books. Panics are injected by the test-only `panic-test` feature (fb-app/fb-db `test_panic`).
use super::*;
use serde_json::{Value, json};
use std::path::PathBuf;

const OWNER: &str = "Synthetic Panic Owner";
const COMPANY: &str = "Synthetic Panic Co";
const SECOND: &str = "Synthetic Second Co";
const PASSWORD: &str = "Synthetic owner sign-in password 7";
const BOOK_PASSWORD: &str = "Synthetic book password 9";

/// Book words, amounts, a memo, secrets of every shape, a URL with credentials, a home-folder
/// path and SQL with values (the same set as fb-diag's own tests).
const BOOK_TEXT: &[&str] = &[
    "Synthetic Carol Hardware",
    "Alicé",
    "1234.56",
    "98765.43",
    "rent for March unit 4B",
    "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6",
    "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP",
    "hunter2-synthetic",
    "https://synthuser:synthpass@bank.example/feed",
    "/home/synthetic-owner/Books/main.fbbook",
    "INSERT INTO sales_receipt VALUES ('Synthetic Dave', 777.77)",
];
/// What must never appear in the log in any form: fragments of [`BOOK_TEXT`], the injected panics'
/// own text and the fixture's names and password.
const FORBIDDEN: &[&str] = &[
    "Carol", "Alic", "1234.56", "1234", "98765", "rent for", "unit 4B", "a1b2c3d4", "a#b#", "JBSWY", "PXPJ", "hunter", "synthuser", "synthpass", "bank.example",
    "synthetic-owner", "Books/main", "INSERT", "sales_receipt VALUES", "Dave", "777.77", "Synthetic", "stored row shape", "injected test panic",
];

struct Fixture {
    dir: tempfile::TempDir,
    logs: PathBuf,
    book: PathBuf,
    company: String,
    _turn: std::sync::MutexGuard<'static, ()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fb_app::test_panic::disarm();
        let _ = lock();
    }
}

fn fixture() -> Fixture {
    let turn = crate::tests::turn();
    fb_app::test_panic::disarm();
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    configure_log(logs.display().to_string()).unwrap();
    lock().unwrap();
    configure_backups(dir.path().display().to_string()).unwrap();
    let book = dir.path().join("books").join("default.fbbook");
    create_protected_book(book.display().to_string(), OWNER.into(), COMPANY.into(), String::new(), false, Some("standard".into())).unwrap();
    sign_in();
    let company = companies()[0]["id"].as_str().unwrap().to_owned();
    Fixture { dir, logs, book, company, _turn: turn }
}

fn sign_in() {
    let mut app = app().unwrap();
    if app.bootstrap_owner().unwrap().is_some() {
        app.enroll_owner(PASSWORD).unwrap();
    } else {
        app.sign_in(OWNER, PASSWORD).unwrap();
    }
    assert!(app.authenticated_session().is_ok());
}

fn companies() -> Vec<Value> {
    serde_json::from_str::<Value>(&query(r#"{"type":"Companies"}"#.into()).unwrap()).unwrap().as_array().unwrap().clone()
}

fn page(target: &str) -> String {
    let rendered: Value = serde_json::from_str(&render(target.into()).unwrap()).unwrap();
    assert_eq!(rendered["status"], 200, "{target}");
    rendered["html"].as_str().unwrap().to_owned()
}

fn token(html: &str) -> String {
    let marker = "name=\"fb_command_token\" value=\"";
    html.split_once(marker).unwrap().1.split('"').next().unwrap().to_owned()
}

/// A form token for `path`, as rendering its page would mint (the lease is released at once).
fn mint(path: &str) -> String {
    app().unwrap().tokens.mint(path)
}

fn submit(path: &str, token: &str, fields: Value) -> Result<Value> {
    run(json!({"type": "SubmitForm", "path": path, "token": token, "fields": fields}).to_string()).map(|s| serde_json::from_str(&s).unwrap())
}

fn panic_detail(error: &Error) -> Value {
    let detail = error.reason.strip_prefix("FB_PANIC ").unwrap_or_else(|| panic!("not a panic error: {}", error.reason));
    serde_json::from_str(detail).unwrap()
}

fn state() -> Value {
    serde_json::from_str(&app_state().unwrap()).unwrap()
}

fn log_text(f: &Fixture) -> String {
    let mut text = String::new();
    for entry in std::fs::read_dir(&f.logs).unwrap() {
        text.push_str(&std::fs::read_to_string(entry.unwrap().path()).unwrap());
    }
    text
}

fn panics(f: &Fixture) -> Vec<Value> {
    log_text(f).lines().map(|l| serde_json::from_str::<Value>(l).unwrap()).filter(|v| v["kind"] == "panic").collect()
}

/// None of the forbidden text, the book path, the temporary folder or the home folder may appear
/// anywhere in the log.
fn assert_clean(f: &Fixture, also: &[&str]) {
    let text = log_text(f);
    let book = f.book.display().to_string();
    let temp = f.dir.path().display().to_string();
    let home = std::env::var("HOME").unwrap_or_else(|_| "/nonexistent-home".into());
    for forbidden in FORBIDDEN.iter().chain([OWNER, COMPANY, SECOND, PASSWORD, BOOK_PASSWORD, book.as_str(), temp.as_str(), home.as_str()].iter()).chain(also) {
        assert!(!text.contains(forbidden), "the log holds {forbidden:?}:\n{text}");
    }
    for line in text.lines() {
        let v: Value = serde_json::from_str(line).unwrap();
        assert!(v.get("message").is_none(), "no entry has a message: {line}");
    }
}

fn assert_entry_fields(entry: &Value) {
    for field in ["t", "build", "schema", "source", "op", "route", "kind", "detail", "location", "backtrace", "outcome", "book"] {
        assert!(entry.get(field).is_some(), "{field} missing: {entry}");
    }
    assert_eq!(entry["schema"], fb_diag::build().migrations);
    assert!(entry["backtrace"].as_array().is_some_and(|b| !b.is_empty()), "{entry}");
}

fn integrity_ok() {
    let app = app().unwrap();
    let book = &app.open.as_ref().expect("the books are open").book;
    book.idle_after_unwind().unwrap();
    assert!(book.check().unwrap().is_empty(), "integrity check");
}

/// Reopen the way startup does: finish or roll back an interrupted protection change, then open
/// (through the bridge's boundary, as `open_protected_book` does) and sign in.
fn reopen(f: &Fixture, secret: &str) -> Result<()> {
    recover_key_change(f.book.display().to_string())?;
    guarded("open_protected_book", Class::Write, |app| app.open_with_secret(&f.book, secret).map_err(app_error))?;
    sign_in();
    Ok(())
}

#[test]
fn the_stamped_build_identity_is_the_one_registered() {
    let _turn = crate::tests::turn();
    boundary::init();
    let info: Value = serde_json::from_str(&build_info().unwrap()).unwrap();
    assert_eq!(info["commit"], env!("FB_BUILD_COMMIT"), "{info}");
    assert_ne!(info["commit"], "unknown", "this test tree has git metadata: {info}");
    assert_eq!(info["date"], env!("FB_BUILD_DATE"));
    assert!(info["features"].as_array().unwrap().iter().any(|f| f == "panic-test"), "{info}");
    assert_eq!(info["migrations"], fb_diag::build::known_migrations());
    assert!(info["version"].as_str().unwrap().contains(&env!("FB_BUILD_COMMIT")[..10]), "{info}");
}

#[test]
fn a_panic_in_a_read_is_an_error_the_books_stay_open_and_other_screens_work() {
    let f = fixture();
    let home = format!("/c/{}/home/", f.company);
    let accounts = format!("/c/{}/accounts/", f.company);
    page(&home);
    fb_app::test_panic::arm("render", "company_home");
    let error = render(home.clone()).unwrap_err();
    assert_eq!(panic_detail(&error), json!({"closed": false, "was_open": true, "saved": "none", "op": "render"}));
    // Every time that screen is opened, and the rest of the app meanwhile.
    assert!(render(home.clone()).is_err());
    let s = state();
    assert_eq!((s["open"].as_bool(), s["signed_in"].as_bool()), (Some(true), Some(true)), "{s}");
    assert!(page(&accounts).contains("<h1"));
    // A panic inside the book's read transaction: rolled back, query_only reset, still usable.
    fb_app::test_panic::disarm();
    fb_app::test_panic::arm("read", "accounts");
    let error = render(accounts.clone()).unwrap_err();
    assert_eq!(panic_detail(&error)["closed"], false, "the read connection was idle again: {}", error.reason);
    fb_app::test_panic::disarm();
    page(&accounts);
    page(&home);
    assert!(state()["open"].as_bool().unwrap());
    integrity_ok();
    let found = panics(&f);
    assert_eq!(found.len(), 3, "{found:?}");
    let entry = &found[0];
    assert_entry_fields(entry);
    assert_eq!((entry["source"].as_str(), entry["op"].as_str(), entry["route"].as_str()), (Some("rust"), Some("render"), Some("company_home")));
    assert_eq!(entry["detail"], "panic with a message", "a fixed category, not the expect text: {entry}");
    assert!(entry["location"].as_str().unwrap().starts_with("crates/fb-app/src/test_panic.rs:"), "{entry}");
    assert_eq!(entry["book"], "open");
    assert!(entry["outcome"].as_str().unwrap().contains("books kept open"), "{entry}");
    let frames: Vec<&str> = entry["backtrace"].as_array().unwrap().iter().map(|f| f.as_str().unwrap()).collect();
    assert!(frames.iter().any(|f| f.contains("crates/fb-app/src/screens.rs")), "{frames:?}");
    assert!(frames.iter().all(|f| f.starts_with("fb_") || f.starts_with("<fb_") || f.contains(" at crates/")), "code frames of this tree only: {frames:?}");
    assert!(!frames.iter().any(|f| f.contains("fb_diag::panic") || f.contains("PanicHookInfo")), "no hook frames: {frames:?}");
    let read = &found[2];
    assert_eq!(read["route"], "accounts");
    assert!(read["location"].as_str().unwrap().starts_with("crates/fb-db/src/lib.rs:"), "{read}");
    assert_eq!(read["detail"], "panic with a message");
    assert_clean(&f, &[]);
}

#[test]
fn a_panic_in_the_middle_of_a_write_rolls_back_closes_the_books_and_they_reopen_consistent() {
    let f = fixture();
    let path = "/companies/new/";
    let fields = json!([["name", SECOND], ["currency", "USD"], ["starter_chart", "empty"]]);
    let form_token = token(&page(path));
    fb_app::test_panic::arm("write", "company_new");
    let error = submit(path, &form_token, fields.clone()).unwrap_err();
    assert_eq!(panic_detail(&error), json!({"closed": true, "was_open": true, "saved": "none", "op": "run"}));
    // Closed cleanly: no session, no tokens, no key; nothing thinks the post was saved.
    assert_eq!(state()["open"], false);
    assert!(render(format!("/c/{}/home/", f.company)).is_err());
    {
        let app = app().unwrap();
        assert!(app.messages.is_empty());
        assert!(app.master_key().is_err());
        assert!(!app.tokens.valid(&form_token, path));
    }
    fb_app::test_panic::disarm();
    reopen(&f, "").unwrap();
    let names: Vec<String> = companies().iter().map(|c| c["name"].as_str().unwrap().to_owned()).collect();
    assert_eq!(names, vec![COMPANY.to_owned()], "the panicking write was rolled back");
    integrity_ok();
    // The same post now works, once.
    let done = submit(path, &token(&page(path)), fields).unwrap();
    assert_eq!(done["outcome"], "redirect", "{done}");
    let mut names: Vec<String> = companies().iter().map(|c| c["name"].as_str().unwrap().to_owned()).collect();
    names.sort();
    let mut want = vec![COMPANY.to_owned(), SECOND.to_owned()];
    want.sort();
    assert_eq!(names, want);
    let found = panics(&f);
    assert_eq!(found.len(), 1, "{found:?}");
    let entry = &found[0];
    assert_entry_fields(entry);
    assert_eq!((entry["op"].as_str(), entry["route"].as_str()), (Some("run"), Some("company_new")));
    assert!(entry["location"].as_str().unwrap().starts_with("crates/fb-db/src/lib.rs:"), "{entry}");
    assert_eq!(entry["outcome"], "nothing saved by this call; books closed");
    assert_eq!(entry["book"], "closed");
    // `SECOND` was saved after the panic (it is in the book, never in the log).
    assert_clean(&f, &[form_token.as_str()]);
}

#[test]
fn a_panic_while_finishing_a_change_of_main_books_keeps_them_open_and_is_tried_again() {
    let f = fixture();
    let home = format!("/c/{}/home/", f.company);
    let actor = app().unwrap().authenticated_session().unwrap().actor.to_db();
    // A committed change whose audit event is still owed, naming the open book as the main one.
    let journal = fb_app::books::change_journal(&f.dir.path().join("books"));
    std::fs::create_dir_all(journal.parent().unwrap()).unwrap();
    std::fs::write(&journal, json!({"format": 1, "id": "synthetic-change", "phase": "committed", "main": f.book, "previous": null,
        "set_aside": null, "folder": null, "moves": [], "actor": actor, "detail": {"change": "synthetic-change"}}).to_string()).unwrap();
    fb_app::test_panic::arm_next_write();
    let error = finish_book_change().unwrap_err();
    assert_eq!(panic_detail(&error), json!({"closed": false, "was_open": true, "saved": "none", "op": "finish_book_change"}));
    let s = state();
    assert_eq!((s["open"].as_bool(), s["signed_in"].as_bool()), (Some(true), Some(true)), "the owner keeps working: {s}");
    assert!(journal.exists(), "the change is still owed");
    page(&home);
    // The shell tries again after the next command or open; once it works, it is done for good.
    fb_app::test_panic::disarm();
    assert!(finish_book_change().unwrap());
    assert!(!journal.exists());
    assert!(!finish_book_change().unwrap());
    {
        let mut app = app().unwrap();
        let book = &mut app.open.as_mut().unwrap().book;
        book.idle_after_unwind().unwrap();
        assert!(book.check().unwrap().is_empty(), "integrity check after the panic");
        let written: i64 = book.read(|q| q.query_row(
            "SELECT count(*) FROM ledger_auditevent WHERE action='book.restored_made_active'", [], |r| r.get(0))).unwrap();
        assert_eq!(written, 1, "the audit event is written once");
    }
    let found = panics(&f);
    assert_eq!(found.len(), 1, "{found:?}");
    // Not a screen, so no route; everything else is there.
    for field in ["t", "build", "schema", "source", "op", "kind", "location", "backtrace", "outcome", "book"] {
        assert!(found[0].get(field).is_some(), "{field} missing: {}", found[0]);
    }
    assert!(found[0]["location"].as_str().unwrap().starts_with("crates/fb-db/src/lib.rs:"), "{}", found[0]);
    assert_eq!(found[0]["op"], "finish_book_change");
    assert_eq!(found[0]["outcome"], "nothing saved by this call; books kept open");
    assert_clean(&f, &[]);
}


#[test]
fn worker_thread_panics_are_caught_too_and_never_poison_the_books_lock() {
    let f = fixture();
    let home = format!("/c/{}/home/", f.company);
    fb_app::test_panic::arm("render", "company_home");
    let generation = app().unwrap().session_generation();
    let mut work = Work { generation, action: Action::Render(home.clone()) };
    let error = std::thread::spawn(move || work.compute().unwrap_err()).join().expect("the worker thread did not unwind");
    assert_eq!(panic_detail(&error)["op"], "render_async");
    assert_eq!(panic_detail(&error)["closed"], false);
    assert!(!APP.is_poisoned());
    assert!(app().is_ok());
    // A write on a worker closes the books, like on the main thread.
    fb_app::test_panic::disarm();
    let path = "/companies/new/";
    let command = json!({"type": "SubmitForm", "path": path, "token": token(&page(path)), "fields": [["name", SECOND], ["currency", "USD"]]}).to_string();
    fb_app::test_panic::arm("write", "company_new");
    let mut work = Work { generation: app().unwrap().session_generation(), action: Action::Run(Zeroizing::new(command)) };
    let error = work.compute().unwrap_err();
    assert_eq!(panic_detail(&error)["closed"], true);
    assert!(!APP.is_poisoned());
    assert_eq!(state()["open"], false);
    assert_clean(&f, &[]);
}

#[test]
fn a_failed_post_comes_back_with_a_fresh_token_only_when_nothing_was_saved_and_the_route_is_book_only() {
    let f = fixture();
    // Not found after the token was spent: nothing committed, and accounts write only the book,
    // so the form comes back usable.
    let missing = format!("/c/{}/accounts/0195b4a4-55af-719e-96cb-a2f946b067bf/edit/", f.company);
    let spent = app().unwrap().tokens.mint(&missing);
    let retry = submit(&missing, &spent, json!([["code", "1999"], ["name", "Synthetic"], ["detail_type", "BANK"]])).unwrap();
    assert_eq!(retry["outcome"], "retry", "{retry}");
    let fresh = retry["token"].as_str().unwrap().to_owned();
    assert_ne!(fresh, spent);
    assert!(retry["message"].as_str().unwrap().starts_with("That record is no longer there."), "{retry}");
    {
        let app = app().unwrap();
        assert!(app.tokens.valid(&fresh, &missing), "the fresh token belongs to the same form");
        assert!(!app.tokens.valid(&spent, &missing));
    }
    // The same failure on a route that works outside the book (bank feeds): no token.
    let outside = format!("/c/{}/bank-feeds/rules/0195b4a4-55af-719e-96cb-a2f946b067bf/edit/", f.company);
    let spent = app().unwrap().tokens.mint(&outside);
    let retry = submit(&outside, &spent, json!([["name", "Synthetic"]])).unwrap();
    assert_eq!(retry["outcome"], "retry", "{retry}");
    assert!(retry["token"].is_null(), "{retry}");
    assert!(retry["message"].as_str().unwrap().contains("It may already have been done. Open this page again and check before trying again."), "{retry}");
    // A write that committed and then failed: no token, the page must be reopened, never reposted.
    let path = "/companies/new/";
    let form_token = token(&page(path));
    fb_app::test_panic::arm("error-after-commit", "company_new");
    let retry = submit(path, &form_token, json!([["name", SECOND], ["currency", "USD"]])).unwrap();
    assert_eq!(retry["outcome"], "retry", "{retry}");
    assert!(retry["token"].is_null(), "{retry}");
    let message = retry["message"].as_str().unwrap();
    assert!(message.contains("Part of it may already have been saved.") && !message.contains("Nothing was saved"), "{retry}");
    assert_eq!(companies().len(), 2, "the company was committed before the error");
    assert!(submit(path, &form_token, json!([["name", SECOND], ["currency", "USD"]])).unwrap_err().reason.contains("expired"), "the spent token cannot repost it");
    assert_eq!(companies().len(), 2);
    assert!(log_text(&f).contains("\"kind\":\"error:DatabaseError\""), "{}", log_text(&f));
    assert_clean(&f, &[form_token.as_str(), spent.as_str()]);
}

/// Every way text reaches the log from the bridge: panic payloads that are a `String`, a `&str`
/// and not a string, an `Err` whose text quotes the input, an `AppError::Render` carrying it, and
/// main-process events with a crafted message, stack, name, code, frames and route. None of the
/// text arrives; the diagnostic summary holds none of it either.
#[test]
fn book_text_never_reaches_the_log_through_any_route() {
    let f = fixture();
    let joined = BOOK_TEXT.join(" | ");
    {
        let mut app = app().unwrap();
        let string = within("render", Class::Read, &mut app, |_| -> Result<()> { panic!("row for {joined} has no id") }).unwrap_err();
        assert_eq!(panic_detail(&string)["closed"], false);
        let static_str = within("render", Class::Read, &mut app, |_| -> Result<()> { std::panic::panic_any("Synthetic Carol Hardware owes 1234.56 hunter2-synthetic") }).unwrap_err();
        assert!(static_str.reason.starts_with("FB_PANIC "));
        let other = within("render", Class::Read, &mut app, |_| -> Result<()> { std::panic::panic_any(BOOK_TEXT.iter().map(|s| s.to_string()).collect::<Vec<_>>()) }).unwrap_err();
        assert!(other.reason.starts_with("FB_PANIC "));
        let stored: std::result::Result<(), String> = Err(joined.clone());
        assert!(within("render", Class::Read, &mut app, |_| -> Result<()> { stored.clone().expect("stored row shape"); Ok(()) }).is_err());
        assert!(within("render", Class::Read, &mut app, |_| -> Result<()> { stored.clone().unwrap(); Ok(()) }).is_err());
        // Errors that are not panics: their text goes to the page, never to the log.
        let _ = within("render", Class::Read, &mut app, |_| -> Result<()> { Err(app_error(AppError::Render(joined.clone()))) });
        let _ = within("render", Class::Read, &mut app, |_| -> Result<()> { Err(app_error(AppError::Open(fb_app::opening::OpenError::Unavailable(joined.clone())))) });
    }
    for event in [
        json!({"op": "sync-keys", "kind": "caught", "name": "TypeError", "code": "ERR_INVALID_ARG_TYPE", "message": joined,
               "stack": format!("TypeError: {joined}\n    at Synthetic (/home/synthetic-owner/app/main/main.cjs:1:2)"),
               "frames": [{"fn": "Synthetic Carol Hardware", "file": "main/main.cjs", "line": 216, "col": 31},
                          {"fn": "f", "file": "/home/synthetic-owner/Books/main.fbbook", "line": 1, "col": 1},
                          {"file": "main/hunter2-synthetic.cjs", "line": 1, "col": 1},
                          {"file": "main/Synthetic Carol Hardware.cjs", "line": 1, "col": 1}],
               "route": format!("/c/{}/lists/customers/Synthetic Carol Hardware/", f.company), "outcome": joined}),
        json!({"op": "main", "kind": "uncaughtException", "name": "Synthetic Carol Hardware", "code": "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP", "message": joined}),
        json!({"op": "main", "kind": "unhandledRejection", "name": "string", "code": "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6", "route": "/home/synthetic-owner/Books/main.fbbook"}),
        json!({"op": "Synthetic Carol Hardware", "kind": joined, "name": "Error", "code": "hunter2-synthetic"}),
        json!({"op": "main", "kind": "error", "name": "Error", "code": "ERR_1234", "extra": joined}),
    ] {
        log_event(event.to_string()).unwrap();
    }
    let text = log_text(&f);
    let entries: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).filter(|v: &Value| v.get("header").is_none()).collect();
    let main: Vec<&Value> = entries.iter().filter(|e| e["source"] == "main").collect();
    assert_eq!(main.len(), 5, "{text}");
    assert_eq!(main[0]["kind"], "caught:TypeError:ERR_INVALID_ARG_TYPE");
    assert_eq!(main[0]["backtrace"], json!(["main/main.cjs:216:31"]), "only the app's own frame, without its name");
    assert_eq!(main[0]["route"], "unresolved");
    assert_eq!(main[1]["kind"], "uncaughtException:other:other");
    assert_eq!(main[2]["route"], "unresolved");
    assert_eq!((main[3]["op"].as_str(), main[3]["kind"].as_str()), (Some("main"), Some("main:Error:other")));
    assert_eq!(main[4]["kind"], "error:Error:other");
    let rust: Vec<&Value> = entries.iter().filter(|e| e["source"] == "rust").collect();
    assert_eq!(rust.iter().filter(|e| e["kind"] == "panic").count(), 5, "{text}");
    assert_eq!(rust.iter().filter(|e| e["kind"] == "error:Render").count(), 1, "{text}");
    assert_eq!(rust.iter().filter(|e| e["kind"] == "error:Open:Unavailable").count(), 1, "{text}");
    let details: Vec<&str> = rust.iter().filter_map(|e| e["detail"].as_str()).collect();
    assert_eq!(details, ["panic with a message", "panic with a message", "panic with a non-text payload", "panic with a message", "unwrap on Err"]);
    assert_clean(&f, &[]);
    let summary = diagnostic_summary().unwrap();
    for forbidden in FORBIDDEN.iter().filter(|s| **s != "Synthetic") {
        assert!(!summary.contains(forbidden), "{forbidden}: {summary}");
    }
}

/// The hook under the conditions that used to abort the process, through the bridge: stderr
/// closed, stderr on a full device, the log folder unwritable, and log writes failing as on a full
/// disk. Each runs in a child process (this test binary again); in each the process survives and
/// the call returns the typed `FB_PANIC` error.
#[cfg(unix)]
#[test]
fn panics_return_the_typed_error_with_stderr_closed_or_full_an_unwritable_log_and_a_full_disk() {
    for mode in ["stderr-closed", "stderr-full", "folder-unwritable", "disk-full"] {
        let dir = tempfile::tempdir().unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "panic_tests::child_bridge_under_test", "--nocapture", "--test-threads=1"])
            .env("FB_PANIC_CHILD", mode)
            .env("FB_PANIC_CHILD_DIR", dir.path())
            .env_remove("FB_DIAG_STDERR")
            .env_remove("RUST_BACKTRACE");
        if mode == "stderr-full" {
            command.stderr(std::fs::OpenOptions::new().write(true).open("/dev/full").unwrap());
        }
        let out = command.output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{mode}: {:?}\n{stdout}\n{}", out.status, String::from_utf8_lossy(&out.stderr));
        let expected = if mode == "disk-full" { "CHILD-SURVIVED read=FB_PANIC" } else { "CHILD-SURVIVED read=FB_PANIC write=FB_PANIC" };
        assert!(stdout.contains(expected), "{mode}: {stdout}");
    }
}

#[cfg(unix)]
#[test]
fn child_bridge_under_test() {
    let Ok(mode) = std::env::var("FB_PANIC_CHILD") else { return };
    let root = PathBuf::from(std::env::var("FB_PANIC_CHILD_DIR").unwrap());
    let logs = root.join("logs");
    match mode.as_str() {
        // SAFETY: closing this process's own stderr descriptor.
        "stderr-closed" => assert_eq!(unsafe { libc::close(2) }, 0),
        "folder-unwritable" => {
            use std::os::unix::fs::PermissionsExt;
            std::fs::create_dir(&logs).unwrap();
            std::fs::set_permissions(&logs, std::fs::Permissions::from_mode(0o500)).unwrap();
        }
        _ => {}
    }
    let _turn = crate::tests::turn();
    let _ = configure_log(logs.display().to_string());
    configure_backups(root.display().to_string()).unwrap();
    let book = root.join("books").join("default.fbbook");
    create_protected_book(book.display().to_string(), OWNER.into(), COMPANY.into(), String::new(), false, Some("standard".into())).unwrap();
    sign_in();
    let company = companies()[0]["id"].as_str().unwrap().to_owned();
    if mode == "disk-full" {
        // No file may grow past its size now: every log write fails (EFBIG, signal ignored).
        // SAFETY: plain libc calls with valid arguments.
        unsafe {
            libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
            let size = std::fs::metadata(logs.join(fb_diag::log::FILE)).map(|m| m.len()).unwrap_or(0).max(1);
            let limit = libc::rlimit { rlim_cur: size as libc::rlim_t, rlim_max: libc::RLIM_INFINITY };
            assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &limit), 0);
        }
    }
    fb_app::test_panic::arm("render", "company_home");
    let read = render(format!("/c/{company}/home/")).unwrap_err();
    let mut report = format!("CHILD-SURVIVED read={}", read.reason.split(' ').next().unwrap_or(""));
    if mode != "disk-full" {
        let path = "/companies/new/";
        let form_token = token(&page(path));
        fb_app::test_panic::arm("write", "company_new");
        let write = submit(path, &form_token, json!([["name", SECOND], ["currency", "USD"]])).unwrap_err();
        report.push_str(&format!(" write={}", write.reason.split(' ').next().unwrap_or("")));
    }
    // A panic on a thread nobody guards is recorded by the next call, without aborting anything.
    assert!(std::thread::spawn(|| panic!("{}", BOOK_TEXT.join(" "))).join().is_err());
    let _ = app_state();
    use std::io::Write;
    let _ = writeln!(std::io::stdout(), "{report}");
    let _ = std::io::stdout().flush();
}

/// Fix 5: a call that replaced the open book (a restore opening the restored copy) and then
/// panicked cannot know from commit counts whether anything was saved: two connections' counts
/// are never compared. Also proves the restored copy and the main books stay usable.
#[test]
fn a_panic_after_the_open_book_was_replaced_reports_saved_unknown_and_both_books_recover() {
    let f = fixture();
    let backup = make_backup(&f);
    fb_app::test_panic::arm("point", "restore-opened");
    let error = restore_book_from_gate(backup.display().to_string(), "Synthetic restored copy".into(), String::new()).unwrap_err();
    let detail = panic_detail(&error);
    assert_eq!(detail["saved"], "unknown", "{detail}");
    assert_eq!(detail["closed"], true, "{detail}");
    assert_eq!(state()["open"], false);
    let entry = panics(&f).pop().unwrap();
    assert_eq!(entry["outcome"], "saved state unknown; books closed", "{entry}");
    // The main books reopen as they were; the restored copy opens too.
    reopen(&f, "").unwrap();
    integrity_ok();
    assert_eq!(companies().len(), 1);
    lock().unwrap();
    let restored = f.dir.path().join("books").join("restored-synthetic-restored-copy").join("restored-synthetic-restored-copy.fbbook");
    guarded("open_protected_book", Class::Write, |app| app.open_with_secret(&restored, "").map_err(app_error)).unwrap();
    sign_in();
    integrity_ok();
    assert_eq!(companies()[0]["name"], COMPANY);
    assert_clean(&f, &[]);
}

fn make_backup(f: &Fixture) -> PathBuf {
    let path = format!("/c/{}/backups/create/", f.company);
    let done = submit(&path, &mint(&path), json!([])).unwrap();
    assert_eq!(done["outcome"], "redirect", "{done}");
    newest_backup(f).expect("a backup was written")
}

fn backups(f: &Fixture) -> Vec<PathBuf> {
    let folder = app().unwrap().backups.folder().unwrap();
    let mut found: Vec<PathBuf> = std::fs::read_dir(folder).map(|d| d.map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|e| e == "fbbackup")).collect()).unwrap_or_default();
    found.sort();
    let _ = f;
    found
}

fn newest_backup(f: &Fixture) -> Option<PathBuf> {
    backups(f).pop()
}

#[test]
fn a_panic_during_a_backup_closes_the_books_and_they_reopen_and_back_up_again() {
    let f = fixture();
    let path = format!("/c/{}/backups/create/", f.company);
    fb_app::test_panic::arm("point", "backup-written");
    let error = submit(&path, &mint(&path), json!([])).unwrap_err();
    assert_eq!(panic_detail(&error)["closed"], true, "{}", error.reason);
    assert_eq!(state()["open"], false);
    // The half-finished backup was written but never verified or audited.
    let half = newest_backup(&f).expect("the file was written before the panic");
    assert!(!fb_core_retention_verified(&half), "never marked verified");
    reopen(&f, "").unwrap();
    integrity_ok();
    let audited: i64 = app().unwrap().open.as_mut().unwrap().book.read(|q| q.query_row("SELECT count(*) FROM ledger_auditevent WHERE action='backup.create'", [], |r| r.get(0))).unwrap();
    assert_eq!(audited, 0, "the panicking backup left no audit event");
    let good = make_backup(&f);
    assert_ne!(good, half);
    assert!(fb_core_retention_verified(&good), "the next backup verifies");
    assert_clean(&f, &[]);
}

fn fb_core_retention_verified(file: &std::path::Path) -> bool {
    // fb-core's retention sidecar: `<backup>.verified.json`, written only after verification.
    let mut note = file.as_os_str().to_owned();
    note.push(".verified.json");
    std::path::Path::new(&note).exists()
}

#[test]
fn a_panic_during_a_restore_leaves_the_books_as_they_were_and_the_restore_can_be_done_again() {
    let f = fixture();
    let backup = make_backup(&f);
    fb_app::test_panic::arm("point", "restore-record-saved");
    let error = restore_book_from_gate(backup.display().to_string(), "Synthetic restored copy".into(), String::new()).unwrap_err();
    assert_eq!(panic_detail(&error)["closed"], true, "{}", error.reason);
    reopen(&f, "").unwrap();
    integrity_ok();
    assert_eq!(companies()[0]["name"], COMPANY);
    lock().unwrap();
    // The same restore again: the half-made folder is cleared or refused in plain words, and with
    // another name it works.
    let again = restore_book_from_gate(backup.display().to_string(), "Synthetic restored copy".into(), String::new());
    let opened = match again {
        Ok(opened) => opened,
        Err(e) => {
            assert!(!e.reason.starts_with("FB_PANIC"), "{}", e.reason);
            restore_book_from_gate(backup.display().to_string(), "Synthetic second copy".into(), String::new()).unwrap()
        }
    };
    assert!(opened.contains("sign-in") || opened.contains("/c/"), "{opened}");
    sign_in();
    integrity_ok();
    assert_eq!(companies()[0]["name"], COMPANY);
    assert_clean(&f, &[]);
}

#[test]
fn a_panic_during_a_protection_change_recovers_to_one_working_key() {
    for point in ["rekey-journal-saved", "rekey-pages-written", "protection-audit-written"] {
        let f = fixture();
        let path = fb_app::protection::SETTINGS;
        fb_app::test_panic::arm("point", point);
        let fields = json!([["action", "add"], ["current_password", ""], ["new_password", BOOK_PASSWORD], ["repeat_password", BOOK_PASSWORD]]);
        let error = submit(path, &mint(path), fields.clone()).unwrap_err();
        let detail = panic_detail(&error);
        assert_eq!(detail["closed"], true, "{point}: {detail}");
        assert_eq!(state()["open"], false, "{point}");
        {
            let app = app().unwrap();
            assert!(app.master_key().is_err(), "{point}: the key left memory");
        }
        // Startup recovery finishes or rolls back the change; exactly one secret opens the books.
        let with_old = reopen(&f, "");
        if with_old.is_err() {
            lock().unwrap();
            reopen(&f, BOOK_PASSWORD).unwrap_or_else(|e| panic!("{point}: neither the old nor the new key opens the books: {}", e.reason));
        }
        integrity_ok();
        assert_eq!(companies()[0]["name"], COMPANY, "{point}");
        // And the change can then be made (again) normally.
        let fields = if with_old.is_ok() { fields } else { json!([["action", "add"], ["current_password", BOOK_PASSWORD], ["new_password", BOOK_PASSWORD], ["repeat_password", BOOK_PASSWORD]]) };
        let done = submit(path, &mint(path), fields).unwrap();
        assert_eq!(done["outcome"], "redirect", "{point}: {done}");
        lock().unwrap();
        reopen(&f, BOOK_PASSWORD).unwrap();
        integrity_ok();
        assert_clean(&f, &[]);
    }
}

#[test]
fn a_panic_during_a_migration_at_open_leaves_a_book_that_opens_and_finishes_the_upgrade() {
    let f = fixture();
    lock().unwrap();
    // The same book, one migration behind this build (a retained-fixture-style older book).
    let key = fb_app::protection::builtin_key();
    let prior = f.book.with_extension("prior");
    {
        let source = fb_db::Book::open(&f.book, &key).unwrap();
        let last = fb_db::MIGRATIONS.last().unwrap().0;
        fb_db::test_support::populated_copy(source.connection(), &prior, &key, last, false).unwrap();
    }
    for suffix in ["-wal", "-shm"] {
        let mut s = f.book.as_os_str().to_owned();
        s.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(s));
    }
    std::fs::rename(&prior, &f.book).unwrap();
    let behind = fb_db::MIGRATIONS.len() - 1;
    fb_app::test_panic::arm("point", "migration");
    let error = reopen(&f, "").unwrap_err();
    let detail = panic_detail(&error);
    assert_eq!((detail["closed"].as_bool(), detail["was_open"].as_bool()), (Some(false), Some(false)), "{detail}");
    assert_eq!(state()["open"], false);
    assert_eq!(user_version(&f.book, &key), behind as i64, "the panicking migration rolled back");
    // The verified copy made before the upgrade is there; opening again finishes the upgrade.
    assert!(backups(&f).iter().any(|b| b.file_name().unwrap().to_str().unwrap().contains("before-upgrade")), "{:?}", backups(&f));
    reopen(&f, "").unwrap();
    assert_eq!(user_version(&f.book, &key), fb_db::MIGRATIONS.len() as i64);
    integrity_ok();
    assert_eq!(companies()[0]["name"], COMPANY);
    let entry = panics(&f).pop().unwrap();
    assert_eq!(entry["op"], "open_protected_book");
    assert!(entry["location"].as_str().unwrap().starts_with("crates/fb-db/src/lib.rs:"), "{entry}");
    assert_clean(&f, &[]);
}

fn user_version(path: &std::path::Path, key: &fb_app::keys::MasterKey) -> i64 {
    let c = fb_db::rusqlite::Connection::open(path).unwrap();
    c.execute_batch(&format!("PRAGMA key=\"x'{}'\"", keys::to_hex(key.as_ref()))).unwrap();
    c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap()
}
