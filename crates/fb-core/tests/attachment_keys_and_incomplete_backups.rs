//! Attachment files whose key was wrapped under the book's master key (attached through the desktop
//! chooser before the receipt-key fix) stay readable, verify in backups and gain a receipt-key wrap: a
//! permanent `attachments_keywrap` row per file, tied to one `attachment.key_rewrapped` audit event per
//! company that names the files and holds no key material (attachment rows stay immutable). Every wrap a
//! file has is tried under every key; a file is unreadable only when none opens, and "no key opens it" is
//! kept apart from "damaged". Backups with missing, damaged or keyless attachment files are refused by
//! default, naming the files, and can be made incomplete: labelled in the header and manifest, verified,
//! restorable, and never pruning anything. A system error reading a file stops the backup instead.
mod common;
use common::*;
use fb_core::{
    Book, CompanyId,
    attachments::{self as a, Limits, Owner, storage::{ReadError, Store}},
    backups::{self, BackupOptions, RestoreOptions},
    users,
};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

const RECEIPTS: [u8; 32] = [77; 32];
const PDF: &[u8] = b"%PDF-1.4\n1 0 obj<</Type/Catalog>>endobj\n%%EOF\n";
const PDF2: &[u8] = b"%PDF-1.4\n2 0 obj<</Type/Catalog>>endobj\n%%EOF\n";
const PDF3: &[u8] = b"%PDF-1.4\n3 0 obj<</Type/Catalog>>endobj\n%%EOF\n";

struct F {
    dir: tempfile::TempDir,
    fx: Fx,
    path: PathBuf,
    co: CompanyId,
    owner: Owner,
    second: Owner,
}

impl F {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("live");
        fs::create_dir(&live).unwrap();
        let path = live.join("book.fbbook");
        let mut book = Book::create(&path, &KEY).unwrap();
        let user = users::create_user(&mut book, "owner", true).unwrap();
        let mut fx = Fx { book, user };
        let (co, cash, eq) = fx.chart();
        let j1 = fx.draft(co, cash, eq, "1.00", "2026-01-01");
        let j2 = fx.draft(co, cash, eq, "2.00", "2026-01-02");
        Self { dir, fx, path, co, owner: Owner::Journal(j1.id), second: Owner::Journal(j2.id) }
    }
    /// Attach with the store built from `wrap` (the receipt key, or the master key as the old chooser did).
    fn add(&mut self, wrap: &[u8; 32], owner: &Owner, name: &str, bytes: &[u8]) -> Value {
        let store = Store::for_book(&self.path, wrap).unwrap();
        a::add(&mut self.fx.book, &store, self.fx.user, self.co, owner, bytes, name, &uuid::Uuid::new_v4().to_string(), &Limits::default()).unwrap().0
    }
    fn store(&self) -> Store {
        Store::for_book(&self.path, &RECEIPTS).unwrap().with_legacy(&KEY).unwrap()
    }
    fn download(&mut self, store: &Store, id: &str) -> fb_core::FbResult<Vec<u8>> {
        a::download(&mut self.fx.book, store, self.fx.user, self.co, id)
    }
    fn receipts(&self) -> PathBuf {
        self.path.parent().unwrap().join("receipts")
    }
    fn work(&self) -> PathBuf {
        self.dir.path().join("private")
    }
    fn out(&self) -> PathBuf {
        let out = self.dir.path().join("backups");
        fs::create_dir_all(&out).unwrap();
        out
    }
    fn backup(&self, name: &str, allow_incomplete: bool, keep: Option<usize>, stamp: &str) -> backups::Result<backups::BackupResult> {
        let verifier = backups::attachment_receipt_verifier_with(&RECEIPTS, Some(&KEY));
        let work = self.work();
        let receipts = self.receipts();
        let receipts = receipts.is_dir().then_some(receipts);
        let output = self.out().join(name);
        backups::create_backup(&self.fx.book, &KEY, &BackupOptions {
            output: &output, work_root: &work, receipts_root: receipts.as_deref(), verify_receipt: Some(&verifier),
            label: "synthetic", app_version: "test", created_at: stamp, passphrase: None, keep, allow_incomplete,
        })
    }
    fn verify(&self, file: &Path) -> backups::Result<backups::Manifest> {
        let verifier = backups::attachment_receipt_verifier_with(&RECEIPTS, Some(&KEY));
        backups::verify_backup_with_receipts(file, &KEY, &self.work(), None, true, Some(&verifier))
    }
    /// The row's own wrap, and every further wrap recorded for its file (newest first).
    fn wrap_of(&self, id: &str) -> ((String, String), Vec<a::storage::Wrap>) {
        let (co, name, own): (String, String, (String, String)) = self.fx.book.connection().query_row(
            "SELECT company_id, stored_name, wrapped_key, wrap_nonce FROM attachments_attachment WHERE id=?1",
            [id], |r| Ok((r.get(0)?, r.get(1)?, (r.get(2)?, r.get(3)?)))).unwrap();
        (own, a::recorded_wraps(self.fx.book.connection(), CompanyId::parse(&co).unwrap(), &name).unwrap())
    }
    fn stored_name(&self, id: &str) -> String {
        self.fx.book.connection().query_row("SELECT stored_name FROM attachments_attachment WHERE id=?1", [id], |r| r.get(0)).unwrap()
    }
    fn restore(&self, archive: &Path, tag: &str) -> PathBuf {
        let target_dir = self.dir.path().join(tag);
        fs::create_dir(&target_dir).unwrap();
        let target = target_dir.join("book.fbbook");
        let verifier = backups::attachment_receipt_verifier_with(&RECEIPTS, Some(&KEY));
        backups::restore_backup(archive, &KEY, &RestoreOptions {
            verify_receipt: Some(&verifier), target: &target, receipts_target: Some(&target_dir.join("receipts")), work_root: &self.work(),
            protected: &[self.path.clone(), self.receipts(), self.out(), self.work()], passphrase: None, require_encrypted: false,
        }).unwrap();
        target
    }
}

