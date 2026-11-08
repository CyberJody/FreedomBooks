mod support;
use fb_app::{App, Command, Outcome, commands::parse_command, screens};
use serde_json::json;

#[test]
fn company_document_settings_form_validation_tokens_and_escaping() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new();
    let key = fb_app::keys::new_master_key().unwrap();
    let c = app
        .create_book(
            &dir.path().join("synthetic.fbbook"),
            &key,
            "Owner",
            "Synthetic seller",
        )
        .unwrap()
        .company_id
        .to_db();
    support::sign_in(&mut app);
    let path = format!("/c/{c}/edit/");
    let html = screens::render(&mut app, &path).unwrap().html;
    assert!(
        html.contains("128 KB")
            && html.contains("id_payment_instructions")
            && html.contains("id_tax_id")
    );
    let submit = |app: &mut App, fields: Vec<(String, String)>| {
        let token = app.tokens.mint(&path);
        app.run(Command::SubmitForm {
            path: path.clone(),
            token,
            fields,
        })
        .unwrap()
    };
    let base = vec![
        ("name".into(), "Synthetic seller".into()),
        ("currency".into(), "USD".into()),
    ];
    let mut invalid = base.clone();
    invalid.push(("email".into(), "invalid".into()));
    let out = submit(&mut app, invalid);
    let Outcome::Invalid { html } = out else {
        panic!("invalid email accepted");
    };
    assert!(html.contains("Enter a valid email address."));
    assert!(!html.contains("ValidationError:"));
    let o = app.open_mut().unwrap();
    assert_eq!(
        o.book
            .read(|tx| fb_core::company_documents::get(tx, c.parse().unwrap()))
            .unwrap()
            .email,
        ""
    );
    let mut fields = base.clone();
    fields.extend([
        ("street1".into(), "<address> & co.".into()),
        ("payment_instructions".into(), "By check\nThank you".into()),
    ]);
    assert!(matches!(submit(&mut app, fields), Outcome::Redirect { .. }));
    let html = screens::render(&mut app, &path).unwrap().html;
    assert!(html.contains("&lt;address&gt; &amp; co."));
    assert!(html.contains("By check\nThank you"));
    assert!(
        app.run(Command::SubmitForm {
            path: path.clone(),
            token: "spent".into(),
            fields: base
        })
        .is_err()
    );
    let unknown = json!({"type":"SaveCompanyDetails","company_id":c,"name":"x","currency":"USD","values":{"unknown":"x"}});
    assert!(parse_command(&unknown.to_string()).is_err());
}
