use super::*;
#[test]
fn editable_reconciliation_and_cleared_status_reject_fresh_primary_keys_immediately() {
    let mut f = Fx::new();
    let line = f.journal("2026-10-01", true, "1");
    f.clear(&line);
    let rec = f.begin("1");
    let q = f.book.connection();
    for recursive in [0, 1] {
        q.execute_batch(&format!(
            "PRAGMA recursive_triggers={recursive};BEGIN IMMEDIATE"
        ))
        .unwrap();
        let before = f.snap();
        for operation in ["UPDATE", "UPDATE OR REPLACE"] {
            let fresh = new_id();
            let err=q.execute(&format!("{operation} banking_reconciliation SET id=?1,revision=2,data=json_set(data,'$.id',?1,'$.revision',2) WHERE id=?2"),params![fresh,s(&rec,"id").unwrap()]).unwrap_err();
            assert_eq!(
                fb_db::constraint_of(&err).as_deref(),
                Some("primary_key_immutable")
            );
            assert_eq!(f.snap(), before);
            let err=q.execute(&format!("{operation} banking_banklinestatus SET id=?1,revision=2 WHERE journal_line_id=?2"),params![new_id(),line]).unwrap_err();
            assert_eq!(
                fb_db::constraint_of(&err).as_deref(),
                Some("primary_key_immutable")
            );
            assert_eq!(f.snap(), before);
        }
        q.execute_batch("ROLLBACK").unwrap();
    }
    q.execute_batch("PRAGMA recursive_triggers=ON").unwrap();
    assert!(f.book.check().unwrap().is_empty());
    assert!(
        Book::open(&f.path, &[87; 32])
            .unwrap()
            .check()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn raw_completion_cannot_omit_cleared_items_even_when_the_omitted_net_is_zero() {
    let mut f = Fx::new();
    let debit = f.journal("2026-10-01", true, "10");
    let credit = f.journal("2026-10-02", false, "10");
    f.clear(&debit);
    f.clear(&credit);
    let r = f.begin("0.00");
    let before = f.snap();
    let result:FbResult<()>=f.book.write(|tx|{
        tx.execute("UPDATE banking_reconciliation SET status='DONE',revision=2,data=json_set(data,'$.status','DONE','$.revision',2,'$.beginning_balance','0.00','$.cleared_debit','0.00','$.cleared_credit','0.00','$.cleared_count',0,'$.completion_key',?1,'$.completed_by','banking-test') WHERE id=?2",params![new_id(),s(&r,"id")?])?;
        seal(tx,"reconciliation",&format!("{}:2",s(&r,"id")?))?;
        Ok(())
    });
    assert!(
        result.is_err(),
        "raw completion omitted the two cleared lines"
    );
    assert_eq!(f.snap(), before);
}
fn two_histories() -> Fx {
    let mut f = history();
    let a = f.sale("3");
    let p = f.payload(vec![a["id"].clone()]);
    let d = f.record(&p);
    reverse_deposit(
        &mut f.book,
        f.u,
        f.c,
        s(&d, "id").unwrap(),
        Some(1),
        &json!("2026-10-06"),
        &json!("Second correction"),
    )
    .unwrap();
    let line = f.journal("2026-10-07", true, "2");
    f.clear(&line);
    let r = f.begin("7");
    f.finish(&r);
    f
}
#[test]
fn every_unique_key_refuses_distinct_victims_with_recursion_on_and_off_and_view_propagation() {
    let f = two_histories();
    let q = f.book.connection();
    for recursive in [0, 1] {
        q.execute_batch(&format!("PRAGMA recursive_triggers={recursive}"))
            .unwrap();
        for table in TABLES {
            let composite = table.starts_with("banking_seal");
            let actor_table = *table == "banking_reconciliationactor";
            let pk = if composite {
                "kind||':'||doc_id"
            } else if actor_table {
                "reconciliation_id||':'||phase"
            } else {
                "id"
            };
            let mut st = q.prepare(&format!("PRAGMA index_list({table})")).unwrap();
            let indices = st
                .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            let keys = q
                .prepare(&format!("SELECT {pk} FROM {table} ORDER BY {pk} LIMIT 2"))
                .unwrap()
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(keys.len(), 2, "{table} needs source and victim");
            let source = keys[0].replace('\'', "''");
            let victim = keys[1].replace('\'', "''");
            for (index, unique) in indices {
                if unique == 0 {
                    continue;
                }
                let columns = q
                    .prepare(&format!("PRAGMA index_info('{index}')"))
                    .unwrap()
                    .query_map([], |r| r.get::<_, String>(2))
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                let assignment = columns
                    .iter()
                    .map(|col| format!("{col}=(SELECT {col} FROM {table} WHERE {pk}='{victim}')"))
                    .collect::<Vec<_>>()
                    .join(",");
                for operation in ["UPDATE", "UPDATE OR REPLACE"] {
                    let before = f.snap();
                    let sql = format!("{operation} {table} SET {assignment} WHERE {pk}='{source}'");
                    assert!(q.execute_batch(&sql).is_err(), "{sql}");
                    assert_eq!(f.snap(), before);
                }
                q.execute_batch(&format!("CREATE VIEW banking_matrix_view AS SELECT * FROM {table};CREATE TRIGGER banking_matrix_view_update INSTEAD OF UPDATE ON banking_matrix_view BEGIN UPDATE {table} SET {} WHERE {} ; END;",columns.iter().map(|k|format!("{k}=NEW.{k}")).collect::<Vec<_>>().join(","),if composite{"kind=OLD.kind AND doc_id=OLD.doc_id"}else if actor_table{"reconciliation_id=OLD.reconciliation_id AND phase=OLD.phase"}else{"id=OLD.id"})).unwrap();
                let before = f.snap();
                let assignment = columns
                    .iter()
                    .map(|col| format!("{col}=(SELECT {col} FROM {table} WHERE {pk}='{victim}')"))
                    .collect::<Vec<_>>()
                    .join(",");
                assert!(q.execute_batch(&format!("UPDATE OR REPLACE banking_matrix_view SET {assignment} WHERE {pk}='{source}'")).is_err());
                assert_eq!(f.snap(), before);
                q.execute_batch(
                    "DROP TRIGGER banking_matrix_view_update;DROP VIEW banking_matrix_view",
                )
                .unwrap();
            }
        }
    }
    q.execute_batch("PRAGMA recursive_triggers=ON").unwrap();
    assert!(f.book.check().unwrap().is_empty());
    assert!(
        Book::open(&f.path, &[87; 32])
            .unwrap()
            .check()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn reconciliation_and_status_exhausted_revisions_return_errors_without_panics() {
    let mut f = Fx::new();
    let line = f.journal("2026-10-01", true, "1");
    f.clear(&line);
    let r = f.begin("1");
    let q = f.book.connection();
    let definitions=q.prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger' AND tbl_name IN('banking_reconciliation','banking_banklinestatus')").unwrap().query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
    for (name, _) in &definitions {
        q.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
    }
    q.execute_batch("UPDATE banking_reconciliation SET revision=9223372036854775807,data=json_set(data,'$.revision',9223372036854775807);UPDATE banking_banklinestatus SET revision=9223372036854775807;").unwrap();
    for (_, sql) in definitions {
        q.execute_batch(&sql).unwrap();
    }
    let before = f.snap();
    assert!(set_cleared(&mut f.book, f.u, f.c, &line, "U", Some(i64::MAX), None).is_err());
    assert_eq!(f.snap(), before);
    assert!(
        reconcile_now(
            &mut f.book,
            f.u,
            f.c,
            s(&r, "id").unwrap(),
            &f.bank.to_db(),
            Some(i64::MAX),
            &json!(new_id())
        )
        .is_err()
    );
    assert_eq!(f.snap(), before);
}
#[test]
fn charges_cannot_be_rewritten_and_invalid_totals_cannot_be_sealed() {
    let mut f = Fx::new();
    let r=begin(&mut f.book,f.u,f.c,&f.bank.to_db(),&json!({"request_key":new_id(),"statement_date":"2026-10-31","ending_balance":"-2","service_charge":"2","service_account_id":f.expense.to_db()})).unwrap();
    let before = f.snap();
    let result:FbResult<()>=f.book.write(|tx|{tx.execute("UPDATE banking_reconciliation SET revision=2,data=json_set(data,'$.revision',2,'$.service_charge','3.00') WHERE id=?1",[s(&r,"id")?])?;Ok(())});
    assert_eq!(
        result.unwrap_err().constraint(),
        Some("reconciliation_charges_fixed")
    );
    assert_eq!(f.snap(), before);
}
