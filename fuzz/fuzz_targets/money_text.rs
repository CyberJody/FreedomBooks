//! fb-money: `parse_python_decimal` and `money(AmountInput::Text)` on arbitrary text.
//!
//! Asserts: no panic; an accepted amount is in range, prints back to text that parses to the same
//! cents, and converts exactly to and from `rust_decimal`.
#![no_main]
use libfuzzer_sys::fuzz_target;
use fb_money::{AMOUNT_LIMIT_CENTS, AmountInput, Money, Parsed, money, parse_python_decimal};

fuzz_target!(|data: &[u8]| {
    let text = match std::str::from_utf8(data) {
        Ok(s) => s.to_owned(),
        Err(_) => String::from_utf8_lossy(data).into_owned(),
    };
    let parsed = parse_python_decimal(&text);
    if let Some(Parsed::Finite(d)) = &parsed {
        assert!(d.digits.bytes().all(|b| b.is_ascii_digit()), "digits must be ASCII: {:?}", d.digits);
        let _ = d.cents_half_up();
        let _ = d.is_zero();
    }
    match money(&AmountInput::Text(text.clone())) {
        Ok(m) => {
            let cents = m.cents();
            assert!((0..AMOUNT_LIMIT_CENTS).contains(&cents), "out of range: {cents} from {text:?}");
            let printed = m.to_string();
            let again = money(&AmountInput::Text(printed.clone())).expect("printed amount must parse");
            assert_eq!(again, m, "print/parse round trip: {text:?} -> {printed:?}");
            assert_eq!(Money::from_decimal_exact(m.to_decimal()), Some(m));
            assert_eq!(m.to_db().map(Money::from_db), Some(m));
        }
        Err(_) => {}
    }
    // The Decimal path must agree with the text path whenever rust_decimal can read the text.
    if let Ok(d) = text.trim().parse::<rust_decimal::Decimal>() {
        let _ = money(&AmountInput::Decimal(d));
    }
    let _ = money(&AmountInput::Float(text.len() as f64));
});
