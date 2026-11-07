//! Pure exact-cent predicates. They capture no connection, hook or mutable state.
use rusqlite::{Connection, Result, functions::FunctionFlags};
pub(crate) fn register(conn: &Connection) -> Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8
        | FunctionFlags::SQLITE_DETERMINISTIC
        | FunctionFlags::SQLITE_INNOCUOUS;
    conn.create_scalar_function("fb_banking_payload", 2, flags, |ctx| {
        let kind = ctx.get::<String>(0)?;
        let raw = ctx.get::<String>(1)?;
        Ok(crate::parse_unique_json(&raw)
            .ok()
            .is_some_and(|v| crate::banking_evidence::payload(&kind, &v)))
    })?;
    conn.create_scalar_function("fb_banking_date", 1, flags, |ctx| {
        Ok(ctx
            .get::<Option<String>>(0)?
            .is_some_and(|s| crate::banking_evidence::calendar_date(&s)))
    })?;
    conn.create_scalar_function("fb_banking_uuid", 1, flags, |ctx| {
        Ok(ctx
            .get::<Option<String>>(0)?
            .is_some_and(|s| crate::banking_evidence::canonical_uuid(&s)))
    })?;
    conn.create_scalar_function("fb_banking_money", 1, flags, |ctx| {
        let raw = ctx.get::<String>(0)?;
        let parsed = crate::parse_unique_json(&raw)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned));
        Ok(parsed
            .and_then(|s| rust_decimal::Decimal::from_str_exact(&s).ok())
            .and_then(fb_money::Money::from_decimal_exact)
            .filter(|m| m.cents().unsigned_abs() <= 9_999_999_999_999_999)
            .and_then(|m| m.to_db()))
    })?;
    conn.create_scalar_function("fb_banking_sum", 1, flags, |ctx| {
        let raw = ctx.get::<String>(0)?;
        let value: serde_json::Value = crate::json_evidence::parse_unique_json_as(&raw)
            .map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))?;
        let result = value
            .as_array()
            .and_then(|a| a.iter().try_fold(0i64, |n, v| n.checked_add(v.as_i64()?)))
            .filter(|n| n.unsigned_abs() <= 9_999_999_999_999_999);
        result.ok_or_else(|| {
            rusqlite::Error::UserFunctionError("Banking amount exceeds supported limits".into())
        })
    })?;
    conn.create_scalar_function("fb_banking_balance", 4, flags, |ctx| {
        let begin = ctx.get::<i64>(0)?;
        let debit = ctx.get::<i64>(1)?;
        let credit = ctx.get::<i64>(2)?;
        let sign = ctx.get::<i64>(3)?;
        begin
            .checked_add(
                debit
                    .checked_sub(credit)
                    .and_then(|n| n.checked_mul(sign))
                    .ok_or_else(|| {
                        rusqlite::Error::UserFunctionError(
                            "Banking amount exceeds supported limits".into(),
                        )
                    })?,
            )
            .filter(|n| n.unsigned_abs() <= 9_999_999_999_999_999)
            .ok_or_else(|| {
                rusqlite::Error::UserFunctionError("Banking amount exceeds supported limits".into())
            })
    })
}
