//! Fuzz audit 2026-11, area: identifier, integer and text validators.
//!
//! Properties: the two Python `int()` ports (`fb_core::py::py_int`, `fb_templates::forms::python_integer`)
//! agree with each other and with CPython; `py_uuid` agrees with CPython `uuid.UUID`; every
//! canonical UUID round-trips through `py_uuid`, `fb_ids::parse_uuid` and `CompanyId::parse`;
//! check numbers accepted by `normalize` are `valid`; `csv_text` never leaves a formula lead; and
//! `email_validation::valid_email` accepts every simple address and refuses the shapes it documents.
//! Python checks are skipped, with a message, when `python3` is not on PATH.
use fb_core::py::{PyInt, py_int, py_uuid};
use fb_templates::forms::python_integer;
use proptest::prelude::*;
use std::io::Write;
use std::process::{Command, Stdio};

fn intish() -> impl Strategy<Value = String> {
    let atom = prop_oneof![
        8 => "[0-9]{1,25}",
        2 => Just("_".to_string()),
        2 => Just("-".to_string()),
        2 => Just("+".to_string()),
        1 => Just(" ".to_string()),
        1 => Just("\t".to_string()),
        1 => Just("\u{3000}".to_string()),
        1 => Just("\u{1c}".to_string()),
        1 => Just("\u{662}".to_string()),
        1 => Just("\u{ff17}".to_string()),
        1 => Just("\u{b2}".to_string()),
        1 => Just(".".to_string()),
        1 => Just("0".repeat(4300)),
    ];
    prop::collection::vec(atom, 0..6).prop_map(|parts| parts.concat())
}

fn uuidish() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => any::<u128>().prop_map(|n| uuid::Uuid::from_u128(n).to_string()),
        1 => any::<u128>().prop_map(|n| uuid::Uuid::from_u128(n).hyphenated().to_string().to_uppercase()),
        1 => any::<u128>().prop_map(|n| format!("{{{}}}", uuid::Uuid::from_u128(n))),
        1 => any::<u128>().prop_map(|n| format!("urn:uuid:{}", uuid::Uuid::from_u128(n))),
        1 => any::<u128>().prop_map(|n| uuid::Uuid::from_u128(n).simple().to_string()),
        1 => any::<u128>().prop_map(|n| format!(" {} ", uuid::Uuid::from_u128(n))),
        1 => any::<u128>().prop_map(|n| format!("+{}", &uuid::Uuid::from_u128(n).simple().to_string()[1..])),
        1 => "[0-9a-fA-F{}urn:uid \\-_]{0,40}",
    ]
}

/// FINDING F-05 (cosmetic, wrong accept): `python_integer` strips U+001C..U+001F (Python's
/// `str.isspace` is true for them) but CPython's `int()` refuses them: `int("5\x1c")` is a
/// ValueError. `py_int` matches CPython. The agreement test below leaves those inputs out.
fn has_information_separator(s: &str) -> bool {
    s.chars().any(|c| ('\u{1c}'..='\u{1f}').contains(&c))
}

/// FINDING F-06 (cosmetic, wrong reject): CPython `uuid.UUID(hex)` runs `int(hex, 16)` on the
/// 32-character body, so a leading `+` or an underscore between hex digits is accepted
/// (`uuid.UUID("+b7b2ced7654da4bd68b7c98b39a0faf")` is a UUID). `py_uuid` refuses both.
fn has_int16_extras(s: &str) -> bool {
    s.contains(['+', '_'])
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, .. ProptestConfig::default() })]

    #[test]
    fn the_two_python_int_ports_agree(s in intish().prop_filter("F-05", |s| !has_information_separator(s))) {
        let a = py_int(&s);
        let b = python_integer(&s);
        prop_assert_eq!(a.is_some(), b.is_some(), "py_int {:?} vs python_integer {:?} on {:?}", a, b, s);
        if let (Some(PyInt::Small(x)), Some(y)) = (a, b) {
            prop_assert_eq!(x, y, "values differ on {:?}", s);
        }
    }

    #[test]
    fn small_ints_round_trip(n in any::<i64>()) {
        prop_assert_eq!(py_int(&n.to_string()), Some(PyInt::Small(n)));
        prop_assert_eq!(python_integer(&n.to_string()), Some(n));
        prop_assert_eq!(python_integer(&format!(" +{} ", n.unsigned_abs())), Some(n.unsigned_abs().min(i64::MAX as u64) as i64));
    }

    #[test]
    fn canonical_uuids_round_trip(n in any::<u128>()) {
        let u = uuid::Uuid::from_u128(n);
        let canonical = u.to_string();
        prop_assert_eq!(py_uuid(&canonical).map(|c| c.0), Some(canonical.clone()));
        prop_assert_eq!(py_uuid(&canonical.to_uppercase()).map(|c| c.0), Some(canonical.clone()));
        prop_assert_eq!(py_uuid(&u.simple().to_string()).map(|c| c.0), Some(canonical.clone()));
        prop_assert_eq!(py_uuid(&format!("{{{}}}", canonical)).map(|c| c.0), Some(canonical.clone()));
        prop_assert_eq!(py_uuid(&format!("urn:uuid:{}", canonical)).map(|c| c.0), Some(canonical.clone()));
        prop_assert_eq!(fb_ids::parse_uuid(&canonical), Some(u));
        prop_assert_eq!(fb_ids::CompanyId::parse(&canonical).map(|c| c.to_string()), Some(canonical.clone()));
        prop_assert!(fb_db::banking_evidence::canonical_uuid(&canonical));
        prop_assert!(!fb_db::banking_evidence::canonical_uuid(&canonical.to_uppercase()));
    }

    #[test]
    fn check_numbers_normalize_then_valid(s in "[ \\t]{0,2}[0-9A-Za-z\\-#/ ]{0,24}[ \\t]{0,2}") {
        if let Ok(n) = fb_db::check_numbers::normalize(&s) {
            prop_assert!(fb_db::check_numbers::valid(n), "{:?} -> {:?}", s, n);
            prop_assert!(n.chars().count() <= 20);
            prop_assert_eq!(n, n.trim());
        }
    }

    #[test]
    fn csv_text_never_leaves_a_formula_lead(s in "[=+\\-@\\t\\r a-z0-9]{0,8}") {
        let cell = fb_core::reports::params::csv_text(&s);
        prop_assert!(!cell.starts_with(['=', '+', '-', '@', '\t', '\r']), "{:?} -> {:?}", s, cell);
        prop_assert!(cell.ends_with(&s));
    }

    #[test]
    fn simple_emails_are_accepted_and_documented_shapes_refused(local in "[a-z0-9]{1,6}(\\.[a-z0-9+_-]{1,6}){0,3}", domain in "[a-z0-9]{1,10}", tld in "[a-z]{2,6}") {
        let good = format!("{local}@{domain}.{tld}");
        prop_assert!(fb_core::email_validation::valid_email(&good), "{}", good);
        prop_assert!(!fb_core::email_validation::valid_email(&format!("{local}@{domain}")), "a bare label is refused");
        prop_assert!(!fb_core::email_validation::valid_email(&format!("{local}{domain}.{tld}")), "no @");
        prop_assert!(!fb_core::email_validation::valid_email(&format!("{good} ")), "trailing space");
        prop_assert!(!fb_core::email_validation::valid_email(&format!("{local}@@{domain}.{tld}")), "two @");
        prop_assert!(!fb_core::email_validation::valid_email(&format!("{local}..x@{domain}.{tld}")), "consecutive dots");
        prop_assert!(!fb_core::email_validation::valid_email(&format!("{}@{domain}.{tld}", "a".repeat(321))), "over 320 chars");
    }
}

