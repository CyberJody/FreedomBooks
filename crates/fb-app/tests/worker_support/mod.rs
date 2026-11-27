//! Shared helpers for the bookkeeper-round regressions (worker_setup, worker_journals, worker_lines,
//! worker_forms): a real encrypted book made the way first run makes it, and form posts with real tokens.
#![allow(dead_code)]
use fb_app::commands::{StarterChart, parse_command};
use fb_app::{App, Command, Outcome, keys, screens};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};

static N: AtomicUsize = AtomicUsize::new(0);

pub fn temp_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("fb-worker-fixes-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn fixture(chart: StarterChart) -> (App, String) {
    let dir = temp_dir();
    let mut app = App::new();
    app.today = || chrono::NaiveDate::from_ymd_opt(2026, 11, 23).unwrap();
    let c = app
        .create_book_with_chart(&dir.join("book.fbbook"), &keys::new_master_key().unwrap(), "Pat Tester", "Maple Lane Woodworks (synthetic)", chart)
        .unwrap()
        .company_id
        .to_db();
    crate::support::sign_in(&mut app);
    (app, c)
}
pub fn run(app: &mut App, v: Value) -> Value {
    match app.run(parse_command(&v.to_string()).unwrap()).unwrap_or_else(|e| panic!("{v}: {e}")) {
        Outcome::Done { data } => data,
        o => panic!("{v}: {o:?}"),
    }
}
pub fn page(app: &mut App, path: &str) -> String {
    let r = screens::render(app, path).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert_eq!(r.status, 200, "{path}: {}", r.html);
    r.html
}
pub fn submit(app: &mut App, path: &str, fields: &[(&str, &str)]) -> Outcome {
    let token = app.tokens.mint(path);
    app.run(Command::SubmitForm { path: path.into(), token, fields: fields.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }).unwrap()
}
pub fn redirect(o: Outcome) -> String {
    match o {
        Outcome::Redirect { location } => location,
        o => panic!("expected a redirect: {o:?}"),
    }
}
pub fn account(app: &mut App, c: &str, code: &str) -> String {
    app.open_mut().unwrap().book.read(|q| q.query_row("SELECT id FROM ledger_account WHERE company_id=?1 AND code=?2", [c, code], |r| r.get(0))).unwrap()
}
pub fn new_account(app: &mut App, c: &str, code: &str, name: &str, detail: &str) -> String {
    run(app, json!({"type":"SaveAccount","company_id":c,"code":code,"name":name,"detail_type":detail}))["id"].as_str().unwrap().to_owned()
}
pub fn customer(app: &mut App, c: &str, name: &str) -> String {
    run(app, json!({"type":"SaveCustomer","company_id":c,"display_name":name}))["id"].as_str().unwrap().to_owned()
}
pub fn key() -> String {
    uuid::Uuid::new_v4().to_string()
}
pub fn invoice(app: &mut App, c: &str, customer: &str, income: &str, tax: Option<&str>) -> String {
    let mut values = json!({"request_key":key(),"customer_id":customer,"date":"2026-03-02","due_date":"2026-04-01","memo":"","tax_mode":"NONE",
        "lines":[{"description":"Custom shelving labor","quantity":"10","rate":"65.00","taxable":true,"income_account_id":income}]});
    if let Some(rate) = tax {
        values["tax_mode"] = json!("MANUAL");
        values["tax_label"] = json!("State tax");
        values["tax_rate"] = json!(rate);
    }
    run(app, json!({"type":"SaveInvoice","company_id":c,"values":values}))["id"].as_str().unwrap().to_owned()
}
pub fn post_invoice(app: &mut App, c: &str, id: &str) -> Result<Outcome, fb_app::AppError> {
    app.run(parse_command(&json!({"type":"PostInvoice","company_id":c,"invoice_id":id,"expected_revision":1}).to_string()).unwrap())
}
pub fn invoice_state(app: &mut App, id: &str) -> String {
    app.open_mut().unwrap().book.read(|q| q.query_row("SELECT r.state FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.number=i.revision WHERE i.id=?1", [id], |r| r.get(0))).unwrap()
}
pub fn tax_on(app: &mut App, c: &str) -> Outcome {
    let path = format!("/c/{c}/sales-tax/");
    let html = page(app, &path);
    let revision = html.split_once("name=\"expected_revision\" value=\"").unwrap().1.split('"').next().unwrap().to_owned();
    submit(app, &path, &[("expected_revision", &revision), ("charge_tax", "yes"), ("default_item", ""), ("owed_basis", "ACCRUAL"), ("filing_frequency", "MONTHLY")])
}