/// A valid further wrap row for `stored` in `co`, with its audit event, written by hand (as a buggy or
/// hostile writer could). The wrap itself is whatever bytes are given.
fn insert_wrap(book: &Book, co: CompanyId, stored: &str, wrapped_key: &str, wrap_nonce: &str) -> Option<String> {
    let event = uuid::Uuid::new_v4().to_string();
    let attachment: String = book.connection().query_row("SELECT id FROM attachments_attachment WHERE company_id=?1 AND stored_name=?2 LIMIT 1", [co.to_db(), stored.to_owned()], |r| r.get(0)).unwrap();
    let actor: String = book.connection().query_row("SELECT id FROM fb_user LIMIT 1", [], |r| r.get(0)).unwrap();
    attack(book, &format!(
        "INSERT INTO ledger_auditevent(id,company_id,actor_id,actor_name,occurred_at,action,target,detail) VALUES('{event}','{c}','{actor}','owner','2026-11-11T00:00:00Z','{action}','Attachment keys',json_object('attachments',json_array('{attachment}'),'files',json_array('{stored}')));
         INSERT INTO attachments_keywrap(company_id,stored_name,position,wrapped_key,wrap_nonce,audit_id) VALUES('{c}','{stored}',(SELECT coalesce(max(position),0)+1 FROM attachments_keywrap WHERE company_id='{c}' AND stored_name='{stored}'),'{wrapped_key}','{wrap_nonce}','{event}');",
        c = co.to_db(), action = a::REWRAPPED))
}
/// Commit what [`insert_wrap`] proves is accepted.
fn commit_wrap(book: &Book, co: CompanyId, stored: &str, wrapped_key: &str, wrap_nonce: &str) {
    assert_eq!(insert_wrap(book, co, stored, wrapped_key, wrap_nonce), None);
    let event = uuid::Uuid::new_v4().to_string();
    let attachment: String = book.connection().query_row("SELECT id FROM attachments_attachment WHERE company_id=?1 AND stored_name=?2 LIMIT 1", [co.to_db(), stored.to_owned()], |r| r.get(0)).unwrap();
    let actor: String = book.connection().query_row("SELECT id FROM fb_user LIMIT 1", [], |r| r.get(0)).unwrap();
    book.connection().execute_batch(&format!(
        "INSERT INTO ledger_auditevent(id,company_id,actor_id,actor_name,occurred_at,action,target,detail) VALUES('{event}','{c}','{actor}','owner','2026-11-11T00:00:00Z','{action}','Attachment keys',json_object('attachments',json_array('{attachment}'),'files',json_array('{stored}')));
         INSERT INTO attachments_keywrap(company_id,stored_name,position,wrapped_key,wrap_nonce,audit_id) VALUES('{c}','{stored}',(SELECT coalesce(max(position),0)+1 FROM attachments_keywrap WHERE company_id='{c}' AND stored_name='{stored}'),'{wrapped_key}','{wrap_nonce}','{event}');",
        c = co.to_db(), action = a::REWRAPPED)).unwrap();
}

