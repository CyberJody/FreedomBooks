use super::tests::Fixture;
use super::*;

#[test]
fn check_and_deposit_refund_share_numbers_release_and_next_hint() {
    let mut f = Fixture::new();
    let deposit = f.receive();
    let refund = f.refund(&deposit);
    let mut cost = services::AccountInput::new("500", "Synthetic cost");
    cost.kind = Some("EXPENSE".into());
    let cost = services::save_account(&mut f.book, f.actor, f.c, cost)
        .unwrap()
        .id;
    let mut input = json!({"request_key":new_id(),"bank_id":f.bank.to_db(),"payee_kind":"OTHER","payee_name":"Supplier","date":"2026-10-04","check_number":"4001","lines":[{"tab":"EXPENSE","account_id":cost.to_db(),"rate":"1"}]});
    let error = crate::checks::save_check(&mut f.book, f.actor, f.c, &input).unwrap_err();
    assert_eq!(error.kind(), "ValidationError");
    assert!(error.message().contains("already used by deposit refund"));
    assert_eq!(
        crate::checks::next_number(&mut f.book, f.actor, f.c, f.bank).unwrap(),
        "4002"
    );
    void_refund(
        &mut f.book,
        f.actor,
        f.c,
        s(&refund, "id"),
        Some(1),
        &json!("2026-10-05"),
        &json!("Release"),
    )
    .unwrap();
    let check = crate::checks::save_check(&mut f.book, f.actor, f.c, &input).unwrap();
    let mut rp = f.refund_input();
    rp["request_key"] = json!(new_id());
    let error = review_refund(&mut f.book, f.actor, f.c, s(&deposit, "id"), &rp).unwrap_err();
    assert_eq!(error.kind(), "ValidationError");
    assert!(error.message().contains("already used by check"));
    // A direct SQL update hits the same collision view. A different text is free.
    input["request_key"] = json!(new_id());
    input["check_number"] = json!("4002");
    let other = crate::checks::save_check(&mut f.book, f.actor, f.c, &input).unwrap();
    let err = f
        .book
        .connection()
        .execute(
            "UPDATE checks_check SET number_key='4001' WHERE id=?",
            [s(&other, "id")],
        )
        .unwrap_err();
    assert!(fb_db::is_constraint(&err));
    rp["check_number"] = json!("4003");
    let review = review_refund(&mut f.book, f.actor, f.c, s(&deposit, "id"), &rp).unwrap();
    let other_refund = record_refund(
        &mut f.book,
        f.actor,
        f.c,
        s(&deposit, "id"),
        Some(&review),
        &rp,
    )
    .unwrap();
    // Drop and restore all triggers to simulate an external cross-document duplicate.
    let q = f.book.connection();
    let definitions: Vec<(String, String)> = q
        .prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger'")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for (name, _) in &definitions {
        q.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
    }
    q.execute("UPDATE prepayments_depositrefund SET check_number='4001',data=json_set(data,'$.check_number','4001') WHERE id=?",[s(&other_refund,"id")]).unwrap();
    for (_, sql) in definitions {
        q.execute_batch(&sql).unwrap();
    }
    assert!(
        f.book
            .check()
            .unwrap()
            .iter()
            .any(|p| p.contains("duplicate live shared check number"))
    );
    assert_ne!(check["id"], other["id"]);
}
