use serde_json::{Value, json};

fn dump() -> Value {
    let scenario: Value =
        serde_json::from_str(include_str!("../scenarios/sales-invoice-payments.json")).unwrap();
    let path = std::env::temp_dir().join(format!(
        "fb-sales-independent-comparer-{}.fbbook",
        fb_ids::new_v7()
    ));
    let mut runner = fb_harness::Runner::new(&scenario, &path);
    runner.run();
    let good = runner.dump();
    assert!(fb_harness::compare(&good, &good).ok());
    good
}

#[test]
fn missing_sales_balance_report_is_not_identical_parity() {
    let mut bad = dump();
    assert!(!bad["sales"]["invoices"].as_array().unwrap().is_empty());
    bad["sales"]["balances"] = json!([]);
    assert!(
        !fb_harness::compare(&bad, &bad).ok(),
        "MATCH accepted with every invoice balance report missing"
    );
}

#[test]
fn malformed_sales_money_is_not_identical_parity() {
    let mut bad = dump();
    bad["sales"]["invoices"][0]["history"][0]["total"] = json!("not money");
    assert!(
        !fb_harness::compare(&bad, &bad).ok(),
        "MATCH accepted non-money invoice total"
    );
}

#[test]
fn every_sales_money_path_rejects_identical_syntax_and_bound_corruption() {
    fn paths(v: &Value, path: String, out: &mut Vec<String>) {
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
            Value::String(s) if s.split_once('.').is_some_and(|(_, f)| f.len() == 2) => {
                out.push(path)
            }
            _ => {}
        }
    }
    let mut checked = 0;
    for name in [
        "sales-invoice-payments",
        "sales-cash-sales",
        "sales-tax-group-retry",
    ] {
        let scenario: Value = serde_json::from_str(
            &std::fs::read_to_string(format!(
                "{}/scenarios/{name}.json",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap(),
        )
        .unwrap();
        let file = std::env::temp_dir().join(format!("fb-sales-money-{}.fbbook", fb_ids::new_v7()));
        let mut runner = fb_harness::Runner::new(&scenario, &file);
        runner.run();
        let good = runner.dump();
        assert!(fb_harness::compare(&good, &good).ok());
        let mut all = Vec::new();
        paths(&good["sales"], "/sales".into(), &mut all);
        // Text fields may coincidentally contain money, so identify actual
        // amount fields rather than treating every two-place string as money.
        all.retain(|p| {
            [
                "/total",
                "/subtotal",
                "/amount",
                "/allocated",
                "/credited",
                "/deposited",
                "/settled",
                "/outstanding",
                "/tax",
                "/taxable",
                "/sale_rate",
            ]
            .iter()
            .any(|k| p.ends_with(k))
                || (p.contains("/lines/") && p.ends_with("/rate"))
        });
        for path in all {
            for bad in [
                "not money",
                "NaN",
                "Infinity",
                "1e2",
                "1",
                "1.001",
                "-0.01",
                "+1.00",
                "01.00",
                " 1.00",
                "100000000000000.00",
                "99999999999999999999999999999999.00",
                "",
                "١.٠٠",
            ] {
                let mut corrupt = good.clone();
                *corrupt.pointer_mut(&path).unwrap() = json!(bad);
                let report = std::panic::catch_unwind(|| fb_harness::compare(&corrupt, &corrupt));
                assert!(report.is_ok(), "panicked at {path}={bad}");
                assert!(!report.unwrap().ok(), "accepted {path}={bad}");
            }
            checked += 1;
        }
    }
    assert!(checked > 30, "checked {checked} amount paths");
}

#[test]
fn extra_unknown_sales_balance_is_not_identical_parity() {
    let mut bad = dump();
    bad["sales"]["balances"][0]["invoice_id"] = json!("unknown-invoice");
    assert!(!fb_harness::compare(&bad, &bad).ok());
}

#[test]
fn sales_money_syntax_accepts_zero_cent_and_inclusive_maximum() {
    let good = dump();
    for amount in ["0.00", "0.01", "99999999999999.99"] {
        let mut changed = good.clone();
        changed["sales"]["invoices"][0]["history"][0]["total"] = json!(amount);
        assert!(
            fb_harness::compare(&changed, &changed).ok(),
            "refused bounded {amount}"
        );
    }
}
