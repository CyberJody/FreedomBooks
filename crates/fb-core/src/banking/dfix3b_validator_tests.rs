use super::*;

#[test]
fn reconcile_raw_revision_grammar_and_order_match_python_without_writes() {
    let mut f = Fx::new();
    let before = f.snap();
    for raw in [json!("abc"), json!(" "), json!("-1"), json!("1000000001"), json!(true), json!(false), json!("1.0"), json!("999999999999999999999999")] {
        let p = json!({"request_key":new_id(),"statement_date":"2026-10-31","ending_balance":"0.00","expected_revision":raw});
        assert_eq!(begin(&mut f.book,f.u,f.c,&f.bank.to_db(),&p).unwrap_err().message(), "Reload the page and try again.");
        assert_eq!(save_ticks(&mut f.book,f.u,f.c,&new_id(),&f.bank.to_db(),&json!({"expected_revision":raw,"shown":null,"ticked":[]})).unwrap_err().message(), "Reload the page and try again.");
        assert_eq!(before,f.snap());
    }
    for raw in [json!(0), json!(1_000_000_000), json!("+0"), json!(" 0 "), json!("０"), json!("0_0"), json!(-0.5), json!(0.9)] {
        let p = json!({"request_key":new_id(),"statement_date":"2026-10-31","ending_balance":"0.00","expected_revision":raw});
        assert_eq!(begin(&mut f.book,f.u,f.c,&f.bank.to_db(),&p).unwrap_err().message(), "This reconciliation was completed or changed meanwhile. Reload Banking → Reconcile.");
        assert_eq!(before,f.snap());
    }
}

#[test]
fn reconcile_key_validation_precedes_all_other_fields() {
    let mut f = Fx::new();
    let before = f.snap();
    for raw in [json!("abc"),json!(""),json!("-1"),json!("1000000001"),Value::Null,json!(true)] {
        assert_eq!(begin(&mut f.book,f.u,f.c,&f.bank.to_db(),&json!({"request_key":raw,"statement_date":"bad","ending_balance":"bad"})).unwrap_err().message(), "Reload the page and try again (missing request key).");
        assert_eq!(reconcile_now(&mut f.book,f.u,f.c,&new_id(),&f.bank.to_db(),None,&raw).unwrap_err().message(), "Reload the page and try again (missing completion key).");
        assert_eq!(undo_last(&mut f.book,f.u,f.c,&new_id(),&f.bank.to_db(),None,&json!(""),&raw).unwrap_err().message(), "Reload the page and try again (missing undo key).");
        assert_eq!(before,f.snap());
    }
}

#[test]
fn optional_charge_accounts_are_validated_before_statement_state() {
    let mut f = Fx::new();
    f.begin("0.00");
    let before = f.snap();
    for (field, label, kind) in [("service_account_id", "service charge", "expense"), ("interest_account_id", "interest earned", "income")] {
        for (raw, malformed) in [(json!("abc"),true),(Value::Null,true),(json!("-1"),false),(json!("1000000001"),false),(json!(f.bank.to_db()),false)] {
            let mut p=json!({"request_key":new_id(),"statement_date":"2026-10-31","ending_balance":"0.00"});
            p[if field=="service_account_id" {"service_charge"} else {"interest"}]=json!("1.00");
            p[field]=raw;
            let message=if malformed {format!("Choose the account for the {label}.")} else {format!("Choose an active {kind} account of this company for the {label}.")};
            assert_eq!(begin(&mut f.book,f.u,f.c,&f.bank.to_db(),&p).unwrap_err().message(),message);
            assert_eq!(before,f.snap());
        }
    }
}

#[test]
fn tick_shape_and_integer_cleaning_precede_lookup_and_preserve_noops() {
    let mut f=Fx::new();
    let line=f.journal("2026-10-01",true,"1.00");
    let r=f.begin("0.00");
    let before=f.snap();
    for (shown,ticked) in [(Value::Null,json!([])),(json!({"abc":0}),json!([])),(json!({line.clone():"abc"}),json!([])),(json!({}),json!(["abc"])),(json!({}),json!([line.clone()]))] {
        assert_eq!(save_ticks(&mut f.book,f.u,f.c,&new_id(),&f.bank.to_db(),&json!({"expected_revision":1,"shown":shown,"ticked":ticked})).unwrap_err().message(),"Reload the reconciliation and try again.");
        assert_eq!(before,f.snap());
    }
    for raw in [json!(-1),json!(" 0 "),json!("９"),json!(true),json!("99999999999999999999999999999")] {
        let out=save_ticks(&mut f.book,f.u,f.c,r["id"].as_str().unwrap(),&f.bank.to_db(),&json!({"expected_revision":0,"shown":{line.clone():raw},"ticked":[]})).unwrap();
        assert_eq!(out["revision"],1);
        assert_eq!(before,f.snap());
    }
}
