//! Fuzz audit 2026-11, area: dates (fb-core `py::{model_date, form_date}`, fb-db `dates::parse_iso_date`).
//!
//! Properties: every real calendar date round-trips through each parser in each accepted spelling;
//! every impossible date (month 0/13, day 0/32, Feb 30, year 0 or 10000) is refused; the three
//! parsers agree on `YYYY-MM-DD`; and `parse_iso_date` agrees with CPython `date.fromisoformat`
//! (skipped, with a message, when `python3` is not on PATH).
use chrono::{Datelike, NaiveDate};
use fb_core::py::{form_date, model_date};
use fb_db::dates::parse_iso_date;
use proptest::prelude::*;
use std::io::Write;
use std::process::{Command, Stdio};

fn date() -> impl Strategy<Value = NaiveDate> {
    (1i32..=9999, 1u32..=12, 1u32..=31).prop_filter_map("real date", |(y, m, d)| NaiveDate::from_ymd_opt(y, m, d))
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, .. ProptestConfig::default() })]

    #[test]
    fn every_real_date_round_trips(d in date()) {
        let iso = d.format("%Y-%m-%d").to_string();
        let (y, m, day) = (d.year(), d.month(), d.day());
        prop_assert_eq!(model_date(&iso), Ok(d));
        prop_assert_eq!(form_date(&iso), Ok(d));
        prop_assert_eq!(parse_iso_date(&iso), Some(iso.clone()));
        prop_assert_eq!(parse_iso_date(&d.format("%Y%m%d").to_string()), Some(iso.clone()), "basic format");
        prop_assert_eq!(model_date(&d.format("%Y%m%d").to_string()), Ok(d), "model_date accepts the basic format via fromisoformat");
        // Django's regex fallback: one-digit month and day, and a trailing newline.
        prop_assert_eq!(model_date(&format!("{y:04}-{m}-{day}")), Ok(d));
        prop_assert_eq!(model_date(&format!("{iso}\n")), Ok(d));
        // Django forms: %m/%d/%Y (strptime %Y needs four digits), %m/%d/%y (69 -> 1969, 68 -> 2068), month names.
        prop_assert_eq!(form_date(&format!("{m}/{day}/{y:04}")), Ok(d));
        prop_assert_eq!(form_date(&format!("{m:02}/{day:02}/{y:04}")), Ok(d));
        prop_assert_eq!(form_date(&format!("{} {day}, {y:04}", MONTHS[d.month0() as usize])), Ok(d));
        prop_assert_eq!(form_date(&format!("{} {day} {y:04}", &MONTHS[d.month0() as usize][..3])), Ok(d));
        prop_assert_eq!(form_date(&format!("{day} {} {y:04}", MONTHS[d.month0() as usize])), Ok(d));
        prop_assert_eq!(form_date(&format!("  {iso}\t")), Ok(d), "forms strip whitespace");
        if (1969..=2068).contains(&y) {
            prop_assert_eq!(form_date(&format!("{m}/{day}/{:02}", y % 100)), Ok(d), "two-digit year pivot");
        }
        // ISO week date of the same day.
        let iso_week = d.iso_week();
        if (1..=9999).contains(&iso_week.year()) {
            let week = format!("{:04}-W{:02}-{}", iso_week.year(), iso_week.week(), d.weekday().number_from_monday());
            prop_assert_eq!(parse_iso_date(&week), Some(iso.clone()), "week date {}", week);
        }
    }

    #[test]
    fn impossible_dates_are_refused(y in 0i32..=10000, m in 0u32..=13, d in 0u32..=32) {
        let real = (1..=9999).contains(&y) && NaiveDate::from_ymd_opt(y, m, d).is_some();
        let iso = format!("{y:04}-{m:02}-{d:02}");
        prop_assert_eq!(model_date(&iso).is_ok(), real, "model_date {}", iso);
        prop_assert_eq!(form_date(&iso).is_ok(), real, "form_date {}", iso);
        prop_assert_eq!(parse_iso_date(&iso).is_some(), real, "parse_iso_date {}", iso);
        let us = format!("{m:02}/{d:02}/{y:04}");
        prop_assert_eq!(form_date(&us).is_ok(), real, "form_date {}", us);
    }

    #[test]
    fn the_three_parsers_agree_on_iso_text(text in "[0-9]{0,5}-?[0-9]{0,3}-?[0-9]{0,3}") {
        let a = model_date(&text).ok();
        let b = form_date(&text).ok();
        let c = parse_iso_date(&text).map(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").unwrap());
        if let Some(c) = c {
            prop_assert_eq!(a, Some(c), "model_date vs fromisoformat on {:?}", text);
        }
        if let (Some(a), Some(b)) = (a, b) {
            prop_assert_eq!(a, b, "model_date vs form_date on {:?}", text);
        }
    }

    #[test]
    fn unicode_digits_are_read_where_python_reads_them(d in date()) {
        // Django's regex fallback uses \d (Unicode); forms' strptime uses \d for years/days.
        let arabic = |n: u32, width: usize| -> String { format!("{n:0width$}").chars().map(|c| char::from_u32(0x660 + c.to_digit(10).unwrap()).unwrap()).collect() };
        let text = format!("{}-{}-{}", arabic(d.year() as u32, 4), arabic(d.month(), 2), arabic(d.day(), 2));
        prop_assert_eq!(model_date(&text), Ok(d), "model_date Unicode digits {}", text);
        prop_assert_eq!(parse_iso_date(&text), None, "fromisoformat is ASCII-only");
        let year_only = format!("{:02}/{:02}/{}", d.month(), d.day(), arabic(d.year() as u32, 4));
        prop_assert_eq!(form_date(&year_only), Ok(d), "form_date Unicode year {}", year_only);
    }
}

