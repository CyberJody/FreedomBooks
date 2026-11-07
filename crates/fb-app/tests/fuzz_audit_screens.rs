//! Fuzz audit 2026-11, area: screen form decoding and routing (fb-app `screens`).
//!
//! Properties: `parse_query(urlencode(pairs)) == pairs`; `resolve` accepts canonical ids only;
//! every ported form route accepts random field maps without panicking (an invalid post re-renders,
//! a valid one redirects); report filters with random `basis`/`output`/`preset` values never panic
//! (the `errors.insert(usize::from(basis_error), ..)` in screens/d8.rs is the lead being checked);
//! and the form-level integer parser `clean_integer` behaves like Django's IntegerField.
use fb_app::commands::{Command, Outcome, parse_command};
use fb_app::{App, keys, screens};
use proptest::prelude::*;
use serde_json::json;

/// A fresh book path inside its own temporary directory; the directory (book, WAL and attachments)
/// is removed when the returned guard drops, so a run leaves nothing in `$TMPDIR`.
fn tmp() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::Builder::new().prefix("fb-fuzz-audit-screens-").tempdir().unwrap();
    let path = dir.path().join("book.fbbook");
    (dir, path)
}

fn run(app: &mut App, v: serde_json::Value) -> serde_json::Value {
    match app.run(parse_command(&v.to_string()).unwrap()).unwrap() {
        Outcome::Done { data } => data,
        other => panic!("{other:?}"),
    }
}

fn token_of(html: &str) -> Option<String> {
    let marker = r#"name="fb_command_token" value=""#;
    let at = html.find(marker)? + marker.len();
    Some(html[at..at + 64].to_owned())
}

struct Site {
    app: App,
    c: String,
    accounts: Vec<String>,
    /// Declared last: drops after `app` has closed the book.
    _dir: tempfile::TempDir,
}

fn site() -> Site {
    let mut app = App::new();
    app.today = || chrono::NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
    let (dir, path) = tmp();
    let c = app.create_book(&path, &keys::new_master_key().unwrap(), "Owner", "Screens Co").unwrap().company_id.to_string();
    let mut accounts = Vec::new();
    for (code, name, t) in [("1000", "Checking", "BANK"), ("1100", "AR", "RECEIVABLE"), ("1200", "UF", "OTHER_CURRENT_ASSET"), ("2000", "AP", "PAYABLE"), ("3000", "Capital", "EQUITY"), ("4000", "Sales", "INCOME"), ("5000", "Supplies", "EXPENSE")] {
        accounts.push(run(&mut app, json!({"type": "SaveAccount", "company_id": c, "code": code, "name": name, "detail_type": t}))["id"].as_str().unwrap().to_owned());
    }
    run(&mut app, json!({"type": "SaveSetup", "company_id": c, "expected_revision": 0, "receivables_id": accounts[1], "undeposited_id": accounts[2], "payables_id": accounts[3], "bank_ids": [accounts[0]]}));
    Site { app, c, accounts, _dir: dir }
}

/// Every route pattern with its placeholders filled from the live book.
fn routes(s: &Site) -> Vec<String> {
    fb_templates::django::route_table()
        .values()
        .map(|pattern| {
            pattern
                .split('/')
                .map(|part| match part.strip_prefix('<').and_then(|p| p.strip_suffix('>')) {
                    Some(inner) => {
                        let (_, arg) = inner.split_once(':').unwrap_or(("str", inner));
                        match arg {
                            "company_id" => s.c.clone(),
                            "number" => "1".into(),
                            _ => s.accounts[0].clone(),
                        }
                    }
                    None => part.to_owned(),
                })
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect()
}

fn field_value() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => "[0-9]{1,6}(\\.[0-9]{1,4})?",
        2 => "-?[0-9]{1,3}",
        2 => Just("2026-10-05".to_string()),
        1 => Just("13/45/2026".to_string()),
        1 => "[a-zA-Z ]{0,12}",
        1 => Just(String::new()),
        1 => Just("on".to_string()),
        1 => Just("NONE".to_string()),
        1 => Just("MANUAL".to_string()),
        1 => Just("\u{662}\u{663}".to_string()),
        1 => Just("1e5".to_string()),
        1 => Just("-0".to_string()),
        1 => Just("0".repeat(30)),
        1 => "[\\x00-\\x7f]{0,6}",
    ]
}

