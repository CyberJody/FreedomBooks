//! Synthetic desktop identity acceptance: no unlock-as-owner shortcut and no authority carried
//! by a caller-supplied user id. Every accounting operation uses the verified session actor.
use fb_app::{
    App, Command, Outcome, Query,
    identity::{Permission, Role, SESSION, SIGN_IN, USERS},
    keys::MasterKey,
    screens,
};
use fb_core::{CompanyId, UserId};
use serde_json::json;
const OWNER: &str = "Synthetic owner";
const PASSWORD: &str = "Synthetic individual password";
struct Fixture {
    _dir: tempfile::TempDir,
    app: App,
    path: std::path::PathBuf,
    key: MasterKey,
    company: CompanyId,
    owner: UserId,
}
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("books/default.fbbook");
    let key = MasterKey::new([77; 32]);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut app = App::new();
    app.backups.configure(dir.path()).unwrap();
    let company = app
        .create_book(&path, &key, OWNER, "Synthetic company")
        .unwrap()
        .company_id;
    assert!(app.authenticated_session().is_err());
    assert!(app.query(Query::Companies).is_err());
    assert_eq!(
        screens::render(&mut app, &format!("/c/{}/home/", company.to_db()))
            .unwrap()
            .html,
        SIGN_IN
    );
    app.enroll_owner(PASSWORD).unwrap();
    let owner = app.authenticated_session().unwrap().actor;
    Fixture {
        _dir: dir,
        app,
        path,
        key,
        company,
        owner,
    }
}
fn create(f: &mut Fixture, name: &str, role: Role) -> UserId {
    let id = f
        .app
        .save_desktop_user(None, name, role, true, Some(PASSWORD), &[f.company])
        .unwrap();
    assert!(f.app.authenticated_session().is_err());
    f.app.sign_in(OWNER, PASSWORD).unwrap();
    id
}
fn cmd(app: &mut App, value: serde_json::Value) -> fb_app::AppResult<Outcome> {
    app.run(serde_json::from_value(value).unwrap())
}
fn post(app: &mut App, path: &str, fields: &[(&str, &str)]) -> fb_app::AppResult<Outcome> {
    let token = app.tokens.mint(path);
    app.run_renderer(Command::SubmitForm {
        path: path.into(),
        token,
        fields: fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    })
}
#[test]
fn unlock_does_not_authenticate_wrong_password_inactive_user_and_reopen() {
    let mut f = fixture();
    let disabled = create(&mut f, "Disabled operator", Role::Bookkeeper);
    f.app
        .save_desktop_user(
            Some(disabled),
            "Disabled operator",
            Role::Bookkeeper,
            false,
            None,
            &[f.company],
        )
        .unwrap();
    assert!(f.app.sign_in("Disabled operator", PASSWORD).is_err());
    assert!(f.app.sign_in(OWNER, "wrong individual password").is_err());
    assert!(f.app.sign_in("unknown", PASSWORD).is_err());
    assert!(f.app.authenticated_session().is_err());
    f.app.sign_in(OWNER, PASSWORD).unwrap();
    let generation = f.app.session_generation();
    f.app.lock();
    f.app.open_book(&f.path, &f.key).unwrap();
    assert!(f.app.session_generation() > generation);
    assert!(f.app.authenticated_session().is_err());
    assert!(f.app.bootstrap_owner().unwrap().is_none());
    assert!(f.app.enroll_owner("replacement password").is_err());
    assert!(f.app.open_mut().is_err());
    f.app.sign_in(OWNER, PASSWORD).unwrap();
    assert_eq!(f.app.authenticated_session().unwrap().actor, f.owner);
    let hash: String = f
        .app
        .open_mut()
        .unwrap()
        .book
        .read(|q| {
            q.query_row(
                "SELECT password_hash FROM desktop_user WHERE user_id=?1",
                [f.owner.to_db()],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert!(hash.starts_with("$argon2id$v=19$"));
    assert!(!hash.contains(PASSWORD));
}
#[test]
fn company_membership_readonly_writes_and_nonadmin_management_are_enforced() {
    let mut f = fixture();
    let c2 = fb_core::services::create_company(
        &mut f.app.open_mut().unwrap().book,
        f.owner,
        "Other synthetic company",
        "USD",
        false,
    )
    .unwrap()
    .id;
    create(&mut f, "Reader", Role::Readonly);
    f.app.sign_out();
    f.app.sign_in("Reader", PASSWORD).unwrap();
    assert!(f.app.authorize(Some(f.company), Permission::Read).is_ok());
    assert!(f.app.authorize(Some(c2), Permission::Read).is_err());
    assert!(
        f.app
            .query(Query::Accounts {
                company_id: c2.to_db()
            })
            .is_err()
    );
    assert!(screens::render(&mut f.app, &format!("/c/{}/home/", c2.to_db())).is_err());
    assert!(cmd(&mut f.app,json!({"type":"SaveCustomer","company_id":f.company.to_db(),"display_name":"Must refuse"})).is_err());
    let path = format!("/c/{}/lists/customers/new/", f.company.to_db());
    assert!(post(&mut f.app, &path, &[("display_name", "Must refuse")]).is_err());
    assert!(
        f.app
            .save_desktop_user(
                None,
                "Attack",
                Role::Administrator,
                true,
                Some(PASSWORD),
                &[f.company]
            )
            .is_err()
    );
    assert!(screens::render(&mut f.app, USERS).is_err());
    assert!(f.app.authorize_recovery("anything").is_err());
    assert!(f.app.recovery_text().is_err());
    assert!(f.app.change_ask_password("anything", true).is_err());
    f.app.sign_out();
    f.app.sign_in(OWNER, PASSWORD).unwrap();
    create(&mut f, "Bookkeeper", Role::Bookkeeper);
    f.app.sign_out();
    f.app.sign_in("Bookkeeper", PASSWORD).unwrap();
    assert!(cmd(&mut f.app,json!({"type":"SaveCustomer","company_id":f.company.to_db(),"display_name":"Bookkeeper customer"})).is_ok());
    assert!(f.app.authorize(None, Permission::Administer).is_err());
    assert!(
        f.app
            .save_desktop_user(
                None,
                "Attack",
                Role::Administrator,
                true,
                Some(PASSWORD),
                &[f.company]
            )
            .is_err()
    );
}
#[test]
fn last_administrator_and_each_company_remain_recoverable_and_failed_change_is_atomic() {
    let mut f = fixture();
    for (role, active) in [
        (Role::Administrator, false),
        (Role::Bookkeeper, true),
        (Role::Readonly, true),
    ] {
        assert!(
            f.app
                .save_desktop_user(Some(f.owner), OWNER, role, active, None, &[f.company])
                .is_err()
        );
        assert_eq!(f.app.authenticated_session().unwrap().actor, f.owner);
    }
    let c2 = fb_core::services::create_company(
        &mut f.app.open_mut().unwrap().book,
        f.owner,
        "Company needing an admin",
        "USD",
        false,
    )
    .unwrap()
    .id;
    let admin = create(&mut f, "Second admin", Role::Administrator);
    assert!(
        f.app
            .save_desktop_user(
                Some(f.owner),
                OWNER,
                Role::Administrator,
                false,
                None,
                &[f.company, c2]
            )
            .is_err(),
        "other admin is not a member of company 2"
    );
    assert!(
        f.app
            .save_desktop_user(
                Some(f.owner),
                OWNER,
                Role::Administrator,
                true,
                None,
                &[f.company]
            )
            .is_err(),
        "cannot remove company 2's only administrator"
    );
    f.app
        .save_desktop_user(
            Some(admin),
            "Second admin",
            Role::Administrator,
            true,
            None,
            &[f.company, c2],
        )
        .unwrap();
    f.app.sign_in(OWNER, PASSWORD).unwrap();
    f.app
        .save_desktop_user(
            Some(f.owner),
            OWNER,
            Role::Administrator,
            false,
            None,
            &[f.company, c2],
        )
        .unwrap();
    assert!(f.app.sign_in(OWNER, PASSWORD).is_err());
    f.app.sign_in("Second admin", PASSWORD).unwrap();
    assert!(f.app.open_mut().unwrap().book.check().unwrap().is_empty());
}
#[test]
fn session_generation_forms_reviews_and_accounting_audit_follow_the_person() {
    let mut f = fixture();
    let keeper = create(&mut f, "Pat Bookkeeper", Role::Bookkeeper);
    let (cash, equity) = {
        let open = f.app.open_mut().unwrap();
        let cash = fb_core::services::save_account(
            &mut open.book,
            open.owner,
            f.company,
            fb_core::services::AccountInput::new("1000", "Cash").kind("ASSET"),
        )
        .unwrap()
        .id;
        let equity = fb_core::services::save_account(
            &mut open.book,
            open.owner,
            f.company,
            fb_core::services::AccountInput::new("3000", "Equity").kind("EQUITY"),
        )
        .unwrap()
        .id;
        (cash, equity)
    };
    let generation = f.app.authenticated_session().unwrap().generation;
    let path = format!("/c/{}/lists/customers/new/", f.company.to_db());
    let old = f.app.tokens.mint(&path);
    let recovery = f.app.tokens.mint("desktop:recovery-export");
    f.app.sign_out();
    f.app.sign_in("Pat Bookkeeper", PASSWORD).unwrap();
    assert!(!f.app.tokens.valid(&recovery, "desktop:recovery-export"));
    assert!(
        f.app
            .require_session(generation, Some(f.company), Permission::Write)
            .is_err()
    );
    assert!(
        f.app
            .run_renderer(Command::SubmitForm {
                path,
                token: old,
                fields: vec![]
            })
            .is_err()
    );
    cmd(&mut f.app,json!({"type":"SaveCustomer","company_id":f.company.to_db(),"display_name":"Actual signed-in customer"})).unwrap();
    let (actor,name):(String,String)=f.app.open_mut().unwrap().book.read(|q|q.query_row("SELECT actor_id,actor_name FROM ledger_auditevent WHERE action='customer.created' ORDER BY id DESC LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?)))).unwrap();
    assert_eq!(actor, keeper.to_db());
    assert_eq!(name, "Pat Bookkeeper");
    let Outcome::Done { data }=cmd(&mut f.app,json!({"type":"SaveJournal","company_id":f.company.to_db(),"date":"2026-11-01","memo":"Named person's balanced entry","entry_number":"NAMED-1","adjusting":false,"lines":[{"account":cash.to_db(),"debit":"12.34","credit":"0.00","description":"","name":""},{"account":equity.to_db(),"debit":"0.00","credit":"12.34","description":"","name":""}]})).unwrap() else { panic!("journal did not save"); };
    let journal = data["id"].as_str().unwrap();
    cmd(&mut f.app,json!({"type":"PostJournal","company_id":f.company.to_db(),"journal_id":journal,"expected_revision":1})).unwrap();
    let (created, posted): (String, String) = f
        .app
        .open_mut()
        .unwrap()
        .book
        .read(|q| {
            q.query_row(
                "SELECT created_by_id,posted_by_id FROM ledger_journal WHERE id=?1",
                [journal],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
        })
        .unwrap();
    assert_eq!(created, keeper.to_db());
    assert_eq!(posted, keeper.to_db());
    let (actor,name):(String,String)=f.app.open_mut().unwrap().book.read(|q|q.query_row("SELECT actor_id,actor_name FROM ledger_auditevent WHERE action='journal.posted' ORDER BY id DESC LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?)))).unwrap();
    assert_eq!(actor, keeper.to_db());
    assert_eq!(name, "Pat Bookkeeper");
    let html = screens::render(&mut f.app, SESSION).unwrap().html;
    assert!(html.contains("Pat Bookkeeper"));
    assert!(matches!(
        post(&mut f.app, SESSION, &[("action", "signout")]).unwrap(),
        Outcome::Redirect { .. }
    ));
    assert!(f.app.query(Query::Companies).is_err());
}
#[test]
fn interrupted_owner_enrollment_reopens_and_a_genuine_0184_book_upgrades_with_backup() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("books")).unwrap();
    let path = dir.path().join("books/genuine.fbbook");
    let key = MasterKey::new([61; 32]);
    let mut source = fb_db::Book::create(&dir.path().join("fixture-source.fbbook"), &key).unwrap();
    let owner = fb_core::accounts::create_owner(&mut source, OWNER).unwrap();
    // Some legacy operator rows predate desktop name trimming; preserve their exact identity.
    let legacy_name = "  Historical owner  ";
    source
        .write(|q| {
            q.execute(
                "UPDATE fb_user SET username=?2 WHERE id=?1",
                fb_db::rusqlite::params![owner.to_db(), legacy_name],
            )
        })
        .unwrap();
    let company = fb_core::services::create_company(
        &mut source,
        owner,
        "Historical synthetic company",
        "USD",
        false,
    )
    .unwrap()
    .id;
    fb_core::bankfeeds::connect(
        &mut source,
        owner,
        company,
        &json!("Historical synthetic feed"),
        "fixture.test",
        &fb_core::bankfeeds::SecretSet::none(),
    )
    .unwrap();
    let before = fb_db::test_support::populated_copy(
        source.connection(),
        &path,
        &key,
        "0184_release_guard_union",
        true,
    )
    .unwrap()
    .before;
    drop(source);
    let mut app = App::new();
    app.backups.configure(dir.path()).unwrap();
    app.open_book(&path, &key).unwrap();
    assert!(app.upgrade_notice.is_some());
    let after =
        fb_db::test_support::snapshot(app.open.as_ref().unwrap().book.connection()).unwrap();
    for (table, rows) in before {
        assert_eq!(after[&table], rows, "retained rows changed in {table}");
    }
    assert_eq!(
        app.bootstrap_owner().unwrap(),
        Some((owner, legacy_name.into()))
    );
    assert!(app.authenticated_session().is_err());
    let backups =
        fb_core::backups::retention::backup_files(&app.backups.folder().unwrap(), None).unwrap();
    assert_eq!(backups.len(), 1);
    assert!(backups[0].with_extension("fbbackup.verified.json").exists());
    app.lock();
    app.open_book(&path, &key).unwrap();
    assert!(app.upgrade_notice.is_none());
    assert_eq!(app.bootstrap_owner().unwrap().unwrap().0, owner);
    assert!(app.enroll_owner("short").is_err());
    assert!(app.bootstrap_owner().unwrap().is_some());
    app.enroll_owner(PASSWORD).unwrap();
    assert_eq!(app.authenticated_session().unwrap().actor, owner);
    assert_eq!(app.first_company().unwrap(), Some(company));
    app.lock();
    app.open_book(&path, &key).unwrap();
    assert!(app.authenticated_session().is_err());
    app.sign_in(legacy_name, PASSWORD).unwrap();
    assert_eq!(app.authenticated_session().unwrap().actor, owner);
    assert!(app.open_mut().unwrap().book.check().unwrap().is_empty());
}
#[test]
fn ui_routes_enroll_then_require_password_and_roster_edits_expire_every_form() {
    let mut f = fixture();
    let users = screens::render(&mut f.app, USERS).unwrap();
    assert!(users.html.contains("Create a named user"));
    f.app.sign_out();
    let page = screens::render(&mut f.app, SIGN_IN).unwrap();
    assert!(page.html.contains("Sign in to FreedomBooks"));
    let result = post(
        &mut f.app,
        SIGN_IN,
        &[
            ("action", "signin"),
            ("username", OWNER),
            ("password", "wrong password"),
        ],
    )
    .unwrap();
    assert!(matches!(result,Outcome::Invalid{html} if html.contains("incorrect")));
    assert!(matches!(
        post(
            &mut f.app,
            SIGN_IN,
            &[
                ("action", "signin"),
                ("username", OWNER),
                ("password", PASSWORD)
            ]
        )
        .unwrap(),
        Outcome::Redirect { .. }
    ));
    let keeper = create(&mut f, "Keeper", Role::Bookkeeper);
    let old = f.app.tokens.mint(USERS);
    f.app
        .save_desktop_user(
            Some(keeper),
            "Keeper",
            Role::Bookkeeper,
            true,
            Some("Reset individual password"),
            &[f.company],
        )
        .unwrap();
    assert!(!f.app.tokens.valid(&old, USERS));
    assert!(f.app.sign_in("Keeper", PASSWORD).is_err());
    f.app
        .sign_in("Keeper", "Reset individual password")
        .unwrap();
}

#[test]
fn foreign_company_connections_do_not_block_a_members_write_or_leak_reconnect_labels() {
    let mut f = fixture();
    let other = fb_core::services::create_company(
        &mut f.app.open_mut().unwrap().book,
        f.owner,
        "Private company",
        "USD",
        false,
    )
    .unwrap()
    .id;
    fb_core::bankfeeds::connect(
        &mut f.app.open_mut().unwrap().book,
        f.owner,
        other,
        &json!("Private feed label"),
        "fixture.test",
        &fb_core::bankfeeds::SecretSet::none(),
    )
    .unwrap();
    let keeper = create(&mut f, "Scoped keeper", Role::Bookkeeper);
    f.app.lock();
    f.app.open_book(&f.path, &f.key).unwrap();
    f.app.sign_in("Scoped keeper", PASSWORD).unwrap();
    assert_eq!(f.app.authenticated_session().unwrap().actor, keeper);
    assert!(
        !f.app
            .messages
            .iter()
            .any(|m| m.contains("Private feed label"))
    );
    cmd(&mut f.app,json!({"type":"SaveCustomer","company_id":f.company.to_db(),"display_name":"Scoped write works"})).unwrap();
    assert!(f.app.authorize(Some(other), Permission::Read).is_err());
    let foreign_audits: i64 = f
        .app
        .open_mut()
        .unwrap()
        .book
        .read(|q| {
            q.query_row(
                "SELECT count(*) FROM ledger_auditevent WHERE company_id=?1 AND actor_id=?2",
                fb_db::rusqlite::params![other.to_db(), keeper.to_db()],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(foreign_audits, 0);
}

#[test]
fn book_unlock_and_individual_signin_are_separate_and_a_bookkeeper_can_land_before_recovery_is_saved()
 {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("books/protected.fbbook");
    let mut app = App::new();
    app.backups.configure(dir.path()).unwrap();
    let book_password = "Synthetic book encryption password";
    let c = app
        .create_protected_book(
            &path,
            OWNER,
            "Protected synthetic company",
            Some(book_password),
        )
        .unwrap()
        .company_id;
    assert!(app.authenticated_session().is_err());
    app.enroll_owner(PASSWORD).unwrap();
    app.save_desktop_user(
        None,
        "Protected keeper",
        Role::Bookkeeper,
        true,
        Some(PASSWORD),
        &[c],
    )
    .unwrap();
    assert!(app.sign_in(OWNER, book_password).is_err());
    let result = post(
        &mut app,
        SIGN_IN,
        &[
            ("action", "signin"),
            ("username", "Protected keeper"),
            ("password", PASSWORD),
        ],
    )
    .unwrap();
    assert!(
        matches!(result,Outcome::Redirect { location } if location==format!("/c/{}/home/",c.to_db()))
    );
    assert!(app.recovery_text().is_err());
    app.lock();
    assert!(app.open_with_secret(&path, PASSWORD).is_err());
    app.open_with_secret(&path, book_password).unwrap();
    assert!(app.authenticated_session().is_err());
    app.sign_in(OWNER, PASSWORD).unwrap();
    assert!(app.recovery_text().is_err());
    app.authorize_recovery(book_password).unwrap();
    assert!(app.recovery_text().is_ok());
    let ticket = app.recovery_export_ticket().unwrap();
    app.sign_out();
    app.sign_in(OWNER, PASSWORD).unwrap();
    assert!(app.validate_recovery_export(&ticket).is_err());
}

#[test]
fn pending_browser_signin_and_direct_plugin_writes_cannot_cross_identity_changes() {
    use std::sync::{Arc, Mutex};
    struct Fake(Arc<Mutex<Vec<String>>>);
    impl fb_app::net::NetHost for Fake {
        fn call(&mut self, request: &serde_json::Value) -> Result<serde_json::Value, String> {
            let op = request["op"].as_str().unwrap().to_owned();
            self.0.lock().unwrap().push(op.clone());
            Ok(match op.as_str() {
                "chatgpt_begin" => {
                    json!({"ok":true,"authorization_url":"https://synthetic.invalid/sign-in","registration":false})
                }
                "chatgpt_poll" => json!({"pending":true}),
                _ => json!({"ok":true}),
            })
        }
    }
    let mut f = fixture();
    create(&mut f, "Another admin", Role::Administrator);
    create(&mut f, "Readonly plugin user", Role::Readonly);
    let calls = Arc::new(Mutex::new(Vec::new()));
    f.app.net = Some(Box::new(Fake(calls.clone())));
    assert!(
        fb_app::plugins::chatgpt_start(&mut f.app, f.company)
            .unwrap()
            .is_ok()
    );
    f.app.sign_out();
    f.app.sign_in("Another admin", PASSWORD).unwrap();
    assert!(
        fb_app::plugins::chatgpt_poll(&mut f.app, f.company)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["chatgpt_begin", "chatgpt_cancel"]
    );
    f.app.sign_out();
    f.app.sign_in("Readonly plugin user", PASSWORD).unwrap();
    assert!(fb_app::plugins::bankfeed_sync(&mut f.app, f.company, "synthetic-connection").is_err());
    assert!(
        fb_app::plugins::bankfeed_connect(
            &mut f.app,
            f.company,
            "synthetic-token",
            "Synthetic feed"
        )
        .is_err()
    );
    assert!(fb_app::plugins::chatgpt_start(&mut f.app, f.company).is_err());
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["chatgpt_begin", "chatgpt_cancel"]
    );
}
