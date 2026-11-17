//! Job tickets and billable time. Tickets and time entries are work records, not accounting postings: nothing here
//! writes a journal. Invoicing a ticket creates an ordinary draft invoice through the sales service in the same
//! writer transaction and records which invoice line billed which entry. An entry stays billed while its line is on
//! the invoice and the invoice's current revision is not VOID: voiding or cancelling the invoice, or removing the line
//! from the draft, releases its time. While time is billed, its invoice keeps the customer and each time line keeps
//! its service, quantity and rate (the sales service refuses other edits; migration 0205's seal guard backs it up).
//!
//! Hours are exact ten-thousandths of an hour (the invoice quantity scale), rates are cents (the invoice rate
//! scale), and an entry's amount is half-up(hours x rate), the same rule the invoice applies to its lines.
use crate::{Book, CompanyId, FbError, FbResult, Money, UserId, directory::{self, Model}, documents, py, services, store};
use fb_db::rusqlite::{OptionalExtension, params};
use fb_db::{Query, Writer};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Ten-thousandths of an hour in one hour.
pub const HOUR: i64 = 10_000;
/// The longest single entry: 1,000 hours.
pub const MAX_HOURS: i64 = 1_000 * HOUR;
/// The largest hourly rate, in cents (99,999,999.99): keeps hours x rate exact in 64-bit SQLite arithmetic.
pub const MAX_RATE: i64 = 9_999_999_999;
pub const TITLE_MAX: usize = 200;
pub const DESCRIPTION_MAX: usize = 4000;
pub const NOTES_MAX: usize = 2000;
/// Invoice line limit of the sales service.
pub const INVOICE_LINES_MAX: usize = 50;
pub const STATUSES: [(&str, &str); 4] = [("OPEN", "Open"), ("COMPLETED", "Completed"), ("INVOICED", "Invoiced"), ("CLOSED", "Closed without billing")];

const HOURS_HELP: &str = "Enter hours such as 3.5, 3.5h, 3:30 or 90m (more than 0, up to 1000 hours, at most 4 decimal places).";

// ------------------------------------------------------------------ exact parsing and arithmetic

/// Parse typed hours into ten-thousandths: `3.5`, `3.5h`, `3.5 hrs`, `3:30`, `90m`, `1h 30m`.
/// H:MM and minutes convert exactly when they can and otherwise round half-up to the fourth decimal
/// (1:20 is 1.3333 h), the precision of an invoice quantity.
pub fn parse_hours(input: &str) -> FbResult<i64> {
    let bad = || FbError::validation(HOURS_HELP);
    let text = input.trim().to_ascii_lowercase();
    if text.is_empty() || text.len() > 32 {
        return Err(bad());
    }
    let units = if let Some((h, m)) = text.split_once(':') {
        let (h, m) = (h.trim(), m.trim());
        if h.is_empty() || !h.bytes().all(|b| b.is_ascii_digit()) || m.len() != 2 || !m.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad());
        }
        let minutes: i64 = m.parse().map_err(|_| bad())?;
        if minutes >= 60 || h.len() > 5 {
            return Err(bad());
        }
        let hours: i64 = h.parse().map_err(|_| bad())?;
        hours * HOUR + minutes_to_units(minutes)
    } else {
        // A sequence of <number><unit> parts: one decimal hours part, one whole minutes part, or hours then minutes.
        let mut rest = text.as_str();
        let mut hours: Option<i64> = None;
        let mut minutes: Option<i64> = None;
        while !rest.trim_start().is_empty() {
            rest = rest.trim_start();
            let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(rest.len());
            let (number, tail) = rest.split_at(end);
            if number.is_empty() {
                return Err(bad());
            }
            let tail = tail.trim_start();
            let unit_end = tail.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(tail.len());
            let (unit, after) = tail.split_at(unit_end);
            rest = after;
            match unit {
                "" | "h" | "hr" | "hrs" | "hour" | "hours" if hours.is_none() && minutes.is_none() => hours = Some(decimal_units(number).ok_or_else(bad)?),
                "m" | "min" | "mins" | "minute" | "minutes" if minutes.is_none() => {
                    if !number.bytes().all(|b| b.is_ascii_digit()) || number.len() > 6 {
                        return Err(bad());
                    }
                    let m: i64 = number.parse().map_err(|_| bad())?;
                    // With an hours part the minutes are the remainder of an hour.
                    if hours.is_some_and(|h| h % HOUR != 0) || (hours.is_some() && m >= 60) {
                        return Err(bad());
                    }
                    minutes = Some(m);
                }
                _ => return Err(bad()),
            }
            if unit.is_empty() && !rest.trim().is_empty() {
                return Err(bad());
            }
        }
        hours.unwrap_or(0) + minutes.map(minutes_to_units).unwrap_or(0)
    };
    if units <= 0 || units > MAX_HOURS {
        return Err(bad());
    }
    Ok(units)
}

fn minutes_to_units(minutes: i64) -> i64 {
    (minutes * HOUR * 2 + 60) / 120
}

/// `3`, `3.5`, `.25`, `0.3333`: at most four decimals, exact.
fn decimal_units(text: &str) -> Option<i64> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if (whole.is_empty() && fraction.is_empty()) || fraction.len() > 4 || whole.len() > 5 || text.matches('.').count() > 1 {
        return None;
    }
    if !whole.bytes().all(|b| b.is_ascii_digit()) || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let whole: i64 = if whole.is_empty() { 0 } else { whole.parse().ok()? };
    let padded = format!("{fraction:0<4}");
    Some(whole * HOUR + padded.parse::<i64>().ok()?)
}

/// Ten-thousandths as hours with at least two decimals: 35000 -> "3.50", 33333 -> "3.3333".
pub fn format_hours(units: i64) -> String {
    let whole = units / HOUR;
    let fraction = format!("{:04}", units % HOUR);
    let trimmed = fraction.trim_end_matches('0');
    format!("{whole}.{}", if trimmed.len() < 2 { &fraction[..2] } else { trimmed })
}

/// The invoice quantity text for ten-thousandths ("3.5000").
pub fn quantity_text(units: i64) -> String {
    format!("{}.{:04}", units / HOUR, units % HOUR)
}

/// half-up(hours x rate), the invoice line rule (`calculations::line_amount`), in cents.
pub fn amount(hours: i64, rate: i64) -> i64 {
    ((hours as i128 * rate as i128 + 5_000) / 10_000) as i64
}

/// A rate typed as money (`125`, `125.00`), in cents, within the time-entry limit.
pub fn parse_rate(input: &Value) -> FbResult<i64> {
    let decimal = crate::calculations::unit_rate(input).map_err(|_| FbError::validation("Enter the hourly rate as an amount such as 125.00."))?;
    let cents = Money::from_decimal_exact(decimal).ok_or_else(|| FbError::validation("Enter the hourly rate as an amount such as 125.00."))?.cents();
    if !(0..=MAX_RATE as i128).contains(&cents) {
        return Err(FbError::validation("The hourly rate must be between 0.00 and 99,999,999.99."));
    }
    Ok(cents as i64)
}

fn money(cents: i64) -> String {
    Money::from_db(cents).to_string()
}

// ------------------------------------------------------------------ input helpers

