//! Small pieces of Python/Django behaviour the ledger services depend on, ported exactly where the
//! books can see the difference and documented where they are narrowed.

use chrono::NaiveDate;
use fb_money::py_isspace;
use serde_json::Value;

/// Python `str.strip()` with no argument (Unicode whitespace per `str.isspace`).
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(py_isspace)
}

/// Python `len(str)`: code points.
pub fn py_len(s: &str) -> usize {
    s.chars().count()
}

/// Python `s[:n]` on code points.
pub fn py_truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Django's `max_length` message.
pub fn max_length_message(limit: usize, value: &str) -> String {
    format!("Ensure this value has at most {limit} characters (it has {}).", py_len(value))
}

pub const BLANK: &str = "This field cannot be blank.";

/// Django model `DateField.to_python` (used by `full_clean`): `date.fromisoformat` (extended
/// `YYYY-MM-DD` and basic `YYYYMMDD`), then the regex `\d{4}-\d{1,2}-\d{1,2}$`.
/// Includes ISO week dates and the Unicode decimal digits accepted by Django's regex fallback.
pub fn model_date(value: &str) -> Result<NaiveDate, String> {
    let invalid_format = || format!("“{value}” value has an invalid date format. It must be in YYYY-MM-DD format.");
    let invalid_date = || format!("“{value}” value has the correct format (YYYY-MM-DD) but it is an invalid date.");
    let digits = |s: &[u8]| !s.is_empty() && s.iter().all(|c| c.is_ascii_digit());
    if let Some(canonical)=fb_db::dates::parse_iso_date(value) {
        return NaiveDate::parse_from_str(&canonical,"%Y-%m-%d").map_err(|_|invalid_format());
    }
    // date_re = r"(?P<year>\d{4})-(?P<month>\d{1,2})-(?P<day>\d{1,2})$" (re.match: anchored at the start;
    // `$` also matches before one trailing newline)
    let normalized: String = value.chars().map(|c| fb_money::unicode_decimal(c).and_then(|n| char::from_digit(n, 10)).unwrap_or(c)).collect();
    let core = normalized.strip_suffix('\n').unwrap_or(&normalized);
    let parts: Vec<&str> = core.split('-').collect();
    if parts.len() == 3
        && parts[0].len() == 4
        && digits(parts[0].as_bytes())
        && (1..=2).contains(&parts[1].len())
        && digits(parts[1].as_bytes())
        && (1..=2).contains(&parts[2].len())
        && digits(parts[2].as_bytes())
    {
        return ymd(parts[0], parts[1], parts[2]).ok_or_else(invalid_date);
    }
    Err(invalid_format())
}

fn ymd(y: &str, m: &str, d: &str) -> Option<NaiveDate> {
    let year: i32 = y.parse().ok()?;
    if year < 1 {
        return None;
    }
    NaiveDate::from_ymd_opt(year, m.parse().ok()?, d.parse().ok()?)
}

