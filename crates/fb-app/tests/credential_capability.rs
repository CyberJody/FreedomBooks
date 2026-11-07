//! The host cannot acknowledge another connection with an unrelated valid mask.
use fb_app::{App, credentials::BookCredentials, keys::MasterKey};
use fb_core::{Book, bankfeeds};
use fb_plugin_api::{CredentialScope, CredentialStore, SecretBytes};
use hkdf::Hkdf;
use sha2::Sha256;
use serde_json::json;
use zeroize::Zeroizing;

#[test]
fn restoration_requires_authenticated_current_envelope_on_the_same_book_and_connection() {
    // Compiler-negative contracts for the host's new secret owners as well as
    // provider token records: deriving either trait must break this test build.
    trait AmbiguousIfSerialize<A> { fn check() {} }
    impl<T: ?Sized> AmbiguousIfSerialize<()> for T {}
    struct Serializes;
    impl<T: ?Sized + serde::Serialize> AmbiguousIfSerialize<Serializes> for T {}
    let _ = <fb_db::credential_mask::BankfeedRestore as AmbiguousIfSerialize<_>>::check;
    let _ = <BookCredentials as AmbiguousIfSerialize<_>>::check;
    trait AmbiguousIfDebug<A> { fn check() {} }
    impl<T: ?Sized> AmbiguousIfDebug<()> for T {}
    struct Debugs;
    impl<T: ?Sized + std::fmt::Debug> AmbiguousIfDebug<Debugs> for T {}
    let _ = <fb_db::credential_mask::BankfeedRestore as AmbiguousIfDebug<_>>::check;
    let _ = <BookCredentials as AmbiguousIfDebug<_>>::check;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("capability.fbbook");
    let key = MasterKey::new([57; 32]);
    let mut app = App::new();
    app.plugins = true;
    let co = app.create_book(&path, &key, "Capability operator", "Synthetic capability").unwrap().company_id;
    let mut scopes = Vec::new();
    for _ in 0..2 {
        let o = app.open_mut().unwrap();
        let id = bankfeeds::connect(&mut o.book, o.owner, co, &json!("Synthetic feed"), "fixture.test", &bankfeeds::SecretSet::none()).unwrap()["id"].as_str().unwrap().to_owned();
        scopes.push(CredentialScope { plugin_id: "simplefin".into(), company_id: co.to_db().parse().unwrap(), connection_id: id.parse().unwrap() });
    }
    let open = app.open.take().unwrap();
    let store = BookCredentials::new(open.book, open.owner, &key).unwrap();
    let value = || SecretBytes::new(br#"{"access_url":"https://SyntheticUser1234:SyntheticPassword1234@fixture.test/access"}"#.to_vec());
    for scope in &scopes { store.save(scope, value()).unwrap(); }
    drop(store);
    let mut wrapping = Zeroizing::new([0; 32]);
    Hkdf::<Sha256>::new(None, &*key).expand(fb_plugin_api::CREDENTIAL_KEY_LABEL, &mut *wrapping).unwrap();
    let mut book = Book::open(&path, &key).unwrap();
    let first = scopes[0].connection_id.to_string();
    let second = scopes[1].connection_id.to_string();
    assert!(book.authenticate_bankfeed_restore(&first, &[58; 32]).is_err());
    let token = book.authenticate_bankfeed_restore(&first, &wrapping).unwrap();
    assert!(book.restore_bankfeed_mask(&second, &token, Vec::<String>::new()).is_err());
    let mut separately_opened = Book::open(&path, &key).unwrap();
    assert!(separately_opened.restore_bankfeed_mask(&first, &token, Vec::<String>::new()).is_err());
    drop(separately_opened);
    book.restore_bankfeed_mask(&first, &token, Vec::<String>::new()).unwrap();
    assert!(book.write(|tx| tx.execute("UPDATE ledger_company SET name='Should never persist'", [])).is_err());
    let second_token = book.authenticate_bankfeed_restore(&second, &wrapping).unwrap();
    book.restore_bankfeed_mask(&second, &second_token, Vec::<String>::new()).unwrap();
    // Replacing an envelope invalidates a previously authenticated capability.
    let store = BookCredentials::new(book, open.owner, &key).unwrap();
    store.save(&scopes[0], value()).unwrap();
    let mut book = store.into_book().unwrap();
    assert!(book.restore_bankfeed_mask(&first, &token, Vec::<String>::new()).is_err());
    let current = book.authenticate_bankfeed_restore(&first, &wrapping).unwrap();
    book.restore_bankfeed_mask(&first, &current, Vec::<String>::new()).unwrap();
}
