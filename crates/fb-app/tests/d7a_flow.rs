//! Real encrypted-book boundaries for the D7a forms, reviews, retries and ordering.
mod support;
use fb_app::{App, Command, Outcome, commands::parse_command, keys, screens};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
static N: AtomicUsize = AtomicUsize::new(0);
fn run(app: &mut App, v: Value) -> Value {
    match app.run(parse_command(&v.to_string()).unwrap()).unwrap() {
        Outcome::Done { data } => data,
        o => panic!("{o:?}"),
    }
}
fn field(html: &str, name: &str) -> String {
    html.split_once(&format!("name=\"{name}\" value=\""))
        .unwrap()
        .1
        .split('"')
        .next()
        .unwrap()
        .into()
}
fn post(app: &mut App, path: &str, p: &[(String, String)]) -> Outcome {
    let html = screens::render(app, path).unwrap().html;
    app.run(Command::SubmitForm {
        path: path.into(),
        token: field(&html, "fb_command_token"),
        fields: p.to_vec(),
    })
    .unwrap()
}
fn count(app: &mut App, table: &str) -> i64 {
    app.open_mut()
        .unwrap()
        .book
        .read(|tx| tx.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)))
        .unwrap()
}
fn pairs(v: Value) -> Vec<(String, String)> {
    v.as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().into()))
        .collect()
}
struct Fixture {
    app: App,
    c: String,
    bank: String,
    expense: String,
    vendor: String,
    ar: String,
    uf: String,
    income: String,
    customer: String,
}
fn fixture() -> Fixture {
    let dir = std::env::temp_dir().join(format!(
        "fb-d7a-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new();
    app.today = || chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
    let c = app
        .create_book(
            &dir.join("book.fbbook"),
            &keys::new_master_key().unwrap(),
            "owner",
            "D7a form tests",
        )
        .unwrap()
        .company_id
        .to_db();
    support::sign_in(&mut app);
    let mut account = |code: &str, detail: &str| {
        run(&mut app,json!({"type":"SaveAccount","company_id":c,"code":code,"name":code,"detail_type":detail}))["id"].as_str().unwrap().to_owned()
    };
    let bank = account("1000", "BANK");
    let ar = account("1100", "RECEIVABLE");
    let uf = account("1200", "OTHER_CURRENT_ASSET");
    let expense = account("5000", "EXPENSE");
    let income = account("4000", "INCOME");
    run(
        &mut app,
        json!({"type":"SaveSetup","company_id":c,"expected_revision":0,"receivables_id":ar,"undeposited_id":uf,"bank_ids":[bank]}),
    );
    let o = app.open_mut().unwrap();
    let vendor = fb_core::directory::save_record(
        &mut o.book,
        o.owner,
        c.parse().unwrap(),
        fb_core::directory::Model::Vendor,
        &json!({"display_name":"Supplier"}),
        None,
    )
    .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let customer = run(
        &mut app,
        json!({"type":"SaveCustomer","company_id":c,"display_name":"Buyer"}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    Fixture {
        app,
        c,
        bank,
        expense,
        vendor,
        ar,
        uf,
        income,
        customer,
    }
}
fn check_fields(f: &Fixture, key: &str) -> Vec<(String, String)> {
    pairs(
        json!({"request_key":key,"expected_revision":"0","bank":f.bank,"date":"2026-10-15","payee_kind":"VENDOR","vendor":f.vendor,"print_later":"on","lines-TOTAL_FORMS":"2","lines-INITIAL_FORMS":"0","lines-0-tab":"EXPENSE","lines-0-description":"Supplies","lines-0-quantity":"1","lines-0-rate":"10.25","lines-0-account":f.expense,"lines-1-tab":"ITEM","lines-1-quantity":"1","lines-1-description":" \t\n ","lines-1-DELETE":"false","lines-1-refresh_item":"false"}),
    )
}
fn set(p: &mut Vec<(String, String)>, key: &str, value: &str) {
    p.retain(|(k, _)| k != key);
    p.push((key.into(), value.into()));
}
fn saved(o: Outcome) -> String {
    let Outcome::Redirect { location } = o else {
        panic!("{o:?}")
    };
    location
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap()
        .into()
}
fn check(f: &mut Fixture) -> String {
    let path = format!("/c/{}/checks/new/", f.c);
    let p = check_fields(f, "00000000-0000-0000-0000-000000007001");
    saved(post(&mut f.app, &path, &p))
}
fn receipt(f: &mut Fixture) -> String {
    let invoice=run(&mut f.app,json!({"type":"SaveInvoice","company_id":f.c,"values":{"request_key":"00000000-0000-0000-0000-000000007002","customer_id":f.customer,"date":"2026-10-01","due_date":"2026-10-31","tax_mode":"NONE","lines":[{"description":"Sale","quantity":"1","rate":"100.00","income_account_id":f.income}]}}))["id"].as_str().unwrap().to_owned();
    run(
        &mut f.app,
        json!({"type":"PostInvoice","company_id":f.c,"invoice_id":invoice,"expected_revision":1}),
    );
    let o = f.app.open_mut().unwrap();
    let c = f.c.parse().unwrap();
    let values = json!({"request_key":"00000000-0000-0000-0000-000000007003","customer_id":f.customer,"date":"2026-10-05","amount":"10.00","method":"CASH","destination":"UNDEPOSITED","allocations":[{"invoice_id":invoice,"amount":"10.00"}]});
    let review = fb_core::sales::review_receipt(&mut o.book, o.owner, c, &values).unwrap();
    fb_core::sales::record_receipt(&mut o.book, o.owner, c, &values, Some(&review)).unwrap()["id"]
        .as_str()
        .unwrap()
        .into()
}

#[test]
fn check_form_ignores_unchanged_whitespace_extras_and_records_only_after_review() {
    let mut f = fixture();
    let id = check(&mut f);
    assert_eq!(count(&mut f.app, "checks_checkline"), 1);
    assert_eq!(count(&mut f.app, "ledger_journal"), 0);
    let path = format!("/c/{}/checks/{id}/review/?revision=1", f.c);
    let html = screens::render(&mut f.app, &path).unwrap().html;
    assert!(html.contains("Supplies"));
    let rt = field(&html, "review_token");
    let token = field(&html, "fb_command_token");
    let path = format!("/c/{}/checks/{id}/record/", f.c);
    let out = f
        .app
        .run(Command::SubmitForm {
            path,
            token,
            fields: pairs(json!({"expected_revision":"1","review_token":rt})),
        })
        .unwrap();
    assert!(matches!(out, Outcome::Redirect { .. }));
    assert_eq!(count(&mut f.app, "ledger_journal"), 1);
    let html = screens::render(&mut f.app, &format!("/c/{}/checks/{id}/", f.c))
        .unwrap()
        .html;
    assert!(html.contains("Recorded"));
    assert!(html.contains("To print"));
    let out = post(
        &mut f.app,
        &format!("/c/{}/checks/{id}/number/", f.c),
        &pairs(json!({"expected_revision":"2","check_number":"  1001  "})),
    );
    assert!(matches!(out, Outcome::Redirect { .. }));
    let out = post(
        &mut f.app,
        &format!("/c/{}/checks/{id}/void/", f.c),
        &pairs(
            json!({"expected_revision":"3","date":"2026-10-20","reason":"Synthetic correction"}),
        ),
    );
    assert!(matches!(out, Outcome::Redirect { .. }));
    assert_eq!(count(&mut f.app, "ledger_journal"), 2);
}

#[test]
fn deleted_invalid_check_row_is_cleaned_for_display_and_excluded_from_saving() {
    let mut f = fixture();
    let path = format!("/c/{}/checks/new/", f.c);
    let mut p = check_fields(&f, "00000000-0000-0000-0000-000000007090");
    set(&mut p, "date", "bad");
    set(&mut p, "lines-0-rate", "bad");
    set(&mut p, "lines-0-DELETE", "on");
    set(&mut p, "lines-1-tab", "EXPENSE");
    set(&mut p, "lines-1-account", &f.expense);
    set(&mut p, "lines-1-description", "Kept line");
    set(&mut p, "lines-1-rate", "3.00");
    let before = count(&mut f.app, "ledger_auditevent");
    let Outcome::Invalid { html } = post(&mut f.app, &path, &p) else { panic!("expected invalid header"); };
    assert!(html.contains("id_lines-0-rate_error"));
    assert!(html.contains("aria-describedby=\"id_lines-0-rate_error\""));
    assert_eq!(count(&mut f.app, "ledger_auditevent"), before);
    set(&mut p, "date", "2026-10-15");
    saved(post(&mut f.app, &path, &p));
    assert_eq!(count(&mut f.app, "checks_checkline"), 1);
    let o = f.app.open_mut().unwrap();
    assert!(o.book.check().unwrap().is_empty());
}
#[test]
fn check_field_errors_precede_services_and_render_independently() {
    let mut f = fixture();
    let path = format!("/c/{}/checks/new/", f.c);
    let mut p = check_fields(&f, "bad");
    set(&mut p, "date", "bad");
    set(&mut p, "vendor", "");
    set(&mut p, "payee_kind", " VENDOR ");
    set(&mut p, "lines-0-rate", "bad");
    let before = count(&mut f.app, "ledger_auditevent");
    let Outcome::Invalid { html } = post(&mut f.app, &path, &p) else {
        panic!("expected invalid")
    };
    for message in [
        "Enter a valid UUID.",
        "Enter a valid date.",
        "Select a valid choice.",
        "finite nonnegative value",
    ] {
        assert!(html.contains(message), "{message}: {html}");
    }
    assert!(!html.contains("id_lines-1-rate_error"));
    assert_eq!(count(&mut f.app, "ledger_auditevent"), before);
    assert_eq!(count(&mut f.app, "checks_check"), 0);
}
#[test]
fn check_management_form_and_initial_template_rows_follow_django() {
    let mut f = fixture();
    let path = format!("/c/{}/checks/new/", f.c);
    let mut p = check_fields(&f, "00000000-0000-0000-0000-000000007004");
    set(&mut p, "lines-INITIAL_FORMS", "bad");
    let Outcome::Invalid { html } = post(&mut f.app, &path, &p) else {
        panic!("expected invalid")
    };
    assert!(html.contains("ManagementForm data is missing"));
    assert_eq!(count(&mut f.app, "checks_check"), 0);
    set(&mut p, "lines-INITIAL_FORMS", "0");
    set(&mut p, "lines-0-description", "");
    set(&mut p, "lines-0-rate", "");
    set(&mut p, "lines-0-account", "");
    let Outcome::Invalid { html } = post(&mut f.app, &path, &p) else {
        panic!("expected invalid")
    };
    assert!(html.contains("Please submit at least 1 form."));
    assert!(!html.contains("id_lines-0-rate_error"));
}
#[test]
fn stale_check_review_and_forged_capability_do_not_post() {
    let mut f = fixture();
    let id = check(&mut f);
    let review = run(
        &mut f.app,
        json!({"type":"ReviewCheck","company_id":f.c,"check_id":id,"expected_revision":1}),
    );
    let mut p = check_fields(&f, "00000000-0000-0000-0000-000000007001");
    set(&mut p, "expected_revision", "1");
    set(&mut p, "lines-TOTAL_FORMS", "1");
    set(&mut p, "lines-INITIAL_FORMS", "1");
    set(&mut p, "lines-0-origin_position", "1");
    set(&mut p, "lines-0-rate", "12.00");
    let path = format!("/c/{}/checks/{id}/edit/", f.c);
    assert!(matches!(
        post(&mut f.app, &path, &p),
        Outcome::Redirect { .. }
    ));
    let before = count(&mut f.app, "ledger_auditevent");
    for token in [review["token"].as_str().unwrap(), "forged"] {
        let err=f.app.run(parse_command(&json!({"type":"RecordCheck","company_id":f.c,"check_id":id,"expected_revision":2,"review_token":token}).to_string()).unwrap()).unwrap_err();
        assert!(err.to_string().contains("review"));
    }
    assert_eq!(count(&mut f.app, "ledger_auditevent"), before);
    assert_eq!(count(&mut f.app, "ledger_journal"), 0);
}
#[test]
fn deposit_review_binds_selection_and_record_reverse_release_the_whole_receipt() {
    let mut f = fixture();
    let receipt = receipt(&mut f);
    let path = format!("/c/{}/deposits/new/", f.c);
    let html = screens::render(&mut f.app, &path).unwrap().html;
    assert!(html.contains(&format!("id=\"receipt-{receipt}\"")));
    let p = pairs(
        json!({"request_key":"00000000-0000-0000-0000-000000007005","bank":f.bank,"date":"2026-10-15","memo":"Whole receipt deposit","receipts":receipt,"action":"review"}),
    );
    let Outcome::Page { html, status } = post(&mut f.app, &path, &p) else {
        panic!("expected review")
    };
    assert_eq!(status, 200);
    assert!(html.contains("10.00"));
    let rt = field(&html, "review_token");
    let mut record = p.clone();
    set(&mut record, "action", "record");
    set(&mut record, "review_token", &rt);
    let mut changed = record.clone();
    set(&mut changed, "memo", "Changed after review");
    let before = count(&mut f.app, "ledger_auditevent");
    let Outcome::Page { html, status } = post(&mut f.app, &path, &changed) else {
        panic!("expected refusal")
    };
    assert_eq!(status, 400);
    assert!(html.contains("changed"));
    assert_eq!(count(&mut f.app, "ledger_auditevent"), before);
    assert_eq!(count(&mut f.app, "banking_deposit"), 0);
    let Outcome::Page { html, status } = post(&mut f.app, &path, &record) else { panic!("consumed review must fail") };
    assert_eq!(status,400);assert!(html.contains("review"));
    assert_eq!(count(&mut f.app,"ledger_auditevent"),before);
    assert_eq!(count(&mut f.app,"banking_deposit"),0);
    let Outcome::Page { html, status } = post(&mut f.app,&path,&p) else { panic!("fresh review") };
    assert_eq!(status,200);set(&mut record,"review_token",&field(&html,"review_token"));
    let id = saved(post(&mut f.app, &path, &record));
    assert_eq!(count(&mut f.app, "banking_deposit"), 1);
    let html = screens::render(&mut f.app, &path).unwrap().html;
    assert!(!html.contains(&format!("id=\"receipt-{receipt}\"")));
    let reverse = format!("/c/{}/deposits/{id}/reverse/", f.c);
    assert!(matches!(
        post(
            &mut f.app,
            &reverse,
            &pairs(
                json!({"expected_revision":"1","date":"2026-10-20","reason":"Synthetic correction"})
            )
        ),
        Outcome::Redirect { .. }
    ));
    let html = screens::render(&mut f.app, &path).unwrap().html;
    assert!(html.contains(&format!("id=\"receipt-{receipt}\"")));
}
#[test]
fn deposit_missing_selection_padded_choice_and_cross_company_input_do_not_write() {
    let mut f = fixture();
    let path = format!("/c/{}/deposits/new/", f.c);
    let before = count(&mut f.app, "ledger_auditevent");
    let out = post(
        &mut f.app,
        &path,
        &pairs(
            json!({"request_key":"bad","bank":"00000000-0000-0000-0000-000000000000","date":"bad"}),
        ),
    );
    let Outcome::Page { html, status } = out else {
        panic!("invalid")
    };
    assert_eq!(status, 400);
    for msg in [
        "Enter a valid UUID.",
        "Select a valid choice.",
        "Enter a valid date.",
        "Select 1–50",
    ] {
        assert!(html.contains(msg), "{html}");
    }
    assert_eq!(count(&mut f.app, "ledger_auditevent"), before);
    for suffix in ["checks/?state=%20DRAFT%20", "deposits/?state=%20LIVE%20"] {
        assert_eq!(
            screens::render(&mut f.app, &format!("/c/{}/{suffix}", f.c))
                .unwrap()
                .status,
            400
        );
    }
    assert!(!f.ar.is_empty() && !f.uf.is_empty());
}
#[test]
fn d7a_contract_refuses_unknown_fields_and_lock_drops_capabilities() {
    assert!(parse_command(r#"{"type":"ReviewDeposit","company_id":"c","values":{"request_key":"k","bank_id":"b","date":"d","reference":"","memo":"","receipt_ids":[],"post_immediately":true}}"#).is_err());
    let mut f = fixture();
    let id = check(&mut f);
    let review = run(
        &mut f.app,
        json!({"type":"ReviewCheck","company_id":f.c,"check_id":id,"expected_revision":1}),
    );
    f.app.lock();
    assert!(f.app.run(parse_command(&json!({"type":"RecordCheck","company_id":f.c,"check_id":id,"expected_revision":1,"review_token":review["token"]}).to_string()).unwrap()).is_err());
}

#[test]
fn model_choice_and_uuid_fields_strip_padding_but_plain_choices_do_not() {
    let mut f = fixture();
    let path = format!("/c/{}/checks/new/", f.c);
    let mut p = check_fields(&f, " \t00000000-0000-0000-0000-000000007009 \n");
    for name in ["bank", "vendor", "lines-0-account"] {
        let value = p.iter().find(|(k, _)| k == name).unwrap().1.clone();
        set(&mut p, name, &format!(" \t{value} \n"));
    }
    assert!(matches!(
        post(&mut f.app, &path, &p),
        Outcome::Redirect { .. }
    ));
    assert_eq!(count(&mut f.app, "checks_check"), 1);
    let mut p = check_fields(&f, "00000000-0000-0000-0000-000000007010");
    set(&mut p, "payee_kind", " VENDOR ");
    let Outcome::Invalid { html } = post(&mut f.app, &path, &p) else {
        panic!("plain ChoiceField must reject padding")
    };
    assert!(html.contains("Select a valid choice.  VENDOR  is not one"));
    assert_eq!(count(&mut f.app, "checks_check"), 1);
}

#[test]
fn editing_a_fifty_line_check_does_not_render_two_extra_rows_beyond_the_limit() {
    let mut f = fixture();
    let line = json!({"tab":"EXPENSE","origin_position":null,"account_id":f.expense,"item_id":null,"description":"Supplies","quantity":"1","rate":"0.01","refresh_item":false,"keep_archived":false});
    let saved = run(
        &mut f.app,
        json!({"type":"SaveCheck","company_id":f.c,"values":{"check_id":null,"expected_revision":0,"request_key":"00000000-0000-0000-0000-000000007050","bank_id":f.bank,"date":"2026-10-15","check_number":"","print_later":true,"payee_kind":"OTHER","vendor_id":null,"payee_name":"Synthetic payee","address":"","memo":"","refresh_payee":false,"keep_archived":false,"lines":vec![line;50]}}),
    );
    let html = screens::render(
        &mut f.app,
        &format!("/c/{}/checks/{}/edit/", f.c, saved["id"].as_str().unwrap()),
    )
    .unwrap()
    .html;
    assert_eq!(field(&html, "lines-TOTAL_FORMS"), "50");
    assert_eq!(field(&html, "lines-INITIAL_FORMS"), "50");
    assert_eq!(html.matches("check-line tab-expense").count(), 50);
}
