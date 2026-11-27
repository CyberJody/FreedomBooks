use super::*;
use crate::company_periods::{PeriodChange, PeriodPolicyError, period_policy};
use fb_ids::{CompanyId, UserId};
use rusqlite::params;

pub(crate) const CHANGED: &[&str] = &[
    "table:ledger_company", "table:directory_customer", "table:bankfeeds_account",
    "table:bankfeeds_rule", "trigger:fb_transfer_insert",
];
pub(crate) const OBJECTS: &[&str] = &[
    "trigger:fb_period_journal_post",
    "trigger:fb_period_credit_apply",
    "trigger:fb_period_credit_unapply",
    "trigger:fb_period_feed_accept",
    "trigger:fb_period_company_insert",
    "trigger:fb_period_company_update",
    "trigger:fb_feed_import_account_insert",
    "trigger:fb_feed_import_account_update",
    "trigger:fb_feed_import_insert",
    "trigger:fb_rule_feed_scope_insert",
    "trigger:fb_rule_feed_scope_update",
    "trigger:fb_rule_policy_manual_scope",
    "view:fb_transfer_eligible",
];

const KEY: Key = [97; 32];
const STAMP: &str = "2024-03-01T12:00:00.000000Z";
const CLOSE: &str = "2024-02-29";

fn id(number: u32) -> String {
    format!("00000000-0000-7000-8000-{number:012x}")
}

