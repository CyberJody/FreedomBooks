#[path = "fix1_tests.rs"]
mod fix1;
#[path = "review_extra.rs"]
mod review_extra;
#[path = "guards_tests.rs"]
mod guards;
#[path = "more_tests.rs"]
mod more;
#[path = "parity_tests.rs"]
mod parity;
#[path = "primitive_tests.rs"]
mod primitive;
#[path = "upgrade_tests.rs"]
mod upgrade;
use super::*;
use crate::{services::AccountInput, users};
use std::sync::atomic::{AtomicUsize, Ordering};
static NEXT: AtomicUsize = AtomicUsize::new(0);
pub(crate) struct Fx {
    book: Book,
    actor: UserId,
    co: CompanyId,
    vendor: String,
    expense: AccountId,
    ap: AccountId,
    bank2: AccountId,
    bank: AccountId,
}
impl Fx {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "checks-{}-{}.fbbook",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let mut book = Book::create(&path, &[42; 32]).unwrap();
        let actor = users::create_user(&mut book, "bill-test", true).unwrap();
        let co = services::create_company(&mut book, actor, "Synthetic bills", "USD", false)
            .unwrap()
            .id;
        let expense = services::save_account(
            &mut book,
            actor,
            co,
            AccountInput::new("500", "Expense").kind("EXPENSE"),
        )
        .unwrap()
        .id;
        let ap = services::save_account(
            &mut book,
            actor,
            co,
            AccountInput::new("200", "Payables").kind("LIABILITY"),
        )
        .unwrap()
        .id;
        let bank = services::save_account(
            &mut book,
            actor,
            co,
            AccountInput::new("100", "Bank").kind("ASSET"),
        )
        .unwrap()
        .id;
        let bank2 = services::save_account(
            &mut book,
            actor,
            co,
            AccountInput::new("101", "Savings").kind("ASSET"),
        )
        .unwrap()
        .id;
        documents::save_setup(
            &mut book,
            actor,
            co,
            &json!({"payables":ap.to_db(),"banks":[bank.to_db(),bank2.to_db()]}),
            Some(0),
        )
        .unwrap();
        let vendor = directory::save_record(
            &mut book,
            actor,
            co,
            Model::Vendor,
            &json!({"display_name":"Synthetic supplier","billing_address":"Test address"}),
            None,
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .into();
        Self {
            book,
            actor,
            co,
            vendor,
            expense,
            ap,
            bank2,
            bank,
        }
    }
    fn payload(&self) -> Value {
        json!({"bank_id":self.bank.to_db(),"payee_kind":"VENDOR","vendor_id":self.vendor,"date":"2026-10-23","check_number":"101","memo":"Synthetic test","request_key":fb_ids::new_v7().to_string(),"lines":[{"tab":"EXPENSE","description":"Cost","rate":"10.01","account_id":self.expense.to_db()}]})
    }
    fn transfer(&self) -> Value {
        json!({"from_account_id":self.bank.to_db(),"to_account_id":self.bank2.to_db(),"date":"2026-10-23","amount":"12.34","memo":"","request_key":fb_ids::new_v7().to_string()})
    }
    fn save(&mut self, p: &Value) -> Value {
        save_check(&mut self.book, self.actor, self.co, p).unwrap()
    }
    fn review(&mut self, b: &Value) -> Review {
        review_check(
            &mut self.book,
            self.actor,
            self.co,
            s(b, "id"),
            b["revision"].as_i64(),
        )
        .unwrap()
    }
    fn post(&mut self, b: &Value) -> Value {
        let r = self.review(b);
        record_check(
            &mut self.book,
            self.actor,
            self.co,
            s(b, "id"),
            b["revision"].as_i64(),
            &r,
        )
        .unwrap()
    }
    fn void(&mut self, b: &Value) -> Value {
        void_check(
            &mut self.book,
            self.actor,
            self.co,
            s(b, "id"),
            b["revision"].as_i64(),
            &json!("2026-10-24"),
            &json!("Correction"),
        )
        .unwrap()
    }
    fn counts(&self) -> Value {
        let mut v = json!({});
        for t in [
            "checks_check",
            "checks_checkrevision",
            "checks_checkline",
            "transfers_transfer",
            "transfers_transfervoid",
            "payables_bill",
            "payables_billrevision",
            "payables_billline",
            "payables_billpayment",
            "payables_paymentallocation",
            "payables_paymentreversal",
            "documents_documentidentity",
            "documents_documentaccount",
            "ledger_journal",
            "ledger_journalline",
            "ledger_auditevent",
            "fb_seal",
            "fb_seal_due",
        ] {
            v[t] = json!(
                self.book
                    .connection()
                    .query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r
                        .get::<_, i64>(0))
                    .unwrap()
            );
        }
        v["seq"] = json!(
            rows(
                self.book.connection(),
                "SELECT kind,last_number FROM documents_documentsequence ORDER BY kind",
                &[]
            )
            .unwrap()
        );
        v
    }
}

