//! Card numbers in payment documents' free text are refused at the form and the service, and text
//! that reaches the audit history or a CSV export is shown masked.
mod support;
use fb_app::{App, Command, Outcome, commands::parse_command, keys, screens};
use fb_core::services;
use serde_json::{Value, json};

const PAN: &str = "4111 1111 1111 1111";
const REFUSED: &str = "This looks like a payment card number.";

fn field(html: &str, name: &str) -> String {
    html.split_once(&format!("name=\"{name}\" value=\""))
        .unwrap_or_else(|| panic!("missing {name}")).1.split('"').next().unwrap().into()
}
fn run(app: &mut App, value: Value) -> Value {
    match app.run(parse_command(&value.to_string()).unwrap()).unwrap() {
        Outcome::Done { data } => data,
        other => panic!("{other:?}"),
    }
}
fn refused(app: &mut App, value: Value) -> String {
    app.run(parse_command(&value.to_string()).unwrap()).unwrap_err().to_string()
}
fn submit(app: &mut App, path: &str, values: &Value) -> Outcome {
    let html = screens::render(app, path).unwrap().html;
    let mut values = values.as_object().unwrap().clone();
    if html.contains("name=\"request_key\"") {
        values.entry("request_key").or_insert_with(|| json!(field(&html, "request_key")));
    }
    app.run(Command::SubmitForm {
        path: path.into(),
        token: field(&html, "fb_command_token"),
        fields: values.into_iter().map(|(k, v)| (k, v.as_str().unwrap().into())).collect(),
    })
    .unwrap()
}
fn record(app: &mut App, path: &str, values: &Value) -> String {
    let mut values = values.as_object().unwrap().clone();
    values.insert("action".into(), json!("review"));
    let html = match submit(app, path, &Value::Object(values.clone())) {
        Outcome::Page { html, status: 200 } => html,
        other => panic!("expected review: {other:?}"),
    };
    assert!(html.contains("name=\"review_token\""), "no review: {html}");
    values.insert("request_key".into(), json!(field(&html, "request_key")));
    values.insert("review_token".into(), json!(field(&html, "review_token")));
    values.insert("action".into(), json!("record"));
    match app.run(Command::SubmitForm {
        path: path.into(),
        token: field(&html, "fb_command_token"),
        fields: values.into_iter().map(|(k, v)| (k, v.as_str().unwrap().into())).collect(),
    })
    .unwrap()
    {
        Outcome::Redirect { location } => location,
        other => panic!("did not record: {other:?}"),
    }
}
fn writes(app: &mut App) -> i64 {
    app.open_mut().unwrap().book.read(|tx| tx.query_row("SELECT count(*) FROM ledger_auditevent", [], |r| r.get(0))).unwrap()
}

struct Book {
    app: App,
    _temp: tempfile::TempDir,
    c: String,
    bank: String,
    savings: String,
    income: String,
    expense: String,
    customer: String,
    vendor: String,
}

fn book() -> Book {
    let temp = tempfile::tempdir().unwrap();
    let mut app = App::new();
    app.today = || chrono::NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
    let c = app.create_book(&temp.path().join("book.fbbook"), &keys::new_master_key().unwrap(), "Owner", "Synthetic cards").unwrap().company_id.to_db();
    support::sign_in(&mut app);
    let mut account = |code: &str, kind: &str| run(&mut app, json!({"type":"SaveAccount","company_id":c,"code":code,"name":code,"detail_type":kind}))["id"].as_str().unwrap().to_owned();
    let bank = account("1000", "BANK");
    let savings = account("1010", "BANK");
    let ar = account("1100", "RECEIVABLE");
    let uf = account("1200", "OTHER_CURRENT_ASSET");
    let ap = account("2000", "PAYABLE");
    let liab = account("2300", "OTHER_CURRENT_LIABILITY");
    let income = account("4000", "INCOME");
    let expense = account("5000", "EXPENSE");
    run(&mut app, json!({"type":"SaveSetup","company_id":c,"expected_revision":0,"receivables_id":ar,"payables_id":ap,"undeposited_id":uf,"customer_deposits_id":liab,"bank_ids":[bank,savings]}));
    let mut parties = vec![];
    for kind in ["customers", "vendors"] {
        let Outcome::Redirect { location } = submit(&mut app, &format!("/c/{c}/lists/{kind}/new/"), &json!({"display_name":"Synthetic person"})) else { panic!("{kind}") };
        parties.push(location.trim_end_matches('/').rsplit('/').next().unwrap().to_owned());
    }
    Book { app, _temp: temp, c, bank, savings, income, expense, customer: parties[0].clone(), vendor: parties[1].clone() }
}

fn sale(b: &Book, memo: &str, reference: &str) -> Value {
    json!({"customer":b.customer,"date":"2026-11-05","memo":memo,"reference":reference,"method":"CARD","destination":"UNDEPOSITED","tax_mode":"NONE","lines-TOTAL_FORMS":"1","lines-INITIAL_FORMS":"0","lines-0-description":"Work","lines-0-quantity":"1","lines-0-rate":"10","lines-0-income_account":b.income})
}

