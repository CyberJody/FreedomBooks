//! SIM-FIX: findings of the v1 virtual-employee run, checked against the desktop screens
//! (TRIAGE.md at the repo root has the decision and the v1 file:line for each item).
use fb_app::{App, Command, Outcome, keys, screens};
use fb_core::services;
use serde_json::json;

struct Fixture {
    app: App,
    c: String,
    customer: String,
    income: String,
    bank: String,
    ar: String,
    uf: String,
}

impl Fixture {
    fn new() -> Self {
        let mut app = App::new();
        let path = std::env::temp_dir().join(format!("fb-app-simfix-{}.fbbook", uuid::Uuid::new_v4()));
        let c = app.create_book(&path, &keys::new_master_key().unwrap(), "Owner", "Synthetic SIM-FIX books").unwrap().company_id;
        let o = app.open_mut().unwrap();
        let mut account = |code: &str, kind: &str, detail: Option<&str>| {
            let mut v = services::AccountInput::new(code, code);
            v.kind = Some(kind.into());
            v.detail_type = detail.map(Into::into);
            services::save_account(&mut o.book, o.owner, c, v).unwrap().id.to_db()
        };
        let ar = account("1100", "ASSET", Some("RECEIVABLE"));
        let uf = account("1200", "ASSET", None);
        let bank = account("1000", "ASSET", Some("BANK"));
        let income = account("4000", "INCOME", None);
        fb_core::documents::save_setup(&mut o.book, o.owner, c, &json!({"receivables":ar,"undeposited":uf,"banks":[bank]}), Some(0)).unwrap();
        let customer = fb_core::directory::save_record(&mut o.book, o.owner, c, fb_core::directory::Model::Customer, &json!({"display_name":"Synthetic customer"}), None)
            .unwrap()["id"].as_str().unwrap().into();
        Self { app, c: c.to_db(), customer, income, bank, ar, uf }
    }

    fn posted_invoice(&mut self, amount: &str) -> String {
        let values = serde_json::from_value(json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":self.customer,"date":"2026-10-01","due_date":"2026-11-01","tax_mode":"NONE","lines":[{"description":"Work","quantity":"1","rate":amount,"income_account_id":self.income}]})).unwrap();
        let Outcome::Done { data } = self.app.run(Command::SaveInvoiceDraft { company_id: self.c.clone(), invoice_id: None, expected_revision: 0, values }).unwrap() else { panic!() };
        let id: String = data["id"].as_str().unwrap().into();
        self.app.run(Command::PostInvoice { company_id: self.c.clone(), invoice_id: id.clone(), expected_revision: 1 }).unwrap();
        id
    }

    /// Review and record a customer receipt for `amount` against `invoice` through the real form.
    fn pay(&mut self, invoice: &str, amount: &str) {
        let path = format!("/c/{}/receipts/new/", self.c);
        let html = render(&mut self.app, &format!("{path}?customer={}", self.customer));
        let mut p = pairs(&[
            ("request_key", &field(&html, "request_key")), ("customer", &self.customer), ("date", "2026-10-02"), ("amount", amount),
            ("method", "CASH"), ("destination", "UNDEPOSITED"), ("allocations-TOTAL_FORMS", "3"), ("allocations-INITIAL_FORMS", "0"),
            ("allocations-0-invoice", invoice), ("allocations-0-amount", amount),
        ]);
        let mut review = p.clone();
        review.push(("action".into(), "review".into()));
        let Outcome::Page { html, .. } = post(&mut self.app, &path, &html, review) else { panic!("receipt review failed") };
        p.extend(pairs(&[("action", "record"), ("review_token", &field(&html, "review_token"))]));
        assert!(matches!(post(&mut self.app, &path, &html, p), Outcome::Redirect { .. }), "receipt record failed");
    }
}

fn field(html: &str, name: &str) -> String {
    html.split_once(&format!("name=\"{name}\" value=\"")).unwrap_or_else(|| panic!("missing {name}")).1.split('"').next().unwrap().into()
}
fn post(app: &mut App, path: &str, html: &str, p: Vec<(String, String)>) -> Outcome {
    app.run(Command::SubmitForm { path: path.into(), token: field(html, "fb_command_token"), fields: p }).unwrap()
}
fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
    p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}
