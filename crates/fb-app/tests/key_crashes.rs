#![cfg(all(unix, feature = "key-test-hooks"))]
mod support;
use fb_app::{
    App,
    keys::MasterKey,
    protection::{self, Record},
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
const PASSWORD: &str = "Synthetic crash password";
fn app(root: &Path) -> App {
    let mut a = App::new();
    a.backups.configure(root).unwrap();
    a.device_key = Some(MasterKey::new([97; 32]));
    a
}
#[test]
fn crash_child() {
    let Ok(root) = std::env::var("FB_CRASH_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let path = root.join("books/default.fbbook");
    let own = std::env::var("FB_CRASH_OWN").unwrap() == "1";
    let mut a = app(&root);
    if std::env::var("FB_CRASH_OPERATION").unwrap() == "recover" {
        protection::recover_key_change(&path).unwrap();
    } else if std::env::var("FB_CRASH_OPERATION").unwrap() == "create" {
        a.create_protected_book(
            &path,
            "Crash owner",
            "Crash-safe synthetic books",
            own.then_some(PASSWORD),
        )
        .unwrap();
        support::sign_in(&mut a);
    } else {
        let record = Record::read(&path).unwrap().unwrap();
        a.open_book(&path, &record.keys.unlock(PASSWORD).unwrap())
            .unwrap();
        support::sign_in(&mut a);
        a.os_store_usable = true;
        match std::env::var("FB_CRASH_OPERATION").unwrap().as_str() {
            "prompt" => a.change_ask_password(PASSWORD, true).unwrap(),
            "saved" => { a.authorize_recovery(PASSWORD).unwrap(); a.recovery_saved().unwrap(); },
            "rekey-refused" => {
                let refused = a.change_protection(PASSWORD, (!own).then_some(PASSWORD)).unwrap_err().to_string();
                assert!(refused.contains("Password protection was not changed") && refused.contains("“before-the-fix.pdf”"), "{refused}");
                assert!(a.is_open(), "a refusal before the change leaves the books open");
            }
            _ => a.change_protection(PASSWORD, (!own).then_some(PASSWORD)).unwrap(),
        }
    }
}

#[test]
fn rollback_itself_can_be_interrupted_and_retried_without_an_actor_event() {
    for point in ["rollback-book-published", "rollback-record-published"] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("books/default.fbbook");
        let mut a = app(root.path());
        a.create_protected_book(&path, "Rollback owner", "Synthetic rollback books", None).unwrap();
        support::sign_in(&mut a);
        let old = Record::read(&path).unwrap().unwrap();
        a.lock();
        kill_at(root.path(), false, "rekey", "protection-before-audit");
        fs::remove_file(root.path().join("stopped-at")).unwrap();
        kill_at(root.path(), false, "recover", point);
        protection::recover_key_change(&path).unwrap();
        assert!(!protection::recover_key_change(&path).unwrap());
        assert_eq!(serde_json::to_value(Record::read(&path).unwrap().unwrap()).unwrap(), serde_json::to_value(&old).unwrap());
        a.open_book(&path, &old.keys.unlock(PASSWORD).unwrap()).unwrap();
        support::sign_in(&mut a);
        assert_eq!(a.company_name_for_kit().unwrap(), "Synthetic rollback books");
        let count: i64 = a.open_mut().unwrap().book.read(|q| q.query_row(
            "SELECT count(*) FROM ledger_auditevent WHERE action='book.protection_changed'", [], |r| r.get(0))).unwrap();
        assert_eq!(count, 0);
    }
}
fn kill_at(root: &Path, own: bool, operation: &str, point: &str) {
    let marker = root.join("stopped-at");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_child", "--nocapture"])
        .env("FB_CRASH_ROOT", root)
        .env("FB_CRASH_OWN", if own { "1" } else { "0" })
        .env("FB_CRASH_OPERATION", operation)
        .env("FB_KEY_STOP_AT", point)
        .env("FB_KEY_STOP_MARKER", &marker)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while !marker.exists() && start.elapsed() < Duration::from_secs(240) {
        assert!(
            child.try_wait().unwrap().is_none(),
            "child exited before fault point {point}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let reached = marker.exists();
    let during = point != "rekey-in-progress"
        || fs::read_to_string(&marker).unwrap_or_default() == "started";
    child.kill().unwrap();
    let result = child.wait().unwrap();
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(result.signal(), Some(9));
    assert!(reached, "fault point not reached: {point}");
    assert!(
        during,
        "rekey completed before the SIGKILL; repeat with a larger synthetic book"
    );
}
#[test]
fn first_run_sigkill_never_leaves_an_unopenable_book() {
    for own in [false, true] {
        for point in [
            "create-keys-saved",
            "create-book-written",
            "create-book-published",
        ] {
            let root = tempfile::tempdir().unwrap();
            kill_at(root.path(), own, "create", point);
            let path = root.path().join("books/default.fbbook");
            let mut a = app(root.path());
            if path.exists() {
                // Match native startup: remove the known first-run stage under maintenance
                // before admitting a book whose publication briefly used two hard links.
                protection::recover_key_change(&path).unwrap();
                let record = Record::read(&path).unwrap().unwrap();
                a.open_book(&path, &record.keys.unlock(PASSWORD).unwrap())
                    .unwrap();
                support::sign_in(&mut a);
            } else {
                a.create_protected_book(
                    &path,
                    "Crash owner",
                    "Crash-safe synthetic books",
                    own.then_some(PASSWORD),
                )
                .unwrap();
                support::sign_in(&mut a);
            }
            assert_eq!(
                a.company_name_for_kit().unwrap(),
                "Crash-safe synthetic books"
            );
            // The next start clears temporary key files and a published book's stage.
            protection::clean_leftovers(&path).unwrap();
            let left: Vec<String> = fs::read_dir(path.parent().unwrap())
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|n| n.contains(".tmp-") || n.ends_with(".creating"))
                .collect();
            assert!(left.is_empty(), "{point}: {left:?}");
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
#[test]
fn rekey_sigkill_at_each_commit_boundary_keeps_old_book_openable() {
    for own in [false, true] {
        for point in [
            "rekey-journal-saved",
            "rekey-pages-written",
            "rekey-record-saved",
        ] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("books/default.fbbook");
            let mut a = app(root.path());
            a.create_protected_book(
                &path,
                "Crash owner",
                "Crash-safe synthetic books",
                own.then_some(PASSWORD),
            )
            .unwrap();
            support::sign_in(&mut a);
            let old = Record::read(&path).unwrap().unwrap();
            a.lock();
            kill_at(root.path(), own, "rekey", point);
            assert!(protection::recover_rekey(&path).unwrap());
            assert!(
                !protection::recover_rekey(&path).unwrap(),
                "recovery is idempotent"
            );
            let after = Record::read(&path).unwrap().unwrap();
            assert_eq!(after.keys.mode, old.keys.mode);
            a.open_book(&path, &old.keys.unlock(PASSWORD).unwrap())
                .unwrap();
            support::sign_in(&mut a);
            assert_eq!(
                a.company_name_for_kit().unwrap(),
                "Crash-safe synthetic books"
            );
        }
    }
}
#[test]
fn sigkill_while_sqlcipher_is_rekeying_rolls_back_both_directions() {
    for own in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("books/default.fbbook");
        let mut a = app(root.path());
        a.create_protected_book(
            &path,
            "Crash owner",
            "Crash-safe synthetic books",
            own.then_some(PASSWORD),
        )
        .unwrap();
        support::sign_in(&mut a);
        // Valid audit payload makes the page walk long enough to kill inside PRAGMA rekey.
        let company = a.first_company().unwrap().unwrap();
        let open = a.open_mut().unwrap();
        open.book.write(|tx| {
            let user = fb_core::services::access(tx, open.owner, company)?;
            fb_core::services::audit(tx, &user, company, "book.synthetic_crash_fixture", "Synthetic crash fixture".into(), serde_json::json!({"padding":"x".repeat(32*1024*1024)}))
        }).unwrap();
        let old = Record::read(&path).unwrap().unwrap();
        a.lock();
        kill_at(root.path(), own, "rekey", "rekey-in-progress");
        assert!(protection::recover_rekey(&path).unwrap());
        a.open_book(&path, &old.keys.unlock(PASSWORD).unwrap())
            .unwrap();
        support::sign_in(&mut a);
        assert_eq!(
            a.company_name_for_kit().unwrap(),
            "Crash-safe synthetic books"
        );
        let size:i64=a.open_mut().unwrap().book.read(|q|q.query_row("SELECT length(detail) FROM ledger_auditevent WHERE action='book.synthetic_crash_fixture'",[],|r|r.get(0))).unwrap();
        assert!(size > 32 * 1024 * 1024);
    }
}

// W0-OPEN: exercise the existing pre-upgrade fault points in isolated child processes.
#[test]
fn upgrade_child() {
    let Ok(root) = std::env::var("FB_UPGRADE_ROOT") else { return; };
    let root = PathBuf::from(root);
    let path = root.join("books/default.fbbook");
    let mut a = app(&root);
    let result = a.open_with_secret(&path, PASSWORD);
    if std::env::var("FB_KEY_FAIL_AT").ok().as_deref() == Some("upgrade-verify") {
        assert!(matches!(result, Err(fb_app::AppError::Open(fb_app::opening::OpenError::UpgradeCopy { .. }))));
        assert!(!a.is_open());
        assert!(a.upgrade_notice.is_none());
    } else {
        result.unwrap();
    }
}

fn prior_upgrade(root: &Path) -> (PathBuf, MasterKey) {
    let path = root.join("books/default.fbbook");
    let mut a = app(root);
    a.create_protected_book(&path, "Upgrade owner", "Synthetic upgrade books", Some(PASSWORD)).unwrap();
    let key = MasterKey::new(**a.master_key().unwrap());
    let prior = root.join("prior.fbbook");
    fb_db::test_support::populated_copy(a.open.as_ref().unwrap().book.connection(), &prior, &key, fb_db::MIGRATIONS.last().unwrap().0, false).unwrap();
    a.lock();
    fs::rename(prior, &path).unwrap();
    // Make a persistent journal-mode change visible if opening happens before verification.
    {
        let c = fb_db::rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(&format!("PRAGMA key=\"x'{}'\"; PRAGMA journal_mode=DELETE", fb_app::keys::to_hex(key.as_ref()))).unwrap();
    }
    (path, key)
}

#[test]
fn upgrade_verification_failure_leaves_the_source_byte_identical_and_unmigrated() {
    let root = tempfile::tempdir().unwrap();
    let (path, key) = prior_upgrade(root.path());
    let before = fs::read(&path).unwrap();
    let result = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "upgrade_child", "--nocapture"])
        .env("FB_UPGRADE_ROOT", root.path()).env("FB_KEY_FAIL_AT", "upgrade-verify")
        .status().unwrap();
    assert!(result.success());
    assert!(fs::read(&path).unwrap() == before, "the source book must remain byte-identical before migration");
    let fb_db::Opened::Behind(old) = fb_db::Book::open_staged(&path, &key).unwrap() else { panic!("source was migrated"); };
    assert_eq!(old.from(), fb_db::MIGRATIONS.len() - 1);
    assert!(fb_core::backups::retention::backup_files(&root.path().join("backups"), None).unwrap().is_empty());
}

#[test]
fn upgrade_sigkill_after_verified_copy_precedes_the_first_schema_change() {
    let root = tempfile::tempdir().unwrap();
    let (path, key) = prior_upgrade(root.path());
    let before = fs::read(&path).unwrap();
    let marker = root.path().join("upgrade-stopped-at");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "upgrade_child", "--nocapture"])
        .env("FB_UPGRADE_ROOT", root.path()).env("FB_KEY_STOP_AT", "upgrade-copy-verified")
        .env("FB_KEY_STOP_MARKER", &marker).spawn().unwrap();
    let start = Instant::now();
    while !marker.exists() && start.elapsed() < Duration::from_secs(240) {
        assert!(child.try_wait().unwrap().is_none(), "child exited before verified-copy boundary");
        std::thread::sleep(Duration::from_millis(5));
    }
    let reached = marker.exists();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(reached, "verified-copy boundary not reached");
    assert!(fs::read(&path).unwrap() == before, "the source book must remain byte-identical before migration");
    let copies = fb_core::backups::retention::backup_files(&root.path().join("backups"), None).unwrap();
    assert_eq!(copies.len(), 1);
    assert!(fb_core::backups::retention::verified_note(&copies[0]));
    let receipts = Record::read(&path).unwrap().unwrap().keys.receipts(&key).unwrap();
    let verifier = fb_core::backups::attachment_receipt_verifier(&receipts);
    let work = root.path().join("verify-work");
    fs::create_dir(&work).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&work, fs::Permissions::from_mode(0o700)).unwrap();
    let manifest = fb_core::backups::verify_backup_with_receipts(&copies[0], &key, &work, None, false, Some(&verifier)).unwrap();
    assert_eq!(manifest.migrations.len(), fb_db::MIGRATIONS.len() - 1);
    let fb_db::Opened::Behind(old) = fb_db::Book::open_staged(&path, &key).unwrap() else { panic!("source was migrated"); };
    assert_eq!(old.from(), manifest.migrations.len());
    drop(old);
    let mut a = app(root.path());
    a.open_with_secret(&path, PASSWORD).unwrap();
    support::sign_in(&mut a);
    assert_eq!(a.company_name_for_kit().unwrap(), "Synthetic upgrade books");
}

