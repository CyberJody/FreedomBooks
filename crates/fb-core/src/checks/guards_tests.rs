//! Integrated-core regressions: real SQLCipher books, complete row snapshots.
use super::*;

// Attacker's fresh primary keys: canonical UUIDs, so 0130 passes and the key guards decide.
const FRESH_PRIMARY_KEY: &str = "00000000-0000-7000-8000-0000000ab101"; // re-key target
const FRESH_ID: &str = "00000000-0000-7000-8000-0000000ab102"; // alternate-key replacement row
const STATEMENT_KINDS: &[&str] = &["update", "update_or_replace", "delete", "insert_or_replace", "upsert", "view_update_or_replace"];
const PRIMARY_KEY_GUARDS: &[&str] = &[
    "checks_check rekey: checks_primary_key",
    "checks_checkline noop: checks_history_immutable",
    "checks_checkline rekey: checks_history_immutable",
    "checks_checkrevision noop: checks_history_immutable",
    "checks_checkrevision rekey: checks_history_immutable",
    "transfers_transfer noop: checks_history_immutable",
    "transfers_transfer rekey: checks_history_immutable",
    "transfers_transfervoid noop: checks_history_immutable",
    "transfers_transfervoid rekey: checks_history_immutable",
];
const UNIQUE_KEY_GUARDS: &[&str] = &[
    "checks_check delete: checks_history_immutable",
    "checks_check insert_or_replace: check_immutable_ownership",
    "checks_check update: check_immutable_ownership",
    "checks_check update: checks_primary_key",
    "checks_check update_or_replace: check_immutable_ownership",
    "checks_check update_or_replace: checks_primary_key",
    "checks_check upsert: check_immutable_ownership",
    "checks_check view_update_or_replace: check_immutable_ownership",
    "checks_check view_update_or_replace: checks_primary_key",
    "checks_checkline delete: checks_history_immutable",
    "checks_checkline insert_or_replace: check_history_immutable",
    "checks_checkline update: checks_history_immutable",
    "checks_checkline update_or_replace: checks_history_immutable",
    "checks_checkline upsert: check_history_immutable",
    "checks_checkline view_update_or_replace: checks_history_immutable",
    "checks_checkrevision delete: checks_history_immutable",
    "checks_checkrevision insert_or_replace: check_history_immutable",
    "checks_checkrevision update: checks_history_immutable",
    "checks_checkrevision update_or_replace: checks_history_immutable",
    "checks_checkrevision upsert: check_history_immutable",
    "checks_checkrevision view_update_or_replace: checks_history_immutable",
    "transfers_transfer delete: checks_history_immutable",
    "transfers_transfer insert_or_replace: transfer_immutable",
    "transfers_transfer update: checks_history_immutable",
    "transfers_transfer update_or_replace: checks_history_immutable",
    "transfers_transfer upsert: transfer_immutable",
    "transfers_transfer view_update_or_replace: checks_history_immutable",
    "transfers_transfervoid delete: checks_history_immutable",
    "transfers_transfervoid insert_or_replace: transfer_immutable",
    "transfers_transfervoid update: checks_history_immutable",
    "transfers_transfervoid update_or_replace: checks_history_immutable",
    "transfers_transfervoid upsert: transfer_immutable",
    "transfers_transfervoid view_update_or_replace: checks_history_immutable",
];
const TABLES: &[&str] = &[
    "checks_check",
    "checks_checkrevision",
    "checks_checkline",
    "transfers_transfer",
    "transfers_transfervoid",
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
        p["check_number"] = json!(format!("10{n}"));
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
            &json!("Correction"),
        )
        .unwrap();
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
pub(super) fn primary_keys_reject_fresh_rekeys() {
    let mut f = populated();
    let mut p = f.payload();
    p["check_number"] = json!("editable");
    let b = f.save(&p);
    let c = f.book.connection();
    let before = snapshot(c);
    let mut hit = BTreeSet::new();
    for recursion in [0, 1] {
        c.execute_batch(&format!("PRAGMA recursive_triggers={recursion}"))
            .unwrap();
        for table in TABLES {
            c.execute_batch("BEGIN IMMEDIATE").unwrap();
            let sql = if *table == "checks_check" {
                format!(
                    "UPDATE {table} SET id='{FRESH_PRIMARY_KEY}',revision=revision+1 WHERE id='{}'",
                    s(&b, "id")
                )
            } else {
                format!("UPDATE {table} SET id='{FRESH_PRIMARY_KEY}'")
            };
            let result = c.execute_batch(&sql);
            c.execute_batch("ROLLBACK").unwrap();
            hit.insert(format!("{table} rekey: {}", super::primitive::sql_refused(result)));
            assert_eq!(snapshot(c), before);
            if *table != "checks_check" {
                let guard = super::primitive::sql_refused(c.execute_batch(&format!("UPDATE {table} SET id=id")));
                hit.insert(format!("{table} noop: {guard}"));
                assert_eq!(snapshot(c), before);
            }
        }
    }
    super::primitive::assert_guard_set(&hit, PRIMARY_KEY_GUARDS);
}

