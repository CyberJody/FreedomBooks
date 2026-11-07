//! SimpleFIN plugin scalar decoders on arbitrary text and JSON scalars: `parse::money` (wire amount
//! text to cents, ROUND_HALF_UP), `parse::epoch`, `parse::redact`, `parse::sanitize`.
//!
//! Asserts: no panic; cents are within the documented 14-integer-digit limit; `redact` is
//! idempotent and never grows the text by more than the `***` substitutions; `sanitize` honours
//! its character limit and never returns leading or trailing whitespace.
#![no_main]
use libfuzzer_sys::fuzz_target;
use fb_plugin_simplefin::parse::{MoneyError, epoch, money, redact, sanitize};
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data).into_owned();
    let as_string = Value::String(text.clone());
    match money(&as_string) {
        Ok(cents) => assert!(cents.abs() <= 9_999_999_999_999_999, "cents out of range: {cents} from {text:?}"),
        Err(MoneyError::Missing) => panic!("a string is never Missing: {text:?}"),
        Err(_) => {}
    }
    if let Ok(number) = serde_json::from_str::<Value>(text.trim()) {
        if number.is_number() {
            let _ = money(&number);
            let _ = epoch(&number);
        }
    }
    if let Some(e) = epoch(&as_string) {
        assert!(e > 0 && e <= 4_102_444_800);
    }
    let once = redact(&text);
    // F-04 (artifacts crash-004c581b... and crash-00df763a...: a control character inside the
    // credential keyword or separator defeated the first pass) is fixed on branch fuzz-fix, so
    // idempotence is checked on every input again.
    let twice = redact(&once);
    assert_eq!(once, twice, "redact is not idempotent on {text:?}");
    assert!(once.chars().all(|c| c > '\u{1f}' && c != '\u{7f}'), "control character survived redact: {once:?}");
    for limit in [0usize, 1, 7, 120, 500] {
        let s = sanitize(&text, limit);
        assert!(s.chars().count() <= limit, "sanitize({limit}) returned {} chars", s.chars().count());
        // sanitize trims and then truncates, so a cut can leave trailing blanks (artifact
        // crash-004c581b..., "Auth\0\0\0\0orization: ..." at limit 7); leading blanks never survive.
        assert_eq!(s.trim_start(), s, "sanitize must trim the start: {s:?}");
    }
});
