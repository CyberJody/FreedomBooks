//! Unified sign-in retains encrypted portable books and grants no authority before named auth.
use fb_app::{
    App, Command, Outcome, Query,
    identity::{Role, SIGN_IN},
    keys::MasterKey,
    protection::Record,
    screens,
};
use std::path::{Path, PathBuf};
const OWNER: &str = "Synthetic desktop owner";
const USER_PASSWORD: &str = "Synthetic individual login password";
const BOOK_PASSWORD: &str = "Synthetic portable book password";
fn app_at(root: &Path, keyring: bool) -> App {
    let mut app = App::new();
    app.backups.configure(root).unwrap();
    app.device_key = Some(MasterKey::new([85; 32]));
    app.os_store_usable = keyring;
    app
}
fn create(root: &Path, keyring: bool) -> (App, PathBuf) {
    let mut app = app_at(root, keyring);
    let path = root.join("books/default.fbbook");
    app.create_protected_book(
        &path,
        OWNER,
        "Synthetic desktop company",
        Some(BOOK_PASSWORD),
    )
    .unwrap();
    (app, path)
}
fn backup(app: &mut App) -> PathBuf {
    let company = app.first_company().unwrap().unwrap();
    let path = format!("/c/{}/backups/create/", company.to_db());
    let token = app.tokens.mint(&path);
    let Outcome::Redirect { location } = app
        .run_renderer(Command::SubmitForm {
            path,
            token,
            fields: vec![],
        })
        .unwrap()
    else {
        panic!("backup did not complete")
    };
    let file = app
        .backups
        .folder()
        .unwrap()
        .join(location.split("created=").nth(1).unwrap());
    assert!(fb_core::backups::retention::verified_note(&file));
    file
}
#[test]
fn new_books_use_keyring_opening_without_a_signin_conversion() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, path) = create(dir.path(), true);
    assert!(!Record::read(&path).unwrap().unwrap().ask_password);
    let before = std::fs::read(fb_app::protection::record_path(&path)).unwrap();
    assert!(app.query(Query::Companies).is_err());
    app.enroll_owner(USER_PASSWORD).unwrap();
    let record = Record::read(&path).unwrap().unwrap();
    assert!(
        !record.ask_password,
        "named sign-in should replace the separate book prompt on a keyring-equipped desktop"
    );
    assert!(!app.builtin());
    assert!(record.keys.unlock("").is_err());
    assert!(record.keys.unlock(BOOK_PASSWORD).is_ok());
    let key = MasterKey::new(**app.master_key().unwrap());
    app.lock();
    app.open_book(&path, &key).unwrap();
    assert!(app.query(Query::Companies).is_err());
    let html = screens::render(&mut app, SIGN_IN).unwrap().html;
    assert!(
        html.contains(&format!("value=\"{OWNER}\"")),
        "show the single named user before their password"
    );
    app.sign_in(OWNER, "incorrect password").unwrap_err();
    assert!(app.query(Query::Companies).is_err());
    app.sign_in(OWNER, USER_PASSWORD).unwrap();
    assert!(app.query(Query::Companies).is_ok());
    let count: i64 = app
        .open_mut()
        .unwrap()
        .book
        .read(|q| {
            q.query_row(
                "SELECT count(*) FROM ledger_auditevent WHERE action='book.password_prompt'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(count, 0, "sign-in must not run a protection transaction");
    assert_eq!(std::fs::read(fb_app::protection::record_path(&path)).unwrap(), before);
}
#[test]
fn explicit_extra_book_prompt_survives_signout_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, path) = create(dir.path(), true);
    app.enroll_owner(USER_PASSWORD).unwrap();
    app.change_ask_password(BOOK_PASSWORD, true).unwrap();
    app.sign_out();
    app.sign_in(OWNER, USER_PASSWORD).unwrap();
    assert!(Record::read(&path).unwrap().unwrap().ask_password);
    app.lock();
    app.open_with_secret(&path, BOOK_PASSWORD).unwrap();
    app.sign_in(OWNER, USER_PASSWORD).unwrap();
    assert!(Record::read(&path).unwrap().unwrap().ask_password);
}
#[test]
fn password_rotation_preserves_the_configured_prompt_preference() {
    for ask in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, path) = create(dir.path(), true);
        app.enroll_owner(USER_PASSWORD).unwrap();
        app.change_ask_password(BOOK_PASSWORD, ask).unwrap();
        let rotated = "Synthetic rotated book password";
        app.change_protection(BOOK_PASSWORD, Some(rotated)).unwrap();
        let record = Record::read(&path).unwrap().unwrap();
        assert_eq!(record.ask_password, ask);
        assert!(record.keys.unlock(BOOK_PASSWORD).is_err());
        app.sign_out();
        app.sign_in(OWNER, USER_PASSWORD).unwrap();
        app.lock();
        app.open_with_secret(&path, rotated).unwrap();
        app.sign_in(OWNER, USER_PASSWORD).unwrap();
        let record = Record::read(&path).unwrap().unwrap();
        assert_eq!(record.ask_password, ask);
    }
}
#[test]
fn unavailable_keyring_retains_password_fallback_and_non_admin_cannot_change_policy() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, path) = create(dir.path(), false);
    app.enroll_owner(USER_PASSWORD).unwrap();
    assert!(app.can_lock(), "unavailable keyring still requires book unlock");
    app.change_ask_password(BOOK_PASSWORD, true).unwrap();
    assert!(app.change_ask_password(BOOK_PASSWORD, false).is_err());
    let company = app.first_company().unwrap().unwrap();
    app.save_desktop_user(
        None,
        "Synthetic bookkeeper",
        Role::Bookkeeper,
        true,
        Some(USER_PASSWORD),
        &[company],
    )
    .unwrap();
    app.os_store_usable = true;
    app.sign_in("Synthetic bookkeeper", USER_PASSWORD).unwrap();
    assert!(Record::read(&path).unwrap().unwrap().ask_password);
    assert!(app.change_ask_password(BOOK_PASSWORD, false).is_err());
    app.sign_out();
    app.sign_in(OWNER, USER_PASSWORD).unwrap();
    assert!(Record::read(&path).unwrap().unwrap().ask_password);
    app.change_ask_password(BOOK_PASSWORD, false).unwrap();
    assert!(!app.can_lock());
}
#[test]
fn unified_signin_backup_restores_without_original_keyring_using_password_or_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, path) = create(dir.path(), true);
    app.enroll_owner(USER_PASSWORD).unwrap();
    assert!(!Record::read(&path).unwrap().unwrap().ask_password);
    let record = Record::read(&path).unwrap().unwrap();
    let recovery = record
        .keys
        .recovery_text(app.master_key().unwrap())
        .unwrap();
    let archive = backup(&mut app);
    for secret in [BOOK_PASSWORD, recovery.as_str()] {
        let other_dir = tempfile::tempdir().unwrap();
        let mut other = app_at(other_dir.path(), false);
        other.device_key = Some(MasterKey::new([86; 32]));
        assert!(
            screens::backups::restore_from_gate(
                &mut other,
                &archive,
                "Wrong secret",
                "not the password"
            )
            .is_err()
        );
        let restored =
            screens::backups::restore_from_gate(&mut other, &archive, "Restored books", secret)
                .unwrap();
        assert!(other.query(Query::Companies).is_err());
        assert!(!Record::read(&restored).unwrap().unwrap().ask_password);
        assert!(other.can_lock(), "missing keyring keeps the password/recovery fallback");
        other.sign_in(OWNER, USER_PASSWORD).unwrap();
        assert_eq!(
            other.company_name_for_kit().unwrap(),
            "Synthetic desktop company"
        );
        assert!(!Record::read(&restored).unwrap().unwrap().ask_password);
        assert!(other.can_lock(), "missing keyring keeps the password/recovery fallback");
        other.lock();
        other.open_with_secret(&restored, secret).unwrap();
        assert!(other.query(Query::Companies).is_err());
        other.sign_in(OWNER, USER_PASSWORD).unwrap();
    }
}

