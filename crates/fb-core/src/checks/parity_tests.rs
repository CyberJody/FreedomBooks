//! Additional non-HTTP corefx/FX4 and ordered-race outcomes from the frozen suite.
use super::guards::snapshot;
use super::*;
use crate::{payables, transfers};

fn bill(f: &mut Fx) -> Value {
    let p = json!({"vendor_id":f.vendor,"date":"2026-10-23","due_date":"2026-11-23","request_key":fb_ids::new_v7().to_string(),"lines":[{"description":"Payable","quantity":"1","rate":"100","expense_account_id":f.expense.to_db()}]});
    let b = payables::save_bill(&mut f.book, f.actor, f.co, &p).unwrap();
    let r = payables::review_bill(&mut f.book, f.actor, f.co, s(&b, "id"), Some(1)).unwrap();
    payables::post_bill(&mut f.book, f.actor, f.co, s(&b, "id"), Some(1), &r).unwrap()
}
fn payment(f: &Fx, b: &Value, method: &str, reference: &str) -> Value {
    json!({"vendor_id":f.vendor,"bank_id":f.bank.to_db(),"date":"2026-10-23","amount":"1","method":method,"reference":reference,"request_key":fb_ids::new_v7().to_string(),"allocations":[{"bill_id":b["id"],"amount":"1"}]})
}

