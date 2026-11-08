//! Release 1: plugin surfaces (bank feeds, AI assistant, receipt reader, bill AI actions) are hidden
//! unless the build has the `plugins` feature. Run without it, this file tests the release build;
//! every plugin route refuses GET and POST, and no core page links to one. A book that already has a
//! bank-feed connection (made in a dev build) still opens and takes writes.
mod support;
use fb_app::credentials::BookCredentials;
use fb_app::keys::MasterKey;
use fb_app::{App, AppError, Command, Outcome, screens};
use fb_core::{CompanyId, bankfeeds};
use fb_plugin_api::{CredentialScope, CredentialStore, SecretBytes};
use serde_json::{Value, json};

const ACCESS: &str = "https://HiddenUser12345678:HiddenPassword12345678@fixture.test/access";
const DAMAGED: &str = "https://DamagedUser12345678:DamagedPassword12345678@fixture.test/access";

/// The words and addresses of a plugin surface; none may appear on a core page of a release build.
const SURFACE: [&str; 9] = ["/bank-feeds/", "/ai/", "/auth/openai", "Bank feeds", "AI assistant", "ChatGPT", "Read with AI", "read-with-ai", "start-from-receipt"];

fn field(html: &str, name: &str) -> String {
    html.split_once(&format!("name=\"{name}\" value=\"")).unwrap().1.split('"').next().unwrap().into()
}

fn command(app: &mut App, v: Value) -> Value {
    match app.run(fb_app::commands::parse_command(&v.to_string()).unwrap()).unwrap() {
        Outcome::Done { data } => data,
        other => panic!("{other:?}"),
    }
}

struct Books {
    app: App,
    _dir: tempfile::TempDir,
    path: std::path::PathBuf,
    key: MasterKey,
    c: String,
    bill: String,
}

fn books(plugins: bool) -> Books {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hidden.fbbook");
    let key = MasterKey::new([71; 32]);
    let mut app = App::new();
    app.simplefin = false;
    app.plugins = plugins;
    app.backups.configure(&dir.path().join("data")).unwrap();
    app.today = || chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
    let c = app.create_book(&path, &key, "owner", "Hidden plugin books").unwrap().company_id.to_db();
    support::sign_in(&mut app);
    let account = |app: &mut App, code: &str, detail: &str| -> String {
        command(app, json!({"type":"SaveAccount","company_id":c,"code":code,"name":code,"detail_type":detail}))["id"].as_str().unwrap().into()
    };
    let bank = account(&mut app, "1000", "BANK");
    let ap = account(&mut app, "2000", "PAYABLE");
    let expense = account(&mut app, "5000", "EXPENSE");
    command(&mut app, json!({"type":"SaveSetup","company_id":c,"expected_revision":0,"receivables_id":null,"undeposited_id":null,"payables_id":ap,"bank_ids":[bank]}));
    let o = app.open_mut().unwrap();
    let vendor = fb_core::directory::save_record(&mut o.book, o.owner, c.parse().unwrap(), fb_core::directory::Model::Vendor, &json!({"display_name":"Supplier"}), None).unwrap()["id"].as_str().unwrap().to_owned();
    // The new-bill form only shows its receipt reader to a plugins build; save the draft as a user would.
    app.plugins = true;
    let form = format!("/c/{c}/bills/new/");
    let html = screens::render(&mut app, &form).unwrap().html;
    let fields: Vec<(String, String)> = [("request_key", field(&html, "request_key")), ("expected_revision", "0".into()), ("vendor", vendor), ("date", "2026-10-01".into()), ("due_date", "2026-10-31".into()),
        ("lines-TOTAL_FORMS", "1".into()), ("lines-INITIAL_FORMS", "0".into()), ("lines-0-description", "Supplies".into()), ("lines-0-quantity", "2".into()), ("lines-0-rate", "10.00".into()), ("lines-0-expense_account", expense)]
        .into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    let Outcome::Redirect { location } = app.run(Command::SubmitForm { path: form, token: field(&html, "fb_command_token"), fields }).unwrap() else { panic!("bill") };
    app.plugins = plugins;
    let bill = location.trim_end_matches('/').rsplit('/').next().unwrap().to_owned();
    Books { app, _dir: dir, path, key, c, bill }
}

