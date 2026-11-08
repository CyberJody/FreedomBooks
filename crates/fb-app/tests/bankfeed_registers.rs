//! The exact differential integration story, opened in the desktop after its second restore.
mod support;
use fb_app::{App, Command, Outcome, keys, screens};
use serde_json::{Value, json};

#[test]
fn bankfeed_check_and_deposit_reach_restored_desktop_register_screens() {
    let root = std::env::temp_dir().join(format!("fb-int4b-registers-{}", fb_ids::new_v7()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("live.fbbook");
    let key = keys::new_master_key().unwrap();
    let mut scenario: Value = serde_json::from_str(include_str!("../../fb-harness/scenarios/int-l4b-combined.json")).unwrap();
    // The harness run verifies every checkpoint; this test renders the selected restored final book.
    scenario["capture_steps"] = json!(false);
    let mut runner = fb_harness::Runner::with_key(&scenario, &path, &key);
    runner.run();
    let dump = runner.dump();
    for outcome in dump["outcomes"].as_array().unwrap() {
        assert_ne!(outcome["unexpected"], true, "{outcome}");
    }
    let restored = std::path::PathBuf::from(dump["book_identity"]["selected"].as_str().unwrap());
    assert_ne!(restored, path);
    assert_eq!(dump["bankfeeds"]["transactions"].as_array().unwrap().iter().filter(|r| r["status"] == "ACCEPTED").count(), 2);
    drop(runner);
    // Preserve the pre-host negative control on the genuine restored book.
    let mut unrestored = fb_core::Book::open(&restored, &key).unwrap();
    let (actor, company): (String, String) = unrestored.read(|q| q.query_row(
        "SELECT u.id,c.id FROM fb_user u JOIN ledger_company_members m ON m.user_id=u.id JOIN ledger_company c ON c.id=m.company_id WHERE u.is_active=1 ORDER BY u.id,c.id LIMIT 1", [], |r| Ok((r.get(0)?,r.get(1)?)))).unwrap();
    let error = fb_core::services::edit_company(&mut unrestored, actor.parse().unwrap(), company.parse().unwrap(), "must refuse", "USD").unwrap_err();
    assert!(error.to_string().contains("Restore bank-feed connection credentials"));
    drop(unrestored);
    let mut app = App::new();
    app.plugins = true;
    app.open_book(&restored, &key).unwrap();
    support::sign_in(&mut app);
    let company = app.first_company().unwrap().unwrap();
    let account = |app: &mut App, name: &str| {
        app.open_mut().unwrap().book.read(|q| q.query_row(
            "SELECT id FROM ledger_account WHERE company_id=?1 AND name=?2", [company.to_db(), name.to_owned()], |r| r.get::<_, String>(0))).unwrap()
    };
    let expense = account(&mut app, "expense");
    let undeposited = account(&mut app, "uf");
    for (id, marker) in [(expense, "Feed stationery check"), (undeposited, "12.34")] {
        let page = screens::render(&mut app, &format!("/c/{company}/accounts/{id}/register/")).unwrap();
        assert_eq!(page.status, 200);
        assert!(page.html.contains(marker), "restored register missing {marker}");
        assert!(!page.html.contains("not yet ported"));
    }
    // W4 restores the actual encrypted fake credentials at open. A subsequent
    // desktop form is allowed, and its final writer still masks known secrets.
    let form = format!("/c/{company}/edit/");
    let token = app.tokens.mint(&form);
    let result = app.run_renderer(Command::SubmitForm {
        path: form, token,
        fields: vec![("name".into(), "HarnessPassword12345".into()), ("currency".into(), "USD".into())],
    }).unwrap();
    assert!(matches!(result, Outcome::Redirect { .. }));
    let name: String = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT name FROM ledger_company WHERE id=?1", [company.to_db()], |r| r.get(0))).unwrap();
    assert_eq!(name, "***");
    app.lock();
    std::fs::remove_dir_all(root).unwrap();
}
