//! Isolated synthetic evidence: run create with main b0b216a, then verify with the desktop head.
//! The wrapper tools/verify_desktop_upgrade.py exports main and runs both foreground binaries.
use fb_core::{Book, accounts, services};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

fn hash(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}
fn schema(book: &mut Book) -> BTreeMap<String,String> {
    book.read(|q| q.prepare("SELECT type||':'||name,sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' AND sql IS NOT NULL ORDER BY type,name")?
        .query_map([], |r| Ok((r.get::<_,String>(0)?,hash(&r.get::<_,String>(1)?))))?.collect::<Result<_,_>>()).unwrap()
}
fn tables(book: &mut Book) -> BTreeMap<String,String> {
    book.read(|q| {
        let names=q.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT GLOB 'sqlite_*' ORDER BY name")?
            .query_map([], |r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
        let mut out=BTreeMap::new();
        for name in names {
            let mut st=q.prepare(&format!("SELECT * FROM {name}"))?;
            let n=st.column_count();
            let mut rows=st.query_map([],|r| (0..n).map(|i|r.get::<_,fb_db::rusqlite::types::Value>(i)).collect::<Result<Vec<_>,_>>())?.collect::<Result<Vec<_>,_>>()?;
            rows.sort_by_key(|r|format!("{r:?}"));
            out.insert(name,hash(&format!("{rows:?}")));
        }
        Ok::<_,fb_db::rusqlite::Error>(out)
    }).unwrap()
}
fn main() {
    let args=std::env::args().collect::<Vec<_>>();
    let dir=Path::new(&args[2]);std::fs::create_dir_all(dir).unwrap();
    let path=dir.join("main-populated.fbbook");
    let empty_path=dir.join("main-fresh.fbbook");
    let baseline_path=dir.join("main-baseline.json");
    // Public synthetic fixture key, used only for disposable test books; never log it.
    let key=[37;32];
    if args[1]=="create" {
        assert_eq!(fb_db::MIGRATIONS.last().unwrap().0,"0132_estimate_validators");
        let populated = path.exists();
        let mut book=if populated { Book::open(&path,&key).unwrap() } else { Book::create(&path,&key).unwrap() };
        if !populated {
        let owner=accounts::create_owner(&mut book,"Synthetic upgrade owner").unwrap();
        let c=services::create_company(&mut book,owner,"Synthetic main upgrade books","USD",false).unwrap().id;
        let a=services::save_account(&mut book,owner,c,services::AccountInput::new("1000","Cash").kind("ASSET")).unwrap().id;
        let b=services::save_account(&mut book,owner,c,services::AccountInput::new("3000","Capital").kind("EQUITY")).unwrap().id;
        let j=services::save_draft(&mut book,owner,c,services::DraftInput::new("2026-11-01","Synthetic opening",vec![services::line(a).debit("123.45").build(),services::line(b).credit("123.45").build()])).unwrap();
        services::post_journal(&mut book,owner,c,j.id,None).unwrap();
        }
        assert!(book.check().unwrap().is_empty());
        let mut empty=Book::create(&empty_path,&key).unwrap();
        assert_eq!(schema(&mut book),schema(&mut empty));
        let baseline=json!({"source":"b0b216a","highest_migration":fb_db::MIGRATIONS.last().unwrap().0,"version":fb_db::MIGRATIONS.len(),"schema":schema(&mut book),"tables":tables(&mut book)});
        std::fs::write(&baseline_path,serde_json::to_string_pretty(&baseline).unwrap()).unwrap();
        println!("MAIN CREATE PASS: b0b216a, {} migrations, populated book and fresh book",fb_db::MIGRATIONS.len());
    } else {
        let baseline:Value=serde_json::from_str(&std::fs::read_to_string(baseline_path).unwrap()).unwrap();
        assert_eq!(baseline["source"],"b0b216a");
        let mut book=Book::open(&path,&key).unwrap();
        let mut empty=Book::open(&empty_path,&key).unwrap();
        let mut fresh=Book::create(&dir.join("int-l4b-fresh.fbbook"),&key).unwrap();
        let rows=tables(&mut book);
        for (table,before) in baseline["tables"].as_object().unwrap() {assert_eq!(rows[table],before.as_str().unwrap(),"main table {table} changed");}
        let schema=schema(&mut book);
        for (object,before) in baseline["schema"].as_object().unwrap() {
            if !["view:fb_value_invalid","view:fb_json_invalid"].contains(&object.as_str()) {assert_eq!(schema[object],before.as_str().unwrap(),"main schema object {object} changed");}
        }
        assert_eq!(schema,self::schema(&mut empty));assert_eq!(schema,self::schema(&mut fresh));
        assert!(book.check().unwrap().is_empty()&&empty.check().unwrap().is_empty()&&fresh.check().unwrap().is_empty());
        let schema_hash=hash(&serde_json::to_string(&schema).unwrap());
        let result=json!({"source":"b0b216a","main_version":baseline["version"],"head_version":fb_db::MIGRATIONS.len(),"main_table_hashes_preserved":true,"main_objects_preserved_except_extended_integrity_views":true,"schema_objects":schema.len(),"schema_sha256":schema_hash,"all_three_schema_hashes_equal":true});
        std::fs::write(dir.join("upgrade-result.json"),serde_json::to_string_pretty(&result).unwrap()).unwrap();
        println!("UPGRADE PASS: {} -> {} migrations; main rows and guards preserved",baseline["version"],fb_db::MIGRATIONS.len());
        println!("SCHEMA HASH MATCH: main-populated -> head = main-fresh -> head = head-fresh: {schema_hash}");
    }
}
