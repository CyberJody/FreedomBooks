//! Payables SQL predicates registered before migrations and callback authorization.
use rusqlite::{Connection, Result, functions::FunctionFlags};

pub(crate) fn register(conn: &Connection) -> Result<()> {
    conn.create_scalar_function(
        "fb_recurring_data_valid",
        2,
        FunctionFlags::SQLITE_UTF8
            | FunctionFlags::SQLITE_DETERMINISTIC
            | FunctionFlags::SQLITE_INNOCUOUS,
        |ctx| {
            Ok(recurring_data_valid(
                &ctx.get::<String>(0)?,
                ctx.get::<i64>(1)? == 1,
            ))
        },
    )?;
    conn.create_scalar_function(
        "fb_payables_line_valid",
        3,
        FunctionFlags::SQLITE_UTF8
            | FunctionFlags::SQLITE_DETERMINISTIC
            | FunctionFlags::SQLITE_INNOCUOUS,
        |ctx| {
            let quantity = ctx.get::<i64>(0)?;
            let rate = ctx.get::<i64>(1)?;
            let amount = ctx.get::<i64>(2)?;
            // Quantity is ten-thousandths; rate and amount are whole cents.
            // Positive integer arithmetic gives exact HALF_UP, without SQL floats
            // or overflowing Decimal's 96-bit intermediate for extreme inputs.
            Ok((1..=999_999_999_999_999_999).contains(&quantity)
                && (0..=9_999_999_999_999_999).contains(&rate)
                && (0..=9_999_999_999_999_999).contains(&amount)
                && (i128::from(quantity) * i128::from(rate) + 5_000) / 10_000 == i128::from(amount))
        },
    )
}

/// Validate stored template content independently of its service reader, including exact arithmetic.
fn recurring_data_valid(raw: &str, auto_post: bool) -> bool {
    use crate::validators;
    use serde_json::{Value, json};
    if raw.len() > 1_048_576 {
        return false;
    }
    let Ok(v) = crate::parse_unique_json(raw) else {
        return false;
    };
    let valid_text = |v: &Value, max: usize, required: bool| {
        v.as_str().is_some_and(|s| {
            validators::text(s) && s.chars().count() <= max && (!required || !s.trim().is_empty())
        })
    };
    let uuid = |v: &Value| v.as_str().is_some_and(validators::uuid);
    if v.as_object().is_none_or(|o| o.len() != 3)
        || !uuid(&v["vendor_id"])
        || !valid_text(&v["memo"], 2000, false)
    {
        return false;
    }
    let Some(lines) = v["lines"]
        .as_array()
        .filter(|ls| (1..=50).contains(&ls.len()))
    else {
        return false;
    };
    for l in lines {
        if l.as_object().is_none_or(|o| {
            o.len() != 5
                || [
                    "item_id",
                    "expense_account_id",
                    "description",
                    "quantity",
                    "rate",
                ]
                .iter()
                .any(|k| !o.contains_key(*k))
        }) || !valid_text(&l["description"], 1000, true)
            || !l["quantity"].is_string()
            || !l["rate"].is_string()
            || (!l["item_id"].is_null() && !uuid(&l["item_id"]))
            || (!l["expense_account_id"].is_null() && !uuid(&l["expense_account_id"]))
            || (auto_post && l["expense_account_id"].is_null())
        {
            return false;
        }
    }
    let lines = lines
        .iter()
        .map(|l| {
            let mut l = l.clone();
            l["taxable"] = json!(false);
            l
        })
        .collect::<Vec<_>>();
    let Some(total) = crate::sales::recurring_bill_total(&lines) else {
        return false;
    };
    !auto_post || total != "0.00"
}
