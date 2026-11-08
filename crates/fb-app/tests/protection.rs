mod support;
use fb_app::{
    App, Command, Outcome,
    keys::MasterKey,
    protection::{self, Record},
};
use fb_core::backups::{
    self,
    portable::{BUILTIN_KEY, BUILTIN_WARNING, Mode},
};
use fb_plugin_api::{CredentialScope, CredentialStore, SecretBytes};
use std::{
    fs,
    path::{Path, PathBuf},
};

const PASSWORD: &str = "Synthetic owner password";

#[test]
fn recovery_copy_requires_authorization_renews_form_token_and_does_not_mark_saved() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, path) = setup(dir.path(), true);
    let token = app.tokens.mint(protection::RECOVERY);
    let command = |token: String| Command::SubmitForm {path:protection::RECOVERY.into(),token,fields:vec![("action".into(),"copy".into())]};
    let Outcome::Done {data} = app.run_renderer(command(token.clone())).unwrap() else {panic!("copy refused")};
    assert_eq!(data["desktop_keys"],"copy-recovery");
    assert!(!data.to_string().contains(app.recovery_text().unwrap().as_str()));
    assert!(!app.protection.as_ref().unwrap().recovery_saved);
    assert!(app.run_renderer(command(token)).is_err());
    let next = data["next_token"].as_str().unwrap().to_owned();
    assert!(matches!(app.run_renderer(command(next.clone())).unwrap(),Outcome::Done {..}));
    app.lock();
    app.open_with_secret(&path, PASSWORD).unwrap();
    support::sign_in(&mut app);
    assert!(app.run_renderer(command(next)).is_err());
    let token = app.tokens.mint(protection::RECOVERY);
    assert!(matches!(app.run_renderer(command(token)).unwrap(),Outcome::Invalid {..}));
    assert!(!app.protection.as_ref().unwrap().recovery_saved);
}

#[test]
fn recovery_export_tickets_expire_on_lock_reauthorization_and_acknowledgment() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, path) = setup(dir.path(), true);
    let first = app.recovery_export_ticket().unwrap();
    app.validate_recovery_export(&first).unwrap();
    app.authorize_recovery(PASSWORD).unwrap();
    assert!(app.validate_recovery_export(&first).is_err());
    let before_lock = app.recovery_export_ticket().unwrap();
    app.lock();
    app.open_with_secret(&path, PASSWORD).unwrap();
    support::sign_in(&mut app);
    app.authorize_recovery(PASSWORD).unwrap();
    assert!(app.recovery_saved_export(&before_lock).is_err());
    assert!(!app.protection.as_ref().unwrap().recovery_saved);
    let current = app.recovery_export_ticket().unwrap();
    app.recovery_saved_export(&current).unwrap();
    assert!(app.recovery_saved_export(&current).is_err());
    let count: i64 = app.open_mut().unwrap().book.read(|q| q.query_row(
        "SELECT count(*) FROM ledger_auditevent WHERE action='book.recovery_saved'", [], |r| r.get(0))).unwrap();
    assert_eq!(count, 1);
}

fn app_at(root: &Path) -> App {
    let mut app = App::new();
    app.backups.configure(root).unwrap();
    app.device_key = Some(MasterKey::new([93; 32]));
    app
}
fn setup(root: &Path, own: bool) -> (App, PathBuf) {
    let mut app = app_at(root);
    let path = root.join("books/default.fbbook");
    app.create_protected_book(
        &path,
        "Synthetic owner",
        "Synthetic portable books",
        own.then_some(PASSWORD),
    )
    .unwrap();
    support::sign_in(&mut app);
    if own { app.authorize_recovery(PASSWORD).unwrap(); }
    (app, path)
}
fn backup(app: &mut App) -> PathBuf {
    let c = app.first_company().unwrap().unwrap();
    let path = format!("/c/{}/backups/create/", c.to_db());
    let token = app.tokens.mint(&path);
    let result = app
        .run_renderer(Command::SubmitForm {
            path,
            token,
            fields: vec![],
        })
        .unwrap();
    let Outcome::Redirect { location } = result else {
        panic!("backup did not finish")
    };
    let name = location.split("created=").nth(1).expect("backup created");
    let file = app.backups.folder().unwrap().join(name);
    assert!(backups::retention::verified_note(&file));
    file
}
fn assert_no_bytes(file: &Path, needle: &[u8]) {
    assert!(
        !fs::read(file)
            .unwrap()
            .windows(needle.len())
            .any(|w| w == needle),
        "unwrapped secret in artifact"
    );
}

