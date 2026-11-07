//! Total, byte-exact bank-feed validators shared by the 0134 write guards and the open-time view.
//! SQLite's `length()` and `GLOB` stop at NUL and `julianday` accepts impossible dates, so text,
//! UUID, date and timestamp checks run here on the full value. UUIDs and dates reuse
//! [`crate::banking_evidence`]; the timestamp form is the one every core write stores
//! (`%Y-%m-%dT%H:%M:%S%.6fZ`, `py::now_utc`).
use rusqlite::{Connection, functions::FunctionFlags, types::ValueRef};

/// Canonical UTC timestamp text with six fractional digits, a real calendar date and no leap second.
pub fn timestamp(s: &str) -> bool {
    crate::validators::utc_timestamp_micros(s)
}
/// Text without NUL whose length in code points (Python `len`) lies in `min..=max`.
pub fn text(s: &str, min: i64, max: i64) -> bool {
    let n = s.chars().count() as i64;
    crate::validators::text(s) && (min..=max).contains(&n)
}
fn as_text<'a>(v: ValueRef<'a>) -> Option<&'a str> {
    match v {
        ValueRef::Text(t) => std::str::from_utf8(t).ok(),
        _ => None,
    }
}
/// Length of the per-book identity secret (`bankfeeds_identity_secret.secret`, migration 0136).
pub const IDENTITY_SECRET_LEN: usize = 32;
/// The stable, credential-free identity of one provider identifier (FIX5): lowercase hex
/// HMAC-SHA256 under the book's random identity secret of `domain`, a NUL and the raw `value`. The
/// domain (`connection-key`, `account`, `transaction`) never holds a NUL, so the message is injective.
/// Uniqueness, dedupe, mapping and lookup use this key; the stored text is the redacted id, for display.
/// A keyed MAC, never a bare hash: without the secret (inside the encrypted book) a short password in
/// the userinfo cannot be recovered by guessing.
pub fn identity_key(secret: &[u8], domain: &str, value: &str) -> String {
    use sha2::{Digest, Sha256};
    const BLOCK: usize = 64;
    let mut key = [0u8; BLOCK];
    if secret.len() > BLOCK {
        key[..32].copy_from_slice(&Sha256::digest(secret));
    } else {
        key[..secret.len()].copy_from_slice(secret);
    }
    let pad = |byte: u8| key.iter().map(|k| k ^ byte).collect::<Vec<u8>>();
    let inner = Sha256::new().chain_update(pad(0x36)).chain_update(domain.as_bytes()).chain_update([0u8]).chain_update(value.as_bytes()).finalize();
    let outer = Sha256::new().chain_update(pad(0x5c)).chain_update(inner).finalize();
    outer.iter().map(|b| format!("{b:02x}")).collect()
}
/// A stored identity key: 64 lowercase hex digits.
pub fn identity_form(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
pub(crate) fn register(conn: &Connection) -> rusqlite::Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC | FunctionFlags::SQLITE_INNOCUOUS;
    // Validators are registered once by the shared module, including the reviewed alias policies.
    crate::validators::register(conn)?;
    conn.create_scalar_function("fb_bankfeeds_identity_form", 1, flags, |ctx| Ok(as_text(ctx.get_raw(0)).is_some_and(identity_form)))?;
    // 0136's backfill: the identity of an already stored (already redacted) id. NULL unless the secret
    // is a 32-byte blob and the domain and value are text.
    conn.create_scalar_function("fb_bankfeeds_identity", 3, flags, |ctx| {
        let secret = match ctx.get_raw(0) {
            ValueRef::Blob(b) if b.len() == IDENTITY_SECRET_LEN => b,
            _ => return Ok(None),
        };
        Ok(match (as_text(ctx.get_raw(1)), as_text(ctx.get_raw(2))) {
            (Some(domain), Some(value)) => Some(identity_key(secret, domain, value)),
            _ => None,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    /// `identity_key` is HMAC-SHA256 over `domain || NUL || value`: checked against a separate textbook
    /// construction, itself pinned to RFC 4231 test case 2.
    #[test]
    fn identity_key_is_hmac_sha256() {
        use sha2::{Digest, Sha256};
        let reference = |key: &[u8], msg: &[u8]| {
            let mut k = [0u8; 64];
            k[..key.len()].copy_from_slice(key);
            let i: Vec<u8> = k.iter().map(|b| b ^ 0x36).chain(msg.iter().copied()).collect();
            let o: Vec<u8> = k.iter().map(|b| b ^ 0x5c).chain(Sha256::digest(&i)).collect();
            Sha256::digest(&o).iter().map(|b| format!("{b:02x}")).collect::<String>()
        };
        // RFC 4231 case 2 through the reference construction, so the reference itself is pinned.
        assert_eq!(reference(b"Jefe", b"what do ya want for nothing?"), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        let secret = [7u8; IDENTITY_SECRET_LEN];
        assert_eq!(identity_key(&secret, "transaction", "TX-1"), reference(&secret, b"transaction\0TX-1"));
        assert_ne!(identity_key(&secret, "transaction", "TX-1"), identity_key(&[8u8; IDENTITY_SECRET_LEN], "transaction", "TX-1"));
        assert_ne!(identity_key(&secret, "account", "TX-1"), identity_key(&secret, "transaction", "TX-1"));
        assert!(identity_form(&identity_key(&secret, "account", "")));
        assert!(!identity_form(&"A".repeat(64)) && !identity_form("00"));
    }
    #[test]
    fn byte_exact_text_timestamps_and_uuids() {
        for s in ["2026-02-30T12:00:00.000000Z", "2026-01-01T24:00:00.000000Z", "2026-01-01T12:60:00.000000Z", "2026-01-01T12:00:60.000000Z",
            "2026-01-01T12:00:00.000000Z\0x", "2026-01-01T12:00:00Z", "0000-01-01T12:00:00.000000Z", "2026-01-01T12:00:00.00000aZ", ""] {
            assert!(!timestamp(s), "{s:?}");
        }
        assert!(timestamp("2024-02-29T23:59:59.999999Z"));
        assert!(timestamp("0001-01-01T00:00:00.000000Z"));
        assert!(!text("ok\0", 0, 500));
        assert!(!text(&"é".repeat(501), 0, 500));
        assert!(text(&"é".repeat(500), 0, 500));
        assert!(!text("", 1, 120));
        let conn = Connection::open_in_memory().unwrap();
        register(&conn).unwrap();
        let q = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, bool>(0)).unwrap();
        assert!(!q("SELECT fb_bankfeeds_uuid('01234567-89ab-cdef-0123-456789abcdef'||char(0)||'x')"));
        assert!(!q("SELECT fb_bankfeeds_uuid(printf('%.36c','x'))"));
        assert!(q("SELECT fb_bankfeeds_uuid('01234567-89ab-cdef-0123-456789abcdef')"));
        assert!(q("SELECT fb_bankfeeds_uuid(NULL)"));
        assert!(!q("SELECT fb_bankfeeds_text('ok'||char(0)||'x',0,500)"));
        assert!(!q("SELECT fb_bankfeeds_date('0000-01-01')"));
        assert!(!q("SELECT fb_bankfeeds_date(20261001)"));
    }

    /// Deterministic xorshift, so a failure names a reproducible case.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn pick<T: Copy>(&mut self, from: &[T]) -> T {
            from[(self.next() % from.len() as u64) as usize]
        }
    }
    const MULTIBYTE: [char; 9] = ['é', '\u{301}', 'ß', '€', '\u{ffff}', '😀', '\u{10ffff}', '\u{a0}', '中'];

    /// Every validator here (and the reused UUID / date ones) is total on multibyte text: each valid form
    /// with any character replaced or inserted by a 2-, 3- or 4-byte one, at every position, plus random
    /// mixed strings around the fixed byte lengths, through the Rust function and the SQL UDF.
    #[test]
    fn every_validator_is_total_on_multibyte_text() {
        let conn = Connection::open_in_memory().unwrap();
        register(&conn).unwrap();
        let sql = |f: &str, v: &str| -> bool {
            let q = if f == "fb_bankfeeds_text" { format!("SELECT {f}(?1,0,500)") } else { format!("SELECT {f}(?1)") };
            conn.query_row(&q, [v], |r| r.get(0)).unwrap()
        };
        type Check = fn(&str) -> bool;
        let fixed: [(&str, Check, &str); 3] = [
            ("fb_bankfeeds_timestamp", timestamp, "2024-02-29T23:59:59.999999Z"),
            ("fb_bankfeeds_uuid", crate::banking_evidence::canonical_uuid, "01234567-89ab-cdef-0123-456789abcdef"),
            ("fb_bankfeeds_date", crate::banking_evidence::calendar_date, "2024-02-29"),
        ];
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut cases = 0;
        let mut run = |name: &str, f: Check, v: &str, expect: Option<bool>| {
            let got = std::panic::catch_unwind(|| f(v));
            assert!(got.is_ok(), "{name} panicked on {v:?}");
            if let Some(e) = expect {
                assert_eq!(got.unwrap(), e, "{name} on {v:?}");
            }
            assert_eq!(sql(name, v), f(v), "{name} UDF disagrees on {v:?}");
            cases += 1;
        };
        for (name, f, valid) in fixed {
            assert!(f(valid));
            let chars: Vec<char> = valid.chars().collect();
            for i in 0..=chars.len() {
                for m in MULTIBYTE {
                    let mut replaced = chars.clone();
                    if i < chars.len() {
                        replaced[i] = m;
                        run(name, f, &replaced.iter().collect::<String>(), Some(false));
                    }
                    let mut inserted = chars.clone();
                    inserted.insert(i, m);
                    run(name, f, &inserted.iter().collect::<String>(), Some(false));
                    // Same byte length as the valid form: drop ASCII so the multibyte char fits exactly.
                    let extra = m.len_utf8() - 1;
                    if i + extra < chars.len() {
                        let mut same = chars.clone();
                        same[i] = m;
                        same.truncate(chars.len() - extra);
                        let v: String = same.iter().collect();
                        assert_eq!(v.len(), valid.len());
                        run(name, f, &v, Some(false));
                    }
                }
            }
            for _ in 0..2000 {
                let target = valid.len() as u64 + rng.next() % 3 - 1;
                let mut v = String::new();
                while (v.len() as u64) < target {
                    if rng.next().is_multiple_of(3) { v.push(rng.pick(&MULTIBYTE)) } else { v.push(rng.pick(&chars)) }
                }
                run(name, f, &v, (!v.is_ascii()).then_some(false));
            }
        }
        let text_check: Check = |v| text(v, 0, 500);
        for _ in 0..2000 {
            let n = (rng.next() % 520) as usize;
            let v: String = (0..n).map(|_| if rng.next().is_multiple_of(2) { rng.pick(&MULTIBYTE) } else { 'a' }).collect();
            run("fb_bankfeeds_text", text_check, &v, Some(n <= 500));
        }
        assert!(cases > 8000, "{cases}");
    }
}
