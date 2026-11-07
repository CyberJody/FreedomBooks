//! Total, byte-exact stored-value validators shared by every module's write guards and open-time checks.
//!
//! SQLite's `length()`, `GLOB`, `LIKE` and the date functions stop at NUL or accept impossible dates
//! (`julianday('2026-02-30')`), so these checks run here on the full value. One predicate per value class:
//!
//! * [`text`]: no NUL, valid UTF-8 text storage (every TEXT column);
//! * [`uuid`]: canonical lower-case hyphenated UUID text, byte-exact ([`crate::banking_evidence::canonical_uuid`]);
//! * [`date`]: `YYYY-MM-DD`, a real calendar date, year 0001..=9999 ([`crate::banking_evidence::calendar_date`]);
//! * [`utc_timestamp`]: `YYYY-MM-DDTHH:MM:SSZ` or `...SS.ffffffZ` (the form `py::now_utc` writes), real date,
//!   year >= 1, no leap second; [`utc_timestamp_micros`] is the six-digit form only.
//!
//! [`register`] installs them as SQL functions. Module-specific names are aliases of the same predicates with
//! the NULL policy their migrations were reviewed with; their stored SQL never changes. Attachments (0111)
//! keep `fb_attachment_uuid` / `fb_attachment_timestamp` (NULL is invalid); INT-L4b adds bank feeds'
//! `fb_bankfeeds_*` names (NULL passes, six-digit timestamps, bounded text) to [`ALIASES`] the same way.
use rusqlite::{Connection, functions::FunctionFlags, types::ValueRef};

/// No NUL anywhere in the stored text. (Text storage is UTF-8 by construction for this connection.)
pub fn text(s: &str) -> bool {
    !s.contains('\0')
}
pub fn uuid(s: &str) -> bool {
    crate::banking_evidence::canonical_uuid(s)
}
pub fn date(s: &str) -> bool {
    crate::banking_evidence::calendar_date(s)
}
/// UTC canonical text with whole seconds or six fractional digits.
pub fn utc_timestamp(s: &str) -> bool {
    let b = s.as_bytes();
    // ASCII first: every byte index below is then a char boundary (multibyte input never panics).
    s.is_ascii()
        && matches!(b.len(), 20 | 27)
        && date(&s[..10])
        && b[10] == b'T'
        && b[13] == b':'
        && b[16] == b':'
        && b[b.len() - 1] == b'Z'
        && [11, 12, 14, 15, 17, 18].iter().all(|&i| b[i].is_ascii_digit())
        && (b.len() == 20 || (b[19] == b'.' && b[20..26].iter().all(u8::is_ascii_digit)))
        // Explicit bounds exclude leap seconds, which chrono otherwise accepts.
        && &b[11..13] <= b"23"
        && &b[14..16] <= b"59"
        && &b[17..19] <= b"59"
}
/// The six-fractional-digit form only (`%Y-%m-%dT%H:%M:%S%.6fZ`).
pub fn utc_timestamp_micros(s: &str) -> bool {
    s.len() == 27 && utc_timestamp(s)
}

/// The value class each shared SQL function checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Text,
    Uuid,
    Date,
    Timestamp,
    TimestampMicros,
}
impl Class {
    pub fn check(self, s: &str) -> bool {
        text(s)
            && match self {
                Class::Text => true,
                Class::Uuid => uuid(s),
                Class::Date => date(s),
                Class::Timestamp => utc_timestamp(s),
                Class::TimestampMicros => utc_timestamp_micros(s),
            }
    }
    /// The shared SQL function name used by migration 0130's guards and open-time view.
    pub fn function(self) -> &'static str {
        match self {
            Class::Text => "fb_valid_text",
            Class::Uuid => "fb_valid_uuid",
            Class::Date => "fb_valid_date",
            Class::Timestamp => "fb_valid_timestamp",
            Class::TimestampMicros => "fb_bankfeeds_timestamp",
        }
    }
}

