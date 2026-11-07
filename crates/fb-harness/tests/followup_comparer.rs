//! Independent follow-up comparer mutations, scratch only.
use serde_json::{Value, json};
mod common;
use common::dump as baseline;
#[test]
fn null_or_scalar_company_trial_balance_silently_matches() {
    let py = baseline();
    for malformed in [Value::Null, json!("lost report"), json!([]), json!(42)] {
        let mut rs = py.clone();
        rs["trial_balances"]["c1"] = malformed;
        let report = fb_harness::compare(&py, &rs);
        assert!(!report.ok(), "{report:?}");
    }
}
#[test]
fn ordinary_valid_shape_corruption_is_detected() {
    let py = baseline();
    for path in [
        "/trial_balances/c1/2026-12-31/debit",
        "/trial_balances/c1/2026-12-31/rows/0/1",
        "/registers/a/closing",
    ] {
        let mut rs = py.clone();
        *rs.pointer_mut(path).unwrap() = json!(102);
        assert!(!fb_harness::compare(&py, &rs).ok(), "{path}");
    }
    let mut rs = py.clone();
    rs["trial_balances"]["c1"]["2027-01-01"] = json!({"rows":[],"debit":0,"credit":0});
    assert!(!fb_harness::compare(&py, &rs).ok());
}
