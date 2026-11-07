//! Public credential admission, stored-credential quarantine and post-command writer coverage.
//! Older adversarial suites use internal masking-only constructors deliberately: their credentials
//! are now refused at admission, but must still exercise final masking for pre-existing books.
use super::*;

#[test]
fn fix10_public_credential_alphabet_and_minimum() {
    let user = "SafeUserName123";
    let pass = "SafePassword123";
    for c in CREDENTIAL_ALPHABET.chars() {
        let value = format!("SafeComponent123{c}");
        assert!(SecretSet::new([&value]).is_ok());
        assert!(SecretSet::from_credentials(&value, pass, [&value]).is_ok());
        let encoded: String = value.bytes().map(|b| format!("%{b:02X}")).collect();
        assert!(SecretSet::from_access_url(&format!("https://{encoded}:{encoded}@fixture.test/access"), [&value]).is_ok());
    }
    for c in (0..=127).filter_map(char::from_u32).chain(['é', '中', '😀', '\u{a0}', '\u{2003}', '\u{200b}', '\u{2028}', '\u{feff}']) {
        if CREDENTIAL_ALPHABET.contains(c) { continue; }
        let value = format!("SafeComponent123{c}");
        for result in [
            SecretSet::new([&value]),
            SecretSet::from_credentials(&value, pass, Vec::<String>::new()),
            SecretSet::from_credentials(user, &value, Vec::<String>::new()),
            SecretSet::from_credentials(user, pass, [&value]),
        ] { assert_eq!(result.unwrap_err().message(), UNSAFE_CREDENTIAL); }
        let encoded: String = value.bytes().map(|b| format!("%{b:02X}")).collect();
        for url in [format!("https://{encoded}:{pass}@fixture.test/access"), format!("https://{user}:{encoded}@fixture.test/access")] {
            assert_eq!(SecretSet::from_access_url(&url, Vec::<String>::new()).unwrap_err().message(), UNSAFE_CREDENTIAL);
        }
    }
    for n in 0..12 {
        assert_eq!(SecretSet::new(["A".repeat(n)]).unwrap_err().message(), TOO_SHORT);
    }
    assert!(SecretSet::new(["A".repeat(12)]).is_ok());
    // A typed escape is not silently decoded into an accepted credential.
    assert_eq!(SecretSet::new(["SafeComponent%41"]).unwrap_err().message(), UNSAFE_CREDENTIAL);
}

#[test]
fn fix10_all_r10_join_credentials_are_refused_before_connect() {
    let mut f = Fx::new();
    let before = fb_db::test_support::snapshot(f.book.connection()).unwrap();
    for source in [include_str!("r10_joins.json"), include_str!("r10_provider.json")] {
        let cases: Vec<Value> = serde_json::from_str(source).unwrap();
        for case in cases {
            assert_eq!(SecretSet::from_credentials(case["user"].as_str().unwrap(), case["pass"].as_str().unwrap(), Vec::<String>::new()).unwrap_err().message(), UNSAFE_CREDENTIAL);
        }
    }
    assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(), before);
    r5_finish(&mut f);
}

#[test]
fn fix10_existing_bad_credential_is_flagged_preserved_and_never_used() {
    let mut f = Fx::new();
    f.connected_books();
    let rows = f.count("SELECT count(*) FROM bankfeeds_transaction");
    let bad_url = "https://SafeUserName123:OldPassword%22WithQuotes@fixture.test/access";
    assert_eq!(restore_connection_credentials(&mut f.book, f.u, f.c, &f.conn.clone(), bad_url, Vec::<String>::new()).unwrap_err().message(), UNSAFE_CREDENTIAL);
    let status = f.book.read(|q| connection_row(q, f.c, &f.conn)).unwrap();
    assert_eq!(status["needs_reconnect"], true);
    assert_eq!(status["active"], true);
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_transaction"), rows);
    assert_eq!(begin_sync(&mut f.book, f.u, f.c, &f.conn.clone(), NOW, 0, true).unwrap_err().message(), UNSAFE_CREDENTIAL);
    assert_eq!(store_pull(&mut f.book, f.u, f.c, &f.conn.clone(), &demo(), NOW, false, None, &SecretSet::none()).unwrap_err().message(), UNSAFE_CREDENTIAL);
    assert_eq!(stage_bank_transactions(&mut f.book, f.u, f.c, &f.conn.clone(), &[], &SecretSet::none()).unwrap_err().message(), UNSAFE_CREDENTIAL);
    // Reopening keeps the flag. Supplying a replacement credential does not silently reactivate
    // the old connection: a reconnect creates a separately audited connection.
    f.book = Book::open(&f.path, &[91;32]).unwrap();
    assert_eq!(restore_connection_credentials(&mut f.book, f.u, f.c, &f.conn.clone(), "https://SafeUserName123:SafePassword123@fixture.test/access", Vec::<String>::new()).unwrap_err().message(), UNSAFE_CREDENTIAL);
    assert_eq!(f.count("SELECT count(*) FROM ledger_auditevent WHERE action='bankfeed.reconnect_required'"), 1);
    f.reopen_clean();
    r5_finish(&mut f);
}

