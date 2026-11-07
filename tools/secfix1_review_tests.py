"""Update old retry expectations to assert single-use review capabilities."""
from pathlib import Path
root=Path(__file__).resolve().parents[1]
p=root/'crates/fb-app/tests/d6_flow.rs';t=p.read_text()
old='''    command(&mut app,json!({"type":"RecordTransfer","company_id":c,"values":values,"review_token":review["token"]}));assert_eq!(count(&mut app,"transfers_transfer"),1);'''
new='''    let err=app.run(parse_command(&json!({"type":"RecordTransfer","company_id":c,"values":values,"review_token":review["token"]}).to_string()).unwrap()).unwrap_err();
    assert!(err.to_string().contains("fresh transfer review"));
    assert_eq!(count(&mut app,"ledger_auditevent"),before);assert_eq!(count(&mut app,"transfers_transfer"),0);
    let review=command(&mut app,json!({"type":"ReviewTransfer","company_id":c,"values":values}));
    command(&mut app,json!({"type":"RecordTransfer","company_id":c,"values":values,"review_token":review["token"]}));assert_eq!(count(&mut app,"transfers_transfer"),1);
    let mut unused=values.clone();unused["request_key"]=json!("00000000-0000-0000-0000-000000000602");
    let review=command(&mut app,json!({"type":"ReviewTransfer","company_id":c,"values":unused}));'''
assert old in t;t=t.replace(old,new);p.write_text(t)
p=root/'crates/fb-app/tests/d7a_flow.rs';t=p.read_text()
old='''    let id = saved(post(&mut f.app, &path, &record));'''
new='''    let Outcome::Page { html, status } = post(&mut f.app, &path, &record) else { panic!("consumed review must fail") };
    assert_eq!(status,400);assert!(html.contains("review"));
    assert_eq!(count(&mut f.app,"ledger_auditevent"),before);
    assert_eq!(count(&mut f.app,"banking_deposit"),0);
    let Outcome::Page { html, status } = post(&mut f.app,&path,&p) else { panic!("fresh review") };
    assert_eq!(status,200);set(&mut record,"review_token",&field(&html,"review_token"));
    let id = saved(post(&mut f.app, &path, &record));'''
assert t.count(old)==1;t=t.replace(old,new);p.write_text(t)