#[test]
fn payment_forms_refuse_card_numbers_on_the_field_without_writes() {
    let mut b = book();
    let base = format!("/c/{}", b.c);
    // Sources for the two refund forms.
    let deposit = record(&mut b.app, &format!("{base}/customer-deposits/new/"), &json!({"customer":b.customer,"date":"2026-11-05","amount":"20.00","method":"CARD","destination":"UNDEPOSITED","reference":"","order_reference":"","memo":""}));
    let mut credit = sale(&b, "", "");
    credit.as_object_mut().unwrap().retain(|k, _| !["reference", "method", "destination"].contains(&k.as_str()));
    let credit = record(&mut b.app, &format!("{base}/credit-memos/new/"), &credit);
    let refund = |memo: &str| json!({"date":"2026-11-05","amount":"1.00","bank":b.bank,"method":"CASH","check_number":"","memo":memo});
    let mut cases: Vec<(String, Value, &str)> = vec![];
    for (name, value) in [("reference", PAN), ("memo", "Paid with 4111-1111-1111-1111 exp 12/27")] {
        let (memo, reference) = if name == "memo" { (value, "") } else { ("", value) };
        cases.push((format!("{base}/receipts/new/"), json!({"customer":b.customer,"date":"2026-11-05","method":"CARD","reference":reference,"memo":memo,"destination":"UNDEPOSITED","allocations-TOTAL_FORMS":"0","allocations-INITIAL_FORMS":"0"}), name));
        cases.push((format!("{base}/sales-receipts/new/"), sale(&b, memo, reference), name));
        cases.push((format!("{base}/payments/new/"), json!({"vendor":b.vendor,"date":"2026-11-05","amount":"1.00","method":"OTHER","reference":reference,"memo":memo,"bank":b.bank,"allocations-TOTAL_FORMS":"0","allocations-INITIAL_FORMS":"0"}), name));
        cases.push((format!("{base}/deposits/new/"), json!({"bank":b.bank,"date":"2026-11-05","reference":reference,"memo":memo}), name));
        cases.push((format!("{base}/customer-deposits/new/"), json!({"customer":b.customer,"date":"2026-11-05","amount":"5.00","method":"CARD","destination":"UNDEPOSITED","reference":reference,"order_reference":"","memo":memo}), name));
    }
    cases.push((format!("{base}/customer-deposits/new/"), json!({"customer":b.customer,"date":"2026-11-05","amount":"5.00","method":"CARD","destination":"UNDEPOSITED","reference":"","order_reference":"Order 4111111111111111","memo":""}), "order_reference"));
    cases.push((format!("{deposit}refund/"), refund(PAN), "memo"));
    cases.push((format!("{credit}refund/"), refund(PAN), "memo"));
    cases.push((format!("{base}/transfers/new/"), json!({"date":"2026-11-05","from_account":b.bank,"to_account":b.savings,"amount":"1","memo":PAN}), "memo"));
    cases.push((format!("{base}/checks/new/"), json!({"expected_revision":"0","bank":b.bank,"date":"2026-11-05","print_later":"on","payee_kind":"OTHER","payee_name":"Synthetic payee","address":"","memo":PAN,"lines-TOTAL_FORMS":"1","lines-INITIAL_FORMS":"0","lines-0-tab":"EXPENSE","lines-0-account":b.expense,"lines-0-description":"","lines-0-amount":"1"}), "memo"));
    let before = writes(&mut b.app);
    for (path, values, name) in &cases {
        for pan in [PAN, "4111    1111 1111 1111", "4111        1111 1111 1111"] {
            let mut values = values.clone();
            for value in values.as_object_mut().unwrap().values_mut() {
                if let Some(text) = value.as_str() {
                    *value = json!(text.replace(PAN, pan).replace("4111-1111-1111-1111", pan).replace("4111111111111111", pan));
                }
            }
            values["action"] = json!("review");
            let html = match submit(&mut b.app, path, &values) {
                Outcome::Invalid { html } | Outcome::Page { html, .. } => html,
                other => panic!("{path} {name}: {other:?}"),
            };
            assert!(html.contains(REFUSED), "{path} {name}: {html}");
            assert!(html.contains(&format!("<ul class=\"errorlist\" id=\"id_{name}_error\"><li>{REFUSED}")), "{path} {name}: not on the field: {html}");
        }
    }
    assert_eq!(before, writes(&mut b.app));
}

#[test]
fn near_misses_still_record() {
    let mut b = book();
    let base = format!("/c/{}", b.c);
    let values = sale(&b, "4111.1111.1111.1111 · 411111111111", "card ending 1111");
    record(&mut b.app, &format!("{base}/sales-receipts/new/"), &values);
    record(&mut b.app, &format!("{base}/customer-deposits/new/"), &json!({"customer":b.customer,"date":"2026-11-05","amount":"5.00","method":"CARD","destination":"UNDEPOSITED","reference":"4111 1111 1111 1112","order_reference":"PO 0000 0000 0000 0000","memo":"41111111111111110000 · 4111    1111 1111 1112"}));
    record(&mut b.app, &format!("{base}/transfers/new/"), &json!({"date":"2026-11-05","from_account":b.bank,"to_account":b.savings,"amount":"1","memo":"Confirmation 1234-5678-9012"}));
}