fn populate(conn: &Connection) {
    let (co, admin, bank, expense, card, customer) = (id(1), id(2), id(3), id(4), id(5), id(6));
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    conn.execute(
        "INSERT INTO ledger_company(id,name) VALUES(?1,'Synthetic accounting periods')",
        [&co],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO fb_user(id,username,is_staff) VALUES(?1,'Synthetic administrator',1)",
        [&admin],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO desktop_user(user_id,role,password_hash) VALUES(?1,'administrator',?2)",
        params![admin, format!("$argon2id$v=19${}", "synthetic".repeat(10))],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO ledger_company_members(company_id,user_id) VALUES(?1,?2)",
        [&co, &admin],
    )
    .unwrap();
    conn.execute("INSERT INTO directory_customer(id,company_id,display_name,tax_status,exemption_number) VALUES(?1,?2,'Synthetic customer','EXEMPT','RESALE')", [&customer, &co]).unwrap();
    conn.execute("INSERT INTO ledger_account(id,company_id,code,name,kind,detail_type) VALUES(?1,?2,'1000','Checking','ASSET','BANK'),(?3,?2,'6000','Supplies','EXPENSE','EXPENSE'),(?4,?2,'2000','Card','LIABILITY','CREDIT_CARD')", [&bank, &co, &expense, &card]).unwrap();
    for (number, amount, posted) in [
        (1, 150, true),
        (2, 250, true),
        (3, 100, false),
        (4, 80, true),
    ] {
        let journal = id(10 + number);
        conn.execute("INSERT INTO ledger_journal(id,number,company_id,date,memo,created_by_id,created_at,adjusting,entry_number) VALUES(?1,?2,?3,?4,'Synthetic entry',?5,?6,?7,?8)",
            params![journal, number, co, CLOSE, admin, STAMP, number == 3, format!("JE-{number:06}")]).unwrap();
        conn.execute("INSERT INTO ledger_journalline(id,journal_id,account_id,position,debit,credit,description,name) VALUES(?1,?2,?3,1,?5,0,'',''),(?4,?2,?6,2,0,?5,'','')",
            params![id(20 + number), journal, expense, id(30 + number), amount, bank]).unwrap();
        if posted {
            post(conn, &journal, "UPDATE").unwrap();
        }
    }
    conn.execute("INSERT INTO ledger_auditevent(id,company_id,actor_id,actor_name,occurred_at,action,target,detail)
        VALUES(?1,?2,?3,'Synthetic administrator',?4,'journal.posted',?5,'{}')",
        params![id(51), co, admin, STAMP, format!("journal:{}", id(11))]).unwrap();
    conn.execute(
        "INSERT INTO fb_seal(kind,doc_id) VALUES('synthetic-retained',?1)",
        [id(50)],
    )
    .unwrap();
    conn.execute("INSERT INTO attachments_attachment(id,company_id,journal_id,original_name,stored_name,content_type,size,sha256,original_sha256,wrapped_key,wrap_nonce,nonce,uploaded_by_id,uploaded_by_name,uploaded_at,uploaded_number,request_key)
        VALUES(?1,?2,?3,'Synthetic receipt.pdf',?4,'application/pdf',24,?5,?5,?6,?7,?8,?9,'Synthetic administrator',?10,'JE-000001',?11)",
        params![id(52), co, id(11), format!("{}.bin", "a".repeat(32)), "b".repeat(64), "c".repeat(96), "d".repeat(24), "a".repeat(14), admin, STAMP, id(53)]).unwrap();

    let detail = serde_json::json!({
        "attachment":id(52),"number":"JE-000001","name":"Synthetic receipt.pdf",
        "content_type":"application/pdf","size":24,"sha256":"b".repeat(64)
    });
    conn.execute("INSERT INTO ledger_auditevent(id,company_id,actor_id,actor_name,occurred_at,action,target,detail)
        VALUES(?1,?2,?3,'Synthetic administrator',?4,'attachment.added',?5,?6)",
        params![id(54),co,admin,STAMP,format!("journal:{}",id(11)),detail.to_string()]).unwrap();
    conn.execute(
        "INSERT INTO fb_seal(kind,doc_id) VALUES('attachment_added',?1)",
        [id(52)],
    )
    .unwrap();

    let connection = id(60);
    conn.execute("INSERT INTO bankfeeds_connection(id,company_id,label,host,active,created_by_id,created_at,last_sync_summary,sync_count)
        VALUES(?1,?2,'Synthetic Bank','bank.example',1,?3,?4,'',0)", params![connection,co,admin,STAMP]).unwrap();
    conn.execute(
        "INSERT INTO bankfeeds_identity_secret(name,secret) VALUES('identity',randomblob(32))",
        [],
    )
    .unwrap();
    for (feed, remote, account) in [(id(61), "Bank", bank), (id(62), "Card", card)] {
        conn.execute("INSERT INTO bankfeeds_account(id,connection_id,conn_key,remote_id,name,org_name,currency,first_seen_at,active,conn_key_identity,remote_identity)
            SELECT ?1,?2,'site',?3,?3,'Synthetic Bank','USD',?4,1,
             fb_bankfeeds_identity(secret,'connection-key','site'),fb_bankfeeds_identity(secret,'account',?3) FROM bankfeeds_identity_secret",
            params![feed,connection,remote,STAMP]).unwrap();
        conn.execute(
            "UPDATE bankfeeds_account SET ledger_account_id=?1,mapped_at=?2 WHERE id=?3",
            params![account, STAMP, feed],
        )
        .unwrap();
    }
    for (ordinal, amount, pending) in [
        (1, -400, false),
        (2, -80, false),
        (3, -200, true),
        (4, -300, false),
    ] {
        let remote = format!("Transaction {ordinal}");
        conn.execute("INSERT INTO bankfeeds_transaction(id,ordinal,feed_account_id,remote_id,transacted_on,amount,description,pending,first_seen_at,updated_at,status,accepted_as,reference,reference_url,return_count,remote_identity)
            SELECT ?1,?2,?3,?4,?5,?6,?4,?7,?8,?8,'NEW','','','',0,
              fb_bankfeeds_identity(secret,'transaction',?4) FROM bankfeeds_identity_secret",
            params![id(70 + ordinal), ordinal, id(61), remote, CLOSE, amount, pending, STAMP]).unwrap();
    }
    for number in [1, 2] {
        conn.execute("INSERT INTO bankfeeds_transaction_match(transaction_id,journal_id,journal_line_id) VALUES(?1,?2,?3)", [id(71), id(10 + number), id(30 + number)]).unwrap();
    }
    decide(conn, "UPDATE", 71, "MATCHED", "MATCH", 1).unwrap();
    conn.execute("UPDATE bankfeeds_transaction SET status='IGNORED',decided_at=?1,decided_by_id=?2 WHERE id=?3", params![STAMP,admin,id(74)]).unwrap();
    for (number, name, pattern) in [
        (1, "Manual rule", "Supplies"),
        (2, "Imported rule", "Imported conditions"),
    ] {
        conn.execute("INSERT INTO bankfeeds_rule(id,company_id,position,name,match_text,sign,account_id,payee_name,active,created_at,updated_at)
            VALUES(?1,?2,?3,?4,?5,'OUT',?6,'',1,?7,?7)", params![id(80+number),co,number,name,pattern,expense,STAMP]).unwrap();
    }
    let policy = serde_json::json!({"conditions":[{"code":1,"value":"Office"}],"all":true,"direction":"OUT","scope":"ALL","auto_post":true,"kind":"EXPENSE","payee_id":"","payee_table":""});
    conn.execute("INSERT INTO bankfeeds_rule_policy(rule_id,company_id,source_key,source_hash,source_row,data) VALUES(?1,?2,?3,?4,2,?5)",
        params![id(82),co,"a".repeat(64),"b".repeat(64),policy.to_string()]).unwrap();
    conn.execute(
        "INSERT INTO bankfeeds_auto_queue(item_id,rule_id,fingerprint) VALUES(?1,?2,?3)",
        params![id(72), id(82), "a".repeat(64)],
    )
    .unwrap();
    let violations = collect(conn, "PRAGMA foreign_key_check", |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(2)?))
    })
    .unwrap();
    assert!(violations.is_empty(), "{violations:?}");
    conn.execute_batch("COMMIT").unwrap();
}

