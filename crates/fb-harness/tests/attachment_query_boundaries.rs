use serde_json::{Value, json};

fn dump(name: &str, checkpoints: bool) -> Value {
    let mut scenario: Value = serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/scenarios/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap();
    scenario["capture_steps"] = json!(checkpoints);
    let dir = tempfile::tempdir().unwrap();
    let mut runner = fb_harness::Runner::new(&scenario, &dir.path().join("boundaries.fbbook"));
    runner.run();
    let good = runner.dump();
    let report = fb_harness::compare(&good, &good);
    assert!(report.ok(), "{:?}", report.failures);
    good
}

// Mutate every copy consistently, including the final checkpoint, so rejection
// cannot come merely from two unequal copies of the same generated result.
fn each_dump(v: &mut Value, mutate: impl Fn(&mut Value)) {
    if v.get("final").is_some() {
        mutate(&mut v["final"]);
        for cp in v["checkpoints"].as_array_mut().unwrap() {
            mutate(&mut cp["dump"]);
        }
    } else {
        mutate(v);
    }
}

fn corrupt_reminders(v: &mut Value, cutoffs: bool, groups: bool, nav: bool) {
    for q in v["attachments"]["queries"].as_array_mut().unwrap() {
        if cutoffs {
            q["audit_count"] = json!(0);
        }
    }
    for result in v["attachments"]["results"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        if result.is_object() && groups {
            for family in ["bills", "invoices"] {
                result[family] = json!({"count":0,"total":"0.00","overdue_count":0,
                    "overdue_total":"0.00","capped":false,"rows":[]});
            }
        } else if result.is_number() && nav {
            *result = json!(0);
        }
    }
}

#[test]
fn identical_corruption_is_rejected_in_final_only_and_full_checkpoint_dumps() {
    for checkpoints in [false, true] {
        let good = dump("attachments-reminders-payables-payments", checkpoints);
        for (label, cutoffs, groups, nav) in [
            ("zero cutoffs", true, false, false),
            ("empty groups with consistent totals", false, true, false),
            ("zero nav counts", false, false, true),
            ("all three together", true, true, true),
        ] {
            let mut bad = good.clone();
            each_dump(&mut bad, |v| corrupt_reminders(v, cutoffs, groups, nav));
            let report = fb_harness::compare(&bad, &bad);
            assert!(!report.ok(), "accepted {label}, checkpoints={checkpoints}");
        }
    }
}

#[test]
fn selectors_must_match_the_independent_driver_request() {
    for checkpoints in [false, true] {
        let good = dump("attachments-reminders-payables-payments", checkpoints);
        for (field, value) in [
            ("today", json!("2020-01-01")),
            ("days", json!(999)),
            ("company", json!("other")),
            ("ref", json!("renamed-query")),
            ("step", json!(0)),
            ("op", json!("reminder_count")),
        ] {
            let mut bad = good.clone();
            each_dump(&mut bad, |v| {
                // With the old comparer this changes every request and result to
                // a valid, self-consistent empty view of the wrong date/window.
                corrupt_reminders(v, false, true, true);
                for q in v["attachments"]["queries"].as_array_mut().unwrap() {
                    q[field] = value.clone();
                }
                for result in v["attachments"]["results"]
                    .as_object_mut()
                    .unwrap()
                    .values_mut()
                {
                    if result.is_object() && field == "today" {
                        let days = result["days"].as_i64().unwrap();
                        let today = chrono::NaiveDate::from_ymd_opt(2020, 1, 1).unwrap();
                        result["today"] = value.clone();
                        result["through"] =
                            json!((today + chrono::Duration::days(days)).to_string());
                    }
                }
            });
            assert!(
                !fb_harness::compare(&bad, &bad).ok(),
                "accepted {field}, checkpoints={checkpoints}"
            );
        }
    }
}

#[test]
fn missing_or_forged_driver_evidence_cannot_authorize_a_cutoff() {
    for checkpoints in [false, true] {
        let good = dump("attachments-reminders-payables-payments", checkpoints);
        for field in ["query", "audit_counts"] {
            let mut bad = good.clone();
            each_dump(&mut bad, |v| {
                for outcome in v["outcomes"].as_array_mut().unwrap() {
                    outcome.as_object_mut().unwrap().remove(field);
                }
            });
            assert!(!fb_harness::compare(&bad, &bad).ok(), "missing {field}");
        }
        let mut bad = good.clone();
        each_dump(&mut bad, |v| {
            corrupt_reminders(v, true, true, true);
            for outcome in v["outcomes"].as_array_mut().unwrap() {
                for count in outcome["audit_counts"]
                    .as_object_mut()
                    .unwrap()
                    .values_mut()
                {
                    *count = json!(0);
                }
            }
        });
        assert!(
            !fb_harness::compare(&bad, &bad).ok(),
            "forged counts disagree with checkpoint audit"
        );
    }
}

#[test]
fn download_target_is_anchored_even_when_another_attachment_has_the_same_hash() {
    for checkpoints in [false, true] {
        let mut scenario: Value = serde_json::from_str(
            &std::fs::read_to_string(format!(
                "{}/scenarios/attachments-lifecycle.json",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap(),
        )
        .unwrap();
        scenario["capture_steps"] = json!(checkpoints);
        let steps = scenario["steps"].as_array_mut().unwrap();
        let first = steps
            .iter()
            .position(|s| s["op"] == "attachment_add")
            .unwrap();
        steps.truncate(first + 1);
        let mut second = steps[first].clone();
        let original = second["ref"].clone();
        second["ref"] = json!("second-attachment");
        second["request_key"] = json!("00000000-0000-0000-0000-00000000f202");
        steps.push(second);
        steps.push(json!({"op":"attachment_download", "company":"co", "attachment":original, "ref":"download-first"}));
        let dir = tempfile::tempdir().unwrap();
        let mut runner = fb_harness::Runner::new(&scenario, &dir.path().join("download.fbbook"));
        runner.run();
        let mut bad = runner.dump();
        let report = fb_harness::compare(&bad, &bad);
        assert!(report.ok(), "{:?}", report.failures);
        let current = if checkpoints { &bad["final"] } else { &bad };
        assert!(
            current["outcomes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|o| o["ok"] == true)
        );
        assert_eq!(current["attachments"]["rows"].as_array().unwrap().len(), 2);
        each_dump(&mut bad, |v| {
            for q in v["attachments"]["queries"].as_array_mut().unwrap() {
                q["attachment"] = json!("second-attachment");
            }
        });
        assert!(
            !fb_harness::compare(&bad, &bad).ok(),
            "same-hash target substitution"
        );
    }
}
