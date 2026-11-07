//! Wave 0 items 4 and 5: plain "can't open" messages, and a verified copy before any upgrade.
use fb_app::{
    App, AppError, Command, Outcome,
    keys::MasterKey,
    opening::OpenError,
    protection::Record,
    screens,
};
use fb_core::backups::{self, retention};
use std::{
    fs,
    path::{Path, PathBuf},
};

const PASSWORD: &str = "Synthetic owner password";
const WRONG: &str = "That password or key doesn't open these books. Check it and try again, or use your recovery key.";
const DAMAGED: &str = "This file is damaged or isn't a FreedomBooks book. Nothing was changed.";

fn app_at(root: &Path) -> App {
    let mut app = App::new();
    app.backups.configure(root).unwrap();
    app.device_key = Some(MasterKey::new([71; 32]));
    app
}

/// An own-password (or built-in-key) book in `root/books`, locked again.
fn setup(root: &Path, own: bool) -> (App, PathBuf, MasterKey) {
    let mut app = app_at(root);
    let path = root.join("books/default.fbbook");
    app.create_protected_book(&path, "Synthetic owner", "Synthetic open company", own.then_some(PASSWORD)).unwrap();
    let key = MasterKey::new(**app.master_key().unwrap());
    app.lock();
    (app, path, key)
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Replace the live book with the same data one migration behind this app.
fn make_prior(app: &mut App, path: &Path, key: &MasterKey) {
    let prior = path.with_extension("prior");
    {
        let source = fb_db::Book::open(path, key).unwrap();
        let last = fb_db::MIGRATIONS.last().unwrap().0;
        fb_db::test_support::populated_copy(source.connection(), &prior, key, last, false).unwrap();
    }
    app.lock();
    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(sidecar(path, suffix));
    }
    fs::rename(&prior, path).unwrap();
}

fn user_version(path: &Path, key: &MasterKey) -> i64 {
    let c = fb_db::rusqlite::Connection::open(path).unwrap();
    c.execute_batch(&format!("PRAGMA key=\"x'{}'\"", fb_app::keys::to_hex(key.as_ref()))).unwrap();
    c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap()
}

fn files(folder: &Path) -> Vec<PathBuf> {
    retention::backup_files(folder, None).unwrap()
}

fn message(e: AppError) -> String {
    e.to_string()
}

#[test]
fn wrong_password_and_wrong_recovery_key_get_the_same_words_and_change_nothing() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, path, _) = setup(d.path(), true);
    let other = tempfile::tempdir().unwrap();
    let (_, other_path, other_key) = setup(other.path(), true);
    let other_recovery = Record::read(&other_path).unwrap().unwrap().keys.recovery_text(&other_key).unwrap();
    let before = fs::read(&path).unwrap();
    for secret in ["wrong password", other_recovery.as_str(), ""] {
        let e = app.open_with_secret(&path, secret).unwrap_err();
        assert!(matches!(e, AppError::Open(OpenError::WrongKey)), "{secret:?}: {e:?}");
        assert_eq!(message(e), WRONG);
        assert!(!app.is_open());
    }
    assert_eq!(fs::read(&path).unwrap(), before);
    // A saved key that does not match the key record (an old OS-store seal) is a wrong key too.
    let e = app.open_book(&path, &MasterKey::new([3; 32])).unwrap_err();
    assert_eq!(message(e), WRONG);
    // The recovery key opens it.
    app.open_with_secret(&path, PASSWORD).unwrap();
    let key = MasterKey::new(**app.master_key().unwrap());
    let recovery = Record::read(&path).unwrap().unwrap().keys.recovery_text(&key).unwrap();
    app.lock();
    app.open_with_secret(&path, &recovery).unwrap();
    assert!(app.is_open());
    assert!(files(&app.backups.folder().unwrap()).is_empty(), "no copy when no upgrade is needed");
}

#[test]
fn a_book_without_a_key_record_and_the_wrong_key_says_wrong_key() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("legacy.fbbook");
    let mut app = app_at(d.path());
    app.create_book(&path, &MasterKey::new([9; 32]), "Legacy owner", "Legacy company").unwrap();
    app.lock();
    let e = app.open_book(&path, &MasterKey::new([8; 32])).unwrap_err();
    assert!(matches!(e, AppError::Open(OpenError::WrongKey)), "{e:?}");
}