fn render(app: &mut App, path: &str) -> String {
    screens::render(app, path).unwrap().html
}
/// The document panel's status line: `<number> · <status> · <currency>`.
fn status_line(html: &str) -> String {
    let panel = html.split_once("<section class=\"panel estimate-document\">").unwrap().1;
    panel.split_once("</h2><p>").unwrap().1.split("</p>").next().unwrap().into()
}

// Item 2: a fully paid invoice read "Open" in its detail header.
#[test]
fn item2_paid_invoice_header_shows_payment_state_with_posting_state() {
    let mut f = Fixture::new();
    let paid = f.posted_invoice("100.00");
    let part = f.posted_invoice("80.00");
    let open = f.posted_invoice("60.00");
    f.pay(&paid, "100.00");
    f.pay(&part, "30.00");
    let detail = |f: &mut Fixture, id: &str| render(&mut f.app, &format!("/c/{}/invoices/{id}/", f.c));
    let html = detail(&mut f, &paid);
    assert_eq!(status_line(&html), "INV-000001 · Posted · Paid · USD");
    assert!(html.contains("Current payment state: Paid"));
    assert!(html.contains("<strong id=\"outstanding\">0.00</strong>"), "balance unchanged");
    assert_eq!(status_line(&detail(&mut f, &part)), "INV-000002 · Posted · Partial · USD");
    assert_eq!(status_line(&detail(&mut f, &open)), "INV-000003 · Posted · Open · USD");
    // Historical revisions keep the state recorded in that revision (no live payment state there).
    let revision = render(&mut f.app, &format!("/c/{}/invoices/{paid}/revisions/2/", f.c));
    assert_eq!(status_line(&revision), "INV-000001 · Open · USD");
    // A draft and a voided invoice keep their posting state word.
    let values = serde_json::from_value(json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":f.customer,"date":"2026-10-01","tax_mode":"NONE","lines":[{"description":"Draft work","quantity":"1","rate":"5.00","income_account_id":f.income}]})).unwrap();
    let Outcome::Done { data } = f.app.run(Command::SaveInvoiceDraft { company_id: f.c.clone(), invoice_id: None, expected_revision: 0, values }).unwrap() else { panic!() };
    let draft = data["id"].as_str().unwrap().to_owned();
    assert!(status_line(&detail(&mut f, &draft)).ends_with(" · Draft · USD"));
    // The invoice list already reads the payment state (v1 and v2 alike).
    assert!(render(&mut f.app, &format!("/c/{}/invoices/", f.c)).contains("<td>Paid</td>"));
    let _ = &f.bank;
}

// Item 2, print copy: the current revision's print reads the payment state too (TRIAGE.md item 2).
#[test]
fn item2_invoice_print_copy_shows_payment_state() {
    let mut f = Fixture::new();
    let paid = f.posted_invoice("10.00");
    let open = f.posted_invoice("20.00");
    f.pay(&paid, "10.00");
    let print = |f: &mut Fixture, id: &str| screens::render(&mut f.app, &format!("/c/{}/invoices/{id}/print/", f.c)).unwrap().html;
    assert!(print(&mut f, &paid).contains("· Status: Posted · Paid<br>"));
    assert!(print(&mut f, &open).contains("· Status: Posted · Open<br>"));
}

/// `(value, label)` of every option of `<select name="{name}">`.
fn options(html: &str, name: &str) -> Vec<(String, String)> {
    let select = html.split_once(&format!("<select name=\"{name}\"")).unwrap_or_else(|| panic!("no select {name}")).1.split("</select>").next().unwrap();
    select.split("<option value=\"").skip(1).map(|o| {
        let (value, rest) = o.split_once('"').unwrap();
        (value.into(), rest.split_once('>').unwrap().1.split('<').next().unwrap().into())
    }).collect()
}
fn run(app: &mut App, v: serde_json::Value) -> serde_json::Value {
    match app.run(fb_app::commands::parse_command(&v.to_string()).unwrap()).unwrap() { Outcome::Done { data } => data, v => panic!("{v:?}") }
}

// Item 3: the transfer list's "Bank (from or to)" filter listed every asset account.
#[test]
fn item3_transfer_filter_lists_designated_banks_and_banks_used_by_transfers() {
    let mut f = Fixture::new();
    let c = f.c.clone();
    let account = |app: &mut App, code: &str, detail: &str| run(app, json!({"type":"SaveAccount","company_id":c,"code":code,"name":format!("Acct {code}"),"detail_type":detail}))["id"].as_str().unwrap().to_owned();
    let savings = account(&mut f.app, "1010", "BANK");
    let reserve = account(&mut f.app, "1020", "BANK");
    let prepaid = account(&mut f.app, "1300", "OTHER_CURRENT_ASSET");
    let (ar, uf) = (f.ar.clone(), f.uf.clone());
    let setup = |app: &mut App, banks: Vec<&str>, rev: i64| {
        let o = app.open_mut().unwrap();
        fb_core::documents::save_setup(&mut o.book, o.owner, c.parse().unwrap(), &json!({"receivables":ar,"undeposited":uf,"banks":banks}), Some(rev)).unwrap();
    };
    setup(&mut f.app, vec![&f.bank, &savings], 1);
    let values = json!({"request_key":uuid::Uuid::new_v4().to_string(),"date":"2026-10-31","from_account_id":f.bank,"to_account_id":savings,"amount":"20.00","memo":"Reserve"});
    let review = run(&mut f.app, json!({"type":"ReviewTransfer","company_id":c,"values":values}));
    run(&mut f.app, json!({"type":"RecordTransfer","company_id":c,"values":values,"review_token":review["token"]}));
    // Savings stops being a designated bank; Reserve becomes one.
    setup(&mut f.app, vec![&f.bank, &reserve], 2);
    let list = render(&mut f.app, &format!("/c/{c}/transfers/"));
    let values: Vec<String> = options(&list, "account").into_iter().map(|(v, _)| v).collect();
    let mut want = vec![String::new(), f.bank.clone(), savings.clone(), reserve.clone()];
    want[1..].sort_by_key(|id| ["1000", "1010", "1020"][[&f.bank, &savings, &reserve].iter().position(|x| *x == id).unwrap()]);
    assert_eq!(values, want, "designated banks plus a bank an existing transfer used; no A/R, undeposited funds or other assets");
    assert!(!values.contains(&prepaid));
    // The entry form's choices are the designated banks only (v1 and v2 alike).
    let form = render(&mut f.app, &format!("/c/{c}/transfers/new/"));
    let entry: Vec<String> = options(&form, "from_account").into_iter().map(|(v, _)| v).filter(|v| !v.is_empty()).collect();
    assert_eq!(entry, vec![f.bank.clone(), reserve.clone()]);
    // Filtering by the stale bank still finds its transfer; an asset that is not a bank is refused.
    assert!(render(&mut f.app, &format!("/c/{c}/transfers/?account={savings}")).contains("TRF-000001"));
    let refused = screens::render(&mut f.app, &format!("/c/{c}/transfers/?account={prepaid}")).unwrap();
    assert_eq!(refused.status, 400);
    assert!(refused.html.contains("Select a valid choice."));
}

const OLD_TWIN_HINT: &str = "use town or email to tell them apart in lists.";
const TWIN_HINT: &str = "Required. Two customers or vendors may share a name; lists tell them apart by email, or else by a number.";

// Item 4: the twin-name hint said "use town or email", but there is no Town field. v1 takes the town
// from line 2 of the billing address; the desktop refuses line breaks in new text (SEC-FIX1), so a
// desktop-entered record never has a town and the lists fall back to email, then a number.
#[test]
fn item4_twin_name_hint_matches_what_the_lists_use() {
    let mut f = Fixture::new();
    let c = f.c.clone();
    for path in ["lists/customers/new/", "lists/vendors/new/"] {
        let html = render(&mut f.app, &format!("/c/{c}/{path}"));
        assert!(html.contains(TWIN_HINT), "{path}");
        assert!(!html.contains(OLD_TWIN_HINT), "{path}");
        assert!(!html.contains("name=\"town\""), "still no Town field");
    }
    // A two-line address (v1's only source of a town) is refused by the form.
    let path = format!("/c/{c}/lists/customers/new/");
    let html = render(&mut f.app, &path);
    let Outcome::Invalid { html } = post(&mut f.app, &path, &html, pairs(&[("display_name", "Twin"), ("billing_address", "1 Main St\r\nPond, NY 10000"), ("tax_status", "TAXABLE")])) else { panic!("line break accepted") };
    assert!(html.contains("Control and formatting characters are not allowed."));
    // Twins entered on the desktop: the list tells them apart by email, then by a number.
    for (email, address) in [("a@example.invalid", "1 Main St, Pond, NY 10000"), ("", "9 Elm Rd, Lakeside"), ("", "")] {
        let html = render(&mut f.app, &path);
        let out = post(&mut f.app, &path, &html, pairs(&[("display_name", "Twin"), ("email", email), ("billing_address", address), ("tax_status", "TAXABLE")]));
        assert!(matches!(out, Outcome::Redirect { .. }), "{out:?}");
    }
    let labels: Vec<String> = options(&render(&mut f.app, &format!("/c/{c}/invoices/new/")), "customer").into_iter().map(|(_, l)| l).filter(|l| l.starts_with("Twin")).collect();
    assert_eq!(labels.len(), 3);
    assert_eq!(labels[0], "Twin (a@example.invalid)", "email first");
    assert!(labels[1..].iter().all(|l| l.starts_with("Twin (#")), "then a number: {labels:?}");
    assert!(!labels.iter().any(|l| l.contains("Pond") || l.contains("Lakeside")), "no town from a one-line address");
}

fn attr<'a>(html: &'a str, start: &str, name: &str) -> &'a str {
    let from = html.split_once(start).unwrap_or_else(|| panic!("no {start}")).1;
    from.split_once(&format!("{name}=\"")).unwrap().1.split('"').next().unwrap()
}
fn report_body(html: &str) -> &str {
    html.split_once("</form>").unwrap().1
}

