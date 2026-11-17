mod support;
use chrono::NaiveDate;
use fb_app::{App, Outcome, commands::parse_command, identity::Role, keys, screens};
use fb_core::{
    CompanyId,
    directory::{self, Model},
    payables::recurring,
    services::{self, AccountInput},
};
use serde_json::{Value, json};
struct Fx {
    _dir: tempfile::TempDir,
    app: App,
    c: CompanyId,
    values: Value,
}
fn fixture() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new();
    app.today = || NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
    let c = app
        .create_book(
            &dir.path().join("synthetic.fbbook"),
            &keys::new_master_key().unwrap(),
            "Synthetic owner",
            "Synthetic recurring company",
        )
        .unwrap()
        .company_id;
    support::sign_in(&mut app);
    let o = app.open_mut().unwrap();
    let expense = services::save_account(
        &mut o.book,
        o.owner,
        c,
        AccountInput::new("6000", "Synthetic expense").detail("EXPENSE"),
    )
    .unwrap()
    .id;
    let vendor = directory::save_record(
        &mut o.book,
        o.owner,
        c,
        Model::Vendor,
        &json!({"display_name":"Synthetic supplier <&>"}),
        None,
    )
    .unwrap()["id"]
        .clone();
    let values = json!({"id":null,"request_key":fb_core::JournalId::new().to_db(),"expected_revision":0,"name":"Monthly rent <&>","vendor_id":vendor,"memo":"Synthetic recurring memo","terms_days":30,"frequency":"MONTHLY","start_date":"2026-09-30","end_date":null,"occurrence_limit":3,"active":true,"auto_post":false,"confirmation":{"dates":["2026-09-30","2026-10-30"],"auto_post":false},"lines":[{"item_id":null,"expense_account_id":expense.to_db(),"description":"Synthetic rent","quantity":"1","rate":"12.34","origin_position":null,"refresh_item":false,"keep_archived":false}]});
    Fx {
        _dir: dir,
        app,
        c,
        values,
    }
}
fn save(f: &mut Fx) -> Value {
    let through = (f.app.today)().to_string();
    let open = f.app.open_mut().unwrap();
    f.values["confirmation"] =
        recurring::save_confirmation(&mut open.book, open.owner, f.c, &f.values, &through).unwrap()
            ["token"]
            .clone();
    match f
        .app
        .run(
            parse_command(
                &json!({"type":"SaveRecurringBill","company_id":f.c.to_db(),"values":f.values})
                    .to_string(),
            )
            .unwrap(),
        )
        .unwrap()
    {
        Outcome::Done { data } => data,
        o => panic!("{o:?}"),
    }
}
fn page(f: &mut Fx, suffix: &str) -> String {
    fb_app::recurring_bills::desktop_tick(&mut f.app).unwrap();
    screens::render(&mut f.app, &format!("/c/{}/{suffix}", f.c))
        .unwrap()
        .html
}
fn count(f: &mut Fx) -> i64 {
    f.app
        .open
        .as_ref()
        .unwrap()
        .book
        .connection()
        .query_row("SELECT count(*) FROM payables_bill", [], |r| r.get(0))
        .unwrap()
}
fn hidden(html: &str, name: &str) -> String {
    html.split_once(&format!("name=\"{name}\" value=\""))
        .unwrap()
        .1
        .split('"')
        .next()
        .unwrap()
        .replace("&#34;", "\"")
        .replace("&quot;", "\"")
}
#[test]
fn screens_generate_missed_drafts_show_upcoming_and_origin_and_copy_existing_bill() {
    let mut f = fixture();
    let t = save(&mut f);
    let html = page(&mut f, "recurring-bills/");
    assert!(html.contains("Monthly rent &lt;&amp;&gt;"));
    assert!(html.contains("2026-11-30"));
    assert!(html.contains("Recurring bills"));
    assert_eq!(count(&mut f), 2);
    let html = page(
        &mut f,
        &format!("recurring-bills/{}/", t["id"].as_str().unwrap()),
    );
    assert_eq!(html.matches("data-recurring-occurrence").count(), 2);
    assert!(html.contains("2026-11-30"));
    let bid = {
        let o = f.app.open_mut().unwrap();
        let t = recurring::detail(&mut o.book, o.owner, f.c, t["id"].as_str().unwrap()).unwrap();
        t["occurrences"][0]["bill_id"].as_str().unwrap().to_owned()
    };
    let bill = page(&mut f, &format!("bills/{bid}/"));
    assert!(bill.contains("Recurring bill template"));
    assert!(bill.contains("Make recurring"));
    let form = page(&mut f, &format!("recurring-bills/new/?bill={bid}"));
    assert!(form.contains("value=\"12.34\""));
    assert!(form.contains("Synthetic recurring memo"));
    assert!(form.contains("value=\"2026-11-30\""));
    assert!(form.contains("value=\"MONTHLY\" selected"));
    assert_eq!(count(&mut f), 2);
    let history = page(&mut f, "history/");
    assert!(history.contains("Recurring bill template created"));
    assert!(history.contains(&format!("/recurring-bills/{}/", t["id"].as_str().unwrap())));
    let edit = page(
        &mut f,
        &format!("recurring-bills/{}/edit/", t["id"].as_str().unwrap()),
    );
    assert!(edit.contains("value=\"12.34\""));
    assert!(edit.contains("Start date and frequency cannot change"));
}
#[test]
fn readonly_views_do_not_generate_and_all_write_paths_are_refused() {
    let mut f = fixture();
    let t = save(&mut f);
    let other = {
        let o = f.app.open_mut().unwrap();
        services::create_company(&mut o.book, o.owner, "Private company", "USD", false)
            .unwrap()
            .id
    };
    f.app
        .save_desktop_user(
            None,
            "Synthetic viewer",
            Role::Readonly,
            true,
            Some("SyntheticViewerPassword42"),
            &[f.c],
        )
        .unwrap();
    f.app
        .sign_in("Synthetic viewer", "SyntheticViewerPassword42")
        .unwrap();
    fb_app::recurring_bills::desktop_tick(&mut f.app).unwrap();
    let html = page(&mut f, "recurring-bills/");
    assert_eq!(count(&mut f), 0);
    assert!(!html.contains(">New recurring bill</a>"));
    let html = page(
        &mut f,
        &format!("recurring-bills/{}/", t["id"].as_str().unwrap()),
    );
    assert!(!html.contains(">Pause</button>"));
    assert!(
        f.app
            .run(
                parse_command(
                    &json!({"type":"SaveRecurringBill","company_id":f.c.to_db(),"values":f.values})
                        .to_string()
                )
                .unwrap()
            )
            .is_err()
    );
    assert!(
        screens::submit(
            &mut f.app,
            &format!("/c/{}/recurring-bills/{}/", f.c, t["id"].as_str().unwrap()),
            &[
                ("action".into(), "delete".into()),
                ("expected_revision".into(), "1".into())
            ]
        )
        .is_err()
    );
    assert!(screens::render(&mut f.app, &format!("/c/{other}/recurring-bills/")).is_err());
    assert_eq!(count(&mut f), 0);
}
#[test]
fn trusted_tick_requires_sign_in_and_uses_the_authenticated_operator() {
    let mut f = fixture();
    save(&mut f);
    f.app.sign_out();
    fb_app::recurring_bills::desktop_tick(&mut f.app).unwrap();
    assert_eq!(count(&mut f), 0);
    support::sign_in(&mut f.app);
    fb_app::recurring_bills::desktop_tick(&mut f.app).unwrap();
    assert_eq!(count(&mut f), 2);
    let o = f.app.open_mut().unwrap();
    let actors: Vec<String> = o
        .book
        .connection()
        .prepare("SELECT actor_id FROM ledger_auditevent WHERE action='recurring_bill.generated'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(actors, vec![o.owner.to_db(); 2]);
}

fn posted(v: &Value) -> Vec<(String, String)> {
    let mut fields = vec![];
    for key in [
        "request_key",
        "expected_revision",
        "name",
        "memo",
        "terms_days",
        "frequency",
        "start_date",
        "end_date",
        "occurrence_limit",
        "source_bill_id",
    ] {
        if !v[key].is_null() {
            fields.push((
                key.into(),
                v[key]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| v[key].to_string()),
            ));
        }
    }
    fields.push(("vendor".into(), v["vendor_id"].as_str().unwrap().into()));
    for key in ["active", "auto_post"] {
        if v[key] == true {
            fields.push((key.into(), "on".into()));
        }
    }
    fields.push(("lines-TOTAL_FORMS".into(), "1".into()));
    for (form, key) in [
        ("description", "description"),
        ("quantity", "quantity"),
        ("rate", "rate"),
        ("expense_account", "expense_account_id"),
    ] {
        fields.push((
            format!("lines-0-{form}"),
            v["lines"][0][key].as_str().unwrap().into(),
        ));
    }
    fields
}
#[test]
fn past_date_save_and_earlier_edit_confirm_dates_before_any_generation() {
    for editing in [false, true] {
        let mut f = fixture();
        let new_path = format!("/c/{}/recurring-bills/new/", f.c);
        let path = if editing {
            f.values["start_date"] = json!("2027-09-30");
            let t = save(&mut f);
            f.values["id"] = t["id"].clone();
            f.values["expected_revision"] = json!(1);
            format!(
                "/c/{}/recurring-bills/{}/edit/",
                f.c,
                t["id"].as_str().unwrap()
            )
        } else {
            new_path
        };
        f.values["start_date"] = json!("2026-09-30");
        f.values["auto_post"] = json!(true);
        let mut fields = posted(&f.values);
        let out = screens::submit(&mut f.app, &path, &fields).unwrap();
        let Outcome::Page { html, status } = out else {
            panic!("{out:?}")
        };
        assert_eq!(status, 200);
        assert!(html.contains("Confirm 2 pending bills"));
        assert!(html.contains("2026-09-30") && html.contains("2026-10-30"));
        assert!(html.contains("They will post automatically"));
        fb_app::recurring_bills::desktop_tick(&mut f.app).unwrap();
        assert_eq!(count(&mut f), 0);
        fields.push(("confirmation".into(), hidden(&html, "confirmation")));
        assert!(matches!(
            screens::submit(&mut f.app, &path, &fields).unwrap(),
            Outcome::Redirect { .. }
        ));
        assert_eq!(count(&mut f), 0);
        fb_app::recurring_bills::desktop_tick(&mut f.app).unwrap();
        assert_eq!(count(&mut f), 2);
    }
}
#[test]
fn resume_and_delete_prompts_preserve_state_until_a_choice_and_history_shows_skips() {
    for skip in [true, false] {
        let mut f = fixture();
        f.values["active"] = json!(false);
        let t = save(&mut f);
        let path = format!("/c/{}/recurring-bills/{}/", f.c, t["id"].as_str().unwrap());
        let paused = page(
            &mut f,
            &format!("recurring-bills/{}/", t["id"].as_str().unwrap()),
        );
        assert!(paused.contains("3 scheduled dates (including skipped dates)"));
        assert!(!paused.contains("up to 3 bills"));
        assert!(paused.contains("Scheduled dates (paused)"));
        assert_eq!(paused.matches("Missed — choose on Resume").count(), 2);
        assert_eq!(paused.matches("Upcoming (paused)").count(), 1);
        assert!(page(&mut f, "recurring-bills/").contains("Missed — choose on Resume"));
        let fields = vec![
            ("expected_revision".into(), "1".into()),
            ("action".into(), "resume".into()),
        ];
        let Outcome::Page { html, .. } = screens::submit(&mut f.app, &path, &fields).unwrap()
        else {
            panic!("resume must ask")
        };
        assert!(html.contains("2 missed bill dates"));
        assert!(html.contains("2026-09-30"));
        assert!(html.contains("2026-10-30"));
        fb_app::recurring_bills::desktop_tick(&mut f.app).unwrap();
        assert_eq!(count(&mut f), 0);
        let choice = vec![
            ("expected_revision".into(), "1".into()),
            (
                "action".into(),
                if skip {
                    "resume_skip".into()
                } else {
                    "resume_create".into()
                },
            ),
            ("confirmation".into(), hidden(&html, "confirmation")),
        ];
        screens::submit(&mut f.app, &path, &choice).unwrap();
        let html = page(
            &mut f,
            &format!("recurring-bills/{}/", t["id"].as_str().unwrap()),
        );
        assert_eq!(count(&mut f), if skip { 0 } else { 2 });
        assert_eq!(html.matches("data-recurring-occurrence").count(), 2);
        if skip {
            assert_eq!(html.matches("Skipped</td>").count(), 2);
        }
        let delete = vec![
            ("expected_revision".into(), "2".into()),
            ("action".into(), "delete".into()),
        ];
        let Outcome::Page { html, .. } = screens::submit(&mut f.app, &path, &delete).unwrap()
        else {
            panic!("delete must ask")
        };
        assert!(html.contains("Confirm delete template"));
        let o = f.app.open_mut().unwrap();
        assert_eq!(
            recurring::detail(&mut o.book, o.owner, f.c, t["id"].as_str().unwrap()).unwrap()["deleted"],
            0
        );
        let mut delete = delete;
        delete.push(("confirm_delete".into(), "on".into()));
        screens::submit(&mut f.app, &path, &delete).unwrap();
    }
}
#[test]
fn dismissed_pending_notice_stays_dismissed_and_readonly_cannot_make_recurring() {
    let mut f = fixture();
    save(&mut f);
    {
        let o = f.app.open_mut().unwrap();
        o.book
            .write(|tx| -> fb_core::FbResult<()> {
                tx.execute(
                    "UPDATE directory_vendor SET active=0 WHERE id=?",
                    [f.values["vendor_id"].as_str().unwrap()],
                )?;
                Ok(())
            })
            .unwrap();
    }
    fb_app::recurring_bills::desktop_tick(&mut f.app).unwrap();
    assert_eq!(f.app.messages.len(), 1);
    f.app.messages.clear();
    fb_app::recurring_bills::desktop_tick(&mut f.app).unwrap();
    assert!(f.app.messages.is_empty());
    {
        let o = f.app.open_mut().unwrap();
        o.book
            .write(|tx| -> fb_core::FbResult<()> {
                tx.execute(
                    "UPDATE directory_vendor SET active=1 WHERE id=?",
                    [f.values["vendor_id"].as_str().unwrap()],
                )?;
                Ok(())
            })
            .unwrap();
    }
    fb_app::recurring_bills::desktop_tick(&mut f.app).unwrap();
    let bid: String = f
        .app
        .open
        .as_ref()
        .unwrap()
        .book
        .connection()
        .query_row("SELECT id FROM payables_bill LIMIT 1", [], |r| r.get(0))
        .unwrap();
    f.app
        .save_desktop_user(
            None,
            "Synthetic viewer",
            Role::Readonly,
            true,
            Some("SyntheticViewerPassword42"),
            &[f.c],
        )
        .unwrap();
    f.app
        .sign_in("Synthetic viewer", "SyntheticViewerPassword42")
        .unwrap();
    assert!(!page(&mut f, &format!("bills/{bid}/")).contains(">Make recurring</a>"));
}

