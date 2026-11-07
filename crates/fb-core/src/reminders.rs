//! Read-only reminder lists. The shared live-allocation views provide the tracker's balance rules.
//! The host supplies today in the display timezone. Both groups use a single SQLCipher read snapshot.
use crate::{
    Book, CompanyId, FbError, FbResult, UserId, directory, party_display, services, store,
};
use chrono::{Duration, NaiveDate};
use fb_db::Query;
use serde_json::{Value, json};
use std::collections::BTreeMap;
pub const ROW_CAP: usize = 25;
fn date(s: &str) -> FbResult<NaiveDate> {
    let v = fb_db::dates::parse_iso_date(s)
        .ok_or_else(|| FbError::validation("Enter a valid date."))?;
    NaiveDate::parse_from_str(&v, "%Y-%m-%d")
        .map_err(|_| FbError::validation("Enter a valid date."))
}
fn display(n: i128) -> String {
    format!("{}.{:02}", n / 100, n.unsigned_abs() % 100)
}
pub fn when(days: i64) -> String {
    if days < 0 {
        let n = days.unsigned_abs();
        format!("{n} day{} overdue", if n == 1 { "" } else { "s" })
    } else if days == 0 {
        "Due today".into()
    } else {
        format!("Due in {days} day{}", if days == 1 { "" } else { "s" })
    }
}
#[derive(Clone)]
struct Row {
    kind: String,
    id: String,
    number: String,
    party: String,
    due: NaiveDate,
    balance: i128,
}
// No SQL money arithmetic. One indexed query for both document families and their live settlements.
const CURRENT:&str="SELECT 'bill',b.id,d.number,r.vendor_id,r.due_date,r.total,a.amount FROM payables_bill b JOIN payables_billrevision r ON r.bill_id=b.id AND r.number=b.revision JOIN documents_documentidentity d ON d.id=b.identity_id LEFT JOIN fb_payables_live_allocations a ON a.bill_id=b.id WHERE b.company_id=?1 AND r.state='OPEN'
 UNION ALL SELECT 'invoice',i.id,d.number,r.customer_id,r.due_date,r.total,a.amount FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.number=i.revision JOIN documents_documentidentity d ON d.id=i.identity_id LEFT JOIN prepayments_live_settlements a ON a.invoice_id=i.id WHERE i.company_id=?1 AND r.state='OPEN'";
fn current(c: &(impl Query + ?Sized), co: CompanyId) -> FbResult<Vec<Row>> {
    let mut stmt = c.prepare(CURRENT)?;
    let mut map = BTreeMap::<(String, String), Row>::new();
    for raw in stmt.query_map([co.to_db()], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, i64>(5)?,
            r.get::<_, Option<i64>>(6)?,
        ))
    })? {
        let (k, id, number, party, due, total, paid) = raw?;
        let due = date(&due)?;
        let row = map.entry((k.clone(), id.clone())).or_insert(Row {
            kind: k,
            id,
            number,
            party,
            due,
            balance: i128::from(total),
        });
        row.balance = row
            .balance
            .checked_sub(i128::from(paid.unwrap_or(0)))
            .ok_or_else(|| FbError::validation("Reminder balance overflow."))?;
    }
    Ok(map.into_values().filter(|r| r.balance > 0).collect())
}
fn group(
    c: &(impl Query + ?Sized),
    co: CompanyId,
    mut rows: Vec<Row>,
    today: NaiveDate,
    model: directory::Model,
) -> FbResult<Value> {
    let mut total = 0i128;
    let mut overdue_total = 0i128;
    let mut overdue_count = 0usize;
    for row in &rows {
        total = total
            .checked_add(row.balance)
            .ok_or_else(|| FbError::validation("Reminder total overflow."))?;
        if row.due < today {
            overdue_count += 1;
            overdue_total = overdue_total
                .checked_add(row.balance)
                .ok_or_else(|| FbError::validation("Reminder total overflow."))?;
        }
    }
    rows.sort_by(|a, b| (a.due, &a.id).cmp(&(b.due, &b.id)));
    let count = rows.len();
    rows.truncate(ROW_CAP);
    let labels = if rows.is_empty() {
        json!({})
    } else {
        party_display::contact_label_map(c, co, model, None)?
    };
    let rows=rows.into_iter().map(|r|{let days=(r.due-today).num_days();json!({"document_id":r.id,"number":r.number,"party":labels[r.party],"due_date":r.due.to_string(),"balance":display(r.balance),"days":days,"overdue":days<0,"when":when(days)})}).collect::<Vec<_>>();
    Ok(
        json!({"count":count,"total":display(total),"overdue_count":overdue_count,"overdue_total":display(overdue_total),"capped":count>rows.len(),"rows":rows}),
    )
}
pub fn build(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    today: &str,
    days: Option<i64>,
) -> FbResult<Value> {
    let today = date(today)?;
    book.read(|c|{
        services::access(c,actor,co)?;let days=days.unwrap_or(store::get_company(c,co)?.bill_reminder_days);
        let through=today.checked_add_signed(Duration::try_days(days).ok_or_else(||FbError::validation("Reminder date overflow."))?).filter(|d|(1..=9999).contains(&chrono::Datelike::year(d))).ok_or_else(||FbError::validation("Reminder date overflow."))?;
        let rows=current(c,co)?;let bills=rows.iter().filter(|r|r.kind=="bill"&&r.due<=through).cloned().collect();let invoices=rows.into_iter().filter(|r|r.kind=="invoice"&&r.due<today).collect();
        Ok(json!({"today":today.to_string(),"days":days,"through":through.to_string(),"bills":group(c,co,bills,today,directory::Model::Vendor)?,"invoices":group(c,co,invoices,today,directory::Model::Customer)?}))
    })
}
/// The one document query above is independent of the number of open documents; due-soon bills do not count.
pub fn overdue_count(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    today: &str,
) -> FbResult<usize> {
    let today = date(today)?;
    book.read(|c| {
        services::access(c, actor, co)?;
        Ok(current(c, co)?.iter().filter(|r| r.due < today).count())
    })
}
