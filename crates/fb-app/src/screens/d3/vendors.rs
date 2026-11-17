//! Vendors with money on them: open and overdue balances on the vendor list, and the vendor page's
//! balance summary and one combined list of bills, bill payments and checks. Read-only.
use super::*;
use std::collections::BTreeMap;

fn cents(v: &Value) -> i128 {
    v.as_str().and_then(|s| fb_core::services::money_json(&json!(s)).ok()).map(|m| m.cents()).unwrap_or(0)
}
#[derive(Default, Clone)]
struct Owed {
    open: i128,
    overdue: i128,
    open_bills: usize,
    overdue_bills: usize,
    oldest_due: Option<String>,
}
/// What the company owes each vendor on posted bills, and how much of it is past due.
fn owed(app: &mut App, c: CompanyId, today: &str) -> AppResult<BTreeMap<String, Owed>> {
    let actor = app.open_mut()?.owner;
    let mut out = BTreeMap::<String, Owed>::new();
    for b in fb_core::payables::current_bills(&mut app.open_mut()?.book, actor, c)? {
        if b["state"] != "OPEN" || !["OPEN", "PARTIAL"].contains(&b["payment_state"].as_str().unwrap_or("")) {
            continue;
        }
        let open = cents(&b["outstanding"]);
        if open <= 0 {
            continue;
        }
        let due = b["current"]["due_date"].as_str().unwrap_or("").to_owned();
        let e = out.entry(b["vendor_id"].as_str().unwrap_or("").to_owned()).or_default();
        e.open += open;
        e.open_bills += 1;
        if due.as_str() < today {
            e.overdue += open;
            e.overdue_bills += 1;
        }
        if e.oldest_due.as_ref().is_none_or(|d| due < *d) {
            e.oldest_due = Some(due);
        }
    }
    Ok(out)
}
/// Vendor list: add Open balance and Overdue, and sort by them when asked (`sort=balance|overdue`).
pub(super) fn annotate_list(app: &mut App, c: CompanyId, rows: &mut [Value], sort: &str) -> AppResult<()> {
    let today = (app.today)().to_string();
    let owed = owed(app, c, &today)?;
    for r in rows.iter_mut() {
        let o = owed.get(r["id"].as_str().unwrap_or("")).cloned().unwrap_or_default();
        r["open_balance"] = json!(cash(o.open));
        r["open_cents"] = json!(o.open as i64);
        r["overdue_balance"] = json!(cash(o.overdue));
        r["overdue_cents"] = json!(o.overdue as i64);
        r["open_bills"] = json!(o.open_bills);
    }
    match sort {
        "balance" => rows.sort_by(|a, b| b["open_cents"].as_i64().cmp(&a["open_cents"].as_i64())),
        "overdue" => rows.sort_by(|a, b| b["overdue_cents"].as_i64().cmp(&a["overdue_cents"].as_i64()).then(b["open_cents"].as_i64().cmp(&a["open_cents"].as_i64()))),
        _ => {}
    }
    Ok(())
}
/// Vendor page: balance summary and one transaction list, newest first.
pub(super) fn detail(app: &mut App, c: CompanyId, id: &str, p: Posted<'_>, vm: &mut Value) -> AppResult<()> {
    let today = (app.today)().to_string();
    let o = owed(app, c, &today)?.remove(id).unwrap_or_default();
    let actor = app.open_mut()?.owner;
    let mut rows = vec![];
    for b in fb_core::payables::current_bills(&mut app.open_mut()?.book, actor, c)? {
        if b["vendor_id"] != id {
            continue;
        }
        let r = &b["current"];
        let state = b["payment_state"].as_str().unwrap_or("");
        let due = r["due_date"].as_str().unwrap_or("");
        let open = cents(&b["outstanding"]);
        let status = match state {
            "DRAFT" => "Draft".to_owned(),
            "VOID" => "Voided".to_owned(),
            "PAID" => "Paid".to_owned(),
            _ if due < today.as_str() => "Overdue".to_owned(),
            "PARTIAL" => "Partly paid".to_owned(),
            _ => "Open".to_owned(),
        };
        let open_text = if open > 0 { json!(cash(open)) } else { Value::Null };
        rows.push(json!({
            "kind": "Bill", "date": r["date"], "id": b["id"], "number": b["number"],
            "url": url("bills:detail", &[&c.to_db(), b["id"].as_str().unwrap_or("")]),
            "reference": r["reference"], "due_date": due, "status": status,
            "overdue": status == "Overdue",
            "amount": r["total"], "open": open_text,
            "pay": open > 0,
        }));
    }
    let payments = app.open_mut()?.book.read(|tx| -> Result<Vec<Value>, FbError> {
        let mut st = tx.prepare("SELECT p.id,p.date,p.amount,p.reference,i.number,EXISTS(SELECT 1 FROM payables_paymentreversal v WHERE v.payment_id=p.id) FROM payables_billpayment p JOIN documents_documentidentity i ON i.id=p.identity_id WHERE p.company_id=?1 AND p.vendor_id=?2")?;
        let rows = st.query_map(params![c.to_db(), id], |r| {
            Ok(json!({"id":r.get::<_,String>(0)?,"date":r.get::<_,String>(1)?,"amount":cash(i128::from(r.get::<_,i64>(2)?)),"reference":r.get::<_,String>(3)?,"number":r.get::<_,String>(4)?,"reversed":r.get::<_,bool>(5)?}))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    })?;
    for v in payments {
        let status = if v["reversed"] == true { "Reversed" } else { "Recorded" };
        rows.push(json!({
            "kind": "Bill payment", "date": v["date"], "id": v["id"], "number": v["number"],
            "url": url("payments:detail", &[&c.to_db(), v["id"].as_str().unwrap_or("")]),
            "reference": v["reference"], "due_date": "", "status": status,
            "amount": v["amount"], "open": Value::Null, "pay": false,
        }));
    }
    for k in fb_core::checks::current_checks(&mut app.open_mut()?.book, actor, c)? {
        let r = &k["current"];
        if r["vendor_id"] != id {
            continue;
        }
        let status = match r["state"].as_str().unwrap_or("") { "DRAFT" => "Draft", "RECORDED" => "Recorded", "VOID" => "Voided", s => s };
        rows.push(json!({
            "kind": "Check", "date": r["date"], "id": k["id"], "number": k["number"],
            "url": url("checks:detail", &[&c.to_db(), k["id"].as_str().unwrap_or("")]),
            "reference": r["check_number"],  "due_date": "",
            "status": status,
            "amount": r["total"], "open": Value::Null, "pay": false,
        }));
    }
    rows.sort_by(|a, b| b["date"].as_str().cmp(&a["date"].as_str()).then(b["number"].as_str().cmp(&a["number"].as_str())));
    let last_payment = rows.iter().find(|r| r["kind"] == "Bill payment" && r["status"] == "Recorded").cloned();
    put(vm, "vendor_summary", json!({
        "open": cash(o.open), "overdue": cash(o.overdue), "open_bills": o.open_bills, "overdue_bills": o.overdue_bills,
        "oldest_due": o.oldest_due, "last_payment": last_payment, "has_open": o.open > 0,
    }));
    put(vm, "vendor_rows", Page::of(&rows, 25, get(p, "page")));
    Ok(())
}
