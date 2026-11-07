mod common;
use serde_json::json;

#[test]
fn review_punctuation_exception_must_still_enforce_rust_contract_order() {
    let mut python = common::dump();
    // Regardless of the permitted Python tie order, the Rust side must follow
    // its documented deterministic ICU + UTF-8 tie-break contract.
    python["accounts"][0]["code"] = json!("a-b");
    python["accounts"][1]["code"] = json!("ab");
    assert!(fb_db::collation::punctuation_tie("ab", "a-b"));
    assert_eq!(fb_db::collation::compare("a-b", "ab"), std::cmp::Ordering::Less);
    let mut wrong_rust = python.clone();
    wrong_rust["accounts"].as_array_mut().unwrap().reverse();
    let report = fb_harness::compare(&python, &wrong_rust);
    assert!(!report.ok(), "wrong Rust byte tie-break accepted: {report:?}");
}

#[test]
fn review_tax_name_with_no_punctuation_tie_is_not_hidden() {
    let mut python = common::dump();
    python["accounts"][0]["code"] = json!("a");
    python["accounts"][1]["code"] = json!("A");
    let mut wrong = python.clone();
    wrong["accounts"].as_array_mut().unwrap().reverse();
    assert!(!fb_harness::compare(&python, &wrong).ok());
}

#[test]
fn rust_order_is_required_even_when_python_has_the_same_wrong_order() {
    let mut wrong = common::dump();
    wrong["accounts"][0]["code"] = json!("ab");
    wrong["accounts"][1]["code"] = json!("a-b");
    let report = fb_harness::compare(&wrong, &wrong);
    assert!(report.failures.iter().any(|f| f.contains("Rust order violates ICU/UTF-8 contract")), "{report:?}");
}

#[test]
fn legitimate_python_ties_require_the_rust_contract_in_every_collection() {
    for field in ["chart", "trial_balance", "directory", "tax", "snapshots"] {
        let mut rust = common::dump();
        let pointer = match field {
            "chart" => {
                rust["accounts"][0]["code"] = json!("a-b");
                rust["accounts"][1]["code"] = json!("ab");
                "/accounts"
            }
            "trial_balance" => {
                rust["accounts"][0]["code"] = json!("a-b");
                rust["accounts"][1]["code"] = json!("ab");
                "/trial_balances/c1/2026-12-31/rows"
            }
            "directory" => {
                rust["l2a"]["directory"] = json!([
                    {"id":"x","company_id":"c1","model":"customer","display_name":"a-b","notes":"","active":true,"business_name":"","contact_name":"","email":"","phone":"","billing_address":"","tax_status":"TAXABLE","exemption_number":""},
                    {"id":"y","company_id":"c1","model":"customer","display_name":"ab","notes":"","active":true,"business_name":"","contact_name":"","email":"","phone":"","billing_address":"","tax_status":"TAXABLE","exemption_number":""}
                ]);
                "/l2a/directory"
            }
            "tax" => {
                rust["l2a"]["tax"] = json!([
                    {"id":"x","company":"c1","kind":"RATE","name":"a-b","description":"Tax","rate":"1.0000","agency_id":"v","active":true,"revision":1},
                    {"id":"y","company":"c1","kind":"RATE","name":"ab","description":"Tax","rate":"1.0000","agency_id":"v","active":true,"revision":1}
                ]);
                "/l2a/tax"
            }
            _ => {
                rust["l2a"]["snapshots"] = json!([
                    {"source":"s","role":"a-b","account_id":"a","account_code":"1","account_name":"Cash","account_kind":"ASSET"},
                    {"source":"s","role":"ab","account_id":"b","account_code":"2","account_name":"Equity","account_kind":"EQUITY"}
                ]);
                "/l2a/snapshots"
            }
        };
        assert!(fb_harness::compare(&rust, &rust).ok(), "{field}: valid contract rejected");
        let mut python = rust.clone();
        python.pointer_mut(pointer).unwrap().as_array_mut().unwrap().reverse();
        let legitimate = fb_harness::compare(&python, &rust);
        assert!(legitimate.ok() && legitimate.warnings.is_empty(), "{field}: {legitimate:?}");
        assert_eq!(legitimate.collation_ties, 1, "{field}");
        let wrong = fb_harness::compare(&rust, &python);
        assert!(wrong.failures.iter().any(|f| f.contains("Rust order violates ICU/UTF-8 contract")), "{field}: {wrong:?}");
        assert!(!fb_harness::compare(&python, &python).ok(), "{field}: identical wrong order accepted");
    }
}