#[test]
fn check_lifecycle_and_exact_retries() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let counts = f.counts();
    assert_eq!(f.save(&p)["id"], b["id"]);
    assert_eq!(counts, f.counts());
    let r = f.review(&b);
    let b = f.post(&b);
    assert_eq!(b["current"]["total"], "10.01");
    let counts = f.counts();
    assert_eq!(
        record_check(&mut f.book, f.actor, f.co, s(&b, "id"), Some(1), &r).unwrap()["revision"],
        2
    );
    assert_eq!(counts, f.counts());
    let b = f.void(&b);
    let counts = f.counts();
    assert_eq!(
        void_check(
            &mut f.book,
            f.actor,
            f.co,
            s(&b, "id"),
            Some(2),
            &json!("2026-10-24"),
            &json!("Correction")
        )
        .unwrap()["state"],
        "VOID"
    );
    assert_eq!(counts, f.counts());
    assert!(review_check(&mut f.book, f.actor, f.co, s(&b, "id"), Some(3)).is_err());
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn print_later_number_and_void_release() {
    let mut f = Fx::new();
    let mut p = f.payload();
    p["check_number"] = json!("");
    p["print_later"] = json!(true);
    let b = f.save(&p);
    let b = f.post(&b);
    let b = assign_number(
        &mut f.book,
        f.actor,
        f.co,
        s(&b, "id"),
        Some(2),
        &json!("99999999999999999999"),
    )
    .unwrap();
    assert_eq!(b["revision"], 3);
    let counts = f.counts();
    assign_number(
        &mut f.book,
        f.actor,
        f.co,
        s(&b, "id"),
        Some(2),
        &json!("99999999999999999999"),
    )
    .unwrap();
    assert_eq!(counts, f.counts());
    assert_eq!(
        next_number(&mut f.book, f.actor, f.co, f.bank).unwrap(),
        "1"
    );
    let mut p2 = f.payload();
    p2["check_number"] = json!("99999999999999999999");
    assert!(save_check(&mut f.book, f.actor, f.co, &p2).is_err());
    f.void(&b);
    f.save(&p2);
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn draft_edit_snapshots_item_rounding_and_stale_review() {
    let mut f = Fx::new();
    let item=directory::save_record(&mut f.book,f.actor,f.co,Model::Item,&json!({"code":"Parts","kind":"NONSTOCK","sale_rate":"2.00","purchase_cost":"1.00","expense_account":f.expense.to_db()}),None).unwrap();
    let mut p = f.payload();
    p["lines"].as_array_mut().unwrap().push(json!({"tab":"ITEM","item_id":item["id"],"quantity":"1.005","rate":"1.00","account_id":f.expense.to_db()}));
    let b = f.save(&p);
    assert_eq!(b["current"]["total"], "11.02");
    let r = f.review(&b);
    p["check_id"] = b["id"].clone();
    p["expected_revision"] = json!(1);
    p["memo"] = json!("Edited");
    let b = f.save(&p);
    assert_eq!(b["revision"], 2);
    assert!(record_check(&mut f.book, f.actor, f.co, s(&b, "id"), Some(2), &r).is_err());
    let b = f.post(&b);
    f.void(&b);
}
#[test]
fn malformed_inputs_refuse_without_mutation() {
    let mut f = Fx::new();
    let p = f.payload();
    let before = f.counts();
    for (key, value) in [
        ("check_id", json!(true)),
        ("bank_id", json!(false)),
        ("vendor_id", json!(123)),
        ("print_later", json!("yes")),
        ("memo", json!(null)),
        ("date", json!("bad")),
        ("check_number", json!("a\u{200b}")),
        ("check_number", json!("a\u{e000}")),
        ("check_number", json!("a\u{2028}")),
        ("expected_revision", json!(true)),
    ] {
        let mut bad = p.clone();
        bad[key] = value;
        assert!(
            save_check(&mut f.book, f.actor, f.co, &bad).is_err(),
            "{key}"
        );
        assert_eq!(before, f.counts());
    }
    for v in [
        json!("0"),
        json!("-1"),
        json!(true),
        json!("NaN"),
        json!("100000000000000"),
    ] {
        let mut bad = p.clone();
        bad["lines"][0]["rate"] = v;
        assert!(save_check(&mut f.book, f.actor, f.co, &bad).is_err());
        assert_eq!(before, f.counts());
    }
}
#[test]
fn missing_line_account_draft_and_control_refusal() {
    let mut f = Fx::new();
    let mut p = f.payload();
    p["lines"][0]["account_id"] = Value::Null;
    let b = f.save(&p);
    assert!(review_check(&mut f.book, f.actor, f.co, s(&b, "id"), Some(1)).is_err());
    for id in [f.bank, f.ap] {
        p["check_number"] = json!("102");
        p["request_key"] = json!(fb_ids::new_v7().to_string());
        p["lines"][0]["account_id"] = json!(id.to_db());
        assert!(save_check(&mut f.book, f.actor, f.co, &p).is_err());
    }
}
#[test]
fn transfer_lifecycle_exact_retry_and_balances() {
    let mut f = Fx::new();
    let p = f.transfer();
    let review = crate::transfers::review_transfer(&mut f.book, f.actor, f.co, &p).unwrap();
    let t = crate::transfers::record_transfer(&mut f.book, f.actor, f.co, &p, &review).unwrap();
    assert_eq!(
        crate::transfers::posted_balance(&mut f.book, f.actor, f.co, f.bank).unwrap(),
        "-12.34"
    );
    assert_eq!(
        crate::transfers::posted_balance(&mut f.book, f.actor, f.co, f.bank2).unwrap(),
        "12.34"
    );
    let counts = f.counts();
    crate::transfers::record_transfer(&mut f.book, f.actor, f.co, &p, &review).unwrap();
    assert_eq!(counts, f.counts());
    crate::transfers::void_transfer(
        &mut f.book,
        f.actor,
        f.co,
        s(&t, "id"),
        Some(1),
        &json!("2026-10-24"),
        &json!("Fix"),
    )
    .unwrap();
    let counts = f.counts();
    crate::transfers::void_transfer(
        &mut f.book,
        f.actor,
        f.co,
        s(&t, "id"),
        Some(1),
        &json!("2026-10-24"),
        &json!("Fix"),
    )
    .unwrap();
    crate::transfers::record_transfer(&mut f.book, f.actor, f.co, &p, &review).unwrap();
    assert_eq!(counts, f.counts());
    assert_eq!(
        crate::transfers::posted_balance(&mut f.book, f.actor, f.co, f.bank).unwrap(),
        "0.00"
    );
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn transfer_validation_and_changed_review() {
    let mut f = Fx::new();
    let p = f.transfer();
    let before = f.counts();
    for (k, v) in [
        ("to_account_id", json!(f.bank.to_db())),
        ("to_account_id", json!(f.expense.to_db())),
        ("amount", json!("0")),
        ("amount", json!("-1")),
        ("amount", json!(true)),
        ("request_key", json!("bad")),
        ("date", json!("2026-02-30")),
        ("memo", json!(false)),
    ] {
        let mut bad = p.clone();
        bad[k] = v;
        assert!(crate::transfers::review_transfer(&mut f.book, f.actor, f.co, &bad).is_err());
        assert_eq!(before, f.counts());
    }
    let r = crate::transfers::review_transfer(&mut f.book, f.actor, f.co, &p).unwrap();
    let mut bad = p.clone();
    bad["amount"] = json!("2.00");
    assert!(crate::transfers::record_transfer(&mut f.book, f.actor, f.co, &bad, &r).is_err());
    assert_eq!(before, f.counts());
}
#[test]
fn late_audit_failure_atomic_public_and_composed() {
    let mut f = Fx::new();
    let p = f.payload();
    let tp = f.transfer();
    let tr = crate::transfers::review_transfer(&mut f.book, f.actor, f.co, &tp).unwrap();
    f.book.connection().execute_batch("CREATE TRIGGER test_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action LIKE 'check.%' OR NEW.action LIKE 'transfer.%' BEGIN SELECT RAISE(ABORT,'late test audit'); END;").unwrap();
    let before = f.counts();
    assert!(save_check(&mut f.book, f.actor, f.co, &p).is_err());
    assert_eq!(before, f.counts());
    f.book
        .write(|tx| -> FbResult<()> {
            assert!(save_check_tx(tx, f.actor, f.co, &p).is_err());
            Ok(())
        })
        .unwrap();
    assert_eq!(before, f.counts());
    assert!(crate::transfers::record_transfer(&mut f.book, f.actor, f.co, &tp, &tr).is_err());
    assert_eq!(before, f.counts());
    f.book
        .write(|tx| -> FbResult<()> {
            assert!(crate::transfers::record_transfer_tx(tx, f.actor, f.co, &tp, &tr).is_err());
            Ok(())
        })
        .unwrap();
    assert_eq!(before, f.counts());
    f.book
        .connection()
        .execute_batch("DROP TRIGGER test_audit")
        .unwrap();
    let b = f.save(&p);
    let r = f.review(&b);
    f.book.connection().execute_batch("CREATE TRIGGER test_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='check.recorded' BEGIN SELECT RAISE(ABORT,'late test audit'); END;").unwrap();
    let before = f.counts();
    assert!(record_check(&mut f.book, f.actor, f.co, s(&b, "id"), Some(1), &r).is_err());
    assert_eq!(before, f.counts());
    f.book
        .connection()
        .execute_batch("DROP TRIGGER test_audit")
        .unwrap();
    let b = f.post(&b);
    f.book.connection().execute_batch("CREATE TRIGGER test_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='check.voided' BEGIN SELECT RAISE(ABORT,'late test audit'); END;").unwrap();
    let before = f.counts();
    assert!(
        void_check(
            &mut f.book,
            f.actor,
            f.co,
            s(&b, "id"),
            Some(2),
            &json!("2026-10-24"),
            &json!("Fix")
        )
        .is_err()
    );
    assert_eq!(before, f.counts());
    f.book
        .connection()
        .execute_batch("DROP TRIGGER test_audit")
        .unwrap();
    assert!(f.book.check().unwrap().is_empty());
}
