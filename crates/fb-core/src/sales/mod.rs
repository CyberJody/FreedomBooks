//! Sales backend translated from the frozen `sales` app. No UI, network or provider code.
//! All command entry points own an IMMEDIATE writer transaction; review tokens are opaque
//! in-process capabilities, not caller-supplied digests. Book open registers exact Rust
//! arithmetic before authorizing callbacks and checks the combined schema and row invariants.
mod estimates;
mod guards;
mod invoices;
mod queries;
mod receipts;
pub use estimates::{convert_estimate, estimate, save_estimate, transition_estimate};
pub use guards::{check, initialize};
pub use invoices::{post_invoice, review_invoice, save_invoice, void_invoice};
pub use queries::{customer_activity, invoice_balances};
pub use receipts::{
    record_receipt, record_sales_receipt, reverse_receipt, review_receipt, review_receipt_form,
    review_sales_receipt, void_sales_receipt,
};

use crate::{
    AccountId, Book, CompanyId, FbError, FbResult, Money, UserId, calculations,
    directory::{self, Model},
    documents, py, services, store,
};
#[cfg(test)]
use fb_db::rusqlite::Connection;
use fb_db::rusqlite::{OptionalExtension, params};
use fb_db::{Query, Writer};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// Cannot be constructed or deserialized by a caller. Bound to the precise plan and workflow.
#[derive(Clone, Debug)]
pub struct Review {
    domain: &'static str,
    digest: String,
    pub display: Value,
}
fn invalid_stored() -> FbError {
    FbError::validation("Invalid stored sales evidence. Run the book integrity check.")
}
fn validate_stored(
    conn: &(impl Query + ?Sized),
    c: CompanyId,
    kind: &str,
    v: &Value,
) -> FbResult<()> {
    let ok: bool = conn.query_row(
        "SELECT fb_sales_header_ok(?1,?2) AND json_extract(?2,'$.company_snapshot.id')=?3",
        params![kind, v.to_string(), c.to_db()],
        |r| r.get(0),
    )?;
    if !ok {
        return Err(invalid_stored());
    }
    Ok(())
}
fn digest(v: &Value) -> String {
    format!(
        "{:x}",
        Sha256::digest(py::py_json_dumps_sorted(v).as_bytes())
    )
}
fn text(v: &Value, max: usize, required: bool) -> FbResult<String> {
    if let Some(raw) = v.as_str() { py::clean_free_text(raw)?; }
    let s = v
        .as_str()
        .map(py::py_strip)
        .filter(|s| py::py_len(s) <= max && (!required || !s.is_empty()));
    s.map(str::to_owned).ok_or_else(|| {
        FbError::validation(format!(
            "Enter {}text up to {max} characters.",
            if required { "nonblank " } else { "" }
        ))
    })
}
fn field_text(v: &Value, k: &str, max: usize, required: bool) -> FbResult<String> {
    text(v.get(k).unwrap_or(&json!("")), max, required)
}
fn boolean(v: &Value) -> FbResult<bool> {
    v.as_bool()
        .ok_or_else(|| FbError::validation("Invalid checkbox value."))
}
fn id(v: &Value, optional: bool) -> FbResult<Option<String>> {
    if optional && v.is_null() {
        return Ok(None);
    }
    v.as_str()
        .filter(|s| fb_ids::CompanyId::parse(s).is_some())
        .map(|s| Some(s.into()))
        .ok_or_else(|| FbError::validation("Choose a record from this company."))
}
fn date(v: &Value) -> FbResult<String> {
    v.as_str().and_then(fb_db::dates::parse_iso_date).ok_or_else(|| FbError::validation("Enter a valid calendar date."))
}
fn cents(v: &Value) -> FbResult<i64> {
    Ok(Money::from_decimal_exact(calculations::unit_rate(v)?)
        .ok_or_else(|| FbError::validation("Document amount exceeds 99999999999999.99."))?
        .cents() as i64)
}
fn cash(n: i64) -> Value {
    json!(Money::from_db(n).to_string())
}
fn new_id() -> String {
    fb_ids::new_v7().to_string()
}
fn party(v: &Value) -> Value {
    let mut out = json!({});
    for k in [
        "id",
        "display_name",
        "business_name",
        "contact_name",
        "billing_address",
        "email",
        "phone",
    ] {
        out[k] = v[k].clone()
    }
    out
}
fn company_snapshot(conn: &(impl Query + ?Sized), c: CompanyId) -> FbResult<Value> {
    let co = store::get_company(conn, c)?;
    Ok(json!({"id":c.to_db(),"name":co.name,"currency":co.currency}))
}
fn account(
    conn: &(impl Query + ?Sized),
    c: CompanyId,
    key: &str,
) -> FbResult<crate::model::Account> {
    AccountId::parse(key)
        .map(|i| store::account(conn, i))
        .transpose()?
        .flatten()
        .filter(|a| a.company_id == c)
        .ok_or_else(|| FbError::validation("Historical accounts must belong to this company."))
}
fn account_snapshot(a: &crate::model::Account) -> Value {
    json!({"id":a.id.to_db(),"code":a.code,"name":a.name,"kind":a.kind.as_str()})
}
fn item_snapshot(v: &Value) -> Value {
    let mut out = json!({});
    for k in ["id", "code", "description", "kind", "sale_rate"] {
        out[k] = v[k].clone()
    }
    out
}
fn calculate(lines: &[Value], tax: &Value, posting: bool) -> FbResult<Value> {
    calculations::totals(
        lines,
        tax["tax_mode"].as_str().unwrap_or(""),
        tax.get("tax_rate").filter(|v| !v.is_null()),
        tax.get("tax_label").unwrap_or(&json!("")),
        tax.get("tax_snapshot").filter(|v| !v.is_null()),
        posting,
    )
}
fn copy_calc(target: &mut Value, calc: &Value) {
    for k in [
        "tax_mode",
        "tax_rate",
        "tax_label",
        "tax_snapshot",
        "subtotal",
        "taxable",
        "tax",
        "total",
    ] {
        target[k] = calc[k].clone()
    }
}
pub fn invoice(conn: &(impl Query + ?Sized), c: CompanyId, key: &str) -> FbResult<Value> {
    let mut v:Value=conn.query_row("SELECT i.id,i.identity_id,i.revision,i.creation_digest,r.data FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.number=i.revision WHERE i.company_id=?1 AND i.id=?2",params![c.to_db(),key],|r|Ok(json!({"id":r.get::<_,String>(0)?,"identity_id":r.get::<_,String>(1)?,"revision":r.get::<_,i64>(2)?,"creation_digest":r.get::<_,String>(3)?,"current":fb_db::parse_unique_json_as::<Value>(&r.get::<_,String>(4)?).unwrap_or(Value::Null)}))).optional()?.ok_or_else(||FbError::missing("Invoice"))?;
    validate_stored(conn, c, "invoice", &v["current"])?;
    let rid = v["current"]["id"]
        .as_str()
        .ok_or_else(|| FbError::validation("Invalid stored invoice revision."))?;
    let mut stmt =
        conn.prepare("SELECT data FROM sales_invoiceline WHERE revision_id=?1 ORDER BY position")?;
    v["lines"] = json!(
        stmt.query_map([rid], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .map(|s| fb_db::parse_unique_json_as::<Value>(s).unwrap_or(Value::Null))
            .collect::<Vec<_>>()
    );
    for line in v["lines"].as_array().ok_or_else(invalid_stored)? {
        if line.get("expense_account_id").is_none() {
            return Err(invalid_stored());
        }
        let ok: bool = conn.query_row("SELECT fb_sales_line_ok(?1)", [line.to_string()], |r| {
            r.get(0)
        })?;
        if !ok {
            return Err(invalid_stored());
        }
    }
    Ok(v)
}
pub fn receipt(conn: &(impl Query + ?Sized), c: CompanyId, key: &str) -> FbResult<Value> {
    let s: Option<String> = conn
        .query_row(
            "SELECT data FROM sales_receipt WHERE company_id=?1 AND id=?2",
            params![c.to_db(), key],
            |r| r.get(0),
        )
        .optional()?;
    let v = s
        .map(|s| fb_db::parse_unique_json_as(&s).map_err(|_| invalid_stored()))
        .transpose()?
        .ok_or_else(|| FbError::missing("Receipt"))?;
    validate_stored(conn, c, "receipt", &v)?;
    Ok(v)
}
fn mapping_display(conn: &(impl Query + ?Sized), c: CompanyId, m: &Value) -> FbResult<Value> {
    let mut keys = m
        .as_object()
        .ok_or_else(invalid_stored)?
        .keys()
        .collect::<Vec<_>>();
    keys.sort();
    Ok(json!(
        keys.into_iter()
            .map(|k| {
                let a = account(conn, c, m[k].as_str().ok_or_else(invalid_stored)?)?;
                let mut v = account_snapshot(&a);
                v["role"] = json!(k);
                Ok(v)
            })
            .collect::<FbResult<Vec<_>>>()?
    ))
}
fn entries(destination: &str, total: i64, credits: BTreeMap<String, i64>) -> Vec<Value> {
    let mut out = vec![json!({"account":destination,"debit":cash(total),"credit":"0.00"})];
    out.extend(
        credits
            .into_iter()
            .filter(|(_, n)| *n != 0)
            .map(|(a, n)| json!({"account":a,"debit":"0.00","credit":cash(n)})),
    );
    out
}
fn token<'a>(review: Option<&'a Review>, domain: &str, message: &str) -> FbResult<&'a str> {
    review
        .filter(|r| r.domain == domain)
        .map(|r| r.digest.as_str())
        .ok_or_else(|| FbError::validation(message))
}
fn seal(tx: &Writer<'_>, kind: &str, key: &str) -> FbResult<()> {
    tx.execute(
        "INSERT INTO sales_seal(kind,doc_id) VALUES(?1,?2)",
        params![kind, key],
    )?;
    Ok(())
}
fn finish_journal(tx: &Writer<'_>, key: &str) -> FbResult<()> {
    seal(tx, "sales_journal", key)
}
/// No write primitive accepting `&(impl Query + ?Sized)` is exported by this module.
fn command<T>(book: &mut Book, f: impl FnOnce(&Writer<'_>) -> FbResult<T>) -> FbResult<T> {
    book.write(|tx| tx.atomic(|| f(tx)))
}

#[cfg(test)]
mod tests;
