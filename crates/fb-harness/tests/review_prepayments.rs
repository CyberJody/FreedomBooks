use serde_json::{Value, json};
fn good() -> Value {
    let scenario: Value = serde_json::from_str(include_str!(
        "../scenarios/prepayments-lifecycle-retries.json"
    ))
    .unwrap();
    let path = std::env::temp_dir().join(format!(
        "review-prepayments-comparer-{}.fbbook",
        fb_ids::new_v7()
    ));
    let mut runner = fb_harness::Runner::new(&scenario, &path);
    runner.run();
    let dump = runner.dump();
    assert!(fb_harness::compare(&dump, &dump).ok());
    dump
}
#[test]
fn review_comparer_rejects_identical_invalid_value_syntax() {
    let good = good();
    let mut accepted = vec![];
    for (path, value) in [
        ("/prepayments/applications/0/amount", json!("not money")),
        ("/prepayments/applications/0/date", json!("2026-99-99")),
        ("/prepayments/refunds/0/method", json!("BITCOIN")),
        (
            "/prepayments/details/0/deposit_state",
            json!("UNRECOGNIZED"),
        ),
        (
            "/prepayments/refunds/0/customer_snapshot/id",
            json!("no such customer"),
        ),
    ] {
        let mut bad = good.clone();
        *bad.pointer_mut(path).expect(path) = value;
        if fb_harness::compare(&bad, &bad).ok() {
            accepted.push(path);
        }
    }
    assert!(
        accepted.is_empty(),
        "accepted identical malformed syntax at {accepted:?}"
    );
}
#[test]
fn review_comparer_rejects_identical_missing_prepayment_details() {
    let mut bad = good();
    bad["prepayments"]["details"] = json!([]);
    assert!(
        !fb_harness::compare(&bad, &bad).ok(),
        "accepted CDP receipts without matching prepayment details"
    );
}

#[test]
fn fix1_comparer_rejects_identical_corruption_matrix() {
    let good = good();
    for collection in ["details", "applications", "refunds"] {
        let mut bad = good.clone();
        bad["prepayments"][collection] = json!([]);
        assert!(
            !fb_harness::compare(&bad, &bad).ok(),
            "missing {collection}"
        );
    }
    for (path, values) in [
        (
            "/prepayments/details/0/applied",
            vec![
                json!("-0.01"),
                json!("100000000000000.00"),
                json!("1e2"),
                json!("1.000"),
                json!("0.01"),
            ],
        ),
        (
            "/prepayments/details/0/refunded",
            vec![json!("NaN"), json!("0.01")],
        ),
        (
            "/prepayments/details/0/remaining",
            vec![json!("inf"), json!("0.01")],
        ),
        (
            "/prepayments/applications/0/amount",
            vec![json!("0.00"), json!("-1.00"), json!("100000000000000.00")],
        ),
        (
            "/prepayments/applications/0/date",
            vec![json!("2026-+1-01"), json!("2026-02-30"), json!("2026W01")],
        ),
        (
            "/prepayments/applications/0/invoice_revision",
            vec![json!(0), json!(99)],
        ),
        (
            "/prepayments/applications/0/deposit_id",
            vec![json!("missing")],
        ),
        (
            "/prepayments/applications/0/invoice_id",
            vec![json!("missing")],
        ),
        (
            "/prepayments/applications/0/liability_id",
            vec![json!("bank")],
        ),
        (
            "/prepayments/applications/0/reversal/date",
            vec![json!("2026-13-01"), json!("2026-01-01")],
        ),
        (
            "/prepayments/applications/0/reversal/journal_id",
            vec![json!("missing")],
        ),
        (
            "/prepayments/refunds/0/amount",
            vec![json!("0.00"), json!("-1.00"), json!("100000000000000.00")],
        ),
        ("/prepayments/refunds/0/date", vec![json!("2026-02-30")]),
        (
            "/prepayments/refunds/0/company_snapshot/id",
            vec![json!("missing")],
        ),
        (
            "/prepayments/refunds/0/check_number",
            vec![json!("bad\u{200b}"), json!(" 4001"), json!("x".repeat(21))],
        ),
        (
            "/prepayments/refunds/0/reversal/date",
            vec![json!("2026-13-01")],
        ),
    ] {
        for value in values {
            let mut bad = good.clone();
            *bad.pointer_mut(path).expect(path) = value;
            assert!(
                !fb_harness::compare(&bad, &bad).ok(),
                "{path}: {}",
                bad.pointer(path).unwrap()
            );
        }
    }
}