#[test]
fn builtin_defaults_open_without_secret_and_use_plain_warning() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, path) = setup(d.path(), false);
    assert!(app.builtin());
    assert!(!app.can_lock());
    assert!(app.recovery_text().is_err());
    let r = Record::read(&path).unwrap().unwrap();
    assert!(!r.ask_password && r.keys.password.is_none() && r.keys.recovery.is_none());
    assert_eq!(*r.keys.unlock("").unwrap(), BUILTIN_KEY);
    let screen = fb_app::screens::render(&mut app, protection::SETTINGS)
        .unwrap()
        .html;
    assert!(screen.contains(BUILTIN_WARNING));
    assert!(!screen.to_lowercase().contains("encrypted"));
    assert!(screen.contains("File › Lock, idle lock and switching users still end the individual sign-in session"));
    app.lock();
    app.open_book(&path, &protection::builtin_key()).unwrap();
    support::sign_in(&mut app);
    assert_eq!(
        app.company_name_for_kit().unwrap(),
        "Synthetic portable books"
    );
}

#[test]
fn both_modes_restore_in_second_install_and_wrong_secrets_create_nothing() {
    for own in [false, true] {
        let source = tempfile::tempdir().unwrap();
        let (mut app, path) = setup(source.path(), own);
        let record = Record::read(&path).unwrap().unwrap();
        let master = MasterKey::new(**app.master_key().unwrap());
        let recovery = own.then(|| record.keys.recovery_text(&master).unwrap());
        let file = backup(&mut app);
        let header = backups::read_header(&file).unwrap();
        assert_eq!(header["format_version"], 4);
        assert_eq!(
            header["book_keys"]["mode"],
            if own { "own-password" } else { "built-in" }
        );
        if own {
            assert_no_bytes(&file, &*master);
            assert_no_bytes(&file, fb_app::keys::to_hex(master.as_ref()).as_bytes());
            assert_no_bytes(&file, recovery.as_ref().unwrap().as_bytes());
            assert_no_bytes(&file, PASSWORD.as_bytes());
        }
        let secrets = if own {
            vec![PASSWORD.to_owned(), recovery.as_ref().unwrap().to_string()]
        } else {
            vec![String::new()]
        };
        for secret in secrets {
            let target = tempfile::tempdir().unwrap();
            let mut other = app_at(target.path());
            other.device_key = Some(MasterKey::new([94; 32]));
            if own {
                assert!(
                    fb_app::screens::backups::restore_from_gate(
                        &mut other,
                        &file,
                        "Refused",
                        "wrong password"
                    )
                    .is_err()
                );
                assert!(!target.path().join("books/restored-refused").exists());
            }
            let restored = fb_app::screens::backups::restore_from_gate(
                &mut other,
                &file,
                "Second install",
                &secret,
            )
            .unwrap();
            support::sign_in(&mut other);
            assert_ne!(restored, path);
            assert_eq!(
                other.company_name_for_kit().unwrap(),
                "Synthetic portable books"
            );
            assert_eq!(other.builtin(), !own);
            other.lock();
            let portable = Record::read(&restored).unwrap().unwrap();
            other
                .open_book(&restored, &portable.keys.unlock(&secret).unwrap())
                .unwrap();
            support::sign_in(&mut other);
            assert_eq!(
                other.company_name_for_kit().unwrap(),
                "Synthetic portable books"
            );
        }
    }
}