#[test]
fn master_wrapped_files_read_verify_and_gain_a_receipt_key_wrap_once_with_audit() {
    let mut f = F::new();
    let old = f.add(&KEY, &f.owner.clone(), "old.pdf", PDF);
    let shared = f.add(&KEY, &f.second.clone(), "same-bytes.pdf", PDF); // same file, same wrap
    let new = f.add(&RECEIPTS, &f.owner.clone(), "new.pdf", PDF2);
    let (old_id, shared_id, new_id) = (old["id"].as_str().unwrap().to_owned(), shared["id"].as_str().unwrap().to_owned(), new["id"].as_str().unwrap().to_owned());
    // A store on the receipt key alone cannot read the old file; with the master key as legacy it can.
    let strict = Store::for_book(&f.path, &RECEIPTS).unwrap();
    assert_eq!(ReadError::of(&f.download(&strict, &old_id).unwrap_err()), Some(ReadError::NoKey));
    let store = f.store();
    assert_eq!(f.download(&store, &old_id).unwrap(), PDF);
    assert_eq!(f.download(&store, &new_id).unwrap(), PDF2);
    assert_eq!(a::master_only(f.fx.book.connection(), &RECEIPTS, &KEY).unwrap().len(), 1, "one stored file, two rows");
    // A backup made before any re-wrap (as the pre-upgrade copy is) verifies with both keys, and its
    // manifest is in the old format: no further wraps listed.
    let before = f.backup("before.fbbackup", false, None, "2026-11-01T12:00:00Z").unwrap();
    assert!(before.manifest.complete());
    assert!(serde_json::to_value(&before.manifest).unwrap()["attachments"][0]["owners"][0].get("rewraps").is_none());
    f.verify(&before.path).unwrap();
    // Re-wrap: only the master-wrapped file gains a receipt-key wrap (both rows of the shared file read it),
    // under one audit event. Attachment rows themselves never change.
    let new_wrap = f.wrap_of(&new_id);
    let old_wrap = f.wrap_of(&old_id);
    assert!(old_wrap.1.is_empty());
    let moved = a::rewrap_legacy(&mut f.fx.book, f.fx.user, &RECEIPTS, &KEY).unwrap();
    assert_eq!(moved, { let mut v = vec![old_id.clone(), shared_id.clone()]; v.sort(); v });
    let after_wrap = f.wrap_of(&old_id);
    assert_eq!(after_wrap.0, old_wrap.0, "the attachment row is unchanged");
    assert_eq!(after_wrap.1.len(), 1, "one further wrap");
    assert_eq!(f.wrap_of(&shared_id).1, after_wrap.1, "rows sharing the file share its wraps");
    assert_eq!(f.wrap_of(&new_id), new_wrap);
    assert_eq!(f.fx.audit_count(a::REWRAPPED), 1);
    // The audit event names who and which files, in this company, and holds no key material.
    let (event, detail): (String, String) = f.fx.book.connection().query_row("SELECT id, detail FROM ledger_auditevent WHERE action=?1", [a::REWRAPPED], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    let parsed: Value = serde_json::from_str(&detail).unwrap();
    assert_eq!(parsed, serde_json::json!({"attachments": moved, "files": [f.stored_name(&old_id)]}));
    for hex in [&after_wrap.1[0].wrapped_key, &after_wrap.1[0].wrap_nonce, &old_wrap.0.0, &old_wrap.0.1] {
        assert!(!detail.contains(hex.as_str()), "no wrap in the audit detail");
    }
    let audit_of: String = f.fx.book.connection().query_row("SELECT audit_id FROM attachments_keywrap", [], |r| r.get(0)).unwrap();
    assert_eq!(audit_of, event);
    assert_eq!(f.download(&strict, &old_id).unwrap(), PDF, "the receipt key alone now reads it");
    assert_eq!(f.download(&strict, &shared_id).unwrap(), PDF);
    assert!(a::master_only(f.fx.book.connection(), &RECEIPTS, &KEY).unwrap().is_empty());
    assert!(f.fx.book.check().unwrap().is_empty(), "{:?}", f.fx.book.check());
    // Nothing left to move; a second call writes nothing and records nothing.
    assert!(a::rewrap_legacy(&mut f.fx.book, f.fx.user, &RECEIPTS, &KEY).unwrap().is_empty());
    assert_eq!(f.fx.audit_count(a::REWRAPPED), 1);
    // The backup made after the re-wrap carries the wrap and verifies under the receipt key alone.
    let after = f.backup("after.fbbackup", false, None, "2026-11-01T12:01:00Z").unwrap();
    let carried = after.manifest.attachments.iter().find(|r| r.entry.ends_with(&f.stored_name(&old_id))).unwrap();
    assert!(carried.owners.iter().all(|o| o.rewraps == after_wrap.1));
    let strict_verifier = backups::attachment_receipt_verifier(&RECEIPTS);
    backups::verify_backup_with_receipts(&after.path, &KEY, &f.work(), None, false, Some(&strict_verifier)).unwrap();
    // It restores with the wraps, and the restored book reads the file under the receipt key alone; the
    // backup made before the re-wrap (an older schema's backup reads the same way) restores too.
    let restored = f.restore(&after.path, "restored-after");
    let mut book = Book::open(&restored, &KEY).unwrap();
    let there = Store::for_book(&restored, &RECEIPTS).unwrap();
    assert_eq!(a::download(&mut book, &there, f.fx.user, f.co, &old_id).unwrap(), PDF);
    let earlier = f.restore(&before.path, "restored-before");
    let mut book = Book::open(&earlier, &KEY).unwrap();
    let there = Store::for_book(&earlier, &RECEIPTS).unwrap().with_legacy(&KEY).unwrap();
    assert_eq!(a::download(&mut book, &there, f.fx.user, f.co, &old_id).unwrap(), PDF);
    // The re-wrapped row can still be removed with a reason, and the book checks clean.
    a::remove(&mut f.fx.book, f.fx.user, f.co, &old_id, "Synthetic correction").unwrap();
    assert!(f.fx.book.check().unwrap().is_empty());
}

#[test]
fn a_file_no_key_opens_is_reported_as_keyless_never_as_damaged() {
    let mut f = F::new();
    let row = f.add(&[5; 32], &f.owner.clone(), "foreign.pdf", PDF);
    let id = row["id"].as_str().unwrap().to_owned();
    let store = f.store();
    let error = f.download(&store, &id).unwrap_err();
    assert_eq!(ReadError::of(&error), Some(ReadError::NoKey));
    assert!(error.message().contains("none of the keys these books hold opens this attached file"), "{}", error.message());
    // Re-wrap leaves it alone: it cannot be authenticated, so it must not be blessed under the receipt key.
    assert!(a::rewrap_legacy(&mut f.fx.book, f.fx.user, &RECEIPTS, &KEY).unwrap().is_empty());
    assert_eq!(a::unopenable(f.fx.book.connection(), &KEY).unwrap().len(), 1);
    let refused = f.backup("refused.fbbackup", false, None, "2026-11-01T12:00:00Z").err().expect("refused");
    assert_eq!(refused.absent().len(), 1);
    assert_eq!(refused.absent()[0].problem, backups::NO_KEY);
    let text = refused.to_string();
    assert!(text.contains("“foreign.pdf” on Journal entry") && text.contains("(no key opens it)"), "{text}");
    assert!(text.contains("still in the attachment folder, unchanged") && text.contains("key file"), "{text}");
    assert!(!text.contains("damaged"), "keyless is not damage: {text}");
    // An incomplete backup lists it as keyless, verifies, and the problem survives the manifest check.
    let made = f.backup("incomplete.fbbackup", true, None, "2026-11-01T12:01:00Z").unwrap();
    assert_eq!(made.manifest.absent[0].problem, backups::NO_KEY);
    f.verify(&made.path).unwrap();
}

#[test]
fn reading_tells_a_missing_file_from_a_damaged_one() {
    let mut f = F::new();
    let gone = f.add(&RECEIPTS, &f.owner.clone(), "gone.pdf", PDF);
    let bad = f.add(&RECEIPTS, &f.owner.clone(), "bad.pdf", PDF2);
    let (gone, bad) = (gone["id"].as_str().unwrap().to_owned(), bad["id"].as_str().unwrap().to_owned());
    fs::remove_file(f.receipts().join(f.stored_name(&gone))).unwrap();
    let damaged = f.receipts().join(f.stored_name(&bad));
    let mut bytes = fs::read(&damaged).unwrap();
    bytes[5] ^= 1;
    fs::write(&damaged, bytes).unwrap();
    let store = f.store();
    assert_eq!(ReadError::of(&f.download(&store, &gone).unwrap_err()), Some(ReadError::Missing));
    assert_eq!(ReadError::of(&f.download(&store, &bad).unwrap_err()), Some(ReadError::Damaged));
    // A file cut short is damaged too (wrong size), not missing.
    fs::write(&damaged, b"short").unwrap();
    assert_eq!(ReadError::of(&f.download(&store, &bad).unwrap_err()), Some(ReadError::Damaged));
}

/// Review finding 2: one further wrap that does not open (a bug, a future feature, a crafted row) must not
/// make a good file unreadable: every wrap is tried. The schema keeps wraps permanent, in order, fresh,
/// in their own company, and tied to an audit event that holds no key material.
#[test]
fn a_bad_or_duplicate_further_wrap_never_breaks_a_good_file_and_wraps_are_permanent() {
    let mut f = F::new();
    let good = f.add(&RECEIPTS, &f.owner.clone(), "good.pdf", PDF);
    let old = f.add(&KEY, &f.second.clone(), "old.pdf", PDF2);
    let (good, old) = (good["id"].as_str().unwrap().to_owned(), old["id"].as_str().unwrap().to_owned());
    let (stored, old_stored) = (f.stored_name(&good), f.stored_name(&old));
    // A wrap that authenticates under no key, recorded as the newest one: the row's own wrap still opens.
    commit_wrap(&f.fx.book, f.co, &stored, &"ab".repeat(48), &"cd".repeat(12));
    let strict = Store::for_book(&f.path, &RECEIPTS).unwrap();
    assert_eq!(f.download(&strict, &good).unwrap(), PDF);
    // A bogus wrap ahead of a real re-wrap: the real one still opens after the master key is gone.
    commit_wrap(&f.fx.book, f.co, &old_stored, &"ef".repeat(48), &"01".repeat(12));
    assert_eq!(a::rewrap_legacy(&mut f.fx.book, f.fx.user, &RECEIPTS, &KEY).unwrap(), vec![old.clone()]);
    commit_wrap(&f.fx.book, f.co, &old_stored, &"23".repeat(48), &"45".repeat(12));
    assert_eq!(f.wrap_of(&old).1.len(), 3);
    assert_eq!(f.download(&strict, &old).unwrap(), PDF2, "the bogus newest wrap is skipped");
    assert!(f.fx.book.check().unwrap().is_empty(), "{:?}", f.fx.book.check());
    // Backups carry every wrap, verify, and restore; the files read back.
    let made = f.backup("wraps.fbbackup", false, None, "2026-11-01T12:00:00Z").unwrap();
    assert!(made.manifest.complete(), "{:?}", made.manifest.absent);
    let verifier = backups::attachment_receipt_verifier(&RECEIPTS);
    backups::verify_backup_with_receipts(&made.path, &KEY, &f.work(), None, false, Some(&verifier)).unwrap();
    let restored = f.restore(&made.path, "restored");
    let mut book = Book::open(&restored, &KEY).unwrap();
    let there = Store::for_book(&restored, &RECEIPTS).unwrap();
    assert_eq!(a::download(&mut book, &there, f.fx.user, f.co, &good).unwrap(), PDF);
    assert_eq!(a::download(&mut book, &there, f.fx.user, f.co, &old).unwrap(), PDF2);
    // The schema: a duplicate wrap (same nonce or same bytes) is refused; so is a position out of order,
    // a file of another company, an event of another company, or an event carrying anything but names.
    assert_eq!(insert_wrap(&f.fx.book, f.co, &stored, &"ab".repeat(48), &"99".repeat(12)).as_deref(), Some("attachment_keywrap_fresh"));
    assert_eq!(insert_wrap(&f.fx.book, f.co, &stored, &"98".repeat(48), &"cd".repeat(12)).as_deref(), Some("attachment_keywrap_fresh"));
    let row: (String, String) = f.fx.book.connection().query_row("SELECT wrapped_key, wrap_nonce FROM attachments_attachment WHERE id=?1", [&good], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(insert_wrap(&f.fx.book, f.co, &stored, &row.0, &"97".repeat(12)).as_deref(), Some("attachment_keywrap_fresh"), "the row's own wrap again");
    let actor: String = f.fx.book.connection().query_row("SELECT id FROM fb_user LIMIT 1", [], |r| r.get(0)).unwrap();
    let event = uuid::Uuid::new_v4();
    assert_eq!(attack(&f.fx.book, &format!("INSERT INTO ledger_auditevent(id,company_id,actor_id,actor_name,occurred_at,action,target,detail) VALUES('{event}','{}','{actor}','owner','2026-11-11T00:00:00Z','{}','Attachment keys',json_object('attachments',json_array('{good}'),'files',json_array(json_object('stored_name','{stored}','wrapped_key','{}'))))", f.co.to_db(), a::REWRAPPED, "ab".repeat(48))).as_deref(), Some("attachment_keywrap_event"), "no key material in the event");
    assert_eq!(attack(&f.fx.book, &format!("INSERT INTO ledger_auditevent(id,company_id,actor_id,actor_name,occurred_at,action,target,detail) VALUES('{event}','{}','{actor}','owner','2026-11-11T00:00:00Z','{}','Attachment keys',json_object('attachments',json_array('{good}'),'files',json_array('{stored}'),'wraps',json_array('{}')))", f.co.to_db(), a::REWRAPPED, "ab".repeat(48))).as_deref(), Some("attachment_keywrap_event"));
    let other = f.fx.company("Synthetic other company");
    assert_eq!(attack(&f.fx.book, &format!("INSERT INTO ledger_auditevent(id,company_id,actor_id,actor_name,occurred_at,action,target,detail) VALUES('{event}','{}','{actor}','owner','2026-11-11T00:00:00Z','{}','Attachment keys',json_object('attachments',json_array('{good}'),'files',json_array('{stored}')))", other.to_db(), a::REWRAPPED)).as_deref(), Some("attachment_keywrap_event"), "an event names only its own company's files");
    assert_eq!(attack(&f.fx.book, &format!("INSERT INTO attachments_keywrap VALUES('{}','{stored}',5,'{}','{}','{event}')", f.co.to_db(), "77".repeat(48), "76".repeat(12))).as_deref(), Some("attachment_keywrap_order"));
    assert_eq!(attack(&f.fx.book, &format!("INSERT INTO attachments_keywrap VALUES('{}','{stored}',1,'{}','{}','{event}')", other.to_db(), "77".repeat(48), "76".repeat(12))).as_deref(), Some("attachment_keywrap_file"));
    assert_eq!(attack(&f.fx.book, &format!("INSERT INTO attachments_keywrap VALUES('{}','{stored}',2,'{}','{}','{event}')", f.co.to_db(), "77".repeat(48), "76".repeat(12))).as_deref(), Some("attachment_keywrap_audit"), "no such event");
    assert_eq!(attack(&f.fx.book, "UPDATE attachments_keywrap SET wrap_nonce=wrap_nonce").as_deref(), Some("attachment_keywrap_immutable"));
    assert_eq!(attack(&f.fx.book, "DELETE FROM attachments_keywrap").as_deref(), Some("attachment_keywrap_immutable"));
    assert!(attack(&f.fx.book, &format!("DELETE FROM ledger_auditevent WHERE action='{}'", a::REWRAPPED)).is_some(), "audit events are never deleted");
    // The attachment row itself still refuses any key change.
    let update = format!("UPDATE attachments_attachment SET wrap_nonce='{}' WHERE id='{good}'", "cd".repeat(12));
    assert_eq!(attack(&f.fx.book, &update).as_deref(), Some("attachment_number"));
}

/// Review finding 3: each company's files are re-wrapped under an event in that company, naming only its
/// own attachments; a company the owner is not a member of is left alone and reported, never touched.
#[test]
fn each_company_records_its_own_rewrap_and_another_owners_company_is_left_and_reported() {
    let mut f = F::new();
    let first = f.add(&KEY, &f.owner.clone(), "first.pdf", PDF);
    let second_co = f.fx.company("Synthetic second company");
    let cash = f.fx.account(second_co, "1000", "Cash", "ASSET");
    let equity = f.fx.account(second_co, "3000", "Capital", "EQUITY");
    let j = f.fx.draft(second_co, cash, equity, "3.00", "2026-01-03");
    let store = Store::for_book(&f.path, &KEY).unwrap();
    let second = a::add(&mut f.fx.book, &store, f.fx.user, second_co, &Owner::Journal(j.id), PDF2, "second.pdf", &uuid::Uuid::new_v4().to_string(), &Limits::default()).unwrap().0;
    // A third company made by someone else; the owner is not a member.
    let stranger = users::create_user(&mut f.fx.book, "stranger", false).unwrap();
    let third_co = fb_core::services::create_company(&mut f.fx.book, stranger, "Synthetic stranger company", "USD", false).unwrap().id;
    let cash = fb_core::services::save_account(&mut f.fx.book, stranger, third_co, fb_core::services::AccountInput::new("1000", "Cash").kind("ASSET")).unwrap().id;
    let equity = fb_core::services::save_account(&mut f.fx.book, stranger, third_co, fb_core::services::AccountInput::new("3000", "Capital").kind("EQUITY")).unwrap().id;
    let lines = vec![fb_core::services::line(cash).debit("4.00").credit("0").build(), fb_core::services::line(equity).debit("0").credit("4.00").build()];
    let j = fb_core::services::save_draft(&mut f.fx.book, stranger, third_co, fb_core::services::DraftInput::new("2026-01-04", "Synthetic", lines)).unwrap();
    let third = a::add(&mut f.fx.book, &store, stranger, third_co, &Owner::Journal(j.id), PDF3, "third.pdf", &uuid::Uuid::new_v4().to_string(), &Limits::default()).unwrap().0;
    let moved = a::rewrap_legacy(&mut f.fx.book, f.fx.user, &RECEIPTS, &KEY).unwrap();
    let mut expected = vec![first["id"].as_str().unwrap().to_owned(), second["id"].as_str().unwrap().to_owned()];
    expected.sort();
    assert_eq!(moved, expected);
    let events: Vec<(String, String)> = f.fx.book.connection().prepare("SELECT company_id, detail FROM ledger_auditevent WHERE action=?1 ORDER BY company_id").unwrap()
        .query_map([a::REWRAPPED], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
    assert_eq!(events.len(), 2, "one event per company");
    for (company, detail) in &events {
        let detail: Value = serde_json::from_str(detail).unwrap();
        let (row, name) = if *company == f.co.to_db() { (&first, "first.pdf") } else { (&second, "second.pdf") };
        assert_eq!(detail, serde_json::json!({"attachments": [row["id"]], "files": [row["stored_name"]]}), "{name}");
    }
    let mismatched: i64 = f.fx.book.connection().query_row("SELECT count(*) FROM attachments_keywrap w JOIN ledger_auditevent e ON e.id=w.audit_id WHERE e.company_id<>w.company_id", [], |r| r.get(0)).unwrap();
    assert_eq!(mismatched, 0);
    let left = a::master_only(f.fx.book.connection(), &RECEIPTS, &KEY).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!((left[0].company, left[0].stored_name.as_str()), (third_co, third["stored_name"].as_str().unwrap()));
    assert!(f.fx.book.check().unwrap().is_empty(), "{:?}", f.fx.book.check());
}

/// Review finding 6: a permission error reading an attached file says nothing about the file. It stops
/// the backup (even one allowed to be incomplete), naming the cause, instead of calling the file damaged.
#[cfg(unix)]
#[test]
fn a_system_error_reading_an_attached_file_stops_the_backup_and_names_the_cause() {
    use std::os::unix::fs::PermissionsExt;
    let mut f = F::new();
    let row = f.add(&RECEIPTS, &f.owner.clone(), "locked.pdf", PDF);
    let file = f.receipts().join(row["stored_name"].as_str().unwrap());
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::File::open(&file).is_ok() {
        // Running with privileges that ignore file modes: nothing to prove here.
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        return;
    }
    let stopped = f.backup("stopped.fbbackup", true, None, "2026-11-01T12:00:00Z").err().expect("stopped");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(stopped.absent().is_empty(), "not reported as a missing or damaged file");
    let text = stopped.to_string();
    assert!(text.contains("could not be read") && text.contains("ermission denied"), "{text}");
    assert!(!f.out().join("stopped.fbbackup").exists());
    f.backup("after.fbbackup", false, None, "2026-11-01T12:01:00Z").unwrap();
}

/// Review finding 7: retention protects the newest verified COMPLETE backup; a newer incomplete one does
/// not stand in for it.
#[test]
fn retention_keeps_the_newest_complete_backup_when_a_newer_one_is_incomplete() {
    let mut f = F::new();
    let row = f.add(&RECEIPTS, &f.owner.clone(), "kept.pdf", PDF);
    let file = f.receipts().join(row["stored_name"].as_str().unwrap());
    let complete = f.backup("freedombooks-synthetic-20261101T100000Z.fbbackup", false, None, "2026-11-01T10:00:00Z").unwrap();
    f.verify(&complete.path).unwrap();
    let bytes = fs::read(&file).unwrap();
    fs::remove_file(&file).unwrap();
    let incomplete = f.backup("freedombooks-synthetic-20261101T110000Z.fbbackup", true, None, "2026-11-01T11:00:00Z").unwrap();
    f.verify(&incomplete.path).unwrap();
    assert!(backups::retention::verified_note(&incomplete.path) && !backups::retention::complete(&incomplete.path));
    fs::write(&file, bytes).unwrap();
    let newest = f.backup("freedombooks-synthetic-20261101T120000Z.fbbackup", false, Some(1), "2026-11-01T12:00:00Z").unwrap();
    assert_eq!(newest.pruned, vec![incomplete.path.clone()], "the incomplete one goes, the last verified complete one stays");
    assert!(complete.path.exists() && newest.path.exists());
}

#[test]
fn missing_and_damaged_files_are_named_and_an_incomplete_backup_is_labelled_verified_and_restorable() {
    let mut f = F::new();
    let kept = f.add(&RECEIPTS, &f.owner.clone(), "kept.pdf", PDF);
    let gone = f.add(&RECEIPTS, &f.owner.clone(), "gone.pdf", PDF2);
    let bad = f.add(&RECEIPTS, &f.second.clone(), "bad.pdf", PDF3);
    a::remove(&mut f.fx.book, f.fx.user, f.co, bad["id"].as_str().unwrap(), "Synthetic correction").unwrap();
    // A complete backup first, kept by retention below.
    let complete = f.backup("complete.fbbackup", false, None, "2026-11-01T11:00:00Z").unwrap();
    fs::remove_file(f.receipts().join(gone["stored_name"].as_str().unwrap())).unwrap();
    let damaged = f.receipts().join(bad["stored_name"].as_str().unwrap());
    let mut bytes = fs::read(&damaged).unwrap();
    bytes[3] ^= 1;
    fs::write(&damaged, bytes).unwrap();
    // By default the backup is refused, naming each file and its document; nothing is written.
    let refused = f.backup("refused.fbbackup", false, None, "2026-11-01T12:00:00Z").err().expect("refused");
    let text = refused.to_string();
    assert!(text.starts_with("2 attached files are missing or damaged"), "{text}");
    assert!(text.contains("“gone.pdf” on Journal entry") && text.contains("(missing)"), "{text}");
    assert!(text.contains("“bad.pdf” on Journal entry") && text.contains("already removed (damaged)"), "{text}");
    assert!(!text.contains("disk space"), "{text}");
    assert_eq!(refused.absent().len(), 2);
    assert!(!f.out().join("refused.fbbackup").exists());
    // Incomplete on request: labelled in the public header and the manifest, verified, and it prunes nothing.
    let made = f.backup("incomplete.fbbackup", true, Some(1), "2026-11-01T12:00:00Z").unwrap();
    assert!(!made.manifest.complete());
    assert!(made.pruned.is_empty(), "an incomplete backup never prunes a complete one");
    assert!(complete.path.exists());
    assert_eq!(backups::read_header(&made.path).unwrap()["incomplete"], true);
    assert!(backups::read_header(&complete.path).unwrap().get("incomplete").is_none());
    assert_eq!(made.manifest.attachments.len(), 1);
    let mut problems: Vec<_> = made.manifest.absent.iter().map(|a| (a.name.as_str(), a.problem.as_str(), a.removed)).collect();
    problems.sort();
    assert_eq!(problems, [("bad.pdf", backups::DAMAGED, true), ("gone.pdf", backups::MISSING, false)]);
    let verified = f.verify(&made.path).unwrap();
    assert_eq!(verified.absent, made.manifest.absent);
    let note: Value = serde_json::from_str(&fs::read_to_string(backups::retention::sidecar(&made.path)).unwrap()).unwrap();
    assert_eq!(note["complete"], false);
    assert_eq!(note["missing_attachments"], 2);
    // It restores: the kept file reads back, the absent ones are simply not there.
    let target_dir = f.dir.path().join("restored");
    fs::create_dir(&target_dir).unwrap();
    let target = target_dir.join("book.fbbook");
    let verifier = backups::attachment_receipt_verifier(&RECEIPTS);
    let restored = backups::restore_backup(&made.path, &KEY, &RestoreOptions {
        verify_receipt: Some(&verifier), target: &target, receipts_target: Some(&target_dir.join("receipts")), work_root: &f.work(),
        protected: &[f.path.clone(), f.receipts(), f.out(), f.work()], passphrase: None, require_encrypted: false,
    }).unwrap();
    assert_eq!(restored.manifest.absent.len(), 2);
    let mut book = Book::open(&target, &KEY).unwrap();
    let store = Store::for_book(&target, &RECEIPTS).unwrap();
    assert_eq!(a::download(&mut book, &store, f.fx.user, f.co, kept["id"].as_str().unwrap()).unwrap(), PDF);
    assert!(a::download(&mut book, &store, f.fx.user, f.co, gone["id"].as_str().unwrap()).is_err());
    // A later complete backup may prune; an incomplete one with the same label is not mistaken for complete.
    let whole = f.receipts();
    fs::remove_dir_all(&whole).unwrap();
    let no_folder = f.backup("no-folder.fbbackup", false, None, "2026-11-01T13:00:00Z").err().expect("refused");
    assert!(no_folder.to_string().contains("“receipts” folder"), "{no_folder}");
    assert_eq!(no_folder.absent().len(), 3);
}

#[test]
fn an_incomplete_flag_that_does_not_match_the_manifest_is_refused() {
    let mut f = F::new();
    let gone = f.add(&RECEIPTS, &f.owner.clone(), "gone.pdf", PDF);
    fs::remove_file(f.receipts().join(gone["stored_name"].as_str().unwrap())).unwrap();
    let made = f.backup("incomplete.fbbackup", true, None, "2026-11-01T12:00:00Z").unwrap();
    // Rewrite the archive with the header's incomplete flag dropped: the header is the manifest's
    // associated data, so the backup no longer verifies.
    let mut archive = tar::Archive::new(fs::File::open(&made.path).unwrap());
    let forged = f.out().join("forged.fbbackup");
    let mut builder = tar::Builder::new(fs::File::create(&forged).unwrap());
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let name = entry.path().unwrap().to_string_lossy().into_owned();
        let mut data = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut data).unwrap();
        if name == "header.json" {
            let mut header: Value = serde_json::from_slice(&data).unwrap();
            header.as_object_mut().unwrap().remove("incomplete");
            data = serde_json::to_vec_pretty(&header).unwrap();
            data.push(b'\n');
        }
        let mut h = tar::Header::new_ustar();
        h.set_size(data.len() as u64);
        h.set_mode(0o600);
        h.set_cksum();
        builder.append_data(&mut h, &name, data.as_slice()).unwrap();
    }
    builder.into_inner().unwrap().sync_all().unwrap();
    assert!(f.verify(&forged).is_err());
}

/// Every schema object of a book, exactly as stored (`sqlite_master` minus the page numbers).
fn schema_of(conn: &fb_db::rusqlite::Connection) -> Vec<(String, String, String, Option<String>)> {
    let mut stmt = conn.prepare("SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name").unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().collect::<Result<_, _>>().unwrap()
}

/// The two migrations developed in parallel land in order: 0196 (bank-feed review safety) then 0197
/// (attachment key wraps). A populated book from before both, holding a file wrapped under its master key,
/// upgrades in one open, and also step by step (0196 alone, as a build that had only it would leave the
/// book, then 0197). Each stage authenticates against its versioned schema; both end byte-identical in
/// schema to a fresh book, with every old row unchanged; and the upgraded book re-wraps and reads the file.
#[test]
fn a_book_from_before_0196_upgrades_through_bank_feed_safety_and_key_wraps_to_the_fresh_schema() {
    let mut f = F::new();
    let old = f.add(&KEY, &f.owner.clone(), "old.pdf", PDF);
    let old_id = old["id"].as_str().unwrap().to_owned();
    let bank = fb_db::MIGRATIONS.iter().position(|m| m.0 == "0196_bankfeeds_review_safety").unwrap();
    assert_eq!(fb_db::MIGRATIONS[bank + 1].0, "0197_attachment_key_wraps");
    assert_eq!(fb_db::MIGRATIONS[bank + 2].0, "0198_legacy_attachment_key_wraps");
    let live = f.path.parent().unwrap().to_owned();
    let fresh = Book::create(&live.join("fresh.fbbook"), &KEY).unwrap();
    let fresh_schema = schema_of(fresh.connection());
    drop(fresh);
    for stepped in [false, true] {
        let path = live.join(if stepped { "stepped.fbbook" } else { "direct.fbbook" });
        let prior = fb_db::test_support::populated_copy(f.fx.book.connection(), &path, &KEY, "0196_bankfeeds_review_safety", false).unwrap();
        let prior_schema = {
            let c = fb_db::rusqlite::Connection::open(&path).unwrap();
            c.execute_batch(&format!("PRAGMA key=\"x'{}'\"", "2a".repeat(32))).unwrap();
            let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
            assert_eq!(v as usize, bank, "the copy is at the schema before 0196");
            let schema = schema_of(&c);
            assert!(!schema.iter().any(|o| o.1 == "bankfeeds_link" || o.1 == "attachments_keywrap"));
            if stepped {
                // Exactly what the migration loop does for one step.
                c.execute_batch(&format!("BEGIN IMMEDIATE; {} PRAGMA user_version = {}; COMMIT;", fb_db::MIGRATIONS[bank].1, bank + 1)).unwrap();
            }
            schema
        };
        let from = if stepped { bank + 1 } else { bank };
        let fb_db::Opened::Behind(behind) = fb_db::Book::open_staged(&path, &KEY).unwrap() else { panic!("not behind") };
        assert_eq!(behind.from(), from, "the stage authenticates at its own version");
        assert_eq!(fb_db::SnapshotSource::migrations(&behind).iter().map(|m| m.0).collect::<Vec<_>>(), fb_db::MIGRATIONS[..from].iter().map(|m| m.0).collect::<Vec<_>>());
        let upgraded = behind.upgrade().unwrap();
        assert_eq!(fb_db::test_support::snapshot(upgraded.connection()).unwrap(), prior.expected, "every old row is unchanged");
        let schema = schema_of(upgraded.connection());
        assert_eq!(schema, fresh_schema, "an upgraded book has exactly the fresh schema (stepped: {stepped})");
        // Pin the prior objects intentionally replaced by later migrations. All other historical
        // schema objects remain byte-exact after the full upgrade.
        let changed: Vec<_> = prior_schema.iter().filter(|object| !schema.contains(object))
            .map(|object| object.1.as_str()).collect();
        // 0200 rebuilds the numbering tables, later migrations extend the shared views,
        // 0207 adds exact-ID evidence to the two bank-feed tables, 0209 replaces the two deposit views,
        // and 0210 replaces the two bank-feed decision triggers and two bank-feed integrity views.
        assert_eq!(changed, ["bankfeeds_account", "bankfeeds_rule", "bankfeeds_transaction", "directory_customer", "documents_documentidentity", "documents_documentsequence", "ledger_company", "fb_bankfeeds_transaction_eligible", "fb_bankfeeds_transaction_update", "fb_transfer_insert", "bankfeeds_invalid", "bankfeeds_text_invalid", "banking_deposit_expected", "banking_deposit_invalid", "fb_json_invalid", "fb_value_invalid"]);
        assert!(upgraded.check().unwrap().is_empty(), "{:?}", upgraded.check());
        drop(upgraded);
        let mut book = Book::open(&path, &KEY).unwrap();
        assert!(book.check().unwrap().is_empty());
        // The key-wrap migration works on the upgraded book: the master-wrapped file gains its receipt-key
        // wrap and then opens under the receipt key alone.
        let strict = Store::for_book(&path, &RECEIPTS).unwrap();
        assert_eq!(ReadError::of(&a::download(&mut book, &strict, f.fx.user, f.co, &old_id).unwrap_err()), Some(ReadError::NoKey));
        assert_eq!(a::rewrap_legacy(&mut book, f.fx.user, &RECEIPTS, &KEY).unwrap(), std::slice::from_ref(&old_id));
        assert_eq!(a::download(&mut book, &strict, f.fx.user, f.co, &old_id).unwrap(), PDF);
        assert!(book.check().unwrap().is_empty());
        drop(book);
        assert!(Book::open(&path, &KEY).unwrap().check().unwrap().is_empty());
    }
}


/// Authenticated wraps can still hold the wrong content key. Preserve all outgoing-master
/// candidates by key equality, including when the receipts folder is away during a password change.
#[test]
fn authentic_wrong_content_keys_cannot_skip_preserving_the_real_key() {
    use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce, aead::{Aead, Payload}};
    use hkdf::Hkdf;
    use sha2::Sha256;
    let mut f = F::new();
    let row = f.add(&KEY, &f.owner.clone(), "old.pdf", PDF);
    let id = row["id"].as_str().unwrap().to_owned();
    let stored = f.stored_name(&id);
    let aad = format!("FreedomBooks receipt v1:{}:{stored}", f.co.to_db());
    let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    for (key, content, nonce) in [(RECEIPTS, [92; 32], [91; 12]), (KEY, [93; 32], [94; 12])] {
        let mut derived = [0; 32];
        Hkdf::<Sha256>::new(None, &key).expand(b"FreedomBooks receipt key wrapping v1", &mut derived).unwrap();
        let wrapped = ChaCha20Poly1305::new_from_slice(&derived).unwrap().encrypt(Nonce::from_slice(&nonce), Payload { msg: &content, aad: aad.as_bytes() }).unwrap();
        commit_wrap(&f.fx.book, f.co, &stored, &hex(&wrapped), &hex(&nonce));
    }
    assert_eq!(f.download(&f.store(), &id).unwrap(), PDF, "the original master wrap still reads");
    assert_eq!(a::master_only(f.fx.book.connection(), &RECEIPTS, &KEY).unwrap().len(), 1);
    let away = f.dir.path().join("receipts-away");
    fs::rename(f.receipts(), &away).unwrap();
    assert_eq!(a::rewrap_legacy(&mut f.fx.book, f.fx.user, &RECEIPTS, &KEY).unwrap(), vec![id.clone()]);
    assert!(a::master_only(f.fx.book.connection(), &RECEIPTS, &KEY).unwrap().is_empty());
    fs::rename(&away, f.receipts()).unwrap();
    let strict = Store::for_book(&f.path, &RECEIPTS).unwrap();
    assert_eq!(f.download(&strict, &id).unwrap(), PDF, "no original master is needed");
    assert_eq!(f.wrap_of(&id).1.len(), 4, "both distinct master candidates gained receipt wraps");
    let repaired = a::rewrap_legacy(&mut f.fx.book, f.fx.user, &RECEIPTS, &KEY).unwrap();
    assert!(repaired.is_empty(), "the repair is idempotent");
    assert!(f.fx.book.check().unwrap().is_empty());
}