#[test]
fn source_period_warning_names_dates_and_requires_explicit_confirmation() {
    let mut f = fixture();
    let open = f.app.open_mut().unwrap();
    let source = fb_core::payables::save_bill(&mut open.book,open.owner,f.c,
        &json!({"request_key":fb_core::JournalId::new().to_db(),"vendor_id":f.values["vendor_id"],"date":"2026-11-15","due_date":"2026-12-15","memo":"Synthetic source","reference":"","lines":f.values["lines"]})).unwrap();
    let source_id = source["id"].as_str().unwrap();
    let form = page(&mut f, &format!("recurring-bills/new/?bill={source_id}"));
    assert!(form.contains("value=\"2026-12-15\""));
    let path = format!("/c/{}/recurring-bills/new/", f.c);
    f.values["source_bill_id"] = source["id"].clone();
    for (frequency, first, warning, active) in [
        ("MONTHLY", "2026-12-01", true, true),
        ("MONTHLY", "2026-12-15", false, true),
        ("YEARLY", "2027-11-01", true, true),
        ("YEARLY", "2027-11-15", false, true),
        ("MONTHLY", "2026-11-20", true, true),
        ("MONTHLY", "2026-12-01", true, false),
    ] {
        f.values["request_key"] = json!(fb_core::JournalId::new().to_db());
        f.values["frequency"] = json!(frequency);
        f.values["start_date"] = json!(first);
        f.values["active"] = json!(active);
        let mut fields = posted(&f.values);
        let outcome = screens::submit(&mut f.app, &path, &fields).unwrap();
        if warning {
            let Outcome::Page { html, .. } = outcome else {
                panic!("warning required: {outcome:?}")
            };
            assert!(html.contains("data-recurring-source-warning"));
            assert!(html.contains(first) && html.contains("2026-11-15"));
            assert!(html.contains("might duplicate"));
            assert!(html.contains("Confirm and save recurring bill"));
            assert_eq!(count(&mut f), 1);
            fields.push(("confirmation".into(), hidden(&html, "confirmation")));
            assert!(matches!(
                screens::submit(&mut f.app, &path, &fields).unwrap(),
                Outcome::Redirect { .. }
            ));
        } else {
            assert!(matches!(outcome, Outcome::Redirect { .. }));
        }
        assert_eq!(count(&mut f), 1);
    }
    for frequency in ["MONTHLY", "YEARLY"] {
        f.values["frequency"] = json!(frequency);
        for invalid in ["2026-11-15", "2026-11-14"] {
            f.values["request_key"] = json!(fb_core::JournalId::new().to_db());
            f.values["start_date"] = json!(invalid);
            let Outcome::Invalid { html } =
                screens::submit(&mut f.app, &path, &posted(&f.values)).unwrap()
            else {
                panic!("the form must return with the source-date rule explained")
            };
            assert!(html.contains("must be after the source bill date"));
        }
    }
    assert_eq!(count(&mut f), 1);
}