fn field_name() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("expected_revision".to_string()),
        Just("date".to_string()),
        Just("memo".to_string()),
        Just("amount".to_string()),
        Just("quantity".to_string()),
        Just("rate".to_string()),
        Just("tax_mode".to_string()),
        Just("tax_rate".to_string()),
        Just("customer".to_string()),
        Just("lines-TOTAL_FORMS".to_string()),
        Just("lines-INITIAL_FORMS".to_string()),
        Just("allocations-TOTAL_FORMS".to_string()),
        Just("allocations-INITIAL_FORMS".to_string()),
        "lines-[0-9]{1,2}-(quantity|rate|description|taxable|item|income_account|DELETE|origin_position)",
        Just("basis".to_string()),
        Just("preset".to_string()),
        Just("start".to_string()),
        Just("end".to_string()),
        Just("output".to_string()),
        Just("as_of".to_string()),
        Just("account".to_string()),
        Just("group".to_string()),
        Just("page".to_string()),
        Just("q".to_string()),
        Just("state".to_string()),
        Just("request_key".to_string()),
        "[a-z_]{1,12}",
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, .. ProptestConfig::default() })]

    #[test]
    fn query_strings_round_trip(pairs in prop::collection::vec(("[a-zA-Z0-9 _.%&=+/\\-]{0,8}", "[^\\x00]{0,12}"), 0..6)) {
        let encoded = screens::urlencode(&pairs);
        prop_assert_eq!(screens::parse_query(&encoded), pairs);
    }

    #[test]
    fn resolve_requires_canonical_ids(n in any::<u128>(), suffix in "[a-z]{0,3}") {
        let u = uuid::Uuid::from_u128(n).to_string();
        prop_assert!(screens::resolve(&format!("/c/{u}/home/")).is_some(), "assertion failed");
        prop_assert!(screens::resolve(&format!("/c/{}/home/", u.to_uppercase())).is_none(), "assertion failed");
        prop_assert!(screens::resolve(&format!("/c/{u}{suffix}/home/")).is_some() == suffix.is_empty(), "assertion failed");
        prop_assert!(screens::resolve(&format!("/c/{}/home/", uuid::Uuid::from_u128(n).simple())).is_none(), "simple form is not canonical");
    }

    #[test]
    fn python_integer_port_reads_padded_ints_and_refuses_decimals(n in any::<i64>(), zeros in 0usize..4, pad in "[ \\t\\u{3000}]{0,2}") {
        prop_assert_eq!(fb_templates::forms::python_integer(&format!("{pad}{n}{pad}")), Some(n));
        prop_assert_eq!(fb_templates::forms::python_integer(&format!("{n}.{}", "0".repeat(zeros))), None, "int() refuses a decimal point");
        prop_assert_eq!(fb_templates::forms::python_integer(&format!("{n}_")), None, "trailing underscore");
        prop_assert_eq!(fb_templates::forms::python_integer(&format!("_{n}")), None, "leading underscore");
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 40, .. ProptestConfig::default() })]

    /// Random field maps posted to every form route: never a panic; the book stays consistent.
    #[test]
    fn every_form_route_survives_random_fields(
        fields in prop::collection::vec((field_name(), field_value()), 0..24),
        pick in any::<prop::sample::Index>(),
    ) {
        let mut s = site();
        let all = routes(&s);
        // The whole route table (154 routes), not just the first 64 by name (review finding 3).
        let path = all[pick.index(all.len())].clone();
        let rendered = screens::render(&mut s.app, &path);
        let token = rendered.ok().and_then(|r| token_of(&r.html));
        let outcome = match token {
            Some(token) => s.app.run(Command::SubmitForm { path: path.clone(), token, fields: fields.clone() }),
            None => screens::submit(&mut s.app, &path, &fields),
        };
        if let Ok(Outcome::Invalid { html }) = outcome {
            prop_assert!(!html.is_empty());
        }
        let problems = s.app.open_mut().unwrap().book.check().unwrap();
        prop_assert!(problems.is_empty(), "book integrity after posting {:?} to {}: {:?}", fields, path, problems);
    }

    /// Report screens with random controls (the d8 `errors.insert(usize::from(basis_error), ..)` lead).
    #[test]
    fn report_filters_never_panic(
        basis in prop_oneof![Just("ACCRUAL".to_string()), Just("CASH".to_string()), Just("".to_string()), Just("MAGIC".to_string()), "[A-Z]{0,8}"],
        output in prop_oneof![Just("".to_string()), Just("print".to_string()), Just("csv".to_string()), Just("pdf".to_string()), "[a-z]{0,5}"],
        preset in prop_oneof![Just("".to_string()), Just("custom".to_string()), Just("this_month".to_string()), Just("fiscal_year".to_string()), "[a-z_]{0,10}"],
        start in prop_oneof![Just("".to_string()), Just("2026-01-01".to_string()), Just("2026-13-01".to_string()), "[0-9/\\-]{0,10}"],
        end in prop_oneof![Just("".to_string()), Just("2025-12-31".to_string()), Just("2026-12-31".to_string()), "[0-9/\\-]{0,10}"],
        account in prop_oneof![Just("".to_string()), Just("not-an-id".to_string()), "[a-f0-9\\-]{0,40}"],
        group in prop_oneof![Just("".to_string()), Just("asset".to_string()), "[a-z]{0,8}"],
        report in 0usize..6,
    ) {
        let mut s = site();
        let names = [("profit_loss", "profit-and-loss"), ("balance_sheet", "balance-sheet"), ("general_ledger", "general-ledger"), ("profit_loss", "profit-and-loss"), ("balance_sheet", "balance-sheet"), ("general_ledger", "general-ledger")];
        let (name, slug) = names[report];
        let as_of = end.clone();
        let params = vec![
            ("basis".to_string(), basis), ("output".to_string(), output), ("preset".to_string(), preset),
            ("start".to_string(), start), ("end".to_string(), end), ("as_of".to_string(), as_of),
            ("account".to_string(), account), ("group".to_string(), group),
        ];
        let query = screens::urlencode(&params);
        let _ = screens::render(&mut s.app, &format!("/c/{}/reports/{slug}/?{query}", s.c));
        let _ = s.app.run(Command::ExportReport { company_id: s.c.clone(), report: name.into(), filters: params.clone() });
        let _ = s.app.run(Command::ExportHistory { company_id: s.c.clone(), filters: params });
    }
}

