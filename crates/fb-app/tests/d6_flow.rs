//! D6 form and review boundaries against a real encrypted book.
use fb_app::{App, Command, Outcome, commands::parse_command, keys, screens};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
static N: AtomicUsize = AtomicUsize::new(0);
fn command(app:&mut App,v:Value)->Value {match app.run(parse_command(&v.to_string()).unwrap()).unwrap(){Outcome::Done{data}=>data,v=>panic!("{v:?}")}}
fn field(html:&str,name:&str)->String{html.split_once(&format!("name=\"{name}\" value=\"")).unwrap().1.split('"').next().unwrap().into()}
fn submit(app:&mut App,path:&str,values:Vec<(&str,String)>)->Outcome{
    let html=screens::render(app,path).unwrap().html;
    app.run(Command::SubmitForm{path:path.into(),token:field(&html,"fb_command_token"),fields:values.into_iter().map(|(k,v)|(k.into(),v)).collect()}).unwrap()
}
fn account(app:&mut App,c:&str,code:&str,detail:&str)->String{command(app,json!({"type":"SaveAccount","company_id":c,"code":code,"name":code,"detail_type":detail}))["id"].as_str().unwrap().into()}
fn fixture()->(App,String,String,String,String,String){
    let path=std::env::temp_dir().join(format!("fb-d6-{}-{}",std::process::id(),N.fetch_add(1,Ordering::SeqCst)));std::fs::create_dir_all(&path).unwrap();
    let mut app=App::new();app.today=||chrono::NaiveDate::from_ymd_opt(2026,11,1).unwrap();
    let c=app.create_book(&path.join("book.fbbook"),&keys::new_master_key().unwrap(),"owner","D6 test books").unwrap().company_id.to_db();
    let bank=account(&mut app,&c,"1000","BANK");let savings=account(&mut app,&c,"1010","BANK");let ap=account(&mut app,&c,"2000","PAYABLE");let expense=account(&mut app,&c,"5000","EXPENSE");
    command(&mut app,json!({"type":"SaveSetup","company_id":c,"expected_revision":0,"receivables_id":null,"undeposited_id":null,"payables_id":ap,"bank_ids":[bank,savings]}));
    let o=app.open_mut().unwrap();let vendor=fb_core::directory::save_record(&mut o.book,o.owner,c.parse().unwrap(),fb_core::directory::Model::Vendor,&json!({"display_name":"Supplier"}),None).unwrap()["id"].as_str().unwrap().to_owned();
    (app,c,bank,savings,expense,vendor)
}
fn save_bill(app:&mut App,c:&str,vendor:&str,expense:&str)->String{
    let path=format!("/c/{c}/bills/new/");let html=screens::render(app,&path).unwrap().html;
    let out=submit(app,&path,vec![("request_key",field(&html,"request_key")),("expected_revision","0".into()),("vendor",vendor.into()),("date","2026-10-01".into()),("due_date","2026-10-31".into()),("lines-TOTAL_FORMS","3".into()),("lines-INITIAL_FORMS","0".into()),("lines-0-description","Supplies".into()),("lines-0-quantity","2".into()),("lines-0-rate","10.00".into()),("lines-0-expense_account",expense.into())]);
    let Outcome::Redirect{location}=out else{panic!("{out:?}")};location.trim_end_matches('/').rsplit('/').next().unwrap().into()
}
fn count(app:&mut App,table:&str)->i64{app.open_mut().unwrap().book.read(|tx|tx.query_row(&format!("SELECT count(*) FROM {table}"),[],|r|r.get(0))).unwrap()}

#[test]
fn bill_review_is_bound_to_revision_and_cannot_be_forged(){
    let (mut app,c,_,_,expense,vendor)=fixture();let bill=save_bill(&mut app,&c,&vendor,&expense);
    let review=command(&mut app,json!({"type":"ReviewBill","company_id":c,"bill_id":bill,"expected_revision":1}));
    let path=format!("/c/{c}/bills/{bill}/edit/");let out=submit(&mut app,&path,vec![("expected_revision","1".into()),("vendor",vendor), ("date","2026-10-01".into()),("due_date","2026-10-31".into()),("lines-TOTAL_FORMS","1".into()),("lines-INITIAL_FORMS","1".into()),("lines-0-origin_position","1".into()),("lines-0-description","Edited supplies".into()),("lines-0-quantity","1".into()),("lines-0-rate","20.00".into()),("lines-0-expense_account",expense)]);assert!(matches!(out,Outcome::Redirect{..}));
    let before=count(&mut app,"ledger_auditevent");
    assert!(app.run(parse_command(&json!({"type":"PostBill","company_id":c,"bill_id":bill,"expected_revision":2,"review_token":review["token"]}).to_string()).unwrap()).is_err());
    assert!(app.run(parse_command(&json!({"type":"PostBill","company_id":c,"bill_id":bill,"expected_revision":2,"review_token":"forged"}).to_string()).unwrap()).is_err());
    assert_eq!(count(&mut app,"ledger_auditevent"),before);assert_eq!(count(&mut app,"ledger_journal"),0);
    let r=screens::render(&mut app,&format!("/c/{c}/bills/{bill}/revisions/1/")).unwrap();assert_eq!(r.status,200);assert!(r.html.contains("Historical revision 1"));
}