#[test]
fn fix10_central_guard_masks_later_edit_revision_reverse_and_void() {
    let mut f = Fx::new();
    let secret = "PostCommandPassword123";
    let known = SecretSet::from_credentials("PostCommandUsername123", secret, Vec::<String>::new()).unwrap();
    // One feed command retains the policy. None of the following calls take a SecretSet.
    connect(&mut f.book, f.u, f.c, &json!("Another connection"), "fixture.test", &known).unwrap();
    services::edit_company(&mut f.book, f.u, f.c, secret, "USD").unwrap();
    let mut p = json!({"bank_id":f.acc("100"),"payee_kind":"OTHER","payee_name":"Ordinary vendor","date":"2026-10-01","check_number":"LATER-1","memo":secret,"lines":[{"tab":"EXPENSE","account_id":f.acc("500"),"description":"Expense","rate":"84.19"}]});
    p["request_key"] = json!(fb_ids::new_v7().to_string());
    let check = checks::save_check(&mut f.book, f.u, f.c, &p).unwrap();
    p["check_id"] = check["id"].clone();
    p["expected_revision"] = check["revision"].clone();
    p["memo"] = json!(format!("Revised {secret}"));
    let revised = checks::save_check(&mut f.book, f.u, f.c, &p).unwrap();
    let id = revised["id"].as_str().unwrap();
    let rev = revised["revision"].as_i64();
    let review = checks::review_check(&mut f.book, f.u, f.c, id, rev).unwrap();
    let recorded = checks::record_check(&mut f.book, f.u, f.c, id, rev, &review).unwrap();
    checks::void_check(&mut f.book, f.u, f.c, id, recorded["revision"].as_i64(), &json!("2026-10-30"), &json!(secret)).unwrap();
    let journal = f.journal("2026-10-01", "100", "410", "1.00");
    services::reverse_journal(&mut f.book, f.u, f.c, journal.parse().unwrap(), "2026-10-30", secret).unwrap();
    let stored = r9::snapshot_texts(&f);
    assert!(!stored.iter().any(|(_, t)| t.contains(secret)), "a post-command writer persisted the credential");
    for action in ["company.updated", "check.revised", "check.voided", "journal.reversed"] {
        assert!(stored.iter().any(|(_, t)| t == action), "missing lifecycle coverage: {action}");
    }
    f.reopen_clean();
    r5_finish(&mut f);
}

#[test]
fn fix10_restored_credentials_protect_other_connections_and_later_writers() {
    let mut f = Fx::new();
    for password in ["FirstBookPassword123", "SecondBookPassword123"] {
        restore_connection_credentials(&mut f.book, f.u, f.c, &f.conn.clone(), &format!("https://SafeUserName123:{password}@fixture.test/access"), Vec::<String>::new()).unwrap();
    }
    services::edit_company(&mut f.book, f.u, f.c, "FirstBookPassword123 SecondBookPassword123", "USD").unwrap();
    assert!(r9::snapshot_texts(&f).iter().all(|(_, t)| !t.contains("FirstBookPassword123") && !t.contains("SecondBookPassword123")));
    r5_finish(&mut f);
}
