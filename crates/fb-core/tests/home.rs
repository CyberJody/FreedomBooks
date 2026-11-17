//! Company Home figures: each must equal the figure on the screen Home links to.
mod common;
use common::*;
use chrono::NaiveDate;
use fb_core::reports::financial::{self, Basis};
use fb_core::services::{DraftInput, line};
use fb_core::{
    AccountId, CompanyId, banking,
    directory::{self, Model},
    documents, home, payables, reminders, sales, services,
};
use serde_json::{Value, json};

const TODAY: &str = "2026-10-15";

#[derive(Clone)]
struct Ids {
    co: CompanyId,
    vendor: String,
    customer: String,
    bank: AccountId,
    savings: AccountId,
    card: AccountId,
    expense: AccountId,
    income: AccountId,
}

struct F {
    fx: Fx,
    ids: Ids,
}

impl Ids {
    fn new(fx: &mut Fx, co: CompanyId) -> Ids {
        let ar = fx.account(co, "110", "Receivables", "ASSET");
        let ap = fx.account(co, "200", "Payables", "LIABILITY");
        let bank = services::save_account(&mut fx.book, fx.user, co, services::AccountInput::new("100", "Checking").kind("ASSET").detail("BANK")).unwrap().id;
        let savings = services::save_account(&mut fx.book, fx.user, co, services::AccountInput::new("101", "Savings").kind("ASSET").detail("BANK")).unwrap().id;
        let card = services::save_account(&mut fx.book, fx.user, co, services::AccountInput::new("220", "Store card").kind("LIABILITY").detail("CREDIT_CARD")).unwrap().id;
        let tax = fx.account(co, "210", "Tax", "LIABILITY");
        let expense = fx.account(co, "500", "Cost", "EXPENSE");
        let income = fx.account(co, "400", "Income", "INCOME");
        documents::save_setup(&mut fx.book, fx.user, co, &json!({"receivables":ar.to_db(),"payables":ap.to_db(),"collected_tax":tax.to_db(),"banks":[bank.to_db(),savings.to_db()]}), Some(0)).unwrap();
        let vendor = directory::save_record(&mut fx.book, fx.user, co, Model::Vendor, &json!({"display_name":"Flour vendor","email":"vendor@example.invalid"}), None).unwrap()["id"].as_str().unwrap().into();
        let customer = directory::save_record(&mut fx.book, fx.user, co, Model::Customer, &json!({"display_name":"Cafe customer","email":"customer@example.invalid"}), None).unwrap()["id"].as_str().unwrap().into();
        Ids { co, vendor, customer, bank, savings, card, expense, income }
    }
}

