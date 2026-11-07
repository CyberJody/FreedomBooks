//! Shared check/refund number syntax for services, SQL guards, and integrity scans.
//! Pure functions only: these validators never capture a connection or write state.
use rusqlite::{Connection, functions::FunctionFlags};
include!("credits_number_categories.rs");

/// Shared Python Unicode category rule for each check-number character.
pub fn char_allowed(ch: char) -> bool {
    let cp=ch as u32;
    CHECK_NUMBER_FORBIDDEN.binary_search_by(|(low,high)| {
        if cp<*low {std::cmp::Ordering::Greater} else if cp>*high {std::cmp::Ordering::Less} else {std::cmp::Ordering::Equal}
    }).is_err()
}

pub fn normalize(value: &str) -> Result<&str, &'static str> {
    let value = value.trim_matches([' ', '\t', '\n', '\r', '\u{c}', '\u{b}']);
    if value.chars().count() > 20 {
        return Err("Enter a check number up to 20 characters.");
    }
    if !value.chars().all(char_allowed) {
        return Err("A check number cannot contain control or formatting characters.");
    }
    Ok(value)
}

/// Stored numbers must already be normalized. Empty is valid for print-later.
pub fn valid(value: &str) -> bool {
    normalize(value) == Ok(value)
}

pub(crate) fn register(conn: &Connection) -> rusqlite::Result<()> {
    conn.create_scalar_function(
        "fb_check_number_valid",
        1,
        FunctionFlags::SQLITE_UTF8
            | FunctionFlags::SQLITE_DETERMINISTIC
            | FunctionFlags::SQLITE_INNOCUOUS,
        |ctx| Ok(ctx.get::<String>(0).is_ok_and(|s| valid(&s))),
    )
}

include!("free_text_categories.rs");
/// Free text permits TAB, but no Unicode Cc/Cf (including bidi/zero-width controls).
/// Shares the check-number character gate; unlike numbers, permits other categories.
pub fn free_text_valid(text: &str) -> bool {
    text.chars().all(free_text_char_allowed)
}

/// Same frozen Cc/Cf set for refusal of typed text and cleaning of provider text (TAB allowed).
pub fn free_text_char_allowed(c: char) -> bool {
    char_allowed(c) || !FREE_TEXT_FORBIDDEN.iter().any(|&(lo, hi)| (lo..=hi).contains(&(c as u32)))
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("credits_number_categories.rs");
    #[test]
    fn shared_character_rule_matches_the_frozen_python_unicode_categories() {
        let mut ranges=CHECK_NUMBER_FORBIDDEN.iter().peekable();
        for cp in 0..=0x10ffff {
            let Some(ch)=char::from_u32(cp) else {continue;};
            while ranges.peek().is_some_and(|&&(_,high)| cp>high) {ranges.next();}
            let forbidden=ranges.peek().is_some_and(|&&(low,high)| (low..=high).contains(&cp));
            assert_eq!(char_allowed(ch),!forbidden,"U+{cp:04X}");
        }
    }
}
