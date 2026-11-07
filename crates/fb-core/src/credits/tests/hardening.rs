use super::*;

#[test]
fn refund_numbers_use_ascii_trimming_and_reject_control_or_format_characters() {
    let mut f = Fixture::new();
    let m = f.memo();
    let key = m["id"].as_str().unwrap();
    let before = snapshot(&f);
    for bad in [
        json!("1\n2"),
        json!("1\u{200d}2"),
        json!("\u{2028}"),
        json!("\u{202e}123"),
        json!("\u{e000}"),
        json!("123456789012345678901"),
        json!(false),
        Value::Null,
    ] {
        let mut p = f.refund_input("0.01");
        p["check_number"] = bad;
        assert!(
            review_refund(&mut f.book, f.actor, f.company, key, &p).is_err(),
            "{p}"
        );
        assert_eq!(snapshot(&f), before);
    }
    for (input, stored) in [
        (" \t1001\n", "1001"),
        ("\u{a0}1002\u{a0}", "\u{a0}1002\u{a0}"),
        ("\u{2007}1003\u{2007}", "\u{2007}1003\u{2007}"),
    ] {
        let mut p = f.refund_input("0.01");
        p["check_number"] = json!(input);
        let r = review_refund(&mut f.book, f.actor, f.company, key, &p).unwrap();
        let rf = record_refund(&mut f.book, f.actor, f.company, key, &p, Some(&r)).unwrap();
        assert_eq!(rf["check_number"], stored);
    }
    f.clean();
}

#[test]
fn open_checks_customer_company_ownership_even_after_exact_guard_restoration() {
    let mut f = Fixture::new();
    f.memo();
    let other = services::create_company(
        &mut f.book,
        f.actor,
        "Other synthetic company",
        "USD",
        false,
    )
    .unwrap()
    .id;
    let customer = directory::save_record(
        &mut f.book,
        f.actor,
        other,
        Model::Customer,
        &json!({"display_name":"Other customer"}),
        None,
    )
    .unwrap();
    let c = f.book.connection();
    let definition: String = c
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='fb_credits_creditmemo_update'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    c.execute_batch("BEGIN IMMEDIATE; DROP TRIGGER fb_credits_creditmemo_update;")
        .unwrap();
    c.execute(
        "UPDATE credits_creditmemo SET customer_id=?1,data=json_set(data,'$.customer_id',?1)",
        [customer["id"].as_str().unwrap()],
    )
    .unwrap();
    c.execute_batch(&definition).unwrap();
    c.execute_batch("COMMIT").unwrap();
    let problems = f.book.check().unwrap();
    assert!(
        problems.iter().any(|s| s.contains("credit memo scope")),
        "{problems:?}"
    );
    assert!(Book::open(&f.path, &[24; 32]).is_err());
}

#[test]
fn exported_invoice_lifecycle_hook_preserves_python_messages() {
    let mut f = Fixture::new();
    let inv = f.invoice();
    let m = f.memo();
    let p = f.application(&inv, "1.00");
    let k = m["id"].as_str().unwrap();
    let r = review_application(&mut f.book, f.actor, f.company, k, &p).unwrap();
    let a = apply_credit(&mut f.book, f.actor, f.company, k, &p, Some(&r)).unwrap();
    let e = f
        .book
        .read(|tx| refuse_live_credits(tx, inv["id"].as_str().unwrap(), "2026-10-20"))
        .unwrap_err();
    assert_eq!(
        e.message(),
        "Unapply credit memo CM-000001 from this invoice before voiding it."
    );
    unapply_credit(
        &mut f.book,
        f.actor,
        f.company,
        a["id"].as_str().unwrap(),
        Some(1),
        &json!("2026-10-20"),
        &json!("Release"),
    )
    .unwrap();
    let e = f
        .book
        .read(|tx| refuse_live_credits(tx, inv["id"].as_str().unwrap(), "2026-10-19"))
        .unwrap_err();
    assert_eq!(
        e.message(),
        "Void date cannot precede 2026-10-20, when a credit memo was unapplied from this invoice."
    );
    f.book
        .read(|tx| refuse_live_credits(tx, inv["id"].as_str().unwrap(), "2026-10-20"))
        .unwrap();
    f.clean();
}

