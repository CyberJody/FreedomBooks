//! Immutable customer credits, non-posting applications, and record-only refunds.
use crate::{
    AccountId, Book, CompanyId, FbError, FbResult, Money, UserId, calculations,
    directory::{self, Model},
    documents, py, services, store,
};
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
    v.as_str().and_then(fb_db::dates::parse_iso_date)
        .ok_or_else(|| FbError::validation("Enter a valid calendar date."))
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
fn mapping_display(conn: &(impl Query + ?Sized), c: CompanyId, m: &Value) -> FbResult<Value> {
    let mut keys = m
        .as_object()
        .ok_or_else(|| FbError::validation("Invalid stored credit mapping."))?
        .keys()
        .collect::<Vec<_>>();
    keys.sort();
    Ok(json!(
        keys.into_iter()
            .map(|k| {
                let a = account(conn, c, stored_str(m, k)?)?;
                let mut v = account_snapshot(&a);
                v["role"] = json!(k);
                Ok(v)
            })
            .collect::<FbResult<Vec<_>>>()?
    ))
}
fn token<'a>(review: Option<&'a Review>, domain: &str, message: &str) -> FbResult<&'a str> {
    review
        .filter(|r| r.domain == domain)
        .map(|r| r.digest.as_str())
        .ok_or_else(|| FbError::validation(message))
}

mod applications;
mod memos;
mod refunds;
pub use applications::{apply_credit, review_application, unapply_credit};
pub use memos::{record_credit_memo, review_credit_memo, void_credit_memo};
pub use refunds::{record_refund, refuse_number_conflict, review_refund, void_refund};

