//! Sales forms exercise real encrypted books and opaque review capabilities.
mod support;
use fb_app::{App, Command, Outcome, keys, screens};
use fb_core::{CompanyId, services};
use serde_json::json;

struct Fixture {
    app: App,
    c: String,
    customer: String,
    income: String,
    ar: String,
    bank: String,
    uf: String,
}
impl Fixture {
    fn new() -> Self {
        let mut app = App::new();
        let path = std::env::temp_dir().join(format!("fb-app-d4-{}.fbbook", uuid::Uuid::new_v4()));
        let c = app
            .create_book(
                &path,
                &keys::new_master_key().unwrap(),
                "Owner",
                "Synthetic sales forms",
            )
            .unwrap()
            .company_id;
        support::sign_in(&mut app);
        let o = app.open_mut().unwrap();
        let mut account = |code: &str, kind: &str| {
            let mut v = services::AccountInput::new(code, code);
            v.kind = Some(kind.into());
            services::save_account(&mut o.book, o.owner, c, v)
                .unwrap()
                .id
                .to_db()
        };
        let ar = account("1100", "ASSET");
        let uf = account("1200", "ASSET");
        let bank = account("1000", "ASSET");
        let income = account("4000", "INCOME");
        fb_core::documents::save_setup(
            &mut o.book,
            o.owner,
            c,
            &json!({"receivables":ar,"undeposited":uf,"banks":[bank]}),
            Some(0),
        )
        .unwrap();
        let customer = fb_core::directory::save_record(
            &mut o.book,
            o.owner,
            c,
            fb_core::directory::Model::Customer,
            &json!({"display_name":"Synthetic customer"}),
            None,
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .into();
        Self {
            app,
            c: c.to_db(),
            customer,
            income,
            ar,
            bank,
            uf,
        }
    }
    fn invoice(&mut self) -> String {
        self.invoice_on("2026-10-01", "100.00")
    }
    /// A posted invoice for the fixture customer with one line at `rate`.
    fn posted(&mut self, date: &str, rate: &str) -> String {
        let id = self.invoice_on(date, rate);
        self.app.run(Command::PostInvoice { company_id: self.c.clone(), invoice_id: id.clone(), expected_revision: 1 }).unwrap();
        id
    }
    fn outstanding(&mut self, invoice: &str) -> serde_json::Value {
        let o = self.app.open_mut().unwrap();
        fb_core::sales::invoice_balances(&mut o.book, o.owner, self.c.parse().unwrap()).unwrap()
            .into_iter().find(|b| b["invoice_id"] == invoice).unwrap()["outstanding"].clone()
    }
    fn invoice_on(&mut self, date: &str, rate: &str) -> String {
        let values=serde_json::from_value(json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":self.customer,"date":date,"due_date":"2026-11-01","tax_mode":"NONE","lines":[{"description":"Work","quantity":"1","rate":rate,"income_account_id":self.income}]})).unwrap();
        let Outcome::Done { data } = self
            .app
            .run(Command::SaveInvoiceDraft {
                company_id: self.c.clone(),
                invoice_id: None,
                expected_revision: 0,
                values,
            })
            .unwrap()
        else {
            panic!()
        };
        data["id"].as_str().unwrap().into()
    }
    fn journal_count(&mut self) -> i64 {
        self.app
            .open_mut()
            .unwrap()
            .book
            .read(|tx| tx.query_row("SELECT count(*) FROM ledger_journal", [], |r| r.get(0)))
            .unwrap()
    }
}
fn field(html: &str, name: &str) -> String {
    html.split_once(&format!("name=\"{name}\" value=\""))
        .unwrap_or_else(|| panic!("missing {name}: {html}"))
        .1
        .split('"')
        .next()
        .unwrap()
        .into()
}
fn post(app: &mut App, path: &str, html: &str, p: Vec<(String, String)>) -> Outcome {
    app.run(Command::SubmitForm {
        path: path.into(),
        token: field(html, "fb_command_token"),
        fields: p,
    })
    .unwrap()
}
fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
    p.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}
fn render(app: &mut App, path: &str) -> String {
    screens::render(app, path).unwrap().html
}

/// The Post form's fields as the invoice page rendered them, with the button pressed.
fn post_fields(html: &str, then: &str) -> Vec<(String, String)> {
    let form = html.split("class=\"invoice-post\"").nth(1).expect("a Post form");
    pairs(&[("expected_revision", &field(form, "expected_revision")), ("review_token", &field(form, "review_token")), ("then", then)])
}

#[test]
fn invoice_post_uses_the_displayed_plan_and_refuses_changed_setup() {
    let mut f = Fixture::new();
    let invoice = f.invoice();
    let detail_path = format!("/c/{}/invoices/{invoice}/", f.c);
    let post_path = format!("/c/{}/invoices/{invoice}/post/", f.c);
    // The draft's own page carries the review of this exact revision and shows its entries.
    let html = render(&mut f.app, &detail_path);
    assert!(html.contains("Show accounting entries") && html.contains("1100 · 1100"));
    let posting = post_fields(&html, "view");
    let o = f.app.open_mut().unwrap();
    let mut a = services::AccountInput::new("1110", "Replacement receivables");
    a.kind = Some("ASSET".into());
    let other = services::save_account(&mut o.book, o.owner, CompanyId::parse(&f.c).unwrap(), a)
        .unwrap()
        .id;
    fb_core::documents::save_setup(
        &mut o.book,
        o.owner,
        f.c.parse().unwrap(),
        &json!({"receivables":other.to_db(),"undeposited":f.uf,"banks":[f.bank]}),
        Some(1),
    )
    .unwrap();
    let outcome = post(&mut f.app, &post_path, &html, posting);
    let Outcome::Invalid { html } = outcome else {
        panic!("expected a changed-plan refusal")
    };
    assert!(html.contains("Not posted. The accounts this invoice posts to changed after this page was opened."), "{html}");
    assert_eq!(f.journal_count(), 0);
    // The refusal page shows the new plan and a fresh Post form for it.
    assert!(html.contains("1110 · Replacement receivables"));
    assert!(matches!(post(&mut f.app, &post_path, &html, post_fields(&html, "view")), Outcome::Redirect { .. }));
    assert_eq!(f.journal_count(), 1);
    let detail = render(&mut f.app, &detail_path);
    assert!(detail.contains("100.00") && detail.contains("Invoice posted."));
    assert!(detail.contains("status-chip open\">Open<") || detail.contains("status-chip overdue\">Overdue<"));
}

#[test]
fn invoice_page_actions_follow_the_state_and_post_refuses_a_changed_draft() {
    let mut f = Fixture::new();
    let invoice = f.invoice();
    let detail_path = format!("/c/{}/invoices/{invoice}/", f.c);
    let post_path = format!("/c/{}/invoices/{invoice}/post/", f.c);
    let html = render(&mut f.app, &detail_path);
    let bar = html.split("class=\"action-bar\"").nth(1).unwrap().split("</nav>").next().unwrap();
    for action in [">Post</button>", ">Edit</a>", ">Print or PDF</a>", ">Delete draft</a>"] {
        assert!(bar.contains(action), "{action}: {bar}");
    }
    // No Gmail connection here, so Post is the main action rather than Post and email.
    assert!(!bar.contains("Post and email"));
    assert!(html.contains("status-chip draft\">Draft<"));
    for gone in ["Posting a draft records", "Line extensions round", "Review posting", "Cancel draft", "Receipt allocation history", "Payments and credits", "Memo:"] {
        assert!(!html.contains(gone), "{gone}");
    }
    // Saving another revision after the page was shown makes its Post refuse, with nothing posted.
    let stale = post_fields(&html, "view");
    let edit_path = format!("/c/{}/invoices/{invoice}/edit/", f.c);
    let form = render(&mut f.app, &edit_path);
    let mut p = invoice_fields(&f, &form);
    for (k, v) in &mut p {
        match k.as_str() { "expected_revision" => *v = "1".into(), "lines-0-rate" => *v = "120.00".into(), _ => {} }
    }
    assert!(matches!(post(&mut f.app, &edit_path, &form, p), Outcome::Redirect { .. }));
    let Outcome::Invalid { html } = post(&mut f.app, &post_path, &html, stale) else { panic!("stale post accepted") };
    assert!(html.contains("Not posted. This invoice changed after this page was opened."), "{html}");
    assert_eq!(f.journal_count(), 0);
    // Posting the current revision, then the open invoice leads with what is owed.
    let Outcome::Redirect { .. } = post(&mut f.app, &post_path, &html, post_fields(&html, "view")) else { panic!() };
    let html = render(&mut f.app, &detail_path);
    let bar = html.split("class=\"action-bar\"").nth(1).unwrap().split("</nav>").next().unwrap();
    assert!(bar.contains(&format!("/receipts/new/?invoice={invoice}\">Receive payment</a>")), "{bar}");
    assert!(bar.contains(">Email</a>") && bar.contains(">Void</a>") && !bar.contains("Delete draft"));
    assert!(html.contains("<span class=\"label\">Balance due</span><strong id=\"header-balance\">120.00</strong>"), "{html}");
    let totals = html.split("class=\"estimate-totals\"").nth(1).unwrap().split("</dl>").next().unwrap();
    assert!(totals.trim_end().ends_with("<dd id=\"balance-due\" class=\"balance\">120.00</dd>"), "the totals end with Balance due: {totals}");
    // A payment shows in one Payments and credits list, and the balance follows it.
    let path = format!("/c/{}/receipts/new/", f.c);
    let form = render(&mut f.app, &format!("{path}?invoice={invoice}"));
    let entries = html.split_once("<table class=\"posted-entries\">").unwrap().1.split_once("</table>").unwrap().0;
    assert_eq!(entries.matches(">120.00<").count(), 2);
    let p = payment(&field(&form, "request_key"), &f.customer, "20.00", &[(&invoice, true, "20.00")]);
    let Outcome::Redirect { location } = post(&mut f.app, &path, &form, p) else { panic!("payment not saved") };
    let receipt = render(&mut f.app, &location);
    assert!(receipt.contains("Still open on invoice") && receipt.contains(">20.00</td><td class=\"num\">100.00</td>"));
    let html = render(&mut f.app, &detail_path);
    assert!(html.contains("Payments and credits") && html.contains("<td>Payment</td>"));
    assert!(html.contains("<dt>Payments received</dt><dd id=\"payments-received\">−20.00</dd>"));
    assert!(html.contains("status-chip partial\">Partly paid<") || html.contains("Partly paid · Overdue"));
}

#[test]
fn post_and_email_continues_to_the_email_preview() {
    let mut f = Fixture::new();
    let invoice = f.invoice();
    let html = render(&mut f.app, &format!("/c/{}/invoices/{invoice}/", f.c));
    let Outcome::Redirect { location } = post(&mut f.app, &format!("/c/{}/invoices/{invoice}/post/", f.c), &html, post_fields(&html, "email")) else { panic!() };
    assert!(location.ends_with(&format!("/invoices/{invoice}/email-preview/")), "{location}");
    assert_eq!(f.journal_count(), 1);
}

#[test]
fn delete_draft_needs_no_date_and_void_explains_itself() {
    let mut f = Fixture::new();
    let draft = f.invoice_on("2026-10-01", "100.00");
    let void_path = format!("/c/{}/invoices/{draft}/void/", f.c);
    let html = render(&mut f.app, &void_path);
    assert!(html.contains("<h1>Delete draft INV-") && html.contains("Reason (optional)"));
    assert!(!html.contains("name=\"date\"") && !html.contains("Reversal date"));
    let Outcome::Redirect { location } = post(&mut f.app, &void_path, &html, pairs(&[("expected_revision", "1"), ("reason", "")])) else { panic!("an optional reason") };
    let page = render(&mut f.app, &location);
    assert!(page.contains("status-chip void\">Deleted draft<"));
    assert_eq!(f.journal_count(), 0);
    let reason: String = f.app.open_mut().unwrap().book.read(|tx| tx.query_row("SELECT json_extract(data,'$.reason') FROM sales_invoicerevision r JOIN sales_invoice i ON i.id=r.invoice_id WHERE i.id=?1 AND r.number=2", [&draft], |r| r.get(0))).unwrap();
    assert_eq!(reason, "No reason given.");
    let posted = f.posted("2026-10-01", "50.00");
    let void_path = format!("/c/{}/invoices/{posted}/void/", f.c);
    let html = render(&mut f.app, &void_path);
    assert!(html.contains("<h1>Void invoice INV-") && html.contains("stays on file marked Void") && html.contains("Void date"));
    let Outcome::Invalid { html } = post(&mut f.app, &void_path, &html, pairs(&[("expected_revision", "2"), ("date", "2026-10-05"), ("reason", "")])) else { panic!("a void needs a reason") };
    assert!(html.contains("This field is required."));
    let Outcome::Redirect { .. } = post(&mut f.app, &void_path, &html, pairs(&[("expected_revision", "2"), ("date", "2026-10-05"), ("reason", "Billed twice")])) else { panic!() };
    assert_eq!(f.journal_count(), 2);
}

/// The Receive payment form fields for `rows` of (invoice, pay box, amount to apply).
fn payment(key: &str, customer: &str, amount: &str, rows: &[(&str, bool, &str)]) -> Vec<(String, String)> {
    let mut p = pairs(&[
        ("request_key", key),
        ("customer", customer),
        ("date", "2026-10-20"),
        ("amount", amount),
        ("method", "CHECK"),
        ("reference", "1042"),
        ("destination", "UNDEPOSITED"),
        ("action", "save"),
    ]);
    p.push(("allocations-TOTAL_FORMS".into(), rows.len().to_string()));
    for (i, (invoice, pay, apply)) in rows.iter().enumerate() {
        p.push((format!("allocations-{i}-invoice"), invoice.to_string()));
        if *pay {
            p.push((format!("allocations-{i}-pay"), "on".into()));
        }
        p.push((format!("allocations-{i}-amount"), apply.to_string()));
    }
    p
}
fn receipt_count(app: &mut App) -> i64 {
    app.open_mut().unwrap().book.read(|tx| tx.query_row("SELECT count(*) FROM sales_receipt", [], |r| r.get(0))).unwrap()
}

#[test]
fn receive_payment_lists_open_invoices_oldest_first_and_records_with_one_save() {
    let mut f = Fixture::new();
    let older = f.posted("2026-09-01", "100.00");
    let newer = f.posted("2026-10-01", "80.00");
    let draft = f.invoice_on("2026-10-05", "55.00");
    let path = format!("/c/{}/receipts/new/", f.c);
    let html = render(&mut f.app, &format!("{path}?customer={}", f.customer));
    assert!(html.contains("<h1>Receive payment</h1>"));
    // One customer field, and that customer's open invoices oldest first; drafts are not payable.
    assert_eq!(html.matches("name=\"customer\"").count(), 1);
    let (first, second) = (html.find(&format!("value=\"{older}\"")).unwrap(), html.find(&format!("value=\"{newer}\"")).unwrap());
    assert!(first < second, "oldest invoice first");
    assert!(!html.contains(&format!("value=\"{draft}\"")));
    assert!(html.contains("Sep 1, 2026") && html.contains("100.00") && html.contains("80.00"));
    // Undeposited funds is the default destination, as on the sales receipt.
    assert!(html.contains("<option value=\"UNDEPOSITED\" selected>Undeposited funds</option>"), "{html}");
    for gone in ["RECORD ONLY", "Synthetic data", "Explicit invoice allocations", "Load invoices", "books only", "review_token"] {
        assert!(!html.contains(gone), "{gone}");
    }
    let before = f.journal_count();
    let key = field(&html, "request_key");
    let out = post(&mut f.app, &path, &html, payment(&key, &f.customer, "150.00", &[(&older, true, "100.00"), (&newer, true, "50.00")]));
    let Outcome::Redirect { location } = out else { panic!("one Save records the payment: {out:?}") };
    assert!(location.contains("/receipts/"));
    assert_eq!(f.journal_count(), before + 1);
    assert_eq!(f.outstanding(&older), "0.00");
    assert_eq!(f.outstanding(&newer), "30.00");
    let detail = render(&mut f.app, &location);
    assert!(detail.contains("Payment RCP-") && detail.contains("150.00"));
    // The journal is the core's: undeposited funds debited, receivables credited, one audit record.
    let (debit, credit, audits): (i64, i64, i64) = f.app.open_mut().unwrap().book.read(|tx| tx.query_row(
        "SELECT (SELECT sum(l.debit) FROM ledger_journalline l JOIN ledger_journal j ON j.id=l.journal_id JOIN sales_receipt r ON r.journal_id=j.id WHERE l.account_id=?1),(SELECT sum(l.credit) FROM ledger_journalline l JOIN ledger_journal j ON j.id=l.journal_id JOIN sales_receipt r ON r.journal_id=j.id WHERE l.account_id=?2),(SELECT count(*) FROM ledger_auditevent WHERE action='receipt.recorded')",
        [&f.uf, &f.ar], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))).unwrap();
    assert_eq!((debit, credit, audits), (15000, 15000, 1));
    // Voiding the payment reverses it and reopens the invoices.
    let reverse_path = format!("{location}reverse/");
    let html = render(&mut f.app, &reverse_path);
    assert!(html.contains("Void payment"));
    let p = pairs(&[("expected_revision", "1"), ("date", "2026-10-21"), ("reason", "Synthetic correction")]);
    let Outcome::Redirect { location } = post(&mut f.app, &reverse_path, &html, p) else { panic!() };
    let detail = render(&mut f.app, &location);
    assert!(detail.contains("Voided") && detail.contains("Synthetic correction"));
    assert_eq!(f.outstanding(&older), "100.00");
}

