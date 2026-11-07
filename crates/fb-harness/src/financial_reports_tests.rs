//! Book-level checks of the L4 reports through the real document services, and comparer self-tests: malformed
//! report dumps fail even when both sides are identical, and every corrupted value is a failure.
use super::*;
use fb_core::Money;
use fb_core::reports::financial::{self, Basis};
use chrono::NaiveDate;

fn scenario() -> Value {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/scenarios/int-l3-combined.json")).unwrap();
    let mut scenario: Value = serde_json::from_str(&text).unwrap();
    scenario.as_object_mut().unwrap().remove("capture_steps");
    scenario["financial_reports"] = json!({
        "profit_loss": [[null, "2026-10-31"], ["2026-10-01", "2026-10-09"], ["2027-01-01", "2027-12-31"]],
        "balance_sheet": ["2025-12-31", "2026-10-21", "2027-01-01"],
        "general_ledger": [[null, "2027-12-31", "", null], [null, "2026-12-31", "", null], ["2026-10-10", "2026-10-21", "pl", null]],
        "forms": [{"form": "range", "data": {}, "today": "2026-10-29"}, {"form": "as_of", "data": {"as_of": "garbage"}, "today": "2026-10-29"}]
    });
    scenario
}

fn run(scenario: &Value) -> (Value, Runner<'_>) {
    let dir = std::env::temp_dir().join(format!("fb-l4r-{}", fb_ids::new_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut runner = Runner::new(scenario, &dir.join("book.fb"));
    runner.run();
    (runner.dump(), runner)
}

fn day(text: &str) -> NaiveDate {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
}

#[test]
fn combined_company_reports_balance_and_cash_differs_from_accrual() {
    let scenario = scenario();
    let (dump, mut runner) = run(&scenario);
    let report = compare_scenario(Some(&scenario), &dump, &dump);
    assert!(report.ok(), "{:?}", report.failures);
    let company = runner.companies["c"];
    let actor = runner.company_actor["c"];
    for basis in [Basis::Accrual, Basis::Cash] {
        for as_of in ["2026-10-03", "2026-10-21", "2026-10-31", "2026-11-01", "2027-01-01"] {
            let sheet = financial::balance_sheet(&mut runner.book, actor, company, basis, day(as_of)).unwrap();
            assert!(sheet.balanced, "{basis:?} {as_of}");
            // Net income on the sheet is the P&L for the fiscal year to date.
            let pl = financial::profit_and_loss(&mut runner.book, actor, company, basis, Some(sheet.fiscal_year_start), day(as_of)).unwrap();
            assert_eq!(sheet.net_income, pl.net_income);
        }
        // Retained earnings at the fiscal-year boundary is everything before it.
        let next = financial::balance_sheet(&mut runner.book, actor, company, basis, day("2027-01-01")).unwrap();
        let all = financial::profit_and_loss(&mut runner.book, actor, company, basis, None, day("2026-12-31")).unwrap();
        assert_eq!((next.retained_earnings, next.net_income), (all.net_income, Money::ZERO));
        // An empty period reports no rows; the general ledger closing equals the balance sheet.
        let empty = financial::profit_and_loss(&mut runner.book, actor, company, basis, Some(day("2025-01-01")), day("2025-12-31")).unwrap();
        assert!(empty.sections.iter().all(|s| s.rows.is_empty()) && empty.net_income == Money::ZERO);
        let gl = financial::general_ledger(&mut runner.book, actor, company, basis, None, day("2026-12-31"), None, "asset").unwrap();
        let sheet = financial::balance_sheet(&mut runner.book, actor, company, basis, day("2026-12-31")).unwrap();
        let closing: Vec<_> = gl.sections.iter().filter(|s| !s.closing.is_zero()).map(|s| (s.account.id, s.closing)).collect();
        let rows: Vec<_> = sheet.assets.rows.iter().map(|r| (r.account.id, r.amount)).collect();
        assert_eq!(closing, rows, "{basis:?}");
    }
    // A deposit is a liability, not income: the October cash P&L differs from accrual (invoice income deferred).
    let accrual = financial::profit_and_loss(&mut runner.book, actor, company, Basis::Accrual, Some(day("2026-10-01")), day("2026-10-09")).unwrap();
    let cash = financial::profit_and_loss(&mut runner.book, actor, company, Basis::Cash, Some(day("2026-10-01")), day("2026-10-09")).unwrap();
    assert_ne!(accrual.net_income, cash.net_income);
    // Cash-basis general-ledger shares carry the settled document number.
    let gl = financial::general_ledger(&mut runner.book, actor, company, Basis::Cash, None, day("2026-12-31"), None, "income").unwrap();
    assert!(gl.sections.iter().flat_map(|s| &s.rows).any(|r| r.synthetic.is_some() && r.memo.starts_with("Cash basis: share of ")));
    // Another company's account and an unknown group are refused.
    assert!(matches!(
        financial::general_ledger(&mut runner.book, actor, company, Basis::Cash, None, day("2026-12-31"), Some(fb_ids::AccountId::new()), ""),
        Err(FbError::DoesNotExist(_))
    ));
    assert!(matches!(financial::general_ledger(&mut runner.book, actor, company, Basis::Cash, None, day("2026-12-31"), None, "nope"), Err(FbError::Validation(_))));
    // Reports are company scoped.
    let stranger = users::create_user(&mut runner.book, "stranger", false).unwrap();
    assert!(matches!(financial::balance_sheet(&mut runner.book, stranger, company, Basis::Cash, day("2026-10-31")), Err(FbError::PermissionDenied(_))));
}

fn reports_of(dump: &Value) -> Value {
    dump["financial_reports"].clone()
}

fn first_pl(v: &mut Value) -> &mut Value {
    v["financial_reports"]["companies"]["c"]["CASH"]["profit_loss"].as_object_mut().unwrap().values_mut().next().unwrap()
}

fn first_bs(v: &mut Value) -> &mut Value {
    &mut v["financial_reports"]["companies"]["c"]["ACCRUAL"]["balance_sheet"]["2026-10-21"]
}

fn first_gl(v: &mut Value) -> &mut Value {
    &mut v["financial_reports"]["companies"]["c"]["CASH"]["general_ledger"]["..2026-12-31||"]
}

#[test]
fn comparer_rejects_malformed_and_corrupted_financial_reports() {
    let scenario = scenario();
    let (dump, _runner) = run(&scenario);
    let expected = financial_reports::expected(&scenario, None);
    assert_eq!(financial_reports::validate(&reports_of(&dump), &dump, dump["outcomes"].as_array().unwrap(), &expected, &scenario["financial_reports"]), Ok(()));
    let compare = |py: &Value, rs: &Value| compare_scenario(Some(&scenario), py, rs);
    type Mutation = Box<dyn Fn(&mut Value)>;
    let malformed: Vec<(&str, Mutation)> = vec![
        ("missing key", Box::new(|v| { first_pl(v).as_object_mut().unwrap().remove("unconverted"); })),
        ("extra key", Box::new(|v| { first_pl(v)["extra"] = json!(1); })),
        ("string amount", Box::new(|v| { first_bs(v)["assets"]["total"] = json!("1"); })),
        ("unbalanced", Box::new(|v| { first_bs(v)["total_equity"] = json!(-1); })),
        ("balanced flag", Box::new(|v| { first_bs(v)["balanced"] = json!(false); })),
        ("row total", Box::new(|v| { let rows = first_bs(v)["assets"]["rows"].as_array_mut().unwrap(); rows[0][1] = json!(rows[0][1].as_i64().unwrap() + 1); })),
        ("duplicate row", Box::new(|v| { let rows = first_bs(v)["assets"]["rows"].as_array_mut().unwrap(); let r = rows[0].clone(); rows.push(r); })),
        ("gl running balance", Box::new(|v| { let s = first_gl(v)["sections"].as_array_mut().unwrap(); s[0]["rows"][0][10] = json!(123456789); })),
        ("gl null", Box::new(|v| { first_gl(v)["sections"] = Value::Null; })),
        ("basis missing", Box::new(|v| { v["financial_reports"]["companies"]["c"].as_object_mut().unwrap().remove("CASH"); })),
        ("report set", Box::new(|v| { v["financial_reports"]["companies"]["c"]["CASH"]["profit_loss"].as_object_mut().unwrap().remove("2026-10-01..2026-10-09"); })),
        ("form shape", Box::new(|v| { v["financial_reports"]["forms"][0]["ok"] = json!("yes"); })),
        ("empty errors", Box::new(|v| { v["financial_reports"]["forms"][1]["errors"] = json!([]); })),
        ("accrual vs journals", Box::new(|v| {
            let pl = v["financial_reports"]["companies"]["c"]["ACCRUAL"]["profit_loss"]["..2026-10-31"]["sections"]["income"].as_object_mut().unwrap();
            let rows = pl["rows"].as_array_mut().unwrap();
            rows[0][1] = json!(rows[0][1].as_i64().unwrap() + 100);
            pl["total"] = json!(pl["total"].as_i64().unwrap() + 100);
            let p = &mut v["financial_reports"]["companies"]["c"]["ACCRUAL"]["profit_loss"]["..2026-10-31"];
            for key in ["gross_profit", "net_ordinary_income", "net_income"] { p[key] = json!(p[key].as_i64().unwrap() + 100); }
        })),
    ];
    for (name, mutate) in &malformed {
        let mut bad = dump.clone();
        mutate(&mut bad);
        // Identical malformed dumps on both sides are not parity.
        let report = compare(&bad, &bad);
        assert!(!report.ok(), "identical malformed {name} passed");
        assert!(!compare(&dump, &bad).ok(), "one-sided malformed {name} passed");
    }
    // A valid but different value on one side is a mismatch.
    let mut other = dump.clone();
    let rows = first_gl(&mut other)["sections"][0]["rows"].as_array_mut().unwrap();
    rows[0][7] = json!("different memo");
    assert!(!compare(&dump, &other).ok());
    // Reports on one side only.
    let mut missing = dump.clone();
    missing.as_object_mut().unwrap().remove("financial_reports");
    assert!(!compare(&dump, &missing).ok());
    assert!(!compare(&missing, &dump).ok());
    // Reports the scenario requests, missing on both sides, are not parity either.
    assert!(!compare(&missing, &missing).ok());
    // Without the scenario a dump without reports compares; one with reports cannot be checked.
    assert!(super::compare(&missing, &missing).ok());
    assert!(!super::compare(&dump, &dump).ok());
}
