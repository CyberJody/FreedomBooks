//! fb-money: two-place money as integer cents, with the Python app's exact parsing and half-up rule.
//!
//! Port of `ledger.services.money` (frozen Python app, tag python-reference-20261030):
//!
//! ```python
//! def money(value):
//!     if isinstance(value, (float, bool)) or not isinstance(value, (str, int, Decimal)): raise
//!     raw = Decimal(value)
//!     if not raw.is_finite() or raw < 0: raise
//!     rounded = raw.quantize(Decimal("0.01"), rounding=ROUND_HALF_UP)
//!     if rounded >= Decimal("100000000000000"): raise
//!     return rounded
//! ```
//!
//! Text parsing follows CPython's C `decimal` constructor, quirks included (see `parse_python_decimal`):
//! surrounding Unicode whitespace is stripped, every `_` is ignored, and non-ASCII decimal digits
//! (Arabic-Indic, full-width, ...) count as digits. Money math never happens in SQL: the database
//! stores INTEGER cents and every sum, compare and rounding happens here.

use rust_decimal::{Decimal, RoundingStrategy};
use std::fmt;
use std::iter::Sum;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};

/// The user-facing message of every refused amount (Python: `ValidationError("Enter a valid monetary amount.")`).
pub const INVALID_AMOUNT: &str = "Enter a valid monetary amount.";
/// Exclusive upper bound of one stored amount, in cents (Python: `rounded >= Decimal("100000000000000")`).
pub const AMOUNT_LIMIT_CENTS: i128 = 10_000_000_000_000_000;
/// `MAX_TOTAL = Decimal("99999999999999.99")` in ledger.services, in cents.
pub const MAX_TOTAL_CENTS: i128 = 9_999_999_999_999_999;

/// An amount of money in whole cents. Stored amounts are 0..=MAX_TOTAL_CENTS; sums and balances
/// (which Python keeps as unbounded `Decimal`) may be negative or larger, so the inner type is i128.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Money(i128);

impl Money {
    pub const ZERO: Money = Money(0);

    pub const fn from_cents(cents: i128) -> Money {
        Money(cents)
    }

    pub const fn cents(self) -> i128 {
        self.0
    }

    /// Cents as i64 for an INTEGER column; None if out of range (never for a bounded amount).
    pub fn to_db(self) -> Option<i64> {
        i64::try_from(self.0).ok()
    }

    pub fn from_db(cents: i64) -> Money {
        Money(cents as i128)
    }

    pub fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// The amount as a `rust_decimal::Decimal` with scale 2 (exact).
    pub fn to_decimal(self) -> Decimal {
        Decimal::from_i128_with_scale(self.0, 2)
    }

    /// Exact conversion from a Decimal already on a cent boundary; None if it has a fraction of a cent.
    pub fn from_decimal_exact(value: Decimal) -> Option<Money> {
        let scaled = value.checked_mul(Decimal::ONE_HUNDRED)?.normalize();
        if scaled.scale() != 0 {
            return None;
        }
        Some(Money(scaled.mantissa()))
    }

    pub fn max(self, other: Money) -> Money {
        if self >= other { self } else { other }
    }
}

/// Python's `str(Decimal)` for a 2-place value: `-5.00`, `0.00`, `125.01`.
impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let abs = self.0.unsigned_abs();
        write!(f, "{}{}.{:02}", sign, abs / 100, abs % 100)
    }
}

impl Add for Money {
    type Output = Money;
    fn add(self, rhs: Money) -> Money {
        Money(self.0 + rhs.0)
    }
}
impl Sub for Money {
    type Output = Money;
    fn sub(self, rhs: Money) -> Money {
        Money(self.0 - rhs.0)
    }
}
impl Neg for Money {
    type Output = Money;
    fn neg(self) -> Money {
        Money(-self.0)
    }
}
impl AddAssign for Money {
    fn add_assign(&mut self, rhs: Money) {
        self.0 += rhs.0;
    }
}
impl SubAssign for Money {
    fn sub_assign(&mut self, rhs: Money) {
        self.0 -= rhs.0;
    }
}
impl Sum for Money {
    fn sum<I: Iterator<Item = Money>>(iter: I) -> Money {
        iter.fold(Money::ZERO, |a, b| a + b)
    }
}
impl<'a> Sum<&'a Money> for Money {
    fn sum<I: Iterator<Item = &'a Money>>(iter: I) -> Money {
        iter.fold(Money::ZERO, |a, b| a + *b)
    }
}

