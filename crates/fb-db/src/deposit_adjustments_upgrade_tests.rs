//! Migration 0209 (book format 96, worker item 47) lets a deposit carry fee and adjustment lines in its
//! frozen payload. It replaces the two deposit integrity views and adds one helper view; no table, trigger
//! or row changes. A book at format 95 (the last format before it) is backed up, upgraded and reopened.
use super::*;
use std::collections::BTreeSet;

/// The views 0209 replaces: each keeps every prior condition and gains the adjustment ones.
pub(crate) const CHANGED: &[&str] = &["view:banking_deposit_expected", "view:banking_deposit_invalid"];
/// What 0209 adds.
pub(crate) const OBJECTS: &[&str] = &["view:banking_deposit_adjustment"];

const CO: &str = "00000000-0000-7000-8000-0000000005a1";
const USER: &str = "00000000-0000-7000-8000-0000000005a2";

#[test]
fn a_format_95_book_is_backed_up_upgraded_and_reopens_with_only_the_deposit_views_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("format-95.fbbook");
    let key = [95; 32];
    let boundary = 95;
    assert_eq!(MIGRATIONS[boundary - 1].0, "0208_ai_unattended");
    assert_eq!(MIGRATIONS[boundary].0, "0209_deposit_adjustments");
    assert_eq!(MIGRATIONS[boundary + 1].0, "0210_bankfeeds_matching");
    assert_eq!(MIGRATIONS.len(), boundary + 2);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&format!("PRAGMA key=\"x'{}'\"", hex(&key))).unwrap();
    Book::configure(&conn).unwrap();
    for (_, sql) in &MIGRATIONS[..boundary] {
        conn.execute_batch(sql).unwrap();
    }
    conn.execute_batch("PRAGMA user_version=95").unwrap();
    conn.execute("INSERT INTO fb_user(id,username) VALUES(?1,'Synthetic bookkeeper')", [USER]).unwrap();
    conn.execute("INSERT INTO ledger_company(id,name,currency) VALUES(?1,'Synthetic payout company','USD')", [CO]).unwrap();
    let before = test_support::snapshot(&conn).unwrap();
    let schema = schema_hashes(&conn).unwrap();
    let sql = |c: &Connection, name: &str| -> String { c.query_row("SELECT sql FROM sqlite_schema WHERE name=?1", [name], |r| r.get(0)).unwrap() };
    let prior_invalid = sql(&conn, "banking_deposit_invalid");
    let prior_expected = sql(&conn, "banking_deposit_expected");
    drop(conn);

    let old = match Book::open_staged(&path, &key).unwrap() {
        Opened::Behind(old) => old,
        Opened::Current(_) => panic!("expected a format-95 book"),
    };
    assert_eq!((old.from(), old.to()), (95, 97));
    let backup = dir.path().join("verified-format-95.fbbook");
    old.snapshot(&backup, &key).unwrap();
    assert_eq!(test_support::snapshot_file(&backup, &key).unwrap(), before);
    let book = old.upgrade().unwrap();
    let after = test_support::snapshot(&book.conn).unwrap();
    assert_eq!(after.len(), before.len() + crate::bankfeeds_matching_upgrade_tests::TABLES);
    for (table, rows) in &before {
        assert_eq!(&after[table], rows, "upgrade changed {table}");
    }
    let upgraded = schema_hashes(&book.conn).unwrap();
    for (object, hash) in &schema {
        if CHANGED.contains(&object.as_str()) {
            assert_ne!(&upgraded[object], hash, "0209 replaces {object}");
        } else if !crate::bankfeeds_matching_upgrade_tests::CHANGED.contains(&object.as_str()) {
            assert_eq!(&upgraded[object], hash, "changed prior object {object}");
        }
    }
    let added: BTreeSet<&str> = upgraded.keys().filter(|k| !schema.contains_key(*k)).map(String::as_str)
        .filter(|k| !crate::bankfeeds_matching_upgrade_tests::OBJECTS.contains(k)).collect();
    assert_eq!(added, OBJECTS.iter().copied().collect());
    // Every prior condition of the two views is kept; the sum of the deposit now counts the adjustments.
    let (invalid, expected) = (sql(&book.conn, "banking_deposit_invalid"), sql(&book.conn, "banking_deposit_expected"));
    for line in prior_invalid.lines().skip(1) {
        let line = line.trim_end_matches(';').trim_end_matches(" OR");
        if line.contains("d.amount<>fb_banking_sum") {
            assert!(invalid.contains("UNION ALL SELECT amount FROM banking_deposit_adjustment WHERE deposit_id=d.id"), "{invalid}");
        } else {
            assert!(invalid.contains(line), "kept: {line}");
        }
    }
    for line in prior_expected.lines().skip(1) {
        assert!(expected.contains(line.trim_end_matches(';')), "kept: {line}");
    }
    let fresh = Book::create(&dir.path().join("fresh.fbbook"), &key).unwrap();
    assert_eq!(upgraded, schema_hashes(&fresh.conn).unwrap());
    assert!(book.check().unwrap().is_empty());
    drop(book);
    let reopened = Book::open(&path, &key).unwrap();
    assert!(reopened.check().unwrap().is_empty());
    assert_eq!(test_support::snapshot(&reopened.conn).unwrap(), after);
}

/// The payload check (`fb_banking_payload('deposit', …)`) takes a deposit without adjustments as before and
/// refuses adjustment lines that are empty, zero, malformed, repeated or too many.
#[test]
fn the_deposit_payload_check_takes_well_formed_adjustments_only() {
    let base = serde_json::json!({"id":CO,"company_id":CO,"identity_id":CO,"bank_id":CO,"journal_id":CO,"date":"2026-08-12","amount":"825.05",
        "creation_digest":"a".repeat(64),"review_digest":"b".repeat(64),"memo":"","reference":"","actor_name":"Synthetic bookkeeper",
        "company_snapshot":{"id":CO,"name":"Synthetic payout company","currency":"USD"}});
    assert!(banking_evidence::payload("deposit", &base));
    let with = |adjustments: serde_json::Value| { let mut v = base.clone(); v["adjustments"] = adjustments; banking_evidence::payload("deposit", &v) };
    assert!(with(serde_json::json!([{"account_id":USER,"amount":"-24.95"}])));
    assert!(with(serde_json::json!([{"account_id":USER,"amount":"-24.95"},{"account_id":CO,"amount":"1.00"}])));
    for bad in [
        serde_json::json!([]),
        serde_json::json!(null),
        serde_json::json!({"account_id":USER,"amount":"-24.95"}),
        serde_json::json!([{"account_id":USER,"amount":"0.00"}]),
        serde_json::json!([{"account_id":USER,"amount":"-24.950"}]),
        serde_json::json!([{"account_id":USER,"amount":-24.95}]),
        serde_json::json!([{"account_id":"not-an-id","amount":"-24.95"}]),
        serde_json::json!([{"account_id":USER,"amount":"-24.95","memo":"x"}]),
        serde_json::json!([{"account_id":USER,"amount":"-24.95"},{"account_id":USER,"amount":"-1.00"}]),
        serde_json::Value::Array((0..21).map(|i| serde_json::json!({"account_id":format!("00000000-0000-7000-8000-0000000006{i:02}"),"amount":"-1.00"})).collect()),
    ] {
        assert!(!with(bad.clone()), "refused: {bad}");
    }
}
