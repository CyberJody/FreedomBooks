//! Sales tax pages against a real encrypted book: What you owe, Record payment (review then record),
//! the payment's page, Void, the bank register link, and who may record a payment.
mod support;
use fb_app::{App, Command, Outcome, identity::Role, keys, screens};
use fb_core::{CompanyId, UserId, directory, documents, sales, salestax, services};
use serde_json::{Value, json};

const OWNER: &str = "owner";
struct Fx {
    _dir: tempfile::TempDir,
    app: App,
    c: String,
    co: CompanyId,
    bank: String,
    agency: String,
}
fn field(html: &str, name: &str) -> String {
    html.split_once(&format!("name=\"{name}\" value=\"")).unwrap_or_else(|| panic!("no {name}")).1.split('"').next().unwrap().into()
}
fn submit(app: &mut App, path: &str, values: Vec<(&str, String)>) -> Outcome {
    let html = screens::render(app, path).unwrap().html;
    app.run(Command::SubmitForm { path: path.into(), token: field(&html, "fb_command_token"), fields: values.into_iter().map(|(k, v)| (k.into(), v)).collect() }).unwrap()
}
fn count(app: &mut App, sql: &str) -> i64 {
    app.open_mut().unwrap().book.read(|q| q.query_row(sql, [], |r| r.get(0))).unwrap()
}
fn fixture() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new();
    app.today = || chrono::NaiveDate::from_ymd_opt(2026, 11, 11).unwrap();
    let co = app.create_book(&dir.path().join("book.fbbook"), &keys::new_master_key().unwrap(), OWNER, "Synthetic tax books").unwrap().company_id;
    support::sign_in(&mut app);
    let o = app.open_mut().unwrap();
    let (actor, book) = (o.owner, &mut o.book);
    let acct = |book: &mut fb_core::Book, code: &str, kind: &str| services::save_account(book, actor, co, services::AccountInput::new(code, &format!("Account {code}")).kind(kind)).unwrap().id;
    let (income, ar, bank, tax) = (acct(book, "4000", "INCOME"), acct(book, "1100", "ASSET"), acct(book, "1000", "ASSET"), acct(book, "2100", "LIABILITY"));
    let setup = documents::setup_snapshot(book.connection(), co).unwrap();
    documents::save_setup(book, actor, co, &json!({"receivables":ar.to_db(),"collected_tax":tax.to_db(),"banks":[bank.to_db()]}), setup["revision"].as_i64()).unwrap();
    let rate = salestax::save_rate(book, actor, co, &json!({"name":"State","description":"State tax","rate":"6","new_agency_name":"State Revenue","expected_revision":0})).unwrap();
    let settings = salestax::settings_for(book.connection(), co).unwrap();
    salestax::save_settings(book, actor, co, &json!({"charge_tax":true,"default_item_id":null,"owed_basis":"ACCRUAL","filing_frequency":"QUARTERLY","expected_revision":settings["revision"]})).unwrap();
    let customer = directory::save_record(book, actor, co, directory::Model::Customer, &json!({"display_name":"Synthetic customer"}), None).unwrap()["id"].as_str().unwrap().to_owned();
    for (date, amount) in [("2026-08-04", "100.00"), ("2026-09-12", "250.00")] {
        let inv = sales::save_invoice(book, actor, co, &json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":customer,"date":date,"memo":"","tax_mode":"ITEM","tax_item_id":rate["id"],"lines":[{"item_id":null,"description":"Work","quantity":"1","rate":amount,"taxable":true,"income_account_id":income.to_db()}]})).unwrap();
        let key = inv["id"].as_str().unwrap().to_owned();
        let review = sales::review_invoice(book, actor, co, &key, Some(1)).unwrap();
        sales::post_invoice(book, actor, co, &key, Some(1), Some(&review)).unwrap();
    }
    Fx { _dir: dir, app, c: co.to_db(), co, bank: bank.to_db(), agency: rate["agency_id"].as_str().unwrap().to_owned() }
}
fn values(agency: &str, bank: &str, request_key: &str, amount: &str) -> Vec<(&'static str, String)> {
    vec![("request_key", request_key.into()), ("agency", agency.into()), ("date", "2026-10-20".into()), ("bank", bank.into()), ("tax_amount", amount.into()),
        ("period_start", "2026-07-01".into()), ("period_end", "2026-09-30".into()), ("reference", "Q3-2026".into()), ("penalty_amount", "".into()), ("expense_account", "".into()), ("memo", "".into())]
}

