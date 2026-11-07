use super::*;

#[test]
fn review_reconciliation_numeric_boolean_must_be_rejected() {
    let mut f = Fx::new();
    let late = f.journal("2026-11-01", true, "10.00");
    f.clear(&late);
    let r = f.begin("10.00");
    let key = s(&r, "id").unwrap().to_owned();
    let result: FbResult<()> = f.book.write(|tx| {
        tx.execute("UPDATE banking_reconciliation SET hide_later=0,revision=2,data=json_set(data,'$.hide_later',0,'$.revision',2) WHERE id=?1", [&key])?;
        seal(tx, "reconciliation", &format!("{key}:2"))?;
        Ok(())
    });
    if result.is_ok() {
        let q = f.book.connection();
        let rec = reconciliation(q, f.c, &key, &f.bank.to_db()).unwrap();
        let totals = figures(q, f.c, &rec).unwrap();
        println!(
            "numeric hide_later accepted; totals={totals}; check={:?}; reopen={}",
            f.book.check(),
            Book::open(&f.path, &[87; 32]).is_ok()
        );
    }
    assert!(
        result.is_err(),
        "JSON integer 0 was accepted where a boolean is required"
    );
}

#[test]
fn review_deposit_snapshot_must_be_validated_at_insert_and_open() {
    let mut f = Fx::new();
    let receipt = f.sale("10.00");
    let u = f.u;
    let c = f.c;
    let bank = f.bank.to_db();
    let uf = f.uf.to_db();
    let key = new_id();
    let result: FbResult<()> = f.book.write(|tx| {
        let source = documents::create_identity(tx,u,c,"DEP",&new_id())?;
        let sid = s(&source,"id")?;
        documents::snapshot_source_accounts(tx,u,c,sid,&json!({"bank":bank,format!("undeposited:{uf}"):uf}))?;
        let journal = documents::post_source(tx,u,c,sid,"2026-10-02","Synthetic review deposit",vec![json!({"account":bank,"debit":"10.00"}),json!({"account":uf,"credit":"10.00"})])?;
        let data = json!({"id":key,"company_id":c.to_db(),"identity_id":sid,"bank_id":bank,"journal_id":journal.id.to_db(),"date":"2026-10-02","amount":"10.00","creation_digest":"a".repeat(64),"review_digest":"b".repeat(64),"company_snapshot":[],"memo":null,"reference":12,"actor_name":{}});
        tx.execute("INSERT INTO banking_deposit(id,company_id,identity_id,bank_id,journal_id,date,amount,data,actor_id) VALUES(?1,?2,?3,?4,?5,'2026-10-02',1000,?6,?7)",params![key,c.to_db(),sid,bank,journal.id.to_db(),data.to_string(),u.to_db()])?;
        tx.execute("INSERT INTO banking_depositline(id,deposit_id,receipt_id,undeposited_id,amount) VALUES(?1,?2,?3,?4,1000)",params![new_id(),key,s(&receipt,"id")?,uf])?;
        seal(tx,"deposit",&key)?;
        seal(tx,"journal",&journal.id.to_db())?;
        Ok(())
    });
    if result.is_ok() {
        println!(
            "malformed deposit accepted; check={:?}; reopen={}",
            f.book.check(),
            Book::open(&f.path, &[87; 32]).is_ok()
        );
    }
    assert!(
        result.is_err(),
        "Malformed snapshot, memo, reference and actor name committed"
    );
}

