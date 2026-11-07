//! fb-db text validators and SQL-side decoders on arbitrary text and JSON: `validators::*`,
//! `checks_snapshots::{money_text, valid}`, `banking_evidence::payload`, `credits::fields`,
//! `prepayments::prepayment_snapshot_ok`, `assistant::{choices, selection, result}`,
//! `bankfeeds_text::{text, identity_form, identity_key}`, `collation::{compare, lower, upper}` and
//! fb-core's CSV export cell guard `reports::params::csv_text`.
//!
//! Asserts: no panic; collation is a total order (antisymmetric, reflexive); `csv_text` never
//! returns a cell a spreadsheet would run as a formula.
#![no_main]
use libfuzzer_sys::fuzz_target;
use std::cmp::Ordering;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data).into_owned();
    let value: serde_json::Value = serde_json::from_slice(data).unwrap_or(serde_json::Value::String(text.clone()));
    let _ = fb_db::validators::text(&text);
    let _ = fb_db::checks_snapshots::money_text(&text);
    for kind in ["check", "deposit", "bill", "invoice", "receipt", &text] {
        let _ = fb_db::checks_snapshots::valid(kind, &value, Some(&text));
        let _ = fb_db::checks_snapshots::valid(kind, &value, None);
        let _ = fb_db::banking_evidence::payload(kind, &value);
        let _ = fb_db::prepayments::prepayment_snapshot_ok(kind, &value);
        let _ = fb_db::assistant::selection(kind, &value);
    }
    let _ = fb_db::assistant::choices(&value);
    let _ = fb_db::assistant::result(&value);
    let _ = fb_db::bankfeeds_text::text(&text, 1, 200);
    let _ = fb_db::bankfeeds_text::text(&text, 0, 0);
    let _ = fb_db::bankfeeds_text::identity_form(&text);
    let _ = fb_db::bankfeeds_text::identity_key(&data[..data.len().min(64)], "domain", &text);
    let mid = (0..=text.len() / 2).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
    let (a, b) = text.split_at(mid);
    let ab = fb_db::collation::compare(a, b);
    let ba = fb_db::collation::compare(b, a);
    assert_eq!(ab, ba.reverse(), "collation not antisymmetric for {a:?} / {b:?}");
    assert_eq!(fb_db::collation::compare(a, a), Ordering::Equal);
    let _ = fb_db::collation::punctuation_tie(a, b);
    let _ = fb_db::collation::lower(&text);
    let _ = fb_db::collation::upper(&text);
    let cell = fb_core::reports::params::csv_text(&text);
    assert!(!cell.starts_with(['=', '+', '-', '@', '\t', '\r']), "csv_text left a formula lead: {cell:?}");
    let _ = fb_core::reports::params::page_number(Some(&text), 7, 3);
});