impl F {
    fn new() -> Self {
        let mut fx = Fx::new("home");
        let co = fx.company("Synthetic home");
        let ids = Ids::new(&mut fx, co);
        F { fx, ids }
    }
    fn bill(&mut self, date: &str, due: &str, rate: &str, post: bool) -> Value {
        let b = payables::save_bill(&mut self.fx.book, self.fx.user, self.ids.co, &json!({"vendor_id":self.ids.vendor,"date":date,"due_date":due,"request_key":uuid::Uuid::new_v4().to_string(),"lines":[{"description":"Flour","quantity":"1","rate":rate,"expense_account_id":self.ids.expense.to_db()}]})).unwrap();
        if !post {
            return b;
        }
        let id = b["id"].as_str().unwrap();
        let review = payables::review_bill(&mut self.fx.book, self.fx.user, self.ids.co, id, Some(1)).unwrap();
        payables::post_bill(&mut self.fx.book, self.fx.user, self.ids.co, id, Some(1), &review).unwrap()
    }
    fn invoice(&mut self, date: &str, due: &str, rate: &str, post: bool) -> Value {
        let i = sales::save_invoice(&mut self.fx.book, self.fx.user, self.ids.co, &json!({"customer_id":self.ids.customer,"date":date,"due_date":due,"request_key":uuid::Uuid::new_v4().to_string(),"tax_mode":"NONE","lines":[{"description":"Bread","quantity":"1","rate":rate,"taxable":false,"income_account_id":self.ids.income.to_db()}]})).unwrap();
        if !post {
            return i;
        }
        let id = i["id"].as_str().unwrap();
        let review = sales::review_invoice(&mut self.fx.book, self.fx.user, self.ids.co, id, Some(1)).unwrap();
        sales::post_invoice(&mut self.fx.book, self.fx.user, self.ids.co, id, Some(1), Some(&review)).unwrap()
    }
    fn receive(&mut self, invoice: &Value, amount: &str) {
        let input = json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":self.ids.customer,"date":"2026-10-10","amount":amount,"method":"CASH","reference":"","memo":"","destination":"BANK","bank_id":self.ids.bank.to_db(),"allocations":[{"invoice_id":invoice["id"],"amount":amount}]});
        let review = sales::review_receipt(&mut self.fx.book, self.fx.user, self.ids.co, &input).unwrap();
        sales::record_receipt(&mut self.fx.book, self.fx.user, self.ids.co, &input, Some(&review)).unwrap();
    }
    fn pay(&mut self, bill: &Value, amount: &str) {
        let p = json!({"vendor_id":self.ids.vendor,"date":"2026-10-11","amount":amount,"method":"CHECK","reference":"","memo":"","bank_id":self.ids.bank.to_db(),"request_key":uuid::Uuid::new_v4().to_string(),"allocations":[{"bill_id":bill["id"],"amount":amount}]});
        let r = payables::review_payment(&mut self.fx.book, self.fx.user, self.ids.co, &p).unwrap();
        payables::record_payment(&mut self.fx.book, self.fx.user, self.ids.co, &p, &r).unwrap();
    }
    fn journal(&mut self, date: &str, debit: AccountId, credit: AccountId, amount: &str) {
        let j = services::save_draft(&mut self.fx.book, self.fx.user, self.ids.co, DraftInput::new(date, "Synthetic entry", vec![line(debit).debit(amount).credit("0").build(), line(credit).debit("0").credit(amount).build()])).unwrap();
        services::post_journal(&mut self.fx.book, self.fx.user, self.ids.co, j.id, None).unwrap();
    }
    fn summary(&mut self) -> Value {
        home::summary(&mut self.fx.book, self.fx.user, self.ids.co, TODAY).unwrap()
    }
    /// A small book touching every figure: overdue, partly paid, paid, current and draft invoices; overdue,
    /// due-soon, later and draft bills; money in two banks and a card; activity before and after October.
    fn seeded() -> Self {
        let mut f = F::new();
        let equity = f.fx.account(f.ids.co, "300", "Owner equity", "EQUITY");
        f.journal("2026-09-20", f.ids.savings, equity, "5000.00");
        f.journal("2026-10-02", f.ids.bank, equity, "1200.00");
        f.journal("2026-10-03", f.ids.expense, f.ids.card, "48.25");
        f.journal("2026-11-05", f.ids.expense, f.ids.bank, "99.00");
        f.invoice("2026-09-25", "2026-10-01", "1250.00", true);
        let partial = f.invoice("2026-10-01", "2026-10-14", "300.00", true);
        f.receive(&partial, "100.00");
        let paid = f.invoice("2026-10-01", "2026-10-05", "75.00", true);
        f.receive(&paid, "75.00");
        f.invoice("2026-10-05", "2026-11-04", "410.10", true);
        f.invoice("2026-10-09", "2026-10-30", "999.00", false);
        f.bill("2026-09-30", "2026-10-10", "220.00", true);
        let soon = f.bill("2026-10-01", "2026-10-20", "140.00", true);
        f.pay(&soon, "40.00");
        f.bill("2026-10-01", "2026-10-25", "60.00", true);
        f.bill("2026-10-01", "2026-11-30", "35.50", true);
        f.bill("2026-10-01", "2026-10-21", "500.00", false);
        f
    }
}

