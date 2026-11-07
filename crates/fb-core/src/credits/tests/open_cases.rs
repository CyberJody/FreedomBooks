#[test]
fn review_credit_reopen_must_reject_duplicate_live_refund_numbers() {
    let mut f = Fixture::new();
    let memo = f.memo();
    let key = memo["id"].as_str().unwrap();
    for number in ["1001", "1002"] {
        let mut p = f.refund_input("1.00");
        p["check_number"] = json!(number);
        let r = review_refund(&mut f.book, f.actor, f.company, key, &p).unwrap();
        record_refund(&mut f.book, f.actor, f.company, key, &p, Some(&r)).unwrap();
    }
    f.clean();
    let c = f.book.connection();
    let definition: String = c
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='fb_credits_creditrefund_update'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    c.execute_batch("BEGIN IMMEDIATE; DROP TRIGGER fb_credits_creditrefund_update;")
        .unwrap();
    c.execute("UPDATE credits_creditrefund SET check_number='1001', data=json_set(data,'$.check_number','1001') WHERE check_number='1002'", []).unwrap();
    c.execute_batch(&definition).unwrap();
    c.execute_batch("COMMIT").unwrap();
    let problems = f.book.check().unwrap();
    let reopened = Book::open(&f.path, &[24; 32]);
    println!(
        "Duplicate live refund check 1001, exact schema restored: check={problems:?}; reopen={:?}",
        reopened.as_ref().map(|_| "accepted")
    );
    assert!(
        !problems.is_empty() && reopened.is_err(),
        "duplicate live refund numbers pass check/open"
    );
}

#[test]
fn review_credit_foreign_snapshot_ids_must_match_owning_rows() {
    let mut f = Fixture::new();
    let input = f.input();
    let review = review_credit_memo(&mut f.book, f.actor, f.company, &input).unwrap();
    let base = record_credit_memo(&mut f.book, f.actor, f.company, &input, Some(&review)).unwrap();
    let full = get_credit_memo(
        &mut f.book,
        f.actor,
        f.company,
        base["id"].as_str().unwrap(),
    )
    .unwrap();
    let actor = f.actor;
    let co = f.company;
    let result: FbResult<()> = f.book.write(|tx| {
        let mut m = memo_shell(tx, actor, co, &base, &review.display, false, false)?;
        m["customer_snapshot"]["id"] = json!(new_id());
        m["company_snapshot"]["id"] = json!(new_id());
        insert(tx, "credits_creditmemo", &m)?;
        lines(tx, &m, full["lines"].as_array().unwrap())?;
        seal(tx, "memo", m["id"].as_str().unwrap())?;
        seal(tx, "journal", m["journal_id"].as_str().unwrap())?;
        Ok(())
    });
    if result.is_ok() {
        println!(
            "Memo party/company snapshots contain unrelated nonexistent IDs: check={:?}; reopen={:?}",
            f.book.check(),
            Book::open(&f.path, &[24; 32]).map(|_| "accepted")
        );
    }
    assert!(result.is_err(), "foreign snapshot identities accepted");
}
