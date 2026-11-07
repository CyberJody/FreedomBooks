"""Corrections found by the first D2 service and screen runs."""
from pathlib import Path


def replace(path,old,new):
    p=Path(path);t=p.read_text();assert old in t,(path,old);p.write_text(t.replace(old,new))


replace('crates/fb-core/src/first_run.rs','''    services::audit(tx,&user,company,"setup.filled",format!("accountingsetup:{}",saved["id"].as_str().unwrap()),json!({"filled":values}))?;''','''    let id:String=tx.query_row("SELECT id FROM documents_accountingsetup WHERE company_id=?1",[company.to_db()],|r|r.get(0))?;
    services::audit(tx,&user,company,"setup.filled",format!("accountingsetup:{id}"),json!({"filled":values}))?;''')
replace('crates/fb-app/src/screens/d2.rs','''vm["setup"]=if setup["id"].is_null(){Value::Null}else{setup.clone()};''','''vm["setup"]=if setup["revision"]==0{Value::Null}else{setup.clone()};''')