#[test]
pub(super) fn all_unique_keys_refuse_replacement_and_delete_with_recursion_on_or_off() {
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
            unique.retain(|cols| !cols.iter().any(|c| c == "number_key"));
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
                    let guard = super::primitive::sql_refused(c.execute_batch(&statement));
                    hit.insert(format!("{table} {kind}: {guard}"));
                    assert_eq!(snapshot(c), before, "changed: {statement}");
                }
                c.execute_batch("DROP VIEW matrix_view").unwrap();
            }
        }
    }
    super::primitive::assert_guard_set(&hit, UNIQUE_KEY_GUARDS);
}

#[test]
pub(super) fn editable_check_cannot_displace_live_reference_victim() {
    let mut f = Fx::new();
    let p = f.payload();
    let victim = f.save(&p);
    let victim = f.post(&victim);
    let mut p = f.payload();
    p["check_number"] = json!("editable");
    let source = f.save(&p);
    let c = f.book.connection();
    let before = snapshot(c);
    for recursion in [0, 1] {
        c.execute_batch(&format!("PRAGMA recursive_triggers={recursion}"))
            .unwrap();
        let set = format!(
            "number_key=(SELECT number_key FROM checks_check WHERE id='{}'),revision=revision+1",
            s(&victim, "id")
        );
        c.execute_batch(&format!("CREATE TEMP VIEW matrix_view AS SELECT * FROM checks_check; CREATE TEMP TRIGGER matrix_update INSTEAD OF UPDATE ON matrix_view BEGIN UPDATE checks_check SET {set} WHERE id=OLD.id; END")).unwrap();
        for sql in [
            format!(
                "UPDATE OR REPLACE checks_check SET {set} WHERE id='{}'",
                s(&source, "id")
            ),
            format!(
                "UPDATE OR REPLACE matrix_view SET revision=revision+1 WHERE id='{}'",
                s(&source, "id")
            ),
        ] {
            super::primitive::sql_refused(c.execute_batch(&sql));
            assert_eq!(snapshot(c), before);
        }
        c.execute_batch("DROP VIEW matrix_view").unwrap();
    }
}

#[test]
fn every_checks_schema_object_is_definition_hashed() {
    let f = populated();
    let c = f.book.connection();
    let objects: Vec<(String,String)> = c.prepare("SELECT name,sql FROM sqlite_master WHERE sql IS NOT NULL AND (name GLOB '*checks*' OR tbl_name GLOB 'checks_*' OR name GLOB 'fb_check*' OR name GLOB '*transfer*')").unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<Result<_,_>>().unwrap();
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
fn externally_corrupted_checks_evidence_fails_check_and_reopen() {
    for (table, setter) in [
        ("checks_checkline", "amount=amount+1"),
        ("checks_check", "number_key='changed'"),
        ("transfers_transfer", "amount=amount+1"),
        ("transfers_transfervoid", "date='2026-10-25'"),
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
#[test]
fn retained_nonpostable_account_matches_service() {
    let mut f = Fx::new();
    let mut p = f.payload();
    let b = f.save(&p);
    f.book
        .connection()
        .execute(
            "UPDATE ledger_account SET postable=0 WHERE id=?",
            [f.expense.to_db()],
        )
        .unwrap();
    p["check_id"] = b["id"].clone();
    p["expected_revision"] = json!(1);
    let b = f.save(&p);
    assert_eq!(b["revision"], 2);
    assert!(review_check(&mut f.book, f.actor, f.co, s(&b, "id"), Some(2)).is_err());
}
