//! Migration 0130: every TEXT column of the live schema carries the shared value guard for its class,
//! at insert, update and open, with exact counts (a later table, e.g. bank feeds at INT-L4b, cannot be missed).
use super::*;
use std::collections::BTreeMap;
use validators::Class;

/// Independent of the generator: the same naming rule, recomputed in Rust from the live schema.
fn class_of(column: &str) -> Class {
    if column == "doc_id" {
        Class::Text // seal/obligation bookkeeping (SEAL-DOCID-UUID deferral)
    } else if column == "id" || column.ends_with("_id") || column == "request_key" {
        Class::Uuid
    } else if column == "date" || column.ends_with("_date") {
        Class::Date
    } else if column.ends_with("_at") {
        Class::Timestamp
    } else {
        Class::Text
    }
}

fn live_text_columns(conn: &Connection) -> BTreeMap<String, BTreeMap<String, Class>> {
    let tables: Vec<String> = collect(conn, "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name", |r| r.get(0)).unwrap();
    let mut out = BTreeMap::new();
    for table in tables {
        let cols: Vec<(String, String)> = collect(conn, &format!("PRAGMA table_xinfo(\"{table}\")"), |r| Ok((r.get(1)?, r.get(2)?))).unwrap();
        let text: BTreeMap<String, Class> = cols.into_iter().filter(|(_, t)| t.eq_ignore_ascii_case("TEXT")).map(|(c, _)| { let k = if (table.starts_with("bankfeeds_") && c == "remote_id") || (table == "plugin_credential" && c == "plugin_id") { Class::Text } else { class_of(&c) }; (c, k) }).collect();
        if !text.is_empty() {
            out.insert(table, text);
        }
    }
    out
}

#[test]
fn every_text_column_has_its_class_guard_at_insert_update_and_open_with_exact_counts() {
    let dir = tempfile::tempdir().unwrap();
    let book = Book::create(&dir.path().join("inventory.fbbook"), &[61; 32]).unwrap();
    let live = live_text_columns(&book.conn);
    // The checked-in generator inventory agrees with the live schema and the Rust rule.
    let fixture: serde_json::Value = serde_json::from_str(include_str!("../tests/fb_value_guard_inventory.json")).unwrap();
    let fixture: BTreeMap<String, BTreeMap<String, Class>> = fixture.as_object().unwrap().iter().map(|(t, cols)| {
        (t.clone(), cols.as_object().unwrap().iter().map(|(c, k)| (c.clone(), match k.as_str().unwrap() {
            "uuid" => Class::Uuid, "date" => Class::Date, "timestamp" => Class::Timestamp, "text" => Class::Text, other => panic!("{other}"),
        })).collect())
    }).collect();
    assert_eq!(fixture, live, "regenerate the extension with tests/generate_company_document_guards.py");
    let count = |class: Class| live.values().flat_map(|c| c.values()).filter(|k| **k == class).count();
    assert_eq!(live.len(), 87);
    assert_eq!(live.values().map(BTreeMap::len).sum::<usize>(), 590);
    assert_eq!((count(Class::Uuid), count(Class::Date), count(Class::Timestamp), count(Class::Text)), (309, 26, 24, 231));
    let guards: Vec<&str> = value_guard_triggers().collect();
    assert_eq!(guards.len(), 174);
    let view: String = book.conn.query_row("SELECT sql FROM sqlite_schema WHERE name='fb_value_invalid'", [], |r| r.get(0)).unwrap();
    for (table, columns) in &live {
        for event in ["insert", "update"] {
            let name = format!("fb_valid_{table}_{event}");
            assert!(guards.contains(&name.as_str()), "{name}");
            let sql: String = book.conn.query_row("SELECT sql FROM sqlite_schema WHERE type='trigger' AND name=?1", [&name], |r| r.get(0)).unwrap();
            for (column, class) in columns {
                assert!(sql.contains(&format!("NOT {}(NEW.{column})", class.function())), "{name} {column}");
            }
        }
        let arm = view.split("UNION ALL").find(|arm| arm.contains(&format!("FROM {table} WHERE"))).unwrap_or_else(|| panic!("view arm {table}"));
        for (column, class) in columns {
            assert!(arm.contains(&format!("NOT {}({column})", class.function())), "view {table}.{column}");
        }
    }
    assert!(VIOLATION_QUERIES.iter().any(|(_, q)| *q == "SELECT problem FROM fb_value_invalid"));
}

