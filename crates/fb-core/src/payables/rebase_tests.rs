//! Integrated-core regressions: real SQLCipher books, complete row snapshots.
use super::*;

// Attacker's fresh primary keys: canonical UUIDs, so 0130 passes and the key guards decide.
const FRESH_PRIMARY_KEY: &str = "00000000-0000-7000-8000-0000000b0101"; // re-key target
const FRESH_ID: &str = "00000000-0000-7000-8000-0000000b0102"; // alternate-key replacement row
const STATEMENT_KINDS: &[&str] = &["update", "update_or_replace", "delete", "insert_or_replace", "upsert", "view_update_or_replace"];
const PRIMARY_KEY_GUARDS: &[&str] = &[
    "payables_bill rekey: payables_permanent_id",
    "payables_billline noop: payables_permanent_id",
    "payables_billline rekey: payables_permanent_id",
    "payables_billpayment noop: payables_permanent_id",
    "payables_billpayment rekey: payables_permanent_id",
    "payables_billrevision noop: payables_permanent_id",
    "payables_billrevision rekey: payables_permanent_id",
    "payables_paymentallocation noop: payables_permanent_id",
    "payables_paymentallocation rekey: payables_permanent_id",
    "payables_paymentreversal noop: payables_permanent_id",
    "payables_paymentreversal rekey: payables_permanent_id",
];
const UNIQUE_KEY_GUARDS: &[&str] = &[
    "payables_bill delete: bill_immutable_ownership",
    "payables_bill insert_or_replace: bill_immutable_ownership",
    "payables_bill update: bill_immutable_ownership",
    "payables_bill update: payables_permanent_id",
    "payables_bill update_or_replace: bill_immutable_ownership",
    "payables_bill update_or_replace: payables_permanent_id",
    "payables_bill upsert: bill_immutable_ownership",
    "payables_bill view_update_or_replace: bill_immutable_ownership",
    "payables_bill view_update_or_replace: payables_permanent_id",
    "payables_billline delete: bill_history_immutable",
    "payables_billline insert_or_replace: bill_history_immutable",
    "payables_billline insert_or_replace: payables_line_origin",
    "payables_billline update: bill_history_immutable",
    "payables_billline update: payables_permanent_id",
    "payables_billline update_or_replace: bill_history_immutable",
    "payables_billline update_or_replace: payables_permanent_id",
    "payables_billline upsert: payables_line_origin",
    "payables_billline view_update_or_replace: bill_history_immutable",
    "payables_billline view_update_or_replace: payables_permanent_id",
    "payables_billpayment delete: payment_immutable",
    "payables_billpayment insert_or_replace: payment_immutable",
    "payables_billpayment update: payables_permanent_id",
    "payables_billpayment update: payment_immutable",
    "payables_billpayment update_or_replace: payables_permanent_id",
    "payables_billpayment update_or_replace: payment_immutable",
    "payables_billpayment upsert: payment_immutable",
    "payables_billpayment view_update_or_replace: payables_permanent_id",
    "payables_billpayment view_update_or_replace: payment_immutable",
    "payables_billrevision delete: bill_history_immutable",
    "payables_billrevision insert_or_replace: bill_history_immutable",
    "payables_billrevision update: bill_history_immutable",
    "payables_billrevision update: payables_permanent_id",
    "payables_billrevision update_or_replace: bill_history_immutable",
    "payables_billrevision update_or_replace: payables_permanent_id",
    "payables_billrevision upsert: bill_history_immutable",
    "payables_billrevision view_update_or_replace: bill_history_immutable",
    "payables_billrevision view_update_or_replace: payables_permanent_id",
    "payables_paymentallocation delete: payment_immutable",
    "payables_paymentallocation insert_or_replace: payment_immutable",
    "payables_paymentallocation update: payables_permanent_id",
    "payables_paymentallocation update: payment_immutable",
    "payables_paymentallocation update_or_replace: payables_permanent_id",
    "payables_paymentallocation update_or_replace: payment_immutable",
    "payables_paymentallocation upsert: payment_immutable",
    "payables_paymentallocation view_update_or_replace: payables_permanent_id",
    "payables_paymentallocation view_update_or_replace: payment_immutable",
    "payables_paymentreversal delete: payment_immutable",
    "payables_paymentreversal insert_or_replace: payment_immutable",
    "payables_paymentreversal update: payables_permanent_id",
    "payables_paymentreversal update: payment_immutable",
    "payables_paymentreversal update_or_replace: payables_permanent_id",
    "payables_paymentreversal update_or_replace: payment_immutable",
    "payables_paymentreversal upsert: payment_immutable",
    "payables_paymentreversal view_update_or_replace: payables_permanent_id",
    "payables_paymentreversal view_update_or_replace: payment_immutable",
];
const LIVE_REFERENCE_GUARDS: &[&str] = &[
    "table: bill_immutable_ownership",
    "view: bill_immutable_ownership",
];
const TABLES: &[&str] = &[
    "payables_bill",
    "payables_billrevision",
    "payables_billline",
    "payables_billpayment",
    "payables_paymentallocation",
    "payables_paymentreversal",
];