/// Every route of the plugin namespaces, from the same route table the app resolves.
fn plugin_paths(c: &str) -> Vec<(String, String)> {
    let urls: serde_json::Map<String, Value> = serde_json::from_str(include_str!("../../fb-templates/urls.json")).unwrap();
    let paths: Vec<(String, String)> = urls.iter().filter(|(name, _)| screens::plugin_route(name)).map(|(name, pattern)| {
        let mut path = pattern.as_str().unwrap().replace("<int:company_id>", c);
        while let Some(start) = path.find('<') {
            let end = path[start..].find('>').unwrap() + start;
            path.replace_range(start..=end, &uuid::Uuid::new_v4().to_string());
        }
        (name.clone(), path)
    }).collect();
    assert!(paths.len() >= 20, "bank feeds and assistant routes: {paths:?}");
    paths
}

#[test]
fn the_default_build_is_the_release_build_with_plugins_hidden() {
    assert_eq!(App::new().plugins, cfg!(feature = "plugins"));
    assert_eq!(App::new().simplefin, cfg!(feature = "simplefin"));
}

#[test]
fn simplefin_can_be_enabled_without_ai_or_receipt_actions() {
    let mut b = books(false);
    b.app.simplefin = true;
    let home = screens::render(&mut b.app, &format!("/c/{}/home/", b.c)).unwrap().html;
    assert!(home.contains("Bank feeds"));
    assert!(!home.contains("AI assistant"));
    for page in ["bank-feeds/", "bank-feeds/connect/"] {
        let html = screens::render(&mut b.app, &format!("/c/{}/{page}", b.c)).unwrap().html;
        assert!(!html.contains("manage.py"));
        assert!(!html.contains("FB_SECRETS_KEY"));
    }
    for (name, path) in plugin_paths(&b.c).into_iter().filter(|(name, _)| name.starts_with("assistant:")) {
        assert!(matches!(screens::render(&mut b.app, &path), Err(AppError::NotFound(_))), "GET {name}");
        let token = b.app.tokens.mint(&path);
        assert!(matches!(b.app.run_renderer(Command::SubmitForm {path,token,fields:vec![]}), Err(AppError::NotFound(_))), "POST {name}");
    }
    let bill = screens::render(&mut b.app, &format!("/c/{}/bills/new/", b.c)).unwrap().html;
    assert!(!bill.contains("Read with AI") && !bill.contains("start-from-receipt"));
}

#[test]
fn every_plugin_route_refuses_get_and_post_in_a_release_build() {
    let mut b = books(false);
    for (name, path) in plugin_paths(&b.c) {
        assert!(matches!(screens::render(&mut b.app, &path), Err(AppError::NotFound(_))), "GET {name} {path}");
        let token = b.app.tokens.mint(&path);
        let posted = b.app.run_renderer(Command::SubmitForm { path: path.clone(), token, fields: vec![("action".into(), "connect".into())] });
        assert!(matches!(posted, Err(AppError::NotFound(_))), "POST {name} {path}: {posted:?}");
    }
    // The same pages exist behind the flag: the refusal is the build, not a missing screen.
    b.app.plugins = true;
    for page in ["bank-feeds/", "ai/"] {
        assert_eq!(screens::render(&mut b.app, &format!("/c/{}/{page}", b.c)).unwrap().status, 200, "{page}");
    }
}

