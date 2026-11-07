//! The frozen `fb_reports` display tags. Money is exact decimal text, never a float.
use chrono::{Datelike, NaiveDate};
use minijinja::{Environment, Error, ErrorKind, Value};

fn date(v: &Value) -> Result<NaiveDate, Error> {
    NaiveDate::parse_from_str(v.as_str().unwrap_or(""), "%Y-%m-%d")
        .map_err(|e| Error::new(ErrorKind::InvalidOperation, e.to_string()))
}
fn long(d: NaiveDate) -> String {
    format!("{} {}, {}", d.format("%B"), d.day(), d.year())
}
fn money(v: Value) -> String {
    if v.is_none() || v.is_undefined() || v.as_str() == Some("") {
        return String::new();
    }
    let text = v.to_string();
    let (whole, fraction) = text.split_once('.').unwrap_or((&text, "00"));
    let digits = whole.trim_start_matches('-');
    let negative =
        whole.starts_with('-') && digits.chars().chain(fraction.chars()).any(|c| c != '0');
    let mut out = if negative { "-".into() } else { String::new() };
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out.push('.');
    out.push_str(fraction);
    out
}
pub fn register(env: &mut Environment<'_>) {
    env.add_filter("money", money);
    env.add_filter("neg", |v: Value| {
        v.as_str()
            .is_some_and(|s| s.starts_with('-') && s != "-0.00")
    });
    env.add_filter("report_nonzero", |v: Value| {
        v.as_str()
            .is_some_and(|s| !matches!(s, "" | "0.00" | "-0.00"))
    });
    env.add_function("long_date", |v: Value| date(&v).map(long));
    env.add_function(
        "date_span",
        |start: Value, end: Value| -> Result<String, Error> {
            let e = date(&end)?;
            if start.is_none() || start.is_undefined() {
                return Ok(format!("Through {}", long(e)));
            }
            let s = date(&start)?;
            Ok(if s == e {
                long(e)
            } else if s.year() == e.year() && s.month() == e.month() {
                format!("{} {} – {}, {}", s.format("%B"), s.day(), e.day(), e.year())
            } else if s.year() == e.year() {
                format!(
                    "{} {} – {} {}, {}",
                    s.format("%B"),
                    s.day(),
                    e.format("%B"),
                    e.day(),
                    e.year()
                )
            } else {
                format!("{} – {}", long(s), long(e))
            })
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn money_preserves_cents_and_sign_without_floats() {
        for (raw, expected) in [
            ("12345678901234567890.01", "12,345,678,901,234,567,890.01"),
            ("-1234.50", "-1,234.50"),
            ("-0.00", "0.00"),
        ] {
            assert_eq!(money(Value::from(raw)), expected);
        }
    }
}
