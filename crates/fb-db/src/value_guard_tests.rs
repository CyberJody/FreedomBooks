//! Migration 0130: every TEXT column of the live schema carries the shared value guard for its class,
//! at insert, update and open, with exact counts (a later table, e.g. bank feeds at INT-L4b, cannot be missed).
use super::*;
use rusqlite::params;
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
    assert_eq!(fixture, live, "review and update the shared value-guard inventory for the new schema");
    let count = |class: Class| live.values().flat_map(|c| c.values()).filter(|k| **k == class).count();
    assert_eq!(live.len(), 90);
    assert_eq!(live.values().map(BTreeMap::len).sum::<usize>(), 595);
    assert_eq!((count(Class::Uuid), count(Class::Date), count(Class::Timestamp), count(Class::Text)), (312, 26, 24, 233));
    let guards: Vec<&str> = value_guard_triggers().collect();
    assert_eq!(guards.len(), 180);
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

#[test]
fn desktop_user_insert_values_are_checked_before_other_constraints() {
    let dir = tempfile::tempdir().unwrap();
    let mut book = Book::create(&dir.path().join("desktop-values.fbbook"), &[64;32]).unwrap();
    let triggers: Vec<String> = collect(&book.conn, "SELECT name FROM sqlite_schema WHERE type='trigger'", |r| r.get(0)).unwrap();
    book.conn.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA ignore_check_constraints=ON").unwrap();
    for (user, role, hash) in [
        ("not-a-uuid", "bookkeeper", None),
        ("00000000-0000-7000-8000-000000000101", "bookkeeper\0", None),
        ("00000000-0000-7000-8000-000000000101", "bookkeeper", Some("$argon2id$v=19$\0")),
    ] {
        let tx = book.conn.transaction().unwrap();
        for name in &triggers { if name != "fb_valid_desktop_user_insert" { tx.execute_batch(&format!("DROP TRIGGER {name}")).unwrap(); } }
        let error = tx.execute("INSERT INTO desktop_user(user_id,role,password_hash) VALUES(?1,?2,?3)", params![user,role,hash]).unwrap_err();
        assert_eq!(constraint_of(&error).as_deref(), Some("fb_valid"));
        assert_eq!(tx.query_row("SELECT count(*) FROM desktop_user", [], |r| r.get::<_,i64>(0)).unwrap(), 0);
        // Rollback also restores every other guard for the next case.
        tx.rollback().unwrap();
    }
}