#[test]
fn protection_publication_and_actor_audit_have_one_crash_boundary() {
    for operation in ["prompt", "saved", "rekey"] {
        for committed in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("books/default.fbbook");
            let own = operation != "rekey";
            let mut a = app(root.path());
            a.create_protected_book(&path,"Crash owner","Crash-safe synthetic books",own.then_some(PASSWORD)).unwrap();
            support::sign_in(&mut a);
            let before = Record::read(&path).unwrap().unwrap();
            a.lock();
            kill_at(root.path(), own, operation, if committed { "protection-committed" } else { "protection-before-audit" });
            assert!(protection::recover_key_change(&path).unwrap());
            let after = Record::read(&path).unwrap().unwrap();
            let action = match operation { "prompt" => "book.password_prompt", "saved" => "book.recovery_saved", _ => "book.protection_changed" };
            if !committed { assert_eq!(serde_json::to_value(&after).unwrap(),serde_json::to_value(&before).unwrap()); }
            a.open_book(&path,&after.keys.unlock(PASSWORD).unwrap()).unwrap();
            support::sign_in(&mut a);
            let count: i64 = a.open_mut().unwrap().book.read(|q| q.query_row("SELECT count(*) FROM ledger_auditevent WHERE action=?1",[action],|r| r.get(0))).unwrap();
            assert_eq!(count,i64::from(committed),"{operation}");
            if committed {
                match operation {
                    "prompt" => assert!(after.ask_password),
                    "saved" => assert!(after.recovery_saved),
                    _ => assert_ne!(after.keys.mode,before.keys.mode),
                }
            }
        }
    }
}

