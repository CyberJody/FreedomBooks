use fb_app::{App,Command,Outcome,commands::StarterChart,keys,screens};

fn post(app:&mut App,path:&str,fields:Vec<(String,String)>)->Outcome {
    let token=app.tokens.mint(path);
    app.run(Command::SubmitForm{path:path.into(),token,fields}).unwrap()
}
fn count(app:&mut App,table:&str)->i64 {
    app.open_mut().unwrap().book.read(|tx|tx.query_row(&format!("SELECT count(*) FROM {table}"),[],|r|r.get(0))).unwrap()
}
fn field(html:&str,name:&str)->String {
    html.split_once(&format!("name=\"{name}\" value=\"" )).unwrap().1.split('"').next().unwrap().into()
}

#[test]
fn chart_creation_document_details_and_numbering_share_real_company_forms_without_erasure() {
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("combined.fbbook");let key=keys::new_master_key().unwrap();let mut app=App::new();
    let c=app.create_book_with_chart(&path,&key,"Owner","First seller",StarterChart::Standard).unwrap().company_id.to_db();
    assert_eq!(count(&mut app,"ledger_account"),34);
    let new=screens::render(&mut app,"/companies/new/").unwrap().html;
    assert!(new.contains("starter_chart") && !new.contains("id_payment_instructions"));
    let company_fields=vec![("name".into(),"Second seller".into()),("currency".into(),"USD".into()),("starter_chart".into(),"standard".into())];
    assert!(matches!(post(&mut app,"/companies/new/",company_fields),Outcome::Redirect{..}));
    assert_eq!(count(&mut app,"ledger_account"),68);
    let edit=format!("/c/{c}/edit/");let html=screens::render(&mut app,&edit).unwrap().html;
    assert!(html.contains("id_payment_instructions") && html.contains("Document numbering") && !html.contains("starter_chart"));
    let values=vec![("name".into(),"Renamed seller".into()),("currency".into(),"USD".into()),("street1".into(),"Preserved address".into()),("payment_instructions".into(),"Pay by check\nThank you".into())];
    assert!(matches!(post(&mut app,&edit,values),Outcome::Redirect{..}));
    let html=screens::render(&mut app,&edit).unwrap().html;
    let number_form=html.split("<form method=\"post\">").find(|f|f.contains("name=\"kind\" value=\"INV\"" )).unwrap().split("</form>").next().unwrap();
    let mut number_fields:Vec<(String,String)>=["action","kind","bank_id","expected_next","expected_revision"].into_iter().map(|n|(n.into(),field(number_form,n))).collect();
    number_fields.push(("next_number".into(),"100".into()));
    assert!(matches!(post(&mut app,&edit,number_fields),Outcome::Redirect{..}));
    let html=screens::render(&mut app,&edit).unwrap().html;
    assert!(html.contains("Preserved address") && html.contains("Pay by check\nThank you") && html.contains("INV-000100"));
    assert_eq!(count(&mut app,"ledger_account"),68);
    assert_eq!(count(&mut app,"company_document_details"),1);assert_eq!(count(&mut app,"fb_numbering"),1);
    app.lock();app.open_book(&path,&key).unwrap();
    let html=screens::render(&mut app,&edit).unwrap().html;
    assert!(html.contains("Preserved address") && html.contains("INV-000100"));
    assert_eq!(count(&mut app,"ledger_account"),68);
}
