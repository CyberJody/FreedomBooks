//! Shared stored-evidence JSON parser. Object names must be unique at every depth.
//! Decoded names are compared, so escaped aliases and same-value duplicates fail too.
use rusqlite::{Connection, functions::FunctionFlags};
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use std::collections::HashSet;
use std::fmt;

/// Parse JSON without discarding duplicate object keys. Use before any typed evidence read.
pub fn parse_unique_json(raw: &str) -> serde_json::Result<Value> {
    // Let serde_json interpret its own private number representation. In particular,
    // arbitrary_precision delivers some numbers to deserialize_any as synthetic maps.
    let value = serde_json::from_str::<Value>(raw)?;
    let mut deserializer = serde_json::Deserializer::from_str(raw);
    UniqueKeys::deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(value)
}

/// Typed readers validate uniqueness before deserialization can discard evidence.
pub fn parse_unique_json_as<T: serde::de::DeserializeOwned>(raw: &str) -> serde_json::Result<T> {
    serde_json::from_value(parse_unique_json(raw)?)
}

/// Every stored JSON column, including audit evidence and nested snapshots.
pub const STORED_JSON_COLUMNS: &[(&str, &str)] = &[
    ("assistant_connection", "model_choices"),
    ("assistant_reading", "result"),
    ("banking_deposit", "data"),
    ("banking_depositreversal", "data"),
    ("banking_reconciliation", "data"),
    ("checks_checkline", "account_snapshot"),
    ("checks_checkline", "item_snapshot"),
    ("checks_checkrevision", "bank_snapshot"),
    ("checks_checkrevision", "company_snapshot"),
    ("checks_checkrevision", "payee_snapshot"),
    ("company_document_details", "data"),
    ("credits_creditapplication", "data"),
    ("credits_creditapplicationline", "data"),
    ("credits_creditmemo", "data"),
    ("credits_creditmemoline", "data"),
    ("credits_creditmemovoid", "data"),
    ("credits_creditrefund", "data"),
    ("credits_creditrefundvoid", "data"),
    ("credits_creditunapplication", "data"),
    ("ledger_auditevent", "detail"),
    ("payables_billline", "account_snapshot"),
    ("payables_billline", "item_snapshot"),
    ("payables_billpayment", "company_snapshot"),
    ("payables_billpayment", "vendor_snapshot"),
    ("payables_billrevision", "company_snapshot"),
    ("payables_billrevision", "vendor_snapshot"),
    ("prepayments_depositapplication", "data"),
    ("prepayments_depositrefund", "data"),
    ("prepayments_depositrefundvoid", "data"),
    ("prepayments_depositunapplication", "data"),
    ("sales_estimateline", "data"),
    ("sales_estimaterevision", "data"),
    ("sales_invoiceline", "data"),
    ("sales_invoicerevision", "data"),
    ("sales_receipt", "data"),
    ("sales_receiptreversal", "data"),
    ("sales_salesreceipt", "data"),
    ("sales_salesreceiptline", "data"),
    ("transfers_transfer", "company_snapshot"),
    ("transfers_transfer", "from_snapshot"),
    ("transfers_transfer", "to_snapshot"),
];