#[test]
fn owe_record_review_detail_register_and_void_through_the_screens() {
    let mut f = fixture();
    let (agency, bank) = (f.agency.clone(), f.bank.clone());
    let owed = format!("/c/{}/sales-tax/owed/", f.c);
    let page = screens::render(&mut f.app, &owed).unwrap();
    assert_eq!(page.status, 200);
    assert!(page.html.contains("value=\"2026-10-01\"") && page.html.contains("value=\"2026-12-31\""));
    let reports = screens::render(&mut f.app, &format!("/c/{}/reports/", f.c)).unwrap().html;
    assert!(reports.contains(&owed) && reports.contains("What you owe"));
    // Read the prior quarter explicitly. Six percent of 350.00 is owed to the state.
    let page = screens::render(&mut f.app, &format!("{owed}?start=2026-07-01&end=2026-09-30")).unwrap();
    assert!(page.html.contains("value=\"2026-07-01\"") && page.html.contains("value=\"2026-09-30\""), "{}", page.html);
    assert!(page.html.contains("State Revenue") && page.html.contains("21.00"));
    // Worker item 29: the list follows the dates shown.
    assert!(page.html.contains("Payments made from 2026-07-01 to 2026-09-30") && page.html.contains("No payments in these dates."));
    let pay_link = format!("/c/{}/sales-tax/payments/new/?agency={}&amp;start=2026-07-01&amp;end=2026-09-30&amp;amount=21.00", f.c, f.agency);
    assert!(page.html.contains(&pay_link), "Record payment is prefilled with the agency, period and balance");

    // The form opens prefilled from that link.
    let new = format!("/c/{}/sales-tax/payments/new/", f.c);
    let form = screens::render(&mut f.app, &format!("{new}?agency={}&start=2026-07-01&end=2026-09-30&amount=21.00", f.agency)).unwrap().html;
    assert!(form.contains("value=\"21.00\""));
    assert!(form.contains(&format!("<option value=\"{}\" selected", f.agency)), "agency preselected");
    for label in ["Tax agency", "Payment date", "Paid from", "Sales tax paid", "Period covered: from", "Reference", "Penalty and interest"] {
        assert!(form.contains(&format!(">{label}")), "labelled field {label}");
    }

    // Review first: nothing is written, and the review shows what will post.
    let key = field(&form, "request_key");
    let before = count(&mut f.app, "SELECT count(*) FROM ledger_auditevent");
    let Outcome::Page { html, status } = submit(&mut f.app, &new, [values(&agency, &bank, &key, "15.00"), vec![("action", "review".into())]].concat()) else { panic!() };
    assert_eq!(status, 200);
    assert!(html.contains("Check before recording") && html.contains("2100 · Account 2100") && html.contains("15.00"), "{html}");
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM ledger_auditevent"), before);
    let token = field(&html, "review_token");
    // Recording a changed amount with that review is refused and writes nothing.
    let Outcome::Page { html: refused, .. } = submit(&mut f.app, &new, [values(&agency, &bank, &key, "16.00"), vec![("action", "record".into()), ("review_token", token.clone())]].concat()) else { panic!() };
    assert!(refused.contains("changed since the review"), "{refused}");
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM salestax_payment"), 0);
    // A fresh review, then record.
    let Outcome::Page { html, .. } = submit(&mut f.app, &new, [values(&agency, &bank, &key, "15.00"), vec![("action", "review".into())]].concat()) else { panic!() };
    let token = field(&html, "review_token");
    let out = submit(&mut f.app, &new, [values(&agency, &bank, &key, "15.00"), vec![("action", "record".into()), ("review_token", token.clone())]].concat());
    let Outcome::Redirect { location } = out else { panic!("{out:?}") };
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM salestax_payment"), 1);
    let detail = screens::render(&mut f.app, &location).unwrap().html;
    assert!(detail.contains("Sales tax payment STP-000001") && detail.contains("Payment STP-000001 recorded."), "{detail}");
    // Shared navigation includes the new document without exposing another company.
    let exact = screens::render(&mut f.app, &format!("/c/{}/find/?q=STP-000001", f.c)).unwrap();
    assert_eq!(exact.status, 303);
    assert_eq!(exact.html, location);
    for query in ["STP-000001", "Q3-2026", "15.00"] {
        let results = screens::render(&mut f.app, &format!("/c/{}/find/?q={query}&all=1", f.c)).unwrap().html;
        assert!(results.contains("Sales tax payments") && results.contains(&location), "{results}");
    }
    let home = screens::render(&mut f.app, &format!("/c/{}/home/", f.c)).unwrap().html;
    assert!(home.contains("Sales tax payment") && home.contains("STP-000001") && home.contains(&location));
    let o = f.app.open_mut().unwrap();
    let other = services::create_company(&mut o.book, o.owner, "Other synthetic company", "USD", false).unwrap().id;
    let terms = fb_core::find::Terms::parse("STP-000001").unwrap();
    let hits = fb_core::find::find(&mut o.book, o.owner, other, &terms, 20).unwrap();
    assert!(hits.iter().all(|group| group.hits.is_empty()), "tax payment stays in its company");
    // The review is spent: sending the same record again cannot post twice.
    let again = submit(&mut f.app, &new, [values(&agency, &bank, &key, "15.00"), vec![("action", "record".into()), ("review_token", token)]].concat());
    assert!(matches!(again, Outcome::Page { status: 400, .. }));
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM salestax_payment"), 1);
    assert!(detail.contains("What posted") && detail.contains("Q3-2026"));
    let owed_now = screens::render(&mut f.app, &format!("{owed}?start=2026-10-01&end=2026-10-31")).unwrap().html;
    assert!(owed_now.contains("6.00") && owed_now.contains("STP-000001"), "{owed_now}");

    // The bank register lists the payment as a document and links to its page.
    let register = screens::render(&mut f.app, &format!("/c/{}/accounts/{}/register/", f.c, f.bank)).unwrap().html;
    let id = location.trim_end_matches('/').rsplit('/').next().unwrap().to_owned();
    assert!(register.contains("Sales tax payment") && register.contains(&format!("/sales-tax/payments/{id}/")), "{register}");

    // Void: a date before the payment is refused; then the reversal posts and the amount is owed again.
    let void = format!("/c/{}/sales-tax/payments/{id}/void/", f.c);
    // Worker item 10 and review S3: the form starts on the payment's own date, marked for the
    // earlier-month notice (review S1).
    let form = screens::render(&mut f.app, &void).unwrap().html;
    assert!(form.contains("name=\"date\" value=\"2026-10-20\" data-month-notice=\"2026-11-11\""), "{form}");
    let Outcome::Invalid { html } = submit(&mut f.app, &void, vec![("expected_revision", "1".into()), ("date", "2026-10-19".into()), ("reason", "Paid the wrong agency".into())]) else { panic!() };
    assert!(html.contains("cannot be before the payment date"));
    let out = submit(&mut f.app, &void, vec![("expected_revision", "1".into()), ("date", "2026-10-25".into()), ("reason", "Paid the wrong agency".into())]);
    assert!(matches!(out, Outcome::Redirect { .. }), "{out:?}");
    let detail = screens::render(&mut f.app, &location).unwrap().html;
    assert!(detail.contains("Voided") && detail.contains("Paid the wrong agency") && !detail.contains("/void/\""));
    let recent = screens::render(&mut f.app, &format!("/c/{}/home/", f.c)).unwrap().html;
    assert!(recent.contains("STP-000001") && recent.contains("doc-state\">Void"), "{recent}");
    let owed_after = screens::render(&mut f.app, &format!("{owed}?start=2026-10-01&end=2026-10-31")).unwrap().html;
    assert!(owed_after.contains("21.00"));
    // History names the actions in plain words.
    let history = screens::render(&mut f.app, &format!("/c/{}/history/", f.c)).unwrap().html;
    assert!(history.contains("Sales tax payment recorded") && history.contains("Sales tax payment voided"));
}

