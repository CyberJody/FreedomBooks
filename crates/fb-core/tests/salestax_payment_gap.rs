//! Accounting review finding 1: once a taxed invoice posts, the collected sales tax account is a control
//! account and every general-purpose entry refuses it. These tests pin that the control guard stays closed
//! for journals, checks, bills and transfers; the sales tax payment document is the one way to reduce it.
use fb_core::directory::{self, Model};
use fb_core::services::{self, AccountInput, DraftInput, line};
use fb_core::{AccountId, Book, CompanyId, UserId, checks, documents, payables, sales, salestax, transfers, users};
use serde_json::json;

struct Fx {
    book: Book,
    actor: UserId,
    co: CompanyId,
    bank: AccountId,
    bank2: AccountId,
    tax: AccountId,
    expense: AccountId,
    agency: String,
}

fn id() -> String {
    fb_ids::new_v7().to_string()
}

impl Fx {
    fn new() -> Fx {
        let path = std::env::temp_dir().join(format!("fb-taxgap-{}.fbbook", id()));
        let mut book = Book::create(&path, &[29; 32]).unwrap();
        let actor = users::create_user(&mut book, "Tax tester", false).unwrap();
        let co = services::create_company(&mut book, actor, "Synthetic tax gap", "USD", false).unwrap().id;
        let mut acct = |code: &str, kind: &str| services::save_account(&mut book, actor, co, AccountInput::new(code, code).kind(kind)).unwrap().id;
        let (income, ar, bank, bank2, ap) = (acct("400", "INCOME"), acct("110", "ASSET"), acct("100", "ASSET"), acct("101", "ASSET"), acct("200", "LIABILITY"));
        let (tax, expense) = (acct("210", "LIABILITY"), acct("600", "EXPENSE"));
        documents::save_setup(&mut book, actor, co, &json!({"receivables":ar.to_db(),"payables":ap.to_db(),"collected_tax":tax.to_db(),"banks":[bank.to_db(),bank2.to_db()]}), Some(0)).unwrap();
        let rate = salestax::save_rate(&mut book, actor, co, &json!({"name":"State","description":"State sales tax","rate":"6","new_agency_name":"State Revenue","expected_revision":0})).unwrap();
        salestax::save_settings(&mut book, actor, co, &json!({"charge_tax":true,"default_item_id":null,"owed_basis":"ACCRUAL","filing_frequency":"QUARTERLY","expected_revision":0})).unwrap();
        let customer = directory::save_record(&mut book, actor, co, Model::Customer, &json!({"display_name":"Synthetic customer"}), None).unwrap()["id"].as_str().unwrap().to_string();
        let inv = sales::save_invoice(&mut book, actor, co, &json!({"request_key":id(),"customer_id":customer,"date":"2026-10-01","memo":"Taxed sale","tax_mode":"ITEM","tax_item_id":rate["id"],"lines":[{"item_id":null,"description":"Work","quantity":"1","rate":"100.00","taxable":true,"income_account_id":income.to_db()}]})).unwrap();
        let key = inv["id"].as_str().unwrap().to_string();
        let review = sales::review_invoice(&mut book, actor, co, &key, Some(1)).unwrap();
        sales::post_invoice(&mut book, actor, co, &key, Some(1), Some(&review)).unwrap();
        Fx { book, actor, co, bank, bank2, tax, expense, agency: rate["agency_id"].as_str().unwrap().to_string() }
    }
    fn tax_balance(&self) -> i64 {
        self.book.connection().query_row("SELECT coalesce(sum(l.credit-l.debit),0) FROM ledger_journalline l JOIN ledger_journal j ON j.id=l.journal_id WHERE l.account_id=?1 AND j.status='POSTED'", [self.tax.to_db()], |r| r.get(0)).unwrap()
    }
}

fn message(e: fb_core::FbError) -> String {
    e.message()
}

#[test]
fn every_general_purpose_entry_still_refuses_the_collected_tax_account() {
    let mut f = Fx::new();
    assert_eq!(f.tax_balance(), 600, "6% of 100.00 is owed to the state");

    // A manual journal: debit the liability, credit the bank.
    let draft = services::save_draft(&mut f.book, f.actor, f.co, DraftInput::new("2026-10-20", "Pay state", vec![line(f.tax).debit("6.00").credit("0").build(), line(f.bank).debit("0").credit("6.00").build()]));
    assert!(draft.is_err() && message(draft.unwrap_err()).contains("collected sales tax control account"));

    // A check to the agency with a line on the liability.
    let p = json!({"bank_id":f.bank.to_db(),"payee_kind":"VENDOR","vendor_id":f.agency,"date":"2026-10-20","check_number":"101","memo":"","request_key":id(),"lines":[{"tab":"EXPENSE","description":"Sales tax","rate":"6.00","account_id":f.tax.to_db()}]});
    let check = match checks::save_check(&mut f.book, f.actor, f.co, &p) {
        Err(e) => Err(e),
        Ok(saved) => checks::review_check(&mut f.book, f.actor, f.co, saved["id"].as_str().unwrap(), saved["revision"].as_i64()).map(|_| ()),
    };
    assert!(check.is_err_and(|e| message(e).contains("collected sales tax control account")), "a check cannot debit collected sales tax");

    // A bill from the agency charged to the liability.
    let bill = payables::save_bill(&mut f.book, f.actor, f.co, &json!({"vendor_id":f.agency,"date":"2026-10-20","due_date":"2026-10-31","reference":"Q3","memo":"","request_key":id(),"lines":[{"description":"Sales tax","quantity":"1","rate":"6.00","expense_account_id":f.tax.to_db()}]}));
    let refused = match bill {
        Err(_) => true,
        Ok(b) => payables::review_bill(&mut f.book, f.actor, f.co, b["id"].as_str().unwrap(), b["revision"].as_i64()).is_err(),
    };
    assert!(refused, "a bill cannot be charged to collected sales tax");

    // A transfer is bank-to-bank only.
    let t = json!({"from_account_id":f.bank.to_db(),"to_account_id":f.tax.to_db(),"date":"2026-10-20","amount":"6.00","memo":"","request_key":id()});
    assert!(transfers::review_transfer(&mut f.book, f.actor, f.co, &t).is_err());

    assert_eq!(f.tax_balance(), 600, "nothing reduced the liability");
    let _ = (f.bank2, f.expense);
}