pub(super) fn snapshot(c: &(impl Query + ?Sized)) -> Value {
    let tables = rows(c, "SELECT name FROM sqlite_master WHERE type='table' AND name NOT GLOB 'sqlite_*' ORDER BY name", &[]).unwrap();
    let mut result = json!({});
    for t in tables {
        let table = s(&t, "name");
        let mut data = rows(c, &format!("SELECT * FROM {table}"), &[]).unwrap();
        data.sort_by_key(Value::to_string);
        result[table] = json!(data);
    }
    result
}

fn populated() -> Fx {
    let mut f = Fx::new();
    for n in 0..2 {
        let mut p = f.payload();
        p["reference"] = json!(format!("Reference {n}"));
        let b = f.save(&p);
        let b = f.post(&b);
        let mut p = f.payment(&b, "1.01");
        p["reference"] = json!(format!("10{n}"));
        let p = f.pay(&p);
        reverse_payment(
            &mut f.book,
            f.actor,
            f.co,
            s(&p, "id"),
            Some(1),
            &json!("2026-10-25"),
            &json!("Synthetic reversal"),
        )
        .unwrap();
        f.void(&b);
    }
    f
}

#[test]
fn protected_tables_have_no_hidden_identity() {
    let f = populated();
    let c = f.book.connection();
    let before = snapshot(c);
    for recursion in [0, 1] {
        c.execute_batch(&format!("PRAGMA recursive_triggers={recursion}"))
            .unwrap();
        for table in TABLES {
            let sql: String = c
                .query_row("SELECT sql FROM sqlite_master WHERE name=?", [table], |r| {
                    r.get(0)
                })
                .unwrap();
            assert!(
                sql.contains("WITHOUT ROWID"),
                "{table} still exposes hidden identity"
            );
            for alias in ["rowid", "_rowid_", "oid"] {
                for statement in [
                    format!("INSERT OR REPLACE INTO {table}({alias},id) VALUES(1,'fresh-key')"),
                    format!("UPDATE OR REPLACE {table} SET {alias}=1"),
                ] {
                    assert!(c.execute_batch(&statement).is_err(), "{statement}");
                    assert_eq!(snapshot(c), before);
                }
            }
        }
    }
}

