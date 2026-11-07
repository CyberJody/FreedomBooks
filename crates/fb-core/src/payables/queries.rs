//! Current balances and the non-HTTP tracker/calendar contracts. All sums happen in Rust.
use super::*;
use chrono::{Datelike, Duration, NaiveDate};

pub(super) fn current(tx: &(impl Query + ?Sized), co: CompanyId) -> FbResult<Vec<Value>> {
    // Fixed three indexed reads, not a query per bill. No SQL money arithmetic.
    let headers = rows(
        tx,
        "SELECT r.*,b.company_id,b.identity_id,b.creation_digest,b.reference_key,b.revision,i.number AS document_number FROM payables_bill b JOIN payables_billrevision r ON r.bill_id=b.id AND r.number=b.revision JOIN documents_documentidentity i ON i.id=b.identity_id WHERE b.company_id=? ORDER BY b.id",
        &[&co.to_db()],
    )?;
    let mut lines: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for l in rows(
        tx,
        "SELECT l.* FROM payables_billline l JOIN payables_billrevision r ON r.id=l.revision_id JOIN payables_bill b ON b.id=r.bill_id AND b.revision=r.number WHERE b.company_id=? ORDER BY l.position",
        &[&co.to_db()],
    )? {
        lines
            .entry(s(&l, "revision_id").into())
            .or_default()
            .push(l);
    }
    let mut paid: BTreeMap<String, (i64, String)> = BTreeMap::new();
    for a in rows(
        tx,
        "SELECT a.bill_id,a.amount,p.date FROM fb_payables_live_allocations a JOIN payables_billpayment p ON p.id=a.payment_id WHERE p.company_id=?",
        &[&co.to_db()],
    )? {
        let e = paid.entry(s(&a, "bill_id").into()).or_default();
        e.0 += stored_i64(&a, "amount")?;
        if s(&a, "date") > e.1.as_str() {
            e.1 = s(&a, "date").into();
        }
    }
    let mut out = vec![];
    for mut r in headers {
        let (allocated, paid_on) = paid.remove(s(&r, "bill_id")).unwrap_or_default();
        let total = stored_i64(&r, "total")?;
        let state = s(&r, "state").to_string();
        let mut b = json!({"id":r["bill_id"],"vendor_id":r["vendor_id"],"state":r["state"],"number":r["document_number"]});
        for k in [
            "company_id",
            "identity_id",
            "creation_digest",
            "reference_key",
            "revision",
        ] {
            b[k] = object_mut(&mut r)?
                .remove(k)
                .ok_or_else(|| err("Invalid stored bill header."))?;
        }
        object_mut(&mut r)?.remove("document_number");
        r["lines"] = json!(lines.remove(s(&r, "id")).unwrap_or_default());
        b["current"] = r;
        b = money_display(b)?;
        b["allocated"] = json!(Money::from_db(allocated).to_string());
        b["outstanding"] = if state == "OPEN" {
            json!(Money::from_db(total - allocated).to_string())
        } else {
            Value::Null
        };
        b["payment_state"] = json!(if state != "OPEN" {
            state.as_str()
        } else if allocated == total {
            "PAID"
        } else if allocated > 0 {
            "PARTIAL"
        } else {
            "OPEN"
        });
        b["paid_on"] = if paid_on.is_empty() {
            Value::Null
        } else {
            json!(paid_on)
        };
        out.push(b);
    }
    Ok(out)
}
fn scoped(b: &Value, filters: &Value) -> bool {
    let r = &b["current"];
    let q = fb_db::collation::upper(s(filters, "q"));
    (q.is_empty()
        || [
            s(b, "number"),
            s(&r["vendor_snapshot"], "display_name"),
            s(r, "reference"),
            s(r, "memo"),
        ]
        .iter()
        .any(|v| fb_db::collation::upper(v).contains(&q)))
        && (s(filters, "vendor").is_empty() || filters["vendor"] == r["vendor_id"])
        && (s(filters, "start").is_empty() || s(r, "due_date") >= s(filters, "start"))
        && (s(filters, "end").is_empty() || s(r, "due_date") <= s(filters, "end"))
}
fn day(value: &Value) -> FbResult<NaiveDate> {
    NaiveDate::parse_from_str(&date(value)?, "%Y-%m-%d")
        .map_err(|_| err("Enter a valid calendar date."))
}
fn recent(b: &Value, cutoff: &str) -> bool {
    s(b, "payment_state") == "PAID" && s(b, "paid_on") >= cutoff
}
fn open(b: &Value) -> bool {
    ["OPEN", "PARTIAL"].contains(&s(b, "payment_state"))
        && cents(&b["outstanding"]).is_ok_and(|n| n > 0)
}
fn display_sum(n: i128) -> String {
    rust_decimal::Decimal::from_i128_with_scale(n, 2).to_string()
}
/// Tracker filters, independently calculated totals, and scoped rows. No URL or template concerns.
/// The frozen PAID_RECENT rule intentionally has no upper date bound (future payments count).
pub fn bill_tracker(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    filters: &Value,
    today: &Value,
) -> FbResult<Value> {
    let today = day(today)?;
    let cutoff = today
        .checked_sub_signed(Duration::days(30))
        .ok_or_else(|| err("Enter a valid calendar date."))?
        .to_string();
    let today = today.to_string();
    book.read(|tx|{services::access(tx,actor,co)?;let bills=current(tx,co)?.into_iter().filter(|b|scoped(b,filters)).collect::<Vec<_>>();let(mut owed,mut overdue,mut paid,mut drafts)=(0i128,0i128,0i128,0usize);for b in &bills{if open(b){let n=cents(&b["outstanding"])? as i128;owed+=n;if s(&b["current"],"due_date")<today.as_str(){overdue+=n;}}
if recent(b,&cutoff){paid+=cents(&b["current"]["total"])? as i128;}
if s(b,"payment_state")=="DRAFT"{drafts+=1;}}
let selected=bills.into_iter().filter(|b|match s(filters,"state"){""=>true,"OUTSTANDING"=>open(b),"OVERDUE"=>open(b)&&s(&b["current"],"due_date")<today.as_str(),"PAID_RECENT"=>recent(b,&cutoff),state=>s(b,"payment_state")==state}).collect::<Vec<_>>();Ok(json!({"rows":selected,"totals":{"open_money":display_sum(owed),"overdue_money":display_sum(overdue),"paid_recent_money":display_sum(paid),"draft_count":drafts,"currency":store::get_company(tx,co)?.currency,"today":today}}))})
}
/// Plain accounting dates; the host supplies today in its explicit display timezone.
#[allow(clippy::too_many_arguments)]
pub fn bill_calendar(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    year: i32,
    month: u32,
    today: &Value,
    include_paid: bool,
    vendor: Option<&str>,
) -> FbResult<Value> {
    let today = day(today)?.to_string();
    if !(1900..=2200).contains(&year) {
        return Err(err("Calendar year is outside the supported range."));
    }
    let first = NaiveDate::from_ymd_opt(year, month, 1)
        .ok_or_else(|| err("Choose a valid calendar month."))?;
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    }
    .ok_or_else(|| err("Choose a valid calendar month."))?;
    book.read(|tx|{services::access(tx,actor,co)?;let mut entries=vec![];for b in current(tx,co)?{let r=&b["current"];if s(&b,"state")!="OPEN" || s(r,"due_date")<first.to_string().as_str() || s(r,"due_date")>=next.to_string().as_str() || vendor.is_some_and(|id|s(r,"vendor_id")!=id) || (!include_paid && !open(&b)){continue;}let status=if s(&b,"payment_state")=="PAID"{"paid"}else if s(r,"due_date")<today.as_str(){"overdue"}else if s(r,"due_date")==today{"today"}else{"upcoming"};entries.push(json!({"bill_id":b["id"],"number":b["number"],"vendor_id":r["vendor_id"],"vendor_name":r["vendor_snapshot"]["display_name"],"reference":r["reference"],"due_date":r["due_date"],"total":r["total"],"remaining":b["outstanding"],"payment_state":b["payment_state"],"status":status}));}entries.sort_by(|a,b|(s(a,"due_date"),s(a,"bill_id")).cmp(&(s(b,"due_date"),s(b,"bill_id"))));let mut days=vec![];let mut day_totals=json!({});let mut total=0i128;let mut d=first;while d<next{let es=entries.iter().filter(|e|s(e,"due_date")==d.to_string()).cloned().collect::<Vec<_>>();let mut sum=0i128;for e in &es{if s(e,"payment_state")!="PAID"{sum+=cents(&e["remaining"])? as i128;}}total+=sum;if !es.is_empty(){day_totals[d.to_string()]=json!(display_sum(sum));}days.push(json!({"date":d.to_string(),"entries":es,"total":display_sum(sum)}));d+=Duration::days(1);}
let agenda=days.iter().filter(|d|!d["entries"].as_array().unwrap().is_empty()).cloned().collect::<Vec<_>>();let mut cells=vec![Value::Null;first.weekday().num_days_from_monday() as usize];cells.extend(days.clone());while !cells.len().is_multiple_of(7){cells.push(Value::Null);}let weeks=cells.chunks(7).map(|c|json!(c)).collect::<Vec<_>>();Ok(json!({"entries":entries,"days":days,"weeks":weeks,"agenda":agenda,"day_totals":day_totals,"month_total":display_sum(total),"month_label":first.format("%B %Y").to_string(),"weekdays":["Monday","Tuesday","Wednesday","Thursday","Friday","Saturday","Sunday"]}))})
}
