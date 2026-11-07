//! R-L1b reviewer probes converted from reproduction to regression expectations.
mod common;
use common::*;
use fb_core::{AccountId, Book, reports, users};
use fb_db::rusqlite::{self, Connection};

fn fixture(tag: &str) -> (Fx, std::path::PathBuf) {
    let path = book_path(tag);
    let mut book = Book::create(&path, &KEY).unwrap();
    let user = users::create_user(&mut book, "tester", false).unwrap();
    (Fx { book, user }, path)
}
fn account_swap(source: AccountId, dest: AccountId) -> String {
    format!(
        "UPDATE OR REPLACE ledger_account SET id='{dest}',code='1000',kind='INCOME',detail_type='INCOME' WHERE id='{source}'"
    )
}
fn text(conn: &Connection, sql: &str) -> String {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn update_or_replace_target_guard_is_bypassed_with_recursion_off() {
    for recursive in [true, false] {
        let (mut fx, path) = fixture("update-replace");
        let (c, a, b) = fx.chart();
        let j = fx.draft(c, a, b, "10", "2026-01-01");
        fx.post(c, j.id);
        let spare = fx.account(c, "spare", "Unused", "INCOME");
        fx.book
            .connection()
            .execute_batch(&format!(
                "PRAGMA recursive_triggers={}",
                i32::from(recursive)
            ))
            .unwrap();
        let result = fx.book.connection().execute_batch(&account_swap(spare, a));
        assert!(
            result.is_err(),
            "protected replacement victim was displaced"
        );
        fx.book
            .connection()
            .execute_batch("PRAGMA recursive_triggers=ON")
            .unwrap();
        assert_eq!(
            reports::register(&mut fx.book, fx.user, c, a, None, None)
                .unwrap()
                .closing
                .to_string(),
            "10.00"
        );
        assert!(fx.book.check().unwrap().is_empty());
        drop(fx);
        assert!(Book::open(&path, &KEY).is_ok());
    }
}

#[test]
fn rowid_replace_deletes_audit_without_colliding_on_declared_keys() {
    for recursive in [true, false] {
        let (mut fx, path) = fixture("rowid");
        fx.chart();
        let old: String = text(
            fx.book.connection(),
            "SELECT id FROM ledger_auditevent ORDER BY id LIMIT 1",
        );
        let id = fb_ids::AuditEventId::new();
        fx.book
            .connection()
            .execute_batch(&format!(
                "PRAGMA recursive_triggers={}",
                i32::from(recursive)
            ))
            .unwrap();
        let sql = format!(
            "INSERT OR REPLACE INTO ledger_auditevent(rowid,id,company_id,actor_id,actor_name,occurred_at,action,target,detail) SELECT rowid,'{id}',company_id,actor_id,actor_name,occurred_at,action,target,'{{\"rewritten\":true}}' FROM ledger_auditevent WHERE id='{old}'"
        );
        let result = fx.book.connection().execute_batch(&sql);
        assert!(result.is_err(), "hidden rowid displaced audit");
        assert_eq!(
            fx.count(&format!(
                "SELECT count(*) FROM ledger_auditevent WHERE id='{old}'"
            )),
            1
        );
        fx.book
            .connection()
            .execute_batch("PRAGMA recursive_triggers=ON")
            .unwrap();
        assert!(fx.book.check().unwrap().is_empty());
        drop(fx);
        assert!(Book::open(&path, &KEY).is_ok());
    }
}

#[test]
fn callback_can_toggle_and_restore_recursive_triggers() {
    let (mut fx, path) = fixture("callback-toggle");
    let (c, a, b) = fx.chart();
    let j = fx.draft(c, a, b, "10", "2026-01-01");
    fx.post(c, j.id);
    let spare = fx.account(c, "spare", "Unused", "INCOME");
    let result: rusqlite::Result<()> = fx.book.write(|tx| {
        tx.execute_batch(&format!(
            "PRAGMA recursive_triggers=OFF; {}; PRAGMA recursive_triggers=ON",
            account_swap(spare, a)
        ))?;
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(
        reports::register(&mut fx.book, fx.user, c, a, None, None)
            .unwrap()
            .closing
            .to_string(),
        "10.00"
    );
    assert!(fx.book.check().unwrap().is_empty());
    drop(fx);
    assert!(Book::open(&path, &KEY).is_ok());
}

#[test]
fn writer_callback_can_end_transaction_and_leave_committed_work_on_error() {
    let (mut fx, path) = fixture("early-commit");
    let result: rusqlite::Result<()> = fx.book.write(|tx| {
        tx.execute_batch("INSERT INTO fb_seal VALUES('early','committed'); COMMIT;")?;
        Err(rusqlite::Error::InvalidQuery)
    });
    assert!(result.is_err());
    assert_eq!(
        fx.count("SELECT count(*) FROM fb_seal WHERE kind='early'"),
        0
    );
    drop(fx);
    let book = Book::open(&path, &KEY).unwrap();
    assert_eq!(
        book.connection()
            .query_row("SELECT count(*) FROM fb_seal WHERE kind='early'", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn standard_upserts_and_view_writes_are_refused() {
    let (mut fx, _) = fixture("upsert-view");
    let (c, a, b) = fx.chart();
    let j = fx.draft(c, a, b, "10", "2026-01-01");
    fx.post(c, j.id);
    let spare = fx.account(c, "spare", "Unused", "INCOME");
    fx.book.connection().execute_batch("CREATE TEMP VIEW audit_view AS SELECT * FROM main.ledger_auditevent;
    CREATE TEMP TRIGGER audit_view_update INSTEAD OF UPDATE ON audit_view BEGIN UPDATE ledger_auditevent SET detail=NEW.detail WHERE id=OLD.id; END;
    CREATE TEMP VIEW account_view AS SELECT * FROM main.ledger_account;
    CREATE TEMP TRIGGER account_view_update INSTEAD OF UPDATE ON account_view BEGIN UPDATE ledger_account SET id=NEW.id,code=NEW.code,kind=NEW.kind,detail_type=NEW.detail_type WHERE id=OLD.id; END;").unwrap();
    for recursive in [true, false] {
        fx.book
            .connection()
            .execute_batch(&format!(
                "PRAGMA recursive_triggers={}",
                i32::from(recursive)
            ))
            .unwrap();
        let new_id = fb_ids::AuditEventId::new();
        for sql in [
            format!(
                "INSERT INTO ledger_company(id,name,currency) VALUES('{c}','Changed','EUR') ON CONFLICT(id) DO UPDATE SET currency=excluded.currency"
            ),
            format!(
                "INSERT INTO ledger_account(id,company_id,code,name,kind,detail_type) VALUES('{a}','{c}','1000','Changed','INCOME','INCOME') ON CONFLICT(id) DO UPDATE SET kind=excluded.kind,detail_type=excluded.detail_type"
            ),
            format!(
                "INSERT INTO ledger_auditevent(rowid,id,company_id,actor_id,actor_name,occurred_at,action,target,detail) SELECT rowid,'{new_id}',company_id,actor_id,actor_name,occurred_at,action,target,'{{}}' FROM ledger_auditevent WHERE 1 LIMIT 1 ON CONFLICT(rowid) DO UPDATE SET detail='{{}}'"
            ),
            "UPDATE audit_view SET detail='{\"changed\":true}'".into(),
            format!("UPDATE account_view SET kind='INCOME',detail_type='INCOME' WHERE id='{a}'"),
        ] {
            assert!(attack(&fx.book, &sql).is_some(), "{sql}");
        }
        let bypass = format!(
            "UPDATE OR REPLACE account_view SET id='{a}',code='1000',kind='INCOME',detail_type='INCOME' WHERE id='{spare}'"
        );
        assert!(attack(&fx.book, &bypass).is_some());
    }
}

#[test]
fn attached_book_guards_resolve_in_the_attached_schema() {
    let (mut fx, _) = fixture("attach-main");
    let (c, a, b) = fx.chart();
    let j = fx.draft(c, a, b, "10", "2026-01-01");
    fx.post(c, j.id);
    let (mut other, path) = fixture("attach-other");
    let (oc, oa, ob) = other.chart();
    let oj = other.draft(oc, oa, ob, "20", "2026-01-01");
    other.post(oc, oj.id);
    drop(other);
    let key: String = KEY.iter().map(|b| format!("{b:02x}")).collect();
    fx.book
        .connection()
        .execute_batch(&format!(
            "ATTACH DATABASE '{}' AS aux KEY \"x'{key}'\"",
            path.display()
        ))
        .unwrap();
    for recursive in [true, false] {
        fx.book
            .connection()
            .execute_batch(&format!(
                "PRAGMA recursive_triggers={}",
                i32::from(recursive)
            ))
            .unwrap();
        for sql in [
            format!(
                "UPDATE aux.ledger_journal SET memo='changed' WHERE id='{}'",
                oj.id
            ),
            format!(
                "UPDATE aux.ledger_account SET kind='INCOME',detail_type='INCOME' WHERE id='{oa}'"
            ),
            "UPDATE aux.ledger_auditevent SET detail='{}'".into(),
            format!(
                "INSERT INTO aux.ledger_journalline(id,journal_id,account_id,position,debit) VALUES('{}','{}','{a}',3,100)",
                fb_ids::JournalLineId::new(),
                oj.id
            ),
        ] {
            assert!(attack(&fx.book, &sql).is_some(), "{sql}");
        }
    }
    fx.book
        .connection()
        .execute_batch("PRAGMA recursive_triggers=ON; DETACH DATABASE aux")
        .unwrap();
    assert!(fx.book.check().unwrap().is_empty());
    assert!(Book::open(&path, &KEY).is_ok());
}

#[test]
fn writable_schema_and_trigger_changes_are_detected_unless_restored() {
    for mode in ["drop", "noop", "writable"] {
        let (mut fx, path) = fixture(mode);
        fx.chart();
        match mode {
            "drop"=>fx.book.connection().execute_batch("DROP TRIGGER fb_audit_no_update").unwrap(),
            "noop"=>fx.book.connection().execute_batch("DROP TRIGGER fb_audit_no_update; CREATE TRIGGER fb_audit_no_update BEFORE UPDATE ON ledger_auditevent BEGIN SELECT 1; END").unwrap(),
            _=>fx.book.connection().execute_batch("PRAGMA writable_schema=ON; UPDATE sqlite_master SET sql=replace(sql,'SELECT RAISE(ABORT, ''fb_audit_guard: Audit events are append-only'')','SELECT 1') WHERE name='fb_audit_no_update'; PRAGMA writable_schema=OFF").unwrap(),
        }
        drop(fx);
        assert!(Book::open(&path, &KEY).is_err(), "{mode}");
    }
    let (mut fx, path) = fixture("writable-restored");
    fx.chart();
    let sql = text(
        fx.book.connection(),
        "SELECT sql FROM sqlite_master WHERE name='fb_audit_no_update'",
    );
    let result:rusqlite::Result<()>=fx.book.write(|tx| {
        tx.execute_batch("PRAGMA writable_schema=ON; DELETE FROM sqlite_master WHERE name='fb_audit_no_update'; PRAGMA writable_schema=RESET;")?;
        tx.execute_batch("UPDATE ledger_auditevent SET detail='{\"rewritten\":true}'")?;
        tx.execute_batch(&sql)?;
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(
        fx.count("SELECT count(*) FROM ledger_auditevent WHERE detail='{\"rewritten\":true}'"),
        0
    );
    assert!(fx.book.check().unwrap().is_empty());
    drop(fx);
    assert!(Book::open(&path, &KEY).is_ok());
}
