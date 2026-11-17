use super::*;

fn adjustment(kind: &str, rate: &str, taxable: bool, account: AccountId) -> Value {
    json!({"line_kind":kind,"description":"Synthetic adjustment","quantity":"1","rate":rate,"taxable":taxable,"income_account_id":account.to_db()})
}
#[test]
fn discounts_notes_long_description_post_void_and_reopen() {
    let mut f = Fixture::new();
    let mut input = f.input();
    input["tax_rate"] = json!("7");
    input["lines"] = json!([
        adjustment("CHARGE", "840.25", true, f.income),
        adjustment("DISCOUNT", "84.03", true, f.income),
        {"line_kind":"NOTE","description":("An archived detail line.\n").repeat(65).trim().to_owned(),"quantity":"0","rate":"0","taxable":false}
    ]);
    let saved = save_invoice(&mut f.book, f.actor, f.company, &input).unwrap();
    assert_eq!(saved["current"]["subtotal"], "756.22");
    assert_eq!(saved["current"]["tax"], "52.94");
    assert_eq!(saved["current"]["total"], "809.16");
    assert_eq!(saved["lines"][1]["amount"], "84.03");
    assert_eq!(
        saved["lines"][2]["description"],
        input["lines"][2]["description"]
    );
    assert!(saved["lines"][0].get("line_kind").is_none());
    let key = saved["id"].as_str().unwrap();
    let r = review_invoice(&mut f.book, f.actor, f.company, key, Some(1)).unwrap();
    let posted = post_invoice(&mut f.book, f.actor, f.company, key, Some(1), Some(&r)).unwrap();
    assert_eq!(posted["current"]["state"], "OPEN");
    let retry = save_invoice(&mut f.book, f.actor, f.company, &input).unwrap();
    assert_eq!(retry["revision"], 2);
    initialize(&f.book).unwrap();
    let voided = void_invoice(
        &mut f.book,
        f.actor,
        f.company,
        key,
        Some(2),
        &json!("2026-10-03"),
        &json!("Synthetic correction"),
    )
    .unwrap();
    assert_eq!(voided["current"]["state"], "VOID");
    initialize(&f.book).unwrap();
    let path: String = f
        .book
        .connection()
        .query_row("PRAGMA database_list", [], |r| r.get(2))
        .unwrap();
    let key = key.to_owned();
    drop(f.book);
    let mut reopened = Book::open(std::path::Path::new(&path), &[13; 32]).unwrap();
    initialize(&reopened).unwrap();
    let again = reopened.read(|tx| invoice(tx, f.company, &key)).unwrap();
    assert_eq!(again["current"]["state"], "VOID");
    assert_eq!(
        again["lines"][2]["description"],
        input["lines"][2]["description"]
    );
}
#[test]
fn discount_separate_income_account_creates_a_debit() {
    let mut f = Fixture::new();
    let mut a = services::AccountInput::new("401", "Discounts");
    a.kind = Some("INCOME".into());
    let contra = services::save_account(&mut f.book, f.actor, f.company, a)
        .unwrap()
        .id;
    let mut input = f.input();
    input["tax_mode"] = json!("NONE");
    input["tax_rate"] = json!("0");
    input["lines"] = json!([
        adjustment("CHARGE", "100", false, f.income),
        adjustment("DISCOUNT", "10", false, contra)
    ]);
    let inv = save_invoice(&mut f.book, f.actor, f.company, &input).unwrap();
    let key = inv["id"].as_str().unwrap();
    let r = review_invoice(&mut f.book, f.actor, f.company, key, Some(1)).unwrap();
    assert!(
        r.display["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["account"] == contra.to_db()
                && e["debit"] == "10.00"
                && e["credit"] == "0.00")
    );
    post_invoice(&mut f.book, f.actor, f.company, key, Some(1), Some(&r)).unwrap();
    initialize(&f.book).unwrap();
}
#[test]
fn invalid_adjustments_do_not_create_invoices() {
    let mut f = Fixture::new();
    for bad in [
        adjustment("DISCOUNT", "4.02", true, f.income),
        adjustment("DISCOUNT", "0.01", false, f.income),
        adjustment("UNKNOWN", "0.01", true, f.income),
        json!({"line_kind":"NOTE","description":"Note","quantity":"1","rate":"0","taxable":false}),
        json!({"line_kind":"NOTE","description":"Note","quantity":"0","rate":"0","taxable":true}),
        json!({"line_kind":"NOTE","description":"Note","quantity":"0","rate":"0","taxable":false,"income_account_id":f.income.to_db()}),
        adjustment("DISCOUNT", "-0.01", true, f.income),
    ] {
        let mut input = f.input();
        input["lines"].as_array_mut().unwrap().push(bad);
        assert!(save_invoice(&mut f.book, f.actor, f.company, &input).is_err());
    }
    assert_eq!(
        f.book
            .read(
                |tx| tx.query_row("SELECT count(*) FROM sales_invoice", [], |r| r
                    .get::<_, i64>(0))
            )
            .unwrap(),
        0
    );
}
#[test]
fn legacy_invoice_digest_and_other_documents_keep_charge_semantics() {
    let mut f = Fixture::new();
    let input = f.input();
    let inv = save_invoice(&mut f.book, f.actor, f.company, &input).unwrap();
    let mut explicit = input.clone();
    explicit["lines"][0]["line_kind"] = json!("CHARGE");
    assert_eq!(
        save_invoice(&mut f.book, f.actor, f.company, &explicit).unwrap()["id"],
        inv["id"]
    );
    let line = inv["lines"][0].clone();
    f.book
        .read(|tx| -> FbResult<()> {
            for kind in ["DISCOUNT", "NOTE", "CHARGE"] {
                let mut l = line.clone();
                l["line_kind"] = json!(kind);
                let allowed: bool =
                    tx.query_row("SELECT fb_sales_line_ok(?1)", [l.to_string()], |r| r.get(0))?;
                assert!(!allowed);
            }
            Ok(())
        })
        .unwrap();
    initialize(&f.book).unwrap();
}