#[test]
fn stale_resume_choices_refresh_dates_and_revision_instead_of_retrying() {
    for action in ["resume_skip", "resume_create"] {
        for edit in [false, true] {
            let mut f = fixture();
            f.values["active"] = json!(false);
            let t = save(&mut f);
            let path = format!("/c/{}/recurring-bills/{}/", f.c, t["id"].as_str().unwrap());
            let Outcome::Page { html, .. } = screens::submit(
                &mut f.app,
                &path,
                &[
                    ("action".into(), "resume".into()),
                    ("expected_revision".into(), "1".into()),
                ],
            )
            .unwrap() else {
                panic!("prompt")
            };
            let stale = vec![
                ("action".into(), action.into()),
                ("expected_revision".into(), "1".into()),
                ("confirmation".into(), hidden(&html, "confirmation")),
            ];
            f.app.today = || NaiveDate::from_ymd_opt(2026, 11, 30).unwrap();
            if edit {
                f.values["id"] = t["id"].clone();
                f.values["expected_revision"] = json!(1);
                f.values["name"] = json!("Updated paused template");
                save(&mut f);
            }
            let Outcome::Page { html, status } =
                screens::submit(&mut f.app, &path, &stale).unwrap()
            else {
                panic!("fresh prompt, not Retry")
            };
            assert_eq!(status, 200);
            assert!(html.contains("3 missed bill dates"));
            assert!(html.contains("2026-11-30"));
            assert_ne!(hidden(&html, "confirmation"), stale[2].1);
            assert_eq!(
                hidden(&html, "expected_revision"),
                if edit { "2" } else { "1" }
            );
            fb_app::recurring_bills::desktop_tick(&mut f.app).unwrap();
            assert_eq!(count(&mut f), 0);
            let fresh = vec![
                ("action".into(), action.into()),
                (
                    "expected_revision".into(),
                    hidden(&html, "expected_revision"),
                ),
                ("confirmation".into(), hidden(&html, "confirmation")),
            ];
            assert!(matches!(
                screens::submit(&mut f.app, &path, &fresh).unwrap(),
                Outcome::Redirect { .. }
            ));
            fb_app::recurring_bills::desktop_tick(&mut f.app).unwrap();
            assert_eq!(count(&mut f), if action == "resume_skip" { 0 } else { 3 });
            assert!(matches!(
                screens::submit(&mut f.app, &path, &stale).unwrap(),
                Outcome::Redirect { .. }
            ));
        }
    }
}

