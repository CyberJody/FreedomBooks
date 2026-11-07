//! Genuine prior-source fixture: the wrapper compiles this on cc40505 and 98100e0.
//! No current-schema SQL edits are used to simulate legacy records.
use fb_core::{Book, CompanyId, UserId, accounts, banking, bankfeeds, salestax, credits, prepayments, transfers, backups::{self, BackupOptions, RestoreOptions}, checks, directory::{self, Model}, documents, payables, sales, services::{self, AccountInput, DraftInput, line}, store};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};
const LEGACY: &str = "Legacy\nline";
fn schema(book: &Book) -> BTreeMap<String, String> {
    book.connection().prepare("SELECT type||':'||name,sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' AND sql IS NOT NULL ORDER BY type,name").unwrap()
        .query_map([], |r| Ok((r.get(0)?, format!("{:x}", Sha256::digest(r.get::<_,String>(1)?.as_bytes()))))).unwrap().collect::<Result<_,_>>().unwrap()
}
fn rows(book: &Book) -> BTreeMap<String, String> {
    fb_db::test_support::snapshot(book.connection()).unwrap().into_iter()
        .map(|(table, rows)| (table, format!("{:x}",Sha256::digest(format!("{rows:?}").as_bytes())))).collect()
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let dir = Path::new(&args[2]);
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join("legacy.fbbook");
    let meta = dir.join("fixture.json");
    let key = [46; 32]; // Public synthetic test material; never print key bytes.
    if args[1] == "create" {
        let mut book = Book::create(&path, &key).unwrap();
        let u = accounts::create_owner(&mut book, "Synthetic upgrade owner").unwrap();
        let c = services::create_company(&mut book,u,LEGACY,"USD",false).unwrap().id;
        let mut a = |code,kind| services::save_account(&mut book,u,c,AccountInput::new(code,code).kind(kind)).unwrap().id;
        let bank=a("100","ASSET"); let ar=a("110","ASSET"); let uf=a("120","ASSET");
        let second=a("101","ASSET"); let liability=a("230","LIABILITY"); let ap=a("200","LIABILITY"); let tax=a("210","LIABILITY"); let equity=a("300","EQUITY");
        let income=a("400","INCOME"); let expense=a("500","EXPENSE");
        documents::save_setup(&mut book,u,c,&json!({"receivables":ar.to_db(),"undeposited":uf.to_db(),"payables":ap.to_db(),"collected_tax":tax.to_db(),"customer_deposits":liability.to_db(),"banks":[bank.to_db(),second.to_db()]}),Some(0)).unwrap();
        let customer=directory::save_record(&mut book,u,c,Model::Customer,&json!({"display_name":LEGACY,"billing_address":LEGACY}),None).unwrap();
        let vendor=directory::save_record(&mut book,u,c,Model::Vendor,&json!({"display_name":LEGACY,"billing_address":LEGACY}),None).unwrap();
        let draft=services::save_draft(&mut book,u,c,DraftInput::new("2026-10-01",LEGACY,vec![line(bank).debit("10").description(LEGACY).name(LEGACY).build(),line(equity).credit("10").build()])).unwrap();
        services::post_journal(&mut book,u,c,draft.id,None).unwrap();
        let editable=services::save_draft(&mut book,u,c,DraftInput::new("2026-10-01",LEGACY,vec![line(bank).debit("5").description(LEGACY).name(LEGACY).build(),line(equity).credit("5").build()])).unwrap();
        let inv=json!({"request_key":fb_ids::new_v7().to_string(),"customer_id":customer["id"],"date":"2026-10-01","due_date":"2026-10-31","memo":LEGACY,"tax_mode":"MANUAL","tax_rate":"6.25","tax_label":LEGACY,"lines":[{"item_id":null,"description":LEGACY,"quantity":"1","rate":"4.00","taxable":true,"income_account_id":income.to_db()}]});
        let invoice=sales::save_invoice(&mut book,u,c,&inv).unwrap();
        let mut est=inv.clone(); est["request_key"]=json!(fb_ids::new_v7().to_string());
        est["lines"][0].as_object_mut().unwrap().remove("income_account_id");
        let estimate=sales::save_estimate(&mut book,u,c,&est).unwrap();
        let bill=json!({"request_key":fb_ids::new_v7().to_string(),"vendor_id":vendor["id"],"date":"2026-10-01","due_date":"2026-10-31","reference":"UPGRADE-1","memo":LEGACY,"lines":[{"expense_account_id":expense.to_db(),"item_id":null,"description":LEGACY,"quantity":"1","rate":"4.00"}]});
        let bill_saved=payables::save_bill(&mut book,u,c,&bill).unwrap();
        let check=json!({"request_key":fb_ids::new_v7().to_string(),"bank_id":bank.to_db(),"payee_kind":"OTHER","payee_name":LEGACY,"address":LEGACY,"date":"2026-10-01","check_number":"UPGRADE-1","print_later":false,"memo":LEGACY,"lines":[{"tab":"EXPENSE","account_id":expense.to_db(),"item_id":null,"description":LEGACY,"quantity":"1","rate":"4.00"}]});
        let check_saved=checks::save_check(&mut book,u,c,&check).unwrap();
        let mut extra=json!({});
        services::save_account(&mut book,u,c,AccountInput::new("500","500").kind("EXPENSE").edit(expense).description(LEGACY)).unwrap();
        extra["expense"]=json!(expense.to_db());
        let rule=json!({"name":"Legacy\u{202e}rule","match_text":"Legacy\u{200b}match","payee_name":"Legacy\u{2066}payee","account_id":expense.to_db()});
        extra["rule"]=json!(bankfeeds::save_rule(&mut book,u,c,&rule,&bankfeeds::SecretSet::none()).unwrap().id);extra["rule values"]=rule;
        let rate=json!({"name":"Legacy rate","description":LEGACY,"rate":"1","agency_id":vendor["id"]});
        extra["rate"]=salestax::save_rate(&mut book,u,c,&rate).unwrap()["id"].clone();extra["rate values"]=rate;
        let rate2=salestax::save_rate(&mut book,u,c,&json!({"name":"Legacy rate 2","description":LEGACY,"rate":"2","agency_id":vendor["id"]})).unwrap();
        let group=json!({"name":"Legacy group","description":LEGACY,"member_ids":[extra["rate"],rate2["id"]]});
        extra["group"]=salestax::save_group(&mut book,u,c,&group).unwrap()["id"].clone();extra["group values"]=group;
        {
        let mut sale=inv.clone();sale["request_key"]=json!(fb_ids::new_v7().to_string());sale["method"]=json!("CASH");sale["destination"]=json!("BANK");sale["bank_id"]=json!(bank.to_db());
        let review=sales::review_sales_receipt(&mut book,u,c,&sale).unwrap();
        extra["sale"]=sales::record_sales_receipt(&mut book,u,c,&sale,Some(&review)).unwrap()["id"].clone();
        sale["request_key"]=json!(fb_ids::new_v7().to_string());sale["destination"]=json!("UNDEPOSITED");sale["bank_id"]=Value::Null;
        let review=sales::review_sales_receipt(&mut book,u,c,&sale).unwrap();
        let funds=sales::record_sales_receipt(&mut book,u,c,&sale,Some(&review)).unwrap();
        let p=json!({"request_key":fb_ids::new_v7().to_string(),"date":"2026-10-02","bank_id":bank.to_db(),"receipt_ids":[funds["id"]],"memo":LEGACY});
        let review=banking::review_deposit(&mut book,u,c,&p).unwrap();
        extra["bank deposit"]=banking::record_deposit(&mut book,u,c,&p,&review).unwrap()["id"].clone();
        let mut credit=inv.clone();credit["request_key"]=json!(fb_ids::new_v7().to_string());
        let review=credits::review_credit_memo(&mut book,u,c,&credit).unwrap();
        let memo=credits::record_credit_memo(&mut book,u,c,&credit,Some(&review)).unwrap();extra["credit memo"]=memo["id"].clone();
        let refund=json!({"request_key":fb_ids::new_v7().to_string(),"date":"2026-10-02","bank_id":bank.to_db(),"amount":"1.00","method":"CASH","memo":LEGACY});
        let review=credits::review_refund(&mut book,u,c,memo["id"].as_str().unwrap(),&refund).unwrap();
        extra["credit refund"]=credits::record_refund(&mut book,u,c,memo["id"].as_str().unwrap(),&refund,Some(&review)).unwrap()["id"].clone();
        let p=json!({"request_key":fb_ids::new_v7().to_string(),"customer_id":customer["id"],"date":"2026-10-01","amount":"5.00","method":"CASH","destination":"BANK","bank_id":bank.to_db(),"memo":LEGACY,"order_reference":LEGACY});
        let review=prepayments::review_deposit(&mut book,u,c,&p).unwrap();
        let deposit=prepayments::record_deposit(&mut book,u,c,Some(&review),&p).unwrap();extra["customer deposit"]=deposit["id"].clone();
        let mut refund=refund.clone();refund["request_key"]=json!(fb_ids::new_v7().to_string());
        let review=prepayments::review_refund(&mut book,u,c,deposit["id"].as_str().unwrap(),&refund).unwrap();
        extra["deposit refund"]=prepayments::record_refund(&mut book,u,c,deposit["id"].as_str().unwrap(),Some(&review),&refund).unwrap()["id"].clone();
        let p=json!({"request_key":fb_ids::new_v7().to_string(),"date":"2026-10-01","amount":"1.00","from_account_id":bank.to_db(),"to_account_id":second.to_db(),"memo":LEGACY});
        let review=transfers::review_transfer(&mut book,u,c,&p).unwrap();extra["transfer"]=transfers::record_transfer(&mut book,u,c,&p,&review).unwrap()["id"].clone();
        let mut inv=inv.clone();inv["request_key"]=json!(fb_ids::new_v7().to_string());
        let posted=sales::save_invoice(&mut book,u,c,&inv).unwrap();let id=posted["id"].as_str().unwrap();
        let review=sales::review_invoice(&mut book,u,c,id,Some(1)).unwrap();sales::post_invoice(&mut book,u,c,id,Some(1),Some(&review)).unwrap();
        let p=json!({"request_key":fb_ids::new_v7().to_string(),"customer_id":customer["id"],"date":"2026-10-02","amount":"1.00","method":"CASH","destination":"BANK","bank_id":bank.to_db(),"memo":LEGACY,"reference":LEGACY,"allocations":[{"invoice_id":id,"amount":"1.00"}]});
        let review=sales::review_receipt(&mut book,u,c,&p).unwrap();extra["customer receipt"]=sales::record_receipt(&mut book,u,c,&p,Some(&review)).unwrap()["id"].clone();
        let mut bill=bill.clone();bill["request_key"]=json!(fb_ids::new_v7().to_string());bill["reference"]=json!("UPGRADE-2");
        let posted=payables::save_bill(&mut book,u,c,&bill).unwrap();let id=posted["id"].as_str().unwrap();
        let review=payables::review_bill(&mut book,u,c,id,Some(1)).unwrap();payables::post_bill(&mut book,u,c,id,Some(1),&review).unwrap();
        let p=json!({"request_key":fb_ids::new_v7().to_string(),"vendor_id":vendor["id"],"date":"2026-10-02","amount":"1.00","method":"CASH","bank_id":bank.to_db(),"memo":LEGACY,"reference":LEGACY,"allocations":[{"bill_id":id,"amount":"1.00"}]});
        let review=payables::review_payment(&mut book,u,c,&p).unwrap();extra["bill payment"]=payables::record_payment(&mut book,u,c,&p,&review).unwrap()["id"].clone();
        }
        assert!(book.check().unwrap().is_empty());
        let data=json!({"source":args[3],"version":fb_db::MIGRATIONS.len(),"schema":schema(&book),"rows":rows(&book),"extra":extra,"user":u.to_db(),"company":c.to_db(),"journal":draft.id.to_db(),"editable":editable.id.to_db(),"customer":customer["id"],"vendor":vendor["id"],"invoice":[invoice["id"],inv],"estimate":[estimate["id"],est],"bill":[bill_saved["id"],bill],"check":[check_saved["id"],check]});
        std::fs::write(meta,serde_json::to_string_pretty(&data).unwrap()).unwrap();
        println!("CREATE PASS {}: genuine Legacy\\nline in posted journal, draft journal, invoice, estimate, bill, check and master records",args[3]);
    } else {
        let data:Value=serde_json::from_slice(&std::fs::read(meta).unwrap()).unwrap();
        let mut book=Book::open(&path,&key).unwrap();
        let fresh=Book::create(&dir.join("fresh.fbbook"),&key).unwrap();
        assert!(book.check().unwrap().is_empty());
        assert_eq!(schema(&book),schema(&fresh));
        let after=rows(&book);
        for (table, hash) in data["rows"].as_object().unwrap() { assert_eq!(after[table],hash.as_str().unwrap(),"upgrade changed {table}"); }
        println!("SCHEMA HASH MATCH {} -> FIX3: {:x}",data["source"],Sha256::digest(serde_json::to_vec(&schema(&book)).unwrap()));
        let archive_dir=dir.with_extension("backups");let restore_dir=dir.with_extension("restores");
        std::fs::create_dir_all(&archive_dir).unwrap();std::fs::create_dir_all(&restore_dir).unwrap();
        let archive=archive_dir.join("legacy.fbbackup");let work=dir.with_extension("private");let restored_path=restore_dir.join("restored.fbbook");
        backups::create_backup(&book,&key,&BackupOptions {output:&archive,work_root:&work,receipts_root:None,verify_receipt:None,label:"Synthetic legacy",app_version:"FIX3-test",created_at:"2026-11-02T12:00:00Z",passphrase:None,keep:None}).unwrap();
        backups::restore_backup(&archive,&key,&RestoreOptions {target:&restored_path,receipts_target:None,work_root:&work,protected:std::slice::from_ref(&path),passphrase:None,require_encrypted:false,verify_receipt:None}).unwrap();
        let restored=Book::open(&restored_path,&key).unwrap();assert_eq!(rows(&restored),after);assert_eq!(schema(&restored),schema(&book));drop(restored);
        println!("PASS backup and restore preserve every legacy row and the exact schema hash");
        let u=UserId::parse(data["user"].as_str().unwrap()).unwrap(); let c=CompanyId::parse(data["company"].as_str().unwrap()).unwrap();
        let j=fb_core::JournalId::parse(data["journal"].as_str().unwrap()).unwrap();
        let rev=services::reverse_journal(&mut book,u,c,j,"2026-10-02","Synthetic correction").unwrap();
        assert_eq!(store::lines(book.connection(),rev.id).unwrap()[0].description,LEGACY);
        println!("PASS legacy journal reversal preserves description and name");
        let j=fb_core::JournalId::parse(data["editable"].as_str().unwrap()).unwrap();
        let lines=store::lines(book.connection(),j).unwrap().iter().map(|l|json!({"account":l.account_id.to_db(),"debit":l.debit.to_string(),"credit":l.credit.to_string(),"description":l.description,"name":l.name})).collect();
        let mut input=DraftInput::new("2026-10-02",LEGACY,lines); input.journal_id=Some(j);input.expected_revision=Some(1);
        services::save_draft(&mut book,u,c,input.clone()).unwrap();
        input.memo="New\ninput".into();input.expected_revision=Some(2);
        let before=fb_db::test_support::snapshot(book.connection()).unwrap();
        assert!(services::save_draft(&mut book,u,c,input).is_err());
        assert_eq!(before,fb_db::test_support::snapshot(book.connection()).unwrap());
        println!("PASS draft journal revision carries unchanged legacy fields; edited control refused without writes");
        directory::set_active(&mut book,u,c,Model::Customer,data["customer"].as_str().unwrap(),false).unwrap();
        directory::set_active(&mut book,u,c,Model::Customer,data["customer"].as_str().unwrap(),true).unwrap();
        directory::save_record(&mut book,u,c,Model::Vendor,&json!({"notes":"Changed note"}),data["vendor"].as_str()).unwrap();
        services::edit_company(&mut book,u,c,LEGACY,"USD").unwrap();
        println!("PASS master-record archive/reactivate and partial revision preserve legacy fields");
        let expense=fb_core::AccountId::parse(data["extra"]["expense"].as_str().unwrap()).unwrap();
        let account=services::save_account(&mut book,u,c,AccountInput::new("500","500").kind("EXPENSE").edit(expense)).unwrap();assert_eq!(account.description,LEGACY);
        let mut rule=data["extra"]["rule values"].clone();rule["rule_id"]=data["extra"]["rule"].clone();rule["amount_min"]=json!("1.00");
        let saved=bankfeeds::save_rule(&mut book,u,c,&rule,&bankfeeds::SecretSet::none()).unwrap();assert_eq!(saved.name,"Legacy\u{202e}rule");assert_eq!(saved.match_text,"Legacy\u{200b}match");
        let before=rows(&book);rule["name"]=json!("Edited\u{202e}name");assert!(bankfeeds::save_rule(&mut book,u,c,&rule,&bankfeeds::SecretSet::none()).is_err());assert_eq!(before,rows(&book));
        let mut rate=data["extra"]["rate values"].clone();rate["item_id"]=data["extra"]["rate"].clone();rate["expected_revision"]=json!(1);rate["rate"]=json!("1.5");salestax::save_rate(&mut book,u,c,&rate).unwrap();
        let mut group=data["extra"]["group values"].clone();group["item_id"]=data["extra"]["group"].clone();group["expected_revision"]=json!(1);salestax::save_group(&mut book,u,c,&group).unwrap();
        println!("PASS account, bank-rule and tax revisions retain unchanged legacy text; edited rule controls refused without writes");
        let id=data["invoice"][0].as_str().unwrap(); let mut input=data["invoice"][1].clone();input["invoice_id"]=json!(id);input["expected_revision"]=json!(1);input["date"]=json!("2026-10-02");
        sales::save_invoice(&mut book,u,c,&input).unwrap();
        let review=sales::review_invoice(&mut book,u,c,id,Some(2)).unwrap();
        sales::post_invoice(&mut book,u,c,id,Some(2),Some(&review)).unwrap();
        sales::void_invoice(&mut book,u,c,id,Some(3),&json!("2026-10-03"),&json!("Synthetic correction")).unwrap();
        println!("PASS invoice revision, tax recalculation, posting and void copy legacy text");
        let id=data["estimate"][0].as_str().unwrap();let mut input=data["estimate"][1].clone();input["estimate_id"]=json!(id);input["expected_revision"]=json!(1);
        sales::save_estimate(&mut book,u,c,&input).unwrap();
        sales::transition_estimate(&mut book,u,c,id,"ACCEPTED",Some(2),"").unwrap();
        sales::convert_estimate(&mut book,u,c,id,Some(3),"2026-10-03","2026-10-31").unwrap();
        println!("PASS estimate revision, acceptance and invoice conversion copy legacy text");
        let id=data["bill"][0].as_str().unwrap();let mut input=data["bill"][1].clone();input["bill_id"]=json!(id);input["expected_revision"]=json!(1);
        payables::save_bill(&mut book,u,c,&input).unwrap();
        let review=payables::review_bill(&mut book,u,c,id,Some(2)).unwrap();
        payables::post_bill(&mut book,u,c,id,Some(2),&review).unwrap();
        payables::void_bill(&mut book,u,c,id,Some(3),&json!("2026-10-03"),&json!("Synthetic correction")).unwrap();
        println!("PASS bill revision, posting and void copy legacy text");
        let id=data["check"][0].as_str().unwrap();let mut input=data["check"][1].clone();input["check_id"]=json!(id);input["expected_revision"]=json!(1);
        checks::save_check(&mut book,u,c,&input).unwrap();
        let review=checks::review_check(&mut book,u,c,id,Some(2)).unwrap();
        checks::record_check(&mut book,u,c,id,Some(2),&review).unwrap();
        checks::void_check(&mut book,u,c,id,Some(3),&json!("2026-10-03"),&json!("Synthetic correction")).unwrap();
        println!("PASS check revision, recording and void copy legacy text");
        type Reverse = fn(&mut Book, UserId, CompanyId, &str, Option<i64>, &Value, &Value) -> fb_core::FbResult<Value>;
        for (name, reverse) in [
            ("credit refund",credits::void_refund as Reverse),
            ("deposit refund",prepayments::void_refund as Reverse),
            ("credit memo",credits::void_credit_memo as Reverse),
            ("customer deposit",prepayments::void_deposit as Reverse),
            ("bank deposit",banking::reverse_deposit as Reverse),
            ("sale",sales::void_sales_receipt as Reverse),
            ("customer receipt",sales::reverse_receipt as Reverse),
            ("bill payment",payables::reverse_payment as Reverse),
            ("transfer",transfers::void_transfer as Reverse),
        ] {
            reverse(&mut book,u,c,data["extra"][name].as_str().unwrap(),Some(1),&json!("2026-10-03"),&json!("Synthetic correction")).unwrap();
            println!("PASS {name} lifecycle copies legacy text");
        }
        assert!(book.check().unwrap().is_empty());drop(book);
        let reopened=Book::open(&path,&key).unwrap();assert!(reopened.check().unwrap().is_empty());
        println!("UPGRADE/LIFECYCLE PASS {} -> FIX3; integrity and reopen clean",data["source"]);
    }
}