#[test]
fn a_newer_book_says_so_with_both_versions_and_is_untouched() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, path, key) = setup(d.path(), true);
    {
        let c = fb_db::rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(&format!("PRAGMA key=\"x'{}'\"; PRAGMA user_version={}", fb_app::keys::to_hex(key.as_ref()), fb_db::MIGRATIONS.len() + 2)).unwrap();
    }
    let before = fs::read(&path).unwrap();
    let e = app.open_with_secret(&path, PASSWORD).unwrap_err();
    let known = fb_db::MIGRATIONS.len();
    assert_eq!(message(e), format!(
        "These books were saved by a newer version of FreedomBooks (book version {}; this app reads up to {known}). Install the latest version to open them. Nothing was changed.",
        known + 2
    ));
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(files(&app.backups.folder().unwrap()).is_empty());
}

#[test]
fn damaged_files_are_damaged_whatever_secret_is_typed() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, path, _) = setup(d.path(), true);
    let good = fs::read(&path).unwrap();
    let mut first_page = good.clone();
    first_page[200] ^= 0xff;
    let mut later_page = good.clone();
    let at = later_page.len() - 3000;
    later_page[at] ^= 0xff;
    let cases: Vec<(&str, Vec<u8>, bool)> = vec![
        // Key-free checks: the same words for every secret.
        ("cut short", good[..good.len() - 700].to_vec(), true),
        ("empty", vec![], true),
        ("not a book", b"Name,Amount\nSynthetic,1.00\n".to_vec(), true),
        // After the key record proves the password: page 1 or a later page will not decrypt.
        ("first page", first_page, false),
        ("later page", later_page, false),
        ("cut at a page", good[..8192].to_vec(), false),
    ];
    for (what, bytes, any_secret) in cases {
        fs::write(&path, &bytes).unwrap();
        let secrets: &[&str] = if any_secret { &[PASSWORD, "wrong password", ""] } else { &[PASSWORD] };
        for secret in secrets {
            let e = app.open_with_secret(&path, secret).unwrap_err();
            assert!(matches!(e, AppError::Open(OpenError::Damaged)), "{what} {secret:?}: {e:?}");
            assert!(message(e).starts_with(DAMAGED), "{what}");
            assert_eq!(fs::read(&path).unwrap(), bytes, "{what}: nothing was changed");
        }
        if !any_secret {
            // A wrong secret never reaches the file: it is a wrong key, not damage.
            assert_eq!(message(app.open_with_secret(&path, "wrong password").unwrap_err()), WRONG, "{what}");
        }
    }
    // The unlock screen shows the words and keeps the restore route.
    let html = screens::render_gate(&app, "unlock", true, "", &[message(AppError::Open(OpenError::Damaged))]).unwrap();
    assert!(html.contains("This file is damaged or isn&#x27;t a FreedomBooks book.") || html.contains("This file is damaged or isn't a FreedomBooks book."), "{html}");
    assert!(html.contains("<summary>Restore a backup</summary>"));
}

#[test]
fn built_in_key_books_name_damage_too() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, path, _) = setup(d.path(), false);
    let good = fs::read(&path).unwrap();
    fs::write(&path, &good[..good.len() - 10]).unwrap();
    let e = app.open_with_secret(&path, "").unwrap_err();
    assert!(matches!(e, AppError::Open(OpenError::Damaged)), "{e:?}");
}

fn company(app: &mut App) -> String {
    app.first_company().unwrap().unwrap().to_db()
}

fn render(app: &mut App, path: &str) -> String {
    screens::render(app, path).unwrap().html
}

