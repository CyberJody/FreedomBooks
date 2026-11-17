//! Screenshot fixture for Review transactions: a populated synthetic company rendered to HTML when
//! `FB_REVIEW_SCREEN` names an output folder (otherwise the test only checks that the page renders).
//! A bank connection was replaced (reinstall): the old one recorded two purchases, and the new one's first
//! download brings them again with six new transactions. Uses only long-standing services, so the same
//! file renders the screen before and after the 2026-11-10 review changes.
use fb_app::{App, keys::MasterKey, screens};
use fb_core::{AccountId, bankfeeds as bf, services};
use serde_json::{Value, json};
mod support;

fn tx(id: &str, date: &str, amount: i64, description: &str) -> bf::StagedTransaction {
    bf::StagedTransaction { remote_id: id.into(), posted_on: Some(date.into()), transacted_on: None, amount, description: description.into(), pending: false }
}
fn pull(transactions: Vec<bf::StagedTransaction>) -> bf::StagedPull {
    bf::StagedPull {
        accounts: vec![bf::StagedAccount {
            conn_key: "harbor".into(),
            remote_id: "ACT-checking".into(),
            name: "Business Checking".into(),
            org_name: "Harbor Community Bank".into(),
            currency: "USD".into(),
            balance: Some(1_248_055),
            available: None,
            balance_epoch: None,
            skipped: vec![],
            transactions,
        }],
        ..Default::default()
    }
}

#[test]
fn populated_review_screen_renders() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new();
    app.simplefin = true;
    let co = app.create_book(&dir.path().join("synthetic.fbbook"), &MasterKey::new([73; 32]), "Dana Whitfield", "Harbor Street Bakery").unwrap().company_id;
    support::sign_in(&mut app);
    let o = app.open_mut().unwrap();
    let none = bf::SecretSet::none();
    let mut ids = std::collections::HashMap::new();
    for (code, name, kind, detail) in [
        ("6070", "Office supplies", "EXPENSE", "EXPENSE"),
        ("6100", "Advertising", "EXPENSE", "EXPENSE"),
        ("6200", "Software subscriptions", "EXPENSE", "EXPENSE"),
        ("6300", "Fuel and mileage", "EXPENSE", "EXPENSE"),
        ("6310", "Vehicle fuel", "EXPENSE", "EXPENSE"),
        ("6400", "Meals with clients", "EXPENSE", "EXPENSE"),
        ("6500", "Ingredients", "EXPENSE", "EXPENSE"),
        ("4000", "Bakery sales", "INCOME", "INCOME"),
        ("4100", "Interest income", "INCOME", "OTHER_INCOME"),
    ] {
        ids.insert(code, services::save_account(&mut o.book, o.owner, co, services::AccountInput::new(code, name).kind(kind).detail(detail)).unwrap().id.to_db());
    }
    let (office, software) = (ids["6070"].clone(), ids["6200"].clone());
    let earlier = vec![
        tx("TX-1001", "2026-10-27", -8419, "OFFICE DEPOT #221 PURCHASE"),
        tx("TX-1002", "2026-10-29", -5900, "ACME CLOUD SOFTWARE AUTOPAY"),
    ];
    let old = bf::connect(&mut o.book, o.owner, co, &json!("Harbor bank"), "bridge.simplefin.org", &none).unwrap()["id"].as_str().unwrap().to_owned();
    bf::store_pull(&mut o.book, o.owner, co, &old, &pull(earlier.clone()), "2026-10-30T12:00:00Z", false, None, &none).unwrap();
    let feed = |book: &mut fb_core::Book, conn: &str| -> String { book.read(|q| q.query_row("SELECT id FROM bankfeeds_account WHERE connection_id=?1", [conn], |r| r.get(0))).unwrap() };
    let old_feed = feed(&mut o.book, &old);
    bf::save_mappings(&mut o.book, o.owner, co, &old, &[(old_feed.clone(), bf::MappingTarget::New { name: "Checking".into(), kind: bf::NewAccountKind::Checking })], &none).unwrap();
    let bank: String = o.book.read(|q| q.query_row("SELECT ledger_account_id FROM bankfeeds_account WHERE id=?1", [&old_feed], |r| r.get(0))).unwrap();
    for (remote, category) in [("TX-1001", &office), ("TX-1002", &software)] {
        let id: String = o.book.read(|q| q.query_row("SELECT id FROM bankfeeds_transaction WHERE remote_id=?1", [remote], |r| r.get(0))).unwrap();
        bf::accept(&mut o.book, o.owner, co, &id, &json!({"kind":"CHECK","account_id":category}), &none).unwrap();
    }
    bf::save_rule(&mut o.book, o.owner, co, &json!({"name":"Fuel stops","match_text":"FUEL","account_id":ids["6310"],"payee_name":"Fuel Stop 18"}), &none).unwrap();
    // The reinstall: the old connection is unmapped and forgotten, a new one downloads the same days again.
    bf::map_account(&mut o.book, o.owner, co, &old_feed, &Value::Null, &none).unwrap();
    bf::forget(&mut o.book, o.owner, co, &old, &none).unwrap();
    let new = bf::connect(&mut o.book, o.owner, co, &json!("Harbor bank (reconnected)"), "bridge.simplefin.org", &none).unwrap()["id"].as_str().unwrap().to_owned();
    let mut again = earlier;
    again.extend([
        tx("TX-1003", "2026-11-02", 250_000, "MOBILE DEPOSIT CATERING ORDER"),
        tx("TX-1004", "2026-11-03", -120_000, "CHECK 1041"),
        tx("TX-1005", "2026-11-04", -4275, "FUEL STOP 18 QUEENSBURY"),
        tx("TX-1006", "2026-11-05", -1899, "SQUARESPACE WEBSITE"),
        tx("TX-1007", "2026-11-06", -3640, "HARBOR DINER CLIENT LUNCH"),
        tx("TX-1008", "2026-11-07", 312, "INTEREST PAYMENT"),
    ]);
    bf::store_pull(&mut o.book, o.owner, co, &new, &pull(again), "2026-11-10T12:00:00Z", false, None, &none).unwrap();
    let new_feed = feed(&mut o.book, &new);
    bf::save_mappings(&mut o.book, o.owner, co, &new, &[(new_feed, bf::MappingTarget::Existing(AccountId::parse(&bank)))], &none).unwrap();
    let html = screens::render(&mut app, &format!("/c/{co}/bank-feeds/review/")).unwrap().html;
    assert!(html.contains("OFFICE DEPOT #221 PURCHASE") && html.contains("HARBOR DINER CLIENT LUNCH"));
    if let Ok(out) = std::env::var("FB_REVIEW_SCREEN") {
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(std::path::Path::new(&out).join("populated.html"), html).unwrap();
    }
}
