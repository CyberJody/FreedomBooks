use super::*;

fn book(label: &str) -> Book {
    Book::create(&tests::temp(label), &[27; 32]).unwrap()
}
fn denied_statements() -> Vec<&'static str> {
    vec![
        "BEGIN",
        "BEGIN IMMEDIATE",
        "COMMIT",
        "END",
        "ROLLBACK",
        "SAVEPOINT x",
        "RELEASE x",
        "ROLLBACK TO x",
        "PRAGMA recursive_triggers=OFF",
        "PRAGMA recursive_triggers=OFF; PRAGMA recursive_triggers=ON",
        "PRAGMA defer_foreign_keys=ON",
        "PRAGMA defer_foreign_keys=OFF",
        "PRAGMA writable_schema=ON",
        "PRAGMA writable_schema=RESET",
        "PRAGMA query_only=OFF",
        "PRAGMA query_only=ON",
        "PRAGMA foreign_keys=OFF",
        "PRAGMA ignore_check_constraints=ON",
        "PRAGMA synchronous=OFF",
        "PRAGMA user_version=999",
        "PRAGMA schema_version=999",
        "PRAGMA journal_mode=DELETE",
        "PRAGMA trusted_schema=ON",
        "PRAGMA legacy_alter_table=ON",
        "PRAGMA analysis_limit=1",
        "PRAGMA optimize",
        "PRAGMA wal_checkpoint(TRUNCATE)",
        "CREATE TABLE evil(x)",
        "CREATE TEMP TABLE evil(x)",
        "CREATE VIEW evil AS SELECT 1",
        "CREATE TEMP VIEW evil AS SELECT 1",
        "CREATE TRIGGER evil AFTER INSERT ON fb_seal BEGIN SELECT 1; END",
        "CREATE TEMP TRIGGER evil AFTER INSERT ON fb_seal BEGIN SELECT 1; END",
        "CREATE INDEX evil ON fb_seal(doc_id)",
        "ALTER TABLE fb_seal ADD COLUMN evil TEXT",
        "DROP TABLE fb_seal",
        "DROP INDEX journal_entry_number_per_company",
        "DROP TRIGGER fb_audit_no_update",
        "CREATE VIRTUAL TABLE evil USING fts5(x)",
        "ATTACH ':memory:' AS evil",
        "DETACH temp",
        "ANALYZE",
        "REINDEX",
        "SELECT sqlcipher_export('evil')",
        "SELECT load_extension('evil')",
    ]
}

