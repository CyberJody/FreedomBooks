use serde_json::{Value, json};
#[test]
fn r2_checkpoint_audit_boundary_must_be_anchored_to_query_step() {
    let mut scenario: Value = serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/scenarios/attachments-reminders-payables-payments.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap();
    scenario["capture_steps"] = json!(true);
    let dir = tempfile::tempdir().unwrap();
    let mut runner = fb_harness::Runner::new(&scenario, &dir.path().join("book.fbbook"));
    runner.run();
    let mut bad = runner.dump();
    assert!(fb_harness::compare(&bad, &bad).ok());
    fn erase(v: &mut Value) {
        for q in v["attachments"]["queries"].as_array_mut().unwrap() {
            q["audit_count"] = json!(0);
        }
        for result in v["attachments"]["results"]
            .as_object_mut()
            .unwrap()
            .values_mut()
        {
            if result.is_object() {
                for family in ["bills", "invoices"] {
                    result[family] = json!({"count":0,"total":"0.00","overdue_count":0,"overdue_total":"0.00","capped":false,"rows":[]});
                }
            } else {
                *result = json!(0);
            }
        }
    }
    erase(&mut bad["final"]);
    for cp in bad["checkpoints"].as_array_mut().unwrap() {
        erase(&mut cp["dump"]);
    }
    let report = fb_harness::compare(&bad, &bad);
    assert!(
        !report.ok(),
        "full checkpoint envelope accepted empty reminders with audit_count=0 despite preserved audit/source evidence: {:?}",
        report.failures
    );
}
