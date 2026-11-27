//! Context for the frozen activity fragments embedded in directory/detail.
use super::*;
fn newest(rows: &mut [Value], date: &str, id: &str) {
    rows.sort_by(|a, b| {
        b[date]
            .as_str()
            .cmp(&a[date].as_str())
            .then(b[id].as_str().cmp(&a[id].as_str()))
    });
}
fn page(vm: &mut Value, key: &str, rows: &[Value], p: Posted<'_>, parameter: &str) {
    put(vm, key, Page::of(rows, 25, get(p, parameter)));
}
fn number(tx: &(impl Query + ?Sized), table: &str, key: &str) -> Result<Value, FbError> {
    assert!(
        [
            "sales_invoice",
            "sales_receipt",
            "credits_creditmemo",
            "credits_creditrefund",
            "payables_billpayment",
            "payables_bill",
            "checks_check"
        ]
        .contains(&table)
    );
    Ok(tx.query_row(&format!("SELECT i.number FROM {table} d JOIN documents_displayidentity i ON i.id=d.identity_id WHERE d.id=?1"),[key],|r|r.get::<_,String>(0)).map(|s|json!({"number":s}))?)
}
fn total<'a>(mut rows: impl Iterator<Item = &'a Value>, field: &str) -> AppResult<i128> {
    rows.try_fold(0i128, |sum, r| {
        sum.checked_add(money(&r[field]))
            .ok_or_else(|| AppError::Invalid("Activity total overflow.".into()))
    })
}
fn state(s: &str, deposit: bool) -> &str {
    match s {
        "AVAILABLE" => {
            if deposit {
                "Unapplied"
            } else {
                "Credit available"
            }
        }
        "PARTIAL" => "Partly used",
        "USED" => "Fully used",
        "VOID" => "Voided",
        _ => s,
    }
}
pub(super) fn build(
    app: &mut App,
    c: CompanyId,
    m: Model,
    id: &str,
    p: Posted<'_>,
    vm: &mut Value,
) -> AppResult<()> {
    if m == Model::Item {
        return Ok(());
    }
    let actor = app.open_mut()?.owner;
    if m == Model::Customer {
        customer(app, c, id, p, vm)?;
    } else {
        let mut bills = fb_core::payables::current_bills(&mut app.open_mut()?.book, actor, c)?
            .into_iter()
            .filter(|b| b["vendor_id"] == id)
            .map(|b| {
                let mut r = b["current"].clone();
                r["bill_id"] = b["id"].clone();
                r["payment_state"] = b["payment_state"].clone();
                r["outstanding"] = b["outstanding"].clone();
                r["bill"] = json!({"identity":{"number":b["number"]}});
                r
            })
            .collect::<Vec<_>>();
        let posted = bills
            .iter()
            .filter(|r| r["state"] == "OPEN")
            .collect::<Vec<_>>();
        put(
            vm,
            "vendor_open_total",
            if posted.is_empty() {
                Value::Null
            } else {
                json!(cash(total(posted.into_iter(), "outstanding")?))
            },
        );
        newest(&mut bills, "date", "id");
        page(vm, "vendor_bills", &bills, p, "bills_page");
        let payments=app.open_mut()?.book.read(|tx|->Result<Vec<_>,FbError>{let mut st=tx.prepare("SELECT p.id,p.date,p.amount,EXISTS(SELECT 1 FROM payables_paymentreversal v WHERE v.payment_id=p.id) FROM payables_billpayment p WHERE p.company_id=?1 AND p.vendor_id=?2 ORDER BY p.date DESC,p.id DESC")?;let raw=st.query_map(params![c.to_db(),id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,bool>(3)?)))?.collect::<Result<Vec<_>,_>>()?;raw.into_iter().map(|(id,date,amount,reversed)|Ok(json!({"pk":id,"date":date,"amount":cash(i128::from(amount)),"reversed":reversed,"identity":number(tx,"payables_billpayment",&id)?}))).collect()})?;
        page(vm, "vendor_payments", &payments, p, "payments_page");
        let mut checks = fb_core::checks::current_checks(&mut app.open_mut()?.book, actor, c)?
            .into_iter()
            .filter(|b| b["current"]["vendor_id"] == id)
            .map(|b| {
                let mut r = b["current"].clone();
                r["document_id"] = b["id"].clone();
                r["document"] = json!({"identity":{"number":b["number"]}});
                r["get_state_display"] = json!(match r["state"].as_str().unwrap_or("") {
                    "DRAFT" => "Draft",
                    "RECORDED" => "Recorded",
                    "VOID" => "Voided",
                    s => s,
                });
                r
            })
            .collect::<Vec<_>>();
        newest(&mut checks, "date", "id");
        page(vm, "vendor_checks", &checks, p, "checks_page");
    }
    Ok(())
}

