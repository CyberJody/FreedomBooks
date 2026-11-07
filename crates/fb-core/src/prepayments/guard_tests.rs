use super::tests::Fixture;
use super::*;
use fb_db::rusqlite::{Connection, params_from_iter};
use std::collections::BTreeSet;

// Attacker's re-keyed primary key: canonical, so 0130 passes and the table's own guard decides.
const FRESH: &str = "00000000-0000-7000-8000-0000000e0001";

/// The guard that refused `result`; 0130's `fb_valid` never counts.
fn refused(result: Result<usize, fb_db::rusqlite::Error>, what: &str) -> String {
    let e = result.expect_err(what);
    let constraint = fb_db::constraint_of(&e).unwrap_or_else(|| e.to_string());
    assert_ne!(constraint, "fb_valid", "masked by fb_valid: {what}: {e}");
    constraint
}

fn snapshot(q: &impl Query) -> Value {
    let mut st=q.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT GLOB 'sqlite_*' ORDER BY name").unwrap();
    let tables = st
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mut out = json!({});
    for t in tables {
        let mut st = q.prepare(&format!("SELECT * FROM {t}")).unwrap();
        let n = st.column_count();
        let mut rows = st
            .query_map([], |r| {
                (0..n)
                    .map(|i| r.get::<_, SqlValue>(i))
                    .collect::<Result<Vec<_>, _>>()
            })
            .unwrap()
            .map(|row| format!("{:?}", row.unwrap()))
            .collect::<Vec<_>>();
        rows.sort();
        out[t] = json!(rows);
    }
    out
}
fn populated() -> Fixture {
    let mut f = Fixture::new();
    let r = f.receive();
    let i = f.invoice();
    let a = f.apply(&r, &i, "100.00");
    unapply_deposit(
        &mut f.book,
        f.actor,
        f.c,
        s(&a, "id"),
        Some(1),
        &json!("2026-10-05"),
        &json!("undo"),
    )
    .unwrap();
    let refund = f.refund(&r);
    void_refund(
        &mut f.book,
        f.actor,
        f.c,
        s(&refund, "id"),
        Some(1),
        &json!("2026-10-05"),
        &json!("undo"),
    )
    .unwrap();
    f
}

/// Every attack in the matrix below and the guard that must refuse it (sorted).
const EXPECTED_MATRIX_GUARDS: &[&str] = &[
    "prepayments_customerdeposit delete: customer_deposit_immutable",
    "prepayments_customerdeposit replace keeping id: customer_deposit_scope",
    "prepayments_customerdeposit replace keeping receipt_id: customer_deposit_scope",
    "prepayments_customerdeposit update id: customer_deposit_immutable",
    "prepayments_customerdeposit update or replace id: customer_deposit_immutable",
    "prepayments_customerdeposit upsert: customer_deposit_scope",
    "prepayments_customerdeposit view rekey: customer_deposit_immutable",
    "prepayments_depositapplication delete: prepayment_parent_retained",
    "prepayments_depositapplication replace keeping id: prepayment_fields",
    "prepayments_depositapplication replace keeping identity_id: prepayment_fields",
    "prepayments_depositapplication replace keeping journal_id: prepayment_fields",
    "prepayments_depositapplication update id: customer_deposit_immutable",
    "prepayments_depositapplication update or replace id: customer_deposit_immutable",
    "prepayments_depositapplication upsert: customer_deposit_immutable",
    "prepayments_depositapplication view rekey: customer_deposit_immutable",
    "prepayments_depositrefund delete: prepayment_parent_retained",
    "prepayments_depositrefund replace keeping id: deposit_refund_scope",
    "prepayments_depositrefund replace keeping identity_id: deposit_refund_scope",
    "prepayments_depositrefund replace keeping journal_id: deposit_refund_scope",
    "prepayments_depositrefund update id: customer_deposit_immutable",
    "prepayments_depositrefund update or replace id: customer_deposit_immutable",
    "prepayments_depositrefund upsert: customer_deposit_immutable",
    "prepayments_depositrefund view rekey: customer_deposit_immutable",
    "prepayments_depositrefundvoid delete: customer_deposit_immutable",
    "prepayments_depositrefundvoid replace keeping id: prepayment_fields",
    "prepayments_depositrefundvoid replace keeping journal_id: prepayment_fields",
    "prepayments_depositrefundvoid replace keeping refund_id: prepayment_fields",
    "prepayments_depositrefundvoid update id: customer_deposit_immutable",
    "prepayments_depositrefundvoid update or replace id: customer_deposit_immutable",
    "prepayments_depositrefundvoid upsert: customer_deposit_immutable",
    "prepayments_depositrefundvoid view rekey: customer_deposit_immutable",
    "prepayments_depositunapplication delete: customer_deposit_immutable",
    "prepayments_depositunapplication replace keeping application_id: prepayment_fields",
    "prepayments_depositunapplication replace keeping id: prepayment_fields",
    "prepayments_depositunapplication replace keeping journal_id: prepayment_fields",
    "prepayments_depositunapplication update id: customer_deposit_immutable",
    "prepayments_depositunapplication update or replace id: customer_deposit_immutable",
    "prepayments_depositunapplication upsert: customer_deposit_immutable",
    "prepayments_depositunapplication view rekey: customer_deposit_immutable",
    "prepayments_due delete: prepayment_seal_immutable",
    "prepayments_due replace: prepayment_seal_immutable",
    "prepayments_due update kind: prepayment_seal_immutable",
    "prepayments_due update or replace doc_id: prepayment_seal_immutable",
    "prepayments_seal delete: prepayment_seal_immutable",
    "prepayments_seal replace: prepayment_seal_immutable",
    "prepayments_seal update kind: prepayment_seal_immutable",
    "prepayments_seal update or replace doc_id: prepayment_seal_immutable",
];

