use fb_plugin_api::{Date, DateWindow, PluginError};
use fb_plugin_simplefin::parse::*;
use serde_json::{json, Value};

fn account() -> Value {
    json!({"id":"A1", "currency":"USD", "name":"Checking", "balance":"12480.55", "available-balance":"12380.55",
        "balance-date":978366153, "org":{"domain":"bank.example", "name":"Demo Bank"},
        "transactions":[{"id":"T1", "posted":1792800000, "transacted_at":1792713600, "amount":"-42.75", "description":"FUEL STOP"}]})
}

#[test]
fn decimal_half_up_matches_python_without_binary_floats() {
    for (raw, cents) in [
        ("-33293.43", -3_329_343),
        ("1e2", 10000),
        ("0.125", 13),
        ("-0.125", -13),
        ("2.675", 268),
        ("1.005", 101),
        ("0.124", 12),
        ("-84.19", -8419),
        ("1e-9000000", 0),
        ("0e9000000", 0),
        (".005", 1),
        ("000.005", 1),
        ("1.234e2", 12340),
        ("1_000.00", 100000),
        ("+5.", 500),
        ("99999999999999.99", 9_999_999_999_999_999),
        ("99999999999999.989", 9_999_999_999_999_999),
    ] {
        assert_eq!(money(&json!(raw)), Ok(cents), "{raw}");
    }
    for raw in [
        "0.125",
        "1.005",
        "99999999999999.99",
        "-99999999999999.99",
        "2.675",
        "1e2",
    ] {
        // Exercise the wire boundary: a generic Value has already rounded large
        // numeric tokens, so exact amount normalization must precede that conversion.
        let input = format!(r#"{{"accounts":[{{"id":"A","currency":"USD","balance":{raw}}}]}}"#);
        let numeric = parse_account_set_json(input.as_bytes()).unwrap();
        assert_eq!(Ok(numeric.accounts[0].balance_cents), money(&json!(raw)));
    }
}

#[test]
fn decimal_rejects_nonfinite_missing_malformed_and_pre_round_overflow() {
    for (value, reason) in [
        (Value::Null, MoneyError::Missing),
        (json!(true), MoneyError::Missing),
        (json!("abc"), MoneyError::NotNumeric),
        (json!("NaN"), MoneyError::NotFinite),
        (json!("Infinity"), MoneyError::NotFinite),
        (json!("-Infinity"), MoneyError::NotFinite),
        (json!("100000000000000.00"), MoneyError::TooLarge),
        (json!("99999999999999.991"), MoneyError::TooLarge),
        (json!("-99999999999999.999"), MoneyError::TooLarge),
        (
            json!("1e999999999999999999999999999999999"),
            MoneyError::TooLarge,
        ),
        (json!({}), MoneyError::NotNumeric),
    ] {
        assert_eq!(money(&value), Err(reason));
    }
    for raw in [
        "", ".", "-", "1e", "1e1e2", "1.2.3", "1 2", "1e--2", "1e+", "--2",
    ] {
        assert_eq!(money(&json!(raw)), Err(MoneyError::NotNumeric));
    }
}

#[test]
fn epoch_boundaries_bool_and_non_integer_strings_match_python() {
    for value in [json!(978366153), json!("978366153"), json!(978366153.8)] {
        assert_eq!(epoch(&value), Some(978366153));
    }
    assert_eq!(epoch(&json!(4102444800u64)), Some(4102444800));
    for value in [
        json!(0),
        json!(-5),
        Value::Null,
        json!("x"),
        json!(true),
        json!(99999999999u64),
        json!("1.5"),
    ] {
        assert_eq!(epoch(&value), None);
    }
}

#[test]
fn malformed_accounts_and_transactions_are_skipped_without_inventing_values() {
    let mut good = account();
    good["transactions"].as_array_mut().unwrap().extend([
        json!({"posted":1, "amount":"1"}),
        json!({"id":"huge", "amount":"1e20"}),
        json!({"id":"nan", "amount":"NaN"}),
        json!("junk"),
        json!({"id":"pending", "posted":0, "amount":"1.00"}),
    ]);
    let parsed = parse_account_set(&json!({"accounts":[good,
        {"currency":"USD", "balance":"1"}, {"id":"no-currency", "balance":"1"},
        {"id":"bad-balance", "currency":"USD", "balance":"lots"}, "junk"]}))
    .unwrap();
    assert_eq!(parsed.accounts.len(), 1);
    assert_eq!(
        parsed.skipped,
        [
            SkipReason::AccountWithoutId,
            SkipReason::AccountWithoutCurrency,
            SkipReason::AccountBalance(MoneyError::NotNumeric),
            SkipReason::AccountNotObject
        ]
    );
    let a = &parsed.accounts[0];
    assert_eq!(a.organization_name, "Demo Bank");
    assert_eq!(a.connection_key, "bank.example");
    assert_eq!(a.balance_cents, 1248055);
    assert_eq!(a.available_cents, Some(1238055));
    assert_eq!(a.balance_epoch, Some(978366153));
    assert_eq!(
        a.skipped,
        [
            SkipReason::TransactionWithoutId,
            SkipReason::TransactionAmount(MoneyError::TooLarge),
            SkipReason::TransactionAmount(MoneyError::NotFinite),
            SkipReason::TransactionNotObject
        ]
    );
    // Python parse_account_set's reason strings (they reach the sync summary and its audit).
    assert_eq!(
        parsed.skipped_text,
        ["account without an id", "account no-currency: no currency", "account bad-balance: balance amount not numeric", "account entry is not an object"]
    );
    assert_eq!(
        a.skipped_text,
        ["transaction without an id", "transaction huge: amount too large", "transaction nan: amount not finite", "transaction entry is not an object"]
    );
    assert_eq!(a.transactions.len(), 2);
    assert!(a.transactions[1].pending);
    assert_eq!(a.transactions[1].posted_epoch, None);
}

#[test]
fn v1_errors_and_org_and_v2_connections_and_errlist_are_normalized() {
    let v1 = parse_account_set(&json!({"accounts":[account()],"errors":["Bank login expired", 2]}))
        .unwrap();
    assert_eq!(v1.errors.len(), 1);
    assert_eq!(v1.errors[0].message, "Bank login expired");
    assert_eq!(v1.errors[0].code, "");
    let mut a = account();
    a["conn_id"] = json!(42);
    let v2 = parse_account_set(&json!({"accounts":[a],"connections":[
        {"conn_id":"42", "org_name":"Earlier"}, {"conn_id":42,"org_name":"New Bank"}],
        "errlist":["junk",{"code":"gen.auth", "msg":"<b>Expired</b>", "conn_id":42, "account_id":"A1"}],
        "errors":["ignored older warning"]})).unwrap();
    assert_eq!(v2.accounts[0].connection_key, "42");
    assert_eq!(v2.accounts[0].organization_name, "New Bank");
    assert_eq!(v2.errors[0].code, "gen.auth");
    assert_eq!(v2.errors[0].message, "Expired");
    assert_eq!(v2.errors[0].connection_id, "42");
    assert_eq!(v2.errors[0].account_id, "A1");
}

#[test]
fn missing_account_list_is_not_an_empty_success() {
    for value in [
        json!({}),
        json!({"accounts":null}),
        json!({"accounts":{}}),
        json!([]),
        json!("text"),
    ] {
        assert!(matches!(
            parse_account_set(&value),
            Err(PluginError::InvalidResponse)
        ));
    }
    assert!(parse_account_set(&json!({"accounts":[]}))
        .unwrap()
        .accounts
        .is_empty());
}

#[test]
fn optional_fields_pending_and_bad_transaction_container_are_preserved() {
    let mut a = account();
    a["name"] = Value::Null;
    a["available-balance"] = json!("NaN");
    a["balance-date"] = json!(true);
    a["transactions"] = json!({"oops":1});
    let parsed = parse_account_set(&json!({"accounts":[a]})).unwrap();
    let a = &parsed.accounts[0];
    assert_eq!(a.name, "A1");
    assert_eq!(a.available_cents, None);
    assert_eq!(a.balance_epoch, None);
    assert_eq!(a.skipped, [SkipReason::TransactionsNotList]);
    for pending in [json!(true), json!(false), json!("true"), json!(1)] {
        let mut a = account();
        a["available-balance"] = Value::Null;
        a["transactions"][0]["pending"] = pending.clone();
        let p = parse_account_set(&json!({"accounts":[a]})).unwrap();
        assert_eq!(p.accounts[0].available_cents, Some(1248055));
        assert_eq!(
            p.accounts[0].transactions[0].pending,
            pending == json!(true)
        );
    }
}

#[test]
fn sanitization_covers_at_sign_passwords_basic_headers_tags_and_controls() {
    assert_eq!(
        redact("see https://user:pw@host.example/x and http://a:b@h/y"),
        "see https://***@host.example/x and http://***@h/y"
    );
    assert_eq!(
        sanitize("<b>Auth</b> failed\0 for https://u:p@h/", 100),
        "Auth failed  for https://***@h/"
    );
    for input in [
        "https://user:p@ss@bank.example/simplefin",
        "https://user:p%40ss@bank.example/simplefin",
        "https://us@er:p@ss@word@bank.example:8443/simplefin",
        "https://u:pa/ss@bank.example/simplefin",
    ] {
        let out = redact(input);
        for secret in ["user", "word", "ss@", "p%40ss"] {
            assert!(!out.contains(secret));
        }
        assert!(out.starts_with("https://***@bank.example"));
    }
    let out = redact("Authorization: Basic ZGVtby...dA== authorization=basic ZGVtbzpzZWNyZXQ= basic YWJjZGVmZ2hpams=");
    for value in ["ZGVtby", "ZGVtbzpzZWNyZXQ", "YWJjZGVmZ2hpams"] {
        assert!(!out.contains(value));
    }
    assert_eq!(redact("a basic understanding"), "a basic understanding");
    assert_eq!(
        redact("plain https://bank.example/x text"),
        "plain https://bank.example/x text"
    );
    assert_eq!(sanitize(&"é".repeat(1000), 300).chars().count(), 300);
}

#[test]
fn provider_fields_are_sanitized_bounded_and_do_not_implement_debug() {
    let mut a = account();
    a["name"] = json!("x".repeat(130));
    a["transactions"][0]["description"] = json!(format!(
        "<b>test</b> https://user:p@ss@bank.example/x {}",
        "é".repeat(600)
    ));
    let p = parse_account_set(&json!({"accounts":[a]})).unwrap();
    assert_eq!(p.accounts[0].name.len(), 120);
    assert_eq!(
        p.accounts[0].transactions[0].description.chars().count(),
        500
    );
    assert!(!p.accounts[0].transactions[0].description.contains("user"));
}

#[test]
fn epoch_to_book_timezone_matches_python_instead_of_utc_date() {
    let mut a = account();
    a["transactions"][0]["posted"] = json!(1792800000); // 2026-10-24 UTC, prior evening in NY
    let p = parse_account_set(&json!({"accounts":[a]})).unwrap();
    let ny = p.feed_transactions(chrono_tz::America::New_York);
    let utc = p.feed_transactions(chrono_tz::UTC);
    assert_eq!(ny[0].posted_on, Some(Date::new(2026, 10, 23).unwrap()));
    assert_eq!(utc[0].posted_on, Some(Date::new(2026, 10, 24).unwrap()));
    assert_eq!(ny[0].amount.cents, -4275);
    assert_eq!(ny[0].account_remote_id, "A1");
    assert_eq!(ny[0].amount.currency, "USD");
}

#[test]
fn date_windows_are_inclusive_and_dst_aware() {
    for (month, day, seconds) in [(3, 8, 23 * 3600), (11, 1, 25 * 3600), (10, 30, 24 * 3600)] {
        let d = Date::new(2026, month, day).unwrap();
        let (start, end) = window_epochs(
            DateWindow { start: d, end: d },
            chrono_tz::America::New_York,
        )
        .unwrap();
        assert_eq!(end - start + 1, seconds);
    }
    assert!(window_epochs(
        DateWindow {
            start: Date::new(2026, 10, 30).unwrap(),
            end: Date::new(2026, 10, 29).unwrap()
        },
        chrono_tz::UTC
    )
    .is_err());
    assert!(window_epochs(
        DateWindow {
            start: Date::new(1960, 1, 1).unwrap(),
            end: Date::new(1960, 1, 1).unwrap()
        },
        chrono_tz::UTC
    )
    .is_err());
}

#[test]
fn wire_amount_forms_preserve_exact_digits_without_global_precision() {
    for (token, expected) in [
        (r#""1.005""#, Ok(101)),
        ("1.005", Ok(101)),
        ("-1.005", Ok(-101)),
        ("99999999999999.99", Ok(9_999_999_999_999_999)),
        ("0.00499999999999999999999999999999999", Ok(0)),
        ("1.005e2", Ok(10050)),
        ("1e-9000000", Ok(0)),
        ("1e9000000", Err(MoneyError::TooLarge)),
        ("99999999999999.991", Err(MoneyError::TooLarge)),
        (r#""NaN""#, Err(MoneyError::NotFinite)),
        ("true", Err(MoneyError::Missing)),
        ("null", Err(MoneyError::Missing)),
        ("{}", Err(MoneyError::NotNumeric)),
    ] {
        let raw = format!(r#"{{"accounts":[{{"id":"A","currency":"USD","balance":{token}}}]}}"#);
        let parsed = parse_account_set_json(raw.as_bytes()).unwrap();
        match expected {
            Ok(cents) => assert_eq!(parsed.accounts[0].balance_cents, cents, "{token}"),
            Err(reason) => assert_eq!(
                parsed.skipped,
                [SkipReason::AccountBalance(reason)],
                "{token}"
            ),
        }
    }
    let raw = br#"{"accounts":[{"id":"A","currency":"USD","balance":"1.005",
        "available-balance":99999999999999.99,"transactions":[
            {"id":"T1","amount":0.00499999999999999999999999999999999},
            {"id":"T2","amount":"-1.005"},
            {"id":"T3","amount":1e9000000},
            {"id":"T4","amount":1e-9000000}]}]}"#;
    let parsed = parse_account_set_json(raw).unwrap();
    let account = &parsed.accounts[0];
    assert_eq!(account.balance_cents, 101);
    assert_eq!(account.available_cents, Some(9_999_999_999_999_999));
    assert_eq!(
        account
            .transactions
            .iter()
            .map(|t| t.cents)
            .collect::<Vec<_>>(),
        [0, -101, 0]
    );
    assert_eq!(
        account.skipped,
        [SkipReason::TransactionAmount(MoneyError::TooLarge)]
    );
    for raw in [b"".as_slice(), b"{} {}", b"{", b"[]", br#"{"accounts":{}}"#] {
        assert!(parse_account_set_json(raw).is_err());
    }
}

#[test]
fn local_date_is_the_display_zone_calendar_date_like_python_epoch_to_date() {
    let zone: chrono_tz::Tz = "America/New_York".parse().unwrap();
    // 2023-11-15T02:00:00Z is still 2023-11-14 in New York; the UTC date would be one day late.
    assert_eq!(local_date(Some(1_700_013_600), zone).as_deref(), Some("2023-11-14"));
    assert_eq!(local_date(Some(1_700_013_600), chrono_tz::UTC).as_deref(), Some("2023-11-15"));
    assert_eq!(local_date(None, zone), None);
}
