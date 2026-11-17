use super::*;

#[test]
fn retained_8ff0188_preferences_book_has_verified_backup_and_additive_email_upgrade() {
    let fixture:serde_json::Value=serde_json::from_str(include_str!("../tests/email_base_migrations.json")).unwrap();
    assert_eq!(fixture["source"],"8ff5211a9e8d16c71dfc40e92431aa995131b4a4");
    let prefix=fixture["migrations"].as_array().unwrap();assert_eq!(prefix.last().unwrap()["name"],"0188_invoice_preferences");
    for (n,row) in prefix.iter().enumerate(){assert_eq!(MIGRATIONS[n].0,row["name"]);assert_eq!(format!("{:x}",Sha256::digest(MIGRATIONS[n].1.as_bytes())),row["sha256"]);}
    assert_eq!(MIGRATIONS[prefix.len()].0,"0189_invoice_email");
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("retained-0188.fbbook");let key=[68;32];
    let q=Connection::open(&path).unwrap();q.execute_batch(&format!("PRAGMA key=\"x'{}'\"",hex(&key))).unwrap();Book::configure(&q).unwrap();
    for (_,sql) in &MIGRATIONS[..prefix.len()]{q.execute_batch(sql).unwrap();}
    q.execute_batch(&format!("PRAGMA user_version={}; INSERT INTO ledger_company(id,name,currency) VALUES('00000000-0000-7000-8000-000000000068','Retained synthetic invoice seller','USD')",prefix.len())).unwrap();
    let settings=invoice_preferences::Settings{footer:"Retained invoice footer".into(),..Default::default()};
    q.execute("INSERT INTO invoice_preferences(company_id,revision,data) VALUES('00000000-0000-7000-8000-000000000068',1,?1)",[serde_json::to_string(&settings).unwrap()]).unwrap();
    let before=test_support::snapshot(&q).unwrap();let schema=schema_hashes(&q).unwrap();
    let prior_json_view:String=q.query_row("SELECT sql FROM sqlite_schema WHERE name='fb_json_invalid'",[],|r|r.get(0)).unwrap();
    let prior_view:String=q.query_row("SELECT sql FROM sqlite_schema WHERE name='fb_value_invalid'",[],|r|r.get(0)).unwrap();drop(q);
    let old=match Book::open_staged(&path,&key).unwrap(){Opened::Behind(old)=>old,_=>panic!("expected 0188")};
    let backup=dir.path().join("verified-0188.fbbook");old.snapshot(&backup,&key).unwrap();
    let copy=Connection::open(&backup).unwrap();copy.execute_batch(&format!("PRAGMA key=\"x'{}'\"",hex(&key))).unwrap();Book::configure(&copy).unwrap();
    assert_eq!(test_support::snapshot(&copy).unwrap(),before);assert_eq!(schema_hashes(&copy).unwrap(),schema);drop(copy);
    let book=old.upgrade().unwrap();let after=test_support::snapshot(&book.conn).unwrap();let upgraded=schema_hashes(&book.conn).unwrap();
    for (table,rows) in before {assert_eq!(after[&table],rows,"changed {table}");}
    for (object,hash) in schema {if !["table:documents_documentidentity","table:documents_documentsequence","trigger:fb_sales_revision_insert","trigger:fb_sales_invoice_tax_account","trigger:fb_sales_invoice_line_insert","trigger:fb_sales_invoice_seal","view:sales_snapshot_violations","view:sales_expected_entries","view:sales_expected_references","view:fb_value_invalid","view:fb_json_invalid"].contains(&object.as_str()) {assert_eq!(upgraded[&object],hash,"changed {object}");}}
    let view:String=book.conn.query_row("SELECT sql FROM sqlite_schema WHERE name='fb_value_invalid'",[],|r|r.get(0)).unwrap();assert!(view.starts_with(&prior_view));
    let json_view:String=book.conn.query_row("SELECT sql FROM sqlite_schema WHERE name='fb_json_invalid'",[],|r|r.get(0)).unwrap();assert!(json_view.starts_with(&prior_json_view));
    assert!(after["invoice_email_connection"].is_empty()&&after["invoice_email_delivery"].is_empty());
    assert!(book.check().unwrap().is_empty());drop(book);
    let reopened=Book::open(&path,&key).unwrap();assert_eq!(test_support::snapshot(&reopened.conn).unwrap(),after);assert!(reopened.check().unwrap().is_empty());
}

