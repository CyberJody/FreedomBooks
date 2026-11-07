//! WAVE4 fix 1, lead decision on review finding 3: one connection's unreadable saved sign-in never
//! makes the books unopenable. The books open, that connection is marked "reconnect needed" (audited,
//! no pulls), and every other connection keeps restore-before-any-command and its mask.
use fb_app::{App, Command, Outcome, credentials::BookCredentials, keys::MasterKey};
use fb_core::{CompanyId, bankfeeds, directory::{self, Model}, services::{self, AccountInput, DraftInput, line}};
use fb_plugin_api::{CredentialScope, CredentialStore, SecretBytes};
use serde_json::json;

const HEALTHY: &str = "https://HealthyUser12345678:HealthyPassword12345678@fixture.test/access";
const DAMAGED: &str = "https://DamagedUser12345678:DamagedPassword12345678@fixture.test/access";
const OTHER: &str = "https://OtherCoUser12345678:OtherCoPassword12345678@fixture.test/access";

fn connect(app: &mut App, co: CompanyId, label: &str, access: Option<&str>) -> String {
    let secrets = match access {
        Some(a) => bankfeeds::SecretSet::from_access_url(a, Vec::<String>::new()).unwrap(),
        None => bankfeeds::SecretSet::none(),
    };
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

fn count(app: &mut App, sql: &str) -> i64 {
    app.open_mut().unwrap().book.read(|q| q.query_row(sql, [], |r| r.get(0))).unwrap()
}

fn flagged(app: &mut App, connection: &str) -> bool {
    app.open_mut().unwrap().book.read(|q| q.query_row("SELECT needs_reconnect FROM bankfeeds_connection WHERE id=?1", [connection], |r| r.get(0))).unwrap()
}

fn pull(app: &mut App, co: CompanyId, connection: &str) -> Result<i64, String> {
    let o = app.open_mut().unwrap();
    bankfeeds::begin_sync(&mut o.book, o.owner, co, connection, "2026-11-03T12:00:00Z", 0, true).map_err(|e| e.message())
}

/// Submit a company rename carrying `secret` through the renderer boundary; return the stored name.
fn rename(app: &mut App, co: CompanyId, secret: &str) -> String {
    let form = format!("/c/{}/edit/", co.to_db());
    let token = app.tokens.mint(&form);
    let outcome = app.run_renderer(Command::SubmitForm { path: form, token, fields: vec![("name".into(), format!("Renamed {secret}")), ("currency".into(), "USD".into())] }).unwrap();
    assert!(matches!(outcome, Outcome::Redirect { .. }));
    app.open_mut().unwrap().book.read(|q| q.query_row("SELECT name FROM ledger_company WHERE id=?1", [co.to_db()], |r| r.get(0))).unwrap()
}

enum Damage { Missing, Corrupt }

fn damaged_connection_opens(damage: Damage) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reconnect.fbbook");
    let key = MasterKey::new([91; 32]);
    let mut app = App::new();
    app.plugins = true;
    let co = app.create_book(&path, &key, "Reconnect operator", "Synthetic reconnect").unwrap().company_id;
    let other_co = {
        let o = app.open_mut().unwrap();
        services::create_company(&mut o.book, o.owner, "Synthetic second company", "USD", true).unwrap().id
    };
    let healthy = connect(&mut app, co, "Healthy feed", Some(HEALTHY));
    save_envelope(&mut app, &key, co, &healthy, HEALTHY);
    let damaged = connect(&mut app, co, "Checking feed", Some(DAMAGED));
    let other = connect(&mut app, other_co, "Other feed", Some(OTHER));
    save_envelope(&mut app, &key, other_co, &other, OTHER);
    match damage {
        Damage::Missing => {}
        Damage::Corrupt => {
            save_envelope(&mut app, &key, co, &damaged, DAMAGED);
            app.lock();
            app.open_book(&path, &key).unwrap();
            app.open_mut().unwrap().book.write(|tx| {
                let mut blob: Vec<u8> = tx.query_row("SELECT cipher_blob FROM plugin_credential WHERE connection_id=?1", [&damaged], |r| r.get(0))?;
                blob[3] ^= 0x40;
                tx.execute("UPDATE plugin_credential SET cipher_blob=?1 WHERE connection_id=?2", fb_db::rusqlite::params![blob, damaged])
            }).unwrap();
        }
    }
    app.lock();

    app.open_book(&path, &key).expect("one unreadable sign-in must not lock the books");
    assert!(app.is_open());
    assert_eq!(app.messages, vec!["The saved sign-in for Checking feed couldn't be read. Your books are fine. Reconnect or forget this connection on the Bank feeds page.".to_owned()]);
    assert!(flagged(&mut app, &damaged));
    assert!(!flagged(&mut app, &healthy) && !flagged(&mut app, &other));
    assert_eq!(count(&mut app, "SELECT count(*) FROM ledger_auditevent WHERE action='bankfeed.reconnect_required' AND detail LIKE '%\"credential\":\"unreadable\"%'"), 1);
    // The flagged connection keeps its row, history and active state (reconnect or forget remain the user's choice).
    assert_eq!(count(&mut app, "SELECT count(*) FROM bankfeeds_connection WHERE active=1"), 3);
    // No pulls for it; the others still pull.
    assert_eq!(pull(&mut app, co, &damaged), Err(bankfeeds::UNSAFE_CREDENTIAL.to_owned()));
    assert!(pull(&mut app, co, &healthy).is_ok());
    assert!(pull(&mut app, other_co, &other).is_ok());
    // The other connections' masks were restored before any command: their secrets never persist.
    for (company, secret) in [(co, "HealthyPassword12345678"), (other_co, "OtherCoPassword12345678")] {
        let stored = rename(&mut app, company, secret);
        assert!(!stored.contains(secret) && stored.contains("***"), "{stored}");
    }
    // The vault never hands the unreadable credential to a plugin.
    let open = app.open.take().unwrap();
    let store = BookCredentials::new(open.book, open.owner, &key).unwrap();
    let scope = CredentialScope { plugin_id: "simplefin".into(), company_id: co.to_db().parse().unwrap(), connection_id: damaged.parse().unwrap() };
    assert!(!matches!(store.load(&scope), Ok(Some(_))));
    drop(store);
    // Reopening keeps it flagged without a second audit, and still says so.
    app.open_book(&path, &key).unwrap();
    assert!(flagged(&mut app, &damaged));
    assert_eq!(count(&mut app, "SELECT count(*) FROM ledger_auditevent WHERE action='bankfeed.reconnect_required'"), 1);
    assert_eq!(app.messages.len(), 1);
    // Forgetting it (the user's way out) clears the notice on the next open.
    let o = app.open_mut().unwrap();
    bankfeeds::forget(&mut o.book, o.owner, co, &damaged, &bankfeeds::SecretSet::none()).unwrap();
    app.lock();
    app.open_book(&path, &key).unwrap();
    assert!(app.messages.is_empty());
    assert!(pull(&mut app, co, &healthy).is_ok());
}

