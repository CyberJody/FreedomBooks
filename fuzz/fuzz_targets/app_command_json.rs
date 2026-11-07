//! fb-app: the bridge's command and query JSON (`commands::parse_command`, `commands::parse_query`,
//! both over `fb_db::parse_unique_json_as`), `fb_db::parse_unique_json`, and the app's wrapped-key
//! blob (`keys::WrappedKey` JSON then `keys::unwrap` with a fixed password at minimum cost).
//!
//! Asserts: no panic; an accepted command re-serializes to JSON that parses to the same command;
//! unique-key JSON parsing agrees with serde_json on what is valid JSON.
#![no_main]
use libfuzzer_sys::fuzz_target;
use fb_app::commands::{parse_command, parse_query};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data).into_owned();
    if let Ok(command) = parse_command(&text) {
        let again = serde_json::to_string(&command).expect("command serializes");
        let back = parse_command(&again).expect("re-serialized command parses");
        assert_eq!(serde_json::to_string(&back).unwrap(), again, "command round trip for {text:?}");
    }
    if let Ok(query) = parse_query(&text) {
        let again = serde_json::to_string(&query).expect("query serializes");
        assert!(parse_query(&again).is_ok(), "re-serialized query parses: {again}");
    }
    let unique = fb_db::parse_unique_json(&text);
    let plain = serde_json::from_str::<serde_json::Value>(&text);
    if unique.is_ok() {
        assert!(plain.is_ok(), "unique parse accepted what serde_json refuses: {text:?}");
    }
    if let Ok(blob) = fb_db::parse_unique_json_as::<fb_app::keys::WrappedKey>(&text) {
        let _ = fb_app::keys::unwrap(&blob, "correct horse battery staple");
    }
});
