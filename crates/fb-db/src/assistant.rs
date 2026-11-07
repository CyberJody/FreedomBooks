//! Pure receipt-evidence predicates; no connection or mutable state is captured.
use rust_decimal::Decimal;
use serde_json::Value;

pub const MAX: i128 = 9_999_999_999_999_999;
/// Derived receipt evidence can exceed the document input cap; human Save still enforces it.
pub const EVIDENCE_MAX: i128 = 9_999_999_999_999_999_999_999_999_999;
pub fn scaled(v: &Value, places: u32, max: i128, positive: bool) -> Option<i128> {
    let text = v.as_str()?;
    let d = Decimal::from_str_exact(text).ok()?;
    let factor = Decimal::from(10u64.checked_pow(places)?);
    let n = d.checked_mul(factor)?.normalize();
    (n.scale() == 0 && n.mantissa() >= i128::from(positive) && n.mantissa() <= max)
        .then_some(n.mantissa())
}
pub fn product(q: i128, r: i128) -> Option<i128> {
    let n = q.checked_mul(r)?;
    let base = n / 10_000;
    let rem = n % 10_000;
    base.checked_add(i128::from(rem > 5000 || (rem == 5000 && base % 2 != 0)))
}
fn string(v: &Value, max: usize) -> bool {
    v.as_str().is_some_and(|s| s.chars().count() <= max)
}
pub fn choices(v: &Value) -> bool {
    v.as_array().is_some_and(|a| {
        a.len() <= 1000
            && a.iter().all(|c| {
                c.as_object().is_some_and(|m| m.len() == 2)
                    && c["slug"]
                        .as_str()
                        .is_some_and(|s| !s.is_empty() && s.len() <= 100)
                    && string(&c["display_name"], 200)
            })
    })
}
/// A picker selection is empty exactly when the provider offers no model choices.
pub fn selection(model: &str, model_choices: &Value) -> bool {
    choices(model_choices)
        && model_choices.as_array().is_some_and(|items| {
            if items.is_empty() {
                model.is_empty()
            } else {
                items.iter().any(|item| item["slug"] == model)
            }
        })
}
pub fn result(v: &Value) -> bool {
    let fields = [
        "vendor_name",
        "vendor_address",
        "document_number",
        "document_date",
        "due_date",
        "currency",
        "subtotal",
        "tax",
        "total",
        "lines",
        "notes",
        "warnings",
    ];
    if v.as_object()
        .is_none_or(|m| m.len() != fields.len() || fields.iter().any(|k| !m.contains_key(*k)))
    {
        return false;
    }
    let date = |k: &str| {
        v[k].as_str()
            .is_some_and(|s| s.is_empty() || crate::banking_evidence::calendar_date(s))
    };
    string(&v["vendor_name"], 200)
        && string(&v["document_number"], 120)
        && v["vendor_address"]
            .as_array()
            .is_some_and(|a| a.len() <= 5 && a.iter().all(|s| string(s, 200)))
        && date("document_date")
        && date("due_date")
        && v["currency"]
            .as_str()
            .is_some_and(|s| s.len() == 3 && s.bytes().all(|b| b.is_ascii_uppercase()))
        && ["tax", "total"]
            .iter()
            .all(|k| scaled(&v[*k], 2, MAX, false).is_some())
        && scaled(&v["subtotal"], 2, EVIDENCE_MAX, false).is_some()
        && ["notes", "warnings"].iter().all(|k| {
            v[*k]
                .as_array()
                .is_some_and(|a| a.len() <= 200 && a.iter().all(|s| string(s, 2000)))
        })
        && v["lines"].as_array().is_some_and(|a| {
            (1..=50).contains(&a.len())
                && a.iter().all(|l| {
                    let Some(m) = l.as_object() else { return false };
                    let Some(q) = scaled(&l["quantity"], 4, 999_999_999_999_999_999, true) else {
                        return false;
                    };
                    let Some(r) = scaled(&l["rate"], 2, MAX, false) else {
                        return false;
                    };
                    let Some(amount) = scaled(&l["amount"], 2, EVIDENCE_MAX, false) else {
                        return false;
                    };
                    let Some(p) = product(q, r) else { return false };
                    m.keys().all(|k| {
                        [
                            "description",
                            "quantity",
                            "rate",
                            "amount",
                            "mismatch",
                            "tax",
                        ]
                        .contains(&k.as_str())
                    }) && string(&l["description"], 1000)
                        && m.get("mismatch").is_none_or(Value::is_boolean)
                        && m.get("tax").is_none_or(Value::is_boolean)
                        && l.get("mismatch").and_then(Value::as_bool).unwrap_or(false)
                            == (p != amount)
                })
        })
}
pub(crate) fn register(c: &rusqlite::Connection) -> rusqlite::Result<()> {
    use rusqlite::functions::FunctionFlags as F;
    let flags = F::SQLITE_UTF8 | F::SQLITE_DETERMINISTIC | F::SQLITE_INNOCUOUS;
    c.create_scalar_function("fb_assistant_model", 2, flags, |cx| {
        let model = cx.get::<String>(0)?;
        let raw = cx.get::<String>(1)?;
        Ok(crate::parse_unique_json(&raw)
            .ok()
            .is_some_and(|v| selection(&model, &v)))
    })?;
    c.create_scalar_function("fb_assistant_json", 2, flags, |cx| {
        let kind = cx.get::<String>(0)?;
        let raw = cx.get::<String>(1)?;
        Ok(crate::parse_unique_json(&raw)
            .ok()
            .is_some_and(|v| match kind.as_str() {
                "result" => result(&v),
                "choices" => choices(&v),
                _ => false,
            }))
    })
}