#[test]
fn desktop_value_guard_upgrade_preserves_enrolled_users_and_prior_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("enrolled-0185.fbbook");
    let key = [65;32];
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&format!("PRAGMA key=\"x'{}'\"", hex(&key))).unwrap();
    Book::configure(&conn).unwrap();
    let boundary = MIGRATIONS.iter().position(|m| m.0 == "0186_desktop_user_value_guards").unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    for (_, sql) in &MIGRATIONS[..boundary] { conn.execute_batch(sql).unwrap(); }
    let admin = "00000000-0000-7000-8000-000000000101";
    let reader = "00000000-0000-7000-8000-000000000102";
    let company = "00000000-0000-7000-8000-000000000103";
    // Synthetic stored hashes: test preservation, not Argon2 authentication (covered in fb-app).
    let hash = format!("$argon2id$v=19${}", "synthetic".repeat(10));
    conn.execute("INSERT INTO fb_user(id,username,is_staff) VALUES(?1,'Administrator',1),(?2,'Reader',0)", params![admin,reader]).unwrap();
    conn.execute("INSERT INTO ledger_company(id,name,currency) VALUES(?1,'Synthetic named users','USD')", [company]).unwrap();
    conn.execute("INSERT INTO ledger_company_members(company_id,user_id) VALUES(?1,?2),(?1,?3)", params![company,admin,reader]).unwrap();
    conn.execute("INSERT INTO desktop_user(user_id,role,password_hash) VALUES(?1,'administrator',?3),(?2,'readonly',?3)", params![admin,reader,hash]).unwrap();
    conn.execute_batch(&format!("PRAGMA user_version={boundary}; COMMIT")).unwrap();
    let before = test_support::snapshot(&conn).unwrap();
    let schema = schema_hashes(&conn).unwrap();
    let view: String = conn.query_row("SELECT sql FROM sqlite_schema WHERE name='fb_value_invalid'", [], |r| r.get(0)).unwrap();
    drop(conn);
    let old = match Book::open_staged(&path,&key).unwrap() { Opened::Behind(old) => old, _ => panic!("expected 0185 book") };
    let backup = dir.path().join("before-upgrade.fbbook");
    old.snapshot(&backup,&key).unwrap();
    let copy = Connection::open(&backup).unwrap();
    copy.execute_batch(&format!("PRAGMA key=\"x'{}'\"",hex(&key))).unwrap();
    Book::configure(&copy).unwrap();
    assert_eq!(test_support::snapshot(&copy).unwrap(), before);
    assert_eq!(schema_hashes(&copy).unwrap(), schema);
    let book = old.upgrade().unwrap();
    let mut expected=before.clone();
    expected.insert("desktop_mfa_totp".into(),vec![]);
    expected.insert("desktop_mfa_recovery".into(),vec![]);
    assert_eq!(test_support::snapshot(&book.conn).unwrap(), expected);
    let upgraded = schema_hashes(&book.conn).unwrap();
    for (object, hash) in &schema { if object != "view:fb_value_invalid" { assert_eq!(&upgraded[object],hash,"changed {object}"); } }
    let added:std::collections::BTreeSet<_>=upgraded.keys().filter(|k|!schema.contains_key(*k)).map(String::as_str).collect();
    assert_eq!(added,std::collections::BTreeSet::from([
        "trigger:fb_valid_desktop_user_insert","trigger:fb_valid_desktop_user_update",
        "table:desktop_mfa_totp","table:desktop_mfa_recovery","trigger:desktop_mfa_totp_identity",
        "trigger:fb_valid_desktop_mfa_totp_insert","trigger:fb_valid_desktop_mfa_totp_update",
        "trigger:fb_valid_desktop_mfa_recovery_insert","trigger:fb_valid_desktop_mfa_recovery_update",
    ]));
    let new_view: String = book.conn.query_row("SELECT sql FROM sqlite_schema WHERE name='fb_value_invalid'", [], |r| r.get(0)).unwrap();
    assert_eq!(new_view.split("\nUNION ALL\nSELECT 'invalid stored value in desktop_user'").next().unwrap(), view.trim_end_matches(';'));
    assert!(book.check().unwrap().is_empty());
    drop(book);
    let reopened = Book::open(&path,&key).unwrap();
    assert_eq!(test_support::snapshot(&reopened.conn).unwrap(), expected);
    assert!(reopened.check().unwrap().is_empty());
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
    // Unconfigured named users still need the same byte-exact storage boundaries.
    {
        let book = Book::open(&path, &key).unwrap();
        book.conn.execute("INSERT INTO desktop_user(user_id,role) VALUES(?1,'bookkeeper')", [user]).unwrap();
    }
    let cases: [(&str, &str, String); 12] = [
        ("desktop_user", "user_id", format!("'{user}'||char(0)")),
        ("desktop_user", "role", "'bookkeeper'||char(0)".into()),
        ("desktop_user", "password_hash", "'$argon2id$v=19$'||printf('%080d',0)||char(0)".into()),
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
        conn.execute_batch(&format!("UPDATE desktop_user SET user_id='{user}',role='bookkeeper',password_hash=NULL; UPDATE ledger_company SET name='Probe'; UPDATE ledger_journal SET id='{journal}', company_id='{company}', memo='Probe', date='2026-01-01', created_at='{ts}'")).unwrap();
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
