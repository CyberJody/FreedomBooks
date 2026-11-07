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
    Ok(tx.query_row(&format!("SELECT i.number FROM {table} d JOIN documents_documentidentity i ON i.id=d.identity_id WHERE d.id=?1"),[key],|r|r.get::<_,String>(0)).map(|s|json!({"number":s}))?)
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
        let data = fb_core::sales::customer_activity(&mut app.open_mut()?.book, actor, c, id)?;
        let mut invoices = data["invoices"].as_array().unwrap().clone();
        for r in &mut invoices {
            r["invoice"] = json!({"identity":app.open_mut()?.book.read(|tx|number(tx,"sales_invoice",r["invoice_id"].as_str().unwrap()))?});
        }
        // Django's queryset contains current InvoiceRevision rows: -pk means revision order.
        for row in &mut invoices {
            row["current_revision_id"] = json!(app.open_mut()?.book.read(|tx| tx.query_row(
                "SELECT r.id FROM sales_invoicerevision r JOIN sales_invoice i ON i.id=r.invoice_id AND i.revision=r.number WHERE i.id=?1",
                [row["invoice_id"].as_str().unwrap()], |r| r.get::<_,String>(0)))?);
        }
        newest(&mut invoices, "date", "current_revision_id");
        page(vm, "customer_invoices", &invoices, p, "invoices_page");
        let mut receipts = data["receipts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["kind"] == "RCP" || r["kind"] == "SR")
            .cloned()
            .collect::<Vec<_>>();
        for r in &mut receipts {
            r["pk"] = r["id"].clone();
            r["identity"] = app
                .open_mut()?
                .book
                .read(|tx| number(tx, "sales_receipt", r["id"].as_str().unwrap()))?;
            r["reversed"] = json!(!r["reversal"].is_null());
        }
        newest(&mut receipts, "date", "id");
        page(
            vm,
            "customer_receipts",
            &receipts
                .iter()
                .filter(|r| r["kind"] == "RCP")
                .cloned()
                .collect::<Vec<_>>(),
            p,
            "receipts_page",
        );
        page(
            vm,
            "customer_sales_receipts",
            &receipts
                .iter()
                .filter(|r| r["kind"] == "SR")
                .cloned()
                .collect::<Vec<_>>(),
            p,
            "sales_receipts_page",
        );
        let mut memos = fb_core::credits::memo_balances(&mut app.open_mut()?.book, actor, c)?
            .into_iter()
            .filter(|r| r["customer_id"] == id)
            .collect::<Vec<_>>();
        let available = total(memos.iter(), "available")?;
        let open = total(
            invoices.iter().filter(|r| r["state"] == "OPEN"),
            "outstanding",
        )?;
        for r in &mut memos {
            let memo = fb_core::credits::get_credit_memo(
                &mut app.open_mut()?.book,
                actor,
                c,
                r["id"].as_str().unwrap(),
            )?;
            r["date"] = memo["date"].clone();
            r["pk"] = r["id"].clone();
            r["identity"] = app
                .open_mut()?
                .book
                .read(|tx| number(tx, "credits_creditmemo", r["id"].as_str().unwrap()))?;
            r["state_label"] = json!(state(r["credit_state"].as_str().unwrap(), false));
        }
        newest(&mut memos, "date", "id");
        put(
            vm,
            "customer_credit_memos",
            memos.into_iter().take(10).collect::<Vec<_>>(),
        );
        put(vm, "customer_open_total", cash(open));
        put(vm, "customer_available_credit", cash(available));
        put(
            vm,
            "customer_net_balance",
            cash(
                open.checked_sub(available)
                    .ok_or_else(|| AppError::Invalid("Activity total overflow.".into()))?,
            ),
        );
        let mut deposits =
            fb_core::prepayments::deposit_balances(&mut app.open_mut()?.book, actor, c)?
                .into_iter()
                .filter(|r| r["customer_id"] == id)
                .collect::<Vec<_>>();
        put(
            vm,
            "customer_unapplied_deposits",
            cash(total(deposits.iter(), "remaining")?),
        );
        for r in &mut deposits {
            r["pk"] = r["id"].clone();
            r["identity"] = app
                .open_mut()?
                .book
                .read(|tx| number(tx, "sales_receipt", r["id"].as_str().unwrap()))?;
            r["state_label"] = json!(state(r["deposit_state"].as_str().unwrap(), true));
        }
        newest(&mut deposits, "date", "id");
        put(
            vm,
            "customer_deposits",
            deposits.into_iter().take(10).collect::<Vec<_>>(),
        );
        let (lines,refunds)=app.open_mut()?.book.read(|tx|->Result<_,FbError>{
            let mut st=tx.prepare("SELECT l.amount,a.date,a.credit_memo_id,l.invoice_id,NOT EXISTS(SELECT 1 FROM credits_creditunapplication u WHERE u.application_id=a.id) FROM credits_creditapplicationline l JOIN credits_creditapplication a ON a.id=l.application_id JOIN credits_creditmemo m ON m.id=a.credit_memo_id WHERE a.company_id=?1 AND m.customer_id=?2 ORDER BY a.date DESC,l.id DESC LIMIT 10")?;
            let raw=st.query_map(params![c.to_db(),id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,bool>(4)?)))?.collect::<Result<Vec<_>,_>>()?;
            let mut lines=vec![];for (amount,date,memo,invoice,live) in raw{lines.push(json!({"amount":cash(i128::from(amount)),"application":{"date":date,"credit_memo_id":memo,"credit_memo":{"identity":number(tx,"credits_creditmemo",&memo)?},"live":live},"invoice_id":invoice,"invoice":{"identity":number(tx,"sales_invoice",&invoice)?}}));}
            let mut st=tx.prepare("SELECT r.id,r.date,r.amount,r.credit_memo_id,EXISTS(SELECT 1 FROM credits_creditrefundvoid v WHERE v.refund_id=r.id) FROM credits_creditrefund r WHERE r.company_id=?1 AND r.customer_id=?2 ORDER BY r.date DESC,r.id DESC LIMIT 10")?;
            let raw=st.query_map(params![c.to_db(),id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?,r.get::<_,bool>(4)?)))?.collect::<Result<Vec<_>,_>>()?;
            let mut refunds=vec![];for (id,date,amount,memo,voided) in raw{refunds.push(json!({"pk":id,"date":date,"amount":cash(i128::from(amount)),"credit_memo_id":memo,"credit_memo":{"identity":number(tx,"credits_creditmemo",&memo)?},"identity":number(tx,"credits_creditrefund",&id)?,"voided":voided}));}Ok((lines,refunds))
        })?;
        put(vm, "customer_credit_lines", lines);
        put(vm, "customer_refunds", refunds);
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