#[test]
fn commands_are_refused_by_the_service_too() {
    let mut b = book();
    let c = b.c.clone();
    let key = || uuid::Uuid::new_v4().to_string();
    let message = refused(&mut b.app, json!({"type":"ReviewTransfer","company_id":c,"values":{"request_key":key(),"date":"2026-11-05","from_account_id":b.bank,"to_account_id":b.savings,"amount":"1","memo":PAN}}));
    assert!(message.contains("Memo: This looks like a payment card number."), "{message}");
    let message = refused(&mut b.app, json!({"type":"ReviewCustomerDeposit","company_id":c,"values":{"request_key":key(),"customer_id":b.customer,"date":"2026-11-05","amount":"5.00","method":"CARD","reference":"4111111111111111","destination":"UNDEPOSITED","bank_id":null,"estimate_id":null,"order_reference":"","memo":""}}));
    assert!(message.contains("Reference: This looks like a payment card number."), "{message}");
    let message = refused(&mut b.app, json!({"type":"ReviewDeposit","company_id":c,"values":{"request_key":key(),"bank_id":b.bank,"date":"2026-11-05","reference":"","memo":PAN,"receipt_ids":[key()]}}));
    assert!(message.contains("Memo: This looks like a payment card number."), "{message}");
}

#[test]
fn gap_card_numbers_are_refused_by_the_service_without_writes() {
    let mut b = book();
    let before = writes(&mut b.app);
    for pan in ["4111    1111 1111 1111", "4111        1111 1111 1111"] {
        let message = refused(&mut b.app, json!({"type":"ReviewTransfer","company_id":b.c,"values":{"request_key":uuid::Uuid::new_v4().to_string(),"date":"2026-11-05","from_account_id":b.bank,"to_account_id":b.savings,"amount":"1","memo":pan}}));
        assert!(message.contains("Memo: This looks like a payment card number."), "{message}");
    }
    assert_eq!(before, writes(&mut b.app));
}

#[test]
fn audit_history_and_csv_exports_mask_stored_card_numbers() {
    audit_and_csv_mask(PAN, "**** **** **** 1111");
}

#[test]
fn audit_history_and_csv_exports_mask_stored_gap_card_numbers() {
    audit_and_csv_mask("4111    1111 1111 1111", "****    **** **** 1111");
    audit_and_csv_mask("4111        1111 1111 1111", "****        **** **** 1111");
}

fn audit_and_csv_mask(pan: &str, masked: &str) {
    let mut b = book();
    let base = format!("/c/{}", b.c);
    // General journals are outside the refusal, so they stand in for text stored before it existed.
    let journal = {
        let o = b.app.open_mut().unwrap();
        let c = b.c.parse().unwrap();
        let (bank, expense) = (b.bank.parse().unwrap(), b.expense.parse().unwrap());
        let journal = services::save_draft(&mut o.book, o.owner, c, services::DraftInput::new("2026-11-05", &format!("Card {pan} fee"), vec![
            services::line(expense).debit("3").build(),
            services::line(bank).credit("3").build(),
        ])).unwrap();
        let journal = services::post_journal(&mut o.book, o.owner, c, journal.id, None).unwrap();
        services::reverse_journal(&mut o.book, o.owner, c, journal.id, "2026-11-05", &format!("Charged {pan} and 4111111111111111 twice")).unwrap();
        journal.id.to_db()
    };
    // The journal page shows its own memo as stored; its change history is masked.
    let page = screens::render(&mut b.app, &format!("{base}/journals/{journal}/")).unwrap().html;
    let changes = &page[page.find("View change").expect("journal history")..];
    assert!(!changes.contains(pan) && changes.contains(&format!("Card {masked} fee")), "{changes}");
    let history = screens::render(&mut b.app, &format!("{base}/history/")).unwrap().html;
    let csv = screens::render(&mut b.app, &format!("{base}/history/?format=csv")).unwrap().html;
    let ledger = screens::render(&mut b.app, &format!("{base}/reports/general-ledger/?basis=ACCRUAL&preset=custom&start=2026-11-01&end=2026-11-30&output=csv")).unwrap().html;
    for (name, text) in [("history", &history), ("history csv", &csv), ("ledger csv", &ledger)] {
        assert!(!text.contains(pan) && !text.contains("4111111111111111"), "{name}: {text}");
    }
    assert!(history.contains(&format!("Charged {masked} and ************1111 twice")), "{history}");
    assert!(csv.contains(&format!("Charged {masked} and ************1111 twice")), "{csv}");
    assert!(ledger.contains(&format!("Card {masked} fee")), "{ledger}");
}
