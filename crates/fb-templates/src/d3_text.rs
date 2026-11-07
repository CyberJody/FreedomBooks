//! Django Truncator.chars: frozen Python NFC and non-combining character counting.
#[path = "d3_text_tables.rs"]
mod tables;
fn ccc(cp: u32) -> u8 {
    tables::CCC
        .binary_search_by_key(&cp, |row| row.0)
        .ok()
        .map(|i| tables::CCC[i].1)
        .unwrap_or(0)
}
fn decompose(cp: u32, out: &mut Vec<u32>) {
    // Unicode Hangul decomposition is algorithmic, not listed in unicodedata.decomposition.
    if (0xac00..0xd7a4).contains(&cp) {
        let n = cp - 0xac00;
        out.push(0x1100 + n / 588);
        out.push(0x1161 + (n % 588) / 28);
        if !n.is_multiple_of(28) {
            out.push(0x11a7 + n % 28);
        }
        return;
    }
    if let Ok(i) = tables::DECOMP.binary_search_by_key(&cp, |row| row.0) {
        for part in tables::DECOMP[i].1 {
            decompose(*part, out);
        }
    } else {
        out.push(cp);
    }
}
fn compose(a: u32, b: u32) -> Option<u32> {
    if (0x1100..0x1113).contains(&a) && (0x1161..0x1176).contains(&b) {
        return Some(0xac00 + (a - 0x1100) * 588 + (b - 0x1161) * 28);
    }
    if (0xac00..0xd7a4).contains(&a)
        && (a - 0xac00).is_multiple_of(28)
        && (0x11a8..0x11c3).contains(&b)
    {
        return Some(a + b - 0x11a7);
    }
    tables::COMPOSE
        .binary_search_by_key(&(a, b), |r| (r.0, r.1))
        .ok()
        .map(|i| tables::COMPOSE[i].2)
}
fn nfc(text: &str) -> String {
    let mut parts = vec![];
    for c in text.chars() {
        decompose(c as u32, &mut parts);
    }
    // Stable canonical ordering within each starter's combining sequence.
    let mut start = 0;
    for i in 0..parts.len() {
        if ccc(parts[i]) == 0 {
            parts[start..i].sort_by_key(|cp| ccc(*cp));
            start = i + 1;
        }
    }
    parts[start..].sort_by_key(|cp| ccc(*cp));
    let mut out = Vec::<u32>::new();
    let mut starter = None;
    let mut last = 0;
    for cp in parts {
        let class = ccc(cp);
        let joined = starter.and_then(|i: usize| {
            if last < class || last == 0 {
                compose(out[i], cp).map(|combined| (i, combined))
            } else {
                None
            }
        });
        if let Some((i, combined)) = joined {
            out[i] = combined;
        } else {
            if class == 0 {
                starter = Some(out.len());
            }
            out.push(cp);
            last = class;
        }
    }
    out.into_iter().filter_map(char::from_u32).collect()
}
pub(super) fn truncate(text: &str, n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    let text = nfc(text);
    let mut count = 0;
    let mut end = None;
    for (at, c) in text.char_indices() {
        if ccc(c as u32) != 0 {
            continue;
        }
        count += 1;
        if end.is_none() && count > n.saturating_sub(1) {
            end = Some(at);
        }
        if count > n {
            let prefix = &text[..end.unwrap_or(0)];
            return if prefix.ends_with('…') {
                prefix.into()
            } else {
                format!("{prefix}…")
            };
        }
    }
    text
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matches_frozen_django_unicode_vectors() {
        let vectors: Vec<(String, usize, String)> =
            serde_json::from_str(include_str!("d3_text_vectors.json")).unwrap();
        for (text, n, expected) in vectors {
            assert_eq!(truncate(&text, n), expected, "text={text:?}, limit={n}");
        }
    }
}
