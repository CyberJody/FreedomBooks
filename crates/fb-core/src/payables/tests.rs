use super::*;
#[path = "l3b_tests.rs"]
mod l3b;
#[path = "attacks.rs"]
mod attacks;
#[path = "read_tests.rs"]
mod read_tests;
#[path = "rebase_tests.rs"]
mod rebase_tests;
use crate::{services::AccountInput, users};
use std::sync::atomic::{AtomicUsize, Ordering};
/// Returns the guard that refused a raw statement; 0130's `fb_valid` never counts.
fn guard_of<T: std::fmt::Debug>(r: Result<T, fb_db::rusqlite::Error>) -> String {
    let e = r.unwrap_err();
    let constraint = fb_db::constraint_of(&e).unwrap_or_default();
    assert_ne!(constraint, "fb_valid", "masked by fb_valid: {e}");
    constraint
}
/// Asserts the collected `<label>: <guard>` set equals the explicit expected set; on mismatch it prints
/// the actual set as Rust string literals.
fn assert_guard_set(hit: &BTreeSet<String>, expected: &[&str]) {
    let expected: BTreeSet<String> = expected.iter().map(|s| s.to_string()).collect();
    assert!(
        *hit == expected,
        "guard set differs\nACTUAL:\n{}",
        hit.iter().map(|h| format!("    \"{h}\",")).collect::<Vec<_>>().join("\n")
    );
}
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fx {
    book: Book,
    actor: UserId,
    co: CompanyId,
    vendor: String,
    expense: AccountId,
    ap: AccountId,
    bank: AccountId,
}
impl Fx {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "payables-{}-{}.fbbook",
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
        documents::save_setup(
            &mut book,
            actor,
            co,
            &json!({"payables":ap.to_db(),"banks":[bank.to_db()]}),
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
            bank,
        }
    }
    fn payload(&self) -> Value {
        json!({"vendor_id":self.vendor,"date":"2026-10-23","due_date":"2026-11-23","reference":"Supplier 42","memo":"","request_key":fb_ids::new_v7().to_string(),"lines":[{"description":"Gross cost including supplier tax","quantity":"1.005","rate":"1.00","expense_account_id":self.expense.to_db()}]})
    }
    fn save(&mut self, p: &Value) -> Value {
        save_bill(&mut self.book, self.actor, self.co, p).unwrap()
    }
    fn review(&mut self, b: &Value) -> Review {
        review_bill(
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
        post_bill(
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
        void_bill(
            &mut self.book,
            self.actor,
            self.co,
            s(b, "id"),
            b["revision"].as_i64(),
            &json!("2026-10-24"),
            &json!("Test correction"),
        )
        .unwrap()
    }
    fn payment(&self, b: &Value, amount: &str) -> Value {
        json!({"vendor_id":self.vendor,"date":"2026-10-24","amount":amount,"method":"CHECK","reference":"100","memo":"","bank_id":self.bank.to_db(),"request_key":fb_ids::new_v7().to_string(),"allocations":[{"bill_id":b["id"],"amount":amount}]})
    }
    fn pay(&mut self, p: &Value) -> Value {
        let r = review_payment(&mut self.book, self.actor, self.co, p).unwrap();
        record_payment(&mut self.book, self.actor, self.co, p, &r).unwrap()
    }
    fn counts(&self) -> Value {
        let mut v = json!({});
        for t in [
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
fn post_void_exact_retry_and_history() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let r = f.review(&b);
    let b = f.post(&b);
    assert_eq!(b["current"]["total"], "1.01");
    let before = f.counts();
    let retry = post_bill(&mut f.book, f.actor, f.co, s(&b, "id"), Some(1), &r).unwrap();
    assert_eq!(retry["id"], b["id"]);
    assert_eq!(before, f.counts());
    let b = f.void(&b);
    assert_eq!(b["state"], "VOID");
    assert!(review_bill(&mut f.book, f.actor, f.co, s(&b, "id"), Some(3)).is_err());
    let before = f.counts();
    let b = void_bill(
        &mut f.book,
        f.actor,
        f.co,
        s(&b, "id"),
        Some(2),
        &json!("2026-10-24"),
        &json!("Test correction"),
    )
    .unwrap();
    assert_eq!(before, f.counts());
    assert_eq!(f.save(&p)["id"], b["id"]);
}
#[test]
fn reference_reentry_normalization() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    for text in ["supplier 42", "  SUPPLIER 42\t"] {
        let mut p = f.payload();
        p["reference"] = json!(text);
        assert!(save_bill(&mut f.book, f.actor, f.co, &p).is_err());
    }
    let b = f.post(&b);
    let b = f.void(&b);
    let mut p = f.payload();
    assert!(save_bill(&mut f.book, f.actor, f.co, &p).is_err());
    p["supersedes_id"] = b["id"].clone();
    p["reentry_reason"] = json!("Corrected gross amount");
    let b2 = f.save(&p);
    assert_eq!(b2["current"]["supersedes_id"], b["id"]);
}
#[test]
fn edit_stale_zero_and_precision() {
    let mut f = Fx::new();
    for (q, r) in [
        ("0", "1"),
        ("1.00001", "1"),
        ("1", "1.001"),
        ("1", "-1"),
        ("NaN", "1"),
        ("99999999999999", "99999999999999"),
    ] {
        let mut p = f.payload();
        p["lines"][0]["quantity"] = json!(q);
        p["lines"][0]["rate"] = json!(r);
        let n = f.counts();
        assert!(save_bill(&mut f.book, f.actor, f.co, &p).is_err());
        assert_eq!(n, f.counts());
    }
    let mut p = f.payload();
    p["lines"][0]["rate"] = json!("0");
    let b = f.save(&p);
    assert!(review_bill(&mut f.book, f.actor, f.co, s(&b, "id"), Some(1)).is_err());
    p["bill_id"] = b["id"].clone();
    p["expected_revision"] = json!(1);
    p["lines"][0]["rate"] = json!("1");
    let b = f.save(&p);
    assert_eq!(b["revision"], 2);
    assert!(save_bill(&mut f.book, f.actor, f.co, &p).is_err());
}
#[test]
fn mapping_changes_require_review_and_frozen_payable() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let review = f.review(&b);
    let ap2 = services::save_account(
        &mut f.book,
        f.actor,
        f.co,
        AccountInput::new("201", "Other payable").kind("LIABILITY"),
    )
    .unwrap()
    .id;
    documents::save_setup(
        &mut f.book,
        f.actor,
        f.co,
        &json!({"payables":ap2.to_db(),"banks":[f.bank.to_db()]}),
        Some(1),
    )
    .unwrap();
    assert!(post_bill(&mut f.book, f.actor, f.co, s(&b, "id"), Some(1), &review).is_err());
    let b = f.post(&b);
    documents::save_setup(
        &mut f.book,
        f.actor,
        f.co,
        &json!({"payables":f.ap.to_db(),"banks":[f.bank.to_db()]}),
        Some(2),
    )
    .unwrap();
    let p = f.payment(&b, "1.01");
    let paid = f.pay(&p);
    let aid: String = f
        .book
        .connection()
        .query_row(
            "SELECT payables_id FROM payables_paymentallocation WHERE payment_id=?",
            [s(&paid, "id")],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(aid, ap2.to_db());
}
#[test]
fn partial_final_reverse_void() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let b = f.post(&b);
    let p = f.payment(&b, "0.50");
    let review = review_payment(&mut f.book, f.actor, f.co, &p).unwrap();
    let payment = record_payment(&mut f.book, f.actor, f.co, &p, &review).unwrap();
    assert_eq!(
        current_bills(&mut f.book, f.actor, f.co).unwrap()[0]["payment_state"],
        "PARTIAL"
    );
    assert!(
        void_bill(
            &mut f.book,
            f.actor,
            f.co,
            s(&b, "id"),
            Some(2),
            &json!("2026-10-24"),
            &json!("Correction")
        )
        .is_err()
    );
    let mut p2 = f.payment(&b, "0.51");
    p2["reference"] = json!("101");
    let pay2 = f.pay(&p2);
    assert_eq!(
        current_bills(&mut f.book, f.actor, f.co).unwrap()[0]["payment_state"],
        "PAID"
    );
    for pay in [&payment, &pay2] {
        reverse_payment(
            &mut f.book,
            f.actor,
            f.co,
            s(pay, "id"),
            Some(1),
            &json!("2026-10-24"),
            &json!("Correction"),
        )
        .unwrap();
    }
    let b = f.void(&b);
    directory::set_active(&mut f.book, f.actor, f.co, Model::Vendor, &f.vendor, false).unwrap();
    let n = f.counts();
    assert_eq!(
        record_payment(&mut f.book, f.actor, f.co, &p, &review).unwrap()["id"],
        payment["id"]
    );
    assert_eq!(n, f.counts());
    assert_eq!(b["state"], "VOID");
}
#[test]
fn all_public_writes_rollback_on_late_audit_failure() {
    let mut f = Fx::new();
    let mut p = f.payload();
    let b = f.save(&p);
    let review = f.review(&b);
    for action in ["create", "edit", "post"] {
        let before = f.counts();
        f.book.connection().execute_batch("CREATE TEMP TRIGGER fail_audit BEFORE INSERT ON ledger_auditevent BEGIN SELECT RAISE(ABORT,'injected late failure'); END;").unwrap();
        let result = match action {
            "create" => {
                p["request_key"] = json!(fb_ids::new_v7().to_string());
                p["reference"] = json!("Other");
                save_bill(&mut f.book, f.actor, f.co, &p)
            }
            "edit" => {
                p["bill_id"] = b["id"].clone();
                p["expected_revision"] = json!(1);
                save_bill(&mut f.book, f.actor, f.co, &p)
            }
            _ => post_bill(&mut f.book, f.actor, f.co, s(&b, "id"), Some(1), &review),
        };
        assert!(result.is_err());
        f.book
            .connection()
            .execute_batch("DROP TRIGGER fail_audit")
            .unwrap();
        assert_eq!(before, f.counts());
    }
    let b = f.post(&b);
    let p = f.payment(&b, "1.01");
    let review = review_payment(&mut f.book, f.actor, f.co, &p).unwrap();
    let before = f.counts();
    f.book.connection().execute_batch("CREATE TEMP TRIGGER fail_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='payment.recorded' BEGIN SELECT RAISE(ABORT,'injected late failure'); END;").unwrap();
    assert!(record_payment(&mut f.book, f.actor, f.co, &p, &review).is_err());
    f.book
        .connection()
        .execute_batch("DROP TRIGGER fail_audit")
        .unwrap();
    assert_eq!(before, f.counts());
    let pay = f.pay(&p);
    let before = f.counts();
    f.book.connection().execute_batch("CREATE TEMP TRIGGER fail_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='payment.reversed' BEGIN SELECT RAISE(ABORT,'injected late failure'); END;").unwrap();
    assert!(
        reverse_payment(
            &mut f.book,
            f.actor,
            f.co,
            s(&pay, "id"),
            Some(1),
            &json!("2026-10-24"),
            &json!("Correction")
        )
        .is_err()
    );
    f.book
        .connection()
        .execute_batch("DROP TRIGGER fail_audit")
        .unwrap();
    assert_eq!(before, f.counts());
    reverse_payment(
        &mut f.book,
        f.actor,
        f.co,
        s(&pay, "id"),
        Some(1),
        &json!("2026-10-24"),
        &json!("Correction"),
    )
    .unwrap();
    let before = f.counts();
    f.book.connection().execute_batch("CREATE TEMP TRIGGER fail_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='bill.voided' BEGIN SELECT RAISE(ABORT,'injected late failure'); END;").unwrap();
    assert!(
        void_bill(
            &mut f.book,
            f.actor,
            f.co,
            s(&b, "id"),
            Some(2),
            &json!("2026-10-24"),
            &json!("Correction")
        )
        .is_err()
    );
    f.book
        .connection()
        .execute_batch("DROP TRIGGER fail_audit")
        .unwrap();
    assert_eq!(before, f.counts());
}
#[test]
fn receipt_defaults_no_post_and_rollback() {
    let mut f = Fx::new();
    directory::save_record(
        &mut f.book,
        f.actor,
        f.co,
        Model::Vendor,
        &json!({"default_expense_account":f.expense.to_db(),"default_terms":"30"}),
        Some(&f.vendor),
    )
    .unwrap();
    let fields = json!({"document_date":"2026-10-23","document_number":"receipt","lines":[{"description":"Test","quantity":"1.005","rate":"1","amount":"1.01"}]});
    let key = fb_ids::new_v7().to_string();
    let p = receipt_draft_input(&mut f.book, f.actor, f.co, &f.vendor, &fields, &key).unwrap();
    assert_eq!(p["due_date"], "2026-11-22");
    let before = f.counts();
    f.book.connection().execute_batch("CREATE TEMP TRIGGER fail_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='bill.created' BEGIN SELECT RAISE(ABORT,'injected late failure'); END;").unwrap();
    assert!(save_receipt_draft(&mut f.book, f.actor, f.co, &f.vendor, &fields, &key).is_err());
    f.book
        .connection()
        .execute_batch("DROP TRIGGER fail_audit")
        .unwrap();
    assert_eq!(before, f.counts());
    let b = save_receipt_draft(&mut f.book, f.actor, f.co, &f.vendor, &fields, &key).unwrap();
    assert_eq!(b["state"], "DRAFT");
    assert_eq!(f.counts()["ledger_journal"], 0);
}
#[test]
fn replace_delete_reinsert_all_evidence_both_pragma_modes() {
    // Attacker's re-key and alternate-row ids: canonical, so 0130 passes and the history guards decide.
    const CHANGED_ID: &str = "00000000-0000-7000-8000-0000000b0103";
    const NEW_KEY: &str = "00000000-0000-7000-8000-0000000b0104";
    const EXPECTED: &[&str] = &[
        "directory_vendor parent replace: payables_parent",
        "documents_documentidentity parent replace: payables_parent",
        "fb_seal parent replace: payables_seal",
        "fb_seal_due parent replace: payables_seal",
        "ledger_account parent replace: payables_parent",
        "ledger_journal parent replace: payables_parent",
        "payables_bill alternate: bill_immutable_ownership",
        "payables_bill delete: bill_immutable_ownership",
        "payables_bill rekey: payables_permanent_id",
        "payables_bill replace: bill_immutable_ownership",
        "payables_billline alternate: bill_history_immutable",
        "payables_billline delete: bill_history_immutable",
        "payables_billline rekey: payables_permanent_id",
        "payables_billline replace: bill_history_immutable",
        "payables_billpayment alternate: payment_immutable",
        "payables_billpayment delete: payment_immutable",
        "payables_billpayment rekey: payables_permanent_id",
        "payables_billpayment replace: payment_immutable",
        "payables_billrevision alternate: bill_history_immutable",
        "payables_billrevision delete: bill_history_immutable",
        "payables_billrevision rekey: payables_permanent_id",
        "payables_billrevision replace: bill_history_immutable",
        "payables_paymentallocation alternate: payment_immutable",
        "payables_paymentallocation delete: payment_immutable",
        "payables_paymentallocation rekey: payables_permanent_id",
        "payables_paymentallocation replace: payment_immutable",
        "payables_paymentreversal alternate: payment_immutable",
        "payables_paymentreversal delete: payment_immutable",
        "payables_paymentreversal rekey: payables_permanent_id",
        "payables_paymentreversal replace: payment_immutable",
    ];
    let mut hit = BTreeSet::new();
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let b = f.post(&b);
    let p = f.payment(&b, "1.01");
    let pay = f.pay(&p);
    reverse_payment(
        &mut f.book,
        f.actor,
        f.co,
        s(&pay, "id"),
        Some(1),
        &json!("2026-10-24"),
        &json!("Correction"),
    )
    .unwrap();
    f.void(&b);
    for mode in ["ON", "OFF"] {
        f.book
            .connection()
            .execute_batch(&format!("PRAGMA recursive_triggers={mode}"))
            .unwrap();
        for table in [
            "payables_bill",
            "payables_billrevision",
            "payables_billline",
            "payables_billpayment",
            "payables_paymentallocation",
            "payables_paymentreversal",
        ] {
            let c = f.book.connection();
            let n = f.counts();
            let guard = guard_of(c.execute(
                &format!("INSERT OR REPLACE INTO {table} SELECT * FROM {table} LIMIT 1"),
                [],
            ));
            hit.insert(format!("{table} replace: {guard}"));
            hit.insert(format!("{table} delete: {}", guard_of(c.execute(&format!("DELETE FROM {table}"), []))));
            let guard = guard_of(c.execute(&format!("UPDATE {table} SET id='{CHANGED_ID}'"), []));
            hit.insert(format!("{table} rekey: {guard}"));
            assert_eq!(n, f.counts());
            let cols = c
                .prepare(&format!("SELECT * FROM {table}"))
                .unwrap()
                .column_names()
                .map(|s| s.to_string())
                .collect::<Vec<_>>();
            let cols = cols
                .iter()
                .map(|c| {
                    if c == "id" {
                        format!("'{NEW_KEY}'")
                    } else {
                        c.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(",");
            let guard = guard_of(c.execute(
                &format!("INSERT OR REPLACE INTO {table} SELECT {cols} FROM {table} LIMIT 1"),
                [],
            ));
            hit.insert(format!("{table} alternate: {guard}"));
        }
        for table in [
            "directory_vendor",
            "ledger_account",
            "documents_documentidentity",
            "ledger_journal",
            "fb_seal",
            "fb_seal_due",
        ] {
            let guard = guard_of(f.book.connection().execute(
                &format!("INSERT OR REPLACE INTO {table} SELECT * FROM {table} LIMIT 1"),
                [],
            ));
            hit.insert(format!("{table} parent replace: {guard}"));
        }
    }
    assert_guard_set(&hit, EXPECTED);
    f.book
        .connection()
        .execute_batch("PRAGMA recursive_triggers=ON")
        .unwrap();
}
#[test]
fn unsealed_root_or_orphan_source_cannot_commit() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let before = f.counts();
    let result: FbResult<()> = f.book.write(|tx| {
        tx.execute(
            "UPDATE payables_bill SET revision=2 WHERE id=?",
            [s(&b, "id")],
        )?;
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(before, f.counts());
    let result = f.book.write(|tx| {
        documents::post_source(
            tx,
            f.actor,
            f.co,
            s(&b, "identity_id"),
            "2026-10-23",
            "Bypass",
            vec![
                json!({"account":f.expense.to_db(),"debit":"1.01","credit":"0"}),
                json!({"account":f.ap.to_db(),"debit":"0","credit":"1.01"}),
            ],
        )
    });
    assert!(result.is_err());
    assert_eq!(before, f.counts());
}
#[test]
fn pragmas_refused_before_each_write() {
    let mut f = Fx::new();
    let p = f.payload();
    for (pragma, bad, good) in [
        ("foreign_keys", "OFF", "ON"),
        ("recursive_triggers", "OFF", "ON"),
        ("ignore_check_constraints", "ON", "OFF"),
    ] {
        f.book
            .connection()
            .execute_batch(&format!("PRAGMA {pragma}={bad}"))
            .unwrap();
        assert!(save_bill(&mut f.book, f.actor, f.co, &p).is_err());
        f.book
            .connection()
            .execute_batch(&format!("PRAGMA {pragma}={good}"))
            .unwrap();
    }
    assert_eq!(f.counts()["payables_bill"], 0);
}

#[path = "independent_review.rs"]
mod independent_review;