fn cents(v: &Value) -> i128 {
    fb_core::services::money_json(v).unwrap().cents()
}

#[test]
fn receivables_and_payables_equal_reminders_and_both_lists() {
    let mut f = F::seeded();
    let s = f.summary();
    let r = &s["receivables"];
    let p = &s["payables"];
    // Hand-computed: 1250 + 200 (300 less a 100 receipt) + 410.10 open; 1450 of it overdue.
    assert_eq!(r["total"], "1860.10");
    assert_eq!(r["count"], 3);
    assert_eq!(r["overdue_total"], "1450.00");
    assert_eq!(r["overdue_count"], 2);
    // Reminders' overdue invoices.
    let rem = reminders::build(&mut f.fx.book, f.fx.user, f.ids.co, TODAY, None).unwrap();
    assert_eq!(r["overdue_total"], rem["invoices"]["total"]);
    assert_eq!(r["overdue_count"], rem["invoices"]["count"]);
    // The invoice list's balances: open (OPEN or PARTIAL with a balance) and overdue.
    let balances = sales::invoice_balances(&mut f.fx.book, f.fx.user, f.ids.co).unwrap();
    let open: Vec<&Value> = balances.iter().filter(|b| matches!(b["payment_state"].as_str(), Some("OPEN" | "PARTIAL")) && cents(&b["outstanding"]) > 0).collect();
    assert_eq!(cents(&r["total"]), open.iter().map(|b| cents(&b["outstanding"])).sum::<i128>());
    assert_eq!(cents(&r["overdue_total"]), open.iter().filter(|b| b["due_date"].as_str().unwrap() < TODAY).map(|b| cents(&b["outstanding"])).sum::<i128>());
    assert_eq!(s["drafts"]["invoices"], balances.iter().filter(|b| b["payment_state"] == "DRAFT").count());
    // Bills: 220 overdue; 100 (140 less 40) and 60 due within ten days; 35.50 later.
    assert_eq!(p["total"], "415.50");
    assert_eq!(p["count"], 4);
    assert_eq!(p["overdue_total"], "220.00");
    assert_eq!(p["overdue_count"], 1);
    assert_eq!(p["soon_total"], "160.00");
    assert_eq!(p["soon_count"], 2);
    assert_eq!(p["next_due"], "2026-10-20");
    assert_eq!(p["through"], "2026-10-25");
    // Reminders' bills to pay are the overdue and due-soon bills together.
    assert_eq!(cents(&rem["bills"]["total"]), cents(&p["overdue_total"]) + cents(&p["soon_total"]));
    assert_eq!(rem["bills"]["count"].as_u64().unwrap(), p["overdue_count"].as_u64().unwrap() + p["soon_count"].as_u64().unwrap());
    // The bill list's tiles.
    let t = payables::bill_tracker(&mut f.fx.book, f.fx.user, f.ids.co, &json!({}), &json!(TODAY)).unwrap();
    assert_eq!(p["total"], t["totals"]["open_money"]);
    assert_eq!(p["overdue_total"], t["totals"]["overdue_money"]);
    assert_eq!(s["drafts"]["bills"], t["totals"]["draft_count"]);
    assert_eq!(s["drafts"]["invoices"], 1);
}

#[test]
fn bank_balances_equal_each_register_closing() {
    let mut f = F::seeded();
    let s = f.summary();
    let banks = s["banks"].as_array().unwrap();
    assert_eq!(banks.iter().map(|b| b["name"].as_str().unwrap()).collect::<Vec<_>>(), ["Checking", "Savings", "Store card"]);
    for b in banks {
        let register = banking::build(&mut f.fx.book, f.fx.user, f.ids.co, b["id"].as_str().unwrap(), &json!({}), 1, 1).unwrap();
        assert_eq!(b["balance"], register["closing"], "{}", b["name"]);
    }
    // 1200 in, 100 and 75 received, 40 paid out, 99 spent in November: every posted line counts.
    assert_eq!(banks[0]["balance"], "1236.00");
    assert_eq!(banks[1]["balance"], "5000.00");
    // A card shows what is owed, as its register does.
    assert_eq!(banks[2]["balance"], "48.25");
    assert_eq!(banks[2]["card"], true);
}