fn totp(secret: &str, offset: i64) -> String {
    use ring::hmac;
    use std::time::{SystemTime, UNIX_EPOCH};
    let bytes = data_encoding::BASE32_NOPAD
        .decode(secret.as_bytes())
        .unwrap();
    let step = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        / 30) as i64
        + offset;
    let tag = hmac::sign(
        &hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, &bytes),
        &(step as u64).to_be_bytes(),
    );
    let b = tag.as_ref();
    let i = (b[19] & 15) as usize;
    format!(
        "{:06}",
        (u32::from_be_bytes(b[i..i + 4].try_into().unwrap()) & 0x7fff_ffff) % 1_000_000
    )
}
#[test]
fn mfa_gates_named_authority_without_changing_book_protection() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, path) = create(dir.path(), false);
    app.enroll_owner(USER_PASSWORD).unwrap();
    app.mfa_authorize_management(USER_PASSWORD, "").unwrap();
    let secret = app.mfa_begin_totp().unwrap();
    app.mfa_confirm_totp(&totp(&secret, -1)).unwrap();
    let company = app.first_company().unwrap().unwrap();
    app.os_store_usable = true;
    let before = std::fs::read(fb_app::protection::record_path(&path)).unwrap();
    app.sign_out();
    app.sign_in(OWNER, USER_PASSWORD).unwrap();
    assert!(app.mfa_pending());
    assert!(!Record::read(&path).unwrap().unwrap().ask_password);
    assert!(app.query(Query::Companies).is_err());
    let html = screens::render(&mut app, SIGN_IN).unwrap().html;
    assert!(html.contains(&format!("Signing in as <strong>{OWNER}</strong>")));
    assert!(
        app.ai_grant(company, "Must refuse", fb_app::ai::Permission::ReadOnly)
            .is_err()
    );
    app.mfa_complete_signin(&totp(&secret, 0)).unwrap();
    assert_eq!(std::fs::read(fb_app::protection::record_path(&path)).unwrap(), before);
    assert!(app.query(Query::Companies).is_ok());
}

#[test]
fn retired_signin_marker_is_read_only_and_never_changes_explicit_settings() {
    for marker in [None, Some(false), Some(true)] {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, path) = create(dir.path(), false);
        app.enroll_owner(USER_PASSWORD).unwrap();
        app.change_ask_password(BOOK_PASSWORD, true).unwrap();
        app.lock();
        let file = fb_app::protection::record_path(&path);
        let mut saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        saved.as_object_mut().unwrap().remove("desktop_signin_configured");
        if let Some(value) = marker { saved["desktop_signin_configured"] = value.into(); }
        let bytes = serde_json::to_vec(&saved).unwrap();
        std::fs::write(&file, &bytes).unwrap();
        app.os_store_usable = true;
        app.open_with_secret(&path, BOOK_PASSWORD).unwrap();
        assert!(app.query(Query::Companies).is_err());
        app.sign_in(OWNER, USER_PASSWORD).unwrap();
        assert!(app.can_lock());
        assert_eq!(std::fs::read(&file).unwrap(), bytes, "sign-in must leave existing settings alone");
        let record = Record::read(&path).unwrap().unwrap();
        assert!(serde_json::to_value(&record).unwrap().get("desktop_signin_configured").is_none());
        app.change_ask_password(BOOK_PASSWORD, false).unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert!(saved.get("desktop_signin_configured").is_none(), "new writes omit the retired marker");
    }
}
