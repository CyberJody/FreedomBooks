use crate::{protocol, OpenAi};
use fb_plugin_api::*;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

pub const INSTRUCTION: &str = include_str!("receipt-instruction.txt");
pub const TEXT_FORMAT: &str = include_str!("receipt-schema.json");

/// Host checks membership, company opt-in, unlocked book, file integrity and rate
/// limits. Called again after refresh, immediately before sending receipt bytes.
pub trait ReceiptConsent {
    fn check(&self, cx: &PluginContext) -> PluginResult<()>;
}
impl<F: Fn(&PluginContext) -> PluginResult<()>> ReceiptConsent for F {
    fn check(&self, cx: &PluginContext) -> PluginResult<()> {
        self(cx)
    }
}
pub struct ReceiptRequest {
    pub attachment: Attachment,
    pub model: String,
    pub currency: String,
    pub today: Date,
}
/// Printed amounts and original line indexes must remain authoritative on the host.
/// Deliberately no Debug: a receipt is private, not log metadata.
#[derive(Serialize, Deserialize)]
pub struct ReceiptLine {
    pub description: String,
    pub quantity: i64,
    pub unit_price_cents: i64,
    pub printed_amount_cents: i64,
    pub product_cents: i64,
    pub mismatch: bool,
    pub tax: bool,
}
#[derive(Serialize, Deserialize)]
pub struct ReceiptReading {
    pub vendor_name: String,
    pub vendor_address: Vec<String>,
    pub document_number: String,
    pub document_date: Option<String>,
    pub due_date: Option<String>,
    pub currency: String,
    pub subtotal_cents: i64,
    pub tax_cents: i64,
    pub total_cents: i64,
    pub lines: Vec<ReceiptLine>,
    pub notes: Vec<ReceiptNote>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReceiptNote {
    CurrencyAssumed,
    NoLines,
    QuantityOrPriceFilled,
    LineMath,
    LinesSum,
    Total,
    TaxLine,
    MissingDate,
    InvalidDueDate,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawReading {
    vendor_name: String,
    vendor_address: Vec<String>,
    document_number: String,
    document_date: String,
    due_date: String,
    currency: String,
    subtotal: String,
    tax: String,
    total: String,
    lines: Vec<RawLine>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLine {
    description: String,
    quantity: String,
    unit_price: String,
    amount: String,
}

impl OpenAi {
    pub fn read_receipt(
        &self,
        cx: &PluginContext,
        request: ReceiptRequest,
        consent: &dyn ReceiptConsent,
    ) -> PluginResult<ReceiptReading> {
        self.audited(
            cx,
            AuditAction::Respond,
            AuditPurpose::ReceiptReading,
            || {
                consent.check(cx)?;
                let lock = self.connection_lock(cx.scope())?;
                let _guard = lock.lock().map_err(|_| PluginError::CredentialStore)?;
                let record = self.bearer(cx)?;
                consent.check(cx)?;
                let bearer = if record.api_key.expose_secret().is_empty() {
                    &record.access_token
                } else {
                    &record.api_key
                };
                let response = protocol::respond(
                    cx,
                    &self.endpoints.api,
                    bearer,
                    AiRequest {
                        purpose: AiPurpose::ReadReceipt,
                        instructions: SecretString::from(INSTRUCTION),
                        input: vec![
                            AiInput::Text(SecretString::from("Read this receipt or bill.")),
                            AiInput::File(request.attachment),
                        ],
                        text_format: Some(TEXT_FORMAT.into()),
                        model: Some(request.model),
                    },
                );
                if matches!(response, Err(PluginError::NeedsSignIn)) {
                    crate::auth::invalidate(cx, record)?;
                }
                parse_reply(
                    response?.output.expose_secret(),
                    &request.currency,
                    request.today,
                )
            },
        )
    }
    /// Stages only a fully arithmetically consistent draft. A mismatched reading is
    /// returned for review, never silently repaired. Wave 4 owns editable review,
    /// explicit "Keep as entered" audit, and authoritative reading persistence.
    pub fn stage_receipt_draft(
        &self,
        cx: &PluginContext,
        source_id: Uuid,
        reading: &ReceiptReading,
    ) -> PluginResult<DraftRef> {
        self.audited(
            cx,
            AuditAction::CreateDraft,
            AuditPurpose::ReceiptReading,
            || {
                if source_id.is_nil()
                    || reading.lines.is_empty()
                    || reading.lines.iter().any(|l| {
                        !line_matches(l.quantity, l.unit_price_cents, l.printed_amount_cents)
                    })
                {
                    return Err(PluginError::Core);
                }
                let date = reading
                    .document_date
                    .as_deref()
                    .and_then(parse_date)
                    .ok_or(PluginError::Core)?;
                let lines = reading
                    .lines
                    .iter()
                    .map(|l| ImportedLine {
                        description: l.description.clone(),
                        quantity: Quantity {
                            ten_thousandths: l.quantity,
                        },
                        unit_price: Amount {
                            cents: l.unit_price_cents,
                            currency: reading.currency.clone(),
                        },
                        // Python represents purchase tax as a separate expense line.
                        tax_collected: Amount {
                            cents: 0,
                            currency: reading.currency.clone(),
                        },
                    })
                    .collect();
                cx.core().create_draft_bill(DraftBill {
                    source_remote_id: source_id.to_string(),
                    vendor: reading.vendor_name.clone(),
                    date,
                    lines,
                })
            },
        )
    }
}
/// Equivalent to Python's quantity × unit price quantized to cents (half-even).
/// A host review must compare to its stored printed amount, never a posted UI value.
pub fn line_matches(quantity: i64, price_cents: i64, printed_cents: i64) -> bool {
    quantity > 0
        && price_cents >= 0
        && printed_cents >= 0
        && product(quantity, price_cents).is_ok_and(|value| value == printed_cents)
}
fn product(quantity: i64, price_cents: i64) -> PluginResult<i64> {
    // i64 × i64 fits i128; divide before narrowing so hostile large quantities
    // cannot panic or overflow Decimal's 96-bit mantissa.
    let n = i128::from(quantity) * i128::from(price_cents);
    let quotient = n / 10_000;
    let remainder = n % 10_000;
    let round = remainder.abs() > 5_000 || (remainder.abs() == 5_000 && quotient % 2 != 0);
    let rounded = if round {
        quotient + n.signum()
    } else {
        quotient
    };
    i64::try_from(rounded).map_err(|_| PluginError::InvalidResponse)
}
fn clean(s: &str, max: usize) -> String {
    s.nfc()
        .filter(|c| {
            use unicode_general_category::{get_general_category, GeneralCategory};
            !matches!(
                get_general_category(*c),
                GeneralCategory::Control
                    | GeneralCategory::Format
                    | GeneralCategory::PrivateUse
                    | GeneralCategory::Surrogate
                    | GeneralCategory::Unassigned
            ) || matches!(c, '\n' | '\t')
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}
fn number(s: &str) -> String {
    let mut s = s.to_uppercase();
    for marker in ["USD", "EUR", "GBP", "CAD", "AUD"] {
        s = s.replace(marker, "");
    }
    let s: String = s
        .chars()
        .filter(|c| !c.is_whitespace() && !",$€£¥".contains(*c))
        .collect();
    if s.starts_with('(') && s.ends_with(')') {
        format!("-{}", &s[1..s.len() - 1])
    } else {
        s
    }
}
/// Quantize the ORIGINAL decimal coefficient to cents, half-even, just as
/// Python Decimal(text).quantize(CENT) does. A fixed-precision Decimal parse can
/// round before quantization (double rounding), even when the final cents fit.
/// Only the final, bounded integer cents are materialized as a machine number.
fn money(s: &str) -> PluginResult<Option<i64>> {
    let text = number(s);
    if text.is_empty() || text == "-" {
        return Ok(None);
    }
    scaled_decimal(&text, 2, true).map(Some)
}
// Quantities use this same exact path, with no rounding allowed. Otherwise a
// high-precision quantity could round before its four-place equality check.
fn scaled_decimal(text: &str, places: u8, round: bool) -> PluginResult<i64> {
    let invalid = || PluginError::InvalidResponse;
    let negative = text.starts_with('-');
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (mantissa, exponent) = match unsigned.split_once('E') {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i64>().map_err(|_| invalid())?),
        None => (unsigned, 0),
    };
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if integer.is_empty() && fraction.is_empty()
        || !integer
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return Err(invalid());
    }
    let coefficient = format!("{integer}{fraction}");
    let digits = coefficient.trim_start_matches('0').as_bytes();
    if digits.is_empty() {
        return Ok(0); // Python accepts signed zero, including -0e-100.
    }
    if negative {
        return Err(invalid());
    }
    // Decimal position AFTER multiplying by 10^places. i128 accommodates the entire
    // i64 exponent and bounded input length without overflow or huge padding.
    let cut =
        digits.len() as i128 + i128::from(exponent) - fraction.len() as i128 + i128::from(places);
    let max_digits = usize::from(places) + 14;
    if cut > max_digits as i128 {
        return Err(invalid());
    }
    if cut < 0 {
        // Tiny money rounds to zero; a nonzero quantity is not four-place exact.
        return if round { Ok(0) } else { Err(invalid()) };
    }
    let cut = usize::try_from(cut).map_err(|_| invalid())?;
    // Check the unrounded value against Python's maximum (14 integer digits
    // plus `places` nines). A tail above the exact maximum is always invalid.
    if cut == max_digits
        && digits.len() > cut
        && digits[..cut].iter().all(|b| *b == b'9')
        && digits[cut..].iter().any(|b| *b != b'0')
    {
        return Err(invalid());
    }
    let mut cents = 0i64;
    for i in 0..cut {
        cents = cents * 10 + i64::from(digits.get(i).copied().unwrap_or(b'0') - b'0');
    }
    let tail = digits.get(cut..).unwrap_or_default();
    if !round && tail.iter().any(|b| *b != b'0') {
        return Err(invalid());
    }
    let round_up = round
        && tail.first().is_some_and(|first| {
            *first > b'5'
                || (*first == b'5' && (tail[1..].iter().any(|b| *b != b'0') || cents % 2 != 0))
        });
    Ok(cents + i64::from(round_up))
}
fn quantity(s: &str) -> Option<i64> {
    scaled_decimal(&number(s), 4, false).ok().filter(|n| *n > 0)
}
fn parse_date(s: &str) -> Option<Date> {
    let s = s.trim().get(..10)?;
    if s.as_bytes()[4] != b'-' || s.as_bytes()[7] != b'-' {
        return None;
    }
    Date::new(
        s.get(..4)?.parse().ok()?,
        s.get(5..7)?.parse().ok()?,
        s.get(8..)?.parse().ok()?,
    )
    .ok()
}
fn date_text(d: Date) -> String {
    let (y, m, d) = d.parts();
    format!("{y:04}-{m:02}-{d:02}")
}
fn ordinal(d: Date) -> i64 {
    let (y, m, d) = d.parts();
    let year = i64::from(y) - 1;
    let days_before_year = 365 * year + year / 4 - year / 100 + year / 400;
    let before_month: u16 = [31u16, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        .iter()
        .take(usize::from(m) - 1)
        .sum();
    days_before_year
        + i64::from(before_month)
        + i64::from(d)
        + i64::from(m > 2 && Date::new(y, 2, 29).is_ok())
}
pub fn parse_reply(text: &str, currency: &str, today: Date) -> PluginResult<ReceiptReading> {
    if text.len() > 2 * 1024 * 1024
        || currency.len() != 3
        || !currency.bytes().all(|c| c.is_ascii_uppercase())
    {
        return Err(PluginError::InvalidResponse);
    }
    let raw: RawReading = serde_json::from_str(text).map_err(|_| PluginError::InvalidResponse)?;
    let mut notes = vec![];
    let found = clean(&raw.currency, 3).to_uppercase();
    if !found.is_empty() && found != currency {
        return Err(PluginError::InvalidResponse);
    }
    if found.is_empty() {
        notes.push(ReceiptNote::CurrencyAssumed);
    }
    let vendor_name = clean(&raw.vendor_name, 200);
    let total = money(&raw.total)?.ok_or(PluginError::InvalidResponse)?;
    let tax = money(&raw.tax)?.unwrap_or(0);
    let subtotal = money(&raw.subtotal)?;
    if raw.lines.len() > 49 {
        return Err(PluginError::TooLarge);
    }
    let mut lines = vec![];
    for (i, line) in raw.lines.into_iter().enumerate() {
        let q = quantity(&line.quantity);
        let p = money(&line.unit_price)?;
        let amount = money(&line.amount)?
            .or_else(|| product(q?, p?).ok())
            .ok_or(PluginError::InvalidResponse)?;
        let (q, p) = if let (Some(q), Some(p)) = (q, p) {
            (q, p)
        } else {
            notes.push(ReceiptNote::QuantityOrPriceFilled);
            (10_000, amount)
        };
        let product = product(q, p)?;
        let mismatch = product != amount;
        if mismatch {
            notes.push(ReceiptNote::LineMath);
        }
        let description = clean(&line.description, 1000);
        lines.push(ReceiptLine {
            description: if description.is_empty() {
                format!("Line {}", i + 1)
            } else {
                description
            },
            quantity: q,
            unit_price_cents: p,
            printed_amount_cents: amount,
            product_cents: product,
            mismatch,
            tax: false,
        });
    }
    if lines.is_empty() {
        let amount = subtotal.unwrap_or(if tax <= total { total - tax } else { total });
        lines.push(ReceiptLine {
            description: if vendor_name.is_empty() {
                "Receipt total".into()
            } else {
                format!("Purchase from {vendor_name}")
            },
            quantity: 10_000,
            unit_price_cents: amount,
            printed_amount_cents: amount,
            product_cents: amount,
            mismatch: false,
            tax: false,
        });
        notes.push(ReceiptNote::NoLines);
    }
    let sum = lines
        .iter()
        .try_fold(0i64, |s, l| s.checked_add(l.printed_amount_cents))
        .ok_or(PluginError::InvalidResponse)?;
    let subtotal = subtotal.unwrap_or(sum);
    if subtotal != sum {
        notes.push(ReceiptNote::LinesSum);
    }
    if subtotal.checked_add(tax) != Some(total) {
        notes.push(ReceiptNote::Total);
    }
    if tax > 0 {
        lines.push(ReceiptLine {
            description: "Sales tax (as printed on the receipt)".into(),
            quantity: 10_000,
            unit_price_cents: tax,
            printed_amount_cents: tax,
            product_cents: tax,
            mismatch: false,
            tax: true,
        });
        notes.push(ReceiptNote::TaxLine);
    }
    let date = parse_date(&raw.document_date)
        .filter(|d| d.parts().0 >= 1990 && ordinal(*d) <= ordinal(today) + 366);
    if date.is_none() {
        notes.push(ReceiptNote::MissingDate);
    }
    let due = parse_date(&raw.due_date).filter(|due| {
        date.is_some_and(|d| ordinal(*due) >= ordinal(d) && ordinal(*due) <= ordinal(d) + 366)
    });
    if !raw.due_date.is_empty() && due.is_none() {
        notes.push(ReceiptNote::InvalidDueDate);
    }
    Ok(ReceiptReading {
        vendor_name,
        vendor_address: raw
            .vendor_address
            .iter()
            .take(5)
            .map(|s| clean(s, 200))
            .filter(|s| !s.is_empty())
            .collect(),
        document_number: clean(&raw.document_number, 120),
        document_date: date.map(date_text),
        due_date: due.map(date_text),
        currency: currency.into(),
        subtotal_cents: subtotal,
        tax_cents: tax,
        total_cents: total,
        lines,
        notes,
    })
}
