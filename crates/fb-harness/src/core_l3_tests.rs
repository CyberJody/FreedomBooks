//! One book exercises both L3 modules, including populated historical-copy upgrades.
use super::*;
use fb_db::{
    DbError, MIGRATIONS,
    rusqlite::{Connection, types::Value as SqlValue},
};
use std::sync::OnceLock;

type Snapshot = BTreeMap<String, Vec<Vec<SqlValue>>>;

fn scenario() -> &'static Value {
    static SCENARIO: OnceLock<Value> = OnceLock::new();
    SCENARIO.get_or_init(|| {
        serde_json::from_str(include_str!("../scenarios/core-l3-combined.json")).unwrap()
    })
}

fn snapshot(c: &Connection) -> Snapshot {
    let tables: Vec<String> = c.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT GLOB 'sqlite_*' ORDER BY name")
        .unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
    tables
        .into_iter()
        .map(|table| {
            let mut stmt = c.prepare(&format!("SELECT * FROM {table}")).unwrap();
            let columns = stmt.column_count();
            let mut rows: Vec<Vec<SqlValue>> = stmt
                .query_map([], |r| (0..columns).map(|i| r.get(i)).collect())
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            rows.sort_by_key(|r| format!("{r:?}"));
            (table, rows)
        })
        .collect()
}

fn triggers(c: &Connection) -> Vec<(String, String)> {
    c.prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger' ORDER BY name")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

// Copy complete valid service-created rows to the exact prefix immediately before
// the payables table rebuild. Every original trigger is restored before opening.
fn historical_copy(source: &Connection, path: &Path) -> Snapshot {
    let copy=fb_db::test_support::populated_copy(source,path,&[42;32],"0025_payables_core_hardening",false).unwrap();
    assert!(!copy.before["sales_invoice"].is_empty());
    assert!(!copy.before["payables_billpayment"].is_empty());
    copy.expected
}

fn populated(dir: &Path) -> Runner<'static> {
    let mut runner = Runner::new(scenario(), &dir.join("source.fbbook"));
    runner.run();
    assert!(runner.book.check().unwrap().is_empty());
    runner
}

