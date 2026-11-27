//! Sales tax payments: posting, owed arithmetic, refusals, void, audit, access and the database backstops.
use super::*;
use crate::directory::Model;
use crate::reports::financial::Basis;
use crate::services::{AccountInput, DraftInput, line};
use crate::{banking, sales, salestax, users};
use fb_db::rusqlite::Connection;

struct Fx {
    book: Book,
    actor: UserId,
    co: CompanyId,
    bank: AccountId,
    bank2: AccountId,
    tax: AccountId,
    expense: AccountId,
    income: AccountId,
    ar: AccountId,
    customer: String,
    state: Value,
    county: Value,
    group: Value,
}
fn new_id() -> String {
    fb_ids::new_v7().to_string()
}
impl Fx {
    fn new() -> Fx {
        let path = std::env::temp_dir().join(format!("fb-taxpay-{}.fbbook", new_id()));
        let mut book = Book::create(&path, &[31; 32]).unwrap();
        let actor = users::create_user(&mut book, "Tax payer", false).unwrap();
        let co = services::create_company(&mut book, actor, "Synthetic tax payments", "USD", false).unwrap().id;
        let mut acct = |code: &str, kind: &str| services::save_account(&mut book, actor, co, AccountInput::new(code, &format!("Account {code}")).kind(kind)).unwrap().id;
        let (income, ar, bank, bank2, other_bank) = (acct("400", "INCOME"), acct("110", "ASSET"), acct("100", "ASSET"), acct("101", "ASSET"), acct("102", "ASSET"));
        let (tax, expense, undeposited) = (acct("210", "LIABILITY"), acct("650", "EXPENSE"), acct("120", "ASSET"));
        let _ = other_bank;
        documents::save_setup(&mut book, actor, co, &json!({"receivables":ar.to_db(),"undeposited":undeposited.to_db(),"collected_tax":tax.to_db(),"banks":[bank.to_db(),bank2.to_db()]}), Some(0)).unwrap();
        let state = salestax::save_rate(&mut book, actor, co, &json!({"name":"State","description":"State tax","rate":"4","new_agency_name":"State Revenue","expected_revision":0})).unwrap();
        let county = salestax::save_rate(&mut book, actor, co, &json!({"name":"County","description":"County tax","rate":"2.5","new_agency_name":"County Treasurer","expected_revision":0})).unwrap();
        salestax::save_settings(&mut book, actor, co, &json!({"charge_tax":true,"default_item_id":null,"owed_basis":"ACCRUAL","filing_frequency":"QUARTERLY","expected_revision":0})).unwrap();
        let group = salestax::save_group(&mut book, actor, co, &json!({"name":"Combined","description":"Sales tax","member_ids":[state["id"],county["id"]],"expected_revision":0})).unwrap();
        let customer = directory::save_record(&mut book, actor, co, Model::Customer, &json!({"display_name":"Synthetic customer"}), None).unwrap()["id"].as_str().unwrap().to_string();
        Fx { book, actor, co, bank, bank2, tax, expense, income, ar, customer, state, county, group }
    }
    fn state_agency(&self) -> String {
        s(&self.state, "agency_id").to_owned()
    }
    fn county_agency(&self) -> String {
        s(&self.county, "agency_id").to_owned()
    }
    /// Post an invoice of `amount` taxed with `tax` (a rate/group id, or None for a manual 5% rate).
    fn invoice(&mut self, date: &str, amount: &str, tax: Option<&Value>) -> String {
        let lines = json!([{"item_id":null,"description":"Work","quantity":"1","rate":amount,"taxable":true,"income_account_id":self.income.to_db()}]);
        self.invoice_with_lines(date, lines, tax)
    }
    fn invoice_with_lines(&mut self, date: &str, lines: Value, tax: Option<&Value>) -> String {
        let mut input = json!({"request_key":new_id(),"customer_id":self.customer,"date":date,"memo":"Taxed sale","lines":lines});
        match tax {
            Some(item) => {
                input["tax_mode"] = json!("ITEM");
                input["tax_item_id"] = item["id"].clone();
            }
            None => {
                input["tax_mode"] = json!("MANUAL");
                input["tax_rate"] = json!("5");
                input["tax_label"] = json!("Manual tax");
            }
        }
        let inv = sales::save_invoice(&mut self.book, self.actor, self.co, &input).unwrap();
        let key = inv["id"].as_str().unwrap().to_string();
        let review = sales::review_invoice(&mut self.book, self.actor, self.co, &key, Some(1)).unwrap();
        sales::post_invoice(&mut self.book, self.actor, self.co, &key, Some(1), Some(&review)).unwrap();
        key
    }
    fn receive(&mut self, invoice: &str, date: &str, amount: &str) {
        let input = json!({"request_key":new_id(),"customer_id":self.customer,"date":date,"method":"CHECK","destination":"BANK","bank_id":self.bank.to_db(),"amount":amount,"allocations":[{"invoice_id":invoice,"amount":amount}]});
        let r = sales::review_receipt(&mut self.book, self.actor, self.co, &input).unwrap();
        sales::record_receipt(&mut self.book, self.actor, self.co, &input, Some(&r)).unwrap();
    }
    fn input(&self, agency: &str, date: &str, tax: &str) -> Value {
        json!({"request_key":new_id(),"agency_id":agency,"date":date,"bank_id":self.bank.to_db(),"tax_amount":tax,"penalty_amount":"0","expense_account_id":null,
            "period_start":"2026-07-01","period_end":"2026-09-30","reference":"Q3 return","memo":""})
    }
    fn pay(&mut self, p: &Value) -> FbResult<Value> {
        let r = review_payment(&mut self.book, self.actor, self.co, p)?;
        record_payment(&mut self.book, self.actor, self.co, p, &r)
    }
    fn owed(&mut self, start: Option<&str>, end: &str) -> Value {
        owed_report(&mut self.book, self.actor, self.co, start, end).unwrap()
    }
    fn ledger_tax(&self) -> i64 {
        self.book.connection().query_row("SELECT coalesce(sum(l.credit-l.debit),0) FROM ledger_journalline l JOIN ledger_journal j ON j.id=l.journal_id WHERE l.account_id=?1 AND j.status='POSTED'", [self.tax.to_db()], |r| r.get(0)).unwrap()
    }
    fn count(&self, sql: &str) -> i64 {
        self.book.connection().query_row(sql, [], |r| r.get(0)).unwrap()
    }
    fn set_basis(&mut self, basis: &str) {
        let s = salestax::settings_for(self.book.connection(), self.co).unwrap();
        salestax::save_settings(&mut self.book, self.actor, self.co, &json!({"charge_tax":true,"default_item_id":null,"owed_basis":basis,"filing_frequency":"QUARTERLY","expected_revision":s["revision"]})).unwrap();
    }
}
fn row<'a>(report: &'a Value, agency: &str) -> &'a Value {
    report["agencies"].as_array().unwrap().iter().find(|a| a["agency_id"] == agency).unwrap()
}
fn refused<T: std::fmt::Debug>(r: FbResult<T>, needle: &str) {
    let e = r.unwrap_err().message();
    assert!(e.contains(needle), "expected {needle:?} in {e:?}");
}
fn sql_refused(conn: &Connection, sql: &str, constraint: &str) {
    let e = conn.execute_batch(sql).unwrap_err().to_string();
    assert!(e.contains(constraint), "expected {constraint} from {sql}: {e}");
}

