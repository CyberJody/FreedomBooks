//! Real encrypted-book reconciliation boundaries: save/resume, stale tabs, zero difference and undo.
mod support;
use fb_app::{App, Command, Outcome, commands::parse_command, keys, screens};
use serde_json::{Value, json};
fn run(app: &mut App, v: Value) -> Value {
    match app.run(parse_command(&v.to_string()).unwrap()).unwrap() {
        Outcome::Done { data } => data,
        o => panic!("{o:?}"),
    }
}
fn field(html: &str, key: &str) -> String {
    html.split_once(&format!("name=\"{key}\" value=\""))
        .unwrap()
        .1
        .split('"')
        .next()
        .unwrap()
        .into()
}
fn fields(v: Value) -> Vec<(String, String)> {
    v.as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().into()))
        .collect()
}
fn post(app: &mut App, path: &str, p: Vec<(String, String)>) -> Outcome {
    let html = screens::render(app, path).unwrap().html;
    app.run(Command::SubmitForm {
        path: path.into(),
        token: field(&html, "fb_command_token"),
        fields: p,
    })
    .unwrap()
}
struct Fx {
    app: App,
    c: String,
    bank: String,
    other_bank: String,
    dir: std::path::PathBuf,
}
impl Drop for Fx {
    fn drop(&mut self) {
        self.app.lock();
        std::fs::remove_dir_all(&self.dir).unwrap();
    }
}
fn fixture() -> Fx {
    let dir = std::env::temp_dir().join(format!("fb-d7b-{}", fb_ids::new_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new();
    app.today = || chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
    let c = app
        .create_book(
            &dir.join("book.fbbook"),
            &keys::new_master_key().unwrap(),
            "owner",
            "D7b synthetic",
        )
        .unwrap()
        .company_id
        .to_db();
    support::sign_in(&mut app);
    let mut acc = |code, detail| {
        run(&mut app,json!({"type":"SaveAccount","company_id":c,"code":code,"name":code,"detail_type":detail}))["id"].as_str().unwrap().to_owned()
    };
    let bank = acc("1000", "BANK");
    let other_bank = acc("1010", "BANK");
    let income = acc("4000", "INCOME");
    let expense = acc("5000", "EXPENSE");
    for (debit, credit, n, date) in [
        (&bank, &income, "1000.00", "2026-10-01"),
        (&expense, &bank, "200.00", "2026-10-15"),
        (&bank, &income, "50.00", "2026-12-01"),
    ] {
        let j = run(
            &mut app,
            json!({"type":"SaveJournal","company_id":c,"journal_id":null,"date":date,"memo":"Synthetic statement","entry_number":"","adjusting":false,"request_key":null,"expected_revision":null,"lines":[{"account":debit,"debit":n,"credit":"0.00","description":"","name":"Statement payee"},{"account":credit,"debit":"0.00","credit":n,"description":"","name":""}]}),
        );
        run(
            &mut app,
            json!({"type":"PostJournal","company_id":c,"journal_id":j["id"],"expected_revision":1}),
        );
    }
    Fx {
        app,
        c,
        bank,
        other_bank,
        dir,
    }
}
fn begin(f: &mut Fx) -> String {
    let path = format!("/c/{}/reconcile/{}/begin/", f.c, f.bank);
    let html = screens::render(&mut f.app, &path).unwrap().html;
    let result = post(
        &mut f.app,
        &path,
        fields(
            json!({"request_key":field(&html,"request_key"),"statement_date":"2026-10-31","ending_balance":"800.00"}),
        ),
    );
    let Outcome::Redirect { location } = result else {
        panic!("{result:?}")
    };
    location
}
fn rec_fields(html: &str, action: &str) -> Vec<(String, String)> {
    let mut p = vec![
        ("action".into(), action.into()),
        ("options".into(), "1".into()),
        ("hide_later".into(), "1".into()),
    ];
    for k in ["reconciliation", "revision", "shown", "completion_key"] {
        p.push((k.into(), field(html, k)));
    }
    p
}
fn tick_all(html: &str, p: &mut Vec<(String, String)>) {
    for row in field(html, "shown").split(',') {
        p.push(("tick".into(), row.split(':').next().unwrap().into()));
    }
}
#[test]
fn banking_exports_use_single_use_path_bound_submit_forms() {
    let mut f = fixture();
    let path = format!("/c/{}/accounts/{}/register/", f.c, f.bank);
    let html = screens::render(&mut f.app, &path).unwrap().html;
    let token = field(html.split_once(">CSV").unwrap().1, "fb_command_token");
    let command = Command::SubmitForm {
        path: path.clone(), token: token.clone(), fields: vec![],
    };
    let before = fb_db::test_support::snapshot(f.app.open_mut().unwrap().book.connection()).unwrap();
    assert!(f.app.run_renderer(Command::SubmitForm {
        path: format!("/c/{}/accounts/{}/register/", f.c, f.other_bank),
        token, fields: vec![],
    }).is_err());
    let Outcome::Export { contents, .. } = f.app.run_renderer(command.clone()).unwrap() else {
        panic!("expected CSV export")
    };
    assert!(contents.contains("Report,Register"));
    assert!(f.app.run_renderer(command).is_err());
    assert!(f.app.run_renderer(Command::ExportBankRegister {
        company_id: f.c.clone(), account_id: f.bank.clone(), filters: vec![],
    }).is_err());
    assert_eq!(before, fb_db::test_support::snapshot(f.app.open_mut().unwrap().book.connection()).unwrap());
}

#[test]
fn undo_slices_revision_before_parsing_and_refuses_without_any_writes() {
    let mut f=fixture();
    let begin_path=format!("/c/{}/reconcile/{}/begin/",f.c,f.other_bank);
    let html=screens::render(&mut f.app,&begin_path).unwrap().html;
    let result=post(&mut f.app,&begin_path,fields(json!({"request_key":field(&html,"request_key"),"statement_date":"2026-10-31","ending_balance":"0.00"})));
    let Outcome::Redirect { location }=result else {panic!("begin failed")};
    let html=screens::render(&mut f.app,&location).unwrap().html;
    assert!(matches!(post(&mut f.app,&location,rec_fields(&html,"reconcile")),Outcome::Redirect {..}));
    let path=format!("/c/{}/reconcile/{}/undo/",f.c,f.other_bank);
    let html=screens::render(&mut f.app,&path).unwrap().html;
    assert_eq!(field(&html,"revision"),"2");
    let before=fb_db::test_support::snapshot(f.app.open_mut().unwrap().book.connection()).unwrap();
    for (revision, reason, message) in [
        ("0000000000002", "Synthetic correction", "This reconciliation changed after you opened it."),
        ("abc", "Synthetic correction", "Reload the page and try again."),
        ("", "Synthetic correction", "Reload the page and try again."),
        ("-1", "Synthetic correction", "Reload the page and try again."),
        ("1000000001", "Synthetic correction", "Reload the page and try again."),
        ("abc", "", "Reload the page and try again."),
        ("-1", "", "Reload the page and try again."),
    ] {
        let command=Command::SubmitForm {path:path.clone(),token:f.app.tokens.mint(&path),fields:fields(json!({"reconciliation":field(&html,"reconciliation"),"revision":revision,"undo_key":field(&html,"undo_key"),"reason":reason}))};
        let outcome=f.app.run_renderer(command).unwrap();
        assert_eq!(before,fb_db::test_support::snapshot(f.app.open_mut().unwrap().book.connection()).unwrap());
        let Outcome::Invalid {html}=outcome else {panic!("invalid revision admitted: {revision}")};
        assert!(html.contains(message), "{revision}: wrong validation message");
        assert_eq!(field(&html,"revision"),fb_core::py::py_truncate(revision,12));
    }
}
#[test]
fn begin_leave_resume_finish_report_and_undo() {
    let mut f = fixture();
    let path = begin(&mut f);
    let before=fb_db::test_support::snapshot(f.app.open_mut().unwrap().book.connection()).unwrap();
    for reconciliation in ["abc","","-1","1000000000000000001"] {
        let token=f.app.tokens.mint(&path);
        let result=f.app.run(Command::SubmitForm {path:path.clone(),token,fields:fields(json!({"action":"save","reconciliation":reconciliation,"revision":"abc","shown":"bad"}))}).unwrap();
        let Outcome::Invalid {html}=result else {panic!("malformed record ID accepted")};
        assert!(html.contains("Choose Save, Leave or Reconcile now."));
        assert_eq!(before,fb_db::test_support::snapshot(f.app.open_mut().unwrap().book.connection()).unwrap());
    }
    let html = screens::render(&mut f.app, &path).unwrap().html;
    let report_path=format!("/c/{}/reconciliations/{}/?view=bad&output=bad",f.c,field(&html,"reconciliation"));
    assert_eq!(screens::render(&mut f.app,&report_path).unwrap().status,302,"OPEN report redirects before choice validation");
    assert_eq!(
        field(&html, "shown").split(',').count(),
        2,
        "later line stays hidden"
    );
    let mut p = rec_fields(&html, "leave");
    tick_all(&html, &mut p);
    assert!(
        matches!(post(&mut f.app,&path,p),Outcome::Redirect{location} if location.contains("?left="))
    );
    let html = screens::render(&mut f.app, &path).unwrap().html;
    assert!(html.contains("id=\"difference\">0.00"));
    assert!(!html.contains("id=\"reconcile-now\" disabled"));
    let mut p = rec_fields(&html, "reconcile");
    tick_all(&html, &mut p);
    let retry = p.clone();
    let Outcome::Redirect { location } = post(&mut f.app, &path, p) else {
        panic!("finish failed")
    };
    let report = screens::render(&mut f.app, &location).unwrap();
    assert_eq!(report.status, 200);
    assert!(report.html.contains("id=\"report-ending\">800.00"));
    assert!(report.html.contains("These lines now show R"));
    // Exact completion retry works after the screen GET redirects to Begin.
    let token = f.app.tokens.mint(&path);
    let result = f
        .app
        .run(Command::SubmitForm {
            path: path.clone(),
            token,
            fields: retry.clone(),
        })
        .unwrap();
    assert!(matches!(result, Outcome::Redirect { .. }));
    // Even a completed retry validates membership before NotOpen/completion.
    let mut malformed=retry;
    malformed.iter_mut().find(|(k,_)|k=="shown").unwrap().1=String::new();
    let before=fb_db::test_support::snapshot(f.app.open_mut().unwrap().book.connection()).unwrap();
    let token=f.app.tokens.mint(&path);
    let result=f.app.run(Command::SubmitForm {path:path.clone(),token,fields:malformed}).unwrap();
    let Outcome::Invalid {html}=result else {panic!("completed retry bypassed tick validation")};
    assert!(html.contains("Reload the reconciliation and try again."));
    assert_eq!(before,fb_db::test_support::snapshot(f.app.open_mut().unwrap().book.connection()).unwrap());
    let undo = format!("/c/{}/reconcile/{}/undo/", f.c, f.bank);
    let html = screens::render(&mut f.app, &undo).unwrap().html;
    let p = fields(
        json!({"reconciliation":field(&html,"reconciliation"),"revision":field(&html,"revision"),"undo_key":field(&html,"undo_key"),"reason":"Synthetic correction"}),
    );
    assert!(
        matches!(post(&mut f.app,&undo,p),Outcome::Redirect{location} if location.contains("?undone="))
    );
    let o = f.app.open_mut().unwrap();
    let state: Vec<String> = o
        .book
        .read(|q| {
            q.prepare("SELECT state FROM banking_banklinestatus ORDER BY id")?
                .query_map([], |r| r.get(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .unwrap();
    assert_eq!(state, vec!["C", "C"]);
}
#[test]
fn stale_finish_without_tick_changes_is_refused() {
    let mut f = fixture();
    let path = begin(&mut f);
    let html = screens::render(&mut f.app, &path).unwrap().html;
    let mut p = rec_fields(&html, "save");
    tick_all(&html, &mut p);
    post(&mut f.app, &path, p);
    let current = screens::render(&mut f.app, &path).unwrap().html;
    let mut stale = rec_fields(&current, "reconcile");
    tick_all(&current, &mut stale);
    stale.iter_mut().find(|(k, _)| k == "revision").unwrap().1 = "1".into();
    let Outcome::Invalid { html } = post(&mut f.app, &path, stale) else {
        panic!("stale finish accepted")
    };
    assert!(html.contains("Reload it and check the difference again."));
    let rec = field(&current, "reconciliation");
    let o = f.app.open_mut().unwrap();
    assert_eq!(
        o.book
            .read(|q| q.query_row(
                "SELECT status FROM banking_reconciliation WHERE id=?1",
                [rec],
                |r| r.get::<_, String>(0)
            ))
            .unwrap(),
        "OPEN"
    );
}
#[test]
fn mismatch_keeps_ticks_and_statement_open() {
    let mut f = fixture();
    let path = begin(&mut f);
    let html = screens::render(&mut f.app, &path).unwrap().html;
    let mut p = rec_fields(&html, "reconcile");
    let first = field(&html, "shown")
        .split(',')
        .next()
        .unwrap()
        .split(':')
        .next()
        .unwrap()
        .to_owned();
    p.push(("tick".into(), first));
    let Outcome::Invalid { html } = post(&mut f.app, &path, p) else {
        panic!("unbalanced reconciliation finished")
    };
    assert!(html.contains("not 0.00. Tick exactly"));
    assert_eq!(field(&html, "revision"), "2");
}
#[test]
fn register_mark_is_account_scoped_and_gets_do_not_write() {
    let mut f = fixture();
    let path = begin(&mut f);
    let html = screens::render(&mut f.app, &path).unwrap().html;
    let line = field(&html, "shown")
        .split(',')
        .next()
        .unwrap()
        .split(':')
        .next()
        .unwrap()
        .to_owned();
    let c = f.c.clone();
    let bank = f.other_bank.clone();
    let result=f.app.run(parse_command(&json!({"type":"MarkBankLine","company_id":c,"account_id":bank,"line_id":line,"state":"C","expected_revision":0}).to_string()).unwrap());
    assert!(result.is_err());
    let before = {
        let o = f.app.open_mut().unwrap();
        o.book
            .read(|q| {
                q.query_row("SELECT count(*) FROM ledger_auditevent", [], |r| {
                    r.get::<_, i64>(0)
                })
            })
            .unwrap()
    };
    for path in [
        format!("/c/{}/registers/", f.c),
        format!("/c/{}/reconcile/", f.c),
        format!("/c/{}/accounts/{}/register/", f.c, f.bank),
        path,
    ] {
        assert_eq!(screens::render(&mut f.app, &path).unwrap().status, 200);
    }
    let o = f.app.open_mut().unwrap();
    assert_eq!(
        before,
        o.book
            .read(
                |q| q.query_row("SELECT count(*) FROM ledger_auditevent", [], |r| r
                    .get::<_, i64>(0))
            )
            .unwrap()
    );
}
#[test]
fn filter_grammar_and_field_then_cross_field_errors() {
    let mut f = fixture();
    let root = format!("/c/{}/accounts/{}/register/", f.c, f.bank);
    for (query, msg) in [
        (
            "amount=1.234",
            "Enter a positive amount with at most two decimal places.",
        ),
        (
            "amount=NaN",
            "Enter a positive amount with at most two decimal places.",
        ),
        ("amount=bad", "Enter an amount such as 125.00."),
        ("preset=%20this_month%20", "Select a valid choice."),
        (
            "start=2026-10-31&end=2026-10-01",
            "The &quot;From&quot; date must not be after the &quot;Through&quot; date.",
        ),
    ] {
        let p = screens::render(&mut f.app, &format!("{root}?{query}")).unwrap();
        assert_eq!(p.status, 400, "{query}");
        assert!(p.html.contains(msg), "{query}: {}", p.html);
    }
    let p = screens::render(
        &mut f.app,
        &format!("{root}?start=2026-10-31&end=2026-10-01&amount=bad"),
    )
    .unwrap();
    assert!(!p.html.contains("must not be after"));
    let p = screens::render(&mut f.app, &format!("{root}?amount=%241%2C000.000")).unwrap();
    assert_eq!(p.status, 200);
    assert!(p.html.contains("value=\"1000.00\""));
}

#[test]
fn csv_export_preserves_signed_money_and_escapes_user_text() {
    let mut f = fixture();
    let debit = {
        let o = f.app.open_mut().unwrap();
        o.book
            .read(|q| {
                q.query_row(
                    "SELECT id FROM ledger_account WHERE company_id=?1 AND code='5000'",
                    [&f.c],
                    |r| r.get::<_, String>(0),
                )
            })
            .unwrap()
    };
    let j = run(
        &mut f.app,
        json!({"type":"SaveJournal","company_id":f.c,"journal_id":null,"date":"2026-10-01","memo":"=SUM(1,2)","entry_number":"","adjusting":false,"request_key":null,"expected_revision":null,"lines":[{"account":debit,"debit":"10.00","credit":"0.00","description":"","name":""},{"account":f.other_bank,"debit":"0.00","credit":"10.00","description":"","name":"-123"}]}),
    );
    run(
        &mut f.app,
        json!({"type":"PostJournal","company_id":f.c,"journal_id":j["id"],"expected_revision":1}),
    );
    let cmd = json!({"type":"ExportBankRegister","company_id":f.c,"account_id":f.other_bank,"filters":[]});
    let Outcome::Export { filename, contents } =
        f.app.run(parse_command(&cmd.to_string()).unwrap()).unwrap()
    else {
        panic!("CSV did not use native export")
    };
    assert_eq!(filename, "bank-register.csv");
    assert!(contents.contains("Ending balance,-10.00\r\n"));
    assert!(contents.contains("'-123"));
    assert!(contents.contains("\"'=SUM(1,2)\""));
    assert!(!contents.contains("'-10.00"));
    let path = begin(&mut f);
    let html = screens::render(&mut f.app, &path).unwrap().html;
    let mut p = rec_fields(&html, "reconcile");
    tick_all(&html, &mut p);
    assert!(
        matches!(post(&mut f.app,&path,p),Outcome::Redirect{location} if location.contains("?done=1"))
    );
    let rec = field(&html, "reconciliation");
    let cmd = json!({"type":"ExportReconciliation","company_id":f.c,"reconciliation_id":rec,"filters":[["view","summary"]]});
    let Outcome::Export { contents, .. } =
        f.app.run(parse_command(&cmd.to_string()).unwrap()).unwrap()
    else {
        panic!("CSV did not use native export")
    };
    assert!(contents.contains("Cleared checks and payments (−),-200.00\r\n"));
}
