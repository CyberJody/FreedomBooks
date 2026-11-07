//! Manual banking, translated from the frozen banking app. No provider/network work.
mod cleared;
mod deposits;
mod reconcile;
mod register;
use crate::{
    AccountId, Book, CompanyId, FbError, FbResult, Money, UserId, documents, py, sales, services,
    store,
};
pub use cleared::{
    is_register_account, refuse_cleared_reversal, reverse_journal, set_cleared, set_cleared_tx,
};
pub use deposits::{
    deposit, deposit_status, record_deposit, record_deposit_tx, refuse_live_deposit, release_dates,
    reverse_deposit, review_deposit, review_deposit_tx,
};
use fb_db::rusqlite::{OptionalExtension, params};
use fb_db::{Query, Writer};
pub use reconcile::{
    begin, figures, reconcile_now, reconciliation, report, save_ticks, screen, undo_last, words,
};
pub use register::{build, build_on, register_accounts, source_metadata};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub struct Review {
    digest: String,
    pub display: Value,
}
fn command<T>(book: &mut Book, f: impl FnOnce(&Writer<'_>) -> FbResult<T>) -> FbResult<T> {
    book.write(|tx| tx.atomic(|| f(tx)))
}
fn new_id() -> String {
    fb_ids::new_v7().to_string()
}
fn digest(v: &Value) -> String {
    format!(
        "{:x}",
        Sha256::digest(py::py_json_dumps_sorted(v).as_bytes())
    )
}
fn s<'a>(v: &'a Value, k: &str) -> FbResult<&'a str> {
    v[k].as_str()
        .ok_or_else(|| FbError::validation(format!("Invalid {k}.")))
}
fn text(v: &Value, max: usize, required: bool) -> FbResult<String> {
    if let Some(raw) = v.as_str() { py::clean_free_text(raw)?; }
    v.as_str()
        .map(py::py_strip)
        .filter(|s| py::py_len(s) <= max && (!required || !s.is_empty()))
        .map(str::to_owned)
        .ok_or_else(|| {
            FbError::validation(format!(
                "Enter {}text up to {max} characters.",
                if required { "nonblank " } else { "" }
            ))
        })
}
fn field(v: &Value, k: &str, max: usize) -> FbResult<String> {
    text(v.get(k).unwrap_or(&json!("")), max, false)
}
fn id(v: &Value) -> FbResult<String> {
    v.as_str()
        .filter(|s| AccountId::parse(s).is_some())
        .map(str::to_owned)
        .ok_or_else(|| FbError::validation("Choose a record from this company."))
}
fn uuid(v: &Value) -> FbResult<String> {
    v.as_str()
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .map(|u| u.to_string())
        .ok_or_else(|| FbError::validation("Supply a stable UUID request key."))
}
fn date(v: &Value) -> FbResult<String> {
    v.as_str().and_then(fb_db::dates::parse_iso_date)
        .ok_or_else(|| FbError::validation("Enter a valid calendar date."))
}
fn statement_date(v: &Value) -> FbResult<String> {
    let d = v
        .as_str()
        .and_then(|s| py::form_date(s).ok())
        .map(|d| d.to_string())
        .ok_or_else(|| FbError::validation("Enter the statement date as YYYY-MM-DD."))?;
    if !("1900-01-02"..="9999-12-30").contains(&d.as_str()) {
        return Err(FbError::validation(
            "Use a statement date between 1900-01-02 and 9999-12-30.",
        ));
    }
    Ok(d)
}
fn cents(v: &Value) -> FbResult<i64> {
    let raw = if let Some(s) = v.as_str() {
        s.to_owned()
    } else if v.is_i64() || v.is_u64() {
        v.to_string()
    } else {
        return Err(FbError::validation("Enter a valid monetary amount."));
    };
    let invalid = || FbError::validation("Document amount exceeds 99999999999999.99.");
    let Some(fb_money::Parsed::Finite(mut n)) = fb_money::parse_python_decimal(&raw) else {
        return Err(FbError::validation("Enter a valid monetary amount."));
    };
    if n.is_zero() {
        return Ok(0);
    }
    while n.digits.ends_with('0') {
        n.digits.pop();
        n.exponent = n.exponent.checked_add(1).ok_or_else(invalid)?;
    }
    let digits = n.digits.trim_start_matches('0');
    let shift = n.exponent.checked_add(2).ok_or_else(invalid)?;
    if shift < 0
        || i128::try_from(digits.len())
            .ok()
            .and_then(|len| len.checked_add(shift))
            .is_none_or(|len| len > 16)
    {
        return Err(invalid());
    }
    let power = u32::try_from(shift)
        .ok()
        .and_then(|p| 10i64.checked_pow(p))
        .ok_or_else(invalid)?;
    let amount = digits
        .parse::<i64>()
        .ok()
        .and_then(|n| n.checked_mul(power))
        .filter(|n| *n <= 9_999_999_999_999_999)
        .ok_or_else(invalid)?;
    Ok(if n.negative { -amount } else { amount })
}
fn cash(n: i64) -> Value {
    json!(Money::from_db(n).to_string())
}
fn add(a: i64, b: i64) -> FbResult<i64> {
    a.checked_add(b)
        .ok_or_else(|| FbError::validation("Banking amount exceeds supported limits."))
}
fn signed(n: i64, sign: i64) -> FbResult<i64> {
    n.checked_mul(sign)
        .ok_or_else(|| FbError::validation("Banking amount exceeds supported limits."))
}
fn account(q: &(impl Query + ?Sized), c: CompanyId, key: &str) -> FbResult<crate::model::Account> {
    AccountId::parse(key)
        .map(|a| store::account(q, a))
        .transpose()?
        .flatten()
        .filter(|a| a.company_id == c)
        .ok_or_else(|| FbError::validation("Posting accounts must belong to this company."))
}
fn stored(kind: &str, raw: &str) -> FbResult<Value> {
    let v: Value = fb_db::parse_unique_json(raw)
        .map_err(|_| FbError::validation("Invalid banking evidence."))?;
    if !fb_db::banking_evidence::payload(kind, &v) {
        return Err(FbError::validation("Invalid banking evidence."));
    }
    Ok(v)
}
fn boolean(v: &Value, key: &str) -> FbResult<bool> {
    v[key]
        .as_bool()
        .ok_or_else(|| FbError::validation(format!("Invalid {key}.")))
}
fn aggregate_cents(v: &Value) -> FbResult<i64> {
    fb_db::banking_evidence::money(v, false)
        .ok_or_else(|| FbError::validation("Invalid aggregate banking amount."))
}
fn values(q: &(impl Query + ?Sized), kind: &str, sql: &str, key: &str) -> FbResult<Vec<Value>> {
    let mut st = q.prepare(sql)?;
    st.query_map([key], |r| r.get::<_, String>(0))?
        .map(|r| {
            let raw = r?;
            stored(kind, &raw)
        })
        .collect()
}
fn seal(tx: &Writer<'_>, kind: &str, key: &str) -> FbResult<()> {
    tx.execute(
        "INSERT INTO banking_seal(kind,doc_id) VALUES(?1,?2)",
        params![kind, key],
    )?;
    Ok(())
}
#[cfg(test)]
mod tests;