#[test]
fn primary_keys_reject_fresh_rekeys() {
    let mut f = populated();
    let mut p = f.payload();
    p["reference"] = json!("editable");
    let b = f.save(&p);
    let c = f.book.connection();
    let before = snapshot(c);
    let mut hit = BTreeSet::new();
    for recursion in [0, 1] {
        c.execute_batch(&format!("PRAGMA recursive_triggers={recursion}"))
            .unwrap();
        for table in TABLES {
            c.execute_batch("BEGIN IMMEDIATE").unwrap();
            let sql = if *table == "payables_bill" {
                format!(
                    "UPDATE {table} SET id='{FRESH_PRIMARY_KEY}',revision=revision+1 WHERE id='{}'",
                    s(&b, "id")
                )
            } else {
                format!("UPDATE {table} SET id='{FRESH_PRIMARY_KEY}'")
            };
            let result = c.execute_batch(&sql);
            c.execute_batch("ROLLBACK").unwrap();
            hit.insert(format!("{table} rekey: {}", super::guard_of(result)));
            assert_eq!(snapshot(c), before);
            if *table != "payables_bill" {
                let guard = super::guard_of(c.execute_batch(&format!("UPDATE {table} SET id=id")));
                hit.insert(format!("{table} noop: {guard}"));
                assert_eq!(snapshot(c), before);
            }
        }
    }
    super::assert_guard_set(&hit, PRIMARY_KEY_GUARDS);
}