/// What a caller may hand to `money()`. Python accepts `str`, `int` and `Decimal` and refuses
/// `float`, `bool` and everything else; `Float`, `Bool` and `Other` exist so a JSON boundary can
/// pass those through and get the same refusal.
#[derive(Clone, Debug, PartialEq)]
pub enum AmountInput {
    Text(String),
    Int(i128),
    Decimal(Decimal),
    Float(f64),
    Bool(bool),
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoneyError;

impl fmt::Display for MoneyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(INVALID_AMOUNT)
    }
}
impl std::error::Error for MoneyError {}

/// `ledger.services.money`: a non-negative amount rounded half-up to cents, below 10^14.
pub fn money(value: &AmountInput) -> Result<Money, MoneyError> {
    let cents = match value {
        AmountInput::Text(text) => {
            let parsed = parse_python_decimal(text).ok_or(MoneyError)?;
            match parsed {
                Parsed::Finite(number) => {
                    if number.negative && !number.is_zero() {
                        return Err(MoneyError);
                    }
                    number.cents_half_up().ok_or(MoneyError)?
                }
                Parsed::NonFinite => return Err(MoneyError),
            }
        }
        AmountInput::Int(n) => {
            if *n < 0 {
                return Err(MoneyError);
            }
            n.checked_mul(100).ok_or(MoneyError)?
        }
        AmountInput::Decimal(d) => {
            if d.is_sign_negative() && !d.is_zero() {
                return Err(MoneyError);
            }
            let rounded = round_half_up(*d, 2);
            Money::from_decimal_exact(rounded).ok_or(MoneyError)?.0
        }
        AmountInput::Float(_) | AmountInput::Bool(_) | AmountInput::Other => return Err(MoneyError),
    };
    if cents >= AMOUNT_LIMIT_CENTS {
        return Err(MoneyError);
    }
    Ok(Money(cents))
}

/// Python `Decimal.quantize(Decimal(10) ** -dp, rounding=ROUND_HALF_UP)`: halves go away from zero.
pub fn round_half_up(value: Decimal, dp: u32) -> Decimal {
    value.round_dp_with_strategy(dp, RoundingStrategy::MidpointAwayFromZero)
}

/// A finite decimal parsed from text: sign, significant digits (ASCII), and a power-of-ten exponent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecimalText {
    pub negative: bool,
    pub digits: String,
    pub exponent: i128,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parsed {
    Finite(DecimalText),
    NonFinite,
}

impl DecimalText {
    pub fn is_zero(&self) -> bool {
        self.digits.bytes().all(|b| b == b'0')
    }

    /// The value in cents, rounded half-up (magnitude only; the sign is checked by the caller).
    /// None when it cannot fit (far above any bound).
    pub fn cents_half_up(&self) -> Option<i128> {
        let digits = self.digits.trim_start_matches('0');
        if digits.is_empty() {
            return Some(0);
        }
        let shift = self.exponent.checked_add(2)?; // value * 100 = digits * 10^shift
        if shift >= 0 {
            if i128::try_from(digits.len()).ok()?.checked_add(shift)? > 30 {
                return None;
            }
            let base: i128 = digits.parse().ok()?;
            return base.checked_mul(10i128.checked_pow(u32::try_from(shift).ok()?)?);
        }
        let drop = shift.checked_neg()?;
        if drop > i128::try_from(digits.len()).ok()? {
            return Some(0);
        }
        let drop = usize::try_from(drop).ok()?;
        let at = digits.len().checked_sub(drop)?;
        let (keep, rest) = (digits.get(..at)?, digits.get(at..)?);
        if keep.len() > 30 {
            return None;
        }
        let mut kept: i128 = if keep.is_empty() { 0 } else { keep.parse().ok()? };
        if rest.as_bytes().first().copied()? >= b'5' {
            kept = kept.checked_add(1)?;
        }
        Some(kept)
    }
}

