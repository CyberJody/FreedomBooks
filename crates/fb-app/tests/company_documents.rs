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

#[test]
fn invoice_preferences_form_roundtrips_and_stale_submit_preserves_company_details() {
    let dir=tempfile::tempdir().unwrap();let mut app=App::new();
    let c=app.create_book(&dir.path().join("preferences.fbbook"),&fb_app::keys::new_master_key().unwrap(),"Owner","Synthetic seller").unwrap().company_id;
    support::sign_in(&mut app);
    let path=format!("/c/{c}/edit/");
    let html=screens::render(&mut app,&path).unwrap().html;
    assert!(html.contains("Save to Gmail Drafts") && html.contains("invoice_zelle_recipient"));
    let settings=fb_core::invoice_preferences::Settings { zelle_enabled:true,zelle_recipient:"billing@example.test".into(),
        email_greeting:"Hi <recipient>,\nThank you & co.".into(),..Default::default() };
    let mut fields=vec![("name".into(),"Synthetic seller".into()),("currency".into(),"USD".into()),
        ("invoice_preferences_present".into(),"1".into()),("invoice_preferences_revision".into(),"0".into())];
    for (key,value) in serde_json::to_value(settings).unwrap().as_object().unwrap() {
        if value==true {fields.push((format!("invoice_{key}"),"on".into()));}
        else if let Some(text)=value.as_str() {fields.push((format!("invoice_{key}"),text.to_owned()));}
    }
    let submit=|app:&mut App,fields|{let token=app.tokens.mint(&path);app.run(Command::SubmitForm{path:path.clone(),token,fields}).unwrap()};
    assert!(matches!(submit(&mut app,fields.clone()),Outcome::Redirect{..}));
    let html=screens::render(&mut app,&path).unwrap().html;
    assert!(html.contains("Hi &lt;recipient&gt;,\nThank you &amp; co."));
    let before=fb_db::test_support::snapshot(app.open_mut().unwrap().book.connection()).unwrap();
    fields[0].1="Must not change the name".into();
    let Outcome::Invalid{html}=submit(&mut app,fields.clone()) else {panic!("stale settings accepted")};
    assert!(html.contains("Invoice preferences changed."));
    assert!(html.contains("Hi &lt;recipient&gt;,\nThank you &amp; co."),"failed form retains entered values");
    assert_eq!(fb_db::test_support::snapshot(app.open_mut().unwrap().book.connection()).unwrap(),before);
    fields.iter_mut().find(|p|p.0=="invoice_preferences_revision").unwrap().1="1".into();
    fields.iter_mut().find(|p|p.0=="invoice_zelle_recipient").unwrap().1="invalid".into();
    assert!(matches!(submit(&mut app,fields),Outcome::Invalid{..}));
    assert_eq!(fb_db::test_support::snapshot(app.open_mut().unwrap().book.connection()).unwrap(),before);
}
