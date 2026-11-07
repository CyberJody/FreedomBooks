//! Reviewer R9's new_r9.rs, preserving assertions; external evidence-file writes removed.
use super::*;

pub(super) fn snapshot_texts(f:&Fx)->Vec<(String,String)> {
    fb_db::test_support::snapshot(f.book.connection()).unwrap().into_iter().flat_map(|(table,rows)|{
        rows.into_iter().flatten().filter_map(move |v|match v {
            fb_db::rusqlite::types::Value::Text(s)=>Some((table.clone(),s)),_=>None
        })
    }).collect()
}

#[test]
fn r9_audit_serialization_cannot_reconstruct_known_secret() {
    let mut evidence=vec![];
    for (secret,text) in [
        (r#"R9alpha\"omega-secret"#,r#"R9alpha"omega-secret"#),
        (r#"R9alpha\\omega-secret"#,r#"R9alpha\omega-secret"#),
    ] {
        let mut f=Fx::new();
        let known=SecretSet::masking_from_access_url(&format!("https://R9username-long:{}@r9.test/access",secret.bytes().map(|b|format!("%{b:02X}")).collect::<String>()),Vec::<String>::new()).unwrap();
        assert!(!text.contains(secret));
        assert!(!staging::provider_text(&known,text).contains(secret));
        record_sync_failure(&mut f.book,f.u,f.c,&f.conn.clone(),NOW,text,false,&known).unwrap();
        let actions:Vec<String>={
            let mut st=f.book.connection().prepare("SELECT action FROM ledger_auditevent WHERE instr(detail,?1)>0").unwrap();
            st.query_map([secret],|r|r.get(0)).unwrap().map(Result::unwrap).collect()
        };
        evidence.push(json!({"case":"JSON string escaping","leaked_actions":actions}));
        f.reopen_clean();r5_finish(&mut f);
    }
    assert!(evidence.iter().all(|e|e["leaked_actions"].as_array().unwrap().is_empty()),"serialization reconstructed a known credential");
}

#[test]
fn r9_audit_json_field_join_cannot_reconstruct_known_secret() {
    let mut f=Fx::new();
    let secret=r#"R9alpha","remote_id":"R9omega"#;
    let known=SecretSet::masking_from_access_url(&format!("https://R9username-long:{}@r9.test/access",secret.bytes().map(|b|format!("%{b:02X}")).collect::<String>()),Vec::<String>::new()).unwrap();
    let a=acct("R9omega","R9alpha",vec![]);
    store_pull(&mut f.book,f.u,f.c,&f.conn.clone(),&StagedPull{accounts:vec![a],..Default::default()},NOW,false,None,&known).unwrap();
    let feed=f.feed("R9omega");
    map_account(&mut f.book,f.u,f.c,&feed,&Value::Null,&known).unwrap();
    let actions:Vec<String>={
        let mut st=f.book.connection().prepare("SELECT action FROM ledger_auditevent WHERE instr(detail,?1)>0").unwrap();
        st.query_map([secret],|r|r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    f.reopen_clean();r5_finish(&mut f);
    assert!(actions.is_empty(),"JSON object serialization rejoins known credential in audit");
}

#[test]
fn r9_every_accept_kind_provider_and_input_fields_are_masked() {
    let mut counts=0;
    let mut evidence=vec![];
    for (i,c) in (32u8..=126).map(char::from).chain(['é','€','中','😀','\u{301}']).enumerate() {
        let secret=format!("R9-input-{i:03}{c}-password");
        let known=SecretSet::masking_from_credentials("R9username-long",&secret,Vec::<String>::new()).unwrap();
        for kind in ["JOURNAL","CHECK","DEPOSIT","TRANSFER","MATCH"] {
            let (mut f,remote,mut input)=fix1_accept_case(kind);
            f.secrets=known.clone();
            let (identity,feed_id,amount,date)={let item=f.item(remote);(item.id.clone(),item.feed_account_id.clone(),item.amount,item.date().unwrap().to_owned())};
            // Provider data is changed through the staging service, never raw SQL.
            let feed=feed(f.book.connection(),&feed_id).unwrap().unwrap();
            stage_bank_transactions(&mut f.book,f.u,f.c,&f.conn.clone(),&[(feed.remote_id,tx(remote,&date,amount,&secret))],&known).unwrap();
            if kind=="CHECK"||kind=="JOURNAL" {input["payee_name"]=json!(secret);}
            if kind=="CHECK" {input["check_number"]=json!(format!("R9-{i}"));}
            let result=accept(&mut f.book,f.u,f.c,&identity,&input,&known).unwrap();
            let leaks:Vec<String>=snapshot_texts(&f).into_iter().filter(|(_,s)|s.contains(&secret)).map(|(t,_)|t).collect();
            if !leaks.is_empty(){evidence.push(json!({"character_index":i,"kind":kind,"tables":leaks}));}
            assert!(!format!("{result:?}").contains(&secret));
            f.reopen_clean();r5_finish(&mut f);counts+=1;
        }
    }
    eprintln!("R9 accept-path cases={counts}");
    assert!(evidence.is_empty(),"credential persisted after accepting");
}

#[test]
fn r9_final_serialization_all_provider_writers() {
    let mut evidence=vec![];
    for (secret,text) in [
        (r#"r9alpha\"omega-secret"#,r#"r9alpha"omega-secret"#),
        (r#"r9alpha\\omega-secret"#,r#"r9alpha\omega-secret"#),
    ] {
        let mut f=Fx::new();
        let known=SecretSet::masking_from_access_url(&format!("https://R9username-long:{}@r9.test/access",secret.bytes().map(|b|format!("%{b:02X}")).collect::<String>()),Vec::<String>::new()).unwrap();
        f.secrets=known.clone();
        let c=connect(&mut f.book,f.u,f.c,&json!(text),text,&known).unwrap();
        forget(&mut f.book,f.u,f.c,c["id"].as_str().unwrap(),&known).unwrap();
        record_sync_failure(&mut f.book,f.u,f.c,&f.conn.clone(),NOW,text,false,&known).unwrap();
        let mut pull=demo();
        for a in &mut pull.accounts {
            a.name=text.into();a.org_name=text.into();a.skipped=vec![text.into()];
            for t in &mut a.transactions {t.description=text.into();}
        }
        pull.errors=vec![text.into()];pull.skipped=vec![text.into()];
        store_pull(&mut f.book,f.u,f.c,&f.conn.clone(),&pull,NOW,false,None,&known).unwrap();
        for (a,code) in [("ACT-checking","100"),("ACT-savings","101"),("ACT-card","220")] {f.map(a,Some(code)).unwrap();}
        stage_bank_transactions(&mut f.book,f.u,f.c,&f.conn.clone(),&[("ACT-checking".into(),tx(text,"2026-10-20",-100,text))],&known).unwrap();
        let i=f.item(text);ignore(&mut f.book,f.u,f.c,&i.id,&known).unwrap();return_to_review(&mut f.book,f.u,f.c,&i.id,&known).unwrap();
        let receipt=f.undeposited("2500.00","2026-10-04");
        for (remote,input) in [
            ("TX-1006",json!({"kind":"JOURNAL","account_id":f.acc("410"),"payee_name":text})),
            ("TX-1001",json!({"kind":"CHECK","account_id":f.acc("500"),"payee_name":text,"check_number":text})),
            ("TX-1003",json!({"kind":"DEPOSIT","receipt_ids":[receipt]})),
            ("TX-1005",json!({"kind":"TRANSFER","transfer_account_id":f.acc("101")})),
        ] {f.accept(remote,input).unwrap();}
        let accepted=f.item("TX-1001");
        let chk=checks::current_checks(&mut f.book,f.u,f.c).unwrap().into_iter().find(|c|c["number"].as_str()==Some(accepted.reference.as_str())).unwrap();
        checks::void_check(&mut f.book,f.u,f.c,chk["id"].as_str().unwrap(),chk["revision"].as_i64(),&json!("2026-10-30"),&json!("Review correction")).unwrap();
        return_to_review(&mut f.book,f.u,f.c,&accepted.id,&known).unwrap();
        let category=f.acc("500");
        let rule=save_rule(&mut f.book,f.u,f.c,&json!({"name":text,"match_text":text,"payee_name":text,"account_id":category}),&known).unwrap();
        save_rule(&mut f.book,f.u,f.c,&json!({"rule_id":rule.id,"name":text,"match_text":text,"payee_name":text,"account_id":category}),&known).unwrap();
        let tables:std::collections::BTreeSet<String>=snapshot_texts(&f).into_iter().filter(|(_,s)|s.contains(secret)).map(|(t,_)|t).collect();
        let actions:Vec<String>={let mut st=f.book.connection().prepare("SELECT DISTINCT action FROM ledger_auditevent WHERE instr(detail,?1)>0 ORDER BY action").unwrap();st.query_map([secret],|r|r.get(0)).unwrap().map(Result::unwrap).collect()};
        evidence.push(json!({"case":if secret.contains("\\\\"){"backslash"}else{"quote"},"tables":tables,"actions":actions}));
        f.reopen_clean();r5_finish(&mut f);
    }
    assert!(evidence.iter().all(|e|e["tables"].as_array().unwrap().is_empty()),"known credential created by final JSON serialization");
}
