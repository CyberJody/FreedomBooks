//! Strict JSON: duplicate object keys are refused at every depth before the value is typed.
//! The remote policy reads commands as JSON; fb-app parses the same text with its own unique-key
//! parser. Refusing duplicates here too means the policy and the book can never read different
//! values out of one ambiguous document.
use serde::de::{self, DeserializeOwned, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use std::fmt;

struct Unique(Value);

impl<'de> de::Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(UniqueVisitor)
    }
}

struct UniqueVisitor;

impl<'de> Visitor<'de> for UniqueVisitor {
    type Value = Unique;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("JSON")
    }
    fn visit_bool<E>(self, v: bool) -> Result<Unique, E> {
        Ok(Unique(Value::Bool(v)))
    }
    fn visit_i64<E>(self, v: i64) -> Result<Unique, E> {
        Ok(Unique(Value::from(v)))
    }
    fn visit_u64<E>(self, v: u64) -> Result<Unique, E> {
        Ok(Unique(Value::from(v)))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Unique, E> {
        serde_json::Number::from_f64(v).map(|n| Unique(Value::Number(n))).ok_or_else(|| E::custom("invalid number"))
    }
    fn visit_str<E>(self, v: &str) -> Result<Unique, E> {
        Ok(Unique(Value::String(v.to_owned())))
    }
    fn visit_string<E>(self, v: String) -> Result<Unique, E> {
        Ok(Unique(Value::String(v)))
    }
    fn visit_unit<E>(self) -> Result<Unique, E> {
        Ok(Unique(Value::Null))
    }
    fn visit_none<E>(self) -> Result<Unique, E> {
        Ok(Unique(Value::Null))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Unique, A::Error> {
        let mut out = Vec::new();
        while let Some(Unique(v)) = seq.next_element()? {
            out.push(v);
        }
        Ok(Unique(Value::Array(out)))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Unique, A::Error> {
        let mut out = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if out.contains_key(&key) {
                return Err(de::Error::custom(format!("duplicate key {key:?}")));
            }
            let Unique(v) = map.next_value()?;
            out.insert(key, v);
        }
        Ok(Unique(Value::Object(out)))
    }
}

/// Parse `text` as JSON with unique keys, then as `T`.
pub fn parse<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    let Unique(value) = serde_json::from_str::<Unique>(text).map_err(|e| e.to_string())?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    #[test]
    fn duplicate_keys_are_refused_at_any_depth() {
        assert!(super::parse::<Value>(r#"{"a":1,"a":2}"#).is_err());
        assert!(super::parse::<Value>(r#"{"a":{"b":1,"b":1}}"#).is_err());
        assert!(super::parse::<Value>(r#"[{"x":1},{"x":1,"x":2}]"#).is_err());
        assert_eq!(super::parse::<Value>(r#"{"a":[1,"b",null,true]}"#).unwrap()["a"][1], "b");
    }
}
