use super::{tests::Fixture, *};
use sha2::{Digest, Sha256};

fn estimate(f: &mut Fixture) -> Value {
    sales::save_estimate(&mut f.book,f.actor,f.c,&json!({"request_key":new_id(),"customer_id":f.customer,"date":"2026-10-01","tax_mode":"NONE","lines":[{"description":"Work","quantity":"1","rate":"100","taxable":false}]})).unwrap()
}

#[test]
fn linked_deposit_retains_estimate_identity_and_refuses_cross_company_raw_writes() {
    let mut f=Fixture::new();let e=estimate(&mut f);
    let mut p=f.input();p["estimate_id"]=e["id"].clone();
    let review=review_deposit(&mut f.book,f.actor,f.c,&p).unwrap();
    let receipt=record_deposit(&mut f.book,f.actor,f.c,Some(&review),&p).unwrap();
    let q=f.book.connection();
    let id:String=q.query_row("SELECT estimate_id FROM prepayments_customerdeposit WHERE receipt_id=?",[s(&receipt,"id")],|r|r.get(0)).unwrap();
    assert_eq!(id,e["id"].as_str().unwrap());
    assert!(q.execute("DELETE FROM sales_estimate WHERE id=?",[&id]).is_err());
    assert!(q.execute("DELETE FROM documents_documentidentity WHERE id=?",[s(&e,"identity_id")]).is_err());
    // Scope must reject before the older receipt/collision guards, even for raw inserts.
    let other=services::create_company(&mut f.book,f.actor,"Other company","USD",false).unwrap().id;
    let q=f.book.connection();let before=fb_db::test_support::snapshot(q).unwrap();
    q.execute_batch("SAVEPOINT scope").unwrap();
    let error=q.execute("INSERT INTO prepayments_customerdeposit SELECT ?,receipt_id,?,liability_id,estimate_id,order_reference FROM prepayments_customerdeposit WHERE receipt_id=?",params![new_id(),other.to_db(),s(&receipt,"id")]).unwrap_err();
    assert_eq!(fb_db::constraint_of(&error).as_deref(),Some("customer_deposit_estimate_scope"),"{error}");
    q.execute_batch("ROLLBACK TO scope; RELEASE scope").unwrap();
    assert_eq!(before,fb_db::test_support::snapshot(q).unwrap());
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn linked_deposit_late_failure_rolls_back_money_identity_and_detail() {
    let mut f=Fixture::new();let e=estimate(&mut f);
    let mut p=f.input();p["estimate_id"]=e["id"].clone();
    let review=review_deposit(&mut f.book,f.actor,f.c,&p).unwrap();
    f.book.connection().execute_batch("CREATE TEMP TRIGGER secfix1_fail BEFORE INSERT ON ledger_auditevent WHEN NEW.action='customer_deposit.recorded' BEGIN SELECT RAISE(ABORT,'late failure'); END").unwrap();
    let before=fb_db::test_support::snapshot(f.book.connection()).unwrap();
    assert!(record_deposit(&mut f.book,f.actor,f.c,Some(&review),&p).is_err());
    assert_eq!(before,fb_db::test_support::snapshot(f.book.connection()).unwrap());
    f.book.connection().execute_batch("DROP TRIGGER secfix1_fail").unwrap();
    record_deposit(&mut f.book,f.actor,f.c,Some(&review),&p).unwrap();
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn populated_98100e0_upgrade_preserves_every_cell_trigger_and_schema_hash() {
    let mut f=Fixture::new();f.receive();f.invoice();let _=estimate(&mut f);
    let pins:Value=serde_json::from_str(include_str!("../../../fb-db/tests/secfix1_98100e0_migrations.json")).unwrap();
    for (i,pin) in pins.as_array().unwrap().iter().enumerate() {
        assert_eq!(fb_db::MIGRATIONS[i].0,pin["name"].as_str().unwrap());
        assert_eq!(format!("{:x}",Sha256::digest(fb_db::MIGRATIONS[i].1)),pin["sha256"]);
    }
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("prior.fbbook");
    let prior=fb_db::test_support::populated_copy(f.book.connection(),&path,&[72;32],"0137_bankfeeds_reconnect",true).unwrap();
    // Later migrations introduce the empty vault, business-details and numbering tables.
    // Every table at the frozen boundary still compares in full, and opening
    // below must preserve every legacy row while adding these empty tables.
    let mut expected_before = prior.expected.clone();
    for table in ["plugin_credential", "company_document_details", "fb_numbering"] {
        assert_eq!(expected_before.remove(table), Some(Vec::new()), "the later {table} table must be empty");
        assert!(!prior.before.contains_key(table), "{table} did not exist at the frozen boundary");
    }
    assert_eq!(prior.before,expected_before);
    let book=Book::open(&path,&[72;32]).unwrap();
    assert_eq!(prior.expected,fb_db::test_support::snapshot(book.connection()).unwrap());
    let triggers:Value=serde_json::from_str(include_str!("../../../fb-db/tests/secfix1_98100e0_triggers.json")).unwrap();
    for (name,sql) in triggers.as_object().unwrap() {
        let actual:String=book.connection().query_row("SELECT sql FROM sqlite_master WHERE type='trigger' AND name=?",[name],|r|r.get(0)).unwrap();
        assert_eq!(actual,sql.as_str().unwrap());
    }
    // Book::check includes the exact schema hash generated from the migration chain.
    assert!(book.check().unwrap().is_empty());drop(book);
    assert!(Book::open(&path,&[72;32]).unwrap().check().unwrap().is_empty());
}