#[test]
fn partial_payment_debits_collected_tax_credits_the_bank_and_reduces_what_is_owed() {
    let mut f = Fx::new();
    let group = f.group.clone();
    f.invoice("2026-08-10", "200.00", Some(&group)); // 8.00 state, 5.00 county
    assert_eq!(f.ledger_tax(), 1300);
    let before = f.owed(Some("2026-07-01"), "2026-09-30");
    assert_eq!(row(&before, &f.state_agency())["collected"], "8.00");
    assert_eq!(row(&before, &f.county_agency())["collected"], "5.00");
    assert_eq!(before["total"]["balance"], "13.00");
    assert!(before["unlinked"].is_null());

    let p = f.input(&f.state_agency(), "2026-10-15", "5.00");
    let review = review_payment(&mut f.book, f.actor, f.co, &p).unwrap();
    assert_eq!(review.display["total"], "5.00");
    assert_eq!(review.display["owed_to_agency"], "8.00");
    assert!(review.display["warnings"].as_array().unwrap().is_empty());
    let audits = f.count("SELECT count(*) FROM ledger_auditevent");
    let paid = record_payment(&mut f.book, f.actor, f.co, &p, &review).unwrap();
    assert_eq!(paid["number"], "STP-000001");
    assert_eq!(paid["state_label"], "Recorded");
    assert_eq!(paid["lines"], json!([{"account":"210 · Account 210","debit":"5.00","credit":null},{"account":"100 · Account 100","debit":null,"credit":"5.00"}]));
    assert_eq!(f.ledger_tax(), 800, "the liability went down by the payment");
    // Identity, account evidence, journal, payment, seal and audit commit together.
    assert_eq!(f.count("SELECT count(*) FROM ledger_auditevent") - audits, 5, "identity, accounts, draft, post and the payment itself");
    assert_eq!(f.count("SELECT count(*) FROM ledger_auditevent WHERE action='salestax.payment.recorded'"), 1);

    let in_period = f.owed(Some("2026-07-01"), "2026-09-30");
    assert_eq!(in_period["total"]["balance"], "13.00", "a payment after the period does not change the period");
    let october = f.owed(Some("2026-10-01"), "2026-10-31");
    let state = row(&october, &f.state_agency());
    assert_eq!((state["opening"].as_str(), state["paid"].as_str(), state["balance"].as_str()), (Some("8.00"), Some("5.00"), Some("3.00")));
    assert_eq!(row(&october, &f.county_agency())["balance"], "5.00");
    assert_eq!(october["total"]["balance"], "8.00");
    assert_eq!(october["total"]["balance_cents"].as_i64().unwrap(), f.ledger_tax());

    // The bank register shows the payment as a document like any other bank payment.
    let reg = banking::build(&mut f.book, f.actor, f.co, &f.bank.to_db(), &json!({}), 1, 100).unwrap();
    let text = reg.to_string();
    assert!(text.contains("Sales tax payment") && text.contains("State Revenue"), "{text}");
    assert_eq!(payments(f.book.connection(), f.co).unwrap().len(), 1);
}

#[test]
fn retries_return_the_recorded_payment_and_a_changed_request_is_refused() {
    let mut f = Fx::new();
    let state = f.state.clone();
    f.invoice("2026-08-10", "100.00", Some(&state));
    let p = f.input(&f.state_agency(), "2026-10-15", "4.00");
    let r = review_payment(&mut f.book, f.actor, f.co, &p).unwrap();
    let first = record_payment(&mut f.book, f.actor, f.co, &p, &r).unwrap();
    assert_eq!(record_payment(&mut f.book, f.actor, f.co, &p, &r).unwrap(), first);
    assert_eq!(f.count("SELECT count(*) FROM salestax_payment"), 1);
    let mut changed = p.clone();
    changed["tax_amount"] = json!("3.00");
    let r2 = review_payment(&mut f.book, f.actor, f.co, &changed).unwrap();
    refused(record_payment(&mut f.book, f.actor, f.co, &changed, &r2), "already recorded as a different");
    // A review of one payment cannot record another.
    let other = f.input(&f.state_agency(), "2026-10-16", "1.00");
    refused(record_payment(&mut f.book, f.actor, f.co, &other, &r), "changed since the review");
    assert_eq!(f.count("SELECT count(*) FROM salestax_payment"), 1);
    assert_eq!(f.count("SELECT count(*) FROM documents_documentidentity WHERE kind='STP'"), 1, "a refused record leaves no identity behind");
}

#[test]
fn a_review_goes_stale_when_the_accounts_change() {
    let mut f = Fx::new();
    let state = f.state.clone();
    f.invoice("2026-08-10", "100.00", Some(&state));
    let p = f.input(&f.state_agency(), "2026-10-15", "4.00");
    let r = review_payment(&mut f.book, f.actor, f.co, &p).unwrap();
    let new_tax = services::save_account(&mut f.book, f.actor, f.co, AccountInput::new("211", "New tax").kind("LIABILITY")).unwrap().id;
    let setup = documents::setup_snapshot(f.book.connection(), f.co).unwrap();
    documents::save_setup(&mut f.book, f.actor, f.co, &json!({"receivables":f.ar.to_db(),"collected_tax":new_tax.to_db(),"banks":[f.bank.to_db(),f.bank2.to_db()]}), setup["revision"].as_i64()).unwrap();
    refused(record_payment(&mut f.book, f.actor, f.co, &p, &r), "changed since the review");
}

#[test]
fn refusals_name_the_problem_and_write_nothing() {
    let mut f = Fx::new();
    let state = f.state.clone();
    f.invoice("2026-08-10", "100.00", Some(&state));
    let plain_vendor = directory::save_record(&mut f.book, f.actor, f.co, Model::Vendor, &json!({"display_name":"Office supplies"}), None).unwrap()["id"].as_str().unwrap().to_string();
    let other_bank = store_account(&f, "102");
    let journals = f.count("SELECT count(*) FROM ledger_journal");
    let base = f.input(&f.state_agency(), "2026-10-15", "4.00");
    let cases: Vec<(Value, &str)> = vec![
        (json!({"agency_id":plain_vendor}), "Choose a tax agency"),
        (json!({"agency_id":"not-an-id"}), "Choose the tax agency you paid"),
        (json!({"bank_id":other_bank}), "is not an active bank account"),
        (json!({"bank_id":f.tax.to_db()}), "is not an active bank account"),
        (json!({"tax_amount":"0"}), "more than zero"),
        (json!({"tax_amount":"-1.00"}), "zero or more"),
        (json!({"tax_amount":"1.005"}), "two decimal places"),
        (json!({"penalty_amount":"2.00"}), "expense account for the penalty"),
        (json!({"penalty_amount":"2.00","expense_account_id":f.income.to_db()}), "cannot take the penalty"),
        (json!({"penalty_amount":"2.00","expense_account_id":f.tax.to_db()}), "cannot take the penalty"),
        (json!({"period_start":"2026-09-30","period_end":"2026-07-01"}), "must not be after"),
        (json!({"period_start":"2026-07-01","period_end":""}), "both dates"),
        (json!({"date":"2026-02-30"}), "payment date"),
        (json!({"request_key":"x"}), "request key"),
        (json!({"memo":"Card 4111 1111 1111 1111"}), "card"),
    ];
    for (patch, needle) in cases {
        let mut p = base.clone();
        for (k, v) in patch.as_object().unwrap() {
            p[k] = v.clone();
        }
        refused(review_payment(&mut f.book, f.actor, f.co, &p), needle);
    }
    // An inactive agency cannot be paid until it is reactivated.
    let agency = f.state_agency();
    directory::set_active(&mut f.book, f.actor, f.co, Model::Vendor, &agency, false).unwrap();
    refused(review_payment(&mut f.book, f.actor, f.co, &base), "inactive");
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal"), journals);
    assert_eq!(f.count("SELECT count(*) FROM documents_documentidentity WHERE kind='STP'"), 0);
}
fn store_account(f: &Fx, code: &str) -> String {
    f.book.connection().query_row("SELECT id FROM ledger_account WHERE company_id=?1 AND code=?2", params![f.co.to_db(), code], |r| r.get(0)).unwrap()
}

