//! Job tickets and time entries through the real screens, form tokens and typed commands, on synthetic books.
mod support;
use chrono::NaiveDate;
use fb_app::{App, Command, Outcome, commands::parse_command, identity::Role, keys, screens};
use fb_core::{CompanyId, services::{self, AccountInput}};
use serde_json::{Value, json};

struct Fx {
    _dir: tempfile::TempDir,
    app: App,
    c: CompanyId,
    tavern: String,
    support: String,
    owner: String,
}
fn command(app: &mut App, value: Value) -> Value {
    match app.run(parse_command(&value.to_string()).unwrap()).unwrap() {
        Outcome::Done { data } => data,
        other => panic!("{other:?}"),
    }
}
fn fixture() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new();
    app.today = || NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
    let c = app.create_book(&dir.path().join("book.fbbook"), &keys::new_master_key().unwrap(), "Owner", "Synthetic IT services").unwrap().company_id;
    support::sign_in(&mut app);
    let (income, ar) = {
        let o = app.open_mut().unwrap();
        let income = services::save_account(&mut o.book, o.owner, c, AccountInput::new("4000", "Service income").detail("INCOME")).unwrap().id;
        let ar = services::save_account(&mut o.book, o.owner, c, AccountInput::new("1200", "Receivables").detail("RECEIVABLE")).unwrap().id;
        (income, ar)
    };
    command(&mut app, json!({"type":"SaveSetup","company_id":c.to_db(),"expected_revision":0,"receivables_id":ar.to_db(),"undeposited_id":null}));
    let tavern = command(&mut app, json!({"type":"SaveCustomer","company_id":c.to_db(),"display_name":"Harbor Lane Tavern"}))["id"].as_str().unwrap().to_owned();
    let support = command(&mut app, json!({"type":"SaveItem","company_id":c.to_db(),"code":"On-Site Support","description":"Technician on site","kind":"SERVICE","sale_rate":"125.00","income_account_id":income.to_db()}))["id"].as_str().unwrap().to_owned();
    let owner = app.open_mut().unwrap().owner.to_db();
    Fx { _dir: dir, app, c, tavern, support, owner }
}
fn render(app: &mut App, path: &str) -> screens::Rendered {
    screens::render(app, path).unwrap()
}
fn hidden(html: &str, name: &str) -> String {
    html.split_once(&format!("name=\"{name}\" value=\"")).unwrap_or_else(|| panic!("{name}")).1.split('"').next().unwrap().to_owned()
}
fn post(app: &mut App, path: &str, fields: &[(&str, &str)]) -> Outcome {
    let html = render(app, path).html;
    let token = hidden(&html, "fb_command_token");
    let mut fields: Vec<(String, String)> = fields.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    if html.contains("name=\"request_key\"") && !fields.iter().any(|(k, _)| k == "request_key") {
        fields.push(("request_key".into(), hidden(&html, "request_key")));
    }
    app.run(Command::SubmitForm { path: path.into(), token, fields }).unwrap()
}
fn redirect(outcome: Outcome) -> String {
    match outcome {
        Outcome::Redirect { location } => location,
        other => panic!("did not save: {other:?}"),
    }
}
fn invalid(outcome: Outcome) -> String {
    match outcome {
        Outcome::Invalid { html } => html,
        other => panic!("expected the form back: {other:?}"),
    }
}
fn id(location: &str) -> String {
    location.trim_end_matches('/').rsplit('/').next().unwrap().to_owned()
}
fn invoice_state(app: &mut App, c: CompanyId, invoice: &str) -> Value {
    app.open_mut().unwrap().book.read(|q| fb_core::sales::invoice(q, c, invoice)).unwrap()
}

