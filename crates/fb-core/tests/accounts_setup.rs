//! Bookkeeper test rounds of 2026-11-23: account names are unique in a company (item 2); a standard-chart
//! company starts with its default accounts, and turning sales tax on chooses collected tax (items 5, 17).
mod common;
use common::*;
use fb_core::services::{self, AccountInput};
use fb_core::{CompanyId, FbError, documents, salestax, starter_chart::Choice, store};
use serde_json::{Value, json};

fn messages(e: FbError) -> String {
    match e {
        FbError::Validation(m) => m.join("; "),
        other => panic!("expected a validation error, got {other:?}"),
    }
}

fn account_count(f: &mut Fx, c: CompanyId) -> i64 {
    f.book
        .read(|tx| tx.query_row("SELECT count(*) FROM ledger_account WHERE company_id=?1", [c.to_db()], |r| r.get(0)))
        .unwrap()
}

/// Item 2: a second "Shop Supplies" saved silently (6105, 6106, 6107 in the test company).
#[test]
fn a_new_or_renamed_account_cannot_take_another_accounts_name() {
    let mut f = Fx::new("duplicate-account-name");
    let c = f.company("Synthetic Woodworks");
    let other = f.company("Other company");
    let supplies = f.account(c, "6105", "Shop Supplies", "EXPENSE");
    let before = account_count(&mut f, c);
    for name in ["Shop Supplies", "shop supplies", "  SHOP SUPPLIES  "] {
        let e = services::save_account(&mut f.book, f.user, c, AccountInput::new("6106", name).kind("EXPENSE")).unwrap_err();
        assert_eq!(messages(e), "An account named “Shop Supplies” already exists (6105). Choose a different name.", "{name:?}");
    }
    assert_eq!(account_count(&mut f, c), before, "nothing was saved");
    // Another company may use the name.
    f.account(other, "6105", "Shop Supplies", "EXPENSE");
    let tools = f.account(c, "6106", "Shop Tools", "EXPENSE");
    // Renaming onto an existing name is refused too; renaming to a new name or changing only case is not.
    let rename = |name: &str| AccountInput::new("6106", name).kind("EXPENSE").edit(tools);
    let e = services::save_account(&mut f.book, f.user, c, rename("Shop supplies")).unwrap_err();
    assert!(messages(e).contains("already exists (6105)"));
    services::save_account(&mut f.book, f.user, c, rename("Shop tools")).unwrap();
    services::save_account(&mut f.book, f.user, c, AccountInput::new("6105", "shop Supplies").kind("EXPENSE").edit(supplies)).unwrap();
    // An inactive account still holds its name.
    services::save_account(&mut f.book, f.user, c, AccountInput::new("6105", "shop Supplies").kind("EXPENSE").active(false).edit(supplies)).unwrap();
    assert!(services::save_account(&mut f.book, f.user, c, AccountInput::new("6107", "Shop Supplies").kind("EXPENSE")).is_err());
}

/// Item 2: duplicates made before the rule stay, and each can still be edited while it keeps its name.
#[test]
fn existing_duplicate_names_are_left_alone_and_stay_editable() {
    let mut f = Fx::new("existing-duplicates");
    let c = f.company("Synthetic Woodworks");
    let first = f.account(c, "6105", "Shop Supplies", "EXPENSE");
    let second = f.account(c, "6106", "Second", "EXPENSE");
    // A book from before the rule: write the duplicate directly, as the old app could.
    f.book
        .connection()
        .execute("UPDATE ledger_account SET name='Shop Supplies' WHERE id=?1", [second.to_db()])
        .unwrap();
    let edited = services::save_account(
        &mut f.book,
        f.user,
        c,
        AccountInput::new("6106", "Shop Supplies").kind("EXPENSE").description("Kept from before").edit(second),
    )
    .unwrap();
    assert_eq!((edited.name.as_str(), edited.description.as_str()), ("Shop Supplies", "Kept from before"));
    services::save_account(&mut f.book, f.user, c, AccountInput::new("6105", "Shop Supplies").kind("EXPENSE").active(false).edit(first)).unwrap();
}

fn setup(f: &mut Fx, c: CompanyId) -> Value {
    f.book.read(|tx| documents::setup_snapshot(tx, c)).unwrap()
}

fn code_of(f: &mut Fx, id: &Value) -> String {
    let id = fb_core::AccountId::parse(id.as_str().expect("an account is chosen")).unwrap();
    f.book.read(|tx| store::account(tx, id)).unwrap().unwrap().code
}

fn tax_on(f: &mut Fx, c: CompanyId) -> Value {
    let revision = f.book.read(|tx| salestax::settings_for(tx, c)).unwrap()["revision"].clone();
    let input = json!({"expected_revision":revision,"charge_tax":true,"default_item_id":null,"owed_basis":"ACCRUAL","filing_frequency":"MONTHLY"});
    salestax::save_settings(&mut f.book, f.user, c, &input).unwrap().0
}

