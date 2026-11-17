//! Pure source-evidence checks shared by SQL guards and the invoice importer.
use super::*;
use sha2::{Digest, Sha256};

fn bounded(v: &Value, depth: usize, count: &mut usize) -> bool {
    *count += 1;
    if depth > 12 || *count > 4096 {
        return false;
    }
    match v {
        Value::String(s) => s.len() <= 16_384,
        Value::Array(a) => a.len() <= 100 && a.iter().all(|v| bounded(v, depth + 1, count)),
        Value::Object(o) => {
            o.len() <= 100
                && o.iter()
                    .all(|(k, v)| k.len() <= 100 && bounded(v, depth + 1, count))
        }
        _ => true,
    }
}
fn canonical(v: &Value, out: &mut String) {
    match v {
        Value::Object(o) => {
            out.push('{');
            let mut keys = o.keys().collect::<Vec<_>>();
            keys.sort();
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).unwrap());
                out.push(':');
                canonical(&o[*k], out);
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, v) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(v, out);
            }
            out.push(']');
        }
        _ => out.push_str(&v.to_string()),
    }
}
/// Hash exactly the retained source and mapped invoice, independent of object key order.
pub fn evidence_digest(v: &Value) -> Option<String> {
    if !v["source"].is_object()
        || !v["invoice"].is_object()
        || v.to_string().len() > 262144
        || !bounded(v, 0, &mut 0)
    {
        return None;
    }
    let mut text = String::new();
    canonical(
        &json!({"source":v["source"],"invoice":v["invoice"]}),
        &mut text,
    );
    Some(format!("{:x}", Sha256::digest(text.as_bytes())))
}
fn number(v: &Value) -> Option<Value> {
    match v {
        Value::String(s) => Some(json!(s)),
        Value::Number(n) => Some(json!(n.to_string())),
        _ => None,
    }
}
fn signed(v: &Value) -> Option<i64> {
    let s = number(v)?;
    let s = s.as_str()?;
    let (sign, s) = if let Some(s) = s.strip_prefix('-') {
        (-1, s)
    } else {
        (1, s)
    };
    cents(&json!(s)).ok().map(|v| sign * v)
}
fn qty(v: &Value) -> Option<rust_decimal::Decimal> {
    calculations::decimal(&number(v)?, 4, rust_decimal::Decimal::MAX, false).ok()
}
fn flag(v: &Value) -> Option<bool> {
    v.as_bool()
        .or_else(|| v.as_i64().filter(|n| *n == 0 || *n == 1).map(|n| n == 1))
}
fn stripped(v: &Value) -> Option<&str> {
    v.as_str().map(py::py_strip)
}
pub(super) fn matches(v: &Value, h: &Value, lines: &[Value]) -> bool {
    let check = || -> Option<()> {
        evidence_digest(v)?;
        let s = &v["source"];
        let input = &v["invoice"];
        if s["issue_date"] != h["date"] || s["due_date"] != h["due_date"] {
            return None;
        }
        let state = s["status"].as_str()?;
        let payments = s["payments"].as_array()?;
        if payments.len() > 50 {
            return None;
        }
        let paid = payments.iter().try_fold(0i64, |sum, p| {
            let n = signed(&p["amount"])?;
            if n <= 0 {
                return None;
            }
            sum.checked_add(n)
        })?;
        let total = signed(&s["total"])?;
        if paid != signed(&s["amount_paid"])?
            || paid > total
            || !match state {
                "paid" => paid == total,
                "partial" => paid > 0 && paid < total,
                "sent" | "draft" | "void" => paid == 0,
                _ => false,
            }
        {
            return None;
        }

        let original = s["lines"].as_array()?;
        let mapped = input["lines"].as_array()?;
        if original.len() != lines.len()
            || mapped.len() != lines.len()
            || !(1..=50).contains(&lines.len())
        {
            return None;
        }
        if stripped(&input["memo"]) != stripped(&s["notes"])
            || stripped(&input["memo"]) != stripped(&h["memo"])
        {
            return None;
        }
        let rate = calculations::decimal(
            &number(&s["tax_rate"])?,
            4,
            rust_decimal::Decimal::ONE_HUNDRED,
            false,
        )
        .ok()?;
        if rate
            != calculations::decimal(&h["tax_rate"], 4, rust_decimal::Decimal::ONE_HUNDRED, false)
                .ok()?
        {
            return None;
        }
        for ((l, s), m) in lines.iter().zip(original).zip(mapped) {
            let kind = calculations::invoice_kind(l).ok()?;
            let src = signed(&s["amount"])?;
            let rate = signed(&s["unit_price"])?;
            let expected = if src < 0 || rate < 0 {
                "DISCOUNT"
            } else if qty(&s["quantity"])?.is_zero() && rate == 0 {
                "NOTE"
            } else {
                "CHARGE"
            };
            if kind != expected
                || calculations::invoice_kind(m).ok()? != kind
                || stripped(&l["description"]) != stripped(&s["description"])
                || stripped(&l["description"]) != stripped(&m["description"])
                || qty(&l["quantity"])? != qty(&s["quantity"])?
                || qty(&l["quantity"])? != qty(&m["quantity"])?
                || cents(&l["rate"]).ok()? != rate.abs()
                || cents(&l["rate"]).ok()? != cents(&m["rate"]).ok()?
                || flag(&s["taxable"]) != l["taxable"].as_bool()
                || m["taxable"] != l["taxable"]
                || l["item_id"] != m["item_id"]
                || l["income_account_id"] != m["income_account_id"]
            {
                return None;
            }
        }
        Some(())
    };
    check().is_some()
}
