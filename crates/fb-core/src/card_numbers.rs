//! Payment card numbers in free text (a v2 rule; v1 only asks in help text).
//!
//! A card number is 13–19 digits that pass the Luhn check and don't start with 0, written either unbroken or
//! in groups joined by spaces or dashes, with no separator-run limit. Windows start and end at group
//! boundaries, so a number followed by an expiry (`4111 1111 1111 1111 12/27`) or preceded by other digits is still found, while a
//! single unbroken run of 20 or more digits is not a card number. ASCII and fullwidth digits count.
use crate::{FbError, FbResult};

/// The refusal text, shown beside the field (forms) or after the field's label (services).
pub const CARD_NUMBER: &str = "This looks like a payment card number. FreedomBooks doesn't keep card numbers: remove it, or keep only the last four digits (for example, \"card ending 1111\").";

fn digit(c: char) -> Option<u32> {
    match c {
        '0'..='9' => Some(c as u32 - '0' as u32),
        '\u{ff10}'..='\u{ff19}' => Some(c as u32 - 0xff10),
        _ => None,
    }
}

fn separator(c: char) -> bool {
    matches!(c, ' ' | '-' | '\u{a0}' | '\u{2007}' | '\u{202f}' | '\u{3000}' | '\u{2010}'..='\u{2015}' | '\u{2212}' | '\u{fe63}' | '\u{ff0d}')
}

fn luhn(digits: &[u32]) -> bool {
    let sum: u32 = digits.iter().rev().enumerate().map(|(i, &d)| if i % 2 == 1 { if d > 4 { 2 * d - 9 } else { 2 * d } } else { d }).sum();
    sum.is_multiple_of(10)
}

/// Char-index spans `[start, end)` of every card number in `text`, overlapping ones included.
fn spans(text: &[char]) -> Vec<(usize, usize)> {
    // Digit groups as (start, end) char indexes, each chain of groups ending at a non-separator.
    let mut chains: Vec<Vec<(usize, usize)>> = vec![];
    let mut i = 0;
    while i < text.len() {
        if digit(text[i]).is_none() {
            i += 1;
            continue;
        }
        let mut chain = vec![];
        loop {
            let start = i;
            while i < text.len() && digit(text[i]).is_some() {
                i += 1;
            }
            chain.push((start, i));
            let mut j = i;
            while j < text.len() && separator(text[j]) {
                j += 1;
            }
            if j > i && j < text.len() && digit(text[j]).is_some() {
                i = j;
            } else {
                break;
            }
        }
        chains.push(chain);
    }
    let mut found = vec![];
    for chain in chains {
        for a in 0..chain.len() {
            let mut digits = vec![];
            for &(s, e) in &chain[a..] {
                digits.extend(text[s..e].iter().filter_map(|&c| digit(c)));
                if digits.len() > 19 {
                    break;
                }
                if digits.len() >= 13 && digits[0] != 0 && luhn(&digits) {
                    found.push((chain[a].0, e));
                }
            }
        }
    }
    found
}

/// True when `text` holds a card number.
pub fn has_card_number(text: &str) -> bool {
    !spans(&text.chars().collect::<Vec<_>>()).is_empty()
}

/// Refuse a card number in free text; `label` names the field in the message.
pub fn refuse_card_number(text: &str, label: &str) -> FbResult<()> {
    if has_card_number(text) {
        return Err(FbError::validation(format!("{label}: {CARD_NUMBER}")));
    }
    Ok(())
}

/// `text` back when it holds no card number (for cleaned service fields).
pub fn card_free(text: String, label: &str) -> FbResult<String> {
    refuse_card_number(&text, label)?;
    Ok(text)
}