#[test]
fn retained_4caf0187_book_is_backed_up_upgraded_without_row_changes_and_reopened() {
    let fixture:serde_json::Value=serde_json::from_str(include_str!("../tests/invoicing_base_migrations.json")).unwrap();
    assert_eq!(fixture["source"],"4caf4c206952dcaf1c6dedeb386c2a85570d3c03");
    let prefix=fixture["migrations"].as_array().unwrap();
    assert_eq!(prefix.last().unwrap()["name"],"0187_desktop_mfa");
    for (index,row) in prefix.iter().enumerate() {
        assert_eq!(MIGRATIONS[index].0,row["name"]);
        assert_eq!(format!("{:x}",Sha256::digest(MIGRATIONS[index].1.as_bytes())),row["sha256"]);
    }
    assert_eq!(MIGRATIONS[prefix.len()].0,"0188_invoice_preferences");
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("retained-0187.fbbook");let key=[67;32];
    let q=Connection::open(&path).unwrap();q.execute_batch(&format!("PRAGMA key=\"x'{}'\"",hex(&key))).unwrap();Book::configure(&q).unwrap();
    for (_,sql) in &MIGRATIONS[..prefix.len()] {q.execute_batch(sql).unwrap();}
    q.execute_batch(&format!("PRAGMA user_version={}; INSERT INTO ledger_company(id,name,currency) VALUES('00000000-0000-7000-8000-000000000067','Retained synthetic seller','USD')",prefix.len())).unwrap();
    let details:serde_json::Value=crate::company_document_data::FIELDS.iter().map(|(key,_,_)|((*key).to_owned(),serde_json::json!(if *key=="payment_instructions" {"Original instructions\nRetained"} else {""}))).collect();
    q.execute("INSERT INTO company_document_details(company_id,data) VALUES('00000000-0000-7000-8000-000000000067',?1)",[details.to_string()]).unwrap();
    let before=test_support::snapshot(&q).unwrap();let schema=schema_hashes(&q).unwrap();drop(q);
    let old=match Book::open_staged(&path,&key).unwrap(){Opened::Behind(old)=>old,_=>panic!("expected 0187")};
    let backup=dir.path().join("verified-0187.fbbook");old.snapshot(&backup,&key).unwrap();
    let copy=Connection::open(&backup).unwrap();copy.execute_batch(&format!("PRAGMA key=\"x'{}'\"",hex(&key))).unwrap();Book::configure(&copy).unwrap();
    assert_eq!(test_support::snapshot(&copy).unwrap(),before);assert_eq!(schema_hashes(&copy).unwrap(),schema);drop(copy);
    let book=old.upgrade().unwrap();let after=test_support::snapshot(&book.conn).unwrap();let new_schema=schema_hashes(&book.conn).unwrap();
    for (table,rows) in &before {assert_eq!(&after[table],rows,"changed existing {table}");}
    for (object,hash) in schema {if !["table:documents_documentidentity","table:documents_documentsequence","trigger:fb_sales_revision_insert","trigger:fb_sales_invoice_tax_account","trigger:fb_sales_invoice_line_insert","trigger:fb_sales_invoice_seal","view:sales_snapshot_violations","view:sales_expected_entries","view:sales_expected_references","view:fb_value_invalid","view:fb_json_invalid"].contains(&object.as_str()) {assert_eq!(new_schema[&object],hash,"changed existing {object}");}}
    let view:String=book.conn.query_row("SELECT sql FROM sqlite_schema WHERE name='fb_value_invalid'",[],|r|r.get(0)).unwrap();
    let old_view=include_str!("../sql/0187_desktop_mfa.sql").split("DROP VIEW fb_value_invalid;\n").nth(1).unwrap().trim().trim_end_matches(';');
    assert!(view.starts_with(old_view),"every prior value predicate is retained verbatim");
    let json_view:String=book.conn.query_row("SELECT sql FROM sqlite_schema WHERE name='fb_json_invalid'",[],|r|r.get(0)).unwrap();
    let prior_json=include_str!("../sql/0182_company_document_data.sql").split("DROP VIEW fb_json_invalid;\n").nth(1).unwrap().trim().trim_end_matches(';');
    assert!(json_view.starts_with(prior_json));
    assert!(after["invoice_preferences"].is_empty());
    assert!(book.check().unwrap().is_empty());drop(book);
    let book=Book::open(&path,&key).unwrap();assert_eq!(test_support::snapshot(&book.conn).unwrap(),after);
    assert!(book.check().unwrap().is_empty());
}