#[test]
fn rekey_both_ways_preserves_data_and_receipt_key_and_old_password_stops_working() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, path) = setup(d.path(), false);
    let receipts = app.receipt_key().unwrap();
    app.change_protection("", Some(PASSWORD)).unwrap();
    assert_eq!(
        app.protection.as_ref().unwrap().keys.mode,
        Mode::OwnPassword
    );
    assert!(app.can_lock());
    assert_eq!(*app.receipt_key().unwrap(), *receipts);
    let own_key = MasterKey::new(**app.master_key().unwrap());
    assert!(fb_db::Book::open(&path, &BUILTIN_KEY).is_err());
    assert_eq!(
        app.company_name_for_kit().unwrap(),
        "Synthetic portable books"
    );
    assert!(app.change_protection("wrong password", None).is_err());
    std::thread::sleep(std::time::Duration::from_secs(1));
    app.change_protection(PASSWORD, None).unwrap();
    assert!(app.builtin());
    assert!(!app.can_lock());
    assert_eq!(*app.receipt_key().unwrap(), *receipts);
    assert!(fb_db::Book::open(&path, &own_key).is_err());
    app.lock();
    app.open_book(&path, &protection::builtin_key()).unwrap();
    support::sign_in(&mut app);
    assert_eq!(
        app.company_name_for_kit().unwrap(),
        "Synthetic portable books"
    );
    assert!(!protection::sidecar(&path, ".rekey.json").exists());
}

#[test]
fn owner_password_prompt_requires_authentication_and_keyring_with_encrypted_credentials() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, path) = setup(d.path(), true);
    assert!(
        app.change_ask_password(PASSWORD, false)
            .unwrap_err()
            .to_string()
            .contains("no system keyring")
    );
    app.os_store_usable = true;
    assert!(app.change_ask_password("wrong", false).is_err());
    app.change_ask_password(PASSWORD, false).unwrap();
    assert!(!app.can_lock());
    assert!(!Record::read(&path).unwrap().unwrap().ask_password);
    app.change_ask_password(PASSWORD, true).unwrap();
    assert!(app.can_lock());
    let c = app.first_company().unwrap().unwrap();
    let open = app.open.take().unwrap();
    let vault = fb_app::credentials::BookCredentials::new(
        open.book,
        open.owner,
        app.device_key.as_ref().unwrap(),
    )
    .unwrap();
    let scope = CredentialScope {
        company_id: c.to_db().parse().unwrap(),
        plugin_id: "openai".into(),
        connection_id: uuid::Uuid::new_v4(),
    };
    vault
        .save(
            &scope,
            SecretBytes::new(b"Synthetic independent API token".to_vec()),
        )
        .unwrap();
    app.open = Some(fb_app::session::Open {
        book: vault.into_book().unwrap(),
        owner: open.owner,
        owner_name: open.owner_name,
        staff: open.staff,
    });
    app.change_ask_password(PASSWORD, false).unwrap();
    assert!(!Record::read(&path).unwrap().unwrap().ask_password);
    assert!(app.has_credentials().unwrap(), "saved encrypted connections remain available");
}

#[test]
fn connection_secrets_are_unreadable_with_the_builtin_key() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, path) = setup(d.path(), false);
    let c = app.first_company().unwrap().unwrap();
    let open = app.open.take().unwrap();
    let scope = CredentialScope {
        company_id: c.to_db().parse().unwrap(),
        plugin_id: "openai".into(),
        connection_id: uuid::Uuid::new_v4(),
    };
    let vault = fb_app::credentials::BookCredentials::new(
        open.book,
        open.owner,
        app.device_key.as_ref().unwrap(),
    )
    .unwrap();
    let secret = b"Synthetic independent connection credential";
    vault
        .save(&scope, SecretBytes::new(secret.to_vec()))
        .unwrap();
    assert_eq!(vault.load(&scope).unwrap().unwrap().expose(), secret);
    let public = fb_app::credentials::BookCredentials::new(
        vault.into_book().unwrap(),
        open.owner,
        &BUILTIN_KEY,
    )
    .unwrap();
    assert!(public.load(&scope).is_err());
    drop(public);
    assert_no_bytes(&path, secret);
}

