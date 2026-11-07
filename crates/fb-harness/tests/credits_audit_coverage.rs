use serde_json::{Value, json};

fn dump(zero_line: bool) -> Value {
    let mut scenario: Value =
        serde_json::from_str(include_str!("../scenarios/credits-lifecycle.json")).unwrap();
    if zero_line {
        for step in scenario["steps"].as_array_mut().unwrap() {
            if matches!(
                step["op"].as_str(),
                Some("credits_review_memo" | "credits_record_memo")
            ) {
                step["values"]["lines"].as_array_mut().unwrap().push(json!({
                    "item_id":null,"description":"Free replacement part","quantity":"1",
                    "rate":"0.00","taxable":false,"income_account_id":"income2"
                }));
            }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let mut runner = fb_harness::Runner::new(&scenario, &dir.path().join("coverage.fbbook"));
    runner.run();
    let good = runner.dump();
    assert!(fb_harness::compare(&good, &good).ok());
    good
}

fn rejected_by_audit(good: &Value, bad: &Value) {
    for (python, rust, side) in [
        (bad, bad, "python"),
        (bad, good, "python"),
        (good, bad, "rust"),
    ] {
        let report = fb_harness::compare(python, rust);
        assert!(
            report
                .failures
                .iter()
                .any(|f| f.contains(side) && f.contains("audit/document")),
            "missing independent {side} audit check: {:?}",
            report.failures
        );
    }
}

#[test]
fn trailing_zero_memo_line_cannot_disappear_from_identical_dumps() {
    let good = dump(true);
    assert_eq!(
        good["credits"]["creditmemoline"].as_array().unwrap().len(),
        3
    );
    let mut bad = good.clone();
    let removed = bad["credits"]["creditmemoline"]
        .as_array_mut()
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(removed["amount"], "0.00");
    assert_eq!(removed["description"], "Free replacement part");
    rejected_by_audit(&good, &bad);
}

#[test]
fn memo_line_tuples_must_equal_the_recorded_audit() {
    let good = dump(true);
    for field in ["position", "income_account_id", "amount"] {
        let mut bad = good.clone();
        let event = bad["audit"]["c"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["action"] == "credit_memo.recorded")
            .unwrap();
        event["detail"]["lines"][2][field] = match field {
            "position" => json!(4),
            "income_account_id" => json!("income"),
            _ => json!("0.01"),
        };
        rejected_by_audit(&good, &bad);
    }
}

#[test]
fn released_application_lines_need_complete_audit_allocations() {
    let good = dump(false);
    let mut bad = good.clone();
    // Released applications do not affect balances; audit evidence still requires every line.
    bad["credits"]["creditapplicationline"] = json!([]);
    rejected_by_audit(&good, &bad);
    let mut bad = good.clone();
    let event = bad["audit"]["c"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["action"] == "credit_memo.applied")
        .unwrap();
    let duplicate = event["detail"]["invoices"][0].clone();
    event["detail"]["invoices"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    rejected_by_audit(&good, &bad);
}

#[test]
fn refund_and_void_journal_lines_need_exact_audited_coverage() {
    let good = dump(false);
    for journal_id in [
        &good["credits"]["creditrefund"][0]["journal_id"],
        &good["credits"]["creditrefundvoid"][0]["journal_id"],
    ] {
        for extra_zero in [false, true] {
            let mut bad = good.clone();
            let journal = bad["journals"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|j| j["ref"] == *journal_id)
                .unwrap();
            let lines = journal["lines"].as_array_mut().unwrap();
            if extra_zero {
                let mut extra = lines[0].clone();
                extra["position"] = json!(3);
                extra["debit"] = json!(0);
                extra["credit"] = json!(0);
                lines.push(extra);
            } else {
                lines.pop();
            }
            rejected_by_audit(&good, &bad);
        }
    }
}

#[test]
fn refund_postings_must_agree_with_audited_amount_and_bank() {
    let good = dump(false);
    for (field, value) in [("amount", json!("2.01")), ("bank_id", json!("bank2"))] {
        let mut bad = good.clone();
        let event = bad["audit"]["c"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["action"] == "credit_refund.recorded")
            .unwrap();
        event["detail"][field] = value;
        rejected_by_audit(&good, &bad);
    }
}
