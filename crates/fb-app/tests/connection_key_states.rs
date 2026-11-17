//! The installation's connection key after a lost, missing or replaced keyring. A saved connection
//! is marked "reconnect needed" only when no key on this computer (the connection key, or a
//! superseded one kept aside) can read it; a keyring that is merely unavailable for one launch
//! changes nothing durable.
mod support;
use fb_app::{App, ConnectionKeys, credentials::BookCredentials, keys::MasterKey};
use fb_core::{CompanyId, bankfeeds};
use fb_plugin_api::{CredentialScope, CredentialStore, SecretBytes};
use serde_json::json;

const ACCESS: &str = "https://KeyStateUser12345678:KeyStatePassword12345678@fixture.test/access";
const DEVICE: [u8; 32] = [61; 32];

fn count(app: &mut App, sql: &str) -> i64 {
    app.open_mut().unwrap().book.read(|q| q.query_row(sql, [], |r| r.get(0))).unwrap()
}

fn flagged(app: &mut App, connection: &str) -> bool {
    app.open_mut().unwrap().book.read(|q| q.query_row("SELECT needs_reconnect FROM bankfeeds_connection WHERE id=?1", [connection], |r| r.get(0))).unwrap()
}

fn rename(app: &mut App, co: CompanyId, name: &str) {
    let o = app.open_mut().unwrap();
    let owner = o.owner;
    o.book.write(|tx| {
        fb_core::services::access(tx, owner, co)?;
        tx.execute("UPDATE ledger_company SET name=?2 WHERE id=?1", [co.to_db(), name.to_owned()])?;
        Ok::<_, fb_core::FbError>(())
    }).unwrap();
}

/// A book with one saved bank connection, wrapped with the installation key `DEVICE`.
fn book_with_connection(dir: &std::path::Path) -> (std::path::PathBuf, MasterKey, CompanyId, String) {
    let path = dir.join("keys.fbbook");
    let key = MasterKey::new([62; 32]);
    let mut app = App::new();
    app.plugins = true;
    app.device_key = Some(MasterKey::new(DEVICE));
    let co = app.create_book(&path, &key, "Key state owner", "Synthetic key states").unwrap().company_id;
    support::sign_in(&mut app);
    let secrets = bankfeeds::SecretSet::from_access_url(ACCESS, Vec::<String>::new()).unwrap();
    let connection = {
        let o = app.open_mut().unwrap();
        bankfeeds::connect(&mut o.book, o.owner, co, &json!("Checking feed"), "fixture.test", &secrets).unwrap()["id"].as_str().unwrap().to_owned()
    };
    let open = app.open.take().unwrap();
    let store = BookCredentials::new(open.book, open.owner, &DEVICE).unwrap();
    let scope = CredentialScope { plugin_id: "simplefin".into(), company_id: co.to_db().parse().unwrap(), connection_id: connection.parse().unwrap() };
    store.save(&scope, SecretBytes::new(json!({"access_url": ACCESS}).to_string().into_bytes())).unwrap();
    drop(store.into_book().unwrap());
    (path, key, co, connection)
}

fn launch(path: &std::path::Path, key: &MasterKey, state: ConnectionKeys, device: Option<[u8; 32]>) -> App {
    let mut app = App::new();
    app.plugins = true;
    app.connection_keys = state;
    app.device_key = device.map(MasterKey::new);
    app.os_store_usable = state != ConnectionKeys::Unavailable;
    app.open_book(path, key).unwrap();
    support::sign_in(&mut app);
    app
}

#[test]
fn a_keyring_unavailable_for_one_launch_flags_nothing_and_the_next_launch_restores_the_connection() {
    let dir = tempfile::tempdir().unwrap();
    let (path, key, co, connection) = book_with_connection(dir.path());
    let reconnect_events = "SELECT count(*) FROM ledger_auditevent WHERE action='bankfeed.reconnect_required'";

    // No device key at all (the desktop before the shell configures one) behaves the same way.
    for (state, device) in [(ConnectionKeys::Unavailable, None), (ConnectionKeys::Unavailable, Some([99; 32])), (ConnectionKeys::Ready, None)] {
        let mut app = launch(&path, &key, state, device);
        assert!(!flagged(&mut app, &connection), "an unavailable key must not mark the connection");
        assert_eq!(count(&mut app, reconnect_events), 0);
        assert!(app.messages.iter().any(|m| m == fb_app::session::CONNECTIONS_WAITING), "{:?}", app.messages);
        // The books stay writable this session (the connection's write guard was released for it only).
        rename(&mut app, co, "Renamed while the keyring is away");
        app.lock();
    }

    // The keyring is back with the original key: the connection is restored, never flagged.
    let mut app = launch(&path, &key, ConnectionKeys::Ready, Some(DEVICE));
    assert!(!flagged(&mut app, &connection));
    assert_eq!(count(&mut app, reconnect_events), 0);
    assert!(app.messages.is_empty(), "{:?}", app.messages);
    let o = app.open_mut().unwrap();
    bankfeeds::begin_sync(&mut o.book, o.owner, co, &connection, "2026-11-10T12:00:00Z", 0, true).expect("the connection can pull again");
}