/// A real older writer, after changing its master key, used audit wraps as manifest own-wrap fields.
/// Its backup must verify and restore before migration, then reopen with its original audit bytes intact.
#[test]
fn a_genuine_first_lane_backup_restores_upgrades_and_keeps_its_attachment_keys() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/attachment-first-lane");
    let meta: Value = serde_json::from_slice(&fs::read(fixture.join("fixture.json")).unwrap()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("private"); fs::create_dir(&work).unwrap();
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; fs::set_permissions(&work, fs::Permissions::from_mode(0o700)).unwrap(); }
    let archive = fixture.join("first-lane.fbbackup");
    let verifier = backups::attachment_receipt_verifier(&RECEIPTS);
    let manifest = backups::verify_backup_with_receipts(&archive, &[43; 32], &work, None, false, Some(&verifier)).unwrap();
    assert!(manifest.complete());
    assert!(manifest.attachments.iter().all(|f| f.owners.iter().all(|r| r.rewraps.is_empty())), "the fixture keeps its actual old manifest");
    let live = dir.path().join("restored"); fs::create_dir(&live).unwrap();
    let book_path = live.join("book.fbbook");
    backups::restore_backup(&archive, &[43; 32], &RestoreOptions { target: &book_path, receipts_target: Some(&live.join("receipts")), work_root: &work,
        protected: std::slice::from_ref(&fixture), passphrase: None, require_encrypted: false, verify_receipt: Some(&verifier) }).unwrap();
    let mut book = Book::open(&book_path, &[43; 32]).unwrap();
    assert!(book.check().unwrap().is_empty());
    let raw: String = book.read(|q| q.query_row("SELECT detail FROM ledger_auditevent WHERE action=?1", [a::REWRAPPED], |r| r.get(0))).unwrap();
    assert_eq!(raw, meta["audit_detail"].as_str().unwrap(), "historical actor evidence is byte-exact");
    let actor: String = book.read(|q| q.query_row("SELECT id FROM fb_user", [], |r| r.get(0))).unwrap();
    let actor = fb_core::UserId::parse(&actor).unwrap();
    let strict = Store::for_book(&book_path, &RECEIPTS).unwrap();
    for (company, attachment) in [("company", "attachment"), ("other_company", "other_attachment")] {
        let co = CompanyId::parse(meta[company].as_str().unwrap()).unwrap();
        assert_eq!(a::download(&mut book, &strict, actor, co, meta[attachment].as_str().unwrap()).unwrap(), PDF);
    }
    // The old event shape is grandfathered evidence, never an allowed new write.
    let co = meta["company"].as_str().unwrap();
    let literal = raw.replace('\'', "''");
    assert_eq!(attack(&book, &format!("INSERT INTO ledger_auditevent(id,company_id,actor_id,actor_name,occurred_at,action,target,detail) VALUES('{}','{co}','{}','Synthetic legacy owner','2026-11-11T00:00:00Z','{}','Attachment keys','{literal}')", uuid::Uuid::new_v4(), actor.to_db(), a::REWRAPPED)).as_deref(), Some("attachment_keywrap_event"));
    drop(book);
    assert!(Book::open(&book_path, &[43; 32]).unwrap().check().unwrap().is_empty());
}


