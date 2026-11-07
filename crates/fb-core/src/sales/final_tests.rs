use super::*;

#[test]
fn audit_payloads_keep_the_frozen_service_shapes_and_activity_includes_reversals() {
    let mut f = Fixture::new();
    let inv = f.posted();
    let p = f.payment(inv["id"].as_str().unwrap(), "1");
    let review = review_receipt(&mut f.book, f.actor, f.company, &p).unwrap();
    let receipt = record_receipt(&mut f.book, f.actor, f.company, &p, Some(&review)).unwrap();
    let read = |book: &Book, action: &str| -> Value {
        let s: String = book
            .connection()
            .query_row(
                "SELECT detail FROM ledger_auditevent WHERE action=?1",
                [action],
                |r| r.get(0),
            )
            .unwrap();
        serde_json::from_str(&s).unwrap()
    };
    let payload = read(&f.book, "receipt.recorded");
    assert_eq!(
        payload
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "number".into(),
            "amount".into(),
            "memo".into(),
            "allocations".into()
        ])
    );
    assert_eq!(
        payload["allocations"],
        json!([{"invoice_id":inv["id"],"amount":"1.00"}])
    );
    reverse_receipt(
        &mut f.book,
        f.actor,
        f.company,
        receipt["id"].as_str().unwrap(),
        Some(1),
        &json!("2026-10-03"),
        &json!("Correction"),
    )
    .unwrap();
    let s = f.sale_input();
    let review = review_sales_receipt(&mut f.book, f.actor, f.company, &s).unwrap();
    record_sales_receipt(&mut f.book, f.actor, f.company, &s, Some(&review)).unwrap();
    let payload = read(&f.book, "sales_receipt.recorded");
    assert_eq!(
        payload["lines"],
        json!([{"position":1,"income_account_id":f.income.to_db(),"amount":"4.01"}])
    );
    assert!(payload.get("allocations").is_none());
    let activity = customer_activity(&mut f.book, f.actor, f.company, &f.customer).unwrap();
    assert_eq!(
        activity["receipts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == receipt["id"])
            .unwrap()["reversal"]["reason"],
        "Correction"
    );
}
#[test]
fn reopened_connections_register_exact_money_callbacks_before_any_sales_operation() {
    let mut f = Fixture::new();
    f.save();
    let path: String = f
        .book
        .connection()
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name='main'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let book = Book::open(std::path::Path::new(&path), &[13; 32]).unwrap();
    // Attacker's late invoice line id: canonical, so 0130 passes and invoice_line_scope decides.
    let late = "00000000-0000-7000-8000-0000000c5f01";
    let err=book.connection().execute_batch(&format!("INSERT INTO sales_invoiceline SELECT '{late}',revision_id,50,item_id,income_account_id,expense_account_id,amount,taxable,json_set(data,'$.position',50) FROM sales_invoiceline LIMIT 1")).unwrap_err();
    assert_eq!(fb_db::constraint_of(&err).as_deref(), Some("invoice_line_scope"), "{err}");
    initialize(&book).unwrap();
    assert!(check(&book).unwrap().is_empty());
}
#[test]
fn row_tampering_is_refused_even_when_the_guard_definition_is_restored() {
    let mut f = Fixture::new();
    let inv = f.save();
    let conn = f.book.connection();
    let definition: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='fb_sales_invoicerevision_update'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute_batch("DROP TRIGGER fb_sales_invoicerevision_update")
        .unwrap();
    conn.execute("UPDATE sales_invoicerevision SET data=json_set(data,'$.total','999.00') WHERE invoice_id=?1",[inv["id"].as_str().unwrap()]).unwrap();
    conn.execute_batch(&definition).unwrap();
    assert!(
        initialize(&f.book)
            .unwrap_err()
            .message()
            .contains("inconsistent sales totals")
    );
}