fn id_of(v: &Value, what: &str) -> FbResult<Option<String>> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) if s.is_empty() => Ok(None),
        Value::String(s) if fb_ids::parse_uuid(s).is_some_and(|u| u.to_string() == *s) => Ok(Some(s.clone())),
        _ => Err(FbError::validation(format!("Choose {what} from this company."))),
    }
}
fn request_key(v: &Value) -> FbResult<String> {
    v.as_str()
        .and_then(py::py_uuid)
        .map(|k| k.0)
        .ok_or_else(|| FbError::validation("This form is missing its request key. Open it again."))
}
fn date_of(v: &Value, required: bool, label: &str) -> FbResult<Option<String>> {
    match v.as_str().map(str::trim) {
        None | Some("") if !required => Ok(None),
        Some(s) => fb_db::dates::parse_iso_date(s).map(Some).ok_or_else(|| FbError::validation(format!("Enter a valid {label}."))),
        None => Err(FbError::validation(format!("Enter a valid {label}."))),
    }
}
fn text_of(v: &Value, max: usize, required: bool, label: &str, multiline: bool) -> FbResult<String> {
    let raw = match v {
        Value::Null => "",
        Value::String(s) => s.as_str(),
        _ => return Err(FbError::validation(format!("Enter the {label} as text."))),
    };
    if multiline { py::clean_multiline_free_text(raw)?; } else { py::clean_free_text(raw)?; }
    let s = py::py_strip(raw);
    if required && s.is_empty() {
        return Err(FbError::validation(format!("Enter the {label}.")));
    }
    if py::py_len(s) > max {
        return Err(FbError::validation(format!("Keep the {label} to {max} characters or fewer.")));
    }
    Ok(s.to_owned())
}
fn bool_of(v: &Value, default: bool) -> FbResult<bool> {
    match v {
        Value::Null => Ok(default),
        Value::Bool(b) => Ok(*b),
        _ => Err(FbError::validation("Invalid checkbox value.")),
    }
}
fn revision(v: &Value) -> Option<i64> {
    v.as_i64()
}

fn customer(conn: &(impl Query + ?Sized), c: CompanyId, key: &str) -> FbResult<Value> {
    directory::get(conn, c, Model::Customer, key).map_err(|_| FbError::validation("Choose a customer from this company."))
}
fn service_item(conn: &(impl Query + ?Sized), c: CompanyId, key: &str) -> FbResult<Value> {
    let item = directory::get(conn, c, Model::Item, key).map_err(|_| FbError::validation("Choose a service item from this company."))?;
    if item["kind"] != "SERVICE" {
        return Err(FbError::validation("Time is recorded against a service item. Choose a service, not a product."));
    }
    Ok(item)
}
fn worker(conn: &(impl Query + ?Sized), c: CompanyId, key: &str) -> FbResult<store::User> {
    let id = UserId::parse(key).ok_or_else(|| FbError::validation("Choose who did the work from this company's users."))?;
    match store::user(conn, id)? {
        Some(u) if u.is_active && store::is_member(conn, c, id)? => Ok(u),
        _ => Err(FbError::validation("Choose who did the work from this company's users.")),
    }
}

// ------------------------------------------------------------------ reads

/// Active members of the company, for "who did the work".
pub fn workers(conn: &(impl Query + ?Sized), c: CompanyId) -> FbResult<Vec<Value>> {
    let mut st = conn.prepare("SELECT u.id,u.username FROM fb_user u JOIN ledger_company_members m ON m.user_id=u.id WHERE m.company_id=?1 AND u.is_active=1 ORDER BY u.username COLLATE fb_text,u.id")?;
    let rows = st.query_map([c.to_db()], |r| Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?})))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Service items people record time against (active first, then archived for display).
pub fn services(conn: &(impl Query + ?Sized), c: CompanyId) -> FbResult<Vec<Value>> {
    Ok(directory::list(conn, c, Model::Item)?.into_iter().filter(|i| i["kind"] == "SERVICE").collect())
}

const ENTRY_COLUMNS: &str = "e.id,e.customer_id,e.ticket_id,e.item_id,e.date,e.hours,e.rate,e.amount,e.rate_override,e.billable,e.notes,e.worker_id,e.revision,e.created_at,e.updated_at,
 c.display_name,i.code,i.description,u.username,t.number,t.status,
 (SELECT l.invoice_id FROM jobs_livebilling l WHERE l.entry_id=e.id),
 (SELECT d.number FROM jobs_livebilling l JOIN sales_invoice s ON s.id=l.invoice_id JOIN documents_documentidentity d ON d.id=s.identity_id WHERE l.entry_id=e.id),
 (SELECT l.invoice_state FROM jobs_livebilling l WHERE l.entry_id=e.id),
 EXISTS(SELECT 1 FROM jobs_billing b WHERE b.entry_id=e.id),
 (SELECT l.invoiced FROM jobs_livebilling l WHERE l.entry_id=e.id)";
const ENTRY_FROM: &str = "FROM jobs_timeentry e JOIN directory_customer c ON c.id=e.customer_id JOIN directory_item i ON i.id=e.item_id
 JOIN fb_user u ON u.id=e.worker_id LEFT JOIN jobs_ticket t ON t.id=e.ticket_id";

fn entry_row(r: &fb_db::rusqlite::Row<'_>) -> fb_db::rusqlite::Result<Value> {
    let hours: i64 = r.get(5)?;
    let rate: i64 = r.get(6)?;
    let amount: i64 = r.get(7)?;
    let billable: bool = r.get(9)?;
    let invoice: Option<String> = r.get(21)?;
    let invoiced: Option<i64> = r.get(25)?;
    Ok(json!({
        "id": r.get::<_, String>(0)?, "customer_id": r.get::<_, String>(1)?, "ticket_id": r.get::<_, Option<String>>(2)?,
        "item_id": r.get::<_, String>(3)?, "date": r.get::<_, String>(4)?,
        "hours_units": hours, "hours": format_hours(hours), "rate_cents": rate, "rate": money(rate),
        "amount_cents": amount, "amount": money(amount), "rate_override": r.get::<_, bool>(8)?, "billable": billable,
        "notes": r.get::<_, String>(10)?, "worker_id": r.get::<_, String>(11)?, "revision": r.get::<_, i64>(12)?,
        "created_at": r.get::<_, String>(13)?, "updated_at": r.get::<_, String>(14)?,
        "customer_name": r.get::<_, String>(15)?, "item_code": r.get::<_, String>(16)?, "item_description": r.get::<_, String>(17)?,
        "worker_name": r.get::<_, String>(18)?, "ticket_number": r.get::<_, Option<String>>(19)?, "ticket_status": r.get::<_, Option<String>>(20)?,
        "billed": invoice.is_some(), "invoice_id": invoice, "invoice_number": r.get::<_, Option<String>>(22)?,
        "invoice_state": r.get::<_, Option<String>>(23)?, "has_history": r.get::<_, bool>(24)?,
        "unbilled": billable && r.get::<_, Option<String>>(21)?.is_none(),
        // The cents of its invoice line this entry accounts for (equal to `amount` unless lines were grouped).
        "invoiced_cents": invoiced, "invoiced": invoiced.map(money),
    }))
}

/// One time entry of this company.
pub fn entry(conn: &(impl Query + ?Sized), c: CompanyId, key: &str) -> FbResult<Value> {
    conn.query_row(&format!("SELECT {ENTRY_COLUMNS} {ENTRY_FROM} WHERE e.company_id=?1 AND e.id=?2"), params![c.to_db(), key], entry_row)
        .optional()?
        .ok_or_else(|| FbError::missing("Time entry"))
}

/// Which entries a list shows.
#[derive(Default, Clone, Debug)]
pub struct EntryFilter {
    pub customer_id: Option<String>,
    pub ticket_id: Option<String>,
    /// Only entries on no ticket.
    pub unassigned: bool,
    /// Only billable entries that are not on a live invoice.
    pub unbilled: bool,
    pub start: Option<String>,
    pub end: Option<String>,
}