#[test]
fn retained_13370189_email_book_has_verified_backup_and_additive_stripe_upgrade() {
    let fixture:serde_json::Value=serde_json::from_str(include_str!("../tests/stripe_base_migrations.json")).unwrap();
    assert_eq!(fixture["source"],"1337e4dc46714ef60312aec31ad89027685fb7da");
    let prefix=fixture["migrations"].as_array().unwrap();assert_eq!(prefix.last().unwrap()["name"],"0189_invoice_email");
    for (n,row) in prefix.iter().enumerate(){assert_eq!(MIGRATIONS[n].0,row["name"]);assert_eq!(format!("{:x}",Sha256::digest(MIGRATIONS[n].1.as_bytes())),row["sha256"]);}
    assert_eq!(MIGRATIONS[prefix.len()].0,"0190_invoice_stripe");
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("retained-0189.fbbook");let key=[69;32];
    let q=Connection::open(&path).unwrap();q.execute_batch(&format!("PRAGMA key=\"x'{}'\"",hex(&key))).unwrap();Book::configure(&q).unwrap();
    for (_,sql) in &MIGRATIONS[..prefix.len()]{q.execute_batch(sql).unwrap();}
    q.execute_batch(&format!("PRAGMA user_version={}; INSERT INTO ledger_company(id,name,currency) VALUES('00000000-0000-7000-8000-000000000068','Retained synthetic invoice seller','USD')",prefix.len())).unwrap();
    let settings=invoice_preferences::Settings{footer:"Retained invoice footer".into(),..Default::default()};
    q.execute("INSERT INTO invoice_preferences(company_id,revision,data) VALUES('00000000-0000-7000-8000-000000000068',1,?1)",[serde_json::to_string(&settings).unwrap()]).unwrap();
    let before=test_support::snapshot(&q).unwrap();let schema=schema_hashes(&q).unwrap();
    let prior_json_view:String=q.query_row("SELECT sql FROM sqlite_schema WHERE name='fb_json_invalid'",[],|r|r.get(0)).unwrap();
    let prior_view:String=q.query_row("SELECT sql FROM sqlite_schema WHERE name='fb_value_invalid'",[],|r|r.get(0)).unwrap();drop(q);
    let old=match Book::open_staged(&path,&key).unwrap(){Opened::Behind(old)=>old,_=>panic!("expected 0189")};
    let backup=dir.path().join("verified-0189.fbbook");old.snapshot(&backup,&key).unwrap();
    let copy=Connection::open(&backup).unwrap();copy.execute_batch(&format!("PRAGMA key=\"x'{}'\"",hex(&key))).unwrap();Book::configure(&copy).unwrap();
    assert_eq!(test_support::snapshot(&copy).unwrap(),before);assert_eq!(schema_hashes(&copy).unwrap(),schema);drop(copy);
    let book=old.upgrade().unwrap();let after=test_support::snapshot(&book.conn).unwrap();let upgraded=schema_hashes(&book.conn).unwrap();
    for (table,rows) in before {assert_eq!(after[&table],rows,"changed {table}");}
    for (object,hash) in schema {if !["table:documents_documentidentity","table:documents_documentsequence","trigger:fb_sales_revision_insert","trigger:fb_sales_invoice_tax_account","trigger:fb_sales_invoice_line_insert","trigger:fb_sales_invoice_seal","view:sales_snapshot_violations","view:sales_expected_entries","view:sales_expected_references","view:fb_value_invalid","view:fb_json_invalid"].contains(&object.as_str()) {assert_eq!(upgraded[&object],hash,"changed {object}");}}
    let view:String=book.conn.query_row("SELECT sql FROM sqlite_schema WHERE name='fb_value_invalid'",[],|r|r.get(0)).unwrap();assert!(view.starts_with(&prior_view));
    let json_view:String=book.conn.query_row("SELECT sql FROM sqlite_schema WHERE name='fb_json_invalid'",[],|r|r.get(0)).unwrap();assert!(json_view.starts_with(&prior_json_view));
    assert!(after["invoice_stripe_connection"].is_empty()&&after["invoice_stripe_link"].is_empty()&&after["invoice_stripe_payment"].is_empty());
    assert!(book.check().unwrap().is_empty());drop(book);
    let reopened=Book::open(&path,&key).unwrap();assert_eq!(test_support::snapshot(&reopened.conn).unwrap(),after);assert!(reopened.check().unwrap().is_empty());
}