fn assert_callbacks(book: &mut Book) {
    let sql = "SELECT fb_sales_money('\"4.26\"'),fb_payables_line_valid(10050,100,101),fb_payables_line_valid(10050,100,100)";
    let read: fb_core::FbResult<_> = book.read(|r| {
        Ok(r.query_row(sql, [], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?)
    });
    assert_eq!(read.unwrap(), (426, 1, 0));
    let write: fb_core::FbResult<_> = book.write(|w| {
        // Both predicates remain available while transaction control and unsafe
        // PRAGMA writes are still denied by the callback authorizer.
        assert!(w.execute_batch("PRAGMA foreign_keys=OFF").is_err());
        assert!(w.execute_batch("COMMIT").is_err());
        Ok(w.query_row(sql, [], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?)
    });
    assert_eq!(write.unwrap(), (426, 1, 0));
}

#[test]
fn combined_book_copy_upgrade_preserves_rows_reports_and_both_callback_families() {
    let dir = tempfile::tempdir().unwrap();
    let mut runner = populated(dir.path());
    assert_callbacks(&mut runner.book);
    let dump = runner.dump();
    let tb = &dump["trial_balances"]["c"]["2026-10-02"];
    assert_eq!(tb["debit"], 3500);
    assert_eq!(tb["credit"], 3500);
    for row in [
        json!(["bank", 1800, 0]),
        json!(["ar", 500, 0]),
        json!(["ap", 0, 500]),
        json!(["income", 0, 3000]),
        json!(["expense", 1200, 0]),
    ] {
        assert!(
            tb["rows"].as_array().unwrap().contains(&row),
            "missing {row}"
        );
    }
    let path = dir.path().join("upgrade.fbbook");
    let before = historical_copy(runner.book.connection(), &path);
    assert_eq!(before["sales_invoice"].len(), 2);
    assert_eq!(before["sales_receipt"].len(), 1);
    assert_eq!(before["payables_bill"].len(), 1);
    assert_eq!(before["payables_billpayment"].len(), 1);
    for _ in 0..2 {
        runner.book = backups::SelectedBook::open(&path, &[42; 32]).unwrap();
        assert_eq!(snapshot(runner.book.connection()), before);
        assert!(runner.book.check().unwrap().is_empty());
        assert_eq!(
            runner
                .book
                .connection()
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            MIGRATIONS.len() as i64
        );
        assert_callbacks(&mut runner.book);
        assert_eq!(runner.dump(), dump);
    }
    println!(
        "Combined populated copy -> version {}: all rows/keys/reports preserved; callbacks protected; trial balance 35.00 = 35.00",
        MIGRATIONS.len()
    );
}

#[test]
fn open_reports_both_sales_and_payables_integrity_violations_with_intact_schema() {
    let dir = tempfile::tempdir().unwrap();
    let mut runner = populated(dir.path());
    let path = dir.path().join("tampered.fbbook");
    historical_copy(runner.book.connection(), &path);
    runner.book = backups::SelectedBook::open(&path, &[42; 32]).unwrap();
    let c = runner.book.connection();
    let definitions = triggers(c);
    c.execute_batch("BEGIN IMMEDIATE").unwrap();
    for (name, _) in &definitions {
        c.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
    }
    c.execute_batch("UPDATE sales_invoiceline SET data=json_set(data,'$.amount','999.00'); UPDATE payables_billline SET amount=amount+1;").unwrap();
    for (_, sql) in &definitions {
        c.execute_batch(sql).unwrap();
    }
    c.execute_batch("COMMIT").unwrap();
    let problems = runner.book.check().unwrap();
    assert!(
        problems
            .iter()
            .any(|p| p.contains("inconsistent sales totals")),
        "{problems:?}"
    );
    assert!(
        problems
            .iter()
            .any(|p| p.contains("invalid payables evidence")),
        "{problems:?}"
    );
    assert!(
        !problems.iter().any(|p| p.contains("schema")),
        "{problems:?}"
    );
    drop(runner);
    match Book::open(&path, &[42; 32]) {
        Err(DbError::Integrity(problems)) => {
            assert!(
                problems
                    .iter()
                    .any(|p| p.contains("inconsistent sales totals")),
                "{problems:?}"
            );
            assert!(
                problems
                    .iter()
                    .any(|p| p.contains("invalid payables evidence")),
                "{problems:?}"
            );
        }
        other => panic!(
            "expected both module checks to refuse open: {:?}",
            other.map(|_| ())
        ),
    }
}

#[test]
fn frozen_fdd2fa6_populated_book_upgrades_every_row_and_key_and_reopens() {
    let dir=tempfile::tempdir().unwrap();
    let mut runner=populated(dir.path());
    let dump=runner.dump();
    let path=dir.path().join("fdd2fa6.fbbook");
    let copy=fb_db::test_support::populated_copy(runner.book.connection(),&path,&[42;32],"fdd2fa6",true).unwrap();
    assert!(!copy.before["sales_receipt"].is_empty());
    assert!(!copy.before["payables_billpayment"].is_empty());
    for _ in 0..2 {
        runner.book=backups::SelectedBook::open(&path,&[42;32]).unwrap();
        assert_eq!(snapshot(runner.book.connection()),copy.expected);
        assert!(runner.book.check().unwrap().is_empty());
        assert_callbacks(&mut runner.book);
        assert_eq!(runner.dump(),dump);
    }
}

#[test]
fn unused_designated_bank_reclassification_checks_and_reopens() {
    let scenario:Value=serde_json::from_str(include_str!("../scenarios/int-l3-unused-designated-reclassification.json")).unwrap();
    let dir=tempfile::tempdir().unwrap();
    let mut runner=Runner::new(&scenario,&dir.path().join("unused.fbbook"));let path=dir.path().join("reopen.fbbook");runner.book=backups::SelectedBook::create(&path,&[42;32]).unwrap();runner.run();
    assert!(runner.outcomes.iter().all(|row|row["ok"]==true));
    let problems=runner.book.check().unwrap();
    assert!(problems.is_empty(),"service-created unused designation: {problems:?}");
    assert!(Book::open(&path,&[42;32]).unwrap().check().unwrap().is_empty());
}

#[test]
fn frozen_base_upgrade_refuses_changed_schema_without_migrating_rows() {
    let dir=tempfile::tempdir().unwrap();
    let runner=populated(dir.path());
    let path=dir.path().join("tampered-fdd2fa6.fbbook");
    let copy=fb_db::test_support::populated_copy(runner.book.connection(),&path,&[42;32],"fdd2fa6",true).unwrap();
    let c=Connection::open(&path).unwrap();
    c.execute_batch("PRAGMA key=\"x'2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a'\";DROP TRIGGER fb_bill_update_collision").unwrap();
    fb_db::collation::register(&c).unwrap();
    let version:i64=c.query_row("PRAGMA user_version",[],|r|r.get(0)).unwrap();
    assert!(matches!(Book::open(&path,&[42;32]),Err(DbError::Integrity(_))));
    assert_eq!(snapshot(&c),copy.before);
    assert_eq!(c.query_row("PRAGMA user_version",[],|r|r.get::<_,i64>(0)).unwrap(),version);
}
