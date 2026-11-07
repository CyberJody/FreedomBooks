use serde_json::{Value, json};

fn dump() -> Value {
    let mut scenario: Value =
        serde_json::from_str(include_str!("../scenarios/backups-lifecycle.json")).unwrap();
    scenario["steps"].as_array_mut().unwrap().truncate(8);
    let d = tempfile::tempdir().unwrap();
    let mut runner = fb_harness::Runner::new(&scenario, &d.path().join("book.fbbook"));
    runner.run();
    let dump = runner.dump();
    let python: Value = serde_json::from_str(include_str!("data/backups-manifest.json")).unwrap();
    assert_eq!(
        dump["backups"]["7"], python,
        "frozen Python raw-SQL manifest vector"
    );
    assert!(
        fb_harness::compare(&dump, &dump).ok(),
        "{:?}",
        fb_harness::compare(&dump, &dump).failures
    );
    dump
}
#[test]
fn comparer_rejects_identical_missing_zero_rows_malformed_values_and_results() {
    let valid = dump();
    for case in 0..14 {
        let mut bad = valid.clone();
        match case {
            0 => {
                bad.as_object_mut().unwrap().remove("backups");
            }
            1 => bad["backups"] = json!([]),
            2 => {
                bad["backups"].as_object_mut().unwrap().remove("7");
            }
            3 => {
                bad["backups"]["7"]["companies"][0]["accounts"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            }
            4 => bad["backups"]["7"]["companies"] = json!([]),
            5 => bad["backups"]["7"]["companies"][0]["accounts"][0]["debit_balance"] = json!("NaN"),
            6 => bad["backups"]["7"]["companies"][0]["journals"]["posted"] = json!(true),
            7 => bad["backups"]["7"]["companies"][0]["trial_balance"]["debit"] = json!(-1),
            9 => {
                bad["backups"]["7"]["companies"][0]["accounts"][0]["debit_balance"] =
                    json!("999.00")
            }
            10 => bad["backups"]["7"]["companies"][0]["trial_balance"]["debit"] = json!("999.00"),
            11 => {
                bad["backups"]["7"]["companies"][0]["accounts"][0]["posted_debits"] =
                    json!(format!("{}.00", "9".repeat(100)))
            }
            12 => bad["backups"]["7"]["companies"][0]["accounts"][0]["id"] = json!("missing"),
            13 => bad["backups"]["7"]["companies"][0]["accounts"]
                .as_array_mut()
                .unwrap()
                .reverse(),
            _ => {
                let row = bad["backups"]["7"]["companies"][0].clone();
                bad["backups"]["7"]["companies"]
                    .as_array_mut()
                    .unwrap()
                    .push(row);
            }
        }
        assert!(!fb_harness::compare(&bad, &bad).ok(), "case {case}");
    }
}

#[test]
fn review_two_company_negative_nets_and_retention_steps_must_succeed() {
    for raw in [
        include_str!("../scenarios/backups-two-companies.json"),
        include_str!("../scenarios/backups-retention.json"),
    ] {
        let scenario: Value = serde_json::from_str(raw).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut runner = fb_harness::Runner::new(&scenario, &dir.path().join("book.fbbook"));
        runner.run();
        let dump = runner.dump();
        for (step, outcome) in scenario["steps"]
            .as_array()
            .unwrap()
            .iter()
            .zip(dump["outcomes"].as_array().unwrap())
        {
            assert_eq!(outcome["ok"], step["expect_error"] == false, "{outcome}");
        }
        let result = fb_harness::compare(&dump, &dump);
        assert!(result.ok(), "{:?}", result.failures);
        if raw.contains("backups_retention") {
            for field in ["before", "removed", "after", "all_after", "notes_after"] {
                let mut bad = dump.clone();
                bad["backups"]["0"][field] = json!([]);
                assert!(
                    !fb_harness::compare(&bad, &bad).ok(),
                    "accepted malformed retention {field}"
                );
            }
        }
        if raw.contains("Credit balance asset") {
            let rows = dump["backups"].as_object().unwrap();
            assert_eq!(rows.len(), 3);
            for snapshot in rows.values() {
                assert_eq!(snapshot["company_count"], 2);
                assert_eq!(snapshot["account_count"], 8);
                for co in snapshot["companies"].as_array().unwrap() {
                    assert_eq!(co["journals"]["posted"], 1);
                    assert_eq!(co["journals"]["draft"], 0);
                    assert_eq!(
                        co["trial_balance"],
                        json!({"debit":"23.45","credit":"23.45"})
                    );
                    let cash = co["accounts"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|a| a["code"] == "1000")
                        .unwrap();
                    assert_eq!(cash["credit_balance"], "23.45");
                    assert_eq!(cash["debit_balance"], "0.00");
                    assert!(
                        co["accounts"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|a| a["code"] == "9000")
                    );
                }
            }
        }
    }
}
