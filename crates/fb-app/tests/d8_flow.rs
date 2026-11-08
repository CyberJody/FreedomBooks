//! D8 report boundaries on an encrypted, synthetic book. Core arithmetic has its L4 parity suite.
mod support;
use fb_app::{App, Command, Outcome, keys, screens};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
static N: AtomicUsize = AtomicUsize::new(0);
fn fixture() -> (App, String, String) {
    let path = std::env::temp_dir().join(format!(
        "fb-d8-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&path).unwrap();
    let mut app = App::new();
    app.today = || chrono::NaiveDate::from_ymd_opt(2028, 2, 29).unwrap();
    let company = app
        .create_book(
            &path.join("book.fbbook"),
            &keys::new_master_key().unwrap(),
            "owner",
            "D8 synthetic books",
        )
        .unwrap()
        .company_id
        .to_db();
    support::sign_in(&mut app);
    let open = app.open_mut().unwrap();
    let c = company.parse().unwrap();
    let a = fb_core::services::save_account(
        &mut open.book,
        open.owner,
        c,
        fb_core::services::AccountInput {
            code: "100".into(),
            name: "Cash <&>".into(),
            kind: None,
            detail_type: Some("BANK".into()),
            is_contra: None,
            description: None,
            active: true,
            postable: true,
            account: None,
        },
    )
    .unwrap();
    (app, company, a.id.to_db())
}
fn audit(app: &mut App) -> i64 {
    app.open_mut()
        .unwrap()
        .book
        .read(|tx| tx.query_row("SELECT count(*) FROM ledger_auditevent", [], |r| r.get(0)))
        .unwrap()
}
fn page(app: &mut App, c: &str, route: &str, query: &str) -> screens::Rendered {
    screens::render(app, &format!("/c/{c}/reports/{route}/?{query}")).unwrap()
}

#[test]
fn report_controls_use_the_injected_clock_and_keep_invalid_inputs_without_writes() {
    let (mut app, c, _) = fixture();
    let before = audit(&mut app);
    let r = page(&mut app, &c, "profit-and-loss", "");
    assert_eq!(r.status, 200);
    assert!(r.html.contains("February 1 – 29, 2028"));
    assert!(r.html.contains("value=\"2028-02-29\""));
    let r = page(&mut app, &c, "profit-and-loss", "preset=last_month");
    assert!(r.html.contains("January 1 – 31, 2028"));
    let r = page(&mut app, &c, "balance-sheet", "");
    assert!(r.html.contains("As of February 29, 2028"));
    for route in ["profit-and-loss", "balance-sheet", "general-ledger"] {
        let r = page(
            &mut app,
            &c,
            route,
            "basis=%20CASH%20&output=%20csv%20&end=bad&as_of=bad",
        );
        assert_eq!(r.status, 400);
        assert!(
            r.html
                .contains("Select a valid choice.  CASH  is not one of the available choices.")
        );
        assert!(r.html.contains("Enter a valid date."));
        assert!(!r.html.contains("report-actions"));
    }
    let r = page(&mut app, &c, "profit-and-loss", "preset=custom&output=bad");
    assert!(!r.html.contains("Enter a &quot;To&quot; date"));
    let r = page(
        &mut app,
        &c,
        "general-ledger",
        "preset=custom&account=no&group=bad",
    );
    assert_eq!(r.status, 400);
    assert!(!r.html.contains("Enter a &quot;To&quot; date"));
    let r = page(
        &mut app,
        &c,
        "general-ledger",
        "preset=custom&account=1234567890123%00&group=bad",
    );
    assert_eq!(r.status, 400);
    let length = r
        .html
        .find("Ensure this value has at most 12 characters (it has 14).")
        .unwrap();
    let null = r.html.find("Null characters are not allowed.").unwrap();
    let group = r
        .html
        .find("Select a valid choice. bad is not one of the available choices.")
        .unwrap();
    assert!(length < null && null < group);
    assert!(!r.html.contains("Enter a &quot;To&quot; date"));
    assert_eq!(audit(&mut app), before);
}

#[test]
fn account_choices_strip_whitespace_but_choices_do_not_and_scope_is_enforced() {
    let (mut app, c, account) = fixture();
    let r = page(
        &mut app,
        &c,
        "general-ledger",
        &format!("account=%20{account}%20&end=2028-02-29"),
    );
    assert_eq!(r.status, 200);
    assert!(r.html.contains(&format!("value=\"{account}\" selected")));
    assert!(r.html.contains("Cash &lt;&amp;&gt;"));
    let r = page(&mut app, &c, "general-ledger", "group=%20pl%20");
    assert_eq!(r.status, 400);
    let open = app.open_mut().unwrap();
    let other = fb_core::services::create_company(
        &mut open.book,
        open.owner,
        "Outside books",
        "USD",
        false,
    )
    .unwrap();
    let error = screens::render(
        &mut app,
        &format!("/c/{}/reports/general-ledger/?account={account}", other.id),
    )
    .err()
    .expect("the other company cannot select this account");
    assert!(
        matches!(error,fb_app::AppError::Core(fb_core::FbError::DoesNotExist(message)) if message=="Account matching query does not exist.")
    );
    let r = page(&mut app, &c, "general-ledger", "end=2028-02-29&page=2.0");
    assert_eq!(r.status, 200);
}

#[test]
fn reports_refuse_post_and_export_uses_a_strict_typed_command_without_mutation() {
    let (mut app, c, _) = fixture();
    let before = audit(&mut app);
    for route in ["profit-and-loss", "balance-sheet", "general-ledger"] {
        let path = format!("/c/{c}/reports/{route}/");
        let token = app.tokens.mint(&path);
        let out = app
            .run(Command::SubmitForm {
                path,
                token,
                fields: vec![],
            })
            .unwrap();
        assert!(matches!(out, Outcome::Page { status: 405, .. }));
    }
    let command = json!({"type":"ExportReport","company_id":c,"report":"balance_sheet","filters":[["as_of","2028-02-29"]]});
    let cmd = fb_app::commands::parse_command(&command.to_string()).unwrap();
    let out = app.run(cmd).unwrap();
    let Outcome::Export { filename, contents } = out else {
        panic!("{out:?}")
    };
    assert_eq!(filename, "balance-sheet-1-2028-02-29.csv");
    assert!(contents.contains("Row type,Section,Group,Account code,Account,Account ID,Amount\r\n"));
    let mut bad = command;
    bad["unexpected"] = Value::Bool(true);
    assert!(fb_app::commands::parse_command(&bad.to_string()).is_err());
    assert!(
        app.run(Command::ExportReport {
            company_id: c,
            report: "general_ledger".into(),
            filters: vec![("basis".into(), "bad".into())]
        })
        .is_err()
    );
    assert_eq!(audit(&mut app), before);
}


#[test]
fn print_capability_is_single_use_and_bound_to_exact_report_filters_without_audit_writes() {
    let (mut app,c,_) = fixture();
    app.now=||chrono::DateTime::parse_from_rfc3339("2028-02-29T17:15:00Z").unwrap().with_timezone(&chrono::Utc);
    let before=audit(&mut app);
    let target=format!("/c/{c}/reports/profit-and-loss/?start=2028-01-01&end=2028-02-29&output=print&literal=%24");
    let rendered=screens::render(&mut app,&target).unwrap();
    assert_eq!(rendered.status,200);
    assert!(rendered.html.contains("Printed 2028-02-29 17:15 UTC"));
    assert!(rendered.html.contains("report-paper"));
    let token=rendered.html.split_once("name=\"fb_command_token\" value=\"").unwrap().1.split('"').next().unwrap().to_owned();
    let cmd=|path:String,token:String|Command::SubmitForm{path,token,fields:vec![("action".into(),"desktop_print".into())]};
    assert!(app.run_renderer(cmd(target.replace("end=2028-02-29","end=2028-02-28"),token.clone())).is_err());
    // A token never authorizes a different filter set; the matching URL can still consume it.
    assert!(matches!(app.run_renderer(cmd(target.clone(),token)),Ok(Outcome::Done{..})));
    let rendered=screens::render(&mut app,&target).unwrap();
    let token=rendered.html.split_once("name=\"fb_command_token\" value=\"").unwrap().1.split('"').next().unwrap().to_owned();
    let result=app.run_renderer(cmd(target.clone(),token.clone())).unwrap();
    let Outcome::Done{data}=result else {panic!()};
    assert_eq!(data["desktop_print"],true);
    assert_eq!(data["target"],target);
    assert!(data["print_token"].as_str().unwrap().len()>20);
    assert!(app.run_renderer(cmd(target,token)).is_err());
    assert_eq!(audit(&mut app),before);
    let normal=format!("/c/{c}/reports/profit-and-loss/");
    let token=app.tokens.mint(&normal);
    assert!(app.run_renderer(cmd(normal,token)).is_err());
    assert_eq!(audit(&mut app),before);
    for report in ["profit-and-loss","balance-sheet","general-ledger"] {
        let r=page(&mut app,&c,report,"end=2028-02-29&as_of=2028-02-29&output=print");
        assert_eq!(r.status,200);
        assert!(r.html.contains("id=\"print-now\""));
    }
}
