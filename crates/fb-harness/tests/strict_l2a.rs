use serde_json::{Value, json};

#[test]
fn every_l2a_dump_shape_is_validated_before_normalization_even_for_identical_bad_dumps() {
    let mut checked = 0;
    for name in ["directory", "numbering", "tax-rounding", "source-lifecycle"] {
        let scenario: Value = serde_json::from_str(
            &std::fs::read_to_string(format!(
                "{}/scenarios/l2a-{name}.json",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap(),
        )
        .unwrap();
        let path =
            std::env::temp_dir().join(format!("strict-l2a-{}-{name}.fbbook", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut runner = fb_harness::Runner::new(&scenario, &path);
        runner.run();
        let good = runner.dump();
        assert!(fb_harness::compare(&good, &good).ok(), "{name}");
        fn paths(value: &Value, path: String, out: &mut Vec<String>) {
            out.push(path.clone());
            match value {
                Value::Object(map) => {
                    for (key, value) in map {
                        paths(value, format!("{path}/{key}"), out);
                    }
                }
                Value::Array(rows) => {
                    for (index, value) in rows.iter().enumerate() {
                        paths(value, format!("{path}/{index}"), out);
                    }
                }
                _ => {}
            }
        }
        let mut mutations = vec![];
        paths(&good["l2a"], "/l2a".into(), &mut mutations);
        for pointer in mutations {
            let mut bad = good.clone();
            let original = good.pointer(&pointer).unwrap();
            let wrong = if original.is_object() || original.is_array() || original.is_boolean() {
                json!("wrong type")
            } else {
                json!(false)
            };
            *bad.pointer_mut(&pointer).unwrap() = wrong;
            for (py, rs) in [(&good, &bad), (&bad, &bad)] {
                let outcome = std::panic::catch_unwind(|| fb_harness::compare(py, rs));
                assert!(outcome.is_ok(), "panicked at {name}:{pointer}");
                assert!(!outcome.unwrap().ok(), "accepted {name}:{pointer}");
            }
            checked += 1;
        }
        for field in [
            "directory",
            "tax",
            "settings",
            "setup",
            "identities",
            "sequences",
            "snapshots",
            "results",
            "source_journals",
        ] {
            let mut bad = good.clone();
            bad["l2a"].as_object_mut().unwrap().remove(field);
            assert!(!fb_harness::compare(&bad, &bad).ok(), "missing {field}");
        }
        for field in ["settings", "setup"] {
            let mut bad = good.clone();
            let key = bad["l2a"][field]
                .as_object()
                .unwrap()
                .keys()
                .next()
                .unwrap()
                .clone();
            bad["l2a"][field].as_object_mut().unwrap().remove(&key);
            assert!(
                !fb_harness::compare(&bad, &bad).ok(),
                "missing company {field}"
            );
        }
    }
    assert!(checked > 300);
    println!("{checked} L2a structural corruptions rejected for unequal and identical dumps");
}