#[test]
fn missing_envelope_on_an_active_connection_opens_with_reconnect_needed() {
    damaged_connection_opens(Damage::Missing);
}

#[test]
fn corrupt_envelope_on_an_active_connection_opens_with_reconnect_needed() {
    damaged_connection_opens(Damage::Corrupt);
}

/// The release is refused for a credential that IS readable, so the write guard stays for every
/// connection whose mask is expected; a raw Book with one unrestored readable connection can't write.
#[test]
fn readable_credentials_cannot_be_released_from_the_write_guard() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("readable.fbbook");
    let key = MasterKey::new([92; 32]);
    let mut app = App::new();
    app.plugins = true;
    let co = app.create_book(&path, &key, "Guard operator", "Synthetic guard").unwrap().company_id;
    let healthy = connect(&mut app, co, "Healthy feed", Some(HEALTHY));
    save_envelope(&mut app, &key, co, &healthy, HEALTHY);
    app.lock();
    let mut book = fb_db::Book::open(&path, &key).unwrap();
    let mut wrapping = [0u8; 32];
    hkdf::Hkdf::<sha2::Sha256>::new(None, &key[..]).expand(fb_plugin_api::CREDENTIAL_KEY_LABEL, &mut wrapping).unwrap();
    assert!(book.release_unreadable_bankfeed(&healthy, &wrapping).is_err());
    // The readable connection stays pending: no write until its mask is restored.
    let refused = book.write(|tx| tx.execute("UPDATE ledger_company SET name='x' WHERE id=?1", [co.to_db()]));
    assert!(refused.unwrap_err().to_string().contains("Restore bank-feed connection credentials"));
}