fn post(conn: &Connection, journal: &str, op: &str) -> rusqlite::Result<usize> {
    conn.execute(
        &format!(
            "{op} ledger_journal SET status='POSTED',posted_by_id=?2,posted_at=?3 WHERE id=?1"
        ),
        params![journal, id(2), STAMP],
    )
}

fn decide(
    conn: &Connection,
    op: &str,
    item: u32,
    status: &str,
    kind: &str,
    journal: u32,
) -> rusqlite::Result<usize> {
    conn.execute(&format!("{op} bankfeeds_transaction SET status=?2, accepted_as=?3, journal_id=?4, journal_line_id=?5,
        reference='Synthetic entry',reference_url=?6,decided_at=?7,decided_by_id=?8 WHERE id=?1"),
        params![id(item),status,kind,id(10+journal),id(30+journal),format!("journal:{}",id(10+journal)),STAMP,id(2)])
}

fn change(month: u32, closing: Option<&str>) -> PeriodChange {
    PeriodChange {
        fiscal_year_start_month: month,
        closing_date: closing.map(str::to_owned),
    }
}

#[test]
fn format_97_backup_upgrade_and_historical_postings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("format-97.fbbook");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&format!("PRAGMA key=\"x'{}'\"", hex(&KEY)))
        .unwrap();
    Book::configure(&conn).unwrap();
    assert_eq!(MIGRATIONS.len(), 98);
    assert_eq!(MIGRATIONS[96].0, "0210_bankfeeds_matching");
    for (_, sql) in &MIGRATIONS[..97] {
        conn.execute_batch(sql).unwrap();
    }
    conn.execute_batch("PRAGMA user_version=97").unwrap();
    populate(&conn);
    let mut before = test_support::snapshot(&conn).unwrap();
    let schema = schema_hashes(&conn).unwrap();
    let tampered = dir.path().join("tampered-97.fbbook");
    let backup = dir.path().join("verified-97.fbbook");
    drop(conn);
    let Opened::Behind(old) = Book::open_staged(&path, &KEY).unwrap() else {
        panic!("Expected format 97")
    };
    assert_eq!((old.from(), old.to()), (97, 98));
    for copy in [&backup, &tampered] {
        old.snapshot(copy, &KEY).unwrap();
    }
    assert_eq!(test_support::snapshot_file(&backup, &KEY).unwrap(), before);
    let copy = Connection::open(&tampered).unwrap();
    copy.execute_batch(&format!(
        "PRAGMA key=\"x'{}'\"; DROP TRIGGER fb_bankfeeds_transaction_match_update",
        hex(&KEY)
    ))
    .unwrap();
    drop(copy);
    assert!(matches!(
        Book::open_staged(&tampered, &KEY),
        Err(DbError::Integrity(_))
    ));
    let mut book = old.upgrade().unwrap();
    test_support::company_period_defaults(&mut before, &book.conn);
    assert_eq!(test_support::snapshot(&book.conn).unwrap(), before);
    let upgraded = schema_hashes(&book.conn).unwrap();
    for (name, hash) in &schema {
        assert_eq!(
            &upgraded[name] != hash,
            CHANGED.contains(&name.as_str()),
            "{name}"
        );
    }
    assert_eq!(
        upgraded
            .keys()
            .filter(|name| !schema.contains_key(*name))
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        OBJECTS.iter().copied().collect()
    );
    let fresh = Book::create(&dir.path().join("fresh.fbbook"), &KEY).unwrap();
    assert_eq!(upgraded, schema_hashes(&fresh.conn).unwrap());
    book.pending_credential_masks.clear(); // Synthetic feed metadata has no credentials.

    let (company, admin) = (
        CompanyId::parse(&id(1)).unwrap(),
        UserId::parse(&id(2)).unwrap(),
    );
    assert_eq!(
        period_policy(&book.conn, company).unwrap().values,
        change(1, None)
    );
    assert_eq!(
        book.write(|w| w.set_company_period_policy(
            admin,
            company,
            0,
            change(4, Some(CLOSE)),
            None
        ))
        .unwrap()
        .revision,
        1
    );
    assert!(
        book.check().unwrap().is_empty(),
        "{:?}",
        book.check().unwrap()
    );

    let snapshot = test_support::snapshot(&book.conn).unwrap();
    for recursion in ["ON", "OFF"] {
        book.conn
            .execute_batch(&format!("PRAGMA recursive_triggers={recursion}"))
            .unwrap();
        for op in ["UPDATE", "UPDATE OR REPLACE"] {
            assert_eq!(
                constraint_of(&post(&book.conn, &id(13), op).unwrap_err()).as_deref(),
                Some("period_closed")
            );
            assert_eq!(
                constraint_of(&decide(&book.conn, op, 72, "ACCEPTED", "JOURNAL", 4).unwrap_err())
                    .as_deref(),
                Some("period_closed")
            );
            let sql = format!(
                "{op} ledger_company SET closing_date=NULL, period_policy_revision=2 WHERE id=?1"
            );
            assert_eq!(
                constraint_of(&book.conn.execute(&sql, [id(1)]).unwrap_err()).as_deref(),
                Some("period_policy")
            );
            let sql = format!("{op} ledger_company SET period_policy_revision=2 WHERE id=?1");
            assert_eq!(
                constraint_of(&book.conn.execute(&sql, [id(1)]).unwrap_err()).as_deref(),
                Some("period_policy")
            );
            let sql = format!("{op} directory_customer SET default_terms=?1 WHERE id=?2");
            assert!(book.conn.execute(&sql, params!["90", id(6)]).is_err());
        }
        for verb in ["INSERT", "INSERT OR REPLACE"] {
            let sql = format!(
                "{verb} INTO ledger_company(id,name,closing_date) VALUES(?1,'New company',?2)"
            );
            assert_eq!(
                constraint_of(&book.conn.execute(&sql, params![id(95), CLOSE]).unwrap_err())
                    .as_deref(),
                Some("period_policy")
            );
            let sql = format!("{verb} INTO ledger_journal(id,number,company_id,date,memo,status,created_by_id,created_at,posted_by_id,posted_at)
                VALUES(?1,90,?2,?4,'Direct post','POSTED',?3,?5,?3,?5)");
            assert!(is_constraint(
                &book
                    .conn
                    .execute(&sql, params![id(96), id(1), id(2), CLOSE, STAMP])
                    .unwrap_err()
            ));
        }
        assert_eq!(test_support::snapshot(&book.conn).unwrap(), snapshot);
    }
    book.conn
        .execute_batch("PRAGMA recursive_triggers=ON")
        .unwrap();
    decide(&book.conn, "UPDATE", 72, "MATCHED", "MATCH", 4).unwrap();
    assert!(book.check().unwrap().is_empty());
    drop(book);
    assert!(Book::open(&path, &KEY).unwrap().check().unwrap().is_empty());
}

