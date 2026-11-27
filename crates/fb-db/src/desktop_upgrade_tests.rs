use super::*;

#[test]
fn main_b0b216a_prefix_is_byte_exact_and_bankfeeds_append_in_order() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!("../tests/main_b0b216a_migrations.json")).unwrap();
    let prefix = fixture.as_array().unwrap();
    for (n, row) in prefix.iter().enumerate() {
        assert_eq!(MIGRATIONS[n].0, row["name"].as_str().unwrap());
        let hash: String = Sha256::digest(MIGRATIONS[n].1.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hash, row["sha256"].as_str().unwrap(), "main migration changed");
    }
    assert_eq!(MIGRATIONS[prefix.len()].0, "0133_bankfeeds");
}

#[test]
fn main_d14de5e_migration_prefix_is_byte_exact_and_estimates_only_append() {
    let fixture: serde_json::Value=serde_json::from_str(include_str!("../tests/main_d14de5e_migrations.json")).unwrap();
    let prefix=fixture.as_array().unwrap();
    for (n,row) in prefix.iter().enumerate() {
        assert_eq!(MIGRATIONS[n].0,row["name"].as_str().unwrap());
        let hash:String=Sha256::digest(MIGRATIONS[n].1.as_bytes()).iter().map(|b|format!("{b:02x}")).collect();
        assert_eq!(hash,row["sha256"].as_str().unwrap(),"{} changed from genuine main",MIGRATIONS[n].0);
    }
    assert_eq!(MIGRATIONS[prefix.len()].0,"0131_estimates");
    assert_eq!(MIGRATIONS[prefix.len()+1].0,"0132_estimate_validators");
}

#[test]
fn genuine_main_schema_upgrades_without_row_loss_and_hashes_equal_a_fresh_book() {
    let dir=tempfile::tempdir().unwrap();
    let path=dir.path().join("main.fbbook");let key=[38;32];
    let conn=Connection::open(&path).unwrap();
    conn.execute_batch(&format!("PRAGMA key=\"x'{}'\"",hex(&key))).unwrap();
    Book::configure(&conn).unwrap();
    let end=MIGRATIONS.iter().position(|(name,_)|*name=="0130_shared_validators").unwrap()+1;
    for (_,sql) in &MIGRATIONS[..end] {conn.execute_batch(sql).unwrap();}
    conn.execute_batch(&format!("PRAGMA user_version={end}; INSERT INTO ledger_company(id,name,currency) VALUES('00000000-0000-7000-8000-000000000001','Synthetic main company','USD')")).unwrap();
    let before=test_support::snapshot(&conn).unwrap();
    let prior_schema=schema_hashes(&conn).unwrap();
    let prior_deposit_sql:String=conn.query_row("SELECT sql FROM sqlite_master WHERE type='table' AND name='prepayments_customerdeposit'",[],|r|r.get(0)).unwrap();
    drop(conn);
    let upgraded=Book::open(&path,&key).unwrap();
    let fresh=Book::create(&dir.path().join("fresh.fbbook"),&key).unwrap();
    let after=test_support::snapshot(&upgraded.conn).unwrap();
    let mut before = before;
    test_support::company_period_defaults(&mut before, &upgraded.conn);
    for (table,rows) in before {assert_eq!(after[&table],rows,"main table {table} changed");}
    let head_schema=schema_hashes(&upgraded.conn).unwrap();
    // 0140 changes exactly the formerly NULL-only estimate column into an FK.
    // Keep every other table/trigger hash and every prior row checked below.
    let deposit_sql:String=upgraded.conn.query_row("SELECT sql FROM sqlite_master WHERE type='table' AND name='prepayments_customerdeposit'",[],|r|r.get(0)).unwrap();
    assert!(prior_deposit_sql.contains("estimate_id TEXT CHECK(estimate_id IS NULL)"));
    assert_eq!(deposit_sql,prior_deposit_sql.replace("estimate_id TEXT CHECK(estimate_id IS NULL)","estimate_id TEXT REFERENCES sales_estimate(id)"));
    for (object,hash) in prior_schema {
        if crate::company_periods_upgrade_tests::CHANGED.contains(&object.as_str()) { continue; }
        if !["view:banking_deposit_expected","view:banking_deposit_invalid","table:documents_documentidentity","table:documents_documentsequence","trigger:fb_sales_revision_insert","trigger:fb_sales_invoice_tax_account","trigger:fb_sales_invoice_line_insert","trigger:fb_sales_invoice_seal","view:sales_snapshot_violations","view:sales_expected_entries","view:sales_expected_references","view:fb_json_invalid","view:fb_value_invalid","table:prepayments_customerdeposit","trigger:fb_owner_user_insert","trigger:fb_owner_user_update"].contains(&object.as_str()) {
            assert_eq!(head_schema[&object],hash,"main object {object} changed");
        }
    }
    assert_eq!(schema_hashes(&upgraded.conn).unwrap(),schema_hashes(&fresh.conn).unwrap());
    assert!(upgraded.check().unwrap().is_empty());
    assert_eq!(upgraded.conn.query_row("PRAGMA user_version",[],|r|r.get::<_,i64>(0)).unwrap(),MIGRATIONS.len() as i64);
}


