use serde_json::{Value, json};

fn dump(name: &str) -> Value {
    let scenario: Value = serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/scenarios/payables-{name}.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("strict-payables-{name}.fbbook"));
    let mut runner = fb_harness::Runner::new(&scenario, &path);
    runner.run();
    runner.dump()
}

fn paths(v: &Value, path: String, out: &mut Vec<String>) {
    out.push(path.clone());
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                paths(v, format!("{path}/{k}"), out);
            }
        }
        Value::Array(a) => {
            for (i, v) in a.iter().enumerate() {
                paths(v, format!("{path}/{i}"), out);
            }
        }
        _ => {}
    }
}

#[test]
fn all_payables_shapes_reject_identical_malformed_dumps_and_duplicate_rows() {
    let mut checked = 0;
    for name in [
        "boundaries",
        "payments",
        "lifecycle",
        "receipt-core",
        "filtered-tracker",
    ] {
        let good = dump(name);
        let report = fb_harness::compare(&good, &good);
        assert!(report.ok(), "{name}: {:?}", report.failures);
        let mut pointers = vec![];
        paths(&good["payables"], "/payables".into(), &mut pointers);
        for pointer in pointers {
            let mut bad = good.clone();
            *bad.pointer_mut(&pointer).unwrap() = json!(false);
            let result = std::panic::catch_unwind(|| fb_harness::compare(&bad, &bad));
            assert!(result.is_ok(), "panic {name}:{pointer}");
            assert!(
                !result.unwrap().ok(),
                "accepted identical wrong type {name}:{pointer}"
            );
            checked += 1;
        }
        for (table, rows) in good["payables"].as_object().unwrap() {
            let mut bad = good.clone();
            bad["payables"].as_object_mut().unwrap().remove(table);
            assert!(!fb_harness::compare(&bad, &bad).ok(), "missing {table}");
            if !rows.as_array().unwrap().is_empty() {
                let mut bad = good.clone();
                bad["payables"][table]
                    .as_array_mut()
                    .unwrap()
                    .push(rows[0].clone());
                assert!(!fb_harness::compare(&bad, &bad).ok(), "duplicate {table}");
                for field in rows[0].as_object().unwrap().keys() {
                    let mut bad = good.clone();
                    bad["payables"][table][0]
                        .as_object_mut()
                        .unwrap()
                        .remove(field);
                    assert!(
                        !fb_harness::compare(&bad, &bad).ok(),
                        "missing {table}/{field}"
                    );
                }
            }
        }
        for table in ["balances", "tracker", "calendar"] {
            let mut bad = good.clone();
            bad["payables"][table] = json!([]);
            assert!(
                !fb_harness::compare(&bad, &bad).ok(),
                "missing report {table}"
            );
        }
    }
    assert!(checked > 1000);
    println!("Rejected {checked} identical malformed payables structural paths");
}

#[test]
fn all_payables_audit_leaves_and_free_text_remain_significant() {
    let good = dump("payments");
    let mut pointers = vec![];
    paths(&good["audit"], "/audit".into(), &mut pointers);
    let mut checked = 0;
    for pointer in pointers {
        let v = good.pointer(&pointer).unwrap();
        if v.is_array() || v.is_object() {
            continue;
        }
        let mut bad = good.clone();
        *bad.pointer_mut(&pointer).unwrap() = if let Some(n) = v.as_i64() {
            json!(n + 1)
        } else if let Some(s) = v.as_str() {
            json!(format!("{s} changed"))
        } else if let Some(b) = v.as_bool() {
            json!(!b)
        } else {
            json!("non-null")
        };
        assert!(
            !fb_harness::compare(&good, &bad).ok(),
            "ignored audit {pointer}"
        );
        checked += 1;
    }
    assert!(checked > 100);
    println!("Compared {checked} full audit leaves exactly");
}

#[test]
fn filtered_tracker_checks_rows_totals_and_result_shape() {
    let good = dump("filtered-tracker");
    let filtered = &good["payables"]["tracker"][0]["filtered"];
    assert_eq!(
        filtered[0]["bill_ids"],
        json!([
            "accent",
            "decomposed",
            "dotless",
            "dotless-reverse",
            "dotted",
            "draft",
            "ligature",
            "literal",
            "paid",
            "sharp",
            "sigma",
            "void"
        ])
    );
    assert_eq!(filtered[0]["totals"]["open_money"], "9.09");
    for pointer in [
        "/payables/tracker/0/filtered/0/bill_ids/0",
        "/payables/tracker/0/filtered/0/totals/open_money",
    ] {
        let mut bad = good.clone();
        *bad.pointer_mut(pointer).unwrap() = json!("changed");
        assert!(!fb_harness::compare(&good, &bad).ok());
    }
    let mut bad = good.clone();
    bad["payables"]["tracker"][0]["filtered"][0]["bill_ids"] = json!(["unknown"]);
    assert!(!fb_harness::compare(&bad, &bad).ok());
    let mut bad = good.clone();
    let first = bad["payables"]["tracker"][0]["filtered"][0].clone();
    bad["payables"]["tracker"][0]["filtered"]
        .as_array_mut()
        .unwrap()
        .push(first);
    assert!(!fb_harness::compare(&bad, &bad).ok());
}

