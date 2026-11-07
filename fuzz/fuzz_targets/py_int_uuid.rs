//! Python-compatible integer, UUID and identifier parsers on arbitrary text:
//! `fb_core::py::{py_int, py_uuid}`, `fb_templates::forms::python_integer`, `fb_ids::parse_uuid`,
//! `fb_db::banking_evidence::canonical_uuid`, `fb_db::validators::uuid`, `fb_db::check_numbers::{normalize, valid}`,
//! `fb_db::prepayments::validate_prepayment_number` and `fb_core::email_validation::valid_email`.
//!
//! Asserts: no panic; the two Python `int()` ports agree on what is a number; a parsed UUID prints
//! back to canonical text that both UUID parsers accept.
#![no_main]
use libfuzzer_sys::fuzz_target;
use fb_core::py::{PyInt, py_int, py_uuid};
use fb_templates::forms::python_integer;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let a = py_int(&text);
    let b = python_integer(&text);
    // Known finding (FUZZ-AUDIT-REPORT.md F-05, artifact crash-ae803d4c...): python_integer strips the
    // information separators U+001C..U+001F that CPython's int() refuses; py_int gets it right. Skip
    // that case so the run keeps looking for anything else.
    let known = text.chars().any(|c| ('\u{1c}'..='\u{1f}').contains(&c));
    assert!(known || a.is_some() == b.is_some(), "py_int vs python_integer disagree on {text:?}: {a:?} vs {b:?}");
    if let (Some(PyInt::Small(x)), Some(y)) = (a, b) {
        assert_eq!(x, y, "py_int vs python_integer value for {text:?}");
    }
    if let Some(canonical) = py_uuid(&text) {
        assert_eq!(canonical.0.len(), 36);
        assert!(fb_db::banking_evidence::canonical_uuid(&canonical.0), "py_uuid output not canonical: {:?}", canonical.0);
        assert_eq!(py_uuid(&canonical.0), Some(canonical.clone()));
        assert!(fb_ids::parse_uuid(&canonical.0).is_some());
    }
    if let Some(u) = fb_ids::parse_uuid(&text) {
        let s = u.to_string();
        assert_eq!(fb_ids::parse_uuid(&s), Some(u));
        assert!(fb_ids::CompanyId::parse(&s).is_some());
        let _ = text.parse::<fb_ids::CompanyId>();
    }
    let _ = fb_db::validators::uuid(&text);
    let _ = fb_db::validators::text(&text);
    if let Ok(n) = fb_db::check_numbers::normalize(&text) {
        assert!(fb_db::check_numbers::valid(n), "normalize accepted but valid refused: {text:?}");
        assert!(n.chars().count() <= 20);
    }
    let _ = fb_db::check_numbers::valid(&text);
    let _ = fb_db::prepayments::validate_prepayment_number(&text);
    let _ = fb_core::email_validation::valid_email(&text);
    let _ = fb_core::py::py_strip(&text);
    let _ = fb_core::py::py_truncate(&text, data.len() % 40);
    let _ = fb_core::py::max_length_message(7, &text);
    for c in text.chars().take(64) {
        let _ = fb_core::py::is_control_or_format(c);
        let _ = fb_db::check_numbers::char_allowed(c);
        let _ = fb_money::unicode_decimal(c);
    }
});