#[test]
fn a_replaced_key_marks_saved_connections_once_with_a_plain_message() {
    let dir = tempfile::tempdir().unwrap();
    let (path, key, _co, connection) = book_with_connection(dir.path());
    let new_key = [63; 32];
    let mut app = launch(&path, &key, ConnectionKeys::Replaced, Some(new_key));
    assert!(flagged(&mut app, &connection));
    assert_eq!(count(&mut app, "SELECT count(*) FROM ledger_auditevent WHERE action='bankfeed.reconnect_required'"), 1);
    assert_eq!(app.messages.first().map(String::as_str), Some(fb_app::session::CONNECTIONS_REPLACED));
    app.lock();
    // The next launch with the new key is simply "ready": no second flag, no second event.
    let mut app = launch(&path, &key, ConnectionKeys::Ready, Some(new_key));
    assert_eq!(count(&mut app, "SELECT count(*) FROM ledger_auditevent WHERE action='bankfeed.reconnect_required'"), 1);
    assert!(!app.messages.iter().any(|m| m == fb_app::session::CONNECTIONS_REPLACED));
}

/// Finding 5: an older version minted a plaintext key during a keyring outage and the owner
/// reconnected under it; the sealed key that later wins is a different one. The superseded key is
/// offered for reading: the connection is read, re-saved under the key in use, and never flagged.
#[test]
fn a_connection_saved_under_a_superseded_key_is_read_with_it_and_resaved() {
    let dir = tempfile::tempdir().unwrap();
    let (path, key, co, connection) = book_with_connection(dir.path());
    let sealed = [64; 32];
    let events = "SELECT count(*) FROM ledger_auditevent WHERE action='bankfeed.reconnect_required'";
    let mut app = App::new();
    app.plugins = true;
    app.connection_keys = ConnectionKeys::Ready;
    app.device_key = Some(MasterKey::new(sealed));
    app.device_alternates = vec![MasterKey::new(DEVICE)];
    app.os_store_usable = true;
    app.open_book(&path, &key).unwrap();
    support::sign_in(&mut app);
    assert!(!flagged(&mut app, &connection), "readable with the superseded key: not flagged");
    assert_eq!(count(&mut app, events), 0);
    let o = app.open_mut().unwrap();
    bankfeeds::begin_sync(&mut o.book, o.owner, co, &connection, "2026-11-10T12:00:00Z", 0, true).expect("the connection can pull");
    app.lock();
    // Re-saved under the key in use: a later launch without the superseded key reads it too.
    let mut app = launch(&path, &key, ConnectionKeys::Ready, Some(sealed));
    assert!(!flagged(&mut app, &connection), "re-saved under the sealed key");
    assert_eq!(count(&mut app, events), 0);
}

/// Without the superseded key (the code before finding 5) the same connection was flagged durably
/// even though the state was "ready": kept as the regression this guards against.
#[test]
fn without_the_superseded_key_the_connection_cannot_be_read() {
    let dir = tempfile::tempdir().unwrap();
    let (path, key, _co, connection) = book_with_connection(dir.path());
    let mut app = launch(&path, &key, ConnectionKeys::Ready, Some([64; 32]));
    assert!(flagged(&mut app, &connection));
}

#[test]
fn connection_key_states_parse_only_known_words() {
    assert_eq!(ConnectionKeys::parse("ready"), Some(ConnectionKeys::Ready));
    assert_eq!(ConnectionKeys::parse("created"), Some(ConnectionKeys::Ready));
    assert_eq!(ConnectionKeys::parse("replaced"), Some(ConnectionKeys::Replaced));
    assert_eq!(ConnectionKeys::parse("unavailable"), Some(ConnectionKeys::Unavailable));
    assert_eq!(ConnectionKeys::parse("Ready"), None);
    assert_eq!(ConnectionKeys::parse(""), None);
}