#[test]
fn archived_historical_customer_can_apply_refund_and_release_but_cannot_record_a_new_memo() {
    let mut f = Fixture::new();
    let inv = f.invoice();
    let m = f.memo();
    let key = m["id"].as_str().unwrap();
    directory::set_active(
        &mut f.book,
        f.actor,
        f.company,
        Model::Customer,
        &f.customer,
        false,
    )
    .unwrap();
    let p = f.input();
    assert!(review_credit_memo(&mut f.book, f.actor, f.company, &p).is_err());
    let p = f.application(&inv, "1.00");
    let r = review_application(&mut f.book, f.actor, f.company, key, &p).unwrap();
    let a = apply_credit(&mut f.book, f.actor, f.company, key, &p, Some(&r)).unwrap();
    let p = f.refund_input("1.00");
    let r = review_refund(&mut f.book, f.actor, f.company, key, &p).unwrap();
    let refund = record_refund(&mut f.book, f.actor, f.company, key, &p, Some(&r)).unwrap();
    unapply_credit(
        &mut f.book,
        f.actor,
        f.company,
        a["id"].as_str().unwrap(),
        Some(1),
        &json!("2026-10-20"),
        &json!("Release"),
    )
    .unwrap();
    void_refund(
        &mut f.book,
        f.actor,
        f.company,
        refund["id"].as_str().unwrap(),
        Some(1),
        &json!("2026-10-20"),
        &json!("Release"),
    )
    .unwrap();
    void_credit_memo(
        &mut f.book,
        f.actor,
        f.company,
        key,
        Some(1),
        &json!("2026-10-21"),
        &json!("Void"),
    )
    .unwrap();
    f.clean();
}
#[test]
fn line_count_decimal_bounds_dates_and_money_overflow_refuse_without_changes_or_panics() {
    let mut f = Fixture::new();
    let p = f.input();
    let before = snapshot(&f);
    for n in [0, 51] {
        let mut v = p.clone();
        v["lines"] = json!(vec![p["lines"][0].clone(); n]);
        assert!(review_credit_memo(&mut f.book, f.actor, f.company, &v).is_err());
        assert_eq!(snapshot(&f), before);
    }
    for value in [
        "NaN",
        "Infinity",
        "0.001",
        "-0.01",
        "100000000000000.00",
        "9e1000",
    ] {
        let mut v = p.clone();
        v["lines"][0]["rate"] = json!(value);
        assert!(
            review_credit_memo(&mut f.book, f.actor, f.company, &v).is_err(),
            "{value}"
        );
        assert_eq!(snapshot(&f), before);
    }
    let mut v = p.clone();
    v["tax_mode"] = json!("NONE");
    v["tax_rate"] = Value::Null;
    v["tax_label"] = json!("");
    v["lines"][0]["rate"] = json!("99999999999999.99");
    assert!(review_credit_memo(&mut f.book, f.actor, f.company, &v).is_err());
    assert_eq!(snapshot(&f), before);
    let mut v = p.clone();
    v["lines"] = json!(vec![p["lines"][0].clone(); 50]);
    let r = review_credit_memo(&mut f.book, f.actor, f.company, &v).unwrap();
    record_credit_memo(&mut f.book, f.actor, f.company, &v, Some(&r)).unwrap();
    f.clean();
}

#[test]
fn python_catalog_review_oddity_keeps_live_unshown_item_metadata() {
    let mut f = Fixture::new();
    let item=directory::save_record(&mut f.book,f.actor,f.company,Model::Item,&json!({"code":"WORK","description":"Old catalog description","kind":"SERVICE","sale_rate":"9.00","income_account":f.income.to_db()}),None).unwrap();
    let mut p = f.input();
    p["lines"][0]["item_id"] = item["id"].clone();
    let r = review_credit_memo(&mut f.book, f.actor, f.company, &p).unwrap();
    directory::save_record(
        &mut f.book,
        f.actor,
        f.company,
        Model::Item,
        &json!({"description":"New catalog description","sale_rate":"99.00"}),
        item["id"].as_str(),
    )
    .unwrap();
    let m = record_credit_memo(&mut f.book, f.actor, f.company, &p, Some(&r))
        .expect("Python binds the displayed item code, not unshown catalog description/rate");
    let stored =
        get_credit_memo(&mut f.book, f.actor, f.company, m["id"].as_str().unwrap()).unwrap();
    assert_eq!(
        stored["lines"][0]["item_snapshot"]["description"],
        "New catalog description"
    );
    assert_eq!(stored["lines"][0]["item_snapshot"]["sale_rate"], "99.00");
    f.clean();
}

