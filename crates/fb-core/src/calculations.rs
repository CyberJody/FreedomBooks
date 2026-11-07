//! 1:1 document arithmetic (`documents/calculations.py`). No SQL or binary float math.
use crate::{FbError, FbResult, Money};
use fb_money::{AmountInput, MAX_TOTAL_CENTS, Parsed};
use rust_decimal::Decimal;
use serde_json::{Value, json};

pub fn decimal(value: &Value, places: u32, maximum: Decimal, positive: bool) -> FbResult<Decimal> {
    let invalid = || {
        FbError::validation(format!(
            "Enter a finite {} value with at most {places} decimal places within the supported limit.",
            if positive { "positive" } else { "nonnegative" }
        ))
    };
    if places > Decimal::MAX_SCALE {
        return Err(invalid());
    }
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) if n.is_i64() || n.is_u64() => n.to_string(),
        _ => return Err(invalid()),
    };
    let Some(Parsed::Finite(mut raw)) = fb_money::parse_python_decimal(&text) else {
        return Err(invalid());
    };
    if raw.is_zero() {
        return if positive {
            Err(invalid())
        } else {
            Ok(Decimal::from_i128_with_scale(0, places))
        };
    }
    if raw.negative {
        return Err(invalid());
    }
    while raw.digits.ends_with('0') {
        raw.digits.pop();
        raw.exponent += 1;
    }
    let digits = raw.digits.trim_start_matches('0');
    let shift = raw.exponent + i128::from(places);
    if shift < 0 || digits.len() as i128 + shift > 28 {
        return Err(invalid());
    }
    let mantissa = digits
        .parse::<i128>()
        .map_err(|_| invalid())?
        .checked_mul(10i128.checked_pow(shift as u32).ok_or_else(invalid)?)
        .ok_or_else(invalid)?;
    let result = Decimal::from_i128_with_scale(mantissa, places);
    if result > maximum {
        return Err(invalid());
    }
    Ok(result)
}
pub fn quantity(value: &Value) -> FbResult<Decimal> {
    decimal(value, 4, Decimal::from_i128_with_scale(999_999_999_999_999_999, 4), true)
}
pub fn unit_rate(value: &Value) -> FbResult<Decimal> {
    decimal(value, 2, Money::from_cents(MAX_TOTAL_CENTS).to_decimal(), false)
}
fn amount(cents: i128) -> FbResult<Money> {
    if cents > MAX_TOTAL_CENTS {
        Err(FbError::validation("Document amount exceeds 99999999999999.99."))
    } else {
        Ok(Money::from_cents(cents))
    }
}
pub fn line_amount(qty: &Value, rate: &Value) -> FbResult<Money> {
    // Decimal's 96-bit intermediate cannot hold the largest permitted quantity * unit rate.
    // Both normalized mantissas fit i128; this exact fixed-point multiplication cannot overflow it.
    let q = quantity(qty)?;
    let r = unit_rate(rate)?;
    amount((q.mantissa() * r.mantissa() + 5_000) / 10_000)
}
/// Accept JSON at the boundary to preserve Python's strict bool/float/type refusals.
/// Return decimal money as strings, as the Python harness JSON encoder does.
pub fn totals(
    lines: &[Value],
    tax_mode: &str,
    tax_rate: Option<&Value>,
    tax_label: &Value,
    tax_snapshot: Option<&Value>,
    for_posting: bool,
) -> FbResult<Value> {
    let label = tax_label
        .as_str()
        .filter(|s| crate::py::py_len(crate::py::py_strip(s)) <= 120)
        .ok_or_else(|| FbError::validation("Tax label must be text of at most 120 characters."))?;
    let zero = json!("0");
    let null = Value::Null;
    let hundred = Decimal::ONE_HUNDRED;
    let mut rates = Vec::new();
    let mut members: Option<&Vec<Value>> = None;
    let rate: Decimal;
    let output_label: String;
    match tax_mode {
        "NONE" => {
            rate = decimal(tax_rate.unwrap_or(&zero), 4, hundred, false)?;
            if !rate.is_zero() {
                return Err(FbError::validation("No tax requires a zero or absent rate."));
            }
            output_label = "No tax".into();
        }
        "MANUAL" => {
            rate = decimal(tax_rate.unwrap_or(&null), 4, hundred, false)?;
            output_label = crate::py::py_strip(label).to_string();
            if output_label.is_empty() {
                return Err(FbError::validation("Enter a label for the manual combined tax."));
            }
        }
        "RATE" | "GROUP" => {
            let snap = tax_snapshot
                .filter(|s| s.is_object() && s["kind"] == tax_mode && s["members"].is_array())
                .ok_or_else(|| FbError::validation("Choose a sales tax rate or group from this company."))?;
            let rows = snap["members"].as_array().unwrap();
            if (tax_mode == "RATE" && rows.len() != 1) || (tax_mode == "GROUP" && !(2..=20).contains(&rows.len())) {
                return Err(FbError::validation("A tax group needs at least two tax rates."));
            }
            for m in rows {
                if !m.is_object() {
                    return Err(FbError::validation("Invalid sales tax snapshot."));
                }
                rates.push(decimal(&m["rate"], 4, hundred, false)?);
            }
            rate = rates.iter().copied().sum();
            if rate > hundred {
                return Err(FbError::validation("A tax group cannot add up to more than 100%."));
            }
            output_label = snap["description"]
                .as_str()
                .filter(|s| !crate::py::py_strip(s).is_empty() && crate::py::py_len(s) <= 120 && *s == crate::py::py_strip(s))
                .ok_or_else(|| FbError::validation("Invalid sales tax snapshot description."))?
                .to_string();
            members = Some(rows);
        }
        _ => return Err(FbError::validation("Explicitly choose No tax or a sales tax rate.")),
    }
    let mut subtotal = Money::ZERO;
    let mut taxable = Money::ZERO;
    let mut amounts = Vec::new();
    for l in lines {
        let flag = l["taxable"]
            .as_bool()
            .ok_or_else(|| FbError::validation("Each line requires an explicit taxable flag."))?;
        let a = line_amount(&l["quantity"], &l["rate"])?;
        subtotal = amount((subtotal + a).cents())?;
        if flag {
            taxable = amount((taxable + a).cents())?;
        }
        amounts.push(a.to_string());
    }
    // rate is normalized to four places: cents * percent_mantissa / 1,000,000.
    let share = |r: Decimal| -> FbResult<Money> { amount((taxable.cents() * r.mantissa() + 500_000) / 1_000_000) };
    let mut snapshot = json!({});
    let tax = if let Some(rows) = members {
        let shares = rates.iter().map(|r| share(*r)).collect::<FbResult<Vec<_>>>()?;
        let tax = amount(shares.iter().sum::<Money>().cents())?;
        snapshot = tax_snapshot.unwrap().clone();
        snapshot["rate"] = json!(rate.to_string());
        snapshot["taxable"] = json!(taxable.to_string());
        snapshot["tax"] = json!(tax.to_string());
        snapshot["members"] = Value::Array(
            rows.iter()
                .zip(rates.iter().zip(shares))
                .map(|(m, (r, s))| {
                    let mut m = m.clone();
                    m["rate"] = json!(r.to_string());
                    m["tax"] = json!(s.to_string());
                    m
                })
                .collect(),
        );
        tax
    } else {
        share(rate)?
    };
    let total = amount((subtotal + tax).cents())?;
    if for_posting && total.is_zero() {
        return Err(FbError::validation("A zero-total document cannot be posted."));
    }
    Ok(
        json!({"lines":amounts,"subtotal":subtotal.to_string(),"taxable":taxable.to_string(),"tax":tax.to_string(),"total":total.to_string(),"tax_mode":tax_mode,"tax_rate":rate.to_string(),"tax_label":output_label,"tax_snapshot":snapshot}),
    )
}

pub fn money_input(value: &Value) -> AmountInput {
    match value {
        Value::String(s) => AmountInput::Text(s.clone()),
        Value::Number(n) if n.is_i64() => AmountInput::Int(n.as_i64().unwrap().into()),
        Value::Number(n) => AmountInput::Float(n.as_f64().unwrap_or(f64::NAN)),
        Value::Bool(b) => AmountInput::Bool(*b),
        _ => AmountInput::Other,
    }
}
