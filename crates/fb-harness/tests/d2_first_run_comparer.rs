mod common;
use fb_harness::compare_scenario;
use serde_json::json;

#[test]
fn first_run_results_cannot_be_erased_or_lied_about_on_both_sides() {
    let mut dump = common::dump();
    let expected = json!({"empty":true,"revision":0,"chosen":{},"reasons":{},"checklist":{"chart":false,"special":false,"tax":false,"customer":false,"vendor":false,"item":false}});
    let scenario = json!({"steps":[{"op":"l2_first_run_preview","company":"c1","result":"preview","expect_error":false,"expect_result":expected}]});
    dump["outcomes"] = json!([{"audit_counts":{"c1":1},"step":0,"op":"l2_first_run_preview","ok":true}]);
    dump["l2a"]["results"]["preview"] = expected;
    let report=compare_scenario(Some(&scenario), &dump, &dump);
    assert!(report.ok(), "valid fixture refused: {:?}", report.failures);
    let mut erased = dump.clone();
    erased["l2a"]["results"] = json!({});
    assert!(!compare_scenario(Some(&scenario), &erased, &erased).ok());
    let mut lie = dump.clone();
    lie["l2a"]["results"]["preview"]["checklist"]["special"] = json!(true);
    assert!(!compare_scenario(Some(&scenario), &lie, &lie).ok());
    let mut missing = scenario.clone();
    missing["steps"][0]
        .as_object_mut()
        .unwrap()
        .remove("expect_result");
    assert!(!compare_scenario(Some(&missing), &dump, &dump).ok());
}
