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
fn paths(v: &Value, path: String, out: &mut Vec<String>) {
    out.push(path.clone());
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                paths(v, format!("{path}/{k}"), out)
            }
        }
        Value::Array(xs) => {
            for (i, v) in xs.iter().enumerate() {
                paths(v, format!("{path}/{i}"), out)
            }
        }
        _ => {}
    }
}
#[test]
fn attachment_and_reminder_shapes_reject_identical_malformed_dumps() {
    let mut tested = 0;
    for name in [
        "attachments-lifecycle",
        "attachments-reminders-payables-payments",
    ] {
        let good = dump(name);
        let mut pointers = vec![];
        paths(&good["attachments"], "/attachments".into(), &mut pointers);
        for path in pointers {
            let mut bad = good.clone();
            let old = bad.pointer_mut(&path).unwrap();
            *old = if old.is_boolean() {
                json!("not boolean")
            } else {
                json!(false)
            };
            assert!(!fb_harness::compare(&bad, &bad).ok(), "{path}");
            tested += 1;
        }
        let mut bad = good.clone();
        bad["attachments"]["rows"] = json!([]);
        if !good["attachments"]["rows"].as_array().unwrap().is_empty() {
            assert!(!fb_harness::compare(&bad, &bad).ok());
        }
        for field in ["rows", "queries", "results"] {
            let mut bad = good.clone();
            bad["attachments"].as_object_mut().unwrap().remove(field);
            assert!(!fb_harness::compare(&bad, &bad).ok());
        }
    }
    assert!(tested > 100);
}
#[test]
fn semantic_mutations_and_missing_capped_rows_cannot_match_themselves() {
    let good = dump("attachments-reminders-payables-payments");
    let key = good["attachments"]["results"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, v)| v["bills"]["count"].as_i64().is_some_and(|n| n > 0))
        .unwrap()
        .0;
    for (path, value) in [
        (
            format!("/attachments/results/{key}/bills/total"),
            json!("NaN"),
        ),
        (
            format!("/attachments/results/{key}/bills/rows/0/balance"),
            json!("10000000000000000000000000000000000000000000000000.00"),
        ),
        (
            format!("/attachments/results/{key}/bills/rows/0/due_date"),
            json!("2026-02-30"),
        ),
    ] {
        let mut bad = good.clone();
        *bad.pointer_mut(&path).unwrap() = value;
        assert!(!fb_harness::compare(&bad, &bad).ok(), "{path}");
    }
    let mut bad = good.clone();
    let g = &mut bad["attachments"]["results"][key]["bills"];
    g["count"] = json!(30);
    g["capped"] = json!(true);
    g["rows"] = json!([]);
    assert!(
        !fb_harness::compare(&bad, &bad).ok(),
        "an empty capped group is incomplete"
    );
}
