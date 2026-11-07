use serde_json::{Value, json};

#[test]
fn review_credit_comparer_requires_valid_value_syntax() {
    let scenario: Value =
        serde_json::from_str(include_str!("../scenarios/credits-tax-group-retry.json")).unwrap();
    let path = std::env::temp_dir().join(format!(
        "fb-credit-review-comparer-{}.fbbook",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let mut runner = fb_harness::Runner::new(&scenario, &path);
    runner.run();
    let good = runner.dump();
    assert!(fb_harness::compare(&good, &good).ok());
    let mut accepted = vec![];
    for (path, wrong) in [
        ("/credits/creditmemo/0/date", json!("not-a-date")),
        ("/credits/creditmemo/0/tax_rate", json!("NaN")),
        ("/credits/creditmemo/0/tax_mode", json!("BOGUS")),
        (
            "/credits/creditmemo/0/tax_snapshot/members/0/rate",
            json!("NaN"),
        ),
        (
            "/credits/creditmemo/0/tax_snapshot/members/0/tax",
            json!("garbage"),
        ),
        ("/credits/creditmemoline/0/position", json!(-9)),
        ("/credits/balances/0/credit_state", json!("BOGUS")),
    ] {
        let mut bad = good.clone();
        *bad.pointer_mut(path).unwrap() = wrong;
        if fb_harness::compare(&bad, &bad).ok() {
            accepted.push(path);
        }
    }
    println!("Identical malformed credit dumps accepted: {accepted:?}");
    assert!(
        accepted.is_empty(),
        "comparer accepted invalid credit value syntax"
    );
}

#[test]
fn review_credit_comparer_requires_complete_document_sets() {
    let scenario: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/scenarios/credits-lifecycle.json"
    )))
    .unwrap();
    let path = std::env::temp_dir().join(format!(
        "fb-credit-review-coverage-{}.fbbook",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let mut runner = fb_harness::Runner::new(&scenario, &path);
    runner.run();
    let good = runner.dump();
    assert!(fb_harness::compare(&good, &good).ok());
    let mut accepted = vec![];
    for table in ["creditmemoline", "creditapplicationline"] {
        let mut bad = good.clone();
        bad["credits"][table] = json!([]);
        if fb_harness::compare(&bad, &bad).ok() {
            accepted.push(table);
        }
    }
    let mut bad = good.clone();
    for rows in bad["credits"].as_object_mut().unwrap().values_mut() {
        *rows = json!([]);
    }
    if fb_harness::compare(&bad, &bad).ok() {
        accepted.push("entire credits module despite CM/RFD identities and journals");
    }
    println!("Identical incomplete credit dumps accepted: {accepted:?}");
    assert!(
        accepted.is_empty(),
        "comparer accepted omitted credit records"
    );
}

#[test]
fn review_credit_comparer_covers_released_applications_and_audit_allocations() {
    let scenario: Value =
        serde_json::from_str(include_str!("../scenarios/credits-lifecycle.json")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut runner = fb_harness::Runner::new(&scenario, &dir.path().join("coverage.fbbook"));
    runner.run();
    let good = runner.dump();
    assert!(fb_harness::compare(&good, &good).ok());
    let mut missing = good.clone();
    for table in [
        "creditapplication",
        "creditapplicationline",
        "creditunapplication",
    ] {
        missing["credits"][table] = json!([]);
    }
    assert!(
        !fb_harness::compare(&missing, &missing).ok(),
        "released applications disappeared despite immutable audit evidence"
    );
    let mut missing = good.clone();
    let events = missing["audit"]["c"].as_array_mut().unwrap();
    let applied = events
        .iter_mut()
        .find(|a| a["action"] == "credit_memo.applied")
        .unwrap();
    applied["detail"]["invoices"] = json!([]);
    assert!(
        !fb_harness::compare(&missing, &missing).ok(),
        "audit allocation coverage was lost"
    );
}

#[test]
fn review_credit_comparer_allows_unused_numbering_identities() {
    let scenario: Value =
        serde_json::from_str(include_str!("../scenarios/l2a-numbering.json")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut runner = fb_harness::Runner::new(&scenario, &dir.path().join("numbering.fbbook"));
    runner.run();
    let good = runner.dump();
    assert!(fb_harness::compare(&good, &good).ok());
}