#[test]
fn month_figures_equal_the_profit_and_loss_report() {
    let mut f = F::seeded();
    let s = f.summary();
    let (start, end) = (NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(), NaiveDate::from_ymd_opt(2026, 10, 31).unwrap());
    let report = financial::profit_and_loss(&mut f.fx.book, f.fx.user, f.ids.co, Basis::Accrual, Some(start), end).unwrap();
    let period = f.fx.book.read(|q| financial::accrual_period(q, f.ids.co, start, end)).unwrap();
    assert_eq!(period, report);
    assert_eq!(s["month"]["net"], report.net_income.to_string());
    assert_eq!(s["month"]["start"], "2026-10-01");
    assert_eq!(s["month"]["end"], "2026-10-31");
    // October only: invoices 300 + 75 + 410.10 (the draft and September's invoice are out); expenses are
    // the card charge and the posted bills dated in October (the September bill and November entry are out).
    assert_eq!(s["month"]["income"], "785.10");
    assert_eq!(s["month"]["expenses"], "283.75");
    assert_eq!(s["month"]["net"], "501.35");
}

#[test]
fn drafts_review_reconnect_setup_and_posted_flags() {
    let mut f = F::new();
    let s = f.summary();
    assert_eq!(s["posted"], false);
    assert_eq!(s["drafts"], json!({"invoices":0,"bills":0}));
    assert_eq!(s["to_review"], 0);
    assert_eq!(s["reconnect"], json!([]));
    // Accounting setup also wants an undeposited funds account, which this fixture leaves out.
    assert_eq!(s["setup"], json!({"chart":true,"special":false,"tax":false,"customer":true,"vendor":true,"item":false}));
    assert_eq!(s["recent"], json!([]));
    assert_eq!(s["receivables"]["total"], "0.00");
    f.invoice("2026-10-01", "2026-10-31", "10.00", false);
    let s = f.summary();
    assert_eq!(s["posted"], false);
    assert_eq!(s["drafts"]["invoices"], 1);
    f.invoice("2026-10-01", "2026-10-31", "10.00", true);
    assert_eq!(f.summary()["posted"], true);
}

#[test]
fn recent_activity_lists_distinct_documents_newest_first_within_the_company() {
    let mut f = F::new();
    let first = f.invoice("2026-10-01", "2026-10-31", "10.00", true);
    let bill = f.bill("2026-10-01", "2026-10-31", "20.00", true);
    payables::void_bill(&mut f.fx.book, f.fx.user, f.ids.co, bill["id"].as_str().unwrap(), Some(2), &json!("2026-10-02"), &json!("entered twice")).unwrap();
    let draft = f.invoice("2026-10-03", "2026-10-31", "30.00", false);
    // Another company's documents never appear.
    let mine = f.ids.clone();
    let other = f.fx.company("Other company");
    f.ids = Ids::new(&mut f.fx, other);
    f.invoice("2026-10-04", "2026-10-31", "40.00", true);
    f.ids = mine;
    let recent = f.summary()["recent"].clone();
    let rows = recent.as_array().unwrap();
    assert_eq!(rows.len(), 3, "{recent}");
    assert_eq!(rows[0]["document_id"], draft["id"]);
    assert_eq!(rows[0]["kind"], "INV");
    assert_eq!(rows[0]["state"], "DRAFT");
    assert_eq!(rows[0]["amount"], "30.00");
    assert_eq!(rows[0]["party"], "Cafe customer");
    assert_eq!(rows[0]["by"], "tester");
    assert_eq!(rows[1]["document_id"], bill["id"]);
    assert_eq!(rows[1]["kind"], "BILL");
    assert_eq!(rows[1]["state"], "VOID");
    assert_eq!(rows[1]["party"], "Flour vendor");
    assert_eq!(rows[2]["document_id"], first["id"]);
    assert_eq!(rows[2]["state"], "OPEN");
    assert!(rows.iter().all(|r| r["number"].as_str().is_some_and(|n| !n.is_empty())));
    // At most ten, one row per document however many events it has.
    for i in 0..12 {
        f.invoice("2026-10-05", "2026-10-31", &format!("{}.00", i + 1), true);
    }
    let recent = f.summary()["recent"].clone();
    let rows = recent.as_array().unwrap();
    assert_eq!(rows.len(), home::RECENT_LIMIT);
    let mut ids: Vec<_> = rows.iter().map(|r| r["document_id"].as_str().unwrap()).collect();
    ids.dedup();
    assert_eq!(ids.len(), home::RECENT_LIMIT);
    assert_eq!(rows[0]["amount"], "12.00");
}

