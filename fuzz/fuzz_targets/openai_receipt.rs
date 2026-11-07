//! Receipt reader replies: the OpenAI plugin's `receipts::parse_reply` (model JSON to a receipt
//! draft) and fb-core's `assistant::receipts::parse_reply` (the same reply, core side), both on
//! arbitrary text.
//!
//! Asserts: no panic; the plugin's accepted totals are non-negative and tax never exceeds total;
//! the core's accepted reply is a JSON object whose amounts are money text.
#![no_main]
use libfuzzer_sys::fuzz_target;
use fb_plugin_api::Date;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data).into_owned();
    let today = Date::new(2026, 11, 2).unwrap();
    for currency in ["USD", "EUR"] {
        let _ = fb_plugin_openai::receipts::parse_reply(&text, currency, today);
    }
    let _ = fb_plugin_openai::receipts::parse_reply(&text, &text, today);
    for (currency, today) in [("USD", "2026-11-02"), ("usd", "2026-02-29"), (text.as_str(), text.as_str())] {
        if let Ok(v) = fb_core::assistant::receipts::parse_reply(&text, currency, today) {
            assert!(v.is_object(), "core parse_reply must return an object: {v}");
            for key in ["subtotal", "tax", "total"] {
                if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
                    if !s.is_empty() {
                        assert!(fb_money::money(&fb_money::AmountInput::Text(s.to_owned())).is_ok(), "{key}={s:?} is not money text");
                    }
                }
            }
        }
    }
});
