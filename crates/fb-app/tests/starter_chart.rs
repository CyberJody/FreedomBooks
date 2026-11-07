use fb_app::{
    App, Command, Outcome,
    commands::{StarterChart, parse_command},
    keys, screens,
};
use serde_json::{Value, json};

fn count(app: &mut App, table: &str) -> i64 {
    app.open_mut()
        .unwrap()
        .book
        .read(|tx| tx.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)))
        .unwrap()
}
fn post(app: &mut App, fields: Vec<(String, String)>) -> Outcome {
    let path = "/companies/new/";
    let token = app.tokens.mint(path);
    app.run(Command::SubmitForm {
        path: path.into(),
        token,
        fields,
    })
    .unwrap()
}
fn fields(name: &str, chart: &str) -> Vec<(String, String)> {
    [
        ("name", name),
        ("currency", "USD"),
        ("starter_chart", chart),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v.into()))
    .collect()
}
fn run(app: &mut App, value: Value) {
    app.run(parse_command(&value.to_string()).unwrap()).unwrap();
}

#[test]
fn first_run_and_new_company_offer_standard_and_empty_charts() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new();
    app.create_book_with_chart(
        &dir.path().join("standard.fbbook"),
        &keys::new_master_key().unwrap(),
        "Owner",
        "Standard",
        StarterChart::Standard,
    )
    .unwrap();
    assert_eq!(count(&mut app, "ledger_account"), 34);
    let html = screens::render(&mut app, "/companies/new/").unwrap().html;
    assert!(html.contains("Standard small-business chart (recommended)"));
    assert!(html.contains("Start with an empty chart (for accountants)"));
    assert!(html.contains("value=\"standard\" selected"));
    assert!(matches!(
        post(&mut app, fields("Empty", "empty")),
        Outcome::Redirect { .. }
    ));
    assert_eq!(count(&mut app, "ledger_account"), 34);
    assert!(matches!(
        post(&mut app, fields("Another standard", "standard")),
        Outcome::Redirect { .. }
    ));
    assert_eq!(count(&mut app, "ledger_account"), 68);
    let before = count(&mut app, "ledger_auditevent");
    let Outcome::Invalid { html } = post(&mut app, fields("Refused", "unknown")) else {
        panic!("unknown chart accepted")
    };
    assert!(html.contains("Select a valid choice."));
    assert_eq!(count(&mut app, "ledger_company"), 3);
    assert_eq!(count(&mut app, "ledger_auditevent"), before);
    let Outcome::Invalid { html } = post(&mut app, fields("", "empty")) else {
        panic!("blank name accepted")
    };
    assert!(
        html.contains("value=\"empty\" selected"),
        "keep the user's empty selection on errors"
    );
}

#[test]
fn opening_existing_seeded_books_and_editing_companies_never_add_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("existing.fbbook");
    let key = keys::new_master_key().unwrap();
    let mut app = App::new();
    let c = app
        .create_book(&path, &key, "Owner", "Existing")
        .unwrap()
        .company_id
        .to_db();
    run(
        &mut app,
        json!({"type":"SaveAccount", "company_id":c,"code":"1000","name":"Existing cash","detail_type":"BANK"}),
    );
    let before = count(&mut app, "ledger_auditevent");
    app.lock();
    app.open_book(&path, &key).unwrap();
    assert_eq!(count(&mut app, "ledger_account"), 1);
    assert_eq!(count(&mut app, "ledger_auditevent"), before);
    run(
        &mut app,
        json!({"type":"SaveCompany", "company_id":c,"name":"Renamed","currency":"USD","starter_chart":"standard"}),
    );
    assert_eq!(count(&mut app, "ledger_account"), 1);
    let html = screens::render(&mut app, &format!("/c/{c}/edit/"))
        .unwrap()
        .html;
    assert!(!html.contains("starter_chart"));
    run(
        &mut app,
        json!({"type":"SaveCompany", "name":"Legacy caller","currency":"USD"}),
    );
    assert_eq!(
        count(&mut app, "ledger_account"),
        1,
        "omitted choice preserves existing command clients"
    );
    assert!(
        parse_command(
            r#"{"type":"SaveCompany","name":"Bad","currency":"USD","starter_chart":"bogus"}"#
        )
        .is_err()
    );
}

#[test]
fn first_run_keeps_the_empty_choice_when_an_error_is_displayed() {
    let app = App::new();
    let html = screens::render_gate_with_chart(
        &app,
        "setup",
        true,
        "",
        &["Enter the company name.".into()],
        "empty",
    )
    .unwrap();
    assert!(html.contains("value=\"empty\" selected"));
    assert!(!html.contains("value=\"standard\" selected"));
    assert!(screens::render_gate_with_chart(&app, "setup", true, "", &[], "bogus").is_err());
}

#[test]
fn protected_first_run_routes_the_chart_without_changing_password_or_builtin_modes() {
    for (password, chart, expected) in [
        (Some("Synthetic chart password"), StarterChart::Standard, 34),
        (None, StarterChart::Standard, 34),
        (Some("Synthetic chart password"), StarterChart::Empty, 0),
        (None, StarterChart::Empty, 0),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("books/chart.fbbook");
        let mut app = App::new();
        app.backups.configure(dir.path()).unwrap();
        app.create_protected_book_with_chart(&path, "Chart owner", "Chart company", password, chart).unwrap();
        assert_eq!(count(&mut app, "ledger_account"), expected);
        assert_eq!(app.builtin(), password.is_none());
        assert_eq!(app.can_lock(), password.is_some());
        let before = count(&mut app, "ledger_auditevent");
        let record = fb_app::protection::Record::read(&path).unwrap().unwrap();
        assert_eq!(record.keys.password.is_some(), password.is_some());
        assert_eq!(record.keys.recovery.is_some(), password.is_some());
        if password.is_none() {
            let html = screens::render(&mut app, fb_app::protection::SETTINGS).unwrap().html;
            assert!(html.contains(fb_core::backups::portable::BUILTIN_WARNING));
            assert!(!html.to_lowercase().contains("encrypted"));
        } else {
            assert!(app.recovery_text().is_ok());
        }
        app.lock();
        app.open_with_secret(&path, password.unwrap_or("")).unwrap();
        assert_eq!(count(&mut app, "ledger_account"), expected);
        assert_eq!(count(&mut app, "ledger_auditevent"), before);
    }
}
