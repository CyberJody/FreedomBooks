use serde_json::{json, Value};

fn fixture() -> (Value, Value) {
    let scenario = json!({
        "users": [{"username": "review", "is_staff": true}],
        "steps": [
            {"op":"create_company", "ref":"c", "name":"Synthetic report comparison"},
            {"op":"save_account", "company":"c", "ref":"bank", "code":"100", "name":"Bank", "kind":"ASSET"},
            {"op":"save_account", "company":"c", "ref":"income", "code":"400", "name":"Income", "kind":"INCOME"},
            {"op":"save_draft", "company":"c", "ref":"j", "date":"2026-10-01", "memo":"Synthetic income", "lines":[{"account":"bank", "debit":"1.00"}, {"account":"income", "credit":"1.00"}]},
            {"op":"post_journal", "company":"c", "journal":"j"}
        ],
        "financial_reports": {
            "profit_loss": [[null, "2026-10-31"]],
            "balance_sheet": ["2026-10-31"],
            "general_ledger": [[null, "2026-10-31", "", null]],
            "forms": []
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let mut runner = fb_harness::Runner::new(&scenario, &dir.path().join("review.fbbook"));
    runner.run();
    let dump = runner.dump();
    assert!(fb_harness::compare_scenario(Some(&scenario), &dump, &dump).ok());
    (scenario, dump)
}

#[test]
fn review_comparer_rejects_erased_report_sets() {
    let (scenario, mut dump) = fixture();
    for base in ["ACCRUAL", "CASH"] {
        for family in ["profit_loss", "balance_sheet", "general_ledger"] {
            dump["financial_reports"]["companies"]["c"][base][family] = json!({});
        }
    }
    assert!(!fb_harness::compare_scenario(Some(&scenario), &dump, &dump).ok(), "all requested reports vanished and still MATCH");
}

#[test]
fn review_comparer_rejects_null_owned_ledger() {
    let (scenario, mut dump) = fixture();
    for base in ["ACCRUAL", "CASH"] {
        dump["financial_reports"]["companies"]["c"][base]["general_ledger"]["..2026-10-31||"] = Value::Null;
    }
    assert!(!fb_harness::compare_scenario(Some(&scenario), &dump, &dump).ok(), "a valid unfiltered ledger became null and still MATCH");
}

#[test]
fn review_comparer_rejects_empty_csv() {
    let (scenario, mut dump) = fixture();
    for base in ["ACCRUAL", "CASH"] {
        dump["financial_reports"]["companies"]["c"][base]["profit_loss"]["..2026-10-31"]["csv"] = json!([]);
        dump["financial_reports"]["companies"]["c"][base]["balance_sheet"]["2026-10-31"]["csv"] = json!([]);
    }
    assert!(!fb_harness::compare_scenario(Some(&scenario), &dump, &dump).ok(), "complete CSV rows vanished and still MATCH");
}

#[test]
fn review_comparer_rejects_malformed_successful_form() {
    let (scenario, mut dump) = fixture();
    dump["financial_reports"]["forms"] = json!([{"ok":true,"basis":"ACCRUAL","preset":[],"start":{},"end":"not-a-date"}]);
    assert!(!fb_harness::compare_scenario(Some(&scenario), &dump, &dump).ok(), "malformed successful form still MATCH");
}

#[test]
fn review_comparer_rejects_short_date_key_without_panic() {
    let (scenario, mut dump) = fixture();
    for base in ["ACCRUAL", "CASH"] {
        let bs = dump["financial_reports"]["companies"]["c"][base]["balance_sheet"]["2026-10-31"].clone();
        dump["financial_reports"]["companies"]["c"][base]["balance_sheet"] = json!({"x": bs});
    }
    let result = std::panic::catch_unwind(|| fb_harness::compare_scenario(Some(&scenario), &dump, &dump));
    assert!(result.is_ok(), "malformed date key panicked in the comparer");
    assert!(!result.unwrap().ok());
}
