//! Fuzz audit 2026-11, area: bank-feed provider text (fb-core `bankfeeds::text` and `SecretSet`).
//!
//! Properties: `redact` is idempotent, control characters included; `sanitize` honours its
//! limit; a known credential never survives `SecretSet::mask`, however it is embedded; and
//! (finding F-04b, fixed on branch fuzz-fix) a control or zero-width character between or inside
//! `Authorization`, `Basic` and the token no longer hides the token from `redact` (the fuzz run found
//! it in seventeen seconds: fuzz/artifacts/bankfeeds_text/crash-26112b40...).
use fb_core::bankfeeds::SecretSet;
use fb_core::bankfeeds::text::{redact, sanitize};
use proptest::prelude::*;

const SECRET: &str = "s3cretT0kenValue9876";

fn provider_text() -> impl Strategy<Value = String> {
    let atom = prop_oneof![
        4 => "[A-Za-z0-9 ]{1,12}",
        2 => Just("Basic ".to_string()),
        2 => Just("Authorization: ".to_string()),
        2 => Just("dXNlcjpwYXNzd29yZDEyMw==".to_string()),
        1 => Just("https://user:pw@bank.example/path?x=1".to_string()),
        1 => Just(SECRET.to_string()),
        1 => Just("<b>".to_string()),
        1 => Just("&#115;".to_string()),
        1 => Just("\\u0073".to_string()),
        1 => Just("\u{a0}".to_string()),
        1 => "[\\x00-\\x1f]",
    ];
    prop::collection::vec(atom, 0..8).prop_map(|parts| parts.concat())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 600, .. ProptestConfig::default() })]

    #[test]
    fn redact_is_idempotent(text in provider_text()) {
        let once = redact(&text);
        prop_assert_eq!(redact(&once), once.clone(), "redact not idempotent on {:?}", text);
        for limit in [0usize, 1, 50, 500] {
            prop_assert!(sanitize(&text, limit).chars().count() <= limit);
        }
    }

    #[test]
    fn a_known_credential_never_survives_masking(prefix in provider_text(), suffix in provider_text()) {
        let secrets = SecretSet::from_credentials("userName12345", SECRET, Vec::<&str>::new()).unwrap();
        let text = format!("{prefix}{SECRET}{suffix}");
        let masked = secrets.mask(&text);
        prop_assert!(!masked.contains(SECRET), "secret survived masking: {:?}", masked);
        let split = format!("{prefix}Basic\u{0} {SECRET}{suffix}");
        prop_assert!(!secrets.mask(&split).contains(SECRET), "secret survived masking after a split keyword");
    }
}

/// FINDING F-04b (data correctness / secret hygiene, fb-core side of F-04; fixed): `bankfeeds::text::redact`
/// matched `Basic <token>` and `Authorization: Basic <token>` before it mapped control characters to
/// spaces, so `Basic \0<token>` was not redacted on the first pass; a second pass over the output
/// did redact it, which is how the fuzz harness (idempotence) caught it. The Basic matchers now see
/// the text with hidden characters (`text::is_hidden`) folded out.
#[test]
fn redact_masks_a_basic_token_split_by_a_control_character() {
    for text in ["Basic \u{0}\u{8}s3cretT0kenValue9876-abcd==", "Authorization: Basic\u{1} dXNlcjpwYXNzd29yZDEyMw=="] {
        let once = redact(text);
        assert_eq!(redact(&once), once, "redact must be idempotent on {text:?}");
        assert!(!once.contains("s3cretT0kenValue9876") && !once.contains("dXNlcjpwYXNzd29yZDEyMw"), "token survived: {once:?}");
    }
}

/// FINDING F-11 (cosmetic): `redact` is not idempotent on some URL shapes without any control
/// character: a second pass lower-cases a scheme the first pass kept as typed, and drops a `?`
/// after a masked nested userinfo. Found by the `bankfeeds_text` fuzz target
/// (fuzz/artifacts/bankfeeds_text/crash-7f47cd1a... and crash-d75f65af...). Pinned; fails today.
#[test]
#[ignore = "fuzz audit finding F-11: bankfeeds::text::redact is not idempotent on URL scheme case / nested userinfo"]
fn redact_is_idempotent_on_url_shapes() {
    for text in ["hTtps://u/ser:pases)ras@bnps:/rs%\u{d0}path", "https://user:p://user:pass@bpas?<th"] {
        let once = redact(text);
        assert_eq!(redact(&once), once, "redact must be idempotent on {text:?}");
    }
}

/// F-04b fix: the token goes however a hidden character (NUL, other C0 controls, DEL, zero-width
/// space / joiners / word joiner / BOM) or real whitespace (tab, CRLF, a folded header line) splits the
/// header name, the scheme, the separator or the token itself.
#[test]
fn redact_masks_basic_credentials_split_by_controls_crlf_tabs_nul_and_zero_width() {
    let token = "dXNlcjpwYXNzd29yZDEyMw==";
    let (head, tail) = token.split_at(9);
    let splitters = ["\u{0}", "\u{1}", "\u{8}", "\u{1b}", "\u{7f}", "\u{200b}", "\u{200c}", "\u{200d}", "\u{2060}", "\u{feff}"];
    let mut cases = vec![
        format!("Authorization: Basic {token}"),
        format!("Authorization:\tBasic\t{token}"),
        format!("Authorization: Basic\r\n {token}"),
        format!("Authorization:\r\n\tBasic {token}"),
        format!("GET / HTTP/1.1\r\nAuthorization: Basic {token}\r\nHost: bridge.example\r\n"),
        format!("basic\t{token}"),
    ];
    for x in splitters {
        cases.push(format!("Authorization: Basic{x} {token}"));
        cases.push(format!("Authorization: Basic{x}{token}"));
        cases.push(format!("Authorization: Basic {x}{token}"));
        cases.push(format!("Auth{x}orization: Basic {token}"));
        cases.push(format!("Authorization{x}: Basic {token}"));
        cases.push(format!("Authorization:{x} Ba{x}sic {token}"));
        cases.push(format!("Authorization: Basic {head}{x}{tail}"));
        cases.push(format!("Authorization: Basic {head}{x}{x}{tail}\r\nHost: h"));
        cases.push(format!("error Basic{x}{token} refused"));
        cases.push(format!("error{x}Basic {head}{x}{tail} refused"));
        cases.push(format!("Basic {}{x}{}", &token[..3], &token[3..]));
    }
    for text in cases {
        let once = redact(&text);
        for piece in [token, head, tail, &token[3..]] {
            assert!(!once.contains(piece), "{piece:?} survived redact on {text:?}: {once:?}");
        }
        assert!(once.contains("***"), "{text:?}: {once:?}");
        assert_eq!(redact(&once), once, "idempotent on {text:?}");
        assert!(!once.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}'), "controls still blanked: {once:?}");
        assert!(!sanitize(&text, 500).contains(tail), "sanitize on {text:?}");
    }
    // Text around a redaction is kept as written; a CRLF still ends the header value.
    assert_eq!(redact("Authorization: Basic\u{0}dXNlcjpw\r\nHost: h"), "Authorization: Basic ***  Host: h");
    assert_eq!(redact("no\u{200b}credential here\u{0}"), "no\u{200b}credential here ");
}