/// Items 5 and 17: a new company with the standard chart could not post its first invoice (no
/// receivables or undeposited funds chosen) nor its first taxable invoice (no collected tax).
#[test]
fn a_new_standard_chart_company_starts_with_its_default_accounts_chosen() {
    let mut f = Fx::new("standard-chart-setup");
    let c = services::create_company_with_chart(&mut f.book, f.user, "Synthetic Woodworks", "USD", true, Choice::Standard).unwrap().id;
    let s = setup(&mut f, c);
    let mut chosen = vec![];
    for role in ["receivables", "payables", "undeposited", "collected_tax"] {
        chosen.push((role, code_of(&mut f, &s[role])));
    }
    assert_eq!(chosen, [("receivables", "1100".to_string()), ("payables", "2000".into()), ("undeposited", "1200".into()), ("collected_tax", "2100".into())]);
    assert!(s["customer_deposits"].is_null());
    let banks = s["banks"].as_array().unwrap().clone();
    assert_eq!(banks.len(), 1);
    assert_eq!(code_of(&mut f, &banks[0]), "1000");
    // The posting checks a first invoice, payment and taxable invoice run now pass.
    f.book
        .write(|tx| documents::lock_configuration(tx, f.user, c, &["receivables", "undeposited", "collected_tax", "payables"], None, &[]).map(|_| ()))
        .unwrap();
    // Audited as a setup fill in the creation transaction; nothing posted.
    assert_eq!(f.audit_count("setup.filled"), 1);
    assert_eq!(f.count(&format!("SELECT count(*) FROM ledger_journal WHERE company_id='{}'", c.to_db())), 0);
    // A company with an empty chart is left unconfigured.
    let empty = services::create_company_with_chart(&mut f.book, f.user, "Empty", "USD", true, Choice::Empty).unwrap().id;
    assert_eq!(setup(&mut f, empty)["revision"], 0);
    // Turning sales tax on later leaves the chosen collected-tax account as it is.
    let before = setup(&mut f, c);
    assert!(tax_on(&mut f, c).get("collected_tax_set").is_none());
    assert_eq!(setup(&mut f, c), before);
}

/// Item 17: turning sales tax on with no collected-tax account chosen chooses the chart's one sales
/// tax liability account; otherwise it leaves Accounting setup alone and tax is still turned on.
#[test]
fn turning_sales_tax_on_chooses_the_charts_collected_tax_account() {
    let mut f = Fx::new("tax-on-setup");
    // An existing company: receivables chosen by hand, no collected tax.
    let c = f.company("Synthetic Print Shop");
    let ar = f.account(c, "1100", "Accounts Receivable", "ASSET");
    let first = documents::save_setup(&mut f.book, f.user, c, &json!({"receivables": ar.to_db()}), Some(0)).unwrap();
    let tax = services::save_account(&mut f.book, f.user, c, AccountInput::new("2100", "Sales Tax Payable").detail("OTHER_CURRENT_LIABILITY")).unwrap();
    let audits = f.audit_count("accounting.setup");
    let settings = tax_on(&mut f, c);
    assert_eq!(settings["charge_tax"], true);
    assert_eq!(settings["collected_tax_set"], "2100 · Sales Tax Payable");
    let after = setup(&mut f, c);
    assert_eq!(after["collected_tax"], tax.id.to_db());
    assert_eq!(after["receivables"], ar.to_db(), "other roles are kept");
    assert_eq!(after["revision"], first["revision"].as_i64().unwrap() + 1);
    assert_eq!(f.audit_count("accounting.setup"), audits + 1);
    // The stored settings do not carry the notice.
    assert!(f.book.read(|tx| salestax::settings_for(tx, c)).unwrap().get("collected_tax_set").is_none());

    // No clear sales tax account (two of them): nothing is chosen, and tax still turns on.
    let g = f.company("Two tax accounts");
    for (code, name) in [("2100", "Sales Tax Payable"), ("2110", "Sales tax collected - county")] {
        services::save_account(&mut f.book, f.user, g, AccountInput::new(code, name).detail("OTHER_CURRENT_LIABILITY")).unwrap();
    }
    let settings = tax_on(&mut f, g);
    assert_eq!((settings["charge_tax"].clone(), settings.get("collected_tax_set").cloned()), (json!(true), None));
    assert!(setup(&mut f, g)["collected_tax"].is_null());

    // A setup that cannot be saved as it stands (an inactive bank kept on its list) is left alone.
    let h = f.company("Unavailable bank");
    let bank = f.account(h, "1000", "Old bank", "ASSET");
    documents::save_setup(&mut f.book, f.user, h, &json!({"banks": [bank.to_db()]}), Some(0)).unwrap();
    services::save_account(&mut f.book, f.user, h, AccountInput::new("1000", "Old bank").kind("ASSET").active(false).edit(bank)).unwrap();
    services::save_account(&mut f.book, f.user, h, AccountInput::new("2100", "Sales Tax Payable").detail("OTHER_CURRENT_LIABILITY")).unwrap();
    let before = setup(&mut f, h);
    let settings = tax_on(&mut f, h);
    assert_eq!(settings["charge_tax"], true);
    assert!(settings.get("collected_tax_set").is_none());
    assert_eq!(setup(&mut f, h), before);
}