#[test]
fn a_retained_0197_book_with_wrap_rows_upgrades_without_rewriting_any_evidence() {
    let mut f = F::new();
    let row = f.add(&KEY, &f.owner.clone(), "old.pdf", PDF);
    let id = row["id"].as_str().unwrap().to_owned();
    a::rewrap_legacy(&mut f.fx.book, f.fx.user, &RECEIPTS, &KEY).unwrap();
    let path = f.path.with_file_name("prior-0197.fbbook");
    let prior = fb_db::test_support::populated_copy(f.fx.book.connection(), &path, &KEY, "0198_legacy_attachment_key_wraps", false).unwrap();
    let mut upgraded = Book::open(&path, &KEY).unwrap();
    assert_eq!(fb_db::test_support::snapshot(upgraded.connection()).unwrap(), prior.expected, "all table rows, including wraps and actor audit, are unchanged");
    assert_eq!(schema_of(upgraded.connection()), schema_of(f.fx.book.connection()));
    let strict = Store::for_book(&path, &RECEIPTS).unwrap();
    assert_eq!(a::download(&mut upgraded, &strict, f.fx.user, f.co, &id).unwrap(), PDF);
    assert!(upgraded.check().unwrap().is_empty());
}

/// Compatibility admits the exact historical projection only for an authenticated pre-0197 book.
/// A current writer cannot substitute wraps in its manifest and rely on the legacy exception.
#[test]
fn current_schema_backups_cannot_use_the_legacy_substituted_wrap_projection() {
    let mut f = F::new();
    f.add(&KEY, &f.owner.clone(), "old.pdf", PDF);
    a::rewrap_legacy(&mut f.fx.book, f.fx.user, &RECEIPTS, &KEY).unwrap();
    let made = f.backup("current.fbbackup", false, None, "2026-11-11T00:00:00Z").unwrap();
    let mut archive = tar::Archive::new(fs::File::open(&made.path).unwrap());
    let mut members = Vec::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let name = entry.path().unwrap().to_string_lossy().into_owned();
        let mut data = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut data).unwrap();
        members.push((name, data));
    }
    let old_header = &members.iter().find(|m| m.0 == "header.json").unwrap().1;
    let mut header: Value = serde_json::from_slice(old_header).unwrap();
    let encrypted = &members.iter().find(|m| m.0 == "manifest.enc").unwrap().1;
    let plain = backups::manifest::decrypt(encrypted, &KEY, &header, old_header).unwrap();
    let mut data: Value = serde_json::from_slice(&plain).unwrap();
    for file in data["attachments"].as_array_mut().unwrap() {
        for row in file["owners"].as_array_mut().unwrap() {
            let wrap = row["rewraps"][0].clone();
            assert!(wrap.is_object());
            row["wrapped_key"] = wrap["wrapped_key"].clone();
            row["wrap_nonce"] = wrap["wrap_nonce"].clone();
            row.as_object_mut().unwrap().remove("rewraps");
        }
    }
    // Fresh nonce and a valid authentication tag: rejection must be about the schema/manifest
    // mismatch, not accidental ciphertext corruption or nonce reuse.
    header["manifest_nonce"] = serde_json::json!(backups::manifest::new_nonce());
    let raw_header = serde_json::to_vec_pretty(&header).unwrap();
    let encrypted = backups::manifest::encrypt(&serde_json::to_vec(&data).unwrap(), &KEY, &header, &raw_header).unwrap();
    let forged = f.out().join("substituted-current.fbbackup");
    let mut builder = tar::Builder::new(fs::File::create(&forged).unwrap());
    for (name, original) in members {
        let bytes = if name == "header.json" { &raw_header } else if name == "manifest.enc" { &encrypted } else { &original };
        let mut h = tar::Header::new_ustar();
        h.set_size(bytes.len() as u64); h.set_mode(0o600); h.set_cksum();
        builder.append_data(&mut h, &name, bytes.as_slice()).unwrap();
    }
    builder.into_inner().unwrap().sync_all().unwrap();
    let refused = f.verify(&forged).unwrap_err().to_string();
    assert!(refused.contains("does NOT match the backup manifest"), "{refused}");
}