/// Python's whitespace (`str.isspace`): Unicode White_Space plus the four information separators.
pub fn py_isspace(ch: char) -> bool {
    ch.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&ch)
}

include!("python_decimal_digits.rs");

/// The decimal value of a character, exactly as the frozen reference Python sees it (`unicodedata.decimal`,
/// Unicode 15.1). The repertoire is pinned to that Python rather than to this crate's Unicode tables, so
/// digits assigned later (e.g. U+10D40 Garay) stay rejected like the answer key rejects them.
pub fn unicode_decimal(ch: char) -> Option<u32> {
    let cp = ch as u32;
    let index = PYTHON_DECIMAL_DIGITS.partition_point(|&(_, last, _)| last < cp);
    let &(first, last, value) = PYTHON_DECIMAL_DIGITS.get(index)?;
    (first..=last).contains(&cp).then(|| value + (cp - first))
}

/// CPython `decimal.Decimal(str)`: strip surrounding whitespace first, then map Unicode digits to
/// ASCII and whitespace to spaces, drop underscores, and apply the decimal-string grammar (sign,
/// digits with one optional point, optional exponent; `Inf`/`Infinity`/`NaN`/`sNaN` in any case).
/// None = not a number.
pub fn parse_python_decimal(text: &str) -> Option<Parsed> {
    let mut ascii = String::with_capacity(text.len());
    for ch in text.trim_matches(py_isspace).chars() {
        if ch == '_' {
            continue;
        }
        if py_isspace(ch) {
            ascii.push(' ');
        } else if ch.is_ascii() {
            ascii.push(ch);
        } else {
            let d = unicode_decimal(ch)?;
            ascii.push(char::from_digit(d, 10)?);
        }
    }
    // Underscores can surround otherwise interior whitespace ("5 _", "_ 5").
    // Trimming again here would accept those invalid inputs after removing the underscores.
    let s = ascii.as_str();
    let (negative, body) = match s.as_bytes().first() {
        Some(b'+') => (false, &s[1..]),
        Some(b'-') => (true, &s[1..]),
        _ => (false, s),
    };
    let lower = body.to_ascii_lowercase();
    if lower == "inf" || lower == "infinity" {
        return Some(Parsed::NonFinite);
    }
    for prefix in ["nan", "snan"] {
        if let Some(payload) = lower.strip_prefix(prefix) {
            if payload.bytes().all(|b| b.is_ascii_digit()) {
                return Some(Parsed::NonFinite);
            }
            return None;
        }
    }
    let (mantissa, exponent) = match body.find(['e', 'E']) {
        Some(at) => (&body[..at], Some(&body[at + 1..])),
        None => (body, None),
    };
    let (int_part, frac_part) = match mantissa.find('.') {
        Some(at) => (&mantissa[..at], &mantissa[at + 1..]),
        None => (mantissa, ""),
    };
    if int_part.is_empty() && frac_part.is_empty() {
        return None;
    }
    if !int_part.bytes().all(|b| b.is_ascii_digit()) || !frac_part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut exp: i128 = 0;
    if let Some(e) = exponent {
        let (eneg, edigits) = match e.as_bytes().first() {
            Some(b'+') => (false, &e[1..]),
            Some(b'-') => (true, &e[1..]),
            _ => (false, e),
        };
        if edigits.is_empty() || !edigits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        // CPython libmpdec constructor limits (64-bit build), including zero coefficients.
        let trimmed = edigits.trim_start_matches('0');
        let magnitude: i128 = if trimmed.is_empty() { 0 } else { trimmed.parse().ok()? };
        exp = if eneg { -magnitude } else { magnitude };
    }
    let mut digits = String::with_capacity(int_part.len() + frac_part.len());
    digits.push_str(int_part);
    digits.push_str(frac_part);
    let exponent = exp.checked_sub(i128::try_from(frac_part.len()).ok()?)?;
    const MAX_EMAX: i128 = 999_999_999_999_999_999;
    const MIN_ETINY: i128 = -1_999_999_999_999_999_997;
    if !(MIN_ETINY..=MAX_EMAX).contains(&exponent) { return None; }
    let significant = digits.trim_start_matches('0');
    if !significant.is_empty() && exponent.checked_add(i128::try_from(significant.len()).ok()?)?.checked_sub(1)? > MAX_EMAX { return None; }
    Some(Parsed::Finite(DecimalText { negative, digits, exponent }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Result<String, MoneyError> {
        money(&AmountInput::Text(s.to_string())).map(|m| m.to_string())
    }

    #[test]
    fn decimal_digits_are_frozen_python_unicode_15_1() {
        assert_eq!(unicode_decimal('7'), Some(7));
        assert_eq!(unicode_decimal('\u{662}'), Some(2)); // Arabic-Indic two
        assert_eq!(unicode_decimal('\u{1fbf9}'), Some(9)); // segmented nine (Unicode 13)
        assert_eq!(unicode_decimal('\u{b2}'), None); // superscript two is not decimal
        // Unicode 16 digits (Garay, Gurung Khema, Kirat Rai, outlined) are unknown to the answer key.
        for ch in ['\u{10d40}', '\u{10d42}', '\u{16130}', '\u{16d70}', '\u{1ccf0}'] {
            assert_eq!(unicode_decimal(ch), None, "{ch:?}");
        }
        assert!(text("\u{10d41}.00").is_err());
        assert_eq!(text("\u{661}.00").unwrap(), "1.00");
        assert!(PYTHON_DECIMAL_DIGITS.windows(2).all(|w| w[0].1 < w[1].0));
        assert!(PYTHON_DECIMAL_DIGITS.iter().all(|&(a, b, v)| a <= b && v + (b - a) <= 9));
    }

    #[test]
    fn display_matches_python_str_of_two_place_decimal() {
        assert_eq!(Money::from_cents(12501).to_string(), "125.01");
        assert_eq!(Money::from_cents(-500).to_string(), "-5.00");
        assert_eq!(Money::from_cents(5).to_string(), "0.05");
        assert_eq!(Money::ZERO.to_string(), "0.00");
    }

    #[test]
    fn decimal_round_trip_and_half_up() {
        assert_eq!(Money::from_cents(12501).to_decimal().to_string(), "125.01");
        assert_eq!(round_half_up("2.675".parse().unwrap(), 2).to_string(), "2.68");
        assert_eq!(round_half_up("2.665".parse().unwrap(), 2).to_string(), "2.67");
        assert_eq!(money(&AmountInput::Decimal("125.005".parse().unwrap())).unwrap().to_string(), "125.01");
        assert!(money(&AmountInput::Decimal("-0.01".parse().unwrap())).is_err());
    }

    /// ledger/tests.py test_money_rejects_floats_nonfinite_negative_and_overflow, translated.
    #[test]
    fn money_rejects_floats_nonfinite_negative_and_overflow() {
        for value in [
            AmountInput::Float(1.25),
            AmountInput::Bool(true),
            AmountInput::Text("NaN".into()),
            AmountInput::Text("Infinity".into()),
            AmountInput::Text("-0.001".into()),
            AmountInput::Text("-1".into()),
            AmountInput::Text("100000000000000".into()),
            AmountInput::Text("not money".into()),
            AmountInput::Other,
        ] {
            assert_eq!(money(&value), Err(MoneyError), "{value:?}");
        }
        assert_eq!(text("1.005").unwrap(), "1.01");
    }

    #[test]
    fn sqlite_float_rounding_trap_does_not_apply() {
        // D2 measured SQLite round(2.675, 2) = 2.67; the core must say 2.68 like Python.
        assert_eq!(text("2.675").unwrap(), "2.68");
    }
}
