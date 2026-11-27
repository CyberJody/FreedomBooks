use super::*;

fn has(conn: &(impl Query + ?Sized), table: &str) -> FbResult<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |r| r.get(0),
    )?)
}
fn sum_rows(conn: &(impl Query + ?Sized), sql: &str, key: &str) -> FbResult<i64> {
    let mut stmt = conn.prepare(sql)?;
    let values = stmt
        .query_map([key], |r| r.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    values.into_iter().try_fold(0i64, |sum, value| {
        sum.checked_add(value)
            .ok_or_else(|| FbError::validation("Invoice settlements exceed supported limits."))
    })
}
pub(super) fn allocated(conn: &(impl Query + ?Sized), key: &str) -> FbResult<i64> {
    sum_rows(
        conn,
        "SELECT a.amount FROM sales_receiptallocation a WHERE a.invoice_id=?1 AND NOT EXISTS(SELECT 1 FROM sales_receiptreversal z WHERE z.receipt_id=a.receipt_id)",
        key,
    )
}
fn external(conn: &(impl Query + ?Sized), key: &str) -> FbResult<(i64, i64)> {
    let credits = if has(conn, "credits_creditapplicationline")? {
        sum_rows(
            conn,
            "SELECT l.amount FROM credits_creditapplicationline l WHERE l.invoice_id=?1 AND NOT EXISTS(SELECT 1 FROM credits_creditunapplication u WHERE u.application_id=l.application_id)",
            key,
        )?
    } else {
        0
    };
    let deposits = if has(conn, "prepayments_depositapplication")? {
        sum_rows(
            conn,
            "SELECT a.amount FROM prepayments_depositapplication a WHERE a.invoice_id=?1 AND NOT EXISTS(SELECT 1 FROM prepayments_depositunapplication u WHERE u.application_id=a.id)",
            key,
        )?
    } else {
        0
    };
    Ok((credits, deposits))
}
pub(super) fn settled(conn: &(impl Query + ?Sized), key: &str) -> FbResult<i64> {
    let (cr, dep) = external(conn, key)?;
    allocated(conn, key)?
        .checked_add(cr)
        .and_then(|n| n.checked_add(dep))
        .ok_or_else(|| FbError::validation("Invoice settlements exceed supported limits."))
}
/// The most settlement entries one receipt review lists, counted across all the invoices the
/// receipt is allocated to. A stated limit rather than paging: above it the review returns no
/// settlement, and a recording that requires one is refused.
pub const SETTLEMENT_ENTRY_LIMIT: usize = 500;
/// The rows of one family for one invoice, or `None` when there are more than `room`: the query
/// stops at the first row past it.
fn settlement_entries(
    conn: &(impl Query + ?Sized),
    kind: &str,
    sql: &str,
    key: &str,
    room: usize,
) -> FbResult<Option<Vec<Value>>> {
    let mut stmt = conn.prepare(sql)?;
    let entries = stmt
        .query_map(params![key, i64::try_from(room + 1).unwrap_or(i64::MAX)], |r| {
            Ok(json!({"id":r.get::<_, String>(0)?,"kind":kind,"amount_cents":r.get::<_, i64>(1)?.to_string(),"date":r.get::<_, String>(2)?,"reversed":r.get::<_, bool>(3)?}))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok((entries.len() <= room).then_some(entries))
}
/// Everything counted toward one invoice of this company, for a caller that must not record a
/// receipt beside money it has not seen. Read only for a review made with `include_settlement` and
/// for a recording that requires the same settlement; no other receipt path comes here.
/// `entries` lists the same three families `settled` sums (receipt allocations, credit memo
/// applications, customer deposit applications), the reversed and unapplied ones too, flagged.
/// `outstanding_cents` is the amount `settled` leaves open, the very number the allocation check
/// uses, and is null for an invoice that is not open, so it also moves if a family is ever added
/// there only. `room` is how many more entries the review may list; it is reduced by this
/// invoice's, and the answer is `None` when the invoice has more than that.
pub(super) fn settlement(
    conn: &(impl Query + ?Sized),
    c: CompanyId,
    key: &str,
    room: &mut usize,
) -> FbResult<Option<Value>> {
    // Only the current revision's header: the review before this, and the posting after it, load
    // and check the whole invoice.
    let header: Option<String> = conn
        .query_row(
            "SELECT r.data FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.number=i.revision WHERE i.company_id=?1 AND i.id=?2",
            params![c.to_db(), key],
            |r| r.get(0),
        )
        .optional()?;
    let h: Value = header
        .and_then(|data| fb_db::parse_unique_json_as(&data).ok())
        .ok_or_else(|| FbError::validation("Choose invoices belonging to this company."))?;
    let total = cents(&h["total"])?;
    let families = [
        ("receipt", None, "SELECT a.receipt_id,a.amount,r.date,EXISTS(SELECT 1 FROM sales_receiptreversal z WHERE z.receipt_id=a.receipt_id) FROM sales_receiptallocation a JOIN sales_receipt r ON r.id=a.receipt_id WHERE a.invoice_id=?1 LIMIT ?2"),
        ("credit", Some("credits_creditapplicationline"), "SELECT l.application_id,l.amount,p.date,EXISTS(SELECT 1 FROM credits_creditunapplication u WHERE u.application_id=l.application_id) FROM credits_creditapplicationline l JOIN credits_creditapplication p ON p.id=l.application_id WHERE l.invoice_id=?1 LIMIT ?2"),
        ("deposit", Some("prepayments_depositapplication"), "SELECT a.id,a.amount,a.date,EXISTS(SELECT 1 FROM prepayments_depositunapplication u WHERE u.application_id=a.id) FROM prepayments_depositapplication a WHERE a.invoice_id=?1 LIMIT ?2"),
    ];
    let mut entries = Vec::new();
    for (kind, table, sql) in families {
        if let Some(table) = table
            && !has(conn, table)?
        {
            continue;
        }
        let Some(rows) = settlement_entries(conn, kind, sql, key, *room - entries.len())? else {
            return Ok(None);
        };
        entries.extend(rows);
    }
    *room -= entries.len();
    // Oldest first, and one fixed order for the comparison at recording.
    let order = |v: &Value| [&v["date"], &v["kind"], &v["id"]].map(|part| part.as_str().unwrap_or("").to_owned());
    entries.sort_by_key(order);
    let outstanding = if h["state"] == "OPEN" {
        json!((total - settled(conn, key)?).to_string())
    } else {
        Value::Null
    };
    Ok(Some(json!({"invoice_id":key,"total_cents":total.to_string(),"outstanding_cents":outstanding,"entries":entries})))
}
pub(super) fn refuse_external_invoice_dependencies(
    conn: &(impl Query + ?Sized),
    key: &str,
    d: &str,
) -> FbResult<()> {
    crate::credits::refuse_live_credits(conn, key, d)?;
    let (_, dep) = external(conn, key)?;
    if dep > 0 {
        let number: String = conn.query_row(
            "SELECT d.number FROM prepayments_depositapplication a JOIN sales_receipt r ON r.id=a.deposit_id JOIN documents_documentidentity d ON d.id=r.identity_id WHERE a.invoice_id=?1 AND NOT EXISTS(SELECT 1 FROM prepayments_depositunapplication u WHERE u.application_id=a.id) ORDER BY a.id LIMIT 1",
            [key], |r| r.get(0),
        )?;
        return Err(FbError::validation(format!(
            "Unapply customer deposit {number} from this invoice before voiding it."
        )));
    }
    let released: Option<String> = conn.query_row("SELECT max(u.date) FROM prepayments_depositunapplication u JOIN prepayments_depositapplication a ON a.id=u.application_id WHERE a.invoice_id=?1",[key],|r|r.get(0))?;
    if let Some(released)=released.filter(|released| released.as_str()>d) {
        return Err(FbError::validation(format!("Void date cannot precede {released}, when a customer deposit was unapplied from this invoice.")));
    }
    Ok(())
}
pub(super) fn refuse_live_deposit(conn: &(impl Query + ?Sized), key: &str, d: &str) -> FbResult<()> {
    let company: String = conn.query_row("SELECT company_id FROM sales_receipt WHERE id=?", [key], |r| r.get(0))?;
    let company = CompanyId::parse(&company).ok_or_else(invalid_stored)?;
    crate::banking::refuse_live_deposit(conn, company, key, d)
}
/// Same current-only balances as Python. No second mutable paid flag.
pub fn invoice_balances(book: &mut Book, actor: UserId, c: CompanyId) -> FbResult<Vec<Value>> {
    book.read(|conn| invoice_balances_query(conn, actor, c))
}
pub(crate) fn invoice_balances_query(
    conn: &(impl Query + ?Sized),
    actor: UserId,
    c: CompanyId,
) -> FbResult<Vec<Value>> {
    services::access(conn, actor, c)?;
    let mut stmt = conn.prepare("SELECT id FROM sales_invoice WHERE company_id=?1 ORDER BY id")?;
    let keys = stmt
        .query_map([c.to_db()], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    keys.iter().map(|key| invoice_balance(conn, c, key)).collect()
}

/// Current settlement for one invoice, using the same balance as the invoice list.
pub fn invoice_balance(conn: &(impl Query + ?Sized), c: CompanyId, key: &str) -> FbResult<Value> {
    let inv = invoice(conn, c, key)?;
    let h = &inv["current"];
    let paid = allocated(conn, key)?;
    let (cr, dep) = external(conn, key)?;
    let total = cents(&h["total"])?;
    let sum = settled(conn, key)?;
    let state = match h["state"].as_str().ok_or_else(invalid_stored)? {
        "VOID" => "VOID",
        "DRAFT" => "DRAFT",
        _ if sum == total => "PAID",
        _ if sum > 0 => "PARTIAL",
        _ => "OPEN",
    };
    Ok(json!({"invoice_id":key,"customer_id":h["customer_id"],"date":h["date"],"due_date":h["due_date"],"state":h["state"],"total":h["total"],"allocated":cash(paid),"credited":cash(cr),"deposited":cash(dep),"settled":cash(sum),"outstanding":if h["state"]=="OPEN"{cash(total-sum)}else{Value::Null},"payment_state":state}))
}
/// Statement/activity source data only: presentation and aging reports belong to the reports wave.
pub fn customer_activity(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    customer: &str,
) -> FbResult<Value> {
    book.read(|conn| {
        services::access(conn, actor, c)?;
        directory::get(conn, c, Model::Customer, customer)?;
        let invoices = invoice_balances_query(conn, actor, c)?
            .into_iter()
            .filter(|r| r["customer_id"] == customer)
            .collect::<Vec<_>>();
        let mut stmt = conn.prepare(
        "SELECT data FROM sales_receipt WHERE company_id=?1 AND customer_id=?2 ORDER BY date,id",
    )?;
        let receipts = stmt
            .query_map(params![c.to_db(), customer], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|s| {
                fb_db::parse_unique_json_as::<Value>(&s)
                    .map_err(|_| FbError::validation("Invalid stored receipt."))
            })
            .collect::<FbResult<Vec<_>>>()?;
        let mut receipts = receipts;
        for row in &mut receipts {
            validate_stored(conn, c, "receipt", row)?;
            let raw: Option<String> = conn
                .query_row(
                    "SELECT data FROM sales_receiptreversal WHERE receipt_id=?1",
                    [row["id"].as_str().ok_or_else(invalid_stored)?],
                    |r| r.get(0),
                )
                .optional()?;
            row["reversal"] = raw
                .map(|s| fb_db::parse_unique_json_as::<Value>(&s).map_err(|_| invalid_stored()))
                .transpose()?
                .unwrap_or(Value::Null);
            if !row["reversal"].is_null() {
                let ok: bool = conn.query_row(
                    "SELECT fb_sales_reversal_ok(?1)",
                    [row["reversal"].to_string()],
                    |r| r.get(0),
                )?;
                if !ok {
                    return Err(invalid_stored());
                }
            }
        }
        Ok(json!({"invoices":invoices,"receipts":receipts}))
    })
}
