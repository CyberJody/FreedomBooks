"""Exercise carried bank-rule text with real prior-version source, plus account/tax edits."""
from pathlib import Path
p=Path(__file__).resolve().parents[1]/'crates/fb-core/examples/dfix3_upgrade.rs';t=p.read_text()
t=t.replace('accounts, banking, credits,','accounts, banking, bankfeeds, salestax, credits,')
old='''        let mut extra=json!({});
        {'''
new='''        let mut extra=json!({});
        services::save_account(&mut book,u,c,AccountInput::new("500","500").kind("EXPENSE").edit(expense).description(LEGACY)).unwrap();
        extra["expense"]=json!(expense.to_db());
        let rule=json!({"name":"Legacy\\u{202e}rule","match_text":"Legacy\\u{200b}match","payee_name":"Legacy\\u{2066}payee","account_id":expense.to_db()});
        extra["rule"]=json!(bankfeeds::save_rule(&mut book,u,c,&rule,&bankfeeds::SecretSet::none()).unwrap().id);extra["rule values"]=rule;
        let rate=json!({"name":"Legacy rate","description":LEGACY,"rate":"1","agency_id":vendor["id"]});
        extra["rate"]=salestax::save_rate(&mut book,u,c,&rate).unwrap()["id"].clone();extra["rate values"]=rate;
        let rate2=salestax::save_rate(&mut book,u,c,&json!({"name":"Legacy rate 2","description":LEGACY,"rate":"2","agency_id":vendor["id"]})).unwrap();
        let group=json!({"name":"Legacy group","description":LEGACY,"member_ids":[extra["rate"],rate2["id"]]});
        extra["group"]=salestax::save_group(&mut book,u,c,&group).unwrap()["id"].clone();extra["group values"]=group;
        {'''
assert t.count(old)==1;t=t.replace(old,new)
old='''        println!("PASS master-record archive/reactivate and partial revision preserve legacy fields");'''
new=old+'''
        let expense=fb_core::AccountId::parse(data["extra"]["expense"].as_str().unwrap()).unwrap();
        let account=services::save_account(&mut book,u,c,AccountInput::new("500","500").kind("EXPENSE").edit(expense)).unwrap();assert_eq!(account.description,LEGACY);
        let mut rule=data["extra"]["rule values"].clone();rule["rule_id"]=data["extra"]["rule"].clone();rule["amount_min"]=json!("1.00");
        let saved=bankfeeds::save_rule(&mut book,u,c,&rule,&bankfeeds::SecretSet::none()).unwrap();assert_eq!(saved.name,"Legacy\\u{202e}rule");assert_eq!(saved.match_text,"Legacy\\u{200b}match");
        let before=rows(&book);rule["name"]=json!("Edited\\u{202e}name");assert!(bankfeeds::save_rule(&mut book,u,c,&rule,&bankfeeds::SecretSet::none()).is_err());assert_eq!(before,rows(&book));
        let mut rate=data["extra"]["rate values"].clone();rate["item_id"]=data["extra"]["rate"].clone();rate["expected_revision"]=json!(1);rate["rate"]=json!("1.5");salestax::save_rate(&mut book,u,c,&rate).unwrap();
        let mut group=data["extra"]["group values"].clone();group["item_id"]=data["extra"]["group"].clone();group["expected_revision"]=json!(1);salestax::save_group(&mut book,u,c,&group).unwrap();
        println!("PASS account, bank-rule and tax revisions retain unchanged legacy text; edited rule controls refused without writes");'''
assert t.count(old)==1;t=t.replace(old,new)
p.write_text(t)
