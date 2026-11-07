//! The desktop flow without a window: first run -> typed commands -> a ported form posted with its
//! command token -> post -> the invoice in the list and the A/R register -> lock -> reopen.
use fb_app::commands::{Command, Outcome, parse_command};
use fb_app::{App, AppError, keys, screens};
use serde_json::json;

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("fb-app-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("default.fbbook")
}

fn token_of(html: &str) -> String {
    let at = html.find(r#"name="fb_command_token" value=""#).expect("token input") + r#"name="fb_command_token" value=""#.len();
    html[at..at + 64].to_owned()
}

fn field(html: &str, name: &str) -> String {
    let marker = format!(r#"name="{name}" value=""#);
    let at = html.find(&marker).unwrap() + marker.len();
    html[at..].split('"').next().unwrap().to_owned()
}

fn run(app: &mut App, v: serde_json::Value) -> serde_json::Value {
    match app.run(parse_command(&v.to_string()).unwrap()).unwrap() {
        Outcome::Done { data } => data,
        other => panic!("{other:?}"),
    }
}

#[test]
fn first_run_invoice_form_post_and_register() {
    let path = tmp("flow");
    let key = keys::new_master_key().unwrap();
    let mut app = App::new();
    app.today = || chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
    let created = app.create_book(&path, &key, "Dana Owner", "Kettle Pond Supply").unwrap();
    let c = created.company_id.to_db();
    assert!(app.create_book(&path, &key, "x", "y").is_err(), "never overwrites a book");

    let home = screens::render(&mut app, &format!("/c/{c}/home/")).unwrap();
    assert!(home.html.contains("Operator: Dana Owner") && home.html.contains("Needs setup"));

    let acct = |app: &mut App, code: &str, name: &str, t: &str| run(app, json!({"type": "SaveAccount", "company_id": c, "code": code, "name": name, "detail_type": t}))["id"].as_str().unwrap().to_owned();
    let bank = acct(&mut app, "1000", "Checking", "BANK");
    let ar = acct(&mut app, "1100", "Accounts receivable", "RECEIVABLE");
    let uf = acct(&mut app, "1200", "Undeposited funds", "OTHER_CURRENT_ASSET");
    let income = acct(&mut app, "4000", "Sales", "INCOME");
    let customer = run(&mut app, json!({"type": "SaveCustomer", "company_id": c, "display_name": "Maple & Birch Café"}))["id"].as_str().unwrap().to_owned();
    let item = run(&mut app, json!({"type": "SaveItem", "company_id": c, "code": "MOW", "description": "Lawn mowing", "kind": "SERVICE", "sale_rate": "45.00", "income_account_id": income}))["id"].as_str().unwrap().to_owned();
    run(&mut app, json!({"type": "SaveSetup", "company_id": c, "expected_revision": 0, "receivables_id": ar, "undeposited_id": uf, "bank_ids": [bank]}));

    // The ported form, posted back as a command with the token minted by this render.
    let form_path = format!("/c/{c}/invoices/new/");
    let form = screens::render(&mut app, &form_path).unwrap();
    let token = token_of(&form.html);
    let fields: Vec<(String, String)> = [
        ("request_key", field(&form.html, "request_key")),
        ("expected_revision", "0".into()),
        ("customer", customer.clone()),
        ("date", "2026-10-01".into()),
        ("memo", "October mowing".into()),
        ("tax_mode", "NONE".into()),
        ("due_date", "2026-10-31".into()),
        ("lines-TOTAL_FORMS", "2".into()),
        ("lines-INITIAL_FORMS", "0".into()),
        ("lines-0-item", item.clone()),
        ("lines-0-description", "Lawn mowing".into()),
        ("lines-0-quantity", "4".into()),
        ("lines-0-rate", "45.00".into()),
        ("lines-0-income_account", income.clone()),
        ("lines-0-tax_auto", "1".into()),
        ("lines-1-tax_auto", "1".into()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    let submit = Command::SubmitForm { path: form_path.clone(), token: token.clone(), fields: fields.clone() };
    let location = match app.run(submit.clone()).unwrap() {
        Outcome::Redirect { location } => location,
        other => panic!("{other:?}"),
    };
    assert!(location.starts_with(&format!("/c/{c}/invoices/")), "{location}");
    let invoice_id = location.trim_end_matches('/').rsplit('/').next().unwrap().to_owned();
    assert!(matches!(app.run(submit), Err(AppError::Invalid(_))), "a token is single-use");

    // An invalid post re-renders the form with Django's errors, keeping what was typed.
    let form = screens::render(&mut app, &form_path).unwrap();
    let mut bad = fields.clone();
    bad.retain(|(k, _)| k != "customer");
    bad.iter_mut().find(|(k, _)| k == "request_key").unwrap().1 = field(&form.html, "request_key");
    match app.run(Command::SubmitForm { path: form_path.clone(), token: token_of(&form.html), fields: bad }).unwrap() {
        Outcome::Invalid { html } => {
            assert!(html.contains("Invoice not saved.") && html.contains(r#"<ul class="errorlist" id="id_customer_error"><li>This field is required.</li></ul>"#));
            assert!(html.contains("October mowing"));
        }
        other => panic!("{other:?}"),
    }

    let list = screens::render(&mut app, &format!("/c/{c}/invoices/")).unwrap();
    assert!(list.html.contains("INV-000001") && list.html.contains("Draft") && list.html.contains("1 draft"));

    run(&mut app, json!({"type": "PostInvoice", "company_id": c, "invoice_id": invoice_id, "expected_revision": 1}));
    let register = screens::render(&mut app, &format!("/c/{c}/accounts/{ar}/register/")).unwrap();
    assert!(register.html.contains("Invoice <a href=\"/c/"), "{}", register.html);
    assert!(register.html.contains("INV-000001: October mowing") && register.html.contains("180.00"));

    // Lock drops the book and every token; reopening with the same key restores the owner.
    app.lock();
    assert!(matches!(screens::render(&mut app, &format!("/c/{c}/home/")), Err(AppError::Locked)));
    assert!(app.open_book(&path, &keys::new_master_key().unwrap()).is_err(), "wrong key refused");
    app.open_book(&path, &key).unwrap();
    assert_eq!(app.first_company().unwrap().unwrap().to_db(), c);
    let home = screens::render(&mut app, &format!("/c/{c}/home/")).unwrap();
    assert!(home.html.contains("Operator: Dana Owner") && home.html.contains("1000 · Checking"));
}

#[test]
fn form_posts_need_a_live_token_for_that_path() {
    let path = tmp("tokens");
    let key = keys::new_master_key().unwrap();
    let mut app = App::new();
    let c = app.create_book(&path, &key, "Owner", "Co").unwrap().company_id.to_db();
    let form = screens::render(&mut app, &format!("/c/{c}/invoices/new/")).unwrap();
    let token = token_of(&form.html);
    let other = Command::SubmitForm { path: format!("/c/{c}/accounts/new/"), token: token.clone(), fields: vec![] };
    assert!(matches!(app.run(other), Err(AppError::Invalid(_))), "token bound to its path");
    app.lock();
    app.open_book(&path, &key).unwrap();
    let after = Command::SubmitForm { path: format!("/c/{c}/invoices/new/"), token, fields: vec![] };
    assert!(matches!(app.run(after), Err(AppError::Invalid(_))), "lock forgets tokens");
}
