//! Fuzz audit 2026-11, area: document arithmetic (fb-core `calculations`).
//!
//! Properties: quantity/rate parsing bounds and refusals; `line_amount` rounds half-up at the fourth
//! place of qty*rate exactly as the code says (`(q*r + 5000) / 10000` on scaled mantissas); and the
//! `totals` of a random document add up: subtotal is the sum of the printed line amounts, total is
//! subtotal + tax, a single rate's tax is `cents * rate_mantissa / 1_000_000` rounded half up, a
//! group's tax is the sum of the per-member rounded shares (which may differ from rounding the
//! summed rate; that difference is the documented behaviour, checked here as bounded).
use fb_core::calculations::{decimal, line_amount, quantity, totals, unit_rate};
use fb_money::{AMOUNT_LIMIT_CENTS, MAX_TOTAL_CENTS, Money};
use proptest::prelude::*;
use rust_decimal::Decimal;
use serde_json::{Value, json};

fn cents(v: &Value) -> i128 {
    fb_money::money(&fb_money::AmountInput::Text(v.as_str().expect("money text").to_owned())).expect("totals output is money").cents()
}

fn qty_text() -> impl Strategy<Value = String> {
    (1u64..100_000_000, 0u32..10_000).prop_map(|(whole, frac)| format!("{whole}.{frac:04}"))
}
fn rate_text() -> impl Strategy<Value = String> {
    (0u64..100_000_000, 0u32..100).prop_map(|(whole, frac)| format!("{whole}.{frac:02}"))
}
fn pct_text() -> impl Strategy<Value = String> {
    (0u32..100, 0u32..10_000).prop_map(|(whole, frac)| format!("{whole}.{frac:04}"))
}
fn line() -> impl Strategy<Value = Value> {
    (qty_text(), rate_text(), any::<bool>()).prop_map(|(q, r, t)| json!({"quantity": q, "rate": r, "taxable": t}))
}