#[test]
fn retained_c4310190_book_has_verified_backup_and_additive_bank_rules_upgrade() {
    let fixture:serde_json::Value=serde_json::from_str(include_str!("../tests/bank_rules_base_migrations.json")).unwrap();
    assert_eq!(fixture["source"],"c4318022e5925052444ed65c0cd5a4e78d840801");
    let prefix=fixture["migrations"].as_array().unwrap();assert_eq!(prefix.last().unwrap()["name"],"0190_invoice_stripe");
    for (n,row) in prefix.iter().enumerate(){assert_eq!(MIGRATIONS[n].0,row["name"]);assert_eq!(format!("{:x}",Sha256::digest(MIGRATIONS[n].1.as_bytes())),row["sha256"]);}
    assert_eq!(MIGRATIONS[prefix.len()].0,"0191_bank_rule_import");
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("retained-0190.fbbook");let key=[69;32];
    let q=Connection::open(&path).unwrap();q.execute_batch(&format!("PRAGMA key=\"x'{}'\"",hex(&key))).unwrap();Book::configure(&q).unwrap();
    for (_,sql) in &MIGRATIONS[..prefix.len()]{q.execute_batch(sql).unwrap();}
    q.execute_batch(&format!("PRAGMA user_version={}; INSERT INTO ledger_company(id,name,currency) VALUES('00000000-0000-7000-8000-000000000068','Retained synthetic invoice seller','USD')",prefix.len())).unwrap();
    let settings=invoice_preferences::Settings{footer:"Retained invoice footer".into(),..Default::default()};
    q.execute("INSERT INTO invoice_preferences(company_id,revision,data) VALUES('00000000-0000-7000-8000-000000000068',1,?1)",[serde_json::to_string(&settings).unwrap()]).unwrap();
    let before=test_support::snapshot(&q).unwrap();let schema=schema_hashes(&q).unwrap();
    let prior_json_view:String=q.query_row("SELECT sql FROM sqlite_schema WHERE name='fb_json_invalid'",[],|r|r.get(0)).unwrap();
    let prior_view:String=q.query_row("SELECT sql FROM sqlite_schema WHERE name='fb_value_invalid'",[],|r|r.get(0)).unwrap();drop(q);
    let old=match Book::open_staged(&path,&key).unwrap(){Opened::Behind(old)=>old,_=>panic!("expected 0190")};
    let backup=dir.path().join("verified-0190.fbbook");old.snapshot(&backup,&key).unwrap();
    let copy=Connection::open(&backup).unwrap();copy.execute_batch(&format!("PRAGMA key=\"x'{}'\"",hex(&key))).unwrap();Book::configure(&copy).unwrap();
    assert_eq!(test_support::snapshot(&copy).unwrap(),before);assert_eq!(schema_hashes(&copy).unwrap(),schema);drop(copy);
    let book=old.upgrade().unwrap();let after=test_support::snapshot(&book.conn).unwrap();let upgraded=schema_hashes(&book.conn).unwrap();
    for (table,rows) in before {assert_eq!(after[&table],rows,"changed {table}");}
    for (object,hash) in schema {if !["table:documents_documentidentity","table:documents_documentsequence","trigger:fb_sales_revision_insert","trigger:fb_sales_invoice_tax_account","trigger:fb_sales_invoice_line_insert","trigger:fb_sales_invoice_seal","view:sales_snapshot_violations","view:sales_expected_entries","view:sales_expected_references","view:fb_value_invalid","view:fb_json_invalid"].contains(&object.as_str()) {assert_eq!(upgraded[&object],hash,"changed {object}");}}
    let view:String=book.conn.query_row("SELECT sql FROM sqlite_schema WHERE name='fb_value_invalid'",[],|r|r.get(0)).unwrap();assert!(view.starts_with(&prior_view));
    let json_view:String=book.conn.query_row("SELECT sql FROM sqlite_schema WHERE name='fb_json_invalid'",[],|r|r.get(0)).unwrap();assert!(json_view.starts_with(&prior_json_view));
    assert!(after["bankfeeds_rule_policy"].is_empty()&&after["bankfeeds_auto_queue"].is_empty()&&after["bankfeeds_rule_review"].is_empty());
    assert!(book.check().unwrap().is_empty());drop(book);
    let reopened=Book::open(&path,&key).unwrap();assert_eq!(test_support::snapshot(&reopened.conn).unwrap(),after);assert!(reopened.check().unwrap().is_empty());
}

