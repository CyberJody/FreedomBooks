mod common;
use fb_harness::compare_scenario;
use serde_json::json;

#[test]
fn reminder_results_are_checked_against_authored_expectations_even_if_both_sides_lie() {
    let mut dump = common::dump();
    let group = json!({"count":1,"total":"12.00","overdue_count":1,"overdue_total":"12.00","capped":false,"rows":[{"document_id":"inv","number":"INV-000001","party":"Buyer","due_date":"2026-10-31","days":-1,"balance":"12.00","overdue":true,"when":"1 day overdue"}]});
    let empty = json!({"count":0,"total":"0.00","overdue_count":0,"overdue_total":"0.00","capped":false,"rows":[]});
    let expected = json!({"today":"2026-11-01","days":10,"through":"2026-11-11","bills":empty,"invoices":group});
    let scenario = json!({"steps":[{"op":"l2_reminders","company":"c1","result":"reminders","expect_error":false,"expect_result":expected}]});
    dump["outcomes"] = json!([{"audit_counts":{"c1":1},"step":0,"op":"l2_reminders","ok":true}]);
    dump["l2a"]["results"]["reminders"] = expected;
    let report=compare_scenario(Some(&scenario), &dump, &dump);
    assert!(report.ok(), "valid fixture refused: {:?}", report.failures);
    let mut erased = dump.clone();
    erased["l2a"]["results"] = json!({});
    assert!(!compare_scenario(Some(&scenario), &erased, &erased).ok());
    for field in ["rows", "count", "total", "overdue_total", "capped"] {
        let mut lie = dump.clone();
        lie["l2a"]["results"]["reminders"]["invoices"][field] = match field {
            "rows" => json!([]),
            "count" => json!(9999),
            "capped" => json!(true),
            _ => json!("0.00"),
        };
        assert!(
            !compare_scenario(Some(&scenario), &lie, &lie).ok(),
            "accepted changed {field}"
        );
    }
    let mut missing = scenario.clone();
    missing["steps"][0]
        .as_object_mut()
        .unwrap()
        .remove("expect_result");
    assert!(!compare_scenario(Some(&missing), &dump, &dump).ok());
}
