use super::*;

#[test]
fn provider_controls_are_cleaned_before_masking_and_counted_without_blocking_sync() {
    let mut f=Fx::new();
    let mut a=acct("ACT-controls","Account\u{202e}Name",vec![tx("TX-controls","2026-10-01",100,"Description\u{2066}Text\tkept")]);
    a.org_name="Org\u{200b}Name".into();
    let pull=StagedPull { accounts:vec![a], ..Default::default() };
    let result=store_pull(&mut f.book,f.u,f.c,&f.conn,&pull,NOW,false,None,&f.secrets).unwrap();
    assert!(result["summary"].as_str().unwrap().contains("3 control/format characters removed"));
    let id:String=f.book.connection().query_row("SELECT id FROM bankfeeds_account WHERE connection_id=?",[&f.conn],|r|r.get(0)).unwrap();
    let names:(String,String)=f.book.connection().query_row("SELECT name,org_name FROM bankfeeds_account WHERE id=?",[&id],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(names,("AccountName".into(),"OrgName".into()));
    let description:String=f.book.connection().query_row("SELECT description FROM bankfeeds_transaction WHERE feed_account_id=?",[&id],|r|r.get(0)).unwrap();
    // Existing write sanitization folds whitespace; the ingress character filter permits TAB.
    assert_eq!(description,"DescriptionText kept");
    assert!(fb_db::check_numbers::free_text_char_allowed('\t'));
    map_account(&mut f.book,f.u,f.c,&id,&json!(f.a["100"].to_db()),&f.secrets).unwrap();
    let audits:Vec<(String,String)>=f.book.connection().prepare("SELECT action,detail FROM ledger_auditevent WHERE action IN ('bankfeed.synced','bankfeed.mapped') ORDER BY id").unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<Result<_,_>>().unwrap();
    for (_,text) in &audits { assert!(fb_db::check_numbers::free_text_valid(text)); }
    let synced:Value=serde_json::from_str(&audits.iter().find(|(a,_)|a=="bankfeed.synced").unwrap().1).unwrap();
    assert_eq!(synced["removed_characters"],3);
    let mapped=&audits.iter().find(|(a,_)|a=="bankfeed.mapped").unwrap().1;
    assert!(mapped.contains("OrgName") && mapped.contains("AccountName"));
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn removing_controls_cannot_split_a_known_secret_before_masking() {
    let secrets=SecretSet::from_credentials("synthetic-user","synthetic-secret",std::iter::empty::<&str>()).unwrap();
    let cleaned=super::super::staging::provider_ingress(&secrets,"syn\u{200b}thetic-\u{202e}secret");
    assert!(!cleaned.contains("synthetic-secret"));
    assert!(fb_db::check_numbers::free_text_valid(&cleaned));
}

#[test]
fn cleaning_display_ids_never_merges_raw_provider_identities() {
    let mut f=Fx::new();
    let pull=StagedPull {accounts:vec![
        acct("ACT-identity","One",vec![tx("TX-id","2026-10-01",100,"First"),tx("TX-\u{2066}id","2026-10-01",200,"Second")]),
        acct("ACT-\u{200b}identity","Two",vec![]),
    ],..Default::default()};
    store_pull(&mut f.book,f.u,f.c,&f.conn,&pull,NOW,false,None,&f.secrets).unwrap();
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_account"),2);
    assert_eq!(f.count("SELECT count(DISTINCT remote_identity) FROM bankfeeds_account"),2);
    assert_eq!(f.count("SELECT count(DISTINCT remote_id) FROM bankfeeds_account"),1);
    assert_eq!(f.count("SELECT count(DISTINCT remote_identity) FROM bankfeeds_transaction"),2);
    assert_eq!(f.count("SELECT count(DISTINCT remote_id) FROM bankfeeds_transaction"),1);
}

#[test]
fn partial_stage_and_failure_summaries_record_counts_only() {
    let mut f=Fx::new();
    let pull=StagedPull {accounts:vec![acct("ACT-stage","Stage account",vec![])],..Default::default()};
    store_pull(&mut f.book,f.u,f.c,&f.conn,&pull,NOW,false,None,&f.secrets).unwrap();
    stage_bank_transactions(&mut f.book,f.u,f.c,&f.conn,&[("ACT-stage".into(),tx("TX-stage","2026-10-01",100,"Stage\u{200b}description"))],&f.secrets).unwrap();
    record_sync_failure(&mut f.book,f.u,f.c,&f.conn,NOW,"Provider\u{2066}failure",false,&f.secrets).unwrap();
    for action in ["bankfeed.staged","bankfeed.synced"] {
        let raw:String=f.book.connection().query_row("SELECT detail FROM ledger_auditevent WHERE action=? ORDER BY id DESC LIMIT 1",[action],|r|r.get(0)).unwrap();
        let detail:Value=serde_json::from_str(&raw).unwrap();
        assert_eq!(detail["removed_characters"],1);
        assert!(!raw.contains('\u{200b}') && !raw.contains('\u{2066}'));
    }
}

#[test]
fn retained_rule_lookup_does_not_preempt_field_validation() {
    let mut f=Fx::new();
    let before=fb_db::test_support::snapshot(f.book.connection()).unwrap();
    for (values,expected) in [
        (json!({"amount_min":"bad"}),"Enter a finite nonnegative value with at most 2 decimal places within the supported limit."),
        (json!({"sign":"bad"}),"Choose money in, money out, or either."),
        (json!({"amount_min":"2.00","amount_max":"1.00"}),"The minimum amount cannot exceed the maximum."),
    ] {
        let mut p=json!({"rule_id":"00000000-0000-0000-0000-000000df3bad","name":"Valid name","match_text":"Valid match","account_id":f.a["500"].to_db()});
        for (key,value) in values.as_object().unwrap() { p[key]=value.clone(); }
        assert_eq!(save_rule(&mut f.book,f.u,f.c,&p,&f.secrets).unwrap_err().message(),expected);
        assert_eq!(before,fb_db::test_support::snapshot(f.book.connection()).unwrap());
    }
}