const PY: &str = r#"
import sys, json, uuid
def py_int(s):
    try:
        v = int(s)
    except Exception:
        return None
    return {"small": v} if -2**63 <= v < 2**63 else {"large": v < 0}
def py_uuid(s):
    try:
        return str(uuid.UUID(s))
    except Exception:
        return None
data = json.load(sys.stdin)
print(json.dumps({"ints": [py_int(s) for s in data["ints"]], "uuids": [py_uuid(s) for s in data["uuids"]]}))
"#;

fn python(inputs: &serde_json::Value) -> Option<serde_json::Value> {
    let mut child = Command::new("python3").args(["-I", "-c", PY]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().ok()?;
    child.stdin.take().unwrap().write_all(inputs.to_string().as_bytes()).ok()?;
    let out = child.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 10, .. ProptestConfig::default() })]

    #[test]
    fn int_and_uuid_ports_agree_with_cpython(
        ints in prop::collection::vec(intish(), 300),
        uuids in prop::collection::vec(uuidish().prop_filter("F-06", |s| !has_int16_extras(s)), 300),
    ) {
        let Some(out) = python(&serde_json::json!({"ints": ints, "uuids": uuids})) else {
            eprintln!("python3 not available; skipping the CPython differential check");
            return Ok(());
        };
        for (i, s) in ints.iter().enumerate() {
            let expected = &out["ints"][i];
            let actual = py_int(s);
            match (actual, expected) {
                (None, serde_json::Value::Null) => {}
                (Some(PyInt::Small(v)), e) if e.get("small").and_then(|x| x.as_i64()) == Some(v) => {}
                (Some(PyInt::Large { negative }), e) if e.get("large").and_then(|x| x.as_bool()) == Some(negative) => {}
                (a, e) => prop_assert!(false, "int({:?}): rust {:?} vs python {}", s, a, e),
            }
            if !has_information_separator(s) {
                prop_assert_eq!(python_integer(s).is_some(), !expected.is_null(), "python_integer({:?}) vs CPython", s);
            }
        }
        for (i, s) in uuids.iter().enumerate() {
            let expected = out["uuids"][i].as_str().map(str::to_owned);
            prop_assert_eq!(py_uuid(s).map(|c| c.0), expected, "uuid.UUID({:?})", s);
        }
    }
}

/// FINDING F-05, pinned (fails today): `python_integer` accepts "5\x1c"; CPython `int()` refuses it.
/// The fuzz run hit the same divergence in seconds (fuzz/artifacts/py_int_uuid/crash-ae803d4c...).
#[test]
#[ignore = "fuzz audit finding F-05: python_integer strips U+001C..U+001F that CPython int() refuses"]
fn python_integer_refuses_information_separators_like_cpython() {
    for s in ["5\u{1c}", "\u{1c}5", "\u{1c} 55 ", "7\u{1f}"] {
        assert_eq!(python_integer(s), None, "{s:?}");
        assert_eq!(py_int(s), None, "{s:?} (py_int already agrees with CPython)");
    }
}

/// FINDING F-06, pinned (fails today): CPython `uuid.UUID` accepts a `+` sign and underscores inside
/// the 32 hex characters because it parses them with `int(hex, 16)`; `py_uuid` refuses them.
#[test]
#[ignore = "fuzz audit finding F-06: py_uuid refuses '+'/'_' forms that CPython uuid.UUID accepts"]
fn py_uuid_accepts_cpythons_int16_extras() {
    assert_eq!(py_uuid("+b7b2ced7654da4bd68b7c98b39a0faf").map(|c| c.0), Some("0b7b2ced-7654-da4b-d68b-7c98b39a0faf".into()));
    assert_eq!(py_uuid("0000000000000000_000000000000000").map(|c| c.0), Some("00000000-0000-0000-0000-000000000000".into()));
}