// Review finding 8: a SIGKILL while the pre-change snapshot is being copied leaves the snapshot's own
// rollback journal (`<book>.rekey-before-journal`). Recovery must remove it with the snapshot, or every
// later protection change is refused for "existing SQLite sidecars".
#[test]
fn sigkill_while_copying_the_pre_change_snapshot_leaves_nothing_that_blocks_the_next_change() {
    for own in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("books/default.fbbook");
        let mut a = app(root.path());
        a.create_protected_book(&path, "Crash owner", "Crash-safe synthetic books", own.then_some(PASSWORD)).unwrap();
        support::sign_in(&mut a);
        let old = Record::read(&path).unwrap().unwrap();
        a.lock();
        kill_at(root.path(), own, "rekey", "snapshot-in-progress");
        let snapshot = protection::sidecar(&path, ".rekey-before");
        assert!(protection::sidecar(&snapshot, "-journal").exists(), "the copy was interrupted with its journal open");
        assert!(!protection::sidecar(&path, ".rekey.json").exists(), "no change had begun");
        assert!(protection::recover_key_change(&path).unwrap());
        for leftover in [snapshot.clone(), protection::sidecar(&snapshot, "-journal"), protection::sidecar(&snapshot, "-wal"), protection::sidecar(&snapshot, "-shm")] {
            assert!(!leftover.exists(), "{} was left behind", leftover.display());
        }
        assert!(!protection::recover_key_change(&path).unwrap(), "recovery is idempotent");
        assert_eq!(serde_json::to_value(Record::read(&path).unwrap().unwrap()).unwrap(), serde_json::to_value(&old).unwrap());
        // The next protection change goes through.
        a.open_book(&path, &old.keys.unlock(PASSWORD).unwrap()).unwrap();
        support::sign_in(&mut a);
        a.os_store_usable = true;
        a.change_protection(PASSWORD, (!own).then_some(PASSWORD)).unwrap();
        assert_eq!(a.company_name_for_kit().unwrap(), "Crash-safe synthetic books");
    }
}

