use super::*;

const COMPANY: &str="00000000-0000-7000-8000-000000000001";
const DETAILS: &str=r#"{"street1":"Historical seller","street2":"","city":"","region":"","postal_code":"","country":"","phone":"","email":"","website":"","tax_id":"","payment_instructions":"Historical payment text"}"#;

fn histories() -> serde_json::Value {
    serde_json::from_str(include_str!("../tests/release_lane_migrations.json")).unwrap()
}
fn installed(value: &serde_json::Value) -> Vec<(&'static str,&'static str)> {
    value["migrations"].as_array().unwrap().iter().map(|row| {
        let migration=*MIGRATIONS.iter().find(|m|Some(m.0)==row["name"].as_str()).unwrap();
        let actual:String=Sha256::digest(migration.1.as_bytes()).iter().map(|b|format!("{b:02x}")).collect();
        assert_eq!(actual,row["sha256"].as_str().unwrap(),"source migration changed: {}",migration.0);
        migration
    }).collect()
}

#[test]
fn release_integration_preserves_both_value_view_arms() {
    let dir=tempfile::tempdir().unwrap();let book=Book::create(&dir.path().join("union.fbbook"),&[61;32]).unwrap();
    let view:String=book.conn.query_row("SELECT sql FROM sqlite_schema WHERE name='fb_value_invalid'",[],|r|r.get(0)).unwrap();
    for table in ["fb_numbering","company_document_details"] {
        assert!(view.contains(&format!("FROM {table} WHERE")),"lost {table} value-view arm");
    }
}

#[test]
fn release_integration_authenticates_populated_source_histories_and_reopens() {
    for (label,fixture) in histories().as_object().unwrap() {
        let layout=installed(fixture);let names:Vec<String>=layout.iter().map(|m|m.0.to_owned()).collect();
        assert!(supported_migrations(&names),"unsupported genuine {label} history");
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("prior.fbbook");let key=[62;32];
        let conn=Connection::open(&path).unwrap();conn.execute_batch(&format!("PRAGMA key=\"x'{}'\"",hex(&key))).unwrap();Book::configure(&conn).unwrap();
        for (_,sql) in &layout {conn.execute_batch(sql).unwrap();}
        conn.execute_batch(&format!("PRAGMA user_version={}",layout.len())).unwrap();
        conn.execute("INSERT INTO ledger_company(id,name,currency) VALUES(?1,'Synthetic historical seller','USD')",[COMPANY]).unwrap();
        if layout.iter().any(|m|m.0=="0180_company_documents") {
            conn.execute("INSERT INTO company_document_details(company_id,data) VALUES(?1,?2)",[COMPANY,DETAILS]).unwrap();
        }
        if layout.iter().any(|m|m.0=="0162_numbering") {
            // A real persisted preference with its actor and matching append-only
            // evidence must survive, not just the empty numbering table.
            conn.execute_batch("INSERT INTO fb_user(id,username) VALUES('00000000-0000-7000-8000-000000000002','Historical owner');
                INSERT INTO ledger_auditevent(id,company_id,actor_id,actor_name,occurred_at,action,target,detail)
                VALUES('00000000-0000-7000-8000-000000000003','00000000-0000-7000-8000-000000000001','00000000-0000-7000-8000-000000000002','Historical owner','2026-01-01T00:00:00.000000Z','numbering.changed','INV','{\"kind\":\"INV\",\"scope\":\"\",\"old_start\":null,\"new_start\":\"100\",\"revision\":1}');
                INSERT INTO fb_numbering(company_id,kind,scope,start,revision,audit_id)
                VALUES('00000000-0000-7000-8000-000000000001','INV','','100',1,'00000000-0000-7000-8000-000000000003');").unwrap();
        }
        let before=test_support::snapshot(&conn).unwrap();let prior_schema=schema_hashes(&conn).unwrap();drop(conn);
        let old=match Book::open_staged(&path,&key).unwrap() {Opened::Behind(old)=>old,Opened::Current(_)=>panic!("{label} unexpectedly current")};
        assert_eq!(old.migrations().iter().map(|m|m.0.to_owned()).collect::<Vec<_>>(),names);
        let backup=dir.path().join("verified-before.fbbook");old.snapshot(&backup,&key).unwrap();
        let copy=Connection::open(&backup).unwrap();copy.execute_batch(&format!("PRAGMA key=\"x'{}'\"",hex(&key))).unwrap();Book::configure(&copy).unwrap();
        assert_eq!(test_support::snapshot(&copy).unwrap(),before);assert_eq!(schema_hashes(&copy).unwrap(),prior_schema);drop(copy);
        let book=old.upgrade().unwrap();let after=test_support::snapshot(&book.conn).unwrap();
        for (table,rows) in &before {assert_eq!(&after[table],rows,"{label}: changed {table}");}
        assert_eq!(after.len(),before.len()+usize::from(!before.contains_key("fb_numbering"))+usize::from(!before.contains_key("company_document_details")));
        let upgraded_schema=schema_hashes(&book.conn).unwrap();
        for (object,hash) in prior_schema {
            if object!="view:fb_value_invalid" && object!="view:fb_json_invalid" {assert_eq!(upgraded_schema[&object],hash,"{label}: changed {object}");}
        }
        let fresh=Book::create(&dir.path().join("fresh.fbbook"),&key).unwrap();assert_eq!(schema_hashes(&book.conn).unwrap(),schema_hashes(&fresh.conn).unwrap());
        assert!(book.check().unwrap().is_empty());drop(book);
        let reopened=Book::open(&path,&key).unwrap();assert_eq!(test_support::snapshot(&reopened.conn).unwrap(),after);assert!(reopened.check().unwrap().is_empty());
    }
}

#[test]
fn release_integration_tampered_source_history_is_refused_without_migration() {
    let fixtures=histories();let layout=installed(&fixtures["documents0183"]);
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("tampered.fbbook");let key=[63;32];
    let conn=Connection::open(&path).unwrap();conn.execute_batch(&format!("PRAGMA key=\"x'{}'\"",hex(&key))).unwrap();Book::configure(&conn).unwrap();
    for (_,sql) in &layout {conn.execute_batch(sql).unwrap();}
    conn.execute_batch(&format!("PRAGMA user_version={}; DROP TRIGGER company_documents_data_update",layout.len())).unwrap();
    let before=schema_hashes(&conn).unwrap();drop(conn);
    assert!(matches!(Book::open_staged(&path,&key),Err(DbError::Integrity(_))));
    let conn=Connection::open(&path).unwrap();conn.execute_batch(&format!("PRAGMA key=\"x'{}'\"",hex(&key))).unwrap();Book::configure(&conn).unwrap();
    assert_eq!(schema_hashes(&conn).unwrap(),before);assert_eq!(conn.query_row("PRAGMA user_version",[],|r|r.get::<_,i64>(0)).unwrap(),layout.len() as i64);
}
