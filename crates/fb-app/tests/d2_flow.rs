//! D2 forms exercised against an actual encrypted book; invalid fields must never reach writes.
mod support;
use fb_app::{App, Command, Outcome, commands::parse_command, keys, screens};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
static N: AtomicUsize = AtomicUsize::new(0);

fn fixture() -> (App, String) {
    let dir = std::env::temp_dir().join(format!(
        "fb-app-d2-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new();
    app.today = || chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
    let c = app
        .create_book(
            &dir.join("books.fbbook"),
            &keys::new_master_key().unwrap(),
            "+Owner",
            "=Synthetic company",
        )
        .unwrap()
        .company_id
        .to_db();
    support::sign_in(&mut app);
    (app, c)
}
fn command(app: &mut App, value: Value) -> Value {
    match app.run(parse_command(&value.to_string()).unwrap()).unwrap() {
        Outcome::Done { data } => data,
        other => panic!("{other:?}"),
    }
}
fn acct(app: &mut App, c: &str, code: &str, kind: &str) -> String {
    command(
        app,
        json!({"type":"SaveAccount","company_id":c,"code":code,"name":code,"detail_type":kind}),
    )["id"]
        .as_str()
        .unwrap()
        .into()
}
fn field(html: &str, name: &str) -> String {
    let marker = format!("name=\"{name}\" value=\"");
    html.split_once(&marker)
        .unwrap()
        .1
        .split('"')
        .next()
        .unwrap()
        .into()
}
fn post(app: &mut App, path: &str, fields: &[(String, String)]) -> Outcome {
    let html = screens::render(app, path).unwrap().html;
    app.run(Command::SubmitForm {
        path: path.into(),
        token: field(&html, "fb_command_token"),
        fields: fields.to_vec(),
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

#[test]
fn journal_added_line_field_errors_rounding_and_optimistic_edits() {
    let (mut app, c) = fixture();
    let expense = acct(&mut app, &c, "5000", "EXPENSE");
    let income = acct(&mut app, &c, "4000", "INCOME");
    let path = format!("/c/{c}/journals/new/");
    let html = screens::render(&mut app, &path).unwrap().html;
    assert!(html.contains("data-picker=\"account\"") && !html.contains("Without JavaScript"));
    let mut values = vec![
        ("request_key".into(), field(&html, "request_key")),
        ("expected_revision".into(), "0.00".into()),
        ("date".into(), "10/31/2026".into()),
        ("memo".into(), "Adjustment".into()),
        ("entry_number".into(), "JE-1".into()),
        ("adjusting".into(), "on".into()),
        ("lines-TOTAL_FORMS".into(), "9".into()),
        ("lines-INITIAL_FORMS".into(), "0".into()),
        ("lines-0-account".into(), expense),
        ("lines-0-debit".into(), "12.345".into()),
        ("lines-0-name".into(), "Dana 😀".into()),
        (
            "lines-4-description".into(),
            "Copied memo on unused row".into(),
        ),
        ("lines-8-account".into(), income),
        ("lines-8-credit".into(), "12.35".into()),
    ];
    let mut invalid = values.clone();
    invalid.iter_mut().find(|(k, _)| k == "date").unwrap().1 = "bad".into();
    invalid.iter_mut().find(|(k, _)| k == "lines-0-debit").unwrap().1 = "abc".into();
    let before = count(&mut app, "ledger_auditevent");
    let Outcome::Invalid { html } = post(&mut app, &path, &invalid) else {
        panic!("invalid date accepted")
    };
    assert!(
        html.contains("Enter a valid date.") && html.contains("<td colspan=\"6\">Line 1 debit")
            && html.contains("name=\"lines-8-credit\" value=\"12.35\"")
    );
    assert_eq!(count(&mut app, "ledger_journal"), 0);
    assert_eq!(count(&mut app, "ledger_auditevent"), before);
    let Outcome::Redirect { location } = post(&mut app, &path, &values) else {
        panic!("draft did not save")
    };
    assert_eq!(
        count(&mut app, "ledger_journalline"),
        2,
        "blank memo-only rows are discarded"
    );
    let detail = screens::render(&mut app, &location).unwrap().html;
    assert!(
        detail.contains("12.35")
            && detail.contains("Dana 😀")
            && detail.contains("Adjusting entry")
    );
    let mut clash = values.clone();
    clash.iter_mut().find(|(k, _)| k == "request_key").unwrap().1 = uuid::Uuid::new_v4().to_string();
    let Outcome::Invalid { html } = post(&mut app, &path, &clash) else { panic!("duplicate saved") };
    assert!(html.contains("Entry number JE-1 is already used"));
    assert_eq!(field(&html, "entry_number"), field(&screens::render(&mut app, &path).unwrap().html, "entry_number"));
    assert_eq!(field(&html, "memo"), "Adjustment");
    let edit = format!("{location}edit/");
    values
        .iter_mut()
        .find(|(k, _)| k == "expected_revision")
        .unwrap()
        .1 = "0".into();
    let before = count(&mut app, "ledger_auditevent");
    let Outcome::Invalid { html } = post(&mut app, &edit, &values) else {
        panic!("stale edit accepted")
    };
    assert!(html.contains("changed") || html.contains("revision"));
    assert_eq!(count(&mut app, "ledger_auditevent"), before);
}

#[test]
fn ordinal_labels_stay_short_and_stable_across_edits_and_inactivation() {
    let (mut app, c) = fixture();
    acct(&mut app, &c, "9000", "BANK");
    let income = acct(&mut app, &c, "1000", "INCOME");
    let item=command(&mut app,json!({"type":"SaveItem","company_id":c,"code":"MOW","description":"Mowing","kind":"SERVICE","sale_rate":"1","income_account_id":income}))["id"].as_str().unwrap().to_owned();
    // Since 5a65c381 the invoice editor is item-driven: items show their code alone and the income
    // account is a hidden field; estimates, sales receipts and credit memos now share that editor.
    // The check form still labels items with their ordinal; no current form lists income accounts with
    // ordinals any more, so the account rename/inactivation part of this test went with the old forms.
    let path = format!("/c/{c}/checks/new/");
    let html = screens::render(&mut app, &path).unwrap().html;
    assert!(html.contains(&format!("<option value=\"{item}\">MOW · #1</option>")));
    assert!(!html.contains(&format!("#{income}")) && !html.contains(&format!("#{item}")));
}

#[test]
fn company_and_account_field_errors_never_write_and_cross_company_forms_are_refused() {
    let (mut app, c) = fixture();
    let before = count(&mut app, "ledger_auditevent");
    let Outcome::Invalid { html } = post(
        &mut app,
        "/companies/new/",
        &[("name".into(), "".into()), ("currency".into(), "12".into())],
    ) else {
        panic!()
    };
    assert!(html.contains("id_name_error") && html.contains("id_currency_error"));
    assert_eq!(count(&mut app, "ledger_company"), 1);
    let Outcome::Invalid { html } = post(
        &mut app,
        &format!("/c/{c}/accounts/new/"),
        &[
            ("code".into(), "1000".into()),
            ("name".into(), "Checking".into()),
            ("detail_type".into(), "BANK".into()),
            ("is_contra".into(), "on".into()),
        ],
    ) else {
        panic!()
    };
    assert!(html.contains("A bank account cannot be a contra account."));
    assert_eq!(count(&mut app, "ledger_account"), 0);
    assert_eq!(count(&mut app, "ledger_auditevent"), before);
    let open = app.open_mut().unwrap();
    let stranger = fb_core::users::create_user(&mut open.book, "Other", false).unwrap();
    let other =
        fb_core::services::create_company(&mut open.book, stranger, "Other books", "USD", false)
            .unwrap();
    assert!(screens::render(&mut app, &format!("/c/{}/accounts/new/", other.id.to_db())).is_err());
}

#[test]
fn audit_csv_has_bom_formula_guards_and_scope_and_rejects_invalid_filters() {
    let (mut app, c) = fixture();
    let outcome=app.run(parse_command(&json!({"type":"ExportHistory","company_id":c,"filters":[["action","company.created"]]}).to_string()).unwrap()).unwrap();
    let Outcome::Export { filename, contents } = outcome else {
        panic!()
    };
    assert_eq!(filename, "audit-history.csv");
    assert!(contents.starts_with('\u{feff}'));
    assert!(contents.contains("When,User,Action,Document,Reason\r\n"));
    assert!(contents.contains(",'+Owner,Company created,'=Synthetic company,"));
    assert!(
        app.run(
            parse_command(
                &json!({"type":"ExportHistory","company_id":c,"filters":[["start","bad"]]})
                    .to_string()
            )
            .unwrap()
        )
        .is_err()
    );
}

#[test]
fn new_company_journal_list_reminds_until_checklist_is_dismissed() {
    // The journal list opens as itself (it used to redirect to the checklist); the checklist is a
    // reminder on it until dismissed.
    let (mut app, c) = fixture();
    let path = format!("/c/{c}/");
    let initial = screens::render(&mut app, &path).unwrap();
    assert_eq!(initial.status, 200);
    assert!(initial.html.contains("Journal entries"));
    assert!(initial.html.contains(&format!(r#"<a href="/c/{c}/setup/start/">Finish setting up</a>"#)));
    command(&mut app, json!({"type":"DismissSetup","company_id":c}));
    let ready = screens::render(&mut app, &path).unwrap();
    assert_eq!(ready.status, 200);
    assert!(ready.html.contains("Journal entries") && !ready.html.contains("Finish setting up"));
}

#[test]
fn setup_fill_uses_python_int_and_malformed_revision_has_no_audit_writes() {
    let (mut app,c)=fixture();
    acct(&mut app,&c,"1000","BANK");acct(&mut app,&c,"1100","RECEIVABLE");
    acct(&mut app,&c,"1200","OTHER_CURRENT_ASSET");acct(&mut app,&c,"2000","PAYABLE");
    let path=format!("/c/{c}/setup/start/fill/");
    let o=app.open_mut().unwrap();
    let digest=o.book.read(|tx|fb_core::first_run::propose(tx,c.parse().unwrap())).unwrap()["digest"].as_str().unwrap().to_owned();
    let before=count(&mut app,"ledger_auditevent");
    let token=app.tokens.mint(&path);
    let out=app.run(Command::SubmitForm{path:path.clone(),token,fields:vec![("action".into(),"apply".into()),("digest".into(),digest.clone()),("expected_revision".into(),"0.0".into())]}).unwrap();
    assert!(matches!(out,Outcome::Invalid{..}),"{out:?}");
    assert_eq!(count(&mut app,"ledger_auditevent"),before);
    let token=app.tokens.mint(&path);
    let out=app.run(Command::SubmitForm{path,token,fields:vec![("action".into(),"apply".into()),("digest".into(),digest),("expected_revision".into(),"0".into())]}).unwrap();
    assert!(matches!(out,Outcome::Redirect{..}),"{out:?}");
    assert!(count(&mut app,"ledger_auditevent")>before);
}