fn populate(app: &mut App, co: CompanyId) {
    let o = app.open_mut().unwrap();
    let u = o.owner;
    services::create_company(&mut o.book, u, "Second frozen company", "USD", false).unwrap();
    let bank = services::save_account(&mut o.book, u, co, AccountInput::new("100", "Frozen bank").kind("ASSET")).unwrap().id;
    let equity = services::save_account(&mut o.book, u, co, AccountInput::new("300", "Frozen equity").kind("EQUITY")).unwrap().id;
    directory::save_record(&mut o.book, u, co, Model::Customer, &json!({"display_name":"Frozen customer","billing_address":"1 Frozen St"}), None).unwrap();
    directory::save_record(&mut o.book, u, co, Model::Vendor, &json!({"display_name":"Frozen vendor","billing_address":"2 Frozen St"}), None).unwrap();
    let draft = services::save_draft(&mut o.book, u, co, DraftInput::new("2026-10-01", "Frozen memo", vec![line(bank).debit("10").build(), line(equity).credit("10").build()])).unwrap();
    services::post_journal(&mut o.book, u, co, draft.id, None).unwrap();
}

/// A populated book frozen at the last be0a51f migration (0140, before the vault existed) with an active
/// connection: it upgrades, opens, flags only that connection, and otherwise keeps every row.
#[test]
fn frozen_0140_book_with_an_active_connection_opens_with_reconnect_needed() {
    let dir = tempfile::tempdir().unwrap();
    // Match the desktop layout: the backup folder is outside the books folder.
    let books = dir.path().join("books");
    std::fs::create_dir(&books).unwrap();
    let src = books.join("src140.fbbook");
    let prior = books.join("prior140.fbbook");
    let key = MasterKey::new([93; 32]);
    let mut app = App::new();
    app.plugins = true;
    // The upgrade on open writes its verified copy to the backup folder first.
    app.backups.configure(dir.path()).unwrap();
    let co = app.create_book(&src, &key, "Frozen owner", "Frozen company").unwrap().company_id;
    populate(&mut app, co);
    let conn = connect(&mut app, co, "Prior feed", None);
    let copy = {
        let o = app.open_mut().unwrap();
        fb_db::test_support::populated_copy(o.book.connection(), &prior, &key, "0140_deposit_estimates", true).unwrap()
    };
    let mut expected_before = copy.expected.clone();
    for table in ["plugin_credential", "company_document_details", "fb_numbering"] {
        assert_eq!(expected_before.remove(table), Some(Vec::new()), "the later {table} table must be empty");
        assert!(!copy.before.contains_key(table), "{table} did not exist at the frozen boundary");
    }
    assert_eq!(copy.before, expected_before);
    app.lock();
    app.open_book(&prior, &key).expect("a pre-vault book with a connection must open");
    assert!(app.upgrade_notice.as_deref().is_some_and(|n| n.contains("freedombooks-prior140-before-upgrade-")));
    assert!(flagged(&mut app, &conn));
    assert_eq!(app.messages.len(), 1);
    let after = fb_db::test_support::snapshot(app.open_mut().unwrap().book.connection()).unwrap();
    for (table, rows) in &copy.expected {
        if table != "bankfeeds_connection" && table != "ledger_auditevent" {
            assert_eq!(after.get(table), Some(rows), "{table} changed on open");
        }
    }
    let audits = &after["ledger_auditevent"];
    assert_eq!(audits.len(), copy.expected["ledger_auditevent"].len() + 1);
    assert_eq!(&audits[..audits.len() - 1], &copy.expected["ledger_auditevent"][..]);
    assert_eq!(count(&mut app, "SELECT count(*) FROM fb_value_invalid"), 0);
    let integrity: String = app.open_mut().unwrap().book.read(|q| q.query_row("PRAGMA integrity_check", [], |r| r.get(0))).unwrap();
    assert_eq!(integrity, "ok");
    assert_eq!(pull(&mut app, co, &conn), Err(bankfeeds::UNSAFE_CREDENTIAL.to_owned()));
    // Writes work, and it reopens.
    rename(&mut app, co, "plain");
    app.lock();
    app.open_book(&prior, &key).unwrap();
    assert!(flagged(&mut app, &conn));
}