/// `text` with every digit of each card number but its last four replaced by `*` (separators kept). Text
/// stored before the refusal existed is shown and exported this way.
pub fn mask_card_numbers(text: &str) -> String {
    let mut chars: Vec<char> = text.chars().collect();
    let found = spans(&chars);
    if found.is_empty() {
        return text.to_owned();
    }
    let mut hide = vec![false; chars.len()];
    for (s, e) in found {
        let keep = (s..e).rev().filter(|&k| digit(chars[k]).is_some()).nth(3).unwrap_or(s);
        for k in s..keep {
            hide[k] |= digit(chars[k]).is_some();
        }
    }
    for (c, h) in chars.iter_mut().zip(hide) {
        if h {
            *c = '*';
        }
    }
    chars.into_iter().collect()
}

/// Free-text keys of audit details and snapshots; ids, digests and amounts are never masked.
const TEXT_KEYS: &[&str] = &["memo", "reference", "order_reference", "reason", "description", "notes", "check_number"];

/// `detail` with [`mask_card_numbers`] applied to every string under a free-text key, at any depth.
pub fn mask_text_fields(detail: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    fn walk(v: &Value, text: bool) -> Value {
        match v {
            Value::String(s) if text => Value::String(mask_card_numbers(s)),
            Value::Array(items) => Value::Array(items.iter().map(|i| walk(i, text)).collect()),
            Value::Object(map) => Value::Object(map.iter().map(|(k, i)| (k.clone(), walk(i, TEXT_KEYS.contains(&k.as_str())))).collect()),
            other => other.clone(),
        }
    }
    walk(detail, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_test_numbers_in_common_layouts() {
        for text in [
            "4111111111111111",
            "4111 1111 1111 1111",
            "Reference: 4111-1111-1111-1111",
            "card 4111 1111 1111 1111 exp 12/27",
            "4111111111111111 12/27",
            "visa4111111111111111",
            "3782 822463 10005",
            "6011 0009 9013 9424",
            "4222222222222",
            "4111 - 1111 - 1111 - 1111",
            "ref 12 4111 1111 1111 1111",
            "\u{ff14}\u{ff11}\u{ff11}\u{ff11}111111111111",
            "4111\u{a0}1111\u{a0}1111\u{a0}1111",
        ] {
            assert!(has_card_number(text), "{text}");
        }
    }

    #[test]
    fn separator_runs_have_no_three_character_exception() {
        for gap in ["    ", "        ", " - - ", "\u{a0}\u{a0}\u{a0}\u{a0}"] {
            let text = format!("4111{gap}1111 1111 1111");
            assert!(has_card_number(&text), "{text}");
            assert!(refuse_card_number(&text, "Memo").is_err(), "{text}");
            assert_eq!(mask_card_numbers(&text), format!("****{gap}**** **** 1111"));
        }
    }

    #[test]
    fn leaves_ordinary_references_alone() {
        for text in [
            "",
            "Check 1042",
            "card ending 1111",
            "4111 1111 1111 1112",
            "411111111111",
            "0000 0000 0000 0000",
            "04111111111111111",
            "41111111111111110000",
            "4111.1111.1111.1111",
            "4111    1111 1111 1112",
            "4111\n1111\n1111\n1111",
            "INV-000019, INV-000020",
        ] {
            assert!(!has_card_number(text), "{text}");
            assert_eq!(mask_card_numbers(text), text);
        }
    }

    #[test]
    fn masks_all_but_the_last_four_digits() {
        assert_eq!(mask_card_numbers("Reference: 4111 1111 1111 1111"), "Reference: **** **** **** 1111");
        assert_eq!(mask_card_numbers("4111111111111111 12/27"), "************1111 12/27");
        assert_eq!(mask_card_numbers("ref 12 4111-1111-1111-1111"), "ref 12 ****-****-****-1111");
        assert_eq!(mask_card_numbers("Check 1042"), "Check 1042");
        assert!(refuse_card_number("4111 1111 1111 1111", "Reference").unwrap_err().message().starts_with("Reference: This looks like"));
        assert!(refuse_card_number("card ending 1111", "Reference").is_ok());
    }
}