// A journal left beside an already-removed snapshot (an error path that removed the file) is cleared too.
#[test]
fn a_stray_snapshot_journal_without_its_snapshot_is_cleared_at_recovery() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("books/default.fbbook");
    let mut a = app(root.path());
    a.create_protected_book(&path, "Crash owner", "Crash-safe synthetic books", Some(PASSWORD)).unwrap();
    a.lock();
    let journal = protection::sidecar(&protection::sidecar(&path, ".rekey-before"), "-journal");
    fs::write(&journal, b"stray").unwrap();
    assert!(protection::recover_key_change(&path).unwrap());
    assert!(!journal.exists());
}

// ------------------------------------------------------------------------------------------------
// Review finding 5: re-wrap before rekey is a property of the protection change itself.

const PDF: &[u8] = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n1 0 obj<</Type/Catalog>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n";

/// A first-run book holding one file attached the way the previous build's chooser did (its key wrapped
/// under the master key), closed. Returns the book, its company and the attachment.
fn book_with_an_older_attachment(root: &Path, own: bool) -> (PathBuf, fb_core::CompanyId, String) {
    use fb_core::{attachments as at, services};
    let path = root.join("books/default.fbbook");
    let mut a = app(root);
    a.create_protected_book(&path, "Crash owner", "Crash-safe synthetic books", own.then_some(PASSWORD)).unwrap();
    support::sign_in(&mut a);
    let co = a.first_company().unwrap().unwrap();
    let master = MasterKey::new(**a.master_key().unwrap());
    let o = a.open_mut().unwrap();
    let cash = services::save_account(&mut o.book, o.owner, co, services::AccountInput::new("1000", "Synthetic cash").kind("ASSET")).unwrap().id;
    let equity = services::save_account(&mut o.book, o.owner, co, services::AccountInput::new("3000", "Synthetic equity").kind("EQUITY")).unwrap().id;
    let lines = vec![services::line(cash).debit("10").build(), services::line(equity).credit("10").build()];
    let j = services::save_draft(&mut o.book, o.owner, co, services::DraftInput::new("2026-10-01", "Synthetic opening", lines)).unwrap();
    let store = at::storage::Store::for_book(&path, &master).unwrap();
    let id = at::add(&mut o.book, &store, o.owner, co, &at::Owner::Journal(j.id), PDF, "before-the-fix.pdf", &uuid::Uuid::new_v4().to_string(), &at::Limits::default()).unwrap().0["id"].as_str().unwrap().to_owned();
    a.lock();
    (path, co, id)
}