#[test]
fn someone_outside_the_company_cannot_review_record_void_or_read() {
    let mut f = Fx::new();
    let state = f.state.clone();
    f.invoice("2026-08-10", "100.00", Some(&state));
    let p = f.input(&f.state_agency(), "2026-10-15", "4.00");
    let paid = f.pay(&p).unwrap();
    let outsider = users::create_user(&mut f.book, "Outsider", false).unwrap();
    let one = f.input(&f.state_agency(), "2026-10-15", "1.00");
    refused(review_payment(&mut f.book, outsider, f.co, &one), "");
    let r = review_payment(&mut f.book, f.actor, f.co, &one).unwrap();
    assert!(record_payment(&mut f.book, outsider, f.co, &p, &r).is_err());
    assert!(void_payment(&mut f.book, outsider, f.co, s(&paid, "id"), Some(1), &json!("2026-10-20"), &json!("x")).is_err());
    assert!(owed_report(&mut f.book, outsider, f.co, None, "2026-10-31").is_err());
    assert_eq!(f.count("SELECT count(*) FROM salestax_payment"), 1);
}

#[test]
fn penalty_and_interest_go_to_expense_and_only_the_tax_reduces_the_liability() {
    let mut f = Fx::new();
    let state = f.state.clone();
    f.invoice("2026-08-10", "100.00", Some(&state));
    let mut p = f.input(&f.state_agency(), "2026-11-02", "4.00");
    p["penalty_amount"] = json!("1.25");
    p["expense_account_id"] = json!(f.expense.to_db());
    let paid = f.pay(&p).unwrap();
    assert_eq!(paid["total"], "5.25");
    assert_eq!(paid["lines"], json!([{"account":"210 · Account 210","debit":"4.00","credit":null},{"account":"650 · Account 650","debit":"1.25","credit":null},{"account":"100 · Account 100","debit":null,"credit":"5.25"}]));
    assert_eq!(f.ledger_tax(), 0);
    let report = f.owed(None, "2026-11-30");
    assert_eq!(row(&report, &f.state_agency())["paid"], "4.00");
    assert_eq!(report["total"]["balance"], "0.00");
    // A penalty-only payment touches no tax account at all.
    let mut late = f.input(&f.state_agency(), "2026-11-03", "0");
    late["penalty_amount"] = json!("10.00");
    late["expense_account_id"] = json!(f.expense.to_db());
    let fee = f.pay(&late).unwrap();
    assert!(fee["tax_account_id"].is_null());
    assert_eq!(fee["lines"].as_array().unwrap().len(), 2);
    assert_eq!(f.ledger_tax(), 0);
}

#[test]
fn paying_more_than_is_owed_is_allowed_with_a_warning() {
    let mut f = Fx::new();
    let state = f.state.clone();
    f.invoice("2026-08-10", "100.00", Some(&state)); // 4.00 owed to the state
    let p = f.input(&f.state_agency(), "2026-10-15", "10.00");
    let r = review_payment(&mut f.book, f.actor, f.co, &p).unwrap();
    let w = r.display["warnings"].as_array().unwrap();
    assert_eq!(w.len(), 1);
    assert!(w[0].as_str().unwrap().contains("6.00 more than all the sales tax owed"), "{w:?}");
    record_payment(&mut f.book, f.actor, f.co, &p, &r).unwrap();
    assert_eq!(f.ledger_tax(), -600, "the advance stays visible as a debit balance");
    assert_eq!(f.owed(None, "2026-10-31")["total"]["balance"], "-6.00");
    // Paying the county more than the county is owed, while the state is still owed, warns about the agency.
    let mut g = Fx::new();
    let group = g.group.clone();
    g.invoice("2026-08-10", "200.00", Some(&group)); // 8.00 state, 5.00 county
    let county = g.input(&g.county_agency(), "2026-10-15", "6.00");
    let r = review_payment(&mut g.book, g.actor, g.co, &county).unwrap();
    assert!(r.display["warnings"][0].as_str().unwrap().contains("owed to County Treasurer"), "{}", r.display);
}

