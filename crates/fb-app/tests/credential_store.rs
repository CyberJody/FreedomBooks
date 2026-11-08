mod support;
use fb_app::{App, credentials::BookCredentials, keys::MasterKey};
use fb_plugin_api::{CredentialScope, CredentialStore, SecretBytes};
use uuid::Uuid;

#[test]
fn credentials_persist_encrypt_authenticate_scope_and_forget() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("vault.fbbook");
    let key = MasterKey::new([39; 32]);
    let mut app = App::new();
    let device = MasterKey::new([139; 32]);
    app.device_key = Some(MasterKey::new(*device));
    app.plugins = true;
    let c = app.create_book(&path, &key, "Vault operator", "Synthetic vault company").unwrap().company_id;
    support::sign_in(&mut app);
    let o = app.open_mut().unwrap();
    let other = fb_core::services::create_company(&mut o.book, o.owner, "Other synthetic vault", "USD", false).unwrap().id;
    let open = app.open.take().unwrap();
    let store = BookCredentials::new(open.book, open.owner, &device).unwrap();
    let scope = CredentialScope { company_id: Uuid::parse_str(&c.to_db()).unwrap(), plugin_id: "openai".into(), connection_id: Uuid::new_v4() };
    let secret = b"SYNTHETIC_VAULT_TOKEN_very_long_value";
    assert!(store.load(&scope).unwrap().is_none());
    store.save(&scope, SecretBytes::new(secret.to_vec())).unwrap();
    assert!(store.load(&scope).unwrap().unwrap().expose() == secret);
    let mut book = store.into_book().unwrap();
    book.read(|tx| {
        let blob: Vec<u8> = tx.query_row("SELECT cipher_blob FROM plugin_credential", [], |r| r.get(0))?;
        assert!(!blob.windows(secret.len()).any(|b| b == secret));
        Ok::<_, fb_db::rusqlite::Error>(())
    }).unwrap();
    drop(book);
    app.open_book(&path, &key).unwrap();
    support::sign_in(&mut app);
    let open = app.open.take().unwrap();
    let store = BookCredentials::new(open.book, open.owner, &device).unwrap();
    assert!(store.load(&scope).unwrap().unwrap().expose() == secret);
    let wrong_key = BookCredentials::new(store.into_book().unwrap(), open.owner, &[40; 32]).unwrap();
    assert!(wrong_key.load(&scope).is_err());
    let store = BookCredentials::new(wrong_key.into_book().unwrap(), open.owner, &device).unwrap();
    // Ciphertext corruption must fail authentication without exposing plaintext.
    let mut book = store.into_book().unwrap();
    book.write(|tx| {
        let mut blob: Vec<u8> = tx.query_row("SELECT cipher_blob FROM plugin_credential", [], |r| r.get(0))?;
        blob[0] ^= 1;
        tx.execute("UPDATE plugin_credential SET cipher_blob=?1", [blob])
    }).unwrap();
    let store = BookCredentials::new(book, open.owner, &device).unwrap();
    assert!(store.load(&scope).is_err());
    store.save(&scope, SecretBytes::new(secret.to_vec())).unwrap();
    let mut book = store.into_book().unwrap();
    // Move a valid envelope to another connection: both AEAD layers must bind identity.
    let another = Uuid::new_v4();
    book.write(|tx| tx.execute("UPDATE plugin_credential SET connection_id=?1", [another.to_string()])).unwrap();
    let store = BookCredentials::new(book, open.owner, &device).unwrap();
    let mut swapped = scope.clone();
    swapped.connection_id = another;
    assert!(store.load(&swapped).is_err());
    store.forget(&swapped).unwrap();
    assert!(store.load(&swapped).unwrap().is_none());
    store.save(&scope, SecretBytes::new(secret.to_vec())).unwrap();
    let mut book = store.into_book().unwrap();
    book.write(|tx| tx.execute("UPDATE plugin_credential SET plugin_id='simplefin'", [])).unwrap();
    let store = BookCredentials::new(book, open.owner, &device).unwrap();
    let mut swapped_plugin = scope.clone();
    swapped_plugin.plugin_id = "simplefin".into();
    assert!(store.load(&swapped_plugin).is_err());
    store.forget(&swapped_plugin).unwrap();
    store.save(&scope, SecretBytes::new(secret.to_vec())).unwrap();
    let mut book = store.into_book().unwrap();
    book.write(|tx| tx.execute("UPDATE plugin_credential SET company_id=?1", [other.to_db()])).unwrap();
    let store = BookCredentials::new(book, open.owner, &device).unwrap();
    let mut swapped_company = scope.clone();
    swapped_company.company_id = Uuid::parse_str(&other.to_db()).unwrap();
    assert!(store.load(&swapped_company).is_err());
    store.forget(&swapped_company).unwrap();
    let book = store.into_book().unwrap();
    drop(book);
    assert!(!std::fs::read(&path).unwrap().windows(secret.len()).any(|b| b == secret));
}

#[test]
fn credential_scope_metadata_is_validated_on_write_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metadata.fbbook");
    let key = MasterKey::new([41; 32]);
    let mut app = App::new();
    app.plugins = true;
    let company = app.create_book(&path, &key, "Metadata operator", "Synthetic metadata").unwrap().company_id;
    support::sign_in(&mut app);
    let open = app.open.take().unwrap();
    let scope = CredentialScope { company_id: company.to_db().parse().unwrap(), plugin_id: "openai".into(), connection_id: Uuid::new_v4() };
    let store = BookCredentials::new(open.book, open.owner, &key).unwrap();
    store.save(&scope, SecretBytes::new(b"Synthetic metadata token".to_vec())).unwrap();
    let mut book = store.into_book().unwrap();
    for (column, value) in [
        ("plugin_id", "openai\0hidden".to_owned()),
        ("company_id", "BAD-SYNTHETIC-COMPANY".to_owned()),
        ("connection_id", "FFFFFFFF-FFFF-4FFF-8FFF-FFFFFFFFFFFF".to_owned()),
    ] {
        let error = book.write(|tx| tx.execute(&format!("UPDATE plugin_credential SET {column}=?1"), [&value])).unwrap_err();
        assert_eq!(fb_db::constraint_of(&error).as_deref(), Some("fb_valid"));
    }
    // Simulate a nonconforming writer, restoring the exact trigger definition so
    // refusal on reopen must come from the value-check view, not schema damage.
    let conn = book.connection();
    let trigger: String = conn.query_row("SELECT sql FROM sqlite_schema WHERE name='fb_valid_plugin_credential_update'", [], |r| r.get(0)).unwrap();
    conn.execute_batch("DROP TRIGGER fb_valid_plugin_credential_update").unwrap();
    conn.execute("UPDATE plugin_credential SET plugin_id=?1", ["openai\0hidden"]).unwrap();
    conn.execute_batch(&trigger).unwrap();
    drop(book);
    match fb_core::Book::open(&path, &key) {
        Err(fb_db::DbError::Integrity(problems)) => assert!(problems.iter().any(|p| p.contains("invalid stored value in plugin_credential"))),
        _ => panic!("invalid credential scope metadata reopened"),
    }
}
