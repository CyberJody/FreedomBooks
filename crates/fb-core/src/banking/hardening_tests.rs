use super::*;
#[test]
fn concurrent_begins_serialize_and_leave_exactly_one_open_statement() {
    let f = Fx::new();
    let left = Book::open(&f.path, &[87; 32]).unwrap();
    let right = Book::open(&f.path, &[87; 32]).unwrap();
    let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut handles = vec![];
    for mut book in [left, right] {
        let gate = gate.clone();
        let (u, c, bank) = (f.u, f.c, f.bank);
        handles.push(std::thread::spawn(move||{gate.wait();begin(&mut book,u,c,&bank.to_db(),&json!({"request_key":new_id(),"statement_date":"2026-10-31","ending_balance":"0.00"}))}));
    }
    let results = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| r.as_ref().is_err_and(|e| e.kind() == "ValidationError"))
            .count(),
        1
    );
    assert_eq!(
        f.book
            .connection()
            .query_row(
                "SELECT count(*) FROM banking_reconciliation WHERE status='OPEN'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn concurrent_deposit_and_sales_receipt_void_cannot_both_win() {
    let mut f = Fx::new();
    let receipt = f.sale("10.00");
    let input = f.payload(vec![receipt["id"].clone()]);
    let review = review_deposit(&mut f.book, f.u, f.c, &input).unwrap();
    let mut deposit_book = Book::open(&f.path, &[87; 32]).unwrap();
    let mut void_book = Book::open(&f.path, &[87; 32]).unwrap();
    let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
    let other = gate.clone();
    let (u, c) = (f.u, f.c);
    let deposit_thread = std::thread::spawn(move || {
        gate.wait();
        record_deposit(&mut deposit_book, u, c, &input, &review)
    });
    let void_thread = std::thread::spawn(move || {
        other.wait();
        sales::void_sales_receipt(
            &mut void_book,
            u,
            c,
            s(&receipt, "id").unwrap(),
            Some(1),
            &json!("2026-10-03"),
            &json!("Concurrent synthetic correction"),
        )
    });
    let results = [deposit_thread.join().unwrap(), void_thread.join().unwrap()];
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| r.as_ref().is_err_and(|e| e.kind() == "ValidationError"))
            .count(),
        1
    );
    let counts:(i64,i64)=f.book.connection().query_row("SELECT (SELECT count(*) FROM banking_deposit),(SELECT count(*) FROM sales_receiptreversal)",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(counts.0 + counts.1, 1);
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn register_aggregates_are_not_limited_to_a_single_document_amount() {
    let mut f = Fx::new();
    f.journal("2026-10-01", true, "99999999999999.99");
    f.journal("2026-10-02", true, "99999999999999.99");
    let r = build(&mut f.book, f.u, f.c, &f.bank.to_db(), &json!({}), 1, 50).unwrap();
    assert_eq!(r["closing"], "199999999999999.98");
    assert_eq!(r["totals"]["right_open"], "199999999999999.98");
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn register_query_count_is_bounded_as_rows_grow() {
    struct Counted<'a, 'b> {
        q: &'a fb_db::Reader<'b>,
        n: std::cell::Cell<usize>,
    }
    impl Query for Counted<'_, '_> {
        fn prepare(&self, sql: &str) -> fb_db::rusqlite::Result<fb_db::rusqlite::Statement<'_>> {
            self.n.set(self.n.get() + 1);
            self.q.prepare(sql)
        }
        fn query_row<
            T,
            P: fb_db::rusqlite::Params,
            F: FnOnce(&fb_db::rusqlite::Row<'_>) -> fb_db::rusqlite::Result<T>,
        >(
            &self,
            sql: &str,
            params: P,
            f: F,
        ) -> fb_db::rusqlite::Result<T> {
            self.n.set(self.n.get() + 1);
            self.q.query_row(sql, params, f)
        }
    }
    let mut f = Fx::new();
    f.journal("2026-10-01", true, "1");
    let mut counts = vec![];
    for many in [false, true] {
        if many {
            for _ in 0..20 {
                f.journal("2026-10-01", true, "1");
            }
        }
        let count = f
            .book
            .read(|q| -> FbResult<usize> {
                let q = Counted {
                    q,
                    n: std::cell::Cell::new(0),
                };
                let v = register::build_query(&q, f.c, &f.bank.to_db(), &json!({}), 1, 50)?;
                assert_eq!(v["total"], if many { 21 } else { 1 });
                Ok(q.n.get())
            })
            .unwrap();
        counts.push(count);
    }
    assert_eq!(
        counts[0], counts[1],
        "register metadata/splits must not add a query per displayed row"
    );
}
#[test]
fn interest_eligibility_follows_the_section_side_of_a_reclassified_unused_designated_account() {
    let mut f = Fx::new();
    let mut edit = AccountInput::new("100", "Bank").kind("EXPENSE");
    edit.account = Some(f.bank);
    services::save_account(&mut f.book, f.u, f.c, edit).unwrap();
    let r=begin(&mut f.book,f.u,f.c,&f.bank.to_db(),&json!({"request_key":new_id(),"statement_date":"2026-10-31","ending_balance":"1.00","interest":"1.00","interest_account_id":f.income.to_db()})).unwrap();
    f.finish(&r);
    assert!(f.book.check().unwrap().is_empty());
    assert!(Book::open(&f.path, &[87; 32]).unwrap().check().unwrap().is_empty());
}
#[test]
fn permanent_actor_reference_protects_an_operator_even_without_other_references() {
    let mut f = Fx::new();
    let actor = users::create_user(&mut f.book, "actor-only-reference", false).unwrap();
    let r = f.begin("0.00");
    f.book.write(|tx|->FbResult<()>{
        tx.execute("UPDATE banking_reconciliation SET status='DONE',revision=2,data=json_set(data,'$.status','DONE','$.revision',2,'$.beginning_balance','0.00','$.cleared_debit','0.00','$.cleared_credit','0.00','$.cleared_count',0,'$.completion_key',?1,'$.completed_by','actor-only-reference') WHERE id=?2",params![new_id(),s(&r,"id")?])?;
        tx.execute("INSERT INTO banking_reconciliationactor VALUES(?1,'COMPLETE',?2)",params![s(&r,"id")?,actor.to_db()])?;
        seal(tx,"reconciliation",&format!("{}:2",s(&r,"id")?))?;
        Ok(())
    }).unwrap();
    let before = f.snap();
    let result: FbResult<()> = f.book.write(|tx| {
        tx.execute("DELETE FROM fb_user WHERE id=?1", [actor.to_db()])?;
        Ok(())
    });
    assert_eq!(
        result.unwrap_err().constraint(),
        Some("banking_parent_kept")
    );
    assert_eq!(f.snap(), before);
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn completed_operator_identity_survives_rename_upgrade_and_undo_of_an_empty_statement() {
    let mut f = Fx::new();
    let finisher = users::create_user(&mut f.book, "statement-finisher", true).unwrap();
    users::add_member(&mut f.book, f.c, finisher).unwrap();
    let r = f.begin("0.00");
    let r = reconcile_now(
        &mut f.book,
        finisher,
        f.c,
        s(&r, "id").unwrap(),
        &f.bank.to_db(),
        Some(1),
        &json!(new_id()),
    )
    .unwrap();
    f.book
        .write(|tx| -> FbResult<()> {
            tx.execute(
                "UPDATE fb_user SET username='renamed-finisher' WHERE id=?1",
                [finisher.to_db()],
            )?;
            Ok(())
        })
        .unwrap();
    let r = f
        .book
        .read(|q| reconciliation(q, f.c, s(&r, "id").unwrap(), &f.bank.to_db()))
        .unwrap();
    assert_eq!(r["completed_by"], "renamed-finisher");
    let before = f.snap();
    let result: FbResult<()> = f.book.write(|tx| {
        tx.execute("DELETE FROM fb_user WHERE id=?1", [finisher.to_db()])?;
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(f.snap(), before);
    let path=f.path.with_extension("actors-upgrade.fbbook");
    let copy=fb_db::test_support::populated_copy(f.book.connection(),&path,&[87;32],"0053_banking_actors",false).unwrap();
    f.path = path.clone();
    f.book=Book::open(&f.path,&[87;32]).unwrap();
    let reopened = Book::open(&f.path, &[87;32]).unwrap();
    assert_eq!(fb_db::test_support::snapshot(reopened.connection()).unwrap(), copy.expected);
    drop(reopened);
    assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(),copy.expected,"actor backfill must preserve every row, using the audit identity after rename");
    let mut expected = before.clone();
    let desktop = &mut expected.iter_mut().find(|(table, _)| table == "desktop_user").unwrap().1;
    assert!(desktop.is_empty(), "the source fixture has no configured desktop identities");
    *desktop = copy.expected["desktop_user"].clone();
    assert_eq!(f.snap(), expected);
    let r = undo_last(
        &mut f.book,
        f.u,
        f.c,
        s(&r, "id").unwrap(),
        &f.bank.to_db(),
        Some(2),
        &json!("Synthetic correction"),
        &json!(new_id()),
    )
    .unwrap();
    assert_eq!(r["completed_by"], "renamed-finisher");
    let actor:String=f.book.connection().query_row("SELECT user_id FROM banking_reconciliationactor WHERE reconciliation_id=?1 AND phase='COMPLETE'",[s(&r,"id").unwrap()],|r|r.get(0)).unwrap();
    assert_eq!(actor, finisher.to_db());
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn populated_banking_upgrade_preserves_history_and_refuses_tampered_prior_schema() {
    let mut f = history();
    let p = json!({"request_key":new_id(),"statement_date":"2026-10-31","ending_balance":"3.00","service_charge":"2.00","service_account_id":f.expense.to_db()});
    let r = begin(&mut f.book, f.u, f.c, &f.bank.to_db(), &p).unwrap();
    f.finish(&r);
    let path=f.path.with_extension("0050-upgrade.fbbook");
    let copy=fb_db::test_support::populated_copy(f.book.connection(),&path,&[87;32],"0051_banking_integrity",false).unwrap();
    for _ in 0..2 {
        let upgraded=Book::open(&path,&[87;32]).unwrap();
        assert_eq!(fb_db::test_support::snapshot(upgraded.connection()).unwrap(),copy.expected);
        assert!(upgraded.check().unwrap().is_empty());
    }
    let bad=f.path.with_extension("0050-tampered.fbbook");
    fb_db::test_support::populated_copy(f.book.connection(),&bad,&[87;32],"0051_banking_integrity",false).unwrap();
    let q=fb_db::rusqlite::Connection::open(&bad).unwrap();
    q.execute_batch("PRAGMA key=\"x'5757575757575757575757575757575757575757575757575757575757575757'\";DROP TRIGGER fb_banking_status_update;CREATE TRIGGER fb_banking_status_update BEFORE UPDATE ON banking_banklinestatus BEGIN SELECT 1; END;").unwrap();
    assert!(Book::open(&bad,&[87;32]).is_err());
}

#[test]
fn populated_pre_banking_copy_upgrades_without_changing_a_row() {
    let mut f = Fx::new();
    f.sale("12.25");
    let line = f.journal("2026-10-03", true, "5");
    let row = f.book.read(|q| cleared::line(q, f.c, &line)).unwrap();
    services::reverse_journal(
        &mut f.book,
        f.u,
        f.c,
        row["journal_id"].as_str().unwrap().parse().unwrap(),
        "2026-10-04",
        "Synthetic correction",
    )
    .unwrap();
    let path=f.path.with_extension("pre-banking-upgrade.fbbook");
    let copy=fb_db::test_support::populated_copy(f.book.connection(),&path,&[87;32],"0050_banking",false).unwrap();
    let upgraded=Book::open(&path,&[87;32]).unwrap();
    assert_eq!(fb_db::test_support::snapshot(upgraded.connection()).unwrap(),copy.expected);
    assert!(upgraded.check().unwrap().is_empty());
}
#[test]
fn every_mutation_rolls_back_after_late_audit_failure() {
    let mut f = Fx::new();
    let a = f.sale("12.25");
    let p = f.payload(vec![a["id"].clone()]);
    let review = review_deposit(&mut f.book, f.u, f.c, &p).unwrap();
    for action in ["deposit.recorded", "reconciliation.started"] {
        f.book.connection().execute_batch(&format!("CREATE TRIGGER banking_fail_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='{action}' BEGIN SELECT RAISE(ABORT,'injected late audit'); END;")).unwrap();
        let before = f.snap();
        let result = if action == "deposit.recorded" {
            record_deposit(&mut f.book, f.u, f.c, &p, &review)
        } else {
            begin(
                &mut f.book,
                f.u,
                f.c,
                &f.bank.to_db(),
                &json!({"request_key":new_id(),"statement_date":"2026-10-31","ending_balance":"-2.00","service_charge":"2.00","service_account_id":f.expense.to_db()}),
            )
        };
        assert!(result.is_err());
        assert_eq!(f.snap(), before);
        f.book
            .connection()
            .execute_batch("DROP TRIGGER banking_fail_audit")
            .unwrap();
    }
    let d = f.record(&p);
    f.book.connection().execute_batch("CREATE TRIGGER banking_fail_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='deposit.reversed' BEGIN SELECT RAISE(ABORT,'injected late audit'); END;").unwrap();
    let before = f.snap();
    assert!(
        reverse_deposit(
            &mut f.book,
            f.u,
            f.c,
            s(&d, "id").unwrap(),
            Some(1),
            &json!("2026-10-03"),
            &json!("Correction")
        )
        .is_err()
    );
    assert_eq!(f.snap(), before);
    f.book
        .connection()
        .execute_batch("DROP TRIGGER banking_fail_audit")
        .unwrap();
    let lid = f
        .book
        .connection()
        .query_row(
            "SELECT id FROM ledger_journalline WHERE journal_id=?1 AND account_id=?2",
            params![s(&d, "journal_id").unwrap(), f.bank.to_db()],
            |r| r.get::<_, String>(0),
        )
        .unwrap();
    let r = f.begin("12.25");
    f.book.connection().execute_batch("CREATE TRIGGER banking_fail_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='bank_line.cleared' BEGIN SELECT RAISE(ABORT,'injected late audit'); END;").unwrap();
    let before = f.snap();
    assert!(set_cleared(&mut f.book, f.u, f.c, &lid, "C", Some(0), None).is_err());
    assert_eq!(f.snap(), before);
    let result: FbResult<()> = f.book.write(|tx| {
        assert!(set_cleared_tx(tx, f.u, f.c, &lid, "C", Some(0), None).is_err());
        Ok(())
    });
    result.unwrap();
    assert_eq!(f.snap(), before);
    assert!(
        save_ticks(
            &mut f.book,
            f.u,
            f.c,
            s(&r, "id").unwrap(),
            &f.bank.to_db(),
            &json!({"expected_revision":1,"shown":{lid.clone():0},"ticked":[lid.clone()]})
        )
        .is_err()
    );
    assert_eq!(f.snap(), before);
    f.book
        .connection()
        .execute_batch("DROP TRIGGER banking_fail_audit")
        .unwrap();
    f.clear(&lid);
    for action in ["reconciliation.changed", "reconciliation.completed"] {
        f.book.connection().execute_batch(&format!("CREATE TRIGGER banking_fail_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='{action}' BEGIN SELECT RAISE(ABORT,'injected late audit'); END;")).unwrap();
        let before = f.snap();
        let result = if action == "reconciliation.changed" {
            begin(
                &mut f.book,
                f.u,
                f.c,
                &f.bank.to_db(),
                &json!({"request_key":new_id(),"expected_revision":1,"statement_date":"2026-10-31","ending_balance":"12.25"}),
            )
        } else {
            reconcile_now(
                &mut f.book,
                f.u,
                f.c,
                s(&r, "id").unwrap(),
                &f.bank.to_db(),
                Some(1),
                &json!(new_id()),
            )
        };
        assert!(result.is_err());
        assert_eq!(f.snap(), before);
        f.book
            .connection()
            .execute_batch("DROP TRIGGER banking_fail_audit")
            .unwrap();
    }
    let r = f.finish(&r);
    f.book.connection().execute_batch("CREATE TRIGGER banking_fail_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='reconciliation.undone' BEGIN SELECT RAISE(ABORT,'injected late audit'); END;").unwrap();
    let before = f.snap();
    assert!(
        undo_last(
            &mut f.book,
            f.u,
            f.c,
            s(&r, "id").unwrap(),
            &f.bank.to_db(),
            r["revision"].as_i64(),
            &json!("Correction"),
            &json!(new_id())
        )
        .is_err()
    );
    assert_eq!(f.snap(), before);
    f.book
        .connection()
        .execute_batch("DROP TRIGGER banking_fail_audit")
        .unwrap();
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn banking_schema_tampering_is_rejected_on_check_and_reopen() {
    let f = history();
    f.book.connection().execute_batch("DROP TRIGGER fb_banking_status_update;CREATE TRIGGER fb_banking_status_update BEFORE UPDATE ON banking_banklinestatus BEGIN SELECT 1; END;").unwrap();
    assert!(!f.book.check().unwrap().is_empty());
    assert!(Book::open(&f.path, &[87; 32]).is_err());
}
#[test]
fn bank_commands_reject_cross_company_and_missing_parents_without_changes() {
    let mut f = Fx::new();
    let other = services::create_company(&mut f.book, f.u, "Other synthetic", "USD", false)
        .unwrap()
        .id;
    let a = f.sale("5");
    let p = f.payload(vec![a["id"].clone()]);
    let before = f.snap();
    assert!(review_deposit(&mut f.book, f.u, other, &p).is_err());
    assert_eq!(f.snap(), before);
    let bad: FbResult<()> = f.book.write(|tx| {
        tx.execute(
            "INSERT INTO banking_banklinestatus VALUES(?1,?2,?3,?4,'C',1,?5,NULL)",
            params![new_id(), f.c.to_db(), new_id(), f.bank.to_db(), f.u.to_db()],
        )?;
        Ok(())
    });
    assert!(bad.is_err());
    assert_eq!(f.snap(), before);
}


#[test]
fn review_migration_preserves_populated_0053_books() {
    let mut f = history();
    let path=f.path.with_extension("0054_banking_review-upgrade.fbbook");
    let copy=fb_db::test_support::populated_copy(f.book.connection(),&path,&[87;32],"0054_banking_review",false).unwrap();
    f.path = path.clone();
    f.book=Book::open(&f.path,&[87;32]).unwrap();
    let reopened = Book::open(&f.path, &[87;32]).unwrap();
    assert_eq!(fb_db::test_support::snapshot(reopened.connection()).unwrap(), copy.expected);
    drop(reopened);
    assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(),copy.expected);
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn fix2_unique_json_migration_preserves_populated_0054_books() {
    let mut f = history();
    let path=f.path.with_extension("0055_banking_unique_json-upgrade.fbbook");
    let copy=fb_db::test_support::populated_copy(f.book.connection(),&path,&[87;32],"0055_banking_unique_json",false).unwrap();
    f.path = path.clone();
    f.book=Book::open(&f.path,&[87;32]).unwrap();
    let reopened = Book::open(&f.path, &[87;32]).unwrap();
    assert_eq!(fb_db::test_support::snapshot(reopened.connection()).unwrap(), copy.expected);
    drop(reopened);
    assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(),copy.expected);
    assert!(f.book.check().unwrap().is_empty());
}
