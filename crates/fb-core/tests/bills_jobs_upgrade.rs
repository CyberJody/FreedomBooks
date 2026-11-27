use fb_core::{Book, CompanyId, UserId, backups::{self, BackupOptions, RestoreOptions}, jobs, payables, sales};
use fb_db::{Opened, SnapshotSource, test_support};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const KEY: [u8; 32] = [49; 32];

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/bills-jobs-format90").join(name)
}

fn text<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap()
}

fn assert_prior_invoices(book: &mut Book, metadata: &Value, company: CompanyId) {
    for prior in metadata["invoices"].as_array().unwrap() {
        assert_eq!(book.read(|query| sales::invoice(query, company, text(prior, "id"))).unwrap(), *prior);
    }
}

fn exercise_upgraded_book(mut book: Book, metadata: &Value, root: &Path) {
    let actor = UserId::parse(text(metadata, "owner")).unwrap();
    let company = CompanyId::parse(text(metadata, "company")).unwrap();
    assert_prior_invoices(&mut book, metadata, company);
    let prior = &metadata["invoices"][0];
    let row = &prior["lines"][0];
    let edited = sales::save_invoice(&mut book, actor, company, &json!({"invoice_id":prior["id"],"expected_revision":1,"customer_id":metadata["second"],"date":"2026-11-15","tax_mode":"NONE","lines":[{"origin_position":row["position"],"item_id":metadata["replacement"],"income_account_id":row["income_account_id"],"description":"Changed ordinary legacy invoice","quantity":"3.25","rate":"2.00","taxable":false}]})).unwrap();
    assert_eq!(edited["current"]["total"], "6.50");
    let key = text(&edited, "id");
    let review = sales::review_invoice(&mut book, actor, company, key, Some(2)).unwrap();
    sales::post_invoice(&mut book, actor, company, key, Some(2), Some(&review)).unwrap();
    sales::void_invoice(&mut book, actor, company, key, Some(3), &json!("2026-11-16"), &json!("Void edited legacy invoice")).unwrap();
    sales::void_invoice(&mut book, actor, company, text(&metadata["invoices"][1], "id"), Some(2), &json!("2026-11-16"), &json!("Void prior-build posted invoice")).unwrap();
    let template = payables::recurring::save(&mut book, actor, company, &json!({"id":null,"request_key":fb_ids::new_v7().to_string(),"expected_revision":0,"name":"Synthetic integration rent","vendor_id":metadata["vendor"],"memo":"","terms_days":15,"frequency":"MONTHLY","start_date":"2026-11-16","end_date":null,"occurrence_limit":null,"active":true,"auto_post":false,"lines":[{"item_id":null,"expense_account_id":metadata["expense"],"description":"Rent","quantity":"2","rate":"12.34"}]}), "2026-11-15").unwrap();
    let generated = payables::recurring::generate_due(&mut book, actor, company, "2026-11-16").unwrap();
    assert_eq!(generated["created"].as_array().unwrap().len(), 1);
    assert_eq!(generated["created"][0]["current"]["total"], "24.68");
    let ticket = jobs::save_ticket(&mut book, actor, company, &json!({"request_key":fb_ids::new_v7().to_string(),"customer_id":metadata["second"],"title":"Post-upgrade service","opened_date":"2026-11-16"})).unwrap();
    jobs::save_entry(&mut book, actor, company, &json!({"request_key":fb_ids::new_v7().to_string(),"customer_id":metadata["second"],"ticket_id":ticket["id"],"item_id":metadata["replacement"],"date":"2026-11-16","hours":"0.25","billable":true,"notes":"Synthetic upgrade check"})).unwrap();
    let made = jobs::invoice_ticket(&mut book, actor, company, &json!({"request_key":fb_ids::new_v7().to_string(),"ticket_id":ticket["id"],"expected_revision":1,"date":"2026-11-16","grouping":"SERVICE","mark_invoiced":true})).unwrap();
    assert!(book.check().unwrap().is_empty());
    let before = test_support::snapshot(book.connection()).unwrap();
    let backup_directory = root.join("backups");
    std::fs::create_dir(&backup_directory).unwrap();
    let archive = backup_directory.join("format92.fbbackup");
    let work = root.join("backup-work");
    std::fs::create_dir(&work).unwrap();
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&work, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    backups::create_backup(&book, &KEY, &BackupOptions { output: &archive, work_root: &work, receipts_root: None, verify_receipt: None, label: "Synthetic integrated format 92", app_version: "test", created_at: "2026-11-16T12:00:00Z", passphrase: None, keep: None, allow_incomplete: false }).unwrap();
    let restored_path = root.join("restored92.fbbook");
    let result = backups::restore_backup(&archive, &KEY, &RestoreOptions { verify_receipt: None, target: &restored_path, receipts_target: None, work_root: &work, protected: &[], passphrase: None, require_encrypted: false }).unwrap();
    assert!(!result.upgraded);
    let mut restored = Book::open(&restored_path, &KEY).unwrap();
    assert_eq!(test_support::snapshot(restored.connection()).unwrap(), before);
    assert_eq!(payables::recurring::generate_due(&mut restored, actor, company, "2026-11-16").unwrap()["created"], json!([]));
    assert_eq!(payables::recurring::detail(&mut restored, actor, company, text(&template, "id")).unwrap()["id"], template["id"]);
    let job_key = text(&made, "invoice_id");
    let review = sales::review_invoice(&mut restored, actor, company, job_key, Some(1)).unwrap();
    sales::post_invoice(&mut restored, actor, company, job_key, Some(1), Some(&review)).unwrap();
    sales::void_invoice(&mut restored, actor, company, job_key, Some(2), &json!("2026-11-16"), &json!("Void restored time invoice")).unwrap();
    let live: i64 = restored.connection().query_row("SELECT count(*) FROM jobs_livebilling", [], |row| row.get(0)).unwrap();
    assert_eq!(live, 0);
    assert!(restored.check().unwrap().is_empty());
    drop(restored);
    assert!(Book::open(&restored_path, &KEY).unwrap().check().unwrap().is_empty());
}

