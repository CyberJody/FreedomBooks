use fb_app::{App, Command, Outcome, keys, screens};

fn field(html: &str,name:&str)->String {
    html.split_once(&format!("name=\"{name}\" value=\"")).unwrap().1.split('"').next().unwrap().into()
}

#[test]
fn first_run_refuses_controls_before_trim_and_before_creating_the_book() {
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("test.fbbook");
    let mut app=App::new();let key=keys::new_master_key().unwrap();
    for (owner,company) in [("\nOwner","Company"),("Owner\u{202e}","Company"),("Owner","Company\r"),("Owner","\u{200b}Company")] {
        assert_eq!(app.create_book(&path,&key,owner,company).err().unwrap().to_string(),"Control and formatting characters are not allowed.");
        assert!(!path.exists());assert!(!app.is_open());
    }
    app.create_book(&path,&key,"Owner\tName","Valid company").unwrap();
    assert!(app.is_open());
}

#[test]
fn renderer_rejects_direct_mutations_and_requires_live_path_bound_tokens() {
    let dir=tempfile::tempdir().unwrap();let mut app=App::new();
    let c=app.create_book(&dir.path().join("test.fbbook"),&keys::new_master_key().unwrap(),"Owner","Company").unwrap().company_id.to_db();
    let cmd=Command::PostInvoice{company_id:c.clone(),invoice_id:uuid::Uuid::new_v4().to_string(),expected_revision:1};
    assert!(app.run_renderer(cmd).unwrap_err().to_string().contains("Unknown command"));
    let path=format!("/c/{c}/accounts/new/");
    let html=screens::render(&mut app,&path).unwrap().html;
    let token=field(&html,"fb_command_token");
    let fields=vec![("code".into(),"100".into()),("name".into(),"Bank".into()),("detail_type".into(),"BANK".into())];
    assert!(app.run_renderer(Command::SubmitForm{path:"/wrong/".into(),token:token.clone(),fields:fields.clone()}).is_err());
    let submit=Command::SubmitForm{path:path.clone(),token,fields};
    assert!(matches!(app.run_renderer(submit.clone()).unwrap(),Outcome::Redirect{..}));
    assert!(app.run_renderer(submit).is_err());
    let html=screens::render(&mut app,&path).unwrap().html;
    app.lock();
    assert!(app.run_renderer(Command::SubmitForm{path,token:field(&html,"fb_command_token"),fields:vec![]}).is_err());
}

#[test]
fn security_screen_fixtures_refuse_invisible_text_and_keep_field_errors() {
    let dir=tempfile::tempdir().unwrap();let mut app=App::new();
    let c=app.create_book(&dir.path().join("test.fbbook"),&keys::new_master_key().unwrap(),"Owner","Company").unwrap().company_id.to_db();
    // Dedicated Rust-only fixtures for the documented v1 security difference.
    for (suffix,field_name,value) in [
        ("accounts/new/","name","Account\u{202e}"),
        ("lists/customers/new/","display_name","Customer\u{200b}"),
        ("lists/vendors/new/","billing_address","Address\u{2066}"),
        ("journals/new/","memo","Memo\r"),
    ] {
        let path=format!("/c/{c}/{suffix}");
        let html=screens::render(&mut app,&path).unwrap().html;
        let before=fb_db::test_support::snapshot(app.open_mut().unwrap().book.connection()).unwrap();
        let outcome=app.run_renderer(Command::SubmitForm{path,token:field(&html,"fb_command_token"),fields:vec![(field_name.into(),value.into())]}).unwrap();
        let Outcome::Invalid{html}=outcome else {panic!("expected invalid screen")};
        assert!(html.contains("Control and formatting characters are not allowed."),"{suffix}");
        assert!(html.contains("aria-invalid=\"true\""),"{suffix}");
        assert_eq!(before,fb_db::test_support::snapshot(app.open_mut().unwrap().book.connection()).unwrap());
    }
}
