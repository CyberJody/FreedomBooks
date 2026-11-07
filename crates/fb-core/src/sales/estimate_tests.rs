//! Ports of the frozen estimate tests: non-posting, revision checks, snapshots and conversion.
use super::*;

#[test]
fn d4_estimate_writes_and_both_conversion_audits_rollback_every_row() {
    let mut f = Fixture::new();
    let values = input(&f);
    fn fail_at(book: &Book, action: &str) {
        book.connection().execute_batch(&format!("CREATE TRIGGER d4_injected_failure BEFORE INSERT ON ledger_auditevent WHEN NEW.action='{action}' BEGIN SELECT RAISE(ABORT,'d4_injected: synthetic D4 late audit failure'); END;")).unwrap();
    }
    fn unchanged(book: &Book, before: &fb_db::test_support::Snapshot) {
        book.connection()
            .execute_batch("DROP TRIGGER d4_injected_failure")
            .unwrap();
        assert_eq!(
            &fb_db::test_support::snapshot(book.connection()).unwrap(),
            before
        );
    }
    let before = fb_db::test_support::snapshot(f.book.connection()).unwrap();
    fail_at(&f.book, "estimate.created");
    let e = save_estimate(&mut f.book, f.actor, f.company, &values).unwrap_err();
    assert!(e.to_string().contains("synthetic D4 late audit failure"), "{e}");
    unchanged(&f.book, &before);
    let root = save_estimate(&mut f.book, f.actor, f.company, &values).unwrap();
    let id = root["id"].as_str().unwrap();
    let mut edit = values.clone();
    edit["estimate_id"] = json!(id);
    edit["expected_revision"] = json!(1);
    edit["memo"] = json!("Changed draft");
    let before = fb_db::test_support::snapshot(f.book.connection()).unwrap();
    fail_at(&f.book, "estimate.revised");
    let e = save_estimate(&mut f.book, f.actor, f.company, &edit).unwrap_err();
    assert!(e.to_string().contains("synthetic D4 late audit failure"), "{e}");
    unchanged(&f.book, &before);
    fail_at(&f.book, "estimate.accepted");
    let e = transition_estimate(&mut f.book, f.actor, f.company, id, "ACCEPTED", Some(1), "")
        .unwrap_err();
    assert!(e.to_string().contains("synthetic D4 late audit failure"), "{e}");
    unchanged(&f.book, &before);
    transition_estimate(&mut f.book, f.actor, f.company, id, "ACCEPTED", Some(1), "").unwrap();
    let before = fb_db::test_support::snapshot(f.book.connection()).unwrap();
    for action in ["invoice.created", "invoice.converted"] {
        fail_at(&f.book, action);
        let e = convert_estimate(
            &mut f.book,
            f.actor,
            f.company,
            id,
            Some(2),
            "2026-10-02",
            "2026-11-01",
        )
        .unwrap_err();
        assert!(e.to_string().contains("synthetic D4 late audit failure"), "{e}");
        unchanged(&f.book, &before);
    }
    convert_estimate(
        &mut f.book,
        f.actor,
        f.company,
        id,
        Some(2),
        "2026-10-02",
        "2026-11-01",
    )
    .unwrap();
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn d4_estimate_populated_primary_keys_and_replacement_collisions_are_guarded() {
    let mut f = Fixture::new();
    let values = input(&f);
    let root = save_estimate(&mut f.book, f.actor, f.company, &values).unwrap();
    let id = root["id"].as_str().unwrap();
    transition_estimate(&mut f.book, f.actor, f.company, id, "ACCEPTED", Some(1), "").unwrap();
    convert_estimate(
        &mut f.book,
        f.actor,
        f.company,
        id,
        Some(2),
        "2026-10-02",
        "2026-11-01",
    )
    .unwrap();
    let conn = f.book.connection();
    let before = fb_db::test_support::snapshot(conn).unwrap();
    for recursion in [0, 1] {
        conn.execute_batch(&format!("PRAGMA recursive_triggers={recursion}"))
            .unwrap();
        for (table, replace_guard) in [
            ("sales_estimate", "estimate_identity"),
            ("sales_estimaterevision", "estimate_history"),
            ("sales_estimateline", "estimate_line"),
            ("sales_estimateconversion", "estimate_conversion"),
            ("estimate_seal", "estimate_seal"),
            ("estimate_seal_due", "estimate_history"),
        ] {
            let columns = conn
                .prepare(&format!(
                    "SELECT name FROM pragma_table_info('{table}') WHERE pk>0 ORDER BY pk"
                ))
                .unwrap()
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            for column in columns {
                let error = conn
                    .execute(&format!("UPDATE {table} SET {column}=?1"), [new_id()])
                    .unwrap_err();
                assert_eq!(
                    fb_db::constraint_of(&error).as_deref(),
                    Some(if table == "sales_estimate" {
                        "estimate_identity"
                    } else {
                        "estimate_history"
                    }),
                    "{table}.{column}: {error}"
                );
            }
            let error = conn
                .execute(
                    &format!("INSERT OR REPLACE INTO {table} SELECT * FROM {table} LIMIT 1"),
                    [],
                )
                .unwrap_err();
            assert_eq!(
                fb_db::constraint_of(&error).as_deref(),
                Some(replace_guard),
                "{table}: {error}"
            );
            assert_eq!(
                fb_db::test_support::snapshot(conn).unwrap(),
                before,
                "{table}, recursive={recursion}"
            );
        }
    }
    conn.execute_batch("PRAGMA recursive_triggers=ON").unwrap();
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn d4_unsealed_estimate_root_cannot_commit_and_rolls_back_number_and_audit() {
    let mut f = Fixture::new();
    let before = fb_db::test_support::snapshot(f.book.connection()).unwrap();
    let error = f
        .book
        .write(|tx| -> FbResult<()> {
            let identity = documents::create_identity(tx, f.actor, f.company, "EST", &new_id())?;
            tx.execute(
                "INSERT INTO sales_estimate VALUES(?1,?2,?3,?4,1)",
                params![
                    new_id(),
                    f.company.to_db(),
                    identity["id"].as_str(),
                    "0".repeat(64)
                ],
            )?;
            Ok(())
        })
        .unwrap_err();
    assert!(error.to_string().contains("FOREIGN KEY"), "{error}");
    assert_eq!(
        fb_db::test_support::snapshot(f.book.connection()).unwrap(),
        before
    );
}
fn input(f: &Fixture) -> Value {
    let mut v = f.input();
    v.as_object_mut().unwrap().remove("due_date");
    for l in v["lines"].as_array_mut().unwrap() {
        l.as_object_mut().unwrap().remove("income_account_id");
    }
    v["expiry"] = json!("2026-11-01");
    v
}

#[test]
fn d4_conversion_cannot_link_unrelated_invoice_even_with_a_matching_stored_digest() {
    let mut f = Fixture::new();
    let values = input(&f);
    let root = save_estimate(&mut f.book, f.actor, f.company, &values).unwrap();
    let id = root["id"].as_str().unwrap();
    let accepted =
        transition_estimate(&mut f.book, f.actor, f.company, id, "ACCEPTED", Some(1), "").unwrap();
    let unrelated = f.save();
    let error = f
        .book
        .write(|tx| {
            tx.execute(
                "INSERT INTO sales_estimateconversion VALUES(?1,?2,?3,?4,?5)",
                params![
                    new_id(),
                    id,
                    accepted["current"]["id"].as_str(),
                    unrelated["id"].as_str(),
                    unrelated["creation_digest"].as_str()
                ],
            )
        })
        .unwrap_err();
    assert_eq!(
        fb_db::constraint_of(&error).as_deref(),
        Some("estimate_conversion_copy")
    );
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn d4_estimate_migration_preserves_a_genuine_populated_previous_book() {
    let mut f = Fixture::new();
    f.posted();
    let path = std::env::temp_dir().join(format!("d4-previous-{}.fbbook", new_id()));
    let copy = fb_db::test_support::populated_copy(
        f.book.connection(),
        &path,
        &[13; 32],
        "0130_shared_validators",
        true,
    )
    .unwrap();
    let mut upgraded = Book::open(&path, &[13; 32]).unwrap();
    assert_eq!(
        fb_db::test_support::snapshot(upgraded.connection()).unwrap(),
        copy.expected
    );
    let root = save_estimate(&mut upgraded, f.actor, f.company, &input(&f)).unwrap();
    assert_eq!(root["current"]["total"], "4.26");
    assert!(upgraded.check().unwrap().is_empty());
}
#[test]
fn d4_estimates_are_non_posting_and_conversion_keeps_frozen_prices() {
    let mut f = Fixture::new();
    let item=directory::save_record(&mut f.book,f.actor,f.company,Model::Item,&json!({"code":"WORK","description":"Original catalog","kind":"SERVICE","sale_rate":"4.01","income_account":f.income.to_db()}),None).unwrap();
    let mut v = input(&f);
    v["lines"][0]["item_id"] = item["id"].clone();
    let root = save_estimate(&mut f.book, f.actor, f.company, &v).unwrap();
    let key = root["id"].as_str().unwrap();
    assert_eq!(root["current"]["total"], "4.26");
    let repeated = save_estimate(&mut f.book, f.actor, f.company, &v).unwrap();
    assert_eq!(root, repeated);
    let err = transition_estimate(
        &mut f.book,
        f.actor,
        f.company,
        key,
        "ACCEPTED",
        Some(2),
        "",
    )
    .unwrap_err();
    assert!(err.to_string().contains("changed since you opened"));
    transition_estimate(
        &mut f.book,
        f.actor,
        f.company,
        key,
        "ACCEPTED",
        Some(1),
        "",
    )
    .unwrap();
    let journals: i64 = f
        .book
        .read(|tx| tx.query_row("SELECT count(*) FROM ledger_journal", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(journals, 0);
    directory::save_record(&mut f.book,f.actor,f.company,Model::Item,&json!({"code":"WORK","description":"Changed catalog","kind":"SERVICE","sale_rate":"999.00","income_account":f.income.to_db()}),item["id"].as_str()).unwrap();
    let inv = convert_estimate(
        &mut f.book,
        f.actor,
        f.company,
        key,
        Some(2),
        "2026-10-02",
        "2026-11-01",
    )
    .unwrap();
    assert_eq!(inv["lines"][0]["rate"], root["lines"][0]["rate"]);
    assert_eq!(
        inv["lines"][0]["item_snapshot"],
        root["lines"][0]["item_snapshot"]
    );
    assert_eq!(inv["current"]["total"], root["current"]["total"]);
    assert_eq!(
        inv,
        convert_estimate(
            &mut f.book,
            f.actor,
            f.company,
            key,
            Some(2),
            "2026-10-02",
            "2026-11-01"
        )
        .unwrap()
    );
    assert!(
        convert_estimate(
            &mut f.book,
            f.actor,
            f.company,
            key,
            Some(2),
            "2026-10-03",
            "2026-11-01"
        )
        .unwrap_err()
        .to_string()
        .contains("already has an invoice")
    );
    let id = inv["id"].as_str().unwrap();
    let review = review_invoice(&mut f.book, f.actor, f.company, id, Some(1)).unwrap();
    post_invoice(&mut f.book, f.actor, f.company, id, Some(1), Some(&review)).unwrap();
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn d4_estimate_terminal_states_retry_and_revision_history_are_permanent() {
    let mut f = Fixture::new();
    let mut v = input(&f);
    let root = save_estimate(&mut f.book, f.actor, f.company, &v).unwrap();
    let key = root["id"].as_str().unwrap();
    assert!(
        transition_estimate(
            &mut f.book,
            f.actor,
            f.company,
            key,
            "CANCELLED",
            Some(1),
            ""
        )
        .unwrap_err()
        .to_string()
        .contains("nonblank")
    );
    let cancelled = transition_estimate(
        &mut f.book,
        f.actor,
        f.company,
        key,
        "CANCELLED",
        Some(1),
        "Client declined",
    )
    .unwrap();
    assert_eq!(cancelled["revision"], 2);
    assert_eq!(cancelled["lines"], root["lines"]);
    assert!(
        transition_estimate(&mut f.book, f.actor, f.company, key, "DRAFT", Some(2), "")
            .unwrap_err()
            .to_string()
            .contains("transition is not available")
    );
    assert_eq!(
        save_estimate(&mut f.book, f.actor, f.company, &v).unwrap()["id"],
        root["id"]
    );
    v["memo"] = json!("Changed retry");
    assert!(
        save_estimate(&mut f.book, f.actor, f.company, &v)
            .unwrap_err()
            .to_string()
            .contains("different estimate payload")
    );
    let before: i64 = f
        .book
        .read(|tx| tx.query_row("SELECT count(*) FROM ledger_auditevent", [], |r| r.get(0)))
        .unwrap();
    v["estimate_id"] = root["id"].clone();
    v["expected_revision"] = json!(2);
    assert!(
        save_estimate(&mut f.book, f.actor, f.company, &v)
            .unwrap_err()
            .to_string()
            .contains("Reopen the estimate")
    );
    let after: i64 = f
        .book
        .read(|tx| tx.query_row("SELECT count(*) FROM ledger_auditevent", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(before, after);
    for sql in [
        "DELETE FROM sales_estimate",
        "DELETE FROM sales_estimaterevision",
        "UPDATE sales_estimaterevision SET state='DRAFT'",
        "DELETE FROM sales_estimateline",
        "DELETE FROM estimate_seal",
    ] {
        let e = f.book.write(|tx| tx.execute(sql, [])).unwrap_err();
        assert!(e.to_string().contains("permanent"), "{e}");
    }
}
