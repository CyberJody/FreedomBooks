//! Migration 0200 (sales tax payments): a book at the previous schema upgrades after a verified backup, keeps
//! every row, authenticates, and reopens. The two rebuilt tables differ from their prior text only by the new
//! STP kind, and every trigger on them is recreated byte for byte.
use super::*;
use rusqlite::params;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const CO: &str = "00000000-0000-7000-8000-0000000000c1";
const USER: &str = "00000000-0000-7000-8000-0000000000c2";
const BANK: &str = "00000000-0000-7000-8000-0000000000c3";
const IDS: [(&str, &str, i64); 3] = [
    ("00000000-0000-7000-8000-0000000000d1", "INV", 1),
    ("00000000-0000-7000-8000-0000000000d2", "INV", 2),
    ("00000000-0000-7000-8000-0000000000d3", "TRF", 7),
];

fn prefix() -> usize {
    let fixture: serde_json::Value = serde_json::from_str(include_str!("../tests/salestax_payment_base_migrations.json")).unwrap();
    let pins = fixture["migrations"].as_array().unwrap();
    for ((name, sql), pin) in MIGRATIONS.iter().zip(pins) {
        assert_eq!(*name, pin["name"], "migration order changed");
        assert_eq!(format!("{:x}", Sha256::digest(sql.as_bytes())), pin["sha256"], "{name} changed after it was retained");
    }
    assert_eq!(MIGRATIONS[pins.len()].0, "0196_bankfeeds_review_safety");
    MIGRATIONS.iter().position(|(name, _)| *name == "0201_salestax_payments").unwrap()
}

fn prior_book(path: &Path, key: &Key) -> (test_support::Snapshot, SchemaHashes) {
    let n = prefix();
    let q = Connection::open(path).unwrap();
    q.execute_batch(&format!("PRAGMA key=\"x'{}'\"", hex(key))).unwrap();
    Book::configure(&q).unwrap();
    q.execute_batch("BEGIN IMMEDIATE").unwrap();
    for (_, sql) in &MIGRATIONS[..n] {
        q.execute_batch(sql).unwrap();
    }
    q.execute_batch(&format!("PRAGMA user_version={n}")).unwrap();
    q.execute("INSERT INTO fb_user(id,username) VALUES(?1,'Historical owner')", [USER]).unwrap();
    q.execute("INSERT INTO ledger_company(id,name,currency) VALUES(?1,'Synthetic retained books','USD')", [CO]).unwrap();
    q.execute("INSERT INTO ledger_account(id,company_id,code,name,kind) VALUES(?1,?2,'100','Bank','ASSET')", [BANK, CO]).unwrap();
    for (kind, last) in [("INV", 2), ("TRF", 7)] {
        q.execute("INSERT INTO documents_documentsequence(id,company_id,kind,last_number) VALUES(?1,?2,?3,?4)", params![format!("00000000-0000-7000-8000-0000000000e{last}"), CO, kind, last]).unwrap();
    }
    for (i, (id, kind, ordinal)) in IDS.iter().enumerate() {
        q.execute("INSERT INTO documents_documentidentity(id,company_id,kind,ordinal,number,request_key,currency,created_at) VALUES(?1,?2,?3,?4,?5,?6,'USD','2026-01-02T03:04:05.000000Z')",
            params![id, CO, kind, ordinal, format!("{kind}-{ordinal:06}"), format!("00000000-0000-7000-8000-0000000000f{i}")]).unwrap();
        // Child rows that reference the identity must survive the rebuild of their parent.
        q.execute("INSERT INTO documents_documentaccount VALUES(?1,?2,'bank',?3,'100','Bank','ASSET')", params![format!("00000000-0000-7000-8000-0000000000a{i}"), id, BANK]).unwrap();
    }
    q.execute_batch("COMMIT").unwrap();
    let before = test_support::snapshot(&q).unwrap();
    let schema = schema_hashes(&q).unwrap();
    (before, schema)
}

const REBUILT: [&str; 2] = ["table:documents_documentidentity", "table:documents_documentsequence"];