#[test]
fn legacy_duplicate_review_is_read_only_record_is_plain_refusal_and_renumber_recovers() {
    let mut f = Fx::new();
    let b = bill(&mut f);
    let mut p = f.payload();
    let check = f.save(&p);
    let r = f.review(&check);
    let pp = payment(&f, &b, "CHECK", "102");
    let pr = payables::review_payment(&mut f.book, f.actor, f.co, &pp).unwrap();
    let paid=payables::record_payment(&mut f.book, f.actor, f.co, &pp, &pr).unwrap();
    let q=f.book.connection();
    let guards:Vec<(String,String)>=q.prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger'").unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<Result<_,_>>().unwrap();
    for (name,_) in &guards {q.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();}
    q.execute("UPDATE payables_billpayment SET reference='101' WHERE id=?",[s(&paid,"id")]).unwrap();
    for (_,sql) in guards {q.execute_batch(&sql).unwrap();}
    let before = snapshot(f.book.connection());
    f.review(&check);
    assert_eq!(snapshot(f.book.connection()), before);
    let e = record_check(&mut f.book, f.actor, f.co, s(&check, "id"), Some(1), &r).unwrap_err();
    assert!(matches!(e, crate::FbError::Validation(_)));
    assert!(
        e.message()
            .contains("already used by bill payment BPAY-000001"),
        "{e:?}"
    );
    assert_eq!(snapshot(f.book.connection()), before);
    p["check_id"] = check["id"].clone();
    p["expected_revision"] = json!(1);
    p["check_number"] = json!("102");
    let check = f.save(&p);
    f.post(&check);
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn fx4_numeric_payment_rule_leading_zeros_and_historical_suggestions() {
    let mut f = Fx::new();
    let b = bill(&mut f);
    for (method, reference) in [
        ("CHECK", "Synthetic check"),
        ("CHECK", "ACH-7000"),
        ("CHECK", "١٢٣"),
        ("CHECK", "9999999999999999999999999"),
        ("CASH", "5000"),
        ("OTHER", "5001"),
        ("CHECK", "ACH-7000"),
    ] {
        let p = payment(&f, &b, method, reference);
        let r = review_bill_payment(&mut f.book, f.actor, f.co, &p).unwrap();
        record_bill_payment(&mut f.book, f.actor, f.co, &p, &r).unwrap();
    }
    assert_eq!(
        next_number(&mut f.book, f.actor, f.co, f.bank).unwrap(),
        "1"
    );
    let p = payment(&f, &b, "CHECK", " 0042 ");
    let r = review_bill_payment(&mut f.book, f.actor, f.co, &p).unwrap();
    let paid = record_bill_payment(&mut f.book, f.actor, f.co, &p, &r).unwrap();
    let mut cp = f.payload();
    cp["check_number"] = json!("0042");
    assert!(save_check(&mut f.book, f.actor, f.co, &cp).is_err());
    cp["check_number"] = json!("42");
    let check = f.save(&cp);
    let check = f.post(&check);
    f.void(&check);
    payables::reverse_payment(
        &mut f.book,
        f.actor,
        f.co,
        s(&paid, "id"),
        Some(1),
        &json!("2026-10-24"),
        &json!("Release"),
    )
    .unwrap();
    assert_eq!(
        next_number(&mut f.book, f.actor, f.co, f.bank).unwrap(),
        "43"
    );
    cp["request_key"] = json!(fb_ids::new_v7().to_string());
    cp["check_number"] = json!("0042");
    f.save(&cp);
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn ordered_check_edit_record_number_void_outcomes_are_stale_and_unchanged() {
    for edit_first in [false, true] {
        let mut f = Fx::new();
        let mut p = f.payload();
        let b = f.save(&p);
        let review = f.review(&b);
        p["check_id"] = b["id"].clone();
        p["expected_revision"] = json!(1);
        p["memo"] = json!("Edited");
        if edit_first {
            f.save(&p);
        } else {
            f.post(&b);
        }
        let before = snapshot(f.book.connection());
        let result = if edit_first {
            record_check(&mut f.book, f.actor, f.co, s(&b, "id"), Some(1), &review)
        } else {
            save_check(&mut f.book, f.actor, f.co, &p)
        };
        assert!(result.is_err());
        assert_eq!(snapshot(f.book.connection()), before);
    }
    for number_first in [false, true] {
        let mut f = Fx::new();
        let mut p = f.payload();
        p["check_number"] = json!("");
        p["print_later"] = json!(true);
        let b = f.save(&p);
        let b = f.post(&b);
        if number_first {
            assign_number(
                &mut f.book,
                f.actor,
                f.co,
                s(&b, "id"),
                Some(2),
                &json!("70"),
            )
            .unwrap();
        } else {
            f.void(&b);
        }
        let before = snapshot(f.book.connection());
        let result = if number_first {
            void_check(
                &mut f.book,
                f.actor,
                f.co,
                s(&b, "id"),
                Some(2),
                &json!("2026-10-24"),
                &json!("Correction"),
            )
        } else {
            assign_number(
                &mut f.book,
                f.actor,
                f.co,
                s(&b, "id"),
                Some(2),
                &json!("70"),
            )
        };
        assert!(result.is_err());
        assert_eq!(snapshot(f.book.connection()), before);
        assert!(f.book.check().unwrap().is_empty());
    }
}

#[test]
fn transfer_setup_removal_both_orders_and_inactive_bank_void_refusal() {
    for remove_first in [false, true] {
        let mut f = Fx::new();
        let p = f.transfer();
        let r = transfers::review_transfer(&mut f.book, f.actor, f.co, &p).unwrap();
        if !remove_first {
            transfers::record_transfer(&mut f.book, f.actor, f.co, &p, &r).unwrap();
        }
        documents::save_setup(
            &mut f.book,
            f.actor,
            f.co,
            &json!({"payables":f.ap.to_db(),"banks":[f.bank.to_db()]}),
            Some(1),
        )
        .unwrap();
        let before = snapshot(f.book.connection());
        if remove_first {
            assert!(transfers::record_transfer(&mut f.book, f.actor, f.co, &p, &r).is_err());
        } else {
            transfers::record_transfer(&mut f.book, f.actor, f.co, &p, &r).unwrap();
        }
        assert_eq!(snapshot(f.book.connection()), before);
        assert!(f.book.check().unwrap().is_empty());
    }
    let mut f = Fx::new();
    let p = f.transfer();
    let r = transfers::review_transfer(&mut f.book, f.actor, f.co, &p).unwrap();
    let t = transfers::record_transfer(&mut f.book, f.actor, f.co, &p, &r).unwrap();
    services::save_account(
        &mut f.book,
        f.actor,
        f.co,
        AccountInput::new("101", "Savings")
            .kind("ASSET")
            .edit(f.bank2)
            .active(false),
    )
    .unwrap();
    let before = snapshot(f.book.connection());
    assert!(
        transfers::void_transfer(
            &mut f.book,
            f.actor,
            f.co,
            s(&t, "id"),
            Some(1),
            &json!("2026-10-24"),
            &json!("Too late")
        )
        .is_err()
    );
    assert_eq!(snapshot(f.book.connection()), before);
    assert!(f.book.check().unwrap().is_empty());
}
