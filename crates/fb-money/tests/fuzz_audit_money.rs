//! Fuzz audit 2026-11, area: money text (fb-money).
//!
//! Properties: `parse(format(x)) == x` for every amount in range; half-up rounding exactly as the
//! crate documents it; the refusals the validator promises; and a differential check of
//! `money(Text)`, `parse_python_decimal` and `unicode_decimal` against the real CPython `decimal`
//! module (the frozen reference is CPython 3.13 / Unicode 15.1, which is what `python3` is here).
//! The Python check is skipped, with a message, when `python3` is not on PATH.
use fb_money::{AMOUNT_LIMIT_CENTS, AmountInput, MAX_TOTAL_CENTS, Money, Parsed, money, parse_python_decimal, round_half_up};
use proptest::prelude::*;
use rust_decimal::Decimal;
use std::io::Write;
use std::process::{Command, Stdio};

fn text(s: &str) -> Result<i128, fb_money::MoneyError> {
    money(&AmountInput::Text(s.to_owned())).map(|m| m.cents())
}

/// Text that looks like a number to a human or to CPython's decimal grammar.
fn numberish() -> impl Strategy<Value = String> {
    let atom = prop_oneof![
        8 => "[0-9]{1,20}",
        3 => Just(".".to_string()),
        2 => Just("e".to_string()),
        1 => Just("E".to_string()),
        2 => Just("-".to_string()),
        2 => Just("+".to_string()),
        2 => Just("_".to_string()),
        1 => Just(" ".to_string()),
        1 => Just("\u{3000}".to_string()), // ideographic space (Unicode White_Space)
        1 => Just("\u{1c}".to_string()),   // information separator: Python str.isspace
        1 => Just("\u{a0}".to_string()),
        1 => Just("\u{662}".to_string()),  // Arabic-Indic two
        1 => Just("\u{ff17}".to_string()), // full-width seven
        1 => Just("\u{1d7d8}".to_string()), // mathematical bold zero
        1 => Just("\u{b2}".to_string()),   // superscript two (not a decimal digit)
        1 => Just("\u{10d41}".to_string()), // Garay digit one (Unicode 16; unknown to 15.1)
        1 => Just("inf".to_string()),
        1 => Just("NaN".to_string()),
        1 => Just("sNaN".to_string()),
        1 => Just("Infinity".to_string()),
        1 => Just("\n".to_string()),
        1 => Just(",".to_string()),
        1 => Just("$".to_string()),
    ];
    prop::collection::vec(atom, 0..8).prop_map(|parts| parts.concat())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, .. ProptestConfig::default() })]

    /// parse(format(x)) == x for every storable amount, and the printed form is canonical two-place text.
    #[test]
    fn parse_format_round_trip_over_the_whole_range(cents in 0i128..AMOUNT_LIMIT_CENTS) {
        let m = Money::from_cents(cents);
        let printed = m.to_string();
        prop_assert!(printed.len() >= 4 && printed.as_bytes()[printed.len() - 3] == b'.', "{}", printed);
        prop_assert_eq!(text(&printed), Ok(cents));
        prop_assert_eq!(money(&AmountInput::Decimal(printed.parse::<Decimal>().unwrap())).unwrap(), m);
        prop_assert_eq!(Money::from_decimal_exact(m.to_decimal()), Some(m));
        prop_assert_eq!(m.to_db().map(Money::from_db), Some(m));
        prop_assert_eq!(text(&format!(" {} ", printed)), Ok(cents), "surrounding whitespace is stripped");
        prop_assert_eq!(text(&format!("+{}", printed)), Ok(cents), "a plus sign is accepted");
        prop_assert_eq!(text(&printed.replace('.', "_.")), Ok(cents), "underscores are ignored");
        prop_assert!(text(&format!("-{}", printed)).is_err() || cents == 0, "negative amounts are refused");
    }

    /// Half-up rounding at the third decimal, as `money()` documents (ROUND_HALF_UP, away from zero).
    #[test]
    fn third_decimal_rounds_half_up(cents in 0i128..MAX_TOTAL_CENTS, third in 0u8..10, more in prop::collection::vec(0u8..10, 0..6)) {
        let tail: String = more.iter().map(|d| char::from(b'0' + d)).collect();
        let s = format!("{}{third}{tail}", Money::from_cents(cents));
        let expected = if third >= 5 { cents + 1 } else { cents };
        if expected < AMOUNT_LIMIT_CENTS {
            prop_assert_eq!(text(&s), Ok(expected), "{}", s);
            let d: Decimal = s.parse().unwrap();
            prop_assert_eq!(round_half_up(d, 2), Money::from_cents(expected).to_decimal());
        } else {
            prop_assert!(text(&s).is_err(), "{} must exceed the limit", s);
        }
    }

    /// Int input is cents * 100, with the same limit, and negative ints are refused.
    #[test]
    fn int_input_is_whole_units(n in any::<i128>()) {
        match money(&AmountInput::Int(n)) {
            Ok(m) => { prop_assert!(n >= 0); prop_assert_eq!(m.cents(), n * 100); prop_assert!(m.cents() < AMOUNT_LIMIT_CENTS); }
            Err(_) => prop_assert!(n < 0 || n.checked_mul(100).is_none_or(|c| c >= AMOUNT_LIMIT_CENTS)),
        }
    }

    /// Floats, bools and other JSON types are always refused, whatever their value.
    #[test]
    fn floats_and_bools_are_always_refused(f in any::<f64>(), b in any::<bool>()) {
        prop_assert!(money(&AmountInput::Float(f)).is_err());
        prop_assert!(money(&AmountInput::Bool(b)).is_err());
        prop_assert!(money(&AmountInput::Other).is_err());
    }

    /// Exponent notation: `d e k` equals the shifted decimal, within range.
    #[test]
    fn exponent_notation_matches_shifted_decimal(mantissa in 1u64..1_000_000, exp in -12i32..8) {
        let shifted = Decimal::from(mantissa) * Decimal::from_i128_with_scale(1, 0) / Decimal::from(10u64.pow(exp.unsigned_abs()));
        let value = if exp >= 0 { Decimal::from(mantissa) * Decimal::from(10u64.pow(exp as u32)) } else { shifted };
        let expected = money(&AmountInput::Decimal(value));
        prop_assert_eq!(text(&format!("{mantissa}e{exp}")), expected.clone().map(|m| m.cents()));
        prop_assert_eq!(text(&format!("{mantissa}E{exp:+}")), expected.map(|m| m.cents()));
    }

    /// Anything the parser accepts prints back and re-parses to the same cents (idempotent canonical form).
    #[test]
    fn accepted_text_is_idempotent(s in numberish()) {
        if let Ok(cents) = text(&s) {
            prop_assert!((0..AMOUNT_LIMIT_CENTS).contains(&cents), "{:?} -> {}", s, cents);
            prop_assert_eq!(text(&Money::from_cents(cents).to_string()), Ok(cents));
        }
        if let Some(Parsed::Finite(d)) = parse_python_decimal(&s) {
            prop_assert!(d.digits.bytes().all(|b| b.is_ascii_digit()));
            prop_assert!(!d.digits.is_empty());
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Differential check against CPython's decimal module.

const PY: &str = r#"
import sys, json, unicodedata
from decimal import Decimal, ROUND_HALF_UP
def money(value):
    try:
        raw = Decimal(value)
        if not raw.is_finite() or raw < 0:
            return None
        rounded = raw.quantize(Decimal("0.01"), rounding=ROUND_HALF_UP)
        if rounded >= Decimal("100000000000000"):
            return None
        return str(rounded)
    except Exception:
        return None
def finite(value):
    try:
        d = Decimal(value)
    except Exception:
        return "invalid"
    return "finite" if d.is_finite() else "nonfinite"
def dec(ch):
    try:
        return unicodedata.decimal(ch)
    except Exception:
        return None
inputs = json.load(sys.stdin)
out = {"money": [money(s) for s in inputs["money"]], "finite": [finite(s) for s in inputs["money"]],
       "decimal": [dec(c) for c in inputs["chars"]]}
json.dump(out, sys.stdout)
"#;

fn python(inputs: &serde_json::Value) -> Option<serde_json::Value> {
    let mut child = Command::new("python3").args(["-I", "-c", PY]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().ok()?;
    child.stdin.take().unwrap().write_all(inputs.to_string().as_bytes()).ok()?;
    let out = child.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 12, .. ProptestConfig::default() })]

    /// `money(Text)` and `parse_python_decimal` agree with CPython on 400 random number-like strings per case,
    /// and `unicode_decimal` agrees with `unicodedata.decimal` on random code points.
    #[test]
    fn agrees_with_cpython_decimal(
        strings in prop::collection::vec(numberish(), 400),
        chars in prop::collection::vec(any::<char>(), 300),
    ) {
        let inputs = serde_json::json!({"money": strings, "chars": chars.iter().map(|c| c.to_string()).collect::<Vec<_>>()});
        let Some(out) = python(&inputs) else {
            eprintln!("python3 not available; skipping the CPython differential check");
            return Ok(());
        };
        for (i, s) in strings.iter().enumerate() {
            let expected = out["money"][i].as_str().map(|t| t.replace("-0.00", "0.00"));
            let actual = text(s).ok().map(|c| Money::from_cents(c).to_string());
            prop_assert_eq!(&actual, &expected, "money({:?}): rust {:?} vs python {:?}", s, actual, expected);
            let kind = match parse_python_decimal(s) { None => "invalid", Some(Parsed::NonFinite) => "nonfinite", Some(Parsed::Finite(_)) => "finite" };
            prop_assert_eq!(kind, out["finite"][i].as_str().unwrap(), "parse_python_decimal({:?})", s);
        }
        for (i, c) in chars.iter().enumerate() {
            let expected = out["decimal"][i].as_u64().map(|d| d as u32);
            prop_assert_eq!(fb_money::unicode_decimal(*c), expected, "unicode_decimal({:?})", c);
        }
    }
}

