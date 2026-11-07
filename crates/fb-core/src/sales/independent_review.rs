use super::*;

#[test]
fn review_archived_item_can_move_with_origin_position() {
    let mut f = Fixture::new();
    let item = directory::save_record(
        &mut f.book,
        f.actor,
        f.company,
        Model::Item,
        &json!({"code":"MOVED","description":"Kept item","kind":"SERVICE","sale_rate":"1"}),
        None,
    )
    .unwrap();
    let mut input = f.input();
    let mut second = input["lines"][0].clone();
    second["description"] = json!("Other line");
    input["lines"][0]["item_id"] = item["id"].clone();
    input["lines"].as_array_mut().unwrap().push(second);
    let inv = save_invoice(&mut f.book, f.actor, f.company, &input).unwrap();
    directory::set_active(
        &mut f.book,
        f.actor,
        f.company,
        Model::Item,
        item["id"].as_str().unwrap(),
        false,
    )
    .unwrap();
    input["invoice_id"] = inv["id"].clone();
    input["expected_revision"] = json!(1);
    input["lines"][0]["origin_position"] = json!(1);
    input["lines"][0]["keep_archived"] = json!(true);
    input["lines"][1]["origin_position"] = json!(2);
    input["lines"].as_array_mut().unwrap().reverse();
    let result = save_invoice(&mut f.book, f.actor, f.company, &input);
    assert!(
        result.is_ok(),
        "valid retained archived item reorder refused: {result:?}"
    );
}

#[test]
fn review_inactive_income_can_move_with_origin_position() {
    let mut f = Fixture::new();
    let alternate = services::save_account(
        &mut f.book,
        f.actor,
        f.company,
        AccountInput::new("401", "Alternate").kind("INCOME"),
    )
    .unwrap();
    let mut input = f.input();
    let mut second = input["lines"][0].clone();
    second["income_account_id"] = json!(alternate.id.to_db());
    input["lines"].as_array_mut().unwrap().push(second);
    let inv = save_invoice(&mut f.book, f.actor, f.company, &input).unwrap();
    f.book
        .write(|tx| {
            tx.execute(
                "UPDATE ledger_account SET active=0 WHERE id=?",
                [f.income.to_db()],
            )
        })
        .unwrap();
    input["invoice_id"] = inv["id"].clone();
    input["expected_revision"] = json!(1);
    input["lines"][0]["origin_position"] = json!(1);
    input["lines"][1]["origin_position"] = json!(2);
    input["lines"].as_array_mut().unwrap().reverse();
    let result = save_invoice(&mut f.book, f.actor, f.company, &input);
    assert!(
        result.is_ok(),
        "valid retained inactive income reorder refused: {result:?}"
    );
}

#[test]
fn review_malformed_snapshot_must_be_rejected_before_durable_commit() {
    let mut f = Fixture::new();
    let base = f.save();
    let key = base["id"].as_str().unwrap();
    let result = f.book.write(|tx| -> FbResult<()> {
        tx.execute("UPDATE sales_invoice SET revision=2 WHERE id=?",[key])?;
        let mut header = base["current"].clone();
        let rid = new_id();
        header["id"] = json!(rid);
        header["number"] = json!(2);
        header["action"] = json!("revised");
        let mut lines = base["lines"].as_array().unwrap().clone();
        lines[0]["account_snapshot"] = Value::Null;
        tx.execute("INSERT INTO sales_invoicerevision VALUES(?1,?2,2,?3,'2026-10-01','2026-10-31','DRAFT',426,NULL,NULL,?4,?5)",params![rid,key,f.customer,header.to_string(),f.actor.to_db()])?;
        let l=&lines[0];
        tx.execute("INSERT INTO sales_invoiceline VALUES(?1,?2,1,NULL,?3,NULL,401,1,?4)",params![new_id(),rid,f.income.to_db(),l.to_string()])?;
        seal(tx,"sales_invoice_revision",&rid)?;
        Ok(())
    });
    if result.is_ok() {
        assert!(
            f.book.check().unwrap().is_empty(),
            "check should detect malformed persisted snapshots"
        );
        let path: String = f
            .book
            .connection()
            .query_row(
                "SELECT file FROM pragma_database_list WHERE name='main'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let reopened = Book::open(std::path::Path::new(&path), &[13; 32])
            .expect("malformed snapshot also reopened successfully");
        assert!(reopened.check().unwrap().is_empty());
        let mut input = f.input();
        input["invoice_id"] = json!(key);
        input["expected_revision"] = json!(2);
        input["lines"][0]["income_account_id"] = Value::Null;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            save_invoice(&mut f.book, f.actor, f.company, &input)
        }));
        assert!(
            outcome.is_ok(),
            "malformed snapshot committed, check passed, then public save_invoice panicked"
        );
    }
    assert!(result.is_err(), "malformed snapshot committed successfully");
}

