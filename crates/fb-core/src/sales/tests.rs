use super::*;
#[path = "estimate_tests.rs"]
mod estimates_d4;
#[path = "independent_review.rs"]
mod independent_review;
#[path = "review_hardening.rs"]
mod review_hardening;
#[path = "l3b_tests.rs"]
mod l3b;
use crate::{services::AccountInput, users};

struct Fixture {
    book: Book,
    actor: UserId,
    company: CompanyId,
    customer: String,
    income: AccountId,
    ar: AccountId,
    bank: AccountId,
    undeposited: AccountId,
    tax: AccountId,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("fb-sales-{}.fbbook", new_id()));
        let mut book = Book::create(&path, &[13; 32]).unwrap();
        let actor = users::create_user(&mut book, "Sales tester", false).unwrap();
        let company = services::create_company(&mut book, actor, "Synthetic sales", "USD", false)
            .unwrap()
            .id;
        fn acct(b: &mut Book, u: UserId, c: CompanyId, code: &str, kind: &str) -> AccountId {
            let mut input = AccountInput::new(code, code);
            input.kind = Some(kind.into());
            services::save_account(b, u, c, input).unwrap().id
        }
        let income = acct(&mut book, actor, company, "400", "INCOME");
        let ar = acct(&mut book, actor, company, "110", "ASSET");
        let bank = acct(&mut book, actor, company, "100", "ASSET");
        let undeposited = acct(&mut book, actor, company, "120", "ASSET");
        let tax = acct(&mut book, actor, company, "210", "LIABILITY");
        documents::save_setup(&mut book,actor,company,&json!({"receivables":ar.to_db(),"undeposited":undeposited.to_db(),"collected_tax":tax.to_db(),"banks":[bank.to_db()]}),Some(0)).unwrap();
        let customer = directory::save_record(
            &mut book,
            actor,
            company,
            Model::Customer,
            &json!({"display_name":"Synthetic customer"}),
            None,
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        initialize(&book).unwrap();
        Self {
            book,
            actor,
            company,
            customer,
            income,
            ar,
            bank,
            undeposited,
            tax,
        }
    }
    fn input(&self) -> Value {
        json!({"request_key":new_id(),"customer_id":self.customer,"date":"2026-10-01","memo":"Synthetic invoice","tax_mode":"MANUAL","tax_rate":"6.25","tax_label":"Combined tax","lines":[{"item_id":null,"description":"Work","quantity":"1","rate":"4.01","taxable":true,"income_account_id":self.income.to_db()}]})
    }
    fn save(&mut self) -> Value {
        let input = self.input();
        save_invoice(&mut self.book, self.actor, self.company, &input).unwrap()
    }
    fn posted(&mut self) -> Value {
        let inv = self.save();
        let key = inv["id"].as_str().unwrap();
        let r = review_invoice(&mut self.book, self.actor, self.company, key, Some(1)).unwrap();
        post_invoice(
            &mut self.book,
            self.actor,
            self.company,
            key,
            Some(1),
            Some(&r),
        )
        .unwrap()
    }
    fn sale_input(&self) -> Value {
        let mut input = self.input();
        input["method"] = json!("CASH");
        input["destination"] = json!("UNDEPOSITED");
        input
    }
    fn payment(&self, key: &str, amount: &str) -> Value {
        json!({"request_key":new_id(),"customer_id":self.customer,"date":"2026-10-02","method":"CHECK","destination":"BANK","bank_id":self.bank.to_db(),"amount":amount,"allocations":[{"invoice_id":key,"amount":amount}]})
    }
}
#[test]
fn invoice_create_post_void_retry() {
    let mut f = Fixture::new();
    let input = f.input();
    let inv = save_invoice(&mut f.book, f.actor, f.company, &input).unwrap();
    assert_eq!(inv["current"]["total"], "4.26");
    assert_eq!(
        save_invoice(&mut f.book, f.actor, f.company, &input).unwrap()["id"],
        inv["id"]
    );
    let key = inv["id"].as_str().unwrap();
    let review = review_invoice(&mut f.book, f.actor, f.company, key, Some(1)).unwrap();
    let posted =
        post_invoice(&mut f.book, f.actor, f.company, key, Some(1), Some(&review)).unwrap();
    assert_eq!(posted["current"]["state"], "OPEN");
    assert_eq!(
        post_invoice(&mut f.book, f.actor, f.company, key, Some(1), Some(&review)).unwrap(),
        posted
    );
    let void = void_invoice(
        &mut f.book,
        f.actor,
        f.company,
        key,
        Some(2),
        &json!("2026-10-03"),
        &json!("Correction"),
    )
    .unwrap();
    assert_eq!(void["current"]["state"], "VOID");
    assert_eq!(
        void_invoice(
            &mut f.book,
            f.actor,
            f.company,
            key,
            Some(2),
            &json!("2026-10-03"),
            &json!("Correction")
        )
        .unwrap(),
        void
    );
    assert!(review_invoice(&mut f.book, f.actor, f.company, key, Some(3)).is_err());
    assert!(f.book.check().unwrap().is_empty());
    assert!(check(&f.book).unwrap().is_empty());
}
#[test]
fn payment_partial_final_reverse_dependency() {
    let mut f = Fixture::new();
    let inv = f.posted();
    let key = inv["id"].as_str().unwrap();
    let input = f.payment(key, "1.00");
    let review = review_receipt(&mut f.book, f.actor, f.company, &input).unwrap();
    let r = record_receipt(&mut f.book, f.actor, f.company, &input, Some(&review)).unwrap();
    assert_eq!(
        invoice_balances(&mut f.book, f.actor, f.company).unwrap()[0]["outstanding"],
        "3.26"
    );
    assert!(
        void_invoice(
            &mut f.book,
            f.actor,
            f.company,
            key,
            Some(2),
            &json!("2026-10-03"),
            &json!("Correction")
        )
        .is_err()
    );
    let over = f.payment(key, "3.27");
    assert!(review_receipt(&mut f.book, f.actor, f.company, &over).is_err());
    let last = f.payment(key, "3.26");
    let lr = review_receipt(&mut f.book, f.actor, f.company, &last).unwrap();
    let last = record_receipt(&mut f.book, f.actor, f.company, &last, Some(&lr)).unwrap();
    assert_eq!(
        invoice_balances(&mut f.book, f.actor, f.company).unwrap()[0]["payment_state"],
        "PAID"
    );
    for r in [&r, &last] {
        reverse_receipt(
            &mut f.book,
            f.actor,
            f.company,
            r["id"].as_str().unwrap(),
            Some(1),
            &json!("2026-10-03"),
            &json!("Correction"),
        )
        .unwrap();
    }
    void_invoice(
        &mut f.book,
        f.actor,
        f.company,
        key,
        Some(2),
        &json!("2026-10-03"),
        &json!("Correction"),
    )
    .unwrap();
    assert_eq!(
        record_receipt(&mut f.book, f.actor, f.company, &input, Some(&review)).unwrap(),
        r
    );
}
#[test]
fn sales_receipt_record_void() {
    let mut f = Fixture::new();
    let input = f.sale_input();
    let review = review_sales_receipt(&mut f.book, f.actor, f.company, &input).unwrap();
    let r = record_sales_receipt(&mut f.book, f.actor, f.company, &input, Some(&review)).unwrap();
    assert_eq!(r["kind"], "SR");
    assert_eq!(r["amount"], "4.26");
    assert!(
        reverse_receipt(
            &mut f.book,
            f.actor,
            f.company,
            r["id"].as_str().unwrap(),
            Some(1),
            &json!("2026-10-03"),
            &json!("Correction")
        )
        .is_err()
    );
    void_sales_receipt(
        &mut f.book,
        f.actor,
        f.company,
        r["id"].as_str().unwrap(),
        Some(1),
        &json!("2026-10-03"),
        &json!("Correction"),
    )
    .unwrap();
    assert_eq!(
        record_sales_receipt(&mut f.book, f.actor, f.company, &input, Some(&review)).unwrap(),
        r
    );
    assert!(
        invoice_balances(&mut f.book, f.actor, f.company)
            .unwrap()
            .is_empty()
    );
}

#[path = "attacks.rs"]
mod attacks;

#[path = "more_tests.rs"]
mod more;

#[path = "final_tests.rs"]
mod final_checks;
