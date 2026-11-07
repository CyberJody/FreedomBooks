use super::*;

// Simulate a damaged stored row while restoring every schema definition exactly.
// Product callbacks cannot drop guards; this uses the test-support raw connection.
fn corrupt(f: &Fx, table: &str, sql: &str, key: &str) {
    let q = f.book.connection();
    let triggers = q
        .prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger' AND tbl_name=?1")
        .unwrap()
        .query_map([table], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    q.execute_batch("BEGIN IMMEDIATE;PRAGMA ignore_check_constraints=ON")
        .unwrap();
    for (name, _) in &triggers {
        q.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
    }
    q.execute(sql, [key]).unwrap();
    for (_, sql) in &triggers {
        q.execute_batch(sql).unwrap();
    }
    q.execute_batch("PRAGMA ignore_check_constraints=OFF;COMMIT")
        .unwrap();
}

#[test]
fn malformed_deposit_and_reversal_fields_fail_typed_reads_check_and_reopen() {
    for (table, kind, field, bad) in [
        (
            "banking_deposit",
            "deposit",
            "company_snapshot",
            "json('[]')",
        ),
        (
            "banking_deposit",
            "deposit",
            "company_snapshot",
            "json_object('id','other','name','test','currency','USD')",
        ),
        ("banking_deposit", "deposit", "memo", "NULL"),
        ("banking_deposit", "deposit", "reference", "12"),
        ("banking_deposit", "deposit", "actor_name", "json_object()"),
        ("banking_depositreversal", "reversal", "reason", "42"),
        ("banking_depositreversal", "reversal", "actor_name", "NULL"),
    ] {
        let f = history();
        let key: String = f
            .book
            .connection()
            .query_row(&format!("SELECT id FROM {table}"), [], |r| r.get(0))
            .unwrap();
        corrupt(
            &f,
            table,
            &format!("UPDATE {table} SET data=json_set(data,'$.{field}',{bad}) WHERE id=?1"),
            &key,
        );
        assert!(
            values(
                f.book.connection(),
                kind,
                &format!("SELECT data FROM {table} WHERE id=?1"),
                &key
            )
            .is_err(),
            "{table}.{field}"
        );
        assert!(!f.book.check().unwrap().is_empty(), "{table}.{field}");
        assert!(Book::open(&f.path, &[87; 32]).is_err(), "{table}.{field}");
    }
}

#[test]
fn reconciliation_corruption_is_rejected_independently_on_open_and_read() {
    // Labels of the open-time violation queries each corruption must trip (sorted). corrupt() drops
    // every trigger, so the impossible date is stored; then 0130's fb_value_invalid view reports it
    // (dual variant (a)) AND banking's own evidence view still reports it (dual variant (b)): open and
    // check list every violation, so neither masks the other.
    const SHARED: &str = "invalid stored text, identifier, date or timestamp";
    const EVIDENCE: &str = "invalid banking payloads, dates or keys"; // banking_evidence_invalid, banking's own check
    for (change, expected) in [
        (
            "hide_later=0,data=json_set(data,'$.hide_later',0)",
            &[EVIDENCE][..],
        ),
        (
            "statement_date='2026-99-99',data=json_set(data,'$.statement_date','2026-99-99')",
            &[EVIDENCE, SHARED][..],
        ),
        (
            "data=json_set(data,'$.completion_key','zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz')",
            &[EVIDENCE][..],
        ),
        (
            "data=json_set(data,'$.request_key','zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz')",
            &[EVIDENCE][..],
        ),
        (
            "data=json_set(data,'$.cleared_count',json('true'))",
            &[EVIDENCE, "invalid banking reconciliation"][..],
        ),
        (
            "data=json_remove(data,'$.interest_account_id')",
            &[EVIDENCE][..],
        ),
    ] {
        let expected: &[&str] = expected;
        let mut f = Fx::new();
        let r = f.begin("0.00");
        let r = f.finish(&r);
        let key = s(&r, "id").unwrap();
        corrupt(
            &f,
            "banking_reconciliation",
            &format!("UPDATE banking_reconciliation SET {change} WHERE id=?1"),
            key,
        );
        assert!(
            reconciliation(f.book.connection(), f.c, key, &f.bank.to_db()).is_err(),
            "{change}"
        );
        let labels = |problems: &[String]| {
            problems
                .iter()
                .map(|p| p.split_once(": ").map_or(p.as_str(), |(l, _)| l).to_owned())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        };
        let checked = labels(&f.book.check().unwrap());
        assert_eq!(checked, expected, "check: {change}");
        assert_eq!(
            checked.iter().any(|l| l == SHARED),
            change.contains("2026-99-99"),
            "only the impossible date is a shared value-class problem: {change}"
        );
        match Book::open(&f.path, &[87; 32]) {
            Err(fb_db::DbError::Integrity(problems)) => {
                assert_eq!(labels(&problems), expected, "open: {change}")
            }
            Err(e) => panic!("open refused for another reason: {change}: {e}"),
            Ok(_) => panic!("open accepted: {change}"),
        }
    }
}

#[test]
fn sealing_rechecks_payload_shape_without_relying_on_insert_guards() {
    let mut f = Fx::new();
    let r = f.begin("0.00");
    let key = s(&r, "id").unwrap();
    corrupt(
        &f,
        "banking_reconciliation",
        "UPDATE banking_reconciliation SET revision=2,hide_later=0,data=json_set(data,'$.revision',2,'$.hide_later',0) WHERE id=?1",
        key,
    );
    let before = f.snap();
    let result = f
        .book
        .write(|tx| seal(tx, "reconciliation", &format!("{key}:2")));
    assert!(result.is_err());
    assert_eq!(f.snap(), before);
}

#[test]
fn historical_interest_and_service_survive_account_deactivation() {
    for interest in [false, true] {
        let mut f = Fx::new();
        let other = if interest { f.income } else { f.expense };
        let p = if interest {
            json!({"request_key":new_id(),"statement_date":"2026-10-31","ending_balance":"1.00","interest":"1.00","interest_account_id":other.to_db()})
        } else {
            json!({"request_key":new_id(),"statement_date":"2026-10-31","ending_balance":"-1.00","service_charge":"1.00","service_account_id":other.to_db()})
        };
        let r = begin(&mut f.book, f.u, f.c, &f.bank.to_db(), &p).unwrap();
        let kind = if interest { "INCOME" } else { "EXPENSE" };
        let mut edit =
            AccountInput::new(if interest { "400" } else { "500" }, "Historical charge").kind(kind);
        edit.account = Some(other);
        edit.active = false;
        edit.postable = false;
        services::save_account(&mut f.book, f.u, f.c, edit).unwrap();
        let r = save_ticks(
            &mut f.book,
            f.u,
            f.c,
            s(&r, "id").unwrap(),
            &f.bank.to_db(),
            &json!({"expected_revision":1,"shown":{},"ticked":[],"hide_later":false}),
        )
        .unwrap();
        f.finish(&r);
        assert!(f.book.check().unwrap().is_empty());
        assert!(Book::open(&f.path, &[87; 32]).is_ok());
    }
}

#[test]
fn new_charge_references_require_current_account_and_bank_eligibility() {
    for interest in [false, true] {
        for condition in [
            "inactive",
            "nonpostable",
            "inactive_bank",
            "undesignated_bank",
        ] {
            let mut f = Fx::new();
            let line = f.journal("2026-10-31", interest, "1.00");
            let journal: String = f
                .book
                .connection()
                .query_row(
                    "SELECT journal_id FROM ledger_journalline WHERE id=?1",
                    [&line],
                    |r| r.get(0),
                )
                .unwrap();
            let r = f.begin("0.00");
            let other = if interest { f.income } else { f.expense };
            if condition == "undesignated_bank" {
                documents::save_setup(
                    &mut f.book,
                    f.u,
                    f.c,
                    &json!({"undeposited":f.uf.to_db(),"banks":[]}),
                    Some(1),
                )
                .unwrap();
            } else {
                let (account, code, kind) = if condition == "inactive_bank" {
                    (f.bank, "100", "ASSET")
                } else if interest {
                    (other, "400", "INCOME")
                } else {
                    (other, "500", "EXPENSE")
                };
                let mut edit = AccountInput::new(code, "Eligibility test").kind(kind);
                edit.account = Some(account);
                edit.active = condition == "nonpostable";
                edit.postable = condition != "nonpostable";
                services::save_account(&mut f.book, f.u, f.c, edit).unwrap();
            }
            let before = f.snap();
            let prefix = if interest { "interest" } else { "service" };
            let amount = if interest {
                "interest"
            } else {
                "service_charge"
            };
            let result: FbResult<()> = f.book.write(|tx| {
                tx.execute(&format!("UPDATE banking_reconciliation SET revision=2,data=json_set(data,'$.revision',2,'$.{amount}','1.00','$.{prefix}_account_id',?1,'$.{prefix}_journal_id',?2) WHERE id=?3"),params![other.to_db(),journal,s(&r,"id")?])?;
                Ok(())
            });
            assert!(result.is_err(), "{prefix}: {condition}");
            assert_eq!(before, f.snap(), "{prefix}: {condition}");
        }
    }
}

#[test]
fn every_register_preset_uses_fixed_clock_and_explicit_dates_validate_before_override() {
    let mut f = Fx::new();
    for d in ["2026-01-01", "2026-09-01", "2026-10-01", "2026-11-01"] {
        f.journal(d, true, "1.00");
    }
    for (preset, expected) in [
        ("this_month", 1),
        ("last_month", 1),
        ("this_quarter", 2),
        ("this_year", 4),
        ("year_to_date", 3),
        ("", 4),
        ("custom", 4),
    ] {
        let v = build_on(
            &mut f.book,
            f.u,
            f.c,
            &f.bank.to_db(),
            &json!({"preset":preset}),
            1,
            50,
            "2026-10-30",
        )
        .unwrap();
        assert_eq!(v["total"], expected, "{preset}");
    }
    for bad in [
        json!({"preset":"this_month","start":"invalid"}),
        json!({"amount":"0"}),
        json!({"amount":"-1"}),
        json!({"amount":"1.001"}),
        json!({"output":"invalid"}),
    ] {
        assert!(
            build_on(
                &mut f.book,
                f.u,
                f.c,
                &f.bank.to_db(),
                &bad,
                1,
                50,
                "2026-10-30"
            )
            .is_err(),
            "{bad}"
        );
    }
    for raw in [
        "2026-+1-01",
        "2026-01-+1",
        "+026-01-01",
        "2026-99-99",
        "2026-02-29",
    ] {
        assert!(date(&json!(raw)).is_err(), "{raw}");
    }
    assert_eq!(date(&json!("20260101")).unwrap(), "2026-01-01");
}

#[test]
fn sql_predicates_are_strict_and_safe_inside_both_callback_types() {
    let mut f = Fx::new();
    fn probe(q: &(impl Query + ?Sized)) -> FbResult<()> {
        for d in ["2026-99-99", "2026-02-29", "2026-2-01", "2026-01-+1"] {
            let good: bool = q.query_row("SELECT fb_banking_date(?1)", [d], |r| r.get(0))?;
            assert!(!good, "{d}");
        }
        assert!(
            q.query_row("SELECT fb_banking_date('2024-02-29')", [], |r| r
                .get::<_, bool>(0))?
        );
        assert!(!q.query_row("SELECT fb_banking_date(NULL)", [], |r| r.get::<_, bool>(0))?);
        assert!(!q.query_row("SELECT fb_banking_uuid(NULL)", [], |r| r.get::<_, bool>(0))?);
        assert!(
            !q.query_row("SELECT fb_banking_payload('deposit','[]')", [], |r| r
                .get::<_, bool>(0))?
        );
        Ok(())
    }
    f.book.read(|q| probe(q)).unwrap();
    f.book.write(|q| probe(q)).unwrap();
}

#[test]
fn fix2_duplicate_json_keys_cannot_write_or_seal() {
    let mut f = Fx::new();
    let late = f.journal("2026-11-01", true, "10.00");
    f.clear(&late);
    let mut r = f.begin("10.00");
    let key = s(&r, "id").unwrap().to_owned();
    r["revision"] = json!(2);
    let raw = r.to_string().replace(
        "\"hide_later\":true",
        "\"hide_later\":true,\"hide_later\":false",
    );
    assert!(raw.contains("\"hide_later\":true,\"hide_later\":false"));
    let before = f.snap();
    let result: FbResult<()> = f.book.write(|tx| {
        tx.execute(
            "UPDATE banking_reconciliation SET revision=2,data=?1 WHERE id=?2",
            params![raw, key],
        )?;
        seal(tx, "reconciliation", &format!("{key}:2"))?;
        Ok(())
    });
    assert!(
        result.is_err(),
        "Duplicate keys must fail before SQL/Rust values diverge"
    );
    assert_eq!(f.snap(), before);
}

#[test]
fn fix2_duplicate_json_keys_fail_read_seal_check_and_open() {
    for extra in [
        r#", "hide_later":false"#,
        r#", "nested":{"x":1,"x":2}"#,
        r#", "nested":[{"x":1,"\u0078":1}]"#,
    ] {
        let mut f = Fx::new();
        let mut r = f.begin("0.00");
        let key = s(&r, "id").unwrap().to_owned();
        r["revision"] = json!(2);
        let raw = r.to_string();
        let raw = format!("{}{extra}}}", &raw[..raw.len() - 1]);
        let sql = format!(
            "UPDATE banking_reconciliation SET revision=2,data='{}' WHERE id=?1",
            raw.replace('\'', "''")
        );
        corrupt(&f, "banking_reconciliation", &sql, &key);
        assert!(
            reconciliation(f.book.connection(), f.c, &key, &f.bank.to_db()).is_err(),
            "read: {extra}"
        );
        assert!(!f.book.check().unwrap().is_empty(), "check: {extra}");
        let result = f
            .book
            .write(|tx| seal(tx, "reconciliation", &format!("{key}:2")));
        assert!(result.is_err(), "seal: {extra}");
        assert!(Book::open(&f.path, &[87; 32]).is_err(), "open: {extra}");
    }
}

#[test]
fn fix2_register_payee_nul() {
    let mut f = Fx::new();
    let result = build_on(
        &mut f.book,
        f.u,
        f.c,
        &f.bank.to_db(),
        &json!({"payee": "\0"}),
        1,
        50,
        "2026-10-30",
    );
    assert!(result.is_err(), "{result:?}");
}

#[test]
fn fix2_register_number_nul() {
    let mut f = Fx::new();
    let result = build_on(
        &mut f.book,
        f.u,
        f.c,
        &f.bank.to_db(),
        &json!({"number": "\0"}),
        1,
        50,
        "2026-10-30",
    );
    assert!(result.is_err(), "{result:?}");
}

#[test]
fn fix2_register_amount_array() {
    let mut f = Fx::new();
    let result = build_on(
        &mut f.book,
        f.u,
        f.c,
        &f.bank.to_db(),
        &json!({"amount": []}),
        1,
        50,
        "2026-10-30",
    );
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn fix2_register_amount_object() {
    let mut f = Fx::new();
    let result = build_on(
        &mut f.book,
        f.u,
        f.c,
        &f.bank.to_db(),
        &json!({"amount": {}}),
        1,
        50,
        "2026-10-30",
    );
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn fix2_register_start_array() {
    let mut f = Fx::new();
    let result = build_on(
        &mut f.book,
        f.u,
        f.c,
        &f.bank.to_db(),
        &json!({"start": []}),
        1,
        50,
        "2026-10-30",
    );
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn fix2_register_start_object() {
    let mut f = Fx::new();
    let result = build_on(
        &mut f.book,
        f.u,
        f.c,
        &f.bank.to_db(),
        &json!({"start": {}}),
        1,
        50,
        "2026-10-30",
    );
    assert!(result.is_ok(), "{result:?}");
}