/// Normal current re-wrap history must not increase the work of finding historical object-form wraps.
/// Count SQLite VM steps, so this asserts scaling without any machine-dependent timing threshold.
#[test]
fn modern_audit_history_does_not_increase_legacy_lookup_work() {
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
    let mut f = F::new();
    let row = f.add(&RECEIPTS, &f.owner.clone(), "new.pdf", PDF);
    let id = row["id"].as_str().unwrap().to_owned();
    let stored = f.stored_name(&id);
    let measure = || {
        // Hooks need a mutable connection reference. Use a separate query-only test reader;
        // the production Book boundary remains unchanged.
        let mut conn = fb_db::rusqlite::Connection::open(&f.path).unwrap();
        conn.execute_batch(&format!("PRAGMA key=\"x'{}'\"; PRAGMA query_only=ON;", "2a".repeat(32))).unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let counter = count.clone();
        conn.progress_handler(1, Some(move || { counter.fetch_add(1, Ordering::Relaxed); false })).unwrap();
        assert!(a::recorded_wraps(&conn, f.co, &stored).unwrap().is_empty());
        conn.progress_handler(0, None::<fn() -> bool>).unwrap();
        count.load(Ordering::Relaxed)
    };
    let before = measure();
    assert!(before > 0, "the VM hook must actually count work");
    let actor = f.fx.user;
    let co = f.co;
    f.fx.book.write(|tx| {
        let user = fb_core::services::access(tx, actor, co)?;
        for _ in 0..2000 {
            fb_core::services::audit(tx, &user, co, a::REWRAPPED, "Attachment keys".into(), serde_json::json!({"attachments":[id],"files":[stored]}))?;
        }
        Ok::<_, fb_core::FbError>(())
    }).unwrap();
    let after = measure();
    assert!(after <= before + 100, "lookup work grew with modern events: {before} -> {after}");
}