#[test]
fn original_onboarding_base_upgrades_with_exact_rows_and_schema_inventory() {
    use std::collections::BTreeSet;
    let fixture: serde_json::Value = serde_json::from_str(include_str!("../tests/onboarding_base_migrations.json")).unwrap();
    assert_eq!(fixture["source"], "1edb1b2a2fd79a7288cf7316566df4c302b92568");
    let prefix = fixture["migrations"].as_array().unwrap();
    assert_eq!(prefix.last().unwrap()["name"], "0161_credential_validators");
    for (n, row) in prefix.iter().enumerate() {
        assert_eq!(MIGRATIONS[n].0, row["name"].as_str().unwrap());
        let hash: String = Sha256::digest(MIGRATIONS[n].1.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hash, row["sha256"], "{} changed from assigned original base", MIGRATIONS[n].0);
    }
    assert_eq!(MIGRATIONS[prefix.len()].0, "0162_numbering");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("original-base.fbbook");
    let key = [83;32];
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&format!("PRAGMA key=\"x'{}'\"", hex(&key))).unwrap();
    Book::configure(&conn).unwrap();
    // Install exactly the authenticated old boundary, rather than making a head book look old.
    for (_, sql) in &MIGRATIONS[..prefix.len()] { conn.execute_batch(sql).unwrap(); }
    conn.execute_batch(&format!("PRAGMA user_version={}; INSERT INTO ledger_company(id,name,currency) VALUES('00000000-0000-7000-8000-000000000083','Original synthetic company','USD');", prefix.len())).unwrap();
    let before_rows = test_support::snapshot(&conn).unwrap();
    let before_schema = schema_hashes(&conn).unwrap();
    drop(conn);
    let book = Book::open(&path, &key).unwrap();
    let after_rows = test_support::snapshot(&book.conn).unwrap();
    let after_schema = schema_hashes(&book.conn).unwrap();
    let prior_tables: BTreeSet<_> = before_rows.keys().cloned().collect();
    let head_tables: BTreeSet<_> = after_rows.keys().cloned().collect();
    assert!(prior_tables.is_subset(&head_tables));
    let mut before_rows = before_rows;
    test_support::company_period_defaults(&mut before_rows, &book.conn);
    for (table, rows) in before_rows { assert_eq!(after_rows[&table], rows, "all original rows in {table} survive"); }
    assert!(after_rows["company_document_details"].is_empty(), "existing books get no unsolicited document details");
    assert!(after_rows["fb_numbering"].is_empty(), "upgrading never opts existing books into a sequence");
    for (object, hash) in &before_schema {
        if !["table:bankfeeds_account","table:bankfeeds_transaction","view:banking_deposit_expected","view:banking_deposit_invalid","table:documents_documentidentity","table:documents_documentsequence","trigger:fb_sales_revision_insert","trigger:fb_sales_invoice_tax_account","trigger:fb_sales_invoice_line_insert","trigger:fb_sales_invoice_seal","view:sales_snapshot_violations","view:sales_expected_entries","view:sales_expected_references","view:fb_value_invalid", "view:fb_json_invalid", "trigger:fb_owner_user_insert", "trigger:fb_owner_user_update"].contains(&object.as_str()) && !crate::bankfeeds_matching_upgrade_tests::CHANGED.contains(&object.as_str()) && !crate::company_periods_upgrade_tests::CHANGED.contains(&object.as_str()) { assert_eq!(&after_schema[object], hash, "prior object {object} survives unchanged"); }
    }
    let fresh = Book::create(&dir.path().join("fresh.fbbook"), &key).unwrap();
    assert_eq!(after_schema, schema_hashes(&fresh.conn).unwrap());
    assert!(book.check().unwrap().is_empty());
    drop(book);
    let reopened = Book::open(&path, &key).unwrap();
    assert_eq!(test_support::snapshot(&reopened.conn).unwrap(), after_rows);
    assert_eq!(schema_hashes(&reopened.conn).unwrap(), after_schema);
    assert!(reopened.check().unwrap().is_empty());
}


#[test]
fn retained_bank_and_attachment_migrations_remain_byte_exact() {
    let pins: serde_json::Value = serde_json::from_str(include_str!("../tests/attachment_review_migrations.json")).unwrap();
    for pin in pins.as_array().unwrap() {
        let name = pin["name"].as_str().unwrap();
        let sql = MIGRATIONS.iter().find(|m| m.0 == name).unwrap().1;
        let hash: String = Sha256::digest(sql.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hash, pin["sha256"].as_str().unwrap(), "retained migration {name}");
    }
}

#[test]
fn unmigrated_preflight_is_read_only_and_leaves_the_schema_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("prior.fbbook");
    let key = [23; 32];
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(&format!("PRAGMA key=\"x'{}'\"", "17".repeat(32))).unwrap();
    Book::configure(&conn).unwrap();
    for (_, sql) in &MIGRATIONS[..MIGRATIONS.len() - 1] { conn.execute_batch(sql).unwrap(); }
    conn.execute_batch(&format!("PRAGMA user_version={}", MIGRATIONS.len() - 1)).unwrap();
    drop(conn);
    let Opened::Behind(mut old) = Book::open_staged(&path, &key).unwrap() else { panic!("expected an older book") };
    let version = old.from();
    let refused = old.read(|q| q.query_row("UPDATE fb_user SET username='changed' RETURNING id", [], |r| r.get::<_, String>(0)));
    assert!(refused.is_err(), "preflight cannot write even through a query callback");
    let after: i64 = old.read(|q| q.query_row("PRAGMA user_version", [], |r| r.get(0))).unwrap();
    assert_eq!(after as usize, version);
    assert!(old.upgrade().unwrap().check().unwrap().is_empty(), "the refused callback leaves a usable connection");
}
