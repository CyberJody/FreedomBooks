use fb_core::{company_documents as documents, invoice_preferences as prefs, services, users};
use fb_db::Book;
use serde_json::json;

#[test]
fn preferences_are_company_scoped_atomic_audited_and_reject_stale_saves() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.fbbook");
    let key = [65;32];
    let mut book = Book::create(&path,&key).unwrap();
    let actor = users::create_user(&mut book,"Synthetic owner",true).unwrap();
    let other = users::create_user(&mut book,"Other owner",true).unwrap();
    let c = services::create_company(&mut book,actor,"Sample seller","USD",false).unwrap().id;
    let c2 = services::create_company(&mut book,other,"Other seller","USD",false).unwrap().id;
    let settings = prefs::Settings { zelle_enabled:true,zelle_recipient:"payments@example.test".into(),
        email_greeting:"Hi {customer_name},\r\nThank you.".into(),..Default::default() };
    let save = |book:&mut Book, actor, c, expected_revision, name| documents::save_with_preferences(book,actor,c,name,"USD",
        documents::Changes {details:documents::Details::default(),logo_upload:None,remove_logo:false},
        Some(prefs::Change {expected_revision,settings:settings.clone()}));
    assert!(save(&mut book,other,c,0,"Wrong owner").is_err());
    save(&mut book,actor,c,0,"Sample seller").unwrap();
    let stored = book.read(|q|prefs::get(q,c)).unwrap();
    assert_eq!(stored.revision,1);
    assert_eq!(stored.settings.email_greeting,"Hi {customer_name},\nThank you.");
    assert_eq!(book.read(|q|prefs::get(q,c2)).unwrap().revision,0);
    let before = fb_db::test_support::snapshot(book.connection()).unwrap();
    assert!(save(&mut book,actor,c,0,"Must roll back company rename").is_err());
    assert_eq!(fb_db::test_support::snapshot(book.connection()).unwrap(),before);
    save(&mut book,actor,c,1,"Sample seller").unwrap();
    assert_eq!(book.read(|q|prefs::get(q,c)).unwrap().revision,1,"unchanged preferences keep their revision");
    let count:i64 = book.connection().query_row("SELECT count(*) FROM ledger_auditevent WHERE action='company.invoice_preferences_updated'",[],|r|r.get(0)).unwrap();
    assert_eq!(count,1);
    assert_eq!(book.connection().query_row("SELECT count(*) FROM ledger_journal",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    drop(book);
    let mut book=Book::open(&path,&key).unwrap();
    assert_eq!(book.read(|q|prefs::get(q,c)).unwrap().settings,stored.settings);
    assert!(book.check().unwrap().is_empty());
}

#[test]
fn preferences_validate_multiline_text_recipients_and_closed_choices() {
    assert_eq!(prefs::Settings::default().email_mode,"DRAFT");
    assert_eq!(prefs::Settings::default().stripe_mode,"TEST");
    assert!(!prefs::Settings::default().stripe_enabled);
    for recipient in ["payments@example.test","+1 (518) 555-0123"] {
        assert!(prefs::clean(prefs::Settings{zelle_enabled:true,zelle_recipient:recipient.into(),..Default::default()}).is_ok());
    }
    for (field,value) in [("email_subject","Bad\r\nBcc: x@example.test"),("email_mode","AUTOSEND"),("stripe_mode","live"),("reply_to","wrong"),("footer","hidden\u{202e}text")] {
        let mut raw=serde_json::to_value(prefs::Settings::default()).unwrap();raw[field]=json!(value);
        assert!(prefs::clean(serde_json::from_value(raw).unwrap()).is_err(),"{field}");
    }
    assert!(prefs::clean(prefs::Settings{zelle_enabled:true,..Default::default()}).is_err());
}

#[test]
fn decoded_guards_reject_replacement_unknown_fields_and_detect_corruption_on_reopen() {
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("guard.fbbook");let key=[66;32];
    let mut book=Book::create(&path,&key).unwrap();let actor=users::create_user(&mut book,"Owner",true).unwrap();
    let c=services::create_company(&mut book,actor,"Synthetic","USD",false).unwrap().id;
    let raw=serde_json::to_string(&prefs::Settings::default()).unwrap();
    let q=book.connection();
    q.execute("INSERT INTO invoice_preferences(company_id,revision,data) VALUES(?1,1,?2)",[c.to_db(),raw.clone()]).unwrap();
    assert!(q.execute("INSERT OR REPLACE INTO invoice_preferences(company_id,revision,data) VALUES(?1,1,?2)",[c.to_db(),raw.clone()]).is_err());
    assert!(q.execute("DELETE FROM invoice_preferences",[]).is_err());
    for broken in [raw.replace("\"DRAFT\"","\"DRAFT\",\"email_mode\":\"SEND\""),raw.replace("\"footer\":\"\"","\"footer\":\"x\\u0000hidden\""),raw.replace("\"footer\":\"\"","\"unknown\":\"\""),raw.replace("\"zelle_enabled\":false","\"zelle_enabled\":1")] {
        assert!(q.execute("UPDATE invoice_preferences SET data=?1,revision=revision+1",[broken]).is_err());
    }
    let trigger:String=q.query_row("SELECT sql FROM sqlite_schema WHERE name='invoice_preferences_update'",[],|r|r.get(0)).unwrap();
    q.execute_batch("DROP TRIGGER invoice_preferences_update; PRAGMA ignore_check_constraints=ON;").unwrap();
    q.execute("UPDATE invoice_preferences SET data='{}'",[]).unwrap();
    q.execute_batch("PRAGMA ignore_check_constraints=OFF").unwrap();q.execute_batch(&trigger).unwrap();
    assert!(book.read(|q|prefs::get(q,c)).is_err());
    assert!(!book.check().unwrap().is_empty());drop(book);
    assert!(Book::open(&path,&key).is_err());
}

/// Populated identity matrix for 0188 (registered in tests/identity_permanence.rs): a company's
/// saved invoice preferences cannot move to another key, by UPDATE or UPDATE OR REPLACE.
#[test]
fn saved_preferences_identity_is_permanent() {
    let file = fb_db::test_support::TempBookPath::new("preferences-identity");
    let mut book = file.book(&[67; 32]);
    let actor = users::create_user(&mut book, "Synthetic owner", true).unwrap();
    let c = services::create_company(&mut book, actor, "Sample seller", "USD", false).unwrap().id;
    let other = services::create_company(&mut book, actor, "Other seller", "USD", false).unwrap().id;
    for company in [c, other] {
        documents::save_with_preferences(&mut book, actor, company, "Sample seller", "USD",
            documents::Changes { details: documents::Details::default(), logo_upload: None, remove_logo: false },
            Some(prefs::Change { expected_revision: 0, settings: prefs::Settings { zelle_enabled: true, zelle_recipient: "payments@example.test".into(), ..Default::default() } })).unwrap();
    }
    assert_eq!(book.connection().query_row("SELECT count(*) FROM invoice_preferences", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
    let accepted = fb_db::test_support::primary_key_changes_accepted(book.connection(), &["invoice_preferences"]);
    assert!(accepted.is_empty(), "accepted primary-key changes: {accepted:#?}");
    assert!(book.check().unwrap().is_empty());
}