/// Django `forms.DateField().clean(value)` with the default input formats `%Y-%m-%d`, `%m/%d/%Y`,
/// `%m/%d/%y`, and the eight abbreviated/full month-name formats (value stripped first).
/// Empty -> "This field is required.", otherwise "Enter a valid date.".
pub fn form_date(value: &str) -> Result<NaiveDate, String> {
    let v = py_strip(value);
    if v.is_empty() {
        return Err("This field is required.".into());
    }
    let invalid = || "Enter a valid date.".to_string();
    let num = |s: &str, min: usize, max: usize| -> Option<u32> {
        if (min..=max).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_digit()) { s.parse().ok() } else { None }
    };
    let parts: Vec<&str> = v.split('-').collect();
    if parts.len() == 3 {
        let (y, m, d) = (unicode_number(parts[0], 4, 4), num(parts[1], 1, 2), strptime_day(parts[2]));
        if let (Some(y), Some(m), Some(d)) = (y, m, d) {
            return NaiveDate::from_ymd_opt(y as i32, m, d).filter(|_| y >= 1).ok_or_else(invalid);
        }
        return Err(invalid());
    }
    let parts: Vec<&str> = v.split('/').collect();
    if parts.len() == 3 {
        let (m, d) = (num(parts[0], 1, 2), strptime_day(parts[1]));
        let y = unicode_number(parts[2], 4, 4)
            .map(|y| y as i32)
            .or_else(|| unicode_number(parts[2], 2, 2).map(|y| if y < 69 { 2000 + y as i32 } else { 1900 + y as i32 }));
        if let (Some(m), Some(d), Some(y)) = (m, d, y) {
            return NaiveDate::from_ymd_opt(y, m, d).filter(|_| y >= 1).ok_or_else(invalid);
        }
    }
    // Django en-us DATE_INPUT_FORMATS: %b/%B %d[, ] %Y and %d %b/%B[, ] %Y.
    let words: Vec<&str> = v.split(py_isspace).filter(|word| !word.is_empty()).collect();
    if words.len() == 3 && !words[0].contains(',') {
        let middle = words[1].strip_suffix(',').unwrap_or(words[1]);
        let month = |s: &str| -> Option<u32> {
            const MONTHS: [&str; 12] =
                ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"];
            let lower = s.to_ascii_lowercase();
            MONTHS.iter().position(|m| lower == *m || lower == m[..3]).map(|i| i as u32 + 1)
        };
        let md = month(words[0]).zip(strptime_day(middle)).or_else(|| strptime_day(words[0]).zip(month(middle)).map(|(d, m)| (m, d)));
        if let (Some((m, d)), Some(y)) = (md, unicode_number(words[2], 4, 4)) {
            return NaiveDate::from_ymd_opt(y as i32, m, d).filter(|_| y >= 1).ok_or_else(invalid);
        }
    }
    Err(invalid())
}

// `_strptime` uses Unicode \d for years and parts of days, but ASCII alternatives for months.
fn unicode_number(s: &str, min: usize, max: usize) -> Option<u32> {
    if !(min..=max).contains(&s.chars().count()) {
        return None;
    }
    s.chars().try_fold(0u32, |n, c| n.checked_mul(10)?.checked_add(fb_money::unicode_decimal(c)?))
}
fn strptime_day(s: &str) -> Option<u32> {
    let chars: Vec<char> = s.chars().collect();
    let matches = match chars.as_slice() {
        [one] => ('1'..='9').contains(one),
        [first, second] => {
            ('1'..='2').contains(first)
                || (*first == '0' && ('1'..='9').contains(second))
                || (*first == '3' && matches!(second, '0' | '1'))
                || (*first == ' ' && ('1'..='9').contains(second))
        }
        _ => false,
    };
    if !matches {
        return None;
    }
    unicode_number(s.strip_prefix(' ').unwrap_or(s), 1, 2)
}
/// A base-10 Python `int(str)` result: a value that fits `i64`, or a larger magnitude (still a valid integer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PyInt {
    Small(i64),
    Large { negative: bool },
}

/// CPython `int(text)` (base 10): non-ASCII Unicode whitespace becomes a space and Unicode decimal digits
/// (frozen Python's repertoire) become ASCII; then ASCII whitespace is stripped, one optional sign, digits with
/// single underscores only between digits, and at most 4300 digits (`sys.int_info.default_max_str_digits`;
/// more is a ValueError). None = Python raises ValueError.
pub fn py_int(text: &str) -> Option<PyInt> {
    let mut ascii = Vec::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii() {
            ascii.push(ch as u8);
        } else if fb_money::py_isspace(ch) {
            ascii.push(b' ');
        } else {
            ascii.push(b'0' + u8::try_from(fb_money::unicode_decimal(ch)?).ok()?);
        }
    }
    let space = |c: &u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c);
    let start = ascii.iter().position(|c| !space(c))?;
    let end = ascii.iter().rposition(|c| !space(c))? + 1;
    let mut body = &ascii[start..end];
    let negative = body.first() == Some(&b'-');
    if matches!(body.first(), Some(b'-' | b'+')) {
        body = &body[1..];
    }
    if !body.first().is_some_and(u8::is_ascii_digit) || !body.last().is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let mut digits = 0usize;
    let mut value: Option<i64> = Some(0);
    for (i, &c) in body.iter().enumerate() {
        match c {
            b'_' if body[i - 1] != b'_' => continue,
            b'0'..=b'9' => {
                digits += 1;
                value = value.and_then(|v| v.checked_mul(10)).and_then(|v| v.checked_add(i64::from(c - b'0')));
            }
            _ => return None,
        }
    }
    if digits > 4300 {
        return None;
    }
    Some(match value {
        Some(v) => PyInt::Small(if negative { -v } else { v }),
        None => PyInt::Large { negative },
    })
}