fn money_paths(v: &Value, path: String, out: &mut Vec<String>) {
    match v {
        Value::Object(fields) => {
            for (key, value) in fields {
                let next = format!("{path}/{key}");
                if matches!(
                    key.as_str(),
                    "allocated"
                        | "outstanding"
                        | "purchase_cost"
                        | "open_money"
                        | "overdue_money"
                        | "paid_recent_money"
                        | "total"
                        | "remaining"
                        | "month_total"
                ) && value.is_string()
                {
                    out.push(next);
                } else if key == "day_totals" {
                    for date in value.as_object().unwrap().keys() {
                        out.push(format!("{next}/{date}"));
                    }
                } else {
                    money_paths(value, next, out);
                }
            }
        }
        Value::Array(rows) => {
            for (i, row) in rows.iter().enumerate() {
                money_paths(row, format!("{path}/{i}"), out);
            }
        }
        _ => {}
    }
}

#[test]
fn every_payables_money_string_rejects_identical_corruption() {
    let mut checked = 0;
    for name in ["boundaries", "filtered-tracker"] {
        let good = dump(name);
        let mut pointers = vec![];
        money_paths(&good["payables"], "/payables".into(), &mut pointers);
        for pointer in pointers {
            for invalid in [
                "",
                "garbage",
                "NaN",
                "Infinity",
                "1",
                "1.0",
                "1.000",
                ".01",
                "01.00",
                "+1.00",
                "-0.01",
                "-0.00",
                " 1.00",
                "1.00 ",
                "1e2",
                "1_0.00",
                "١.00",
                "1.0x",
                "1.00.00",
                "999999999999999999999999999999999999999999.99",
            ] {
                let mut bad = good.clone();
                *bad.pointer_mut(&pointer).unwrap() = json!(invalid);
                let report = fb_harness::compare(&bad, &bad);
                assert!(!report.ok(), "accepted {name}:{pointer}={invalid}");
                assert_eq!(report.failures.len(), 2, "both dumps must fail validation");
                assert!(
                    report
                        .failures
                        .iter()
                        .all(|failure| failure.contains("invalid bounded money string")),
                    "money validation must reject {pointer} before calendar consistency: {:?}",
                    report.failures
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 3000);
    println!("Rejected {checked} identical money-string corruptions");
}

#[test]
fn payables_stored_amounts_enforce_python_bounds() {
    let good = dump("boundaries");
    for (table, fields, minimum, maximum) in [
        ("billrevision", vec!["total"], 0, 9_999_999_999_999_999_i64),
        ("billline", vec!["rate", "amount"], 0, 9_999_999_999_999_999),
        ("billline", vec!["quantity"], 1, 999_999_999_999_999_999),
        ("billpayment", vec!["amount"], 1, 9_999_999_999_999_999),
        (
            "paymentallocation",
            vec!["amount"],
            1,
            9_999_999_999_999_999,
        ),
    ] {
        assert!(!good["payables"][table].as_array().unwrap().is_empty());
        for field in fields {
            for invalid in [minimum - 1, maximum + 1, i64::MIN, i64::MAX] {
                let mut bad = good.clone();
                bad["payables"][table][0][field] = json!(invalid);
                assert!(
                    !fb_harness::compare(&bad, &bad).ok(),
                    "accepted {table}/{field}={invalid}"
                );
            }
        }
    }
}

#[test]
fn payables_report_keys_are_complete_even_for_identical_dumps() {
    let good = dump("boundaries");
    for table in ["balances", "tracker", "calendar"] {
        let mut bad = good.clone();
        bad["payables"][table].as_array_mut().unwrap().pop();
        assert!(!fb_harness::compare(&bad, &bad).ok(), "missing {table} row");
    }
    let totals = good["payables"]["calendar"][0]["data"]["day_totals"]
        .as_object()
        .unwrap();
    assert!(!totals.is_empty());
    for date in totals.keys() {
        let mut bad = good.clone();
        bad["payables"]["calendar"][0]["data"]["day_totals"]
            .as_object_mut()
            .unwrap()
            .remove(date);
        assert!(
            !fb_harness::compare(&bad, &bad).ok(),
            "missing day total {date}"
        );
    }
    let mut bad = good.clone();
    bad["payables"]["calendar"][0]["data"]["day_totals"]["unexpected"] = json!("0.00");
    assert!(!fb_harness::compare(&bad, &bad).ok());
    for field in ["entries", "days", "agenda", "weeks"] {
        let mut bad = good.clone();
        bad["payables"]["calendar"][0]["data"][field]
            .as_array_mut()
            .unwrap()
            .pop();
        assert!(
            !fb_harness::compare(&bad, &bad).ok(),
            "missing calendar {field}"
        );
    }
    // Even a company without bills must retain both reports.
    let mut scenario: Value =
        serde_json::from_str(include_str!("../scenarios/payables-boundaries.json")).unwrap();
    scenario["steps"].as_array_mut().unwrap().truncate(1);
    let dir = tempfile::tempdir().unwrap();
    let mut runner = fb_harness::Runner::new(&scenario, &dir.path().join("empty.fbbook"));
    runner.run();
    let mut bad = runner.dump();
    assert!(fb_harness::compare(&bad, &bad).ok());
    bad["payables"]["tracker"] = json!([]);
    bad["payables"]["calendar"] = json!([]);
    assert!(!fb_harness::compare(&bad, &bad).ok());
}

#[test]
fn payables_money_boundaries_and_large_aggregates_are_valid() {
    let mut scenario: Value =
        serde_json::from_str(include_str!("../scenarios/payables-boundaries.json")).unwrap();
    let steps = scenario["steps"].as_array_mut().unwrap();
    let first_bill = steps
        .iter()
        .position(|s| s["op"] == "payables_save")
        .unwrap();
    let mut bill = steps[first_bill].clone();
    steps.truncate(first_bill);
    bill["values"]["lines"][0]["quantity"] = json!("1");
    bill["values"]["lines"][0]["rate"] = json!("99999999999999.99");
    steps.push(bill.clone());
    steps.push(json!({"op":"payables_post", "company":"co", "bill":"b", "expected_revision":1}));
    bill["ref"] = json!("b2");
    bill["values"]["reference"] = json!("Second maximum bill");
    bill["values"]["request_key"] = json!("00000000-0000-0000-0000-000000000002");
    steps.push(bill);
    steps.push(json!({"op":"payables_post", "company":"co", "bill":"b2", "expected_revision":1}));
    let dir = tempfile::tempdir().unwrap();
    let mut runner = fb_harness::Runner::new(&scenario, &dir.path().join("maximum.fbbook"));
    runner.run();
    let good = runner.dump();
    assert_eq!(
        good["payables"]["tracker"][0]["totals"]["open_money"],
        "199999999999999.98"
    );
    assert_eq!(
        good["payables"]["calendar"][0]["data"]["month_total"],
        "199999999999999.98"
    );
    let report = fb_harness::compare(&good, &good);
    assert!(report.ok(), "{:?}", report.failures);
    for pointer in [
        "/payables/balances/0/allocated",
        "/payables/balances/0/outstanding",
        "/payables/billline/0/item_snapshot/purchase_cost",
        "/payables/calendar/0/data/entries/0/total",
        "/payables/calendar/0/data/entries/0/remaining",
    ] {
        for valid in ["0.00", "0.01", "99999999999999.99"] {
            // For calendar entries change each repeated copy together.
            if pointer.contains("/calendar/") {
                continue;
            }
            let mut boundary = good.clone();
            *boundary.pointer_mut(pointer).unwrap() = json!(valid);
            assert!(
                fb_harness::compare(&boundary, &boundary).ok(),
                "rejected boundary {pointer}={valid}"
            );
        }
        let mut bad = good.clone();
        *bad.pointer_mut(pointer).unwrap() = json!("100000000000000.00");
        assert!(
            !fb_harness::compare(&bad, &bad).ok(),
            "accepted overflow {pointer}"
        );
    }
    for pointer in [
        "/payables/tracker/0/totals/open_money",
        "/payables/calendar/0/data/month_total",
    ] {
        let mut bad = good.clone();
        *bad.pointer_mut(pointer).unwrap() = json!("199999999999999.99");
        assert!(
            !fb_harness::compare(&bad, &bad).ok(),
            "accepted aggregate overflow {pointer}"
        );
    }
}
