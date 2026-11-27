//! Actual original-source books, not newer schemas relabelled with old ordinals.
use super::*;

fn fixtures() -> serde_json::Value {
    serde_json::from_str(include_str!("../tests/data/retained-lanes/manifest.json")).unwrap()
}

#[test]
fn original_purchasing_and_tax_books_are_backed_up_upgraded_and_reopened() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/retained-lanes");
    for (lane, fixture) in fixtures().as_object().unwrap() {
        let pins = fixture["migrations"].as_array().unwrap();
        let layout = legacy::histories().into_iter().find(|h| h.len() == pins.len()
            && h.iter().zip(pins).all(|((name, _), pin)| Some(*name) == pin["name"].as_str())).expect("retained history must stay supported");
        for ((name, sql), pin) in layout.iter().zip(pins) {
            assert_eq!(format!("{:x}", Sha256::digest(sql.as_bytes())), pin["sha256"], "changed {lane} migration {name}");
        }
        let key = [fixture["key_byte"].as_u64().unwrap() as u8; 32];
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("original.fbbook");
        std::fs::copy(root.join(format!("{lane}.fbbook")), &path).unwrap();
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&format!("PRAGMA key=\"x'{}'\"", hex(&key))).unwrap();
        Book::configure_connection(&conn).unwrap();
        assert_eq!(conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), layout.len() as i64);
        let before = test_support::snapshot(&conn).unwrap();
        let schema = schema_hashes(&conn).unwrap();
        assert!(!before["ledger_auditevent"].is_empty());
        assert!(!before[if lane == "tax" { "salestax_payment" } else { "payables_billpayment" }].is_empty());
        drop(conn);
        let old = match Book::open_staged(&path, &key).unwrap() {
            Opened::Behind(old) => old,
            Opened::Current(_) => panic!("old {lane} book should need an upgrade"),
        };
        assert_eq!(old.migrations(), layout);
        let backup = dir.path().join("verified-before.fbbook");
        old.snapshot(&backup, &key).unwrap();
        assert_eq!(test_support::snapshot_file(&backup, &key).unwrap(), before);
        let copy = Connection::open(&backup).unwrap();
        copy.execute_batch(&format!("PRAGMA key=\"x'{}'\"", hex(&key))).unwrap();
        Book::configure_connection(&copy).unwrap();
        assert_eq!(schema_hashes(&copy).unwrap(), schema);
        drop(copy);
        let book = old.upgrade().unwrap();
        let after = test_support::snapshot(&book.conn).unwrap();
        let mut before = before;
        test_support::company_period_defaults(&mut before, &book.conn);
        for (table, rows) in before { assert_eq!(after[&table], rows, "{lane}: changed {table}"); }
        let fresh = Book::create(&dir.path().join("fresh.fbbook"), &key).unwrap();
        assert_eq!(schema_hashes(&book.conn).unwrap(), schema_hashes(&fresh.conn).unwrap());
        assert!(book.check().unwrap().is_empty());
        drop(book);
        let reopened = Book::open(&path, &key).unwrap();
        assert_eq!(test_support::snapshot(&reopened.conn).unwrap(), after);
        assert!(reopened.check().unwrap().is_empty());
        drop(reopened);

        // A familiar ordinal alone must never authenticate a damaged historical book.
        let damaged = dir.path().join("damaged.fbbook");
        std::fs::copy(root.join(format!("{lane}.fbbook")), &damaged).unwrap();
        let conn = Connection::open(&damaged).unwrap();
        conn.execute_batch(&format!("PRAGMA key=\"x'{}'\"; DROP TRIGGER fb_valid_ledger_company_insert", hex(&key))).unwrap();
        drop(conn);
        let bytes = std::fs::read(&damaged).unwrap();
        assert!(matches!(Book::open_staged(&damaged, &key), Err(DbError::Integrity(_))));
        assert_eq!(std::fs::read(&damaged).unwrap(), bytes, "refusal must not migrate {lane}");
    }
}