#[test]
fn all_unique_keys_refuse_replacement_and_delete_with_recursion_on_or_off() {
    let f = populated();
    let c = f.book.connection();
    let before = snapshot(c);
    let mut hit = BTreeSet::new();
    for recursion in [0, 1] {
        c.execute_batch(&format!("PRAGMA recursive_triggers={recursion}"))
            .unwrap();
        for table in TABLES {
            let records = rows(c, &format!("SELECT * FROM {table} ORDER BY id"), &[]).unwrap();
            let source = s(&records[0], "id");
            let victim = s(&records[1], "id");
            let mut unique = vec![vec!["id".to_string()]];
            for idx in rows(c, &format!("PRAGMA index_list('{table}')"), &[]).unwrap() {
                if idx["unique"] == 1 {
                    let columns =
                        rows(c, &format!("PRAGMA index_info('{}')", s(&idx, "name")), &[]).unwrap();
                    unique.push(columns.iter().map(|v| s(v, "name").to_owned()).collect());
                }
            }
            // Live-reference partial key uses draft source/victim in a separate test.
            unique.retain(|cols| !cols.iter().any(|c| c == "reference_key"));
            unique.push(records[0].as_object().unwrap().keys().cloned().collect());
            let cols = records[0]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            for key in unique {
                let setters = key
                    .iter()
                    .map(|k| format!("{k}=(SELECT {k} FROM {table} WHERE id='{victim}')"))
                    .collect::<Vec<_>>()
                    .join(",");
                let mut statements = vec![
                    format!("UPDATE {table} SET {setters} WHERE id='{source}'"),
                    format!("UPDATE OR REPLACE {table} SET {setters} WHERE id='{source}'"),
                    format!("DELETE FROM {table} WHERE id='{victim}'"),
                ];
                let values = cols
                    .iter()
                    .map(|k| {
                        if key.contains(k) {
                            format!("(SELECT {k} FROM {table} WHERE id='{victim}')")
                        } else if k == "id" {
                            format!("'{FRESH_ID}'")
                        } else {
                            k.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                statements.push(format!("INSERT OR REPLACE INTO {table}({}) SELECT {values} FROM {table} WHERE id='{source}'", cols.join(",")));
                statements.push(format!("INSERT INTO {table} SELECT * FROM {table} WHERE id='{victim}' ON CONFLICT DO UPDATE SET {setters}"));
                c.execute_batch(&format!("CREATE TEMP VIEW matrix_view AS SELECT * FROM {table}; CREATE TEMP TRIGGER matrix_update INSTEAD OF UPDATE ON matrix_view BEGIN UPDATE {table} SET {setters} WHERE id=OLD.id; END;")).unwrap();
                statements.push(format!(
                    "UPDATE OR REPLACE matrix_view SET id=id WHERE id='{source}'"
                ));
                for (kind, statement) in STATEMENT_KINDS.iter().zip(statements) {
                    let guard = super::guard_of(c.execute_batch(&statement));
                    hit.insert(format!("{table} {kind}: {guard}"));
                    assert_eq!(snapshot(c), before, "changed: {statement}");
                }
                c.execute_batch("DROP VIEW matrix_view").unwrap();
            }
        }
    }
    super::assert_guard_set(&hit, UNIQUE_KEY_GUARDS);
}

#[test]
fn editable_bill_cannot_displace_live_reference_victim() {
    let mut f = Fx::new();
    let p = f.payload();
    let victim = f.save(&p);
    let victim = f.post(&victim);
    let mut p = f.payload();
    p["reference"] = json!("editable");
    let source = f.save(&p);
    let c = f.book.connection();
    let before = snapshot(c);
    let mut hit = BTreeSet::new();
    for recursion in [0, 1] {
        c.execute_batch(&format!("PRAGMA recursive_triggers={recursion}"))
            .unwrap();
        let set = format!(
            "reference_key=(SELECT reference_key FROM payables_bill WHERE id='{}'),revision=revision+1",
            s(&victim, "id")
        );
        c.execute_batch(&format!("CREATE TEMP VIEW matrix_view AS SELECT * FROM payables_bill; CREATE TEMP TRIGGER matrix_update INSTEAD OF UPDATE ON matrix_view BEGIN UPDATE payables_bill SET {set} WHERE id=OLD.id; END")).unwrap();
        for sql in [
            format!(
                "UPDATE OR REPLACE payables_bill SET {set} WHERE id='{}'",
                s(&source, "id")
            ),
            format!(
                "UPDATE OR REPLACE matrix_view SET revision=revision+1 WHERE id='{}'",
                s(&source, "id")
            ),
        ] {
            let target = if sql.contains("matrix_view") { "view" } else { "table" };
            hit.insert(format!("{target}: {}", super::guard_of(c.execute_batch(&sql))));
            assert_eq!(snapshot(c), before);
        }
        c.execute_batch("DROP VIEW matrix_view").unwrap();
    }
    super::assert_guard_set(&hit, LIVE_REFERENCE_GUARDS);
}

#[test]
fn populated_payables_copy_migration_preserves_every_row_and_reopens() {
    let f=populated();
    for boundary in ["0025_payables_core_hardening","0027_payables_snapshots_and_origins"] {
        for tampered in [false,true] {
            let path=std::env::temp_dir().join(format!("payables-prior-{}.fbbook",fb_ids::new_v7()));
            let copy=fb_db::test_support::populated_copy(f.book.connection(),&path,&[42;32],boundary,false).unwrap();
            assert!(!copy.before["payables_billpayment"].is_empty());
            if tampered {
                let c=fb_db::rusqlite::Connection::open(&path).unwrap();
                c.execute_batch("PRAGMA key=\"x'2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a'\"").unwrap();
                c.execute_batch("DROP TRIGGER fb_bill_update_collision;CREATE TRIGGER fb_bill_update_collision BEFORE UPDATE ON payables_bill BEGIN SELECT 1;END").unwrap();
                drop(c);assert!(Book::open(&path,&[42;32]).is_err());continue;
            }
            for _ in 0..2 {
                let book=Book::open(&path,&[42;32]).unwrap();
                assert_eq!(fb_db::test_support::snapshot(book.connection()).unwrap(),copy.expected);
                assert!(book.check().unwrap().is_empty());
            }
        }
    }
}

#[test]
fn line_udf_is_available_at_create_and_reopen_in_both_callback_modes() {
    let mut f = Fx::new();
    let path = std::path::PathBuf::from(f.book.connection().path().unwrap());
    for reopen in [false, true] {
        if reopen {
            f.book = Book::open(&path, &[42; 32]).unwrap();
        }
        let sql = "SELECT fb_payables_line_valid(10050,100,101),fb_payables_line_valid(10050,100,100),fb_payables_line_valid(999999999999999999,9999999999999999,1)";
        f.book
            .read(|r| -> FbResult<()> {
                assert_eq!(
                    r.query_row(sql, [], |row| Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?
                    )))?,
                    (1, 0, 0)
                );
                Ok(())
            })
            .unwrap();
        f.book
            .write(|w| -> FbResult<()> {
                assert_eq!(
                    w.query_row(sql, [], |row| Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?
                    )))?,
                    (1, 0, 0)
                );
                Ok(())
            })
            .unwrap();
        let mut p = f.payload();
        p["reference"] = json!(format!("reopen {reopen}"));
        f.save(&p);
    }
}

