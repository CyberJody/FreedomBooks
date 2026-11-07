//! L4 reports fix 3: the two comparer defects of the third review (runs/2026-10-30/r3-l4-reports-review,
//! `boundary_contract.py`). A refused account creation reserves no creation-order rank (the cent rule's
//! tie-break), and malformed settlement amounts are structured failures, never a panic.
use serde_json::{Value, json};
use std::sync::OnceLock;

fn scenario(name: &str) -> Value {
    let text = std::fs::read_to_string(format!("{}/scenarios/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let mut scenario: Value = fb_db::parse_unique_json(&text).unwrap();
    scenario.as_object_mut().unwrap().remove("capture_steps");
    scenario
}

fn run(scenario: &Value) -> Value {
    let dir = tempfile::tempdir().unwrap();
    let mut runner = fb_harness::Runner::new(scenario, &dir.path().join("fix3.fbbook"));
    runner.run();
    runner.dump()
}

fn step(steps: &[Value], f: impl Fn(&Value) -> bool) -> Value {
    steps.iter().find(|s| f(s)).cloned().unwrap()
}

/// `retry_account_creation.py`: the hand-computed company through its setup, then one invoice whose single
/// 0.01 settlement ties the cent between `income` and `income2`, so the creation order decides it.
fn retry_scenario() -> Value {
    let base = scenario("reports-hand-computed.json");
    let steps = base["steps"].as_array().unwrap();
    let setup = steps.iter().position(|s| s["op"] == "l2_setup").unwrap();
    let mut scenario = base.clone();
    let mut out: Vec<Value> = steps[..=setup].to_vec();
    let mut invoice = step(steps, |s| s["op"] == "sales_save_invoice" && s["ref"] == "i2");
    invoice["values"]["tax_rate"] = json!("0");
    for line in invoice["values"]["lines"].as_array_mut().unwrap() {
        line["item_id"] = Value::Null;
        line["quantity"] = json!("1");
        line["rate"] = json!("0.01");
        line["taxable"] = json!(false);
    }
    out.push(invoice);
    for op in ["sales_review_invoice", "sales_post_invoice"] {
        out.push(step(steps, |s| s["op"] == op && s["record"] == "i2"));
    }
    for op in ["sales_review_receipt", "sales_record_receipt"] {
        let mut receipt = step(steps, |s| s["op"] == op && s["review"] == "r1");
        receipt["values"]["amount"] = json!("0.01");
        receipt["values"]["allocations"] = json!([{"invoice_id": "i2", "amount": "0.01"}]);
        out.push(receipt);
    }
    scenario["steps"] = json!(out);
    scenario
}

/// A refused creation of `income2` before the real one (same code as `bank`, so both runtimes refuse it).
fn refused_income2(pinned: bool) -> Value {
    let mut s = json!({"op": "save_account", "company": "c", "ref": "income2", "code": "100", "name": "Refused duplicate", "kind": "INCOME"});
    if pinned {
        s["expect_error"] = json!(true);
        s["expect_kind"] = json!("ValidationError");
        s["expect_message"] = json!("Account with this Company and Code already exists.");
    }
    s
}

fn assert_compares(scenario: &Value, label: &str) {
    let dump = run(scenario);
    let report = fb_harness::compare_scenario(Some(scenario), &dump, &dump);
    assert!(report.ok(), "{label}: {:?}", report.failures);
}

#[test]
fn refused_account_attempt_does_not_change_actual_creation_order() {
    let baseline = retry_scenario();
    assert_compares(&baseline, "baseline");
    // `boundary_contract.py`: the refusal pinned by `expect_error`.
    let mut pinned = baseline.clone();
    pinned["steps"].as_array_mut().unwrap().insert(2, refused_income2(true));
    let dump = run(&pinned);
    assert_eq!(dump["outcomes"][2]["ok"], json!(false));
    let report = fb_harness::compare_scenario(Some(&pinned), &dump, &dump);
    assert!(report.ok(), "pinned refusal: {:?}", report.failures);
    // Unpinned (INT-L4, main's rule): a missing `expect_error` means "expect success", so the refusal fails the
    // scenario contract on each side. The side's own refused outcome still keeps it out of the report order:
    // every failure is the step-2 outcome contract, none is a financial report or book difference.
    let mut unpinned = baseline.clone();
    unpinned["steps"].as_array_mut().unwrap().insert(2, refused_income2(false));
    let dump = run(&unpinned);
    let report = fb_harness::compare_scenario(Some(&unpinned), &dump, &dump);
    assert!(!report.ok(), "an unpinned refusal must fail under main's rule");
    assert!(
        report.failures.iter().all(|f| f.contains("step 2 \"save_account\"") || f.contains("step 2 save_account")),
        "unpinned refusal: only the outcome contract may fail: {:?}",
        report.failures
    );
}

/// The cent rule still uses the creation order: swapping the two income accounts' creation moves the tied cent,
/// so a report that kept the old assignment is rejected.
#[test]
fn creation_order_still_decides_the_tied_cent() {
    let baseline = retry_scenario();
    let dump = run(&baseline);
    let mut swapped = baseline.clone();
    let steps = swapped["steps"].as_array_mut().unwrap();
    let a = steps.iter().position(|s| s["op"] == "save_account" && s["ref"] == "income").unwrap();
    let b = steps.iter().position(|s| s["op"] == "save_account" && s["ref"] == "income2").unwrap();
    steps.swap(a, b);
    // The dump's books (from the original order) against the swapped scenario's order: the cent is elsewhere.
    let report = fb_harness::compare_scenario(Some(&swapped), &dump, &dump);
    assert!(report.failures.iter().any(|f| f.contains("financial reports")), "{:?}", report.failures);
}

/// The combined company (receipts, bill payments, credit applications, a voided refund, deposit applications).
fn combined() -> &'static (Value, Value) {
    static FIXTURE: OnceLock<(Value, Value)> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let scenario = scenario("reports-combined.json");
        let dump = run(&scenario);
        let report = fb_harness::compare_scenario(Some(&scenario), &dump, &dump);
        assert!(report.ok(), "{:?}", report.failures);
        (scenario, dump)
    })
}