/// Time entries, newest first.
pub fn entries(conn: &(impl Query + ?Sized), c: CompanyId, f: &EntryFilter) -> FbResult<Vec<Value>> {
    let mut sql = format!("SELECT {ENTRY_COLUMNS} {ENTRY_FROM} WHERE e.company_id=?1");
    let mut args: Vec<String> = vec![c.to_db()];
    let mut add = |clause: &str, value: &str, args: &mut Vec<String>| {
        args.push(value.to_owned());
        sql.push_str(&clause.replace('?', &format!("?{}", args.len())));
    };
    if let Some(v) = &f.customer_id { add(" AND e.customer_id=?", v, &mut args); }
    if let Some(v) = &f.ticket_id { add(" AND e.ticket_id=?", v, &mut args); }
    if let Some(v) = &f.start { add(" AND e.date>=?", v, &mut args); }
    if let Some(v) = &f.end { add(" AND e.date<=?", v, &mut args); }
    if f.unassigned { sql.push_str(" AND e.ticket_id IS NULL"); }
    if f.unbilled { sql.push_str(" AND e.billable=1 AND NOT EXISTS(SELECT 1 FROM jobs_livebilling l WHERE l.entry_id=e.id)"); }
    sql.push_str(" ORDER BY e.date DESC, e.created_at DESC, e.id DESC");
    let mut st = conn.prepare(&sql)?;
    let rows = st.query_map(fb_db::rusqlite::params_from_iter(args.iter()), entry_row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Totals over entries: hours, billable hours, unbilled amounts (time value) and billed amounts (what the invoice
/// lines charge for that time), exact.
pub fn totals(entries: &[Value]) -> Value {
    let (mut hours, mut billable_hours, mut nonbillable_hours, mut unbilled_hours) = (0i64, 0i64, 0i64, 0i64);
    let (mut billable, mut unbilled, mut billed) = (0i64, 0i64, 0i64);
    for e in entries {
        let h = e["hours_units"].as_i64().unwrap_or(0);
        let a = e["amount_cents"].as_i64().unwrap_or(0);
        hours += h;
        if e["billable"] == true {
            billable_hours += h;
            billable += a;
            if e["billed"] == true { billed += e["invoiced_cents"].as_i64().unwrap_or(a); } else { unbilled += a; unbilled_hours += h; }
        } else {
            nonbillable_hours += h;
        }
    }
    json!({"hours":format_hours(hours),"billable_hours":format_hours(billable_hours),"nonbillable_hours":format_hours(nonbillable_hours),
        "unbilled_hours":format_hours(unbilled_hours),"unbilled_hours_units":unbilled_hours,"billable":money(billable),"unbilled":money(unbilled),
        "unbilled_cents":unbilled,"billed":money(billed),"count":entries.len()})
}

const TICKET_COLUMNS: &str = "t.id,t.number,t.ordinal,t.customer_id,c.display_name,t.title,t.description,t.status,t.opened_date,t.due_date,t.finished_date,t.revision,t.created_at,t.updated_at,u.username,
 coalesce((SELECT sum(hours) FROM jobs_timeentry WHERE ticket_id=t.id),0),
 coalesce((SELECT sum(e.amount) FROM jobs_timeentry e WHERE e.ticket_id=t.id AND e.billable=1 AND NOT EXISTS(SELECT 1 FROM jobs_livebilling l WHERE l.entry_id=e.id)),0),
 (SELECT count(*) FROM jobs_timeentry e WHERE e.ticket_id=t.id AND e.billable=1 AND NOT EXISTS(SELECT 1 FROM jobs_livebilling l WHERE l.entry_id=e.id))";

fn ticket_row(r: &fb_db::rusqlite::Row<'_>) -> fb_db::rusqlite::Result<Value> {
    let status: String = r.get(7)?;
    let unbilled: i64 = r.get(16)?;
    Ok(json!({
        "id": r.get::<_, String>(0)?, "number": r.get::<_, String>(1)?, "ordinal": r.get::<_, i64>(2)?,
        "customer_id": r.get::<_, String>(3)?, "customer_name": r.get::<_, String>(4)?, "title": r.get::<_, String>(5)?,
        "description": r.get::<_, String>(6)?, "status_label": STATUSES.iter().find(|(k, _)| *k == status).map(|(_, l)| *l).unwrap_or(""),
        "status": status, "opened_date": r.get::<_, String>(8)?, "due_date": r.get::<_, Option<String>>(9)?,
        "finished_date": r.get::<_, Option<String>>(10)?, "revision": r.get::<_, i64>(11)?, "created_at": r.get::<_, String>(12)?,
        "updated_at": r.get::<_, String>(13)?, "created_by": r.get::<_, String>(14)?, "hours": format_hours(r.get(15)?),
        "unbilled": money(unbilled), "unbilled_cents": unbilled, "unbilled_count": r.get::<_, i64>(17)?,
    }))
}
const TICKET_FROM: &str = "FROM jobs_ticket t JOIN directory_customer c ON c.id=t.customer_id JOIN fb_user u ON u.id=t.created_by_id";

fn ticket_header(conn: &(impl Query + ?Sized), c: CompanyId, key: &str) -> FbResult<Value> {
    conn.query_row(&format!("SELECT {TICKET_COLUMNS} {TICKET_FROM} WHERE t.company_id=?1 AND t.id=?2"), params![c.to_db(), key], ticket_row)
        .optional()?
        .ok_or_else(|| FbError::missing("Job ticket"))
}

/// One ticket with its entries (oldest first), totals and the invoices made from it.
pub fn ticket(conn: &(impl Query + ?Sized), c: CompanyId, key: &str) -> FbResult<Value> {
    let mut t = ticket_header(conn, c, key)?;
    let mut list = entries(conn, c, &EntryFilter { ticket_id: Some(key.to_owned()), ..Default::default() })?;
    list.reverse();
    t["totals"] = totals(&list);
    t["entries"] = json!(list);
    let mut st = conn.prepare(
        "SELECT s.id,d.number,r.state,r.total,r.date,count(b.id),coalesce(sum(b.invoiced),0) FROM jobs_billing b JOIN jobs_billingline p ON p.id=b.id
         JOIN sales_invoice s ON s.id=b.invoice_id
         JOIN documents_documentidentity d ON d.id=s.identity_id JOIN sales_invoicerevision r ON r.invoice_id=s.id AND r.number=s.revision
         WHERE b.ticket_id=?1 AND b.company_id=?2 AND p.line_position IS NOT NULL GROUP BY s.id ORDER BY min(b.created_at),s.id")?;
    let invoices = st.query_map(params![key, c.to_db()], |r| Ok(json!({"id":r.get::<_,String>(0)?,"number":r.get::<_,String>(1)?,"state":r.get::<_,String>(2)?,
        "total":money(r.get(3)?),"date":r.get::<_,String>(4)?,"entries":r.get::<_,i64>(5)?,"amount":money(r.get(6)?)})))?.collect::<Result<Vec<_>, _>>()?;
    t["invoices"] = json!(invoices);
    Ok(t)
}

/// Which tickets a list shows.
#[derive(Default, Clone, Debug)]
pub struct TicketFilter {
    pub customer_id: Option<String>,
    /// One status, or `ACTIVE` for open and completed.
    pub status: Option<String>,
}

/// Tickets, newest number first.
pub fn tickets(conn: &(impl Query + ?Sized), c: CompanyId, f: &TicketFilter) -> FbResult<Vec<Value>> {
    let mut sql = format!("SELECT {TICKET_COLUMNS} {TICKET_FROM} WHERE t.company_id=?1");
    let mut args = vec![c.to_db()];
    if let Some(v) = &f.customer_id {
        args.push(v.clone());
        sql.push_str(&format!(" AND t.customer_id=?{}", args.len()));
    }
    match f.status.as_deref() {
        None | Some("") | Some("ALL") => {}
        Some("ACTIVE") => sql.push_str(" AND t.status IN('OPEN','COMPLETED')"),
        Some(s) if STATUSES.iter().any(|(k, _)| *k == s) => {
            args.push(s.to_owned());
            sql.push_str(&format!(" AND t.status=?{}", args.len()));
        }
        Some(_) => return Err(FbError::validation("Choose a ticket status.")),
    }
    sql.push_str(" ORDER BY t.ordinal DESC");
    let mut st = conn.prepare(&sql)?;
    let rows = st.query_map(fb_db::rusqlite::params_from_iter(args.iter()), ticket_row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Open tickets of one customer, for attaching and moving time.
pub fn open_tickets(conn: &(impl Query + ?Sized), c: CompanyId, customer_id: &str) -> FbResult<Vec<Value>> {
    tickets(conn, c, &TicketFilter { customer_id: Some(customer_id.to_owned()), status: Some("OPEN".into()) })
}

// ------------------------------------------------------------------ tickets

fn audit(tx: &Writer<'_>, user: &store::User, c: CompanyId, action: &str, target: String, detail: Value) -> FbResult<()> {
    services::audit(tx, user, c, action, target, detail)
}

/// Create or edit a ticket: `{ticket_id?, expected_revision?, request_key, customer_id, title, description, opened_date, due_date?}`.
/// The customer is fixed once the ticket exists.
pub fn save_ticket(book: &mut Book, actor: UserId, c: CompanyId, input: &Value) -> FbResult<Value> {
    book.write(|tx| tx.atomic(|| save_ticket_tx(tx, actor, c, input)))
}
fn save_ticket_tx(tx: &Writer<'_>, actor: UserId, c: CompanyId, input: &Value) -> FbResult<Value> {
    let user = services::access(tx, actor, c)?;
    let editing = id_of(&input["ticket_id"], "a job ticket")?;
    let title = text_of(&input["title"], TITLE_MAX, true, "title", false)?;
    let description = text_of(&input["description"], DESCRIPTION_MAX, false, "description", true)?;
    let now = py::now_utc();
    if let Some(key) = editing {
        let old = ticket_header(tx, c, &key)?;
        documents::check_revision(old["revision"].as_i64().unwrap_or(0), revision(&input["expected_revision"]))?;
        if !input["customer_id"].is_null() && input["customer_id"] != old["customer_id"] {
            return Err(FbError::validation("A job ticket stays with its customer. Start a new ticket for a different customer."));
        }
        let opened = date_of(&input["opened_date"], true, "opened date")?.unwrap();
        let due = date_of(&input["due_date"], false, "due date")?;
        check_dates(&opened, due.as_deref(), old["finished_date"].as_str())?;
        let rev = documents::next_revision(old["revision"].as_i64().unwrap_or(0))?;
        tx.execute("UPDATE jobs_ticket SET title=?2,description=?3,opened_date=?4,due_date=?5,revision=?6,updated_at=?7 WHERE id=?1",
            params![key, title, description, opened, due, rev, now])?;
        audit(tx, &user, c, "job_ticket.updated", format!("job_ticket:{key}"), json!({"number":old["number"],"revision":rev,"title":title}))?;
        return ticket(tx, c, &key);
    }
    let rk = request_key(&input["request_key"])?;
    if let Some(existing) = tx.query_row("SELECT id FROM jobs_ticket WHERE company_id=?1 AND request_key=?2", params![c.to_db(), rk], |r| r.get::<_, String>(0)).optional()? {
        let prior = ticket_header(tx, c, &existing)?;
        if prior["customer_id"] != input["customer_id"] || prior["title"] != json!(title) {
            return Err(FbError::validation("This retry key belongs to a different job ticket. Start a new ticket instead."));
        }
        return ticket(tx, c, &existing);
    }
    let customer_id = id_of(&input["customer_id"], "a customer")?.ok_or_else(|| FbError::validation("Choose a customer from this company."))?;
    let cust = customer(tx, c, &customer_id)?;
    if cust["active"] != true {
        return Err(FbError::validation("This customer is archived. Reactivate them in Customers before opening a ticket."));
    }
    let opened = date_of(&input["opened_date"], true, "opened date")?.unwrap();
    let due = date_of(&input["due_date"], false, "due date")?;
    check_dates(&opened, due.as_deref(), None)?;
    let ordinal: i64 = tx.query_row("SELECT coalesce(max(ordinal),0)+1 FROM jobs_ticket WHERE company_id=?1", [c.to_db()], |r| r.get(0))?;
    if ordinal > 999_999_999_999 {
        return Err(FbError::validation("Job ticket numbers are exhausted."));
    }
    let key = fb_ids::new_v7().to_string();
    let number = format!("JT-{ordinal:06}");
    tx.execute("INSERT INTO jobs_ticket(id,company_id,ordinal,number,request_key,customer_id,title,description,status,opened_date,due_date,finished_date,revision,created_by_id,created_at,updated_at)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'OPEN',?9,?10,NULL,1,?11,?12,?12)",
        params![key, c.to_db(), ordinal, number, rk, customer_id, title, description, opened, due, actor.to_db(), now])?;
    audit(tx, &user, c, "job_ticket.created", format!("job_ticket:{key}"), json!({"number":number,"customer_id":customer_id,"title":title}))?;
    ticket(tx, c, &key)
}
fn check_dates(opened: &str, due: Option<&str>, finished: Option<&str>) -> FbResult<()> {
    if due.is_some_and(|d| d < opened) {
        return Err(FbError::validation("The due date cannot be before the opened date."));
    }
    if finished.is_some_and(|d| d < opened) {
        return Err(FbError::validation("The opened date cannot be after the date the ticket was finished."));
    }
    Ok(())
}

/// Move a ticket to `OPEN` (reopen), `COMPLETED` or `CLOSED` (without billing). `INVOICED` is set by invoicing.
pub fn set_ticket_status(book: &mut Book, actor: UserId, c: CompanyId, key: &str, expected: Option<i64>, status: &str, when: &Value) -> FbResult<Value> {
    book.write(|tx| tx.atomic(|| {
        let user = services::access(tx, actor, c)?;
        let old = ticket_header(tx, c, key)?;
        let from = old["status"].as_str().unwrap_or("");
        if from == status && expected == Some(old["revision"].as_i64().unwrap_or(0) - 1) {
            return ticket(tx, c, key);
        }
        documents::check_revision(old["revision"].as_i64().unwrap_or(0), expected)?;
        let allowed = match status {
            "OPEN" => from != "OPEN",
            "COMPLETED" => from == "OPEN",
            "CLOSED" => from == "OPEN" || from == "COMPLETED",
            _ => false,
        };
        if !allowed {
            return Err(FbError::validation(match status {
                "INVOICED" => "A ticket is marked invoiced by creating its invoice.",
                "OPEN" => "This ticket is already open.",
                _ => "This ticket cannot move to that status from where it is now.",
            }));
        }
        let finished = if status == "OPEN" { None } else {
            let d = date_of(when, true, "date")?.unwrap();
            if Some(d.as_str()) < old["opened_date"].as_str() {
                return Err(FbError::validation("The ticket cannot finish before it was opened."));
            }
            Some(d)
        };
        let rev = documents::next_revision(old["revision"].as_i64().unwrap_or(0))?;
        tx.execute("UPDATE jobs_ticket SET status=?2,finished_date=?3,revision=?4,updated_at=?5 WHERE id=?1", params![key, status, finished, rev, py::now_utc()])?;
        let action = match status { "OPEN" => "job_ticket.reopened", "COMPLETED" => "job_ticket.completed", _ => "job_ticket.closed" };
        audit(tx, &user, c, action, format!("job_ticket:{key}"), json!({"number":old["number"],"from":from,"to":status,"date":finished,"revision":rev}))?;
        ticket(tx, c, key)
    }))
}

// ------------------------------------------------------------------ time entries

/// Record or edit time: `{entry_id?, expected_revision?, request_key, customer_id, ticket_id?, item_id, date, hours,
/// rate?, billable, notes, worker_id? (blank: the person signed in)}`. A blank rate is the service item's price for new
/// time or a changed service; an edit that keeps the service keeps its recorded rate unless a rate is typed.
/// Billed time cannot change until its invoice is voided.
pub fn save_entry(book: &mut Book, actor: UserId, c: CompanyId, input: &Value) -> FbResult<Value> {
    book.write(|tx| tx.atomic(|| save_entry_tx(tx, actor, c, input)))
}
pub(crate) fn save_entry_tx(tx: &Writer<'_>, actor: UserId, c: CompanyId, input: &Value) -> FbResult<Value> {
    let user = services::access(tx, actor, c)?;
    let editing = id_of(&input["entry_id"], "a time entry")?;
    let old = editing.as_deref().map(|k| entry(tx, c, k)).transpose()?;
    if let Some(o) = &old {
        documents::check_revision(o["revision"].as_i64().unwrap_or(0), revision(&input["expected_revision"]))?;
        if o["billed"] == true {
            return Err(FbError::validation(format!("This time is on invoice {}. Void that invoice to change the entry.", o["invoice_number"].as_str().unwrap_or(""))));
        }
    }
    let rk = if old.is_none() { Some(request_key(&input["request_key"])?) } else { None };
    if let Some(rk) = &rk
        && let Some(existing) = tx.query_row("SELECT id FROM jobs_timeentry WHERE company_id=?1 AND request_key=?2", params![c.to_db(), rk], |r| r.get::<_, String>(0)).optional()?
    {
        // A resubmitted form returns the entry it already made.
        let prior = entry(tx, c, &existing)?;
        if prior["customer_id"] != input["customer_id"] || prior["item_id"] != input["item_id"] {
            return Err(FbError::validation("This retry key belongs to different time. Open Enter time again."));
        }
        return Ok(prior);
    }
    let customer_id = id_of(&input["customer_id"], "a customer")?.ok_or_else(|| FbError::validation("Choose a customer from this company."))?;
    let cust = customer(tx, c, &customer_id)?;
    let item_id = id_of(&input["item_id"], "a service item")?.ok_or_else(|| FbError::validation("Choose a service item from this company."))?;
    let item = service_item(tx, c, &item_id)?;
    // A new entry needs active records; an edit may keep the archived ones it already has.
    if cust["active"] != true && old.as_ref().is_none_or(|o| o["customer_id"] != json!(customer_id)) {
        return Err(FbError::validation("This customer is archived. Reactivate them in Customers before recording time."));
    }
    if item["active"] != true && old.as_ref().is_none_or(|o| o["item_id"] != json!(item_id)) {
        return Err(FbError::validation("This service is archived. Reactivate it in Items & services before recording time."));
    }
    let date = date_of(&input["date"], true, "work date")?.unwrap();
    let hours = parse_hours(input["hours"].as_str().ok_or_else(|| FbError::validation(HOURS_HELP))?)?;
    let price = item["sale_rate"].as_str().map(|s| parse_rate(&json!(s))).transpose()?.unwrap_or(0);
    let typed = match &input["rate"] {
        Value::Null => None,
        Value::String(s) if s.trim().is_empty() => None,
        v => Some(parse_rate(&json!(v.as_str().map(str::trim).unwrap_or("")))?),
    };
    // Editing anything else never reprices recorded time: the same service keeps its rate (and how it was set).
    let kept = old.as_ref().filter(|o| o["item_id"] == json!(item_id)).map(|o| (o["rate_cents"].as_i64().unwrap_or(0), o["rate_override"] == true));
    let (rate, rate_override) = match (typed, kept) {
        (None, Some(k)) => k,
        (Some(t), Some((r, flag))) if t == r => (r, flag),
        (Some(t), _) => (t, t != price),
        (None, None) => (price, false),
    };
    let billable = bool_of(&input["billable"], true)?;
    let notes = text_of(&input["notes"], NOTES_MAX, false, "notes", true)?;
    let worker_id = match id_of(&input["worker_id"], "who did the work")? {
        Some(w) => w,
        None => old.as_ref().and_then(|o| o["worker_id"].as_str().map(str::to_owned)).unwrap_or(actor.to_db()),
    };
    if old.as_ref().is_none_or(|o| o["worker_id"] != json!(worker_id)) {
        worker(tx, c, &worker_id)?;
    }
    let ticket_id = id_of(&input["ticket_id"], "a job ticket")?;
    if let Some(t) = &ticket_id {
        let header = ticket_header(tx, c, t).map_err(|_| FbError::validation("Choose a job ticket from this company."))?;
        let unchanged = old.as_ref().is_some_and(|o| o["ticket_id"] == json!(t) && o["customer_id"] == json!(customer_id));
        if header["customer_id"] != json!(customer_id) {
            return Err(FbError::validation("Choose a job ticket of this customer, or no ticket."));
        }
        if header["status"] != "OPEN" && !unchanged {
            return Err(FbError::validation(format!("Ticket {} is not open. Reopen it, or choose an open ticket.", header["number"].as_str().unwrap_or(""))));
        }
    }
    let total = amount(hours, rate);
    let now = py::now_utc();
    let detail = json!({"date":date,"customer_id":customer_id,"ticket_id":ticket_id,"item_id":item_id,"hours":format_hours(hours),"rate":money(rate),
        "amount":money(total),"billable":billable,"worker_id":worker_id,"rate_override":rate_override});
    let key = if let Some(o) = &old {
        let key = o["id"].as_str().unwrap_or_default().to_owned();
        let rev = documents::next_revision(o["revision"].as_i64().unwrap_or(0))?;
        tx.execute("UPDATE jobs_timeentry SET customer_id=?2,ticket_id=?3,item_id=?4,date=?5,hours=?6,rate=?7,amount=?8,rate_override=?9,billable=?10,notes=?11,worker_id=?12,revision=?13,updated_at=?14 WHERE id=?1",
            params![key, customer_id, ticket_id, item_id, date, hours, rate, total, rate_override, billable, notes, worker_id, rev, now])?;
        let mut d = detail;
        d["revision"] = json!(rev);
        audit(tx, &user, c, "time_entry.updated", format!("time_entry:{key}"), d)?;
        key
    } else {
        let key = fb_ids::new_v7().to_string();
        tx.execute("INSERT INTO jobs_timeentry(id,company_id,request_key,customer_id,ticket_id,item_id,date,hours,rate,amount,rate_override,billable,notes,worker_id,revision,created_by_id,created_at,updated_at)
            VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,1,?15,?16,?16)",
            params![key, c.to_db(), rk, customer_id, ticket_id, item_id, date, hours, rate, total, rate_override, billable, notes, worker_id, actor.to_db(), now])?;
        audit(tx, &user, c, "time_entry.created", format!("time_entry:{key}"), detail)?;
        key
    };
    entry(tx, c, &key)
}

/// Attach time to one of its customer's open tickets, move it to another, or take it off (`ticket_id` None).
pub fn move_entry(book: &mut Book, actor: UserId, c: CompanyId, key: &str, expected: Option<i64>, ticket_id: Option<&str>) -> FbResult<Value> {
    book.write(|tx| tx.atomic(|| {
        let user = services::access(tx, actor, c)?;
        let old = entry(tx, c, key)?;
        documents::check_revision(old["revision"].as_i64().unwrap_or(0), expected)?;
        if old["billed"] == true {
            return Err(FbError::validation(format!("This time is on invoice {}. Void that invoice to move the entry.", old["invoice_number"].as_str().unwrap_or(""))));
        }
        if old["ticket_id"].as_str() == ticket_id {
            return Ok(old);
        }
        let to = ticket_id.map(|t| ticket_header(tx, c, t).map_err(|_| FbError::validation("Choose a job ticket from this company."))).transpose()?;
        if let Some(t) = &to {
            if t["customer_id"] != old["customer_id"] {
                return Err(FbError::validation("Time moves only between tickets of its own customer."));
            }
            if t["status"] != "OPEN" {
                return Err(FbError::validation(format!("Ticket {} is not open. Reopen it first.", t["number"].as_str().unwrap_or(""))));
            }
        }
        let rev = documents::next_revision(old["revision"].as_i64().unwrap_or(0))?;
        tx.execute("UPDATE jobs_timeentry SET ticket_id=?2,revision=?3,updated_at=?4 WHERE id=?1", params![key, ticket_id, rev, py::now_utc()])?;
        audit(tx, &user, c, "time_entry.moved", format!("time_entry:{key}"),
            json!({"from_ticket":old["ticket_number"],"to_ticket":to.as_ref().map(|t| t["number"].clone()),"ticket_id":ticket_id,"revision":rev}))?;
        entry(tx, c, key)
    }))
}

/// Remove time that was never invoiced. Time with invoice history is kept (mark it not billable instead).
pub fn delete_entry(book: &mut Book, actor: UserId, c: CompanyId, key: &str, expected: Option<i64>) -> FbResult<Value> {
    book.write(|tx| tx.atomic(|| {
        let user = services::access(tx, actor, c)?;
        let old = entry(tx, c, key)?;
        documents::check_revision(old["revision"].as_i64().unwrap_or(0), expected)?;
        if old["has_history"] == true {
            return Err(FbError::validation("This time has been on an invoice and is kept with that history. Mark it not billable instead."));
        }
        tx.execute("DELETE FROM jobs_timeentry WHERE id=?1 AND company_id=?2", params![key, c.to_db()])?;
        audit(tx, &user, c, "time_entry.deleted", format!("time_entry:{key}"),
            json!({"date":old["date"],"customer_id":old["customer_id"],"ticket_id":old["ticket_id"],"item_id":old["item_id"],"hours":old["hours"],"rate":old["rate"],"amount":old["amount"],"billable":old["billable"],"notes":old["notes"],"worker_id":old["worker_id"]}))?;
        Ok(json!({"id":key,"deleted":true,"ticket_id":old["ticket_id"]}))
    }))
}

// ------------------------------------------------------------------ invoicing

/// How a ticket's time becomes invoice lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grouping {
    /// One line per time entry.
    Entry,
    /// One line per service and rate, with the hours added up.
    Service,
}
impl Grouping {
    pub fn parse(s: &str) -> FbResult<Self> {
        match s {
            "ENTRY" => Ok(Self::Entry),
            "SERVICE" => Ok(Self::Service),
            _ => Err(FbError::validation("Choose one line per entry or one line per service.")),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self { Self::Entry => "ENTRY", Self::Service => "SERVICE" }
    }
}

/// Make a draft invoice from a ticket's unbilled billable time:
/// `{ticket_id, expected_revision, request_key, date, due_date?, grouping, entry_ids? (default: all unbilled), mark_invoiced}`.
/// The invoice is the sales service's own draft (review, post and void it as usual). Returns `{invoice_id, invoice_number, ticket}`.
pub fn invoice_ticket(book: &mut Book, actor: UserId, c: CompanyId, input: &Value) -> FbResult<Value> {
    book.write(|tx| tx.atomic(|| invoice_ticket_tx(tx, actor, c, input)))
}
fn invoice_ticket_tx(tx: &Writer<'_>, actor: UserId, c: CompanyId, input: &Value) -> FbResult<Value> {
    let user = services::access(tx, actor, c)?;
    let key = id_of(&input["ticket_id"], "a job ticket")?.ok_or_else(|| FbError::validation("Choose a job ticket from this company."))?;
    let t = ticket_header(tx, c, &key)?;
    let rk = request_key(&input["request_key"])?;
    // A retried submit returns the invoice this request already made.
    let made: Option<String> = tx.query_row(
        "SELECT s.id FROM documents_documentidentity d JOIN sales_invoice s ON s.identity_id=d.id WHERE d.company_id=?1 AND d.request_key=?2 AND d.kind='INV'",
        params![c.to_db(), rk], |r| r.get(0)).optional()?;
    if let Some(invoice_id) = made {
        let ours: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM jobs_billing WHERE invoice_id=?1 AND ticket_id=?2)", params![invoice_id, key], |r| r.get(0))?;
        if !ours {
            return Err(FbError::validation("This retry key belongs to a different invoice. Open the ticket again."));
        }
        return invoiced(tx, c, &key, &invoice_id);
    }
    documents::check_revision(t["revision"].as_i64().unwrap_or(0), revision(&input["expected_revision"]))?;
    if t["status"] == "CLOSED" {
        return Err(FbError::validation("This ticket was closed without billing. Reopen it to invoice its time."));
    }
    let grouping = Grouping::parse(input["grouping"].as_str().unwrap_or("ENTRY"))?;
    let date = date_of(&input["date"], true, "invoice date")?.unwrap();
    let due = date_of(&input["due_date"], false, "due date")?;
    let mut list = entries(tx, c, &EntryFilter { ticket_id: Some(key.clone()), unbilled: true, ..Default::default() })?;
    list.reverse();
    if let Some(chosen) = input.get("entry_ids").filter(|v| !v.is_null()) {
        let chosen = chosen.as_array().ok_or_else(|| FbError::validation("Choose the time to invoice."))?;
        let wanted: Vec<&str> = chosen.iter().filter_map(Value::as_str).collect();
        if wanted.len() != chosen.len() || wanted.iter().any(|w| !list.iter().any(|e| e["id"] == *w)) {
            return Err(FbError::validation("Some of the chosen time is already invoiced or not billable. Reload the ticket."));
        }
        list.retain(|e| wanted.contains(&e["id"].as_str().unwrap_or("")));
    }
    if list.is_empty() {
        return Err(FbError::validation("There is no unbilled billable time on this ticket."));
    }
    let cust = customer(tx, c, t["customer_id"].as_str().unwrap_or(""))?;
    // Lines: (item, quantity units, rate cents, description, entries).
    let mut lines: Vec<(String, i64, i64, String, Vec<usize>)> = Vec::new();
    match grouping {
        Grouping::Entry => {
            for (i, e) in list.iter().enumerate() {
                let mut text = format!("{}: {} on {}", e["item_code"].as_str().unwrap_or(""), e["worker_name"].as_str().unwrap_or(""), e["date"].as_str().unwrap_or(""));
                if let Some(n) = e["notes"].as_str().filter(|n| !n.is_empty()) {
                    text.push_str(" — ");
                    text.push_str(n);
                }
                lines.push((e["item_id"].as_str().unwrap_or("").into(), e["hours_units"].as_i64().unwrap_or(0), e["rate_cents"].as_i64().unwrap_or(0), text, vec![i]));
            }
        }
        Grouping::Service => {
            let mut groups: BTreeMap<(String, i64), usize> = BTreeMap::new();
            for (i, e) in list.iter().enumerate() {
                let k = (e["item_id"].as_str().unwrap_or("").to_owned(), e["rate_cents"].as_i64().unwrap_or(0));
                let at = *groups.entry(k.clone()).or_insert_with(|| {
                    lines.push((k.0.clone(), 0, k.1, String::new(), vec![]));
                    lines.len() - 1
                });
                lines[at].1 += e["hours_units"].as_i64().unwrap_or(0);
                lines[at].4.push(i);
            }
            for line in &mut lines {
                let first = &list[line.4[0]];
                let last = &list[*line.4.last().unwrap()];
                let (a, b) = (first["date"].as_str().unwrap_or(""), last["date"].as_str().unwrap_or(""));
                let span = if a == b { a.to_owned() } else { format!("{a} to {b}") };
                let n = line.4.len();
                line.3 = format!("{}: {n} time entr{}, {span}", first["item_code"].as_str().unwrap_or(""), if n == 1 { "y" } else { "ies" });
            }
        }
    }
    if lines.len() > INVOICE_LINES_MAX {
        return Err(FbError::validation(format!("An invoice holds at most {INVOICE_LINES_MAX} lines. Choose one line per service, or invoice fewer entries at a time.")));
    }
    let mut taxable_any = false;
    let mut values = Vec::new();
    for (item_id, units, rate, text, _) in &lines {
        let item = service_item(tx, c, item_id)?;
        let taxable = directory::line_taxable_default(Some(&cust), Some(&item));
        taxable_any |= taxable;
        // The item's income account when it can be posted to; otherwise the draft line waits for one.
        let income = item["income_account"].as_str().and_then(crate::AccountId::parse).map(|a| store::account(tx, a)).transpose()?.flatten()
            .filter(|a| a.company_id == c && a.active && a.postable && a.kind.as_str() == "INCOME").map(|a| a.id.to_db());
        values.push(json!({"item_id":item_id,"description":text,"quantity":quantity_text(*units),"rate":money(*rate),"taxable":taxable,"income_account_id":income}));
    }
    let settings = crate::salestax::settings_for(tx, c)?;
    let (tax_mode, tax_item) = match settings["default_item_id"].as_str() {
        Some(item) if taxable_any && settings["charge_tax"] == true => ("ITEM", json!(item)),
        _ => ("NONE", Value::Null),
    };
    let memo = format!("Job ticket {}: {}", t["number"].as_str().unwrap_or(""), t["title"].as_str().unwrap_or(""));
    let invoice = crate::sales::save_invoice_tx(tx, actor, c, &json!({
        "request_key": rk, "customer_id": t["customer_id"], "date": date, "due_date": due, "memo": py::py_truncate(&memo, 2000),
        "tax_mode": tax_mode, "tax_item_id": tax_item, "lines": values,
    }))?;
    let invoice_id = invoice["id"].as_str().ok_or_else(|| FbError::validation("The invoice was not created."))?.to_owned();
    let now = py::now_utc();
    let (mut hours, mut value, mut charged) = (0i64, 0i64, 0i64);
    for (position, (_, units, rate, _, members)) in lines.iter().enumerate() {
        // The line charges half-up(all its hours x rate). Each entry accounts for the growth of that rounded amount
        // over the hours before it, so the entries' invoiced cents add up to the line exactly.
        let line_cents = crate::calculations::line_amount(&json!(quantity_text(*units)), &json!(money(*rate)))?.cents();
        let mut before = 0i64;
        for i in members {
            let e = &list[*i];
            let (h, r, a) = (e["hours_units"].as_i64().unwrap_or(0), e["rate_cents"].as_i64().unwrap_or(0), e["amount_cents"].as_i64().unwrap_or(0));
            let invoiced = amount(before + h, r) - amount(before, r);
            before += h;
            hours += h;
            value += a;
            charged += invoiced;
            tx.execute("INSERT INTO jobs_billing(id,company_id,entry_id,invoice_id,ticket_id,customer_id,line_position,hours,rate,amount,invoiced,created_by_id,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                params![fb_ids::new_v7().to_string(), c.to_db(), e["id"].as_str(), invoice_id, key, t["customer_id"].as_str(), position as i64 + 1, h, r, a, invoiced, actor.to_db(), now])?;
        }
        if i128::from(amount(before, *rate)) != line_cents || invoice["lines"][position]["amount"] != json!(money(amount(before, *rate))) {
            return Err(FbError::validation("The invoice line amounts do not match the time. Nothing was saved."));
        }
    }
    let mismatched: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM jobs_invoice_mismatch WHERE invoice_id=?1)", [&invoice_id], |r| r.get(0))?;
    if mismatched {
        return Err(FbError::validation("The invoice does not match the time it bills. Nothing was saved."));
    }
    let remaining: i64 = tx.query_row("SELECT count(*) FROM jobs_timeentry e WHERE e.ticket_id=?1 AND e.billable=1 AND NOT EXISTS(SELECT 1 FROM jobs_livebilling l WHERE l.entry_id=e.id)", [&key], |r| r.get(0))?;
    let mark = bool_of(&input["mark_invoiced"], true)? && remaining == 0;
    let mut rev = t["revision"].as_i64().unwrap_or(0);
    if mark && t["status"] != "INVOICED" {
        rev = documents::next_revision(rev)?;
        let finished = if Some(date.as_str()) < t["opened_date"].as_str() { t["opened_date"].as_str().unwrap_or(&date).to_owned() } else { date.clone() };
        tx.execute("UPDATE jobs_ticket SET status='INVOICED',finished_date=?2,revision=?3,updated_at=?4 WHERE id=?1", params![key, finished, rev, now])?;
    } else {
        // Invoicing changes what the ticket shows; take a revision so an open form notices.
        rev = documents::next_revision(rev)?;
        tx.execute("UPDATE jobs_ticket SET revision=?2,updated_at=?3 WHERE id=?1", params![key, rev, now])?;
    }
    let number = tx.query_row("SELECT d.number FROM sales_invoice s JOIN documents_documentidentity d ON d.id=s.identity_id WHERE s.id=?1", [&invoice_id], |r| r.get::<_, String>(0))?;
    audit(tx, &user, c, "job_ticket.invoiced", format!("job_ticket:{key}"), json!({"number":t["number"],"invoice_id":invoice_id,"invoice_number":number,
        "entries":list.len(),"lines":lines.len(),"hours":format_hours(hours),"amount":money(charged),"time_value":money(value),"grouping":grouping.as_str(),"status":if mark {"INVOICED"} else {t["status"].as_str().unwrap_or("")},"revision":rev}))?;
    invoiced(tx, c, &key, &invoice_id)
}
fn invoiced(tx: &Writer<'_>, c: CompanyId, key: &str, invoice_id: &str) -> FbResult<Value> {
    let number = tx.query_row("SELECT d.number FROM sales_invoice s JOIN documents_documentidentity d ON d.id=s.identity_id WHERE s.id=?1 AND s.company_id=?2", params![invoice_id, c.to_db()], |r| r.get::<_, String>(0))?;
    Ok(json!({"invoice_id":invoice_id,"invoice_number":number,"ticket":ticket(tx, c, key)?}))
}

/// Called by the sales service while it writes revision `revision` of a draft invoice it is editing, before that
/// revision exists. Time billed on the draft follows its line (`origin_position`) to the new position; a removed line
/// releases its time. A kept time line must keep its service, quantity, rate and amount, and the invoice its customer:
/// otherwise the edit is refused, because the time would say it is billed for something the invoice no longer charges.
/// The invoice seal (migration 0205) refuses the same mismatch if this were bypassed.
pub(crate) struct InvoiceEdit<'a> {
    pub invoice_id: &'a str,
    /// The invoice as it was before this edit.
    pub old: &'a Value,
    /// The revision being written.
    pub revision: i64,
    pub customer_id: &'a str,
    /// The new lines, with `position` and `origin_position`.
    pub lines: &'a [Value],
}
/// One live billing of the draft: (billing, entry, line position, hours, invoiced cents, ticket).
type LiveBilling = (String, String, i64, i64, i64, Option<String>);
pub(crate) fn follow_invoice_edit(tx: &Writer<'_>, user: &store::User, c: CompanyId, edit: InvoiceEdit<'_>) -> FbResult<()> {
    let InvoiceEdit { invoice_id, old, revision, customer_id, lines } = edit;
    let mut st = tx.prepare("SELECT l.id,l.entry_id,l.line_position,l.hours,l.invoiced,l.ticket_id FROM jobs_livebilling l WHERE l.invoice_id=?1 AND l.company_id=?2 ORDER BY l.line_position,l.id")?;
    let live: Vec<LiveBilling> = st.query_map(params![invoice_id, c.to_db()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    if live.is_empty() {
        return Ok(());
    }
    let old_lines = old["lines"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let ticket_number = |t: &Option<String>| -> FbResult<String> {
        Ok(match t {
            Some(t) => tx.query_row("SELECT number FROM jobs_ticket WHERE id=?1", [t], |r| r.get::<_, String>(0)).optional()?.unwrap_or_default(),
            None => String::new(),
        })
    };
    let mut by_line: BTreeMap<i64, Vec<&LiveBilling>> = BTreeMap::new();
    for b in &live {
        by_line.entry(b.2).or_default().push(b);
    }
    let now = py::now_utc();
    let mut kept = false;
    // Released time per ticket: (entries, hours, cents).
    let mut released: BTreeMap<Option<String>, (Vec<String>, i64, i64)> = BTreeMap::new();
    for (position, members) in &by_line {
        let before = old_lines.iter().find(|l| l["position"].as_i64() == Some(*position));
        let after = lines.iter().find(|l| l["origin_position"].as_i64() == Some(*position));
        let new_position = match after {
            None => None,
            Some(n) => {
                let same = before.is_some_and(|b| ["item_id", "quantity", "rate", "amount", "line_kind"].iter().all(|k| b.get(*k) == n.get(*k)));
                if !same {
                    let number = ticket_number(&members[0].5)?;
                    return Err(FbError::validation(format!(
                        "Line {position} bills recorded time{}. Keep its service, quantity and rate, or remove the line to release that time; then correct the time and invoice it again.",
                        if number.is_empty() { String::new() } else { format!(" from job ticket {number}") })));
                }
                kept = true;
                Some(n["position"].as_i64().unwrap_or(*position))
            }
        };
        if new_position == Some(*position) {
            continue;
        }
        for (billing, entry_id, _, hours, invoiced, ticket) in members.iter().map(|b| (&b.0, &b.1, b.2, b.3, b.4, &b.5)) {
            tx.execute("INSERT INTO jobs_billingstep(id,company_id,billing_id,invoice_id,revision,line_position,created_by_id,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![fb_ids::new_v7().to_string(), c.to_db(), billing, invoice_id, revision, new_position, user.id.to_db(), now])?;
            if new_position.is_none() {
                let r = released.entry(ticket.clone()).or_default();
                r.0.push(entry_id.clone());
                r.1 += hours;
                r.2 += invoiced;
            }
        }
    }
    if kept && old["current"]["customer_id"] != json!(customer_id) {
        return Err(FbError::validation(format!(
            "This invoice bills time recorded for {}. Remove its time lines (that releases the time) or void the invoice before changing the customer.",
            old["current"]["customer_snapshot"]["display_name"].as_str().unwrap_or("its customer"))));
    }
    let number: String = tx.query_row("SELECT d.number FROM sales_invoice s JOIN documents_documentidentity d ON d.id=s.identity_id WHERE s.id=?1", [invoice_id], |r| r.get(0))?;
    for (ticket, (entries, hours, cents)) in released {
        let target = match &ticket { Some(t) => format!("job_ticket:{t}"), None => format!("invoice:{invoice_id}") };
        audit(tx, user, c, "job_ticket.time_released", target, json!({"number":ticket_number(&ticket)?,"invoice_id":invoice_id,"invoice_number":number,
            "invoice_revision":revision,"entries":entries,"hours":format_hours(hours),"amount":money(cents),"reason":"line removed from the draft invoice"}))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hours_accept_decimals_suffixes_and_clock_time_exactly() {
        for (text, units) in [
            ("3.5", 35000), ("3.5h", 35000), ("3.5 h", 35000), ("3.5 hrs", 35000), ("3.5 Hours", 35000), ("3:30", 35000), ("03:30", 35000),
            ("0:15", 2500), (".25", 2500), ("0.1", 1000), ("2", 20000), ("90m", 15000), ("90 min", 15000), ("1h 30m", 15000), ("1h30m", 15000),
            ("1:20", 13333), ("0:01", 167), ("0.0001", 1), ("1000", 10_000_000), ("999:59", 9_999_833), ("1 hour", 10000),
        ] {
            assert_eq!(parse_hours(text).unwrap(), units, "{text}");
        }
        for bad in ["", "0", "0:00", "-1", "3.55555", "3,5", "3:60", "3:5", "1.5h 30m", "1h 90m", "1001", "abc", "3.5x", "1e3", "3..5", "3.5h 2h", "30m 1h", "1000:01", "١٢"] {
            assert!(parse_hours(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn hours_display_and_invoice_quantity_text() {
        assert_eq!(format_hours(35000), "3.50");
        assert_eq!(format_hours(33333), "3.3333");
        assert_eq!(format_hours(30000), "3.00");
        assert_eq!(format_hours(12500), "1.25");
        assert_eq!(format_hours(10010), "1.001");
        assert_eq!(quantity_text(35000), "3.5000");
        assert_eq!(quantity_text(1), "0.0001");
    }
    #[test]
    fn amounts_round_half_up_like_invoice_lines() {
        assert_eq!(amount(35000, 12500), 43750);
        // 1:20 at 100.00: 1.3333 h -> 133.33.
        assert_eq!(amount(13333, 10000), 13333);
        // 0.0001 h at 50.00 is half a cent: rounds half-up to 0.01.
        assert_eq!(amount(1, 5000), 1);
        assert_eq!(amount(1, 4999), 0);
        // half-up: 0.5 h at 0.01 = 0.005 -> 0.01.
        assert_eq!(amount(5000, 1), 1);
        assert_eq!(amount(4999, 1), 0);
        for (h, r) in [(35000i64, 12500i64), (13333, 9999), (MAX_HOURS, MAX_RATE), (1, 1)] {
            let line = crate::calculations::line_amount(&json!(quantity_text(h)), &json!(money(r))).unwrap();
            assert_eq!(line.cents(), amount(h, r) as i128, "{h} x {r} matches the invoice line rule");
        }
    }
    #[test]
    fn rates_are_exact_money_within_the_time_limit() {
        assert_eq!(parse_rate(&json!("125")).unwrap(), 12500);
        assert_eq!(parse_rate(&json!("125.5")).unwrap(), 12550);
        assert_eq!(parse_rate(&json!("0")).unwrap(), 0);
        assert_eq!(parse_rate(&json!("99999999.99")).unwrap(), MAX_RATE);
        for bad in ["-1", "100000000.00", "12.345", "abc"] {
            assert!(parse_rate(&json!(bad)).is_err(), "{bad}");
        }
    }
}

#[cfg(test)]
#[path = "jobs_tests.rs"]
mod lifecycle_tests;