const PY: &str = r#"
import sys, json, datetime
def iso(s):
    try:
        d = datetime.date.fromisoformat(s)
        return d.isoformat()
    except Exception:
        return None
print(json.dumps([iso(s) for s in json.load(sys.stdin)]))
"#;

fn python(inputs: &[String]) -> Option<Vec<Option<String>>> {
    let mut child = Command::new("python3").args(["-I", "-c", PY]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().ok()?;
    child.stdin.take().unwrap().write_all(serde_json::to_string(inputs).unwrap().as_bytes()).ok()?;
    let out = child.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

fn iso_ish() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => "[0-9]{4}-?W?[0-9]{1,2}-?[0-9]{0,2}",
        2 => "[0-9]{6,10}",
        1 => "[0-9 +\\-W:T]{0,12}",
        1 => date().prop_map(|d| d.format("%Y-%m-%d").to_string()),
        1 => date().prop_map(|d| format!("{:04}-W{:02}-{}", d.iso_week().year(), d.iso_week().week(), d.weekday().number_from_monday())),
        1 => date().prop_map(|d| format!("{:04}W{:02}{}", d.iso_week().year(), d.iso_week().week(), d.weekday().number_from_monday())),
    ]
}

/// FINDING F-08 (cosmetic, wrong reject): CPython 3.13 `date.fromisoformat` accepts a ten-character
/// string whose first eight characters are a basic-format date and ignores the last two
/// ("2834121872" -> 2834-12-18, "20261102xx" -> 2026-11-02). `parse_iso_date` refuses them. The port
/// is stricter than the reference, which is the better behaviour, but it is a divergence from the
/// frozen app; the broad differential test below leaves those strings out.
fn cpython_trailing_quirk(s: &str) -> bool {
    s.len() == 10 && s.as_bytes()[..8].iter().all(u8::is_ascii_digit) && s.as_bytes()[4] != b'-'
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 10, .. ProptestConfig::default() })]

    #[test]
    fn parse_iso_date_agrees_with_cpython(strings in prop::collection::vec(iso_ish().prop_filter("CPython trailing-character quirk, see F-08", |s| !cpython_trailing_quirk(s)), 500)) {
        let Some(expected) = python(&strings) else {
            eprintln!("python3 not available; skipping the CPython differential check");
            return Ok(());
        };
        for (s, e) in strings.iter().zip(expected) {
            prop_assert_eq!(parse_iso_date(s), e, "date.fromisoformat({:?})", s);
        }
    }
}

/// FINDING F-08, pinned: fails on the port today (the reference accepts, the port refuses).
#[test]
#[ignore = "fuzz audit finding F-08: CPython date.fromisoformat accepts 'YYYYMMDDxx', parse_iso_date refuses it"]
fn parse_iso_date_accepts_cpythons_ten_character_basic_quirk() {
    assert_eq!(parse_iso_date("2834121872"), Some("2834-12-18".into()));
    assert_eq!(parse_iso_date("20261102xx"), Some("2026-11-02".into()));
}