#[test]
fn summary_needs_company_access_and_reads_without_writing() {
    let mut f = F::seeded();
    let stranger = fb_core::users::create_user(&mut f.fx.book, "stranger", false).unwrap();
    let before = fb_db::test_support::snapshot(f.fx.book.connection()).unwrap();
    assert!(home::summary(&mut f.fx.book, stranger, f.ids.co, TODAY).is_err());
    assert!(home::summary(&mut f.fx.book, fb_core::UserId::new(), f.ids.co, TODAY).is_err());
    f.summary();
    assert_eq!(fb_db::test_support::snapshot(f.fx.book.connection()).unwrap(), before);
    let other = f.fx.company("Empty company");
    let s = home::summary(&mut f.fx.book, f.fx.user, other, TODAY).unwrap();
    assert_eq!(s["receivables"]["total"], "0.00");
    assert_eq!(s["payables"]["total"], "0.00");
    assert_eq!(s["banks"], json!([]));
    assert_eq!(s["recent"], json!([]));
    assert_eq!(s["posted"], false);
}

/// Cost on a long history: `cargo test --release -p fb-core --test home -- --ignored --nocapture`.
/// Four years of paid invoices and bank activity; prints Home's read time beside the invoice list's
/// per-invoice balance computation for the same book.
#[test]
#[ignore]
fn summary_cost_on_years_of_history() {
    let mut f = F::new();
    let equity = f.fx.account(f.ids.co, "300", "Owner equity", "EQUITY");
    let n: usize = std::env::var("FB_HOME_BENCH_INVOICES").ok().and_then(|v| v.parse().ok()).unwrap_or(2000);
    let started = std::time::Instant::now();
    for i in 0..n {
        let date = format!("{}-{:02}-{:02}", 2022 + i * 4 / n, i % 12 + 1, i % 28 + 1);
        let inv = f.invoice(&date, &date, "25.00", true);
        if i % 50 != 0 {
            f.receive(&inv, "25.00");
        }
        f.journal(&date, f.ids.expense, f.ids.bank, "3.10");
        if i % 10 == 0 {
            f.journal(&date, f.ids.bank, equity, "100.00");
        }
    }
    eprintln!("seeded {n} invoices in {:?}", started.elapsed());
    let mut best = std::time::Duration::MAX;
    for _ in 0..5 {
        let t = std::time::Instant::now();
        let s = f.summary();
        best = best.min(t.elapsed());
        assert_eq!(s["receivables"]["count"].as_u64().unwrap() as usize, n.div_ceil(50));
    }
    let t = std::time::Instant::now();
    sales::invoice_balances(&mut f.fx.book, f.fx.user, f.ids.co).unwrap();
    eprintln!("home::summary best of 5: {best:?}; invoice_balances (the invoice list's per-invoice path): {:?}", t.elapsed());
}
