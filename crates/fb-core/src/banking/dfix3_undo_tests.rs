use super::*;

#[test]
fn undo_reason_matches_python_before_lookup_and_without_writes() {
    let mut f=Fx::new();
    let before=f.snap();
    let key=new_id();let undo=json!(new_id());
    for reason in [json!(""),json!(" \t "),Value::Null,json!(7),json!(false)] {
        assert_eq!(undo_last(&mut f.book,f.u,f.c,&key,&f.bank.to_db(),Some(2),&reason,&undo).unwrap_err().message(),"Give a reason for undoing this reconciliation.");
        assert_eq!(before,f.snap());
    }
    assert_eq!(undo_last(&mut f.book,f.u,f.c,&key,&f.bank.to_db(),Some(2),&json!("x".repeat(501)),&undo).unwrap_err().message(),"Keep the reason to 500 characters.");
    assert_eq!(before,f.snap());
}

#[test]
fn required_revisions_and_keys_precede_reason_lookup_and_retries() {
    let mut f=Fx::new();
    let r=f.begin("0.00");
    let r=f.finish(&r);
    let key=r["id"].as_str().unwrap();
    let before=f.snap();
    for revision in [None,Some(-1),Some(1_000_000_001),Some(i64::MAX)] {
        for reason in ["Synthetic correction", ""] {
            assert_eq!(undo_last(&mut f.book,f.u,f.c,key,&f.bank.to_db(),revision,&json!(reason),&json!(new_id())).unwrap_err().message(),"Reload the page and try again.");
            assert_eq!(before,f.snap());
        }
        assert_eq!(reconcile_now(&mut f.book,f.u,f.c,key,&f.bank.to_db(),revision,&r["completion_key"]).unwrap_err().message(),"Reload the page and try again.");
        assert_eq!(before,f.snap());
    }
    for (revision,message) in [(Some(0),"This reconciliation changed after you opened it. Reload the reconciliation history and try again."),(Some(1_000_000_000),"This reconciliation changed after you opened it. Reload the reconciliation history and try again.")] {
        assert_eq!(undo_last(&mut f.book,f.u,f.c,key,&f.bank.to_db(),revision,&json!("Synthetic correction"),&json!(new_id())).unwrap_err().message(),message);
        assert_eq!(before,f.snap());
    }
}
