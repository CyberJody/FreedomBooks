//! Job tickets and time entries (Customers › Job tickets, Time entries, Enter time). Pages read through
//! `fb_core::jobs`; every write is a typed command. Invoicing a ticket makes an ordinary draft invoice.
use super::d2::{clean_date, clean_integer, clean_text, data_value, decorate, draw, error, nonfield, put, token};
use super::*;
use crate::Command;
use crate::commands::{InvoiceJobTicketValues, JobTicketValues, TimeEntryValues};
use fb_core::jobs;

type Errors = HashMap<String, Vec<String>>;
const REQUIRED: &str = "This field is required.";
const CHOICE: &str = "Select a valid choice. That choice is not one of the available choices.";

pub(super) fn is_route(name: &str) -> bool {
    name.starts_with("jobs:") || name.starts_with("time:")
}
pub(crate) fn is_command(c: &Command) -> bool {
    matches!(c, Command::SaveJobTicket { .. } | Command::SetJobTicketStatus { .. } | Command::SaveTimeEntry { .. }
        | Command::MoveTimeEntry { .. } | Command::DeleteTimeEntry { .. } | Command::InvoiceJobTicket { .. })
}

pub(crate) fn run_command(app: &mut App, cmd: Command) -> AppResult<Outcome> {
    let open = app.open_mut()?;
    let actor = open.owner;
    let book = &mut open.book;
    let c = |s: &str| crate::session::company(s);
    let data = match cmd {
        Command::SaveJobTicket { company_id, values } => jobs::save_ticket(book, actor, c(&company_id)?, &serde_json::to_value(values).expect("typed ticket"))?,
        Command::SetJobTicketStatus { company_id, ticket_id, expected_revision, status, date } => {
            jobs::set_ticket_status(book, actor, c(&company_id)?, &ticket_id, Some(expected_revision), &status, &json!(date))?
        }
        Command::SaveTimeEntry { company_id, values } => jobs::save_entry(book, actor, c(&company_id)?, &serde_json::to_value(values).expect("typed time"))?,
        Command::MoveTimeEntry { company_id, entry_id, expected_revision, ticket_id } => {
            jobs::move_entry(book, actor, c(&company_id)?, &entry_id, Some(expected_revision), ticket_id.as_deref())?
        }
        Command::DeleteTimeEntry { company_id, entry_id, expected_revision } => jobs::delete_entry(book, actor, c(&company_id)?, &entry_id, Some(expected_revision))?,
        Command::InvoiceJobTicket { company_id, values } => jobs::invoice_ticket(book, actor, c(&company_id)?, &serde_json::to_value(values).expect("typed invoice"))?,
        _ => return Err(AppError::Invalid("Not a job ticket command.".into())),
    };
    Ok(Outcome::Done { data })
}