fn scalar(q: &(impl Query + ?Sized)) -> rusqlite::Result<bool> {
    q.query_row("SELECT fb_company_period_command()", [], |row| row.get(0))
}

#[test]
fn company_period_capability_is_read_only_and_restored_on_errors_and_panics() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scope.fbbook");
    let mut book = Book::create(&path, &KEY).unwrap();
    populate(&book.conn);
    let co = CompanyId::parse(&id(1)).unwrap();
    let actor = UserId::parse(&id(2)).unwrap();
    // The second connection starts with a different, always-inactive scalar state.
    let mut second = Book::open(&path, &KEY).unwrap();
    // The ordinary Writer authorizer also rejects replacing the read-only function through SQL.
    assert!(!book.read(|q| scalar(q)).unwrap());
    assert!(!second.read(|q| scalar(q)).unwrap());
    let outcome = book.write(|w| -> Result<(), PeriodPolicyError> {
        assert!(!scalar(w)?);
        assert!(w.execute_batch("CREATE TEMP TRIGGER enable BEFORE UPDATE ON ledger_company BEGIN SELECT fb_company_period_command(1); END").is_err());
        assert!(w.query_row("SELECT fb_company_period_command(1)", [], |_| Ok(())).is_err());
        assert!(matches!(w.set_company_period_policy(actor, co, 0, change(4, Some("2023-02-29")), None), Err(PeriodPolicyError::InvalidDate)));
        w.set_company_period_policy(actor, co, 0, change(4, Some(CLOSE)), None)?;
        assert!(!scalar(w)?);
        assert!(!second.read(|q| scalar(q)).unwrap());
        Err(PeriodPolicyError::Stale)
    });
    assert!(matches!(outcome, Err(PeriodPolicyError::Stale)));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: Result<(), PeriodPolicyError> = book.write(|w| {
            w.set_company_period_policy(actor, co, 0, change(4, Some(CLOSE)), None)?;
            panic!("synthetic after policy write");
        });
    }));
    assert!(result.is_err());
    book.idle_after_unwind().unwrap();
    assert!(!book.read(|q| scalar(q)).unwrap());
    assert_eq!(period_policy(&book.conn, co).unwrap().revision, 0);

    book.conn.execute_batch("CREATE TRIGGER reject_second_period_audit BEFORE INSERT ON ledger_auditevent
        WHEN NEW.action='company.period_closed' BEGIN SELECT RAISE(ABORT,'synthetic audit failure'); END").unwrap();
    let before = test_support::snapshot(&book.conn).unwrap();
    book.write(|w| -> rusqlite::Result<()> {
        assert!(matches!(
            w.set_company_period_policy(actor, co, 0, change(4, Some(CLOSE)), None),
            Err(PeriodPolicyError::Sql(_))
        ));
        assert!(!scalar(w)?);
        Ok(())
    })
    .unwrap();
    assert_eq!(test_support::snapshot(&book.conn).unwrap(), before);
    book.conn
        .execute_batch("DROP TRIGGER reject_second_period_audit")
        .unwrap();
    assert!(book.check().unwrap().is_empty());
}