fn bill_payment(f: &mut Fixture, number: &str) -> (Value, crate::payables::Review) {
    let ap = services::save_account(
        &mut f.book,
        f.actor,
        f.company,
        AccountInput::new("200", "Payables").kind("LIABILITY"),
    )
    .unwrap()
    .id;
    let expense = services::save_account(
        &mut f.book,
        f.actor,
        f.company,
        AccountInput::new("500", "Expense").kind("EXPENSE"),
    )
    .unwrap()
    .id;
    documents::save_setup(&mut f.book,f.actor,f.company,&json!({"receivables":f.ar.to_db(),"collected_tax":f.tax.to_db(),"payables":ap.to_db(),"banks":[f.bank.to_db()]}),Some(1)).unwrap();
    let vendor = directory::save_record(
        &mut f.book,
        f.actor,
        f.company,
        Model::Vendor,
        &json!({"display_name":"Synthetic vendor"}),
        None,
    )
    .unwrap();
    let p = json!({"request_key":new_id(),"vendor_id":vendor["id"],"date":"2026-10-01","due_date":"2026-11-01","memo":"Synthetic bill","reference":"B1","lines":[{"item_id":null,"description":"Supplies","quantity":"1","rate":"10.00","expense_account_id":expense.to_db()}]});
    let bill = crate::payables::save_bill(&mut f.book, f.actor, f.company, &p).unwrap();
    let bid = bill["id"].as_str().unwrap();
    let r = crate::payables::review_bill(&mut f.book, f.actor, f.company, bid, Some(1)).unwrap();
    crate::payables::post_bill(&mut f.book, f.actor, f.company, bid, Some(1), &r).unwrap();
    let p = json!({"request_key":new_id(),"vendor_id":vendor["id"],"date":"2026-10-11","amount":"1.00","bank_id":f.bank.to_db(),"method":"CHECK","reference":number,"memo":"Synthetic payment","allocations":[{"bill_id":bid,"amount":"1.00"}]});
    let r = crate::payables::review_payment(&mut f.book, f.actor, f.company, &p).unwrap();
    (p, r)
}
#[test]
fn bill_payment_numbers_block_refunds_and_reversal_releases_the_number() {
    let mut f = Fixture::new();
    let (p, r) = bill_payment(&mut f, "1001");
    let pay = crate::payables::record_payment(&mut f.book, f.actor, f.company, &p, &r).unwrap();
    let m = f.memo();
    let k = m["id"].as_str().unwrap();
    let rp = f.refund_input("1.00");
    let before = snapshot(&f);
    let e = review_refund(&mut f.book, f.actor, f.company, k, &rp).unwrap_err();
    assert!(
        e.message()
            .contains("already used by bill payment BPAY-000001")
    );
    assert_eq!(snapshot(&f), before);
    crate::payables::reverse_payment(
        &mut f.book,
        f.actor,
        f.company,
        pay["id"].as_str().unwrap(),
        Some(1),
        &json!("2026-10-20"),
        &json!("Release number"),
    )
    .unwrap();
    let r = review_refund(&mut f.book, f.actor, f.company, k, &rp).unwrap();
    record_refund(&mut f.book, f.actor, f.company, k, &rp, Some(&r)).unwrap();
    f.clean();
}
#[test]
fn refund_numbers_have_a_database_backstop_for_payables_and_an_exported_review_hook() {
    let mut f = Fixture::new();
    let (p, r) = bill_payment(&mut f, "1001");
    let m = f.memo();
    let rp = f.refund_input("1.00");
    let rr = review_refund(
        &mut f.book,
        f.actor,
        f.company,
        m["id"].as_str().unwrap(),
        &rp,
    )
    .unwrap();
    record_refund(
        &mut f.book,
        f.actor,
        f.company,
        m["id"].as_str().unwrap(),
        &rp,
        Some(&rr),
    )
    .unwrap();
    let before = snapshot(&f);
    let e = crate::payables::record_payment(&mut f.book, f.actor, f.company, &p, &r).unwrap_err();
    assert_eq!(e.kind(), "ValidationError");
    assert!(e.message().contains("already used by refund RFD-000001"));
    let sql_error=f.book.connection().execute("INSERT INTO payables_billpayment(company_id,bank_id,method,reference,vendor_snapshot,company_snapshot) VALUES(?,?,'CHECK','1001','{}','{}')",params![f.company.to_db(),f.bank.to_db()]).unwrap_err();
    assert_eq!(fb_db::constraint_of(&sql_error).as_deref(),Some("check_payment_number"));
    assert_eq!(snapshot(&f), before);
    let e = f
        .book
        .write(|tx| refuse_number_conflict(tx, f.company, &f.bank.to_db(), "1001"))
        .unwrap_err();
    assert!(e.message().contains("already used by refund RFD-000001"));
    assert_eq!(snapshot(&f), before);
    f.clean();
}

