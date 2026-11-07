use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
static NEXT: AtomicUsize = AtomicUsize::new(0);
fn dump() -> Value {
    scenario_dump("checks-lifecycle")
}
fn scenario_dump(name: &str) -> Value {
    let scenario: Value = serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/scenarios/{name}.json",
            env!("CARGO_MANIFEST_DIR"),
        ))
        .unwrap(),
    )
    .unwrap();
    let path = std::env::temp_dir().join(format!(
        "review-checks-comparer-{}-{}.fbbook",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&path);
    let mut runner = fb_harness::Runner::new(&scenario, &path);
    runner.run();
    runner.dump()
}

#[test]
fn transfer_comparer_rejects_invalid_values_snapshots_and_duplicate_evidence() {
    let good = scenario_dump("transfers-lifecycle");
    assert!(fb_harness::compare(&good, &good).ok());
    for (path, bad_value) in [
        ("/checks/transfer/0/date", json!("2026-02-30")),
        ("/checks/transfer/0/amount", json!(0)),
        (
            "/checks/transfer/0/amount",
            json!(10_000_000_000_000_000_i64),
        ),
        ("/checks/transfer/0/from_snapshot", json!({})),
        ("/checks/transfer/0/to_snapshot/id", json!("wrong")),
        ("/checks/transfer/0/company_snapshot/currency", json!(false)),
        ("/checks/transfervoid/0/date", json!("0001-01-01")),
        ("/checks/transfervoid/0/reason", json!("")),
    ] {
        let mut bad = good.clone();
        *bad.pointer_mut(path).unwrap() = bad_value;
        assert!(!fb_harness::compare(&bad, &bad).ok(), "{path}");
        assert!(!fb_harness::compare(&good, &bad).ok(), "{path}");
        assert!(!fb_harness::compare(&bad, &good).ok(), "{path}");
    }
    for table in ["transfer", "transfervoid"] {
        let mut bad = good.clone();
        let mut row = bad["checks"][table][0].clone();
        row["id"] = json!("fresh-id");
        bad["checks"][table].as_array_mut().unwrap().push(row);
        assert!(!fb_harness::compare(&bad, &bad).ok());
    }
}
#[test]
fn review_identical_invalid_check_values_must_fail_comparer() {
    let good = dump();
    assert!(fb_harness::compare(&good, &good).ok());
    let mut accepted = vec![];
    for (pointer, bad) in [
        ("/checks/check/0/state", json!("BROKEN")),
        ("/checks/check/0/revision", json!(-1)),
        ("/checks/checkrevision/0/date", json!("2026-99-99")),
        ("/checks/checkrevision/0/print_later", json!(7)),
        ("/checks/checkrevision/0/total", json!(-1)),
        ("/checks/checkline/0/tab", json!("BROKEN")),
        ("/checks/checkline/0/quantity", json!(-1)),
        ("/checks/checkline/0/amount", json!(-1)),
    ] {
        let mut broken = good.clone();
        *broken.pointer_mut(pointer).unwrap() = bad;
        if fb_harness::compare(&broken, &broken).ok() {
            accepted.push(pointer);
        }
    }
    assert!(
        accepted.is_empty(),
        "Identical invalid values accepted: {accepted:?}"
    );
}

#[test]
fn review_identical_incomplete_check_evidence_must_fail_comparer() {
    let good = dump();
    let mut accepted = vec![];
    let mut broken = good.clone();
    broken["checks"]["checkrevision"] = json!([]);
    broken["checks"]["checkline"] = json!([]);
    if fb_harness::compare(&broken, &broken).ok() {
        accepted.push("all revisions/lines omitted despite existing check roots");
    }
    let mut broken = good.clone();
    broken["checks"]["checkline"][0]["account_snapshot"] = json!({});
    if fb_harness::compare(&broken, &broken).ok() {
        accepted.push("linked account snapshot keys omitted");
    }
    let mut broken = good.clone();
    let mut row = broken["checks"]["checkrevision"][0].clone();
    row["id"] = json!("fresh-comparer-id");
    broken["checks"]["checkrevision"]
        .as_array_mut()
        .unwrap()
        .push(row);
    if fb_harness::compare(&broken, &broken).ok() {
        accepted.push("duplicate document/number with fresh id");
    }
    assert!(
        accepted.is_empty(),
        "Identical incomplete/duplicate evidence accepted: {accepted:?}"
    );
}

#[test]
fn strict_values_snapshots_and_coverage_on_each_side() {
    let good = dump();
    assert!(fb_harness::compare(&good, &good).ok());
    let reject = |bad: &Value| {
        assert!(
            !fb_harness::compare(bad, bad).ok(),
            "identical invalid evidence accepted"
        );
        assert!(!fb_harness::compare(bad, &good).ok());
        assert!(!fb_harness::compare(&good, bad).ok());
    };
    for (path, value) in [
        ("/checks/check/0/identity_id", json!("")),
        ("/checks/checkrevision/0/actor_id", json!("")),
        ("/checks/check/0/number_key", json!("a\u{200b}")),
        ("/checks/checkrevision/0/check_number", json!("a\u{1}")),
        ("/checks/checkrevision/0/action", json!("unknown")),
        ("/checks/checkrevision/0/payee_kind", json!("unknown")),
        ("/checks/checkrevision/0/date", json!("2026-02-30")),
        (
            "/checks/checkrevision/0/total",
            json!(10_000_000_000_000_000_i64),
        ),
        ("/checks/checkrevision/0/bank_snapshot/id", json!("wrong")),
        (
            "/checks/checkrevision/0/bank_snapshot/kind",
            json!("unknown"),
        ),
        (
            "/checks/checkrevision/0/company_snapshot/id",
            json!("wrong"),
        ),
        (
            "/checks/checkrevision/0/payee_snapshot/display_name",
            json!(false),
        ),
        (
            "/checks/checkline/0/quantity",
            json!(1_000_000_000_000_000_000_i64),
        ),
        ("/checks/checkline/0/position", json!(0)),
        (
            "/checks/checkline/0/amount",
            json!(10_000_000_000_000_000_i64),
        ),
        ("/checks/checkline/0/rate", json!(0)),
        ("/checks/checkline/0/account_snapshot/id", json!("wrong")),
    ] {
        let mut bad = good.clone();
        *bad.pointer_mut(path).unwrap() = value;
        reject(&bad);
    }
    for table in ["check", "checkrevision", "checkline"] {
        let mut bad = good.clone();
        let mut row = bad["checks"][table][0].clone();
        row["id"] = json!("new-id");
        bad["checks"][table].as_array_mut().unwrap().push(row);
        reject(&bad);
    }
    let mut bad = good.clone();
    bad["checks"]["checkline"].as_array_mut().unwrap().remove(0);
    reject(&bad);
    for path in [
        "/checks/checkrevision/0/bank_snapshot",
        "/checks/checkrevision/0/company_snapshot",
        "/checks/checkrevision/0/payee_snapshot",
        "/checks/checkline/0/account_snapshot",
    ] {
        for field in good.pointer(path).unwrap().as_object().unwrap().keys() {
            let mut bad = good.clone();
            bad.pointer_mut(path)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(field);
            reject(&bad);
        }
    }
}
