#![cfg(all(unix, feature = "key-test-hooks"))]
mod support;
use fb_app::{App, keys::MasterKey, protection::{self, Record}};
use std::process::Command;

const PASSWORD: &str = "Synthetic publication password";

#[test]
fn publication_child() {
    let Ok(action) = std::env::var("FB_PUBLICATION_ACTION") else { return };
    let point = std::env::var("FB_KEY_FAIL_AT").unwrap();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("books/default.fbbook");
    let mut app = App::new();
    app.backups.configure(root.path()).unwrap();
    app.device_key = Some(MasterKey::new([83;32]));
    app.os_store_usable = true;
    app.create_protected_book(&path, "Publication owner", "Synthetic publication books",
        (action != "book.protection_changed").then_some(PASSWORD)).unwrap();
    support::sign_in(&mut app);
    if !app.builtin() { app.authorize_recovery(PASSWORD).unwrap(); }
    let before = serde_json::to_value(Record::read(&path).unwrap().unwrap()).unwrap();
    let company = app.first_company().unwrap().unwrap();
    let open = app.open_mut().unwrap();
    open.book.write(|tx| {
        let user = fb_core::services::access(tx, open.owner, company)?;
        fb_core::services::audit(tx, &user, company, "book.synthetic_before_recovery",
            "Synthetic work preceding protection".into(), serde_json::json!({}))
    }).unwrap();
    let result = match action.as_str() {
        "book.password_prompt" => app.change_ask_password(PASSWORD, true),
        "book.recovery_saved" => app.recovery_saved(),
        "book.protection_changed" => app.change_protection("", Some(PASSWORD)),
        _ => panic!("unknown action"),
    };
    assert!(result.unwrap_err().to_string().contains("Injected protection journal failure"));
    assert!(!app.is_open(), "{point}: an ambiguous journal error must quarantine the session");
    assert!(app.open_mut().is_err(), "no later accounting work may be posted behind this journal");
    // Recover in a fresh process without the write fault; never assume an error means rename
    // did not happen. A visible committed marker retains the completed operation and its audit.
    let durable_commit = point.starts_with("journal-committed-")
        && (point.ends_with("dir-sync") || point.ends_with("published") || point.contains("cleanup-"));
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "publication_recovery_child", "--nocapture"])
        .env_remove("FB_KEY_FAIL_AT").env_remove("FB_PUBLICATION_ACTION")
        .env("FB_PUBLICATION_ROOT", root.path()).env("FB_PUBLICATION_RECOVER_ACTION", &action)
        .env("FB_PUBLICATION_COMMITTED", if durable_commit { "1" } else { "0" })
        .env("FB_PUBLICATION_OLD", before.to_string()).status().unwrap();
    assert!(status.success(), "{action}: {point}");
}

#[test]
fn publication_recovery_child() {
    let Ok(root) = std::env::var("FB_PUBLICATION_ROOT") else { return };
    let path = std::path::Path::new(&root).join("books/default.fbbook");
    protection::recover_key_change(&path).unwrap();
    assert!(!protection::recover_key_change(&path).unwrap(), "recovery must be idempotent");
    let after = Record::read(&path).unwrap().unwrap();
    let committed = std::env::var("FB_PUBLICATION_COMMITTED").unwrap() == "1";
    let before: serde_json::Value = serde_json::from_str(&std::env::var("FB_PUBLICATION_OLD").unwrap()).unwrap();
    if !committed { assert_eq!(serde_json::to_value(&after).unwrap(), before); }
    let mut app = App::new();
    app.backups.configure(std::path::Path::new(&root)).unwrap();
    app.device_key = Some(MasterKey::new([83;32]));
    app.open_book(&path, &after.keys.unlock(PASSWORD).unwrap()).unwrap();
    support::sign_in(&mut app);
    let action = std::env::var("FB_PUBLICATION_RECOVER_ACTION").unwrap();
    let count: i64 = app.open_mut().unwrap().book.read(|q| q.query_row(
        "SELECT count(*) FROM ledger_auditevent WHERE action=?1", [&action], |r| r.get(0))).unwrap();
    assert_eq!(count, i64::from(committed), "actor audit must match recovered state exactly once");
    let count: i64 = app.open_mut().unwrap().book.read(|q| q.query_row(
        "SELECT count(*) FROM ledger_auditevent WHERE action='book.synthetic_before_recovery'", [], |r| r.get(0))).unwrap();
    assert_eq!(count, 1, "work preceding the change must survive every recovery path");
    assert_eq!(app.company_name_for_kit().unwrap(), "Synthetic publication books");
    assert!(!protection::sidecar(&path, ".rekey.json").exists());
    assert!(!protection::sidecar(&path, ".rekey-before").exists());

}

#[test]
fn journal_errors_quarantine_all_protection_actions() {
    for action in ["book.password_prompt", "book.recovery_saved", "book.protection_changed"] {
        for phase in ["prepared", "committed"] {
            for stage in ["write", "file-sync", "rename", "dir-sync", "published", "cleanup-snapshot-remove", "cleanup-snapshot-sync", "cleanup-marker-remove", "cleanup-marker-sync"] {
                if phase == "prepared" && stage.starts_with("cleanup-") { continue; }
                let point = format!("journal-{phase}-{stage}");
                let status = Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "publication_child", "--nocapture"])
                    .env("FB_PUBLICATION_ACTION", action).env("FB_KEY_FAIL_AT", &point).status().unwrap();
                assert!(status.success(), "{action}: {point}");
            }
        }
    }
}