#[test]
fn form_errors_are_on_the_fields_and_write_nothing() {
    let mut f = fixture();
    let (agency, bank) = (f.agency.clone(), f.bank.clone());
    let new = format!("/c/{}/sales-tax/payments/new/", f.c);
    let form = screens::render(&mut f.app, &new).unwrap().html;
    let key = field(&form, "request_key");
    let mut bad = values(&agency, &bank, &key, "");
    bad.retain(|(k, _)| *k != "agency");
    bad.push(("penalty_amount", "5.00".into()));
    bad.push(("action", "review".into()));
    let Outcome::Page { html, status } = submit(&mut f.app, &new, bad) else { panic!() };
    assert_eq!(status, 400);
    assert!(html.contains("This field is required.") && html.contains("Choose the expense account for the penalty and interest."), "{html}");
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM salestax_payment"), 0);
}

fn user(f: &mut Fx, name: &str, role: Role) -> UserId {
    f.app.save_desktop_user(None, name, role, true, Some("Synthetic individual password"), &[f.co]).unwrap()
}
#[test]
fn recording_a_payment_needs_the_same_permission_as_writing_a_check() {
    let mut f = fixture();
    let (agency, bank) = (f.agency.clone(), f.bank.clone());
    let reader = user(&mut f, "Reader", Role::Readonly);
    let _ = reader;
    support::sign_in(&mut f.app);
    user(&mut f, "Keeper", Role::Bookkeeper);
    let new = format!("/c/{}/sales-tax/payments/new/", f.c);
    let owed = format!("/c/{}/sales-tax/owed/", f.c);
    f.app.sign_out();
    f.app.sign_in("Reader", "Synthetic individual password").unwrap();
    // A read-only user can see what is owed, but not record, and the typed commands are refused too.
    assert_eq!(screens::render(&mut f.app, &owed).unwrap().status, 200);
    let token = f.app.tokens.mint(&new);
    assert!(f.app.run_renderer(Command::SubmitForm { path: new.clone(), token, fields: [values(&agency, &bank, &uuid::Uuid::new_v4().to_string(), "1.00"), vec![("action", "review".into())]].concat().into_iter().map(|(k, v)| (k.to_string(), v)).collect() }).is_err());
    let cmd = |kind: &str| -> Value { json!({"type":kind,"company_id":f.c,"values":{"request_key":uuid::Uuid::new_v4().to_string(),"agency_id":f.agency,"date":"2026-10-20","bank_id":f.bank,"tax_amount":"1.00","penalty_amount":"0","expense_account_id":null,"period_start":"","period_end":"","reference":"","memo":""}}) };
    assert!(f.app.run(serde_json::from_value(cmd("ReviewSalesTaxPayment")).unwrap()).is_err());
    f.app.sign_out();
    f.app.sign_in("Keeper", "Synthetic individual password").unwrap();
    let Outcome::Done { data } = f.app.run(serde_json::from_value(cmd("ReviewSalesTaxPayment")).unwrap()).unwrap() else { panic!() };
    assert!(data["token"].is_string());
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM salestax_payment"), 0);
    // The command contract refuses unknown fields before anything touches the book.
    let mut extra = cmd("ReviewSalesTaxPayment");
    extra["values"]["debit_account_id"] = json!(f.bank);
    assert!(serde_json::from_value::<Command>(extra).is_err());
}