#[test]
fn ancient_first_dates_are_refused_on_the_form_and_long_pauses_skip_only_the_excess() {
    let mut f = fixture();
    f.values["start_date"] = json!("1850-01-31");
    f.values["occurrence_limit"] = Value::Null;
    let new_path = format!("/c/{}/recurring-bills/new/", f.c);
    let Outcome::Invalid { html } =
        screens::submit(&mut f.app, &new_path, &posted(&f.values)).unwrap()
    else {
        panic!("the form must return with the limit explained")
    };
    assert!(html.contains("cannot be more than 24 months before today"));
    assert!(!html.contains("data-recurring-save-confirmation"));
    let open = f.app.open_mut().unwrap();
    assert!(
        recurring::list(&mut open.book, open.owner, f.c)
            .unwrap()
            .is_empty()
    );
    // Paused within the limit, then left for years: 43 dates are missed by 2028-06-15.
    f.values["start_date"] = json!("2024-11-30");
    f.values["active"] = json!(false);
    let t = save(&mut f);
    let path = format!("/c/{}/recurring-bills/{}/", f.c, t["id"].as_str().unwrap());
    f.app.today = || NaiveDate::from_ymd_opt(2028, 6, 15).unwrap();
    let submit = |f: &mut Fx, action: &str, revision: &str, confirmation: Option<String>| {
        let mut fields = vec![
            ("action".into(), action.into()),
            ("expected_revision".into(), revision.into()),
        ];
        fields.extend(confirmation.map(|token| ("confirmation".into(), token)));
        screens::submit(&mut f.app, &path, &fields).unwrap()
    };
    let Outcome::Page { html, .. } = submit(&mut f, "resume", "1", None) else {
        panic!("prompt")
    };
    assert!(html.contains("43 missed bill dates"));
    assert!(html.contains("only the newest 36 missed dates can be created"));
    assert!(html.contains("2024-11-30") && html.contains("2025-05-30"));
    assert!(!html.contains("2025-06-30"));
    assert!(html.contains("Skip these 7 dates and stay paused"));
    assert!(html.contains("whether to skip or create the remaining 36"));
    assert!(!html.contains("resume_create"));
    let token = hidden(&html, "confirmation");
    assert_eq!(token.len(), 66);
    // Posting the hidden create choice anyway only shows the same prompt again.
    for (action, confirmation) in [
        ("resume_create", token.clone()),
        ("resume_skip", json!("tampered").to_string()),
    ] {
        let Outcome::Page { html, .. } = submit(&mut f, action, "1", Some(confirmation)) else {
            panic!("prompt again")
        };
        assert!(html.contains("43 missed bill dates"));
        assert!(!html.contains("resume_create"));
    }
    let skipped = |f: &mut Fx| -> (i64, i64) {
        f.app
            .open
            .as_ref()
            .unwrap()
            .book
            .connection()
            .query_row(
                "SELECT (SELECT count(*) FROM payables_recurring_occurrence WHERE skipped=1),(SELECT active FROM payables_recurring)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
    };
    assert_eq!(skipped(&mut f), (0, 0));
    assert!(matches!(
        submit(&mut f, "resume_skip", "1", Some(token)),
        Outcome::Redirect { .. }
    ));
    assert_eq!(skipped(&mut f), (7, 0));
    fb_app::recurring_bills::desktop_tick(&mut f.app).unwrap();
    assert_eq!(count(&mut f), 0);
    let Outcome::Page { html, .. } = submit(&mut f, "resume", "2", None) else {
        panic!("prompt for the rest")
    };
    assert!(html.contains("36 missed bill dates"));
    assert!(html.contains("resume_create") && html.contains("resume_skip"));
    assert!(html.contains("2025-06-30") && html.contains("2028-05-30"));
    assert!(matches!(
        submit(
            &mut f,
            "resume_create",
            "2",
            Some(hidden(&html, "confirmation"))
        ),
        Outcome::Redirect { .. }
    ));
    assert_eq!(skipped(&mut f), (7, 1));
    fb_app::recurring_bills::desktop_tick(&mut f.app).unwrap();
    assert_eq!(count(&mut f), 36);
}

/// The route the AI connection uses: no screen filters the request first.
fn command(f: &mut Fx, command: Value) -> Result<Outcome, String> {
    f.app
        .run(parse_command(&command.to_string()).unwrap())
        .map_err(|e| e.to_string())
}
fn control(f: &mut Fx, t: &Value, revision: i64, action: &str, token: &Value) -> Result<Outcome, String> {
    let c = f.c.to_db();
    command(
        f,
        json!({"type":"ControlRecurringBill","company_id":c,"template_id":t["id"],"expected_revision":revision,"action":action,"confirmation":token,"confirm_delete":action=="delete"}),
    )
}
fn template_state(f: &mut Fx) -> Value {
    let row: (i64, i64, i64, i64, i64, i64) = f
        .app
        .open
        .as_ref()
        .unwrap()
        .book
        .connection()
        .query_row(
            "SELECT count(*),coalesce(sum(active),0),coalesce(sum(deleted),0),coalesce(sum(revision),0),coalesce(sum(next_index),0),(SELECT count(*) FROM payables_recurring_occurrence) FROM payables_recurring",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .unwrap();
    json!({"templates":row.0,"active":row.1,"deleted":row.2,"revision":row.3,"next_index":row.4,"occurrences":row.5})
}

#[test]
fn command_route_refuses_a_first_date_25_months_back_and_writes_nothing() {
    let mut f = fixture();
    f.values["start_date"] = json!("2024-10-15");
    f.values["occurrence_limit"] = Value::Null;
    f.values["confirmation"] = Value::Null;
    for active in [false, true] {
        f.values["active"] = json!(active);
        let c = f.c.to_db();
        let values = f.values.clone();
        let refused = command(
            &mut f,
            json!({"type":"SaveRecurringBill","company_id":c,"values":values}),
        )
        .unwrap_err();
        assert!(
            refused.contains("cannot be more than 24 months before today"),
            "{refused}"
        );
    }
    assert_eq!(template_state(&mut f)["templates"], 0);
    assert_eq!(count(&mut f), 0);
    // Exactly 24 months back is accepted on the same route.
    f.values["start_date"] = json!("2024-11-15");
    f.values["active"] = json!(false);
    save(&mut f);
    assert_eq!(template_state(&mut f)["templates"], 1);
}

#[test]
fn command_route_resume_with_37_missed_skips_only_one_then_creates_exactly_36() {
    let mut f = fixture();
    f.values["start_date"] = json!("2024-11-30");
    f.values["occurrence_limit"] = Value::Null;
    f.values["active"] = json!(false);
    let t = save(&mut f);
    // Dates 2024-11-30 through 2027-11-30 are missed: 37.
    f.app.today = || NaiveDate::from_ymd_opt(2027, 11, 30).unwrap();
    let preview = |f: &mut Fx| {
        let o = f.app.open_mut().unwrap();
        let stored = recurring::detail(&mut o.book, o.owner, f.c, t["id"].as_str().unwrap()).unwrap();
        recurring::due_confirmation(&stored, "2027-11-30").unwrap()
    };
    let first = preview(&mut f);
    assert_eq!((&first["total"], &first["count"], &first["more"]), (&json!(37), &json!(1), &json!(true)));
    let before = template_state(&mut f);
    let refused = control(&mut f, &t, 1, "resume_create", &first["token"]).unwrap_err();
    assert!(refused.contains("Skip the oldest 1 first"), "{refused}");
    assert!(control(&mut f, &t, 1, "resume", &Value::Null).is_err());
    assert!(control(&mut f, &t, 1, "resume_skip", &json!("0".repeat(64))).is_err());
    assert_eq!(template_state(&mut f), before);
    fb_app::recurring_bills::desktop_tick(&mut f.app).unwrap();
    assert_eq!(count(&mut f), 0);
    // Only the one excess date is skipped; the template stays paused.
    control(&mut f, &t, 1, "resume_skip", &first["token"]).unwrap();
    assert_eq!(
        template_state(&mut f),
        json!({"templates":1,"active":0,"deleted":0,"revision":2,"next_index":1,"occurrences":1})
    );
    assert!(control(&mut f, &t, 2, "resume_create", &first["token"]).is_err());
    // Exactly 36 remain: the create choice is accepted and one check creates all 36.
    let rest = preview(&mut f);
    assert_eq!((&rest["total"], &rest["count"], &rest["more"]), (&json!(36), &json!(36), &json!(false)));
    control(&mut f, &t, 2, "resume_create", &rest["token"]).unwrap();
    assert_eq!(template_state(&mut f)["active"], 1);
    fb_app::recurring_bills::desktop_tick(&mut f.app).unwrap();
    assert_eq!(count(&mut f), 36);
    assert!(f.app.messages.is_empty(), "{:?}", f.app.messages);
    fb_app::recurring_bills::desktop_tick(&mut f.app).unwrap();
    assert_eq!(count(&mut f), 36);
    assert_eq!(template_state(&mut f)["next_index"], 37);
}

#[test]
fn active_template_37_overdue_gives_one_notice_is_marked_stalled_and_can_be_deleted() {
    let mut f = fixture();
    f.values["start_date"] = json!("2026-11-30");
    f.values["occurrence_limit"] = Value::Null;
    let t = save(&mut f);
    let tid = t["id"].as_str().unwrap().to_owned();
    fb_app::recurring_bills::desktop_tick(&mut f.app).unwrap();
    assert_eq!(count(&mut f), 0);
    assert!(f.app.messages.is_empty());
    let list = page(&mut f, "recurring-bills/");
    assert!(!list.contains("data-recurring-stalled"));
    // The books stay closed for three years: 2026-11-30 through 2029-11-30 is 37 dates.
    f.app.today = || NaiveDate::from_ymd_opt(2029, 11, 30).unwrap();
    let before = template_state(&mut f);
    for _ in 0..2 {
        fb_app::recurring_bills::desktop_tick(&mut f.app).unwrap();
        assert_eq!(f.app.messages.len(), 1, "{:?}", f.app.messages);
    }
    assert!(f.app.messages[0].contains("More than 36 bill dates are overdue, so none were created"));
    assert!(f.app.messages[0].contains("The oldest dates beyond the newest 36 must be skipped"));
    assert!(!f.app.messages[0].contains("to skip or create them"));
    assert_eq!(count(&mut f), 0);
    assert_eq!(template_state(&mut f), before);
    // Once the notice is dismissed, the list and detail pages still mark the stall.
    f.app.messages.clear();
    let list = page(&mut f, "recurring-bills/");
    assert!(list.contains("data-recurring-stalled"));
    assert!(list.contains("more than 36 bill dates are overdue"));
    let detail = page(&mut f, &format!("recurring-bills/{tid}/"));
    assert!(detail.contains("data-recurring-stalled"));
    assert!(detail.contains("Overdue — Pause, then Resume"));
    assert!(f.app.messages.is_empty());
    assert_eq!(count(&mut f), 0);
    // An active template over the limit cannot be edited, but it can be deleted.
    let c = f.c.to_db();
    let mut edit = f.values.clone();
    edit["id"] = t["id"].clone();
    edit["expected_revision"] = json!(1);
    edit["confirmation"] = Value::Null;
    let refused = command(
        &mut f,
        json!({"type":"SaveRecurringBill","company_id":c,"values":edit}),
    )
    .unwrap_err();
    assert!(refused.contains("more than 36 missed bill dates"), "{refused}");
    control(&mut f, &t, 1, "delete", &Value::Null).unwrap();
    assert_eq!(
        template_state(&mut f),
        json!({"templates":1,"active":0,"deleted":1,"revision":2,"next_index":0,"occurrences":0})
    );
    fb_app::recurring_bills::desktop_tick(&mut f.app).unwrap();
    assert_eq!(count(&mut f), 0);
    assert!(!page(&mut f, "recurring-bills/").contains("data-recurring-stalled"));
}
