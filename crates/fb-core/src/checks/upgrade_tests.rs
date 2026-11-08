use super::guards::snapshot;
use super::*;
use crate::payables;
use fb_db::{
    MIGRATIONS,
    rusqlite::{Connection, types::Value as SqlValue},
};

fn populated_prior(checks: bool) -> Fx {
    let mut f = Fx::new();
    let p = json!({"vendor_id":f.vendor,"date":"2026-10-23","due_date":"2026-11-23","request_key":fb_ids::new_v7().to_string(),"lines":[{"description":"Prior bill","quantity":"1","rate":"10","expense_account_id":f.expense.to_db()}]});
    let b = payables::save_bill(&mut f.book, f.actor, f.co, &p).unwrap();
    let r = payables::review_bill(&mut f.book, f.actor, f.co, s(&b, "id"), Some(1)).unwrap();
    let b = payables::post_bill(&mut f.book, f.actor, f.co, s(&b, "id"), Some(1), &r).unwrap();
    payables::void_bill(
        &mut f.book,
        f.actor,
        f.co,
        s(&b, "id"),
        Some(2),
        &json!("2026-10-24"),
        &json!("Prior void"),
    )
    .unwrap();
    if checks {
        let p = f.payload();
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
            &json!("Prior void"),
        )
        .unwrap();
    }
    f
}
#[test]
fn populated_prior_prefixes_upgrade_and_tampered_schema_is_not_repaired() {
    for prefix in [
        "0030_checks_transfers",
        "0031_checks_parent_eligibility",
        "0032_checks_root_parent_ordering",
        "0033_checks_review_fixes",
    ] {
        let prior = MIGRATIONS.iter().position(|(n, _)| *n == prefix).unwrap();
        for tamper in [false, true] {
            let f = populated_prior(prefix != "0030_checks_transfers");
            // Legacy lines have no provenance. Preserve their exact old columns.
            let def: String = f
                .book
                .connection()
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE name='fb_checks_checkline_update'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            f.book.connection().execute_batch("DROP TRIGGER fb_checks_checkline_update; UPDATE checks_checkline SET origin_position=NULL").unwrap();
            f.book.connection().execute_batch(&def).unwrap();
            let before = snapshot(f.book.connection());
            let path = std::env::temp_dir().join(format!(
                "checks-upgrade-{}-{prefix}-{tamper}.fbbook",
                std::process::id()
            ));
            let _ = std::fs::remove_file(&path);
            let c = Connection::open(&path).unwrap();
            c.execute_batch("PRAGMA key=\"x'2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a'\";").unwrap();
            fb_db::collation::register(&c).unwrap();
            // Match the production atomic migration boundary, avoiding one fsync per DDL.
            c.execute_batch("BEGIN IMMEDIATE").unwrap();
            for (_, sql) in &MIGRATIONS[..prior] {
                c.execute_batch(sql).unwrap();
            }
            c.execute_batch(&format!("PRAGMA user_version={prior}"))
                .unwrap();
            c.execute_batch("COMMIT").unwrap();
            let triggers: Vec<(String, String)> = c
                .prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger'")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            let prior_tables = snapshot(&c);
            c.execute_batch("BEGIN IMMEDIATE; PRAGMA defer_foreign_keys=ON")
                .unwrap();
            for (name, _) in &triggers {
                c.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
            }
            for table in before.as_object().unwrap().keys() {
                if prior_tables.get(table).is_none() {
                    assert_eq!(
                        before[table],
                        json!([]),
                        "later table must be empty in prior fixture"
                    );
                    continue;
                }
                let columns = c
                    .prepare(&format!("SELECT * FROM {table} LIMIT 0"))
                    .unwrap()
                    .column_names()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
                    .join(",");
                let mut st = f
                    .book
                    .connection()
                    .prepare(&format!("SELECT {columns} FROM {table}"))
                    .unwrap();
                let n = st.column_count();
                let records = st
                    .query_map([], |r| {
                        (0..n)
                            .map(|i| r.get::<_, SqlValue>(i))
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .unwrap();
                for row in records {
                    c.execute(
                        &format!("INSERT INTO {table} VALUES({})", vec!["?"; n].join(",")),
                        fb_db::rusqlite::params_from_iter(row.unwrap()),
                    )
                    .unwrap();
                }
            }
            for (_, sql) in &triggers {
                c.execute_batch(sql).unwrap();
            }
            c.execute_batch("COMMIT").unwrap();
            let mut copied = snapshot(&c);
            if let Some(lines) = copied
                .get_mut("checks_checkline")
                .and_then(Value::as_array_mut)
            {
                for line in lines {
                    line["origin_position"] = Value::Null;
                }
            }
            for (table, rows) in copied.as_object().unwrap() {
                assert_eq!(&before[table], rows, "prior {table}");
            }
            if tamper {
                c.execute_batch("DROP TRIGGER fb_bill_delete; CREATE TRIGGER fb_bill_delete BEFORE DELETE ON payables_bill BEGIN SELECT 1; END;").unwrap();
            }
            drop(c);
            let opened = Book::open(&path, &[42; 32]);
            if tamper {
                assert!(opened.is_err());
                continue;
            }
            let book = opened.unwrap();
            fb_db::test_support::assert_desktop_user_backfill(book.connection());
            let mut after = snapshot(book.connection());
            assert_eq!(before["desktop_user"], json!([]));
            after["desktop_user"] = json!([]); // Only the independently checked 0185 backfill is new.
            assert_eq!(after, before);
            assert!(book.check().unwrap().is_empty());
            drop(book);
            assert!(
                Book::open(&path, &[42; 32])
                    .unwrap()
                    .check()
                    .unwrap()
                    .is_empty()
            );
        }
    }
}
