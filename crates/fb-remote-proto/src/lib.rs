//! Remote access, book side (design: `docs/remote-access-design.md`).
//!
//! One computer hosts the books; other computers connect to that host application, never to the
//! book file. This crate is the part that runs in the process that owns the book: the typed wire
//! protocol, size-limited framing, and the [`Dispatcher`] that maps each remote request onto the
//! session API ([`Backend`]) after the remote route policy, sign-in throttle and retry
//! deduplication. It links no network or TLS stack (the gate's isolation check covers this crate
//! as it does fb-app and fb-node). TLS, pairing and device certificates live in the separate
//! `fb-remote` process, which this crate starts as a child and talks to over stdio ([`link`]).
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]
#![forbid(unsafe_code)]

pub mod audit;
pub mod backend;
pub mod code;
pub mod dedupe;
pub mod dispatch;
pub mod frame;
pub mod link;
pub mod policy;
pub mod registry_file;
pub mod scope;
pub mod strict;
pub mod throttle;
pub mod wire;

pub use backend::{Backend, BackendError, Remote, Rendered, SessionRef, SignInState, SignedIn};
pub use dispatch::{Config, Dispatcher};
pub use wire::{ClientRequest, DeviceRef, HostResponse, RefusalCode};

/// Lower-case hex, used for pins, proofs and audit hashes.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 15) as usize] as char);
    }
    out
}

/// Strict lower-case hex decoding (no prefix, no whitespace, even length).
pub fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    text.as_bytes().chunks(2).map(|p| Some(digit(p[0])? << 4 | digit(p[1])?)).collect()
}

/// Invisible formatting characters (Unicode general category Cf: bidirectional overrides and
/// isolates, zero-width characters, the byte-order mark, tags) and the line and paragraph
/// separators. Names shown in lists and the audit drop them, so a name cannot reorder or hide text.
pub fn is_format_char(c: char) -> bool {
    matches!(c,
        '\u{ad}' | '\u{600}'..='\u{605}' | '\u{61c}' | '\u{6dd}' | '\u{70f}' | '\u{890}'..='\u{891}' | '\u{8e2}' | '\u{180e}'
        | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206f}' | '\u{feff}' | '\u{fff9}'..='\u{fffb}'
        | '\u{110bd}' | '\u{110cd}' | '\u{13430}'..='\u{1343f}' | '\u{1bca0}'..='\u{1bca3}' | '\u{1d173}'..='\u{1d17a}' | '\u{e0001}' | '\u{e0020}'..='\u{e007f}')
}

/// `n` bytes from the operating system's random source.
pub fn random<const N: usize>() -> Result<[u8; N], getrandom::Error> {
    let mut out = [0u8; N];
    getrandom::fill(&mut out)?;
    Ok(out)
}

/// A request id the client generated: a canonical lower-case UUID (8-4-4-4-12).
pub fn is_request_id(id: &str) -> bool {
    let b = id.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_digit() || (b'a'..=b'f').contains(c),
        })
}

#[cfg(test)]
mod tests {
    #[test]
    fn hex_round_trips_and_refuses_bad_text() {
        let bytes = [0u8, 1, 0xab, 0xff];
        assert_eq!(super::hex(&bytes), "0001abff");
        assert_eq!(super::unhex("0001abff").unwrap(), bytes);
        assert!(super::unhex("0001ABFF").is_none());
        assert!(super::unhex("abc").is_none());
        assert!(super::unhex("zz").is_none());
    }

    #[test]
    fn format_characters_are_recognised() {
        for c in ['\u{202e}', '\u{2066}', '\u{200b}', '\u{feff}', '\u{2028}', '\u{e0041}'] {
            assert!(super::is_format_char(c), "{:x}", c as u32);
        }
        for c in ['a', ' ', 'é', '中', '\u{2010}'] {
            assert!(!super::is_format_char(c), "{c}");
        }
        assert_eq!(super::audit::clean("Front\u{202e}desk\u{200b}\n"), "Frontdesk");
    }

    #[test]
    fn request_ids_are_canonical_uuids() {
        assert!(super::is_request_id("211ce6fc-4559-46d4-91ea-c81b0b41ff93"));
        assert!(!super::is_request_id("211CE6FC-4559-46d4-91ea-c81b0b41ff93"));
        assert!(!super::is_request_id("211ce6fc455946d491eac81b0b41ff93"));
        assert!(!super::is_request_id(""));
    }
}
