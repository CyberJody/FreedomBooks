use super::*;

fn raw_edit(
    f: &mut Fx,
    b: &Value,
    mutate: impl FnOnce(&mut Value, &mut Vec<Value>),
) -> FbResult<()> {
    let (actor, co) = (f.actor, f.co);
    f.book.write(|tx| {
        let mut root = check(tx, co, s(b, "id"))?;
        let mut r = root["current"].clone();
        let mut lines = r["lines"].as_array().unwrap().clone();
        mutate(&mut r, &mut lines);
        root["revision"] = json!(2);
        r["action"] = json!("revised");
        tx.execute(
            "UPDATE checks_check SET revision=2,number_key=? WHERE id=?",
            params![s(&r, "check_number"), s(&root, "id")],
        )?;
        append(tx, actor, co, &root, r, lines)
    })
}

#[test]
fn review_snapshot_shapes_must_fail_insert_and_reopen() {
    let mut accepted = vec![];
    for field in [
        "bank_snapshot",
        "payee_snapshot",
        "company_snapshot",
        "account_snapshot",
        "item_snapshot",
    ] {
        for bad in [json!(false), json!([]), json!({"unexpected": "value"})] {
            let mut f = Fx::new();
            let p = f.payload();
            let b = f.save(&p);
            let result = raw_edit(&mut f, &b, |r, lines| {
                if ["account_snapshot", "item_snapshot"].contains(&field) {
                    lines[0][field] = bad.clone();
                } else {
                    r[field] = bad.clone();
                }
            });
            if result.is_ok() {
                let clean = f.book.check().unwrap().is_empty();
                let path = std::path::PathBuf::from(f.book.connection().path().unwrap());
                drop(f);
                let reopened = Book::open(&path, &[42; 32]).is_ok();
                accepted.push(format!(
                    "{field}={bad}: check_clean={clean}, reopened={reopened}"
                ));
            }
        }
    }
    assert!(
        accepted.is_empty(),
        "Malformed snapshot shapes accepted:\n{}",
        accepted.join("\n")
    );
}

#[test]
fn review_new_inactive_line_must_not_borrow_another_lines_retention() {
    let mut f = Fx::new();
    let item = directory::save_record(&mut f.book, f.actor, f.co, Model::Item,
        &json!({"code":"Retained","kind":"NONSTOCK","sale_rate":"1","purchase_cost":"2","expense_account":f.expense.to_db()}), None).unwrap();
    let mut p = f.payload();
    p["lines"] = json!([{"tab":"ITEM","item_id":item["id"],"account_id":f.expense.to_db(),"quantity":"1","rate":"10.01"}]);
    let b = f.save(&p);
    directory::set_active(
        &mut f.book,
        f.actor,
        f.co,
        Model::Item,
        s(&item, "id"),
        false,
    )
    .unwrap();
    f.book
        .write(|tx| -> FbResult<()> {
            tx.execute(
                "UPDATE ledger_account SET active=0 WHERE id=?",
                [f.expense.to_db()],
            )?;
            Ok(())
        })
        .unwrap();
    p["check_id"] = b["id"].clone();
    p["expected_revision"] = json!(1);
    p["lines"][0]["keep_archived"] = json!(true);
    let mut added = p["lines"][0].clone();
    added["origin_position"] = Value::Null;
    p["lines"].as_array_mut().unwrap().push(added);
    assert!(
        save_check(&mut f.book, f.actor, f.co, &p).is_err(),
        "service must reject newly selected inactive references"
    );
    let result = raw_edit(&mut f, &b, |r, lines| {
        lines[0]["origin_position"] = json!(1);
        let mut added = lines[0].clone();
        added["position"] = json!(2);
        added["origin_position"] = Value::Null;
        lines.push(added);
        r["total"] = json!(2002);
    });
    if result.is_ok() {
        assert!(f.book.check().unwrap().is_empty());
    }
    assert!(
        result.is_err(),
        "SQL committed a brand-new inactive item/account line by borrowing the single old line's retention"
    );
}