#[test]
fn a_book_at_the_previous_schema_is_backed_up_upgraded_and_reopens_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retained-0195.fbbook");
    let key = [57; 32];
    let (before, schema) = prior_book(&path, &key);
    let old = match Book::open_staged(&path, &key).unwrap() {
        Opened::Behind(old) => old,
        Opened::Current(_) => panic!("expected the previous schema"),
    };
    assert_eq!(old.to() - old.from(), 6);
    // The verified backup is the prior book, cell for cell and object for object.
    let backup = dir.path().join("verified-0195.fbbook");
    old.snapshot(&backup, &key).unwrap();
    assert_eq!(test_support::snapshot_file(&backup, &key).unwrap(), before);
    let copy = Connection::open(&backup).unwrap();
    copy.execute_batch(&format!("PRAGMA key=\"x'{}'\"", hex(&key))).unwrap();
    Book::configure(&copy).unwrap();
    assert_eq!(schema_hashes(&copy).unwrap(), schema);
    drop(copy);

    let book = old.upgrade().unwrap();
    let after = test_support::snapshot(&book.conn).unwrap();
    for (table, rows) in &before {
        assert_eq!(&after[table], rows, "upgrade changed {table}");
    }
    assert_eq!(after.len(), before.len() + 4 + crate::job_tickets_upgrade_tests::JOB_TICKET_TABLES + crate::remote_host_upgrade_tests::REMOTE_HOST_TABLES);
    assert!(after["salestax_payment"].is_empty() && after["salestax_paymentvoid"].is_empty());

    let upgraded = schema_hashes(&book.conn).unwrap();
    for (object, hash) in &schema {
        if !REBUILT.contains(&object.as_str()) && !["view:fb_value_invalid","view:fb_json_invalid"].contains(&object.as_str()) {
            assert_eq!(&upgraded[object], hash, "changed prior object {object}");
        }
    }
    let sql = |c: &Connection, name: &str| -> String { c.query_row("SELECT sql FROM sqlite_schema WHERE name=?1", [name], |r| r.get(0)).unwrap() };
    let prior = Connection::open(&backup).unwrap();
    prior.execute_batch(&format!("PRAGMA key=\"x'{}'\"", hex(&key))).unwrap();
    Book::configure(&prior).unwrap();
    for table in ["documents_documentidentity", "documents_documentsequence"] {
        assert_eq!(sql(&book.conn, table), sql(&prior, table).replace("'CDA','CDR'))", "'CDA','CDR','STP'))"), "{table} may change only by the new kind");
        // Every trigger on the rebuilt table is still there, with its exact prior text.
        let triggers = |c: &Connection| -> Vec<(String, String)> {
            c.prepare("SELECT name,sql FROM sqlite_schema WHERE type='trigger' AND tbl_name=?1 ORDER BY name").unwrap()
                .query_map([table], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap()
        };
        assert!(!triggers(&prior).is_empty());
        assert_eq!(triggers(&book.conn), triggers(&prior), "triggers on {table}");
    }
    let prior_view = sql(&prior, "fb_value_invalid");
    let next_view = sql(&book.conn, "fb_value_invalid");
    // Integration inserts the tax arms before the retained attachment arm. Every
    // prior predicate still appears verbatim, irrespective of UNION ordering.
    for arm in prior_view.trim_start_matches("CREATE VIEW fb_value_invalid AS").split("UNION ALL") {
        assert!(next_view.contains(arm.trim().trim_end_matches(';')), "lost value predicate: {arm}");
    }
    let added: BTreeSet<&str> = upgraded.keys().filter(|k| !schema.contains_key(*k)).map(String::as_str)
        .filter(|k| !crate::job_tickets_upgrade_tests::JOB_TICKET_OBJECTS.contains(k) && !crate::remote_host_upgrade_tests::REMOTE_HOST_OBJECTS.contains(k)).collect();
    assert_eq!(added, BTreeSet::from([
        "table:salestax_payment", "table:salestax_paymentvoid", "view:salestax_payment_invalid",
        // 0204 adds these objects; every prior object outside the two shared views stays exact.
        "table:payables_recurring", "table:payables_recurring_occurrence", "index:payables_recurring_company", "index:payables_recurring_generated_audit", "index:payables_recurring_skipped_audit", "view:payables_recurring_invalid",
        "trigger:recurring_identity_insert", "trigger:recurring_identity_update", "trigger:recurring_no_delete",
        "trigger:recurring_occurrence_scope", "trigger:recurring_occurrence_update", "trigger:recurring_occurrence_delete",
        "trigger:fb_valid_payables_recurring_insert", "trigger:fb_valid_payables_recurring_update",
        "trigger:fb_valid_payables_recurring_occurrence_insert", "trigger:fb_valid_payables_recurring_occurrence_update",
        "trigger:fb_json_payables_recurring_insert", "trigger:fb_json_payables_recurring_update",
        "trigger:recurring_data_insert", "trigger:recurring_data_update",
        "trigger:fb_salestax_payment_insert", "trigger:fb_salestax_payment_update", "trigger:fb_salestax_payment_delete",
        "trigger:fb_salestax_paymentvoid_insert", "trigger:fb_salestax_paymentvoid_update", "trigger:fb_salestax_paymentvoid_delete",
        "trigger:fb_salestax_journal_due", "trigger:fb_salestax_journal_seal", "trigger:fb_salestax_late_reference",
        "trigger:fb_valid_salestax_payment_insert", "trigger:fb_valid_salestax_payment_update",
        "trigger:fb_valid_salestax_paymentvoid_insert", "trigger:fb_valid_salestax_paymentvoid_update",
    ]));
    // An upgraded book has exactly the schema of a new one, and the new kind works on the old rows' tables.
    let fresh = Book::create(&dir.path().join("fresh.fbbook"), &key).unwrap();
    assert_eq!(upgraded, schema_hashes(&fresh.conn).unwrap());
    assert!(book.check().unwrap().is_empty());
    book.conn.execute_batch("BEGIN").unwrap();
    book.conn.execute("INSERT INTO documents_documentidentity(id,company_id,kind,ordinal,number,request_key,currency,created_at) VALUES('00000000-0000-7000-8000-0000000000b9',?1,'STP',1,'STP-000001','00000000-0000-7000-8000-0000000000b8','USD','2026-11-11T00:00:00.000000Z')", [CO]).unwrap();
    // Old rules still hold on the rebuilt tables: a number must match its kind and ordinal.
    assert!(book.conn.execute("INSERT INTO documents_documentidentity(id,company_id,kind,ordinal,number,request_key,currency,created_at) VALUES('00000000-0000-7000-8000-0000000000b7',?1,'STP',2,'STP-000009','00000000-0000-7000-8000-0000000000b6','USD','2026-11-11T00:00:00.000000Z')", [CO]).is_err());
    assert!(book.conn.execute("DELETE FROM documents_documentidentity WHERE id=?1", [IDS[0].0]).is_err());
    assert!(book.conn.execute("INSERT INTO documents_documentidentity(id,company_id,kind,ordinal,number,request_key,currency,created_at) VALUES('00000000-0000-7000-8000-0000000000b5',?1,'XYZ',1,'XYZ-000001','00000000-0000-7000-8000-0000000000b4','USD','2026-11-11T00:00:00.000000Z')", [CO]).is_err());
    book.conn.execute_batch("ROLLBACK").unwrap();
    drop(book);
    let reopened = Book::open(&path, &key).unwrap();
    assert_eq!(test_support::snapshot(&reopened.conn).unwrap(), after);
    assert!(reopened.check().unwrap().is_empty());
}

#[test]
fn a_damaged_rebuild_is_refused_at_open() {
    // A book whose rebuilt identity table lost one of its guards no longer authenticates.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("damaged.fbbook");
    let key = [58; 32];
    let book = Book::create(&path, &key).unwrap();
    book.conn.execute_batch("DROP TRIGGER fb_checks_documents_documentidentity_kept").unwrap();
    assert!(!book.check().unwrap().is_empty());
}