/// Python `uuid.UUID(str)`: drop `urn:` / `uuid:`, strip braces, drop hyphens, require 32 code points,
/// then `int(hex, 16)` (so whitespace, `+`, `0x` and single `_` are accepted as Python accepts them).
/// None = Python raises ValueError.
pub fn py_uuid(value: &str) -> Option<uuid_text::Canonical> {
    let mut hex = value.replace("urn:", "").replace("uuid:", "");
    hex = hex.trim_matches(|c| c == '{' || c == '}').replace('-', "");
    if py_len(&hex) != 32 {
        return None;
    }
    Some(canonical_uuid(py_int_hex(&hex)?))
}

/// A v2 record ID (Python takes an int primary key here): `py_uuid`'s prefix, brace and hyphen handling,
/// then exactly 32 ASCII hex digits. Not a Python parse, so it keeps the strict grammar.
pub fn record_uuid(value: &str) -> Option<uuid_text::Canonical> {
    let mut hex = value.replace("urn:", "").replace("uuid:", "");
    hex = hex.trim_matches(|c| c == '{' || c == '}').replace('-', "");
    if hex.len() != 32 || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u128::from_str_radix(&hex, 16).ok().map(canonical_uuid)
}

fn canonical_uuid(value: u128) -> uuid_text::Canonical {
    let h = format!("{value:032x}");
    uuid_text::Canonical(format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..]))
}

/// CPython `int(text, 16)` limited to 0..2**128 (what `uuid.UUID` then accepts): the same Unicode
/// transform as `py_int`, ASCII whitespace stripped, one optional sign, an optional `0x`/`0X` prefix
/// that may be followed by one `_`, then hex digits with single underscores only between digits.
fn py_int_hex(text: &str) -> Option<u128> {
    let mut ascii = Vec::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii() {
            ascii.push(ch as u8);
        } else if fb_money::py_isspace(ch) {
            ascii.push(b' ');
        } else {
            ascii.push(b'0' + u8::try_from(fb_money::unicode_decimal(ch)?).ok()?);
        }
    }
    let space = |c: &u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c);
    let start = ascii.iter().position(|c| !space(c))?;
    let end = ascii.iter().rposition(|c| !space(c))? + 1;
    let mut body = &ascii[start..end];
    let negative = body.first() == Some(&b'-');
    if matches!(body.first(), Some(b'-' | b'+')) {
        body = &body[1..];
    }
    if let [b'0', b'x' | b'X', rest @ ..] = body {
        body = rest.strip_prefix(b"_").unwrap_or(rest);
    }
    if !body.first().is_some_and(u8::is_ascii_hexdigit) || !body.last().is_some_and(u8::is_ascii_hexdigit) {
        return None;
    }
    let mut value = 0u128;
    for (i, &c) in body.iter().enumerate() {
        if c == b'_' && body[i - 1] != b'_' {
            continue;
        }
        let digit = char::from(c).to_digit(16)?;
        value = value.checked_mul(16)?.checked_add(u128::from(digit))?;
    }
    (!negative || value == 0).then_some(value)
}

pub mod uuid_text {
    /// A UUID in canonical lowercase hyphenated text (what the book stores).
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Canonical(pub String);
}

/// Python `json.dumps(value, sort_keys=True)` (default separators `", "` / `": "`, `ensure_ascii=True`).
pub fn py_json_dumps_sorted(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, &mut out);
    out
}

fn write_value(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_value(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| a.chars().cmp(b.chars()));
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_string(key, out);
                out.push_str(": ");
                write_value(&map[*key], out);
            }
            out.push('}');
        }
    }
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{:04x}", unit));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// The general category starts with C (control, format, surrogate, private use, unassigned) or is
/// Zl / Zp, by frozen Python 3.13's Unicode 15.1 tables (fb-db's shared table, checked against the frozen
/// interpreter by `py/check_unicode_categories.py`), not the Rust crate's newer assignments.
pub fn is_control_or_format(ch: char) -> bool {
    !fb_db::check_numbers::char_allowed(ch)
}