/// How a SQL function treats NULL (nullability itself belongs to the column's NOT NULL / CHECK).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Null {
    Valid,
    Invalid,
}

/// Shared functions: NULL passes; anything that is not text storage (a BLOB in a TEXT column) fails.
pub const SHARED: [Class; 4] = [Class::Text, Class::Uuid, Class::Date, Class::Timestamp];
/// Reviewed module names kept as aliases of the shared predicates (stored migration SQL calls them).
pub const ALIASES: &[(&str, Class, Null)] = &[
    ("fb_attachment_uuid", Class::Uuid, Null::Invalid),
    ("fb_attachment_timestamp", Class::Timestamp, Null::Invalid),
    ("fb_bankfeeds_uuid", Class::Uuid, Null::Valid),
    ("fb_bankfeeds_date", Class::Date, Null::Valid),
    ("fb_bankfeeds_timestamp", Class::TimestampMicros, Null::Valid),
];

fn stored_text<'a>(v: ValueRef<'a>) -> Option<&'a str> {
    match v {
        ValueRef::Text(t) => std::str::from_utf8(t).ok(),
        _ => None,
    }
}
fn holds(v: ValueRef<'_>, class: Class, null: Null) -> bool {
    match v {
        ValueRef::Null => null == Null::Valid,
        other => stored_text(other).is_some_and(|s| class.check(s)),
    }
}