#[test]
fn old_password_book_keeps_its_key_and_gains_recovery_after_unlock() {
    let d = tempfile::tempdir().unwrap();
    let mut app = app_at(d.path());
    let path = d.path().join("old.fbbook");
    let key = MasterKey::new([71; 32]);
    app.create_book(&path, &key, "Old owner", "Old synthetic books")
        .unwrap();
    support::sign_in(&mut app);
    app.lock();
    app.open_book(&path, &key).unwrap();
    support::sign_in(&mut app);
    app.adopt_legacy_keys(PASSWORD).unwrap();
    assert_eq!(*app.master_key().unwrap(), key);
    assert!(app.recovery_authorized);
    assert!(!app.protection.as_ref().unwrap().recovery_saved);
    app.recovery_saved().unwrap();
    assert!(Record::read(&path).unwrap().unwrap().recovery_saved);
    app.lock();
    app.open_book(&path, &key).unwrap();
    support::sign_in(&mut app);
    assert!(app.recovery_text().is_err());
    assert!(app.authorize_recovery("wrong").is_err());
    app.authorize_recovery(PASSWORD).unwrap();
    assert_eq!(app.recovery_text().unwrap().len(), 41);
}

#[test]
fn setup_and_invalid_password_pages_keep_warning_and_explicit_opt_in() {
    let app = App::new();
    for errors in [vec![], vec!["The two passwords differ.".to_owned()]] {
        let html = fb_app::screens::render_gate(&app, "setup", false, "", &errors).unwrap();
        assert!(html.contains(BUILTIN_WARNING));
        assert!(html.contains("Protect these books with my own password"));
        let checkbox = html
            .split("id=\"id_protect\"")
            .next()
            .unwrap()
            .rsplit('<')
            .next()
            .unwrap();
        assert!(!checkbox.contains("checked"));
        assert!(!html.to_lowercase().contains("encrypted"));
    }
    let html = fb_app::screens::render_gate(
        &app,
        "unlock",
        false,
        "",
        &["The saved key is not available.".into()],
    )
    .unwrap();
    assert!(html.contains("id=\"id_password\""));
    assert!(html.contains("Restore a backup"));
}

#[test]
fn recovery_authorization_expires_when_leaving_the_key_screens() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, _) = setup(d.path(), true);
    assert!(
        fb_app::screens::render(&mut app, protection::RECOVERY)
            .unwrap()
            .html
            .contains("recovery-key")
    );
    fb_app::screens::render(&mut app, protection::SETTINGS).unwrap();
    assert!(app.recovery_text().is_err());
    let html = fb_app::screens::render(&mut app, protection::RECOVERY)
        .unwrap()
        .html;
    assert!(html.contains("Enter your password to show the recovery key"));
}