// This pass retains only decoded object keys, never values or number spellings.
struct UniqueKeys;
impl<'de> Deserialize<'de> for UniqueKeys {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = UniqueKeys;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("JSON with unique object keys")
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<UniqueKeys, E> {
                Ok(UniqueKeys)
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<UniqueKeys, E> {
                Ok(UniqueKeys)
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<UniqueKeys, E> {
                Ok(UniqueKeys)
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<UniqueKeys, E> {
                Ok(UniqueKeys)
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<UniqueKeys, E> {
                Ok(UniqueKeys)
            }
            fn visit_unit<E: de::Error>(self) -> Result<UniqueKeys, E> {
                Ok(UniqueKeys)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<UniqueKeys, A::Error> {
                while seq.next_element::<UniqueKeys>()?.is_some() {}
                Ok(UniqueKeys)
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<UniqueKeys, A::Error> {
                let mut keys = HashSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !keys.insert(key) {
                        return Err(de::Error::custom("Duplicate JSON object key"));
                    }
                    map.next_value::<UniqueKeys>()?;
                }
                Ok(UniqueKeys)
            }
        }
        deserializer.deserialize_any(UniqueVisitor)
    }
}

pub(crate) fn register(conn: &Connection) -> rusqlite::Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8
        | FunctionFlags::SQLITE_DETERMINISTIC
        | FunctionFlags::SQLITE_INNOCUOUS;
    conn.create_scalar_function("fb_json_unique_keys", 1, flags, |ctx| {
        Ok(ctx
            .get::<Option<String>>(0)?
            .is_some_and(|raw| parse_unique_json(&raw).is_ok()))
    })?;
    conn.create_scalar_function("fb_company_document_valid", 1, flags, |ctx| {
        Ok(ctx.get::<Option<String>>(0)?.is_some_and(|raw| crate::company_document_data::valid(&raw)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_duplicates_recursively_and_accepts_independent_objects() {
        for raw in [
            r#"{"a":1,"a":1}"#,
            r#"{"a":{"b":1,"b":2}}"#,
            r#"[{"a":1,"\u0061":2}]"#,
            r#"{"x":[{"a":true,"a":false}]}"#,
            r#"{} {}"#,
            "",
        ] {
            assert!(parse_unique_json(raw).is_err(), "{raw}");
        }
        for raw in [
            r#"{"a":1,"b":[{"a":false},{"a":null}],"c":"😀"}"#,
            r#"[1,-2,3.5,true,false,null,"x",{},[]]"#,
        ] {
            assert_eq!(
                parse_unique_json(raw).unwrap(),
                serde_json::from_str::<Value>(raw).unwrap()
            );
        }
    }
    #[test]
    fn numbers_match_native_values_and_serialization_under_any_features() {
        for raw in [
            "3.50",
            "1e2",
            "-0.0",
            "18446744073709551616",
            "1e400",
            r#"{"n":3.50,"nested":[1e2,{"n":-0.0}]}"#,
            r#"{"$serde_json::private::Number":"3.50","n":3.50}"#,
        ] {
            let native = serde_json::from_str::<Value>(raw);
            let checked = parse_unique_json(raw);
            match (native, checked) {
                (Ok(native), Ok(checked)) => {
                    assert_eq!(checked, native, "{raw}");
                    assert_eq!(checked.to_string(), native.to_string(), "{raw}");
                }
                (Err(_), Err(_)) => (),
                result => panic!("native/checked mismatch for {raw}: {result:?}"),
            }
        }
        for raw in [
            r#"{"n":3.50,"n":3.50}"#,
            r#"{"n":[3.50,{"a":1,"\u0061":2}]}"#,
            r#"{"$serde_json::private::Number":"3.5","$serde_json::private::Number":"3.5"}"#,
            "3.50 true",
            " ",
            "[3.5,]",
        ] {
            assert!(parse_unique_json(raw).is_err(), "{raw}");
        }
        let number: f64 = parse_unique_json_as("3.50").unwrap();
        assert_eq!(number, 3.5);
        assert!(parse_unique_json_as::<Value>(r#"{"a":1,"a":2}"#).is_err());
    }
    #[test]
    fn sql_predicate_is_total_and_uses_the_shared_parser() {
        let conn = Connection::open_in_memory().unwrap();
        register(&conn).unwrap();
        for (raw, expected) in [
            (None, false),
            (Some("not json"), false),
            (Some(r#"{"x":[{"a":1,"a":1}]}"#), false),
            (Some(r#"{"x":[],"y":{}}"#), true),
            (Some(r#"{"n":3.50,"nested":[1e2,{"n":-0.0}]}"#), true),
            (Some(r#"{"n":3.50,"nested":[{"a":1,"\u0061":2}]}"#), false),
            (Some("3.50 true"), false),
        ] {
            assert_eq!(
                conn.query_row("SELECT fb_json_unique_keys(?1)", [raw], |r| r
                    .get::<_, bool>(0))
                    .unwrap(),
                expected
            );
        }
    }
}