#[test]
fn retained_e92f0191_rules_upgrade_preserves_data_and_only_allows_auto_post_off() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!("../tests/bank_rules_autopost_base_migrations.json")).unwrap();
    assert_eq!(fixture["source"], "e92f67fd3e2ea2eaf2e7b2b7cf38bf619c2a5302");
    let prefix = fixture["migrations"].as_array().unwrap();
    assert_eq!(prefix.last().unwrap()["name"], "0191_bank_rule_import");
    for (n, row) in prefix.iter().enumerate() {
        assert_eq!(MIGRATIONS[n].0, row["name"]);
        assert_eq!(format!("{:x}", Sha256::digest(MIGRATIONS[n].1.as_bytes())), row["sha256"]);
    }
    assert_eq!(MIGRATIONS[prefix.len()].0, "0192_bank_rule_autopost_off");
    let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("retained-0191.fbbook"); let key = [71; 32];
    let q = Connection::open(&path).unwrap();
    q.execute_batch(&format!("PRAGMA key=\"x'{}'\"", hex(&key))).unwrap(); Book::configure(&q).unwrap();
    for (_, sql) in &MIGRATIONS[..prefix.len()] { q.execute_batch(sql).unwrap(); }
    let co = "00000000-0000-7000-8000-000000000071";
    let account = "00000000-0000-7000-8000-000000000072";
    let rule = "00000000-0000-7000-8000-000000000073";
    q.execute_batch(&format!("PRAGMA user_version={}", prefix.len())).unwrap();
    q.execute("INSERT INTO ledger_company(id,name,currency) VALUES(?1,'Retained synthetic company','USD')", [co]).unwrap();
    q.execute("INSERT INTO ledger_account(id,company_id,code,name,kind) VALUES(?1,?2,'500','Office','EXPENSE')", [account,co]).unwrap();
    q.execute("INSERT INTO bankfeeds_rule(id,company_id,position,name,match_text,sign,account_id,payee_name,active,created_at,updated_at) VALUES(?1,?2,1,'Retained imported rule','Imported conditions','OUT',?3,'',1,'2026-11-08T12:00:00.000000Z','2026-11-08T12:00:00.000000Z')", [rule,co,account]).unwrap();
    let policy = serde_json::json!({"conditions":[{"code":1,"value":"Office"}],"all":true,"direction":"OUT","scope":"ALL","auto_post":true,"kind":"EXPENSE","payee_id":"","payee_table":""}).to_string();
    q.execute("INSERT INTO bankfeeds_rule_policy(rule_id,company_id,source_key,source_hash,source_row,data) VALUES(?1,?2,?3,?4,2,?5)", [rule,co,&"a".repeat(64),&"b".repeat(64),&policy]).unwrap();
    let before = test_support::snapshot(&q).unwrap(); let schema = schema_hashes(&q).unwrap(); drop(q);
    let old = match Book::open_staged(&path,&key).unwrap() { Opened::Behind(old)=>old, _=>panic!("expected 0191") };
    let backup = dir.path().join("verified-0191.fbbook"); old.snapshot(&backup,&key).unwrap();
    let copy = Connection::open(&backup).unwrap(); copy.execute_batch(&format!("PRAGMA key=\"x'{}'\"",hex(&key))).unwrap(); Book::configure(&copy).unwrap();
    assert_eq!(test_support::snapshot(&copy).unwrap(),before); assert_eq!(schema_hashes(&copy).unwrap(),schema); drop(copy);
    let book = old.upgrade().unwrap();
    for (table,rows) in &before { assert_eq!(&test_support::snapshot(&book.conn).unwrap()[table],rows,"migration changes no existing data: {table}"); }
    let upgraded = schema_hashes(&book.conn).unwrap();
    for (name,hash) in schema { if !["table:documents_documentidentity","table:documents_documentsequence","trigger:fb_sales_revision_insert","trigger:fb_sales_invoice_tax_account","trigger:fb_sales_invoice_line_insert","trigger:fb_sales_invoice_seal","view:sales_snapshot_violations","view:sales_expected_entries","view:sales_expected_references","trigger:fb_rule_policy_fixed","view:fb_value_invalid","view:fb_json_invalid"].contains(&name.as_str()) { assert_eq!(upgraded[&name],hash,"changed {name}"); } }
    // Simultaneous policy/provenance edits must not hide behind turning off Auto-post.
    for sql in [
        "UPDATE bankfeeds_rule_policy SET source_row=3,data=json_set(data,'$.auto_post',json('false'))",
        "UPDATE bankfeeds_rule_policy SET data=json_set(data,'$.auto_post',json('false'),'$.conditions[0].value','Changed')",
        "UPDATE bankfeeds_rule_policy SET data=json_set(data,'$.auto_post',json('false'),'$.kind','TRANSFER')",
    ] { assert!(book.conn.execute(sql,[]).is_err(),"{sql}"); }
    for (table,rows) in &before {assert_eq!(&test_support::snapshot(&book.conn).unwrap()[table],rows,"{table}");}
    book.conn.execute("UPDATE bankfeeds_rule_policy SET data=json_set(data,'$.auto_post',json('false'))",[]).unwrap();
    assert!(book.conn.execute("UPDATE bankfeeds_rule_policy SET data=json_set(data,'$.auto_post',json('true'))",[]).is_err());
    assert!(book.conn.execute("DELETE FROM bankfeeds_rule_policy",[]).is_err());
    assert!(book.check().unwrap().is_empty());
    let after = test_support::snapshot(&book.conn).unwrap(); drop(book);
    let reopened = Book::open(&path,&key).unwrap(); assert_eq!(test_support::snapshot(&reopened.conn).unwrap(),after); assert!(reopened.check().unwrap().is_empty());
}

