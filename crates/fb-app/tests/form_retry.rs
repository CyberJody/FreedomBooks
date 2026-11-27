//! CRASHLOG: a form post that fails after spending its single-use token, on the "one Save" forms
//! that review and record back to back in one request (Receive payment, Sales receipt). Whatever
//! step fails, a resubmit can never record a second time: a failure inside the review commits
//! nothing and gets a fresh token (its resubmit records once); a failure after the review committed
//! gets no token; and a recorded form sent again with the same request key shows the first one.
//! Failures are injected by the test-only `panic-test` feature (fb-app/fb-db `test_panic`).
#![cfg(feature = "panic-test")]
mod support;
use fb_app::{App, Command, Outcome, keys, screens, test_panic};
use fb_core::services;
use serde_json::json;
use std::sync::Mutex;

// The injection points are process-wide; these tests take turns.
static TURN: Mutex<()> = Mutex::new(());

struct Fixture {
    app: App,
    c: String,
    customer: String,
    income: String,
    _turn: std::sync::MutexGuard<'static, ()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        test_panic::disarm();
    }
}

impl Fixture {
    fn new() -> Self {
        let turn = TURN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        test_panic::disarm();
        let mut app = App::new();
        let path = std::env::temp_dir().join(format!("fb-app-retry-{}.fbbook", uuid::Uuid::new_v4()));
        let c = app.create_book(&path, &keys::new_master_key().unwrap(), "Owner", "Synthetic retry forms").unwrap().company_id;
        support::sign_in(&mut app);
        let o = app.open_mut().unwrap();
        let mut account = |code: &str, kind: &str| {
            let mut v = services::AccountInput::new(code, code);
            v.kind = Some(kind.into());
            services::save_account(&mut o.book, o.owner, c, v).unwrap().id.to_db()
        };
        let ar = account("1100", "ASSET");
        let uf = account("1200", "ASSET");
        let bank = account("1000", "ASSET");
        let income = account("4000", "INCOME");
        fb_core::documents::save_setup(&mut o.book, o.owner, c, &json!({"receivables":ar,"undeposited":uf,"banks":[bank]}), Some(0)).unwrap();
        let customer = fb_core::directory::save_record(&mut o.book, o.owner, c, fb_core::directory::Model::Customer, &json!({"display_name":"Synthetic customer"}), None)
            .unwrap()["id"].as_str().unwrap().into();
        Self { app, c: c.to_db(), customer, income, _turn: turn }
    }