/// UTC timestamp text for audit and posting times (Django stores aware datetimes; the accounting date
/// is separate).
pub fn now_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
}

/// SEC-FIX1 intended v1 difference. Reject invisible controls before normalization.
pub fn clean_free_text(text: &str) -> crate::FbResult<()> {
    if !fb_db::check_numbers::free_text_valid(text) {
        return Err(crate::FbError::validation("Control and formatting characters are not allowed."));
    }
    Ok(())
}

/// Notes may contain line endings; all other control/format restrictions still apply.
/// Validate before trimming so invisible characters at the edges cannot be discarded unnoticed.
pub fn clean_multiline_free_text(text: &str) -> crate::FbResult<()> {
    for line in text.split(['\r', '\n']) {
        clean_free_text(line)?;
    }
    Ok(())
}

/// An unchanged stored field is carried evidence, not newly typed input.
pub fn clean_changed_free_text(text: &str, previous: Option<&str>) -> crate::FbResult<()> {
    if previous != Some(text) { clean_free_text(text)?; }
    Ok(())
}

pub(crate) fn input_text(v: &Value, max: usize, required: bool, previous: Option<&str>) -> crate::FbResult<String> {
    if let Some(raw) = v.as_str() { clean_changed_free_text(raw, previous)?; }
    v.as_str().map(py_strip).filter(|s| py_len(s) <= max && (!required || !s.is_empty()))
        .map(str::to_owned).ok_or_else(|| crate::FbError::validation(format!(
            "Enter {}text up to {max} characters.", if required { "nonblank " } else { "" }
        )))
}