#[test]
fn restored_schema_still_detects_every_credit_chronology_violation_on_check_and_open() {
    for (sql, wanted) in [
        (
            "UPDATE credits_creditapplication SET date='2026-10-07',data=json_set(data,'$.date','2026-10-07')",
            "credit application target",
        ),
        (
            "UPDATE credits_creditunapplication SET date='2026-10-08',data=json_set(data,'$.date','2026-10-08')",
            "credit unapply date",
        ),
        (
            "UPDATE credits_creditrefund SET date='2026-10-07',data=json_set(data,'$.date','2026-10-07')",
            "credit refund date",
        ),
        (
            "UPDATE credits_creditrefundvoid SET date='2026-10-10',data=json_set(data,'$.date','2026-10-10')",
            "credit refund void date",
        ),
        (
            "UPDATE credits_creditmemovoid SET date='2026-10-07',data=json_set(data,'$.date','2026-10-07')",
            "credit memo void date",
        ),
        (
            "UPDATE credits_creditmemovoid SET date='2026-10-19',data=json_set(data,'$.date','2026-10-19')",
            "credit memo void date",
        ),
        (
            "UPDATE ledger_journal SET date='2026-10-19' WHERE id=(SELECT reversal_id FROM sales_invoicerevision WHERE state='VOID')",
            "invoice credit release date",
        ),
    ] {
        let mut f = populated();
        let key: String = f
            .book
            .connection()
            .query_row("SELECT id FROM sales_invoice", [], |r| r.get(0))
            .unwrap();
        crate::sales::void_invoice(
            &mut f.book,
            f.actor,
            f.company,
            &key,
            Some(2),
            &json!("2026-10-21"),
            &json!("Release invoice"),
        )
        .unwrap();
        let c = f.book.connection();
        let mut st = c
            .prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger'")
            .unwrap();
        let defs = st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        drop(st);
        c.execute_batch("BEGIN IMMEDIATE").unwrap();
        for (name, _) in &defs {
            c.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
        }
        c.execute_batch(sql).unwrap();
        for (_, definition) in &defs {
            c.execute_batch(definition).unwrap();
        }
        c.execute_batch("COMMIT").unwrap();
        let problems = f.book.check().unwrap();
        assert!(
            problems.iter().any(|s| s.contains(wanted)),
            "{sql}: {problems:?}"
        );
        assert!(!problems.iter().any(|s| s.contains("schema")));
        assert!(Book::open(&f.path, &[24; 32]).is_err());
    }
}

