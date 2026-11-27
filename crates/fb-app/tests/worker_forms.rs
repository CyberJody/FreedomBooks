//! Bookkeeper test rounds of 2026-11-23: Company profile, items, deposits, sales receipts and bills
//! (items 4, 7, 8, 13, 15).
mod support;
mod worker_support;
use fb_app::commands::StarterChart;
use serde_json::json;
use worker_support::*;

/// Item 4: saving Company profile said nothing and jumped to "Finish setting up".
#[test]
fn saving_the_company_profile_stays_and_says_it_saved() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let path = format!("/c/{c}/edit/");
    assert_eq!(redirect(submit(&mut app, &path, &[("name", "Maple Lane Woodworks (synthetic)"), ("currency", "USD"), ("street1", "100 Test Road")])), path);
    let html = page(&mut app, &path);
    assert!(html.contains("Company profile saved."), "{html}");
    assert!(html.contains("Finish setting up"), "the setup checklist stays one click away while it is open");
}

/// Item 7: choosing Product ticked Taxable even with sales tax off.
#[test]
fn a_new_items_taxable_box_follows_its_type_only_while_sales_tax_is_on() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let path = format!("/c/{c}/lists/items/new/");
    assert!(!page(&mut app, &path).contains("directory/item_tax.js"));
    redirect(tax_on(&mut app, &c));
    assert!(page(&mut app, &path).contains("directory/item_tax.js"));
}

/// Item 8: a deposit picked 1000 Checking although the books were kept in another bank account.
#[test]
fn a_first_deposit_chooses_no_bank_when_there_are_several() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let checking = account(&mut app, &c, "1000");
    let path = format!("/c/{c}/deposits/new/");
    let bank_select = |html: &str| html.split_once("name=\"bank\"").unwrap().1.split("</select>").next().unwrap().to_owned();
    assert!(bank_select(&page(&mut app, &path)).contains(&format!("value=\"{checking}\" selected")), "one bank account is chosen");
    let test_checking = new_account(&mut app, &c, "1010", "Test Checking", "BANK");
    let select = bank_select(&page(&mut app, &path));
    assert!(select.contains(&format!("value=\"{test_checking}\"")) && !select.contains("\" selected>1"), "{select}");
}

/// Item 13: the sales receipt form could not add a new customer.
#[test]
fn a_customer_added_from_a_new_sales_receipt_comes_back_chosen() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let html = page(&mut app, &format!("/c/{c}/sales-receipts/new/"));
    let new = format!("/c/{c}/lists/customers/new/");
    assert!(html.contains(&format!("<a href=\"{new}?then=sales_receipt\">New customer</a>")), "{html}");
    let html = page(&mut app, &format!("{new}?then=sales_receipt"));
    assert!(html.contains("<input type=\"hidden\" name=\"then\" value=\"sales_receipt\">") && html.contains("you go back to the new sales receipt"), "{html}");
    let back = redirect(submit(&mut app, &new, &[("display_name", "Walk-in customer (synthetic)"), ("then", "sales_receipt")]));
    let id: String = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT id FROM directory_customer WHERE display_name='Walk-in customer (synthetic)'", [], |r| r.get(0))).unwrap();
    assert_eq!(back, format!("/c/{c}/sales-receipts/new/?customer={id}"));
    assert!(page(&mut app, &back).contains(&format!("value=\"{id}\" selected")));
    // Without it, a new customer opens its own page as before.
    let plain = redirect(submit(&mut app, &new, &[("display_name", "Another customer (synthetic)")]));
    assert!(plain.starts_with(&format!("/c/{c}/lists/customers/")), "{plain}");
}

/// Item 15: a bill with "No terms" demanded a due date.
#[test]
fn a_bill_with_no_due_date_is_due_on_its_date() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let timber = run(&mut app, json!({"type":"SaveDirectoryRecord","company_id":c,"record_id":null,"values":{"kind":"vendor","display_name":"Timber Supply (synthetic)","business_name":"","contact_name":"","email":"","phone":"","billing_address":"","notes":"","default_expense_account":null,"default_terms":null,"expense_shown":false,"terms_shown":false}}))["id"].as_str().unwrap().to_owned();
    let supplies = account(&mut app, &c, "6100");
    let path = format!("/c/{c}/bills/new/");
    let html = page(&mut app, &path);
    assert!(!html.split_once("name=\"due_date\"").unwrap().1.split('>').next().unwrap().contains("required"));
    let save = |app: &mut fb_app::App, terms: &str| -> (String, String) {
        let html = page(app, &path);
        let request = html.split_once("name=\"request_key\" value=\"").unwrap().1.split('"').next().unwrap().to_owned();
        let out = submit(app, &path, &[("request_key", &request), ("expected_revision", "0"), ("vendor", &timber), ("date", "2026-03-04"), ("terms", terms), ("due_date", ""),
            ("lines-TOTAL_FORMS", "1"), ("lines-INITIAL_FORMS", "0"), ("lines-0-expense_account", &supplies), ("lines-0-description", "Lumber"), ("lines-0-rate", "800.00"), ("editor_action", "save")]);
        let location = redirect(out);
        let id = location.trim_end_matches('/').rsplit('/').next().unwrap().to_owned();
        let due: String = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT r.due_date FROM payables_bill b JOIN payables_billrevision r ON r.bill_id=b.id AND r.number=b.revision WHERE b.id=?1", [&id], |r| r.get(0))).unwrap();
        (due, location)
    };
    let (due, location) = save(&mut app, "");
    assert_eq!(due, "2026-03-04");
    assert!(page(&mut app, &location).contains("Due date set to 2026-03-04, the bill date, because no terms were chosen."));
    // Review N1: with terms and a blank due date, it is the bill date plus the terms, and the bill's page
    // says so once.
    for term in ["0", "15", "30", "60"] {
        let (_, location) = save(&mut app, term);
        let edit = page(&mut app, &format!("{location}edit/"));
        let options = edit.split_once(r#"name="terms""#).unwrap().1.split("</select>").next().unwrap();
        assert!(options.contains(&format!(r#"value="{term}" selected"#)), "{options}");
    }
    let (due, location) = save(&mut app, "30");
    assert_eq!(due, "2026-04-03");
    let html = page(&mut app, &location);
    assert!(html.contains("<p class=\"notice\" role=\"status\">Due date set to 2026-04-03 (Net 30).</p>"), "{html}");
    assert!(!page(&mut app, &location).contains("Due date set to"));
    let (due, location) = save(&mut app, "0");
    assert_eq!(due, "2026-03-04");
    assert!(page(&mut app, &location).contains("Due date set to 2026-03-04 (Due on receipt)."));
    // A typed due date is kept, with no notice.
    let html = page(&mut app, &path);
    let request = html.split_once("name=\"request_key\" value=\"").unwrap().1.split('"').next().unwrap().to_owned();
    let location = redirect(submit(&mut app, &path, &[("request_key", &request), ("expected_revision", "0"), ("vendor", &timber), ("date", "2026-03-04"), ("terms", "30"), ("due_date", "2026-03-20"),
        ("lines-TOTAL_FORMS", "1"), ("lines-INITIAL_FORMS", "0"), ("lines-0-expense_account", &supplies), ("lines-0-description", "Lumber"), ("lines-0-rate", "800.00"), ("editor_action", "save")]));
    assert!(!page(&mut app, &location).contains("Due date set to"));
}
