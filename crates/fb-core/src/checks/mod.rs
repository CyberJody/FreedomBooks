//! Written checks, with append-only revisions and record-only bank postings.
use crate::{
    AccountId, Book, CompanyId, FbResult, Money, UserId, calculations,
    directory::{self, Model},
    documents, py, services,
};
use fb_db::rusqlite::params;
use fb_db::{Query, Writer};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
pub(crate) mod support;
use support::*;
pub(crate) mod numbering;
pub use numbering::{
    next_number, number_text, record_bill_payment, refuse_conflict, review_bill_payment,
};
mod lifecycle;
pub use lifecycle::{assign_number, record_check, record_check_tx, review_check, review_check_tx, void_check};
#[cfg(test)]
mod tests;

#[derive(Clone, Debug)]
pub struct Review {
    pub(crate) digest: String,
    pub display: Value,
}

fn check(c: &(impl Query + ?Sized), co: CompanyId, cid: &str) -> FbResult<Value> {
    let mut b = rows(
        c,
        "SELECT * FROM checks_check WHERE company_id=? AND id=?",
        &[&co.to_db(), &cid],
    )?
    .pop()
    .ok_or_else(|| crate::FbError::missing("Check"))?;
    b["number"] = documents::identity(c, co, s(&b, "identity_id"))?["number"].clone();
    let mut r = rows(
        c,
        "SELECT * FROM checks_checkrevision WHERE document_id=? AND number=?",
        &[&cid, &b["revision"].as_i64()],
    )?
    .pop()
    .ok_or_else(|| err("Check requires real revision"))?;
    r["lines"] = json!(rows(
        c,
        "SELECT * FROM checks_checkline WHERE revision_id=? ORDER BY position",
        &[&s(&r, "id")]
    )?);
    b["current"] = r;
    Ok(b)
}
fn money_display(mut b: Value) -> Value {
    let r = &mut b["current"];
    let mut totals = BTreeMap::<String, i64>::from([("EXPENSE".into(), 0), ("ITEM".into(), 0)]);
    for line in r["lines"].as_array().unwrap() {
        *totals.get_mut(s(line, "tab")).unwrap() += line["amount"].as_i64().unwrap();
    }
    r["tab_totals"] = json!(
        totals
            .into_iter()
            .map(|(k, v)| (k, Money::from_db(v).to_string()))
            .collect::<BTreeMap<_, _>>()
    );
    r["number_display"] = json!(if !s(r, "check_number").is_empty() {
        s(r, "check_number")
    } else if r["print_later"] == 1 {
        "To print"
    } else {
        "No number"
    });
    r["total"] = json!(Money::from_db(r["total"].as_i64().unwrap()).to_string());
    r["print_later"] = json!(r["print_later"] == 1);
    for l in r["lines"].as_array_mut().unwrap() {
        for k in ["rate", "amount"] {
            l[k] = json!(Money::from_db(l[k].as_i64().unwrap()).to_string());
        }
        l["quantity"] = json!(
            rust_decimal::Decimal::from_i128_with_scale(l["quantity"].as_i64().unwrap() as i128, 4)
                .to_string()
        );
    }
    b
}
pub fn get_check(book: &mut Book, actor: UserId, co: CompanyId, cid: &str) -> FbResult<Value> {
    book.read(|tx| get_check_tx(tx, actor, co, cid))
}
/// [`get_check`] on a caller's reader or writer.
pub fn get_check_tx(q: &(impl Query + ?Sized), actor: UserId, co: CompanyId, cid: &str) -> FbResult<Value> {
    services::access(q, actor, co)?;
    Ok(money_display(check(q, co, cid)?))
}
pub fn save_check(book: &mut Book, actor: UserId, co: CompanyId, p: &Value) -> FbResult<Value> {
    write(book, |tx| save_check_tx(tx, actor, co, p))
}
/// Current revision per check, scoped to the authorized company.
pub fn current_checks(book: &mut Book, actor: UserId, co: CompanyId) -> FbResult<Vec<Value>> {
    book.read(|tx| {
        services::access(tx, actor, co)?;
        rows(
            tx,
            "SELECT id FROM checks_check WHERE company_id=? ORDER BY id",
            &[&co.to_db()],
        )?
        .iter()
        .map(|r| check(tx, co, s(r, "id")).map(money_display))
        .collect()
    })
}
/// Composable write includes its audit in a rollback boundary, even if the caller catches Err.
/// An autocommit connection cannot supply the writer capability.
/// ```compile_fail
/// use fb_core::{checks, CompanyId, UserId};
/// use fb_db::rusqlite::Connection;
/// fn cannot_write(c: &Connection, actor: UserId, company: CompanyId) {
///     checks::save_check_tx(c, actor, company, &serde_json::json!({}));
/// }
/// ```
/// Nor can an authorized read callback.
/// ```compile_fail
/// use fb_core::{checks, Book, CompanyId, UserId};
/// fn cannot_write(book: &mut Book, actor: UserId, company: CompanyId) {
///     book.read(|r| checks::save_check_tx(r, actor, company, &serde_json::json!({})));
/// }
/// ```
pub fn save_check_tx(tx: &Writer<'_>, actor: UserId, co: CompanyId, p: &Value) -> FbResult<Value> {
    tx.atomic(|| save_inner(tx, actor, co, p))
}
fn save_inner(tx: &Writer<'_>, actor: UserId, co: CompanyId, p: &Value) -> FbResult<Value> {
    services::access(tx, actor, co)?;
    let retained = p["check_id"].as_str().map(|key| check(tx, co, key)).transpose();
    let previous = retained.as_ref().ok().and_then(|o| o.as_ref()).map(|o| &o["current"]);
    let bank_id = id(&p["bank_id"], false)?.unwrap();
    let kind = s(p, "payee_kind");
    if !["VENDOR", "OTHER"].contains(&kind) {
        return Err(err("Choose a vendor payee or an other payee."));
    }
    let vendor_id = id(&p["vendor_id"], true)?;
    if kind == "VENDOR" && vendor_id.is_none() {
        return Err(err(
            "Choose a vendor from this company, or switch to an other payee.",
        ));
    }
    if kind == "OTHER" && vendor_id.is_some() {
        return Err(err("An other payee carries no vendor link."));
    }
    let mut payee = py::input_text(
        p.get("payee_name").unwrap_or(&json!("")), 120, kind == "OTHER",
        previous.and_then(|p| p["payee_name"].as_str()),
    )?;
    let address = py::input_text(p.get("address").unwrap_or(&json!("")), 1000, false, previous.and_then(|p| p["address"].as_str()))?;
    let dt = date(&p["date"])?;
    let number = number_text(p.get("check_number").unwrap_or(&json!("")))?;
    let print = flag(p, "print_later")?;
    if print && !number.is_empty() {
        return Err(err(
            "A check marked print later has no number yet; clear the number or the flag.",
        ));
    }
    if !print && number.is_empty() {
        return Err(err("Enter a check number, or mark the check print later."));
    }
    let memo = crate::card_numbers::card_free(py::input_text(p.get("memo").unwrap_or(&json!("")), 2000, false, previous.and_then(|p| p["memo"].as_str()))?, "Memo")?;
    let refresh = flag(p, "refresh_payee")?;
    let keep = flag(p, "keep_archived")?;
    let cid = id(&p["check_id"], true)?;
    let input = p["lines"]
        .as_array()
        .filter(|a| (1..=50).contains(&a.len()))
        .ok_or_else(|| err("Enter between 1 and 50 lines across the Expenses and Items tabs."))?;
    let mut normalized = vec![];
    let mut total = 0i64;
    for (i, l) in input.iter().enumerate() {
        let allowed = [
            "tab",
            "account_id",
            "item_id",
            "description",
            "quantity",
            "rate",
            "origin_position",
            "refresh_item",
            "keep_archived",
        ];
        if l.as_object()
            .is_none_or(|m| m.keys().any(|k| !allowed.contains(&k.as_str())))
        {
            return Err(err("Unexpected line fields."));
        }
        let tab = s(l, "tab");
        if !["EXPENSE", "ITEM"].contains(&tab) {
            return Err(err(
                "Each line belongs to the Expenses tab or the Items tab.",
            ));
        }
        let item = id(&l["item_id"], true)?;
        let qty = if tab == "EXPENSE" {
            if item.is_some() {
                return Err(err(
                    "Expense lines carry an account and an amount, not an item.",
                ));
            }
            if l.get("quantity")
                .is_some_and(|v| !v.is_null() && v != &json!("1") && v != &json!(1))
            {
                return Err(err(
                    "Expense lines have no quantity; enter the amount only.",
                ));
            }
            calculations::quantity(&json!("1"))?
        } else {
            if item.is_none() {
                return Err(err("Item lines need an item from this company."));
            }
            calculations::quantity(&l["quantity"])?
        };
        let rate = calculations::unit_rate(&l["rate"])?;
        let amount = calculations::line_amount(&json!(qty.to_string()), &json!(rate.to_string()))?
            .cents() as i64;
        if rate.is_zero() || amount <= 0 {
            return Err(err("Every check line must be a positive amount."));
        }
        total = total
            .checked_add(amount)
            .filter(|n| *n <= 9_999_999_999_999_999)
            .ok_or_else(|| err("Check total exceeds 99999999999999.99."))?;
        let origin = l
            .get("origin_position")
            .cloned()
            .unwrap_or(if cid.is_some() {
                json!(i + 1)
            } else {
                Value::Null
            });
        if !origin.is_null() && origin.as_i64().is_none_or(|n| n <= 0) {
            return Err(err("Choose a record from this company."));
        }
        normalized.push(json!({"tab":tab,"account_id":id(&l["account_id"],true)?,"item_id":item,"description":py::input_text(l.get("description").unwrap_or(&json!("")),1000,false,py::retained_line_text(previous.and_then(|p| p["lines"].as_array()), &origin, "description"))?,"quantity":qty.to_string(),"rate":rate.to_string(),"amount":Money::from_db(amount).to_string(),"origin_position":origin,"refresh_item":flag(l,"refresh_item")?,"keep_archived":flag(l,"keep_archived")?}));
    }
    let creation = digest(
        &json!({"bank_id":bank_id,"payee_kind":kind,"vendor_id":vendor_id,"payee_name":payee,"address":address,"date":dt,"check_number":number,"print_later":print,"memo":memo,"lines":normalized,"refresh_payee":refresh,"keep_archived":keep}),
    );
    let expected = p
        .get("expected_revision")
        .map(Value::as_i64)
        .unwrap_or(Some(0));
    let (mut b, old) = if let Some(cid) = cid.as_deref() {
        let mut b = retained?.ok_or_else(|| crate::FbError::missing("Check"))?;
        numbering::validate_number(tx, co, Some(cid), &bank_id, &number)?;
        documents::check_revision(b["revision"].as_i64().unwrap(), expected)?;
        if b["state"] != "DRAFT" {
            return Err(err(
                "Only Draft checks can be edited. Recorded or voided financial history is immutable.",
            ));
        }
        let old = b["current"].clone();
        b["revision"] = json!(documents::next_revision(b["revision"].as_i64().unwrap())?);
        (b, Some(old))
    } else {
        documents::check_revision(0, expected)?;
        let identity = documents::create_identity(tx, actor, co, "CHK", s(p, "request_key"))?;
        if let Some(v) = rows(
            tx,
            "SELECT id,creation_digest FROM checks_check WHERE identity_id=?",
            &[&s(&identity, "id")],
        )?
        .pop()
        {
            if v["creation_digest"] != creation {
                return Err(err(
                    "This retry key belongs to a different check payload. Start a new check instead.",
                ));
            }
            return Ok(money_display(check(tx, co, s(&v, "id"))?));
        }
        (
            json!({"id":fb_ids::new_v7().to_string(),"company_id":co.to_db(),"identity_id":identity["id"],"creation_digest":creation,"revision":1,"bank_id":bank_id,"vendor_id":vendor_id,"number_key":number,"state":"DRAFT"}),
            None,
        )
    };
    numbering::validate_number(tx, co, cid.as_deref(), &bank_id, &number)?;
    let vendor = vendor_id
        .as_ref()
        .map(|id| {
            directory::get(tx, co, Model::Vendor, id)
                .map_err(|_| err("Choose a record from this company."))
        })
        .transpose()?;
    if let Some(v) = &vendor {
        select(
            v,
            old.as_ref().and_then(|o| o["vendor_id"].as_str()),
            keep,
            refresh,
        )?;
    }
    let old_lines = old
        .as_ref()
        .and_then(|o| o["lines"].as_array())
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
        let snapshot = if let Some(prior) =
            prior.filter(|o| o["item_id"] == l["item_id"] && l["refresh_item"] == false)
        {
            prior["item_snapshot"].clone()
        } else if let Some(item) = item {
            json!({"id":item["id"],"code":item["code"],"description":item["description"],"kind":item["kind"],"purchase_cost":item["purchase_cost"],"expense_account_id":item["expense_account"]})
        } else {
            json!({})
        };
        let mut v = json!({"position":i+1,"origin_position":origin,"tab":l["tab"],"account_id":l["account_id"],"item_id":l["item_id"],"item_snapshot":snapshot,"account_snapshot":{},"description":l["description"],"quantity":calculations::quantity(&l["quantity"])?.mantissa() as i64,"rate":cents(&l["rate"])?,"amount":cents(&l["amount"])?});
        if let Some(aid) = l["account_id"].as_str() {
            let a = account(tx, co, aid)
                .map_err(|_| err("Line accounts must belong to this company."))?;

            v["account_snapshot"] = asnap(&a);
        }
        prepared.push(v);
    }
    let bank =
        account(tx, co, &bank_id).map_err(|_| err("Line accounts must belong to this company."))?;
    if bank.kind.as_str() != "ASSET"
        || !bank.active
        || !bank.postable
        || !documents::setup_snapshot(tx, co)?["banks"]
            .as_array()
            .is_some_and(|a| a.contains(&json!(bank_id)))
    {
        return Err(err(
            "Choose a designated bank account (Accounting setup) that is an active postable asset account.",
        ));
    }
    for (v, l) in prepared.iter().zip(&normalized) {
        let prior = old_lines
            .iter()
            .find(|o| o["position"].as_i64() == l["origin_position"].as_i64());
        if let Some(aid) = v["account_id"].as_str() {
            let a = account(tx, co, aid)?;
            if let Some(e) = documents::line_account_error(
                tx,
                co,
                Some(&a),
                AccountId::parse(&bank_id),
                prior.is_some_and(|o| o["account_id"] == l["account_id"]),
            )? {
                return Err(err(
                    &(e + " Check lines may not use the paying bank or the receivables, payables, undeposited-funds and collected-sales-tax control accounts. Clear the account for later correction or replace it.")
                ));
            }
        }
    }
    let payee_snapshot = if let Some(v) = vendor {
        let snap = if old
            .as_ref()
            .is_some_and(|o| o["vendor_id"] == json!(vendor_id))
            && !refresh
        {
            old.as_ref().unwrap()["payee_snapshot"].clone()
        } else {
            party(&v)
        };
        if payee.is_empty() {
            payee = s(&snap, "display_name").into();
        }
        snap
    } else {
        json!({"display_name":payee})
    };
    let r = json!({"bank_id":bank_id,"bank_snapshot":asnap(&bank),"payee_kind":kind,"vendor_id":vendor_id,"payee_name":payee,"payee_snapshot":payee_snapshot,"address":address,"company_snapshot":if let Some(o)=&old{o["company_snapshot"].clone()}else{company_snap(tx,co)?},"date":dt,"check_number":number,"print_later":i64::from(print),"memo":memo,"state":"DRAFT","action":if old.is_some(){"revised"}else{"created"},"reason":if refresh||normalized.iter().any(|r|r["refresh_item"]==true){"Explicit snapshot refresh"}else{""},"total":total,"journal_id":null,"reversal_id":null,"review_digest":""});
    b["bank_id"] = json!(bank_id);
    b["vendor_id"] = json!(vendor_id);
    b["number_key"] = json!(number);
    if old.is_none() {
        insert(tx, "checks_check", &b)?;
    } else {
        tx.execute(
            "UPDATE checks_check SET revision=?,bank_id=?,vendor_id=?,number_key=? WHERE id=?",
            params![
                b["revision"].as_i64(),
                bank_id,
                vendor_id,
                number,
                s(&b, "id")
            ],
        )?;
    }
    append(tx, actor, co, &b, r, prepared)?;
    Ok(money_display(check(tx, co, s(&b, "id"))?))
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
    r["document_id"] = b["id"].clone();
    r["number"] = b["revision"].clone();
    r["actor_id"] = json!(actor.to_db());
    r["actor_name"] = json!(u.username);
    r["created_at"] = json!(py::now_utc());
    r.as_object_mut().unwrap().remove("lines");
    insert(tx, "checks_checkrevision", &r)?;
    for mut l in lines {
        if r["state"] != "DRAFT" {
            l["origin_position"] = l["position"].clone();
        }
        l["id"] = json!(fb_ids::new_v7().to_string());
        l["revision_id"] = r["id"].clone();
        insert(tx, "checks_checkline", &l)?;
    }
    tx.execute(
        "INSERT INTO fb_seal VALUES('check_revision',?)",
        [format!("{}:{}", s(b, "id"), b["revision"])],
    )?;
    let identity = documents::identity(tx, co, s(b, "identity_id"))?;
    services::audit(
        tx,
        &u,
        co,
        &format!("check.{}", s(&r, "action")),
        format!("check:{}", s(b, "id")),
        json!({"revision":r["number"],"previous_revision":if r["number"]==1{Value::Null}else{json!(r["number"].as_i64().unwrap()-1)},"state":r["state"],"reason":r["reason"],"number":identity["number"],"check_number":r["check_number"],"print_later":r["print_later"]==1,"bank_id":r["bank_id"],"payee":r["payee_name"],"total":Money::from_db(r["total"].as_i64().unwrap()).to_string(),"memo":r["memo"]}),
    )?;
    Ok(())
}
fn seal_journal(tx: &Writer<'_>, jid: &str) -> FbResult<()> {
    tx.execute("INSERT INTO fb_seal VALUES('checks_journal',?)", [jid])?;
    Ok(())
}
