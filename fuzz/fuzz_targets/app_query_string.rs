//! fb-app screen routing text: `screens::parse_query` (form-urlencoded query strings),
//! `screens::urlencode`, `screens::resolve` (the Django route table), and the key hex decoders
//! `keys::from_hex` / `keys::master_from_hex`.
//!
//! Asserts: no panic; `parse_query(urlencode(pairs)) == pairs` for whatever pairs came out of the
//! first parse; a resolved route's args are non-empty and ids are canonical UUID text.
#![no_main]
use libfuzzer_sys::fuzz_target;
use fb_app::screens::{parse_query, resolve, urlencode};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data).into_owned();
    let pairs = parse_query(&text);
    let encoded = urlencode(&pairs);
    let again = parse_query(&encoded);
    assert_eq!(again, pairs, "urlencode/parse_query round trip for {text:?} ({encoded:?})");
    if let Some(route) = resolve(&text) {
        assert!(!route.name.is_empty());
        for (k, v) in &route.args {
            assert!(!v.is_empty(), "empty route arg {k}");
            if k.ends_with("_id") {
                let u = fb_ids::parse_uuid(v).expect("id arg parses");
                assert_eq!(&u.to_string(), v, "id arg must be canonical");
            }
        }
    }
    let _ = fb_app::keys::from_hex(&text);
    let _ = fb_app::keys::master_from_hex(&text);
    let _ = fb_app::keys::check_password(&text);
    let _ = fb_templates::django::reverse(&text, &[text.clone()]);
    let _ = fb_templates::django::default_datetime(&text);
});