#[test]
fn an_upgrade_writes_a_verified_copy_first_and_says_where_once() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, path, key) = setup(d.path(), true);
    make_prior(&mut app, &path, &key);
    let from = fb_db::MIGRATIONS.len() - 1;
    let to = fb_db::MIGRATIONS.len();
    app.open_with_secret(&path, PASSWORD).unwrap();
    let folder = app.backups.folder().unwrap();
    let copies = files(&folder);
    assert_eq!(copies.len(), 1);
    let copy = copies[0].clone();
    let name = copy.file_name().unwrap().to_str().unwrap().to_owned();
    assert!(name.starts_with(&format!("freedombooks-default-before-upgrade-{from}-to-{to}-")), "{name}");
    assert!(retention::verified_note(&copy));
    // The copy is the book before the upgrade, in the normal portable format.
    let header = backups::read_header(&copy).unwrap();
    assert_eq!(header["format_version"], 4);
    assert_eq!(header["book_keys"]["mode"], "own-password");
    let work = d.path().join("verify-work");
    fs::create_dir_all(&work).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&work, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    let receipts = Record::read(&path).unwrap().unwrap().keys.receipts(&key).unwrap();
    let verifier = backups::attachment_receipt_verifier(&receipts);
    let manifest = backups::verify_backup_with_receipts(&copy, &key, &work, None, false, Some(&verifier)).unwrap();
    assert_eq!(manifest.migrations.len(), from);
    assert_eq!(user_version(&path, &key), to as i64);
    // Shown once, in the banner of the first page after opening.
    let notice = format!(
        "These books were updated for this version of FreedomBooks (book version {from} to {to}). A checked backup copy from before the update is in your backup folder: {}",
        copy.display()
    );
    assert_eq!(app.upgrade_notice.as_deref(), Some(notice.as_str()));
    let c = company(&mut app);
    let home = render(&mut app, &format!("/c/{c}/home/"));
    let banner = home.split("data-banner=\"notice\">").nth(1).unwrap().split("</div>").next().unwrap();
    assert!(banner.starts_with(&format!("These books were updated for this version of FreedomBooks (book version {from} to {to}). A checked backup copy from before the update is in your backup folder: ")), "{banner}");
    assert!(banner.contains(&name), "{banner}");
    let again = render(&mut app, &format!("/c/{c}/home/"));
    assert!(!again.contains("before the update"), "the notice is shown once");
    // Opening again: nothing to upgrade, so no new copy and no notice.
    app.lock();
    app.open_with_secret(&path, PASSWORD).unwrap();
    assert_eq!(files(&folder), vec![copy.clone()]);
    assert!(app.upgrade_notice.is_none());
    // Retention: it is listed (and verified) beside a normal backup, which removes nothing.
    let create = format!("/c/{c}/backups/create/");
    let token = app.tokens.mint(&create);
    let Outcome::Redirect { .. } = app.run_renderer(Command::SubmitForm { path: create, token, fields: vec![] }).unwrap() else { panic!() };
    let listed = files(&folder);
    assert_eq!(listed.len(), 2);
    assert!(listed.contains(&copy));
    let page = render(&mut app, &format!("/c/{c}/backups/"));
    assert!(page.contains(&name), "{page}");
    // It restores on another computer with the password, like any portable backup.
    let elsewhere = tempfile::tempdir().unwrap();
    let mut other = app_at(elsewhere.path());
    other.device_key = Some(MasterKey::new([72; 32]));
    screens::backups::restore_from_gate(&mut other, &copy, "Before the upgrade", PASSWORD).unwrap();
    assert_eq!(other.company_name_for_kit().unwrap(), "Synthetic open company");
}

#[test]
fn built_in_key_books_get_the_copy_too() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, path, key) = setup(d.path(), false);
    make_prior(&mut app, &path, &key);
    app.open_with_secret(&path, "").unwrap();
    let copies = files(&app.backups.folder().unwrap());
    assert_eq!(copies.len(), 1);
    assert_eq!(backups::read_header(&copies[0]).unwrap()["book_keys"]["mode"], "built-in");
    assert!(retention::verified_note(&copies[0]));
}

#[cfg(unix)]
#[test]
fn a_failed_copy_means_no_upgrade_and_the_book_is_byte_identical() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let (mut app, path, key) = setup(d.path(), true);
    make_prior(&mut app, &path, &key);
    let before = fs::read(&path).unwrap();
    let from = fb_db::MIGRATIONS.len() - 1;
    // A backup folder that cannot be made (its parent is read-only), refused like a full disk.
    let locked = d.path().join("read-only");
    fs::create_dir(&locked).unwrap();
    let folder = locked.join("backups");
    fs::write(d.path().join("backup-folder.json"), serde_json::json!({"folder": folder}).to_string()).unwrap();
    assert_eq!(app.backups.folder().unwrap(), folder);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).unwrap();
    let e = app.open_with_secret(&path, PASSWORD).unwrap_err();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(e, AppError::Open(OpenError::UpgradeCopy { .. })), "{e:?}");
    let text = message(e);
    assert!(text.starts_with(&format!("These books need a one-time update for this version of FreedomBooks (book version {from} to {}).", from + 1)), "{text}");
    assert!(text.contains("Nothing was changed: the books were not updated or opened."), "{text}");
    assert!(text.contains(&format!("Free some disk space in the backup folder ({})", folder.display())), "{text}");
    assert!(!app.is_open());
    assert!(app.upgrade_notice.is_none());
    assert_eq!(fs::read(&path).unwrap(), before, "the book is byte-identical");
    assert_eq!(user_version(&path, &key), from as i64);
    assert!(files(&folder).is_empty());
    // With the folder fixed, the same book copies and upgrades.
    app.open_with_secret(&path, PASSWORD).unwrap();
    assert_eq!(files(&folder).len(), 1);
    assert_eq!(user_version(&path, &key), from as i64 + 1);
}

