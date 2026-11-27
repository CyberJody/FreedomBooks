//! Bookkeeper test rounds of 2026-11-23: the chart of accounts and Accounting setup (items 2, 5, 16, 17).
mod support;
mod worker_support;
use fb_app::Outcome;
use fb_app::commands::StarterChart;
use serde_json::json;
use worker_support::*;

/// Items 5 and 17: a new company with the standard chart posted nothing at first: the first invoice
/// stayed a draft ("Configure receivables in Accounting setup first"), and so did the first taxable one.
#[test]
fn a_standard_chart_companys_first_invoice_payment_and_taxable_invoice_post() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let harbor = customer(&mut app, &c, "Harbor Cafe (synthetic)");
    let sales = account(&mut app, &c, "4100");
    let first = invoice(&mut app, &c, &harbor, &sales, None);
    post_invoice(&mut app, &c, &first).unwrap();
    assert_eq!(invoice_state(&mut app, &first), "OPEN");
    // The payment goes to Undeposited Funds, chosen at creation.
    let receipt = json!({"request_key":key(),"customer_id":harbor,"date":"2026-03-10","method":"CHECK","reference":"1001","memo":"","destination":"UNDEPOSITED","bank_id":null,"amount":"650.00","allocations":[{"invoice_id":first,"amount":"650.00"}]});
    let review = run(&mut app, json!({"type":"ReviewCustomerReceipt","company_id":c,"sales_receipt":false,"values":receipt}));
    run(&mut app, json!({"type":"RecordCustomerReceipt","company_id":c,"sales_receipt":false,"values":receipt,"review_token":review["token"]}));
    // Turning sales tax on changes nothing in the setup (2100 is already chosen), and says nothing about it.
    let next = redirect(tax_on(&mut app, &c));
    assert!(!page(&mut app, &next).contains("Sales tax you collect goes to"));
    let taxed = invoice(&mut app, &c, &harbor, &sales, Some("6.25"));
    post_invoice(&mut app, &c, &taxed).unwrap();
    assert_eq!(invoice_state(&mut app, &taxed), "OPEN");
}

/// Items 5 and 17 for a company that existed before (empty chart, nothing chosen): its draft links straight
/// to the one setting it needs, and turning sales tax on chooses the chart's sales tax account and says so.
#[test]
fn an_existing_companys_draft_links_to_the_missing_setting_and_tax_on_chooses_collected_tax() {
    let (mut app, c) = fixture(StarterChart::Empty);
    let ar = new_account(&mut app, &c, "1100", "Accounts Receivable", "RECEIVABLE");
    let sales = new_account(&mut app, &c, "4100", "Sales", "INCOME");
    let tax = new_account(&mut app, &c, "2100", "Sales Tax Payable", "OTHER_CURRENT_LIABILITY");
    let bakery = customer(&mut app, &c, "Birch Street Bakery (synthetic)");
    let draft = invoice(&mut app, &c, &bakery, &sales, None);
    assert!(post_invoice(&mut app, &c, &draft).is_err());
    let html = page(&mut app, &format!("/c/{c}/invoices/{draft}/"));
    assert!(html.contains("Configure receivables in Accounting setup first."), "{html}");
    assert!(html.contains(&format!("<a href=\"/c/{c}/accounting-setup/#id_receivables\">Choose the Receivables account in Accounting setup</a>")), "{html}");
    assert!(page(&mut app, &format!("/c/{c}/accounting-setup/")).contains("id=\"id_receivables\""));
    run(&mut app, json!({"type":"SaveSetup","company_id":c,"expected_revision":0,"receivables_id":ar,"payables_id":null,"undeposited_id":null,"collected_tax_id":null,"customer_deposits_id":null,"bank_ids":[]}));
    let taxed = invoice(&mut app, &c, &bakery, &sales, Some("6.25"));
    let html = page(&mut app, &format!("/c/{c}/invoices/{taxed}/"));
    assert!(html.contains(&format!("<a href=\"/c/{c}/accounting-setup/#id_collected_tax\">Choose the Collected tax account in Accounting setup</a>")), "{html}");
    // Turning sales tax on now chooses 2100 and says so on the next page.
    let next = redirect(tax_on(&mut app, &c));
    let html = page(&mut app, &next);
    assert!(html.contains("Sales tax you collect goes to 2100 · Sales Tax Payable. You can change this in Accounting setup."), "{next}: {html}");
    let (chosen, receivables): (String, String) = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT collected_tax_id, receivables_id FROM documents_accountingsetup WHERE company_id=?1", [&c], |r| Ok((r.get(0)?, r.get(1)?)))).unwrap();
    assert_eq!((chosen, receivables), (tax, ar), "collected tax chosen, receivables kept");
    post_invoice(&mut app, &c, &taxed).unwrap();
    assert_eq!(invoice_state(&mut app, &taxed), "OPEN");
}

/// Item 2: a second "Shop Supplies" saved without a word from the account form.
#[test]
fn the_account_form_refuses_a_name_another_account_has() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let path = format!("/c/{c}/accounts/new/");
    let before: i64 = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT count(*) FROM ledger_account", [], |r| r.get(0))).unwrap();
    let Outcome::Invalid { html } = submit(&mut app, &path, &[("code", "6105"), ("name", " supplies "), ("detail_type", "EXPENSE"), ("active", "on"), ("postable", "on")]) else {
        panic!("a duplicate name saved")
    };
    assert!(html.contains("An account named “Supplies” already exists (6100). Choose a different name."), "{html}");
    let after: i64 = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT count(*) FROM ledger_account", [], |r| r.get(0))).unwrap();
    assert_eq!(after, before);
    redirect(submit(&mut app, &path, &[("code", "6105"), ("name", "Shop Supplies"), ("detail_type", "EXPENSE"), ("active", "on"), ("postable", "on")]));
}

/// Item 16: 4900 Other Income was listed after the expense accounts.
#[test]
fn the_chart_lists_other_income_with_income() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let html = page(&mut app, &format!("/c/{c}/accounts/"));
    let at = |code: &str| html.find(&format!("data-code=\"{code}\"")).unwrap_or_else(|| panic!("{code}"));
    assert!(at("4200") < at("4900") && at("4900") < at("5000") && at("5000") < at("6000"), "{html}");
    assert!(at("3200") < at("4000") && at("1000") < at("2000"));
}
