use crate::{AccountId, Book, CompanyId, FbError, FbResult, calculations, py, store};
use fb_db::rusqlite::types::Value as SqlValue;
use fb_db::{Query, Writer};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(crate) fn err(message: &str) -> FbError {
    FbError::validation(message)
}
pub(crate) fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}
pub(crate) fn digest(v: &Value) -> String {
    format!(
        "{:x}",
        Sha256::digest(py::py_json_dumps_sorted(v).as_bytes())
    )
}
pub(crate) fn text(v: &Value, max: usize, required: bool) -> FbResult<String> {
    if let Some(raw) = v.as_str() { py::clean_free_text(raw)?; }
    let msg = format!(
        "Enter {}text up to {max} characters.",
        if required { "nonblank " } else { "" }
    );
    let t = v
        .as_str()
        .map(py::py_strip)
        .filter(|t| py::py_len(t) <= max && (!required || !t.is_empty()))
        .ok_or_else(|| err(&msg))?;
    Ok(t.into())
}
pub(crate) fn field_text(v: &Value, k: &str, max: usize) -> FbResult<String> {
    text(v.get(k).unwrap_or(&json!("")), max, false)
}
pub(crate) fn id(v: &Value, optional: bool) -> FbResult<Option<String>> {
    if v.is_null() && optional {
        return Ok(None);
    };
    let x = v
        .as_str()
        .filter(|s| 36 == s.len() && py::record_uuid(s).is_some())
        .ok_or_else(|| err("Choose a record from this company."))?;
    Ok(Some(x.into()))
}
pub(crate) fn flag(v: &Value, k: &str) -> FbResult<bool> {
    v.get(k)
        .unwrap_or(&Value::Bool(false))
        .as_bool()
        .ok_or_else(|| err("Invalid checkbox value."))
}
pub(crate) fn date(v: &Value) -> FbResult<String> {
    v.as_str().and_then(fb_db::dates::parse_iso_date).ok_or_else(|| err("Enter a valid calendar date."))
}
pub(crate) fn cents(v: &Value) -> FbResult<i64> {
    Ok(calculations::unit_rate(v)?.mantissa() as i64)
}
pub(crate) fn write<T>(book: &mut Book, f: impl FnOnce(&Writer<'_>) -> FbResult<T>) -> FbResult<T> {
    book.write(|tx| tx.atomic(|| f(tx)))
}
pub(crate) fn rows(
    c: &(impl Query + ?Sized),
    sql: &str,
    args: &[&dyn fb_db::rusqlite::ToSql],
) -> FbResult<Vec<Value>> {
    let mut stmt = c.prepare(sql)?;
    let cols = stmt
        .column_names()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    let result = stmt
        .query_map(args, |r| {
            let mut v = json!({});
            for (i, k) in cols.iter().enumerate() {
                v[k] = match r.get::<_, SqlValue>(i)? {
                    SqlValue::Null => Value::Null,
                    SqlValue::Integer(n) => json!(n),
                    SqlValue::Text(t) => {
                        if k.ends_with("_snapshot") {
                            fb_db::parse_unique_json_as(&t).map_err(|e| {
                                fb_db::rusqlite::Error::FromSqlConversionFailure(
                                    i,
                                    fb_db::rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            })?
                        } else {
                            json!(t)
                        }
                    }
                    _ => Value::Null,
                };
            }
            if v.get("bank_snapshot").is_some()
                || v.get("tab").is_some()
                || v.get("from_snapshot").is_some()
            {
                for (field, kind, id) in [
                    ("bank_snapshot", "account", "bank_id"),
                    ("account_snapshot", "account", "account_id"),
                    ("item_snapshot", "item", "item_id"),
                    ("payee_snapshot", "party", "vendor_id"),
                    ("company_snapshot", "company", "company_id"),
                    ("from_snapshot", "account", "from_account_id"),
                    ("to_snapshot", "account", "to_account_id"),
                ] {
                    if let Some(snapshot) = v.get(field)
                        && !fb_db::checks_snapshots::valid(kind, snapshot, v[id].as_str())
                    {
                        return Err(fb_db::rusqlite::Error::FromSqlConversionFailure(
                            0,
                            fb_db::rusqlite::types::Type::Text,
                            Box::new(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "Invalid stored snapshot",
                            )),
                        ));
                    }
                }
            }
            Ok(v)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(result)
}
pub(crate) fn insert(tx: &Writer<'_>, table: &str, v: &Value) -> FbResult<()> {
    let m = v
        .as_object()
        .ok_or_else(|| err("Invalid internal record."))?;
    let keys = m.keys().collect::<Vec<_>>();
    let vals = keys
        .iter()
        .map(|k| {
            Ok(match &m[*k] {
                Value::Null => SqlValue::Null,
                Value::Number(n) => {
                    SqlValue::Integer(n.as_i64().ok_or_else(|| err("Invalid stored integer."))?)
                }
                Value::String(s) => SqlValue::Text(s.clone()),
                x => SqlValue::Text(x.to_string()),
            })
        })
        .collect::<FbResult<Vec<_>>>()?;
    tx.execute(
        &format!(
            "INSERT INTO {table}({}) VALUES({})",
            keys.iter()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(","),
            vec!["?"; keys.len()].join(",")
        ),
        fb_db::rusqlite::params_from_iter(vals),
    )?;
    Ok(())
}
pub(crate) fn select(
    record: &Value,
    previous: Option<&str>,
    keep: bool,
    refresh: bool,
) -> FbResult<()> {
    if record["active"] != true && (Some(s(record, "id")) != previous || !keep || refresh) {
        return Err(err(
            "Archived selection: replace it, or explicitly retain its historical snapshot. Reactivate in the directory before refreshing.",
        ));
    }
    Ok(())
}
pub(crate) fn party(v: &Value) -> Value {
    let mut o = json!({});
    for k in [
        "id",
        "display_name",
        "business_name",
        "contact_name",
        "billing_address",
        "email",
        "phone",
    ] {
        o[k] = v[k].clone();
    }
    o
}
pub(crate) fn company_snap(c: &(impl Query + ?Sized), co: CompanyId) -> FbResult<Value> {
    let c = store::get_company(c, co)?;
    Ok(json!({"id":co.to_db(),"name":c.name,"currency":c.currency}))
}
pub(crate) fn account(
    c: &(impl Query + ?Sized),
    co: CompanyId,
    aid: &str,
) -> FbResult<crate::model::Account> {
    AccountId::parse(aid)
        .map(|id| store::account(c, id))
        .transpose()?
        .flatten()
        .filter(|a| a.company_id == co)
        .ok_or_else(|| err("Item account defaults must belong to this company."))
}
pub(crate) fn asnap(a: &crate::model::Account) -> Value {
    json!({"id":a.id.to_db(),"code":a.code,"name":a.name,"kind":a.kind.as_str()})
}
pub(crate) fn display_mapping(
    c: &(impl Query + ?Sized),
    co: CompanyId,
    m: &Value,
) -> FbResult<Vec<Value>> {
    let mut roles = m.as_object().unwrap().keys().collect::<Vec<_>>();
    roles.sort();
    roles
        .iter()
        .map(|r| {
            let a = account(c, co, s(m, r))?;
            let mut v = asnap(&a);
            v["role"] = json!(r);
            Ok(v)
        })
        .collect()
}
