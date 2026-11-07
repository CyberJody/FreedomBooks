use serde_json::{Value, json};

fn paths(v: &Value, path: String, all: &mut Vec<String>) {
    all.push(path.clone());
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                paths(v, format!("{path}/{k}"), all);
            }
        }
        Value::Array(a) => {
            for (i, v) in a.iter().enumerate() {
                paths(v, format!("{path}/{i}"), all);
            }
        }
        _ => {}
    }
}
#[test]
fn all_populated_sales_shapes_reject_identically_malformed_dumps_before_comparison() {
    let mut count = 0;
    for name in ["invoice-payments", "tax-group-retry", "cash-sales"] {
        let scenario: Value = serde_json::from_str(
            &std::fs::read_to_string(format!(
                "{}/scenarios/sales-{name}.json",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap(),
        )
        .unwrap();
        let path =
            std::env::temp_dir().join(format!("strict-sales-{}-{name}.fbbook", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut runner = fb_harness::Runner::new(&scenario, &path);
        runner.run();
        let good = runner.dump();
        assert!(fb_harness::compare(&good, &good).ok());
        let mut all = vec![];
        paths(&good["sales"], "/sales".into(), &mut all);
        for path in all {
            let original = good.pointer(&path).unwrap();
            let wrong = if original.is_object() || original.is_array() || original.is_boolean() {
                json!("bad type")
            } else {
                json!(false)
            };
            let mut bad = good.clone();
            *bad.pointer_mut(&path).unwrap() = wrong;
            for (p, r) in [(&good, &bad), (&bad, &bad)] {
                let result = std::panic::catch_unwind(|| fb_harness::compare(p, r));
                assert!(result.is_ok(), "panicked: {name}{path}");
                assert!(!result.unwrap().ok(), "accepted: {name}{path}");
            }
            if let Some(map) = original.as_object() {
                for key in map.keys() {
                    let mut bad = good.clone();
                    bad.pointer_mut(&path)
                        .unwrap()
                        .as_object_mut()
                        .unwrap()
                        .remove(key);
                    // Account snapshots are maps, so a role may be absent at the shape level.
                    if path.ends_with("/account_snapshot") {
                        continue;
                    }
                    assert!(
                        !fb_harness::compare(&bad, &bad).ok(),
                        "missing: {name}{path}/{key}"
                    );
                }
            }
            if let Some(rows) = original.as_array().filter(|r| !r.is_empty()) {
                let mut bad = good.clone();
                bad.pointer_mut(&path)
                    .unwrap()
                    .as_array_mut()
                    .unwrap()
                    .push(rows[0].clone());
                assert!(
                    !fb_harness::compare(&bad, &bad).ok(),
                    "duplicate: {name}{path}"
                );
            }
            count += 1;
        }
    }
    assert!(count > 500);
    println!(
        "Validated {count} populated sales structural paths, plus missing fields and duplicate rows"
    );
}

#[test]
fn full_sales_audit_payload_leaves_remain_significant() {
    let scenario: Value =
        serde_json::from_str(include_str!("../scenarios/sales-invoice-payments.json")).unwrap();
    let path =
        std::env::temp_dir().join(format!("sales-audit-corrupt-{}.fbbook", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut runner = fb_harness::Runner::new(&scenario, &path);
    runner.run();
    let good = runner.dump();
    let mut all = vec![];
    paths(&good["audit"], "/audit".into(), &mut all);
    let mut count = 0;
    for path in all {
        let value = good.pointer(&path).unwrap();
        if value.is_object() || value.is_array() {
            continue;
        }
        let mut bad = good.clone();
        *bad.pointer_mut(&path).unwrap() = match value {
            Value::String(s) => json!(format!("{s} changed")),
            Value::Number(_) => json!(-12345),
            Value::Bool(b) => json!(!b),
            _ => json!("changed"),
        };
        assert!(
            !fb_harness::compare(&good, &bad).ok(),
            "ignored audit: {path}"
        );
        count += 1;
    }
    assert!(count > 200);
}