/// Each class's hole is refused at write by the shared guard alone, and flagged on open when written around it.
#[test]
fn each_value_class_is_refused_at_write_and_flagged_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("probe.fbbook");
    let key = [62; 32];
    let user = "00000000-0000-7000-8000-000000000101";
    let company = "00000000-0000-7000-8000-000000000102";
    let journal = "00000000-0000-7000-8000-0000000001ab";
    let ts = "2026-01-01T00:00:00.000000Z";
    {
        let book = Book::create(&path, &key).unwrap();
        book.conn.execute_batch(&format!("PRAGMA foreign_keys=OFF; BEGIN;
            INSERT INTO fb_user(id,username) VALUES('{user}','owner');
            INSERT INTO ledger_company(id,name,currency) VALUES('{company}','Probe','USD');
            INSERT INTO ledger_journal(id,number,company_id,date,memo,status,created_by_id,created_at) VALUES('{journal}',1,'{company}','2026-01-01','Probe','DRAFT','{user}','{ts}');
            COMMIT; PRAGMA foreign_keys=ON;")).unwrap();
    }
    let cases: [(&str, &str, String); 9] = [
        ("ledger_company", "name", "'Probe'||char(0)||'x'".into()),
        ("ledger_journal", "memo", "'Probe'||char(0)".into()),
        ("ledger_journal", "id", format!("upper('{journal}')")),
        ("ledger_journal", "company_id", format!("'{company}'||char(0)||'x'")),
        ("ledger_journal", "date", "'2026-02-30'".into()),
        ("ledger_journal", "date", "'0000-12-31'".into()),
        ("ledger_journal", "date", "'2026-01-01'||char(0)".into()),
        ("ledger_journal", "created_at", "'2026-01-01T23:59:60.000000Z'".into()),
        ("ledger_journal", "created_at", "'2026-13-01T00:00:00Z'".into()),
    ];
    for (table, column, value) in cases {
        let mut conn = Connection::open(&path).unwrap();
        conn.execute_batch(&format!("PRAGMA key=\"x'{}'\"", hex(&key))).unwrap();
        Book::configure(&conn).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA ignore_check_constraints=ON").unwrap();
        let tx = conn.transaction().unwrap();
        // Only the shared guard remains, so a refusal can only come from it.
        let triggers: Vec<(String, String)> = collect(&tx, "SELECT name,sql FROM sqlite_schema WHERE type='trigger'", |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        for (name, _) in &triggers {
            if name != &format!("fb_valid_{table}_update") {
                tx.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
            }
        }
        let error = tx.execute_batch(&format!("UPDATE {table} SET {column}={value}")).err().unwrap_or_else(|| panic!("{table}.{column}={value} was accepted"));
        assert_eq!(constraint_of(&error).as_deref(), Some("fb_valid"), "{table}.{column}={value}: {error}");
        tx.execute_batch(&format!("DROP TRIGGER fb_valid_{table}_update; UPDATE {table} SET {column}={value}")).unwrap();
        for (_, sql) in &triggers {
            tx.execute_batch(sql).unwrap();
        }
        let problems: Vec<String> = collect(&tx, "SELECT problem FROM fb_value_invalid", |r| r.get(0)).unwrap();
        assert_eq!(problems, [format!("invalid stored value in {table}")], "{table}.{column}={value}");
        tx.commit().unwrap();
        drop(conn);
        match Book::open(&path, &key) {
            Err(DbError::Integrity(p)) => assert!(p.iter().any(|p| p.contains(&format!("invalid stored value in {table}"))), "{p:?}"),
            other => panic!("{table}.{column}={value} reopened: {:?}", other.map(|_| ())),
        }
        // Restore the clean row for the next case.
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&format!("PRAGMA key=\"x'{}'\"", hex(&key))).unwrap();
        Book::configure(&conn).unwrap();
        let triggers: Vec<(String, String)> = collect(&conn, "SELECT name,sql FROM sqlite_schema WHERE type='trigger'", |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA ignore_check_constraints=ON; BEGIN").unwrap();
        for (name, _) in &triggers {
            conn.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
        }
        conn.execute_batch(&format!("UPDATE ledger_company SET name='Probe'; UPDATE ledger_journal SET id='{journal}', company_id='{company}', memo='Probe', date='2026-01-01', created_at='{ts}'")).unwrap();
        for (_, sql) in &triggers {
            conn.execute_batch(sql).unwrap();
        }
        conn.execute_batch("COMMIT").unwrap();
        drop(conn);
        assert!(Book::open(&path, &key).unwrap().check().unwrap().is_empty());
    }
    // An insert path, through an authorized writer: the guard refuses a NUL company name outright.
    let mut book = Book::open(&path, &key).unwrap();
    let error = book.write(|tx| tx.execute_batch("INSERT INTO ledger_company(id,name,currency) VALUES('00000000-0000-7000-8000-000000000104','x'||char(0),'USD')")).unwrap_err();
    assert_eq!(constraint_of(&error).as_deref(), Some("fb_valid"), "{error}");
}