#[test]
fn bank_feed_import_and_rule_scope_guards_hold_with_or_without_recursive_triggers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bank-policy.fbbook");
    let book = Book::create(&path, &KEY).unwrap();
    let db = &book.conn;
    populate(db);
    let (company, other, checking, pending, manual, imported) = (id(1), id(99), id(61), id(63), id(81), id(82));
    db.execute("INSERT INTO ledger_company(id,name) VALUES(?1,'Other synthetic company')", [&other]).unwrap();
    db.execute("INSERT INTO ledger_company_members(company_id,user_id) VALUES(?1,?2)", params![other, id(2)]).unwrap();
    db.execute("INSERT INTO bankfeeds_connection(id,company_id,label,host,active,created_by_id,created_at,last_sync_summary,sync_count)
        VALUES(?1,?2,'Other connection','bank.example',1,?3,?4,'',0)", params![id(64), other, id(2), STAMP]).unwrap();
    let insert_feed = |key: &str, conn: &str, state: &str, date: Option<&str>| {
        db.execute("INSERT INTO bankfeeds_account(id,connection_id,conn_key,remote_id,name,org_name,currency,first_seen_at,active,conn_key_identity,remote_identity,import_state,import_from_date)
            SELECT ?1,?2,'site',?1,'Synthetic feed','','USD',?3,1,fb_bankfeeds_identity(secret,'connection-key','site'),
            fb_bankfeeds_identity(secret,'account',?1),?4,?5 FROM bankfeeds_identity_secret", params![key, conn, STAMP, state, date])
    };
    let other_feed = id(65);
    insert_feed(&pending, &id(60), "PENDING", None).unwrap();
    insert_feed(&other_feed, &id(64), "ACTIVE", None).unwrap();
    let import = |verb: &str, account: &str, date: Option<&str>, transacted: Option<&str>| {
        db.execute(&format!("{verb} INTO bankfeeds_transaction(id,ordinal,feed_account_id,remote_id,posted_on,transacted_on,amount,description,pending,first_seen_at,updated_at,status,accepted_as,reference,reference_url,return_count,remote_identity)
            SELECT ?1,5,?2,'test',?3,?4,-50,'Imported',0,?5,?5,'NEW','','','',0,
            fb_bankfeeds_identity(secret,'transaction','test') FROM bankfeeds_identity_secret"), params![id(75), account, date, transacted, STAMP])
    };

    for recursive in [false, true] {
        db.pragma_update(None, "recursive_triggers", recursive).unwrap();
        let before = test_support::snapshot(db).unwrap();
        for verb in ["INSERT", "INSERT OR REPLACE", "REPLACE"] {
            for account in [&pending, &id(67)] {
                let error = import(verb, account, Some(CLOSE), None).unwrap_err().to_string();
                assert!(error.contains("feed_import_policy"), "{error}");
            }
            let sql = format!("{verb} INTO bankfeeds_rule(id,company_id,position,name,match_text,sign,account_id,payee_name,active,created_at,updated_at,feed_account_id)
                SELECT ?1,company_id,position+10,'Other rule',match_text,sign,account_id,payee_name,active,created_at,updated_at,?2
                FROM bankfeeds_rule WHERE id=?3");
            for scope in [&other_feed, &id(67)] {
                let error = db.execute(&sql, params![id(83), scope, manual]).unwrap_err().to_string();
                assert!(error.contains("rule_feed_scope"), "{error}");
            }
        }
        for state in ["PENDING", "READY", "ACTIVE"] {
            assert!(insert_feed(&id(68), &id(60), state, Some(CLOSE)).is_err());
        }
        for verb in ["UPDATE", "UPDATE OR REPLACE"] {
            for (feed, state, date) in [
                (&pending, "ACTIVE", None),
                (&pending, "PENDING", Some(CLOSE)),
                (&pending, "READY", Some("2025-02-29")),
                (&checking, "READY", Some(CLOSE)),
                (&checking, "ACTIVE", Some(CLOSE)),
            ] {
                let sql = format!("{verb} bankfeeds_account SET import_state=?1,import_from_date=?2 WHERE id=?3");
                assert!(db.execute(&sql, params![state, date, feed]).is_err());
            }
            for (rule, scope) in [(&manual, &other_feed), (&imported, &checking)] {
                let sql = format!("{verb} bankfeeds_rule SET feed_account_id=?1 WHERE id=?2");
                let error = db.execute(&sql, [scope, rule]).unwrap_err().to_string();
                assert!(error.contains("rule_feed_scope"), "{error}");
            }
        }
        assert_eq!(test_support::snapshot(db).unwrap(), before);
    }
    db.execute("UPDATE bankfeeds_rule SET feed_account_id=?1 WHERE id=?2", [&pending, &manual]).unwrap();
    assert!(db.execute("DELETE FROM bankfeeds_account WHERE id=?1", [&pending]).is_err());
    for verb in ["INSERT", "INSERT OR REPLACE"] {
        let sql = format!("{verb} INTO bankfeeds_rule_policy(rule_id,company_id,source_key,source_hash,source_row,data)
            SELECT ?1,company_id,?2,source_hash,source_row,data FROM bankfeeds_rule_policy WHERE rule_id=?3");
        let error = db.execute(&sql, params![manual, "c".repeat(64), imported]).unwrap_err().to_string();
        assert!(error.contains("rule_feed_scope"), "{error}");
    }

    db.execute("UPDATE bankfeeds_account SET import_state='READY',import_from_date=?1 WHERE id=?2", [CLOSE, &pending]).unwrap();
    assert!(db.execute("UPDATE bankfeeds_account SET import_state='ACTIVE',import_from_date='2024-02-28' WHERE id=?1", [&pending]).is_err());
    assert!(import("INSERT", &pending, None, None).is_err());
    assert!(import("INSERT OR REPLACE", &pending, Some("2024-02-28"), Some(CLOSE)).is_err());
    import("INSERT", &pending, None, Some(CLOSE)).unwrap();
    db.execute("UPDATE bankfeeds_account SET import_state='ACTIVE' WHERE id=?1", [&pending]).unwrap();
    // Existing IDs retain corrections even when the bank revises their accounting date below the floor.
    db.execute("UPDATE bankfeeds_transaction SET transacted_on='2024-02-28' WHERE id=?1", [id(75)]).unwrap();
    assert_eq!(db.query_row("SELECT transacted_on FROM bankfeeds_transaction WHERE id=?1", [id(75)], |r| r.get::<_, String>(0)).unwrap(), "2024-02-28");

    let card = id(5);
    let eligible = |key: &str| db.query_row("SELECT count(*) FROM fb_transfer_eligible WHERE company_id=?1 AND account_id=?2",
        [&company, &key.to_owned()], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(eligible(&card), 1);
    assert_eq!(eligible(&id(3)), 0);
    db.execute("INSERT INTO documents_accountingsetup(id,company_id) VALUES(?1,?2)", params![id(90), company]).unwrap();
    db.execute("INSERT INTO documents_designatedbank(id,setup_id,account_id) VALUES(?1,?2,?3)", [id(91), id(90), id(3)]).unwrap();
    assert_eq!(eligible(&id(3)), 1);
    assert!(book.check().unwrap().is_empty(), "{:?}", book.check());
    drop(book);
    assert!(Book::open(&path, &KEY).unwrap().check().unwrap().is_empty());
}

#[test]
fn period_feed_accept_blocks_closed_period_journals_and_policy_audit_protects_structured_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("guards-extra.fbbook");
    let mut book = Book::create(&path, &KEY).unwrap();
    populate(&book.conn);
    let company = CompanyId::parse(&id(1)).unwrap();
    let admin = UserId::parse(&id(2)).unwrap();
    book.write(|w| w.set_company_period_policy(admin, company, 0, change(1, Some(CLOSE)), None))
        .unwrap();
    // Insert a NEW feed row dated AFTER the closing date, then attempt to ACCEPTED-link it to journal 4 (dated on CLOSE).
    book.conn
        .execute(
            "INSERT INTO bankfeeds_transaction(id,ordinal,feed_account_id,remote_id,transacted_on,amount,description,pending,first_seen_at,updated_at,status,accepted_as,reference,reference_url,return_count,remote_identity)
             SELECT ?1,5,?2,'Open-date feed','2024-03-05',-80,'Open-date feed',0,?3,?3,'NEW','','','',0,
               fb_bankfeeds_identity(secret,'transaction','Open-date feed') FROM bankfeeds_identity_secret",
            params![id(88), id(61), STAMP],
        )
        .unwrap();
    let err = decide(&book.conn, "UPDATE", 88, "ACCEPTED", "JOURNAL", 4).unwrap_err();
    assert_eq!(constraint_of(&err).as_deref(), Some("period_closed"));
    // Matching a historical journal still works through MATCHED.
    decide(&book.conn, "UPDATE", 88, "MATCHED", "MATCH", 4).unwrap();

    // Policy audit allows free-text reason masking while rolling back if a text guard mutates structured evidence.
    book.retain_text_guard("mask-reason".into(), |text| Ok(text.replace("SECRET", "[redacted]")));
    let policy = book
        .write(|w| {
            w.set_company_period_policy(
                admin,
                company,
                1,
                change(1, Some("2024-02-28")),
                Some("Reopen SECRET adjustment"),
            )
        })
        .unwrap();
    assert_eq!(policy.revision, 2);
    let detail: String = book
        .conn
        .query_row(
            "SELECT detail FROM ledger_auditevent WHERE action='company.period_reopened' ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(detail.contains("[redacted]") && !detail.contains("SECRET"), "{detail}");

    // Now install a guard that alters structured audit evidence (`company.period_closed`): must roll back!
    let before = test_support::snapshot(&book.conn).unwrap();
    book.retain_text_guard("tamper-action".into(), |text| {
        Ok(text.replace("company.period_closed", "tampered"))
    });
    let res = book.write(|w| w.set_company_period_policy(admin, company, 2, change(1, Some(CLOSE)), None));
    assert!(res.is_err());
    assert_eq!(test_support::snapshot(&book.conn).unwrap(), before);

    // A guard that alters actor_name must also roll back.
    book.retain_text_guard("tamper-actor".into(), |text| {
        Ok(text.replace("Admin", "Altered"))
    });
    let res = book.write(|w| w.set_company_period_policy(admin, company, 2, change(1, Some(CLOSE)), None));
    assert!(res.is_err());
    assert_eq!(test_support::snapshot(&book.conn).unwrap(), before);

    // Imported rule policy cannot be displaced via INSERT OR REPLACE on (company_id, source_key)
    // even with recursive_triggers=OFF.
    book.conn.execute_batch("PRAGMA recursive_triggers=OFF").unwrap();
    let policy_snap = test_support::snapshot(&book.conn).unwrap();
    let displaced = book.conn.execute(
        "INSERT OR REPLACE INTO bankfeeds_rule_policy(rule_id,company_id,source_key,source_hash,source_row,data)
         SELECT ?1,company_id,source_key,source_hash,source_row,data FROM bankfeeds_rule_policy WHERE rule_id=?2",
        params![id(81), id(82)],
    );
    assert!(displaced.is_err(), "INSERT OR REPLACE displaced imported rule policy");
    assert!(
        displaced.unwrap_err().to_string().contains("bankfeeds_rule_policy_immutable")
    );
    book.conn.execute_batch("PRAGMA recursive_triggers=ON").unwrap();
    assert_eq!(test_support::snapshot(&book.conn).unwrap(), policy_snap);
}
