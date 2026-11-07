//! INT-L4 (L4-BACKUPS request 1): upload, deduplicate and soft-remove attachments, then back up, verify and
//! restore through the attachment module's verifier, and read every file back from the restored book.
mod common;
use common::*;
use fb_core::{
    Book,
    attachments::{self as a, Limits, Owner, storage::Store},
    backups::{self, BackupOptions, RestoreOptions},
    users,
};
use std::fs;

const PDF: &[u8] = b"%PDF-1.4\n1 0 obj<</Type/Catalog>>endobj\n%%EOF\n";
const PDF2: &[u8] = b"%PDF-1.4\n2 0 obj<</Type/Catalog>>endobj\n%%EOF\n";

#[test]
fn attachments_survive_backup_verify_and_restore_and_tampering_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let live = dir.path().join("live");
    fs::create_dir(&live).unwrap();
    let path = live.join("book.fbbook");
    let mut book = Book::create(&path, &KEY).unwrap();
    let user = users::create_user(&mut book, "tester", false).unwrap();
    let mut fx = Fx { book, user };
    let (co, cash, eq) = fx.chart();
    let j1 = fx.draft(co, cash, eq, "1.00", "2026-01-01");
    let j2 = fx.draft(co, cash, eq, "2.00", "2026-01-02");
    let store = Store::for_book(&path, &KEY).unwrap();
    let add = |fx: &mut Fx, owner: &Owner, name: &str, bytes: &[u8]| {
        a::add(&mut fx.book, &store, fx.user, co, owner, bytes, name, &uuid::Uuid::new_v4().to_string(), &Limits::default()).unwrap().0
    };
    let first = add(&mut fx, &Owner::Journal(j1.id), "one.pdf", PDF);
    let shared = add(&mut fx, &Owner::Journal(j2.id), "same-bytes.pdf", PDF); // deduplicated: one encrypted file
    let removed = add(&mut fx, &Owner::Journal(j2.id), "two.pdf", PDF2);
    a::remove(&mut fx.book, fx.user, co, removed["id"].as_str().unwrap(), "Synthetic correction").unwrap();
    assert_eq!(fs::read_dir(store.folder()).unwrap().count(), 2);

    let work = dir.path().join("private");
    let out = dir.path().join("backups");
    fs::create_dir(&out).unwrap();
    let verifier = backups::attachment_receipt_verifier(&KEY);
    let archive = out.join("with-receipts.fbbackup");
    let verify: &backups::ReceiptVerifier<'_> = &verifier;
    let options = |output| BackupOptions {
        output,
        work_root: &work,
        receipts_root: Some(store.folder()),
        verify_receipt: Some(verify),
        label: "synthetic",
        app_version: "test",
        created_at: "2026-10-31T12:00:00Z",
        passphrase: None,
        keep: None,
    };
    let created = backups::create_backup(&fx.book, &KEY, &options(&archive)).unwrap();
    // Every row, removed ones included, is inventoried; the two rows sharing bytes share one file.
    assert_eq!(created.manifest.attachments.len(), 2);
    assert_eq!(created.manifest.attachments.iter().map(|r| r.owners.len()).sum::<usize>(), 3);
    let verified = backups::verify_backup_with_receipts(&archive, &KEY, &work, None, true, Some(&verifier)).unwrap();
    assert_eq!(verified.attachments, created.manifest.attachments);

    let restored_dir = dir.path().join("restored");
    fs::create_dir(&restored_dir).unwrap();
    let target = restored_dir.join("book.fbbook");
    let receipts = restored_dir.join("receipts");
    let restore = RestoreOptions {
        verify_receipt: Some(&verifier),
        target: &target,
        receipts_target: Some(&receipts),
        work_root: &work,
        protected: &[path.clone(), store.folder().to_path_buf(), out.clone(), work.clone()],
        passphrase: None,
        require_encrypted: false,
    };
    backups::restore_backup(&archive, &KEY, &restore).unwrap();
    let mut restored = Book::open(&target, &KEY).unwrap();
    assert!(restored.check().unwrap().is_empty());
    let restored_store = Store::for_book(&target, &KEY).unwrap();
    for (row, bytes) in [(&first, PDF), (&shared, PDF), (&removed, PDF2)] {
        let id = row["id"].as_str().unwrap();
        assert_eq!(a::download(&mut restored, &restored_store, user, co, id).unwrap(), bytes, "{id}");
    }

    // A changed ciphertext byte in the live folder: the verifier refuses the backup; nothing is written.
    let tampered = fs::read_dir(store.folder()).unwrap().next().unwrap().unwrap().path();
    let mut bytes = fs::read(&tampered).unwrap();
    bytes[0] ^= 1;
    fs::write(&tampered, bytes).unwrap();
    let again = out.join("tampered.fbbackup");
    let error = backups::create_backup(&fx.book, &KEY, &options(&again)).err().expect("tampered receipt refused");
    assert!(error.to_string().contains("does not match its attachment record"), "{error}");
    assert!(!again.exists());
}
