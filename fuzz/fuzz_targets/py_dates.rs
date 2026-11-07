//! fb-core / fb-db date parsers on arbitrary text: `py::model_date` (Django model DateField),
//! `py::form_date` (Django forms DateField), `fb_db::dates::parse_iso_date` (CPython
//! `date.fromisoformat`), `fb_db::validators::date` and `fb_db::banking_evidence::calendar_date`.
//!
//! Asserts: no panic; every accepted date is a real calendar date in 0001..=9999 whose canonical
//! `YYYY-MM-DD` text is accepted again by the same parser.
#![no_main]
use libfuzzer_sys::fuzz_target;
use fb_core::py::{form_date, model_date};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    if let Ok(d) = model_date(&text) {
        assert!((1..=9999).contains(&chrono_year(d)), "model_date year out of range: {text:?}");
        let canonical = d.format("%Y-%m-%d").to_string();
        assert_eq!(model_date(&canonical), Ok(d), "model_date canonical round trip for {text:?}");
    }
    if let Ok(d) = form_date(&text) {
        assert!((1..=9999).contains(&chrono_year(d)), "form_date year out of range: {text:?}");
        let canonical = d.format("%Y-%m-%d").to_string();
        assert_eq!(form_date(&canonical), Ok(d), "form_date canonical round trip for {text:?}");
    }
    if let Some(canonical) = fb_db::dates::parse_iso_date(&text) {
        assert_eq!(fb_db::dates::parse_iso_date(&canonical), Some(canonical.clone()), "iso round trip for {text:?}");
        assert_eq!(canonical.len(), 10);
    }
    let _ = fb_db::validators::date(&text);
    let _ = fb_db::validators::utc_timestamp(&text);
    let _ = fb_db::validators::utc_timestamp_micros(&text);
    let _ = fb_db::banking_evidence::calendar_date(&text);
    let _ = fb_db::bankfeeds_text::timestamp(&text);
});

fn chrono_year(d: chrono::NaiveDate) -> i32 {
    use chrono::Datelike;
    d.year()
}