#[test]
fn ticket_time_and_invoice_through_the_screens() {
    let mut f = fixture();
    let c = f.c.to_db();
    let list = render(&mut f.app, &format!("/c/{c}/job-tickets/"));
    assert_eq!(list.status, 200);
    assert!(list.html.contains("No open job tickets"));
    assert!(list.html.contains("aria-current=\"page\"><svg") || list.html.contains("Job tickets"));
    assert!(list.html.contains(&format!("/c/{c}/time/new/")), "Enter time is offered");
    // Open a ticket for the tavern.
    let new = format!("/c/{c}/job-tickets/new/");
    let html = invalid(post(&mut f.app, &new, &[("customer", &f.tavern), ("title", ""), ("opened_date", "2026-11-14")]));
    assert!(html.contains("This field is required."));
    let detail = redirect(post(&mut f.app, &new, &[("customer", &f.tavern), ("title", "Network outage"), ("description", "Bar POS offline"), ("opened_date", "2026-11-14")]));
    let ticket = id(&detail);
    let page = render(&mut f.app, &detail).html;
    assert!(page.contains("Ticket JT-000001") && page.contains("Harbor Lane Tavern") && page.contains("Ticket JT-000001 opened."));
    // Add 3.5h On-Site Support from the ticket page.
    let bad = invalid(post(&mut f.app, &detail, &[("action", "add_time"), ("date", "2026-11-14"), ("item", &f.support), ("hours", "three"), ("worker", &f.owner), ("billable", "on")]));
    assert!(bad.contains("Enter hours such as 3.5"));
    let saved = redirect(post(&mut f.app, &detail, &[("action", "add_time"), ("date", "2026-11-14"), ("item", &f.support), ("hours", "3.5h"), ("rate", ""), ("worker", &f.owner), ("notes", "Replaced switch"), ("billable", "on")]));
    assert_eq!(saved, detail);
    let page = render(&mut f.app, &detail).html;
    assert!(page.contains("Added 3.50 h of On-Site Support (437.50)."));
    assert!(page.contains("id=\"ticket-hours\">3.50<") && page.contains("id=\"ticket-unbilled\">437.50<"));
    // Invoice it: a draft invoice for the tavern, and the time shows as billed.
    let invoice = format!("/c/{c}/job-tickets/{ticket}/invoice/");
    let form = render(&mut f.app, &invoice).html;
    let entry = form.split_once("name=\"entries\" value=\"").unwrap().1.split('"').next().unwrap().to_owned();
    let revision = hidden(&form, "expected_revision");
    let location = redirect(post(&mut f.app, &invoice, &[("entries", &entry), ("grouping", "ENTRY"), ("date", "2026-11-15"), ("due_date", ""), ("mark_invoiced", "on"), ("expected_revision", &revision)]));
    assert!(location.starts_with(&format!("/c/{c}/invoices/")), "{location}");
    let inv = id(&location);
    let shown = render(&mut f.app, &location).html;
    assert!(shown.contains("Draft invoice INV-") && shown.contains("created from ticket JT-000001"));
    let v = invoice_state(&mut f.app, f.c, &inv);
    assert_eq!((v["current"]["state"].as_str(), v["current"]["total"].as_str()), (Some("DRAFT"), Some("437.50")));
    let page = render(&mut f.app, &detail).html;
    assert!(page.contains(">Invoiced</span>") && page.contains("(draft)") && page.contains("Remove its invoice line or void the invoice to change"));
    // The same time cannot be invoiced twice: the invoice page has nothing left.
    assert!(render(&mut f.app, &invoice).html.contains("There is no unbilled billable time"));
    // Cancelling the draft releases the time, and the ticket says so.
    command(&mut f.app, json!({"type":"VoidInvoice","company_id":c,"invoice_id":inv,"expected_revision":1,"date":"2026-11-16","reason":"Redo"}));
    let page = render(&mut f.app, &detail).html;
    assert!(page.contains("can be invoiced again") && page.contains("Voided (its time was released)"));
    assert!(page.contains("id=\"ticket-unbilled\">437.50<"));
    // Time list and filters.
    let time = render(&mut f.app, &format!("/c/{c}/time/?show=unbilled&customer={}", f.tavern));
    assert_eq!(time.status, 200);
    assert!(time.html.contains("JT-000001") && time.html.contains("On-Site Support"));
    assert_eq!(render(&mut f.app, &format!("/c/{c}/time/?show=bogus")).status, 400);
    assert_eq!(render(&mut f.app, &format!("/c/{c}/job-tickets/?status=NOPE")).status, 400);
    let tickets = render(&mut f.app, &format!("/c/{c}/job-tickets/?status=INVOICED&customer={}", f.tavern)).html;
    assert!(tickets.contains("data-ticket=\"JT-000001\"") && tickets.contains("Create invoice"));
}

