mod common;
use common::*;
use fb_core::{JournalId, checks, payables, sales, salestax, services};
use serde_json::json;

#[test]
fn retained_provenance_lookups_preserve_existing_input_error_order_without_writes() {
    let mut f=Fx::new("dfix3-order");let (c,_,_)=f.chart();
    let missing=fb_ids::new_v7().to_string();
    let before=fb_db::test_support::snapshot(f.book.connection()).unwrap();
    let mut draft=services::DraftInput::new("2026-10-01","Valid memo",vec![json!({"unexpected":true})]);
    draft.journal_id=JournalId::parse(&missing);
    assert_eq!(services::save_draft(&mut f.book,f.user,c,draft).unwrap_err().message(),"Unexpected journal line fields.");
    let p=json!({"invoice_id":missing,"estimate_id":missing,"customer_id":missing,"date":"bad","due_date":"2026-10-31","tax_mode":"NONE","lines":[]});
    assert_eq!(sales::save_invoice(&mut f.book,f.user,c,&p).unwrap_err().message(),"Enter a valid calendar date.");
    assert_eq!(sales::save_estimate(&mut f.book,f.user,c,&p).unwrap_err().message(),"Enter a valid calendar date.");
    assert_eq!(payables::save_bill(&mut f.book,f.user,c,&json!({"bill_id":missing})).unwrap_err().message(),"Choose a record from this company.");
    assert_eq!(checks::save_check(&mut f.book,f.user,c,&json!({"check_id":missing})).unwrap_err().message(),"Choose a record from this company.");
    for group in [false,true] {
        let p=json!({"item_id":missing,"name":"","description":"Valid"});
        let result=if group {salestax::save_group(&mut f.book,f.user,c,&p)} else {salestax::save_rate(&mut f.book,f.user,c,&p)};
        assert_eq!(result.unwrap_err().message(),"Enter a name (up to 60 characters).");
    }
    assert_eq!(before,fb_db::test_support::snapshot(f.book.connection()).unwrap());
}