#[test]
fn all_new_keys_replace_upsert_rekey_delete_and_view_matrix() {
    let f = populated();
    let q = f.book.connection();
    let before = snapshot(q);
    for recursive in [0, 1] {
        q.execute_batch(&format!("PRAGMA recursive_triggers={recursive}"))
            .unwrap();
        let mut hit = BTreeSet::new();
        for (table, keys) in [
            ("prepayments_customerdeposit", vec!["id", "receipt_id"]),
            (
                "prepayments_depositapplication",
                vec!["id", "identity_id", "journal_id"],
            ),
            (
                "prepayments_depositrefund",
                vec!["id", "identity_id", "journal_id"],
            ),
            (
                "prepayments_depositunapplication",
                vec!["id", "application_id", "journal_id"],
            ),
            (
                "prepayments_depositrefundvoid",
                vec!["id", "refund_id", "journal_id"],
            ),
        ] {
            let columns = q
                .prepare(&format!("SELECT * FROM {table}"))
                .unwrap()
                .column_names()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            for key in keys {
                // Keep only the independently tested unique key; make every other unique
                // identity fresh, proving the insertion guard covers alternate victims.
                let projection = columns
                    .iter()
                    .map(|k| {
                        if k != key
                            && [
                                "id",
                                "identity_id",
                                "journal_id",
                                "receipt_id",
                                "application_id",
                                "refund_id",
                            ]
                            .contains(&k.as_str())
                        {
                            format!("'{}'", new_id())
                        } else {
                            k.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                let sql = format!(
                    "INSERT OR REPLACE INTO {table} SELECT {projection} FROM {table} LIMIT 1"
                );
                hit.insert(format!(
                    "{table} replace keeping {key}: {}",
                    refused(q.execute(&sql, []), &sql)
                ));
                assert_eq!(snapshot(q), before);
            }
            for (label, sql) in [
                ("update id", format!("UPDATE {table} SET id='{FRESH}'")),
                (
                    "update or replace id",
                    format!("UPDATE OR REPLACE {table} SET id='{FRESH}'"),
                ),
                ("delete", format!("DELETE FROM {table}")),
                (
                    "upsert",
                    format!(
                        "INSERT INTO {table} SELECT * FROM {table} WHERE true ON CONFLICT(id) DO UPDATE SET id=excluded.id"
                    ),
                ),
            ] {
                hit.insert(format!(
                    "{table} {label}: {}",
                    refused(q.execute(&sql, []), &sql)
                ));
                assert_eq!(snapshot(q), before);
            }
            q.execute_batch(&format!("CREATE TEMP VIEW pp_view AS SELECT * FROM {table};CREATE TEMP TRIGGER pp_view_update INSTEAD OF UPDATE ON pp_view BEGIN UPDATE {table} SET id=NEW.id WHERE id=OLD.id;END;")).unwrap();
            hit.insert(format!(
                "{table} view rekey: {}",
                refused(
                    q.execute(&format!("UPDATE OR REPLACE pp_view SET id='{FRESH}'"), []),
                    "view rekey"
                )
            ));
            q.execute_batch("DROP TRIGGER pp_view_update;DROP VIEW pp_view")
                .unwrap();
            assert_eq!(snapshot(q), before);
            for alias in ["rowid", "_rowid_", "oid"] {
                assert!(q.prepare(&format!("SELECT {alias} FROM {table}")).is_err());
            }
        }
        // kind/doc_id are free text to 0130 (not ids), so 'fresh' reaches the seal guards as written.
        for table in ["prepayments_seal", "prepayments_due"] {
            for (label, sql) in [
                ("update kind", format!("UPDATE {table} SET kind='fresh'")),
                (
                    "update or replace doc_id",
                    format!("UPDATE OR REPLACE {table} SET doc_id='fresh'"),
                ),
                (
                    "replace",
                    format!("INSERT OR REPLACE INTO {table} SELECT * FROM {table}"),
                ),
                ("delete", format!("DELETE FROM {table}")),
            ] {
                hit.insert(format!(
                    "{table} {label}: {}",
                    refused(q.execute(&sql, []), &sql)
                ));
                assert_eq!(snapshot(q), before);
            }
        }
        let hit = hit.iter().map(String::as_str).collect::<Vec<_>>();
        assert_eq!(
            hit, EXPECTED_MATRIX_GUARDS,
            "recursive_triggers={recursive}"
        );
    }
    q.execute_batch("PRAGMA recursive_triggers=ON").unwrap();
    assert!(f.book.check().unwrap().is_empty());
    let path = q.path().unwrap().to_string();
    drop(f);
    assert!(
        Book::open(std::path::Path::new(&path), &[72; 32])
            .unwrap()
            .check()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn every_public_mutation_rolls_back_a_late_audit_failure() {
    for action in [
        "customer_deposit.recorded",
        "customer_deposit.applied",
        "customer_deposit.unapplied",
        "customer_deposit.voided",
        "deposit_refund.recorded",
        "deposit_refund.voided",
    ] {
        let mut f = Fixture::new();
        let r = f.receive();
        let i = f.invoice();
        let a = f.apply(&r, &i, "100.00");
        let refund = f.refund(&r);
        unapply_deposit(
            &mut f.book,
            f.actor,
            f.c,
            s(&a, "id"),
            Some(1),
            &json!("2026-10-05"),
            &json!("undo"),
        )
        .unwrap();
        void_refund(
            &mut f.book,
            f.actor,
            f.c,
            s(&refund, "id"),
            Some(1),
            &json!("2026-10-05"),
            &json!("undo"),
        )
        .unwrap();
        let a = f.apply(&r, &i, "100.00");
        let refund = f.refund(&r);
        let p = f.input();
        let dr = review_deposit(&mut f.book, f.actor, f.c, &p).unwrap();
        let ap = json!({"request_key":new_id(),"invoice_id":i["id"],"date":"2026-10-03","amount":"100.00"});
        let ar = review_application(&mut f.book, f.actor, f.c, s(&r, "id"), &ap).unwrap();
        let mut rp = f.refund_input();
        rp["check_number"] = json!("other");
        let rr = review_refund(&mut f.book, f.actor, f.c, s(&r, "id"), &rp).unwrap();
        let clean = f.receive();
        let before = snapshot(f.book.connection());
        f.book.connection().execute_batch(&format!("CREATE TRIGGER pp_fail_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='{action}' BEGIN SELECT RAISE(ABORT,'late audit failure');END;")).unwrap();
        let result = match action {
            "customer_deposit.recorded" => record_deposit(&mut f.book, f.actor, f.c, Some(&dr), &p),
            "customer_deposit.applied" => {
                apply_deposit(&mut f.book, f.actor, f.c, s(&r, "id"), Some(&ar), &ap)
            }
            "customer_deposit.unapplied" => unapply_deposit(
                &mut f.book,
                f.actor,
                f.c,
                s(&a, "id"),
                Some(1),
                &json!("2026-10-06"),
                &json!("undo"),
            ),
            "customer_deposit.voided" => void_deposit(
                &mut f.book,
                f.actor,
                f.c,
                s(&clean, "id"),
                Some(1),
                &json!("2026-10-06"),
                &json!("undo"),
            ),
            "deposit_refund.recorded" => {
                record_refund(&mut f.book, f.actor, f.c, s(&r, "id"), Some(&rr), &rp)
            }
            _ => void_refund(
                &mut f.book,
                f.actor,
                f.c,
                s(&refund, "id"),
                Some(1),
                &json!("2026-10-06"),
                &json!("undo"),
            ),
        };
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("late audit failure"),
            "{action}"
        );
        assert_eq!(snapshot(f.book.connection()), before);
        f.book
            .connection()
            .execute_batch("DROP TRIGGER pp_fail_audit")
            .unwrap();
        assert!(f.book.check().unwrap().is_empty());
    }
}

#[test]
fn populated_pre_0070_copy_upgrades_preserving_receipts_and_all_existing_rows() {
    let mut f = Fixture::new();
    let inv = f.invoice();
    let p = json!({"request_key":new_id(),"customer_id":f.customer,"date":"2026-10-03","amount":"700.00","method":"CASH","destination":"BANK","bank_id":f.bank.to_db(),"allocations":[{"invoice_id":inv["id"],"amount":"700.00"}]});
    let review = sales::review_receipt(&mut f.book, f.actor, f.c, &p).unwrap();
    let r = sales::record_receipt(&mut f.book, f.actor, f.c, &p, Some(&review)).unwrap();
    sales::reverse_receipt(
        &mut f.book,
        f.actor,
        f.c,
        s(&r, "id"),
        Some(1),
        &json!("2026-10-04"),
        &json!("correction"),
    )
    .unwrap();
    let current = snapshot(f.book.connection());
    let prior = fb_db::MIGRATIONS
        .iter()
        .position(|(name, _)| *name == "0070_prepayments")
        .unwrap();
    for tamper in [false, true] {
        let path = std::env::temp_dir().join(format!("pp-upgrade-{}.fbbook", new_id()));
        let q = Connection::open(&path).unwrap();
        q.execute_batch(
            "PRAGMA key=\"x'4848484848484848484848484848484848484848484848484848484848484848'\";",
        )
        .unwrap();
        fb_db::collation::register(&q).unwrap();
        for (_, sql) in &fb_db::MIGRATIONS[..prior] {
            q.execute_batch(sql).unwrap();
        }
        let triggers = q
            .prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger'")
            .unwrap()
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        q.execute_batch("BEGIN IMMEDIATE").unwrap();
        for (name, _) in &triggers {
            q.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
        }
        let tables = q
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT GLOB 'sqlite_*'",
            )
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        for t in tables {
            let mut st = f
                .book
                .connection()
                .prepare(&format!("SELECT * FROM {t}"))
                .unwrap();
            let n = st.column_count();
            for row in st
                .query_map([], |r| {
                    (0..n)
                        .map(|i| r.get::<_, SqlValue>(i))
                        .collect::<Result<Vec<_>, _>>()
                })
                .unwrap()
            {
                q.execute(
                    &format!("INSERT INTO {t} VALUES({})", vec!["?"; n].join(",")),
                    params_from_iter(row.unwrap()),
                )
                .unwrap();
            }
        }
        for (_, sql) in triggers {
            q.execute_batch(&sql).unwrap();
        }
        q.execute_batch(&format!("PRAGMA user_version={prior};COMMIT"))
            .unwrap();
        let before = snapshot(&q);
        if tamper {
            q.execute_batch("DROP TRIGGER fb_sales_receipt_insert;CREATE TRIGGER fb_sales_receipt_insert BEFORE INSERT ON sales_receipt BEGIN SELECT 1;END;").unwrap();
        }
        drop(q);
        let opened = Book::open(&path, &[72; 32]);
        if tamper {
            assert!(opened.is_err());
            continue;
        }
        let b = opened.unwrap();
        let after = snapshot(b.connection());
        for (t, rows) in before.as_object().unwrap() {
            assert_eq!(&after[t], rows, "{t}");
        }
        assert_eq!(after, current);
        assert!(b.check().unwrap().is_empty());
        drop(b);
        assert!(
            Book::open(&path, &[72; 32])
                .unwrap()
                .check()
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn competing_application_and_refund_serialize_without_overuse() {
    let mut f = Fixture::new();
    let r = f.receive();
    let i = f.invoice();
    let ap =
        json!({"request_key":new_id(),"invoice_id":i["id"],"date":"2026-10-03","amount":"400.00"});
    let ar = review_application(&mut f.book, f.actor, f.c, s(&r, "id"), &ap).unwrap();
    let mut rp = f.refund_input();
    rp["amount"] = json!("400.00");
    let rr = review_refund(&mut f.book, f.actor, f.c, s(&r, "id"), &rp).unwrap();
    let path = f.book.connection().path().unwrap().to_string();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let actor = f.actor;
    let c = f.c;
    let handles = [false, true]
        .into_iter()
        .map(|refund| {
            let path = path.clone();
            let barrier = barrier.clone();
            let r = r.clone();
            let ap = ap.clone();
            let rp = rp.clone();
            let ar = ar.clone();
            let rr = rr.clone();
            std::thread::spawn(move || {
                let mut b = Book::open(std::path::Path::new(&path), &[72; 32]).unwrap();
                barrier.wait();
                if refund {
                    record_refund(&mut b, actor, c, s(&r, "id"), Some(&rr), &rp)
                } else {
                    apply_deposit(&mut b, actor, c, s(&r, "id"), Some(&ar), &ap)
                }
            })
        })
        .collect::<Vec<_>>();
    let results = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(f.balance(&r), 10000);
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn udf_trust_boundaries_are_pure_and_registered_on_reopen() {
    let mut f = Fixture::new();
    let path = f.book.connection().path().unwrap().to_string();
    for reopen in [false, true] {
        if reopen {
            drop(f.book);
            f.book = Book::open(std::path::Path::new(&path), &[72; 32]).unwrap();
        }
        let sql = "SELECT fb_pp_within(500,100,'[200,200]'),fb_pp_within(500,101,'[200,200]'),fb_pp_within(500,1,'[9223372036854775807,1]'),fb_pp_within(500,0,'[true]')";
        let read = f
            .book
            .read(|q| {
                q.query_row(sql, [], |r| {
                    Ok((
                        r.get::<_, bool>(0)?,
                        r.get::<_, bool>(1)?,
                        r.get::<_, bool>(2)?,
                        r.get::<_, bool>(3)?,
                    ))
                })
            })
            .unwrap();
        assert_eq!(read, (true, false, false, false));
        f.book
            .write::<_, FbError>(|q| {
                assert!(q.query_row(sql, [], |r| r.get::<_, bool>(0))?);
                assert!(q.execute_batch("PRAGMA recursive_triggers=OFF").is_err());
                Ok(())
            })
            .unwrap();
        assert!(f.book.check().unwrap().is_empty());
    }
}

#[test]
fn corrupted_prepaid_evidence_is_refused_after_original_guard_restoration() {
    let mut f = Fixture::new();
    let d = f.receive();
    let i = f.invoice();
    f.apply(&d, &i, "100.00");
    let q = f.book.connection();
    let definition: String = q
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='pp_depositapplication_update'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    q.execute_batch("DROP TRIGGER pp_depositapplication_update;UPDATE prepayments_depositapplication SET data=json_set(data,'$.amount','101.00')").unwrap();
    q.execute_batch(&definition).unwrap();
    assert!(
        !f.book.check().unwrap().is_empty(),
        "open-time check must detect changed evidence, not only the posting amount column"
    );
    let path = q.path().unwrap().to_string();
    drop(f);
    assert!(Book::open(std::path::Path::new(&path), &[72; 32]).is_err());
}

fn raw_application(
    tx: &Writer<'_>,
    f: &Fixture,
    d: &Value,
    i: &Value,
    amount: &str,
    change: &str,
) -> FbResult<Value> {
    let u = services::access(tx, f.actor, f.c)?;
    let identity = documents::create_identity(tx, f.actor, f.c, "CDA", &new_id())?;
    let mapping = json!({"customer_deposits":f.liability.to_db(),"receivables":f.ar.to_db()});
    documents::snapshot_source_accounts(tx, f.actor, f.c, s(&identity, "id"), &mapping)?;
    let credit = if change == "split" { f.bank } else { f.ar };
    let j = documents::post_source(
        tx,
        f.actor,
        f.c,
        s(&identity, "id"),
        "2026-10-03",
        "raw application",
        entries(&f.liability.to_db(), &credit.to_db(), &json!(amount)),
    )?;
    let mut row = json!({"id":new_id(),"company_id":f.c.to_db(),"identity_id":identity["id"],"deposit_id":d["id"],"invoice_id":i["id"],"invoice_revision_id":i["current"]["id"],"receivables_id":f.ar.to_db(),"liability_id":f.liability.to_db(),"date":"2026-10-03","amount":amount,"journal_id":j.id.to_db(),"actor_id":f.actor.to_db(),"actor_name":u.username,"creation_digest":"a".repeat(64),"review_digest":"b".repeat(64)});
    match change {
        "parent" => row["deposit_id"] = json!(new_id()),
        "target" => row["invoice_revision_id"] = json!(new_id()),
        "fields" => row["actor_name"] = Value::Null,
        _ => {}
    }
    persist(
        tx,
        "prepayments_depositapplication",
        &row,
        &[
            "id",
            "company_id",
            "identity_id",
            "deposit_id",
            "invoice_id",
            "invoice_revision_id",
            "receivables_id",
            "liability_id",
            "date",
            "amount",
            "journal_id",
            "actor_id",
            "data",
        ],
    )?;
    seal(tx, &j.id.to_db())?;
    Ok(row)
}
#[test]
fn raw_application_guards_independently_refuse_bad_parent_target_evidence_and_split() {
    for (change, constraint) in [
        ("parent", "deposit_application_scope"),
        ("target", "deposit_application_target"),
        ("fields", "prepayment_fields"),
        ("split", "prepayment_journal_split"),
    ] {
        let mut f = Fixture::new();
        let d = f.receive();
        let i = f.invoice();
        let before = snapshot(f.book.connection());
        // A separate handle avoids borrowing the fixture while the Writer is active.
        let path = f.book.connection().path().unwrap().to_string();
        let mut b = Book::open(std::path::Path::new(&path), &[72; 32]).unwrap();
        let result = b.write(|tx| raw_application(tx, &f, &d, &i, "100.00", change));
        let e = result.unwrap_err();
        assert_eq!(e.constraint(), Some(constraint), "{change}: {e}");
        assert_eq!(snapshot(f.book.connection()), before);
        assert!(b.check().unwrap().is_empty());
    }
}
#[test]
fn sql_bounds_survive_bypassed_service_balance_checks() {
    let mut f = Fixture::new();
    let d = f.receive();
    let i = f.invoice();
    f.apply(&d, &i, "500.00");
    let path = f.book.connection().path().unwrap().to_string();
    let mut b = Book::open(std::path::Path::new(&path), &[72; 32]).unwrap();
    let before = snapshot(f.book.connection());
    let e = b
        .write(|tx| raw_application(tx, &f, &d, &i, "1.00", ""))
        .unwrap_err();
    assert_eq!(e.constraint(), Some("customer_deposit_overused"), "{e}");
    assert_eq!(snapshot(f.book.connection()), before);
    let mut p = f.input();
    p["amount"] = json!("2000.00");
    let rev = review_deposit(&mut f.book, f.actor, f.c, &p).unwrap();
    let big = record_deposit(&mut f.book, f.actor, f.c, Some(&rev), &p).unwrap();
    let before = snapshot(f.book.connection());
    let e = b
        .write(|tx| raw_application(tx, &f, &big, &i, "700.01", ""))
        .unwrap_err();
    assert_eq!(e.constraint(), Some("invoice_overcredited"), "{e}");
    assert_eq!(snapshot(f.book.connection()), before);
}

fn raw_refund(tx: &Writer<'_>, f: &Fixture, d: &Value, change: &str) -> FbResult<Value> {
    let u = services::access(tx, f.actor, f.c)?;
    let identity = documents::create_identity(tx, f.actor, f.c, "CDR", &new_id())?;
    let bank = if change == "bank" { f.ar } else { f.bank };
    let mapping = json!({"customer_deposits":f.liability.to_db(),"bank":bank.to_db()});
    documents::snapshot_source_accounts(tx, f.actor, f.c, s(&identity, "id"), &mapping)?;
    let j = documents::post_source(
        tx,
        f.actor,
        f.c,
        s(&identity, "id"),
        "2026-10-04",
        "raw refund",
        entries(&f.liability.to_db(), &bank.to_db(), &json!("100.00")),
    )?;
    let mut row = json!({"id":new_id(),"company_id":f.c.to_db(),"identity_id":identity["id"],"deposit_id":d["id"],"customer_id":f.customer,"customer_snapshot":d["customer_snapshot"],"company_snapshot":d["company_snapshot"],"bank_id":bank.to_db(),"liability_id":f.liability.to_db(),"date":"2026-10-04","amount":"100.00","method":"CHECK","check_number":"4001","memo":"","journal_id":j.id.to_db(),"actor_id":f.actor.to_db(),"actor_name":u.username,"creation_digest":"a".repeat(64),"review_digest":"b".repeat(64)});
    if change == "scope" {
        row["customer_id"] = json!(new_id());
        row["customer_snapshot"]["id"] = row["customer_id"].clone();
    }
    if change == "number_shape" {
        row["check_number"] = json!("bad\u{200b}");
    }
    persist(
        tx,
        "prepayments_depositrefund",
        &row,
        &[
            "id",
            "company_id",
            "identity_id",
            "deposit_id",
            "customer_id",
            "bank_id",
            "liability_id",
            "date",
            "amount",
            "method",
            "check_number",
            "journal_id",
            "actor_id",
            "data",
        ],
    )?;
    seal(tx, &j.id.to_db())?;
    Ok(row)
}
#[test]
fn refund_scope_bank_and_number_have_independent_sql_backstops() {
    for (change, constraint) in [
        ("scope", "deposit_refund_scope"),
        ("bank", "deposit_refund_bank"),
        ("number", "deposit_refund_number_in_use"),
        ("number_shape", "prepayment_fields"),
    ] {
        let mut f = Fixture::new();
        let d = f.receive();
        f.refund(&d);
        let before = snapshot(f.book.connection());
        let path = f.book.connection().path().unwrap().to_string();
        let mut b = Book::open(std::path::Path::new(&path), &[72; 32]).unwrap();
        let e = b.write(|tx| raw_refund(tx, &f, &d, change)).unwrap_err();
        assert_eq!(e.constraint(), Some(constraint), "{e}");
        assert_eq!(snapshot(f.book.connection()), before);
    }
}

fn raw_deposit(tx: &Writer<'_>, f: &Fixture, detail: bool) -> FbResult<()> {
    let u = services::access(tx, f.actor, f.c)?;
    let identity = documents::create_identity(tx, f.actor, f.c, "CDP", &new_id())?;
    let mapping = json!({"destination":f.bank.to_db(),"customer_deposits":f.liability.to_db()});
    documents::snapshot_source_accounts(tx, f.actor, f.c, s(&identity, "id"), &mapping)?;
    let j = documents::post_source(
        tx,
        f.actor,
        f.c,
        s(&identity, "id"),
        "2026-10-01",
        "raw deposit",
        entries(&f.bank.to_db(), &f.liability.to_db(), &json!("100.00")),
    )?;
    let row = json!({"id":new_id(),"company_id":f.c.to_db(),"identity_id":identity["id"],"kind":"CDP","customer_id":f.customer,"date":"2026-10-01","amount":"100.00","method":"CASH","reference":"","memo":"","destination":"BANK","destination_account_id":f.bank.to_db(),"customer_snapshot":party(&directory::get(tx,f.c,Model::Customer,&f.customer)?),"company_snapshot":company_snapshot(tx,f.c)?,"journal_id":j.id.to_db(),"actor_id":f.actor.to_db(),"actor_name":u.username,"creation_digest":"a".repeat(64),"review_digest":"b".repeat(64)});
    persist(
        tx,
        "sales_receipt",
        &row,
        &[
            "id",
            "company_id",
            "identity_id",
            "kind",
            "customer_id",
            "date",
            "amount",
            "destination",
            "destination_account_id",
            "journal_id",
            "data",
            "actor_id",
        ],
    )?;
    if detail {
        tx.execute(
            "INSERT INTO prepayments_customerdeposit VALUES(?,?,?,?,NULL,'')",
            params![new_id(), s(&row, "id"), f.c.to_db(), f.bank.to_db()],
        )?;
    }
    sales_seal(tx, "sales_receipt", s(&row, "id"))?;
    seal(tx, &j.id.to_db())?;
    Ok(())
}
#[test]
fn cdp_detail_scope_completeness_and_late_evidence() {
    let mut f = Fixture::new();
    let d = f.receive();
    let before = snapshot(f.book.connection());
    let path = f.book.connection().path().unwrap().to_string();
    let mut b = Book::open(std::path::Path::new(&path), &[72; 32]).unwrap();
    for (detail, name) in [
        (true, "customer_deposit_scope"),
        (false, "customer_deposit_complete"),
    ] {
        let e = b.write(|tx| raw_deposit(tx, &f, detail)).unwrap_err();
        assert_eq!(e.constraint(), Some(name), "{e}");
        assert_eq!(snapshot(f.book.connection()), before);
    }
    let i = f.invoice();
    let a = f.apply(&d, &i, "100.00");
    let e=b.write::<_,FbError>(|tx|{tx.execute("INSERT INTO documents_documentaccount(id,source_id,role,account_id,account_code,account_name,account_kind) VALUES(?,?,'extra',?,'100','100','ASSET')",params![new_id(),s(&a,"identity_id"),f.bank.to_db()])?;Ok(())}).unwrap_err();
    assert_eq!(e.constraint(), Some("deposit_late_reference"), "{e}");
}

#[test]
fn linked_undo_rows_require_real_original_journals() {
    let mut f = Fixture::new();
    let d = f.receive();
    let i = f.invoice();
    let a = f.apply(&d, &i, "100.00");
    let refund = f.refund(&d);
    let before = snapshot(f.book.connection());
    for (table, fk, parent, name) in [
        (
            "prepayments_depositunapplication",
            "application_id",
            &a,
            "deposit_unapplication_link",
        ),
        (
            "prepayments_depositrefundvoid",
            "refund_id",
            &refund,
            "deposit_refund_void_link",
        ),
    ] {
        let row = json!({"id":new_id(),fk:parent["id"],"date":"2026-10-05","reason":"bad link","journal_id":parent["journal_id"],"actor_id":f.actor.to_db(),"actor_name":"Prepayment tester"});
        let e = f
            .book
            .write(|tx| {
                persist(
                    tx,
                    table,
                    &row,
                    &["id", fk, "journal_id", "date", "actor_id", "data"],
                )
            })
            .unwrap_err();
        assert_eq!(e.constraint(), Some(name), "{e}");
        assert_eq!(snapshot(f.book.connection()), before);
    }
}

#[test]
fn raw_deposit_void_requires_live_dependencies_released_and_dates_ordered() {
    let mut f = Fixture::new();
    let d = f.receive();
    let i = f.invoice();
    let a = f.apply(&d, &i, "100.00");
    let reverse = |b: &mut Book, dt: &str| {
        b.write::<_,FbError>(|tx|{let j=documents::reverse_source(tx,f.actor,f.c,s(&d,"identity_id"),dt,"raw")?;let row=json!({"id":new_id(),"receipt_id":d["id"],"journal_id":j.id.to_db(),"date":dt,"reason":"raw","actor_id":f.actor.to_db(),"actor_name":"Prepayment tester"});persist(tx,"sales_receiptreversal",&row,&["id","receipt_id","journal_id","date","actor_id","data"])?;Ok(())})
    };
    let e = reverse(&mut f.book, "2026-10-03").unwrap_err();
    assert_eq!(e.constraint(), Some("customer_deposit_live_dependency"));
    unapply_deposit(
        &mut f.book,
        f.actor,
        f.c,
        s(&a, "id"),
        Some(1),
        &json!("2026-10-05"),
        &json!("release"),
    )
    .unwrap();
    let e = reverse(&mut f.book, "2026-10-04").unwrap_err();
    assert_eq!(e.constraint(), Some("customer_deposit_release_date"));
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn bill_payment_check_numbers_use_only_ascii_digit_references() {
    for number in ["ABC", "4001"] {
        let mut f = Fixture::new();
        let d = f.receive();
        let mut rp = f.refund_input();
        rp["check_number"] = json!(number);
        let rr = review_refund(&mut f.book, f.actor, f.c, s(&d, "id"), &rp).unwrap();
        record_refund(&mut f.book, f.actor, f.c, s(&d, "id"), Some(&rr), &rp).unwrap();
        let expense = services::save_account(
            &mut f.book,
            f.actor,
            f.c,
            crate::services::AccountInput::new("500", "Expense").kind("EXPENSE"),
        )
        .unwrap()
        .id;
        let ap = services::save_account(
            &mut f.book,
            f.actor,
            f.c,
            crate::services::AccountInput::new("200", "Payables").kind("LIABILITY"),
        )
        .unwrap()
        .id;
        documents::save_setup(&mut f.book,f.actor,f.c,&json!({"receivables":f.ar.to_db(),"undeposited":f.undeposited.to_db(),"customer_deposits":f.liability.to_db(),"payables":ap.to_db(),"banks":[f.bank.to_db()]}),Some(1)).unwrap();
        let vendor = directory::save_record(
            &mut f.book,
            f.actor,
            f.c,
            Model::Vendor,
            &json!({"display_name":"Synthetic vendor"}),
            None,
        )
        .unwrap();
        let bill=crate::payables::save_bill(&mut f.book,f.actor,f.c,&json!({"vendor_id":vendor["id"],"date":"2026-10-01","due_date":"2026-10-31","request_key":new_id(),"lines":[{"description":"Work","quantity":"1","rate":"100.00","expense_account_id":expense.to_db()}]})).unwrap();
        let review =
            crate::payables::review_bill(&mut f.book, f.actor, f.c, s(&bill, "id"), Some(1))
                .unwrap();
        let bill =
            crate::payables::post_bill(&mut f.book, f.actor, f.c, s(&bill, "id"), Some(1), &review)
                .unwrap();
        let p = json!({"vendor_id":vendor["id"],"date":"2026-10-05","amount":"100.00","method":"CHECK","reference":number,"bank_id":f.bank.to_db(),"request_key":new_id(),"allocations":[{"bill_id":bill["id"],"amount":"100.00"}]});
        let before = snapshot(f.book.connection());
        let review = crate::payables::review_payment(&mut f.book, f.actor, f.c, &p);
        if number == "ABC" {
            crate::payables::record_payment(&mut f.book,f.actor,f.c,&p,&review.unwrap()).unwrap();
        } else {
            let error=review.unwrap_err();
            assert_eq!(error.kind(),"ValidationError");
            assert!(error.message().contains("already used by deposit refund CDR-000001"));
            assert_eq!(snapshot(f.book.connection()),before);
        }
        assert!(f.book.check().unwrap().is_empty());
    }
}

#[test]
fn invoice_void_sql_guard_requires_application_release_and_release_date() {
    let mut f = Fixture::new();
    let d = f.receive();
    let i = f.invoice();
    let a = f.apply(&d, &i, "100.00");
    let result = sales::void_invoice(
        &mut f.book,
        f.actor,
        f.c,
        s(&i, "id"),
        Some(2),
        &json!("2026-10-06"),
        &json!("void"),
    );
    assert!(result.is_err());
    unapply_deposit(
        &mut f.book,
        f.actor,
        f.c,
        s(&a, "id"),
        Some(1),
        &json!("2026-10-05"),
        &json!("release"),
    )
    .unwrap();
    let result = sales::void_invoice(
        &mut f.book,
        f.actor,
        f.c,
        s(&i, "id"),
        Some(2),
        &json!("2026-10-04"),
        &json!("void"),
    );
    assert!(result.is_err());
    sales::void_invoice(
        &mut f.book,
        f.actor,
        f.c,
        s(&i, "id"),
        Some(2),
        &json!("2026-10-05"),
        &json!("void"),
    )
    .unwrap();
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn liability_and_accounting_equation_balance_at_every_lifecycle_step() {
    fn check(f: &mut Fixture, liability: &str, assets: &str, income: &str) {
        let tb = crate::reports::trial_balance(&mut f.book, f.actor, f.c, "2026-12-31").unwrap();
        assert_eq!(tb.debit, tb.credit);
        let net = |ids: &[AccountId]| {
            tb.rows
                .iter()
                .filter(|r| ids.contains(&r.account.id))
                .fold(Money::ZERO, |n, r| n + r.debit - r.credit)
        };
        let a = net(&[f.bank, f.undeposited, f.ar]);
        let l = Money::ZERO - net(&[f.liability]);
        let profit = Money::ZERO - net(&[f.income]);
        assert_eq!(a.to_string(), assets);
        assert_eq!(l.to_string(), liability);
        assert_eq!(profit.to_string(), income);
        assert_eq!(a, l + profit);
    }
    let mut f = Fixture::new();
    let d = f.receive();
    check(&mut f, "500.00", "500.00", "0.00");
    let i = f.invoice();
    check(&mut f, "500.00", "1700.00", "1200.00");
    let a = f.apply(&d, &i, "500.00");
    check(&mut f, "0.00", "1200.00", "1200.00");
    assert_eq!(
        f.book
            .read(|q| invoice_open_balance(q, f.c, s(&i, "id")))
            .unwrap(),
        70000
    );
    unapply_deposit(
        &mut f.book,
        f.actor,
        f.c,
        s(&a, "id"),
        Some(1),
        &json!("2026-10-04"),
        &json!("release"),
    )
    .unwrap();
    check(&mut f, "500.00", "1700.00", "1200.00");
    let refund = f.refund(&d);
    check(&mut f, "400.00", "1600.00", "1200.00");
    void_refund(
        &mut f.book,
        f.actor,
        f.c,
        s(&refund, "id"),
        Some(1),
        &json!("2026-10-05"),
        &json!("release"),
    )
    .unwrap();
    check(&mut f, "500.00", "1700.00", "1200.00");
    void_deposit(
        &mut f.book,
        f.actor,
        f.c,
        s(&d, "id"),
        Some(1),
        &json!("2026-10-05"),
        &json!("void"),
    )
    .unwrap();
    check(&mut f, "0.00", "1200.00", "1200.00");
}

#[test]
fn raw_invoice_void_and_owner_seals_are_independently_guarded() {
    let mut f = Fixture::new();
    let d = f.receive();
    let i = f.invoice();
    let a = f.apply(&d, &i, "100.00");
    let raw_void = |b: &mut Book, dt: &str| {
        b.write::<_,FbError>(|tx| {
  let j=documents::reverse_source(tx,f.actor,f.c,s(&i,"identity_id"),dt,"void")?;
  tx.execute("UPDATE sales_invoice SET revision=3 WHERE id=?",[s(&i,"id")])?;
  tx.execute("INSERT INTO sales_invoicerevision SELECT ?,invoice_id,3,customer_id,date,due_date,'VOID',total,journal_id,?,json_set(data,'$.state','VOID','$.action','voided','$.reason','void','$.reversal_id',?),actor_id FROM sales_invoicerevision WHERE id=?",params![new_id(),j.id.to_db(),j.id.to_db(),s(&i["current"],"id")])?;
  Ok(())
 })
    };
    let before = snapshot(f.book.connection());
    let e = raw_void(&mut f.book, "2026-10-06").unwrap_err();
    assert_eq!(e.constraint(), Some("invoice_live_deposits"), "{e}");
    assert_eq!(snapshot(f.book.connection()), before);
    unapply_deposit(
        &mut f.book,
        f.actor,
        f.c,
        s(&a, "id"),
        Some(1),
        &json!("2026-10-05"),
        &json!("release"),
    )
    .unwrap();
    let before = snapshot(f.book.connection());
    let e = raw_void(&mut f.book, "2026-10-04").unwrap_err();
    assert_eq!(e.constraint(), Some("invoice_deposit_release_date"), "{e}");
    assert_eq!(snapshot(f.book.connection()), before);
    let e = f
        .book
        .write::<_, FbError>(|tx| seal(tx, &new_id()))
        .unwrap_err();
    assert_eq!(e.constraint(), Some("prepayment_journal_owner"));
}

fn race_commands<A, B>(path: String, left: A, right: B) -> Vec<FbResult<Value>>
where
    A: FnOnce(&mut Book) -> FbResult<Value> + Send + 'static,
    B: FnOnce(&mut Book) -> FbResult<Value> + Send + 'static,
{
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let lp = path.clone();
    let lb = barrier.clone();
    let l = std::thread::spawn(move || {
        let mut b = Book::open(std::path::Path::new(&lp), &[72; 32]).unwrap();
        lb.wait();
        left(&mut b)
    });
    let r = std::thread::spawn(move || {
        let mut b = Book::open(std::path::Path::new(&path), &[72; 32]).unwrap();
        barrier.wait();
        right(&mut b)
    });
    vec![l.join().unwrap(), r.join().unwrap()]
}

#[test]
fn two_applications_and_exact_concurrent_retry_are_serializable() {
    for exact_retry in [false, true] {
        let mut f = Fixture::new();
        let d = f.receive();
        let i = f.invoice();
        let actor = f.actor;
        let c = f.c;
        let p = json!({"request_key":new_id(),"invoice_id":i["id"],"date":"2026-10-03","amount":"400.00"});
        let mut p2 = p.clone();
        if !exact_retry {
            p2["request_key"] = json!(new_id());
        }
        let review = review_application(&mut f.book, actor, c, s(&d, "id"), &p).unwrap();
        let review2 = review_application(&mut f.book, actor, c, s(&d, "id"), &p2).unwrap();
        let d2 = d.clone();
        let d1 = d.clone();
        let results = race_commands(
            f.book.connection().path().unwrap().into(),
            move |b| apply_deposit(b, actor, c, s(&d1, "id"), Some(&review), &p),
            move |b| apply_deposit(b, actor, c, s(&d2, "id"), Some(&review2), &p2),
        );
        assert_eq!(
            results.iter().filter(|r| r.is_ok()).count(),
            if exact_retry { 2 } else { 1 }
        );
        if exact_retry {
            assert_eq!(results[0].as_ref().unwrap(), results[1].as_ref().unwrap());
        }
        assert_eq!(f.balance(&d), 10000);
        assert_eq!(
            f.book
                .read::<_, FbError>(|q| Ok(q.query_row(
                    "SELECT count(*) FROM prepayments_depositapplication",
                    [],
                    |r| r.get::<_, i64>(0)
                )?))
                .unwrap(),
            1
        );
        assert!(f.book.check().unwrap().is_empty());
    }
}

#[test]
fn application_races_with_deposit_or_invoice_void_without_orphans() {
    for void_invoice in [false, true] {
        let mut f = Fixture::new();
        let d = f.receive();
        let i = f.invoice();
        let actor = f.actor;
        let c = f.c;
        let p = json!({"request_key":new_id(),"invoice_id":i["id"],"date":"2026-10-03","amount":"500.00"});
        let review = review_application(&mut f.book, actor, c, s(&d, "id"), &p).unwrap();
        let d1 = d.clone();
        let d2 = d.clone();
        let results = race_commands(
            f.book.connection().path().unwrap().into(),
            move |b| apply_deposit(b, actor, c, s(&d1, "id"), Some(&review), &p),
            move |b| {
                if void_invoice {
                    sales::void_invoice(
                        b,
                        actor,
                        c,
                        s(&i, "id"),
                        Some(2),
                        &json!("2026-10-04"),
                        &json!("race"),
                    )
                } else {
                    void_deposit(
                        b,
                        actor,
                        c,
                        s(&d2, "id"),
                        Some(1),
                        &json!("2026-10-04"),
                        &json!("race"),
                    )
                }
            },
        );
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert!(f.book.check().unwrap().is_empty());
    }
}

#[test]
fn application_and_receipt_race_cannot_oversettle_invoice() {
    let mut f = Fixture::new();
    let d = f.receive();
    let i = f.invoice();
    let actor = f.actor;
    let c = f.c;
    let p =
        json!({"request_key":new_id(),"invoice_id":i["id"],"date":"2026-10-03","amount":"500.00"});
    let review = review_application(&mut f.book, actor, c, s(&d, "id"), &p).unwrap();
    let rp = json!({"request_key":new_id(),"customer_id":f.customer,"date":"2026-10-03","amount":"1000.00","method":"CASH","destination":"BANK","bank_id":f.bank.to_db(),"allocations":[{"invoice_id":i["id"],"amount":"1000.00"}]});
    let rr = sales::review_receipt(&mut f.book, actor, c, &rp).unwrap();
    let results = race_commands(
        f.book.connection().path().unwrap().into(),
        move |b| apply_deposit(b, actor, c, s(&d, "id"), Some(&review), &p),
        move |b| sales::record_receipt(b, actor, c, &rp, Some(&rr)),
    );
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(matches!(
        f.book
            .read(|q| invoice_open_balance(q, c, s(&i, "id")))
            .unwrap(),
        20000 | 70000
    ));
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn reopened_book_detects_refund_number_collisions_after_guard_restoration() {
    let mut f = Fixture::new();
    let d = f.receive();
    f.refund(&d);
    let mut p = f.refund_input();
    p["check_number"] = json!("4002");
    let review = review_refund(&mut f.book, f.actor, f.c, s(&d, "id"), &p).unwrap();
    let r = record_refund(&mut f.book, f.actor, f.c, s(&d, "id"), Some(&review), &p).unwrap();
    let q = f.book.connection();
    let definition: String = q
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='pp_depositrefund_update'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    q.execute_batch("DROP TRIGGER pp_depositrefund_update")
        .unwrap();
    q.execute("UPDATE prepayments_depositrefund SET check_number='4001',data=json_set(data,'$.check_number','4001') WHERE id=?",[s(&r,"id")]).unwrap();
    q.execute_batch(&definition).unwrap();
    assert!(
        !f.book.check().unwrap().is_empty(),
        "restored definitions must not hide colliding live refund numbers"
    );
    let path = q.path().unwrap().to_owned();
    drop(f);
    assert!(Book::open(std::path::Path::new(&path), &[72; 32]).is_err());
}

// Independent review regressions, copied failing-first from the review worktree.
fn review_raw_deposit(
    tx: &Writer<'_>,
    f: &Fixture,
    destination: &str,
    account: AccountId,
) -> FbResult<()> {
    let u = services::access(tx, f.actor, f.c)?;
    let identity = documents::create_identity(tx, f.actor, f.c, "CDP", &new_id())?;
    let mapping = json!({"destination":account.to_db(),"customer_deposits":f.liability.to_db()});
    documents::snapshot_source_accounts(tx, f.actor, f.c, s(&identity, "id"), &mapping)?;
    let j = documents::post_source(
        tx,
        f.actor,
        f.c,
        s(&identity, "id"),
        "2026-10-01",
        "review deposit",
        entries(&account.to_db(), &f.liability.to_db(), &json!("100.00")),
    )?;
    let row = json!({"id":new_id(),"company_id":f.c.to_db(),"identity_id":identity["id"],"kind":"CDP","customer_id":f.customer,"date":"2026-10-01","amount":"100.00","method":"CASH","reference":"","memo":"","destination":destination,"destination_account_id":account.to_db(),"customer_snapshot":party(&directory::get(tx,f.c,Model::Customer,&f.customer)?),"company_snapshot":company_snapshot(tx,f.c)?,"journal_id":j.id.to_db(),"actor_id":f.actor.to_db(),"actor_name":u.username,"creation_digest":"a".repeat(64),"review_digest":"b".repeat(64)});
    persist(
        tx,
        "sales_receipt",
        &row,
        &[
            "id",
            "company_id",
            "identity_id",
            "kind",
            "customer_id",
            "date",
            "amount",
            "destination",
            "destination_account_id",
            "journal_id",
            "data",
            "actor_id",
        ],
    )?;
    tx.execute(
        "INSERT INTO prepayments_customerdeposit VALUES(?,?,?,?,NULL,'')",
        params![new_id(), s(&row, "id"), f.c.to_db(), f.liability.to_db()],
    )?;
    sales_seal(tx, "sales_receipt", s(&row, "id"))?;
    seal(tx, &j.id.to_db())?;
    Ok(())
}

#[test]
fn review_bank_deposit_sql_requires_designation() {
    let mut f = Fixture::new();
    let mut input = f.input();
    input["destination"] = json!("BANK");
    input["bank_id"] = json!(f.ar.to_db());
    assert!(review_deposit(&mut f.book, f.actor, f.c, &input).is_err());
    let path = f.book.connection().path().unwrap().to_string();
    let mut b = Book::open(std::path::Path::new(&path), &[72; 32]).unwrap();
    let result = b.write(|tx| review_raw_deposit(tx, &f, "BANK", f.ar));
    assert!(
        result.is_err(),
        "SQL admitted a CDP BANK destination in un-designated receivables; check={:?}",
        b.check()
    );
}

#[test]
fn review_undeposited_sql_requires_configured_account() {
    let f = Fixture::new();
    let path = f.book.connection().path().unwrap().to_string();
    let mut b = Book::open(std::path::Path::new(&path), &[72; 32]).unwrap();
    let result = b.write(|tx| review_raw_deposit(tx, &f, "UNDEPOSITED", f.bank));
    assert!(
        result.is_err(),
        "SQL admitted a CDP UNDEPOSITED destination in the bank; check={:?}",
        b.check()
    );
}

fn review_raw_refund(tx: &Writer<'_>, f: &Fixture, d: &Value, bad: &str) -> FbResult<Value> {
    let u = services::access(tx, f.actor, f.c)?;
    let identity = documents::create_identity(tx, f.actor, f.c, "CDR", &new_id())?;
    let mapping = json!({"customer_deposits":f.liability.to_db(),"bank":f.bank.to_db()});
    documents::snapshot_source_accounts(tx, f.actor, f.c, s(&identity, "id"), &mapping)?;
    let j = documents::post_source(
        tx,
        f.actor,
        f.c,
        s(&identity, "id"),
        "2026-10-04",
        "review refund",
        entries(&f.liability.to_db(), &f.bank.to_db(), &json!("100.00")),
    )?;
    let mut row = json!({"id":new_id(),"company_id":f.c.to_db(),"identity_id":identity["id"],"deposit_id":d["id"],"customer_id":f.customer,"customer_snapshot":d["customer_snapshot"],"company_snapshot":d["company_snapshot"],"bank_id":f.bank.to_db(),"liability_id":f.liability.to_db(),"date":"2026-10-04","amount":"100.00","method":"CHECK","check_number":"4001","memo":"","journal_id":j.id.to_db(),"actor_id":f.actor.to_db(),"actor_name":u.username,"creation_digest":"a".repeat(64),"review_digest":"b".repeat(64)});
    match bad {
        "customer_snapshot" => row["customer_snapshot"] = json!(false),
        "company_snapshot" => row["company_snapshot"] = json!([]),
        "missing_memo" => {
            row.as_object_mut().unwrap().remove("memo");
        }
        "other_actor" => {
            row["actor_id"] = d["review_actor"].clone();
        }
        _ => panic!("bad review case"),
    }
    persist(
        tx,
        "prepayments_depositrefund",
        &row,
        &[
            "id",
            "company_id",
            "identity_id",
            "deposit_id",
            "customer_id",
            "bank_id",
            "liability_id",
            "date",
            "amount",
            "method",
            "check_number",
            "journal_id",
            "actor_id",
            "data",
        ],
    )?;
    seal(tx, &j.id.to_db())?;
    Ok(row)
}

#[test]
fn review_refund_rejects_malformed_stored_snapshots() {
    for field in ["customer_snapshot", "company_snapshot", "missing_memo"] {
        let mut f = Fixture::new();
        let d = f.receive();
        let path = f.book.connection().path().unwrap().to_string();
        let mut b = Book::open(std::path::Path::new(&path), &[72; 32]).unwrap();
        let result = b.write(|tx| review_raw_refund(tx, &f, &d, field));
        let checks = b.check().unwrap();
        drop(b);
        let reopen = Book::open(std::path::Path::new(&path), &[72; 32]);
        assert!(
            result.is_err(),
            "SQL admitted malformed {field}, check={checks:?}, reopen_ok={}",
            reopen.is_ok()
        );
    }
}

#[test]
fn review_reversal_actor_matches_stored_evidence_at_insert() {
    let mut f = Fixture::new();
    let d = f.receive();
    let i = f.invoice();
    let a = f.apply(&d, &i, "100.00");
    let path = f.book.connection().path().unwrap().to_string();
    let mut b = Book::open(std::path::Path::new(&path), &[72; 32]).unwrap();
    let result=b.write::<_,FbError>(|tx| {
        let j=documents::reverse_source(tx,f.actor,f.c,s(&a,"identity_id"),"2026-10-05","review undo")?;
        let v=json!({"id":new_id(),"application_id":a["id"],"journal_id":j.id.to_db(),"date":"2026-10-05","reason":"review undo","actor_name":"Prepayment tester","actor_id":new_id()});
        tx.execute("INSERT INTO prepayments_depositunapplication(id,application_id,journal_id,date,actor_id,data) VALUES(?,?,?,?,?,?)",params![s(&v,"id"),s(&a,"id"),j.id.to_db(),"2026-10-05",f.actor.to_db(),v.to_string()])?;
        seal(tx,&j.id.to_db())?;
        Ok(())
    });
    assert!(
        result.is_err(),
        "SQL admitted mismatched reversal actor; check={:?}",
        b.check()
    );
}

#[test]
fn review_dates_reject_malformed_iso_syntax() {
    for value in [
        "2026-+1-01",
        "2026-10-+1",
        "+026-10-01",
        "2026W0-1",
        "2026W01-",
    ] {
        assert!(
            date(&json!(value)).is_err(),
            "accepted malformed ISO date {value}: {:?}",
            date(&json!(value))
        );
    }
}

#[test]
fn review_refund_actor_parent_cannot_be_deleted_and_rebound() {
    let mut f = Fixture::new();
    let other = crate::users::create_user(&mut f.book, "Historical refund actor", false).unwrap();
    let mut d = f.receive();
    d["review_actor"] = json!(other.to_db());
    let path = f.book.connection().path().unwrap().to_string();
    let mut b = Book::open(std::path::Path::new(&path), &[72; 32]).unwrap();
    let r = b
        .write(|tx| review_raw_refund(tx, &f, &d, "other_actor"))
        .unwrap();
    assert!(b.check().unwrap().is_empty());
    let q = b.connection();
    q.execute_batch("BEGIN; PRAGMA defer_foreign_keys=ON;")
        .unwrap();
    let deletion = q.execute("DELETE FROM fb_user WHERE id=?", [other.to_db()]);
    if deletion.is_ok() {
        q.execute("INSERT INTO fb_user(id,username,is_active,is_staff,is_superuser) VALUES(?,'Replacement operator',1,0,0)",[other.to_db()]).unwrap();
        q.execute_batch("COMMIT").unwrap();
    } else {
        q.execute_batch("ROLLBACK").unwrap();
    }
    assert!(
        deletion.is_err(),
        "Referenced prepayment actor was deleted and rebound for refund {}; check={:?}",
        r["id"],
        b.check()
    );
}

#[test]
fn review_refund_company_snapshot_and_required_memo_are_validated() {
    let mut accepted = vec![];
    for field in ["company_snapshot", "missing_memo"] {
        let mut f = Fixture::new();
        let d = f.receive();
        let path = f.book.connection().path().unwrap().to_string();
        let mut b = Book::open(std::path::Path::new(&path), &[72; 32]).unwrap();
        if b.write(|tx| review_raw_refund(tx, &f, &d, field)).is_ok() {
            assert!(b.check().unwrap().is_empty());
            drop(b);
            assert!(Book::open(std::path::Path::new(&path), &[72; 32]).is_ok());
            accepted.push(field);
        }
    }
    assert!(
        accepted.is_empty(),
        "malformed refund data survives check/reopen: {accepted:?}"
    );
}

#[test]
fn review_reversal_shape_requires_correct_parent_key() {
    let mut f = Fixture::new();
    let d = f.receive();
    let i = f.invoice();
    let a = f.apply(&d, &i, "100.00");
    let path = f.book.connection().path().unwrap().to_string();
    let mut b = Book::open(std::path::Path::new(&path), &[72; 32]).unwrap();
    let result=b.write::<_,FbError>(|tx| {
        let j=documents::reverse_source(tx,f.actor,f.c,s(&a,"identity_id"),"2026-10-05","review undo")?;
        // Only refund_id is present in an UNAPPLICATION snapshot.
        let v=json!({"id":new_id(),"refund_id":a["id"],"journal_id":j.id.to_db(),"date":"2026-10-05","reason":"review undo","actor_name":"Prepayment tester","actor_id":f.actor.to_db()});
        tx.execute("INSERT INTO prepayments_depositunapplication(id,application_id,journal_id,date,actor_id,data) VALUES(?,?,?,?,?,?)",params![s(&v,"id"),s(&a,"id"),j.id.to_db(),"2026-10-05",f.actor.to_db(),v.to_string()])?;
        seal(tx,&j.id.to_db())?;Ok(())
    });
    let checks = b.check().unwrap();
    drop(b);
    assert!(
        result.is_err(),
        "Unapplication with no application_id accepted; check={checks:?}, reopen_ok={}",
        Book::open(std::path::Path::new(&path), &[72; 32]).is_ok()
    );
}

#[test]
fn fix1_invoice_void_messages_match_python() {
    let mut f = Fixture::new();
    let d = f.receive();
    let i = f.invoice();
    let a = f.apply(&d, &i, "100.00");
    let number = f
        .book
        .read::<_, FbError>(|q| documents::identity(q, f.c, s(&d, "identity_id")))
        .unwrap()["number"]
        .as_str()
        .unwrap()
        .to_owned();
    let e = sales::void_invoice(
        &mut f.book,
        f.actor,
        f.c,
        s(&i, "id"),
        Some(2),
        &json!("2026-10-05"),
        &json!("void"),
    )
    .unwrap_err();
    assert_eq!(
        e.to_string(),
        format!(
            "ValidationError: Unapply customer deposit {number} from this invoice before voiding it."
        )
    );
    unapply_deposit(
        &mut f.book,
        f.actor,
        f.c,
        s(&a, "id"),
        Some(1),
        &json!("2026-10-05"),
        &json!("release"),
    )
    .unwrap();
    let e = sales::void_invoice(
        &mut f.book,
        f.actor,
        f.c,
        s(&i, "id"),
        Some(2),
        &json!("2026-10-04"),
        &json!("void"),
    )
    .unwrap_err();
    assert_eq!(
        e.to_string(),
        "ValidationError: Void date cannot precede 2026-10-05, when a customer deposit was unapplied from this invoice."
    );
}

#[test]
fn fix1_python_date_differential_vectors() {
    let rows: Value = serde_json::from_str(include_str!("date_vectors.json")).unwrap();
    for row in rows.as_array().unwrap() {
        let result = date(&row[0]).ok();
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            row[1],
            "input {}",
            row[0]
        );
    }
}

fn fix1_history(f: &mut Fixture) -> Vec<(&'static str, &'static str, Value)> {
    let d = f.receive();
    let i = f.invoice();
    let a = f.apply(&d, &i, "100.00");
    let r = f.refund(&d);
    let u = unapply_deposit(
        &mut f.book,
        f.actor,
        f.c,
        s(&a, "id"),
        Some(1),
        &json!("2026-10-05"),
        &json!("release"),
    )
    .unwrap();
    let v = void_refund(
        &mut f.book,
        f.actor,
        f.c,
        s(&r, "id"),
        Some(1),
        &json!("2026-10-05"),
        &json!("release"),
    )
    .unwrap();
    let z = void_deposit(
        &mut f.book,
        f.actor,
        f.c,
        s(&d, "id"),
        Some(1),
        &json!("2026-10-05"),
        &json!("release"),
    )
    .unwrap();
    vec![
        ("sales_receipt", "deposit", d),
        ("prepayments_depositapplication", "application", a),
        ("prepayments_depositrefund", "refund", r),
        ("prepayments_depositunapplication", "unapplication", u),
        ("prepayments_depositrefundvoid", "refundvoid", v),
        ("sales_receiptreversal", "depositvoid", z),
    ]
}

#[test]
fn fix1_full_snapshot_schema_matrix_at_insert_seal_check_and_open() {
    let mut f = Fixture::new();
    let history = fix1_history(&mut f);
    for (table, kind, row) in history {
        let raw: String = f
            .book
            .connection()
            .query_row(
                &format!("SELECT data FROM {table} WHERE id=?"),
                [s(&row, "id")],
                |r| r.get(0),
            )
            .unwrap();
        let good: Value = serde_json::from_str(&raw).unwrap();
        assert!(fb_db::prepayments::prepayment_snapshot_ok(kind, &good));
        let mut variants = vec![json!(false), json!([]), json!({})];
        if kind == "refund" {
            let mut missing_pair = good.clone();
            missing_pair.as_object_mut().unwrap().remove("method");
            missing_pair.as_object_mut().unwrap().remove("check_number");
            variants.push(missing_pair);
        }
        for key in good.as_object().unwrap().keys() {
            // These are all mandatory evidence keys; use stored rows, not response enrichment.
            let mut missing = good.clone();
            missing.as_object_mut().unwrap().remove(key);
            variants.push(missing);
            let mut wrong = good.clone();
            wrong[key] = json!(false);
            variants.push(wrong);
        }
        for field in ["customer_snapshot", "company_snapshot"] {
            if let Some(object) = good[field].as_object() {
                for key in object.keys() {
                    let mut bad = good.clone();
                    bad[field].as_object_mut().unwrap().remove(key);
                    variants.push(bad);
                    let mut bad = good.clone();
                    bad[field][key] = json!([]);
                    variants.push(bad);
                }
                let mut bad = good.clone();
                bad[field]["id"] = json!(new_id());
                variants.push(bad);
            }
        }
        // Isolate the new INSERT predicate from collision/other constraints.
        // Every bad value must fail specifically at the evidence guard.
        let q = f.book.connection();
        let definitions = q
            .prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger' AND tbl_name=?")
            .unwrap()
            .query_map([table], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        q.execute_batch("BEGIN").unwrap();
        for (name, _) in &definitions {
            if name != &format!("pp_{kind}_evidence") {
                q.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
            }
        }
        let columns = q
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let selects = columns
            .iter()
            .map(|c| {
                if c == "data" {
                    "?".to_owned()
                } else {
                    c.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(",");
        for bad in &variants {
            assert!(
                !fb_db::prepayments::prepayment_snapshot_ok(kind, bad),
                "{kind}: {bad}"
            );
            let e = q
                .execute(
                    &format!("INSERT INTO {table} SELECT {selects} FROM {table} WHERE id=?"),
                    params![bad.to_string(), s(&row, "id")],
                )
                .unwrap_err();
            assert!(
                e.to_string().contains("prepayment_fields"),
                "{kind}: {bad}: {e}"
            );
        }
        q.execute_batch("ROLLBACK").unwrap();
        // Keep the original schema intact while corrupting evidence through the
        // test-only raw connection; check(), seal and open must fail closed.
        for (name, sql) in &definitions {
            if sql.contains("BEFORE UPDATE") {
                q.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
            }
        }
        q.execute(
            &format!("UPDATE {table} SET data=json_remove(data,'$.actor_name') WHERE id=?"),
            [s(&row, "id")],
        )
        .unwrap();
        for (_, sql) in &definitions {
            if sql.contains("BEFORE UPDATE") {
                q.execute_batch(sql).unwrap();
            }
        }
        assert!(!f.book.check().unwrap().is_empty(), "{table}");
        let path = q.path().unwrap().to_owned();
        assert!(
            Book::open(std::path::Path::new(&path), &[72; 32]).is_err(),
            "{table}"
        );
        let e = q
            .execute(
                "INSERT INTO prepayments_seal VALUES('journal',?)",
                [new_id()],
            )
            .unwrap_err();
        assert!(e.to_string().contains("prepayment_fields"), "{table}: {e}");
        for (name, sql) in &definitions {
            if sql.contains("BEFORE UPDATE") {
                q.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
            }
        }
        q.execute(
            &format!("UPDATE {table} SET data=? WHERE id=?"),
            params![raw, s(&row, "id")],
        )
        .unwrap();
        for (_, sql) in &definitions {
            if sql.contains("BEFORE UPDATE") {
                q.execute_batch(sql).unwrap();
            }
        }
        assert!(f.book.check().unwrap().is_empty());
    }
}

#[test]
fn fix1_every_actor_reference_retained_without_indirect_ledger_or_audit_refs() {
    let mut f = Fixture::new();
    let history = fix1_history(&mut f);
    for (table, _, row) in history
        .into_iter()
        .filter(|(table, _, _)| table.starts_with("prepayments_"))
    {
        let actor =
            crate::users::create_user(&mut f.book, &format!("Only {table}"), false).unwrap();
        let q = f.book.connection();
        let definitions=q.prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger' AND tbl_name=? AND sql LIKE '%BEFORE UPDATE%'")
            .unwrap().query_map([table],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
        for (name, _) in &definitions {
            q.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
        }
        q.execute(
            &format!("UPDATE {table} SET actor_id=?,data=json_set(data,'$.actor_id',?) WHERE id=?"),
            params![actor.to_db(), actor.to_db(), s(&row, "id")],
        )
        .unwrap();
        for (_, sql) in &definitions {
            q.execute_batch(sql).unwrap();
        }
        for recursive in ["OFF", "ON"] {
            q.execute_batch(&format!(
                "PRAGMA recursive_triggers={recursive};BEGIN;PRAGMA defer_foreign_keys=ON"
            ))
            .unwrap();
            assert!(
                q.execute("DELETE FROM fb_user WHERE id=?", [actor.to_db()])
                    .is_err(),
                "{table}"
            );
            for command in ["UPDATE", "UPDATE OR REPLACE"] {
                assert!(
                    q.execute(
                        &format!("{command} fb_user SET id=? WHERE id=?"),
                        params![new_id(), actor.to_db()]
                    )
                    .is_err(),
                    "{table}"
                );
            }
            assert!(q.execute("INSERT OR REPLACE INTO fb_user(id,username,is_active,is_staff,is_superuser) VALUES(?,'rebound',1,0,0)",[actor.to_db()]).is_err(),"{table}");
            q.execute_batch("ROLLBACK").unwrap();
        }
        q.execute_batch("PRAGMA recursive_triggers=ON").unwrap();
        assert!(f.book.check().unwrap().is_empty());
        let path = q.path().unwrap();
        assert!(Book::open(std::path::Path::new(path), &[72; 32]).is_ok());
    }
}

#[test]
fn fix1_every_foreign_parent_has_immediate_retention_and_permanent_keys() {
    let mut f = Fixture::new();
    fix1_history(&mut f);
    // 0140 adds an optional estimate FK. Populate it so the same exhaustive
    // retention/re-key attacks exercise this parent alongside every older FK.
    let estimate = sales::save_estimate(&mut f.book, f.actor, f.c, &json!({
        "request_key": new_id(), "customer_id": f.customer, "date": "2026-10-01",
        "tax_mode": "NONE", "lines": [{"description": "Work", "quantity": "1", "rate": "100", "taxable": false}]
    })).unwrap();
    let mut input = f.input();
    input["estimate_id"] = estimate["id"].clone();
    let review = review_deposit(&mut f.book, f.actor, f.c, &input).unwrap();
    record_deposit(&mut f.book, f.actor, f.c, Some(&review), &input).unwrap();
    let q = f.book.connection();
    let before = snapshot(q);
    for table in [
        "prepayments_customerdeposit",
        "prepayments_depositapplication",
        "prepayments_depositrefund",
        "prepayments_depositunapplication",
        "prepayments_depositrefundvoid",
    ] {
        let parents = q
            .prepare(&format!("PRAGMA foreign_key_list({table})"))
            .unwrap()
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        for (parent, column, key) in parents {
            let id: String = q
                .query_row(&format!("SELECT {column} FROM {table} WHERE {column} IS NOT NULL LIMIT 1"), [], |r| {
                    r.get(0)
                })
                .unwrap();
            for recursive in ["OFF", "ON"] {
                q.execute_batch(&format!(
                    "PRAGMA recursive_triggers={recursive};BEGIN;PRAGMA defer_foreign_keys=ON"
                ))
                .unwrap();
                assert!(
                    q.execute(&format!("DELETE FROM {parent} WHERE {key}=?"), [&id])
                        .is_err(),
                    "{table}.{column}"
                );
                assert!(
                    q.execute(
                        &format!("UPDATE OR REPLACE {parent} SET {key}=? WHERE {key}=?"),
                        params![new_id(), &id]
                    )
                    .is_err(),
                    "{table}.{column}"
                );
                q.execute_batch("ROLLBACK").unwrap();
            }
        }
    }
    q.execute_batch("PRAGMA recursive_triggers=ON").unwrap();
    assert_eq!(snapshot(q), before);
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn fix1_configuration_changes_preserve_history_and_original_liability() {
    let mut f = Fixture::new();
    let d = f.receive();
    let i = f.invoice();
    let a = f.apply(&d, &i, "100.00");
    let r = f.refund(&d);
    let mut p = services::AccountInput::new("231", "Replacement liability");
    p.kind = Some("LIABILITY".into());
    p.detail_type = Some("OTHER_CURRENT_LIABILITY".into());
    let liability = services::save_account(&mut f.book, f.actor, f.c, p)
        .unwrap()
        .id;
    documents::save_setup(&mut f.book,f.actor,f.c,&json!({"receivables":f.ar.to_db(),"customer_deposits":liability.to_db(),"undeposited":f.bank.to_db(),"banks":[]}),Some(1)).unwrap();
    assert!(f.book.check().unwrap().is_empty());
    let path = f.book.connection().path().unwrap().to_owned();
    assert!(Book::open(std::path::Path::new(&path), &[72; 32]).is_ok());
    unapply_deposit(
        &mut f.book,
        f.actor,
        f.c,
        s(&a, "id"),
        Some(1),
        &json!("2026-10-05"),
        &json!("release"),
    )
    .unwrap();
    void_refund(
        &mut f.book,
        f.actor,
        f.c,
        s(&r, "id"),
        Some(1),
        &json!("2026-10-05"),
        &json!("release"),
    )
    .unwrap();
    let later = f.apply(&d, &i, "100.00");
    assert_eq!(later["liability_id"], f.liability.to_db());
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn fix1_populated_0071_copy_upgrades_preserving_every_prepayment_row() {
    let mut f = Fixture::new();
    fix1_history(&mut f);
    let current = snapshot(f.book.connection());
    let prior = fb_db::MIGRATIONS
        .iter()
        .position(|(name, _)| *name == "0072_prepayment_review_fixes")
        .unwrap();
    for tamper in [false, true] {
        let path = std::env::temp_dir().join(format!("pp-upgrade-{}.fbbook", new_id()));
        let q = Connection::open(&path).unwrap();
        q.execute_batch(
            "PRAGMA key=\"x'4848484848484848484848484848484848484848484848484848484848484848'\";",
        )
        .unwrap();
        fb_db::collation::register(&q).unwrap();
        for (_, sql) in &fb_db::MIGRATIONS[..prior] {
            q.execute_batch(sql).unwrap();
        }
        let triggers = q
            .prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger'")
            .unwrap()
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        q.execute_batch("BEGIN IMMEDIATE; PRAGMA defer_foreign_keys=ON")
            .unwrap();
        for (name, _) in &triggers {
            q.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
        }
        let tables = q
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT GLOB 'sqlite_*'",
            )
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        for t in tables {
            let mut st = f
                .book
                .connection()
                .prepare(&format!("SELECT * FROM {t}"))
                .unwrap();
            let n = st.column_count();
            for row in st
                .query_map([], |r| {
                    (0..n)
                        .map(|i| r.get::<_, SqlValue>(i))
                        .collect::<Result<Vec<_>, _>>()
                })
                .unwrap()
            {
                q.execute(
                    &format!("INSERT INTO {t} VALUES({})", vec!["?"; n].join(",")),
                    params_from_iter(row.unwrap()),
                )
                .unwrap();
            }
        }
        for (_, sql) in triggers {
            q.execute_batch(&sql).unwrap();
        }
        q.execute_batch(&format!("PRAGMA user_version={prior};COMMIT"))
            .unwrap();
        let before = snapshot(&q);
        if tamper {
            q.execute_batch("DROP TRIGGER fb_sales_receipt_insert;CREATE TRIGGER fb_sales_receipt_insert BEFORE INSERT ON sales_receipt BEGIN SELECT 1;END;").unwrap();
        }
        drop(q);
        let opened = Book::open(&path, &[72; 32]);
        if tamper {
            assert!(opened.is_err());
            continue;
        }
        let b = opened.unwrap();
        let after = snapshot(b.connection());
        for (t, rows) in before.as_object().unwrap() {
            assert_eq!(&after[t], rows, "{t}");
        }
        assert_eq!(after, current);
        assert!(b.check().unwrap().is_empty());
        drop(b);
        assert!(
            Book::open(&path, &[72; 32])
                .unwrap()
                .check()
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn fix1_sql_destination_and_liability_designation_matrix() {
    let mut f = Fixture::new();
    let path = f.book.connection().path().unwrap().to_owned();
    let mut b = Book::open(std::path::Path::new(&path), &[72; 32]).unwrap();
    for (destination, account, valid) in [
        ("BANK", f.bank, true),
        ("UNDEPOSITED", f.undeposited, true),
        ("BANK", f.ar, false),
        ("BANK", f.undeposited, false),
        ("UNDEPOSITED", f.bank, false),
    ] {
        let result = b.write(|tx| review_raw_deposit(tx, &f, destination, account));
        assert_eq!(result.is_ok(), valid, "{destination}: {result:?}");
    }
    let mut input = services::AccountInput::new("239", "Undesignated liability");
    input.kind = Some("LIABILITY".into());
    input.detail_type = Some("OTHER_CURRENT_LIABILITY".into());
    f.liability = services::save_account(&mut f.book, f.actor, f.c, input)
        .unwrap()
        .id;
    let result = b
        .write(|tx| review_raw_deposit(tx, &f, "BANK", f.bank))
        .unwrap_err();
    assert_eq!(result.constraint(), Some("customer_deposit_scope"));
    assert!(b.check().unwrap().is_empty());
    drop(b);
    assert!(Book::open(std::path::Path::new(&path), &[72; 32]).is_ok());
}

// Test-only hooks prove the second writer actually attempted its lock while
// the first still owned it. No timing assumption chooses the winning command.
static FIX1_WAITING_WRITER: std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>> =
    std::sync::Mutex::new(None);
fn fix1_busy_writer(_: i32) -> bool {
    if let Some(sender) = FIX1_WAITING_WRITER.lock().unwrap().take() {
        let _ = sender.send(());
    }
    std::thread::sleep(std::time::Duration::from_millis(1));
    true
}
type Fix1Command = Box<dyn FnOnce(&mut Book) -> FbResult<Value> + Send>;
fn fix1_ordered_race(
    path: &str,
    first: Fix1Command,
    second: Fix1Command,
) -> (FbResult<Value>, FbResult<Value>) {
    use std::{sync::mpsc, time::Duration};
    let mut a = Book::open(std::path::Path::new(path), &[72; 32]).unwrap();
    let mut b = Book::open(std::path::Path::new(path), &[72; 32]).unwrap();
    let (held_tx, held_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (waiting_tx, waiting_rx) = mpsc::channel();
    *FIX1_WAITING_WRITER.lock().unwrap() = Some(waiting_tx);
    a.connection_mut()
        .commit_hook(Some(move || {
            held_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(20)).is_err()
        }))
        .unwrap();
    b.connection_mut()
        .busy_handler(Some(fix1_busy_writer))
        .unwrap();
    let a = std::thread::spawn(move || first(&mut a));
    held_rx
        .recv_timeout(Duration::from_secs(20))
        .expect("first command must reach commit holding the write lock");
    let b = std::thread::spawn(move || second(&mut b));
    waiting_rx
        .recv_timeout(Duration::from_secs(20))
        .expect("second writer must contend for the held lock");
    release_tx.send(()).unwrap();
    (a.join().unwrap(), b.join().unwrap())
}

#[test]
fn fix1_both_lock_orders_for_every_available_prepayment_race() {
    for competitor in [
        "refund",
        "invoice_void",
        "deposit_void",
        "receipt",
        "application",
        "retry",
    ] {
        for application_first in [true, false] {
            let mut f = Fixture::new();
            let d = f.receive();
            let i = f.invoice();
            let actor = f.actor;
            let c = f.c;
            let p = json!({"request_key":new_id(),"invoice_id":i["id"],"date":"2026-10-03","amount":"400.00"});
            let review = review_application(&mut f.book, actor, c, s(&d, "id"), &p).unwrap();
            let d1 = d.clone();
            let p1 = p.clone();
            let review1 = review.clone();
            let application: Fix1Command =
                Box::new(move |b| apply_deposit(b, actor, c, s(&d1, "id"), Some(&review1), &p1));
            let d2 = d.clone();
            let i2 = i.clone();
            let other: Fix1Command = match competitor {
                "refund" => {
                    let rp = json!({"request_key":new_id(),"date":"2026-10-03","amount":"400.00","bank_id":f.bank.to_db(),"method":"CASH"});
                    let rr = review_refund(&mut f.book, actor, c, s(&d, "id"), &rp).unwrap();
                    Box::new(move |b| record_refund(b, actor, c, s(&d2, "id"), Some(&rr), &rp))
                }
                "invoice_void" => Box::new(move |b| {
                    sales::void_invoice(
                        b,
                        actor,
                        c,
                        s(&i2, "id"),
                        Some(2),
                        &json!("2026-10-04"),
                        &json!("race"),
                    )
                }),
                "deposit_void" => Box::new(move |b| {
                    void_deposit(
                        b,
                        actor,
                        c,
                        s(&d2, "id"),
                        Some(1),
                        &json!("2026-10-04"),
                        &json!("race"),
                    )
                }),
                "receipt" => {
                    let rp = json!({"request_key":new_id(),"customer_id":f.customer,"date":"2026-10-03","amount":"1000.00","method":"CASH","destination":"BANK","bank_id":f.bank.to_db(),"allocations":[{"invoice_id":i["id"],"amount":"1000.00"}]});
                    let rr = sales::review_receipt(&mut f.book, actor, c, &rp).unwrap();
                    Box::new(move |b| sales::record_receipt(b, actor, c, &rp, Some(&rr)))
                }
                "application" => {
                    let mut p = p.clone();
                    p["request_key"] = json!(new_id());
                    let r = review_application(&mut f.book, actor, c, s(&d, "id"), &p).unwrap();
                    Box::new(move |b| apply_deposit(b, actor, c, s(&d2, "id"), Some(&r), &p))
                }
                _ => Box::new(move |b| apply_deposit(b, actor, c, s(&d2, "id"), Some(&review), &p)),
            };
            let path = f.book.connection().path().unwrap().to_owned();
            let (first, second) = if application_first {
                fix1_ordered_race(&path, application, other)
            } else {
                fix1_ordered_race(&path, other, application)
            };
            assert!(
                first.is_ok(),
                "{competitor}, application first={application_first}: {first:?}"
            );
            if competitor == "retry" {
                assert_eq!(first.as_ref().unwrap(), second.as_ref().unwrap());
            } else {
                assert!(
                    second.is_err(),
                    "{competitor}, application first={application_first}: {second:?}"
                );
            }
            assert!(f.book.check().unwrap().is_empty());
            assert!(Book::open(std::path::Path::new(&path), &[72; 32]).is_ok());
        }
    }
}
