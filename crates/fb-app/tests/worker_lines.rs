//! Bookkeeper test rounds of 2026-11-23: sales lines (items 6, 12); the page scripts are in
//! electron/tests/worker-fixes.test.cjs.
mod support;
mod worker_support;
use fb_app::Outcome;
use fb_app::commands::StarterChart;
use serde_json::{Value, json};
use worker_support::*;

/// Item 6: the sales line editor gets each item's name for an item saved without a description.
#[test]
fn the_sales_item_catalog_carries_the_item_name() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let sales = account(&mut app, &c, "4100");
    run(&mut app, json!({"type":"SaveItem","company_id":c,"code":"Custom shelving labor","description":"","kind":"SERVICE","sale_rate":"65.00","income_account_id":sales}));
    let html = page(&mut app, &format!("/c/{c}/invoices/new/"));
    let catalog = html.split_once("id=\"invoice-catalog\"").unwrap().1.split_once('>').unwrap().1.split("</script>").next().unwrap();
    let catalog: Value = serde_json::from_str(catalog).unwrap();
    let item = catalog.as_object().unwrap().values().next().unwrap();
    assert_eq!((item["description"].as_str(), item["code"].as_str()), (Some(""), Some("Custom shelving labor")));
}

/// Item 12: the negative-quantity error rendered inside the 76px quantity column.
#[test]
fn a_quantity_error_renders_across_the_line() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let birch = customer(&mut app, &c, "Birch Street Bakery (synthetic)");
    let sales = account(&mut app, &c, "4100");
    let path = format!("/c/{c}/invoices/new/");
    let html = page(&mut app, &path);
    let request = html.split_once("name=\"request_key\" value=\"").unwrap().1.split('"').next().unwrap().to_owned();
    let Outcome::Invalid { html } = submit(&mut app, &path, &[("request_key", &request), ("customer", &birch), ("date", "2026-11-23"), ("terms", "30"), ("due_date", "2026-12-23"), ("tax_mode", "NONE"),
        ("lines-TOTAL_FORMS", "1"), ("lines-INITIAL_FORMS", "0"), ("lines-0-description", "Negative qty test"), ("lines-0-quantity", "-1"), ("lines-0-rate", "150.00"), ("lines-0-income_account", &sales),
        ("editor_action", "save")]) else { panic!("a negative quantity saved") };
    let row = html.split_once("<legend>Line 1</legend>").unwrap().1.split("</fieldset>").next().unwrap();
    let quantity_cell = row.split_once("data-field=\"quantity\"").unwrap().1.split("</div>").next().unwrap();
    assert!(!quantity_cell.contains("errorlist"), "{quantity_cell}");
    let after_cells = row.split_once("data-remove-line").unwrap().1;
    assert!(after_cells.contains("id=\"id_lines-0-quantity_error\""), "{row}");
}
