use fb_app::{App, Command, Outcome, credentials::BookCredentials, keys::MasterKey};
use fb_core::{Book, bankfeeds};
use fb_plugin_api::{CredentialScope, CredentialStore, SecretBytes};
use serde_json::json;

#[test]
fn desktop_submission_cannot_write_before_restore_and_reopen_masks_later_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("restore.fbbook");
    let key = MasterKey::new([53; 32]);
    let mut app = App::new();
    app.plugins = true;
    let company = app.create_book(&path, &key, "Restore operator", "Synthetic restoration").unwrap().company_id;
    let access = "https://SyntheticUser1234:SyntheticPassword1234@fixture.test/access";
    let secrets = bankfeeds::SecretSet::from_access_url(access, Vec::<String>::new()).unwrap();
    let o = app.open_mut().unwrap();
    let connection = bankfeeds::connect(&mut o.book, o.owner, company, &json!("Synthetic feed"), "fixture.test", &secrets).unwrap()["id"].as_str().unwrap().to_owned();
    let open = app.open.take().unwrap();
    let store = BookCredentials::new(open.book, open.owner, &key).unwrap();
    let scope = CredentialScope { plugin_id: "simplefin".into(), company_id: company.to_db().parse().unwrap(), connection_id: connection.parse().unwrap() };
    store.save(&scope, SecretBytes::new(json!({"access_url": access}).to_string().into_bytes())).unwrap();
    drop(store);
    // Deliberately simulate the pre-W4 host bug: publish a reopened Book without
    // restoring. Exercise actual form cleaning and App::run, never a service call.
    app.open = Some(fb_app::session::Open {
        book: Book::open(&path, &key).unwrap(), owner: open.owner,
        owner_name: open.owner_name, staff: open.staff,
    });
    let form = format!("/c/{}/edit/", company.to_db());
    assert_eq!(fb_app::screens::resolve(&form).unwrap().name, "company_edit");
    let before = snapshot(&mut app);
    let token = app.tokens.mint(&form);
    let result = app.run(Command::SubmitForm {
        path: form.clone(), token,
        fields: vec![("name".into(), "Should never persist".into()), ("currency".into(), "USD".into())],
    });
    assert!(match result {
        Ok(Outcome::Invalid { html }) => html.contains("Restore bank-feed connection credentials"),
        Err(e) => e.to_string().contains("Restore bank-feed connection credentials"),
        _ => false,
    });
    assert_eq!(snapshot(&mut app), before);
    app.lock();
    app.open_book(&path, &key).unwrap();
    let token = app.tokens.mint(&form);
    let outcome = app.run_renderer(Command::SubmitForm {
        path: form, token,
        fields: vec![("name".into(), "SyntheticPassword1234".into()), ("currency".into(), "USD".into())],
    }).unwrap();
    assert!(matches!(outcome, Outcome::Redirect { .. }));
    let after = snapshot(&mut app);
    assert!(!after.contains("SyntheticPassword1234"));
    assert!(after.contains("***"));
    app.lock();
    app.open_book(&path, &key).unwrap();
    assert_eq!(snapshot(&mut app), after);
}

fn snapshot(app: &mut App) -> String {
    app.open_mut().unwrap().book.read(|tx| {
        let company: String = tx.query_row("SELECT name FROM ledger_company LIMIT 1", [], |r| r.get(0))?;
        let mut st = tx.prepare("SELECT action, detail FROM ledger_auditevent ORDER BY id")?;
        let audits = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
        Ok::<_, fb_db::rusqlite::Error>(format!("{company} {audits:?}"))
    }).unwrap()
}

#[test]
fn missing_credential_opens_with_reconnect_needed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.fbbook");
    let key = MasterKey::new([54; 32]);
    let mut app = App::new();
    app.plugins = true;
    let company = app.create_book(&path, &key, "Missing operator", "Synthetic missing").unwrap().company_id;
    let o = app.open_mut().unwrap();
    bankfeeds::connect(&mut o.book, o.owner, company, &json!("Synthetic feed"), "fixture.test", &bankfeeds::SecretSet::none()).unwrap();
    app.lock();
    // Lead decision (WAVE4 fix 1): the books open; only that connection waits for a reconnect.
    app.open_book(&path, &key).unwrap();
    assert!(app.is_open());
    let flagged: i64 = app.open_mut().unwrap().book.read(|tx| tx.query_row("SELECT count(*) FROM bankfeeds_connection WHERE needs_reconnect=1", [], |r| r.get(0))).unwrap();
    assert_eq!(flagged, 1);
}

#[test]
fn all_masks_are_loaded_before_legacy_quarantine_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.fbbook");
    let key = MasterKey::new([55; 32]);
    let mut app = App::new();
    app.plugins = true;
    let co = app.create_book(&path, &key, "Legacy operator", "Synthetic legacy").unwrap().company_id;
    let mut scopes = Vec::new();
    for _ in 0..2 {
        let o = app.open_mut().unwrap();
        let id = bankfeeds::connect(&mut o.book, o.owner, co, &json!("Legacy feed"), "fixture.test", &bankfeeds::SecretSet::none()).unwrap()["id"].as_str().unwrap().to_owned();
        scopes.push(CredentialScope { plugin_id: "simplefin".into(), company_id: co.to_db().parse().unwrap(), connection_id: id.parse().unwrap() });
    }
    let open = app.open.take().unwrap();
    let store = BookCredentials::new(open.book, open.owner, &key).unwrap();
    for scope in scopes {
        store.save(&scope, SecretBytes::new(br#"{"access_url":"https://SyntheticUser1234:LegacyPassword!1234@fixture.test/access"}"#.to_vec())).unwrap();
    }
    drop(store);
    app.open_book(&path, &key).unwrap();
    let count: i64 = app.open_mut().unwrap().book.read(|tx| tx.query_row("SELECT count(*) FROM bankfeeds_connection WHERE active=1 AND needs_reconnect=1", [], |r| r.get(0))).unwrap();
    assert_eq!(count, 2);
    app.lock();
    app.open_book(&path, &key).unwrap();
}
