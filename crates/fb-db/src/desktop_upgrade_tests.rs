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
    assert_eq!(MIGRATIONS[prefix.len()..].iter().map(|m| m.0).collect::<Vec<_>>(), [
        "0133_bankfeeds", "0134_bankfeeds_hardening", "0135_bankfeeds_activation_eligibility",
        "0136_bankfeeds_identity", "0137_bankfeeds_reconnect",
        "0140_deposit_estimates",
        "0160_plugin_credentials",
        "0161_credential_validators",
        "0162_numbering",
        "0180_company_documents",
        "0181_company_document_guards",
        "0182_company_document_data",
        "0183_company_document_json_ownership",
        "0184_release_guard_union",
        "0185_desktop_users",
        "0186_desktop_user_value_guards",
        "0187_desktop_mfa",
    ]);
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
    for (table,rows) in before {assert_eq!(after[&table],rows,"main table {table} changed");}
    let head_schema=schema_hashes(&upgraded.conn).unwrap();
    // 0140 changes exactly the formerly NULL-only estimate column into an FK.
    // Keep every other table/trigger hash and every prior row checked below.
    let deposit_sql:String=upgraded.conn.query_row("SELECT sql FROM sqlite_master WHERE type='table' AND name='prepayments_customerdeposit'",[],|r|r.get(0)).unwrap();
    assert!(prior_deposit_sql.contains("estimate_id TEXT CHECK(estimate_id IS NULL)"));
    assert_eq!(deposit_sql,prior_deposit_sql.replace("estimate_id TEXT CHECK(estimate_id IS NULL)","estimate_id TEXT REFERENCES sales_estimate(id)"));
    for (object,hash) in prior_schema {
        if !["view:fb_json_invalid","view:fb_value_invalid","table:prepayments_customerdeposit","trigger:fb_owner_user_insert","trigger:fb_owner_user_update"].contains(&object.as_str()) {
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
    assert_eq!(MIGRATIONS[prefix.len()..].iter().map(|m| m.0).collect::<Vec<_>>(), ["0162_numbering", "0180_company_documents", "0181_company_document_guards", "0182_company_document_data", "0183_company_document_json_ownership", "0184_release_guard_union", "0185_desktop_users", "0186_desktop_user_value_guards", "0187_desktop_mfa"]);
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
    assert_eq!(head_tables.difference(&prior_tables).map(String::as_str).collect::<Vec<_>>(), ["company_document_details", "desktop_mfa_recovery", "desktop_mfa_totp", "desktop_user", "fb_numbering"]);
    assert!(prior_tables.is_subset(&head_tables));
    for (table, rows) in before_rows { assert_eq!(after_rows[&table], rows, "all original rows in {table} survive"); }
    assert!(after_rows["company_document_details"].is_empty(), "existing books get no unsolicited document details");
    assert!(after_rows["fb_numbering"].is_empty(), "upgrading never opts existing books into a sequence");
    for (object, hash) in &before_schema {
        if !["view:fb_value_invalid", "view:fb_json_invalid", "trigger:fb_owner_user_insert", "trigger:fb_owner_user_update"].contains(&object.as_str()) { assert_eq!(&after_schema[object], hash, "prior object {object} survives unchanged"); }
    }
    let added: BTreeSet<_> = after_schema.keys().filter(|k| !before_schema.contains_key(*k)).map(String::as_str).collect();
    assert_eq!(added, BTreeSet::from([
        "table:desktop_user", "view:desktop_user_invalid",
        "table:desktop_mfa_totp", "table:desktop_mfa_recovery",
        "trigger:fb_valid_desktop_mfa_totp_insert", "trigger:fb_valid_desktop_mfa_totp_update",
        "trigger:fb_valid_desktop_mfa_recovery_insert", "trigger:fb_valid_desktop_mfa_recovery_update",
        "trigger:desktop_mfa_totp_identity",
        "trigger:fb_valid_desktop_user_insert", "trigger:fb_valid_desktop_user_update",
        "trigger:desktop_user_identity", "trigger:desktop_user_kept", "trigger:desktop_user_password_kept",
        "trigger:desktop_user_last_admin", "trigger:desktop_user_last_active", "trigger:desktop_user_last_member",
        "table:company_document_details",
        "trigger:company_documents_data_insert", "trigger:company_documents_data_update",
        "trigger:company_documents_identity_insert", "trigger:company_documents_identity_update", "trigger:company_documents_identity_delete",
        "trigger:fb_json_company_document_details_insert", "trigger:fb_json_company_document_details_update",
        "trigger:fb_valid_company_document_details_insert", "trigger:fb_valid_company_document_details_update",
        "table:fb_numbering",
        "trigger:fb_numbering_insert", "trigger:fb_numbering_update", "trigger:fb_numbering_delete",
        "trigger:fb_numbering_bank_identity", "trigger:fb_numbering_bank_delete",
        "trigger:fb_valid_fb_numbering_insert", "trigger:fb_valid_fb_numbering_update",
        "view:fb_check_number_history", "view:fb_numbering_invalid",
    ]));
    let fresh = Book::create(&dir.path().join("fresh.fbbook"), &key).unwrap();
    assert_eq!(after_schema, schema_hashes(&fresh.conn).unwrap());
    assert!(book.check().unwrap().is_empty());
    drop(book);
    let reopened = Book::open(&path, &key).unwrap();
    assert_eq!(test_support::snapshot(&reopened.conn).unwrap(), after_rows);
    assert_eq!(schema_hashes(&reopened.conn).unwrap(), after_schema);
    assert!(reopened.check().unwrap().is_empty());
}
