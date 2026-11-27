//! Migration 0206 (the remote-access host record, book format 93): a book at book format 92 upgrades
//! after a verified backup, keeps existing rows, authenticates and reopens. Migration 0206 adds only
//! the `remote_host` table, which holds at most one bounded record. The later migrations extend two
//! bank-feed table definitions (0207) and rebuild the stored-value view (0208).
use super::*;
use std::collections::BTreeSet;

/// Tables 0206 adds (later upgrade tests count them).
pub(crate) const REMOTE_HOST_TABLES: usize = 1;
/// Every schema object 0206 adds. No shared view or trigger is replaced.
pub(crate) const REMOTE_HOST_OBJECTS: &[&str] = &["table:remote_host"];

const USER: &str = "00000000-0000-7000-8000-0000000003a1";
const CO: &str = "00000000-0000-7000-8000-0000000003a2";

fn boundary() -> usize {
    let n = MIGRATIONS.iter().position(|(name, _)| *name == "0206_remote_host").unwrap();
    assert_eq!(n, 92, "book format 93 is the 93rd migration");
    assert_eq!(MIGRATIONS[n - 1].0, "0205_job_tickets");
    n
}

#[test]
fn a_format_92_book_is_backed_up_upgraded_and_reopens_with_the_host_record_added() {
    let n = boundary();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("format-92.fbbook");
    let key = [92; 32];
    {
        let q = Connection::open(&path).unwrap();
        q.execute_batch(&format!("PRAGMA key=\"x'{}'\"", hex(&key))).unwrap();
        Book::configure(&q).unwrap();
        q.execute_batch("BEGIN IMMEDIATE").unwrap();
        for (_, sql) in &MIGRATIONS[..n] {
            q.execute_batch(sql).unwrap();
        }
        q.execute_batch(&format!("PRAGMA user_version={n}")).unwrap();
        q.execute("INSERT INTO fb_user(id,username) VALUES(?1,'Synthetic host owner')", [USER]).unwrap();
        q.execute("INSERT INTO ledger_company(id,name,currency) VALUES(?1,'Synthetic hosted company','USD')", [CO]).unwrap();
        q.execute("INSERT INTO ledger_company_members(company_id,user_id) VALUES(?1,?2)", [CO, USER]).unwrap();
        q.execute_batch("COMMIT").unwrap();
    }
    let (before, schema) = {
        let q = Connection::open(&path).unwrap();
        q.execute_batch(&format!("PRAGMA key=\"x'{}'\"", hex(&key))).unwrap();
        Book::configure(&q).unwrap();
        (test_support::snapshot(&q).unwrap(), schema_hashes(&q).unwrap())
    };
    let old = match Book::open_staged(&path, &key).unwrap() {
        Opened::Behind(old) => old,
        Opened::Current(_) => panic!("expected the format-92 schema"),
    };
    assert_eq!((old.from(), old.to()), (92, 97));
    let backup = dir.path().join("verified-format-92.fbbook");
    old.snapshot(&backup, &key).unwrap();
    assert_eq!(test_support::snapshot_file(&backup, &key).unwrap(), before);
    let book = old.upgrade().unwrap();
    let after = test_support::snapshot(&book.conn).unwrap();
    for (table, rows) in &before {
        assert_eq!(&after[table], rows, "upgrade changed {table}");
    }
    assert_eq!(after.len(), before.len() + REMOTE_HOST_TABLES + crate::ai_unattended_upgrade_tests::AI_UNATTENDED_TABLES + crate::bankfeeds_matching_upgrade_tests::TABLES);
    assert!(after["remote_host"].is_empty(), "no book is hosted by the upgrade");
    let upgraded = schema_hashes(&book.conn).unwrap();
    for (object, hash) in &schema {
        if ["table:bankfeeds_account", "table:bankfeeds_transaction"].contains(&object.as_str()) {
            assert_ne!(&upgraded[object], hash, "0207 must extend {object}");
        } else if object != "view:fb_value_invalid"
            && !crate::deposit_adjustments_upgrade_tests::CHANGED.contains(&object.as_str())
            && !crate::bankfeeds_matching_upgrade_tests::CHANGED.contains(&object.as_str())
        {
            assert_eq!(&upgraded[object], hash, "changed prior object {object}");
        }
    }
    let added: BTreeSet<&str> = upgraded.keys().filter(|k| !schema.contains_key(*k)).map(String::as_str)
        .filter(|k| !crate::ai_unattended_upgrade_tests::AI_UNATTENDED_OBJECTS.contains(k)
            && !crate::deposit_adjustments_upgrade_tests::OBJECTS.contains(k)
            && !crate::bankfeeds_matching_upgrade_tests::OBJECTS.contains(k)).collect();
    assert_eq!(added, REMOTE_HOST_OBJECTS.iter().copied().collect::<BTreeSet<_>>());
    let fresh = Book::create(&dir.path().join("fresh.fbbook"), &key).unwrap();
    assert_eq!(upgraded, schema_hashes(&fresh.conn).unwrap());
    assert!(book.check().unwrap().is_empty());
    // One bounded record, nothing else.
    book.conn.execute("INSERT INTO remote_host(id,record) VALUES(1,?1)", [b"{}".to_vec()]).unwrap();
    assert!(book.conn.execute("INSERT INTO remote_host(id,record) VALUES(2,?1)", [b"{}".to_vec()]).is_err(), "only one record");
    assert!(book.conn.execute("UPDATE remote_host SET record=?1", [vec![b'x'; 1_048_577]]).is_err(), "bounded");
    assert!(book.conn.execute("UPDATE remote_host SET record='{}'", []).is_err(), "bytes, not text (STRICT)");
    assert!(book.check().unwrap().is_empty());
    drop(book);
    let reopened = Book::open(&path, &key).unwrap();
    assert!(reopened.check().unwrap().is_empty());
    assert_eq!(test_support::snapshot(&reopened.conn).unwrap().len(), after.len());
}