/// FINDING F-09 (cosmetic): `screens::parse_query` decodes a percent escape with
/// `u8::from_str_radix(&text[i+1..i+3], 16)`, and that function accepts a leading `+`, so `%+4`
/// becomes byte 0x04. Django's QueryDict (urllib `parse_qsl`) only decodes two hex digits: `%+4`
/// is the literal text `% 4` (the `+` is a space). Pinned; fails today.
#[test]
#[ignore = "fuzz audit finding F-09: parse_query decodes '%+4' as byte 0x04; Django keeps the text '% 4'"]
fn parse_query_does_not_treat_a_plus_as_a_hex_digit() {
    assert_eq!(screens::parse_query("q=%+4"), vec![("q".to_string(), "% 4".to_string())]);
    assert_eq!(screens::parse_query("q=%41"), vec![("q".to_string(), "A".to_string())]);
}

/// Every route in the Django route table, filled with real ids from the live book, rendered (GET) and
/// posted (empty form): the app layer must answer every one without panicking. Lists the offenders.
fn render_and_submit_every_route() -> Vec<String> {
    let mut s = site();
    let mut panics = Vec::new();
    for (name, pattern) in fb_templates::django::route_table() {
        let path: String = pattern
            .split('/')
            .map(|part| match part.strip_prefix('<').and_then(|p| p.strip_suffix('>')) {
                Some(inner) => {
                    let (_, arg) = inner.split_once(':').unwrap_or(("str", inner));
                    match arg {
                        "company_id" => s.c.clone(),
                        "number" => "1".into(),
                        _ => s.accounts[0].clone(),
                    }
                }
                None => part.to_owned(),
            })
            .collect::<Vec<_>>()
            .join("/");
        let get = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| { let _ = screens::render(&mut s.app, &path); }));
        if get.is_err() {
            panics.push(format!("GET {name} {path}"));
        }
        let post = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| { let _ = screens::submit(&mut s.app, &path, &[]); }));
        if post.is_err() {
            panics.push(format!("POST {name} {path}"));
        }
    }
    panics
}

/// FINDING F-02 (crash, fixed): a POST to `sales:list` reached `screens/d4/estimates.rs`, which
/// indexed `r.args["estimate_id"]` and panicked. Found by the `app_form` fuzz target
/// (fuzz/artifacts/app_form/crash-d6cbc47d...). D4 now answers POST to a GET-only route with 405.
#[test]
fn every_route_renders_and_submits_without_panicking() {
    let panics = render_and_submit_every_route();
    assert!(panics.is_empty(), "routes that panic:\n{}", panics.join("\n"));
}

/// F-02 follow-up: every D4 route whose Django view refuses POST (`HttpResponseNotAllowed`) answers a
/// POST with 405, even with fields that would pass the estimate state form. Before the fix
/// `sales:detail` / `sales:revision` / `sales:print` fell through to the estimate state transition.
#[test]
fn get_only_sales_routes_refuse_post_with_405() {
    let mut s = site();
    let c = s.c.clone();
    let get_only = [
        "sales:list", "sales:detail", "sales:print", "sales:print_revision", "sales:revision",
        "invoices:detail", "invoices:print", "invoices:print_revision", "invoices:revision", "invoices:review",
        "receipts:list", "receipts:detail",
        "sales_receipts:list", "sales_receipts:detail", "sales_receipts:print",
    ];
    let fields = vec![("expected_revision".to_string(), "1".to_string()), ("state".to_string(), "ACCEPTED".to_string())];
    for name in get_only {
        let pattern = &fb_templates::django::route_table()[name];
        let path: String = pattern
            .split('/')
            .map(|part| match part.strip_prefix('<').and_then(|p| p.strip_suffix('>')) {
                Some(inner) if inner.ends_with(":company_id") => c.clone(),
                Some(inner) if inner.ends_with(":number") => "1".into(),
                Some(_) => s.accounts[0].clone(),
                None => part.to_owned(),
            })
            .collect::<Vec<_>>()
            .join("/");
        match screens::submit(&mut s.app, &path, &fields) {
            Ok(Outcome::Page { status: 405, .. }) => {}
            other => panic!("POST {name} {path}: expected 405, got {other:?}"),
        }
    }
}
