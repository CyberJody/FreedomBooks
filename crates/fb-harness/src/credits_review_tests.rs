#[test]
fn review_credit_normalization_does_not_panic_on_storable_snapshot() {
    let mut row = json!({"account_snapshot":{"income_account":false}});
    let ids = audit::Ids::default();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        normalize_row(&mut row, "creditmemoline", &ids);
    }));
    assert!(
        result.is_ok(),
        "credits dump panicked on a shape accepted by SQL and reopen"
    );
}

#[test]
fn review_credit_normalization_keeps_invalid_shapes_visible_without_panicking() {
    let ids = audit::Ids::default();
    for mut row in [
        json!(false),
        json!([]),
        json!({"customer_snapshot":false}),
        json!({"account_snapshot":{"income_account":[]}}),
    ] {
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| normalize_row(
                &mut row,
                "creditmemoline",
                &ids
            )))
            .is_ok()
        );
    }
    let mut event = json!({"target":"creditmemo:untrusted","action":"credit_memo.recorded","detail":{"lines":[false, [], {}]}});
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| normalize_audit(
            &mut event, &ids
        )))
        .is_ok()
    );
}
