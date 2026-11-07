// Independent review only; included in a disposable source copy.

#[test]
fn review_credit_line_snapshot_shapes_must_be_rejected() {
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
        let m = memo_shell(tx, actor, co, &base, &review.display, false, false)?;
        insert(tx, "credits_creditmemo", &m)?;
        let mut rows = full["lines"].as_array().unwrap().clone();
        rows[0]["account_snapshot"] = json!({"income_account":false});
        rows[0]["item_snapshot"] = json!({"id":[],"code":false});
        lines(tx, &m, &rows)?;
        seal(tx, "memo", m["id"].as_str().unwrap())?;
        seal(tx, "journal", m["journal_id"].as_str().unwrap())?;
        Ok(())
    });
    if result.is_ok() {
        println!(
            "Malformed nested snapshots COMMITTED; check={:?}; reopen={:?}",
            f.book.check(),
            Book::open(&f.path, &[24; 32]).map(|_| "accepted")
        );
    }
    assert!(result.is_err(), "malformed frozen line snapshot committed");
}

#[test]
fn review_credit_memo_must_use_current_configured_receivables() {
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
    // This account is the designated bank, never the configured receivables.
    let wrong_ar = f.bank;
    let income = f.income;
    let tax = f.tax;
    let result: FbResult<()> = f.book.write(|tx| {
        let identity = documents::create_identity(tx, actor, co, "CM", &new_id())?;
        let source = identity["id"].as_str().unwrap();
        let mut mapping = json!({"receivables":wrong_ar.to_db(),"collected_tax":tax.to_db()});
        mapping[format!("income:{}", income.to_db())] = json!(income.to_db());
        documents::snapshot_source_accounts(tx, actor, co, source, &mapping)?;
        let mut entries = review.display["entries"].as_array().unwrap().clone();
        let credit = entries.last_mut().unwrap();
        credit["account"] = json!(wrong_ar.to_db());
        let journal = documents::post_source(
            tx,
            actor,
            co,
            source,
            "2026-10-08",
            "Synthetic review",
            entries,
        )?;
        let mut m = base.clone();
        m["id"] = json!(new_id());
        m["identity_id"] = identity["id"].clone();
        m["journal_id"] = json!(journal.id.to_db());
        m["receivables_id"] = json!(wrong_ar.to_db());
        insert(tx, "credits_creditmemo", &m)?;
        lines(tx, &m, full["lines"].as_array().unwrap())?;
        seal(tx, "memo", m["id"].as_str().unwrap())?;
        seal(tx, "journal", m["journal_id"].as_str().unwrap())?;
        Ok(())
    });
    if result.is_ok() {
        println!(
            "Memo credited designated bank instead of configured AR; check={:?}; reopen={:?}",
            f.book.check(),
            Book::open(&f.path, &[24; 32]).map(|_| "accepted")
        );
    }
    assert!(
        result.is_err(),
        "memo posted credit to non-designated receivables"
    );
}
