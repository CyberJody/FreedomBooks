use super::*;

#[test]
fn begin_invalid_amount_has_exact_field_wording_order_and_no_writes() {
    let mut f=Fx::new();
    for (field,label) in [("ending_balance","statement ending balance"),("service_charge","service charge"),("interest","interest earned")] {
        for (value,ending) in [("bad","as an amount such as 1250.00."),("1e","as an amount such as 1250.00."),("NaN","with at most two decimal places (up to 99999999999999.99)."),("0.001","with at most two decimal places (up to 99999999999999.99)."),("100000000000000.00","with at most two decimal places (up to 99999999999999.99)."),("1e-99999999999999999","with at most two decimal places (up to 99999999999999.99).")] {
            let mut p=json!({"request_key":new_id(),"statement_date":"2026-10-31","ending_balance":"0.00"});p[field]=json!(value);
            let before=f.snap();
            assert_eq!(begin(&mut f.book,f.u,f.c,&f.bank.to_db(),&p).unwrap_err().message(),format!("Enter the {label} {ending}"));
            assert_eq!(before,f.snap());
        }
    }
    let p=json!({"request_key":new_id(),"statement_date":"bad","ending_balance":"bad","service_charge":"bad","interest":"bad"});
    assert_eq!(begin(&mut f.book,f.u,f.c,&f.bank.to_db(),&p).unwrap_err().message(),"Enter the statement date as YYYY-MM-DD.");
}

