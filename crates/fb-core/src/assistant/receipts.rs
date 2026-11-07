//! Frozen receipt parser and local matching. No provider calls or file bytes.
use super::*;
use chrono::{Days, NaiveDate};
use rust_decimal::Decimal;
use unicode_normalization::{UnicodeNormalization, char::canonical_combining_class};

#[derive(Debug)]
pub struct ReceiptError {
    pub code: &'static str,
    message: String,
}
impl ReceiptError {
    fn new(code: &'static str, message: impl Into<String>) -> Self { Self { code, message: message.into() } }
    pub fn message(&self) -> String { self.message.clone() }
}
impl std::fmt::Display for ReceiptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.message) }
}
impl std::error::Error for ReceiptError {}
impl From<ReceiptError> for FbError {
    fn from(error: ReceiptError) -> Self { FbError::validation(error.message) }
}
type ReceiptResult<T> = Result<T, ReceiptError>;
fn schema(m: impl Into<String>) -> ReceiptError { ReceiptError::new("schema", m) }
fn number_junk(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while !rest.is_empty() {
        // Python's case-insensitive ASCII regex also matches the long s in USD.
        let prefix = rest
            .chars()
            .take(3)
            .map(|c| if c == '\u{17f}' { 's' } else { c })
            .collect::<String>();
        if ["USD", "EUR", "GBP", "CAD", "AUD"]
            .iter()
            .any(|code| prefix.eq_ignore_ascii_case(code))
        {
            let bytes = rest.chars().take(3).map(char::len_utf8).sum::<usize>();
            rest = &rest[bytes..];
        } else if let Some(c) = rest.chars().next() {
            if !fb_money::py_isspace(c) && !",$€£¥".contains(c) {
                out.push(c);
            }
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

#[derive(Clone, Copy, Debug)]
struct Cash {
    cents: i128,
    negative_zero: bool,
}
impl Cash {
    fn new(cents: i128) -> Self {
        Self {
            cents,
            negative_zero: false,
        }
    }
    fn product(q: i128, r: Self) -> ReceiptResult<Self> {
        let cents = fb_db::assistant::product(q, r.cents)
            .ok_or_else(|| schema("Receipt amount exceeds supported limits."))?;
        if cents > fb_db::assistant::EVIDENCE_MAX {
            return Err(schema("Receipt amount exceeds supported limits."));
        }
        Ok(Self {
            cents,
            negative_zero: cents == 0 && r.negative_zero,
        })
    }
    fn plus(self, rhs: Self) -> ReceiptResult<Self> {
        let cents = self
            .cents
            .checked_add(rhs.cents)
            .ok_or_else(|| schema("Receipt amount exceeds supported limits."))?;
        Ok(Self {
            cents,
            negative_zero: cents == 0 && self.negative_zero && rhs.negative_zero,
        })
    }
    fn minus(self, rhs: Self) -> ReceiptResult<Self> {
        let cents = self
            .cents
            .checked_sub(rhs.cents)
            .ok_or_else(|| schema("Receipt amount exceeds supported limits."))?;
        Ok(Self {
            cents,
            negative_zero: cents == 0 && self.negative_zero && !rhs.negative_zero,
        })
    }
}
impl PartialEq for Cash {
    fn eq(&self, rhs: &Self) -> bool {
        self.cents == rhs.cents
    }
}
impl Eq for Cash {}
impl std::fmt::Display for Cash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.negative_zero {
            f.write_str("-0.00")
        } else {
            Money::from_cents(self.cents).fmt(f)
        }
    }
}
fn money(v: &Value, field: &str) -> ReceiptResult<Option<Cash>> {
    let value = v
        .as_str()
        .ok_or_else(|| schema(format!("The AI reply had no usable {field}.")))?;
    let mut text = number_junk(value);
    if text.is_empty() || text == "-" {
        return Ok(None);
    }
    if text.starts_with('(') && text.ends_with(')') {
        text = format!("-{}", &text[1..text.len() - 1]);
    }
    let parsed = fb_money::parse_python_decimal(&text).ok_or_else(|| {
        schema(format!(
            "The AI reply's {field} (\"{}\") is not a number, so nothing was filled in.",
            value.chars().take(40).collect::<String>()
        ))
    })?;
    let range = || {
        schema(format!(
            "The AI reply's {field} is out of range, so nothing was filled in."
        ))
    };
    let fb_money::Parsed::Finite(n) = parsed else {
        return Err(range());
    };
    if n.negative && !n.is_zero() {
        return Err(ReceiptError::new("negative",
            "This document shows a negative amount (a refund or credit). Enter it by hand.",
        ));
    }
    let digits = n.digits.trim_start_matches('0');
    let at = (digits.len() as i128)
        .checked_add(n.exponent)
        .and_then(|n| n.checked_add(2))
        .ok_or_else(range)?;
    // Python checks the exact unrounded input against MONEY_MAX, including fractional cents.
    if !digits.is_empty()
        && (at > 16
            || (at == 16
                && digits.bytes().take(16).all(|b| b == b'9')
                && digits.bytes().skip(16).any(|b| b != b'0')))
    {
        return Err(range());
    }
    let mut cents = n.cents_half_up().ok_or_else(range)?;
    if n.exponent < -2 && at >= 0 && at < (digits.len() as i128) {
        let tail = &digits[at as usize..];
        // A half-up result with odd parity came from an even retained coefficient.
        if tail.starts_with('5') && tail.bytes().skip(1).all(|b| b == b'0') && cents % 2 == 1 {
            cents -= 1;
        }
    }
    if cents > fb_db::assistant::MAX {
        return Err(range());
    }
    Ok(Some(Cash {
        cents,
        negative_zero: cents == 0 && n.negative,
    }))
}
fn quantity(v: &Value) -> Option<i128> {
    let fb_money::Parsed::Finite(mut n) =
        fb_money::parse_python_decimal(&number_junk(v.as_str()?))?
    else {
        return None;
    };
    if n.negative || n.is_zero() {
        return None;
    }
    while n.digits.ends_with('0') {
        n.digits.pop();
        n.exponent = n.exponent.checked_add(1)?;
    }
    let digits = n.digits.trim_start_matches('0');
    let shift = n.exponent.checked_add(4)?;
    if shift < 0 || (digits.len() as i128).checked_add(shift)? > 18 {
        return None;
    }
    let value = digits
        .parse::<i128>()
        .ok()?
        .checked_mul(10i128.checked_pow(shift.try_into().ok()?)?)?;
    (value <= 999_999_999_999_999_999).then_some(value)
}
fn text(v: &Value, max: usize, field: &str) -> ReceiptResult<String> {
    let s = v
        .as_str()
        .ok_or_else(|| schema(format!("The AI reply had no usable {field}.")))?;
    let s = s
        .nfc()
        .filter(|c| {
            matches!(c, '\n' | '\t' | '\u{2028}' | '\u{2029}')
                || fb_db::check_numbers::char_allowed(*c)
        })
        .collect::<String>();
    Ok(s.split(fb_money::py_isspace)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect())
}
fn date(v: &Value) -> Option<NaiveDate> {
    let s = v.as_str()?;
    let short = py::py_strip(s).chars().take(10).collect::<String>();
    let parsed = fb_db::dates::parse_iso_date(&short)?;
    NaiveDate::parse_from_str(&parsed, "%Y-%m-%d").ok()
}

pub fn plain(q: i128) -> String {
    Decimal::from_i128_with_scale(q, 4).normalize().to_string()
}
/// Offline equivalent of assistant.receipts.parse_reply, with an injected local calendar date.
pub fn parse_reply(raw: &str, currency: &str, today: &str) -> ReceiptResult<Value> {
    let data = fb_db::parse_unique_json(raw).map_err(|_| {
        ReceiptError::new("bad_json", "The AI reply was not valid JSON, so nothing was filled in. Try again.")
    })?;
    let fields = [
        "vendor_name",
        "vendor_address",
        "document_number",
        "document_date",
        "due_date",
        "currency",
        "subtotal",
        "tax",
        "total",
        "lines",
    ];
    if data
        .as_object()
        .is_none_or(|m| m.len() != fields.len() || fields.iter().any(|k| !m.contains_key(*k)))
    {
        return Err(schema(
            "The AI reply did not have the expected fields, so nothing was filled in.",
        ));
    }
    let mut notes = vec![];
    let mut warnings = vec![];
    fn warn(notes: &mut Vec<String>, warnings: &mut Vec<String>, s: String) {
        notes.push(s.clone());
        warnings.push(s);
    }
    let name = text(&data["vendor_name"], 200, "vendor name")?;
    let address = data["vendor_address"]
        .as_array()
        .filter(|a| a.iter().all(Value::is_string))
        .ok_or_else(|| schema("The AI reply had no usable vendor address."))?;
    let address = address
        .iter()
        .take(5)
        .map(|v| text(v, 200, "address"))
        .collect::<ReceiptResult<Vec<_>>>()?
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    let number = text(&data["document_number"], 120, "document number")?;
    let found = text(&data["currency"], 3, "currency")?.to_uppercase();
    if !found.is_empty() && found != currency {
        return Err(ReceiptError::new("currency", format!(
            "The receipt is in {found}, but this company keeps its books in {currency}, so nothing was filled in. Enter the bill by hand in {currency}."
        )));
    }
    if found.is_empty() {
        warn(
            &mut notes,
            &mut warnings,
            format!("Currency not printed on the receipt: assumed {currency}. Check it."),
        );
    }
    let mut subtotal = money(&data["subtotal"], "subtotal")?;
    let tax = money(&data["tax"], "tax")?.unwrap_or(Cash::new(0));
    let total = money(&data["total"], "total")?.ok_or_else(|| {
        schema("The AI could not read a total on this document, so nothing was filled in.")
    })?;
    let raw_lines = data["lines"]
        .as_array()
        .ok_or_else(|| schema("The AI reply had no usable lines."))?;
    if raw_lines.len() > 49 {
        return Err(schema(
            "The document has more than 49 lines, more than a bill can hold. Enter it by hand.",
        ));
    }
    let mut lines = vec![];
    for (i, l) in raw_lines.iter().enumerate() {
        let n = i + 1;
        if l.as_object().is_none_or(|m| {
            m.len() != 4
                || ["description", "quantity", "unit_price", "amount"]
                    .iter()
                    .any(|k| !m.contains_key(*k))
        }) {
            return Err(schema(
                "The AI reply had a line without the expected fields.",
            ));
        }
        let mut description = text(&l["description"], 1000, "line description")?;
        if description.is_empty() {
            description = format!("Line {n}");
        }
        let amount = money(&l["amount"], &format!("line {n} amount"))?;
        let q = quantity(&l["quantity"]);
        let r = money(&l["unit_price"], &format!("line {n} unit price"))?;
        let amount = match amount {
            Some(a) => a,
            None => match (q, r) {
                (Some(q), Some(r)) => Cash::product(q, r)?,
                _ => return Err(schema(format!("Line {n} of the AI reply has no amount."))),
            },
        };
        let (q, r) = match (q, r) {
            (Some(q), Some(r)) => (q, r),
            _ => {
                notes.push(format!("Line {n}: no usable quantity or unit price was read; quantity 1 and the printed amount were used."));
                (10_000, amount)
            }
        };
        let product = Cash::product(q, r)?;
        let mut line = json!({"description":description,"quantity":plain(q),"rate":r.to_string(),"amount":amount.to_string()});
        if product != amount {
            line["mismatch"] = json!(true);
            warn(
                &mut notes,
                &mut warnings,
                format!(
                    "Line {n} doesn't add up: {} × {r} = {product}, receipt shows {amount}. Nothing was adjusted: fix the quantity or the unit price so the line matches the receipt before you save.",
                    plain(q)
                ),
            );
        }
        lines.push(line);
    }
    if lines.is_empty() {
        let base = match subtotal {
            Some(sub) => sub,
            None => {
                if tax.cents <= total.cents {
                    total.minus(tax)?
                } else {
                    total
                }
            }
        };
        lines.push(json!({"description":if name.is_empty(){"Receipt total".into()}else{format!("Purchase from {name}")},"quantity":"1","rate":base.to_string(),"amount":base.to_string()}));
        notes.push("No charged lines could be read; one line with the receipt total was filled in. Split it as needed.".into());
    }
    let sum = lines
        .iter()
        .try_fold(0i128, |n, l| {
            n.checked_add(fb_db::assistant::scaled(
                &l["amount"],
                2,
                fb_db::assistant::EVIDENCE_MAX,
                false,
            )?)
        })
        .map(Cash::new)
        .ok_or_else(|| schema("Receipt amount exceeds supported limits."))?;
    if let Some(sub) = subtotal {
        if sub != sum {
            warn(
                &mut notes,
                &mut warnings,
                format!(
                    "The lines add up to {sum}, but the receipt's subtotal reads {sub}. Nothing was adjusted: check the lines."
                ),
            );
        }
    } else {
        subtotal = Some(sum);
    }
    let sub = subtotal.ok_or_else(|| schema("Invalid receipt subtotal."))?;
    let combined = sub.plus(tax)?;
    if combined != total {
        warn(
            &mut notes,
            &mut warnings,
            format!(
                "Subtotal {sub} plus tax {tax} is {combined}, but the receipt's total reads {total}. Nothing was adjusted: check the amounts."
            ),
        );
    }
    if tax.cents > 0 {
        lines.push(json!({"description":"Sales tax (as printed on the receipt)","quantity":"1","rate":tax.to_string(),"amount":tax.to_string(),"tax":true}));
        notes.push(format!("The receipt shows {tax} tax; it was added as a separate line so the bill total matches the receipt."));
    }
    let today = NaiveDate::parse_from_str(today, "%Y-%m-%d")
        .map_err(|_| schema("Enter a valid calendar date."))?;
    let latest = today
        .checked_add_days(Days::new(366))
        .ok_or_else(|| schema("Enter a valid calendar date."))?;
    let dt = date(&data["document_date"])
        .filter(|d| d.to_string().as_str() >= "1990-01-01" && *d <= latest);
    if dt.is_none() {
        notes.push(
            "The receipt date could not be read as a sensible calendar date; enter it.".into(),
        );
    }
    let mut due = date(&data["due_date"]);
    if due.is_some_and(|d| {
        dt.is_none_or(|dt| {
            d < dt
                || dt
                    .checked_add_days(Days::new(366))
                    .is_none_or(|last| d > last)
        })
    }) {
        due = None;
        notes.push("The due date on the receipt was not usable.".into());
    }
    let result = json!({"vendor_name":name,"vendor_address":address,"document_number":number,"document_date":dt.map(|d|d.to_string()).unwrap_or_default(),"due_date":due.map(|d|d.to_string()).unwrap_or_default(),"currency":if found.is_empty(){currency}else{&found},"subtotal":sub.to_string(),"tax":tax.to_string(),"total":total.to_string(),"lines":lines,"notes":notes,"warnings":warnings});
    if !fb_db::assistant::result(&result) {
        return Err(schema("Receipt amount exceeds supported limits."));
    }
    Ok(result)
}
pub fn normalize_name(s: &str) -> String {
    let frozen = s
        .chars()
        .map(|c| {
            if fb_db::check_numbers::char_allowed(c) {
                c
            } else {
                ' '
            }
        })
        .collect::<String>();
    let normalized = frozen.nfkd().collect::<String>();
    let folded = icu_casemap::CaseMapper::new().fold_string(&normalized);
    let cleaned = folded
        .chars()
        .filter(|c| canonical_combining_class(*c) == 0)
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect::<String>();
    let mut words = cleaned.split_whitespace().collect::<Vec<_>>();
    while words.last().is_some_and(|s| {
        [
            "inc",
            "incorporated",
            "llc",
            "ltd",
            "limited",
            "co",
            "corp",
            "corporation",
            "company",
            "plc",
            "gmbh",
        ]
        .contains(s)
    }) {
        words.pop();
    }
    words.join(" ")
}
