//! Fuzz audit 2026-11, area: SimpleFIN bank-feed JSON (`parse::{money, epoch, parse_account_set_json}`).
//!
//! Properties: `money(format(cents)) == cents` for every signed cent amount in range, with
//! ROUND_HALF_UP at the third decimal and the pre-rounding 99999999999999.99 limit; exponent
//! spellings equal their shifted decimals; a random Account Set round-trips every account, balance,
//! transaction and amount through the wire parser (amounts kept lexically, never via f64); and the
//! `connections` lookup is measured for its quadratic cost (ignored, see FUZZ-AUDIT-REPORT.md).
use fb_plugin_simplefin::parse::{MoneyError, connections_by_id, epoch, money, parse_account_set, parse_account_set_json, sanitize};
use proptest::prelude::*;
use serde_json::{Value, json};

const LIMIT: i64 = 9_999_999_999_999_999;

fn text(cents: i64) -> String {
    let abs = cents.unsigned_abs();
    format!("{}{}.{:02}", if cents < 0 { "-" } else { "" }, abs / 100, abs % 100)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, .. ProptestConfig::default() })]

    #[test]
    fn parse_format_round_trip_signed(cents in -LIMIT..=LIMIT) {
        let s = text(cents);
        prop_assert_eq!(money(&json!(s)), Ok(cents), "{}", s);
        prop_assert_eq!(money(&json!(format!(" {s} "))), Ok(cents), "trimmed");
        if cents >= 0 { prop_assert_eq!(money(&json!(format!("+{s}"))), Ok(cents), "plus sign"); }
        prop_assert_eq!(money(&json!(s.replace('.', "_."))), Ok(cents), "underscores");
        if cents.abs() < 1_000_000_000_000_000 {
            // Up to 15 significant digits an f64-backed JSON number reads the same as its text (beyond that
            // `Value::Number` itself is lossy; the wire path keeps amounts lexical, see account_set_round_trips).
            let n: Value = serde_json::from_str(&s).unwrap();
            prop_assert_eq!(money(&n), Ok(cents), "json number {}", n);
        }
    }

    #[test]
    fn third_decimal_rounds_half_up_away_from_zero(cents in -LIMIT / 10..=LIMIT / 10, third in 0u8..10, tail in "[0-9]{0,5}") {
        let s = format!("{}{third}{tail}", text(cents));
        let step = if third >= 5 { 1 } else { 0 };
        let expected = if cents < 0 { cents - step } else if cents == 0 && s.starts_with('-') { -step } else { cents + step };
        prop_assert_eq!(money(&json!(s)), Ok(expected), "{}", s);
    }

    #[test]
    fn the_limit_applies_before_rounding(frac in "[0-9]{2}", extra in "[0-9]{1,4}") {
        let s = format!("99999999999999.{frac}{extra}");
        let over = frac == "99" && extra.bytes().any(|b| b != b'0');
        let r = money(&json!(s));
        if over {
            prop_assert_eq!(r, Err(MoneyError::TooLarge), "{}", s);
        } else {
            prop_assert!(r.is_ok(), "{}: {:?}", s, r);
        }
        prop_assert_eq!(money(&json!("100000000000000")), Err(MoneyError::TooLarge));
        prop_assert_eq!(money(&json!("-100000000000000")), Err(MoneyError::TooLarge));
        prop_assert_eq!(money(&json!("1e14")), Err(MoneyError::TooLarge));
        // 0.999999999999999999e14 = 99999999999999.9999 exceeds the limit BEFORE rounding, as documented.
        prop_assert_eq!(money(&json!("0.999999999999999999e14")), Err(MoneyError::TooLarge));
        prop_assert_eq!(money(&json!("0.9999999999999999e14")), Ok(LIMIT));
    }

    #[test]
    fn exponent_spellings_match_shifted_decimals(mantissa in 1i64..1_000_000, exp in -10i32..8) {
        let d = rust_decimal_like(mantissa, exp);
        prop_assert_eq!(money(&json!(format!("{mantissa}e{exp}"))), Ok(d));
        prop_assert_eq!(money(&json!(format!("{mantissa}E{exp:+}"))), Ok(d));
        prop_assert_eq!(money(&json!(format!("-{mantissa}e{exp}"))), Ok(-d));
    }

    #[test]
    fn refusals(s in prop_oneof![Just("NaN"), Just("Infinity"), Just("-inf"), Just("snan12"), Just("1e"), Just("e5"), Just("1.2.3"), Just("1e5e5"), Just("1,000"), Just("$5"), Just("٥"), Just(""), Just("+"), Just("-"), Just("."), Just("1 000")]) {
        prop_assert!(money(&json!(s)).is_err(), "{:?}", s);
        prop_assert_eq!(money(&json!(null)), Err(MoneyError::Missing));
        prop_assert_eq!(money(&json!(true)), Err(MoneyError::Missing));
        prop_assert_eq!(money(&json!([1])), Err(MoneyError::NotNumeric));
    }

    #[test]
    fn epoch_accepts_the_documented_window(n in any::<i64>()) {
        let expected = (n > 0 && n <= 4_102_444_800).then_some(n);
        prop_assert_eq!(epoch(&json!(n)), expected);
        prop_assert_eq!(epoch(&json!(n.to_string())), expected);
        prop_assert_eq!(epoch(&json!(format!(" {n} "))), expected);
        if let Some(e) = expected {
            prop_assert_eq!(epoch(&json!(e as f64 + 0.5)), Some(e), "fractional seconds truncate");
        }
    }
}