#[test]
fn review_completed_reconciliation_requires_uuid_key() {
    let mut f = Fx::new();
    let r = f.begin("0.00");
    let key = s(&r, "id").unwrap().to_owned();
    let u = f.u;
    let result: FbResult<()> = f.book.write(|tx| {
        tx.execute("UPDATE banking_reconciliation SET status='DONE',revision=2,data=json_set(data,'$.status','DONE','$.revision',2,'$.beginning_balance','0.00','$.cleared_debit','0.00','$.cleared_credit','0.00','$.cleared_count',0,'$.completion_key',?1,'$.completed_by','banking-test') WHERE id=?2",params!["z".repeat(36),key])?;
        tx.execute("INSERT INTO banking_reconciliationactor VALUES(?1,'COMPLETE',?2)",params![key,u.to_db()])?;
        seal(tx,"reconciliation",&format!("{key}:2"))?;
        Ok(())
    });
    if result.is_ok() {
        println!(
            "non-UUID completion key accepted; check={:?}; reopen={}",
            f.book.check(),
            Book::open(&f.path, &[87; 32]).is_ok()
        );
    }
    assert!(result.is_err(), "Completion key is only length-checked");
}

#[test]
fn review_reconciliation_invalid_calendar_date_must_be_rejected() {
    let mut f = Fx::new();
    let r = f.begin("0.00");
    let key = s(&r, "id").unwrap().to_owned();
    let before = f.snap();
    const ATTACK: &str = "UPDATE banking_reconciliation SET statement_date='2026-99-99',revision=2,data=json_set(data,'$.statement_date','2026-99-99','$.revision',2) WHERE id=?1";
    // Dual variant (a): with 0130 active its shared date guard answers first.
    let result: FbResult<()> = f.book.write(|tx| {
        tx.execute(ATTACK, [&key])?;
        seal(tx, "reconciliation", &format!("{key}:2"))?;
        Ok(())
    });
    if result.is_ok() {
        println!(
            "invalid calendar date accepted; check={:?}; reopen={}",
            f.book.check(),
            Book::open(&f.path, &[87; 32]).is_ok()
        );
    }
    let e = result.expect_err("SQL date CHECK accepts NULL from invalid date parsing");
    assert_eq!(e.constraint(), Some("fb_valid"), "{e}");
    assert_eq!(f.snap(), before);
    // Dual variant (b): with only fb_valid_banking_reconciliation_update dropped in a savepoint,
    // banking's own date guard still refuses the impossible date.
    let q = f.book.connection();
    q.execute_batch(
        "SAVEPOINT banking_own_date_guard;DROP TRIGGER fb_valid_banking_reconciliation_update",
    )
    .unwrap();
    let own = q.execute(ATTACK, [&key]);
    q.execute_batch("ROLLBACK TO banking_own_date_guard;RELEASE banking_own_date_guard")
        .unwrap();
    let e = own.expect_err("banking's own date guard accepted 2026-99-99");
    assert_eq!(
        fb_db::constraint_of(&e).as_deref(),
        Some("banking_evidence"),
        "{e}"
    );
    assert_eq!(f.snap(), before);
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn review_screen_can_display_cumulative_cleared_amounts() {
    let mut f = Fx::new();
    for _ in 0..2 {
        let line = f.journal("2026-10-01", true, "99999999999999.99");
        f.clear(&line);
    }
    let r = f.begin("0.00");
    let result = screen(&mut f.book, f.u, f.c, s(&r, "id").unwrap(), &f.bank.to_db());
    assert!(
        result.is_ok(),
        "Python can display aggregate amounts above one document's limit: {result:?}"
    );
}

#[test]
fn review_register_reversal_number_is_not_entry_number() {
    let mut f = Fx::new();
    let line = f.journal("2026-10-01", true, "1.00");
    let jid: String = f
        .book
        .connection()
        .query_row(
            "SELECT journal_id FROM ledger_journalline WHERE id=?1",
            [line],
            |r| r.get(0),
        )
        .unwrap();
    let reversal = reverse_journal(
        &mut f.book,
        f.u,
        f.c,
        jid.parse().unwrap(),
        "2026-10-02",
        "Correction",
    )
    .unwrap();
    let number = format!("J{:06}", reversal.number);
    let result = build(
        &mut f.book,
        f.u,
        f.c,
        &f.bank.to_db(),
        &json!({"number":number}),
        1,
        50,
    )
    .unwrap();
    assert_eq!(
        result["total"], 0,
        "Python searches journal.entry_number, not the reversal's generated number"
    );
}

#[test]
fn review_register_currency_amount_filter_matches_python() {
    let mut f = Fx::new();
    f.journal("2026-10-01", true, "1250.00");
    let result = build(
        &mut f.book,
        f.u,
        f.c,
        &f.bank.to_db(),
        &json!({"amount":"$1,250.00"}),
        1,
        50,
    );
    assert!(
        result.is_ok(),
        "Python accepts currency and comma amount filters: {result:?}"
    );
}

#[test]
fn review_register_blank_filter_matches_python() {
    let mut f = Fx::new();
    f.journal("2026-10-01", true, "1.00");
    let result = build(
        &mut f.book,
        f.u,
        f.c,
        &f.bank.to_db(),
        &json!({"amount":"","start":"","end":""}),
        1,
        50,
    );
    assert!(
        result.is_ok(),
        "Python treats blank optional filters as absent: {result:?}"
    );
}

#[test]
fn review_register_generated_number_is_not_entry_number() {
    let mut f = Fx::new();
    f.journal("2026-10-01", true, "1.00");
    let result = build(
        &mut f.book,
        f.u,
        f.c,
        &f.bank.to_db(),
        &json!({"number":"J000001"}),
        1,
        50,
    )
    .unwrap();
    assert_eq!(
        result["total"], 0,
        "Python searches journal.entry_number, not its generated display number"
    );
}

#[test]
fn review_new_charge_requires_currently_eligible_account() {
    let mut f = Fx::new();
    let line = f.journal("2026-10-31", false, "1.00");
    let jid: String = f
        .book
        .connection()
        .query_row(
            "SELECT journal_id FROM ledger_journalline WHERE id=?1",
            [line],
            |r| r.get(0),
        )
        .unwrap();
    let mut edit = AccountInput::new("500", "Expense").kind("EXPENSE");
    edit.account = Some(f.expense);
    edit.active = false;
    services::save_account(&mut f.book, f.u, f.c, edit).unwrap();
    let r = f.begin("0.00");
    let key = s(&r, "id").unwrap().to_owned();
    let expense = f.expense.to_db();
    let service = begin(
        &mut f.book,
        f.u,
        f.c,
        &f.bank.to_db(),
        &json!({"request_key":new_id(),"statement_date":"2026-10-31","ending_balance":"0.00","expected_revision":1,"service_charge":"1.00","service_account_id":expense}),
    );
    assert!(service.is_err());
    let result: FbResult<()> = f.book.write(|tx| {
        tx.execute("UPDATE banking_reconciliation SET revision=2,data=json_set(data,'$.revision',2,'$.service_charge','1.00','$.service_account_id',?1,'$.service_journal_id',?2) WHERE id=?3",params![expense,jid,key])?;
        seal(tx,"reconciliation",&format!("{key}:2"))?;
        Ok(())
    });
    if result.is_ok() {
        println!(
            "new inactive charge reference accepted; service={service:?}; check={:?}; reopen={}",
            f.book.check(),
            Book::open(&f.path, &[87; 32]).is_ok()
        );
    }
    assert!(
        result.is_err(),
        "SQL attaches a new inactive account reference forbidden by begin()"
    );
}

#[test]
fn review_register_presets_validate_and_use_injected_clock() {
    let mut f = Fx::new();
    f.journal("2026-10-01", true, "1.00");
    assert!(
        build(
            &mut f.book,
            f.u,
            f.c,
            &f.bank.to_db(),
            &json!({"preset":"not-a-preset"}),
            1,
            50
        )
        .is_err()
    );
    let out = build_on(
        &mut f.book,
        f.u,
        f.c,
        &f.bank.to_db(),
        &json!({"preset":"last_month"}),
        1,
        50,
        "2026-10-30",
    )
    .unwrap();
    assert_eq!(out["total"], 0);
}