#[test]
fn sealed_composite_identities_never_move_or_replace_with_recursion_on_or_off() {
    let f = populated();
    let c = f.book.connection();
    for recursive in [false, true] {
        c.pragma_update(None, "recursive_triggers", recursive)
            .unwrap();
        for table in ["credits_seal", "credits_seal_due"] {
            let before = snapshot(&f);
            for op in ["UPDATE", "UPDATE OR REPLACE"] {
                for field in ["kind", "doc_id"] {
                    let sql = format!("{op} {table} SET {field}=?1");
                    let e = c.execute(&sql, [new_id()]).unwrap_err();
                    assert_eq!(
                        fb_db::constraint_of(&e).as_deref(),
                        Some("credit_immutable")
                    );
                    assert_eq!(snapshot(&f), before);
                }
            }
            for op in ["INSERT OR REPLACE", "REPLACE", "INSERT OR IGNORE"] {
                let e = c
                    .execute(
                        &format!("{op} INTO {table} SELECT * FROM {table} LIMIT 1"),
                        [],
                    )
                    .unwrap_err();
                assert_eq!(
                    fb_db::constraint_of(&e).as_deref(),
                    Some("credit_immutable")
                );
                assert_eq!(snapshot(&f), before);
            }
            assert!(c.execute(&format!("DELETE FROM {table}"), []).is_err());
            for hidden in ["rowid", "_rowid_", "oid"] {
                assert!(c.execute(&format!("INSERT OR REPLACE INTO {table}({hidden},kind,doc_id) VALUES(1,'memo','fresh')"),[]).is_err());
                assert!(
                    c.execute(&format!("UPDATE OR REPLACE {table} SET {hidden}=1"), [])
                        .is_err()
                );
            }
            assert_eq!(snapshot(&f), before);
        }
    }
    c.pragma_update(None, "recursive_triggers", true).unwrap();
    f.clean();
}

#[test]
fn independent_connections_serialize_credit_creation_and_application() {
    use std::sync::{Arc, Barrier};
    let mut f = Fixture::new();
    let input = f.input();
    let review = review_credit_memo(&mut f.book, f.actor, f.company, &input).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let mut threads = vec![];
    for _ in 0..2 {
        let path = f.path.clone();
        let input = input.clone();
        let review = review.clone();
        let barrier = barrier.clone();
        let actor = f.actor;
        let company = f.company;
        threads.push(std::thread::spawn(move || {
            let mut b = Book::open(&path, &[24; 32]).unwrap();
            barrier.wait();
            record_credit_memo(&mut b, actor, company, &input, Some(&review)).unwrap()["id"].clone()
        }));
    }
    let keys = threads
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(keys[0], keys[1]);
    let m = get_credit_memo(&mut f.book, f.actor, f.company, keys[0].as_str().unwrap()).unwrap();
    let inv = f.invoice();
    let mut pending = vec![];
    for _ in 0..2 {
        let p = f.application(&inv, "10.00");
        let r = review_application(
            &mut f.book,
            f.actor,
            f.company,
            m["id"].as_str().unwrap(),
            &p,
        )
        .unwrap();
        pending.push((p, r));
    }
    let barrier = Arc::new(Barrier::new(2));
    let mut threads = vec![];
    for (input, review) in pending {
        let path = f.path.clone();
        let memo = m["id"].as_str().unwrap().to_owned();
        let barrier = barrier.clone();
        let actor = f.actor;
        let company = f.company;
        threads.push(std::thread::spawn(move || {
            let mut b = Book::open(&path, &[24; 32]).unwrap();
            barrier.wait();
            apply_credit(&mut b, actor, company, &memo, &input, Some(&review)).is_ok()
        }));
    }
    assert_eq!(
        threads
            .into_iter()
            .map(|t| usize::from(t.join().unwrap()))
            .sum::<usize>(),
        1
    );
    assert_eq!(f.available(), "3.30");
    f.clean();
}

