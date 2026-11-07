//! Integration regressions against the desktop/main writer and credential-open boundary.
use super::*;

#[test]
fn reopened_connection_refuses_every_writer_until_all_masks_are_restored() {
    let mut f = Fx::new();
    let secrets = SecretSet::from_credentials("SafeUserName123", "SafePassword123", Vec::<String>::new()).unwrap();
    let second = connect(&mut f.book, f.u, f.c, &json!("Second connection"), "fixture.test", &secrets).unwrap();
    let second = second["id"].as_str().unwrap().to_owned();
    f.book = Book::open(&f.path, &[91; 32]).unwrap();
    let before = fb_db::test_support::snapshot(f.book.connection()).unwrap();
    let refuse = |f: &mut Fx| {
        let mut called = false;
        let error = f.book.write(|_| {
            called = true;
            Ok::<_, FbError>(())
        }).unwrap_err();
        assert!(!called, "even a raw central writer callback must not enter");
        assert!(error.message().contains("Restore bank-feed connection credentials"));
        assert!(services::edit_company(&mut f.book, f.u, f.c, "SafePassword123", "USD").is_err());
    };
    refuse(&mut f);
    // A bank-feed call with a SecretSet must not stand in for the host's per-connection restore.
    assert!(record_sync_failure(&mut f.book, f.u, f.c, &f.conn.clone(), NOW, "SafePassword123", false, &secrets).is_err());
    assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(), before);
    restore_connection_credentials(&mut f.book, f.u, f.c, &f.conn.clone(), "https://SafeUserName123:SafePassword123@fixture.test/access", Vec::<String>::new()).unwrap();
    refuse(&mut f);
    restore_connection_credentials(&mut f.book, f.u, f.c, &second, "https://SafeUserName123:SafePassword123@fixture.test/access", Vec::<String>::new()).unwrap();
    services::edit_company(&mut f.book, f.u, f.c, "SafePassword123", "USD").unwrap();
    let stored = format!("{:?}", fb_db::test_support::snapshot(f.book.connection()).unwrap());
    assert!(!stored.contains("SafePassword123"));
    assert!(f.book.check().unwrap().is_empty());
    f.reopen_clean();
    r5_finish(&mut f);
}
