//! Migration 0210 (book format 97, FY2025 banking fixes) widens bank-feed matching for late-clearing
//! written checks (up to 180 days, enforcing check-number agreement when the bank row names one) and adds
//! `bankfeeds_transaction_match` so one bank row can match 2 to 20 posted entries totaling its amount.
use super::*;
use std::collections::BTreeSet;

pub(crate) const TABLES: usize = 1;
pub(crate) const CHANGED: &[&str] = &[
    "trigger:fb_bankfeeds_transaction_update",
    "trigger:fb_bankfeeds_transaction_eligible",
    "trigger:fb_bankfeeds_link_insert",
    "view:bankfeeds_invalid",
    "view:bankfeeds_text_invalid",
    "view:fb_value_invalid",
];
pub(crate) const OBJECTS: &[&str] = &[
    "table:bankfeeds_transaction_match",
    "index:bankfeeds_transaction_id_status",
    "index:bankfeeds_transaction_match_journal",
    "trigger:fb_valid_bankfeeds_transaction_match_insert",
    "trigger:fb_valid_bankfeeds_transaction_match_update",
    "trigger:fb_bankfeeds_transaction_match_insert",
    "trigger:fb_bankfeeds_transaction_match_update",
    "trigger:fb_bankfeeds_transaction_match_delete",
];

const CO: &str = "00000000-0000-7000-8000-0000000006a1";
const USER: &str = "00000000-0000-7000-8000-0000000006a2";

#[test]
fn a_format_96_book_is_backed_up_upgraded_and_reopens_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("format-96.fbbook");
    let key = [96; 32];
    let boundary = 96;
    assert_eq!(MIGRATIONS[boundary - 1].0, "0209_deposit_adjustments");
    assert_eq!(MIGRATIONS[boundary].0, "0210_bankfeeds_matching");
    assert_eq!(MIGRATIONS[boundary + 1].0, "0211_company_and_bankfeed_defaults");
    assert_eq!(MIGRATIONS.len(), boundary + 2);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&format!("PRAGMA key=\"x'{}'\"", hex(&key))).unwrap();
    Book::configure(&conn).unwrap();
    for (_, sql) in &MIGRATIONS[..boundary] {
        conn.execute_batch(sql).unwrap();
    }
    conn.execute_batch("PRAGMA user_version=96").unwrap();
    conn.execute("INSERT INTO fb_user(id,username) VALUES(?1,'Synthetic bookkeeper')", [USER]).unwrap();
    conn.execute("INSERT INTO ledger_company(id,name,currency) VALUES(?1,'Synthetic matching company','USD')", [CO]).unwrap();
    let before = test_support::snapshot(&conn).unwrap();
    let schema = schema_hashes(&conn).unwrap();
    drop(conn);

    let old = match Book::open_staged(&path, &key).unwrap() {
        Opened::Behind(old) => old,
        Opened::Current(_) => panic!("expected a format-96 book"),
    };
    assert_eq!((old.from(), old.to()), (96, 98));
    let backup = dir.path().join("verified-format-96.fbbook");
    old.snapshot(&backup, &key).unwrap();
    assert_eq!(test_support::snapshot_file(&backup, &key).unwrap(), before);
    let book = old.upgrade().unwrap();
    let after = test_support::snapshot(&book.conn).unwrap();
    assert_eq!(after.len(), before.len() + TABLES);
    let mut before = before;
    test_support::company_period_defaults(&mut before, &book.conn);
    for (table, rows) in &before {
        assert_eq!(&after[table], rows, "upgrade changed {table}");
    }
    assert!(after["bankfeeds_transaction_match"].is_empty());
    let upgraded = schema_hashes(&book.conn).unwrap();
    for (object, hash) in &schema {
        if crate::company_periods_upgrade_tests::CHANGED.contains(&object.as_str()) {
            assert_ne!(&upgraded[object], hash, "0211 extends {object}");
            continue;
        }
        if CHANGED.contains(&object.as_str()) {
            assert_ne!(&upgraded[object], hash, "0210 replaces {object}");
        } else {
            assert_eq!(&upgraded[object], hash, "changed prior object {object}");
        }
    }
    let added: BTreeSet<&str> = upgraded.keys().filter(|k| !schema.contains_key(*k)).map(String::as_str).collect();
    assert_eq!(added, OBJECTS.iter().chain(crate::company_periods_upgrade_tests::OBJECTS).copied().collect());
    let fresh = Book::create(&dir.path().join("fresh.fbbook"), &key).unwrap();
    assert_eq!(upgraded, schema_hashes(&fresh.conn).unwrap());
    assert!(book.check().unwrap().is_empty());
    drop(book);
    let reopened = Book::open(&path, &key).unwrap();
    assert!(reopened.check().unwrap().is_empty());
    assert_eq!(test_support::snapshot(&reopened.conn).unwrap(), after);
}

#[test]
fn check_number_udf_matches_rust_parser() {
    let conn = Connection::open_in_memory().unwrap();
    bankfeeds_text::register(&conn).unwrap();
    let q = |v: &str| conn.query_row("SELECT fb_bankfeeds_check_number(?1)", [v], |r| r.get::<_, Option<String>>(0)).unwrap();
    assert_eq!(q("CHECK 1003"), Some("1003".into()));
    assert_eq!(q("Check # 42 cleared"), Some("42".into()));
    assert_eq!(q("CHECK 12345678901234567890"), Some("12345678901234567890".into()));
    assert_eq!(q("CHECK 123456789012345678901"), None);
    assert_eq!(q("CHECK 123-A"), None);
    assert_eq!(q("CHECK 123/4"), None);
    assert_eq!(q("CHECK ١٢٣"), None);
    assert_eq!(q("NYS DTF SALES TAX"), None);
    assert_eq!(q("CHECKBOOK FEE"), None);
}
