mod common;
use common::*;
use fb_core::{Book,attachments::{self as a,Owner,Limits,storage::Store},backups::{self,BackupOptions},users};
use std::{fs,path::PathBuf};
const RECEIPTS:[u8;32]=[77;32];
const NEXT:[u8;32]=[43;32];
const PDF:&[u8]=b"%PDF-1.4\n1 0 obj<</Type/Catalog>>endobj\n%%EOF\n";
#[test]
fn write_legacy_fixture() {
    let root=PathBuf::from(std::env::var_os("FB_LEGACY_FIXTURE_OUT").expect("explicit synthetic output"));
    fs::create_dir_all(&root).unwrap();
    let live=root.join("live");fs::create_dir(&live).unwrap();
    let path=live.join("book.fbbook");
    let mut book=Book::create(&path,&KEY).unwrap();
    let user=users::create_user(&mut book,"Synthetic legacy owner",true).unwrap();
    let mut fx=Fx{book,user};let (co,cash,eq)=fx.chart();
    let journal=fx.draft(co,cash,eq,"1.00","2026-01-01");
    let store=Store::for_book(&path,&KEY).unwrap();
    let row=a::add(&mut fx.book,&store,user,co,&Owner::Journal(journal.id),PDF,"legacy.pdf",&uuid::Uuid::new_v4().to_string(),&Limits::default()).unwrap().0;
    let id=row["id"].as_str().unwrap().to_owned();
    let (other_co,other_cash,other_eq)=fx.chart();
    let other_journal=fx.draft(other_co,other_cash,other_eq,"2.00","2026-01-02");
    let other=a::add(&mut fx.book,&store,user,other_co,&Owner::Journal(other_journal.id),PDF,"other-company.pdf",&uuid::Uuid::new_v4().to_string(),&Limits::default()).unwrap().0;
    let other_id=other["id"].as_str().unwrap().to_owned();
    a::rewrap_legacy(&mut fx.book,user,co,&RECEIPTS,&KEY).unwrap();
    fx.book.rekey(&NEXT).unwrap();
    let work=root.join("work");fs::create_dir(&work).unwrap();
    #[cfg(unix)] {use std::os::unix::fs::PermissionsExt;fs::set_permissions(&work,fs::Permissions::from_mode(0o700)).unwrap();}
    let receipts=live.join("receipts");let verifier=backups::attachment_receipt_verifier(&RECEIPTS);
    let backups_dir=root.join("backups");fs::create_dir(&backups_dir).unwrap();let output=backups_dir.join("first-lane.fbbackup");
    let envelope=backups::portable::KeyEnvelope::new(&NEXT,&RECEIPTS,Some("Synthetic legacy book password")).unwrap();
    let made=backups::create_portable_backup(&fx.book,&NEXT,&BackupOptions{output:&output,work_root:&work,receipts_root:Some(&receipts),verify_receipt:Some(&verifier),label:"Synthetic legacy attachment fixture",app_version:"f125c3ea",created_at:"2026-11-11T00:00:00Z",passphrase:None,keep:None,allow_incomplete:false},&envelope).unwrap();
    assert!(made.manifest.complete());
    let event:String=fx.book.connection().query_row("SELECT detail FROM ledger_auditevent WHERE action='attachment.key_rewrapped'",[],|r|r.get(0)).unwrap();
    fs::write(root.join("fixture.json"),serde_json::to_vec_pretty(&serde_json::json!({"source_commit":"f125c3ea","company":co.to_db(),"attachment":id,"other_company":other_co.to_db(),"other_attachment":other_id,"audit_detail":event,"master_byte":43,"receipt_byte":77,"backup":"backups/first-lane.fbbackup"})).unwrap()).unwrap();
    drop(fx);
}