fn rust_decimal_like(mantissa: i64, exp: i32) -> i64 {
    // mantissa * 10^exp in cents, half up (mantissa < 1e6, exp in -10..8 keeps this in range).
    let scaled = i128::from(mantissa) * 100;
    if exp >= 0 {
        (scaled * 10i128.pow(exp as u32)) as i64
    } else {
        let div = 10i128.pow(exp.unsigned_abs());
        ((scaled + div / 2) / div) as i64
    }
}

// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Tx {
    id: String,
    amount: i64,
    posted: Option<i64>,
    description: String,
    pending: bool,
}
#[derive(Clone, Debug)]
struct Acct {
    id: String,
    name: String,
    currency: String,
    balance: i64,
    available: Option<i64>,
    balance_date: Option<i64>,
    txs: Vec<Tx>,
}

fn tx() -> impl Strategy<Value = Tx> {
    ("[a-zA-Z0-9_-]{1,30}", -LIMIT..=LIMIT, prop::option::of(1i64..4_102_444_800), "[^<>\\p{Cc}]{0,80}", any::<bool>())
        .prop_map(|(id, amount, posted, description, pending)| Tx { id, amount, posted, description: description.trim().to_owned(), pending })
}
fn acct() -> impl Strategy<Value = Acct> {
    ("[a-zA-Z0-9_-]{1,30}", "[A-Za-z ]{0,40}", "[A-Z]{3}", -LIMIT..=LIMIT, prop::option::of(-LIMIT..=LIMIT), prop::option::of(1i64..4_102_444_800), prop::collection::vec(tx(), 0..6))
        .prop_map(|(id, name, currency, balance, available, balance_date, txs)| Acct { id, name: name.trim().to_owned(), currency, balance, available, balance_date, txs })
}