fn command<T>(book: &mut Book, f: impl FnOnce(&Writer<'_>) -> FbResult<T>) -> FbResult<T> {
    book.write(|tx| tx.atomic(|| f(tx)))
}
fn key(v: &Value) -> FbResult<String> {
    v.as_str()
        .and_then(py::py_uuid)
        .map(|u| u.0)
        .ok_or_else(|| FbError::validation("Supply a stable UUID request key."))
}
fn add(a: i64, b: i64) -> FbResult<i64> {
    a.checked_add(b)
        .ok_or_else(|| FbError::validation("Document amount exceeds 99999999999999.99."))
}
fn sum(conn: &(impl Query + ?Sized), sql: &str, key: &str) -> FbResult<i64> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map([key], |r| r.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().try_fold(0, add)
}
fn has(conn: &(impl Query + ?Sized), table: &str) -> FbResult<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |r| r.get(0),
    )?)
}
fn stored_str<'a>(v: &'a Value, k: &str) -> FbResult<&'a str> {
    v.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| FbError::validation("Invalid stored credit evidence."))
}
fn parse_stored(table: &str, raw: &str) -> FbResult<Value> {
    let v: Value = fb_db::parse_unique_json_as(raw)
        .map_err(|_| FbError::validation("Invalid stored credit evidence."))?;
    if !fb_db::credit_fields_valid(table.trim_start_matches("credits_"), &v) {
        return Err(FbError::validation("Invalid stored credit evidence."));
    }
    Ok(v)
}
fn load(conn: &(impl Query + ?Sized), table: &str, key: &str) -> FbResult<Value> {
    let raw: Option<String> = conn
        .query_row(
            &format!("SELECT data FROM {table} WHERE id=?1"),
            [key],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|s| parse_stored(table, &s))
        .transpose()?
        .ok_or_else(|| FbError::missing("Credit record"))
}
fn scoped(conn: &(impl Query + ?Sized), c: CompanyId, table: &str, key: &str) -> FbResult<Value> {
    let row = load(conn, table, key)?;
    if row["company_id"] != c.to_db() {
        return Err(FbError::missing("Credit record"));
    }
    Ok(row)
}
fn undo(
    conn: &(impl Query + ?Sized),
    table: &str,
    field: &str,
    key: &str,
) -> FbResult<Option<Value>> {
    let raw: Option<String> = conn
        .query_row(
            &format!("SELECT data FROM {table} WHERE {field}=?1"),
            [key],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|s| parse_stored(table, &s)).transpose()
}
fn seal(tx: &Writer<'_>, kind: &str, key: &str) -> FbResult<()> {
    tx.execute(
        "INSERT INTO credits_seal(kind,doc_id) VALUES(?1,?2)",
        params![kind, key],
    )?;
    Ok(())
}
pub fn live_applied(conn: &(impl Query + ?Sized), memo: &str) -> FbResult<i64> {
    sum(
        conn,
        "SELECT a.amount FROM credits_creditapplication a WHERE a.credit_memo_id=?1 AND NOT EXISTS(SELECT 1 FROM credits_creditunapplication u WHERE u.application_id=a.id)",
        memo,
    )
}
pub fn live_refunded(conn: &(impl Query + ?Sized), memo: &str) -> FbResult<i64> {
    sum(
        conn,
        "SELECT a.amount FROM credits_creditrefund a WHERE a.credit_memo_id=?1 AND NOT EXISTS(SELECT 1 FROM credits_creditrefundvoid u WHERE u.refund_id=a.id)",
        memo,
    )
}
pub fn available_credit(conn: &(impl Query + ?Sized), memo: &Value) -> FbResult<i64> {
    let key = memo["id"]
        .as_str()
        .ok_or_else(|| FbError::validation("Invalid credit memo."))?;
    if undo(conn, "credits_creditmemovoid", "credit_memo_id", key)?.is_some() {
        return Ok(0);
    }
    let applied = live_applied(conn, key)?;
    let refunded = live_refunded(conn, key)?;
    cents(&memo["total"])?
        .checked_sub(applied)
        .and_then(|n| n.checked_sub(refunded))
        .ok_or_else(|| FbError::validation("Invalid available credit."))
}
pub fn invoice_credits(conn: &(impl Query + ?Sized), invoice: &str) -> FbResult<i64> {
    sum(
        conn,
        "SELECT l.amount FROM credits_creditapplicationline l WHERE l.invoice_id=?1 AND NOT EXISTS(SELECT 1 FROM credits_creditunapplication u WHERE u.application_id=l.application_id)",
        invoice,
    )
}
/// Sales lifecycle hook. Call after receipt checks, within the same authorized
/// writer that owns the invoice void; it retains the frozen Python messages.
pub fn refuse_live_credits(
    conn: &(impl Query + ?Sized),
    invoice: &str,
    when: &str,
) -> FbResult<()> {
    let when = date(&json!(when))?;
    let live:Option<String>=conn.query_row("SELECT d.number FROM credits_creditapplicationline l JOIN credits_creditapplication a ON a.id=l.application_id JOIN credits_creditmemo m ON m.id=a.credit_memo_id JOIN documents_documentidentity d ON d.id=m.identity_id WHERE l.invoice_id=?1 AND NOT EXISTS(SELECT 1 FROM credits_creditunapplication u WHERE u.application_id=a.id) ORDER BY l.id LIMIT 1",[invoice],|r|r.get(0)).optional()?;
    if let Some(number) = live {
        return Err(FbError::validation(format!(
            "Unapply credit memo {number} from this invoice before voiding it."
        )));
    }
    let released:Option<String>=conn.query_row("SELECT max(u.date) FROM credits_creditunapplication u JOIN credits_creditapplicationline l ON l.application_id=u.application_id WHERE l.invoice_id=?1",[invoice],|r|r.get(0))?;
    if let Some(released) = released.filter(|d| d > &when) {
        return Err(FbError::validation(format!(
            "Void date cannot precede {released}, when a credit memo was unapplied from this invoice."
        )));
    }
    Ok(())
}
pub fn get_credit_memo(book: &mut Book, actor: UserId, c: CompanyId, key: &str) -> FbResult<Value> {
    book.read(|tx| {
        services::access(tx, actor, c)?;
        let mut m = scoped(tx, c, "credits_creditmemo", key)?;
        let mut stmt = tx.prepare(
            "SELECT data FROM credits_creditmemoline WHERE credit_memo_id=?1 ORDER BY position",
        )?;
        let raw = stmt
            .query_map([key], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        m["lines"] = json!(
            raw.iter()
                .map(|r| parse_stored("creditmemoline", r))
                .collect::<FbResult<Vec<_>>>()?
        );
        m["void"] =
            undo(tx, "credits_creditmemovoid", "credit_memo_id", key)?.unwrap_or(Value::Null);
        Ok(m)
    })
}
pub fn memo_balances(book: &mut Book, actor: UserId, c: CompanyId) -> FbResult<Vec<Value>> {
    book.read(|tx|{services::access(tx,actor,c)?;
      let mut stmt=tx.prepare("SELECT id FROM credits_creditmemo WHERE company_id=?1 ORDER BY id")?;
      let keys=stmt.query_map([c.to_db()],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
      keys.iter().map(|key|{let m=scoped(tx,c,"credits_creditmemo",key)?;let applied=live_applied(tx,key)?;let refunded=live_refunded(tx,key)?;let available=available_credit(tx,&m)?;let voided=undo(tx,"credits_creditmemovoid","credit_memo_id",key)?.is_some();
      Ok(json!({"id":key,"customer_id":m["customer_id"],"total":m["total"],"applied":cash(applied),"refunded":cash(refunded),"available":cash(available),"credit_state":if voided{"VOID"}else if available==0{"USED"}else if available<cents(&m["total"])?{"PARTIAL"}else{"AVAILABLE"}}))}).collect()})
}
#[cfg(test)]
mod tests;