#[test]
fn portable_restore_reconnects_connections_with_another_installation_key() {
    for own in [false, true] {
        let source = tempfile::tempdir().unwrap();
        let (mut app, _) = setup(source.path(), own);
        let c = app.first_company().unwrap().unwrap();
        let access = "https://SyntheticUser1234:SyntheticPassword1234@fixture.test/access";
        let secrets =
            fb_core::bankfeeds::SecretSet::from_access_url(access, Vec::<String>::new()).unwrap();
        let o = app.open_mut().unwrap();
        let id = fb_core::bankfeeds::connect(
            &mut o.book,
            o.owner,
            c,
            &serde_json::json!("Synthetic feed"),
            "fixture.test",
            &secrets,
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let open = app.open.take().unwrap();
        let vault = fb_app::credentials::BookCredentials::new(
            open.book,
            open.owner,
            app.device_key.as_ref().unwrap(),
        )
        .unwrap();
        let scope = CredentialScope {
            company_id: c.to_db().parse().unwrap(),
            plugin_id: "simplefin".into(),
            connection_id: id.parse().unwrap(),
        };
        vault
            .save(
                &scope,
                SecretBytes::new(
                    serde_json::json!({"access_url":access})
                        .to_string()
                        .into_bytes(),
                ),
            )
            .unwrap();
        app.open = Some(fb_app::session::Open {
            book: vault.into_book().unwrap(),
            owner: open.owner,
            owner_name: open.owner_name,
            staff: open.staff,
        });
        let file = backup(&mut app);
        assert_no_bytes(&file, access.as_bytes());
        let target = tempfile::tempdir().unwrap();
        let mut other = app_at(target.path());
        other.device_key = Some(MasterKey::new([94; 32]));
        other.plugins = true;
        fb_app::screens::backups::restore_from_gate(
            &mut other,
            &file,
            "Reconnected",
            if own { PASSWORD } else { "" },
        )
        .unwrap();
        support::sign_in(&mut other);
        let count: i64 = other
            .open_mut()
            .unwrap()
            .book
            .read(|q| {
                q.query_row(
                    "SELECT count(*) FROM bankfeeds_connection WHERE needs_reconnect=1",
                    [],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(count, 1);
        assert!(other.messages.iter().any(|m| m.contains("Reconnect")));
        let open = other.open.take().unwrap();
        let public =
            fb_app::credentials::BookCredentials::new(open.book, open.owner, &BUILTIN_KEY).unwrap();
        assert!(public.load(&scope).is_err());
    }
}

#[test]
fn attachments_survive_both_rekeys_and_portable_restore() {
    use fb_core::{
        attachments::{self as a, Limits, Owner, storage::Store},
        services::{self, AccountInput, DraftInput},
    };
    let d = tempfile::tempdir().unwrap();
    let (mut app, path) = setup(d.path(), false);
    let c = app.first_company().unwrap().unwrap();
    let receipt_key = app.receipt_key().unwrap();
    let o = app.open_mut().unwrap();
    let cash = services::save_account(
        &mut o.book,
        o.owner,
        c,
        AccountInput::new("1000", "Synthetic cash").kind("ASSET"),
    )
    .unwrap()
    .id;
    let equity = services::save_account(
        &mut o.book,
        o.owner,
        c,
        AccountInput::new("3000", "Synthetic equity").kind("EQUITY"),
    )
    .unwrap()
    .id;
    let lines = vec![services::line(cash).debit("1.00").credit("0").build(),services::line(equity).debit("0").credit("1.00").build()];
    let journal = services::save_draft(
        &mut o.book,
        o.owner,
        c,
        DraftInput::new("2026-01-01", "Synthetic receipt", lines),
    )
    .unwrap();
    let store = Store::for_book(&path, &receipt_key).unwrap();
    let pdf = b"%PDF-1.4\n1 0 obj<</Type/Catalog>>endobj\n%%EOF\n";
    let row = a::add(
        &mut o.book,
        &store,
        o.owner,
        c,
        &Owner::Journal(journal.id),
        pdf,
        "synthetic.pdf",
        &uuid::Uuid::new_v4().to_string(),
        &Limits::default(),
    )
    .unwrap()
    .0;
    let id = row["id"].as_str().unwrap();
    for password in [Some(PASSWORD), None] {
        app.change_protection(PASSWORD, password).unwrap();
        let o = app.open_mut().unwrap();
        assert_eq!(
            a::download(&mut o.book, &store, o.owner, c, id).unwrap(),
            pdf
        );
        let archive = backup(&mut app);
        let target = tempfile::tempdir().unwrap();
        let mut other = app_at(target.path());
        let restored = fb_app::screens::backups::restore_from_gate(
            &mut other,
            &archive,
            "Receipt restore",
            password.unwrap_or(""),
        )
        .unwrap();
        support::sign_in(&mut other);
        let store = Store::for_book(&restored, &other.receipt_key().unwrap()).unwrap();
        let o = other.open_mut().unwrap();
        assert_eq!(
            a::download(&mut o.book, &store, o.owner, c, id).unwrap(),
            pdf
        );
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}


#[test]
fn restored_ai_account_requires_reconnect_and_local_account_stays_usable() {
    let source=tempfile::tempdir().unwrap();
    let (mut app,path)=setup(source.path(),true);
    let c=app.first_company().unwrap().unwrap();
    let key=MasterKey::new(**app.master_key().unwrap());
    let o=app.open_mut().unwrap();
    fb_core::assistant::connected(&mut o.book,o.owner,c,&serde_json::json!({"mode":"api_key","key_last4":"1234","model_choices":[],"model":""})).unwrap();
    let open=app.open.take().unwrap();
    let scope=CredentialScope {company_id:c.to_db().parse().unwrap(),plugin_id:"openai".into(),connection_id:c.to_db().parse().unwrap()};
    let vault=fb_app::credentials::BookCredentials::new(open.book,open.owner,app.device_key.as_ref().unwrap()).unwrap();
    vault.save(&scope,SecretBytes::new(b"Synthetic AI account credential".to_vec())).unwrap();
    drop(vault);
    app.open_book(&path,&key).unwrap();
    support::sign_in(&mut app);
    let o=app.open_mut().unwrap();
    assert_eq!(fb_core::assistant::connection_for(&mut o.book,o.owner,c).unwrap().unwrap()["status"],"connected");
    let file=backup(&mut app);
    let target=tempfile::tempdir().unwrap();
    let mut other=app_at(target.path());
    other.device_key=Some(MasterKey::new([94;32]));
    fb_app::screens::backups::restore_from_gate(&mut other,&file,"AI restore",PASSWORD).unwrap();
    support::sign_in(&mut other);
    let o=other.open_mut().unwrap();
    let row=fb_core::assistant::connection_for(&mut o.book,o.owner,c).unwrap().unwrap();
    assert_eq!(row["status"],"needs_signin");
    assert_eq!(row["credential_present"],false);
    assert!(row["last_error"].as_str().unwrap().starts_with("Reconnect needed."));
}

#[test]
fn switch_forms_say_when_older_backups_cannot_be_restored_after_the_change() {
    let d = tempfile::tempdir().unwrap();
    let mut app = app_at(d.path());
    let path = d.path().join("books/default.fbbook");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let key = MasterKey::new([73; 32]);
    app.create_book(&path, &key, "Old owner", "Old synthetic books")
        .unwrap();
    support::sign_in(&mut app);
    // A book from before recovery keys makes backups with no key envelope.
    let older = backup(&mut app);
    assert!(backups::read_header(&older).unwrap()["book_keys"].is_null());
    app.adopt_legacy_keys(PASSWORD).unwrap();
    let html = fb_app::screens::render(&mut app, protection::SETTINGS)
        .unwrap()
        .html;
    assert!(html.contains(protection::OLDER_BACKUPS), "{html}");
    assert!(!html.contains("Older backups keep the password or recovery key"));
    fs::remove_file(&older).unwrap();
    backup(&mut app);
    let html = fb_app::screens::render(&mut app, protection::SETTINGS)
        .unwrap()
        .html;
    assert!(!html.contains(protection::OLDER_BACKUPS));
    assert!(html.contains("Backups made since recovery keys were added keep the password or recovery key"));
}

#[test]
fn leftover_temporary_key_files_and_a_published_stage_are_cleared() {
    let d = tempfile::tempdir().unwrap();
    let (mut app, path) = setup(d.path(), true);
    app.lock();
    let books = path.parent().unwrap();
    let id = "0b0e1a1e-5f2c-4c1e-9d55-6a7c0c1f2e3d";
    let leftovers = [format!("default.fbbook.keys.json.tmp-{id}"), format!("default.fbbook.os-key.tmp-{id}")];
    for name in &leftovers {
        fs::write(books.join(name), b"synthetic wrapped record").unwrap();
    }
    let stage = books.join("default.fbbook.creating");
    fs::create_dir(&stage).unwrap();
    fs::hard_link(&path, stage.join("book.fbbook")).unwrap();
    let kept = [format!("other.fbbook.keys.json.tmp-{id}"), "default.fbbook.keys.json.tmp-not-a-uuid".to_owned()];
    for name in &kept {
        fs::write(books.join(name), b"not ours").unwrap();
    }
    protection::clean_leftovers(&path).unwrap();
    for name in &leftovers {
        assert!(!books.join(name).exists(), "{name}");
    }
    assert!(!stage.exists());
    for name in &kept {
        assert!(books.join(name).exists(), "{name}");
    }
    let record = Record::read(&path).unwrap().unwrap();
    app.open_book(&path, &record.keys.unlock(PASSWORD).unwrap())
        .unwrap();
    support::sign_in(&mut app);
    assert_eq!(app.company_name_for_kit().unwrap(), "Synthetic portable books");
}