// ------------------------------------------------------------------ shared

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}
fn page_vm(app: &mut App, route: &Route) -> AppResult<Value> {
    let c = arg_company(route)?;
    let mut vm = serde_json::to_value(shell(app, Some(c), &Current { route: &route.name, kind: None })?).map_err(|e| AppError::Invalid(e.to_string()))?;
    put(&mut vm, "can_write", app.authenticated_session()?.role != crate::identity::Role::Readonly);
    put(&mut vm, "messages", std::mem::take(&mut app.messages));
    Ok(vm)
}
fn read<T>(app: &mut App, c: CompanyId, f: impl FnOnce(&fb_db::Reader<'_>) -> Result<T, FbError>) -> AppResult<T> {
    let open = app.open_mut()?;
    let actor = open.owner;
    Ok(open.book.read(|q| {
        fb_core::services::access(q, actor, c)?;
        f(q)
    })?)
}
fn found<T>(result: AppResult<T>, what: &str) -> AppResult<T> {
    result.map_err(|e| match e {
        AppError::Core(FbError::DoesNotExist(_)) => AppError::NotFound(what.into()),
        e => e,
    })
}
fn ticket(app: &mut App, c: CompanyId, id: &str) -> AppResult<Value> {
    found(read(app, c, |q| jobs::ticket(q, c, id)), "job ticket")
}
fn entry(app: &mut App, c: CompanyId, id: &str) -> AppResult<Value> {
    found(read(app, c, |q| jobs::entry(q, c, id)), "time entry")
}
fn core_errors(result: AppResult<Outcome>, errors: &mut Errors) -> AppResult<Option<Value>> {
    match result {
        Ok(Outcome::Done { data }) => Ok(Some(data)),
        Err(AppError::Core(FbError::Validation(messages))) => {
            for m in messages {
                error(errors, "__all__", m);
            }
            Ok(None)
        }
        Err(e) => Err(e),
        Ok(_) => Err(AppError::Invalid("Unexpected result.".into())),
    }
}
fn chosen(p: Posted<'_>, f: &Field, errors: &mut Errors) -> String {
    let v = get(p, &f.name).unwrap_or("").to_owned();
    let ok = match &f.widget {
        Widget::Select { choices } => choices.iter().any(|(id, _)| *id == v),
        Widget::GroupedSelect { groups } => groups.iter().any(|(_, cs)| cs.iter().any(|(id, _)| *id == v)),
        _ => true,
    };
    if v.is_empty() {
        if f.required {
            error(errors, &f.name, REQUIRED);
        }
    } else if !ok {
        error(errors, &f.name, CHOICE);
    }
    v
}
fn request_key(p: Posted<'_>, errors: &mut Errors) -> String {
    match get(p, "request_key").and_then(fb_core::py::py_uuid) {
        Some(k) => k.0,
        None => {
            error(errors, "__all__", "This form is missing its request key. Open it again.");
            String::new()
        }
    }
}
fn revision(p: Posted<'_>) -> i64 {
    clean_integer(get(p, "expected_revision").unwrap_or("")).unwrap_or(-1)
}
fn opt(v: String) -> Option<String> {
    (!v.is_empty()).then_some(v)
}
fn new_key() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn select(placeholder: &str, opts: Vec<(String, String)>) -> Widget {
    let mut choices = vec![(String::new(), placeholder.to_owned())];
    choices.extend(opts);
    Widget::Select { choices }
}
fn customers(app: &mut App, c: CompanyId, keep: Option<&str>) -> AppResult<Vec<(String, String)>> {
    read(app, c, |q| fb_core::directory::list(q, c, fb_core::directory::Model::Customer)).map(|list| {
        list.into_iter()
            .filter(|r| r["active"] == true || keep == r["id"].as_str())
            .map(|r| (s(&r, "id").to_owned(), format!("{}{}", s(&r, "display_name"), if r["active"] == true { "" } else { " (archived)" })))
            .collect()
    })
}

// ------------------------------------------------------------------ tickets

fn list_page(app: &mut App, route: &Route, p: Posted<'_>) -> AppResult<(String, u16)> {
    let c = arg_company(route)?;
    let mut vm = page_vm(app, route)?;
    let customer = get(p, "customer").filter(|v| !v.is_empty()).map(str::to_owned);
    let status = get(p, "status").unwrap_or("ACTIVE").to_owned();
    let all = customers(app, c, customer.as_deref())?;
    let mut errors = vec![];
    if customer.as_ref().is_some_and(|k| !all.iter().any(|(id, _)| id == k)) {
        errors.push("Choose a customer from this company.".to_owned());
    }
    let rows = if errors.is_empty() {
        match read(app, c, |q| jobs::tickets(q, c, &jobs::TicketFilter { customer_id: customer.clone(), status: Some(status.clone()) })) {
            Ok(rows) => rows,
            Err(AppError::Core(FbError::Validation(m))) => {
                errors.extend(m);
                vec![]
            }
            Err(e) => return Err(e),
        }
    } else {
        vec![]
    };
    let unbilled: i64 = rows.iter().map(|r| r["unbilled_cents"].as_i64().unwrap_or(0)).sum();
    put(&mut vm, "customers", &all);
    put(&mut vm, "statuses", jobs::STATUSES.iter().map(|(k, l)| (k.to_string(), l.to_string())).collect::<Vec<_>>());
    put(&mut vm, "controls", json!({"customer": customer, "status": status}));
    put(&mut vm, "errors", &errors);
    put(&mut vm, "unbilled_total", fb_money::Money::from_db(unbilled).to_string());
    let pairs = vec![("customer".to_owned(), customer.clone().unwrap_or_default()), ("status".to_owned(), status.clone())];
    put(&mut vm, "query_string", urlencode(&pairs));
    put(&mut vm, "page", Page::of(&rows, 50, get(p, "page")));
    let status_code = if errors.is_empty() { 200 } else { 400 };
    Ok((draw(app, "jobs/list.html", &vm)?, status_code))
}

fn ticket_fields(app: &mut App, c: CompanyId, editing: Option<&Value>, keep: Option<&str>) -> AppResult<Vec<Field>> {
    let customer = match editing {
        // The customer is fixed once a ticket exists.
        Some(t) => Field::new("customer", "Customer", Widget::Select { choices: vec![(s(t, "customer_id").to_owned(), s(t, "customer_name").to_owned())] }).required(),
        None => Field::new("customer", "Customer", select("Choose the customer…", customers(app, c, keep)?)).required(),
    };
    Ok(vec![
        Field::new("request_key", "Request key", Widget::Hidden).required(),
        Field::new("expected_revision", "Expected revision", Widget::Hidden),
        customer,
        Field::new("title", "Title", Widget::Text).required().attr("maxlength", jobs::TITLE_MAX.to_string()).attr("placeholder", "For example, Network outage at the bar"),
        Field::new("description", "Description", Widget::Textarea { cols: 50, rows: 4 }).attr("maxlength", jobs::DESCRIPTION_MAX.to_string()),
        Field::new("opened_date", "Opened", Widget::Date).required(),
        Field::new("due_date", "Due (optional)", Widget::Date).help("When the work should be finished."),
    ])
}
fn ticket_form(app: &mut App, route: &Route, path: &str, posted: Option<Posted<'_>>, query: Posted<'_>, errors: &Errors) -> AppResult<String> {
    let c = arg_company(route)?;
    let editing = route.args.get("ticket_id").map(|id| ticket(app, c, id)).transpose()?;
    let keep = posted.and_then(|p| get(p, "customer")).or(get(query, "customer")).map(str::to_owned);
    let mut fs = ticket_fields(app, c, editing.as_ref(), keep.as_deref())?;
    let initial = match &editing {
        Some(t) => json!({"request_key":new_key(),"expected_revision":t["revision"].to_string(),"customer":t["customer_id"],"title":t["title"],"description":t["description"],"opened_date":t["opened_date"],"due_date":t["due_date"]}),
        None => json!({"request_key":new_key(),"customer":get(query, "customer").unwrap_or(""),"opened_date":(app.today)().to_string()}),
    };
    for f in &mut fs {
        let v = initial[&f.name].as_str().map(str::to_owned);
        f.value = data_value(posted, &f.name, v);
    }
    let mut vm = page_vm(app, route)?;
    put(&mut vm, "ticket", &editing);
    put(&mut vm, "form", forms::bind(&decorate(fs, errors), &nonfield(errors, "__all__")));
    put(&mut vm, "csrf_token", token(app, path));
    draw(app, "jobs/ticket_form.html", &vm)
}

fn detail_page(app: &mut App, route: &Route, path: &str, posted: Option<Posted<'_>>, errors: &Errors) -> AppResult<String> {
    let c = arg_company(route)?;
    let id = route.args["ticket_id"].clone();
    let mut t = ticket(app, c, &id)?;
    let others: Vec<Value> = read(app, c, |q| jobs::open_tickets(q, c, s(&t, "customer_id")))?.into_iter().filter(|o| o["id"] != t["id"]).collect();
    let mut vm = page_vm(app, route)?;
    let can_write = vm["can_write"] == true;
    // Each row's move form and each action form spends its own single-use token.
    if can_write {
        for e in t["entries"].as_array_mut().unwrap() {
            if e["billed"] != true {
                e["move_token"] = json!(token(app, path));
            }
        }
        put(&mut vm, "status_token", token(app, path));
        if s(&t, "status") == "OPEN" {
            let fs = quick_time_fields(app, c, posted)?;
            put(&mut vm, "time_form", forms::bind(&decorate(fs, errors), &[]));
            put(&mut vm, "time_token", token(app, path));
        }
    }
    put(&mut vm, "errors", nonfield(errors, "__all__"));
    put(&mut vm, "today", (app.today)().to_string());
    put(&mut vm, "other_tickets", others);
    put(&mut vm, "ticket", t);
    draw(app, "jobs/detail.html", &vm)
}

fn quick_time_fields(app: &mut App, c: CompanyId, posted: Option<Posted<'_>>) -> AppResult<Vec<Field>> {
    let actor = app.open_mut()?.owner.to_db();
    let mut fs = vec![
        Field::new("request_key", "Request key", Widget::Hidden).required(),
        Field::new("date", "Date", Widget::Date).required(),
        Field::new("item", "Service", select("Choose the service…", service_choices(app, c, None)?)).required(),
        Field::new("hours", "Hours", Widget::Text).required().attr("inputmode", "decimal").attr("placeholder", "3.5 or 3:30").attr("maxlength", "32").attr("size", "8"),
        Field::new("rate", "Rate", Widget::Text).attr("inputmode", "decimal").attr("placeholder", "Service price").attr("maxlength", "32").attr("size", "10"),
        Field::new("billable", "Billable", Widget::Checkbox { checked: posted.is_none_or(|p| super::d2::checked(p, "billable")) }),
        Field::new("worker", "Worked by", Widget::Select { choices: worker_choices(app, c)? }).required(),
        Field::new("notes", "Notes", Widget::Text).attr("maxlength", jobs::NOTES_MAX.to_string()),
    ];
    let initial = json!({"request_key":new_key(),"date":(app.today)().to_string(),"worker":actor});
    for f in &mut fs {
        if !matches!(f.widget, Widget::Checkbox { .. }) {
            f.value = data_value(posted, &f.name, initial[&f.name].as_str().map(str::to_owned));
        }
    }
    Ok(fs)
}
fn service_choices(app: &mut App, c: CompanyId, keep: Option<&str>) -> AppResult<Vec<(String, String)>> {
    Ok(read(app, c, |q| jobs::services(q, c))?
        .into_iter()
        .filter(|i| i["active"] == true || keep == i["id"].as_str())
        .map(|i| {
            let rate = i["sale_rate"].as_str().unwrap_or("0.00");
            (s(&i, "id").to_owned(), format!("{} — {rate}/h{}", s(&i, "code"), if i["active"] == true { "" } else { " (archived)" }))
        })
        .collect())
}
fn worker_choices(app: &mut App, c: CompanyId) -> AppResult<Vec<(String, String)>> {
    Ok(read(app, c, |q| jobs::workers(q, c))?.into_iter().map(|w| (s(&w, "id").to_owned(), s(&w, "name").to_owned())).collect())
}

fn invoice_page(app: &mut App, route: &Route, path: &str, posted: Option<Posted<'_>>, errors: &Errors) -> AppResult<String> {
    let c = arg_company(route)?;
    let t = ticket(app, c, &route.args["ticket_id"])?;
    let unbilled: Vec<Value> = t["entries"].as_array().unwrap().iter().filter(|e| e["unbilled"] == true).cloned().collect();
    let selected: Vec<String> = match posted {
        Some(p) => p.iter().filter(|(k, _)| k == "entries").map(|(_, v)| v.clone()).collect(),
        None => unbilled.iter().map(|e| s(e, "id").to_owned()).collect(),
    };
    let choices = unbilled.iter().map(|e| (s(e, "id").to_owned(), format!("{} · {} · {} h × {} = {}", s(e, "date"), s(e, "item_code"), s(e, "hours"), s(e, "rate"), s(e, "amount")))).collect();
    let mut fs = vec![
        Field::new("request_key", "Request key", Widget::Hidden).required().value(new_key()),
        Field::new("expected_revision", "Expected revision", Widget::Hidden).required().value(t["revision"].to_string()),
        Field::new("entries", "Time to invoice", Widget::Checkboxes { choices, selected }),
        Field::new("grouping", "Invoice lines", Widget::Radio { choices: vec![("ENTRY".into(), "One line per time entry".into()), ("SERVICE".into(), "One line per service (hours added up)".into())] }).required().value("ENTRY"),
        Field::new("date", "Invoice date", Widget::Date).required().value((app.today)().to_string()),
        Field::new("due_date", "Due date (optional)", Widget::Date).help("Leave blank for 30 days after the invoice date."),
        Field::new("mark_invoiced", "Mark the ticket invoiced when all its billable time is on invoices", Widget::Checkbox { checked: posted.is_none_or(|p| super::d2::checked(p, "mark_invoiced")) }),
    ];
    if let Some(p) = posted {
        for f in &mut fs {
            if !matches!(f.widget, Widget::Checkbox { .. } | Widget::Checkboxes { .. }) {
                f.value = get(p, &f.name).map(str::to_owned);
            }
        }
    }
    let mut vm = page_vm(app, route)?;
    put(&mut vm, "ticket", &t);
    put(&mut vm, "unbilled", unbilled);
    put(&mut vm, "form", forms::bind(&decorate(fs, errors), &nonfield(errors, "__all__")));
    put(&mut vm, "csrf_token", token(app, path));
    draw(app, "jobs/invoice.html", &vm)
}

// ------------------------------------------------------------------ time

fn time_list(app: &mut App, route: &Route, p: Posted<'_>) -> AppResult<(String, u16)> {
    let c = arg_company(route)?;
    let mut vm = page_vm(app, route)?;
    let customer = get(p, "customer").filter(|v| !v.is_empty()).map(str::to_owned);
    let show = get(p, "show").unwrap_or("all").to_owned();
    let all = customers(app, c, customer.as_deref())?;
    let mut errors: Errors = Errors::new();
    if customer.as_ref().is_some_and(|k| !all.iter().any(|(id, _)| id == k)) {
        error(&mut errors, "customer", "Choose a customer from this company.");
    }
    let start = clean_date(p, "start", false, &mut errors);
    let end = clean_date(p, "end", false, &mut errors);
    if !["all", "unbilled", "unassigned"].contains(&show.as_str()) {
        error(&mut errors, "show", CHOICE);
    }
    let rows = if errors.is_empty() {
        read(app, c, |q| jobs::entries(q, c, &jobs::EntryFilter { customer_id: customer.clone(), unbilled: show == "unbilled", unassigned: show == "unassigned", start: start.clone(), end: end.clone(), ..Default::default() }))?
    } else {
        vec![]
    };
    put(&mut vm, "totals", jobs::totals(&rows));
    put(&mut vm, "customers", &all);
    put(&mut vm, "controls", json!({"customer": customer, "show": show, "start": get(p, "start").unwrap_or(""), "end": get(p, "end").unwrap_or("")}));
    put(&mut vm, "errors", errors.values().flatten().cloned().collect::<Vec<_>>());
    let pairs: Vec<(String, String)> = ["customer", "show", "start", "end"].iter().map(|k| (k.to_string(), get(p, k).unwrap_or("").to_owned())).collect();
    put(&mut vm, "query_string", urlencode(&pairs));
    put(&mut vm, "page", Page::of(&rows, 100, get(p, "page")));
    let status = if errors.is_empty() { 200 } else { 400 };
    Ok((draw(app, "jobs/time_list.html", &vm)?, status))
}

/// Select groups: (customer name, [(ticket id, label)]).
type Grouped = Vec<(String, Vec<(String, String)>)>;
fn ticket_choices(app: &mut App, c: CompanyId, keep: Option<&str>) -> AppResult<Grouped> {
    let open = read(app, c, |q| jobs::tickets(q, c, &jobs::TicketFilter { customer_id: None, status: None }))?;
    let mut groups: Grouped = vec![(String::new(), vec![(String::new(), "No ticket".into())])];
    for t in open.into_iter().rev().filter(|t| t["status"] == "OPEN" || keep == t["id"].as_str()) {
        let label = format!("{} · {}{}", s(&t, "number"), s(&t, "title"), if t["status"] == "OPEN" { String::new() } else { format!(" ({})", s(&t, "status_label")) });
        let customer = s(&t, "customer_name").to_owned();
        match groups.iter_mut().find(|(g, _)| *g == customer) {
            Some((_, list)) => list.push((s(&t, "id").to_owned(), label)),
            None => groups.push((customer, vec![(s(&t, "id").to_owned(), label)])),
        }
    }
    Ok(groups)
}
fn time_fields(app: &mut App, c: CompanyId, editing: Option<&Value>, posted: Option<Posted<'_>>) -> AppResult<Vec<Field>> {
    let keep_customer = posted.and_then(|p| get(p, "customer")).map(str::to_owned).or_else(|| editing.map(|e| s(e, "customer_id").to_owned()));
    let keep_item = editing.map(|e| s(e, "item_id").to_owned());
    let keep_ticket = editing.and_then(|e| e["ticket_id"].as_str().map(str::to_owned));
    let billable = match (posted, editing) {
        (Some(p), _) => super::d2::checked(p, "billable"),
        (None, Some(e)) => e["billable"] == true,
        (None, None) => true,
    };
    Ok(vec![
        Field::new("request_key", "Request key", Widget::Hidden).required(),
        Field::new("expected_revision", "Expected revision", Widget::Hidden),
        Field::new("customer", "Customer", select("Choose the customer…", customers(app, c, keep_customer.as_deref())?)).required(),
        Field::new("ticket", "Job ticket", Widget::GroupedSelect { groups: ticket_choices(app, c, keep_ticket.as_deref())? }).help("Only that customer's open tickets can take time."),
        Field::new("item", "Service", select("Choose the service…", service_choices(app, c, keep_item.as_deref())?)).required(),
        Field::new("date", "Date worked", Widget::Date).required(),
        Field::new("hours", "Hours", Widget::Text).required().attr("inputmode", "decimal").attr("placeholder", "3.5, 3.5h or 3:30").attr("maxlength", "32"),
        Field::new("rate", "Rate per hour", Widget::Text).attr("inputmode", "decimal").attr("placeholder", "From the service").attr("maxlength", "32")
            .help(if editing.is_some() { "The recorded rate stays unless you type a new one. Leave blank to keep it, or to use the price of a different service." } else { "Leave blank to use the service's price." }),
        Field::new("billable", "Billable", Widget::Checkbox { checked: billable }),
        Field::new("worker", "Worked by", Widget::Select { choices: worker_choices(app, c)? }).required(),
        Field::new("notes", "Notes", Widget::Textarea { cols: 50, rows: 3 }).attr("maxlength", jobs::NOTES_MAX.to_string()),
    ])
}
fn time_form(app: &mut App, route: &Route, path: &str, posted: Option<Posted<'_>>, query: Posted<'_>, errors: &Errors) -> AppResult<String> {
    let c = arg_company(route)?;
    let editing = route.args.get("entry_id").map(|id| entry(app, c, id)).transpose()?;
    let mut fs = time_fields(app, c, editing.as_ref(), posted)?;
    let actor = app.open_mut()?.owner.to_db();
    let initial = match &editing {
        Some(e) => json!({"request_key":new_key(),"expected_revision":e["revision"].to_string(),"customer":e["customer_id"],"ticket":e["ticket_id"].as_str().unwrap_or(""),"item":e["item_id"],"date":e["date"],
            "hours":e["hours"],"rate":e["rate"],"worker":e["worker_id"],"notes":e["notes"]}),
        None => {
            // From a ticket: its customer; otherwise the customer and date carried from "Save and enter more".
            let ticket_id = get(query, "ticket").filter(|t| !t.is_empty());
            let from_ticket = ticket_id.map(|t| ticket(app, c, t)).transpose().ok().flatten();
            let customer = from_ticket.as_ref().map(|t| s(t, "customer_id").to_owned()).or_else(|| get(query, "customer").map(str::to_owned)).unwrap_or_default();
            json!({"request_key":new_key(),"customer":customer,"ticket":from_ticket.as_ref().map(|t| s(t, "id")).unwrap_or(""),
                "date":get(query, "date").filter(|d| fb_db::dates::parse_iso_date(d).is_some()).map(str::to_owned).unwrap_or_else(|| (app.today)().to_string()),"worker":actor})
        }
    };
    for f in &mut fs {
        if !matches!(f.widget, Widget::Checkbox { .. }) {
            f.value = data_value(posted, &f.name, initial[&f.name].as_str().map(str::to_owned));
        }
    }
    let mut vm = page_vm(app, route)?;
    put(&mut vm, "entry", &editing);
    put(&mut vm, "form", forms::bind(&decorate(fs, errors), &nonfield(errors, "__all__")));
    put(&mut vm, "csrf_token", token(app, path));
    if editing.as_ref().is_some_and(|e| e["has_history"] != true && e["billed"] != true) {
        put(&mut vm, "delete_token", token(app, path));
    }
    draw(app, "jobs/time_form.html", &vm)
}

// ------------------------------------------------------------------ render and submit

pub(super) fn render(app: &mut App, route: &Route, p: Posted<'_>, path: &str) -> AppResult<(String, u16)> {
    let none = Errors::new();
    match route.name.as_str() {
        "jobs:list" => list_page(app, route, p),
        "jobs:new" | "jobs:edit" => Ok((ticket_form(app, route, path, None, p, &none)?, 200)),
        "jobs:detail" => Ok((detail_page(app, route, path, None, &none)?, 200)),
        "jobs:invoice" => Ok((invoice_page(app, route, path, None, &none)?, 200)),
        "time:list" => time_list(app, route, p),
        "time:new" | "time:edit" => Ok((time_form(app, route, path, None, p, &none)?, 200)),
        _ => Err(AppError::NotFound(path.into())),
    }
}

pub(super) fn submit(app: &mut App, route: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(route)?;
    let cid = c.to_db();
    let mut errors = Errors::new();
    match route.name.as_str() {
        "jobs:new" | "jobs:edit" => {
            let editing = route.args.get("ticket_id").map(|id| ticket(app, c, id)).transpose()?;
            let fs = ticket_fields(app, c, editing.as_ref(), get(p, "customer"))?;
            let key = request_key(p, &mut errors);
            let customer = chosen(p, &fs[2], &mut errors);
            let title = clean_text(p, "title", jobs::TITLE_MAX, true, true, &mut errors);
            let description = clean_text(p, "description", jobs::DESCRIPTION_MAX, false, false, &mut errors);
            let opened = clean_date(p, "opened_date", true, &mut errors).unwrap_or_default();
            let due = clean_date(p, "due_date", false, &mut errors);
            if errors.is_empty() {
                let values = JobTicketValues {
                    ticket_id: editing.as_ref().map(|t| s(t, "id").to_owned()),
                    expected_revision: editing.as_ref().map(|_| revision(p)),
                    request_key: key, customer_id: customer, title, description, opened_date: opened, due_date: due,
                };
                if let Some(d) = core_errors(app.run(Command::SaveJobTicket { company_id: cid.clone(), values }), &mut errors)? {
                    app.messages.push(if editing.is_some() { format!("Ticket {} saved.", s(&d, "number")) } else { format!("Ticket {} opened. Add the time worked below.", s(&d, "number")) });
                    return Ok(Outcome::Redirect { location: url("jobs:detail", &[&cid, s(&d, "id")]) });
                }
            }
            Ok(Outcome::Invalid { html: ticket_form(app, route, path, Some(p), &[], &errors)? })
        }
        "jobs:detail" => {
            let id = route.args["ticket_id"].clone();
            let t = ticket(app, c, &id)?;
            let back = url("jobs:detail", &[&cid, &id]);
            match get(p, "action").unwrap_or("") {
                action @ ("complete" | "close" | "reopen") => {
                    let status = match action { "complete" => "COMPLETED", "close" => "CLOSED", _ => "OPEN" };
                    let date = clean_date(p, "date", status != "OPEN", &mut errors);
                    if errors.is_empty()
                        && core_errors(app.run(Command::SetJobTicketStatus { company_id: cid.clone(), ticket_id: id.clone(), expected_revision: revision(p), status: status.into(), date }), &mut errors)?.is_some()
                    {
                        app.messages.push(match status { "COMPLETED" => "Ticket marked completed.", "CLOSED" => "Ticket closed without billing.", _ => "Ticket reopened." }.into());
                        return Ok(Outcome::Redirect { location: back });
                    }
                }
                "add_time" => {
                    let fs = quick_time_fields(app, c, Some(p))?;
                    let key = request_key(p, &mut errors);
                    let date = clean_date(p, "date", true, &mut errors).unwrap_or_default();
                    let item = chosen(p, &fs[2], &mut errors);
                    let hours = clean_text(p, "hours", 32, true, true, &mut errors);
                    let rate = clean_text(p, "rate", 32, false, true, &mut errors);
                    let worker = chosen(p, &fs[6], &mut errors);
                    let notes = clean_text(p, "notes", jobs::NOTES_MAX, false, true, &mut errors);
                    if errors.is_empty() {
                        let values = TimeEntryValues {
                            entry_id: None, expected_revision: None, request_key: key, customer_id: s(&t, "customer_id").to_owned(), ticket_id: Some(id.clone()),
                            item_id: item, date, hours, rate: opt(rate), billable: super::d2::checked(p, "billable"), notes, worker_id: opt(worker),
                        };
                        if let Some(d) = core_errors(app.run(Command::SaveTimeEntry { company_id: cid.clone(), values }), &mut errors)? {
                            app.messages.push(format!("Added {} h of {} ({}).", s(&d, "hours"), s(&d, "item_code"), if d["billable"] == true { s(&d, "amount") } else { "not billable" }));
                            return Ok(Outcome::Redirect { location: back });
                        }
                    }
                }
                "move" => {
                    let entry_id = get(p, "entry").unwrap_or("").to_owned();
                    let to = get(p, "ticket").filter(|v| !v.is_empty()).map(str::to_owned);
                    if core_errors(app.run(Command::MoveTimeEntry { company_id: cid.clone(), entry_id, expected_revision: revision(p), ticket_id: to.clone() }), &mut errors)?.is_some() {
                        let note = match to {
                            Some(t) => format!("Time moved to ticket {}.", s(&ticket(app, c, &t)?, "number")),
                            None => "Time taken off this ticket.".into(),
                        };
                        app.messages.push(note);
                        return Ok(Outcome::Redirect { location: back });
                    }
                }
                _ => error(&mut errors, "__all__", "Choose an action on this ticket."),
            }
            Ok(Outcome::Invalid { html: detail_page(app, route, path, Some(p), &errors)? })
        }
        "jobs:invoice" => {
            let id = route.args["ticket_id"].clone();
            let key = request_key(p, &mut errors);
            let grouping = get(p, "grouping").unwrap_or("").to_owned();
            if !["ENTRY", "SERVICE"].contains(&grouping.as_str()) {
                error(&mut errors, "grouping", REQUIRED);
            }
            let date = clean_date(p, "date", true, &mut errors).unwrap_or_default();
            let due = clean_date(p, "due_date", false, &mut errors);
            let entries: Vec<String> = p.iter().filter(|(k, _)| k == "entries").map(|(_, v)| v.clone()).collect();
            if entries.is_empty() {
                error(&mut errors, "entries", "Choose the time to invoice.");
            }
            if errors.is_empty() {
                let values = InvoiceJobTicketValues { ticket_id: id, expected_revision: revision(p), request_key: key, date, due_date: due, grouping, entry_ids: Some(entries), mark_invoiced: super::d2::checked(p, "mark_invoiced") };
                if let Some(d) = core_errors(app.run(Command::InvoiceJobTicket { company_id: cid.clone(), values }), &mut errors)? {
                    app.messages.push(format!("Draft invoice {} created from ticket {}. Review it, then post it when it is ready.", s(&d, "invoice_number"), s(&d["ticket"], "number")));
                    return Ok(Outcome::Redirect { location: url("invoices:detail", &[&cid, s(&d, "invoice_id")]) });
                }
            }
            Ok(Outcome::Invalid { html: invoice_page(app, route, path, Some(p), &errors)? })
        }
        "time:new" | "time:edit" => {
            let editing = route.args.get("entry_id").map(|id| entry(app, c, id)).transpose()?;
            if get(p, "action") == Some("delete") {
                let e = editing.ok_or_else(|| AppError::NotFound(path.into()))?;
                if core_errors(app.run(Command::DeleteTimeEntry { company_id: cid.clone(), entry_id: s(&e, "id").to_owned(), expected_revision: revision(p) }), &mut errors)?.is_some() {
                    app.messages.push("Time entry removed.".into());
                    return Ok(Outcome::Redirect { location: match e["ticket_id"].as_str() { Some(t) => url("jobs:detail", &[&cid, t]), None => url("time:list", &[&cid]) } });
                }
                return Ok(Outcome::Invalid { html: time_form(app, route, path, Some(p), &[], &errors)? });
            }
            let fs = time_fields(app, c, editing.as_ref(), Some(p))?;
            let key = request_key(p, &mut errors);
            let customer = chosen(p, &fs[2], &mut errors);
            let ticket_id = chosen(p, &fs[3], &mut errors);
            let item = chosen(p, &fs[4], &mut errors);
            let date = clean_date(p, "date", true, &mut errors).unwrap_or_default();
            let hours = clean_text(p, "hours", 32, true, true, &mut errors);
            let rate = clean_text(p, "rate", 32, false, true, &mut errors);
            let worker = chosen(p, &fs[9], &mut errors);
            let notes = clean_text(p, "notes", jobs::NOTES_MAX, false, false, &mut errors);
            if errors.is_empty() {
                let values = TimeEntryValues {
                    entry_id: editing.as_ref().map(|e| s(e, "id").to_owned()), expected_revision: editing.as_ref().map(|_| revision(p)), request_key: key,
                    customer_id: customer.clone(), ticket_id: opt(ticket_id), item_id: item, date: date.clone(), hours, rate: opt(rate),
                    billable: super::d2::checked(p, "billable"), notes, worker_id: opt(worker),
                };
                if let Some(d) = core_errors(app.run(Command::SaveTimeEntry { company_id: cid.clone(), values }), &mut errors)? {
                    let what = format!("{} h of {} for {}", s(&d, "hours"), s(&d, "item_code"), s(&d, "customer_name"));
                    app.messages.push(if editing.is_some() { format!("Saved {what}.") } else { format!("Recorded {what}.") });
                    let location = if get(p, "action") == Some("more") {
                        let mut q = vec![("customer".to_owned(), customer), ("date".to_owned(), date)];
                        if let Some(t) = d["ticket_id"].as_str() { q.push(("ticket".into(), t.into())); }
                        format!("{}?{}", url("time:new", &[&cid]), urlencode(&q))
                    } else {
                        match d["ticket_id"].as_str() { Some(t) => url("jobs:detail", &[&cid, t]), None => url("time:list", &[&cid]) }
                    };
                    return Ok(Outcome::Redirect { location });
                }
            }
            Ok(Outcome::Invalid { html: time_form(app, route, path, Some(p), &[], &errors)? })
        }
        _ => d8::method_not_allowed(app, route),
    }
}
