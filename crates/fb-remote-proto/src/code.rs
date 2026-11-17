//! One-time pairing codes: 80 random bits as 16 Crockford base32 characters, shown as
//! `XXXX-XXXX-XXXX-XXXX`. Entry ignores case, spaces and dashes and folds O→0, I/L→1 (the same
//! folding as recovery keys). The code is the key for the pairing proofs (`fb-remote` pairing), so
//! its length is what keeps an offline guess out of reach within its ten-minute life.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
pub const LEN: usize = 16;
/// How long a code admits a device.
pub const LIFETIME_SECONDS: u64 = 10 * 60;
/// Wrong proofs before a code is destroyed.
pub const MAX_ATTEMPTS: u32 = 5;

/// A fresh code in its canonical form (no dashes).
pub fn generate() -> Result<String, getrandom::Error> {
    let bytes: [u8; 10] = crate::random()?;
    let mut bits: u128 = 0;
    for b in bytes {
        bits = bits << 8 | u128::from(b);
    }
    Ok((0..LEN).rev().map(|i| ALPHABET[((bits >> (i * 5)) & 31) as usize] as char).collect())
}

/// `ABCD-EFGH-JKMN-PQRS` for display.
pub fn display(code: &str) -> String {
    code.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join("-")
}

/// The canonical code from what someone typed, or None when it cannot be a code.
pub fn normalize(typed: &str) -> Option<String> {
    let mut out = String::with_capacity(LEN);
    for c in typed.chars() {
        let c = match c.to_ascii_uppercase() {
            ' ' | '-' => continue,
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        };
        if !c.is_ascii() || !ALPHABET.contains(&(c as u8)) {
            return None;
        }
        out.push(c);
    }
    (out.len() == LEN).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_sixteen_characters_and_typing_is_forgiving() {
        let a = generate().unwrap();
        let b = generate().unwrap();
        assert_eq!(a.len(), LEN);
        assert_ne!(a, b);
        assert_eq!(normalize(&display(&a)).unwrap(), a);
        assert_eq!(normalize(&display(&a).to_lowercase()).unwrap(), a);
        assert_eq!(normalize("oooo iiii llll 2222").unwrap(), "0000111111112222");
        assert!(normalize("ABCD-EFGH-JKMN-PQR").is_none(), "too short");
        assert!(normalize("ABCD-EFGH-JKMN-PQRSU").is_none(), "U is not in the alphabet");
        assert!(normalize("ABCD-EFGH-JKMN-PQRÉ").is_none());
    }
}
