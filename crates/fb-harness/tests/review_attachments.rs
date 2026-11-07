use serde_json::{Value, json};
fn dump(name: &str) -> Value {
    let mut scenario: Value = serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/scenarios/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap();
    scenario["capture_steps"] = json!(false);
    let dir = tempfile::tempdir().unwrap();
    let mut runner = fb_harness::Runner::new(&scenario, &dir.path().join("book.fbbook"));
    runner.run();
    let value = runner.dump();
    assert!(fb_harness::compare(&value, &value).ok());
    value
}

#[test]
fn review_empty_reminders_cannot_hide_existing_debt() {
    let mut bad = dump("attachments-reminders-payables-payments");
    let results = bad["attachments"]["results"].as_object_mut().unwrap();
    assert!(
        results
            .values()
            .any(|r| r["bills"]["count"].as_i64().is_some_and(|n| n > 0))
    );
    for result in results.values_mut() {
        if result.is_object() {
            for family in ["bills", "invoices"] {
                result[family] = json!({"count":0,"total":"0.00","overdue_count":0,"overdue_total":"0.00","capped":false,"rows":[]});
            }
        }
    }
    assert!(
        !fb_harness::compare(&bad, &bad).ok(),
        "comparer accepted empty reminders despite independent posted-bill evidence"
    );
}

#[test]
fn review_nav_count_cannot_be_arbitrary() {
    let mut bad = dump("attachments-reminders-payables-payments");
    let refs = bad["attachments"]["queries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|q| q["op"] == "reminder_count")
        .map(|q| q["ref"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert!(!refs.is_empty());
    for key in refs {
        bad["attachments"]["results"][key] = json!(999999);
    }
    assert!(
        !fb_harness::compare(&bad, &bad).ok(),
        "comparer accepted arbitrary nav count"
    );
}

#[test]
fn review_download_hash_must_match_attachment() {
    let mut bad = dump("attachments-lifecycle");
    let refs = bad["attachments"]["queries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|q| q["op"] == "attachment_download")
        .map(|q| q["ref"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert!(!refs.is_empty());
    for key in refs {
        bad["attachments"]["results"][key] = json!("0".repeat(64));
    }
    assert!(
        !fb_harness::compare(&bad, &bad).ok(),
        "comparer accepted download hash unrelated to stored attachment"
    );
}

#[test]
fn review_supported_date_grammars_are_normalized() {
    let mut scenario: Value = serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/scenarios/attachments-reminders-payables-payments.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap();
    scenario["capture_steps"] = json!(false);
    for step in scenario["steps"].as_array_mut().unwrap() {
        if step["op"] == "reminder_build" || step["op"] == "reminder_count" {
            step["today"] = json!("20261201");
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let mut runner = fb_harness::Runner::new(&scenario, &dir.path().join("dates.fbbook"));
    runner.run();
    let value = runner.dump();
    let report = fb_harness::compare(&value, &value);
    assert!(
        report.ok(),
        "supported compact date rejected: {:?}",
        report.failures
    );
}

#[test]
fn review_reminder_member_identity_cannot_change_without_source_evidence() {
    let good = dump("attachments-reminders-payables-payments");
    let key = good["attachments"]["results"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, r)| r["bills"]["count"].as_u64().is_some_and(|n| n > 0))
        .unwrap()
        .0;
    for field in ["document_id", "number", "party"] {
        let mut bad = good.clone();
        bad["attachments"]["results"][key]["bills"]["rows"][0][field] =
            json!("unrelated source identity");
        assert!(
            !fb_harness::compare(&bad, &bad).ok(),
            "accepted corrupted member {field}"
        );
    }
}
#[test]
fn review_default_window_and_download_identity_require_evidence() {
    let good = dump("attachments-reminders-supported-dates");
    let q = good["attachments"]["queries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|q| q["op"] == "reminder_build" && q["days"].is_null())
        .unwrap();
    let key = q["ref"].as_str().unwrap();
    let mut bad = good.clone();
    // Keep the date arithmetic consistent while changing an implicit configured window.
    bad["attachments"]["results"][key]["days"] = json!(0);
    bad["attachments"]["results"][key]["through"] =
        bad["attachments"]["results"][key]["today"].clone();
    assert!(!fb_harness::compare(&bad, &bad).ok());
    let good = dump("attachments-lifecycle");
    for field in ["attachment", "audit_count"] {
        let mut bad = good.clone();
        let q = bad["attachments"]["queries"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|q| q["op"] == "attachment_download")
            .unwrap();
        q[field] = if field == "attachment" {
            json!("missing-attachment")
        } else {
            json!(0)
        };
        assert!(!fb_harness::compare(&bad, &bad).ok(), "download {field}");
    }
}

#[test]
fn review_capped_groups_require_all_source_members_and_full_totals() {
    let good = dump("attachments-reminders-capped");
    let original = &good["attachments"]["results"]["capped"]["bills"];
    assert_eq!(original["count"], 31);
    assert_eq!(original["rows"].as_array().unwrap().len(), 25);
    for truncated in [false, true] {
        let mut bad = good.clone();
        let group = &mut bad["attachments"]["results"]["capped"]["bills"];
        group["total"] = json!("25.25");
        group["overdue_total"] = json!("25.25");
        if truncated {
            group["count"] = json!(25);
            group["overdue_count"] = json!(25);
            group["capped"] = json!(false);
        }
        assert!(
            !fb_harness::compare(&bad, &bad).ok(),
            "accepted capped corruption, truncated={truncated}"
        );
    }
}
#[test]
fn review_all_supported_date_inputs_match_canonical_results() {
    let value = dump("attachments-reminders-supported-dates");
    assert_eq!(
        value["attachments"]["queries"].as_array().unwrap().len(),
        12
    );
    assert!(
        value["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|o| o["ok"] == true)
    );
}

#[test]
fn reminder_evidence_survives_a_later_document_retry_alias() {
    let mut scenario: Value = serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/scenarios/attachments-reminders-payables-payments.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap();
    scenario["capture_steps"] = json!(true);
    scenario["steps"].as_array_mut().unwrap().truncate(8);
    let original: Value = serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/scenarios/attachments-reminders-payables-payments.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap();
    let mut create = original["steps"][8].clone();
    create["ref"] = json!("z-bill");
    let steps = scenario["steps"].as_array_mut().unwrap();
    steps.push(create.clone());
    steps.push(json!({"op":"payables_post","company":"co","bill":"z-bill","expected_revision":1}));
    steps.push(json!({"op":"reminder_build","company":"co","today":"2026-12-01","ref":"before"}));
    create["ref"] = json!("a-bill");
    steps.push(create);
    steps.push(json!({"op":"reminder_build","company":"co","today":"2026-12-01","ref":"after"}));
    let dir = tempfile::tempdir().unwrap();
    let mut runner = fb_harness::Runner::new(&scenario, &dir.path().join("aliases.fbbook"));
    runner.run();
    let value = runner.dump();
    assert!(
        value["final"]["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|o| o["ok"] == true)
    );
    let report = fb_harness::compare(&value, &value);
    assert!(
        report.ok(),
        "valid retry alias rejected: {:?}",
        report.failures
    );
}