#[test]
fn review_receipt_sql_must_enforce_designated_bank() {
    let mut f = Fixture::new();
    let inv = f.posted();
    let key = inv["id"].as_str().unwrap();
    let other = services::save_account(
        &mut f.book,
        f.actor,
        f.company,
        AccountInput::new("155", "Other asset").kind("ASSET"),
    )
    .unwrap();
    let mut input = f.payment(key, "1.00");
    input["bank_id"] = json!(other.id.to_db());
    assert!(review_receipt(&mut f.book, f.actor, f.company, &input).is_err());
    let result = f.book.write(|tx| -> FbResult<()> {
        let source=documents::create_identity(tx,f.actor,f.company,"RCP",&new_id())?;
        let sid=source["id"].as_str().unwrap();
        let mut mapping=json!({"destination":other.id.to_db()});
        mapping[format!("receivables:{}",f.ar.to_db())]=json!(f.ar.to_db());
        documents::snapshot_source_accounts(tx,f.actor,f.company,sid,&mapping)?;
        let entries=vec![json!({"account":other.id.to_db(),"debit":"1.00","credit":"0.00"}),json!({"account":f.ar.to_db(),"debit":"0.00","credit":"1.00"})];
        let j=documents::post_source(tx,f.actor,f.company,sid,"2026-10-02","Synthetic invalid bank",entries)?;
        let id=new_id();
        let data=json!({"id":id,"identity_id":sid,"kind":"RCP","customer_id":f.customer,"date":"2026-10-02","amount":"1.00","destination":"BANK","destination_account_id":other.id.to_db(),"journal_id":j.id.to_db(),"method":"CASH","reference":"","memo":"","creation_digest":"0".repeat(64),"review_digest":"1".repeat(64),"actor_name":"Sales tester","customer_snapshot":base_customer(tx,f.company,&f.customer)?,"company_snapshot":company_snapshot(tx,f.company)?});
        tx.execute("INSERT INTO sales_receipt VALUES(?1,?2,?3,'RCP',?4,'2026-10-02',100,'BANK',?5,?6,?7,?8)",params![id,f.company.to_db(),sid,f.customer,other.id.to_db(),j.id.to_db(),data.to_string(),f.actor.to_db()])?;
        tx.execute("INSERT INTO sales_receiptallocation VALUES(?1,?2,?3,?4,?5,100)",params![new_id(),id,key,inv["current"]["id"].as_str(),f.ar.to_db()])?;
        seal(tx,"sales_receipt",&id)?;
        finish_journal(tx,&j.id.to_db())?;
        Ok(())
    });
    if result.is_ok() {
        assert!(f.book.check().unwrap().is_empty());
        assert_eq!(
            invoice_balances(&mut f.book, f.actor, f.company).unwrap()[0]["outstanding"],
            "3.26"
        );
    }
    assert!(
        result.is_err(),
        "SQL committed receipt to a non-designated bank and check passed"
    );
}

fn base_customer(tx: &Writer<'_>, c: CompanyId, key: &str) -> FbResult<Value> {
    Ok(party(&directory::get(tx, c, Model::Customer, key)?))
}
