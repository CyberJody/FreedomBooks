//! Python date.fromisoformat shared by document services. Django form/model
//! fallback grammars stay with their callers. No trimming or coercion.
/// CPython date.fromisoformat: calendar and ISO week dates, basic/extended.
/// Check the grammar before numeric parsing; signs are never date digits.
pub fn parse_iso_date(s: &str) -> Option<String> {
    use chrono::{Datelike, NaiveDate, Weekday};
    if !s.is_ascii() {
        return None;
    }
    let b = s.as_bytes();
    let digits = |s: &str| -> Option<u32> {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        s.parse().ok()
    };
    let calendar = |y: &str, m: &str, d: &str| {
        NaiveDate::from_ymd_opt(digits(y)? as i32, digits(m)?, digits(d)?)
    };
    let parsed = match b.len() {
        10 if b[4] == b'-' && b[7] == b'-' => calendar(&s[..4], &s[5..7], &s[8..]),
        8 if b.iter().all(u8::is_ascii_digit) => calendar(&s[..4], &s[4..6], &s[6..]),
        _ => {
            let (week, day) = match b.len() {
                7 if b[4] == b'W' => (&s[5..7], "1"),
                8 if b[4] == b'W' => (&s[5..7], &s[7..8]),
                8 if b[4] == b'-' && b[5] == b'W' => (&s[6..8], "1"),
                10 if b[4] == b'-' && b[5] == b'W' && b[8] == b'-' => (&s[6..8], &s[9..10]),
                _ => return None,
            };
            let weekday = match digits(day)? {
                1 => Weekday::Mon,
                2 => Weekday::Tue,
                3 => Weekday::Wed,
                4 => Weekday::Thu,
                5 => Weekday::Fri,
                6 => Weekday::Sat,
                7 => Weekday::Sun,
                _ => return None,
            };
            let year = digits(&s[..4])?;
            if year == 0 {
                return None;
            }
            NaiveDate::from_isoywd_opt(year as i32, digits(week)?, weekday)
        }
    }?;
    (1..=9999)
        .contains(&parsed.year())
        .then(|| parsed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn python_differential_vectors() {
        for raw in [
            include_str!("dates_vectors.json"),
            include_str!("dates_extra_vectors.json"),
            include_str!("dates_credit_vectors.json"),
        ] {
            let vectors: Vec<(serde_json::Value, Option<String>)> = serde_json::from_str(raw).unwrap();
            for (input, expected) in vectors {
                assert_eq!(input.as_str().and_then(parse_iso_date), expected, "{input:?}");
            }
        }
    }
}
