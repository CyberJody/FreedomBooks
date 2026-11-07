//! Payables SQL predicates registered before migrations and callback authorization.
use rusqlite::{Connection, Result, functions::FunctionFlags};

pub(crate) fn register(conn: &Connection) -> Result<()> {
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