#[test]
fn enter_time_moves_between_tickets_and_respects_open_status() {
    let mut f = fixture();
    let c = f.c.to_db();
    let t1 = command(&mut f.app, json!({"type":"SaveJobTicket","company_id":c,"values":{"request_key":uuid(),"customer_id":f.tavern,"title":"First","opened_date":"2026-11-01"}}));
    let t2 = command(&mut f.app, json!({"type":"SaveJobTicket","company_id":c,"values":{"request_key":uuid(),"customer_id":f.tavern,"title":"Second","opened_date":"2026-11-01"}}));
    let (t1, t2) = (t1["id"].as_str().unwrap().to_owned(), t2["id"].as_str().unwrap().to_owned());
    // "Enter time" from a ticket preselects its customer and ticket; "Save and enter more" keeps them.
    let new = format!("/c/{c}/time/new/");
    let form = render(&mut f.app, &format!("{new}?ticket={t1}")).html;
    assert!(form.contains(&format!("<option value=\"{}\" selected>Harbor Lane Tavern", f.tavern)));
    assert!(form.contains(&format!("<option value=\"{t1}\" selected>JT-000001 · First")));
    let more = redirect(post(&mut f.app, &new, &[("customer", &f.tavern), ("ticket", &t1), ("item", &f.support), ("date", "2026-11-03"), ("hours", "3:30"), ("rate", "110"), ("worker", &f.owner), ("billable", "on"), ("action", "more")]));
    assert!(more.starts_with(&new) && more.contains(&format!("ticket={t1}")) && more.contains("date=2026-11-03"));
    let entries = f.app.open_mut().unwrap().book.read(|q| fb_core::jobs::entries(q, f.c, &fb_core::jobs::EntryFilter::default())).unwrap();
    assert_eq!((entries[0]["hours"].as_str(), entries[0]["amount"].as_str(), entries[0]["rate_override"].as_bool()), (Some("3.50"), Some("385.00"), Some(true)));
    let entry = entries[0]["id"].as_str().unwrap().to_owned();
    // Move it to the second ticket from the first ticket's page.
    let detail = format!("/c/{c}/job-tickets/{t1}/");
    redirect(post(&mut f.app, &detail, &[("action", "move"), ("entry", &entry), ("expected_revision", "1"), ("ticket", &t2)]));
    assert!(render(&mut f.app, &detail).html.contains("Time moved to ticket JT-000002."));
    // A completed ticket takes no time; reopening it does.
    redirect(post(&mut f.app, &detail, &[("action", "complete"), ("date", "2026-11-05"), ("expected_revision", "1")]));
    let html = render(&mut f.app, &detail).html;
    assert!(html.contains("Ticket marked completed.") && !html.contains("id=\"add-time\""));
    let edit = format!("/c/{c}/time/{entry}/");
    let refused = invalid(post(&mut f.app, &edit, &[("customer", &f.tavern), ("ticket", &t1), ("item", &f.support), ("date", "2026-11-03"), ("hours", "3.5"), ("worker", &f.owner), ("billable", "on"), ("expected_revision", "2")]));
    assert!(refused.contains("Select a valid choice"), "a completed ticket is not offered");
    // Remove unbilled time.
    let gone = redirect(post(&mut f.app, &edit, &[("action", "delete"), ("expected_revision", "2")]));
    assert_eq!(gone, format!("/c/{c}/job-tickets/{t2}/"));
    assert!(render(&mut f.app, &gone).html.contains("Time entry removed."));
}

