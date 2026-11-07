use fb_app::{App, Command, Outcome, keys::MasterKey, screens};
use fb_core::{bankfeeds as bf, services, store};
use serde_json::json;

fn fixture() -> (tempfile::TempDir, App, String, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new();
    app.simplefin = true;
    let c = app
        .create_book(
            &dir.path().join("test.fbbook"),
            &MasterKey::new([63; 32]),
            "Owner",
            "Mapping test",
        )
        .unwrap()
        .company_id;
    let o = app.open_mut().unwrap();
    let conn = bf::connect(
        &mut o.book,
        o.owner,
        c,
        &json!("Synthetic bank"),
        "bridge.simplefin.org",
        &bf::SecretSet::none(),
    )
    .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let accounts = ["Checking", "Savings", "Card"]
        .into_iter()
        .map(|name| bf::StagedAccount {
            conn_key: "synthetic-connection".into(),
            remote_id: name.into(),
            name: format!("Bank {name}"),
            org_name: "Test bank".into(),
            currency: "USD".into(),
            balance: None,
            available: None,
            balance_epoch: None,
            skipped: vec![],
            transactions: vec![],
        })
        .collect();
    bf::store_pull(
        &mut o.book,
        o.owner,
        c,
        &conn,
        &bf::StagedPull {
            accounts,
            skipped: vec![],
            errors: vec![],
        },
        "2026-11-07T12:00:00Z",
        false,
        None,
        &bf::SecretSet::none(),
    )
    .unwrap();
    let feeds = o
        .book
        .read(|q| {
            let mut st = q.prepare("SELECT id FROM bankfeeds_account ORDER BY name")?;
            st.query_map([], |r| r.get(0))?
                .collect::<Result<Vec<String>, _>>()
        })
        .unwrap();
    (
        dir,
        app,
        format!("/c/{c}/bank-feeds/{conn}/accounts/"),
        feeds,
    )
}

fn post(app: &mut App, path: &str, fields: Vec<(String, String)>) -> Outcome {
    let html = screens::render(app, path).unwrap().html;
    let token = html
        .split_once("id=\"bankfeed-mapping\"")
        .unwrap()
        .1
        .split_once("name=\"fb_command_token\" value=\"")
        .unwrap()
        .1
        .split('"')
        .next()
        .unwrap()
        .to_owned();
    app.run_renderer(Command::SubmitForm {
        path: path.into(),
        token,
        fields,
    })
    .unwrap()
}

#[test]
fn mapping_offers_editable_suggested_names_and_creates_all_three_types() {
    let (_dir, mut app, path, feeds) = fixture();
    let html = screens::render(&mut app, &path).unwrap().html;
    assert!(html.contains("Add new account"));
    assert!(html.contains("Bank Checking"));
    let mut fields = vec![];
    for (feed, kind) in feeds.iter().zip(["CREDIT_CARD", "CHECKING", "SAVINGS"]) {
        fields.extend([
            (format!("map_{feed}"), "__new__".into()),
            (format!("name_{feed}"), format!("Edited {kind}")),
            (format!("type_{feed}"), kind.into()),
        ]);
    }
    assert!(matches!(
        post(&mut app, &path, fields),
        Outcome::Redirect { .. }
    ));
    let c = app.first_company().unwrap().unwrap();
    let o = app.open_mut().unwrap();
    let accounts = o.book.read(|q| store::accounts(q, c)).unwrap();
    assert_eq!(accounts.len(), 3);
    for a in accounts {
        assert_eq!(
            a.detail_type.as_str(),
            if a.name.ends_with("CREDIT_CARD") {
                "CREDIT_CARD"
            } else {
                "BANK"
            }
        );
        let mapped: i64 = o
            .book
            .read(|q| {
                q.query_row(
                    "SELECT count(*) FROM bankfeeds_account WHERE ledger_account_id=?1",
                    [a.id.to_db()],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(mapped, 1);
    }
}

#[test]
fn invalid_new_account_keeps_other_choices_and_does_not_create_any_accounts() {
    let (_dir, mut app, path, feeds) = fixture();
    let c = app.first_company().unwrap().unwrap();
    let o = app.open_mut().unwrap();
    let existing = services::save_account(
        &mut o.book,
        o.owner,
        c,
        services::AccountInput::new("1000", "Existing").detail("BANK"),
    )
    .unwrap();
    let fields = vec![
        (format!("map_{}", feeds[0]), existing.id.to_db()),
        (format!("map_{}", feeds[1]), "__new__".into()),
        (format!("name_{}", feeds[1]), "Edited checking".into()),
        (format!("type_{}", feeds[1]), "INCOME".into()),
    ];
    let Outcome::Invalid { html } = post(&mut app, &path, fields) else {
        panic!("invalid type accepted")
    };
    assert!(html.contains("Edited checking"));
    assert!(html.contains(&format!("value=\"{}\" selected", existing.id)));
    assert_eq!(
        app.open_mut()
            .unwrap()
            .book
            .read(|q| store::accounts(q, c))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn connection_instructions_name_the_bridge_app_button() {
    let (_dir, mut app, _, _) = fixture();
    let c = app.first_company().unwrap().unwrap();
    let html = screens::render(&mut app, &format!("/c/{c}/bank-feeds/connect/"))
        .unwrap()
        .html;
    assert!(html.contains("Apps"));
    assert!(html.contains("New app connection"));
}