#[test]
fn receive_payment_from_an_invoice_preselects_it_with_its_balance() {
    let mut f = Fixture::new();
    f.posted("2026-09-01", "100.00");
    let target = f.posted("2026-10-01", "80.00");
    let html = render(&mut f.app, &format!("/c/{}/receipts/new/?invoice={target}", f.c));
    assert!(html.contains(&format!("<option value=\"{}\" selected>", f.customer)), "customer preselected");
    assert_eq!(field(&html, "amount"), "80.00");
    let row = html.split(&format!("value=\"{target}\"")).nth(1).unwrap();
    assert!(row.split("</tr>").next().unwrap().contains(" checked"), "invoice ticked");
    assert!(row.contains("value=\"80.00\""));
    let other = html.split("<tbody id=\"invoice-rows\">").nth(1).unwrap().split("</tr>").next().unwrap();
    assert!(!other.contains(" checked"), "only the invoice it came from is ticked");
}

#[test]
fn receive_payment_refusals_are_plain_keep_the_entries_and_write_nothing() {
    let mut f = Fixture::new();
    let a = f.posted("2026-09-01", "100.00");
    let b = f.posted("2026-10-15", "80.00");
    let path = format!("/c/{}/receipts/new/", f.c);
    let before = (f.journal_count(), receipt_count(&mut f.app));
    type Case<'a> = (Vec<(&'a str, bool, &'a str)>, &'a str, &'a str);
    let cases: Vec<Case> = vec![
        (vec![(&a, true, "100.00")], "150.00", "You applied 100.00 but received 150.00"),
        (vec![(&a, true, "120.00")], "120.00", "More than the open balance of 100.00."),
        (vec![(&a, true, "")], "100.00", "Enter the amount to apply, or untick Pay."),
        (vec![(&a, false, ""), (&b, false, "")], "50.00", "Tick at least one invoice to apply this payment to."),
        // The core's own refusal, said plainly: the payment is dated before invoice b.
        (vec![(&b, true, "80.00")], "80.00", "The payment date is before the date of an invoice you are applying it to."),
    ];
    for (rows, amount, message) in cases {
        let html = render(&mut f.app, &format!("{path}?customer={}", f.customer));
        let mut p = payment(&field(&html, "request_key"), &f.customer, amount, &rows);
        if message.starts_with("The payment date") {
            for (k, v) in &mut p { if k == "date" { *v = "2026-10-01".into(); } }
        }
        let out = post(&mut f.app, &path, &html, p);
        let Outcome::Invalid { html } = out else { panic!("{message}: {out:?}") };
        assert!(html.contains(message), "{message}: {html}");
        assert!(html.contains("Payment not saved."));
        assert_eq!(field(&html, "amount"), amount, "entered amount kept");
        assert!(html.contains("value=\"1042\""), "reference kept");
        assert_eq!((f.journal_count(), receipt_count(&mut f.app)), before, "{message}");
    }
    // A customer with nothing open is pointed at customer deposits instead.
    let o = f.app.open_mut().unwrap();
    let other: String = fb_core::directory::save_record(&mut o.book, o.owner, f.c.parse().unwrap(), fb_core::directory::Model::Customer, &json!({"display_name":"Paid-up customer"}), None).unwrap()["id"].as_str().unwrap().into();
    let html = render(&mut f.app, &format!("{path}?customer={other}"));
    assert!(html.contains("This customer has no open invoices."));
    let Outcome::Invalid { html } = post(&mut f.app, &path, &html, payment(&field(&html, "request_key"), &other, "25.00", &[])) else { panic!() };
    assert!(html.contains("use Receive customer deposit") && html.contains("/customer-deposits/new/?customer="));
    assert_eq!((f.journal_count(), receipt_count(&mut f.app)), before);
}