/// The kinds of transaction the customer page lists, as (filter value, label).
const CUSTOMER_TYPES: &[(&str, &str)] = &[
    ("invoices", "Invoices"),
    ("payments", "Payments received"),
    ("sales_receipts", "Sales receipts"),
    ("estimates", "Estimates"),
    ("credits", "Credit memos"),
    ("deposits", "Customer deposits"),
    ("refunds", "Refunds"),
];
/// One row of the customer's combined transaction list.
#[allow(clippy::too_many_arguments)]
fn row(kind: &str, label: &str, date: &Value, number: &Value, route: &str, target: &Value, status: &str, amount: &Value, open: Value) -> Value {
    json!({"kind":kind,"type":label,"date":date,"number":number,"route":route,"target":target,"status":status,"amount":amount,"open":open})
}
/// Worker item 24: the Customers list shows what each customer owes and how much of it is overdue, by
/// the customer page's rule (open invoices' outstanding amounts; overdue once past the due date).
pub(super) fn annotate_customers(app: &mut App, c: CompanyId, rows: &mut [Value]) -> AppResult<()> {
    let actor = app.open_mut()?.owner;
    let today = (app.today)().to_string();
    let mut owed: std::collections::HashMap<String, (i128, i128)> = std::collections::HashMap::new();
    for r in fb_core::sales::invoice_balances(&mut app.open_mut()?.book, actor, c)? {
        if r["state"] != "OPEN" {
            continue;
        }
        let outstanding = money(&r["outstanding"]);
        let late = outstanding > 0 && r["due_date"].as_str().is_some_and(|d| d < today.as_str());
        let entry = owed.entry(r["customer_id"].as_str().unwrap_or("").to_owned()).or_default();
        let overflow = || AppError::Invalid("Activity total overflow.".into());
        entry.0 = entry.0.checked_add(outstanding).ok_or_else(overflow)?;
        if late {
            entry.1 = entry.1.checked_add(outstanding).ok_or_else(overflow)?;
        }
    }
    for r in rows.iter_mut() {
        let (open, overdue) = owed.get(r["id"].as_str().unwrap_or("")).copied().unwrap_or_default();
        r["open_balance"] = json!(cash(open));
        r["open_cents"] = json!(open != 0);
        r["overdue_balance"] = json!(cash(overdue));
        r["overdue_cents"] = json!(overdue != 0);
    }
    Ok(())
}
/// The customer page: what they owe at a glance, and every document with them in one list.
fn customer(app: &mut App, c: CompanyId, id: &str, p: Posted<'_>, vm: &mut Value) -> AppResult<()> {
    let actor = app.open_mut()?.owner;
    let today = (app.today)().to_string();
    let mut rows = vec![];
    let data = fb_core::sales::customer_activity(&mut app.open_mut()?.book, actor, c, id)?;
    let invoices = data["invoices"].as_array().unwrap().clone();
    let mut overdue = 0i128;
    for r in &invoices {
        let number = app.open_mut()?.book.read(|tx| number(tx, "sales_invoice", r["invoice_id"].as_str().unwrap()))?;
        let late = r["state"] == "OPEN" && money(&r["outstanding"]) > 0 && r["due_date"].as_str().is_some_and(|d| d < today.as_str());
        if late {
            overdue = overdue.checked_add(money(&r["outstanding"])).ok_or_else(|| AppError::Invalid("Activity total overflow.".into()))?;
        }
        let status = match (r["state"].as_str().unwrap_or(""), r["payment_state"].as_str().unwrap_or("")) {
            ("DRAFT", _) => "Draft",
            ("VOID", _) => "Voided",
            (_, "PAID") => "Paid",
            (_, "PARTIAL") if late => "Partly paid · Overdue",
            (_, "PARTIAL") => "Partly paid",
            _ if late => "Overdue",
            _ => "Open",
        };
        rows.push(row("invoices", "Invoice", &r["date"], &number["number"], "invoices:detail", &r["invoice_id"], status, &r["total"], r["outstanding"].clone()));
    }
    for r in data["receipts"].as_array().unwrap() {
        let (kind, label, route) = match r["kind"].as_str() {
            Some("RCP") => ("payments", "Payment", "receipts:detail"),
            Some("SR") => ("sales_receipts", "Sales receipt", "sales_receipts:detail"),
            _ => continue,
        };
        let number = app.open_mut()?.book.read(|tx| number(tx, "sales_receipt", r["id"].as_str().unwrap()))?;
        rows.push(row(kind, label, &r["date"], &number["number"], route, &r["id"], if r["reversal"].is_null() { "Recorded" } else { "Voided" }, &r["amount"], Value::Null));
    }
    let estimates = app.open_mut()?.book.read(|q| -> Result<Vec<Value>, FbError> {
        let mut st = q.prepare("SELECT e.id,i.number,r.data FROM sales_estimate e JOIN sales_estimaterevision r ON r.estimate_id=e.id AND r.number=e.revision JOIN documents_displayidentity i ON i.id=e.identity_id WHERE e.company_id=?1 AND json_extract(r.data,'$.customer_id')=?2")?;
        let raw = st.query_map(params![c.to_db(), id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
        raw.into_iter().map(|(id, number, data)| {
            let v: Value = fb_db::parse_unique_json_as(&data).map_err(|_| FbError::validation("Invalid stored estimate."))?;
            Ok(json!({"id":id,"number":number,"date":v["date"],"state":v["state"],"total":v["total"]}))
        }).collect()
    })?;
    for e in &estimates {
        let status = match e["state"].as_str().unwrap_or("") {
            "DRAFT" => "Draft",
            "ACCEPTED" => "Accepted",
            "DECLINED" => "Declined",
            "CANCELLED" => "Cancelled",
            "CONVERTED" => "Converted",
            s => s,
        };
        rows.push(row("estimates", "Estimate", &e["date"], &e["number"], "sales:detail", &e["id"], status, &e["total"], Value::Null));
    }
    let memos = fb_core::credits::memo_balances(&mut app.open_mut()?.book, actor, c)?
        .into_iter()
        .filter(|r| r["customer_id"] == id)
        .collect::<Vec<_>>();
    let available = total(memos.iter(), "available")?;
    for r in &memos {
        let memo = fb_core::credits::get_credit_memo(&mut app.open_mut()?.book, actor, c, r["id"].as_str().unwrap())?;
        let number = app.open_mut()?.book.read(|tx| number(tx, "credits_creditmemo", r["id"].as_str().unwrap()))?;
        rows.push(row("credits", "Credit memo", &memo["date"], &number["number"], "credits:detail", &r["id"], state(r["credit_state"].as_str().unwrap(), false), &r["total"], r["available"].clone()));
    }
    let deposits = fb_core::prepayments::deposit_balances(&mut app.open_mut()?.book, actor, c)?
        .into_iter()
        .filter(|r| r["customer_id"] == id)
        .collect::<Vec<_>>();
    let held = total(deposits.iter(), "remaining")?;
    for r in &deposits {
        let number = app.open_mut()?.book.read(|tx| number(tx, "sales_receipt", r["id"].as_str().unwrap()))?;
        rows.push(row("deposits", "Customer deposit", &r["date"], &number["number"], "prepayments:detail", &r["id"], state(r["deposit_state"].as_str().unwrap(), true), &r["amount"], r["remaining"].clone()));
    }
    let refunds = app.open_mut()?.book.read(|tx| -> Result<Vec<Value>, FbError> {
        let mut st = tx.prepare("SELECT r.id,r.date,r.amount,EXISTS(SELECT 1 FROM credits_creditrefundvoid v WHERE v.refund_id=r.id) FROM credits_creditrefund r WHERE r.company_id=?1 AND r.customer_id=?2")?;
        let raw = st.query_map(params![c.to_db(), id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, bool>(3)?)))?.collect::<Result<Vec<_>, _>>()?;
        raw.into_iter().map(|(id, date, amount, voided)| Ok(row("refunds", "Refund", &json!(date), &number(tx, "credits_creditrefund", &id)?["number"], "credits:refund_detail", &json!(id), if voided { "Voided" } else { "Recorded" }, &json!(cash(i128::from(amount))), Value::Null))).collect()
    })?;
    rows.extend(refunds);
    let open = total(invoices.iter().filter(|r| r["state"] == "OPEN"), "outstanding")?;
    put(vm, "customer_balance", json!({"open":cash(open),"overdue":cash(overdue),"credits":cash(available),"deposits":cash(held)}));
    // Newest first; the type filter narrows the one list.
    rows.sort_by(|a, b| b["date"].as_str().cmp(&a["date"].as_str()).then(b["number"].as_str().cmp(&a["number"].as_str())));
    let kind = get(p, "type").filter(|k| CUSTOMER_TYPES.iter().any(|(v, _)| v == k)).unwrap_or("");
    let shown = rows.into_iter().filter(|r| kind.is_empty() || r["kind"] == kind).collect::<Vec<_>>();
    put(vm, "activity_types", CUSTOMER_TYPES.iter().map(|(v, l)| json!([v, l])).collect::<Vec<_>>());
    put(vm, "activity_type", kind);
    page(vm, "activity", &shown, p, "page");
    Ok(())
}