#[test]
fn review_reordered_retention_uses_origin_and_preserves_snapshot() {
    let mut f = Fx::new();
    let item = directory::save_record(&mut f.book, f.actor, f.co, Model::Item,
        &json!({"code":"Before","kind":"NONSTOCK","sale_rate":"1","purchase_cost":"2","expense_account":f.expense.to_db()}), None).unwrap();
    let mut p = f.payload();
    p["lines"].as_array_mut().unwrap().push(json!({"tab":"ITEM","item_id":item["id"],"account_id":f.bank2.to_db(),"quantity":"1","rate":"2"}));
    let b = f.save(&p);
    directory::set_active(
        &mut f.book,
        f.actor,
        f.co,
        Model::Item,
        s(&item, "id"),
        false,
    )
    .unwrap();
    f.book
        .write(|tx| -> FbResult<()> {
            tx.execute(
                "UPDATE ledger_account SET active=0,postable=0 WHERE id=?",
                [f.bank2.to_db()],
            )?;
            Ok(())
        })
        .unwrap();
    p["check_id"] = b["id"].clone();
    p["expected_revision"] = json!(1);
    p["lines"][0]["origin_position"] = json!(1);
    p["lines"][1]["origin_position"] = json!(2);
    p["lines"][1]["keep_archived"] = json!(true);
    p["lines"].as_array_mut().unwrap().swap(0, 1);
    let edited = f.save(&p);
    assert_eq!(
        edited["current"]["lines"][0]["item_snapshot"],
        b["current"]["lines"][1]["item_snapshot"]
    );
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn review_fresh_key_update_or_replace_refused_for_all_five_tables() {
    // Attacker's unused primary key: canonical, so 0130 passes and the permanent-key guards decide.
    const FRESH_UNUSED_KEY: &str = "00000000-0000-7000-8000-0000000ab103";
    const EXPECTED: &[&str] = &[
        "checks_check: checks_primary_key",
        "checks_checkline: checks_history_immutable",
        "checks_checkrevision: checks_history_immutable",
        "transfers_transfer: checks_history_immutable",
        "transfers_transfervoid: checks_history_immutable",
    ];
    let mut hit = BTreeSet::new();
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let b = f.post(&b);
    f.void(&b);
    let p = f.transfer();
    let r = crate::transfers::review_transfer(&mut f.book, f.actor, f.co, &p).unwrap();
    let t = crate::transfers::record_transfer(&mut f.book, f.actor, f.co, &p, &r).unwrap();
    crate::transfers::void_transfer(
        &mut f.book,
        f.actor,
        f.co,
        s(&t, "id"),
        Some(1),
        &json!("2026-10-24"),
        &json!("fix"),
    )
    .unwrap();
    let c = f.book.connection();
    let before = super::guards::snapshot(c);
    for recursion in [0, 1] {
        c.execute_batch(&format!("PRAGMA recursive_triggers={recursion}"))
            .unwrap();
        for table in [
            "checks_check",
            "checks_checkrevision",
            "checks_checkline",
            "transfers_transfer",
            "transfers_transfervoid",
        ] {
            let guard = super::primitive::sql_refused(c.execute_batch(&format!(
                "UPDATE OR REPLACE {table} SET id='{FRESH_UNUSED_KEY}'"
            )));
            hit.insert(format!("{table}: {guard}"));
            assert_eq!(before, super::guards::snapshot(c));
        }
    }
    super::primitive::assert_guard_set(&hit, EXPECTED);
}

#[test]
fn review_sql_numbers_must_reject_control_and_format_characters() {
    let mut accepted = vec![];
    for number in ["a\u{200b}", "a\u{e000}", "a\u{2028}", "a\u{0001}"] {
        let mut f = Fx::new();
        let p = f.payload();
        let b = f.save(&p);
        assert!(number_text(&json!(number)).is_err());
        let result = raw_edit(&mut f, &b, |r, _| r["check_number"] = json!(number));
        if result.is_ok() {
            let clean = f.book.check().unwrap().is_empty();
            let path = std::path::PathBuf::from(f.book.connection().path().unwrap());
            drop(f);
            accepted.push(format!(
                "{number:?}: clean={clean}, reopened={}",
                Book::open(&path, &[42; 32]).is_ok()
            ));
        }
    }
    assert!(
        accepted.is_empty(),
        "SQL accepted forbidden check-number characters: {accepted:?}"
    );
}

#[test]
fn review_transfer_snapshots_must_fail_reopen_when_shape_corrupted() {
    let mut accepted = vec![];
    for field in ["from_snapshot", "to_snapshot", "company_snapshot"] {
        let mut f = Fx::new();
        let p = f.transfer();
        let r = crate::transfers::review_transfer(&mut f.book, f.actor, f.co, &p).unwrap();
        crate::transfers::record_transfer(&mut f.book, f.actor, f.co, &p, &r).unwrap();
        let c = f.book.connection();
        let definition: String = c
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='fb_transfers_transfer_update'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        c.execute_batch("DROP TRIGGER fb_transfers_transfer_update")
            .unwrap();
        c.execute_batch(&format!("UPDATE transfers_transfer SET {field}='[]'"))
            .unwrap();
        c.execute_batch(&definition).unwrap();
        let clean = f.book.check().unwrap().is_empty();
        let path = std::path::PathBuf::from(c.path().unwrap());
        drop(f);
        if Book::open(&path, &[42; 32]).is_ok() {
            accepted.push(format!("{field}: clean={clean}"));
        }
    }
    assert!(
        accepted.is_empty(),
        "Malformed transfer snapshots accepted on open: {accepted:?}"
    );
}

#[test]
fn review_stored_numeric_snapshot_must_not_panic_during_public_record() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    // First prove insertion refuses the original valid-JSON numeric reproducer.
    assert!(raw_edit(&mut f, &b, |r, _| r["bank_snapshot"] = json!("1.5")).is_err());
    let review = f.review(&b);
    // Simulate external corruption, restoring the exact schema afterwards.
    let c = f.book.connection();
    let definition: String = c
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='fb_checks_checkrevision_update'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    c.execute_batch("DROP TRIGGER fb_checks_checkrevision_update; UPDATE checks_checkrevision SET bank_snapshot='1.5'").unwrap();
    c.execute_batch(&definition).unwrap();
    assert!(!f.book.check().unwrap().is_empty());
    let path = std::path::PathBuf::from(c.path().unwrap());
    let before = f.counts();
    let recorded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        record_check(&mut f.book, f.actor, f.co, s(&b, "id"), Some(1), &review)
    }));
    assert_eq!(
        before,
        f.counts(),
        "malformed evidence must not change the book"
    );
    assert!(recorded.is_ok(), "public record_check must not panic");
    assert!(
        recorded.unwrap().is_err(),
        "malformed stored snapshot must be a controlled error"
    );
    drop(f);
    assert!(Book::open(&path, &[42; 32]).is_err());
}