/// The origin position identifies a retained line, even when the revision reorders it.
pub(crate) fn retained_line_text<'a>(lines: Option<&'a Vec<Value>>, origin: &Value, key: &str) -> Option<&'a str> {
    let position = origin.as_u64()?;
    lines?.iter().find(|l| l["position"].as_u64() == Some(position))?[key].as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_text_uses_exact_field_and_origin_provenance() {
        let legacy = "Legacy\nline";
        assert!(clean_changed_free_text(legacy, Some(legacy)).is_ok());
        assert!(clean_changed_free_text(legacy, None).is_err());
        assert!(clean_changed_free_text("New\nline", Some(legacy)).is_err());
        let lines = vec![serde_json::json!({"position":2,"description":legacy}),serde_json::json!({"position":1,"description":"ordinary"})];
        assert!(input_text(&serde_json::json!(legacy),1000,true,retained_line_text(Some(&lines),&serde_json::json!(2),"description")).is_ok());
        assert!(input_text(&serde_json::json!(legacy),1000,true,retained_line_text(Some(&lines),&serde_json::json!(1),"description")).is_err());
        assert!(retained_line_text(Some(&lines),&Value::Null,"description").is_none());
    }

    /// Every scalar against an independent reading of the frozen 15.1 table; U+0897 (assigned Mn in
    /// Unicode 16, unassigned Cn in 15.1) is the known case where the crate's newer tables disagreed.
    #[test]
    fn control_or_format_uses_frozen_python_unicode_15_1_for_every_scalar() {
        let table = include_str!("../../fb-db/src/credits_number_categories.rs");
        let ranges: Vec<(u32, u32)> = table
            .split("(0x")
            .skip(1)
            .map(|part| {
                let (low, rest) = part.split_once(", 0x").unwrap();
                let high = rest.split(')').next().unwrap();
                (u32::from_str_radix(low, 16).unwrap(), u32::from_str_radix(high, 16).unwrap())
            })
            .collect();
        assert_eq!(ranges.len(), 712);
        let mut checked = 0;
        for cp in 0..=0x10ffffu32 {
            let Some(ch) = char::from_u32(cp) else { continue };
            let frozen = ranges.iter().any(|(low, high)| (*low..=*high).contains(&cp));
            assert_eq!(is_control_or_format(ch), frozen, "U+{cp:04X}");
            checked += 1;
        }
        assert_eq!(checked, 0x110000 - 0x800);
        assert!(is_control_or_format('\u{0897}'));
        assert!(!is_control_or_format('a') && is_control_or_format('\u{200b}') && is_control_or_format('\u{2028}'));
    }

    #[test]
    fn json_dumps_matches_python_shape() {
        let v = serde_json::json!({"b": [1, "x"], "a": {"z": true, "y": null}, "é": "🧾"});
        assert_eq!(py_json_dumps_sorted(&v), r#"{"a": {"y": null, "z": true}, "b": [1, "x"], "\u00e9": "\ud83e\uddfe"}"#);
    }

    #[test]
    fn dates_follow_django() {
        assert_eq!(model_date("2026-10-31").unwrap().to_string(), "2026-10-31");
        assert_eq!(model_date("20261031").unwrap().to_string(), "2026-10-31");
        assert_eq!(model_date("2026-1-5").unwrap().to_string(), "2026-01-05");
        assert!(model_date("2026-02-30").unwrap_err().contains("invalid date."));
        assert!(model_date("bad").unwrap_err().contains("invalid date format"));
        assert_eq!(form_date(" 10/31/26 ").unwrap().to_string(), "2026-10-31");
        assert_eq!(form_date("bad").unwrap_err(), "Enter a valid date.");
    }

    #[test]
    fn uuid_text_follows_python() {
        let c = py_uuid("{6D1F3A52-0D1E-4C6B-9A57-1E5B3F0C2A10}").unwrap();
        assert_eq!(c.0, "6d1f3a52-0d1e-4c6b-9a57-1e5b3f0c2a10");
        assert!(py_uuid("nope").is_none());
    }

    // Review 2 of FIX3 (finding 1): `uuid.UUID` runs `int(hex, 16)` after the 32-character check.
    #[test]
    fn uuid_text_accepts_python_int_grammar() {
        let key = |s: &str| py_uuid(s).map(|c| c.0);
        let tail = |s: &str| Some(format!("00000000-0000-0000-0000-{s}"));
        assert_eq!(key("0x000000000000000000000000000df3"), tail("000000000df3"));
        assert_eq!(key("+0000000000000000000000000000df3"), tail("000000000df3"));
        assert_eq!(key("0000_000000000000000000000000df3"), tail("000000000df3"));
        assert_eq!(key("0x00000000000000000000000000d7b0"), tail("00000000d7b0"));
        assert_eq!(key("0x000000000000000000000000000000"), tail("000000000000"));
        assert_eq!(key("0X_00000000000000000000000000df3"), tail("000000000df3"));
        assert_eq!(key(" +0x_00000000000000000000000df3 "), tail("000000000df3"));
        assert_eq!(key("\u{a0}0000000000000000000000000000df3"), tail("000000000df3"));
        assert_eq!(key("\u{663}0000000000000000000000000000df3"), Some("30000000-0000-0000-0000-000000000df3".into()));
        // Near-misses Python refuses.
        for refused in [
            "0000__00000000000000000000000df3",
            "0000000000000000000000000000df3_",
            "_0000000000000000000000000000df3",
            "0x__000000000000000000000000df3",
            "+_000000000000000000000000000df3",
            "00x00000000000000000000000000df3",
            "++000000000000000000000000000df3",
            "+ 000000000000000000000000000df3",
            "\u{1c}0000000000000000000000000000df3",
            "\u{ff21}0000000000000000000000000000df3",
            "0x                              ",
            "0x",
            " 00000000000000000000000000df3003",
        ] {
            assert_eq!(key(refused), None, "{refused:?}");
        }
        // Record IDs keep the strict grammar.
        assert!(record_uuid("0x000000000000000000000000000df3").is_none());
        assert_eq!(record_uuid("{00000000-0000-0000-0000-000000DF3003}").map(|c| c.0), tail("000000df3003"));
    }

    // Every vector from tools/dfix3c_uuid_vectors.py (Python 3.13 `str(uuid.UUID(text))`, or null).
    #[test]
    fn uuid_text_matches_python_vectors() {
        let vectors: Vec<(String, Option<String>)> =
            serde_json::from_str(include_str!("../tests/data/py_uuid_vectors.json")).unwrap();
        assert!(vectors.len() > 6000);
        for (text, expected) in vectors {
            assert_eq!(py_uuid(&text).map(|c| c.0), expected, "{text:?}");
        }
    }
}