    fn posted_invoice(&mut self, rate: &str) -> String {
        let values = serde_json::from_value(json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":self.customer,"date":"2026-10-01","due_date":"2026-11-01","tax_mode":"NONE","lines":[{"description":"Work","quantity":"1","rate":rate,"income_account_id":self.income}]})).unwrap();
        let Outcome::Done { data } = self.app.run(Command::SaveInvoiceDraft { company_id: self.c.clone(), invoice_id: None, expected_revision: 0, values }).unwrap() else { panic!() };
        let id: String = data["id"].as_str().unwrap().into();
        self.app.run(Command::PostInvoice { company_id: self.c.clone(), invoice_id: id.clone(), expected_revision: 1 }).unwrap();
        id
    }

    fn count(&mut self, kind: &str) -> i64 {
        self.app.open_mut().unwrap().book.read(|tx| tx.query_row("SELECT count(*) FROM sales_receipt WHERE kind=?1", [kind], |r| r.get(0))).unwrap()
    }
}

fn field(html: &str, name: &str) -> String {
    html.split_once(&format!("name=\"{name}\" value=\"")).unwrap_or_else(|| panic!("missing {name}")).1.split('"').next().unwrap().into()
}

fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
    p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn payment(key: &str, customer: &str, invoice: &str) -> Vec<(String, String)> {
    pairs(&[
        ("request_key", key), ("customer", customer), ("date", "2026-10-20"), ("amount", "40.00"), ("method", "CHECK"), ("reference", "1042"),
        ("destination", "UNDEPOSITED"), ("action", "save"), ("allocations-TOTAL_FORMS", "1"), ("allocations-0-invoice", invoice),
        ("allocations-0-pay", "on"), ("allocations-0-amount", "40.00"),
    ])
}

fn sale(key: &str, customer: &str, income: &str) -> Vec<(String, String)> {
    pairs(&[
        ("request_key", key), ("customer", customer), ("date", "2026-10-05"), ("method", "CASH"), ("reference", "R1"), ("memo", "Counter sale"),
        ("destination", "UNDEPOSITED"), ("bank", ""), ("tax_mode", "NONE"), ("lines-TOTAL_FORMS", "1"), ("lines-INITIAL_FORMS", "0"),
        ("lines-0-description", "Mulch"), ("lines-0-quantity", "2"), ("lines-0-rate", "15.00"), ("lines-0-income_account", income),
        ("lines-0-tax_auto", "1"), ("editor_action", "save"),
    ])
}

fn send(app: &mut App, path: &str, token: &str, fields: &[(String, String)]) -> fb_app::AppResult<Outcome> {
    app.run_desktop(Command::SubmitForm { path: path.into(), token: token.into(), fields: fields.to_vec() })
}

/// The three ways a one-Save form can fail between its steps, for one form.
fn one_save_never_records_twice(f: &mut Fixture, route: &str, path: &str, kind: &str, fields: impl Fn(&str) -> Vec<(String, String)>) {
    // 1. The review's own write fails: nothing committed, so a fresh token, and the resubmit
    //    records exactly once.
    let html = screens::render(&mut f.app, path).unwrap().html;
    let p = fields(&field(&html, "request_key"));
    let before = f.count(kind);
    test_panic::arm("error-before-commit", route);
    let Outcome::Retry { token: Some(fresh), message } = send(&mut f.app, path, &field(&html, "fb_command_token"), &p).unwrap() else { panic!("{route}: a fresh token") };
    test_panic::disarm();
    assert!(message.contains("Nothing was saved."), "{route}: {message}");
    assert_eq!(f.count(kind), before, "{route}: nothing recorded");
    assert!(matches!(send(&mut f.app, path, &fresh, &p).unwrap(), Outcome::Redirect { .. }), "{route}: the resubmit records");
    assert_eq!(f.count(kind), before + 1, "{route}: recorded once");

    // 2. The review committed, then the record's write fails before COMMIT: no token, the spent
    //    one cannot repost, nothing was recorded.
    let html = screens::render(&mut f.app, path).unwrap().html;
    let p = fields(&field(&html, "request_key"));
    let spent = field(&html, "fb_command_token");
    test_panic::arm("error-before-second-commit", route);
    let Outcome::Retry { token, message } = send(&mut f.app, path, &spent, &p).unwrap() else { panic!("{route}: retry") };
    test_panic::disarm();
    assert!(token.is_none(), "{route}: the review committed, so no fresh token");
    assert!(message.contains("Part of it may already have been saved. Open this page again and check before trying again."), "{route}: {message}");
    assert_eq!(f.count(kind), before + 1, "{route}: the record rolled back");
    assert!(send(&mut f.app, path, &spent, &p).unwrap_err().to_string().contains("expired"), "{route}");
    assert_eq!(f.count(kind), before + 1);

    // 3. The record committed, then the request failed: no token; the same form sent again (its
    //    request key) with any token shows the recorded one and records nothing.
    let html = screens::render(&mut f.app, path).unwrap().html;
    let p = fields(&field(&html, "request_key"));
    test_panic::arm("error-after-second-commit", route);
    let Outcome::Retry { token, .. } = send(&mut f.app, path, &field(&html, "fb_command_token"), &p).unwrap() else { panic!("{route}: retry") };
    test_panic::disarm();
    assert!(token.is_none(), "{route}");
    assert_eq!(f.count(kind), before + 2, "{route}: recorded by the failed request");
    let token = f.app.tokens.mint(path);
    let Outcome::Redirect { .. } = send(&mut f.app, path, &token, &p).unwrap() else { panic!("{route}: shows the recorded one") };
    assert_eq!(f.count(kind), before + 2, "{route}: never a second recording");
    assert!(f.app.messages.iter().any(|m| m.contains("was already saved as")), "{route}: {:?}", f.app.messages);
    if kind == "RCP" {
        let mut changed = p;
        for (key, amount) in &mut changed {
            if key == "allocations-0-amount" { *amount = "41.00".into(); }
        }
        let token = f.app.tokens.mint(path);
        assert!(matches!(send(&mut f.app, path, &token, &changed).unwrap(), Outcome::Page {status:409, html} if html.contains("different payment details, amount or allocation")));
        assert_eq!(f.count(kind), before + 2);
    }
}

#[test]
fn receive_payment_never_records_twice_whichever_step_fails() {
    let mut f = Fixture::new();
    let invoice = f.posted_invoice("500.00");
    let path = format!("/c/{}/receipts/new/", f.c);
    let customer = f.customer.clone();
    one_save_never_records_twice(&mut f, "receipts:new", &path, "RCP", |key| payment(key, &customer, &invoice));
}

#[test]
fn a_sales_receipt_never_records_twice_whichever_step_fails() {
    let mut f = Fixture::new();
    let path = format!("/c/{}/sales-receipts/new/", f.c);
    let (customer, income) = (f.customer.clone(), f.income.clone());
    one_save_never_records_twice(&mut f, "sales_receipts:new", &path, "SR", |key| sale(key, &customer, &income));
}
