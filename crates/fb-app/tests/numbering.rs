mod support;
use fb_app::{App, Command, Outcome, Query, commands::parse_command, keys, screens};
use serde_json::{Value, json};

fn fixture() -> (tempfile::TempDir, App, String) {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new();
    let c = app
        .create_book(
            &dir.path().join("books.fbbook"),
            &keys::new_master_key().unwrap(),
            "Numbering Owner",
            "Synthetic numbers",
        )
        .unwrap()
        .company_id
        .to_db();
    support::sign_in(&mut app);
    (dir, app, c)
}
fn field(html: &str, name: &str) -> String {
    html.split_once(&format!("name=\"{name}\" value=\""))
        .unwrap()
        .1
        .split('"')
        .next()
        .unwrap()
        .to_owned()
}
fn form<'a>(html: &'a str, kind: &str) -> &'a str {
    html.split("<form method=\"post\">")
        .find(|f| f.contains(&format!("name=\"kind\" value=\"{kind}\"")))
        .unwrap()
        .split("</form>")
        .next()
        .unwrap()
}
fn fields(html: &str, kind: &str, next: &str) -> Vec<(String, String)> {
    let f = form(html, kind);
    let mut fields: Vec<_> = [
        "action",
        "kind",
        "bank_id",
        "expected_next",
        "expected_revision",
    ]
    .into_iter()
    .map(|n| (n.into(), field(f, n)))
    .collect();
    fields.push(("next_number".into(), next.into()));
    fields
}

#[test]
fn settings_row_validation_save_collision_staleness_and_history_are_real_form_flows() {
    let (_dir, mut app, c) = fixture();
    let path = format!("/c/{c}/edit/");
    let html = screens::render(&mut app, &path).unwrap().html;
    assert!(html.contains("Document numbering"));
    assert!(html.contains("INV-000001"));
    assert!(html.contains("Designate a bank account"));
    let invalid = app
        .run(Command::SubmitForm {
            path: path.clone(),
            token: field(&html, "fb_command_token"),
            fields: fields(&html, "INV", "0"),
        })
        .unwrap();
    let Outcome::Invalid { html } = invalid else {
        panic!("{invalid:?}")
    };
    assert!(html.contains("Enter a positive whole number"));
    assert!(form(&html, "INV").contains("value=\"0\" aria-invalid=\"true\""));
    let saved = app
        .run(Command::SubmitForm {
            path: path.clone(),
            token: field(&html, "fb_command_token"),
            fields: fields(&html, "INV", "100"),
        })
        .unwrap();
    assert_eq!(
        saved,
        Outcome::Redirect {
            location: format!("{path}#document-numbering")
        }
    );
    let html = screens::render(&mut app, &path).unwrap().html;
    assert!(html.contains("INV-000100"));
    assert!(html.contains("Next number saved."));
    let open = app.open_mut().unwrap();
    let actor = open.owner;
    let co = fb_core::CompanyId::parse(&c).unwrap();
    let existing = open
        .book
        .write(|q| {
            fb_core::documents::create_identity(q, actor, co, "INV", &fb_ids::new_v7().to_string())
        })
        .unwrap();
    // Reusing the pre-allocation form is refused even though the settings revision is unchanged.
    let stale = app
        .run(Command::SubmitForm {
            path: path.clone(),
            token: field(&html, "fb_command_token"),
            fields: fields(&html, "INV", "200"),
        })
        .unwrap();
    let Outcome::Invalid { html } = stale else {
        panic!("{stale:?}")
    };
    assert!(html.contains("changed since"));
    let collision = app
        .run(Command::SubmitForm {
            path: path.clone(),
            token: field(&html, "fb_command_token"),
            fields: fields(&html, "INV", "100"),
        })
        .unwrap();
    let Outcome::Invalid { html } = collision else {
        panic!("{collision:?}")
    };
    assert!(html.contains(existing["number"].as_str().unwrap()));
    let rows = app
        .query(Query::NumberingSettings {
            company_id: c.clone(),
        })
        .unwrap();
    assert_eq!(
        rows.as_array()
            .unwrap()
            .iter()
            .find(|r| r["kind"] == "INV")
            .unwrap()["next"],
        "101"
    );
    let history = screens::render(&mut app, &format!("/c/{c}/history/"))
        .unwrap()
        .html;
    assert!(history.contains("Next document number changed"));
    assert!(history.contains("Invoices: 1 → 100"));
    assert!(history.contains("Numbering Owner"));
    let count: i64 = app
        .open_mut()
        .unwrap()
        .book
        .read(|q| {
            q.query_row(
                "SELECT count(*) FROM ledger_auditevent WHERE action='numbering.changed'",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn typed_numbering_command_refuses_unknown_fields_and_cross_company_bank() {
    let (_dir, mut app, c) = fixture();
    let payload = json!({"type":"SetNextNumber","company_id":c,"kind":"INV","bank_id":null,"next":"300","expected_next":"1","expected_revision":0});
    assert!(matches!(
        app.run(parse_command(&payload.to_string()).unwrap())
            .unwrap(),
        Outcome::Done { .. }
    ));
    let mut bad = payload;
    bad["ignore_collisions"] = Value::Bool(true);
    assert!(parse_command(&bad.to_string()).is_err());
    let payload = json!({"type":"SetNextNumber","company_id":c,"kind":"CHECK","bank_id":fb_ids::new_v7().to_string(),"next":"100","expected_next":"1","expected_revision":0});
    assert!(
        app.run(parse_command(&payload.to_string()).unwrap())
            .is_err()
    );
}
