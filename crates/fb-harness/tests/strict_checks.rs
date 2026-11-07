use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
static NEXT: AtomicUsize = AtomicUsize::new(0);
fn dump(name: &str) -> Value {
    let scenario: Value = serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/scenarios/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap();
    let path = std::env::temp_dir().join(format!(
        "strict-checks-{}-{name}-{}.fbbook",
        std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&path);
    let mut runner = fb_harness::Runner::new(&scenario, &path);
    runner.run();
    runner.dump()
}
fn paths(v: &Value, p: String, out: &mut Vec<String>) {
    out.push(p.clone());
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                paths(v, format!("{p}/{k}"), out)
            }
        }
        Value::Array(a) => {
            for (i, v) in a.iter().enumerate() {
                paths(v, format!("{p}/{i}"), out)
            }
        }
        _ => {}
    }
}
#[test]
fn checks_shapes_reject_identical_malformed_dumps() {
    let mut tested = 0;
    for name in [
        "checks-lifecycle",
        "checks-numbering",
        "transfers-lifecycle",
    ] {
        let good = dump(name);
        let r = fb_harness::compare(&good, &good);
        assert!(r.ok(), "{:?}", r.failures);
        let mut pointers = vec![];
        paths(&good["checks"], "/checks".into(), &mut pointers);
        for pointer in pointers {
            let mut bad = good.clone();
            *bad.pointer_mut(&pointer).unwrap() = json!(false);
            assert!(!fb_harness::compare(&bad, &bad).ok(), "{pointer}");
            tested += 1;
        }
        for (table, values) in good["checks"].as_object().unwrap() {
            let mut bad = good.clone();
            bad["checks"].as_object_mut().unwrap().remove(table);
            assert!(!fb_harness::compare(&bad, &bad).ok(), "missing {table}");
            if let Some(a) = values.as_array().filter(|a| !a.is_empty()) {
                let mut bad = good.clone();
                bad["checks"][table]
                    .as_array_mut()
                    .unwrap()
                    .push(a[0].clone());
                assert!(!fb_harness::compare(&bad, &bad).ok(), "duplicate {table}");
                for key in a[0].as_object().unwrap().keys() {
                    let mut bad = good.clone();
                    bad["checks"][table][0].as_object_mut().unwrap().remove(key);
                    assert!(
                        !fb_harness::compare(&bad, &bad).ok(),
                        "missing {table}/{key}"
                    );
                }
            }
        }
    }
    assert!(tested > 500);
    println!("Rejected {tested} identical malformed structural paths");
}
#[test]
fn checks_audit_payloads_and_free_text_remain_exact() {
    for name in ["checks-lifecycle", "transfers-lifecycle"] {
        let good = dump(name);
        let mut pointers = vec![];
        paths(&good["audit"], "/audit".into(), &mut pointers);
        for pointer in pointers {
            let value = good.pointer(&pointer).unwrap();
            if value.is_array() || value.is_object() {
                continue;
            }
            let mut bad = good.clone();
            *bad.pointer_mut(&pointer).unwrap() = json!(format!("{value} changed"));
            assert!(!fb_harness::compare(&good, &bad).ok(), "{pointer}");
        }
    }
}
