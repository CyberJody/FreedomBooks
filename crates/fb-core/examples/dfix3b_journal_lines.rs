//! Compiled on genuine 98100e0 to create the fixture, then on FIX3b to verify it.
use fb_core::{Book, CompanyId, JournalId, UserId, AccountId, users, store,
    services::{self, AccountInput, DraftInput, line}};
use serde_json::{Value, json};
use std::path::Path;

fn main() {
    let args:Vec<_>=std::env::args().collect();
    let dir=Path::new(&args[2]);
    std::fs::create_dir_all(dir).unwrap();
    let path=dir.join("legacy.fbbook");
    let meta=dir.join("ids.json");
    let key=[63u8;32]; // Synthetic fixture key, not user data.
    if args[1]=="create" {
        let mut book=Book::create(&path,&key).unwrap();
        let u=users::create_user(&mut book,"Synthetic FIX3b",false).unwrap();
        let c=services::create_company(&mut book,u,"Synthetic FIX3b","USD",false).unwrap().id;
        let bank=services::save_account(&mut book,u,c,AccountInput::new("100","Cash").kind("ASSET")).unwrap().id;
        let equity=services::save_account(&mut book,u,c,AccountInput::new("300","Capital").kind("EQUITY")).unwrap().id;
        let d=services::save_draft(&mut book,u,c,DraftInput::new("2026-10-01","Draft",vec![
            line(bank).debit("5").description("plain").build(),
            line(bank).debit("5").description("Legacy\nline").name("Legacy\nname").build(),
            line(equity).credit("10").build(),
        ])).unwrap();
        std::fs::write(meta,json!({"user":u.to_db(),"company":c.to_db(),"journal":d.id.to_db(),"bank":bank.to_db(),"equity":equity.to_db(),"boundary":fb_db::MIGRATIONS.last().unwrap().0}).to_string()).unwrap();
        println!("CREATE PASS genuine 98100e0 draft [plain, Legacy LF, credit] at {}",fb_db::MIGRATIONS.last().unwrap().0);
        return;
    }
    let ids:Value=serde_json::from_slice(&std::fs::read(meta).unwrap()).unwrap();
    assert_eq!(ids["boundary"],"0137_bankfeeds_reconnect");
    let u=UserId::parse(ids["user"].as_str().unwrap()).unwrap();
    let c=CompanyId::parse(ids["company"].as_str().unwrap()).unwrap();
    let j=JournalId::parse(ids["journal"].as_str().unwrap()).unwrap();
    let bank=AccountId::parse(ids["bank"].as_str().unwrap()).unwrap();
    let equity=AccountId::parse(ids["equity"].as_str().unwrap()).unwrap();
    let mut book=Book::open(&path,&key).unwrap();
    assert!(book.check().unwrap().is_empty());
    let before=store::lines(book.connection(),j).unwrap();
    assert_eq!(before.len(),3);
    assert_eq!(before[1].description,"Legacy\nline");
    let mut input=DraftInput::new("2026-10-01","Draft",vec![
        line(bank).debit("5").description("Legacy\nline").name("Legacy\nname").build(),
        line(equity).credit("5").build(),
    ]);
    input.journal_id=Some(j);
    input.expected_revision=Some(1);
    let saved=services::save_draft(&mut book,u,c,input.clone()).unwrap();
    assert_eq!(saved.revision,2);
    assert_eq!(store::lines(book.connection(),j).unwrap()[0].description,"Legacy\nline");
    println!("PASS deleting line 1 retains unchanged legacy description and name on genuine 98100e0 draft");
    input.expected_revision=Some(2);
    let before=fb_db::test_support::snapshot(book.connection()).unwrap();
    for field in ["description","name"] {
        let mut changed=input.clone();
        changed.lines[0][field]=json!("Typed new\nline");
        assert_eq!(services::save_draft(&mut book,u,c,changed).unwrap_err().message(),"Control and formatting characters are not allowed.");
        assert_eq!(before,fb_db::test_support::snapshot(book.connection()).unwrap());
    }
    assert!(book.check().unwrap().is_empty());
    drop(book);
    assert!(Book::open(&path,&key).unwrap().check().unwrap().is_empty());
    println!("PASS typed new LF description/name refused with no writes; upgraded book reopens cleanly");
}
