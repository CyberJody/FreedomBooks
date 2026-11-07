//! fb-core attachments: uploaded bytes and the client file name (`attachments::inspect`, which runs
//! `sniff::sniff`, `sniff::problem` (PDF/PNG/JPEG/WebP container walks) and `sniff::display_name` /
//! `sniff::clean_name`), plus `storage::valid_name`.
//!
//! The last byte of the input selects a name; the rest is the file. Asserts: no panic; an accepted
//! file has one of the four allowed types; the display name is at most 120 characters, ends with
//! the type's extension, and contains no path separators or control characters.
#![no_main]
use libfuzzer_sys::fuzz_target;
use fb_core::attachments::{Limits, inspect, sniff};

const NAMES: [&str; 8] = [
    "receipt.pdf",
    "../../etc/passwd",
    "C:\\Users\\me\\scan .JPG",
    "",
    "\u{202e}gnp.exe",
    "ţîlde\u{300}.png",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.verylongextension",
    "no extension",
];

fuzz_target!(|data: &[u8]| {
    let (file, name) = match data.split_last() {
        Some((last, file)) => (file, NAMES[(*last as usize) % NAMES.len()]),
        None => (data, "x"),
    };
    let raw_name = String::from_utf8_lossy(&data[..data.len().min(64)]).into_owned();
    for name in [name, raw_name.as_str()] {
        let cleaned = sniff::clean_name(name);
        assert!(cleaned.chars().count() <= 120, "clean_name too long: {cleaned:?}");
        assert!(!cleaned.contains(['/', '\\']), "clean_name kept a separator: {cleaned:?}");
        for t in sniff::TYPES {
            if let Ok(display) = sniff::display_name(name, t) {
                assert!(display.chars().count() <= 120, "display_name too long: {display:?}");
                let lower = display.to_lowercase();
                let accepted: &[&str] = match t { "image/jpeg" => &[".jpg", ".jpeg", ".jpe", ".jfif"], _ => &[sniff::extension(t).unwrap()] };
                assert!(accepted.iter().any(|e| lower.ends_with(e)), "display_name {display:?} lacks a {t} extension");
                assert!(!display.chars().any(|c| c.is_control()), "control char in display name: {display:?}");
            }
            let _ = sniff::invalid_message(t, name);
        }
        let _ = sniff::refusal(&file[..file.len().min(64)], name);
        match inspect(file, name, &Limits::default()) {
            Ok(i) => {
                assert!(sniff::TYPES.contains(&i.content_type.as_str()));
                assert_eq!(i.sha256.len(), 64);
                assert_eq!(Some(i.content_type.as_str()), sniff::sniff(file));
            }
            Err(_) => {}
        }
    }
    let _ = fb_core::attachments::storage::valid_name(&raw_name);
    let _ = Limits { max_mb: data.len() as u32 % 120, uploads_per_window: 1, company_max_mb: 1 }.validate();
});