#[test]
fn authorizer_denies_each_unsafe_operation_and_preserves_write_atomicity() {
    let mut book = book("authorize-write");
    for sql in denied_statements() {
        let result:rusqlite::Result<()>=book.write(|tx| {
            tx.execute_batch("INSERT INTO fb_seal VALUES('marker','rollback')")?;
            let error=tx.execute_batch(sql).expect_err(sql);
            // Authorizer refusal rather than a chance syntax/foreign-key/state failure.
            assert!(matches!(&error,rusqlite::Error::SqliteFailure(code,_) if code.code==rusqlite::ErrorCode::AuthorizationForStatementDenied) || error.to_string().contains("not authorized to use function:"),"{sql}: {error}");
            Err(error)
        });
        assert!(result.is_err(), "{sql}");
        assert!(book.conn.is_autocommit(), "{sql}");
        assert_eq!(
            book.conn
                .query_row("SELECT count(*) FROM fb_seal", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0,
            "{sql}"
        );
    }
    assert!(book.check().unwrap().is_empty());
}

#[test]
fn read_authorizer_denies_control_and_all_dml() {
    let mut book = book("authorize-read");
    for sql in denied_statements().into_iter().chain([
        "INSERT INTO fb_seal VALUES('read','write')",
        "UPDATE fb_seal SET kind='changed'",
        "DELETE FROM fb_seal",
    ]) {
        let result:rusqlite::Result<()>=book.read(|tx| {
            let error=tx.execute_batch(sql).expect_err(sql);
            assert!(matches!(&error,rusqlite::Error::SqliteFailure(code,_) if code.code==rusqlite::ErrorCode::AuthorizationForStatementDenied) || error.to_string().contains("not authorized to use function:"),"{sql}: {error}");
            Err(error)
        });
        assert!(result.is_err());
        assert!(book.conn.is_autocommit());
        assert_eq!(
            book.conn
                .query_row("PRAGMA query_only", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn authorizer_is_active_for_trigger_and_view_bodies() {
    let mut book = book("authorize-view");
    book.conn.execute_batch("CREATE TEMP VIEW seal_view AS SELECT * FROM fb_seal;
        CREATE TEMP TRIGGER view_insert INSTEAD OF INSERT ON seal_view BEGIN INSERT INTO fb_seal VALUES(NEW.kind,NEW.doc_id); END").unwrap();
    let result: rusqlite::Result<()> = book.read(|tx| {
        tx.execute_batch("INSERT INTO seal_view VALUES('read','escape')")?;
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(
        book.conn
            .query_row("SELECT count(*) FROM fb_seal", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn callback_error_and_panic_reset_authorization_before_library_rollback() {
    let mut book = book("authorize-unwind");
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: rusqlite::Result<()> = book.write(|tx| {
            tx.execute_batch("INSERT INTO fb_seal VALUES('panic','rollback')")?;
            panic!("injected callback panic")
        });
    }));
    assert!(panic.is_err());
    assert!(book.conn.is_autocommit());
    assert_eq!(
        book.conn
            .query_row("SELECT count(*) FROM fb_seal", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    book.write(|tx| -> rusqlite::Result<()> {
        tx.execute_batch("INSERT INTO fb_seal VALUES('after','works')")?;
        let on: i64 = tx.query_row("PRAGMA recursive_triggers", [], |r| r.get(0))?;
        assert_eq!(on, 1);
        Ok(())
    })
    .unwrap();
    book.read(|tx| -> rusqlite::Result<()> {
        assert_eq!(
            tx.query_row("SELECT count(*) FROM fb_seal", [], |r| r.get::<_, i64>(0))?,
            1
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn exact_literal_internal_prefix_includes_all_near_reserved_names() {
    for (i, name) in [
        "sqliteXskip_audit",
        "sqlite-skip_audit",
        "SQLiteXskip_audit",
        "SQLITE-skip_audit",
        "sQlItExskip_audit",
    ]
    .into_iter()
    .enumerate()
    {
        let path = tests::temp(&format!("near-reserved-{i}"));
        let key = [28; 32];
        let book = Book::create(&path, &key).unwrap();
        book.conn.execute_batch(&format!("CREATE TRIGGER \"{name}\" BEFORE INSERT ON ledger_auditevent BEGIN SELECT RAISE(IGNORE); END")).unwrap();
        assert!(
            book.check().unwrap().iter().any(|s| s.contains(name)),
            "{name}"
        );
        drop(book);
        assert!(
            matches!(Book::open(&path, &key), Err(DbError::Integrity(_))),
            "{name}"
        );
    }
}

#[test]
fn populated_v2_copy_migration_preserves_posted_history_reversals_members_and_seals() {
    populated_copy_migration(2);
}

#[test]
fn populated_v3_migration_preserves_posted_history_reversals_members_and_seals() {
    populated_copy_migration(3);
}

fn populated_copy_migration(prior_version: usize) {
    let path = tests::temp(&format!("copy-migration-v{prior_version}"));
    let key = [29; 32];
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&format!("PRAGMA key=\"x'{}'\"", hex(&key)))
        .unwrap();
    Book::configure(&conn).unwrap();
    for (_, sql) in &MIGRATIONS[..prior_version] {
        conn.execute_batch(sql).unwrap();
    }
    conn.execute_batch(&format!("PRAGMA user_version={prior_version}"))
        .unwrap();
    // Remove guards solely to install a complete synthetic historical fixture; definitions
    // are restored byte-for-byte before Book::open validates and migrates the prior schema.
    let triggers: Vec<(String, String)> = conn
        .prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger'")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let tables = [
        "fb_user",
        "ledger_company",
        "ledger_company_members",
        "ledger_account",
        "ledger_auditevent",
        "ledger_journal",
        "ledger_journalline",
        "fb_seal",
        "fb_seal_due",
    ];
    let tx = conn.unchecked_transaction().unwrap();
    for (name, _) in &triggers {
        tx.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
    }
    let u = "00000000-0000-7000-8000-000000000001";
    let c = "00000000-0000-7000-8000-000000000002";
    let a = "00000000-0000-7000-8000-000000000003";
    let b = "00000000-0000-7000-8000-000000000004";
    let j = "00000000-0000-7000-8000-000000000005";
    let rev = "00000000-0000-7000-8000-000000000006";
    tx.execute_batch(&format!("INSERT INTO fb_user(id,username) VALUES('{u}','owner');
      INSERT INTO ledger_company(id,name,currency) VALUES('{c}','Copy fixture','USD'); INSERT INTO ledger_company_members VALUES('{c}','{u}');
      INSERT INTO ledger_account(id,company_id,code,name,kind,detail_type) VALUES('{a}','{c}','1','Cash','ASSET','BANK'),('{b}','{c}','2','Capital','EQUITY','EQUITY');
      INSERT INTO ledger_journal(id,number,company_id,date,memo,status,created_by_id,created_at,posted_by_id,posted_at,entry_number) VALUES('{j}',1,'{c}','2026-01-01','Posted','POSTED','{u}','2026-01-01T00:00:00.000000Z','{u}','2026-01-01T00:00:00.000000Z','JE1');
      INSERT INTO ledger_journal(id,number,company_id,date,memo,status,created_by_id,created_at,posted_by_id,posted_at,reversal_of_id) VALUES('{rev}',2,'{c}','2026-01-02','Reversal','POSTED','{u}','2026-01-01T00:00:00.000000Z','{u}','2026-01-01T00:00:00.000000Z','{j}');
      INSERT INTO ledger_journalline VALUES('00000000-0000-7000-8000-000000000011','{j}','{a}',1,100,0,'line','name'),('00000000-0000-7000-8000-000000000012','{j}','{b}',2,0,100,'',''),('00000000-0000-7000-8000-000000000013','{rev}','{a}',1,0,100,'line','name'),('00000000-0000-7000-8000-000000000014','{rev}','{b}',2,100,0,'','');
      INSERT INTO ledger_auditevent VALUES('00000000-0000-7000-8000-000000000015','{c}','{u}','owner','2026-01-01T00:00:00.000000Z','journal.posted','journal:{j}','{{\"amount\":\"1.00\"}}');
      INSERT INTO fb_seal VALUES('journal','{j}');INSERT INTO fb_seal_due VALUES('journal','{j}');")).unwrap();
    for (_, sql) in &triggers {
        tx.execute_batch(sql).unwrap();
    }
    tx.commit().unwrap();
    let dump = |conn: &Connection, table: &str| -> Vec<Vec<rusqlite::types::Value>> {
        let mut stmt = conn
            .prepare(&format!("SELECT * FROM {table} ORDER BY 1,2"))
            .unwrap();
        // L2a appends the nullable source_id column; compare every pre-L2a
        // column exactly and check the new column independently below.
        let n = stmt.column_count() - usize::from(table == "ledger_journal" && stmt.column_names().any(|name| name == "source_id"));
        stmt.query_map([], |row| (0..n).map(|i| row.get(i)).collect())
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    let before: Vec<_> = tables.iter().map(|t| dump(&conn, t)).collect();
    drop(conn);
    let book = Book::open(&path, &key).unwrap();
    assert!(book.check().unwrap().is_empty());
    for (t, expected) in tables.iter().zip(before) {
        assert_eq!(dump(&book.conn, t), expected, "{t}");
    }
    assert_eq!(book.conn.query_row("SELECT count(*) FROM ledger_journal WHERE source_id IS NOT NULL", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    assert_eq!(
        book.conn
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        MIGRATIONS.len() as i64
    );
    drop(book);
    assert!(Book::open(&path, &key).unwrap().check().unwrap().is_empty());
}

#[test]
fn read_callback_panic_restores_query_only_and_the_library_transaction_boundary() {
    let mut book = book("read-unwind");
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: rusqlite::Result<()> = book.read(|_| panic!("injected read panic"));
    }));
    assert!(panic.is_err());
    assert!(book.conn.is_autocommit());
    assert_eq!(
        book.conn
            .query_row("PRAGMA query_only", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    book.write(|tx| -> rusqlite::Result<()> {
        tx.execute_batch("INSERT INTO fb_seal VALUES('after-read','works')")
    })
    .unwrap();
}

#[test]
fn every_permanent_identity_guard_definition_is_hashed_and_required() {
    let sql = [MIGRATIONS[6].1, MIGRATIONS[7].1].concat();
    let names: Vec<&str> = sql
        .lines()
        .filter_map(|line| line.strip_prefix("CREATE TRIGGER "))
        .map(|line| line.split_whitespace().next().unwrap())
        .collect();
    assert_eq!(names.len(), 35);
    for name in names {
        assert!(REQUIRED_TRIGGERS.contains(&name), "{name}");
        for replace in [false, true] {
            let path = tests::temp(&format!("permanent-definition-{name}-{replace}"));
            let key = [31; 32];
            let book = Book::create(&path, &key).unwrap();
            book.conn
                .execute_batch(&format!("DROP TRIGGER {name}"))
                .unwrap();
            if replace {
                book.conn
                    .execute_batch(&format!(
                        "CREATE TRIGGER {name} BEFORE UPDATE ON fb_user BEGIN SELECT 1; END"
                    ))
                    .unwrap();
            }
            assert!(
                book.check()
                    .unwrap()
                    .iter()
                    .any(|p| p.contains(&format!("schema definition mismatch: trigger:{name}")))
            );
            drop(book);
            assert!(
                matches!(Book::open(&path, &key), Err(DbError::Integrity(_))),
                "{name}"
            );
        }
    }
}
