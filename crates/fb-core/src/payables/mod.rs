//! Bills and record-only payments, translated from frozen payables services.
//! Public writes always own BEGIN IMMEDIATE. Internal primitives cannot escape this module.
use crate::{
    AccountId, Book, CompanyId, FbError, FbResult, Money, UserId, calculations,
    directory::{self, Model},
    documents, py, services, store,
};
use fb_db::rusqlite::{OptionalExtension, params, types::Value as SqlValue};
use fb_db::{Query, Writer};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

mod payments;
mod queries;
pub use queries::{bill_calendar, bill_tracker};
mod receipt;
pub use payments::{record_payment, reverse_payment, review_payment};
pub use receipt::{receipt_draft_input, save_receipt_draft};
#[cfg(test)]
mod tests;

/// Opaque review capability. Only review commands construct one; callers cannot forge a digest.
/// IPC signing/serialization belongs to the host, not the accounting module.
#[derive(Clone, Debug)]
pub struct Review {
    digest: String,
    kind: &'static str,
    pub display: Value,
}

fn err(message: &str) -> FbError {
    FbError::validation(message)
}
fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}
fn digest(v: &Value) -> String {
    format!(
        "{:x}",
        Sha256::digest(py::py_json_dumps_sorted(v).as_bytes())
    )
}
fn text(v: &Value, max: usize, required: bool) -> FbResult<String> {
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
fn field_text(v: &Value, k: &str, max: usize) -> FbResult<String> {
    text(v.get(k).unwrap_or(&json!("")), max, false)
}
fn id(v: &Value, optional: bool) -> FbResult<Option<String>> {
    if v.is_null() && optional {
        return Ok(None);
    };
    let x = v
        .as_str()
        .filter(|s| 36 == s.len() && py::record_uuid(s).is_some())
        .ok_or_else(|| err("Choose a record from this company."))?;
    Ok(Some(x.into()))
}
fn flag(v: &Value, k: &str) -> FbResult<bool> {
    v.get(k)
        .unwrap_or(&Value::Bool(false))
        .as_bool()
        .ok_or_else(|| err("Invalid checkbox value."))
}
fn date(v: &Value) -> FbResult<String> {
    v.as_str().and_then(fb_db::dates::parse_iso_date).ok_or_else(|| err("Enter a valid calendar date."))
}
fn cents(v: &Value) -> FbResult<i64> {
    Ok(calculations::unit_rate(v)?.mantissa() as i64)
}
fn write<T>(book: &mut Book, f: impl FnOnce(&Writer<'_>) -> FbResult<T>) -> FbResult<T> {
    book.write(|tx| tx.atomic(|| f(tx)))
}
fn rows(
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
            Ok(v)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for row in &result {
        check_snapshots(row)?;
    }
    Ok(result)
}
// Stored JSON is checked independently of SQL guards: corrupt or legacy books
// must produce a controlled error, not a panic in a public command.
fn stored_i64(v: &Value, key: &str) -> FbResult<i64> {
    v[key]
        .as_i64()
        .ok_or_else(|| err("Invalid stored payables integer."))
}
fn stored_lines(v: &Value) -> FbResult<&Vec<Value>> {
    v["lines"]
        .as_array()
        .ok_or_else(|| err("Invalid stored payables lines."))
}
fn object_mut(v: &mut Value) -> FbResult<&mut serde_json::Map<String, Value>> {
    v.as_object_mut()
        .ok_or_else(|| err("Invalid stored payables object."))
}
fn check_snapshots(v: &Value) -> FbResult<()> {
    for (key, value) in v
        .as_object()
        .ok_or_else(|| err("Invalid stored payables record."))?
    {
        if key.ends_with("_snapshot")
            && (!value.is_object()
                || (key == "account_snapshot"
                    && value
                        .as_object()
                        .is_some_and(|m| m.values().any(|v| !v.is_object()))))
        {
            return Err(err("Invalid stored payables snapshot shape."));
        }
    }
    Ok(())
}
fn insert(tx: &Writer<'_>, table: &str, v: &Value) -> FbResult<()> {
    let m = v
        .as_object()
        .ok_or_else(|| err("Invalid payables record."))?;
    let keys = m.keys().collect::<Vec<_>>();
    let vals = keys
        .iter()
        .map(|k| {
            Ok(match &m[*k] {
                Value::Null => SqlValue::Null,
                Value::Number(n) => {
                    SqlValue::Integer(n.as_i64().ok_or_else(|| err("Invalid payables integer."))?)
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
fn bill(c: &(impl Query + ?Sized), co: CompanyId, bid: &str) -> FbResult<Value> {
    let mut b = rows(
        c,
        "SELECT * FROM payables_bill WHERE company_id=? AND id=?",
        &[&co.to_db(), &bid],
    )?
    .pop()
    .ok_or_else(|| FbError::missing("Bill"))?;
    b["number"] = documents::identity(c, co, s(&b, "identity_id"))?["number"].clone();
    let mut r = rows(
        c,
        "SELECT * FROM payables_billrevision WHERE bill_id=? AND number=?",
        &[&bid, &stored_i64(&b, "revision")?],
    )?
    .pop()
    .ok_or_else(|| err("Bill requires real revision"))?;
    r["lines"] = json!(rows(
        c,
        "SELECT * FROM payables_billline WHERE revision_id=? ORDER BY position",
        &[&s(&r, "id")]
    )?);
    b["current"] = r;
    Ok(b)
}
fn money_display(mut b: Value) -> FbResult<Value> {
    let r = &mut b["current"];
    r["total"] = json!(Money::from_db(stored_i64(r, "total")?).to_string());
    for l in r["lines"]
        .as_array_mut()
        .ok_or_else(|| err("Invalid stored payables lines."))?
    {
        for k in ["rate", "amount"] {
            l[k] = json!(Money::from_db(stored_i64(l, k)?).to_string());
        }
        l["quantity"] = json!(
            rust_decimal::Decimal::from_i128_with_scale(stored_i64(l, "quantity")? as i128, 4)
                .to_string()
        );
    }
    Ok(b)
}
pub fn get_bill(book: &mut Book, actor: UserId, co: CompanyId, bid: &str) -> FbResult<Value> {
    book.read(|tx| {
        services::access(tx, actor, co)?;
        money_display(bill(tx, co, bid)?)
    })
}
pub fn current_bills(book: &mut Book, actor: UserId, co: CompanyId) -> FbResult<Vec<Value>> {
    book.read(|tx| {
        services::access(tx, actor, co)?;
        queries::current(tx, co)
    })
}
fn allocated(c: &(impl Query + ?Sized), bid: &str) -> FbResult<i64> {
    let vals = rows(
        c,
        "SELECT amount FROM fb_payables_live_allocations WHERE bill_id=?",
        &[&bid],
    )?;
    vals.iter().try_fold(0i64, |sum, v| {
        sum.checked_add(stored_i64(v, "amount")?)
            .ok_or_else(|| err("Stored allocation total overflow."))
    })
}
fn select(record: &Value, previous: Option<&str>, keep: bool, refresh: bool) -> FbResult<()> {
    if record["active"] != true && (Some(s(record, "id")) != previous || !keep || refresh) {
        return Err(err(
            "Archived selection: replace it, or explicitly retain its historical snapshot. Reactivate in the directory before refreshing.",
        ));
    }
    Ok(())
}
fn party(v: &Value) -> Value {
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
fn company_snap(c: &(impl Query + ?Sized), co: CompanyId) -> FbResult<Value> {
    let c = store::get_company(c, co)?;
    Ok(json!({"id":co.to_db(),"name":c.name,"currency":c.currency}))
}
fn account(c: &(impl Query + ?Sized), co: CompanyId, aid: &str) -> FbResult<crate::model::Account> {
    AccountId::parse(aid)
        .map(|id| store::account(c, id))
        .transpose()?
        .flatten()
        .filter(|a| a.company_id == co)
        .ok_or_else(|| err("Item account defaults must belong to this company."))
}
fn asnap(a: &crate::model::Account) -> Value {
    json!({"id":a.id.to_db(),"code":a.code,"name":a.name,"kind":a.kind.as_str()})
}
fn validate_reference(
    c: &(impl Query + ?Sized),
    co: CompanyId,
    bid: Option<&str>,
    vendor: &str,
    key: &str,
    sup: Option<&str>,
    reason: &str,
) -> FbResult<()> {
    let matches = rows(
        c,
        "SELECT b.id,b.state,d.number FROM payables_bill b JOIN documents_documentidentity d ON d.id=b.identity_id WHERE b.company_id=? AND b.vendor_id=? AND b.reference_key=? AND b.reference_key<>'' AND b.id<>coalesce(?,'') ORDER BY b.id DESC",
        &[&co.to_db(), &vendor, &key, &bid],
    )?;
    if matches.iter().any(|v| v["state"] != "VOID") {
        return Err(err(
            "A nonvoid bill already uses this vendor reference for this vendor. No duplicate override is available.",
        ));
    }
    if let Some(v) = matches.first() {
        if sup != Some(s(v, "id")) || reason.is_empty() {
            return Err(err(&format!(
                "Choose the latest voided bill {} and give a reason. The original voided bill is kept.",
                s(v, "number")
            )));
        }
    } else if sup.is_some() || !reason.is_empty() {
        return Err(err(
            "A replacement link and reason are only for the latest voided bill with this same vendor and reference.",
        ));
    }
    Ok(())
}
/// Same explicit payload as save_bill; account defaults are suggestions, never silently applied.
pub fn save_bill(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    payload: &Value,
) -> FbResult<Value> {
    write(book, |tx| save_bill_tx(tx, actor, co, payload))
}
pub(crate) fn save_bill_tx(tx: &Writer<'_>, actor: UserId, co: CompanyId, p: &Value) -> FbResult<Value> {
    tx.atomic(|| save_bill_inner(tx, actor, co, p))
}
fn save_bill_inner(tx: &Writer<'_>, actor: UserId, co: CompanyId, p: &Value) -> FbResult<Value> {
    services::access(tx, actor, co)?;
    let retained = p["bill_id"].as_str().map(|key| bill(tx, co, key)).transpose();
    let previous = retained.as_ref().ok().and_then(|o| o.as_ref()).map(|o| &o["current"]);
    let vendor_id = id(&p["vendor_id"], false)?.unwrap();
    let dt = date(&p["date"])?;
    let due = date(&p["due_date"])?;
    if due < dt {
        return Err(err("Due date cannot precede the bill date."));
    }
    let memo = py::input_text(p.get("memo").unwrap_or(&json!("")), 2000, false, previous.and_then(|p| p["memo"].as_str()))?;
    let reference = p
        .get("reference")
        .unwrap_or(&Value::Null)
        .as_str()
        .unwrap_or("")
        .trim_matches([' ', '\t', '\n', '\r', '\u{c}', '\u{b}'])
        .to_string();
    if p.get("reference").is_some_and(|v| !v.is_string()) || py::py_len(&reference) > 120 {
        return Err(err("Enter a vendor reference up to 120 characters."));
    }
    let key: String = tx.query_row("SELECT fb_lower(?)", [&reference], |r| r.get(0))?;
    let sup = id(&p["supersedes_id"], true)?;
    let reentry = py::input_text(p.get("reentry_reason").unwrap_or(&json!("")), 500, false, previous.and_then(|p|p["reentry_reason"].as_str()))?;
    let refresh = flag(p, "refresh_vendor")?;
    let keep = flag(p, "keep_archived")?;
    let input = p["lines"]
        .as_array()
        .filter(|a| (1..=50).contains(&a.len()))
        .ok_or_else(|| err("Enter between 1 and 50 meaningful lines."))?;
    let bid = id(&p["bill_id"], true)?;
    let mut normalized = vec![];
    for (index, l) in input.iter().enumerate() {
        let allowed = [
            "item_id",
            "description",
            "quantity",
            "rate",
            "refresh_item",
            "keep_archived",
            "origin_position",
            "expense_account_id",
        ];
        if l.as_object()
            .is_none_or(|m| m.keys().any(|k| !allowed.contains(&k.as_str())))
        {
            return Err(err("Unexpected line fields."));
        }
        let origin = l
            .get("origin_position")
            .cloned()
            .unwrap_or(if bid.is_some() {
                json!(index + 1)
            } else {
                Value::Null
            });
        if !origin.is_null() && origin.as_i64().is_none_or(|n| n <= 0) {
            return Err(err("Choose a record from this company."));
        }
        normalized.push(json!({"expense_account_id":id(&l["expense_account_id"],true)?,"origin_position":origin,"item_id":id(&l["item_id"],true)?,"description":py::input_text(l.get("description").unwrap_or(&json!("")),1000,true,py::retained_line_text(previous.and_then(|p| p["lines"].as_array()), &origin, "description"))?,"quantity":calculations::quantity(&l["quantity"])?.to_string(),"rate":calculations::unit_rate(&l["rate"])?.to_string(),"refresh_item":flag(l,"refresh_item")?,"keep_archived":flag(l,"keep_archived")?}));
    }
    let calc = calculations::totals(
        &normalized
            .iter()
            .map(|v| {
                let mut v = v.clone();
                v["taxable"] = json!(false);
                v
            })
            .collect::<Vec<_>>(),
        "NONE",
        None,
        &json!(""),
        None,
        false,
    )?;
    let intent = json!({"vendor_id":vendor_id,"date":dt,"due_date":due,"memo":memo,"lines":normalized,"reference":reference,"supersedes_id":sup,"reentry_reason":reentry,"refresh_vendor":refresh,"keep_archived":keep});
    let creation = digest(&intent);
    let expected = p
        .get("expected_revision")
        .map(Value::as_i64)
        .unwrap_or(Some(0));
    let (mut b, old) = if let Some(bid) = bid.as_deref() {
        let mut b = retained?.ok_or_else(|| crate::FbError::missing("Bill"))?;
        validate_reference(
            tx,
            co,
            Some(bid),
            &vendor_id,
            &key,
            sup.as_deref(),
            &reentry,
        )?;
        documents::check_revision(stored_i64(&b, "revision")?, expected)?;
        if b["state"] != "DRAFT" {
            return Err(err(
                "Only Draft bills can be edited. Posted or voided financial history is immutable.",
            ));
        }
        let old = b["current"].clone();
        b["revision"] = json!(documents::next_revision(stored_i64(&b, "revision")?)?);
        (b, Some(old))
    } else {
        documents::check_revision(0, expected)?;
        let identity = documents::create_identity(tx, actor, co, "BILL", s(p, "request_key"))?;
        if let Some(v) = rows(
            tx,
            "SELECT id,creation_digest FROM payables_bill WHERE identity_id=?",
            &[&s(&identity, "id")],
        )?
        .pop()
        {
            if v["creation_digest"] != creation {
                return Err(err(
                    "This retry key belongs to a different bill payload. Start a new bill instead.",
                ));
            }
            return money_display(bill(tx, co, s(&v, "id"))?);
        }
        (
            json!({"id":fb_ids::new_v7().to_string(),"company_id":co.to_db(),"identity_id":identity["id"],"creation_digest":creation,"revision":1,"vendor_id":vendor_id,"reference_key":key,"state":"DRAFT"}),
            None,
        )
    };
    validate_reference(
        tx,
        co,
        bid.as_deref(),
        &vendor_id,
        &key,
        sup.as_deref(),
        &reentry,
    )?;
    let vendor = directory::get(tx, co, Model::Vendor, &vendor_id)
        .map_err(|_| err("Choose a record from this company."))?;
    select(
        &vendor,
        old.as_ref().map(|o| s(o, "vendor_id")),
        keep,
        refresh,
    )?;
    let old_lines = old
        .as_ref()
        .and_then(|v| v["lines"].as_array())
        .cloned()
        .unwrap_or_default();
    let mut seen = BTreeSet::new();
    let mut prepared = vec![];
    for (i, l) in normalized.iter().enumerate() {
        let origin = l["origin_position"].as_i64();
        let prior = old_lines.iter().find(|o| o["position"].as_i64() == origin);
        if let Some(n) = origin
            && (!seen.insert(n) || prior.is_none())
        {
            return Err(err(
                "Invalid or duplicated retained line identity. Reload and review.",
            ));
        }
        let item = l["item_id"]
            .as_str()
            .map(|id| {
                directory::get(tx, co, Model::Item, id)
                    .map_err(|_| err("Choose items from this company."))
            })
            .transpose()?;
        if let Some(item) = &item {
            select(
                item,
                prior.and_then(|v| v["item_id"].as_str()),
                l["keep_archived"] == true,
                l["refresh_item"] == true,
            )?;
        }
        let retain =
            prior.is_some_and(|o| o["item_id"] == l["item_id"] && l["refresh_item"] == false);
        let mut v = json!({"position":i+1,"origin_position":origin,"item_id":l["item_id"],"description":l["description"],"quantity":calculations::quantity(&l["quantity"])?.mantissa() as i64,"rate":cents(&l["rate"])?,"amount":cents(&calc["lines"][i])?,"expense_account_id":l["expense_account_id"],"income_account_id":null,"item_snapshot":{},"account_snapshot":{}});
        if let Some(prior) = prior.filter(|_| retain) {
            for k in ["item_snapshot", "income_account_id", "account_snapshot"] {
                v[k] = prior[k].clone();
            }
        } else if let Some(item) = &item {
            v["item_snapshot"] = json!({"id":item["id"],"code":item["code"],"description":item["description"],"kind":item["kind"],"purchase_cost":item["purchase_cost"]});
            v["income_account_id"] = item["income_account"].clone();
        }
        for role in ["income_account", "expense_account"] {
            if let Some(aid) = v[format!("{role}_id")].as_str() {
                let a = account(tx, co, aid)?;
                if role == "expense_account"
                    && let Some(e) = documents::line_account_error(
                        tx,
                        co,
                        Some(&a),
                        None,
                        prior.is_some_and(|o| o["expense_account_id"] == v["expense_account_id"]),
                    )?
                {
                    return Err(err(
                        &(e + " Bill lines may not use the receivables, payables, undeposited-funds and collected-sales-tax control accounts. Replace it, or clear it for later correction.")
                    ));
                }
                if !retain
                    || (role == "expense_account"
                        && prior.is_none_or(|o| o["expense_account_id"] != v["expense_account_id"]))
                {
                    v["account_snapshot"][role] = asnap(&a);
                }
            } else if role == "expense_account" {
                object_mut(&mut v["account_snapshot"])?.remove(role);
            }
        }
        prepared.push(v);
    }
    let r = json!({"vendor_id":vendor_id,"vendor_snapshot":if let Some(o)=old.as_ref().filter(|o|o["vendor_id"]==vendor_id&&!refresh){o["vendor_snapshot"].clone()}else{party(&vendor)},"company_snapshot":if let Some(o)=&old{o["company_snapshot"].clone()}else{company_snap(tx,co)?},"date":dt,"due_date":due,"memo":memo,"state":"DRAFT","action":if old.is_some(){"revised"}else{"created"},"reason":if refresh||normalized.iter().any(|r|r["refresh_item"]==true){"Explicit snapshot refresh"}else{""},"reference":reference,"supersedes_id":sup,"reentry_reason":reentry,"total":cents(&calc["total"])?,"journal_id":null,"reversal_id":null,"review_digest":""});
    b["vendor_id"] = json!(vendor_id);
    b["reference_key"] = json!(key);
    if old.is_none() {
        insert(tx, "payables_bill", &b)?;
    } else {
        tx.execute(
            "UPDATE payables_bill SET revision=?,vendor_id=?,reference_key=? WHERE id=?",
            params![b["revision"].as_i64(), vendor_id, key, s(&b, "id")],
        )?;
    }
    append(tx, actor, co, &b, r, prepared)?;
    money_display(bill(tx, co, s(&b, "id"))?)
}
fn append(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    b: &Value,
    mut r: Value,
    lines: Vec<Value>,
) -> FbResult<()> {
    let u = services::access(tx, actor, co)?;
    r["id"] = json!(fb_ids::new_v7().to_string());
    r["bill_id"] = b["id"].clone();
    r["number"] = b["revision"].clone();
    r["actor_id"] = json!(actor.to_db());
    r["actor_name"] = json!(u.username);
    r["created_at"] = json!(py::now_utc());
    object_mut(&mut r)?.remove("lines");
    insert(tx, "payables_billrevision", &r)?;
    for mut l in lines {
        if r["state"] != "DRAFT" {
            l["origin_position"] = l["position"].clone();
        }
        l["id"] = json!(fb_ids::new_v7().to_string());
        l["revision_id"] = r["id"].clone();
        insert(tx, "payables_billline", &l)?;
    }
    tx.execute(
        "INSERT INTO fb_seal VALUES('bill_revision',?)",
        [format!("{}:{}", s(b, "id"), b["revision"])],
    )?;
    let identity = documents::identity(tx, co, s(b, "identity_id"))?;
    services::audit(
        tx,
        &u,
        co,
        &format!("bill.{}", s(&r, "action")),
        format!("bill:{}", s(b, "id")),
        json!({"revision":r["number"],"previous_revision":if r["number"]==1{Value::Null}else{json!(stored_i64(&r, "number")?-1)},"state":r["state"],"reason":r["reason"],"number":identity["number"],"total":Money::from_db(stored_i64(&r, "total")?).to_string(),"memo":r["memo"],"reference":r["reference"],"supersedes_id":r["supersedes_id"],"reentry_reason":r["reentry_reason"]}),
    )?;
    Ok(())
}
fn references(c: &(impl Query + ?Sized), co: CompanyId, r: &Value, eligible: bool) -> FbResult<()> {
    let vendor=directory::get(c,co,Model::Vendor,s(r,"vendor_id")).map_err(|_|err("The historical vendor is no longer available in this company. Review its ownership before proceeding."))?;
    let mut active = vendor["active"] == true;
    for l in stored_lines(r)? {
        if let Some(id) = l["item_id"].as_str() {
            active &= directory::get(c, co, Model::Item, id)
                .map_err(|_| err("Historical items must belong to this company."))?["active"]
                == true;
        }
        for k in ["income_account_id", "expense_account_id"] {
            if let Some(id) = l[k].as_str() {
                account(c, co, id)
                    .map_err(|_| err("Historical accounts must belong to this company."))?;
            }
        }
    }
    if eligible && !active {
        return Err(err(
            "Vendor and every selected item must be active before posting.",
        ));
    }
    Ok(())
}
fn posting(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    b: &Value,
) -> FbResult<(Value, Vec<Value>, Review)> {
    let r = &b["current"];
    if r["state"] != "DRAFT" {
        return Err(err(
            "Only a draft bill can be reviewed or posted. Voided bills cannot be reposted.",
        ));
    }
    let lines = stored_lines(r)?;
    let mut total = 0;
    for l in lines {
        let q = rust_decimal::Decimal::from_i128_with_scale(stored_i64(l, "quantity")? as i128, 4)
            .to_string();
        let rate = Money::from_db(stored_i64(l, "rate")?).to_string();
        let a = calculations::line_amount(&json!(q), &json!(rate))?;
        if a.cents() != stored_i64(l, "amount")? as i128 {
            return Err(err("Stored bill arithmetic does not match its lines."));
        }
        total += stored_i64(l, "amount")?;
    }
    if total == 0 {
        return Err(err("A zero-total document cannot be posted."));
    }
    if total != stored_i64(r, "total")? {
        return Err(err("Stored bill arithmetic does not match its lines."));
    }
    references(tx, co, r, true)?;
    let mut ids = vec![];
    for l in lines {
        let a = l["expense_account_id"]
            .as_str()
            .and_then(AccountId::parse)
            .ok_or_else(|| {
                err("Choose an explicit expense account on every line before posting.")
            })?;
        ids.push(a);
    }
    let mut mapping = documents::lock_configuration(tx, actor, co, &["payables"], None, &ids)?;
    let mut debits = BTreeMap::<String, i64>::new();
    for l in lines {
        let a = account(tx, co, s(l, "expense_account_id"))?;
        if let Some(e) = documents::line_account_error(tx, co, Some(&a), None, false)? {
            return Err(err(&e));
        }
        mapping[format!("expense:{}", l["position"])] = json!(a.id.to_db());
        *debits.entry(a.id.to_db()).or_default() += stored_i64(l, "amount")?;
    }
    let mut entries = vec![
        json!({"account":mapping["payables"],"debit":"0.00","credit":Money::from_db(total).to_string()}),
    ];
    for (a, amount) in debits {
        if amount > 0 {
            entries.push(
                json!({"account":a,"debit":Money::from_db(amount).to_string(),"credit":"0.00"}),
            );
        }
    }
    let display = display_mapping(tx, co, &mapping)?;
    let digest = digest(
        &json!({"bill":b["id"],"company":co.to_db(),"revision":b["revision"],"mapping":display,"entries":entries,"total":Money::from_db(total).to_string()}),
    );
    Ok((
        mapping,
        entries.clone(),
        Review {
            digest,
            kind: "bill",
            display: json!({"mapping":display,"entries":entries}),
        },
    ))
}
fn display_mapping(c: &(impl Query + ?Sized), co: CompanyId, m: &Value) -> FbResult<Vec<Value>> {
    let mut roles = m
        .as_object()
        .ok_or_else(|| err("Invalid payables account mapping."))?
        .keys()
        .collect::<Vec<_>>();
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
pub fn review_bill(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    bid: &str,
    expected: Option<i64>,
) -> FbResult<Review> {
    write(book, |tx| {
        services::access(tx, actor, co)?;
        let b = bill(tx, co, bid)?;
        documents::check_revision(stored_i64(&b, "revision")?, expected)?;
        Ok(posting(tx, actor, co, &b)?.2)
    })
}
pub fn post_bill(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    bid: &str,
    expected: Option<i64>,
    review: &Review,
) -> FbResult<Value> {
    write(book, |tx| {
        let u = services::access(tx, actor, co)?;
        let mut b = bill(tx, co, bid)?;
        let mut r = b["current"].clone();
        let revision = stored_i64(&b, "revision")?;
        if review.kind != "bill" {
            return Err(err("Invalid review. Open a new posting review."));
        }
        if r["state"] == "OPEN"
            && expected == Some(revision - 1)
            && r["review_digest"] == review.digest
        {
            return money_display(b);
        }
        documents::check_revision(revision, expected)?;
        let next_revision = documents::next_revision(revision)?;
        let (mapping, entries, current) = posting(tx, actor, co, &b)?;
        if current.digest != review.digest {
            return Err(err(
                "Posting mapping changed. Open a new review before posting.",
            ));
        }
        documents::snapshot_source_accounts(tx, actor, co, s(&b, "identity_id"), &mapping)?;
        let j = documents::post_source(
            tx,
            actor,
            co,
            s(&b, "identity_id"),
            s(&r, "date"),
            &py::py_truncate(&format!("{}: {}", s(&b, "number"), s(&r, "memo")), 240),
            entries,
        )?;
        services::audit(
            tx,
            &u,
            co,
            "bill.journal_evidence",
            format!("bill:{bid}"),
            json!({"journal_id":j.id.to_db(),"memo":r["memo"],"reference":r["reference"],"total":Money::from_db(stored_i64(&r, "total")?).to_string()}),
        )?;
        b["revision"] = json!(next_revision);
        tx.execute(
            "UPDATE payables_bill SET revision=?,state='OPEN' WHERE id=?",
            params![next_revision, bid],
        )?;
        r["state"] = json!("OPEN");
        r["action"] = json!("posted");
        r["reason"] = json!("");
        r["journal_id"] = json!(j.id.to_db());
        r["review_digest"] = json!(review.digest);
        let lines = stored_lines(&r)?.clone();
        append(tx, actor, co, &b, r, lines)?;
        seal_journal(tx, &j.id.to_db())?;
        money_display(bill(tx, co, bid)?)
    })
}
fn seal_journal(tx: &Writer<'_>, jid: &str) -> FbResult<()> {
    tx.execute("INSERT INTO fb_seal VALUES('payables_journal',?)", [jid])?;
    Ok(())
}
pub fn void_bill(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    bid: &str,
    expected: Option<i64>,
    dt: &Value,
    reason: &Value,
) -> FbResult<Value> {
    write(book, |tx| {
        services::access(tx, actor, co)?;
        let mut b = bill(tx, co, bid)?;
        let mut r = b["current"].clone();
        let revision = stored_i64(&b, "revision")?;
        let dt = date(dt)?;
        let reason = text(reason, 500, true)?;
        if r["state"] == "VOID" && expected == Some(revision - 1) && r["reason"] == reason {
            let old: String = tx.query_row(
                "SELECT date FROM ledger_journal WHERE id=?",
                [s(&r, "reversal_id")],
                |r| r.get(0),
            )?;
            if old == dt {
                return money_display(b);
            }
        }
        documents::check_revision(revision, expected)?;
        let next_revision = documents::next_revision(revision)?;
        if r["state"] != "OPEN" {
            return Err(err("Only an open posted bill can be voided."));
        }
        if dt.as_str() < s(&r, "date") {
            return Err(err("Reversal date cannot precede the bill date."));
        }
        if allocated(tx, bid)? > 0 {
            return Err(err(
                "Reverse every live payment allocated to this bill before voiding it.",
            ));
        }
        references(tx, co, &r, false)?;
        let j = documents::reverse_source(tx, actor, co, s(&b, "identity_id"), &dt, &reason)?;
        b["revision"] = json!(next_revision);
        tx.execute(
            "UPDATE payables_bill SET revision=?,state='VOID' WHERE id=?",
            params![next_revision, bid],
        )?;
        r["state"] = json!("VOID");
        r["action"] = json!("voided");
        r["reason"] = json!(reason);
        r["reversal_id"] = json!(j.id.to_db());
        let lines = stored_lines(&r)?.clone();
        append(tx, actor, co, &b, r, lines)?;
        seal_journal(tx, &j.id.to_db())?;
        money_display(bill(tx, co, bid)?)
    })
}
