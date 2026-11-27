//! A synthetic company with one customer, for the receipt settlement tests: posted invoices,
//! receipts, credit memos and customer deposits, each through the ordinary services.
#![allow(dead_code)]
use fb_core::{
    Book, CompanyId, UserId, credits,
    directory::{self, Model},
    documents, prepayments, sales,
    services::{self, AccountInput},
    users,
};
use serde_json::{Value, json};

pub const REFUSAL: &str = "Receipt precondition failed. Nothing was recorded. Review the invoice again.";

/// One change placed between a review and its recording: the books, the invoice and what was
/// already on it (a receipt, a credit application, a deposit application).
pub type Change = fn(&mut Books, &str, &[Value; 3]);

pub struct Books {
    pub book: Book,
    pub actor: UserId,
    pub company: CompanyId,
    pub customer: String,
    pub bank: String,
    pub income: String,
    _file: fb_db::test_support::TempBookPath,
}
pub fn key() -> String {
    uuid::Uuid::new_v4().to_string()
}
pub fn id(value: &Value) -> &str {
    value["id"].as_str().unwrap()
}
/// A company with its accounts, accounting setup and one customer: `(company, customer, bank, income)`.
fn open_company(book: &mut Book, actor: UserId, name: &str) -> (CompanyId, String, String, String) {
    let company = services::create_company(book, actor, name, "USD", false).unwrap().id;
    let mut account = |code: &str, kind: &str, detail: &str| {
        services::save_account(book, actor, company, AccountInput::new(code, code).kind(kind).detail(detail))
            .unwrap().id.to_db()
    };
    let bank = account("100", "ASSET", "BANK");
    let receivables = account("110", "ASSET", "RECEIVABLE");
    let undeposited = account("120", "ASSET", "OTHER_CURRENT_ASSET");
    let income = account("400", "INCOME", "INCOME");
    let deposits = account("230", "LIABILITY", "OTHER_CURRENT_LIABILITY");
    documents::save_setup(book, actor, company,
        &json!({"receivables":receivables,"undeposited":undeposited,"customer_deposits":deposits,"banks":[bank]}), Some(0)).unwrap();
    let customer = id(&directory::save_record(book, actor, company, Model::Customer,
        &json!({"display_name":"Synthetic Orchard"}), None).unwrap()).to_owned();
    (company, customer, bank, income)
}
fn lines(income: &str, amount: &str) -> Value {
    json!([{"item_id":null,"description":"Synthetic work","quantity":"1","rate":amount,"taxable":false,"income_account_id":income}])
}
/// A posted invoice dated 2026-11-01.
fn post_invoice(book: &mut Book, actor: UserId, company: CompanyId, customer: &str, income: &str, total: &str) -> String {
    let input = json!({"request_key":key(),"customer_id":customer,"date":"2026-11-01","memo":"Synthetic invoice","tax_mode":"NONE","lines":lines(income, total)});
    let saved = sales::save_invoice(book, actor, company, &input).unwrap();
    let review = sales::review_invoice(book, actor, company, id(&saved), Some(1)).unwrap();
    sales::post_invoice(book, actor, company, id(&saved), Some(1), Some(&review)).unwrap();
    id(&saved).to_owned()
}
impl Books {
    pub fn new() -> Self {
        let file = fb_db::test_support::TempBookPath::new("receipt-settlement");
        let mut book = file.book(&[57; 32]);
        let actor = users::create_user(&mut book, "Settlement tester", false).unwrap();
        let (company, customer, bank, income) = open_company(&mut book, actor, "Synthetic settlement");
        Self { book, actor, company, customer, bank, income, _file: file }
    }
    fn lines(&self, amount: &str) -> Value {
        lines(&self.income, amount)
    }
    /// A posted invoice dated 2026-11-01.
    pub fn invoice(&mut self, total: &str) -> String {
        post_invoice(&mut self.book, self.actor, self.company, &self.customer, &self.income, total)
    }
    /// A posted 200.00 invoice of a second company in the same book, with a 50.00 receipt on it.
    pub fn other_company_invoice(&mut self) -> String {
        let (company, customer, bank, income) = open_company(&mut self.book, self.actor, "Synthetic neighbour");
        let invoice = post_invoice(&mut self.book, self.actor, company, &customer, &income, "200.00");
        let mut input = self.payment(&[(&invoice, "50.00")]);
        input["customer_id"] = json!(customer);
        input["bank_id"] = json!(bank);
        let review = sales::review_receipt(&mut self.book, self.actor, company, &input).unwrap();
        sales::record_receipt(&mut self.book, self.actor, company, &input, Some(&review)).unwrap();
        invoice
    }
    /// The values of a check receipt dated 2026-11-05 into the bank.
    pub fn payment(&self, allocations: &[(&str, &str)]) -> Value {
        let cents: i64 = allocations.iter().map(|(_, amount)| amount.replace('.', "").parse::<i64>().unwrap()).sum();
        json!({"request_key":key(),"customer_id":self.customer,"date":"2026-11-05","method":"CHECK","reference":"Check 5521","memo":"",
            "destination":"BANK","bank_id":self.bank,"amount":format!("{}.{:02}", cents / 100, cents % 100),
            "allocations":allocations.iter().map(|(invoice, amount)| json!({"invoice_id":invoice,"amount":amount})).collect::<Vec<_>>()})
    }
    /// A receipt entered natively: reviewed and recorded at once, without the key.
    pub fn receive(&mut self, invoice: &str, amount: &str) -> Value {
        let input = self.payment(&[(invoice, amount)]);
        let review = sales::review_receipt(&mut self.book, self.actor, self.company, &input).unwrap();
        sales::record_receipt(&mut self.book, self.actor, self.company, &input, Some(&review)).unwrap()
    }
    pub fn reverse(&mut self, receipt: &Value) {
        sales::reverse_receipt(&mut self.book, self.actor, self.company, id(receipt), Some(1), &json!("2026-11-06"), &json!("Synthetic bounced check")).unwrap();
    }
    /// A new credit memo applied to the invoice on 2026-11-04. Returns the application.
    pub fn credit(&mut self, invoice: &str, amount: &str) -> Value {
        let input = json!({"request_key":key(),"customer_id":self.customer,"date":"2026-11-03","memo":"Synthetic return","tax_mode":"NONE","lines":self.lines(amount)});
        let review = credits::review_credit_memo(&mut self.book, self.actor, self.company, &input).unwrap();
        let memo = credits::record_credit_memo(&mut self.book, self.actor, self.company, &input, Some(&review)).unwrap();
        let input = json!({"request_key":key(),"date":"2026-11-04","allocations":[{"invoice_id":invoice,"amount":amount}]});
        let review = credits::review_application(&mut self.book, self.actor, self.company, id(&memo), &input).unwrap();
        credits::apply_credit(&mut self.book, self.actor, self.company, id(&memo), &input, Some(&review)).unwrap()
    }
    pub fn unapply_credit(&mut self, application: &Value) {
        credits::unapply_credit(&mut self.book, self.actor, self.company, id(application), Some(1), &json!("2026-11-06"), &json!("Synthetic correction")).unwrap();
    }
    /// A new customer deposit applied to the invoice on 2026-11-02. Returns the application.
    pub fn deposit(&mut self, invoice: &str, amount: &str) -> Value {
        let input = json!({"request_key":key(),"customer_id":self.customer,"date":"2026-10-20","amount":amount,"method":"CHECK","destination":"UNDEPOSITED","order_reference":"SO-7"});
        let review = prepayments::review_deposit(&mut self.book, self.actor, self.company, &input).unwrap();
        let deposit = prepayments::record_deposit(&mut self.book, self.actor, self.company, Some(&review), &input).unwrap();
        let input = json!({"request_key":key(),"invoice_id":invoice,"date":"2026-11-02","amount":amount});
        let review = prepayments::review_application(&mut self.book, self.actor, self.company, id(&deposit), &input).unwrap();
        prepayments::apply_deposit(&mut self.book, self.actor, self.company, id(&deposit), Some(&review), &input).unwrap()
    }
    pub fn unapply_deposit(&mut self, application: &Value) {
        prepayments::unapply_deposit(&mut self.book, self.actor, self.company, id(application), Some(1), &json!("2026-11-06"), &json!("Synthetic correction")).unwrap();
    }
    /// Every row of every table.
    pub fn everything(&self) -> fb_db::test_support::Snapshot {
        fb_db::test_support::snapshot(self.book.connection()).unwrap()
    }
    pub fn count(&mut self, rows: &str) -> i64 {
        self.book.read(|q| q.query_row(&format!("SELECT count(*) FROM {rows}"), [], |row| row.get(0))).unwrap()
    }
    /// `[journals, receipts, allocations, "receipt.recorded" audit events]`: one more of each for
    /// every recorded receipt.
    pub fn counts(&mut self) -> [i64; 4] {
        ["ledger_journal", "sales_receipt", "sales_receiptallocation", "ledger_auditevent WHERE action='receipt.recorded'"]
            .map(|rows| self.count(rows))
    }
    /// The invoice's outstanding amount as the invoice list reports it.
    pub fn outstanding(&mut self, invoice: &str) -> String {
        sales::invoice_balances(&mut self.book, self.actor, self.company).unwrap().into_iter()
            .find(|row| row["invoice_id"] == invoice).unwrap()["outstanding"].as_str().unwrap().to_owned()
    }
    /// Each change the tests place between a review and its recording, on an invoice that already
    /// has `seen` (a receipt), `credit` and `deposit` applied.
    pub fn changes() -> Vec<(&'static str, Change)> {
        vec![
            ("a receipt is added", |books, invoice, _| { books.receive(invoice, "10.00"); }),
            ("a credit is applied", |books, invoice, _| { books.credit(invoice, "10.00"); }),
            ("a customer deposit is applied", |books, invoice, _| { books.deposit(invoice, "10.00"); }),
            ("a receipt is reversed", |books, _, seen| books.reverse(&seen[0])),
            ("a credit is unapplied", |books, _, seen| books.unapply_credit(&seen[1])),
            ("a customer deposit is unapplied", |books, _, seen| books.unapply_deposit(&seen[2])),
        ]
    }
    /// A 200.00 invoice with a 30.00 receipt, a 40.00 credit and a 25.00 customer deposit: 105.00 outstanding.
    pub fn settled_invoice(&mut self) -> (String, [Value; 3]) {
        let invoice = self.invoice("200.00");
        let seen = [self.receive(&invoice, "30.00"), self.credit(&invoice, "40.00"), self.deposit(&invoice, "25.00")];
        (invoice, seen)
    }
}