/// F-07: stripping surrounding whitespace must precede removing underscores. Removing
/// underscores must not turn interior whitespace into newly accepted boundary whitespace.
#[test]
fn underscore_next_to_whitespace_is_refused_like_cpython() {
    // All 29 whitespace characters recognized by the frozen Python reference,
    // including its four ASCII information separators and Unicode spaces.
    for whitespace in "\t\n\u{b}\u{c}\r\u{1c}\u{1d}\u{1e}\u{1f} \u{85}\u{a0}\u{1680}\u{2000}\u{2001}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}\u{2008}\u{2009}\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}".chars() {
        for s in [format!("5{whitespace}_"), format!("_{whitespace}5"), format!("{whitespace}_5{whitespace}_{whitespace}")] {
            assert!(parse_python_decimal(&s).is_none(), "{s:?} must be refused (CPython: InvalidOperation)");
            assert!(text(&s).is_err(), "{s:?} must not become a money amount");
        }
        for s in [format!("{whitespace}_5"), format!("5_{whitespace}"), format!("{whitespace}__５__{whitespace}")] {
            assert_eq!(text(&s), Ok(500), "{s:?} remains valid");
        }
        for value in ["NaN", "sNaN42", "Infinity", "-Inf"] {
            assert!(parse_python_decimal(&format!("{value}{whitespace}_")).is_none());
            assert!(parse_python_decimal(&format!("_{whitespace}{value}")).is_none());
            assert!(matches!(parse_python_decimal(&format!("{whitespace}_{value}_{whitespace}")), Some(Parsed::NonFinite)));
        }
    }
}