#[test]
fn refund_bank_eligibility_matches_services_for_all_asset_detail_types() {
    let mut f = Fixture::new();
    let m = f.memo();
    let key = m["id"].as_str().unwrap();
    let mut revision = 1;
    for (index, detail) in crate::model::DetailType::ALL.into_iter().enumerate() {
        let a = services::save_account(
            &mut f.book,
            f.actor,
            f.company,
            AccountInput::new(&format!("D{index}"), detail.as_str())
                .kind(detail.kind().as_str())
                .detail(detail.as_str()),
        )
        .unwrap();
        if detail.kind().as_str() != "ASSET" {
            continue;
        }
        documents::save_setup(&mut f.book,f.actor,f.company,&json!({"receivables":f.ar.to_db(),"collected_tax":f.tax.to_db(),"banks":[a.id.to_db()]}),Some(revision)).unwrap();
        revision += 1;
        let mut p = f.refund_input("0.01");
        p["bank_id"] = json!(a.id.to_db());
        p["check_number"] = json!("");
        let r = review_refund(&mut f.book, f.actor, f.company, key, &p)
            .expect("a designated, never-used ASSET is accepted by the frozen service");
        let result = record_refund(&mut f.book, f.actor, f.company, key, &p, Some(&r));
        assert!(result.is_ok(), "{}: {result:?}", detail.as_str());
    }
    f.clean();
}