/// Open with whatever secret the record now takes (the password, or none after a switch to the built-in
/// key) and read the attachment with the receipt key alone.
fn read_with_receipt_key(root: &Path, path: &Path, co: fb_core::CompanyId, id: &str) -> Vec<u8> {
    let record = Record::read(path).unwrap().unwrap();
    let key = record.keys.unlock(PASSWORD).or_else(|_| record.keys.unlock("")).unwrap();
    let mut a = app(root);
    a.open_book(path, &key).unwrap();
    support::sign_in(&mut a);
    let store = fb_core::attachments::storage::Store::for_book(path, &a.receipt_key().unwrap()).unwrap();
    let o = a.open_mut().unwrap();
    fb_core::attachments::download(&mut o.book, &store, o.owner, co, id).unwrap()
}

fn run_child(root: &Path, own: bool, operation: &str, faults: &str) {
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_child", "--nocapture"])
        .env("FB_CRASH_ROOT", root)
        .env("FB_CRASH_OWN", if own { "1" } else { "0" })
        .env("FB_CRASH_OPERATION", operation)
        .env("FB_KEY_FAIL_AT", faults)
        .status()
        .unwrap();
    assert!(status.success(), "{operation} with {faults}");
}

/// A safety backup that skipped its own re-wrap (or a future path that reuses or skips the backup) does
/// not matter: the protection change moves the keys itself just before the master key changes.
#[test]
fn a_password_change_moves_attachment_keys_itself_even_when_its_backup_did_not() {
    for own in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (path, co, id) = book_with_an_older_attachment(root.path(), own);
        let before = Record::read(&path).unwrap().unwrap();
        run_child(root.path(), own, "rekey", "backup-skip-rewrap");
        let after = Record::read(&path).unwrap().unwrap();
        assert!(after.keys.mode != before.keys.mode, "the protection changed");
        assert_eq!(read_with_receipt_key(root.path(), &path, co, &id), PDF);
    }
}