#[test]
fn receive_payment_saved_twice_records_once_and_shows_the_first() {
    let mut f = Fixture::new();
    let a = f.posted("2026-09-01", "100.00");
    let path = format!("/c/{}/receipts/new/", f.c);
    let html = render(&mut f.app, &format!("{path}?customer={}", f.customer));
    let p = payment(&field(&html, "request_key"), &f.customer, "40.00", &[(&a, true, "40.00")]);
    let Outcome::Redirect { location: first } = post(&mut f.app, &path, &html, p.clone()) else { panic!() };
    let journals = f.journal_count();
    // The same form sent again (a double click, or Back then Save) records nothing new.
    let html = render(&mut f.app, &path);
    let Outcome::Redirect { location } = post(&mut f.app, &path, &html, p) else { panic!() };
    assert_eq!(location, first);
    assert_eq!(f.journal_count(), journals);
    assert_eq!(receipt_count(&mut f.app), 1);
    assert!(render(&mut f.app, &location).contains("This payment was already saved as RCP-"));
    assert_eq!(f.outstanding(&a), "60.00");
}

#[test]
fn invalid_estimate_and_tampered_formsets_keep_input_and_write_nothing() {
    let mut f = Fixture::new();
    let path = format!("/c/{}/estimates/new/", f.c);
    let html = render(&mut f.app, &path);
    let before = f.journal_count();
    let p = pairs(&[
        ("request_key", "bad"),
        ("expected_revision", "0"),
        ("customer", &f.customer),
        ("date", "bad"),
        ("tax_mode", "NONE"),
        ("lines-TOTAL_FORMS", "1"),
        ("lines-0-description", "Retain <my text>"),
        ("lines-0-quantity", "wrong"),
        ("lines-0-rate", "1"),
    ]);
    let Outcome::Invalid { html } = post(&mut f.app, &path, &html, p) else {
        panic!()
    };
    assert!(html.contains("Retain &lt;my text&gt;"));
    assert!(html.contains("ManagementForm data is missing"));
    assert!(html.contains("Enter a valid date."));
    assert_eq!(before, f.journal_count());
    let o = f.app.open_mut().unwrap();
    let n: i64 = o
        .book
        .read(|tx| tx.query_row("SELECT count(*) FROM sales_estimate", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(n, 0);
    // An account UUID cannot be substituted for a scoped customer selection.
    let p = pairs(&[
        ("request_key", &uuid::Uuid::new_v4().to_string()),
        ("expected_revision", "0"),
        ("customer", &f.ar),
        ("date", "2026-10-01"),
        ("tax_mode", "NONE"),
        ("lines-TOTAL_FORMS", "1"),
        ("lines-INITIAL_FORMS", "0"),
        ("lines-0-description", "Work"),
        ("lines-0-quantity", "1"),
        ("lines-0-rate", "1"),
    ]);
    let Outcome::Invalid { html } = post(&mut f.app, &path, &html, p) else {
        panic!()
    };
    assert!(html.contains("Select a valid choice."));
}

#[test]
fn estimate_creation_retry_keeps_only_its_own_archived_choices() {
    let mut f = Fixture::new();
    let o = f.app.open_mut().unwrap();
    let item=fb_core::directory::save_record(&mut o.book,o.owner,f.c.parse().unwrap(),fb_core::directory::Model::Item,&json!({"code":"OLD","description":"Original item","kind":"SERVICE","sale_rate":"10.00","income_account":f.income}),None).unwrap()["id"].as_str().unwrap().to_owned();
    let path = format!("/c/{}/estimates/new/", f.c);
    let html = render(&mut f.app, &path);
    let key = field(&html, "request_key");
    let mut p = pairs(&[
        ("request_key", &key),
        ("expected_revision", "0"),
        ("customer", &f.customer),
        ("date", "2026-10-01"),
        ("tax_mode", "NONE"),
        ("lines-TOTAL_FORMS", "1"),
        ("lines-INITIAL_FORMS", "0"),
        ("lines-0-item", &item),
        ("lines-0-description", "Agreed work"),
        ("lines-0-quantity", "1"),
        ("lines-0-rate", "10.00"),
    ]);
    let Outcome::Redirect { location } = post(&mut f.app, &path, &html, p.clone()) else {
        panic!()
    };
    let o = f.app.open_mut().unwrap();
    for (model, id) in [
        (fb_core::directory::Model::Customer, f.customer.as_str()),
        (fb_core::directory::Model::Item, item.as_str()),
    ] {
        fb_core::directory::set_active(
            &mut o.book,
            o.owner,
            f.c.parse().unwrap(),
            model,
            id,
            false,
        )
        .unwrap();
    }
    let html = render(&mut f.app, &path);
    assert_eq!(
        post(&mut f.app, &path, &html, p.clone()),
        Outcome::Redirect { location }
    );
    for (k, v) in &mut p {
        if k == "request_key" {
            *v = uuid::Uuid::new_v4().to_string();
        }
    }
    let html = render(&mut f.app, &path);
    let Outcome::Invalid { html } = post(&mut f.app, &path, &html, p) else {
        panic!("a new key cannot borrow archived retry choices")
    };
    assert!(html.contains("Select a valid choice."));
    let n: i64 = f
        .app
        .open_mut()
        .unwrap()
        .book
        .read(|tx| tx.query_row("SELECT count(*) FROM sales_estimate", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn payment_retry_after_reversal_void_and_archive_shows_the_original_and_writes_nothing() {
    let mut f = Fixture::new();
    let invoice = f.posted("2026-10-01", "100.00");
    let path = format!("/c/{}/receipts/new/", f.c);
    let html = render(&mut f.app, &format!("{path}?customer={}", f.customer));
    let mut p = payment(&field(&html, "request_key"), &f.customer, "20.00", &[(&invoice, true, "20.00")]);
    for (k, v) in &mut p { if k == "destination" { *v = "BANK".into(); } }
    p.push(("bank".into(), f.bank.clone()));
    let Outcome::Redirect { location } = post(&mut f.app, &path, &html, p.clone()) else { panic!() };
    let id = location.trim_end_matches('/').rsplit('/').next().unwrap().to_owned();
    f.app.run(Command::ReverseCustomerReceipt { company_id: f.c.clone(), receipt_id: id, sales_receipt: false, expected_revision: 1, date: "2026-10-21".into(), reason: "Correction".into() }).unwrap();
    f.app.run(Command::VoidInvoice { company_id: f.c.clone(), invoice_id: invoice, expected_revision: 2, date: "2026-10-22".into(), reason: "Correction".into() }).unwrap();
    let o = f.app.open_mut().unwrap();
    fb_core::directory::set_active(&mut o.book, o.owner, f.c.parse().unwrap(), fb_core::directory::Model::Customer, &f.customer, false).unwrap();
    fb_core::documents::save_setup(&mut o.book, o.owner, f.c.parse().unwrap(), &json!({"receivables":f.ar,"undeposited":f.uf,"banks":[]}), Some(1)).unwrap();
    let before = audit_count(&mut f.app);
    let journals = f.journal_count();
    let html = render(&mut f.app, &path);
    let Outcome::Redirect { location: again } = post(&mut f.app, &path, &html, p) else { panic!("a recorded form shows its payment") };
    assert_eq!(again, location);
    assert_eq!((audit_count(&mut f.app), f.journal_count()), (before, journals));
}
#[test]
fn invoice_review_findings_clean_each_field_before_cross_validation() {
    let mut f=Fixture::new();
    let path=format!("/c/{}/invoices/new/", f.c);
    for (name,value,error_id) in [
        ("terms","bogus","id_terms_error"),
        ("lines-0-origin_position","nonsense","origin_position"),
        ("lines-INITIAL_FORMS","nonsense","ManagementForm data"),
    ] {
        let html=render(&mut f.app,&path);
        let mut p=invoice_fields(&f,&html);
        p.retain(|(k,_)|k!=name);p.push((name.into(),value.into()));
        let before=audit_count(&mut f.app);
        let Outcome::Invalid{html}=post(&mut f.app,&path,&html,p) else {panic!("{name} accepted")};
        assert!(html.contains(error_id),"{name}: {html}");
        assert_eq!(audit_count(&mut f.app),before);
    }
    let html=render(&mut f.app,&path);
    let mut p=invoice_fields(&f,&html);
    p.retain(|(k,_)|k!="customer"&&k!="lines-0-quantity");
    p.extend(pairs(&[("customer",""),("lines-0-quantity","nonsense"),("lines-0-DELETE","false")]));
    let Outcome::Invalid{html}=post(&mut f.app,&path,&html,p) else {panic!("invalid quantity hidden")};
    assert!(html.contains("id_customer_error")&&html.contains("id_lines-0-quantity_error"));
}
fn audit_count(app:&mut App)->i64 {
    app.open_mut().unwrap().book.read(|tx|tx.query_row("SELECT count(*) FROM ledger_auditevent",[],|r|r.get(0))).unwrap()
}
fn invoice_fields(f:&Fixture,html:&str)->Vec<(String,String)> {
    pairs(&[("request_key",&field(html,"request_key")),("expected_revision","0"),("customer",&f.customer),
        ("date","2026-10-01"),("due_date","2026-10-31"),("terms",""),("tax_mode","NONE"),
        ("lines-TOTAL_FORMS","1"),("lines-INITIAL_FORMS","0"),("lines-0-description","Work"),
        ("lines-0-quantity","1"),("lines-0-rate","100.00"),("lines-0-income_account",&f.income),("lines-0-tax_auto","1")])
}
#[test]
fn invoice_non_iso_dates_and_false_delete_save_the_original_line() {
    let mut f=Fixture::new();let path=format!("/c/{}/invoices/new/",f.c);
    let html=render(&mut f.app,&path);let mut p=invoice_fields(&f,&html);
    for (k,v) in &mut p {if k=="date" {*v="10/01/2026".into();} else if k=="due_date" {*v="Oct 31, 2026".into();}}
    p.push(("lines-0-DELETE".into(),"false".into()));
    let Outcome::Redirect{..}=post(&mut f.app,&path,&html,p) else {panic!("valid date/false deletion refused")};
    let o=f.app.open_mut().unwrap();
    let actual:(String,String,i64)=o.book.read(|tx|tx.query_row("SELECT r.date,r.due_date,(SELECT count(*) FROM sales_invoiceline) FROM sales_invoicerevision r",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))).unwrap();
    assert_eq!(actual,("2026-10-01".into(),"2026-10-31".into(),1));
}
#[test]
fn sales_extra_rows_use_field_specific_has_changed_semantics() {
    for route in ["invoices","estimates","sales-receipts"] {
        let mut f=Fixture::new();let path=format!("/c/{}/{route}/new/",f.c);
        let html=render(&mut f.app,&path);let mut p=invoice_fields(&f,&html);
        p.retain(|(k,_)|k!="lines-TOTAL_FORMS");
        p.extend(pairs(&[("lines-TOTAL_FORMS","3"),("lines-1-description"," \t\n "),("lines-1-quantity","  "),
            ("lines-1-tax_auto","1"),("lines-2-tax_auto","1"),("lines-1-taxable","false"),
            ("lines-1-refresh_item","false"),("method","CASH"),("destination","UNDEPOSITED"),("action","review")]));
        let out=post(&mut f.app,&path,&html,p);
        assert!(matches!(out,Outcome::Redirect{..}|Outcome::Page{status:200,..}),"{route}: {out:?}");
    }
}
#[test]
fn receive_payment_ignores_unticked_rows_with_only_whitespace() {
    let mut f=Fixture::new();let invoice=f.posted("2026-10-01","100.00");let other=f.posted("2026-10-02","30.00");
    let path=format!("/c/{}/receipts/new/",f.c);let html=render(&mut f.app,&format!("{path}?customer={}",f.customer));
    let p=payment(&field(&html,"request_key"),&f.customer,"10.00",&[(&invoice,true,"10.00"),(&other,false," \t ")]);
    let out=post(&mut f.app,&path,&html,p);
    assert!(matches!(out,Outcome::Redirect{..}),"{out:?}");
}
#[test]
fn print_invoice_uses_stored_revision_and_only_current_open_balance_without_writes() {
    let mut f = Fixture::new();
    // 19:30 local is already the following UTC day. Entry defaults stay local.
    f.app.today=||chrono::NaiveDate::from_ymd_opt(2026,11,1).unwrap();
    f.app.now=||chrono::DateTime::parse_from_rfc3339("2026-11-02T00:30:00Z").unwrap().with_timezone(&chrono::Utc);
    let invoice = f.invoice();
    let target = format!("/c/{}/invoices/{invoice}/print/", f.c);
    let draft = render(&mut f.app,&target);
    assert!(draft.contains("Draft — not posted. No amount due."));
    assert!(!draft.contains("id=\"print-balance\""));
    f.app.run(Command::PostInvoice { company_id:f.c.clone(),invoice_id:invoice.clone(),expected_revision:1 }).unwrap();
    let before = f.journal_count();
    let open = render(&mut f.app,&target);
    assert!(open.contains("id=\"print-total\">100.00"),"{open}");
    assert!(open.contains("id=\"print-balance\">100.00"),"{open}");
    // The customer's copy carries no internal revision or printed-at lines.
    assert!(!open.contains("Revision 2 of 2") && !open.contains("Printed 2026"));
    // 2026-10-01 to 2026-11-01 is not a term people choose, so only the due date is printed.
    assert!(!open.contains("Net 31"));
    assert!(open.contains("as of 2026-11-02 UTC"),"UTC-labelled balance date must follow the print clock: {open}");
    assert_eq!((f.app.today)(),chrono::NaiveDate::from_ymd_opt(2026,11,1).unwrap());
    let old = render(&mut f.app,&format!("/c/{}/invoices/{invoice}/revisions/1/print/",f.c));
    assert!(old.contains("Earlier version"));
    assert!(old.contains("Earlier version (v1 of 2)"));
    assert!(!old.contains("id=\"print-balance\""));
    assert_eq!(f.journal_count(),before);
    let unrelated = uuid::Uuid::new_v4();
    assert!(screens::render(&mut f.app,&format!("/c/{unrelated}/invoices/{invoice}/print/")).is_err());
    assert!(screens::render(&mut f.app,&format!("/c/{}/invoices/{invoice}/revisions/99/print/",f.c)).is_err());
    assert_eq!(f.journal_count(),before);
}

#[test]
fn invoice_payment_instructions_and_email_preview_follow_live_balance_without_writes() {
    use fb_core::{company_documents as docs, invoice_preferences as prefs};
    let mut f=Fixture::new();
    let c:CompanyId=f.c.parse().unwrap();
    let o=f.app.open_mut().unwrap();
    docs::save_with_preferences(&mut o.book,o.owner,c,"Synthetic seller","USD",docs::Changes {
        details:docs::Details{payment_instructions:"Prefer a check? Contact us.".into(),..Default::default()},logo_upload:None,remove_logo:false
    },Some(prefs::Change{expected_revision:0,settings:prefs::Settings{
        zelle_enabled:true,zelle_recipient:"payments@example.test".into(),stripe_enabled:true,
        footer:"Thank you <customer> & co.".into(),email_greeting:"Hi {customer_name},\nA note <script> & text.".into(),
        ..Default::default()
    }})).unwrap();
    let invoice=f.invoice();
    let print=format!("/c/{}/invoices/{invoice}/print/",f.c);
    let email=format!("/c/{}/invoices/{invoice}/email-preview/",f.c);
    for path in [&print,&email] {
        let html=render(&mut f.app,path);
        assert!(html.contains("payments@example.test"));
        assert!(html.contains("preview"));
        assert!(!html.contains("href=\"https://buy.stripe.com"));
        assert!(!html.contains("<svg xmlns=\"http://www.w3.org/2000/svg"));
    }
    f.app.run(Command::PostInvoice{company_id:f.c.clone(),invoice_id:invoice.clone(),expected_revision:1}).unwrap();
    let before=fb_db::test_support::snapshot(f.app.open_mut().unwrap().book.connection()).unwrap();
    let html=render(&mut f.app,&email);
    assert!(html.contains("Save to Gmail Drafts") && html.contains("Hi Synthetic customer,<br>"));
    assert!(html.contains("&lt;script&gt; &amp; text.") && !html.contains("<script> &"));
    assert!(html.contains("payments@example.test") && html.contains("$100.00"));
    let printed=render(&mut f.app,&print);
    assert!(printed.contains("payments@example.test") && printed.contains("Thank you &lt;customer&gt; &amp; co."));
    let old=render(&mut f.app,&format!("/c/{}/invoices/{invoice}/revisions/1/print/",f.c));
    assert!(old.contains("Earlier version — no amount due"));
    assert!(!old.contains("payments@example.test"));
    assert_eq!(fb_db::test_support::snapshot(f.app.open_mut().unwrap().book.connection()).unwrap(),before);
    let bad=format!("/c/{}/invoices/{invoice}/email-preview/",uuid::Uuid::new_v4());
    assert!(screens::render(&mut f.app,&bad).is_err());
    // The same posted revision becomes paid through a separately reviewed receipt.
    // Both customer-facing views must use the live balance, not the revision total.
    let paid=f.invoice();
    f.app.run(Command::PostInvoice{company_id:f.c.clone(),invoice_id:paid.clone(),expected_revision:1}).unwrap();
    let payment=json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":f.customer,
        "date":"2026-10-02","amount":"100.00","method":"CHECK","destination":"UNDEPOSITED",
        "allocations":[{"invoice_id":paid,"amount":"100.00"}]});
    let o=f.app.open_mut().unwrap();
    let review=fb_core::sales::review_receipt(&mut o.book,o.owner,c,&payment).unwrap();
    fb_core::sales::record_receipt(&mut o.book,o.owner,c,&payment,Some(&review)).unwrap();
    let before=fb_db::test_support::snapshot(o.book.connection()).unwrap();
    for suffix in ["print","email-preview"] {
        let html=render(&mut f.app,&format!("/c/{}/invoices/{paid}/{suffix}/",f.c));
        assert!(!html.contains("payments@example.test"));
        assert!(!html.contains("Prefer a check?"));
        assert!(html.contains("PAID") || html.contains("Paid"));
    }
    assert_eq!(fb_db::test_support::snapshot(f.app.open_mut().unwrap().book.connection()).unwrap(),before);
    f.app.run(Command::VoidInvoice {company_id:f.c.clone(),invoice_id:invoice,expected_revision:2,date:"2026-11-02".into(),reason:"Synthetic cancellation".into()}).unwrap();
    for path in [&print,&email] {
        let html=render(&mut f.app,path);
        assert!(!html.contains("payments@example.test"));
        assert!(!html.contains("Prefer a check?"));
    }
}

#[test]
fn a_review_cannot_be_posted_by_the_next_signed_in_person_even_with_a_fresh_form() {
    use fb_app::identity::Role;
    let mut f=Fixture::new();
    let invoice=f.invoice();
    let c=CompanyId::parse(&f.c).unwrap();
    f.app.save_desktop_user(None,"Another bookkeeper",Role::Bookkeeper,true,Some("Synthetic review password"),&[c]).unwrap();
    support::sign_in(&mut f.app);
    let html=render(&mut f.app,&format!("/c/{}/invoices/{invoice}/review/?revision=1",f.c));
    let review_token=field(&html,"review_token");
    f.app.sign_out();f.app.sign_in("Another bookkeeper","Synthetic review password").unwrap();
    let post_path=format!("/c/{}/invoices/{invoice}/post/",f.c);
    // A new form token cannot revive the previous person's review capability.
    let token=f.app.tokens.mint(&post_path);
    let outcome=f.app.run(Command::SubmitForm {path:post_path,token,fields:pairs(&[("expected_revision","1"),("review_token",&review_token)])}).unwrap();
    assert!(matches!(outcome,Outcome::Invalid { html } if html.to_lowercase().contains("review")));
    let state:String=f.app.open_mut().unwrap().book.read(|q|q.query_row("SELECT state FROM sales_invoicerevision WHERE invoice_id=?1 ORDER BY number DESC LIMIT 1",[invoice],|r|r.get(0))).unwrap();
    assert_eq!(state,"DRAFT");
}

#[test]
fn invoice_editor_keeps_saved_defaults_and_archived_copies_without_visible_internal_controls() {
    let mut f = Fixture::new();
    let o = f.app.open_mut().unwrap();
    let c = f.c.parse().unwrap();
    let item = fb_core::directory::save_record(&mut o.book, o.owner, c, fb_core::directory::Model::Item,
        &json!({"code":"On-site service","description":"Original work","kind":"SERVICE","sale_rate":"129.00","income_account":f.income}), None).unwrap();
    let id = item["id"].as_str().unwrap();
    let inv = fb_core::sales::save_invoice(&mut o.book, o.owner, c,
        &json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":f.customer,"date":"2026-10-01","due_date":"2026-11-01","tax_mode":"NONE","lines":[{"item_id":id,"description":"Agreed work","quantity":"2","rate":"110","taxable":false,"income_account_id":f.income}]})).unwrap();
    fb_core::directory::save_record(&mut o.book,o.owner,c,fb_core::directory::Model::Item,
        &json!({"code":"Renamed service","description":"Changed work","kind":"SERVICE","sale_rate":"999","income_account":f.income}),Some(id)).unwrap();
    for (model, key) in [(fb_core::directory::Model::Item,id),(fb_core::directory::Model::Customer,f.customer.as_str())] {
        fb_core::directory::set_active(&mut o.book,o.owner,c,model,key,false).unwrap();
    }
    let path=format!("/c/{}/invoices/{}/edit/",f.c,inv["id"].as_str().unwrap());
    let html=render(&mut f.app,&path);
    assert!(!html.contains("Item snapshot options"));
    assert!(!html.contains("Customer snapshot options"));
    assert!(!html.contains("Income account (required to post)"));
    assert!(html.contains("Renamed service (archived)"));
    assert_eq!(field(&html,"lines-0-income_account"),f.income);
    assert_eq!(field(&html,"lines-0-keep_archived"),"1");
    assert_eq!(field(&html,"keep_archived"),"1");
    let p=pairs(&[("request_key",&field(&html,"request_key")),("expected_revision","1"),("customer",&f.customer),("keep_archived","1"),("date","2026-10-01"),("due_date","2026-11-01"),("tax_mode","NONE"),("lines-TOTAL_FORMS","1"),("lines-INITIAL_FORMS","1"),("lines-0-origin_position","1"),("lines-0-keep_archived","1"),("lines-0-item",id),("lines-0-description","Agreed work"),("lines-0-quantity","2"),("lines-0-rate","110"),("lines-0-income_account",&f.income)]);
    assert!(matches!(post(&mut f.app,&path,&html,p),Outcome::Redirect{..}));
    let o=f.app.open_mut().unwrap();
    let saved=o.book.read(|tx|fb_core::sales::invoice(tx,c,inv["id"].as_str().unwrap())).unwrap();
    assert_eq!(saved["lines"][0]["item_snapshot"],inv["lines"][0]["item_snapshot"]);
    assert_eq!(saved["current"]["customer_snapshot"],inv["current"]["customer_snapshot"]);
    assert_eq!(saved["current"]["total"],"220.00");
    assert!(fb_core::sales::review_invoice(&mut o.book,o.owner,c,inv["id"].as_str().unwrap(),Some(2)).is_err());
}

#[test]
fn save_and_post_posts_the_revision_just_saved_or_keeps_the_draft_and_says_why() {
    let mut f = Fixture::new();
    let path = format!("/c/{}/invoices/new/", f.c);
    let html = render(&mut f.app, &path);
    assert!(html.contains(">Save draft</button>") && html.contains(">Save and post</button>") && !html.contains("Save &amp; preview"));
    assert!(html.contains("data-total-bar"), "the total shows in the sticky bar");
    let mut p = invoice_fields(&f, &html);
    p.push(("editor_action".into(), "post".into()));
    let Outcome::Redirect { location } = post(&mut f.app, &path, &html, p) else { panic!() };
    assert_eq!(f.journal_count(), 1);
    let page = render(&mut f.app, &location);
    assert!(page.contains("Invoice saved and posted.") && (page.contains(">Open<") || page.contains(">Overdue<")));
    // With no receivables account the save stands, nothing posts, and the page says why.
    let o = f.app.open_mut().unwrap();
    fb_core::documents::save_setup(&mut o.book, o.owner, f.c.parse().unwrap(), &json!({"undeposited":f.uf,"banks":[f.bank]}), Some(1)).unwrap();
    let html = render(&mut f.app, &path);
    let mut p = invoice_fields(&f, &html);
    p.push(("editor_action".into(), "post".into()));
    let Outcome::Redirect { location } = post(&mut f.app, &path, &html, p) else { panic!() };
    assert_eq!(f.journal_count(), 1);
    let page = render(&mut f.app, &location);
    assert!(page.contains("Saved as a draft. Not posted."), "{page}");
    assert!(page.contains("status-chip draft\">Draft<"));
}

#[test]
fn customer_page_leads_with_actions_balances_and_one_filtered_list() {
    let mut f = Fixture::new();
    let older = f.posted("2026-09-01", "100.00");
    f.posted("2026-10-15", "80.00");
    let path = format!("/c/{}/receipts/new/", f.c);
    let html = render(&mut f.app, &format!("{path}?invoice={older}"));
    let p = payment(&field(&html, "request_key"), &f.customer, "40.00", &[(&older, true, "40.00")]);
    assert!(matches!(post(&mut f.app, &path, &html, p), Outcome::Redirect { .. }));
    let page = format!("/c/{}/lists/customers/{}/", f.c, f.customer);
    let html = render(&mut f.app, &page);
    for action in [format!("/invoices/new/?customer={}\">New invoice</a>", f.customer), format!("/receipts/new/?customer={}\">Receive payment</a>", f.customer), format!("/estimates/new/?customer={}\">New estimate</a>", f.customer)] {
        assert!(html.contains(&action), "{action}");
    }
    assert!(html.contains("<strong id=\"customer-open\">140.00</strong>"), "{html}");
    assert!(html.contains("<strong id=\"customer-credits\">0.00</strong>") && html.contains("<strong id=\"customer-deposits\">0.00</strong>"));
    assert!(!html.contains("FreedomBooks documents only") && !html.contains("No recorded sales receipts."));
    assert_eq!(html.matches("<table").count(), 1, "one combined list");
    let table = html.split("id=\"customer-activity\"").nth(1).unwrap();
    assert!(table.contains("<td>Invoice</td>") && table.contains("<td>Payment</td>") && table.contains("INV-000002"));
    let payments = render(&mut f.app, &format!("{page}?type=payments"));
    let table = payments.split("id=\"customer-activity\"").nth(1).unwrap();
    assert!(table.contains("<td>Payment</td>") && !table.contains("<td>Invoice</td>"));
    // New invoice and New estimate open with the customer chosen.
    for new in ["invoices", "estimates"] {
        let html = render(&mut f.app, &format!("/c/{}/{new}/new/?customer={}", f.c, f.customer));
        assert!(html.contains(&format!("<option value=\"{}\" selected>", f.customer)), "{new}");
    }
    // The invoice list offers Receive payment for open invoices.
    let list = render(&mut f.app, &format!("/c/{}/invoices/", f.c));
    assert!(list.contains(&format!("/receipts/new/?invoice={older}\">Receive payment</a>")));
}

#[test]
fn new_invoices_default_to_net_30_and_net_60_is_offered() {
    let mut f = Fixture::new();
    let html = render(&mut f.app, &format!("/c/{}/invoices/new/", f.c));
    assert!(html.contains("<option value=\"30\" selected>Net 30</option>"), "{html}");
    assert!(html.contains("<option value=\"60\">Net 60</option>") && !html.contains("Custom due date"));
    let path = format!("/c/{}/invoices/new/", f.c);
    let mut p = invoice_fields(&f, &html);
    for (k, v) in &mut p { match k.as_str() { "terms" => *v = "60".into(), "due_date" => *v = "2026-11-30".into(), _ => {} } }
    let Outcome::Redirect { location } = post(&mut f.app, &path, &html, p) else { panic!("Net 60 accepted") };
    let page = render(&mut f.app, &location);
    assert!(page.contains("<dt>Terms</dt><dd>Net 60</dd>"), "{page}");
    let edit = render(&mut f.app, &format!("{location}edit/"));
    assert!(edit.contains("<option value=\"60\" selected>Net 60</option>"));
}

#[test]
fn invoice_email_greets_the_contact_shows_the_due_date_and_offers_to_connect_gmail() {
    let mut f = Fixture::new();
    let o = f.app.open_mut().unwrap();
    fb_core::directory::save_record(&mut o.book, o.owner, f.c.parse().unwrap(), fb_core::directory::Model::Customer, &json!({"display_name":"Synthetic customer","contact_name":"Casey Sample"}), Some(&f.customer)).unwrap();
    let invoice = f.posted("2026-10-01", "100.00");
    f.app.invoice_delivery = true;
    let html = render(&mut f.app, &format!("/c/{}/invoices/{invoice}/email-preview/", f.c));
    assert!(html.contains("Hi Casey Sample,<br>"), "{html}");
    assert!(html.contains("Due Nov 1, 2026"));
    assert!(html.contains("#google-connection\">Connect Gmail</a>"));
    assert!(!html.contains("Connect Google in company settings"));
}

/// A two-line cash sale as the sales receipt form sends it.
fn sale_fields(f: &Fixture, key: &str) -> Vec<(String, String)> {
    pairs(&[
        ("request_key", key), ("customer", &f.customer), ("date", "2026-10-05"), ("method", "CASH"),
        ("reference", "R1"), ("memo", "Counter sale"), ("destination", "UNDEPOSITED"), ("bank", ""),
        ("tax_mode", "NONE"), ("lines-TOTAL_FORMS", "2"), ("lines-INITIAL_FORMS", "0"),
        ("lines-0-description", "Mulch"), ("lines-0-quantity", "2"), ("lines-0-rate", "15.00"),
        ("lines-0-income_account", &f.income), ("lines-0-tax_auto", "1"),
        ("lines-1-description", "Delivery"), ("lines-1-quantity", "1"), ("lines-1-rate", "5.00"),
        ("lines-1-income_account", &f.income), ("lines-1-tax_auto", "1"),
        ("editor_action", "save"),
    ])
}
fn sales_receipt_count(app: &mut App) -> i64 {
    app.open_mut().unwrap().book.read(|tx| tx.query_row("SELECT count(*) FROM sales_receipt WHERE kind='SR'", [], |r| r.get(0))).unwrap()
}
fn deactivate_income(f: &mut Fixture) {
    f.app.run(fb_app::commands::parse_command(&json!({"type":"SaveAccount","company_id":f.c,"account_id":f.income,"code":"4000","name":"4000","detail_type":"INCOME","active":false}).to_string()).unwrap()).unwrap();
}

#[test]
fn sales_receipt_uses_the_invoice_editor_and_saves_in_one_step() {
    let mut f = Fixture::new();
    let path = format!("/c/{}/sales-receipts/new/", f.c);
    let html = render(&mut f.app, &path);
    assert_eq!(html.matches("<fieldset class=\"invoice-row\"><legend>Line").count(), 1);
    for part in ["invoice-tax-rates", "posting-accounts", "Show accounting entries", "data-total-bar", "name=\"editor_action\" value=\"save\">Save</button>", "data-bank-field hidden", ">Bank account:</label>"] {
        assert!(html.contains(part), "{part}");
    }
    for gone in ["RECORD ONLY", "Synthetic data", "No credentials", "books only", "review_token", "Designated bank", "Every line needs an explicit income account"] {
        assert!(!html.contains(gone), "{gone}");
    }
    let journals = f.journal_count();
    // A bank left in the hidden field is not part of a sale to undeposited funds.
    let mut p = sale_fields(&f, &field(&html, "request_key"));
    for (k, v) in &mut p { if k == "bank" { *v = f.bank.clone(); } }
    let Outcome::Redirect { location } = post(&mut f.app, &path, &html, p) else { panic!("not saved") };
    assert!(location.contains("/sales-receipts/"));
    assert_eq!((sales_receipt_count(&mut f.app), f.journal_count()), (1, journals + 1));
    let page = render(&mut f.app, &location);
    assert!(page.contains("class=\"status-chip paid\">Paid</span>"));
    assert!(page.contains(">Print or PDF</a>") && page.contains("class=\"button danger\""));
    assert!(page.contains("Show accounting entries") && page.contains("35.00"));
    for gone in ["RECORD ONLY", "no funds moved", "books only"] {
        assert!(!page.contains(gone), "{gone}");
    }
}

#[test]
fn sales_receipt_refusals_keep_what_was_entered_and_write_nothing() {
    let mut f = Fixture::new();
    let path = format!("/c/{}/sales-receipts/new/", f.c);
    let before = (audit_count(&mut f.app), f.journal_count());
    // A description-only line must name its income account.
    let html = render(&mut f.app, &path);
    let mut p = sale_fields(&f, &field(&html, "request_key"));
    p.retain(|(k, _)| k != "lines-1-income_account");
    let Outcome::Invalid { html } = post(&mut f.app, &path, &html, p) else { panic!() };
    assert!(html.contains("Choose the income account for this line."));
    assert!(html.contains(">\nDelivery</textarea>") && html.contains("value=\"15.00\""));
    // The core refuses an inactive income account, in plain words, keeping every entry.
    deactivate_income(&mut f);
    let html = render(&mut f.app, &path);
    let p = sale_fields(&f, &field(&html, "request_key"));
    let Outcome::Invalid { html } = post(&mut f.app, &path, &html, p) else { panic!() };
    assert!(html.contains("Sales receipt not saved."), "{html}");
    assert!(html.contains("One of the accounts this would post to is inactive."), "{html}");
    assert!(html.contains(">\nMulch</textarea>") && html.contains("Counter sale"));
    assert_eq!(sales_receipt_count(&mut f.app), 0);
    // Deactivating the account is the only change written.
    assert_eq!(f.journal_count(), before.1);
}

#[test]
fn sales_receipt_saved_twice_records_once_and_shows_the_first() {
    let mut f = Fixture::new();
    let path = format!("/c/{}/sales-receipts/new/", f.c);
    let html = render(&mut f.app, &path);
    let p = sale_fields(&f, &field(&html, "request_key"));
    let Outcome::Redirect { location: first } = post(&mut f.app, &path, &html, p.clone()) else { panic!() };
    let journals = f.journal_count();
    let html = render(&mut f.app, &path);
    let Outcome::Redirect { location } = post(&mut f.app, &path, &html, p) else { panic!() };
    assert_eq!(location, first);
    assert_eq!((sales_receipt_count(&mut f.app), f.journal_count()), (1, journals));
    assert!(render(&mut f.app, &location).contains("This sales receipt was already saved as SR-"));
}

#[test]
fn a_sales_receipt_line_with_an_item_uses_the_items_income_account() {
    let mut f = Fixture::new();
    let o = f.app.open_mut().unwrap();
    let item = fb_core::directory::save_record(&mut o.book, o.owner, f.c.parse().unwrap(), fb_core::directory::Model::Item, &json!({"code":"MULCH","description":"Mulch","kind":"NONSTOCK","sale_rate":"15.00","income_account":f.income}), None).unwrap()["id"].as_str().unwrap().to_owned();
    let path = format!("/c/{}/sales-receipts/new/", f.c);
    let html = render(&mut f.app, &path);
    let mut p = sale_fields(&f, &field(&html, "request_key"));
    p.retain(|(k, _)| k != "lines-0-income_account");
    p.push(("lines-0-item".into(), item));
    let Outcome::Redirect { .. } = post(&mut f.app, &path, &html, p) else { panic!("item line refused") };
    let accounts: Vec<String> = f.app.open_mut().unwrap().book.read(|tx| {
        let mut st = tx.prepare("SELECT json_extract(data,'$.income_account_id') FROM sales_salesreceiptline ORDER BY position")?;
        st.query_map([], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()
    }).unwrap();
    assert_eq!(accounts, vec![f.income.clone(), f.income.clone()]);
}

fn estimate_fields(f: &Fixture, key: &str) -> Vec<(String, String)> {
    pairs(&[
        ("request_key", key), ("expected_revision", "0"), ("customer", &f.customer), ("date", "2026-10-01"),
        ("expiry", "2026-10-31"), ("memo", "Spring cleanup"), ("tax_mode", "NONE"),
        ("lines-TOTAL_FORMS", "3"), ("lines-INITIAL_FORMS", "0"),
        ("lines-0-description", "Agreed work"), ("lines-0-quantity", "1"), ("lines-0-rate", "10.00"),
        ("lines-1-description", "Hauling"), ("lines-1-quantity", "2"), ("lines-1-rate", "20.00"),
        ("lines-2-description", "Edging"), ("lines-2-quantity", "3"), ("lines-2-rate", "5.00"),
        ("editor_action", "save"),
    ])
}
fn estimate_count(app: &mut App) -> i64 {
    app.open_mut().unwrap().book.read(|tx| tx.query_row("SELECT count(*) FROM sales_estimate", [], |r| r.get(0))).unwrap()
}

#[test]
fn new_estimate_uses_the_invoice_editor_with_live_tax_and_a_30_day_expiry() {
    let mut f = Fixture::new();
    let html = render(&mut f.app, &format!("/c/{}/estimates/new/", f.c));
    assert_eq!(html.matches("<fieldset class=\"invoice-row\"><legend>Line").count(), 1);
    let expiry = ((f.app.today)() + chrono::Duration::days(30)).to_string();
    assert!(html.contains(&format!("name=\"expiry\" value=\"{expiry}\"")), "{html}");
    for part in ["invoice-tax-rates", "invoice-customers", "data-total-bar", "data-tax", "name=\"editor_action\" value=\"save\">Save</button>"] {
        assert!(html.contains(part), "{part}");
    }
    for gone in ["Customer snapshot options", "Update item details", "Keep archived", "Save &amp; preview calculates", "estimate-line"] {
        assert!(!html.contains(gone), "{gone}");
    }
}

#[test]
fn estimate_save_refusal_and_double_submit() {
    let mut f = Fixture::new();
    let path = format!("/c/{}/estimates/new/", f.c);
    // Refused by the core: entered values come back and nothing is written.
    let html = render(&mut f.app, &path);
    let mut p = estimate_fields(&f, &field(&html, "request_key"));
    for (k, v) in &mut p { if k == "expiry" { *v = "2026-09-01".into(); } }
    let Outcome::Invalid { html } = post(&mut f.app, &path, &html, p) else { panic!() };
    assert!(html.contains("Expiry cannot precede the estimate date."), "{html}");
    assert!(html.contains(">\nHauling</textarea>") && html.contains("Spring cleanup"));
    assert_eq!(estimate_count(&mut f.app), 0);
    // A normal save of three lines, then the same form again: one estimate.
    let html = render(&mut f.app, &path);
    let p = estimate_fields(&f, &field(&html, "request_key"));
    let Outcome::Redirect { location: first } = post(&mut f.app, &path, &html, p.clone()) else { panic!() };
    let html = render(&mut f.app, &path);
    let Outcome::Redirect { location } = post(&mut f.app, &path, &html, p) else { panic!() };
    assert_eq!((location, estimate_count(&mut f.app)), (first.clone(), 1));
    let page = render(&mut f.app, &first);
    assert!(page.contains("class=\"status-chip draft\">Draft</span>") && page.contains("65.00"));
    assert!(page.contains(">Edit</a>") && page.contains("Mark as accepted"));
    assert!(!page.contains("Non-posting estimate") && !page.contains("for your reference only"));
}

#[test]
fn an_accepted_estimate_leads_with_create_invoice() {
    let mut f = Fixture::new();
    let path = format!("/c/{}/estimates/new/", f.c);
    let html = render(&mut f.app, &path);
    let p = estimate_fields(&f, &field(&html, "request_key"));
    let Outcome::Redirect { location } = post(&mut f.app, &path, &html, p) else { panic!() };
    let id = location.trim_end_matches('/').rsplit('/').next().unwrap().to_owned();
    f.app.run(Command::TransitionEstimate { company_id: f.c.clone(), estimate_id: id, expected_revision: 1, state: "ACCEPTED".into(), reason: String::new() }).unwrap();
    let page = render(&mut f.app, &location);
    let bar = page.split("class=\"action-bar\"").nth(1).unwrap();
    assert!(bar.trim_start_matches(|c| c != '<').starts_with("<a class=\"button\" href=\"") && bar.contains(">Create invoice from this estimate</a>"), "{bar}");
    let list = render(&mut f.app, &format!("/c/{}/estimates/", f.c));
    assert!(list.contains(">Create invoice</a>"));
}

#[test]
fn estimate_lines_offer_item_choices_only_when_the_item_is_archived_or_changed() {
    let mut f = Fixture::new();
    let c: CompanyId = f.c.parse().unwrap();
    let o = f.app.open_mut().unwrap();
    let mut item = |code: &str| fb_core::directory::save_record(&mut o.book, o.owner, c, fb_core::directory::Model::Item, &json!({"code":code,"description":code,"kind":"SERVICE","sale_rate":"10.00","income_account":f.income}), None).unwrap()["id"].as_str().unwrap().to_owned();
    let (kept, archived, changed) = (item("KEPT"), item("GONE"), item("NEWER"));
    let path = format!("/c/{}/estimates/new/", f.c);
    let html = render(&mut f.app, &path);
    let mut p = estimate_fields(&f, &field(&html, "request_key"));
    p.extend(pairs(&[("lines-0-item", &kept), ("lines-1-item", &archived), ("lines-2-item", &changed)]));
    let Outcome::Redirect { location } = post(&mut f.app, &path, &html, p) else { panic!() };
    let o = f.app.open_mut().unwrap();
    fb_core::directory::set_active(&mut o.book, o.owner, c, fb_core::directory::Model::Item, &archived, false).unwrap();
    fb_core::directory::save_record(&mut o.book, o.owner, c, fb_core::directory::Model::Item, &json!({"code":"NEWER","description":"Newer wording","kind":"SERVICE","sale_rate":"12.00","income_account":f.income}), Some(&changed)).unwrap();
    let html = render(&mut f.app, &format!("{location}edit/"));
    let line = |n: usize| html.split("<fieldset class=\"invoice-row\">").nth(n + 1).unwrap().split("</fieldset>").next().unwrap().to_owned();
    assert!(!line(0).contains("type=\"checkbox\" name=\"lines-0-keep_archived\"") && !line(0).contains("type=\"checkbox\" name=\"lines-0-refresh_item\""));
    assert!(line(1).contains("Keep this archived item on the line") && !line(1).contains("current details"));
    assert!(line(2).contains("Use the item&#x27;s current details from the item list") || line(2).contains("Use the item's current details from the item list"), "{}", line(2));
    assert!(!line(2).contains("Keep this archived item"));
}

#[test]
fn customer_deposit_page_explains_the_missing_setup_instead_of_a_form() {
    let mut f = Fixture::new();
    let html = render(&mut f.app, &format!("/c/{}/customer-deposits/new/", f.c));
    assert!(html.contains("Choose where deposits are held first") && html.contains("Open Accounting setup"));
    assert!(!html.contains("id=\"deposit-entry\"") && !html.contains("RECORD ONLY"));
}


#[test]
fn stale_new_invoice_replay_never_posts_a_newer_draft() {
    let mut f = Fixture::new();
    let path = format!("/c/{}/invoices/new/", f.c);
    let html = render(&mut f.app, &path);
    let original = invoice_fields(&f, &html);
    let Outcome::Redirect { location } = post(&mut f.app, &path, &html, original.clone()) else { panic!() };
    let id = location.trim_end_matches('/').rsplit('/').next().unwrap();
    let o = f.app.open_mut().unwrap();
    fb_core::sales::save_invoice(&mut o.book, o.owner, f.c.parse().unwrap(), &json!({
        "invoice_id":id,"expected_revision":1,"customer_id":f.customer,"date":"2026-10-02","due_date":"2026-10-31","tax_mode":"NONE",
        "lines":[{"description":"Changed work","quantity":"1","rate":"999.00","taxable":false,"income_account_id":f.income}]
    })).unwrap();
    let token = f.app.tokens.mint(&path);
    let mut fields = original;
    fields.push(("editor_action".into(), "post".into()));
    let Outcome::Redirect { location } = f.app.run(Command::SubmitForm {path, token, fields}).unwrap() else { panic!() };
    let page = render(&mut f.app, &location);
    assert!(page.contains("this draft was changed after that page was shown"), "{page}");
    assert_eq!(f.journal_count(), 0);
    let o = f.app.open_mut().unwrap();
    let current = o.book.read(|q| fb_core::sales::invoice(q, f.c.parse().unwrap(), id)).unwrap();
    assert_eq!(current["current"]["state"], "DRAFT");
    assert_eq!(current["current"]["total"], "999.00");
}