pub(crate) fn register(conn: &Connection) -> rusqlite::Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC | FunctionFlags::SQLITE_INNOCUOUS;
    for class in SHARED {
        conn.create_scalar_function(class.function(), 1, flags, move |ctx| Ok(holds(ctx.get_raw(0), class, Null::Valid)))?;
    }
    for &(name, class, null) in ALIASES {
        conn.create_scalar_function(name, 1, flags, move |ctx| Ok(holds(ctx.get_raw(0), class, null)))?;
    }
    // The reviewed bounded-text alias has three arguments and rejects NULL, unlike nullable dates.
    conn.create_scalar_function("fb_bankfeeds_text", 3, flags, |ctx| {
        let (min, max) = (ctx.get::<i64>(1)?, ctx.get::<i64>(2)?);
        Ok(stored_text(ctx.get_raw(0)).is_some_and(|s| text(s) && (min..=max).contains(&(s.chars().count() as i64))))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_calendar_and_byte_boundaries() {
        for s in [
            "2026-02-30T12:00:00Z",
            "2026-04-31T12:00:00Z",
            "2026-01-01T24:00:00Z",
            "2026-01-01T12:60:00Z",
            "2026-01-01T12:00:60Z",
            "2026-01-01T12:00:00Z\0suffix",
            "2026-01-01",
            "2026-01-01T12:00:00+00:00",
            "0000-01-01T00:00:00Z",
            "2026-01-01T12:00:00.12345Z",
            "2026-01-01T12:00:00.00000aZ",
            "",
        ] {
            assert!(!utc_timestamp(s), "{s:?}");
        }
        for s in ["2024-02-29T23:59:59Z", "2026-10-31T12:34:56.123456Z", "0001-01-01T00:00:00Z", "9999-12-31T23:59:59.999999Z"] {
            assert!(utc_timestamp(s), "{s:?}");
        }
        assert!(!utc_timestamp_micros("2024-02-29T23:59:59Z"));
        assert!(utc_timestamp_micros("2024-02-29T23:59:59.000000Z"));
        assert!(uuid("01234567-89ab-cdef-0123-456789abcdef"));
        for s in ["01234567-89ab-cdef-0123-456789abcdef\0suffix", "01234567-89AB-cdef-0123-456789abcdef", "0123456789abcdef0123456789abcdef", "{01234567-89ab-cdef-0123-456789abcdef}"] {
            assert!(!uuid(s), "{s:?}");
        }
        for s in ["0000-01-01", "2026-02-29", "2026-13-01", "10000-01-01", "2026-1-01", "2026-01-01\0"] {
            assert!(!date(s), "{s:?}");
        }
        assert!(date("0001-01-01") && date("2024-02-29") && date("9999-12-31"));
        assert!(!text("ok\0") && text("") && text("é€😀"));
    }

    #[test]
    fn sql_functions_check_the_full_stored_value_and_their_null_policy() {
        let conn = Connection::open_in_memory().unwrap();
        register(&conn).unwrap();
        let q = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, bool>(0)).unwrap();
        let u = "'01234567-89ab-cdef-0123-456789abcdef'";
        assert!(q(&format!("SELECT fb_valid_uuid({u})")));
        assert!(!q(&format!("SELECT fb_valid_uuid({u}||char(0)||'x')")));
        assert!(!q(&format!("SELECT fb_valid_uuid(CAST({u} AS BLOB))")));
        assert!(!q("SELECT fb_valid_text('ok'||char(0)||'x')"));
        assert!(!q("SELECT fb_valid_text(x'00')"));
        assert!(q("SELECT fb_valid_text('')"));
        assert!(!q("SELECT fb_valid_date('2026-02-30')"));
        assert!(!q("SELECT fb_valid_date('0000-12-31')"));
        assert!(!q("SELECT fb_valid_timestamp('2026-01-01T23:59:60Z')"));
        for f in ["fb_valid_text", "fb_valid_uuid", "fb_valid_date", "fb_valid_timestamp"] {
            assert!(q(&format!("SELECT {f}(NULL)")), "{f}");
        }
        assert!(!q("SELECT fb_attachment_uuid(NULL)") && !q("SELECT fb_attachment_timestamp(NULL)"));
        assert!(q(&format!("SELECT fb_attachment_uuid({u})")));
        assert!(q("SELECT fb_attachment_timestamp('2026-10-31T12:34:56Z')"));
        assert!(!q("SELECT fb_attachment_timestamp('2026-10-31T12:34:56Z'||char(0))"));
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
    }

    /// Every predicate is total on multibyte text and the SQL function agrees with the Rust predicate:
    /// each valid form with each character replaced or preceded by a 2-, 3- or 4-byte one.
    #[test]
    fn every_validator_is_total_on_multibyte_text_and_sql_agrees() {
        const MULTIBYTE: [char; 8] = ['é', '\u{301}', 'ß', '€', '\u{ffff}', '😀', '\u{10ffff}', '中'];
        let conn = Connection::open_in_memory().unwrap();
        register(&conn).unwrap();
        let valid = [
            (Class::Uuid, "01234567-89ab-cdef-0123-456789abcdef"),
            (Class::Date, "2024-02-29"),
            (Class::Timestamp, "2024-02-29T23:59:59.999999Z"),
            (Class::Timestamp, "2024-02-29T23:59:59Z"),
        ];
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut cases = 0;
        for (class, form) in valid {
            assert!(class.check(form));
            let chars: Vec<char> = form.chars().collect();
            for i in 0..=chars.len() {
                let m = MULTIBYTE[(rng.next() % MULTIBYTE.len() as u64) as usize];
                let mut inserted = chars.clone();
                inserted.insert(i, m);
                let mut variants = vec![inserted.into_iter().collect::<String>()];
                if i < chars.len() {
                    let mut replaced = chars.clone();
                    replaced[i] = m;
                    variants.push(replaced.into_iter().collect());
                }
                for v in variants {
                    let got = std::panic::catch_unwind(|| class.check(&v));
                    assert!(matches!(got, Ok(false)), "{class:?} {v:?}");
                    let sql: bool = conn.query_row(&format!("SELECT {}(?1)", class.function()), [&v], |r| r.get(0)).unwrap();
                    assert!(!sql, "{class:?} UDF on {v:?}");
                    cases += 1;
                }
            }
        }
        assert!(cases > 150);
    }
}