/// Every settlement amount cell of the dump, as a JSON pointer.
fn settlement_amounts(dump: &Value) -> Vec<String> {
    let mut found = Vec::new();
    for (i, r) in dump["sales"]["receipts"].as_array().unwrap().iter().enumerate() {
        for j in 0..r["allocations"].as_array().map_or(0, Vec::len) {
            found.push(format!("/sales/receipts/{i}/allocations/{j}/amount"));
        }
    }
    for (family, list) in [("payables", "paymentallocation"), ("credits", "creditapplicationline"), ("credits", "creditrefund")] {
        for i in 0..dump[family][list].as_array().map_or(0, Vec::len) {
            found.push(format!("/{family}/{list}/{i}/amount"));
        }
    }
    for i in 0..dump["prepayments"]["applications"].as_array().map_or(0, Vec::len) {
        found.push(format!("/prepayments/applications/{i}/amount"));
    }
    found
}

#[test]
fn malformed_settlement_amounts_are_failures_not_panics() {
    let (scenario, dump) = combined();
    let cells = settlement_amounts(dump);
    for family in ["/sales/receipts/", "/payables/", "/credits/creditapplicationline/", "/credits/creditrefund/", "/prepayments/"] {
        assert!(cells.iter().any(|c| c.starts_with(family)), "the fixture has no {family} settlement");
    }
    let voided = dump["credits"]["creditrefundvoid"].as_array().unwrap();
    assert!(!voided.is_empty(), "the fixture has no voided refund");
    let bad = [json!(i64::MIN), json!(i64::MIN + 1), json!(i64::MAX), json!(-1), json!(10_000_000_000_000_000i64), json!("-92233720368547758.08")];
    for cell in &cells {
        for amount in &bad {
            let mut corrupt = dump.clone();
            *corrupt.pointer_mut(cell).unwrap() = amount.clone();
            let outcome = std::panic::catch_unwind(|| fb_harness::compare_scenario(Some(scenario), &corrupt, &corrupt));
            let report = outcome.unwrap_or_else(|_| panic!("{cell} = {amount}: the comparer panicked"));
            assert!(!report.ok(), "{cell} = {amount}: identical malformed settlements passed");
            assert!(report.failures.iter().any(|f| f.contains("amount: ")),"{cell} = {amount}: {:?}", report.failures);
        }
    }
}

/// `boundary_contract.py` through the CLI: exit 1 with a FAIL line, not 101.
#[test]
fn cli_minimum_refund_amount_exits_with_a_failure() {
    let (scenario, dump) = combined();
    let mut corrupt = dump.clone();
    let refunds = corrupt["credits"]["creditrefund"].as_array_mut().unwrap();
    let voided: Vec<&str> = dump["credits"]["creditrefundvoid"].as_array().unwrap().iter().filter_map(|v| v["refund_id"].as_str()).collect();
    let refund = refunds.iter_mut().find(|r| r["id"].as_str().is_some_and(|id| voided.contains(&id))).unwrap();
    refund["amount"] = json!(i64::MIN);
    let dir = tempfile::tempdir().unwrap();
    let (d, s) = (dir.path().join("dump.json"), dir.path().join("scenario.json"));
    std::fs::write(&d, serde_json::to_string(&corrupt).unwrap()).unwrap();
    std::fs::write(&s, serde_json::to_string(scenario).unwrap()).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_fb-harness")).arg("compare").arg(&d).arg(&d).arg(&s).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("FAIL final python financial reports") && stdout.contains("refund amount"), "{stdout}");
}
