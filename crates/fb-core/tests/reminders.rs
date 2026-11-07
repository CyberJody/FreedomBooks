mod common;
use common::*;
use fb_core::{
    directory::{self, Model},
    documents, payables, reminders, sales, services,
};
use serde_json::{Value, json};
struct F {
    fx: Fx,
    co: fb_core::CompanyId,
    vendor: String,
    customer: String,
    expense: fb_core::AccountId,
    income: fb_core::AccountId,
}
impl F {
    fn new() -> Self {
        let mut fx = Fx::new("reminders");
        let co = fx.company("Synthetic reminders");
        let ar = fx.account(co, "110", "Receivables", "ASSET");
        let ap = fx.account(co, "200", "Payables", "LIABILITY");
        let bank = fx.account(co, "100", "Bank", "ASSET");
        let tax = fx.account(co, "210", "Tax", "LIABILITY");
        let expense = fx.account(co, "500", "Cost", "EXPENSE");
        let income = fx.account(co, "400", "Income", "INCOME");
        documents::save_setup(&mut fx.book,fx.user,co,&json!({"receivables":ar.to_db(),"payables":ap.to_db(),"collected_tax":tax.to_db(),"banks":[bank.to_db()]}),Some(0)).unwrap();
        let vendor = directory::save_record(
            &mut fx.book,
            fx.user,
            co,
            Model::Vendor,
            &json!({"display_name":"Twin vendor","email":"vendor@example.invalid"}),
            None,
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .into();
        let customer = directory::save_record(
            &mut fx.book,
            fx.user,
            co,
            Model::Customer,
            &json!({"display_name":"Twin customer","email":"customer@example.invalid"}),
            None,
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .into();
        Self {
            fx,
            co,
            vendor,
            customer,
            expense,
            income,
        }
    }
    fn bill(&mut self, due: &str, post: bool) -> Value {
        let b=payables::save_bill(&mut self.fx.book,self.fx.user,self.co,&json!({"vendor_id":self.vendor,"date":"2026-10-01","due_date":due,"request_key":uuid::Uuid::new_v4().to_string(),"lines":[{"description":"Work","quantity":"2","rate":"15.25","expense_account_id":self.expense.to_db()}]})).unwrap();
        if !post {
            return b;
        }
        let id = b["id"].as_str().unwrap();
        let review =
            payables::review_bill(&mut self.fx.book, self.fx.user, self.co, id, Some(1)).unwrap();
        payables::post_bill(
            &mut self.fx.book,
            self.fx.user,
            self.co,
            id,
            Some(1),
            &review,
        )
        .unwrap()
    }
    fn invoice(&mut self, due: &str, post: bool) -> Value {
        let i=sales::save_invoice(&mut self.fx.book,self.fx.user,self.co,&json!({"customer_id":self.customer,"date":"2026-10-01","due_date":due,"request_key":uuid::Uuid::new_v4().to_string(),"tax_mode":"MANUAL","tax_rate":"8","tax_label":"Synthetic","lines":[{"description":"Work","quantity":"3","rate":"10.00","taxable":true,"income_account_id":self.income.to_db()}]})).unwrap();
        if !post {
            return i;
        }
        let id = i["id"].as_str().unwrap();
        let review =
            sales::review_invoice(&mut self.fx.book, self.fx.user, self.co, id, Some(1)).unwrap();
        sales::post_invoice(
            &mut self.fx.book,
            self.fx.user,
            self.co,
            id,
            Some(1),
            Some(&review),
        )
        .unwrap()
    }
    fn build(&mut self, days: Option<i64>) -> Value {
        reminders::build(&mut self.fx.book, self.fx.user, self.co, "2026-10-15", days).unwrap()
    }
}
#[test]
fn grouping_boundaries_sort_and_void_draft_exclusions() {
    let mut f = F::new();
    f.bill("2026-10-12", false);
    let overdue = f.bill("2026-10-14", true);
    let today = f.bill("2026-10-15", true);
    let n = f.bill("2026-10-25", true);
    let n1 = f.bill("2026-10-26", true);
    let v = f.bill("2026-10-12", true);
    payables::void_bill(
        &mut f.fx.book,
        f.fx.user,
        f.co,
        v["id"].as_str().unwrap(),
        Some(2),
        &json!("2026-10-12"),
        &json!("wrong"),
    )
    .unwrap();
    f.invoice("2026-10-12", false);
    let oi = f.invoice("2026-10-14", true);
    f.invoice("2026-10-15", true);
    f.invoice("2026-11-04", true);
    let b = f.build(None);
    assert_eq!(
        b["bills"]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["document_id"].clone())
            .collect::<Vec<_>>(),
        vec![overdue["id"].clone(), today["id"].clone(), n["id"].clone()]
    );
    assert_eq!(b["invoices"]["count"], 1);
    assert_eq!(b["invoices"]["rows"][0]["document_id"], oi["id"]);
    assert_eq!(b["bills"]["total"], "91.50");
    assert_eq!(b["bills"]["overdue_total"], "30.50");
    assert_eq!(f.build(Some(0))["bills"]["count"], 2);
    assert_eq!(
        f.build(Some(11))["bills"]["rows"][3]["document_id"],
        n1["id"]
    );
    assert_eq!(
        reminders::overdue_count(&mut f.fx.book, f.fx.user, f.co, "2026-10-15").unwrap(),
        2
    );
}
#[test]
fn totals_cover_every_row_when_the_display_is_capped_and_twin_labels_disambiguate() {
    let mut f = F::new();
    directory::save_record(
        &mut f.fx.book,
        f.fx.user,
        f.co,
        Model::Vendor,
        &json!({"display_name":"Twin vendor","email":"second@example.invalid"}),
        None,
    )
    .unwrap();
    directory::save_record(
        &mut f.fx.book,
        f.fx.user,
        f.co,
        Model::Customer,
        &json!({"display_name":"Twin customer","email":"second@example.invalid"}),
        None,
    )
    .unwrap();
    for _ in 0..30 {
        f.bill("2026-10-14", true);
    }
    f.invoice("2026-10-14", true);
    let b = f.build(None);
    assert_eq!(b["bills"]["count"], 30);
    assert_eq!(b["bills"]["rows"].as_array().unwrap().len(), 25);
    assert_eq!(b["bills"]["capped"], true);
    assert_eq!(b["bills"]["total"], "915.00");
    assert_eq!(b["bills"]["overdue_count"], 30);
    assert_eq!(
        b["bills"]["rows"][0]["party"],
        "Twin vendor (vendor@example.invalid)"
    );
    assert_eq!(
        b["invoices"]["rows"][0]["party"],
        "Twin customer (customer@example.invalid)"
    );
}
#[test]
fn company_setting_is_used_and_reads_are_scoped_without_mutation() {
    let mut f = F::new();
    f.bill("2026-10-25", true);
    assert_eq!(f.build(None)["bills"]["count"], 1);
    let staff = fb_core::users::create_user(&mut f.fx.book, "staff", true).unwrap();
    fb_core::users::add_member(&mut f.fx.book, f.co, staff).unwrap();
    services::set_bill_reminder_days(&mut f.fx.book, staff, f.co, 0, Some(10)).unwrap();
    assert_eq!(f.build(None)["bills"]["count"], 0);
    let other = f.fx.company("Other");
    let before = fb_db::test_support::snapshot(f.fx.book.connection()).unwrap();
    assert_eq!(
        reminders::overdue_count(&mut f.fx.book, f.fx.user, other, "2026-10-15").unwrap(),
        0
    );
    let b = reminders::build(&mut f.fx.book, f.fx.user, other, "2026-10-15", None).unwrap();
    assert_eq!(b["bills"]["count"], 0);
    assert!(
        reminders::build(
            &mut f.fx.book,
            fb_core::UserId::new(),
            f.co,
            "2026-10-15",
            None
        )
        .is_err()
    );
    assert!(reminders::build(&mut f.fx.book, f.fx.user, f.co, "2026-02-30", None).is_err());
    assert!(
        reminders::build(
            &mut f.fx.book,
            f.fx.user,
            f.co,
            "9999-12-31",
            Some(i64::MAX)
        )
        .is_err()
    );
    assert_eq!(
        before,
        fb_db::test_support::snapshot(f.fx.book.connection()).unwrap()
    );
}
#[test]
fn timing_labels_are_frozen_python_strings() {
    for (days, label) in [
        (-2, "2 days overdue"),
        (-1, "1 day overdue"),
        (0, "Due today"),
        (1, "Due in 1 day"),
        (2, "Due in 2 days"),
    ] {
        assert_eq!(reminders::when(days), label);
    }
}
