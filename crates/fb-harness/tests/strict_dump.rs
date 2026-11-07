mod common;
use serde_json::{Value, json};
#[test]
fn malformed_types_missing_keys_and_identical_bad_dumps_fail_without_panics() {
    let good = common::dump();
    for path in [
        "",
        "/outcomes",
        "/outcomes/0",
        "/outcomes/0/ok",
        "/companies",
        "/companies/0",
        "/companies/0/ref",
        "/accounts",
        "/accounts/0/code",
        "/journals",
        "/journals/0/memo",
        "/journals/0/lines",
        "/journals/0/lines/0/debit",
        "/display",
        "/display/j1",
        "/registers",
        "/registers/a",
        "/registers/a/opening",
        "/registers/a/rows",
        "/registers/a/rows/0",
        "/registers/a/rows/0/2",
        "/trial_balances",
        "/trial_balances/c1",
        "/trial_balances/c1/2026-12-31",
        "/trial_balances/c1/2026-12-31/rows",
        "/trial_balances/c1/2026-12-31/rows/0",
        "/trial_balances/c1/2026-12-31/rows/0/1",
        "/trial_balances/c1/2026-12-31/debit",
        "/audit",
        "/audit/c1",
        "/audit/c1/0",
        "/audit/c1/0/detail",
    ] {
        for wrong in [
            Value::Null,
            json!(42),
            json!("bad"),
            json!([]),
            json!({}),
            json!(false),
            json!(1.5),
        ] {
            let original = good.pointer(path).unwrap();
            if (original.is_string() && wrong.is_string())
                || (original.is_boolean() && wrong.is_boolean())
                || (original.as_i64().is_some() && wrong.as_i64().is_some())
            {
                continue;
            }
            let mut bad = good.clone();
            *bad.pointer_mut(path).unwrap() = wrong;
            if bad == good {
                continue;
            }
            // Empty event/row collections and arbitrary audit detail objects are valid.
            let replacement = bad.pointer(path).unwrap();
            if (replacement.is_array()
                && [
                    "/outcomes",
                    "/journals/0/lines",
                    "/registers/a/rows",
                    "/trial_balances/c1/2026-12-31/rows",
                    "/audit/c1",
                ]
                .contains(&path))
                || (path == "/audit/c1/0/detail" && replacement.is_object())
            {
                continue;
            }
            let report = fb_harness::compare(&good, &bad);
            assert!(!report.ok(), "accepted {path}: {bad}");
            // The same malformed type on both sides must not count as parity.
            assert!(
                !fb_harness::compare(&bad, &bad).ok(),
                "accepted identical malformed {path}: {bad}"
            );
        }
    }
    fn remove_each(value: &Value, root: &Value, path: &str) {
        match value {
            Value::Object(fields) => {
                for (key, child) in fields {
                    let mut bad = root.clone();
                    bad.pointer_mut(path)
                        .unwrap()
                        .as_object_mut()
                        .unwrap()
                        .remove(key);
                    // This fixture has one date and no optional annotations: every
                    // object field or report-map entry is required by its structure.
                    assert!(
                        !fb_harness::compare(&bad, &bad).ok(),
                        "accepted missing {path}/{key}"
                    );
                    remove_each(child, root, &format!("{path}/{key}"));
                }
            }
            Value::Array(rows) => {
                for (i, row) in rows.iter().enumerate() {
                    remove_each(row, root, &format!("{path}/{i}"));
                }
            }
            _ => {}
        }
    }
    remove_each(&good, &good, "");
}
#[test]
fn no_company_report_or_account_register_can_be_silently_omitted() {
    let good = common::dump();
    for path in [
        "/trial_balances/c1",
        "/trial_balances",
        "/audit",
        "/registers",
        "/display",
    ] {
        let mut bad = good.clone();
        *bad.pointer_mut(path).unwrap() = json!({});
        assert!(!fb_harness::compare(&bad, &bad).ok(), "{path}");
    }
}
