mod common;
use serde_json::json;

#[test]
fn either_side_unexpected_outcome_is_a_failure_even_when_errors_match() {
    for side in ["python", "rust", "both"] {
        let mut py = common::dump();
        let mut rs = py.clone();
        if side != "rust" {
            py["outcomes"][0]["unexpected"] = json!(true);
        }
        if side != "python" {
            rs["outcomes"][0]["unexpected"] = json!(true);
        }
        let report = fb_harness::compare(&py, &rs);
        assert!(!report.ok(), "{side}: {:?}", report);
        assert!(report.warnings.is_empty());
    }
}

#[test]
fn checked_in_scenarios_pin_every_success_or_named_refusal() {
    let scenarios = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios");
    let mut missing = Vec::new();
    for entry in std::fs::read_dir(scenarios).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let scenario: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for (index, step) in scenario["steps"].as_array().unwrap().iter().enumerate() {
            if !step["expect_error"].is_boolean()
                || (step["expect_error"] == true
                    && (step["expect_kind"].as_str().is_none_or(str::is_empty)
                        || step["expect_message"].as_str().is_none_or(str::is_empty)))
                || (step["expect_error"] == false && step.get("expect_kind").is_some())
                || step["expect_message"] == ""
                || (matches!(step["expect_kind"].as_str(), Some("DoesNotExist" | "MissingRef"))
                    && !step["intent"].is_string())
            {
                missing.push(format!(
                    "{} step {index}",
                    path.file_name().unwrap().to_string_lossy()
                ));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "steps without an explicit intended outcome/reason:\n{}",
        missing.join("\n")
    );
}

#[test]
fn scenario_contract_rejects_matching_wrong_outcomes_without_unexpected_flags() {
    let scenario = json!({"steps":[{"op":"save_draft","expect_error":true,"expect_kind":"ValidationError"}]});
    let good = common::dump();
    assert!(fb_harness::compare_scenario(Some(&scenario), &good, &good).ok());
    for field in ["kind", "op", "ok", "step"] {
        let mut bad = good.clone();
        bad["outcomes"][0][field] = match field {
            "kind" => json!("DoesNotExist"),
            "op" => json!("wrong_op"),
            "ok" => json!(true),
            _ => json!(1),
        };
        let report = fb_harness::compare_scenario(Some(&scenario), &bad, &bad);
        assert!(!report.ok(), "{field}: {report:?}");
        for side in ["python", "rust"] {
            assert!(report.failures.iter().any(|f| f.starts_with(side) && f.contains("scenario intends")), "{report:?}");
        }
        assert!(report.warnings.is_empty());
    }
}

#[test]
fn scenario_contract_rejects_right_operation_refused_for_wrong_reason_on_each_side() {
    let scenario = json!({"steps":[{
        "op":"save_draft", "expect_error":true, "expect_kind":"ValidationError",
        "expect_message":"Enter a valid monetary amount."
    }]});
    let good = common::dump();
    assert!(fb_harness::compare_scenario(Some(&scenario), &good, &good).ok());
    for bad_side in ["python", "rust", "both"] {
        let mut py = good.clone();
        let mut rs = good.clone();
        let wrong_reason = json!("This record changed since you opened it. Reload and review before saving.");
        if bad_side != "rust" {
            py["outcomes"][0]["message"] = wrong_reason.clone();
        }
        if bad_side != "python" {
            rs["outcomes"][0]["message"] = wrong_reason;
        }
        let report = fb_harness::compare_scenario(Some(&scenario), &py, &rs);
        assert!(!report.ok(), "{bad_side}: {report:?}");
        for side in ["python", "rust"] {
            let rejected = report.failures.iter().any(|failure| {
                failure.starts_with(side) && failure.contains("scenario intends")
            });
            assert_eq!(rejected, bad_side == side || bad_side == "both", "{bad_side}/{side}: {report:?}");
        }
        if bad_side == "both" {
            assert!(report.warnings.is_empty());
        }
    }
}

#[test]
fn scenario_contract_requires_success_and_outcome_inventory_on_each_side() {
    let good = common::dump();
    for step in [json!({"op":"save_draft"}), json!({"op":"save_draft","expect_error":false})] {
        let scenario = json!({"steps":[step]});
        let report = fb_harness::compare_scenario(Some(&scenario), &good, &good);
        assert_eq!(report.failures.len(), 2, "{report:?}");
    }
    let scenario = json!({"steps":[{"op":"save_draft","expect_error":true,"expect_kind":"ValidationError"}]});
    let mut erased = good.clone();
    erased["outcomes"] = json!([]);
    for (py, rs) in [(&erased, &good), (&good, &erased), (&erased, &erased)] {
        let report = fb_harness::compare_scenario(Some(&scenario), py, rs);
        assert!(report.failures.iter().any(|f| f.contains("scenario's 1 steps")), "{report:?}");
    }
}

#[test]
fn scenario_contract_checks_checkpoint_inventory_against_driver() {
    let scenario = json!({"capture_steps":true,"users":[{"username":"expectations","is_staff":true}],"steps":[{"op":"create_company","ref":"c","name":"Synthetic outcome contract","expect_error":false}]});
    let dir = tempfile::tempdir().unwrap();
    let mut runner = fb_harness::Runner::new(&scenario, &dir.path().join("expectations.fbbook"));
    runner.run();
    let good = runner.dump();
    assert!(fb_harness::compare_scenario(Some(&scenario), &good, &good).ok());
    let mut erased = good.clone();
    erased["checkpoints"] = json!([]);
    erased["final"]["outcomes"] = json!([]);
    let report = fb_harness::compare_scenario(Some(&scenario), &erased, &erased);
    assert!(report.failures.iter().any(|f| f.contains("checkpoints: 0 for the scenario's 1 steps")), "{report:?}");
}

#[test]
fn random_prepayments_exercise_successful_live_records_and_releases() {
    use std::collections::{HashMap, HashSet};
    let scenario: serde_json::Value = serde_json::from_str(include_str!("../scenarios/prepayments-random-500.json")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut runner = fb_harness::Runner::new(&scenario, &dir.path().join("random-prepayments.fbbook"));
    runner.run();
    let dump = runner.dump();
    assert!(fb_harness::compare_scenario(Some(&scenario), &dump, &dump).ok());
    let mut live: HashMap<&str, HashSet<&str>> = HashMap::new();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for (step, outcome) in scenario["steps"].as_array().unwrap().iter().zip(dump["outcomes"].as_array().unwrap()) {
        let op = step["op"].as_str().unwrap();
        assert_ne!(outcome["kind"], "DoesNotExist", "{outcome}");
        assert_ne!(outcome["message"], "", "{outcome}");
        let release = match op {
            "pp_unapply" => Some("pp_apply"),
            "pp_void_refund" => Some("pp_record_refund"),
            "pp_void_deposit" => Some("pp_record_deposit"),
            _ => None,
        };
        if let Some(creation) = release {
            assert!(live.entry(creation).or_default().contains(step["record"].as_str().unwrap()), "{step}");
        }
        if outcome["ok"] == true {
            *counts.entry(op).or_default() += 1;
            if let Some(creation) = release {
                live.get_mut(creation).unwrap().remove(step["record"].as_str().unwrap());
            } else if matches!(op, "pp_apply" | "pp_record_refund" | "pp_record_deposit") {
                live.entry(op).or_default().insert(step["ref"].as_str().unwrap());
            }
        }
    }
    for op in ["pp_apply", "pp_unapply", "pp_record_refund", "pp_void_refund", "pp_void_deposit"] {
        assert_eq!(counts.get(op), Some(&50), "{op}: {counts:?}");
    }
    assert!(live.values().all(HashSet::is_empty));
    let combined: serde_json::Value = serde_json::from_str(include_str!("../scenarios/int-l3-combined.json")).unwrap();
    assert_eq!(combined["steps"].as_array().unwrap().len(), 108);
    for index in [75, 97, 98] {
        assert_eq!(combined["steps"][index]["record"], "undeposited-sale");
        assert_eq!(combined["steps"][index]["op"], "sales_void_sale");
    }
}
