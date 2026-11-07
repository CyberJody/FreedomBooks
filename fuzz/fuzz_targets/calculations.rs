//! fb-core document arithmetic on arbitrary JSON: `calculations::{decimal, quantity, unit_rate,
//! line_amount, totals}`, `services::money_json` and `fb_db::sales::calculations` style helpers.
//!
//! The fuzzer input is arbitrary bytes read as JSON; when it is not JSON the bytes are used as a
//! quantity/rate text. Asserts: no panic; accepted values are inside the documented bounds; the
//! totals a document reports add up (subtotal = sum of lines, total = subtotal + tax, the tax
//! snapshot's member shares sum to the tax) exactly as the code documents its rounding.
#![no_main]
use libfuzzer_sys::fuzz_target;
use fb_core::calculations::{decimal, line_amount, quantity, totals, unit_rate};
use fb_money::{MAX_TOTAL_CENTS, Money};
use rust_decimal::Decimal;
use serde_json::{Value, json};

fn cents(s: &str) -> i128 {
    fb_money::money(&fb_money::AmountInput::Text(s.to_owned())).expect("totals output is money text").cents()
}

fuzz_target!(|data: &[u8]| {
    let value: Value = match serde_json::from_slice(data) {
        Ok(v) => v,
        Err(_) => Value::String(String::from_utf8_lossy(data).into_owned()),
    };
    // Scalars.
    if let Ok(q) = quantity(&value) {
        assert!(q > Decimal::ZERO && q.scale() == 4, "quantity {q} from {value}");
    }
    if let Ok(r) = unit_rate(&value) {
        assert!(r >= Decimal::ZERO && r.scale() == 2 && r <= Money::from_cents(MAX_TOTAL_CENTS).to_decimal(), "rate {r} from {value}");
    }
    let _ = decimal(&value, 4, Decimal::ONE_HUNDRED, false);
    let _ = decimal(&value, 0, Decimal::MAX, true);
    let _ = decimal(&value, 28, Decimal::MAX, false);
    let _ = decimal(&value, 29, Decimal::MAX, false);
    let _ = fb_core::services::money_json(&value);
    let _ = fb_core::services::amount_input(&value);
    let _ = fb_db::prepayments::prepayment_cents(&value);
    let _ = fb_db::banking_evidence::money(&value, true);
    let _ = fb_db::banking_evidence::money(&value, false);
    let _ = fb_db::assistant::scaled(&value, 2, 1_000_000_000_000_000, false);
    if let Ok(a) = line_amount(&value, &value) {
        assert!(a.cents() <= MAX_TOTAL_CENTS && a.cents() >= 0);
    }
    // A document: the object (or an array of them) as lines, with the same value as tax rate.
    let lines: Vec<Value> = match &value {
        Value::Array(items) => items.clone(),
        other => vec![other.clone()],
    };
    let lines: Vec<Value> = lines
        .into_iter()
        .take(60)
        .map(|l| if l.is_object() { l } else { json!({"quantity": l, "rate": l, "taxable": true}) })
        .collect();
    let rate = value.get("rate").cloned().unwrap_or_else(|| value.clone());
    let snapshot = value.get("tax_snapshot").cloned().unwrap_or(Value::Null);
    for mode in ["NONE", "MANUAL", "RATE", "GROUP", "other"] {
        let label = value.get("tax_label").cloned().unwrap_or_else(|| json!("Tax"));
        let snap = if snapshot.is_null() {
            json!({"kind": mode, "description": "Snap", "members": [{"rate": rate.clone()}, {"rate": rate.clone()}]})
        } else {
            snapshot.clone()
        };
        if let Ok(out) = totals(&lines, mode, Some(&rate), &label, Some(&snap), true) {
            let printed: Vec<i128> = out["lines"].as_array().unwrap().iter().map(|l| cents(l.as_str().unwrap())).collect();
            let subtotal = cents(out["subtotal"].as_str().unwrap());
            let taxable = cents(out["taxable"].as_str().unwrap());
            let tax = cents(out["tax"].as_str().unwrap());
            let total = cents(out["total"].as_str().unwrap());
            assert_eq!(printed.len(), lines.len());
            assert_eq!(printed.iter().sum::<i128>(), subtotal, "subtotal must equal the line sum: {out}");
            assert_eq!(subtotal + tax, total, "total must equal subtotal + tax: {out}");
            assert!(taxable <= subtotal, "taxable above subtotal: {out}");
            assert!(total <= MAX_TOTAL_CENTS, "total above the limit: {out}");
            assert!(total > 0, "a posted document must not total zero: {out}");
            let pct: Decimal = out["tax_rate"].as_str().unwrap().parse().unwrap();
            if let Some(members) = out["tax_snapshot"]["members"].as_array() {
                let shares: i128 = members.iter().map(|m| cents(m["tax"].as_str().unwrap())).sum();
                assert_eq!(shares, tax, "member shares must sum to the tax: {out}");
            } else {
                // rate normalized to four places: cents * percent_mantissa / 1_000_000, half up.
                let mantissa = (pct * Decimal::from(10_000)).mantissa() / 10i128.pow((pct * Decimal::from(10_000)).scale());
                assert_eq!((taxable * mantissa + 500_000) / 1_000_000, tax, "single-rate tax rounding: {out}");
            }
        }
    }
});
