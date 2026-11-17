//! Real form commands against encrypted books: omission compatibility, scoping, no-write errors.
mod support;
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
    support::sign_in(&mut app);
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
fn contact_notes_save_edit_and_render_line_breaks_without_executing_markup() {
    let (mut app, c) = fixture();
    for (list, model) in [("customers", Model::Customer), ("vendors", Model::Vendor)] {
        let notes = "First line\n\n<script>alert(1)</script>\r\nLast line\rFinal line";
        let path = format!("/c/{c}/lists/{list}/new/");
        let loc = saved(post(&mut app, &path,
            &[("display_name", "Synthetic contact"), ("notes", notes)]));
        assert_eq!(get(&mut app, &c, model, id(&loc))["notes"], notes);
        let html = screens::render(&mut app, &loc).unwrap().html;
        assert!(html.contains("First line<br><br>&lt;script&gt;alert(1)&lt;/script&gt;<br>Last line<br>Final line"));
        assert!(!html.contains("<script>alert(1)</script>"));
        let edit = format!("{loc}edit/");
        let html = screens::render(&mut app, &edit).unwrap().html;
        assert!(html.contains("First line\n\n&lt;script&gt;alert(1)&lt;/script&gt;"));
        let revised = "First line\n\nUpdated context";
        saved(post(&mut app, &edit,
            &[("display_name", "Synthetic contact"), ("notes", revised)]));
        let before = get(&mut app, &c, model, id(&loc));
        assert_eq!(before["notes"], revised);
        let audits = audit_count(&mut app);
        for invalid in ["\u{85}Hidden\ncontext".to_owned(), "\nBad\u{202e}direction".into(), "é".repeat(4001)] {
            assert!(matches!(post(&mut app, &edit,
                &[("display_name", "Synthetic contact"), ("notes", &invalid)]), Outcome::Invalid { .. }));
            assert_eq!(get(&mut app, &c, model, id(&loc)), before);
            assert_eq!(audit_count(&mut app), audits);
        }
    }
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

#[test]
fn guided_sales_tax_is_atomic_scoped_and_preserves_existing_work() {
    let (mut app,c)=fixture();
    let company=c.parse().unwrap();
    let o=app.open_mut().unwrap();
    let before=fb_db::test_support::snapshot(o.book.connection()).unwrap();
    let input=json!({"expected_revision":0,"charge_tax":true,"name":"Example state and local","rate":"7","authority_name":"Example tax authority"});
    let mut invalid=input.clone();invalid["rate"]=json!("101");
    assert!(fb_core::salestax::configure(&mut o.book,o.owner,company,&invalid).is_err());
    assert_eq!(fb_db::test_support::snapshot(o.book.connection()).unwrap(),before);
    let settings=fb_core::salestax::configure(&mut o.book,o.owner,company,&input).unwrap();
    assert_eq!(settings["charge_tax"],true);
    assert_eq!(settings["owed_basis"],"ACCRUAL");assert_eq!(settings["filing_frequency"],"MONTHLY");
    let after=fb_db::test_support::snapshot(o.book.connection()).unwrap();
    assert!(fb_core::salestax::configure(&mut o.book,o.owner,company,&input).is_err());
    assert_eq!(fb_db::test_support::snapshot(o.book.connection()).unwrap(),after);
    let (rates,vendors)=o.book.read(|tx|Ok::<_,fb_core::FbError>((fb_core::salestax::list(tx,company)?,directory::list(tx,company,Model::Vendor)?))).unwrap();
    assert_eq!(rates.len(),1);assert_eq!(vendors.len(),1);assert_eq!(rates[0]["rate"],"7.0000");
    // Same authority name uses the existing payee rather than creating duplicates.
    let second=fb_core::salestax::configure(&mut o.book,o.owner,company,&json!({"expected_revision":1,"charge_tax":true,"name":"Another local rate","rate":"8.25","authority_name":"EXAMPLE TAX AUTHORITY"})).unwrap();
    assert_eq!(second["revision"],2);
    assert_eq!(o.book.read(|tx|directory::list(tx,company,Model::Vendor)).unwrap().len(),1);
    // A new rate followed by invalid settings must roll back the payee, rate and audit.
    let mut bad=input.clone();bad["expected_revision"]=json!(2);bad["name"]=json!("Another local rate");bad["authority_name"]=json!("New uncommitted authority");
    let snapshot=fb_db::test_support::snapshot(o.book.connection()).unwrap();
    assert!(fb_core::salestax::configure(&mut o.book,o.owner,company,&bad).is_err());
    assert_eq!(fb_db::test_support::snapshot(o.book.connection()).unwrap(),snapshot);
    let journals:i64=o.book.read(|tx|tx.query_row("SELECT count(*) FROM ledger_journal",[],|r|r.get(0))).unwrap();assert_eq!(journals,0);
}

#[test]
fn first_company_home_asks_sales_tax_and_guided_form_sets_the_default() {
    let (mut app,c)=fixture();
    let home=screens::render(&mut app,&format!("/c/{c}/home/")).unwrap().html;
    assert!(home.contains("Do you collect sales tax?"));
    let start=screens::render(&mut app,&format!("/c/{c}/setup/start/")).unwrap().html;
    assert!(start.contains("Do you collect sales tax?"));
    let path=format!("/c/{c}/sales-tax/setup/");
    let invalid=post(&mut app,&path,&[("expected_revision","0"),("charge_tax","yes"),("name","Example tax"),("rate","7"),("authority_name","")]);
    let Outcome::Invalid{html}=invalid else {panic!("missing authority should stay on form")};assert!(html.contains("Enter a tax authority name"));
    let location=saved(post(&mut app,&path,&[("expected_revision","0"),("charge_tax","yes"),("name","Example tax"),("rate","7"),("authority_name","Example authority")]));
    assert_eq!(location,format!("/c/{c}/setup/start/"));
    let invoice=screens::render(&mut app,&format!("/c/{c}/invoices/new/")).unwrap().html;
    assert!(invoice.contains("Example tax · 7%"));assert!(!invoice.contains("tax is turned off"));
    let home=screens::render(&mut app,&format!("/c/{c}/home/")).unwrap().html;assert!(!home.contains("Do you collect sales tax?"));
}

#[test]
fn explicit_no_tax_requires_no_authority_and_wrong_company_rate_rolls_back() {
    let (mut app,c)=fixture();let company=c.parse().unwrap();
    let location=saved(post(&mut app,&format!("/c/{c}/sales-tax/setup/"),&[("expected_revision","0"),("charge_tax","no")]));assert!(location.ends_with("/setup/start/"));
    let o=app.open_mut().unwrap();
    assert!(o.book.read(|tx|directory::list(tx,company,Model::Vendor)).unwrap().is_empty());
    let other=fb_core::services::create_company(&mut o.book,o.owner,"Other tax company","USD",false).unwrap();
    let foreign=fb_core::salestax::save_rate(&mut o.book,o.owner,other.id,&json!({"name":"Foreign rate","description":"Sales tax","rate":"8","new_agency_name":"Other authority"})).unwrap();
    let snapshot=fb_db::test_support::snapshot(o.book.connection()).unwrap();
    assert!(fb_core::salestax::configure(&mut o.book,o.owner,company,&json!({"expected_revision":1,"charge_tax":true,"default_item_id":foreign["id"]})).is_err());
    assert_eq!(fb_db::test_support::snapshot(o.book.connection()).unwrap(),snapshot);
}

#[test]
fn guided_tax_failure_after_rate_creation_rolls_back_every_write() {
    let (mut app,c)=fixture();let company=c.parse().unwrap();let o=app.open_mut().unwrap();
    fb_core::salestax::save_settings(&mut o.book,o.owner,company,&json!({"expected_revision":0,"charge_tax":false,"default_item_id":null,"owed_basis":"ACCRUAL","filing_frequency":"MONTHLY"})).unwrap();
    // Exhausted settings revisions fail after the new rate has been prepared.
    o.book.write(|tx|tx.execute("UPDATE salestax_salestaxsettings SET revision=?1 WHERE company_id=?2",fb_db::rusqlite::params![i64::MAX,c])).unwrap();
    let before=fb_db::test_support::snapshot(o.book.connection()).unwrap();
    assert!(fb_core::salestax::configure(&mut o.book,o.owner,company,&json!({"expected_revision":i64::MAX,"charge_tax":true,"name":"Example tax","rate":"7","authority_name":"Example authority"})).is_err());
    assert_eq!(fb_db::test_support::snapshot(o.book.connection()).unwrap(),before);
    assert!(o.book.read(|tx|directory::list(tx,company,Model::Vendor)).unwrap().is_empty());
    assert!(o.book.read(|tx|fb_core::salestax::list(tx,company)).unwrap().is_empty());
}

#[test]
fn item_form_puts_type_first_and_asks_for_an_income_account_only_for_items_you_sell() {
    let (mut app, c) = fixture();
    let o = app.open_mut().unwrap();
    let co = c.parse().unwrap();
    let income = fb_core::services::save_account(&mut o.book, o.owner, co, fb_core::services::AccountInput::new("4000", "Sales").detail("INCOME")).unwrap().id.to_db();
    let expense = fb_core::services::save_account(&mut o.book, o.owner, co, fb_core::services::AccountInput::new("5000", "Supplies").detail("EXPENSE")).unwrap().id.to_db();
    let path = format!("/c/{c}/lists/items/new/");
    let html = screens::render(&mut app, &path).unwrap().html;
    let kind = html.find("name=\"kind\"").unwrap();
    assert!(kind < html.find("name=\"code\"").unwrap() && kind < html.find("name=\"sale_rate\"").unwrap());
    for gone in ["99999999999999", "Rounded half-up", "nonnegative", "Defaults do not post", "No balances or transactions are changed"] {
        assert!(!html.contains(gone), "{gone}");
    }
    // A priced item without an income account is refused, and nothing is written.
    let Outcome::Invalid { html } = post(&mut app, &path, &[("kind", "SERVICE"), ("code", "MOW"), ("sale_rate", "45.00"), ("income_account", ""), ("expense_account", "")]) else { panic!() };
    assert!(html.contains("Choose the income account this item&#x27;s sales are recorded in.") || html.contains("Choose the income account this item's sales are recorded in."), "{html}");
    assert!(html.contains("value=\"MOW\""));
    let items = app.open_mut().unwrap().book.read(|tx| directory::list(tx, co, Model::Item)).unwrap();
    assert!(items.is_empty());
    // A sold item with its income account saves; so does an item you only buy.
    saved(post(&mut app, &path, &[("kind", "SERVICE"), ("code", "MOW"), ("sale_rate", "45.00"), ("income_account", &income), ("expense_account", "")]));
    saved(post(&mut app, &path, &[("kind", "NONSTOCK"), ("code", "GLOVES"), ("sale_rate", "0"), ("income_account", ""), ("expense_account", &expense)]));
}