/// Build the body text directly: a JSON number must reach the parser lexically (`RawValue`), never
/// through serde_json's f64 `Value::Number`, which cannot hold 70371781450727.99 exactly.
fn wire(accounts: &[Acct], pretty_numbers: bool) -> Vec<u8> {
    let amount = |c: i64| if pretty_numbers { json!(format!("\u{0}NUM:{}", text(c))) } else { json!(text(c)) };
    let body = json!({
        "errors": [],
        "accounts": accounts.iter().map(|a| json!({
            "org": {"domain": "bank.example", "name": "Example Bank", "sfin-url": "https://bridge.simplefin.org/simplefin"},
            "id": a.id, "name": a.name, "currency": a.currency,
            "balance": amount(a.balance),
            "available-balance": a.available.map(amount),
            "balance-date": a.balance_date,
            "transactions": a.txs.iter().map(|t| json!({
                "id": t.id, "posted": t.posted, "amount": amount(t.amount), "description": t.description, "pending": t.pending,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    });
    let mut out = serde_json::to_string(&body).unwrap();
    while let Some(start) = out.find("\"\\u0000NUM:") {
        let end = start + 1 + out[start + 1..].find('"').unwrap();
        let literal = out[start + "\"\\u0000NUM:".len()..end].to_owned();
        out.replace_range(start..=end, &literal);
    }
    out.into_bytes()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 300, .. ProptestConfig::default() })]

    /// Every account, amount and transaction of a random Account Set survives the wire parser exactly,
    /// whether amounts are sent as strings or as JSON numbers.
    #[test]
    fn account_set_round_trips(accounts in prop::collection::vec(acct(), 0..5), numbers in any::<bool>()) {
        // Distinct ids, so each parsed account pairs with exactly one generated one.
        let mut seen = std::collections::HashSet::new();
        let accounts: Vec<Acct> = accounts.into_iter().filter(|a| seen.insert(a.id.clone())).collect();
        let set = parse_account_set_json(&wire(&accounts, numbers)).map_err(|e| format!("{e:?}")).unwrap();
        prop_assert!(set.errors.is_empty());
        prop_assert!(set.skipped.is_empty(), "skipped: {:?}", set.skipped);
        let expected: Vec<&Acct> = accounts.iter().collect();
        prop_assert_eq!(set.accounts.len(), expected.len());
        for (parsed, want) in set.accounts.iter().zip(expected) {
            prop_assert_eq!(&parsed.remote_id, &want.id);
            prop_assert_eq!(parsed.balance_cents, want.balance, "balance of {}", want.id);
            prop_assert_eq!(parsed.available_cents, Some(want.available.unwrap_or(want.balance)), "available of {}", want.id);
            prop_assert_eq!(parsed.balance_epoch, want.balance_date);
            prop_assert_eq!(&parsed.currency, &want.currency);
            prop_assert_eq!(&parsed.name, if want.name.is_empty() { &want.id } else { &want.name });
            prop_assert!(parsed.skipped.is_empty(), "transactions skipped: {:?}", parsed.skipped);
            prop_assert_eq!(parsed.transactions.len(), want.txs.len());
            for (p, w) in parsed.transactions.iter().zip(&want.txs) {
                prop_assert_eq!(&p.remote_id, &w.id);
                prop_assert_eq!(p.cents, w.amount, "amount of {}", w.id);
                prop_assert_eq!(p.posted_epoch, w.posted);
                prop_assert_eq!(p.pending, w.pending || w.posted.is_none());
                prop_assert_eq!(&p.description, &w.description);
            }
        }
        let feed = set.feed_transactions(chrono_tz::UTC);
        prop_assert_eq!(feed.len(), accounts.iter().map(|a| a.txs.len()).sum::<usize>());
    }
}

/// FINDING F-01 (hang; fixed): `parse_account_set` scanned every `connections` entry for every account,
/// running the regex-based `text()` sanitizer on both sides of each comparison, so a response with N
/// accounts and N connections cost O(N^2) regex passes (6,000 x 6,000 took 220 s). The connections are
/// now indexed once per response (`parse::connections_by_id`).
#[test]
fn connections_lookup_is_linear() {
    let n = 6_000;
    let accounts: Vec<Value> = (0..n).map(|i| json!({"id": format!("a{i}"), "currency": "USD", "balance": "1.00", "conn_id": format!("c{i}")})).collect();
    let connections: Vec<Value> = (0..n).map(|i| json!({"conn_id": format!("z{i}")})).collect();
    let body = serde_json::to_vec(&json!({"accounts": accounts, "connections": connections})).unwrap();
    let started = std::time::Instant::now();
    let set = parse_account_set_json(&body).unwrap();
    let took = started.elapsed();
    eprintln!("{n} accounts x {n} connections ({} KiB) took {took:?}", body.len() / 1024);
    assert_eq!(set.accounts.len(), n);
    assert!(took.as_secs_f64() < 1.0, "quadratic connections scan: {n}x{n} took {took:?} (body {} KiB)", body.len() / 1024);
}

/// FINDING F-04 (data correctness / secret hygiene; fixed): `parse::redact` ran its credential regexes
/// on the raw provider text and only afterwards turned control characters into spaces. A control
/// character inside the keyword or the separator (`Basic\0 <token>`, `Auth\0orization: Basic <token>`)
/// defeated `BASIC_HEADER` and `BASIC`, so the token survived into the sanitized message while the
/// final control-to-space pass made the output read like an ordinary header. The fuzz run found the
/// shape in two seconds (fuzz/artifacts/simplefin_scalars/crash-004c581b...). The patterns now see
/// the text with hidden characters (`parse::is_hidden`) folded out.
#[test]
fn redact_masks_basic_credentials_even_when_a_control_character_splits_the_keyword() {
    use fb_plugin_simplefin::parse::redact;
    let token = "dXNlcjpwYXNzd29yZDEyMw==";
    assert!(!redact(&format!("Authorization: Basic {token}")).contains(token), "plain form is redacted");
    for text in [
        format!("Authorization: Basic\u{0} {token}"),
        format!("Auth\u{0}orization: Basic {token}"),
        format!("Authorization:\u{1} Basic {token}"),
        format!("Basic\u{7f} {token}"),
    ] {
        let out = redact(&text);
        assert!(!out.contains(token), "token survived redact on {text:?}: {out:?}");
    }
}

/// F-04 fix: the token goes however a hidden character (NUL, other C0 controls, DEL, zero-width
/// space / joiners / word joiner / BOM) or real whitespace (tab, CRLF, a folded header line) splits the
/// header name, the scheme, the separator or the token itself; text around it is kept as written.
#[test]
fn redact_masks_basic_credentials_split_by_controls_crlf_tabs_nul_and_zero_width() {
    use fb_plugin_simplefin::parse::{redact, sanitize};
    let token = "dXNlcjpwYXNzd29yZDEyMw==";
    let (head, tail) = token.split_at(9);
    let splitters = ["\u{0}", "\u{1}", "\u{8}", "\u{1b}", "\u{1f}", "\u{7f}", "\u{200b}", "\u{200c}", "\u{200d}", "\u{2060}", "\u{feff}"];
    let mut cases = vec![
        format!("Authorization: Basic {token}"),
        format!("Authorization:\tBasic\t{token}"),
        format!("Authorization: Basic\r\n {token}"),
        format!("Authorization:\r\n\tBasic {token}"),
        format!("GET / HTTP/1.1\r\nAuthorization: Basic {token}\r\nHost: bridge.example\r\n"),
        format!("basic\t{token}"),
    ];
    for x in splitters {
        cases.push(format!("Authorization: Basic{x} {token}"));
        cases.push(format!("Authorization: Basic{x}{token}"));
        cases.push(format!("Authorization: Basic {x}{token}"));
        cases.push(format!("Auth{x}orization: Basic {token}"));
        cases.push(format!("Authorization{x}: Basic {token}"));
        cases.push(format!("Authorization:{x} Ba{x}sic {token}"));
        cases.push(format!("Authorization: Basic {head}{x}{tail}"));
        cases.push(format!("Authorization: Basic {head}{x}{x}{tail}\r\nHost: h"));
        cases.push(format!("error Basic{x}{token} refused"));
        cases.push(format!("error{x}Basic {head}{x}{tail} refused"));
        cases.push(format!("Basic {}{x}{}", &token[..3], &token[3..]));
    }
    for text in cases {
        let once = redact(&text);
        for piece in [token, head, tail, &token[3..]] {
            assert!(!once.contains(piece), "{piece:?} survived redact on {text:?}: {once:?}");
        }
        assert!(once.contains("***"), "{text:?}: {once:?}");
        assert_eq!(redact(&once), once, "idempotent on {text:?}");
        assert!(!once.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}'), "controls still blanked: {once:?}");
        assert!(!sanitize(&text, 500).contains(tail), "sanitize on {text:?}");
    }
    assert_eq!(redact("Authorization: Basic\u{0}dXNlcjpw\r\nHost: h"), "Authorization: Basic ***  Host: h");
    assert_eq!(redact("no\u{200b}credential here\u{0}"), "no\u{200b}credential here ");
    assert_eq!(redact("a basic understanding"), "a basic understanding");
    assert_eq!(redact("a basic\u{0}understanding"), "a basic understanding");
    assert_eq!(redact("abasic dXNlcjpwYXNz1"), "abasic dXNlcjpwYXNz1", "\\b before basic still applies");
}

/// The pre-F-01 lookup, verbatim apart from `text()` (private in the crate, same body here): the last
/// object in `connections` with a non-null `conn_id` whose sanitized text equals the account's.
fn old_connection<'a>(data: &'a Value, raw: &Value) -> &'a Value {
    fn text(value: &Value, limit: usize) -> String {
        match value {
            Value::String(s) => sanitize(s, limit),
            Value::Number(n) => sanitize(&n.to_string(), limit),
            _ => String::new(),
        }
    }
    data["connections"]
        .as_array()
        .and_then(|cs| {
            cs.iter().rev().find(|c| {
                c.is_object()
                    && !c["conn_id"].is_null()
                    && text(&c["conn_id"], 200) == text(&raw["conn_id"], 200)
            })
        })
        .unwrap_or(&Value::Null)
}

/// Connection ids that collide after sanitizing (tags, controls, padding, numbers vs strings, empty
/// text from non-text ids) or differ only slightly.
fn conn_id() -> impl Strategy<Value = Option<Value>> {
    prop_oneof![
        Just(None),
        Just(Some(Value::Null)),
        Just(Some(json!("c1"))),
        Just(Some(json!(" c1 "))),
        Just(Some(json!("<b>c1</b>"))),
        Just(Some(json!("c1\u{0}"))),
        Just(Some(json!("C1"))),
        Just(Some(json!("c2"))),
        Just(Some(json!(""))),
        Just(Some(json!("   "))),
        Just(Some(json!(1))),
        Just(Some(json!("1"))),
        Just(Some(json!(1.5))),
        Just(Some(json!(true))),
        Just(Some(json!({"x": 1}))),
        Just(Some(json!(["c1"]))),
        Just(Some(json!("Basic dXNlcjpwYXNz1"))),
        Just(Some(json!("Basic\u{0}dXNlcjpwYXNz1"))),
        "[a-c1 <>\\x00]{0,4}".prop_map(|s| Some(json!(s))),
    ]
}

fn with_conn_id(mut v: Value, id: Option<Value>) -> Value {
    if let (Some(id), Some(map)) = (id, v.as_object_mut()) {
        map.insert("conn_id".into(), id);
    }
    v
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, .. ProptestConfig::default() })]

    /// F-01 parity: the indexed lookup picks the very same connection object as the old scan, for every
    /// account, on random collisions, duplicates, non-objects and missing `connections`.
    #[test]
    fn connection_index_picks_what_the_old_scan_picked(
        connections in prop::option::of(prop::collection::vec((conn_id(), any::<bool>()), 0..8)),
        accounts in prop::collection::vec(conn_id(), 0..8),
    ) {
        let mut data = json!({"accounts": accounts.iter().enumerate().map(|(i, id)| with_conn_id(json!({"id": format!("a{i}"), "currency": "USD", "balance": "1.00"}), id.clone())).collect::<Vec<_>>()});
        if let Some(connections) = connections {
            data["connections"] = json!(connections.into_iter().enumerate().map(|(i, (id, object))| if object { with_conn_id(json!({"n": i, "org_name": format!("Org {i}")}), id) } else { json!(i) }).collect::<Vec<_>>());
        }
        let index = connections_by_id(&data);
        for raw in data["accounts"].as_array().unwrap() {
            let key = match &raw["conn_id"] {
                Value::String(s) => sanitize(s, 200),
                Value::Number(n) => sanitize(&n.to_string(), 200),
                _ => String::new(),
            };
            let new = index.get(&key).copied().unwrap_or(&Value::Null);
            let old = old_connection(&data, raw);
            prop_assert!(std::ptr::eq(new, old) || (new.is_null() && old.is_null()), "account {:?}: new {:?}, old {:?}", raw, new, old);
        }
        // And end to end: the organization name the parser reports comes from that connection.
        let set = parse_account_set(&data).unwrap();
        for (parsed, raw) in set.accounts.iter().zip(data["accounts"].as_array().unwrap()) {
            let old = old_connection(&data, raw);
            if let Some(name) = old["org_name"].as_str() {
                prop_assert_eq!(&parsed.organization_name, name);
            } else {
                prop_assert_eq!(&parsed.organization_name, "");
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, .. ProptestConfig::default() })]

    /// F-04 fix: `redact` is idempotent and leaves no control character, on provider-like text full of
    /// credential keywords, tokens, controls, zero-width characters and URLs.
    #[test]
    fn redact_is_idempotent_with_hidden_characters(parts in prop::collection::vec(prop_oneof![
        3 => "[A-Za-z0-9 ]{1,10}",
        2 => Just("Basic ".to_string()),
        2 => Just("Authorization: ".to_string()),
        2 => Just("dXNlcjpwYXNzd29yZDEyMw==".to_string()),
        1 => Just("https://user:pw@bank.example/p?x=1".to_string()),
        1 => Just("\r\n".to_string()),
        1 => Just("\t".to_string()),
        2 => "[\\x00-\\x1f\\x7f\u{200b}\u{200c}\u{200d}\u{2060}\u{feff}]",
    ], 0..10)) {
        use fb_plugin_simplefin::parse::redact;
        let text = parts.concat();
        let once = redact(&text);
        prop_assert_eq!(redact(&once), once.clone(), "not idempotent on {:?}", text);
        prop_assert!(!once.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}'), "control kept: {:?}", once);
    }
}