fn mantissa_4(pct: &str) -> i128 {
    let d: Decimal = pct.parse().unwrap();
    (d * Decimal::from(10_000)).normalize().mantissa()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1500, .. ProptestConfig::default() })]

    #[test]
    fn quantity_and_rate_keep_their_scale_and_bounds(q in qty_text(), r in rate_text()) {
        let qd = quantity(&json!(q)).unwrap();
        prop_assert_eq!(qd.scale(), 4);
        prop_assert_eq!(qd.to_string(), q.parse::<Decimal>().unwrap().to_string());
        let rd = unit_rate(&json!(r)).unwrap();
        prop_assert_eq!(rd.scale(), 2);
        prop_assert_eq!(rd.to_string(), r.parse::<Decimal>().unwrap().to_string());
        // Non-string JSON is refused except plain integers.
        prop_assert!(quantity(&json!(1.5)).is_err());
        prop_assert!(quantity(&json!(true)).is_err());
        prop_assert!(quantity(&json!(null)).is_err());
        prop_assert!(quantity(&json!(0)).is_err(), "quantity must be positive");
        prop_assert!(unit_rate(&json!(0)).is_ok(), "a zero rate is allowed");
        prop_assert!(quantity(&json!(format!("-{q}"))).is_err(), "assertion failed");
        prop_assert!(unit_rate(&json!(format!("{r}1"))).is_err(), "three decimals are refused");
        prop_assert!(quantity(&json!(format!("{q}1"))).is_err(), "five decimals are refused");
    }

    #[test]
    fn line_amount_rounds_half_up_at_the_fourth_place(q in qty_text(), r in rate_text()) {
        let a = line_amount(&json!(q), &json!(r));
        let exact = q.parse::<Decimal>().unwrap() * r.parse::<Decimal>().unwrap();
        let expected = fb_money::round_half_up(exact, 2);
        match a {
            Ok(m) => {
                prop_assert_eq!(m.to_decimal(), expected, "{} * {}", q, r);
                prop_assert!(m.cents() <= MAX_TOTAL_CENTS);
            }
            Err(_) => prop_assert!(expected > Money::from_cents(MAX_TOTAL_CENTS).to_decimal(), "{q} * {r} = {expected} was refused"),
        }
    }

    #[test]
    fn totals_add_up_for_single_rates(lines in prop::collection::vec(line(), 1..12), pct in pct_text(), mode in prop_oneof![Just("MANUAL"), Just("RATE"), Just("NONE")]) {
        let snapshot = json!({"kind": "RATE", "description": "State tax", "members": [{"rate": pct}]});
        let rate = if mode == "NONE" { json!("0") } else { json!(pct) };
        let out = match totals(&lines, mode, Some(&rate), &json!("Combined"), Some(&snapshot), false) {
            Ok(out) => out,
            Err(e) => {
                // Only the overflow refusal is legitimate here: every input is individually valid, so the
                // subtotal, the taxable amount or the total with tax must exceed the document limit.
                let amounts: Vec<i128> = lines.iter().map(|l| line_amount(&l["quantity"], &l["rate"]).map(|m| m.cents()).unwrap_or(MAX_TOTAL_CENTS + 1)).collect();
                let sum: i128 = amounts.iter().sum();
                let taxable: i128 = lines.iter().zip(&amounts).filter(|(l, _)| l["taxable"] == true).map(|(_, c)| c).sum();
                let m = if mode == "NONE" { 0 } else { mantissa_4(&pct) };
                let tax = (taxable * m + 500_000) / 1_000_000;
                prop_assert!(sum + tax > MAX_TOTAL_CENTS, "unexpected refusal {:?} for {:?}", e, lines);
                return Ok(());
            }
        };
        let printed: Vec<i128> = out["lines"].as_array().unwrap().iter().map(cents).collect();
        let (subtotal, taxable, tax, total) = (cents(&out["subtotal"]), cents(&out["taxable"]), cents(&out["tax"]), cents(&out["total"]));
        prop_assert_eq!(printed.iter().sum::<i128>(), subtotal);
        prop_assert_eq!(subtotal + tax, total);
        let expected_taxable: i128 = lines.iter().zip(&printed).filter(|(l, _)| l["taxable"] == true).map(|(_, c)| c).sum();
        prop_assert_eq!(taxable, expected_taxable);
        let m = if mode == "NONE" { 0 } else { mantissa_4(&pct) };
        prop_assert_eq!(tax, (taxable * m + 500_000) / 1_000_000, "single-rate tax rounding");
        prop_assert_eq!(out["tax_rate"].as_str().unwrap().parse::<Decimal>().unwrap(), if mode == "NONE" { Decimal::ZERO } else { pct.parse().unwrap() });
    }

    #[test]
    fn group_tax_is_the_sum_of_member_shares(lines in prop::collection::vec(line(), 1..8), members in prop::collection::vec(pct_text(), 2..6)) {
        let sum: Decimal = members.iter().map(|m| m.parse::<Decimal>().unwrap()).sum();
        let snapshot = json!({"kind": "GROUP", "description": "Combined", "members": members.iter().map(|m| json!({"rate": m, "name": "part"})).collect::<Vec<_>>()});
        let result = totals(&lines, "GROUP", None, &json!("x"), Some(&snapshot), false);
        if sum > Decimal::ONE_HUNDRED {
            prop_assert!(result.is_err(), "a group above 100% must be refused");
            return Ok(());
        }
        let Ok(out) = result else { return Ok(()) };
        let (taxable, tax, total, subtotal) = (cents(&out["taxable"]), cents(&out["tax"]), cents(&out["total"]), cents(&out["subtotal"]));
        prop_assert_eq!(subtotal + tax, total);
        let shares: Vec<i128> = out["tax_snapshot"]["members"].as_array().unwrap().iter().map(|m| cents(&m["tax"])).collect();
        prop_assert_eq!(shares.iter().sum::<i128>(), tax, "group tax is the sum of rounded member shares");
        for (share, pct) in shares.iter().zip(&members) {
            prop_assert_eq!(*share, (taxable * mantissa_4(pct) + 500_000) / 1_000_000);
        }
        // Per-member rounding can differ from whole-rate rounding by at most half a cent per member.
        let whole = (taxable * mantissa_4(&sum.to_string()) + 500_000) / 1_000_000;
        prop_assert!((tax - whole).abs() <= members.len() as i128, "group tax {} vs whole-rate tax {}", tax, whole);
    }

    #[test]
    fn totals_refuse_what_the_validator_promises(lines in prop::collection::vec(line(), 1..4), pct in pct_text()) {
        let snap = json!({"kind": "RATE", "description": "T", "members": [{"rate": pct}]});
        prop_assert!(totals(&lines, "NONE", Some(&json!("0.5")), &json!("x"), None, false).is_err(), "NONE with a rate");
        prop_assert!(totals(&lines, "MANUAL", Some(&json!(pct)), &json!("   "), None, false).is_err(), "MANUAL without a label");
        prop_assert!(totals(&lines, "MANUAL", Some(&json!(pct)), &json!("x".repeat(121)), None, false).is_err(), "label over 120 chars");
        prop_assert!(totals(&lines, "RATE", None, &json!("x"), None, false).is_err(), "RATE without a snapshot");
        prop_assert!(totals(&lines, "GROUP", None, &json!("x"), Some(&snap), false).is_err(), "GROUP with one member");
        prop_assert!(totals(&lines, "WEIRD", None, &json!("x"), None, false).is_err(), "unknown mode");
        prop_assert!(totals(&lines, "MANUAL", Some(&json!("100.0001")), &json!("x"), None, false).is_err(), "rate above 100%");
        prop_assert!(totals(&lines, "MANUAL", Some(&json!("-1")), &json!("x"), None, false).is_err(), "negative rate");
        let mut untagged = lines.clone();
        untagged[0]["taxable"] = json!("yes");
        prop_assert!(totals(&untagged, "NONE", None, &json!("x"), None, false).is_err(), "taxable must be a bool");
        let zero = vec![json!({"quantity": "1", "rate": "0", "taxable": false})];
        prop_assert!(totals(&zero, "NONE", None, &json!("x"), None, true).is_err(), "a zero total cannot post");
        prop_assert!(totals(&zero, "NONE", None, &json!("x"), None, false).is_ok(), "a zero total can be saved");
    }

    /// `decimal()` with every place count the code allows, and the one above the limit, never panics and
    /// keeps the requested scale.
    #[test]
    fn decimal_keeps_requested_scale(places in 0u32..30, text in "[0-9]{0,30}(\\.[0-9]{0,30})?(e-?[0-9]{0,3})?") {
        if let Ok(d) = decimal(&json!(text), places, Decimal::MAX, false) {
            prop_assert_eq!(d.scale(), places);
            prop_assert!(d >= Decimal::ZERO);
        }
        let _ = decimal(&json!(text), places, Decimal::from(5), true);
    }

    #[test]
    fn money_json_matches_fb_money(cents in 0i128..AMOUNT_LIMIT_CENTS) {
        let m = Money::from_cents(cents);
        prop_assert_eq!(fb_core::services::money_json(&json!(m.to_string())).unwrap(), m);
        prop_assert!(fb_core::services::money_json(&json!(cents as f64 / 100.0)).is_err(), "JSON floats are refused");
        prop_assert!(fb_core::services::money_json(&json!(true)).is_err());
    }
}
