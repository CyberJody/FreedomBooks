//! Job tickets and time: lifecycle, rates, invoicing, no double billing, void releasing time, company and
//! member scope, audit, and that nothing here posts to the ledger by itself.
use super::*;
use crate::services::AccountInput;
use crate::{sales, users};

struct Fx {
    path: std::path::PathBuf,
    book: Book,
    actor: UserId,
    tech: UserId,
    co: CompanyId,
    other: CompanyId,
    income: crate::AccountId,
    tavern: String,
    diner: String,
    support: String,
    remote: String,
    router: String,
}
fn new_id() -> String {
    fb_ids::new_v7().to_string()
}
fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or_else(|| panic!("{k} in {v}"))
}
/// An edit row that keeps invoice line `l` (its origin is its current position).
fn line(l: &Value) -> Value {
    json!({"origin_position":l["position"],"item_id":l["item_id"],"income_account_id":l["income_account_id"],"description":l["description"],
        "quantity":l["quantity"],"rate":l["rate"],"taxable":l["taxable"]})
}
fn message(e: FbError) -> String {
    match e {
        FbError::Validation(m) => m.join(" "),
        other => format!("{other:?}"),
    }
}
impl Fx {
    fn new() -> Fx {
        let path = std::env::temp_dir().join(format!("fb-jobs-{}.fbbook", new_id()));
        let mut book = Book::create(&path, &[47; 32]).unwrap();
        let actor = users::create_user(&mut book, "Owner", false).unwrap();
        let tech = users::create_user(&mut book, "Sam Technician", false).unwrap();
        let co = services::create_company(&mut book, actor, "Synthetic IT services", "USD", false).unwrap().id;
        let other = services::create_company(&mut book, actor, "Synthetic other company", "USD", false).unwrap().id;
        users::add_member(&mut book, co, tech).unwrap();
        let mut acct = |c: CompanyId, code: &str, kind: &str| services::save_account(&mut book, actor, c, AccountInput::new(code, &format!("Account {code}")).kind(kind)).unwrap().id;
        let (income, ar) = (acct(co, "400", "INCOME"), acct(co, "110", "ASSET"));
        documents::save_setup(&mut book, actor, co, &json!({"receivables":ar.to_db(),"banks":[]}), Some(0)).unwrap();
        let mut record = |c: CompanyId, m: Model, v: Value| directory::save_record(&mut book, actor, c, m, &v, None).unwrap()["id"].as_str().unwrap().to_owned();
        let tavern = record(co, Model::Customer, json!({"display_name":"Harbor Lane Tavern"}));
        let diner = record(co, Model::Customer, json!({"display_name":"Harbor Diner"}));
        let support = record(co, Model::Item, json!({"code":"On-Site Support","description":"Technician on site","kind":"SERVICE","sale_rate":"125.00","income_account":income.to_db()}));
        let remote = record(co, Model::Item, json!({"code":"Remote Support","description":"","kind":"SERVICE","sale_rate":"95.00","income_account":income.to_db()}));
        let router = record(co, Model::Item, json!({"code":"Router","description":"","kind":"NONSTOCK","sale_rate":"199.00","income_account":income.to_db()}));
        Fx { path, book, actor, tech, co, other, income, tavern, diner, support, remote, router }
    }
    fn ticket(&mut self, customer: &str, title: &str) -> Value {
        save_ticket(&mut self.book, self.actor, self.co, &json!({"request_key":new_id(),"customer_id":customer,"title":title,"description":"","opened_date":"2026-11-02"})).unwrap()
    }
    fn time(&mut self, customer: &str, ticket: Option<&str>, item: &str, hours: &str, rate: Option<&str>, billable: bool) -> FbResult<Value> {
        save_entry(&mut self.book, self.actor, self.co, &json!({"request_key":new_id(),"customer_id":customer,"ticket_id":ticket,"item_id":item,
            "date":"2026-11-03","hours":hours,"rate":rate,"billable":billable,"notes":"Replaced switch"}))
    }
    fn read<T>(&mut self, f: impl FnOnce(&fb_db::Reader<'_>) -> FbResult<T>) -> T {
        self.book.read(f).unwrap()
    }
    fn invoice(&mut self, ticket: &Value, grouping: &str) -> FbResult<Value> {
        invoice_ticket(&mut self.book, self.actor, self.co, &json!({"ticket_id":ticket["id"],"expected_revision":ticket["revision"],"request_key":new_id(),
            "date":"2026-11-15","grouping":grouping,"mark_invoiced":true}))
    }
    fn invoice_fresh(&mut self, id: &str, grouping: &str) -> FbResult<Value> {
        let t = self.fresh_ticket(id);
        self.invoice(&t, grouping)
    }
    fn fresh_ticket(&mut self, id: &str) -> Value {
        let co = self.co;
        self.read(|q| super::ticket(q, co, id))
    }
    /// The open-time checks pass, and the book closes and opens again through the normal path.
    fn reopen(&mut self, after: &str) {
        assert_eq!(self.book.check().unwrap(), Vec::<String>::new(), "open-time checks after {after}");
        let placeholder = Book::create(&std::env::temp_dir().join(format!("fb-jobs-swap-{}.fbbook", new_id())), &[47; 32]).unwrap();
        drop(std::mem::replace(&mut self.book, placeholder));
        self.book = Book::open(&self.path, &[47; 32]).unwrap_or_else(|e| panic!("the book reopens after {after}: {e:?}"));
    }
    fn entry(&mut self, id: &str) -> Value {
        let co = self.co;
        self.read(|q| super::entry(q, co, id))
    }
    fn invoice_of(&mut self, id: &str) -> Value {
        let co = self.co;
        self.read(|q| sales::invoice(q, co, id))
    }
    /// Save the draft with `rows` (built from its current lines by `line`), for `customer`.
    fn edit_invoice(&mut self, id: &str, customer: &str, rows: Vec<Value>) -> FbResult<Value> {
        let inv = self.invoice_of(id);
        sales::save_invoice(&mut self.book, self.actor, self.co, &json!({"invoice_id":id,"expected_revision":inv["revision"],"customer_id":customer,
            "date":inv["current"]["date"],"due_date":inv["current"]["due_date"],"memo":inv["current"]["memo"],"tax_mode":"NONE","lines":rows}))
    }
    fn live_billings(&mut self) -> i64 {
        self.read(|q| Ok(q.query_row("SELECT count(*) FROM jobs_livebilling", [], |r| r.get::<_, i64>(0))?))
    }
    fn journals(&mut self) -> i64 {
        self.read(|q| Ok(q.query_row("SELECT count(*) FROM ledger_journal", [], |r| r.get::<_, i64>(0))?))
    }
    fn audit_actions(&mut self) -> Vec<String> {
        let co = self.co;
        self.read(|q| {
            let mut st = q.prepare("SELECT action FROM ledger_auditevent WHERE company_id=?1 AND (action LIKE 'job_ticket.%' OR action LIKE 'time_entry.%') ORDER BY occurred_at,id")?;
            Ok(st.query_map([co.to_db()], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?)
        })
    }
}

#[test]
fn the_tavern_flow_records_three_and_a_half_hours_and_invoices_them_as_a_draft() {
    let mut fx = Fx::new();
    let journals = fx.journals();
    let t = fx.ticket(&fx.tavern.clone(), "Network outage");
    assert_eq!(s(&t, "number"), "JT-000001");
    assert_eq!(s(&t, "status"), "OPEN");
    let (tavern, support) = (fx.tavern.clone(), fx.support.clone());
    let e = fx.time(&tavern, Some(s(&t, "id")), &support, "3.5h", None, true).unwrap();
    assert_eq!((s(&e, "hours"), s(&e, "rate"), s(&e, "amount")), ("3.50", "125.00", "437.50"));
    assert_eq!(e["rate_override"], false);
    assert_eq!(s(&e, "worker_name"), "Owner", "the person signed in by default");
    assert_eq!(fx.journals(), journals, "tickets and time are not postings");
    let t = fx.fresh_ticket(s(&t, "id"));
    assert_eq!(t["totals"]["unbilled"], "437.50");
    assert_eq!(t["unbilled_count"], 1);
    let done = fx.invoice(&t, "ENTRY").unwrap();
    assert_eq!(fx.journals(), journals, "the invoice is a draft: nothing posts until it is reviewed and posted");
    let co = fx.co;
    let inv = fx.read(|q| sales::invoice(q, co, s(&done, "invoice_id")));
    assert_eq!(inv["current"]["state"], "DRAFT");
    assert_eq!(inv["current"]["customer_id"], json!(tavern));
    assert_eq!(inv["current"]["total"], "437.50");
    let line = &inv["lines"][0];
    assert_eq!((line["quantity"].as_str().unwrap(), line["rate"].as_str().unwrap(), line["amount"].as_str().unwrap()), ("3.5000", "125.00", "437.50"));
    assert_eq!(line["item_id"], json!(support));
    assert_eq!(line["income_account_id"], json!(fx.income.to_db()));
    assert!(s(line, "description").starts_with("On-Site Support: Owner on 2026-11-03 — Replaced switch"));
    assert!(s(&inv["current"], "memo").starts_with("Job ticket JT-000001: Network outage"));
    let t = &done["ticket"];
    assert_eq!(s(t, "status"), "INVOICED");
    assert_eq!(t["finished_date"], "2026-11-15");
    assert_eq!(t["entries"][0]["billed"], true);
    assert_eq!(t["entries"][0]["invoice_number"], done["invoice_number"]);
    assert_eq!(t["invoices"][0]["number"], done["invoice_number"]);
    // The invoice posts through the ordinary sales service.
    let key = s(&done, "invoice_id").to_owned();
    let review = sales::review_invoice(&mut fx.book, fx.actor, fx.co, &key, Some(1)).unwrap();
    sales::post_invoice(&mut fx.book, fx.actor, fx.co, &key, Some(1), Some(&review)).unwrap();
    assert_eq!(fx.journals(), journals + 1);
    assert_eq!(fx.audit_actions(), ["job_ticket.created", "time_entry.created", "job_ticket.invoiced"]);
}

#[test]
fn rates_default_from_the_service_and_can_be_overridden_per_entry() {
    let mut fx = Fx::new();
    let (tavern, support, remote, router) = (fx.tavern.clone(), fx.support.clone(), fx.remote.clone(), fx.router.clone());
    let a = fx.time(&tavern, None, &support, "1:20", None, true).unwrap();
    assert_eq!((s(&a, "hours"), s(&a, "amount")), ("1.3333", "166.66"), "1.3333 h x 125.00 = 166.6625, half-up 166.66");
    let b = fx.time(&tavern, None, &remote, "2", Some("80"), true).unwrap();
    assert_eq!((s(&b, "rate"), s(&b, "amount")), ("80.00", "160.00"));
    assert_eq!(b["rate_override"], true);
    let same = fx.time(&tavern, None, &remote, "2", Some("95.00"), true).unwrap();
    assert_eq!(same["rate_override"], false, "typing the service price is not an override");
    assert!(message(fx.time(&tavern, None, &router, "1", None, true).unwrap_err()).contains("service item"));
    for bad in ["0", "3,5", "1001", "abc", "3:75"] {
        assert!(message(fx.time(&tavern, None, &support, bad, None, true).unwrap_err()).contains("Enter hours"), "{bad}");
    }
    assert!(fx.time(&tavern, None, &support, "1", Some("-5"), true).is_err());
    // Editing re-defaults an unchanged rate from a new service, and keeps an override.
    let edited = save_entry(&mut fx.book, fx.actor, fx.co, &json!({"entry_id":a["id"],"expected_revision":1,"customer_id":tavern,"item_id":remote,
        "date":"2026-11-04","hours":"1.5","rate":null,"billable":true,"notes":""})).unwrap();
    assert_eq!((s(&edited, "rate"), s(&edited, "amount"), edited["revision"].as_i64()), ("95.00", "142.50", Some(2)));
    // A stale revision is refused.
    let stale = save_entry(&mut fx.book, fx.actor, fx.co, &json!({"entry_id":a["id"],"expected_revision":1,"customer_id":tavern,"item_id":remote,
        "date":"2026-11-04","hours":"9","billable":true,"notes":""}));
    assert!(message(stale.unwrap_err()).contains("changed since you opened it"));
}

#[test]
fn time_joins_and_moves_only_between_open_tickets_of_its_customer() {
    let mut fx = Fx::new();
    let (tavern, diner, support) = (fx.tavern.clone(), fx.diner.clone(), fx.support.clone());
    let t1 = fx.ticket(&tavern, "Outage");
    let t2 = fx.ticket(&tavern, "Printer setup");
    let t3 = fx.ticket(&diner, "Diner POS");
    assert_eq!(s(&t3, "number"), "JT-000003");
    assert!(message(fx.time(&tavern, Some(s(&t3, "id")), &support, "1", None, true).unwrap_err()).contains("ticket of this customer"));
    let e = fx.time(&tavern, None, &support, "2", None, true).unwrap();
    let e = move_entry(&mut fx.book, fx.actor, fx.co, s(&e, "id"), Some(1), Some(s(&t1, "id"))).unwrap();
    assert_eq!(s(&e, "ticket_number"), "JT-000001");
    let e = move_entry(&mut fx.book, fx.actor, fx.co, s(&e, "id"), Some(2), Some(s(&t2, "id"))).unwrap();
    assert_eq!(s(&e, "ticket_number"), "JT-000002");
    assert!(message(move_entry(&mut fx.book, fx.actor, fx.co, s(&e, "id"), Some(3), Some(s(&t3, "id"))).unwrap_err()).contains("own customer"));
    let t1 = set_ticket_status(&mut fx.book, fx.actor, fx.co, s(&t1, "id"), Some(1), "COMPLETED", &json!("2026-11-05")).unwrap();
    assert_eq!(s(&t1, "status"), "COMPLETED");
    assert!(message(move_entry(&mut fx.book, fx.actor, fx.co, s(&e, "id"), Some(3), Some(s(&t1, "id"))).unwrap_err()).contains("not open"));
    let e = move_entry(&mut fx.book, fx.actor, fx.co, s(&e, "id"), Some(3), None).unwrap();
    assert!(e["ticket_id"].is_null());
    // Reopen, then close without billing; a closed ticket cannot be invoiced.
    let t1 = set_ticket_status(&mut fx.book, fx.actor, fx.co, s(&t1, "id"), Some(2), "OPEN", &Value::Null).unwrap();
    assert!(t1["finished_date"].is_null());
    let e = move_entry(&mut fx.book, fx.actor, fx.co, s(&e, "id"), Some(4), Some(s(&t1, "id"))).unwrap();
    let t1 = set_ticket_status(&mut fx.book, fx.actor, fx.co, s(&t1, "id"), Some(3), "CLOSED", &json!("2026-11-06")).unwrap();
    assert!(message(fx.invoice(&t1, "ENTRY").unwrap_err()).contains("closed without billing"));
    assert!(set_ticket_status(&mut fx.book, fx.actor, fx.co, s(&t1, "id"), Some(4), "INVOICED", &json!("2026-11-06")).is_err());
    // A ticket stays with its customer.
    let bad = save_ticket(&mut fx.book, fx.actor, fx.co, &json!({"ticket_id":t2["id"],"expected_revision":1,"customer_id":diner,"title":"x","opened_date":"2026-11-02"}));
    assert!(message(bad.unwrap_err()).contains("stays with its customer"));
    // Unbilled time can be removed; the audit trail records every step.
    delete_entry(&mut fx.book, fx.actor, fx.co, s(&e, "id"), Some(5)).unwrap();
    let actions = fx.audit_actions();
    assert_eq!(actions.iter().filter(|a| *a == "time_entry.moved").count(), 4);
    assert!(actions.contains(&"job_ticket.completed".into()) && actions.contains(&"job_ticket.reopened".into()) && actions.contains(&"job_ticket.closed".into()));
    assert_eq!(actions.last().unwrap(), "time_entry.deleted");
}

#[test]
fn the_same_time_is_never_billed_twice_and_voiding_the_invoice_releases_it() {
    let mut fx = Fx::new();
    let (tavern, support, remote) = (fx.tavern.clone(), fx.support.clone(), fx.remote.clone());
    let t = fx.ticket(&tavern, "Office move");
    let id = s(&t, "id").to_owned();
    fx.time(&tavern, Some(&id), &support, "3.5", None, true).unwrap();
    fx.time(&tavern, Some(&id), &support, "1.25", None, true).unwrap();
    fx.time(&tavern, Some(&id), &remote, "0.5", None, true).unwrap();
    let skipped = fx.time(&tavern, Some(&id), &remote, "4", None, false).unwrap();
    let t = fx.fresh_ticket(&id);
    assert_eq!(t["totals"]["hours"], "9.25");
    assert_eq!(t["totals"]["nonbillable_hours"], "4.00");
    assert_eq!(t["totals"]["unbilled"], "641.25");
    let done = fx.invoice(&t, "SERVICE").unwrap();
    let co = fx.co;
    let inv = fx.read(|q| sales::invoice(q, co, s(&done, "invoice_id")));
    let lines = inv["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2, "one line per service and rate");
    assert_eq!((s(&lines[0], "quantity"), s(&lines[0], "amount")), ("4.7500", "593.75"));
    assert!(s(&lines[0], "description").starts_with("On-Site Support: 2 time entries, 2026-11-03"));
    assert_eq!((s(&lines[1], "quantity"), s(&lines[1], "amount")), ("0.5000", "47.50"));
    assert_eq!(inv["current"]["total"], "641.25");
    let t = &done["ticket"];
    assert_eq!(t["unbilled_count"], 0);
    assert_eq!(s(t, "status"), "INVOICED");
    // Invoicing again finds nothing; billed time is frozen; the database refuses a second live billing.
    let again = fx.invoice(t, "ENTRY");
    assert!(message(again.unwrap_err()).contains("no unbilled billable time"));
    let billed = &t["entries"][0];
    let edit = save_entry(&mut fx.book, fx.actor, fx.co, &json!({"entry_id":billed["id"],"expected_revision":1,"customer_id":tavern,"item_id":support,"date":"2026-11-03","hours":"8","billable":true,"notes":""}));
    assert!(message(edit.unwrap_err()).contains("Void that invoice"));
    assert!(move_entry(&mut fx.book, fx.actor, fx.co, s(billed, "id"), Some(1), None).is_err());
    assert!(delete_entry(&mut fx.book, fx.actor, fx.co, s(billed, "id"), Some(1)).is_err());
    let raw = fx.book.write(|tx| -> FbResult<usize> {
        let other = sales::save_invoice_tx(tx, fx.actor, fx.co, &json!({"request_key":new_id(),"customer_id":tavern,"date":"2026-11-15","tax_mode":"NONE",
            "lines":[{"item_id":null,"description":"x","quantity":"1","rate":"1.00","taxable":false,"income_account_id":null}]}))?;
        Ok(tx.execute("INSERT INTO jobs_billing(id,company_id,entry_id,invoice_id,ticket_id,customer_id,line_position,hours,rate,amount,invoiced,created_by_id,created_at) VALUES(?1,?2,?3,?4,?5,?11,1,?6,?7,?8,?8,?9,?10)",
            params![new_id(), fx.co.to_db(), billed["id"].as_str(), other["id"].as_str(), id, billed["hours_units"].as_i64(), billed["rate_cents"].as_i64(), billed["amount_cents"].as_i64(), fx.actor.to_db(), py::now_utc(), tavern])?)
    });
    assert!(format!("{:?}", raw.unwrap_err()).contains("jobs_billing_twice"));
    // Non-billable time is never billed, even by a direct insert.
    let raw = fx.book.write(|tx| -> FbResult<usize> {
        Ok(tx.execute("INSERT INTO jobs_billing(id,company_id,entry_id,invoice_id,ticket_id,customer_id,line_position,hours,rate,amount,invoiced,created_by_id,created_at) VALUES(?1,?2,?3,?4,?5,?11,1,?6,?7,?8,?8,?9,?10)",
            params![new_id(), fx.co.to_db(), skipped["id"].as_str(), done["invoice_id"].as_str(), id, skipped["hours_units"].as_i64(), skipped["rate_cents"].as_i64(), skipped["amount_cents"].as_i64(), fx.actor.to_db(), py::now_utc(), tavern])?)
    });
    assert!(format!("{:?}", raw.unwrap_err()).contains("jobs_billing_entry"));
    // Cancelling the draft invoice releases the time; it can be invoiced again on a new invoice.
    let key = s(&done, "invoice_id").to_owned();
    sales::void_invoice(&mut fx.book, fx.actor, fx.co, &key, Some(1), &json!("2026-11-16"), &json!("Wrong grouping")).unwrap();
    let t = fx.fresh_ticket(&id);
    assert_eq!(t["unbilled_count"], 3);
    assert_eq!(t["totals"]["unbilled"], "641.25");
    assert_eq!(t["invoices"][0]["state"], "VOID");
    assert!(t["entries"].as_array().unwrap().iter().all(|e| e["billed"] == false));
    assert_eq!(s(&t, "status"), "INVOICED", "the status stays until the time is invoiced again or the ticket reopened");
    let second = fx.invoice(&t, "ENTRY").unwrap();
    assert_eq!(second["ticket"]["invoices"].as_array().unwrap().len(), 2);
    let co = fx.co;
    let inv2 = fx.read(|q| sales::invoice(q, co, s(&second, "invoice_id")));
    assert_eq!(inv2["lines"].as_array().unwrap().len(), 3);
    assert_eq!(inv2["current"]["total"], "641.25");
    // Released time with invoice history is kept: it cannot be deleted, but posting and voiding the new invoice frees it again.
    let key2 = s(&second, "invoice_id").to_owned();
    let review = sales::review_invoice(&mut fx.book, fx.actor, fx.co, &key2, Some(1)).unwrap();
    sales::post_invoice(&mut fx.book, fx.actor, fx.co, &key2, Some(1), Some(&review)).unwrap();
    assert_eq!(fx.fresh_ticket(&id)["unbilled_count"], 0);
    sales::void_invoice(&mut fx.book, fx.actor, fx.co, &key2, Some(2), &json!("2026-11-17"), &json!("Customer dispute")).unwrap();
    let t = fx.fresh_ticket(&id);
    assert_eq!(t["unbilled_count"], 3, "voiding a posted invoice releases its time too");
    let first = &t["entries"][0];
    assert!(message(delete_entry(&mut fx.book, fx.actor, fx.co, s(first, "id"), first["revision"].as_i64()).unwrap_err()).contains("kept with that history"));
    assert!(fx.book.check().unwrap().is_empty(), "open-time evidence holds");
}

#[test]
fn a_retried_invoice_request_returns_the_same_invoice_and_partial_selection_keeps_the_ticket_open() {
    let mut fx = Fx::new();
    let (tavern, support) = (fx.tavern.clone(), fx.support.clone());
    let t = fx.ticket(&tavern, "Two visits");
    let id = s(&t, "id").to_owned();
    let a = fx.time(&tavern, Some(&id), &support, "1", None, true).unwrap();
    fx.time(&tavern, Some(&id), &support, "2", None, true).unwrap();
    let t = fx.fresh_ticket(&id);
    let input = json!({"ticket_id":id,"expected_revision":t["revision"],"request_key":new_id(),"date":"2026-11-15","grouping":"ENTRY","entry_ids":[a["id"]],"mark_invoiced":true});
    let first = invoice_ticket(&mut fx.book, fx.actor, fx.co, &input).unwrap();
    assert_eq!(s(&first["ticket"], "status"), "OPEN", "time is left to bill");
    assert_eq!(first["ticket"]["unbilled_count"], 1);
    let retry = invoice_ticket(&mut fx.book, fx.actor, fx.co, &input).unwrap();
    assert_eq!(retry["invoice_id"], first["invoice_id"]);
    let co = fx.co;
    let invoices = fx.read(|q| Ok(q.query_row("SELECT count(*) FROM sales_invoice WHERE company_id=?1", [co.to_db()], |r| r.get::<_, i64>(0))?));
    assert_eq!(invoices, 1);
    // A stale ticket revision is refused for a new request.
    let mut stale = input.clone();
    stale["request_key"] = json!(new_id());
    stale["entry_ids"] = Value::Null;
    assert!(message(invoice_ticket(&mut fx.book, fx.actor, fx.co, &stale).unwrap_err()).contains("changed since you opened it"));
    // Time already billed cannot be chosen again.
    stale["expected_revision"] = first["ticket"]["revision"].clone();
    stale["entry_ids"] = json!([a["id"]]);
    assert!(message(invoice_ticket(&mut fx.book, fx.actor, fx.co, &stale).unwrap_err()).contains("already invoiced"));
}

#[test]
fn companies_members_and_workers_are_kept_apart() {
    let mut fx = Fx::new();
    let (tavern, support, tech) = (fx.tavern.clone(), fx.support.clone(), fx.tech);
    let t = fx.ticket(&tavern, "Scoped");
    // The other company cannot see or use this company's tickets, customers or items.
    let other = fx.other;
    assert!(fx.book.read(|q| super::ticket(q, other, s(&t, "id"))).is_err());
    let foreign = save_entry(&mut fx.book, fx.actor, other, &json!({"request_key":new_id(),"customer_id":tavern,"item_id":support,"date":"2026-11-03","hours":"1","billable":true,"notes":""}));
    assert!(message(foreign.unwrap_err()).contains("customer from this company"));
    let foreign = save_ticket(&mut fx.book, fx.actor, other, &json!({"request_key":new_id(),"customer_id":tavern,"title":"x","opened_date":"2026-11-02"}));
    assert!(foreign.is_err());
    assert!(fx.book.read(|q| super::tickets(q, other, &TicketFilter::default())).unwrap().is_empty());
    // A user outside the company is refused outright.
    let outsider = users::create_user(&mut fx.book, "Outsider", false).unwrap();
    let denied = save_ticket(&mut fx.book, outsider, fx.co, &json!({"request_key":new_id(),"customer_id":tavern,"title":"x","opened_date":"2026-11-02"}));
    assert!(matches!(denied.unwrap_err(), FbError::PermissionDenied(_)));
    let denied = fx.book.read(|q| -> FbResult<()> { services::access(q, outsider, fx.co).map(|_| ()) });
    assert!(denied.is_err());
    // Work can be recorded for another member; not for someone outside the company.
    let e = save_entry(&mut fx.book, fx.actor, fx.co, &json!({"request_key":new_id(),"customer_id":tavern,"ticket_id":t["id"],"item_id":support,"date":"2026-11-03","hours":"2","billable":true,"notes":"","worker_id":tech.to_db()})).unwrap();
    assert_eq!(s(&e, "worker_name"), "Sam Technician");
    let bad = save_entry(&mut fx.book, fx.actor, fx.co, &json!({"request_key":new_id(),"customer_id":tavern,"item_id":support,"date":"2026-11-03","hours":"2","billable":true,"notes":"","worker_id":outsider.to_db()}));
    assert!(message(bad.unwrap_err()).contains("who did the work"));
    let workers = fx.book.read(|q| super::workers(q, fx.co)).unwrap();
    assert_eq!(workers.iter().map(|w| s(w, "name").to_owned()).collect::<Vec<_>>(), ["Owner", "Sam Technician"]);
    // Filters by customer and status.
    let co = fx.co;
    let diner = fx.diner.clone();
    fx.ticket(&diner, "Diner");
    let by_customer = fx.read(|q| super::tickets(q, co, &TicketFilter { customer_id: Some(tavern.clone()), status: None }));
    assert_eq!(by_customer.len(), 1);
    let open = fx.read(|q| super::tickets(q, co, &TicketFilter { customer_id: None, status: Some("OPEN".into()) }));
    assert_eq!(open.len(), 2);
    let invoiced = fx.read(|q| super::tickets(q, co, &TicketFilter { customer_id: None, status: Some("INVOICED".into()) }));
    assert!(invoiced.is_empty());
    assert!(fx.book.read(|q| super::tickets(q, co, &TicketFilter { customer_id: None, status: Some("NOPE".into()) })).is_err());
}

#[test]
fn a_retried_entry_request_returns_the_same_entry_and_audit_names_the_operator() {
    let mut fx = Fx::new();
    let (tavern, support) = (fx.tavern.clone(), fx.support.clone());
    let input = json!({"request_key":new_id(),"customer_id":tavern,"item_id":support,"date":"2026-11-03","hours":"3:30","billable":true,"notes":"Rack wiring"});
    let a = save_entry(&mut fx.book, fx.actor, fx.co, &input).unwrap();
    let b = save_entry(&mut fx.book, fx.actor, fx.co, &input).unwrap();
    assert_eq!(a["id"], b["id"]);
    let co = fx.co;
    let (count, actor, detail): (i64, String, String) = fx.read(|q| Ok(q.query_row(
        "SELECT count(*),max(actor_name),max(detail) FROM ledger_auditevent WHERE company_id=?1 AND action='time_entry.created'", [co.to_db()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?));
    assert_eq!((count, actor.as_str()), (1, "Owner"));
    let detail: Value = serde_json::from_str(&detail).unwrap();
    assert_eq!((s(&detail, "hours"), s(&detail, "amount")), ("3.50", "437.50"));
}

#[test]
fn job_ticket_entry_and_billing_identities_are_permanent() {
    let mut fx = Fx::new();
    let (tavern, support) = (fx.tavern.clone(), fx.support.clone());
    let t = fx.ticket(&tavern, "Keys");
    let e = fx.time(&tavern, Some(s(&t, "id")), &support, "1", None, true).unwrap();
    let t = fx.fresh_ticket(s(&t, "id"));
    let done = fx.invoice(&t, "ENTRY").unwrap();
    let billing: String = fx.read(|q| Ok(q.query_row("SELECT id FROM jobs_billing", [], |r| r.get(0))?));
    let fresh = new_id();
    for recursion in [true, false] {
        fx.book.connection().execute_batch(&format!("PRAGMA recursive_triggers={}", i32::from(recursion))).unwrap();
        let mut cases = vec![];
        for (table, key) in [("jobs_ticket", s(&t, "id")), ("jobs_timeentry", s(&e, "id")), ("jobs_billing", billing.as_str())] {
            for column in ["id", "company_id"] {
                for op in ["UPDATE", "UPDATE OR REPLACE"] {
                    cases.push(format!("{op} {table} SET {column}='{fresh}' WHERE id='{key}'"));
                }
            }
            cases.push(format!("DELETE FROM {table} WHERE id='{key}'"));
        }
        cases.push(format!("UPDATE jobs_ticket SET request_key='{fresh}', revision=revision+1 WHERE id='{}'", s(&t, "id")));
        cases.push(format!("UPDATE jobs_timeentry SET request_key='{fresh}', revision=revision+1 WHERE id='{}'", s(&e, "id")));
        // REPLACE through a colliding insert must not silently swap a billed entry or a ticket.
        cases.push(format!("INSERT OR REPLACE INTO jobs_timeentry SELECT id,company_id,'{fresh}',customer_id,ticket_id,item_id,date,hours*2,rate,(hours*2*rate+5000)/10000,rate_override,billable,notes,worker_id,1,created_by_id,created_at,updated_at FROM jobs_timeentry WHERE id='{}'", s(&e, "id")));
        cases.push(format!("INSERT OR REPLACE INTO jobs_ticket SELECT id,company_id,ordinal+1,'JT-'||printf('%06d',ordinal+1),'{fresh}',customer_id,title,description,status,opened_date,due_date,finished_date,1,created_by_id,created_at,updated_at FROM jobs_ticket WHERE id='{}'", s(&t, "id")));
        cases.push(format!("INSERT OR REPLACE INTO jobs_billing SELECT '{fresh}',company_id,entry_id,invoice_id,ticket_id,customer_id,line_position,hours,rate,amount,invoiced,created_by_id,created_at FROM jobs_billing WHERE id='{billing}'"));
        for sql in cases {
            fx.book.connection().execute_batch("SAVEPOINT identity_case").unwrap();
            let result = fx.book.connection().execute_batch(&sql);
            fx.book.connection().execute_batch("ROLLBACK TO identity_case; RELEASE identity_case").unwrap();
            assert!(result.is_err(), "accepted (recursive_triggers={recursion}): {sql}");
        }
    }
    let co = fx.co;
    assert_eq!(fx.read(|q| super::ticket(q, co, s(&t, "id")))["invoices"][0]["id"], done["invoice_id"]);
    assert!(fx.book.check().unwrap().is_empty());
}

// ------------------------------------------------------------------ independent-review regressions (review-sol.md)

/// B1: removing a time line from the draft releases exactly that time; reordering, retitling and copying keep the
/// rest billed on the line that charges it; the books check and reopen after every edit.
#[test]
fn removing_a_time_line_releases_that_time_and_the_other_lines_follow_their_charges() {
    let mut fx = Fx::new();
    let (tavern, support, remote) = (fx.tavern.clone(), fx.support.clone(), fx.remote.clone());
    let t = fx.ticket(&tavern, "Line edits");
    let id = s(&t, "id").to_owned();
    let first = fx.time(&tavern, Some(&id), &support, "3.5", None, true).unwrap();
    let second = fx.time(&tavern, Some(&id), &support, "2", None, true).unwrap();
    let third = fx.time(&tavern, Some(&id), &remote, "1", None, true).unwrap();
    let t = fx.fresh_ticket(&id);
    let made = fx.invoice(&t, "ENTRY").unwrap();
    let key = s(&made, "invoice_id").to_owned();
    fx.reopen("invoicing the ticket");
    // Reorder (3, 1, 2), retitle line 1 and change its tax flag: still billed, now on the line that charges it.
    let inv = fx.invoice_of(&key);
    let ls = inv["lines"].as_array().unwrap().clone();
    let mut retitled = line(&ls[0]);
    retitled["description"] = json!("On-site visit, rewritten for the customer");
    retitled["taxable"] = json!(true);
    fx.edit_invoice(&key, &tavern, vec![line(&ls[2]), retitled, line(&ls[1])]).unwrap();
    fx.reopen("reordering and retitling time lines");
    let positions: Vec<(String, i64)> = fx.read(|q| {
        let mut st = q.prepare("SELECT entry_id,line_position FROM jobs_livebilling ORDER BY line_position")?;
        Ok(st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?)
    });
    assert_eq!(positions, vec![(s(&third, "id").to_owned(), 1), (s(&first, "id").to_owned(), 2), (s(&second, "id").to_owned(), 3)]);
    // Copying a time line adds an ordinary charge; it bills no time and the original stays linked once.
    let inv = fx.invoice_of(&key);
    let ls = inv["lines"].as_array().unwrap().clone();
    let mut copy = line(&ls[1]);
    copy["origin_position"] = Value::Null;
    fx.edit_invoice(&key, &tavern, vec![line(&ls[0]), line(&ls[1]), line(&ls[2]), copy]).unwrap();
    assert_eq!(fx.live_billings(), 3);
    fx.reopen("copying a time line");
    // Remove the copy and the line for the 3.5 h: that entry alone is released.
    let inv = fx.invoice_of(&key);
    let ls = inv["lines"].as_array().unwrap().clone();
    let edited = fx.edit_invoice(&key, &tavern, vec![line(&ls[0]), line(&ls[2])]).unwrap();
    assert_eq!(edited["current"]["total"], "345.00", "2 h x 125.00 + 1 h x 95.00");
    fx.reopen("removing a time line");
    let released = fx.entry(s(&first, "id"));
    assert_eq!((released["billed"].clone(), released["unbilled"].clone(), released["has_history"].clone()), (json!(false), json!(true), json!(true)));
    assert_eq!(fx.entry(s(&second, "id"))["billed"], true);
    assert_eq!(fx.entry(s(&third, "id"))["billed"], true);
    let t = fx.fresh_ticket(&id);
    assert_eq!(t["unbilled_count"], 1);
    assert_eq!((t["totals"]["unbilled"].clone(), t["totals"]["billed"].clone()), (json!("437.50"), json!("345.00")), "billed totals are what the invoice charges");
    assert_eq!((t["invoices"][0]["entries"].clone(), t["invoices"][0]["amount"].clone(), t["invoices"][0]["total"].clone()), (json!(2), json!("345.00"), json!("345.00")));
    assert!(fx.audit_actions().contains(&"job_ticket.time_released".to_owned()));
    let detail: Value = fx.read(|q| Ok(serde_json::from_str(&q.query_row("SELECT detail FROM ledger_auditevent WHERE action='job_ticket.time_released'", [], |r| r.get::<_, String>(0))?).unwrap()));
    assert_eq!((detail["entries"].clone(), detail["amount"].clone()), (json!([s(&first, "id")]), json!("437.50")));
    // The released time can be corrected and invoiced again; the edited draft posts what it shows.
    let corrected = save_entry(&mut fx.book, fx.actor, fx.co, &json!({"entry_id":first["id"],"expected_revision":1,"customer_id":tavern,"ticket_id":id,"item_id":support,
        "date":"2026-11-03","hours":"3","billable":true,"notes":"Corrected hours"})).unwrap();
    assert_eq!(s(&corrected, "amount"), "375.00");
    let again = fx.invoice_fresh(&id, "ENTRY").unwrap();
    assert_eq!(fx.invoice_of(s(&again, "invoice_id"))["current"]["total"], "375.00");
    fx.reopen("invoicing released time again");
    let rev = fx.invoice_of(&key)["revision"].as_i64();
    let review = sales::review_invoice(&mut fx.book, fx.actor, fx.co, &key, rev).unwrap();
    let posted = sales::post_invoice(&mut fx.book, fx.actor, fx.co, &key, rev, Some(&review)).unwrap();
    assert_eq!(posted["current"]["total"], "345.00");
    fx.reopen("posting the edited draft");
    sales::void_invoice(&mut fx.book, fx.actor, fx.co, &key, posted["revision"].as_i64(), &json!("2026-11-16"), &json!("Customer dispute")).unwrap();
    assert_eq!(fx.fresh_ticket(&id)["unbilled_count"], 2, "voiding releases what was still on it");
    fx.reopen("voiding the edited invoice");
}

/// B1: while time is billed on a draft, its line keeps its service, quantity and rate, and the invoice its customer;
/// once the time lines are removed (releasing the time) the draft may go to another customer.
#[test]
fn time_lines_keep_their_charge_and_the_invoice_its_customer_until_the_time_is_released() {
    let mut fx = Fx::new();
    let (tavern, diner, support, remote) = (fx.tavern.clone(), fx.diner.clone(), fx.support.clone(), fx.remote.clone());
    let t = fx.ticket(&tavern, "Customer edit");
    let id = s(&t, "id").to_owned();
    let a = fx.time(&tavern, Some(&id), &support, "3.5", None, true).unwrap();
    let b = fx.time(&tavern, Some(&id), &support, "1.25", None, true).unwrap();
    let made = fx.invoice_fresh(&id, "SERVICE").unwrap();
    let key = s(&made, "invoice_id").to_owned();
    let inv = fx.invoice_of(&key);
    let ls = inv["lines"].as_array().unwrap().clone();
    assert_eq!(ls.len(), 1, "one grouped line");
    let mut extra = line(&ls[0]);
    extra["origin_position"] = Value::Null;
    extra["item_id"] = Value::Null;
    extra["description"] = json!("Trip charge");
    extra["quantity"] = json!("1");
    extra["rate"] = json!("25.00");
    let refusals = [("quantity", json!("1")), ("rate", json!("150.00")), ("item_id", json!(remote))];
    for (field, value) in refusals {
        let mut changed = line(&ls[0]);
        changed[field] = value;
        let err = message(fx.edit_invoice(&key, &tavern, vec![changed]).unwrap_err());
        assert!(err.contains("bills recorded time from job ticket JT-000001"), "{field}: {err}");
    }
    let err = message(fx.edit_invoice(&key, &diner, vec![line(&ls[0]), extra.clone()]).unwrap_err());
    assert!(err.contains("Harbor Lane Tavern"), "{err}");
    assert_eq!(fx.invoice_of(&key)["revision"], 1, "refused edits change nothing");
    fx.reopen("refused invoice edits");
    // Adding an ordinary charge beside the time is fine.
    fx.edit_invoice(&key, &tavern, vec![line(&ls[0]), extra.clone()]).unwrap();
    fx.reopen("adding a charge beside the time");
    // Removing the grouped line releases both entries; then the draft can change customer and post for the Diner.
    let edited = fx.edit_invoice(&key, &diner, vec![extra]).unwrap();
    assert_eq!(edited["current"]["customer_id"], json!(diner));
    assert_eq!(fx.live_billings(), 0);
    fx.reopen("removing the grouped line and changing customer");
    for e in [&a, &b] {
        assert_eq!(fx.entry(s(e, "id"))["unbilled"], true);
    }
    let rev = edited["revision"].as_i64();
    let review = sales::review_invoice(&mut fx.book, fx.actor, fx.co, &key, rev).unwrap();
    let posted = sales::post_invoice(&mut fx.book, fx.actor, fx.co, &key, rev, Some(&review)).unwrap();
    assert_eq!((posted["current"]["customer_id"].clone(), posted["current"]["total"].clone()), (json!(diner), json!("25.00")));
    let t = fx.fresh_ticket(&id);
    assert_eq!((t["unbilled_count"].clone(), t["totals"]["billed"].clone(), t["totals"]["unbilled"].clone()), (json!(2), json!("0.00"), json!("593.75")));
    fx.reopen("posting the Diner invoice");
}

/// B1 backstop: a release written around the service cannot leave an invoice charging different time than the time
/// says is billed: the invoice seal refuses the next revision, and the books stay openable.
#[test]
fn the_invoice_seal_refuses_a_revision_that_no_longer_matches_its_billed_time() {
    let mut fx = Fx::new();
    let (tavern, support) = (fx.tavern.clone(), fx.support.clone());
    let t = fx.ticket(&tavern, "Seal");
    let id = s(&t, "id").to_owned();
    fx.time(&tavern, Some(&id), &support, "1", None, true).unwrap();
    fx.time(&tavern, Some(&id), &support, "2", None, true).unwrap();
    let made = fx.invoice_fresh(&id, "SERVICE").unwrap();
    let key = s(&made, "invoice_id").to_owned();
    let ls = fx.invoice_of(&key)["lines"].as_array().unwrap().clone();
    let (actor, co, tavern2) = (fx.actor, fx.co, tavern.clone());
    let attack = fx.book.write(|tx| tx.atomic(|| -> FbResult<Value> {
        // Release one of the two entries on the grouped line directly, then save the draft unchanged.
        let billing: String = tx.query_row("SELECT id FROM jobs_billing ORDER BY hours LIMIT 1", [], |r| r.get(0))?;
        tx.execute("INSERT INTO jobs_billingstep(id,company_id,billing_id,invoice_id,revision,line_position,created_by_id,created_at) VALUES(?1,?2,?3,?4,2,NULL,?5,?6)",
            params![new_id(), co.to_db(), billing, key, actor.to_db(), py::now_utc()])?;
        sales::save_invoice_tx(tx, actor, co, &json!({"invoice_id":key,"expected_revision":1,"customer_id":tavern2,"date":"2026-11-15","tax_mode":"NONE","lines":[line(&ls[0])]}))
    }));
    assert!(format!("{:?}", attack.unwrap_err()).contains("jobs_invoice_time"));
    // A step for a revision that is not the next one, or for released or void time, is refused outright.
    let refused = fx.book.write(|tx| -> FbResult<usize> {
        let billing: String = tx.query_row("SELECT id FROM jobs_billing LIMIT 1", [], |r| r.get(0))?;
        Ok(tx.execute("INSERT INTO jobs_billingstep(id,company_id,billing_id,invoice_id,revision,line_position,created_by_id,created_at) VALUES(?1,?2,?3,?4,3,1,?5,?6)",
            params![new_id(), co.to_db(), billing, key, actor.to_db(), py::now_utc()])?)
    });
    assert!(format!("{:?}", refused.unwrap_err()).contains("jobs_billing_step"));
    fx.reopen("refused attacks");
    // Steps are history: they cannot be changed or removed.
    let ls = fx.invoice_of(&key)["lines"].as_array().unwrap().clone();
    let mut note = line(&ls[0]);
    note["origin_position"] = Value::Null;
    note["item_id"] = Value::Null;
    note["description"] = json!("Replacement charge");
    fx.edit_invoice(&key, &tavern, vec![note]).unwrap();
    let step: String = fx.read(|q| Ok(q.query_row("SELECT id FROM jobs_billingstep LIMIT 1", [], |r| r.get(0))?));
    for sql in [format!("UPDATE jobs_billingstep SET line_position=1 WHERE id='{step}'"), format!("DELETE FROM jobs_billingstep WHERE id='{step}'"),
        format!("INSERT OR REPLACE INTO jobs_billingstep SELECT id,company_id,billing_id,invoice_id,revision,1,created_by_id,created_at FROM jobs_billingstep WHERE id='{step}'")] {
        fx.book.connection().execute_batch("SAVEPOINT step_case").unwrap();
        let result = fx.book.connection().execute_batch(&sql);
        fx.book.connection().execute_batch("ROLLBACK TO step_case; RELEASE step_case").unwrap();
        assert!(result.is_err(), "accepted: {sql}");
    }
    fx.reopen("removing the grouped line");
}

/// B2: correcting the customer (and ticket) of time released by a void keeps the books valid and openable, across
/// repeated void and re-invoice cycles.
#[test]
fn correcting_the_customer_of_released_time_keeps_the_books_openable() {
    let mut fx = Fx::new();
    let (tavern, diner, support) = (fx.tavern.clone(), fx.diner.clone(), fx.support.clone());
    let t = fx.ticket(&tavern, "Released edit");
    let entry = fx.time(&tavern, Some(s(&t, "id")), &support, "3.5", None, true).unwrap();
    let made = fx.invoice_fresh(s(&t, "id"), "ENTRY").unwrap();
    sales::void_invoice(&mut fx.book, fx.actor, fx.co, s(&made, "invoice_id"), Some(1), &json!("2026-11-15"), &json!("Wrong customer")).unwrap();
    let mut revision = 1;
    for (cycle, customer) in [(1, &diner), (2, &tavern), (3, &diner)] {
        let target = fx.ticket(customer, &format!("Cycle {cycle}"));
        let changed = save_entry(&mut fx.book, fx.actor, fx.co, &json!({"entry_id":entry["id"],"expected_revision":revision,"customer_id":customer,"ticket_id":null,
            "item_id":support,"date":"2026-11-03","hours":"3.5","rate":"125.00","billable":true,"notes":"Correct customer"})).unwrap();
        assert_eq!(changed["customer_id"], json!(customer));
        fx.reopen(&format!("cycle {cycle}: changing the customer of released time"));
        let moved = move_entry(&mut fx.book, fx.actor, fx.co, s(&entry, "id"), changed["revision"].as_i64(), Some(s(&target, "id"))).unwrap();
        let made = fx.invoice_fresh(s(&target, "id"), "ENTRY").unwrap();
        let inv = fx.invoice_of(s(&made, "invoice_id"));
        assert_eq!((inv["current"]["customer_id"].clone(), inv["current"]["total"].clone()), (json!(customer), json!("437.50")));
        fx.reopen(&format!("cycle {cycle}: invoicing it for the new customer"));
        sales::void_invoice(&mut fx.book, fx.actor, fx.co, s(&made, "invoice_id"), Some(1), &json!("2026-11-15"), &json!("Again")).unwrap();
        revision = moved["revision"].as_i64().unwrap();
    }
    fx.reopen("three void and re-invoice cycles");
    let history: i64 = fx.read(|q| Ok(q.query_row("SELECT count(*) FROM jobs_billing", [], |r| r.get(0))?));
    assert_eq!(history, 4, "every billing is kept with the customer it was made for");
}

/// B3: a service with recorded time (unbilled, billed, or released) keeps its kind; an unused service may change.
#[test]
fn a_service_with_recorded_time_stays_a_service() {
    let mut fx = Fx::new();
    let (tavern, support, remote) = (fx.tavern.clone(), fx.support.clone(), fx.remote.clone());
    let unused = directory::save_record(&mut fx.book, fx.actor, fx.co, Model::Item, &json!({"code":"Consulting","description":"","kind":"SERVICE","sale_rate":"80.00","income_account":fx.income.to_db()}), None).unwrap();
    let t = fx.ticket(&tavern, "Kinds");
    fx.time(&tavern, None, &support, "1", None, true).unwrap();
    fx.time(&tavern, Some(s(&t, "id")), &remote, "1", None, true).unwrap();
    let made = fx.invoice_fresh(s(&t, "id"), "ENTRY").unwrap();
    let mut items = vec![(support.clone(), "unbilled"), (remote.clone(), "billed")];
    for (item, state) in items.drain(..) {
        let err = message(directory::save_record(&mut fx.book, fx.actor, fx.co, Model::Item, &json!({"kind":"NONSTOCK"}), Some(&item)).unwrap_err());
        assert!(err.contains("stays a service"), "{state}: {err}");
        let co = fx.co;
        assert_eq!(fx.read(|q| directory::get(q, co, Model::Item, &item))["kind"], "SERVICE");
    }
    sales::void_invoice(&mut fx.book, fx.actor, fx.co, s(&made, "invoice_id"), Some(1), &json!("2026-11-15"), &json!("Released")).unwrap();
    assert!(message(directory::save_record(&mut fx.book, fx.actor, fx.co, Model::Item, &json!({"kind":"NONSTOCK"}), Some(&remote)).unwrap_err()).contains("stays a service"));
    // Other edits of a used service are fine, and so is changing an unused one.
    directory::save_record(&mut fx.book, fx.actor, fx.co, Model::Item, &json!({"sale_rate":"130.00","description":"Renamed"}), Some(&support)).unwrap();
    let changed = directory::save_record(&mut fx.book, fx.actor, fx.co, Model::Item, &json!({"kind":"NONSTOCK"}), Some(s(&unused, "id"))).unwrap();
    assert_eq!(changed["kind"], "NONSTOCK");
    // The database refuses it too.
    let raw = fx.book.write(|tx| -> FbResult<usize> { Ok(tx.execute("UPDATE directory_item SET kind='NONSTOCK' WHERE id=?1", [&support])?) });
    assert!(format!("{:?}", raw.unwrap_err()).contains("jobs_item_kind"));
    fx.reopen("refused kind changes");
}

/// S1: editing notes (or anything but the rate and service) keeps the recorded rate, even after the catalog price
/// changes; a typed rate or a different service reprices.
#[test]
fn editing_time_keeps_its_recorded_rate_unless_the_rate_or_service_changes() {
    let mut fx = Fx::new();
    let (tavern, support, remote) = (fx.tavern.clone(), fx.support.clone(), fx.remote.clone());
    let defaulted = fx.time(&tavern, None, &support, "3.5", None, true).unwrap();
    let typed = fx.time(&tavern, None, &support, "3.5", Some("125.00"), true).unwrap();
    let custom = fx.time(&tavern, None, &support, "3.5", Some("110.00"), true).unwrap();
    assert_eq!((typed["rate_override"].clone(), custom["rate_override"].clone()), (json!(false), json!(true)));
    directory::save_record(&mut fx.book, fx.actor, fx.co, Model::Item, &json!({"sale_rate":"150.00"}), Some(&support)).unwrap();
    let edit = |fx: &mut Fx, e: &Value, item: &str, rate: Value, notes: &str| save_entry(&mut fx.book, fx.actor, fx.co, &json!({"entry_id":e["id"],"expected_revision":e["revision"],
        "customer_id":tavern,"item_id":item,"date":"2026-11-03","hours":"3.5","rate":rate,"billable":true,"notes":notes})).unwrap();
    for (e, rate, cents, flag) in [(&defaulted, json!(""), "437.50", false), (&typed, Value::Null, "437.50", false), (&custom, json!(""), "385.00", true), (&defaulted, json!("125.00"), "437.50", false)] {
        let e = fx.entry(s(e, "id"));
        let kept = edit(&mut fx, &e, &support, rate.clone(), "Notes only edit");
        assert_eq!((s(&kept, "rate"), s(&kept, "amount"), kept["rate_override"].clone()), (e["rate"].as_str().unwrap(), cents, json!(flag)), "{rate}");
    }
    // A typed rate changes it; the current price typed is not an override; a different service takes its price.
    let e = fx.entry(s(&defaulted, "id"));
    let repriced = edit(&mut fx, &e, &support, json!("150.00"), "Reprice");
    assert_eq!((s(&repriced, "amount"), repriced["rate_override"].clone()), ("525.00", json!(false)));
    let moved = edit(&mut fx, &repriced, &remote, json!(""), "Remote instead");
    assert_eq!((s(&moved, "rate"), s(&moved, "amount")), ("95.00", "332.50"));
    fx.reopen("rate edits");
}

/// S2: a grouped line allocates its own rounded amount to its entries exactly, so ticket totals, the invoices made
/// from the ticket and the audit all show what the invoice charges.
#[test]
fn grouped_lines_allocate_their_rounded_amount_to_the_time_exactly() {
    let mut fx = Fx::new();
    let (tavern, support) = (fx.tavern.clone(), fx.support.clone());
    let t = fx.ticket(&tavern, "Rounding");
    let id = s(&t, "id").to_owned();
    for _ in 0..2 {
        fx.time(&tavern, Some(&id), &support, "0.5", Some("0.01"), true).unwrap();
    }
    let t = fx.fresh_ticket(&id);
    assert_eq!(t["totals"]["unbilled"], "0.02", "unbilled time is valued per entry");
    let made = fx.invoice(&t, "SERVICE").unwrap();
    let key = s(&made, "invoice_id").to_owned();
    assert_eq!(fx.invoice_of(&key)["current"]["total"], "0.01");
    let t = &made["ticket"];
    assert_eq!((t["totals"]["billed"].clone(), t["invoices"][0]["amount"].clone()), (json!("0.01"), json!("0.01")));
    let detail: Value = fx.read(|q| Ok(serde_json::from_str(&q.query_row("SELECT detail FROM ledger_auditevent WHERE action='job_ticket.invoiced'", [], |r| r.get::<_, String>(0))?).unwrap()));
    assert_eq!((detail["amount"].clone(), detail["time_value"].clone()), (json!("0.01"), json!("0.02")));
    fx.reopen("grouped invoicing");
    // Void and re-invoice one line per entry: each entry is charged its own amount again.
    sales::void_invoice(&mut fx.book, fx.actor, fx.co, &key, Some(1), &json!("2026-11-15"), &json!("Regroup")).unwrap();
    let again = fx.invoice_fresh(&id, "ENTRY").unwrap();
    assert_eq!(fx.invoice_of(s(&again, "invoice_id"))["current"]["total"], "0.02");
    assert_eq!(again["ticket"]["totals"]["billed"], "0.02");
    // Many small entries at awkward rates: allocations add up to every grouped line and stay within a cent each.
    let other = fx.ticket(&tavern, "Many small entries");
    let oid = s(&other, "id").to_owned();
    for (hours, rate) in [("0.3333", "0.07"), ("0.1667", "0.07"), ("1:20", "33.33"), ("0.0001", "49.99"), ("0.0001", "50.00"), ("2:40", "33.33"), ("0.5", "0.07"), ("0.0003", "0.07")] {
        fx.time(&tavern, Some(&oid), &support, hours, Some(rate), true).unwrap();
    }
    let made = fx.invoice_fresh(&oid, "SERVICE").unwrap();
    let inv = fx.invoice_of(s(&made, "invoice_id"));
    let rows: Vec<(i64, i64, i64, i64)> = fx.read(|q| {
        let mut st = q.prepare("SELECT b.line_position,b.amount,b.invoiced,n.amount FROM jobs_billing b JOIN sales_invoice i ON i.id=b.invoice_id JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.number=i.revision
            JOIN sales_invoiceline n ON n.revision_id=r.id AND n.position=b.line_position WHERE b.invoice_id=?1")?;
        Ok(st.query_map([s(&made, "invoice_id")], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<Vec<_>, _>>()?)
    });
    let mut per_line: BTreeMap<i64, (i64, i64)> = BTreeMap::new();
    for (position, value, invoiced, line_cents) in &rows {
        assert!((value - invoiced).abs() <= 1);
        let e = per_line.entry(*position).or_insert((0, *line_cents));
        e.0 += invoiced;
    }
    assert!(per_line.values().all(|(sum, line)| sum == line), "{per_line:?}");
    let total = money(per_line.values().map(|(sum, _)| sum).sum());
    assert_eq!((inv["current"]["total"].clone(), made["ticket"]["totals"]["billed"].clone()), (json!(total), json!(total)));
    fx.reopen("many small grouped entries");
}

#[test]
fn removing_time_before_a_later_refusal_rolls_back_every_row() {
    let mut fx = Fx::new();
    let (customer, support) = (fx.tavern.clone(), fx.support.clone());
    let ticket = fx.ticket(&customer, "Atomic mixed edit");
    let first = fx.time(&customer, Some(s(&ticket, "id")), &support, "1", None, true).unwrap();
    let second = fx.time(&customer, Some(s(&ticket, "id")), &support, "2", None, true).unwrap();
    let made = fx.invoice_fresh(s(&ticket, "id"), "ENTRY").unwrap();
    let key = s(&made, "invoice_id").to_owned();
    let invoice = fx.invoice_of(&key);
    let before = fb_db::test_support::snapshot(fx.book.connection()).unwrap();
    for (field, value) in [("quantity", json!("1.5")), ("rate", json!("150.00")), ("item_id", json!(fx.remote)), ("line_kind", json!("DISCOUNT"))] {
        let mut retained = line(&invoice["lines"][1]);
        retained[field] = value;
        let mut rows = vec![retained];
        if field == "line_kind" {
            rows.push(json!({"origin_position":null,"description":"Separate charge","quantity":"1","rate":"1000.00","taxable":false,"income_account_id":fx.income.to_db()}));
        }
        let error = message(fx.edit_invoice(&key, &customer, rows).unwrap_err());
        assert!(error.contains("Line 2") && error.contains("JT-000001") && error.contains("remove"), "{field}: {error}");
        assert_eq!(fb_db::test_support::snapshot(fx.book.connection()).unwrap(), before, "{field}: release, revision and audit must roll back");
        assert_eq!(fx.live_billings(), 2);
        assert_eq!(fx.entry(s(&first, "id"))["billed"], true);
        assert_eq!(fx.entry(s(&second, "id"))["billed"], true);
    }
    let error = message(fx.edit_invoice(&key, &fx.diner.clone(), vec![line(&invoice["lines"][1])]).unwrap_err());
    assert!(error.contains("Harbor Lane Tavern"), "{error}");
    assert_eq!(fb_db::test_support::snapshot(fx.book.connection()).unwrap(), before);
    fx.reopen("mixed removal then refusal");
}

#[test]
fn a_hundred_twenty_entries_keep_zero_and_penny_allocations_after_reorder_and_discount() {
    let mut fx = Fx::new();
    let (customer, support) = (fx.tavern.clone(), fx.support.clone());
    let ticket = fx.ticket(&customer, "Many penny entries and free work");
    for number in 0..120 {
        let rate = if number % 3 == 0 { "0.00" } else { "0.01" };
        fx.time(&customer, Some(s(&ticket, "id")), &support, "0.5", Some(rate), true).unwrap();
    }
    let made = fx.invoice_fresh(s(&ticket, "id"), "SERVICE").unwrap();
    let key = s(&made, "invoice_id").to_owned();
    let invoice = fx.invoice_of(&key);
    assert_eq!(invoice["current"]["total"], "0.40");
    assert_eq!(made["ticket"]["totals"]["billed"], "0.40");
    assert_eq!(fx.live_billings(), 120);
    let rows: Vec<(i64, i64, i64)> = fx.read(|query| {
        let mut statement = query.prepare("SELECT live.line_position,sum(live.invoiced),line.amount FROM jobs_livebilling live JOIN sales_invoiceline line ON line.revision_id=live.revision_id AND line.position=live.line_position WHERE live.invoice_id=?1 GROUP BY live.line_position")?;
        Ok(statement.query_map([&key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?.collect::<Result<Vec<_>, _>>()?)
    });
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.1 == row.2), "{rows:?}");
    let adjusted = fx.edit_invoice(&key, &customer, vec![line(&invoice["lines"][1]), line(&invoice["lines"][0]),
        json!({"origin_position":null,"description":"Manual discount","quantity":"1","rate":"0.10","taxable":false,"line_kind":"DISCOUNT","item_id":fx.router,"income_account_id":fx.income.to_db()})]).unwrap();
    assert_eq!(adjusted["current"]["total"], "0.30");
    fx.reopen("reordering zero and penny lines with discount");
    let review = sales::review_invoice(&mut fx.book, fx.actor, fx.co, &key, Some(2)).unwrap();
    sales::post_invoice(&mut fx.book, fx.actor, fx.co, &key, Some(2), Some(&review)).unwrap();
    fx.reopen("posting the many-entry discounted invoice");
    sales::void_invoice(&mut fx.book, fx.actor, fx.co, &key, Some(3), &json!("2026-11-16"), &json!("Release all grouped time")).unwrap();
    assert_eq!(fx.live_billings(), 0);
    fx.reopen("voiding all 120 entries");
}

fn enable_time_tax(fx: &mut Fx, customer: &str, item: &str, exempt: bool) {
    directory::save_record(&mut fx.book, fx.actor, fx.co, Model::Item, &json!({"taxable":true}), Some(item)).unwrap();
    if exempt {
        directory::save_record(&mut fx.book, fx.actor, fx.co, Model::Customer, &json!({"tax_status":"EXEMPT","exemption_number":"SYNTHETIC-ONLY"}), Some(customer)).unwrap();
    }
    let collected = services::save_account(&mut fx.book, fx.actor, fx.co, AccountInput::new("210", "Collected tax").kind("LIABILITY")).unwrap().id;
    let company = fx.co;
    let mut setup = fx.read(|query| documents::setup_snapshot(query, company));
    setup["collected_tax"] = json!(collected.to_db());
    documents::save_setup(&mut fx.book, fx.actor, fx.co, &setup, Some(1)).unwrap();
    let rate = crate::salestax::save_rate(&mut fx.book, fx.actor, fx.co, &json!({"name":"Synthetic tax","description":"Synthetic tax","rate":"7.5","new_agency_name":"Synthetic Revenue","expected_revision":0})).unwrap();
    crate::salestax::save_settings(&mut fx.book, fx.actor, fx.co, &json!({"charge_tax":true,"default_item_id":rate["id"],"owed_basis":"ACCRUAL","filing_frequency":"QUARTERLY","expected_revision":0})).unwrap();
}

#[test]
fn taxable_time_reorders_and_restores_then_posts_and_voids() {
    let mut fx = Fx::new();
    let (customer, support) = (fx.tavern.clone(), fx.support.clone());
    enable_time_tax(&mut fx, &customer, &support, false);
    let ticket = fx.ticket(&customer, "Tax and backup restore");
    for _ in 0..2 {
        fx.time(&customer, Some(s(&ticket, "id")), &support, "0.5", Some("1.00"), true).unwrap();
    }
    let made = fx.invoice_fresh(s(&ticket, "id"), "SERVICE").unwrap();
    let key = s(&made, "invoice_id").to_owned();
    let invoice = fx.invoice_of(&key);
    assert_eq!((invoice["current"]["subtotal"].clone(), invoice["current"]["tax"].clone(), invoice["current"]["total"].clone()), (json!("1.00"), json!("0.08"), json!("1.08")));
    assert_eq!(made["ticket"]["totals"]["billed"], "1.00");
    sales::save_invoice(&mut fx.book, fx.actor, fx.co, &json!({"invoice_id":key,"expected_revision":1,"customer_id":customer,"date":"2026-11-15","tax_mode":"KEEP","lines":[
        {"origin_position":null,"description":"Separate charge","quantity":"1","rate":"1.00","taxable":false,"income_account_id":fx.income.to_db()}, line(&invoice["lines"][0])]})).unwrap();
    fx.reopen("moving a taxable time line");
    let directory = tempfile::tempdir().unwrap();
    let work = directory.path().join("work");
    let backup_directory = directory.path().join("backups");
    std::fs::create_dir(&backup_directory).unwrap();
    let archive = backup_directory.join("time.fbbackup");
    let target = directory.path().join("restored.fbbook");
    crate::backups::create_backup(&fx.book, &[47; 32], &crate::backups::BackupOptions {
        output: &archive, work_root: &work, receipts_root: None, verify_receipt: None, label: "Synthetic taxable time",
        app_version: "test", created_at: "2026-11-16T12:00:00Z", passphrase: None, keep: None, allow_incomplete: false,
    }).unwrap();
    let receipt = crate::backups::restore_backup(&archive, &[47; 32], &crate::backups::RestoreOptions {
        target: &target, receipts_target: None, work_root: &work, protected: std::slice::from_ref(&fx.path), passphrase: None,
        require_encrypted: false, verify_receipt: None,
    }).unwrap();
    assert!(!receipt.upgraded);
    let mut restored = Book::open(&target, &[47; 32]).unwrap();
    let review = sales::review_invoice(&mut restored, fx.actor, fx.co, &key, Some(2)).unwrap();
    sales::post_invoice(&mut restored, fx.actor, fx.co, &key, Some(2), Some(&review)).unwrap();
    sales::void_invoice(&mut restored, fx.actor, fx.co, &key, Some(3), &json!("2026-11-16"), &json!("Restored time canceled")).unwrap();
    assert_eq!(restored.connection().query_row("SELECT count(*) FROM jobs_livebilling", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    assert!(restored.check().unwrap().is_empty());
    drop(restored);
    assert!(Book::open(&target, &[47; 32]).unwrap().check().unwrap().is_empty());
}

#[test]
fn zero_only_time_cancels_and_nonzero_exempt_time_posts() {
    let mut fx = Fx::new();
    let (customer, support) = (fx.tavern.clone(), fx.support.clone());
    enable_time_tax(&mut fx, &customer, &support, true);
    let ticket = fx.ticket(&customer, "Free work");
    let entry = fx.time(&customer, Some(s(&ticket, "id")), &support, "1", Some("0.00"), true).unwrap();
    let made = fx.invoice_fresh(s(&ticket, "id"), "SERVICE").unwrap();
    let key = s(&made, "invoice_id").to_owned();
    let invoice = fx.invoice_of(&key);
    assert_eq!(invoice["lines"][0]["taxable"], false);
    assert_eq!(invoice["current"]["total"], "0.00");
    fx.reopen("zero-only time invoice");
    let error = message(sales::review_invoice(&mut fx.book, fx.actor, fx.co, &key, Some(1)).unwrap_err());
    assert!(error.contains("zero-total"), "{error}");
    sales::void_invoice(&mut fx.book, fx.actor, fx.co, &key, Some(1), &json!("2026-11-16"), &json!("Cancel free-work draft")).unwrap();
    assert_eq!(fx.entry(s(&entry, "id"))["unbilled"], true);
    fx.reopen("canceling free work");
    let ticket = fx.ticket(&customer, "Exempt work");
    fx.time(&customer, Some(s(&ticket, "id")), &support, "1", Some("1.00"), true).unwrap();
    let made = fx.invoice_fresh(s(&ticket, "id"), "ENTRY").unwrap();
    let key = s(&made, "invoice_id");
    assert_eq!(fx.invoice_of(key)["current"]["tax"], "0.00");
    let review = sales::review_invoice(&mut fx.book, fx.actor, fx.co, key, Some(1)).unwrap();
    sales::post_invoice(&mut fx.book, fx.actor, fx.co, key, Some(1), Some(&review)).unwrap();
    fx.reopen("posting exempt time");
}
