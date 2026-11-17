use fb_core::{Book, backups::{self, BackupOptions}, directory::{self, Model}, documents, payables, sales, services::{self, AccountInput}, users};
use serde_json::json;

#[test]
fn write_format_90_fixture() {
    let root = std::path::PathBuf::from(std::env::var_os("FB_LEGACY_FIXTURE_OUT").expect("explicit synthetic output"));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("book.fbbook");
    let mut book = Book::create(&path, &[49; 32]).unwrap();
    let owner = users::create_user(&mut book, "Synthetic prior-build owner", false).unwrap();
    let company = services::create_company(&mut book, owner, "Synthetic prior-build books", "USD", false).unwrap().id;
    let mut account = |code: &str, name: &str, kind: &str| services::save_account(&mut book, owner, company, AccountInput::new(code, name).kind(kind)).unwrap().id;
    let income = account("400", "Services income", "INCOME");
    let receivables = account("110", "Receivables", "ASSET");
    let expense = account("500", "Supplier expense", "EXPENSE");
    let payable = account("200", "Payables", "LIABILITY");
    documents::save_setup(&mut book, owner, company, &json!({"receivables":receivables.to_db(),"payables":payable.to_db(),"banks":[]}), Some(0)).unwrap();
    let mut record = |model, value: serde_json::Value| directory::save_record(&mut book, owner, company, model, &value, None).unwrap()["id"].clone();
    let first = record(Model::Customer, json!({"display_name":"Prior customer"}));
    let second = record(Model::Customer, json!({"display_name":"Other prior customer"}));
    let vendor = record(Model::Vendor, json!({"display_name":"Prior supplier"}));
    let service = record(Model::Item, json!({"code":"Prior service","kind":"SERVICE","sale_rate":"1.25","income_account":income.to_db()}));
    let replacement = record(Model::Item, json!({"code":"Replacement service","kind":"SERVICE","sale_rate":"2.00","income_account":income.to_db()}));
    let mut invoices = Vec::new();
    for state in ["DRAFT", "OPEN", "VOID"] {
        let mut invoice = sales::save_invoice(&mut book, owner, company, &json!({"request_key":fb_ids::new_v7().to_string(),"customer_id":first,"date":"2026-11-15","tax_mode":"NONE","lines":[{"item_id":service,"income_account_id":income.to_db(),"description":"Prior invoice service","quantity":"2.5000","rate":"1.25","taxable":false}]})).unwrap();
        let key = invoice["id"].as_str().unwrap().to_owned();
        if state != "DRAFT" {
            let review = sales::review_invoice(&mut book, owner, company, &key, Some(1)).unwrap();
            invoice = sales::post_invoice(&mut book, owner, company, &key, Some(1), Some(&review)).unwrap();
        }
        if state == "VOID" {
            invoice = sales::void_invoice(&mut book, owner, company, &key, Some(2), &json!("2026-11-16"), &json!("Synthetic prior-build void")).unwrap();
        }
        assert_eq!(invoice["current"]["state"], state);
        invoices.push(invoice);
    }
    let bill = payables::save_bill(&mut book, owner, company, &json!({"vendor_id":vendor,"date":"2026-11-15","due_date":"2026-12-15","reference":"Supplier42","memo":"","request_key":fb_ids::new_v7().to_string(),"lines":[{"description":"Prior supplier cost","quantity":"1","rate":"12.34","expense_account_id":expense.to_db()}]})).unwrap();
    assert!(book.check().unwrap().is_empty());
    let work = root.join("work");
    std::fs::create_dir(&work).unwrap();
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&work, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let backup_directory = root.join("backups");
    std::fs::create_dir(&backup_directory).unwrap();
    let output = backup_directory.join("format90.fbbackup");
    let made = backups::create_backup(&book, &[49; 32], &BackupOptions { output: &output, work_root: &work, receipts_root: None, verify_receipt: None, label: "Synthetic format-90 integration fixture", app_version: "5a8c4fd9c9b8fc06ee2ee7df810f20efd5e86148", created_at: "2026-11-16T12:00:00Z", passphrase: None, keep: None, allow_incomplete: false }).unwrap();
    assert!(made.manifest.complete());
    std::fs::write(root.join("fixture.json"), serde_json::to_vec_pretty(&json!({"source_commit":"5a8c4fd9c9b8fc06ee2ee7df810f20efd5e86148","book_format":90,"master_byte":49,"owner":owner.to_db(),"company":company.to_db(),"income":income.to_db(),"expense":expense.to_db(),"first":first,"second":second,"vendor":vendor,"service":service,"replacement":replacement,"invoices":invoices,"bill":bill})).unwrap()).unwrap();
}
