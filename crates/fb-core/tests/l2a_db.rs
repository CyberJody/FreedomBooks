//! Migration and open-time integrity backstops for the new tables.
mod common;
use common::*;
use fb_core::{
    Book,
    directory::{self, Model},
    documents as d, salestax,
};
use fb_db::rusqlite::{Connection, params};
use serde_json::json;
#[test]
fn a_layer_one_book_migrates_with_its_existing_chart_intact() {
    let path = book_path("l1-migration");
    let conn = Connection::open(&path).unwrap();
    let hex = KEY.iter().map(|x| format!("{x:02x}")).collect::<String>();
    conn.execute_batch(&format!(
        "PRAGMA key=\"x'{hex}'\"; PRAGMA foreign_keys=ON; PRAGMA recursive_triggers=ON;"
    ))
    .unwrap();
    conn.execute_batch(fb_db::MIGRATIONS[0].1).unwrap();
    let c = fb_core::CompanyId::new();
    let a = fb_core::AccountId::new();
    conn.execute(
        "INSERT INTO ledger_company(id,name,currency)VALUES(?1,'Old book','USD')",
        [c.to_db()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO ledger_account(id,company_id,code,name,kind)VALUES(?1,?2,'100-1','Old asset','ASSET')",
        params![a.to_db(), c.to_db()],
    )
    .unwrap();
    conn.execute_batch("PRAGMA user_version=1").unwrap();
    drop(conn);
    let book = Book::open(&path, &KEY).unwrap();
    assert!(book.check().unwrap().is_empty());
    assert_eq!(fb_core::store::account(book.connection(), a).unwrap().unwrap().name, "Old asset");
}
#[test]
fn deleted_new_guard_is_refused_and_cross_company_damage_is_detected() {
    let path = book_path("l2-integrity");
    let mut b = Book::create(&path, &KEY).unwrap();
    let u = fb_core::users::create_user(&mut b, "writer", false).unwrap();
    let c = fb_core::services::create_company(&mut b, u, "C", "USD", false).unwrap().id;
    let o = fb_core::services::create_company(&mut b, u, "O", "USD", false).unwrap().id;
    let v = directory::save_record(&mut b, u, c, Model::Vendor, &json!({"display_name":"Agency"}), None).unwrap();
    let foreign = directory::save_record(&mut b, u, o, Model::Vendor, &json!({"display_name":"Foreign"}), None).unwrap();
    let t = salestax::save_rate(
        &mut b,
        u,
        c,
        &json!({"name":"Tax","description":"Tax","rate":"3","agency_id":v["id"]}),
    )
    .unwrap();
    b.connection().execute_batch("DROP TRIGGER fb_salestax_item_agency_update; DROP TRIGGER fb_l2_tax_agency_update").unwrap();
    b.connection()
        .execute(
            "UPDATE salestax_taxitem SET agency_id=?1 WHERE id=?2",
            params![foreign["id"].as_str(), t["id"].as_str()],
        )
        .unwrap();
    // check() diagnoses data and definitions together; open refuses the changed
    // schema before migrations or row checks, preserving the L1 fail-closed gate.
    let problems = b.check().unwrap();
    assert!(problems.iter().any(|p| p.contains("fb_salestax_item_agency_update")));
    assert!(problems.iter().any(|p| p.contains("sales tax agency outside")));
    drop(b);
    match Book::open(&path, &KEY) {
        Err(fb_db::DbError::Integrity(problems)) => {
            assert!(problems.iter().any(|p| p.contains("fb_salestax_item_agency_update")));
        }
        _ => panic!("tampered book opened"),
    }
}
#[test]
fn varchar_lengths_and_unique_indexes_also_guard_raw_writes() {
    let mut f = Fx::new("l2-constraints");
    let c = f.company("C");
    let v = directory::save_record(&mut f.book, f.user, c, Model::Vendor, &json!({"display_name":"Agency"}), None).unwrap();
    let id = v["id"].as_str().unwrap();
    // Fresh raw-insert row ids: an item and a customer that do not exist yet.
    const ITEM: &str = "00000000-0000-7000-8000-0000000d0001";
    const CUSTOMER: &str = "00000000-0000-7000-8000-0000000d0002";
    for (sql, guard) in [
        (format!("UPDATE directory_vendor SET display_name='{}' WHERE id='{id}'", "x".repeat(121)), "length(display_name)<=120"),
        (format!("UPDATE directory_vendor SET default_terms='45' WHERE id='{id}'"), "vendor_default_terms"),
        (format!("INSERT INTO directory_item(id,company_id,code,kind,sale_rate)VALUES('{ITEM}','{c}','Bad','SERVICE',1.5)"), "other: cannot store REAL value in INTEGER column directory_item.sale_rate"),
        (format!("INSERT INTO directory_customer(id,company_id,display_name,tax_status)VALUES('{CUSTOMER}','{c}','Bad','NO')"), "customer_tax_status"),
    ] {
        assert_eq!(attack(&f.book, &sql).as_deref(), Some(guard), "{sql}");
    }
    let source = f
        .book
        .write(|tx| d::create_identity(tx, f.user, c, "INV", "00000000-0000-0000-0000-000000000001"))
        .unwrap();
    let sid = source["id"].as_str().unwrap();
    let sql = format!("UPDATE documents_documentsequence SET last_number=1000000000000 WHERE company_id='{c}'");
    assert_eq!(attack(&f.book, &sql).as_deref(), Some("document_sequence_limit"));
    let result: Result<(), fb_db::rusqlite::Error> = f.book.write(|tx| {
        tx.execute("DELETE FROM ledger_company WHERE id=?1", [c.to_db()])?;
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(d::identity(f.book.connection(), c, sid).unwrap()["ordinal"], 1);
}