// Item 5: "Last month" did not move the From/To boxes before Run, and the address / Print / CSV kept the
// stale submitted dates while the preset overrode them.
#[test]
fn item5_report_preset_and_dates_agree_and_math_is_unchanged() {
    let mut f = Fixture::new();
    f.app.today = || chrono::NaiveDate::from_ymd_opt(2026, 11, 4).unwrap();
    f.posted_invoice("100.00"); // dated 2026-10-01, inside last month
    let c = f.c.clone();
    for report in ["profit-and-loss", "general-ledger"] {
        let base = format!("/c/{c}/reports/{report}/");
        let stale = render(&mut f.app, &format!("{base}?preset=last_month&start=2026-01-01&end=2026-01-31&basis=ACCRUAL"));
        // Each preset option says which dates it stands for (the preload fills From/To from these).
        assert!(stale.contains(r#"<option value="last_month" data-start="2026-10-01" data-end="2026-10-31" selected>Last month</option>"#), "{report}");
        assert!(stale.contains(r#"<option value="this_month" data-start="2026-11-01" data-end="2026-11-30">This month</option>"#));
        assert!(stale.contains(r#"<option value="custom">Custom</option>"#), "Custom carries no dates");
        // The boxes show what the report used (v1 already did this after Run).
        assert!(stale.contains(r#"<input type="date" name="start" value="2026-10-01">"#));
        assert!(stale.contains(r#"<input type="date" name="end" value="2026-10-31">"#));
        // Print and CSV carry the resolved dates, not the stale ones.
        let actions = stale.split_once("<span class=\"report-actions\">").unwrap().1.split("</span>").next().unwrap();
        assert!(!actions.contains("2026-01-"), "{actions}");
        assert_eq!(actions.matches("preset=last_month&amp;start=2026-10-01&amp;end=2026-10-31&amp;basis=ACCRUAL").count(), 2, "{actions}");
        // Report math unchanged: the same body as the explicit custom range.
        let custom = render(&mut f.app, &format!("{base}?preset=custom&start=2026-10-01&end=2026-10-31&basis=ACCRUAL"));
        assert_eq!(report_body(&stale), report_body(&custom), "{report}");
        assert!(report_body(&stale).contains("100.00"));
        // A preset without dates in the query keeps its links exactly as before.
        let plain = render(&mut f.app, &format!("{base}?preset=last_month"));
        assert_eq!(attr(&plain, "<span class=\"report-actions\">", "href"), format!("{base}?preset=last_month&amp;output=print"));
    }
    // The CSV itself runs the preset's dates.
    let csv = render(&mut f.app, &format!("/c/{c}/reports/profit-and-loss/?preset=last_month&start=2026-01-01&end=2026-01-31&output=csv"));
    assert!(csv.contains("2026-10-01") && csv.contains("2026-10-31") && !csv.contains("2026-01-31"), "{csv}");
}

// Item 6: v1's upgrade lost the tax setup (its migrations backfilled defaults: sales tax off, no rate,
// every customer taxable, every item taxable then services not). The desktop has no v1 import and no
// book migration that rewrites tax setup; this pins both, so a future importer must handle it.
#[test]
fn item6_no_v1_import_or_tax_backfill_in_the_desktop() {
    let tax_columns = ["taxable", "tax_status", "exemption_number", "charge_tax", "default_item_id", "owed_basis"];
    for (name, sql) in fb_db::MIGRATIONS {
        let lower = sql.to_lowercase();
        for statement in lower.split(';') {
            let s = statement.trim_start();
            let writes_tax = (s.starts_with("update") || s.starts_with("insert"))
                && (s.contains("directory_customer") || s.contains("directory_item") || s.contains("salestax_"))
                && tax_columns.iter().any(|c| s.contains(c));
            assert!(!writes_tax, "migration {name} backfills tax setup: {s:.160}");
            assert!(!(s.starts_with("alter table") && tax_columns.iter().any(|c| s.contains(&format!("add column {c}")))), "migration {name} adds a tax column to existing books: {s:.160}");
        }
    }
    // The only desktop commands that bring data in are typed forms; nothing reads a v1 database.
    for forbidden in ["ImportV1", "ImportLegacy", "ImportDjango", "UpgradeFromV1"] {
        let probe = json!({"type": forbidden}).to_string();
        assert!(fb_app::commands::parse_command(&probe).is_err(), "{forbidden} exists: handle SIM-FIX item 6 there");
    }
    // A manual-rate invoice keeps its own rate; nothing turns it into company settings or a tax item.
    let mut f = Fixture::new();
    let values = serde_json::from_value(json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":f.customer,"date":"2026-10-01","tax_mode":"MANUAL","tax_label":"County tax","tax_rate":"7.5","lines":[{"description":"Work","quantity":"1","rate":"100.00","taxable":true,"income_account_id":f.income}]})).unwrap();
    let Outcome::Done { data } = f.app.run(Command::SaveInvoiceDraft { company_id: f.c.clone(), invoice_id: None, expected_revision: 0, values }).unwrap() else { panic!() };
    let html = render(&mut f.app, &format!("/c/{}/invoices/{}/", f.c, data["id"].as_str().unwrap()));
    assert!(html.contains("County tax · 7.5"), "manual rate kept on the document");
    let o = f.app.open_mut().unwrap();
    let (items, charge): (i64, i64) = o.book.read(|tx| tx.query_row("SELECT (SELECT count(*) FROM salestax_taxitem),(SELECT coalesce(max(charge_tax),0) FROM salestax_salestaxsettings)", [], |r| Ok((r.get(0)?, r.get(1)?)))).unwrap();
    assert_eq!((items, charge), (0, 0));
}

// Extra: audit history showed code names ("salestax.rate.created") and "Record (no longer on file)" for
// sales tax records that are on file.
#[test]
fn extra_audit_history_names_sales_tax_actions_and_records() {
    let mut f = Fixture::new();
    let c = f.c.clone();
    let rate = run(&mut f.app, json!({"type":"SaveTaxRate","company_id":c,"item_id":null,"expected_revision":null,"name":"County","description":"County tax","rate":"7.5","agency_id":null,"new_agency_name":"County treasurer"}));
    let rate_id = rate["id"].as_str().unwrap().to_owned();
    run(&mut f.app, json!({"type":"SetTaxItemActive","company_id":c,"item_id":rate_id,"active":false,"expected_revision":rate["revision"]}));
    let html = render(&mut f.app, &format!("/c/{c}/history/"));
    let table = html.split_once("<tbody>").unwrap().1.split("</tbody>").next().unwrap();
    assert!(!table.contains("salestax."), "no code names: {table}");
    assert!(table.contains(r#"<td data-label="Action">Sales tax rate created</td>"#));
    assert!(table.contains(r#"<td data-label="Action">Sales tax rate made inactive</td>"#));
    assert!(table.contains(&format!(r#"<a href="/c/{c}/sales-tax/rates/{rate_id}/edit/">County</a>"#)), "{table}");
    assert!(!table.contains("no longer on file"), "{table}");
    // The filter offers the new labels and accepts them.
    assert!(html.contains(r#"<option value="salestax.rate.created">Sales tax rate created</option>"#));
    let filtered = screens::render(&mut f.app, &format!("/c/{c}/history/?action=salestax.rate.created")).unwrap();
    assert_eq!(filtered.status, 200);
    assert_eq!(filtered.html.matches("Sales tax rate created</td>").count(), 1);
}
