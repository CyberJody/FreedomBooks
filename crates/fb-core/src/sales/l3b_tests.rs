use super::*;

#[test]
fn noop_invoice_identity_can_accompany_a_valid_revision_advance() {
    let mut f = Fixture::new();
    let b = f.save();
    let c = f.book.connection();
    let before = super::attacks::snapshot(&f.book);
    for recursion in [0, 1] {
        c.execute_batch(&format!(
            "PRAGMA recursive_triggers={recursion}; BEGIN IMMEDIATE"
        ))
        .unwrap();
        let result = c.execute("UPDATE sales_invoice SET id=id,identity_id=identity_id,company_id=company_id,revision=revision+1 WHERE id=?", [b["id"].as_str().unwrap()]);
        c.execute_batch("ROLLBACK").unwrap();
        result.expect("unchanged keys must not block a legitimate revision advance");
        assert_eq!(super::attacks::snapshot(&f.book), before);
    }
}

#[test]
fn every_income_detail_type_matches_at_service_and_sql_boundaries() {
    let mut f = Fixture::new();
    let base = f.save();
    for (index, detail) in crate::model::DetailType::ALL.into_iter().enumerate() {
        let a = services::save_account(
            &mut f.book,
            f.actor,
            f.company,
            AccountInput::new(&format!("D{index}"), detail.as_str())
                .kind(detail.kind().as_str())
                .detail(detail.as_str()),
        )
        .unwrap();
        let expected = ["INCOME", "OTHER_INCOME"].contains(&detail.as_str());
        let mut p = f.input();
        p["lines"][0]["income_account_id"] = json!(a.id.to_db());
        let saved = save_invoice(&mut f.book, f.actor, f.company, &p);
        assert_eq!(
            saved.is_ok(),
            expected,
            "service {}: {saved:?}",
            detail.as_str()
        );
        if let Ok(saved) = saved {
            let key = saved["id"].as_str().unwrap();
            let review = review_invoice(&mut f.book, f.actor, f.company, key, Some(1)).unwrap();
            post_invoice(&mut f.book, f.actor, f.company, key, Some(1), Some(&review)).unwrap();
        }
        let before = super::attacks::snapshot(&f.book);
        f.book.write(|tx| -> FbResult<()> {
            let identity = documents::create_identity(tx, f.actor, f.company, "INV", &new_id())?;
            let key = new_id(); let rid = new_id();
            tx.execute("INSERT INTO sales_invoice VALUES(?1,?2,?3,?4,1)", params![key,f.company.to_db(),identity["id"].as_str(),"0".repeat(64)])?;
            let mut h = base["current"].clone(); h["id"] = json!(rid);
            tx.execute("INSERT INTO sales_invoicerevision VALUES(?1,?2,1,?3,'2026-10-01','2026-10-31','DRAFT',426,NULL,NULL,?4,?5)", params![rid,key,f.customer,h.to_string(),f.actor.to_db()])?;
            let mut l = base["lines"][0].clone(); l["income_account_id"] = json!(a.id.to_db()); l["account_snapshot"]["income_account"] = account_snapshot(&a);
            let result = super::attacks::insert_line(tx,&rid,&l);
            assert_eq!(result.is_ok(),expected,"SQL {}: {result:?}",detail.as_str());
            Err(FbError::validation("rollback fixture"))
        }).unwrap_err();
        assert_eq!(super::attacks::snapshot(&f.book), before);
    }
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn new_draft_customer_eligibility_matches_service_and_sql() {
    let mut f = Fixture::new();
    let base = f.save();
    f.book
        .connection()
        .execute(
            "UPDATE directory_customer SET active=0 WHERE id=?",
            [&f.customer],
        )
        .unwrap();
    let p = f.input();
    assert!(save_invoice(&mut f.book, f.actor, f.company, &p).is_err());
    let before = super::attacks::snapshot(&f.book);
    f.book.write(|tx| -> FbResult<()> {
        let identity = documents::create_identity(tx, f.actor, f.company, "INV", &new_id())?;
        let key = new_id(); let rid = new_id();
        tx.execute("INSERT INTO sales_invoice VALUES(?1,?2,?3,?4,1)", params![key,f.company.to_db(),identity["id"].as_str(),"0".repeat(64)])?;
        let mut h = base["current"].clone(); h["id"] = json!(rid);
        let error = tx.execute("INSERT INTO sales_invoicerevision VALUES(?1,?2,1,?3,'2026-10-01','2026-10-31','DRAFT',426,NULL,NULL,?4,?5)", params![rid,key,f.customer,h.to_string(),f.actor.to_db()]).expect_err("SQL accepted archived new customer");
        assert_eq!(fb_db::constraint_of(&error).as_deref(), Some("invoice_customer_eligible"));
        Err(FbError::validation("rollback fixture"))
    }).unwrap_err();
    assert_eq!(super::attacks::snapshot(&f.book), before);
}

#[test]
fn malformed_invoice_edit_id_never_creates_a_new_document() {
    let mut f = Fixture::new();
    for value in [
        json!(true),
        json!(23),
        json!({}),
        json!([]),
        json!(""),
        json!("bad"),
    ] {
        let mut input = f.input();
        input["invoice_id"] = value.clone();
        let before = super::attacks::snapshot(&f.book);
        let error = save_invoice(&mut f.book, f.actor, f.company, &input)
            .expect_err(&format!("malformed edit id created a document: {value}"));
        assert_eq!(error.kind(), "ValidationError");
        assert_eq!(super::attacks::snapshot(&f.book), before);
    }
}

// Simulate exhausted stored evidence without billions of revisions. Restore every
// exact trigger; only the synthetic sequence continuity is abbreviated.
fn exhaust(f: &Fixture, key: &str) {
    let c = f.book.connection();
    let definitions: Vec<(String, String)> = c.prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger' AND tbl_name IN('sales_invoice','sales_invoicerevision')").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
    for (name, _) in &definitions {
        c.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
    }
    c.execute("UPDATE sales_invoicerevision SET number=?1,data=json_set(data,'$.number',?1) WHERE invoice_id=?2 AND number=(SELECT revision FROM sales_invoice WHERE id=?2)", params![i64::MAX,key]).unwrap();
    c.execute(
        "UPDATE sales_invoice SET revision=? WHERE id=?",
        params![i64::MAX, key],
    )
    .unwrap();
    for (_, sql) in definitions {
        c.execute_batch(&sql).unwrap();
    }
}

#[test]
fn exhausted_invoice_revisions_fail_validation_without_mutation_or_panic() {
    for action in ["save", "post", "void"] {
        let mut f = Fixture::new();
        let inv = if action == "void" {
            f.posted()
        } else {
            f.save()
        };
        let key = inv["id"].as_str().unwrap();
        exhaust(&f, key);
        let review = if action == "post" {
            Some(review_invoice(&mut f.book, f.actor, f.company, key, Some(i64::MAX)).unwrap())
        } else {
            None
        };
        let before = super::attacks::snapshot(&f.book);
        let result = match action {
            "save" => {
                let mut p = f.input();
                p["invoice_id"] = json!(key);
                p["expected_revision"] = json!(i64::MAX);
                save_invoice(&mut f.book, f.actor, f.company, &p)
            }
            "post" => post_invoice(
                &mut f.book,
                f.actor,
                f.company,
                key,
                Some(i64::MAX),
                review.as_ref(),
            ),
            _ => void_invoice(
                &mut f.book,
                f.actor,
                f.company,
                key,
                Some(i64::MAX),
                &json!("2026-10-03"),
                &json!("Correction"),
            ),
        };
        let error = result.unwrap_err();
        assert_eq!(error.kind(), "ValidationError", "{action}: {error:?}");
        assert!(error.message().contains("exhausted"), "{action}: {error:?}");
        assert_eq!(super::attacks::snapshot(&f.book), before);
    }
}
