//! Sales forms exercise real encrypted books and opaque review capabilities.
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
        let values=serde_json::from_value(json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":self.customer,"date":"2026-10-01","due_date":"2026-11-01","tax_mode":"NONE","lines":[{"description":"Work","quantity":"1","rate":"100.00","income_account_id":self.income}]})).unwrap();
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

#[test]
fn invoice_post_uses_the_displayed_plan_and_refuses_changed_setup() {
    let mut f = Fixture::new();
    let invoice = f.invoice();
    let review_path = format!("/c/{}/invoices/{invoice}/review/?revision=1", f.c);
    let post_path = format!("/c/{}/invoices/{invoice}/post/", f.c);
    let html = render(&mut f.app, &review_path);
    let capability = field(&html, "review_token");
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
    let outcome = post(
        &mut f.app,
        &post_path,
        &html,
        pairs(&[("expected_revision", "1"), ("review_token", &capability)]),
    );
    let Outcome::Invalid { html } = outcome else {
        panic!("expected a changed-plan refusal")
    };
    assert!(html.contains("Posting mapping changed. Open a new review before posting."));
    assert_eq!(f.journal_count(), 0);
    let html = render(&mut f.app, &review_path);
    let capability = field(&html, "review_token");
    assert!(matches!(
        post(
            &mut f.app,
            &post_path,
            &html,
            pairs(&[("expected_revision", "1"), ("review_token", &capability)])
        ),
        Outcome::Redirect { .. }
    ));
    assert_eq!(f.journal_count(), 1);
    let detail = render(&mut f.app, &format!("/c/{}/invoices/{invoice}/", f.c));
    assert!(detail.contains("100.00"));
    assert!(detail.contains("Current payment state: Open"));
}

#[test]
fn customer_payment_review_record_reverse_and_changed_review_are_real_forms() {
    let mut f = Fixture::new();
    let invoice = f.invoice();
    f.app
        .run(Command::PostInvoice {
            company_id: f.c.clone(),
            invoice_id: invoice.clone(),
            expected_revision: 1,
        })
        .unwrap();
    let path = format!("/c/{}/receipts/new/", f.c);
    let html = render(&mut f.app, &format!("{path}?customer={}", f.customer));
    let key = field(&html, "request_key");
    let mut p = pairs(&[
        ("request_key", &key),
        ("customer", &f.customer),
        ("date", "2026-10-02"),
        ("amount", "20.00"),
        ("method", "CASH"),
        ("destination", "UNDEPOSITED"),
        ("allocations-TOTAL_FORMS", "3"),
        ("allocations-INITIAL_FORMS", "0"),
        ("allocations-0-invoice", &invoice),
        ("allocations-0-amount", "20.00"),
        ("action", "review"),
    ]);
    let Outcome::Page { html, status } = post(&mut f.app, &path, &html, p.clone()) else {
        panic!("review failed")
    };
    assert_eq!(status, 200);
    assert!(html.contains("Review before recording payment"));
    let cap = field(&html, "review_token");
    p.retain(|(k, _)| k != "action");
    p.push(("action".into(), "record".into()));
    p.push(("review_token".into(), cap));
    let original = p.clone();
    for (k, v) in &mut p {
        if k == "amount" || k == "allocations-0-amount" {
            *v = "30.00".into();
        }
    }
    let Outcome::Invalid { html } = post(&mut f.app, &path, &html, p) else {
        panic!("changed plan must fail")
    };
    assert!(html.contains("Review"));
    assert_eq!(f.journal_count(), 1);
    // A failed recording attempt consumes its review capability too.
    let Outcome::Invalid { html } = post(&mut f.app, &path, &html, original.clone()) else {
        panic!("consumed review must fail")
    };
    assert!(html.contains("review"));
    assert_eq!(f.journal_count(), 1);
    let mut fresh = original;
    fresh.retain(|(k, _)| k != "action" && k != "review_token");
    fresh.push(("action".into(), "review".into()));
    let Outcome::Page { html, .. } = post(&mut f.app, &path, &html, fresh.clone()) else { panic!("fresh review failed") };
    fresh.retain(|(k, _)| k != "action");
    fresh.extend(pairs(&[("action", "record"), ("review_token", &field(&html, "review_token"))]));
    let Outcome::Redirect { location } = post(&mut f.app, &path, &html, fresh) else { panic!("record failed") };
    assert_eq!(f.journal_count(), 2);
    let reverse_path = format!("{location}reverse/");
    let html = render(&mut f.app, &reverse_path);
    let p = pairs(&[
        ("expected_revision", "1"),
        ("date", "2026-10-03"),
        ("reason", "Synthetic correction"),
    ]);
    let Outcome::Redirect { location } = post(&mut f.app, &reverse_path, &html, p) else {
        panic!()
    };
    let detail = render(&mut f.app, &location);
    assert!(detail.contains("Reversed"));
    assert!(detail.contains("Synthetic correction"));
    assert_eq!(f.journal_count(), 3);
    let o = f.app.open_mut().unwrap();
    let balances =
        fb_core::sales::invoice_balances(&mut o.book, o.owner, f.c.parse().unwrap()).unwrap();
    assert_eq!(balances[0]["outstanding"], "100.00");
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
fn payment_retry_retains_original_bank_and_allocations_after_reversal_void_and_archive() {
    let mut f = Fixture::new();
    let invoice = f.invoice();
    f.app
        .run(Command::PostInvoice {
            company_id: f.c.clone(),
            invoice_id: invoice.clone(),
            expected_revision: 1,
        })
        .unwrap();
    let path = format!("/c/{}/receipts/new/", f.c);
    let html = render(&mut f.app, &format!("{path}?customer={}", f.customer));
    let key = field(&html, "request_key");
    let mut p = pairs(&[
        ("request_key", &key),
        ("customer", &f.customer),
        ("date", "2026-10-02"),
        ("amount", "20.00"),
        ("method", "CHECK"),
        ("destination", "BANK"),
        ("bank", &f.bank),
        ("allocations-TOTAL_FORMS", "1"),
        ("allocations-INITIAL_FORMS", "0"),
        ("allocations-0-invoice", &invoice),
        ("allocations-0-amount", "20.00"),
        ("action", "review"),
    ]);
    let Outcome::Page { html, .. } = post(&mut f.app, &path, &html, p.clone()) else {
        panic!()
    };
    assert!(
        html.split("class=\"receipt-review\"")
            .nth(1)
            .unwrap()
            .contains("<td>INV-000001</td>")
    );
    let capability = field(&html, "review_token");
    p.retain(|(k, _)| k != "action");
    p.extend(pairs(&[
        ("action", "record"),
        ("review_token", &capability),
    ]));
    let Outcome::Redirect { location } = post(&mut f.app, &path, &html, p.clone()) else {
        panic!()
    };
    let id = location.trim_end_matches('/').rsplit('/').next().unwrap();
    f.app
        .run(Command::ReverseCustomerReceipt {
            company_id: f.c.clone(),
            receipt_id: id.into(),
            sales_receipt: false,
            expected_revision: 1,
            date: "2026-10-03".into(),
            reason: "Correction".into(),
        })
        .unwrap();
    f.app
        .run(Command::VoidInvoice {
            company_id: f.c.clone(),
            invoice_id: invoice,
            expected_revision: 2,
            date: "2026-10-04".into(),
            reason: "Correction".into(),
        })
        .unwrap();
    let o = f.app.open_mut().unwrap();
    fb_core::directory::set_active(
        &mut o.book,
        o.owner,
        f.c.parse().unwrap(),
        fb_core::directory::Model::Customer,
        &f.customer,
        false,
    )
    .unwrap();
    fb_core::documents::save_setup(
        &mut o.book,
        o.owner,
        f.c.parse().unwrap(),
        &json!({"receivables":f.ar,"undeposited":f.uf,"banks":[]}),
        Some(1),
    )
    .unwrap();
    let before: i64 = o
        .book
        .read(|tx| tx.query_row("SELECT count(*) FROM ledger_auditevent", [], |r| r.get(0)))
        .unwrap();
    let html = render(&mut f.app, &path);
    let Outcome::Invalid { html } = post(&mut f.app, &path, &html, p) else {
        panic!("a consumed review cannot retry a reversed payment")
    };
    assert!(html.contains("review"));
    let after: i64 = f
        .app
        .open_mut()
        .unwrap()
        .book
        .read(|tx| tx.query_row("SELECT count(*) FROM ledger_auditevent", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(f.journal_count(), 4);
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
fn customer_receipt_ignores_whitespace_only_extra_allocation_amount() {
    let mut f=Fixture::new();let invoice=f.invoice();
    f.app.run(Command::PostInvoice{company_id:f.c.clone(),invoice_id:invoice.clone(),expected_revision:1}).unwrap();
    let path=format!("/c/{}/receipts/new/",f.c);let html=render(&mut f.app,&format!("{path}?customer={}",f.customer));
    let p=pairs(&[("request_key",&field(&html,"request_key")),("customer",&f.customer),("date","2026-10-02"),
        ("amount","10.00"),("method","CASH"),("destination","UNDEPOSITED"),("action","review"),
        ("allocations-TOTAL_FORMS","3"),("allocations-INITIAL_FORMS","0"),("allocations-0-invoice",&invoice),
        ("allocations-0-amount","10.00"),("allocations-1-amount"," \t "),("allocations-1-DELETE","false")]);
    let out=post(&mut f.app,&path,&html,p);
    assert!(matches!(out,Outcome::Page{status:200,..}),"{out:?}");
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
    assert!(open.contains("Revision 2 of 2"));
    assert!(open.contains("Net 31"));
    assert!(open.contains("Printed 2026-11-02 00:30 UTC"));
    assert!(open.contains("as of 2026-11-02 UTC"),"UTC-labelled balance date must follow the print clock: {open}");
    assert_eq!((f.app.today)(),chrono::NaiveDate::from_ymd_opt(2026,11,1).unwrap());
    let old = render(&mut f.app,&format!("/c/{}/invoices/{invoice}/revisions/1/print/",f.c));
    assert!(old.contains("Earlier version"));
    assert!(old.contains("Revision 1 of 2"));
    assert!(!old.contains("id=\"print-balance\""));
    assert_eq!(f.journal_count(),before);
    let unrelated = uuid::Uuid::new_v4();
    assert!(screens::render(&mut f.app,&format!("/c/{unrelated}/invoices/{invoice}/print/")).is_err());
    assert!(screens::render(&mut f.app,&format!("/c/{}/invoices/{invoice}/revisions/99/print/",f.c)).is_err());
    assert_eq!(f.journal_count(),before);
}