fn audit_fail(f: &Fixture, action: &str) {
    f.book.connection().execute_batch(&format!("CREATE TRIGGER credits_test_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='{action}' BEGIN SELECT RAISE(ABORT,'credit_test_audit: injected late audit failure'); END;")).unwrap();
}
fn audit_restore(f: &Fixture) {
    f.book
        .connection()
        .execute_batch("DROP TRIGGER credits_test_audit")
        .unwrap();
}
#[test]
fn all_mutations_roll_back_on_late_audit_failure() {
    let mut f = Fixture::new();
    let p = f.input();
    let r = review_credit_memo(&mut f.book, f.actor, f.company, &p).unwrap();
    audit_fail(&f, "credit_memo.recorded");
    let before = snapshot(&f);
    assert!(record_credit_memo(&mut f.book, f.actor, f.company, &p, Some(&r)).is_err());
    assert_eq!(snapshot(&f), before);
    audit_restore(&f);
    let m = f.memo();
    let inv = f.invoice();
    let k = m["id"].as_str().unwrap();
    let p = f.application(&inv, "1.00");
    let r = review_application(&mut f.book, f.actor, f.company, k, &p).unwrap();
    audit_fail(&f, "credit_memo.applied");
    let before = snapshot(&f);
    assert!(apply_credit(&mut f.book, f.actor, f.company, k, &p, Some(&r)).is_err());
    assert_eq!(snapshot(&f), before);
    audit_restore(&f);
    let a = apply_credit(&mut f.book, f.actor, f.company, k, &p, Some(&r)).unwrap();
    let aid = a["id"].as_str().unwrap();
    audit_fail(&f, "credit_memo.unapplied");
    let before = snapshot(&f);
    assert!(
        unapply_credit(
            &mut f.book,
            f.actor,
            f.company,
            aid,
            Some(1),
            &json!("2026-10-20"),
            &json!("Release")
        )
        .is_err()
    );
    assert_eq!(snapshot(&f), before);
    audit_restore(&f);
    unapply_credit(
        &mut f.book,
        f.actor,
        f.company,
        aid,
        Some(1),
        &json!("2026-10-20"),
        &json!("Release"),
    )
    .unwrap();
    let p = f.refund_input("1.00");
    let r = review_refund(&mut f.book, f.actor, f.company, k, &p).unwrap();
    audit_fail(&f, "credit_refund.recorded");
    let before = snapshot(&f);
    assert!(record_refund(&mut f.book, f.actor, f.company, k, &p, Some(&r)).is_err());
    assert_eq!(snapshot(&f), before);
    audit_restore(&f);
    let rf = record_refund(&mut f.book, f.actor, f.company, k, &p, Some(&r)).unwrap();
    let rid = rf["id"].as_str().unwrap();
    audit_fail(&f, "credit_refund.voided");
    let before = snapshot(&f);
    assert!(
        void_refund(
            &mut f.book,
            f.actor,
            f.company,
            rid,
            Some(1),
            &json!("2026-10-20"),
            &json!("Release")
        )
        .is_err()
    );
    assert_eq!(snapshot(&f), before);
    audit_restore(&f);
    void_refund(
        &mut f.book,
        f.actor,
        f.company,
        rid,
        Some(1),
        &json!("2026-10-20"),
        &json!("Release"),
    )
    .unwrap();
    audit_fail(&f, "credit_memo.voided");
    let before = snapshot(&f);
    assert!(
        void_credit_memo(
            &mut f.book,
            f.actor,
            f.company,
            k,
            Some(1),
            &json!("2026-10-21"),
            &json!("Void")
        )
        .is_err()
    );
    assert_eq!(snapshot(&f), before);
    audit_restore(&f);
    f.clean();
}
#[test]
fn malformed_inputs_and_review_domains_change_nothing() {
    let mut f = Fixture::new();
    let inv = f.invoice();
    let m = f.memo();
    let k = m["id"].as_str().unwrap();
    let before = snapshot(&f);
    let base = f.input();
    for (field, values) in [
        (
            "customer_id",
            vec![json!(true), json!([]), json!({}), json!(10)],
        ),
        ("memo", vec![Value::Null, json!(1)]),
        ("date", vec![json!("2026-02-30"), json!(false), json!("💥")]),
        ("request_key", vec![json!(false), Value::Null, json!("bad")]),
        ("tax_item_id", vec![json!(false), json!(1), json!([])]),
    ] {
        for value in values {
            let mut p = base.clone();
            p[field] = value;
            assert!(
                review_credit_memo(&mut f.book, f.actor, f.company, &p).is_err(),
                "{p}"
            );
            assert_eq!(snapshot(&f), before);
        }
    }
    for field in [
        "item_id",
        "income_account_id",
        "description",
        "quantity",
        "rate",
        "taxable",
    ] {
        for value in [Value::Null, json!(true), json!([]), json!({})] {
            if (field == "item_id" && value.is_null()) || (field == "taxable" && value.is_boolean())
            {
                continue;
            }
            let mut p = base.clone();
            p["lines"][0][field] = value;
            assert!(
                review_credit_memo(&mut f.book, f.actor, f.company, &p).is_err(),
                "{field}: {p}"
            );
            assert_eq!(snapshot(&f), before);
        }
    }
    let a = f.application(&inv, "1.00");
    let ar = review_application(&mut f.book, f.actor, f.company, k, &a).unwrap();
    assert!(record_credit_memo(&mut f.book, f.actor, f.company, &base, Some(&ar)).is_err());
    let rp = f.refund_input("1.00");
    let rr = review_refund(&mut f.book, f.actor, f.company, k, &rp).unwrap();
    assert!(apply_credit(&mut f.book, f.actor, f.company, k, &a, Some(&rr)).is_err());
    let mr = review_credit_memo(&mut f.book, f.actor, f.company, &base).unwrap();
    assert!(record_refund(&mut f.book, f.actor, f.company, k, &rp, Some(&mr)).is_err());
    assert_eq!(snapshot(&f), before);
    f.clean();
}
#[test]
fn stale_review_binds_party_mapping_and_balances() {
    let mut f = Fixture::new();
    let p = f.input();
    let r = review_credit_memo(&mut f.book, f.actor, f.company, &p).unwrap();
    directory::save_record(
        &mut f.book,
        f.actor,
        f.company,
        Model::Customer,
        &json!({"display_name":"Renamed synthetic customer"}),
        Some(&f.customer),
    )
    .unwrap();
    let before = snapshot(&f);
    assert!(record_credit_memo(&mut f.book, f.actor, f.company, &p, Some(&r)).is_err());
    assert_eq!(snapshot(&f), before);
    let m = f.memo();
    let inv = f.invoice();
    let k = m["id"].as_str().unwrap();
    let p = f.application(&inv, "1.00");
    let stale = review_application(&mut f.book, f.actor, f.company, k, &p).unwrap();
    let other = f.application(&inv, "1.00");
    let r = review_application(&mut f.book, f.actor, f.company, k, &other).unwrap();
    apply_credit(&mut f.book, f.actor, f.company, k, &other, Some(&r)).unwrap();
    let before = snapshot(&f);
    assert!(apply_credit(&mut f.book, f.actor, f.company, k, &p, Some(&stale)).is_err());
    assert_eq!(snapshot(&f), before);
    f.clean();
}
#[test]
fn orphan_journal_and_incomplete_application_cannot_commit() {
    let mut f = Fixture::new();
    let m = f.memo();
    let before = snapshot(&f);
    let e = f
        .book
        .write(|tx| -> FbResult<()> {
            documents::reverse_source(
                tx,
                f.actor,
                f.company,
                m["identity_id"].as_str().unwrap(),
                "2026-10-20",
                "Bypass lifecycle",
            )?;
            Ok(())
        })
        .unwrap_err();
    assert!(e.message().contains("FOREIGN KEY"));
    assert_eq!(snapshot(&f), before);
    let p = f.refund_input("1.00");
    let e = f
        .book
        .write(|tx| -> FbResult<()> {
            let id = documents::create_identity(
                tx,
                f.actor,
                f.company,
                "RFD",
                p["request_key"].as_str().unwrap(),
            )?;
            documents::post_source(
                tx,
                f.actor,
                f.company,
                id["id"].as_str().unwrap(),
                "2026-10-20",
                "Orphan",
                vec![
                    json!({"account":f.ar.to_db(),"debit":"1.00","credit":"0.00"}),
                    json!({"account":f.bank.to_db(),"debit":"0.00","credit":"1.00"}),
                ],
            )?;
            Ok(())
        })
        .unwrap_err();
    assert!(e.message().contains("FOREIGN KEY"));
    assert_eq!(snapshot(&f), before);
    f.clean();
}
#[test]
fn populated_prior_book_upgrade_preserves_all_old_rows_and_rejects_tampering() {
    let mut f = Fixture::new();
    f.invoice();
    let path=f.path.with_extension("prior-upgrade.fbbook");
    let copy=fb_db::test_support::populated_copy(f.book.connection(),&path,&[24;32],"0060_credits",false).unwrap();
    f.path = path.clone();
    f.book=Book::open(&f.path,&[24;32]).unwrap();
    assert_eq!(f.book.connection().path(), f.path.to_str(), "fixture must follow upgraded copy");
    for _ in 0..2 {
        assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(),copy.expected);
        f.clean();
        f.book = Book::open(&f.path, &[24; 32]).unwrap();
    }
    assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(),copy.expected);
    f.book.connection().execute_batch("DROP TRIGGER fb_credits_memo_scope; CREATE TRIGGER fb_credits_memo_scope BEFORE INSERT ON credits_creditmemo BEGIN SELECT 1; END;").unwrap();
    assert!(!f.book.check().unwrap().is_empty());
    drop(f.book);
    assert!(Book::open(&path, &[24; 32]).is_err());
}

