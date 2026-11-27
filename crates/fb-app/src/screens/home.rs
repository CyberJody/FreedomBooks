//! The company Home page: what needs attention, how the money stands, a row of common tasks and recent
//! activity. Every figure comes from `fb_core::home::summary` (one read snapshot built from the queries the
//! linked screens use); this module only decides what the viewer may see and words it.
use super::d2::put;
use super::*;
use crate::identity::Role;
use chrono::NaiveDate;

/// A backup older than this many days is flagged on Home.
pub const BACKUP_STALE_DAYS: i64 = 7;
/// Bank and card accounts listed on the balances card before "and N more".
const BANK_LINES: usize = 5;

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn int(v: &Value) -> i64 {
    v.as_i64().unwrap_or(0)
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}

/// Count only a complete verified archive containing this company. Check a bounded
/// amount of data on Home; when the budget cannot establish freshness, keep a link
/// to the full Backups page rather than claiming the books are backed up.
fn newest_backup(app: &App, company: CompanyId, today: NaiveDate) -> Option<Result<Option<NaiveDate>, ()>> {
    use fb_core::backups::retention;
    let folder = app.backups.folder().ok()?;
    let (_, key) = app.backups.open_book().ok()?;
    let tag = retention::company_tag(key, &company.to_db()).ok()?;
    let files = match retention::backup_files(&folder, None) {
        Ok(files) => files,
        Err(_) => return Some(Err(())),
    };
    let mut bytes = 0u64;
    for (index, path) in files.iter().enumerate() {
        if index == 32 { return Some(Err(())); }
        let Some(note) = backups::fs_read_small(&retention::sidecar(path)).and_then(|raw| fb_db::parse_unique_json(&raw).ok()) else { continue };
        if note["complete"] != true || !note["company_tags"].as_array().is_some_and(|tags| tags.contains(&json!(tag))) { continue; }
        let Some(day) = note["backup_created_at"].as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).map(|t| t.date_naive()) else { continue };
        if day > today || note["verified_at"].as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).is_none() { continue; }
        bytes = bytes.saturating_add(std::fs::metadata(path).map_or(u64::MAX, |m| m.len()));
        if bytes > 64 * 1024 * 1024 { return Some(Err(())); }
        if retention::verified_note(path) { return Some(Ok(Some(day))); }
    }
    Some(Ok(None))
}

/// "Today, 2:05 p.m.", "Yesterday, 9 a.m.", "3 Nov" or "3 Nov 2025", in the display time zone (the one
/// Audit history uses).
fn when_in(stamp: &str, today: NaiveDate, zone: chrono_tz::Tz) -> String {
    use chrono::{Datelike, Timelike};
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(stamp) else { return String::new() };
    let t = t.with_timezone(&zone);
    let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let time = || {
        let (h, m) = (t.hour(), t.minute());
        format!("{}{} {}", if h % 12 == 0 { 12 } else { h % 12 }, if m == 0 { String::new() } else { format!(":{m:02}") }, if h < 12 { "a.m." } else { "p.m." })
    };
    let day = t.date_naive();
    if day == today {
        format!("Today, {}", time())
    } else if today.pred_opt() == Some(day) {
        format!("Yesterday, {}", time())
    } else if day.year() == today.year() {
        format!("{} {}", day.day(), months[day.month0() as usize])
    } else {
        format!("{} {} {}", day.day(), months[day.month0() as usize], day.year())
    }
}

fn short_date(raw: &str) -> String {
    use chrono::Datelike;
    let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    NaiveDate::parse_from_str(raw, "%Y-%m-%d").map(|d| format!("{} {}", d.day(), months[d.month0() as usize])).unwrap_or_default()
}

/// Display label and detail route of a document kind (as the general ledger's source links).
fn kind(kind: &str) -> Option<(&'static str, &'static str)> {
    Some(match kind {
        "STP" => ("Sales tax payment", "salestax:tax_payment"),
        "INV" => ("Invoice", "invoices:detail"),
        "EST" => ("Estimate", "sales:detail"),
        "RCP" => ("Payment received", "receipts:detail"),
        "SR" => ("Sales receipt", "sales_receipts:detail"),
        "CDP" => ("Customer deposit", "prepayments:detail"),
        "BILL" => ("Bill", "bills:detail"),
        "BPAY" => ("Bill payment", "payments:detail"),
        "CHK" => ("Check", "checks:detail"),
        "DEP" => ("Deposit", "deposits:detail"),
        "TRF" => ("Transfer", "transfers:detail"),
        "CM" => ("Credit memo", "credits:detail"),
        "RFD" => ("Refund", "credits:refund_detail"),
        "CDR" => ("Deposit refund", "prepayments:refund_detail"),
        _ => return None,
    })
}