/// The check at the point of change: with both re-wraps skipped, a key that only the outgoing master key
/// opens stops the change before anything is changed, and the books keep working with the old password.
#[test]
fn the_change_stops_while_a_key_still_opens_only_under_the_outgoing_master_key() {
    for own in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (path, co, id) = book_with_an_older_attachment(root.path(), own);
        let before = serde_json::to_value(Record::read(&path).unwrap().unwrap()).unwrap();
        run_child(root.path(), own, "rekey-refused", "backup-skip-rewrap,rekey-skip-rewrap");
        assert_eq!(serde_json::to_value(Record::read(&path).unwrap().unwrap()).unwrap(), before, "the key record is unchanged");
        assert!(!protection::sidecar(&path, ".rekey.json").exists() && !protection::sidecar(&path, ".rekey-before").exists(), "no change began");
        let record = Record::read(&path).unwrap().unwrap();
        let mut a = app(root.path());
        a.open_book(&path, &record.keys.unlock(PASSWORD).unwrap()).unwrap();
        support::sign_in(&mut a);
        let changed: i64 = a.open_mut().unwrap().book.read(|q| q.query_row("SELECT count(*) FROM ledger_auditevent WHERE action='book.protection_changed'", [], |r| r.get(0))).unwrap();
        assert_eq!(changed, 0);
        let store = fb_core::attachments::storage::Store::for_book(&path, &a.receipt_key().unwrap()).unwrap().with_legacy(a.master_key().unwrap()).unwrap();
        let o = a.open_mut().unwrap();
        assert_eq!(fb_core::attachments::download(&mut o.book, &store, o.owner, co, &id).unwrap(), PDF);
    }
}

/// Rekey crash recovery: a SIGKILL after the pages are rewritten rolls back to the old key and record,
/// and an attachment added before the change still reads.
#[test]
fn a_rekey_killed_midway_with_an_older_attachment_rolls_back_and_the_file_reads() {
    for own in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (path, co, id) = book_with_an_older_attachment(root.path(), own);
        kill_at(root.path(), own, "rekey", "rekey-pages-written");
        assert!(protection::recover_rekey(&path).unwrap());
        let record = Record::read(&path).unwrap().unwrap();
        let mut a = app(root.path());
        a.open_book(&path, &record.keys.unlock(PASSWORD).unwrap()).unwrap();
        support::sign_in(&mut a);
        let store = fb_core::attachments::storage::Store::for_book(&path, &a.receipt_key().unwrap()).unwrap().with_legacy(a.master_key().unwrap()).unwrap();
        let o = a.open_mut().unwrap();
        assert_eq!(fb_core::attachments::download(&mut o.book, &store, o.owner, co, &id).unwrap(), PDF);
        drop(a);
        // The next change goes through and the file still reads after it, with the receipt key alone.
        let mut a = app(root.path());
        a.open_book(&path, &record.keys.unlock(PASSWORD).unwrap()).unwrap();
        support::sign_in(&mut a);
        a.os_store_usable = true;
        a.change_protection(PASSWORD, (!own).then_some(PASSWORD)).unwrap();
        a.lock();
        assert_eq!(read_with_receipt_key(root.path(), &path, co, &id), PDF);
    }
}