#[test]
fn retained_420b0192_history_is_pinned_and_upgrades_without_rewriting_old_objects() {
    use sha2::{Digest,Sha256};
    let fixture:serde_json::Value=serde_json::from_str(include_str!("../tests/invoice_import_base_migrations.json")).unwrap();
    assert_eq!(fixture["source"],"420b8b0f6b9792f85e9dbf6dd3a4200a00ee2d4f");
    let prefix=fixture["migrations"].as_array().unwrap();assert_eq!(prefix.last().unwrap()["name"],"0192_bank_rule_autopost_off");
    for ((name,sql),pin) in MIGRATIONS.iter().zip(prefix) {assert_eq!(*name,pin["name"]);assert_eq!(format!("{:x}",Sha256::digest(sql.as_bytes())),pin["sha256"]);}
    assert_eq!(MIGRATIONS[prefix.len()].0,"0193_invoice_adjustments");
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("retained-0192.fbbook");let key=[73;32];
    let q=Connection::open(&path).unwrap();q.execute_batch(&format!("PRAGMA key=\"x'{}'\"",hex(&key))).unwrap();Book::configure(&q).unwrap();
    for (_,sql) in &MIGRATIONS[..prefix.len()]{q.execute_batch(sql).unwrap();}
    q.execute_batch(&format!("PRAGMA user_version={}",prefix.len())).unwrap();let before=test_support::snapshot(&q).unwrap();let schema=schema_hashes(&q).unwrap();drop(q);
    let old=match Book::open_staged(&path,&key).unwrap(){Opened::Behind(old)=>old,_=>panic!("expected old schema")};
    let backup=dir.path().join("verified-0192.fbbook");old.snapshot(&backup,&key).unwrap();
    let q=Connection::open(&backup).unwrap();q.execute_batch(&format!("PRAGMA key=\"x'{}'\"",hex(&key))).unwrap();Book::configure(&q).unwrap();assert_eq!(test_support::snapshot(&q).unwrap(),before);assert_eq!(schema_hashes(&q).unwrap(),schema);drop(q);
    let book=old.upgrade().unwrap();let after=test_support::snapshot(&book.conn).unwrap();let upgraded=schema_hashes(&book.conn).unwrap();
    for (table,rows) in &before {assert_eq!(&after[table],rows,"changed {table}");}
    for (object,hash) in schema {if !["table:documents_documentidentity","table:documents_documentsequence","trigger:fb_sales_revision_insert","trigger:fb_sales_invoice_tax_account","trigger:fb_sales_invoice_line_insert","trigger:fb_sales_invoice_seal","view:sales_snapshot_violations","view:sales_expected_entries","view:sales_expected_references","view:fb_value_invalid","view:fb_json_invalid"].contains(&object.as_str()){assert_eq!(upgraded[&object],hash,"changed {object}");}}
    assert!(book.check().unwrap().is_empty());drop(book);assert!(Book::open(&path,&key).unwrap().check().unwrap().is_empty());
}
