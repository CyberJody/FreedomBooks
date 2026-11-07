#![cfg(all(unix, feature = "key-test-hooks"))]
use fb_app::{App, keys::MasterKey, protection::Record};
const PASSWORD: &str = "Synthetic audit password";

#[test]
fn audit_failure_child() {
    let Ok(action) = std::env::var("FB_KEY_FAIL_AUDIT") else { return };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("books/default.fbbook");
    let mut app = App::new();
    app.backups.configure(dir.path()).unwrap();
    app.device_key = Some(MasterKey::new([91;32]));
    app.os_store_usable = true;
    let own = action != "book.protection_changed";
    app.create_protected_book(&path,"Synthetic audit owner","Synthetic audit company",own.then_some(PASSWORD)).unwrap();
    let old = serde_json::to_value(Record::read(&path).unwrap().unwrap()).unwrap();
    let result = match action.as_str() {
        "book.password_prompt" => app.change_ask_password(PASSWORD,false),
        "book.recovery_saved" => app.recovery_saved(),
        "book.protection_changed" => app.change_protection("",Some(PASSWORD)),
        _ => panic!("unknown action"),
    };
    assert!(result.unwrap_err().to_string().contains("Injected protection audit failure"));
    assert!(!app.is_open(), "a failed transition must close before releasing its locks");
    fb_app::protection::recover_key_change(&path).unwrap();
    assert_eq!(serde_json::to_value(Record::read(&path).unwrap().unwrap()).unwrap(),old,
        "failed actor audit must roll back published protection state");
    app.open_book(&path, &Record::read(&path).unwrap().unwrap().keys.unlock(PASSWORD).unwrap()).unwrap();
    assert!(app.is_open());
    assert_eq!(app.company_name_for_kit().unwrap(),"Synthetic audit company");
}

#[test]
fn every_failed_protection_audit_preserves_the_old_state() {
    for action in ["book.password_prompt","book.recovery_saved","book.protection_changed"] {
        let status=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","audit_failure_child","--nocapture"])
            .env("FB_KEY_FAIL_AUDIT",action).status().unwrap();
        assert!(status.success(),"{action}");
    }
}