#[test]
fn review_open_detects_live_refund_payment_collision_and_void_releases_it() {
    let mut f = Fixture::new();
    let (p, r) = bill_payment(&mut f, "1001");
    crate::payables::record_payment(&mut f.book, f.actor, f.company, &p, &r).unwrap();
    let m = f.memo();
    let mut p = f.refund_input("1.00");
    p["check_number"] = json!("1002");
    let r = review_refund(
        &mut f.book,
        f.actor,
        f.company,
        m["id"].as_str().unwrap(),
        &p,
    )
    .unwrap();
    let refund = record_refund(
        &mut f.book,
        f.actor,
        f.company,
        m["id"].as_str().unwrap(),
        &p,
        Some(&r),
    )
    .unwrap();
    f.clean();
    let c = f.book.connection();
    let definition: String = c
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='fb_credits_creditrefund_update'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    c.execute_batch("BEGIN IMMEDIATE; DROP TRIGGER fb_credits_creditrefund_update;")
        .unwrap();
    c.execute("UPDATE credits_creditrefund SET check_number='1001',data=json_set(data,'$.check_number','1001') WHERE id=?1", [refund["id"].as_str().unwrap()]).unwrap();
    c.execute_batch(&definition).unwrap();
    c.execute_batch("COMMIT").unwrap();
    let problems = f.book.check().unwrap();
    assert!(
        problems
            .iter()
            .any(|p| p.starts_with("duplicate live refund number")),
        "{problems:?}"
    );
    assert!(Book::open(&f.path, &[24; 32]).is_err());
    void_refund(
        &mut f.book,
        f.actor,
        f.company,
        refund["id"].as_str().unwrap(),
        Some(1),
        &json!("2026-10-20"),
        &json!("Release synthetic number"),
    )
    .unwrap();
    f.clean();
}