#[test]
fn authentic_format_90_rows_upgrade_to_current_and_roundtrip_both_features() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("format90.fbbook");
    let mut archive = tar::Archive::new(std::fs::File::open(fixture("format90.fbbackup")).unwrap());
    let mut extracted = false;
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if entry.path().unwrap() == Path::new("book.fbbook") {
            entry.unpack(&path).unwrap();
            extracted = true;
        }
    }
    assert!(extracted);
    let before = test_support::snapshot_file(&path, &KEY).unwrap();
    let older = match Book::open_staged(&path, &KEY).unwrap() {
        Opened::Behind(older) => older,
        Opened::Current(_) => panic!("expected authentic installed-build format 90"),
    };
    assert_eq!((older.from(), older.to()), (90, 98)); // 0204 through 0211
    let snapshot = root.path().join("verified-format90.fbbook");
    older.snapshot(&snapshot, &KEY).unwrap();
    assert_eq!(test_support::snapshot_file(&snapshot, &KEY).unwrap(), before);
    let book = older.upgrade().unwrap();
    let after = test_support::snapshot(book.connection()).unwrap();
    for (table, rows) in &before {
        let mut expected = rows.clone();
        use fb_db::rusqlite::types::Value as Cell;
        let defaults = match table.as_str() {
            "directory_customer" => vec![("default_terms", Cell::Null)],
            "ledger_company" => vec![("fiscal_year_start_month", Cell::Integer(1)),
                ("closing_date", Cell::Null), ("period_policy_revision", Cell::Integer(0))],
            "bankfeeds_account" => vec![("import_from_date", Cell::Null), ("import_state", Cell::Text("ACTIVE".into()))],
            "bankfeeds_rule" => vec![("feed_account_id", Cell::Null)],
            _ => vec![],
        };
        for (column, value) in defaults {
            let st = book.connection().prepare(&format!("SELECT * FROM {table}")).unwrap();
            let index = st.column_names().position(|name| name == column).unwrap();
            for row in &mut expected {
                assert_eq!(index, row.len(), "{table}.{column}");
                row.push(value.clone());
            }
        }
        assert_eq!(&after[table], &expected, "upgrade changed retained rows in {table}");
    }
    let metadata: Value = serde_json::from_slice(&std::fs::read(fixture("fixture.json")).unwrap()).unwrap();
    exercise_upgraded_book(book, &metadata, root.path());
}

#[test]
fn authentic_format_90_archive_restore_upgrades_then_edits_posts_voids_and_reopens() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("restored90.fbbook");
    let work = root.path().join("restore-work");
    let result = backups::restore_backup(&fixture("format90.fbbackup"), &KEY, &RestoreOptions { verify_receipt: None, target: &path, receipts_target: None, work_root: &work, protected: &[], passphrase: None, require_encrypted: false }).unwrap();
    assert!(result.upgraded);
    let metadata: Value = serde_json::from_slice(&std::fs::read(fixture("fixture.json")).unwrap()).unwrap();
    exercise_upgraded_book(Book::open(&path, &KEY).unwrap(), &metadata, root.path());
}