#[test]
fn a_void_reverses_exactly_restores_what_is_owed_and_cannot_predate_the_payment() {
    let mut f = Fx::new();
    let state = f.state.clone();
    f.invoice("2026-08-10", "100.00", Some(&state));
    let paid = f.pay(&f.input(&f.state_agency(), "2026-10-15", "3.00")).unwrap();
    let id = s(&paid, "id").to_owned();
    refused(void_payment(&mut f.book, f.actor, f.co, &id, Some(1), &json!("2026-10-14"), &json!("Wrong agency")), "before the payment date");
    refused(void_payment(&mut f.book, f.actor, f.co, &id, Some(1), &json!("2026-10-20"), &json!("  ")), "reason");
    refused(void_payment(&mut f.book, f.actor, f.co, &id, Some(0), &json!("2026-10-20"), &json!("Wrong agency")), "changed since you opened it");
    let voided = void_payment(&mut f.book, f.actor, f.co, &id, Some(1), &json!("2026-10-20"), &json!("Wrong agency")).unwrap();
    assert_eq!(voided["state_label"], "Voided");
    assert_eq!(voided["void"]["lines"], json!([{"account":"210 · Account 210","debit":null,"credit":"3.00"},{"account":"100 · Account 100","debit":"3.00","credit":null}]));
    assert_eq!(f.ledger_tax(), 400);
    assert_eq!(void_payment(&mut f.book, f.actor, f.co, &id, Some(1), &json!("2026-10-20"), &json!("Wrong agency")).unwrap(), voided, "an exact retry returns the void");
    refused(void_payment(&mut f.book, f.actor, f.co, &id, Some(1), &json!("2026-10-21"), &json!("Other")), "already voided");
    assert_eq!(f.count("SELECT count(*) FROM ledger_auditevent WHERE action='salestax.payment.voided'"), 1);
    // As of a date between payment and void, the payment still counts; after the void it does not.
    assert_eq!(f.owed(None, "2026-10-16")["total"]["balance"], "1.00");
    let after = f.owed(Some("2026-10-01"), "2026-10-31");
    assert_eq!(row(&after, &f.state_agency())["paid"], "0.00");
    assert_eq!(after["total"]["balance"], "4.00");
    // The ledger's own reversal guard: the void journal is the exact mirror of the payment journal.
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal r JOIN ledger_journal o ON o.id=r.reversal_of_id JOIN documents_documentidentity i ON i.id=o.source_id WHERE i.kind='STP'"), 1);
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn a_cleared_payment_cannot_be_voided_until_it_is_uncleared() {
    let mut f = Fx::new();
    let state = f.state.clone();
    f.invoice("2026-08-10", "100.00", Some(&state));
    let paid = f.pay(&f.input(&f.state_agency(), "2026-10-15", "4.00")).unwrap();
    let line: String = f.book.connection().query_row("SELECT id FROM ledger_journalline WHERE journal_id=?1 AND account_id=?2", params![s(&paid, "journal_id"), f.bank.to_db()], |r| r.get(0)).unwrap();
    banking::set_cleared(&mut f.book, f.actor, f.co, &line, "C", Some(0), Some(&f.bank.to_db())).unwrap();
    assert!(void_payment(&mut f.book, f.actor, f.co, s(&paid, "id"), Some(1), &json!("2026-10-20"), &json!("Mistake")).is_err());
    assert_eq!(f.count("SELECT count(*) FROM salestax_paymentvoid"), 0);
}

#[test]
fn group_rates_attribute_to_each_agency_and_manual_tax_is_shown_unlinked() {
    let mut f = Fx::new();
    let group = f.group.clone();
    let state = f.state.clone();
    f.invoice("2026-07-05", "333.33", Some(&group)); // state 13.33, county 8.33
    f.invoice("2026-08-05", "50.00", Some(&state)); // state 2.00
    f.invoice("2026-09-05", "100.00", None); // manual 5.00, no agency
    let r = f.owed(Some("2026-07-01"), "2026-09-30");
    assert_eq!(row(&r, &f.state_agency())["collected"], "15.33");
    assert_eq!(row(&r, &f.county_agency())["collected"], "8.33");
    assert_eq!(r["unlinked"]["collected"], "5.00");
    assert_eq!(r["unlinked"]["name"], UNLINKED);
    assert_eq!(r["total"]["collected"], "28.66");
    assert_eq!(r["total"]["balance_cents"].as_i64().unwrap(), f.ledger_tax());
    // Voiding an invoice takes its tax back out, per agency, in the period of the void.
    let inv = f.invoice("2026-09-10", "100.00", Some(&group));
    sales::void_invoice(&mut f.book, f.actor, f.co, &inv, Some(2), &json!("2026-10-02"), &json!("Cancelled")).unwrap();
    let q3 = f.owed(Some("2026-07-01"), "2026-09-30");
    assert_eq!(row(&q3, &f.state_agency())["collected"], "19.33");
    let oct = f.owed(Some("2026-10-01"), "2026-10-31");
    assert_eq!(row(&oct, &f.state_agency())["collected"], "-4.00");
    assert_eq!(row(&oct, &f.county_agency())["collected"], "-2.50");
    assert_eq!(oct["total"]["balance_cents"].as_i64().unwrap(), f.ledger_tax());
}

/// Totals on both bases equal the collected-tax balance that the financial reports compute.
fn assert_matches_reports(f: &mut Fx, end: &str) {
    for basis in [Basis::Accrual, Basis::Cash] {
        f.set_basis(basis.as_str());
        let r = f.owed(None, end);
        let books = Books::load(f.book.connection(), f.co, basis, NaiveDate::parse_from_str(end, "%Y-%m-%d").unwrap()).unwrap();
        let net = books.nets(None, None).unwrap().get(&f.tax).map(|n| -n.0.cents()).unwrap_or(0);
        assert_eq!(r["total"]["balance_cents"].as_i64().unwrap() as i128, net, "{basis:?} on {end}");
        let parts: i64 = r["agencies"].as_array().unwrap().iter().map(|a| a["balance_cents"].as_i64().unwrap()).sum::<i64>() + r["unlinked"]["balance_cents"].as_i64().unwrap_or(0);
        assert_eq!(parts as i128, net, "agency rows add up to the total ({basis:?} on {end})");
    }
}

#[test]
fn cash_basis_owes_tax_as_customers_pay_and_agrees_with_the_reports() {
    let mut f = Fx::new();
    let group = f.group.clone();
    f.set_basis("CASH");
    let inv = f.invoice("2026-07-10", "200.00", Some(&group)); // 213.00 total, 13.00 tax
    let r = f.owed(Some("2026-07-01"), "2026-09-30");
    assert_eq!(r["basis"], "CASH");
    assert_eq!(r["total"]["collected"], "0.00", "nothing is owed until the customer pays");
    f.receive(&inv, "2026-08-01", "106.50");
    let r = f.owed(Some("2026-07-01"), "2026-09-30");
    assert_eq!(r["total"]["collected"], "6.50");
    assert_eq!(row(&r, &f.state_agency())["collected"], "4.00");
    assert_eq!(row(&r, &f.county_agency())["collected"], "2.50");
    f.pay(&f.input(&f.state_agency(), "2026-10-10", "4.00")).unwrap();
    let r = f.owed(Some("2026-10-01"), "2026-10-31");
    assert_eq!(row(&r, &f.state_agency())["balance"], "0.00");
    assert_eq!(r["total"]["balance"], "2.50");
    f.receive(&inv, "2026-10-20", "106.50");
    f.invoice("2026-10-21", "40.00", None);
    for end in ["2026-07-31", "2026-08-31", "2026-10-15", "2026-10-31"] {
        assert_matches_reports(&mut f, end);
    }
}

#[test]
fn the_control_guard_stays_closed_for_journals_after_payments_exist() {
    let mut f = Fx::new();
    let state = f.state.clone();
    f.invoice("2026-08-10", "100.00", Some(&state));
    f.pay(&f.input(&f.state_agency(), "2026-10-15", "4.00")).unwrap();
    let draft = services::save_draft(&mut f.book, f.actor, f.co, DraftInput::new("2026-10-20", "Adjust", vec![line(f.tax).debit("1.00").credit("0").build(), line(f.bank).debit("0").credit("1.00").build()]));
    refused(draft, "paid down by recording a sales tax payment");
    // The generic journal actions cannot reverse a payment's journal either.
    let jid: String = f.book.connection().query_row("SELECT journal_id FROM salestax_payment", [], |r| r.get(0)).unwrap();
    refused(services::reverse_journal(&mut f.book, f.actor, f.co, jid.parse().unwrap(), "2026-10-21", "x"), "owning source document");
}

// Database backstops, attacked directly ------------------------------------------------------------------

/// Stage a forged STP journal and its evidence inside a rolled-back write, then run `attack`.
fn forged(f: &mut Fx, lines: Vec<Value>, mapping: Value, attack: impl FnOnce(&Writer<'_>, &str, &str, &str)) {
    let (actor, co) = (f.actor, f.co);
    let result: FbResult<()> = f.book.write(|tx| {
        let identity = documents::create_identity(tx, actor, co, KIND, &new_id()).unwrap();
        let iid = s(&identity, "id").to_owned();
        documents::snapshot_source_accounts(tx, actor, co, &iid, &mapping).unwrap();
        let j = documents::post_source(tx, actor, co, &iid, "2026-10-15", "Forged", lines).unwrap();
        attack(tx, &iid, &j.id.to_db(), &actor.to_db());
        Err(FbError::validation("rolled back"))
    });
    assert!(result.is_err());
}
struct Ids {
    co: String,
    agency: String,
}
fn row_sql(ids: &Ids, iid: &str, jid: &str, actor: &str, tax_account: &str, tax: i64, bank: &str) -> String {
    format!("INSERT INTO salestax_payment(id,company_id,identity_id,creation_digest,review_digest,agency_id,agency_name,bank_id,tax_account_id,expense_account_id,date,tax_amount,penalty_amount,period_start_date,period_end_date,reference,memo,journal_id,actor_id,actor_name,created_at) VALUES('{}','{}','{iid}','{}','{}','{}','State Revenue','{bank}','{tax_account}',NULL,'2026-10-15',{tax},0,NULL,NULL,'','','{jid}','{actor}','Tax payer','2026-10-15T00:00:00Z')",
        new_id(), ids.co, "0".repeat(64), "0".repeat(64), ids.agency)
}

#[test]
fn triggers_refuse_forged_payments_that_rust_would_never_write() {
    let mut f = Fx::new();
    let state = f.state.clone();
    f.invoice("2026-08-10", "100.00", Some(&state));
    let (tax, bank, bank2, expense) = (f.tax.to_db(), f.bank.to_db(), f.bank2.to_db(), f.expense.to_db());
    let ok_lines = || vec![json!({"account":tax,"debit":"4.00","credit":"0.00"}), json!({"account":bank,"debit":"0.00","credit":"4.00"})];
    let ok_map = json!({"bank":bank,"collected_tax":tax});
    let ids = Ids { co: f.co.to_db(), agency: f.state_agency() };
    // An STP journal without its payment row cannot commit: the seal obligation is never met.
    let (actor, co) = (f.actor, f.co);
    let unsealed: FbResult<()> = f.book.write(|tx| {
        let identity = documents::create_identity(tx, actor, co, KIND, &new_id())?;
        documents::post_source(tx, actor, co, s(&identity, "id"), "2026-10-15", "No owner", ok_lines())?;
        Ok(())
    });
    assert!(unsealed.is_err(), "an unowned sales tax journal must not commit");
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal j JOIN documents_documentidentity i ON i.id=j.source_id WHERE i.kind='STP'"), 0);
    let expect = |tx: &Writer<'_>, sql: String, constraint: &str| {
        let e = tx.execute_batch(&sql).unwrap_err().to_string();
        assert!(e.contains(constraint), "expected {constraint}: {e}");
    };
    // A payment journal that debits another liability instead of collected tax.
    let other = services::save_account(&mut f.book, f.actor, f.co, AccountInput::new("220", "Loan").kind("LIABILITY")).unwrap().id.to_db();
    forged(&mut f, vec![json!({"account":other,"debit":"4.00","credit":"0.00"}), json!({"account":bank,"debit":"0.00","credit":"4.00"})], json!({"bank":bank,"collected_tax":other}), |tx, iid, jid, a| {
        expect(tx, row_sql(&ids, iid, jid, a, &other, 400, &bank), "salestax_payment_tax_account");
    });
    // Lines that do not match the row: part of the total is credited to an expense instead of the bank.
    forged(&mut f, vec![json!({"account":tax,"debit":"4.00","credit":"0.00"}), json!({"account":bank,"debit":"0.00","credit":"3.00"}), json!({"account":expense,"debit":"0.00","credit":"1.00"})], ok_map.clone(), |tx, iid, jid, a| {
        expect(tx, row_sql(&ids, iid, jid, a, &tax, 400, &bank), "salestax_payment_journal_split");
    });
    // A bank account that is not a designated bank.
    let other_bank = store_account(&f, "102");
    forged(&mut f, vec![json!({"account":tax,"debit":"4.00","credit":"0.00"}), json!({"account":other_bank,"debit":"0.00","credit":"4.00"})], json!({"bank":other_bank,"collected_tax":tax}), |tx, iid, jid, a| {
        expect(tx, row_sql(&ids, iid, jid, a, &tax, 400, &other_bank), "salestax_payment_bank");
    });
    // A payee that is not a tax agency.
    let vendor = directory::save_record(&mut f.book, f.actor, f.co, Model::Vendor, &json!({"display_name":"Not an agency"}), None).unwrap()["id"].as_str().unwrap().to_string();
    forged(&mut f, ok_lines(), ok_map.clone(), |tx, iid, jid, a| {
        expect(tx, row_sql(&ids, iid, jid, a, &tax, 400, &bank).replace(&ids.agency, &vendor), "salestax_payment_scope");
    });
    // Account evidence that names an extra role.
    forged(&mut f, ok_lines(), json!({"bank":bank,"collected_tax":tax,"extra":bank2}), |tx, iid, jid, a| {
        expect(tx, row_sql(&ids, iid, jid, a, &tax, 400, &bank), "salestax_payment_account_evidence");
    });
    // A seal for a journal no payment owns.
    forged(&mut f, ok_lines(), ok_map.clone(), |tx, _iid, jid, _a| {
        expect(tx, format!("INSERT INTO fb_seal VALUES('salestax_journal','{jid}')"), "salestax_journal_owner");
    });

    // Recorded history is immutable, evidence cannot be appended, and a void cannot predate its payment.
    let paid = f.pay(&f.input(&f.state_agency(), "2026-10-15", "4.00")).unwrap();
    let conn = f.book.connection();
    sql_refused(conn, "UPDATE salestax_payment SET tax_amount=1", "salestax_payment_immutable");
    sql_refused(conn, "UPDATE salestax_payment SET memo='changed'", "salestax_payment_immutable");
    sql_refused(conn, "DELETE FROM salestax_payment", "salestax_payment_immutable");
    sql_refused(conn, &format!("INSERT INTO documents_documentaccount VALUES('{}','{}','late','{}','100','Account 100','ASSET')", new_id(), s(&paid, "identity_id"), bank), "salestax_late_reference");
    let pid = s(&paid, "id").to_owned();
    let iid = s(&paid, "identity_id").to_owned();
    let early: FbResult<()> = f.book.write(|tx| {
        let j = documents::reverse_source(tx, actor, co, &iid, "2026-10-15", "Same day").unwrap();
        expect(tx, format!("INSERT INTO salestax_paymentvoid VALUES('{}','{pid}','{}','2026-10-14','Backdated','{}','Tax payer','2026-10-15T00:00:00Z')", new_id(), j.id.to_db(), actor.to_db()), "salestax_payment_void_link");
        Err(FbError::validation("rolled back"))
    });
    assert!(early.is_err());
    let problems = f.book.check().unwrap();
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn open_time_checks_catch_a_payment_whose_triggers_were_bypassed() {
    let mut f = Fx::new();
    let state = f.state.clone();
    f.invoice("2026-08-10", "100.00", Some(&state));
    f.pay(&f.input(&f.state_agency(), "2026-10-15", "4.00")).unwrap();
    assert!(f.book.check().unwrap().is_empty());
    let conn = f.book.connection();
    conn.execute_batch("PRAGMA ignore_check_constraints=ON; DROP TRIGGER fb_salestax_payment_update; UPDATE salestax_payment SET tax_amount=300;").unwrap();
    let problems = f.book.check().unwrap();
    assert!(problems.iter().any(|p| p.starts_with("invalid sales tax payments")), "{problems:?}");
}

#[test]
fn wide_tax_report_totals_preserve_exact_cents_and_sign() {
    for n in [i128::from(i64::MAX) + 1, i128::from(i64::MIN) - 1] {
        let value = Figures {
            opening: Money::from_cents(n), collected: Money::from_db(123), paid: Money::from_db(23),
        }.json();
        assert_eq!(report_cents(&value["opening_cents"]), Some(n));
        assert_eq!(report_cents(&value["balance_cents"]), Some(n + 100));
        assert_eq!(value["balance"], money(n + 100));
    }
}

#[test]
fn reopening_refuses_lost_tax_owners_and_changed_account_evidence_with_all_guards_restored() {
    for attack in ["payment", "void", "roles", "account"] {
        let mut f = Fx::new();
        let state = f.state.clone();
        f.invoice("2026-08-10", "100.00", Some(&state));
        let paid = f.pay(&f.input(&f.state_agency(), "2026-10-15", "4.00")).unwrap();
        if attack == "void" {
            void_payment(&mut f.book, f.actor, f.co, s(&paid, "id"), Some(1), &json!("2026-10-16"), &json!("Synthetic correction")).unwrap();
        }
        assert!(f.book.check().unwrap().is_empty());
        let conn = f.book.connection();
        let path = std::path::PathBuf::from(conn.path().unwrap());
        let triggers = conn.prepare("SELECT name,sql FROM sqlite_schema WHERE type='trigger'").unwrap()
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
        for (name, _) in &triggers { conn.execute_batch(&format!("DROP TRIGGER {name}")).unwrap(); }
        match attack {
            "payment" => { conn.execute("DELETE FROM salestax_payment", []).unwrap(); }
            "void" => { conn.execute("DELETE FROM salestax_paymentvoid", []).unwrap(); }
            "roles" => { conn.execute("UPDATE documents_documentaccount SET role='renamed-'||role WHERE source_id=?1", [s(&paid, "identity_id")]).unwrap(); }
            "account" => { conn.execute("UPDATE documents_documentaccount SET account_id=?1 WHERE source_id=?2 AND role='bank'", params![f.bank2.to_db(), s(&paid, "identity_id")]).unwrap(); }
            _ => unreachable!(),
        }
        for (_, sql) in &triggers { conn.execute_batch(sql).unwrap(); }
        let invalid: i64 = conn.query_row("SELECT count(*) FROM salestax_payment_invalid", [], |r| r.get(0)).unwrap();
        assert!(invalid > 0, "{attack}: semantic evidence must find the corruption");
        let problems = f.book.check().unwrap();
        assert!(problems.iter().any(|p| p.starts_with("invalid sales tax payments")), "{attack}: {problems:?}");
        assert!(!problems.iter().any(|p| p.contains("schema definition")), "restored exact guards: {problems:?}");
        drop(f);
        assert!(Book::open(&path, &[31;32]).is_err(), "{attack}: corrupted book must refuse to reopen");
    }
}

/// Worker item 26: Pay bills suggested check 127, which had gone out on a sales tax payment whose
/// Reference was "Check 127" (that form has no check-number box). The next suggested check number steps
/// over a number a tax payment's reference names, on that bank only, and other references (a
/// confirmation number) never move it.
#[test]
fn the_next_check_number_steps_over_one_a_tax_payment_reference_names() {
    let mut f = Fx::new();
    let group = f.group.clone();
    f.invoice("2026-08-10", "1000.00", Some(&group));
    let (bank1, bank2) = (f.bank, f.bank2);
    let next = |f: &mut Fx, bank: AccountId| crate::checks::next_number(&mut f.book, f.actor, f.co, bank).unwrap();
    assert_eq!(next(&mut f, bank1), "1");
    let mut p = f.input(&f.state_agency(), "2026-10-15", "5.00");
    p["reference"] = json!("Check 1");
    f.pay(&p).unwrap();
    let mut p = f.input(&f.state_agency(), "2026-10-16", "5.00");
    p["reference"] = json!("Confirmation 88812345");
    f.pay(&p).unwrap();
    assert_eq!(next(&mut f, bank1), "2");
    assert_eq!(next(&mut f, bank2), "1", "another bank account is not affected");
    // A warning can name it before the number is used again.
    let bank = f.bank.to_db();
    let earlier = |f: &Fx, n: &str| crate::checks::earlier_use(f.book.connection(), f.co, &bank, n).unwrap();
    let live = earlier(&f, "1").unwrap();
    assert!(live.is_tax_payment());
    assert_eq!(live.text(), "sales tax payment STP-000001 (reference “Check 1”)");
    assert_eq!(earlier(&f, "2"), None);
    // Review 2, S1: a voided tax payment says so.
    let id: String = f.book.connection().query_row("SELECT id FROM salestax_payment WHERE reference='Check 1'", [], |r| r.get(0)).unwrap();
    void_payment(&mut f.book, f.actor, f.co, &id, Some(1), &json!("2026-10-20"), &json!("Check lost in the mail")).unwrap();
    let voided = earlier(&f, "1").unwrap();
    assert!(voided.is_tax_payment());
    assert_eq!(voided.text(), "sales tax payment STP-000001 (reference “Check 1”), which was voided");
}

/// Review 2, S2: with a check numbering preference set, the next suggested number (`check_next`) also
/// steps over a number a sales tax payment's Reference names, keeping the preference's width.
#[test]
fn a_numbering_preference_steps_over_a_tax_payment_reference_too() {
    let mut f = Fx::new();
    let group = f.group.clone();
    f.invoice("2026-08-10", "1000.00", Some(&group));
    // Changing numbering needs an administrator.
    f.book.connection().execute("UPDATE fb_user SET is_staff=1 WHERE id=?1", [f.actor.to_db()]).unwrap();
    let bank = f.bank;
    let before = crate::numbering::settings(&mut f.book, f.actor, f.co).unwrap().as_array().unwrap().iter()
        .find(|r| r["kind"] == "CHECK" && r["bank_id"] == json!(bank.to_db())).unwrap().clone();
    crate::numbering::set_next(&mut f.book, f.actor, f.co, crate::numbering::Change { kind: "CHECK", bank: Some(bank), next: "0500", expected_next: s(&before, "next"), expected_revision: before["revision"].as_i64().unwrap() }).unwrap();
    let next = |f: &mut Fx| crate::checks::next_number(&mut f.book, f.actor, f.co, bank).unwrap();
    assert_eq!(next(&mut f), "0500");
    let mut p = f.input(&f.state_agency(), "2026-10-15", "5.00");
    p["reference"] = json!("Check 0500");
    f.pay(&p).unwrap();
    assert_eq!(next(&mut f), "0501");
    let mut p = f.input(&f.state_agency(), "2026-10-16", "5.00");
    p["reference"] = json!("Check 501");
    f.pay(&p).unwrap();
    assert_eq!(next(&mut f), "0502");
    assert_eq!(crate::checks::next_number(&mut f.book, f.actor, f.co, f.bank2).unwrap(), "1", "another bank account is not affected");
}

fn breakdown_row<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["breakdown"].as_array().unwrap().iter().find(|row| row["name"] == name)
        .unwrap_or_else(|| panic!("Missing {name} in {report}"))
}

#[test]
fn breakdown_uses_frozen_jurisdictions_and_splits_taxable_and_exempt_on_both_bases() {
    let mut f = Fx::new();
    let group = f.group.clone();
    let invoice = f.invoice_with_lines("2027-12-01", json!([
        {"description":"Taxable work","quantity":"1","rate":"100.00","taxable":true,"income_account_id":f.income.to_db()},
        {"description":"Exempt work","quantity":"1","rate":"30.00","taxable":false,"income_account_id":f.income.to_db()}
    ]), Some(&group));
    f.receive(&invoice, "2028-02-29", "68.25");
    f.receive(&invoice, "2028-03-01", "68.25");
    f.invoice("2027-11-30", "100.00", Some(&group));
    f.invoice("2028-03-01", "100.00", Some(&group));
    let direct_sale = json!({"request_key":new_id(),"customer_id":f.customer,"date":"2028-02-29",
        "method":"CASH","destination":"BANK","bank_id":f.bank.to_db(),"tax_mode":"NONE",
        "lines":[{"item_id":null,"description":"No tax","quantity":"1","rate":"12.34","taxable":true,"income_account_id":f.income.to_db()}]});
    let review = sales::review_sales_receipt(&mut f.book, f.actor, f.co, &direct_sale).unwrap();
    sales::record_sales_receipt(&mut f.book, f.actor, f.co, &direct_sale, Some(&review)).unwrap();
    let agency = f.state_agency();
    salestax::save_rate(&mut f.book, f.actor, f.co, &json!({
        "item_id":f.state["id"],"expected_revision":f.state["revision"],"name":"Renamed state",
        "description":"Updated state tax","rate":"5","agency_id":agency
    })).unwrap();
    let (start, end) = (Some("2027-12-01"), "2028-02-29");
    let row_amounts = |report: &Value, name| {
        let row = breakdown_row(report, name);
        (row["taxable"].clone(), row["exempt"].clone(), row["collected"].clone())
    };
    let accrual = f.owed(start, end);
    assert_eq!(breakdown_row(&accrual, "Combined")["jurisdiction"], "County Treasurer, State Revenue");
    assert_eq!(breakdown_row(&accrual, "Combined")["rate"], "6.5");
    assert_eq!(breakdown_row(&accrual, "Combined")["is_component"], false);
    assert_eq!(row_amounts(&accrual, "Combined"), (json!("100.00"), json!("30.00"), json!("6.50")));
    assert_eq!(breakdown_row(&accrual, "State")["jurisdiction"], "State Revenue");
    assert_eq!(breakdown_row(&accrual, "County")["jurisdiction"], "County Treasurer");
    assert_eq!(breakdown_row(&accrual, "County")["rate"], "2.5");
    assert_eq!(breakdown_row(&accrual, "State")["is_component"], true);
    assert_eq!(breakdown_row(&accrual, "County")["is_component"], true);
    assert_eq!(row_amounts(&accrual, "State"), (Value::Null, Value::Null, json!("4.00")));
    assert_eq!(row_amounts(&accrual, "County"), (Value::Null, Value::Null, json!("2.50")));
    assert_eq!(row_amounts(&accrual, "No tax selected"), (json!("0.00"), json!("12.34"), json!("0.00")));
    assert_eq!(accrual["breakdown_total"], json!({"taxable":"100.00","exempt":"42.34","collected":"6.50"}));
    assert_eq!(accrual["total"]["collected"], "6.50");
    f.set_basis("CASH");
    let cash = f.owed(start, end);
    assert_eq!(row_amounts(&cash, "Combined"), (json!("50.00"), json!("15.00"), json!("3.25")));
    assert_eq!(row_amounts(&cash, "State"), (Value::Null, Value::Null, json!("2.00")));
    assert_eq!(row_amounts(&cash, "County"), (Value::Null, Value::Null, json!("1.25")));
    assert_eq!(row_amounts(&cash, "No tax selected"), (json!("0.00"), json!("12.34"), json!("0.00")));
    assert_eq!(cash["breakdown_total"], json!({"taxable":"50.00","exempt":"27.34","collected":"3.25"}));
    assert_eq!(cash["total"]["collected"], "3.25");

    let immediate = f.owed(Some("2028-03-01"), "2028-03-01");
    assert_eq!(row_amounts(&immediate, "Combined"), (json!("50.00"), json!("15.00"), json!("3.25")));
    assert_eq!(row_amounts(&immediate, "State"), (Value::Null, Value::Null, json!("2.00")));
    assert_eq!(row_amounts(&immediate, "County"), (Value::Null, Value::Null, json!("1.25")));
}

#[test]
fn cash_breakdown_allocates_the_same_cents_across_partial_payments_and_period_boundaries() {
    let mut f = Fx::new();
    let group = f.group.clone();
    let invoice = f.invoice("2027-12-01", "0.25", Some(&group));
    f.receive(&invoice, "2027-12-02", "0.14");
    f.receive(&invoice, "2027-12-03", "0.13");
    f.set_basis("CASH");
    let first = f.owed(Some("2027-12-02"), "2027-12-02");
    let second = f.owed(Some("2027-12-03"), "2027-12-03");
    let whole = f.owed(Some("2027-12-02"), "2027-12-03");
    assert_eq!(breakdown_row(&whole, "Combined")["taxable"], "0.25");
    assert_eq!(breakdown_row(&whole, "Combined")["collected"], "0.02");
    for (name, agency) in [("State", f.state_agency()), ("County", f.county_agency())] {
        assert_eq!(breakdown_row(&whole, name)["collected"], "0.01");
        assert_eq!(breakdown_row(&whole, name)["taxable"], Value::Null);
        assert_eq!(row(&whole, &agency)["collected"], "0.01");
        let before = report_cents(&row(&first, &agency)["collected_cents"]).unwrap();
        let after = report_cents(&row(&second, &agency)["collected_cents"]).unwrap();
        assert_eq!(before + after, 1);
    }
    assert_eq!(whole["total"]["collected"], "0.02");
}

#[test]
fn positive_cash_receipt_never_reduces_an_agencys_recognized_tax_and_reversal_releases_exact_cents() {
    let mut f = Fx::new();
    let r1 = salestax::save_rate(&mut f.book, f.actor, f.co, &json!({
        "name": "Local A", "description": "Local A", "rate": "1", "new_agency_name": "Agency A", "expected_revision": 0
    })).unwrap();
    let r2 = salestax::save_rate(&mut f.book, f.actor, f.co, &json!({
        "name": "Local B", "description": "Local B", "rate": "3", "new_agency_name": "Agency B", "expected_revision": 0
    })).unwrap();
    let r3 = salestax::save_rate(&mut f.book, f.actor, f.co, &json!({
        "name": "Local C", "description": "Local C", "rate": "3", "new_agency_name": "Agency C", "expected_revision": 0
    })).unwrap();
    let trio = salestax::save_group(&mut f.book, f.actor, f.co, &json!({
        "name": "Trio", "description": "Trio", "member_ids": [r1["id"], r2["id"], r3["id"]], "expected_revision": 0
    })).unwrap();
    f.set_basis("CASH");
    let inv = f.invoice("2027-12-01", "1.00", Some(&trio)); // 1.07 total, 7 cents tax [1, 3, 3]
    f.receive(&inv, "2027-12-02", "0.46"); // brings recognized tax to 3 cents
    let p2_input = json!({
        "request_key": new_id(), "customer_id": f.customer, "date": "2027-12-03", "method": "CASH",
        "destination": "BANK", "bank_id": f.bank.to_db(), "amount": "0.15",
        "allocations": [{"invoice_id": inv, "amount": "0.15"}]
    });
    let p2_rev = sales::review_receipt(&mut f.book, f.actor, f.co, &p2_input).unwrap();
    let p2 = sales::record_receipt(&mut f.book, f.actor, f.co, &p2_input, Some(&p2_rev)).unwrap(); // brings recognized tax from 3 cents to 4 cents
    let agencies = [
        s(&r1, "agency_id").to_owned(),
        s(&r2, "agency_id").to_owned(),
        s(&r3, "agency_id").to_owned(),
    ];
    let day1 = f.owed(Some("2027-12-02"), "2027-12-02");
    let day2 = f.owed(Some("2027-12-03"), "2027-12-03");
    let mut day2_deltas = Vec::new();
    for agency in &agencies {
        let c1 = report_cents(&row(&day1, agency)["collected_cents"]).unwrap();
        let c2 = report_cents(&row(&day2, agency)["collected_cents"]).unwrap();
        assert!(c1 >= 0, "day1 agency {agency} went negative: {c1}");
        assert!(c2 >= 0, "day2 agency {agency} went negative on positive receipt: {c2}");
        day2_deltas.push(c2);
    }
    assert_eq!(day2_deltas.iter().sum::<i128>(), 1);
    // Reversing the second receipt on day 3 releases the exact same agency cent.
    sales::reverse_receipt(
        &mut f.book,
        f.actor,
        f.co,
        s(&p2, "id"),
        Some(1),
        &json!("2027-12-04"),
        &json!("Undo second receipt"),
    )
    .unwrap();
    let day3 = f.owed(Some("2027-12-04"), "2027-12-04");
    for (agency, delta) in agencies.iter().zip(&day2_deltas) {
        let c3 = report_cents(&row(&day3, agency)["collected_cents"]).unwrap();
        assert_eq!(c3, -*delta, "reversal should release exact same cent for {agency}");
    }
}

#[test]
fn same_agency_tax_group_reports_sales_base_once_and_component_tax_separately() {
    let mut f = Fx::new();
    let agency = f.state_agency();
    let part_a = salestax::save_rate(&mut f.book, f.actor, f.co, &json!({
        "name": "NY State", "description": "NY State", "rate": "4", "agency_id": agency, "expected_revision": 0
    })).unwrap();
    let part_b = salestax::save_rate(&mut f.book, f.actor, f.co, &json!({
        "name": "NY Local", "description": "NY Local", "rate": "4", "agency_id": agency, "expected_revision": 0
    })).unwrap();
    let ny_group = salestax::save_group(&mut f.book, f.actor, f.co, &json!({
        "name": "NY Combined", "description": "NY Combined", "member_ids": [part_a["id"], part_b["id"]], "expected_revision": 0
    })).unwrap();
    f.invoice_with_lines("2027-12-05", json!([
        {"description":"Taxable","quantity":"1","rate":"100.00","taxable":true,"income_account_id":f.income.to_db()},
        {"description":"Exempt","quantity":"1","rate":"30.00","taxable":false,"income_account_id":f.income.to_db()}
    ]), Some(&ny_group));
    let report = f.owed(Some("2027-12-01"), "2028-02-29");
    let group_row = breakdown_row(&report, "NY Combined");
    assert_eq!(group_row["jurisdiction"], "State Revenue");
    assert_eq!(group_row["taxable"], "100.00");
    assert_eq!(group_row["exempt"], "30.00");
    assert_eq!(group_row["collected"], "8.00");
    assert_eq!(group_row["is_component"], false);
    for comp in ["NY State", "NY Local"] {
        let r = breakdown_row(&report, comp);
        assert_eq!(r["jurisdiction"], "State Revenue");
        assert_eq!(r["taxable"], Value::Null);
        assert_eq!(r["exempt"], Value::Null);
        assert_eq!(r["collected"], "4.00");
        assert_eq!(r["is_component"], true);
    }
    assert_eq!(report["breakdown_total"], json!({"taxable":"100.00","exempt":"30.00","collected":"8.00"}));
}

#[test]
fn accrual_breakdown_tracks_credit_memo_and_void_in_their_own_periods() {
    let mut f = Fx::new();
    let input = json!({"request_key":new_id(),"customer_id":f.customer,"date":"2027-11-30",
        "tax_mode":"ITEM","tax_item_id":f.state["id"],"lines":[
            {"item_id":null,"description":"Taxed return","quantity":"1","rate":"25.00","taxable":true,"income_account_id":f.income.to_db()},
            {"item_id":null,"description":"Exempt return","quantity":"1","rate":"3.00","taxable":false,"income_account_id":f.income.to_db()}
        ]});
    let review = crate::credits::review_credit_memo(&mut f.book, f.actor, f.co, &input).unwrap();
    let credit = crate::credits::record_credit_memo(&mut f.book, f.actor, f.co, &input, Some(&review)).unwrap();
    crate::credits::void_credit_memo(&mut f.book, f.actor, f.co, s(&credit, "id"), Some(1),
        &json!("2027-12-01"), &json!("Synthetic void")).unwrap();

    for (date, taxable, exempt, collected) in [
        ("2027-11-30", "-25.00", "-3.00", "-1.00"),
        ("2027-12-01", "25.00", "3.00", "1.00"),
    ] {
        let report = f.owed(Some(date), date);
        let state = breakdown_row(&report, "State");
        assert_eq!(state["taxable"], taxable);
        assert_eq!(state["exempt"], exempt);
        assert_eq!(state["collected"], collected);
        assert_eq!(report["total"]["collected"], collected);
    }
    f.set_basis("CASH");
    let cash = f.owed(Some("2027-11-30"), "2027-12-01");
    assert_eq!(cash["breakdown"], json!([]));
}