#[test]
fn editing_notes_on_the_time_screen_keeps_the_recorded_rate_after_a_price_change() {
    let mut f = fixture();
    let c = f.c.to_db();
    let made = command(&mut f.app, json!({"type":"SaveTimeEntry","company_id":c,"values":{"request_key":uuid(),"customer_id":f.tavern,"item_id":f.support,"date":"2026-11-03","hours":"3.5"}}));
    let entry = made["id"].as_str().unwrap().to_owned();
    {
        let o = f.app.open_mut().unwrap();
        fb_core::directory::save_record(&mut o.book, o.owner, f.c, fb_core::directory::Model::Item, &json!({"sale_rate":"150.00"}), Some(&f.support)).unwrap();
    }
    let edit = format!("/c/{c}/time/{entry}/");
    let form = render(&mut f.app, &edit).html;
    assert!(form.contains("name=\"rate\"") && form.contains("value=\"125.00\""), "the edit form shows the recorded rate");
    // A notes-only change, submitting the rate the form shows, and then a blank rate: neither reprices.
    for (rate, revision) in [("125.00", "1"), ("", "2")] {
        redirect(post(&mut f.app, &edit, &[("customer", &f.tavern), ("ticket", ""), ("item", &f.support), ("date", "2026-11-03"), ("hours", "3.5"), ("rate", rate),
            ("worker", &f.owner), ("billable", "on"), ("notes", "Notes only"), ("expected_revision", revision)]));
    }
    let e = f.app.open_mut().unwrap().book.read(|q| fb_core::jobs::entry(q, f.c, &entry)).unwrap();
    assert_eq!((e["amount"].as_str(), e["rate_override"].as_bool(), e["notes"].as_str(), e["revision"].as_i64()), (Some("437.50"), Some(false), Some("Notes only"), Some(3)));
}

#[test]
fn read_only_users_view_but_cannot_change_and_other_companies_stay_hidden() {
    let mut f = fixture();
    let c = f.c.to_db();
    let t = command(&mut f.app, json!({"type":"SaveJobTicket","company_id":c,"values":{"request_key":uuid(),"customer_id":f.tavern,"title":"Visible","opened_date":"2026-11-01"}}));
    let ticket = t["id"].as_str().unwrap().to_owned();
    command(&mut f.app, json!({"type":"SaveTimeEntry","company_id":c,"values":{"request_key":uuid(),"customer_id":f.tavern,"ticket_id":ticket,"item_id":f.support,"date":"2026-11-03","hours":"2"}}));
    let other = {
        let o = f.app.open_mut().unwrap();
        services::create_company(&mut o.book, o.owner, "Private company", "USD", false).unwrap().id
    };
    f.app.save_desktop_user(None, "Synthetic viewer", Role::Readonly, true, Some("SyntheticViewerPassword42"), &[f.c]).unwrap();
    f.app.sign_in("Synthetic viewer", "SyntheticViewerPassword42").unwrap();
    let list = render(&mut f.app, &format!("/c/{c}/job-tickets/")).html;
    assert!(list.contains("data-ticket=\"JT-000001\"") && !list.contains("New job ticket</a>"));
    let detail = format!("/c/{c}/job-tickets/{ticket}/");
    let page = render(&mut f.app, &detail).html;
    assert!(page.contains("250.00") && !page.contains("id=\"add-time\"") && !page.contains("Create invoice"));
    assert!(screens::submit(&mut f.app, &detail, &[("action".into(), "complete".into())]).is_err());
    let refused = f.app.run(parse_command(&json!({"type":"SaveTimeEntry","company_id":c,"values":{"request_key":uuid(),"customer_id":f.tavern,"item_id":f.support,"date":"2026-11-03","hours":"1"}}).to_string()).unwrap());
    assert!(refused.is_err(), "read-only users cannot record time");
    assert!(screens::render(&mut f.app, &format!("/c/{other}/job-tickets/")).is_err());
    assert!(screens::render(&mut f.app, &format!("/c/{other}/job-tickets/{ticket}/")).is_err());
}

fn uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[test]
fn audit_history_names_job_actions_and_links_their_records() {
    let mut f = fixture();
    let c = f.c.to_db();
    let t = command(&mut f.app, json!({"type":"SaveJobTicket","company_id":c,"values":{"request_key":uuid(),"customer_id":f.tavern,"title":"Audited","opened_date":"2026-11-01"}}));
    let ticket = t["id"].as_str().unwrap().to_owned();
    let e = command(&mut f.app, json!({"type":"SaveTimeEntry","company_id":c,"values":{"request_key":uuid(),"customer_id":f.tavern,"ticket_id":ticket,"item_id":f.support,"date":"2026-11-03","hours":"1.5"}}));
    let history = render(&mut f.app, &format!("/c/{c}/history/")).html;
    assert!(history.contains("Job ticket opened") && history.contains("Time recorded"), "plain action names");
    assert!(history.contains(&format!("/c/{c}/job-tickets/{ticket}/")) && history.contains("JT-000001 · Audited"));
    assert!(history.contains(&format!("/c/{c}/time/{}/", e["id"].as_str().unwrap())));
}