#[test]
fn without_a_backup_folder_an_older_book_is_not_upgraded() {
    let d = tempfile::tempdir().unwrap();
    let (mut configured, path, key) = setup(d.path(), true);
    make_prior(&mut configured, &path, &key);
    // A staged open must not persist even a journal-mode change before the copy succeeds.
    {
        let c = fb_db::rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(&format!("PRAGMA key=\"x'{}'\"; PRAGMA journal_mode=DELETE", fb_app::keys::to_hex(key.as_ref()))).unwrap();
    }
    let before = fs::read(&path).unwrap();
    let mut app = App::new();
    app.device_key = Some(MasterKey::new([71; 32]));
    let e = app.open_with_secret(&path, PASSWORD).unwrap_err();
    assert!(matches!(e, AppError::Open(OpenError::UpgradeCopy { .. })), "{e:?}");
    assert!(fs::read(&path).unwrap() == before, "a failed pre-upgrade copy must not change the journal mode or any book bytes");
    assert_eq!(user_version(&path, &key), fb_db::MIGRATIONS.len() as i64 - 1);
}

#[test]
fn a_damaged_older_book_is_named_as_damage_before_attempting_a_copy() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, path, key) = setup(d.path(), true);
    make_prior(&mut app, &path, &key);
    {
        let c = fb_db::rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(&format!("PRAGMA key=\"x'{}'\"", fb_app::keys::to_hex(key.as_ref()))).unwrap();
        fb_db::collation::register(&c).unwrap();
        c.execute_batch("PRAGMA foreign_keys=OFF; DROP TABLE ledger_company").unwrap();
    }
    let before = fs::read(&path).unwrap();
    let e = app.open_with_secret(&path, PASSWORD).unwrap_err();
    assert!(matches!(e, AppError::Open(OpenError::Damaged)), "{e:?}");
    assert!(message(e).starts_with(DAMAGED));
    assert!(!app.is_open());
    assert!(fs::read(&path).unwrap() == before, "damaged source must remain byte-identical");
    assert!(files(&app.backups.folder().unwrap()).is_empty());
}


#[test]
fn a_copied_builtin_book_gets_a_portable_copy_before_local_key_adoption() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, path, key) = setup(d.path(), false);
    make_prior(&mut app, &path, &key);
    // Copying a built-in-key book without local metadata is a supported opening path.
    fs::remove_file(sidecar(&path, ".keys.json")).unwrap();
    let before = fs::read(&path).unwrap();
    let mut unconfigured = App::new();
    let e = unconfigured.open_with_secret(&path, "").unwrap_err();
    assert!(matches!(e, AppError::Open(OpenError::UpgradeCopy { .. })), "{e:?}");
    assert!(!unconfigured.is_open());
    assert!(fs::read(&path).unwrap() == before);
    assert!(Record::read(&path).unwrap().is_none(), "a failed copy must not adopt keys early");

    app.open_with_secret(&path, "").unwrap();
    let copies = files(&app.backups.folder().unwrap());
    assert_eq!(copies.len(), 1);
    assert!(retention::verified_note(&copies[0]));
    let elsewhere = tempfile::tempdir().unwrap();
    let mut other = app_at(elsewhere.path());
    other.device_key = Some(MasterKey::new([73; 32]));
    screens::backups::restore_from_gate(&mut other, &copies[0], "Copied built-in books", "").unwrap();
    assert_eq!(other.company_name_for_kit().unwrap(), "Synthetic open company");
    let header = backups::read_header(&copies[0]).unwrap();
    assert_eq!(header["format_version"], 4);
    assert_eq!(header["book_keys"]["mode"], "built-in");
}

#[test]
fn historical_layouts_are_copied_before_desktop_upgrade() {
    for boundary in ["fdd2fa6", "a2be4d8"] {
        let d = tempfile::tempdir().unwrap();
        let (mut app, path, key) = setup(d.path(), false);
        let prior = d.path().join("historical.fbbook");
        {
            let source = fb_db::Book::open(&path, &key).unwrap();
            fb_db::test_support::populated_copy(source.connection(), &prior, &key, boundary, false).unwrap();
        }
        app.lock();
        for suffix in ["-wal", "-shm"] { let _ = fs::remove_file(sidecar(&path, suffix)); }
        fs::rename(prior, &path).unwrap();
        app.open_with_secret(&path, "").unwrap();
        assert_eq!(app.company_name_for_kit().unwrap(), "Synthetic open company");
        let copies = files(&app.backups.folder().unwrap());
        assert_eq!(copies.len(), 1, "{boundary}");
        let elsewhere = tempfile::tempdir().unwrap();
        let mut restored = app_at(elsewhere.path());
        screens::backups::restore_from_gate(&mut restored, &copies[0], "Historical restored", "").unwrap();
        assert_eq!(restored.company_name_for_kit().unwrap(), "Synthetic open company");
        app.lock(); app.open_with_secret(&path, "").unwrap();
        assert_eq!(app.company_name_for_kit().unwrap(), "Synthetic open company");
    }
}
