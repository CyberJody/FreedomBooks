use serde_json::{Value, json};

fn restored() -> (Value, Value) {
    let mut scenario: Value =
        serde_json::from_str(include_str!("../scenarios/backups-lifecycle.json")).unwrap();
    scenario["capture_steps"] = json!(true);
    let steps = scenario["steps"].as_array_mut().unwrap();
    steps.truncate(8);
    steps.extend([
        json!({"op":"backups_verify","backup":"first","expect_error":false}),
        json!({"op":"backups_restore","backup":"first","expect_error":false}),
        json!({"op":"backups_live_post","company":"c","account":"cash","expect_error":false,
            "draft":{"op":"save_draft","company":"c","ref":"live-only","date":"2026-10-31","memo":"Live only","lines":[{"account":"cash","debit":"4.56","credit":"0"},{"account":"capital","debit":"0","credit":"4.56"}]},
            "post":{"op":"post_journal","company":"c","journal":"live-only"},
            "expected":{"kind":"isolation","live_before":12345,"live_after":12801,"restored":12345}}),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let mut runner = fb_harness::Runner::new(&scenario, &dir.path().join("live.fbbook"));
    runner.run();
    let dump = runner.dump();
    let report = fb_harness::compare_scenario(Some(&scenario), &dump, &dump);
    assert!(report.ok(), "{:?}", report.failures);
    assert_eq!(dump["final"]["registers"]["cash"]["closing"], 12345);
    assert!(
        dump["final"]["journals"]
            .as_array()
            .unwrap()
            .iter()
            .all(|j| j["ref"] != "live-only")
    );
    (scenario, dump)
}

#[test]
fn restore_opens_target_and_live_mutations_do_not_enter_restored_reports() {
    restored();
}

#[test]
fn identical_live_fallback_and_forged_restore_evidence_fail_closed() {
    let (scenario, valid) = restored();
    for case in 0..7 {
        let mut bad = valid.clone();
        let last = bad["checkpoints"].as_array().unwrap().len() - 1;
        {
            let dump = &mut bad["final"];
            match case {
                0 => {
                    dump.as_object_mut().unwrap().remove("book_identity");
                }
                1 => dump["book_identity"]["selected"] = dump["book_identity"]["live"].clone(),
                2 => dump["book_identity"]["sha256"] = json!("0".repeat(64)),
                3 => dump["book_identity"]["backup"] = json!("different"),
                4 => dump["book_identity"]["history"] = json!([]),
                5 => dump["backups"]["10"]["live_after"] = json!(12345),
                _ => dump["registers"]["cash"]["closing"] = json!(12801),
            }
        }
        bad["checkpoints"][last]["dump"] = bad["final"].clone();
        let report = fb_harness::compare_scenario(Some(&scenario), &bad, &bad);
        assert!(
            !report.ok(),
            "accepted identical forged evidence case {case}"
        );
    }
}

#[test]
fn restored_snapshot_discards_later_live_post_and_repeated_restore_stays_selected() {
    let scenario: Value =
        serde_json::from_str(include_str!("../scenarios/backups-lifecycle.json")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut runner = fb_harness::Runner::new(&scenario, &dir.path().join("live.fbbook"));
    runner.run();
    let dump = runner.dump();
    let report = fb_harness::compare_scenario(Some(&scenario), &dump, &dump);
    assert!(report.ok(), "{:?}", report.failures);
    assert_eq!(dump["book_identity"]["restore_step"], 13);
    assert_eq!(dump["book_identity"]["backup"], "second");
    assert_eq!(dump["registers"]["cash"]["closing"], 12345);
    assert_eq!(
        dump["journals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|j| j["ref"] == "draft")
            .unwrap()["status"],
        "DRAFT"
    );
}