#[test]
fn core_pages_show_no_plugin_surface_in_a_release_build() {
    let mut b = books(false);
    let c = b.c.clone();
    let pages = ["/".to_owned(), format!("/c/{c}/home/"), format!("/c/{c}/accounts/"), format!("/c/{c}/bills/"), format!("/c/{c}/bills/new/"),
        format!("/c/{c}/bills/{}/", b.bill), format!("/c/{c}/backups/"), format!("/c/{c}/edit/"), "/help/report/".to_owned()];
    for path in &pages {
        let html = screens::render(&mut b.app, path).unwrap().html;
        for word in SURFACE {
            assert!(!html.contains(word), "{path}: {word}");
        }
    }
    // A plugins build keeps Django's nav, footer and the bill's Read with AI section.
    b.app.plugins = true;
    let home = screens::render(&mut b.app, &format!("/c/{c}/home/")).unwrap().html;
    for word in ["Bank feeds", "AI assistant", "/bank-feeds/", "/ai/"] {
        assert!(home.contains(word), "plugins build home: {word}");
    }
    assert!(screens::render(&mut b.app, &format!("/c/{c}/bills/{}/", b.bill)).unwrap().html.contains(r#"id="read-with-ai""#));
}

fn connect(app: &mut App, co: CompanyId, label: &str, access: &str) -> String {
    let secrets = bankfeeds::SecretSet::from_access_url(access, Vec::<String>::new()).unwrap();
    let o = app.open_mut().unwrap();
    bankfeeds::connect(&mut o.book, o.owner, co, &json!(label), "fixture.test", &secrets).unwrap()["id"].as_str().unwrap().to_owned()
}

fn save_envelope(app: &mut App, key: &MasterKey, co: CompanyId, connection: &str, access: &str) {
    let open = app.open.take().unwrap();
    let store = BookCredentials::new(open.book, open.owner, key).unwrap();
    let scope = CredentialScope { plugin_id: "simplefin".into(), company_id: co.to_db().parse().unwrap(), connection_id: connection.parse().unwrap() };
    store.save(&scope, SecretBytes::new(json!({"access_url": access}).to_string().into_bytes())).unwrap();
    let book = store.into_book().unwrap();
    app.open = Some(fb_app::session::Open { book, owner: open.owner, owner_name: open.owner_name, staff: open.staff });
}

/// A company rename through the renderer boundary; the stored name.
fn rename(app: &mut App, c: &str, name: &str) -> String {
    let form = format!("/c/{c}/edit/");
    let token = app.tokens.mint(&form);
    let outcome = app.run_renderer(Command::SubmitForm { path: form, token, fields: vec![("name".into(), name.into()), ("currency".into(), "USD".into())] }).unwrap();
    assert!(matches!(outcome, Outcome::Redirect { .. }), "{outcome:?}");
    app.open_mut().unwrap().book.read(|q| q.query_row("SELECT name FROM ledger_company WHERE id=?1", [c], |r| r.get(0))).unwrap()
}

/// INT-D9 scope (perf audit §7): an active bank-feed connection makes fb-db refuse writes until its
/// credentials are restored. Opening the books restores them whatever the build, so a book with
/// connections from a dev build (one readable, one whose saved sign-in is gone) is not locked by a
/// release build, keeps masking the readable secret, and shows no bank-feed notice.
#[test]
fn a_book_with_bank_feed_connections_opens_and_writes_in_a_release_build() {
    let mut b = books(true);
    let co: CompanyId = b.c.parse().unwrap();
    let readable = connect(&mut b.app, co, "Checking feed", ACCESS);
    save_envelope(&mut b.app, &b.key, co, &readable, ACCESS);
    connect(&mut b.app, co, "Savings feed", DAMAGED);
    b.app.lock();

    // The raw book is write-guarded until credentials are restored (the condition under test).
    let mut raw = fb_core::Book::open(&b.path, &b.key).unwrap();
    let owner: String = raw.read(|q| q.query_row("SELECT id FROM fb_user ORDER BY id LIMIT 1", [], |r| r.get(0))).unwrap();
    let refused = fb_core::services::edit_company(&mut raw, owner.parse().unwrap(), co, "must refuse", "USD").unwrap_err();
    assert!(refused.to_string().contains("Restore bank-feed connection credentials"), "{refused}");
    drop(raw);

    let mut release = App::new();
    release.plugins = false;
    release.simplefin = false;
    release.backups.configure(&b._dir.path().join("data")).unwrap();
    release.open_book(&b.path, &b.key).expect("a release build opens a book with bank-feed connections");
    support::sign_in(&mut release);
    assert!(release.messages.is_empty(), "no bank-feed notice in a release build: {:?}", release.messages);
    assert_eq!(rename(&mut release, &b.c, "Renamed in release"), "Renamed in release");
    // The readable connection's mask was restored too: its secret never reaches the books.
    let stored = rename(&mut release, &b.c, "Renamed HiddenPassword12345678");
    assert!(!stored.contains("HiddenPassword12345678") && stored.contains("***"), "{stored}");
    // Pages after unlock (the Backups page shows pending messages) carry no plugin wording.
    let html = screens::render(&mut release, &format!("/c/{}/backups/", b.c)).unwrap().html;
    for word in SURFACE {
        assert!(!html.contains(word), "backups page: {word}");
    }
    // And the same book opens with its notice in a plugins build.
    release.lock();
    let mut dev = App::new();
    dev.plugins = true;
    dev.open_book(&b.path, &b.key).unwrap();
    support::sign_in(&mut dev);
    assert_eq!(dev.messages.len(), 1, "{:?}", dev.messages);
}