#[test]
fn transfer_changed_after_review_refuses_without_posting_and_lock_drops_review(){
    let (mut app,c,bank,savings,_,_)=fixture();let values=json!({"request_key":"00000000-0000-0000-0000-000000000601","date":"2026-10-31","from_account_id":bank,"to_account_id":savings,"amount":"20.00","memo":"Reserve"});
    let review=command(&mut app,json!({"type":"ReviewTransfer","company_id":c,"values":values}));let before=count(&mut app,"ledger_auditevent");
    let mut changed=values.clone();changed["amount"]=json!("21.00");let err=app.run(parse_command(&json!({"type":"RecordTransfer","company_id":c,"values":changed,"review_token":review["token"]}).to_string()).unwrap()).unwrap_err();assert!(err.to_string().contains("changed"));assert_eq!(count(&mut app,"ledger_auditevent"),before);assert_eq!(count(&mut app,"transfers_transfer"),0);
    let err=app.run(parse_command(&json!({"type":"RecordTransfer","company_id":c,"values":values,"review_token":review["token"]}).to_string()).unwrap()).unwrap_err();
    assert!(err.to_string().contains("fresh transfer review"));
    assert_eq!(count(&mut app,"ledger_auditevent"),before);assert_eq!(count(&mut app,"transfers_transfer"),0);
    let review=command(&mut app,json!({"type":"ReviewTransfer","company_id":c,"values":values}));
    command(&mut app,json!({"type":"RecordTransfer","company_id":c,"values":values,"review_token":review["token"]}));assert_eq!(count(&mut app,"transfers_transfer"),1);
    let mut unused=values.clone();unused["request_key"]=json!("00000000-0000-0000-0000-000000000602");
    let review=command(&mut app,json!({"type":"ReviewTransfer","company_id":c,"values":unused}));
    let path=std::env::temp_dir().join(format!("fb-d6-other-{}-{}.fbbook",std::process::id(),N.fetch_add(1,Ordering::SeqCst)));app.create_book(&path,&keys::new_master_key().unwrap(),"owner","Other book").unwrap();
    let err=app.run(parse_command(&json!({"type":"RecordTransfer","company_id":c,"values":values,"review_token":review["token"]}).to_string()).unwrap()).unwrap_err();assert!(err.to_string().contains("fresh transfer review"));assert_eq!(count(&mut app,"transfers_transfer"),0);
}

#[test]
fn invalid_form_fields_precede_business_rules_and_do_not_write(){
    let (mut app,c,bank,_,_,_)=fixture();let before=count(&mut app,"ledger_auditevent");
    let out=submit(&mut app,&format!("/c/{c}/transfers/new/"),vec![("request_key","bad".into()),("date","bad".into()),("from_account",bank.clone()),("to_account",bank),("amount","1".into())]);
    let Outcome::Page{html,status}=out else{panic!("{out:?}")};assert_eq!(status,400);assert!(html.contains("Enter a valid UUID."));assert!(html.contains("Enter a valid date."));assert!(html.contains("Choose a different bank account"));assert_eq!(count(&mut app,"ledger_auditevent"),before);
    let html=screens::render(&mut app,&format!("/c/{c}/bills/calendar/?year=1900&month=1")).unwrap().html;assert!(html.contains("January 1900"));assert!(html.contains("aria-disabled=\"true\""));
}

#[test]
fn command_contract_rejects_unknown_money_fields_and_revision_routes_are_numeric(){
    assert!(parse_command(r#"{"type":"RecordTransfer","company_id":"c","review_token":"t","values":{"request_key":"k","date":"d","from_account_id":"a","to_account_id":"b","amount":"1","memo":"","auto_post":true}}"#).is_err());
    let c="0195b4a4-55af-719e-96cb-a2f946b067bf";let b="0195b4a4-55af-719e-96cb-a2f946b067c0";
    assert_eq!(screens::resolve(&format!("/c/{c}/bills/{b}/revisions/1/")).unwrap().name,"bills:revision");assert!(screens::resolve(&format!("/c/{c}/bills/1/revisions/1/")).is_none());
}

#[test]
fn bill_ignores_whitespace_only_extra_rows_and_false_checkboxes() {
    let (mut app,c,_,_,expense,vendor)=fixture();
    let path=format!("/c/{c}/bills/new/");let html=screens::render(&mut app,&path).unwrap().html;
    let out=submit(&mut app,&path,vec![("request_key",field(&html,"request_key")),("expected_revision","0".into()),
        ("vendor",vendor),("date","2026-10-01".into()),("due_date","2026-10-31".into()),
        ("lines-TOTAL_FORMS","3".into()),("lines-INITIAL_FORMS","0".into()),("lines-0-description","Supplies".into()),
        ("lines-0-quantity","2".into()),("lines-0-rate","10.00".into()),("lines-0-expense_account",expense),
        ("lines-1-description"," \t\n ".into()),("lines-1-quantity","  ".into()),("lines-1-DELETE","false".into()),("lines-1-refresh_item","false".into())]);
    assert!(matches!(out,Outcome::Redirect{..}),"{out:?}");
    assert_eq!(count(&mut app,"payables_bill"),1);
    assert_eq!(count(&mut app,"payables_billline"),1);
}
#[test]
fn filter_choices_and_calendar_integer_whitespace_match_django_fields() {
    let (mut app,c,_,_,_,_)=fixture();let before=count(&mut app,"ledger_auditevent");
    for suffix in ["bills/?state=%20DRAFT%20","payments/?state=%20LIVE%20","transfers/?state=%20LIVE%20", "bills/calendar/?year=+&month=+"] {
        assert_eq!(screens::render(&mut app,&format!("/c/{c}/{suffix}")).unwrap().status,400,"{suffix}");
    }
    assert_eq!(count(&mut app,"ledger_auditevent"),before);
}