#[test]
fn every_payables_schema_object_is_definition_hashed() {
    let f = populated();
    let c = f.book.connection();
    let objects: Vec<(String,String)> = c.prepare("SELECT name,sql FROM sqlite_master WHERE sql IS NOT NULL AND (name GLOB '*payables*' OR tbl_name GLOB 'payables_*' OR name GLOB 'fb_bill*' OR name GLOB 'fb_payment*')").unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<Result<_,_>>().unwrap();
    for (name, sql) in objects {
        c.execute_batch("PRAGMA writable_schema=ON").unwrap();
        c.execute(
            "UPDATE sqlite_master SET sql=sql||' ' WHERE name=?",
            [&name],
        )
        .unwrap();
        c.execute_batch("PRAGMA writable_schema=OFF").unwrap();
        assert!(
            f.book.check().unwrap().iter().any(|p| p.contains(&name)),
            "{name}"
        );
        assert!(
            Book::open(std::path::Path::new(c.path().unwrap()), &[42; 32]).is_err(),
            "reopened changed {name}"
        );
        c.execute_batch("PRAGMA writable_schema=ON").unwrap();
        c.execute(
            "UPDATE sqlite_master SET sql=? WHERE name=?",
            params![sql, name],
        )
        .unwrap();
        c.execute_batch("PRAGMA writable_schema=OFF").unwrap();
    }
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
pub(super) fn new_draft_lines_require_eligible_accounts_before_they_can_be_sealed() {
    for invalid in [
        "missing",
        "other-company",
        "inactive",
        "nonpostable",
        "income",
        "control",
    ] {
        let mut f = Fx::new();
        let mut p = f.payload();
        p["lines"][0]["expense_account_id"] = Value::Null;
        let b = f.save(&p);
        let raw = bill(f.book.connection(), f.co, s(&b, "id")).unwrap();
        let aid = if invalid == "missing" {
            fb_ids::new_v7().to_string()
        } else if invalid == "control" {
            f.ap.to_db()
        } else if invalid == "other-company" {
            let co = services::create_company(&mut f.book, f.actor, "Other", "USD", false).unwrap();
            services::save_account(
                &mut f.book,
                f.actor,
                co.id,
                AccountInput::new("5", "Other expense").kind("EXPENSE"),
            )
            .unwrap()
            .id
            .to_db()
        } else {
            let c = f.book.connection();
            let setter = match invalid {
                "inactive" => "active=0",
                "nonpostable" => "postable=0",
                _ => "kind='INCOME',detail_type='INCOME'",
            };
            c.execute(
                &format!("UPDATE ledger_account SET {setter} WHERE id=?"),
                [f.expense.to_db()],
            )
            .unwrap();
            f.expense.to_db()
        };
        let before = snapshot(f.book.connection());
        f.book
            .write(|tx| -> FbResult<()> {
                tx.execute(
                    "UPDATE payables_bill SET revision=revision+1 WHERE id=?",
                    [s(&raw, "id")],
                )?;
                let mut r = raw["current"].clone();
                r.as_object_mut().unwrap().remove("lines");
                r["id"] = json!(fb_ids::new_v7().to_string());
                r["number"] = json!(2);
                r["action"] = json!("revised");
                insert(tx, "payables_billrevision", &r)?;
                let mut l = raw["current"]["lines"][0].clone();
                l["id"] = json!(fb_ids::new_v7().to_string());
                l["revision_id"] = r["id"].clone();
                l["expense_account_id"] = json!(aid);
                let error = insert(tx, "payables_billline", &l)
                    .expect_err(&format!("accepted {invalid} account at insertion"));
                super::attacks::observed(format!(
                    "{}: {}",
                    error.constraint().unwrap_or(""),
                    error.message()
                ));
                Err::<(), _>(err("rollback fixture"))
            })
            .unwrap_err();
        assert_eq!(snapshot(f.book.connection()), before);
    }
}

#[test]
fn callback_history_insertion_requires_existing_actors() {
    for kind in ["revision", "payment", "reversal"] {
        let mut f = Fx::new();
        let p = f.payload();
        let b = f.save(&p);
        let b = if kind == "revision" { b } else { f.post(&b) };
        let payment = if kind == "reversal" {
            let p = f.payment(&b, "1.01");
            Some(f.pay(&p))
        } else {
            None
        };
        let b = bill(f.book.connection(), f.co, s(&b, "id")).unwrap();
        let before = snapshot(f.book.connection());
        f.book.write(|tx| -> FbResult<()> {
            let (table,mut row)=match kind {
                "revision"=>("payables_billrevision",super::attacks::new_revision(tx,&b,"DRAFT")?),
                "payment"=>("payables_billpayment",super::attacks::raw_payment_data(tx,f.actor,f.co,&b,f.bank,f.ap,false,false)?),
                _=>{
                    let p=payment.as_ref().unwrap();
                    let j=documents::reverse_source(tx,f.actor,f.co,s(p,"identity_id"),"2026-10-25","fixture")?;
                    ("payables_paymentreversal",json!({"id":fb_ids::new_v7().to_string(),"payment_id":p["id"],"journal_id":j.id.to_db(),"date":"2026-10-25","reason":"fixture","actor_id":f.actor.to_db(),"actor_name":"fixture","created_at":py::now_utc()}))
                }
            };
            row["actor_id"]=json!(fb_ids::new_v7().to_string());
            let e = insert(tx,table,&row).unwrap_err();
            assert_ne!(e.constraint(), Some("fb_valid"), "masked by fb_valid in {table}: {e}");
            Err::<(),_>(err("rollback fixture"))
        }).unwrap_err();
        assert_eq!(snapshot(f.book.connection()), before);
    }
}

#[test]
fn caught_composed_save_failure_rolls_back_document_ledger_and_audit() {
    let mut f = Fx::new();
    let p = f.payload();
    let before = snapshot(f.book.connection());
    f.book.connection().execute_batch("CREATE TEMP TRIGGER late_failure BEFORE INSERT ON ledger_auditevent WHEN NEW.action='bill.created' BEGIN SELECT RAISE(ABORT,'injected audit failure'); END").unwrap();
    f.book
        .write(|tx| -> FbResult<()> {
            assert!(save_bill_tx(tx, f.actor, f.co, &p).is_err());
            // The caller catches the primitive error and commits independent work.
            assert!(tx.execute_batch("COMMIT").is_err());
            Ok(())
        })
        .unwrap();
    assert_eq!(snapshot(f.book.connection()), before);
    f.book
        .connection()
        .execute_batch("DROP TRIGGER late_failure")
        .unwrap();
    f.save(&p);
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn externally_corrupted_payables_evidence_fails_check_and_reopen() {
    for (table, setter) in [
        ("payables_billline", "amount=amount+1"),
        ("payables_bill", "reference_key='changed'"),
        ("payables_paymentallocation", "amount=amount+1"),
    ] {
        let f = populated();
        let c = f.book.connection();
        let path = std::path::PathBuf::from(c.path().unwrap());
        let triggers: Vec<(String, String)> = c
            .prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger' AND tbl_name=?")
            .unwrap()
            .query_map([table], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        c.execute_batch("BEGIN IMMEDIATE").unwrap();
        for (name, _) in &triggers {
            c.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
        }
        c.execute_batch(&format!("UPDATE {table} SET {setter}"))
            .unwrap();
        for (_, sql) in &triggers {
            c.execute_batch(sql).unwrap();
        }
        c.execute_batch("COMMIT").unwrap();
        assert!(
            !f.book.check().unwrap().is_empty(),
            "unnoticed {table} corruption"
        );
        drop(f);
        assert!(
            Book::open(&path, &[42; 32]).is_err(),
            "reopened {table} corruption"
        );
    }
}
