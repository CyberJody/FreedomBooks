//! Real form commands against encrypted books: omission compatibility, scoping, no-write errors.
use fb_app::{App, Command, Outcome, keys, screens};
use fb_core::directory::{self, Model};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
static N: AtomicUsize = AtomicUsize::new(0);
fn fixture() -> (App, String) {
    let dir = std::env::temp_dir().join(format!(
        "fb-app-d3-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new();
    let c = app
        .create_book(
            &dir.join("books.fbbook"),
            &keys::new_master_key().unwrap(),
            "Owner",
            "Synthetic D3",
        )
        .unwrap()
        .company_id
        .to_db();
    (app, c)
}
fn post(app: &mut App, path: &str, fields: &[(&str, &str)]) -> Outcome {
    let html = screens::render(app, path).unwrap().html;
    let token = html
        .split_once("name=\"fb_command_token\" value=\"")
        .unwrap()
        .1
        .split('"')
        .next()
        .unwrap()
        .into();
    app.run(Command::SubmitForm {
        path: path.into(),
        token,
        fields: fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    })
    .unwrap()
}
fn saved(outcome: Outcome) -> String {
    let Outcome::Redirect { location } = outcome else {
        panic!("did not save: {outcome:?}")
    };
    location
}
fn id(location: &str) -> &str {
    location.trim_end_matches('/').rsplit('/').next().unwrap()
}
fn get(app: &mut App, c: &str, m: Model, id: &str) -> Value {
    app.open_mut()
        .unwrap()
        .book
        .read(|tx| directory::get(tx, c.parse().unwrap(), m, id))
        .unwrap()
}
fn audit_count(app: &mut App) -> i64 {
    app.open_mut()
        .unwrap()
        .book
        .read(|tx| tx.query_row("SELECT count(*) FROM ledger_auditevent", [], |r| r.get(0)))
        .unwrap()
}

#[test]
fn legacy_customer_and_item_posts_preserve_omitted_tax_fields() {
    let (mut app, c) = fixture();
    let path = format!("/c/{c}/lists/customers/new/");
    let loc = saved(post(
        &mut app,
        &path,
        &[
            ("display_name", "Resale"),
            ("tax_status", "EXEMPT"),
            ("exemption_number", "CERT-1"),
        ],
    ));
    let edit = format!("{loc}edit/");
    saved(post(&mut app, &edit, &[("display_name", "Resale revised")]));
    let r = get(&mut app, &c, Model::Customer, id(&loc));
    assert_eq!(r["tax_status"], "EXEMPT");
    assert_eq!(r["exemption_number"], "CERT-1");
    let before = audit_count(&mut app);
    assert!(matches!(
        post(
            &mut app,
            &edit,
            &[("display_name", "Resale"), ("tax_status", "")]
        ),
        Outcome::Invalid { .. }
    ));
    assert_eq!(audit_count(&mut app), before);
    let path = format!("/c/{c}/lists/items/new/");
    let loc = saved(post(
        &mut app,
        &path,
        &[
            ("code", "SEED"),
            ("kind", "NONSTOCK"),
            ("sale_rate", "9.995"),
        ],
    ));
    assert_eq!(
        get(&mut app, &c, Model::Item, id(&loc))["sale_rate"],
        "10.00"
    );
    assert_eq!(get(&mut app, &c, Model::Item, id(&loc))["taxable"], true);
    let edit = format!("{loc}edit/");
    saved(post(
        &mut app,
        &edit,
        &[
            ("code", "SEED"),
            ("kind", "NONSTOCK"),
            ("sale_rate", "10"),
            ("taxable_shown", "1"),
        ],
    ));
    assert_eq!(get(&mut app, &c, Model::Item, id(&loc))["taxable"], false);
    saved(post(
        &mut app,
        &edit,
        &[("code", "SEED"), ("kind", "NONSTOCK"), ("sale_rate", "10")],
    ));
    assert_eq!(get(&mut app, &c, Model::Item, id(&loc))["taxable"], false);
}
#[test]
fn vendor_defaults_are_independently_preserved_and_foreign_accounts_are_refused() {
    let (mut app, c) = fixture();
    let open = app.open_mut().unwrap();
    let co = c.parse().unwrap();
    let actor = open.owner;
    let account = fb_core::services::save_account(
        &mut open.book,
        actor,
        co,
        fb_core::services::AccountInput::new("5000", "Cost").detail("EXPENSE"),
    )
    .unwrap()
    .id
    .to_db();
    let other = fb_core::services::create_company(&mut open.book, actor, "Other", "USD", false)
        .unwrap()
        .id;
    let foreign = fb_core::services::save_account(
        &mut open.book,
        actor,
        other,
        fb_core::services::AccountInput::new("5000", "Foreign cost").detail("EXPENSE"),
    )
    .unwrap()
    .id
    .to_db();
    let path = format!("/c/{c}/lists/vendors/new/");
    let loc = saved(post(
        &mut app,
        &path,
        &[
            ("display_name", "Supplier"),
            ("default_expense_account", &account),
            ("default_terms", "30"),
        ],
    ));
    let edit = format!("{loc}edit/");
    saved(post(
        &mut app,
        &edit,
        &[("display_name", "Supplier"), ("default_terms", "")],
    ));
    let r = get(&mut app, &c, Model::Vendor, id(&loc));
    assert_eq!(r["default_expense_account"], account);
    assert!(r["default_terms"].is_null());
    let before = audit_count(&mut app);
    let Outcome::Invalid { html } = post(
        &mut app,
        &edit,
        &[
            ("display_name", "Supplier"),
            ("default_expense_account", &foreign),
        ],
    ) else {
        panic!("foreign account accepted")
    };
    assert!(html.contains("Select a valid choice."));
    assert_eq!(audit_count(&mut app), before);
    assert_eq!(
        get(&mut app, &c, Model::Vendor, id(&loc))["default_expense_account"],
        account
    );
}
#[test]
fn invalid_forms_do_not_write_and_schema_rejects_untyped_values() {
    let (mut app, c) = fixture();
    let before = audit_count(&mut app);
    for path in [
        format!("/c/{c}/lists/customers/new/"),
        format!("/c/{c}/lists/vendors/new/"),
        format!("/c/{c}/lists/items/new/"),
        format!("/c/{c}/sales-tax/rates/new/"),
        format!("/c/{c}/sales-tax/groups/new/"),
    ] {
        assert!(matches!(
            post(&mut app, &path, &[]),
            Outcome::Invalid { .. }
        ));
    }
    assert_eq!(audit_count(&mut app), before);
    assert!(fb_app::commands::parse_command(&json!({"type":"SaveDirectoryRecord","company_id":c,"record_id":null,"values":{"kind":"customer","display_name":"Test","sql":"DROP"}}).to_string()).is_err());
}
#[test]
fn a_full_tax_list_keeps_every_action_token_alive_across_cache_eviction() {
    let mut tokens = fb_app::session::Tokens::default();
    let old = tokens.mint("/old/");
    for _ in 0..900 {
        tokens.mint("/prior/");
    }
    let current = (0..500)
        .map(|n| {
            let path = format!("/sales-tax/{n}/status/");
            let token = tokens.mint(&path);
            (path, token)
        })
        .collect::<Vec<_>>();
    assert!(!tokens.consume(&old, "/old/"));
    for (path, token) in current {
        assert!(!tokens.consume(&token, "/other/"));
        assert!(tokens.consume(&token, &path));
        assert!(!tokens.consume(&token, &path));
    }
    let t = tokens.mint("/last/");
    tokens.clear();
    assert!(!tokens.consume(&t, "/last/"));
}

#[test]
fn company_list_follows_creation_order_even_when_a_later_name_sorts_first() {
    let (mut app, first) = fixture();
    let Outcome::Done { data } = app
        .run(Command::SaveCompany {
            company_id: None,
            name: "A later company".into(),
            currency: "USD".into(),
            starter_chart: fb_app::commands::StarterChart::Empty,
        })
        .unwrap()
    else {
        panic!("company not created")
    };
    let rows = app.query(fb_app::Query::Companies).unwrap();
    assert_eq!(rows[0]["id"], first);
    assert_eq!(rows[1]["id"], data["id"]);
}