fn state_label(state: &str) -> &'static str {
    match state {
        "DRAFT" => "Draft",
        "VOID" => "Void",
        "DECLINED" => "Declined",
        "CANCELLED" => "Cancelled",
        "ACCEPTED" => "Accepted",
        _ => "",
    }
}

/// The required setup steps (the checklist's own words), done or not.
const SETUP_STEPS: &[(&str, &str, &str)] = &[
    ("chart", "Chart of accounts", "accounts"),
    ("special", "Accounting setup", "documents:setup"),
    ("tax", "Sales tax", "salestax:setup"),
];

pub(super) fn render(app: &mut App, route: &Route) -> AppResult<String> {
    let c = arg_company(route)?;
    let cpk = c.to_db();
    let zone: chrono_tz::Tz = std::env::var("FB_TIME_ZONE").ok().and_then(|z| z.parse().ok()).unwrap_or(chrono_tz::America::New_York);
    let today = (app.now)().with_timezone(&zone).date_naive();
    let bankfeeds = app.bankfeeds_enabled();
    let role = app.authenticated_session()?.role;
    let shell = shell(app, Some(c), &Current { route: "company_home", kind: None })?;
    let currency = shell.company.as_ref().map(|co| co.currency.clone()).unwrap_or_default();
    // The Backups page needs an administrator who is a staff operator; nobody else sees its status here.
    let admin = role == Role::Administrator && app.open_mut()?.staff;
    let backup = if admin { newest_backup(app, c, today) } else { None };
    let open = app.open_mut()?;
    let data = fb_core::home::summary(&mut open.book, open.owner, c, &today.to_string())?;
    let posted = data["posted"] == true;

    let mut attention: Vec<Value> = Vec::new();
    // `link` names an action ("Back up now"); items that only open a list leave it empty, the item being the link.
    let mut item = |label: String, amount: Option<String>, link: String, href: String, alert: bool| {
        attention.push(json!({"label": label, "amount": amount, "link": link, "href": href, "alert": alert}));
    };
    let r = &data["receivables"];
    if int(&r["overdue_count"]) > 0 {
        item(plural(int(&r["overdue_count"]), "overdue invoice", "overdue invoices"), Some(text(&r["overdue_total"])), String::new(),
            format!("{}?state=OVERDUE", url("invoices:list", &[&cpk])), true);
    }
    let p = &data["payables"];
    if int(&p["overdue_count"]) > 0 {
        item(plural(int(&p["overdue_count"]), "overdue bill", "overdue bills"), Some(text(&p["overdue_total"])), String::new(),
            format!("{}?state=OVERDUE", url("bills:list", &[&cpk])), true);
    }
    if int(&p["soon_count"]) > 0 {
        let days = int(&p["days"]);
        let window = if days == 0 { "due today".to_owned() } else { format!("due in the next {}", plural(days, "day", "days")) };
        item(format!("{} {window}", plural(int(&p["soon_count"]), "bill", "bills")), Some(text(&p["soon_total"])), String::new(),
            format!("{}?{}", url("bills:list", &[&cpk]), urlencode(&[("state".into(), "OUTSTANDING".into()), ("start".into(), today.to_string()), ("end".into(), text(&p["through"]))])), false);
    }
    if bankfeeds && int(&data["to_review"]) > 0 {
        item(format!("{} to review", plural(int(&data["to_review"]), "downloaded bank transaction", "downloaded bank transactions")), None, "Review transactions".into(),
            url("bankfeeds:review", &[&cpk]), false);
    }
    if int(&data["drafts"]["invoices"]) > 0 {
        item(format!("{} not posted", plural(int(&data["drafts"]["invoices"]), "draft invoice", "draft invoices")), None, String::new(),
            format!("{}?state=DRAFT", url("invoices:list", &[&cpk])), false);
    }
    if int(&data["drafts"]["bills"]) > 0 {
        item(format!("{} not posted", plural(int(&data["drafts"]["bills"]), "draft bill", "draft bills")), None, String::new(),
            format!("{}?state=DRAFT", url("bills:list", &[&cpk])), false);
    }
    if bankfeeds && let Some(labels) = data["reconnect"].as_array().filter(|l| !l.is_empty()) {
        let label = if labels.len() == 1 { format!("Bank connection {} needs reconnecting", text(&labels[0])) } else { format!("{} bank connections need reconnecting", labels.len()) };
        item(label, None, "Open bank feeds".into(), url("bankfeeds:list", &[&cpk]), true);
    }
    match backup {
        Some(Ok(None)) => item("No verified complete backup yet".into(), None, "Back up now".into(), url("backups:page", &[&cpk]), true),
        Some(Ok(Some(day))) if (today - day).num_days() > BACKUP_STALE_DAYS => {
            item(format!("Last verified backup {} ago", plural((today - day).num_days(), "day", "days")), None, "Back up now".into(), url("backups:page", &[&cpk]), false)
        }
        Some(Err(())) => item("Check your latest backup".into(), None, "Open Backups".into(), url("backups:page", &[&cpk]), true),
        _ => {}
    }
    let setup: Vec<Value> = SETUP_STEPS
        .iter()
        .map(|(key, label, route)| json!({"label": label, "done": data["setup"][key] == true, "href": url(route, &[&cpk])}))
        .collect();
    let unfinished: Vec<String> = setup.iter().filter(|s| s["done"] != true).map(|s| text(&s["label"])).collect();
    // A company with nothing posted gets the setup panel instead of this line.
    if posted && !unfinished.is_empty() {
        item(format!("Setup not finished: {}", unfinished.join(", ")), None, "Finish setup".into(), url("setup_start", &[&cpk]), false);
    }

    let banks: Vec<Value> = data["banks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|b| json!({"label": format!("{} · {}", text(&b["code"]), text(&b["name"])), "card": b["card"], "active": b["active"], "balance": b["balance"],
                         "href": url("register", &[&cpk, b["id"].as_str().unwrap_or_default()])}))
        .collect();
    let recent: Vec<Value> = data["recent"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            let (label, route) = kind(d["kind"].as_str()?)?;
            Some(json!({"type": label, "number": d["number"], "party": d["party"], "amount": d["amount"], "state": state_label(d["state"].as_str().unwrap_or("")),
                        "when": when_in(d["occurred_at"].as_str().unwrap_or(""), today, zone), "occurred_at": d["occurred_at"], "by": d["by"],
                        "href": url(route, &[&cpk, d["document_id"].as_str().unwrap_or_default()])}))
        })
        .collect();
    let month = &data["month"];
    let pl_query = urlencode(&[("preset".into(), "this_month".into()), ("basis".into(), fb_core::reports::financial::Basis::Accrual.as_str().into())]);
    let mut vm = serde_json::to_value(shell).map_err(|e| AppError::Invalid(e.to_string()))?;
    put(&mut vm, "today", today.to_string());
    put(&mut vm, "currency", currency);
    put(&mut vm, "attention", attention);
    put(&mut vm, "posted", posted);
    put(&mut vm, "setup", setup);
    put(&mut vm, "setup_done", unfinished.is_empty());
    put(&mut vm, "tasks", role != Role::Readonly);
    put(&mut vm, "banks", banks.iter().take(BANK_LINES).collect::<Vec<_>>());
    put(&mut vm, "more_banks", banks.len().saturating_sub(BANK_LINES));
    put(&mut vm, "receivables", json!({"count": r["count"], "total": r["total"], "overdue_count": r["overdue_count"], "overdue_total": r["overdue_total"],
        "href": format!("{}?state=OUTSTANDING", url("invoices:list", &[&cpk])), "overdue_href": format!("{}?state=OVERDUE", url("invoices:list", &[&cpk]))}));
    put(&mut vm, "payables", json!({"count": p["count"], "total": p["total"], "overdue_count": p["overdue_count"], "overdue_total": p["overdue_total"],
        "next_due": p["next_due"].as_str().map(short_date), "next_due_iso": p["next_due"],
        "href": format!("{}?state=OUTSTANDING", url("bills:list", &[&cpk])), "overdue_href": format!("{}?state=OVERDUE", url("bills:list", &[&cpk]))}));
    put(&mut vm, "month", json!({"income": month["income"], "expenses": month["expenses"], "net": month["net"],
        "negative": month["net"].as_str().is_some_and(|n| n.starts_with('-')),
        "label": NaiveDate::parse_from_str(month["start"].as_str().unwrap_or(""), "%Y-%m-%d").map(|d| d.format("%B %Y").to_string()).unwrap_or_default(),
        "href": format!("{}?{pl_query}", url("profit_loss", &[&cpk]))}));
    put(&mut vm, "recent", recent);
    put(&mut vm, "bankfeeds", bankfeeds);
    super::d3::tax_setup_context(app, c, &mut vm)?;
    Ok(app.screens.render("ledger/company_home.html", &vm)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn when_reads_today_yesterday_and_older_dates_in_the_display_zone() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();
        let ny = chrono_tz::America::New_York;
        // 19:05 UTC is 2:05 p.m. in New York (EST); 02:00 UTC on the 11th is still the 10th there.
        assert_eq!(when_in("2026-11-10T19:05:00+00:00", today, ny), "Today, 2:05 p.m.");
        assert_eq!(when_in("2026-11-11T02:00:00+00:00", today, ny), "Today, 9 p.m.");
        assert_eq!(when_in("2026-11-09T14:00:00+00:00", today, ny), "Yesterday, 9 a.m.");
        assert_eq!(when_in("2026-11-03T14:00:00+00:00", today, ny), "3 Nov");
        assert_eq!(when_in("2025-11-03T14:00:00+00:00", today, ny), "3 Nov 2025");
        assert_eq!(when_in("not a time", today, ny), "");
    }
}
