use super::attacks::snapshot;
use super::*;

fn append_raw(tx: &Writer<'_>, actor: UserId, base: &Value, line: &Value) -> FbResult<()> {
    let key = base["id"].as_str().unwrap();
    tx.execute("UPDATE sales_invoice SET revision=2 WHERE id=?", [key])?;
    let mut h = base["current"].clone();
    let rid = new_id();
    h["id"] = json!(rid);
    h["number"] = json!(2);
    h["action"] = json!("revised");
    tx.execute(
        "INSERT INTO sales_invoicerevision VALUES(?1,?2,2,?3,?4,?5,'DRAFT',?6,NULL,NULL,?7,?8)",
        params![
            rid,
            key,
            h["customer_id"].as_str(),
            h["date"].as_str(),
            h["due_date"].as_str(),
            cents(&h["total"])?,
            h.to_string(),
            actor.to_db()
        ],
    )?;
    super::attacks::insert_line(tx, &rid, line)?;
    seal(tx, "sales_invoice_revision", &rid)
}

#[test]
fn snapshots_reject_wrong_shapes_partial_objects_and_mismatched_identities_at_insert() {
    let mut f = Fixture::new();
    let base = f.save();
    let before = snapshot(&f.book);
    let mut mutations = Vec::new();
    for path in [
        "/account_snapshot",
        "/item_snapshot",
        "/account_snapshot/income_account",
    ] {
        for bad in [Value::Null, json!(false), json!(4), json!("bad"), json!([])] {
            mutations.push((path, bad));
        }
    }
    mutations.extend([
        ("/account_snapshot/income_account", json!({})),
        ("/account_snapshot/income_account/id", json!(new_id())),
        ("/account_snapshot/income_account/kind", json!([])),
        ("/item_snapshot", json!({"id":new_id()})),
    ]);
    for (path, bad) in mutations {
        let mut line = base["lines"][0].clone();
        *line.pointer_mut(path).unwrap() = bad.clone();
        let actor = f.actor;
        let result = f.book.write(|tx| append_raw(tx, actor, &base, &line));
        assert!(result.is_err(), "committed {path}={bad}");
        assert_eq!(snapshot(&f.book), before);
    }
}

#[test]
fn selected_item_snapshot_requires_complete_fields_and_matching_identity() {
    let mut f = Fixture::new();
    let item = directory::save_record(
        &mut f.book,
        f.actor,
        f.company,
        Model::Item,
        &json!({"code":"SHAPE","description":"Synthetic item","kind":"SERVICE","sale_rate":"1.00"}),
        None,
    )
    .unwrap();
    let mut input = f.input();
    input["lines"][0]["item_id"] = item["id"].clone();
    let base = save_invoice(&mut f.book, f.actor, f.company, &input).unwrap();
    let before = snapshot(&f.book);
    let actor = f.actor;
    for (field, bad) in [
        ("sale_rate", json!(1)),
        ("sale_rate", json!(true)),
        ("sale_rate", json!("not money")),
        ("id", json!(new_id())),
        ("kind", Value::Null),
    ] {
        let mut line = base["lines"][0].clone();
        line["item_snapshot"][field] = bad;
        assert!(
            f.book
                .write(|tx| append_raw(tx, actor, &base, &line))
                .is_err()
        );
        assert_eq!(snapshot(&f.book), before);
    }
}

fn replace_data(f: &Fixture, table: &str, key: &str, data: &Value) {
    let conn = f.book.connection();
    let guards: Vec<(String, String)> = conn
        .prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger' AND tbl_name=?")
        .unwrap()
        .query_map([table], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for (name, _) in &guards {
        conn.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
    }
    conn.execute(
        &format!("UPDATE {table} SET data=?1 WHERE id=?2"),
        params![data.to_string(), key],
    )
    .unwrap();
    for (_, sql) in guards {
        conn.execute_batch(&sql).unwrap();
    }
}

#[test]
fn restored_guards_do_not_hide_corrupt_snapshots_on_check_open_or_public_save() {
    let mut f = Fixture::new();
    let base = f.save();
    let (line_id, line_data): (String, String) = f
        .book
        .connection()
        .query_row("SELECT id,data FROM sales_invoiceline LIMIT 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    let original: Value = serde_json::from_str(&line_data).unwrap();
    let path: String = f
        .book
        .connection()
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name='main'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    for bad in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"income_account":{"id":"wrong","code":"400","name":"Work","kind":"INCOME"}}),
    ] {
        let mut line = original.clone();
        line["account_snapshot"] = bad;
        replace_data(&f, "sales_invoiceline", &line_id, &line);
        assert!(!f.book.check().unwrap().is_empty());
        assert!(Book::open(std::path::Path::new(&path), &[13; 32]).is_err());
        let mut input = f.input();
        input["invoice_id"] = base["id"].clone();
        input["expected_revision"] = json!(1);
        input["lines"][0]["income_account_id"] = Value::Null;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            save_invoice(&mut f.book, f.actor, f.company, &input)
        }));
        assert!(result.is_ok());
        assert!(result.unwrap().is_err());
        replace_data(&f, "sales_invoiceline", &line_id, &original);
        assert!(f.book.check().unwrap().is_empty());
    }
    for field in ["customer_snapshot", "company_snapshot"] {
        let mut header = base["current"].clone();
        header[field]["id"] = json!(new_id());
        let rid = base["current"]["id"].as_str().unwrap();
        replace_data(&f, "sales_invoicerevision", rid, &header);
        assert!(!f.book.check().unwrap().is_empty());
        assert!(Book::open(std::path::Path::new(&path), &[13; 32]).is_err());
        replace_data(&f, "sales_invoicerevision", rid, &base["current"]);
    }
}

#[test]
fn receipt_sale_and_reversal_snapshot_shapes_are_rechecked_on_open() {
    let mut f = Fixture::new();
    let invoice = f.posted();
    let payment = f.payment(invoice["id"].as_str().unwrap(), "1.00");
    let review = review_receipt(&mut f.book, f.actor, f.company, &payment).unwrap();
    let receipt = record_receipt(&mut f.book, f.actor, f.company, &payment, Some(&review)).unwrap();
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
    let input = f.sale_input();
    let review = review_sales_receipt(&mut f.book, f.actor, f.company, &input).unwrap();
    record_sales_receipt(&mut f.book, f.actor, f.company, &input, Some(&review)).unwrap();
    let path: String = f
        .book
        .connection()
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name='main'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    for (table, fields) in [
        (
            "sales_receipt",
            &["customer_snapshot", "company_snapshot"][..],
        ),
        (
            "sales_salesreceiptline",
            &["account_snapshot", "item_snapshot"][..],
        ),
        ("sales_salesreceipt", &["tax_snapshot"][..]),
        ("sales_receiptreversal", &["reason", "actor_name"][..]),
    ] {
        let (key, raw): (String, String) = f
            .book
            .connection()
            .query_row(&format!("SELECT id,data FROM {table} LIMIT 1"), [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        let original: Value = serde_json::from_str(&raw).unwrap();
        for field in fields {
            for bad in [Value::Null, json!(false), json!([]), json!("wrong shape")] {
                // A text reason or actor is legal; malformed shapes are not.
                if table == "sales_receiptreversal" && bad.is_string() {
                    continue;
                }
                let mut corrupt = original.clone();
                corrupt[*field] = bad;
                replace_data(&f, table, &key, &corrupt);
                assert!(
                    !f.book.check().unwrap().is_empty(),
                    "accepted {table}.{field}"
                );
                assert!(
                    Book::open(std::path::Path::new(&path), &[13; 32]).is_err(),
                    "reopened {table}.{field}"
                );
                replace_data(&f, table, &key, &original);
            }
        }
    }
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn seal_revalidates_snapshot_shape_even_if_insert_guard_was_bypassed() {
    let mut f = Fixture::new();
    let base = f.save();
    let before = snapshot(&f.book);
    let actor = f.actor;
    f.book
        .write(|tx| -> FbResult<()> {
            let mut line = base["lines"][0].clone();
            // Insert correct evidence first, then simulate an external corruptor.
            // Authorised callbacks cannot disable guards, so this fixture uses the
            // dedicated dev-only escape hatch outside a callback below instead.
            line["account_snapshot"] = Value::Null;
            append_raw(tx, actor, &base, &line)
        })
        .unwrap_err();
    assert_eq!(snapshot(&f.book), before);
    let conn = f.book.connection();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut h = base["current"].clone();
    let rid = new_id();
    h["id"] = json!(rid);
    h["number"] = json!(2);
    h["action"] = json!("revised");
    conn.execute(
        "UPDATE sales_invoice SET revision=2 WHERE id=?",
        [base["id"].as_str().unwrap()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sales_invoicerevision VALUES(?1,?2,2,?3,?4,?5,'DRAFT',426,NULL,NULL,?6,?7)",
        params![
            rid,
            base["id"].as_str(),
            f.customer,
            h["date"].as_str(),
            h["due_date"].as_str(),
            h.to_string(),
            f.actor.to_db()
        ],
    )
    .unwrap();
    let mut line = base["lines"][0].clone();
    line["account_snapshot"] = Value::Null;
    // Only the two insertion checks are bypassed; the seal checks remain live.
    conn.execute_batch(
        "DROP TRIGGER fb_sales_line_snapshot; DROP TRIGGER fb_sales_invoice_line_insert;",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sales_invoiceline VALUES(?1,?2,1,NULL,?3,NULL,401,1,?4)",
        params![new_id(), rid, f.income.to_db(), line.to_string()],
    )
    .unwrap();
    assert!(
        conn.execute(
            "INSERT INTO sales_seal VALUES('sales_invoice_revision',?)",
            [&rid]
        )
        .is_err()
    );
    conn.execute_batch("ROLLBACK").unwrap();
    assert_eq!(snapshot(&f.book), before);
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn origin_provenance_is_checked_and_cannot_duplicate_or_borrow_another_revision() {
    let mut f = Fixture::new();
    let base = f.save();
    let before = snapshot(&f.book);
    for (position, revision) in [
        (json!(1), json!(new_id())),
        (json!(2), base["current"]["id"].clone()),
        (json!("1"), base["current"]["id"].clone()),
        (Value::Null, base["current"]["id"].clone()),
    ] {
        let mut line = base["lines"][0].clone();
        line["origin_position"] = position;
        line["origin_revision_id"] = revision;
        let actor = f.actor;
        assert!(
            f.book
                .write(|tx| append_raw(tx, actor, &base, &line))
                .is_err()
        );
        assert_eq!(snapshot(&f.book), before);
    }
    let mut input = f.input();
    input["invoice_id"] = base["id"].clone();
    input["expected_revision"] = json!(1);
    input["lines"][0]["origin_position"] = json!(1);
    let line = input["lines"][0].clone();
    input["lines"].as_array_mut().unwrap().push(line);
    assert!(save_invoice(&mut f.book, f.actor, f.company, &input).is_err());
    assert_eq!(snapshot(&f.book), before);
    let actor = f.actor;
    f.book.write(|tx| -> FbResult<()> {
        let key = base["id"].as_str().unwrap();
        tx.execute("UPDATE sales_invoice SET revision=2 WHERE id=?", [key])?;
        let mut h = base["current"].clone(); let rid = new_id();
        h["id"] = json!(rid); h["number"] = json!(2); h["action"] = json!("revised");
        tx.execute("INSERT INTO sales_invoicerevision VALUES(?1,?2,2,?3,?4,?5,'DRAFT',426,NULL,NULL,?6,?7)",params![rid,key,h["customer_id"].as_str(),h["date"].as_str(),h["due_date"].as_str(),h.to_string(),actor.to_db()])?;
        let mut line = base["lines"][0].clone(); line["origin_position"] = json!(1); line["origin_revision_id"] = base["current"]["id"].clone();
        super::attacks::insert_line(tx, &rid, &line)?;
        line["position"] = json!(2);
        let error = super::attacks::insert_line(tx, &rid, &line).unwrap_err();
        assert_eq!(fb_db::constraint_of(&error).as_deref(), Some("invoice_line_origin"));
        Err(FbError::validation("roll back duplicate-origin fixture"))
    }).unwrap_err();
    assert_eq!(snapshot(&f.book), before);
    f.book
        .write(|tx| {
            tx.execute(
                "UPDATE ledger_account SET active=0 WHERE id=?",
                [f.income.to_db()],
            )
        })
        .unwrap();
    let before = snapshot(&f.book);
    // An old position without explicit provenance does not confer retention.
    assert!(
        f.book
            .write(|tx| append_raw(tx, actor, &base, &base["lines"][0]))
            .is_err()
    );
    assert_eq!(snapshot(&f.book), before);
    let mut retained = base["lines"][0].clone();
    retained["origin_position"] = json!(1);
    retained["origin_revision_id"] = base["current"]["id"].clone();
    f.book
        .write(|tx| append_raw(tx, actor, &base, &retained))
        .unwrap();
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn both_receipt_kinds_require_destination_designations_and_preserve_history_after_setup_changes() {
    for kind in ["RCP", "SR"] {
        for destination in ["BANK", "UNDEPOSITED"] {
            let mut f = Fixture::new();
            let inv = f.posted();
            let other = services::save_account(
                &mut f.book,
                f.actor,
                f.company,
                AccountInput::new("155", "Other asset").kind("ASSET"),
            )
            .unwrap();
            let before = snapshot(&f.book);
            let result = f.book.write(|tx| -> FbResult<()> {
                let source = documents::create_identity(tx,f.actor,f.company,kind,&new_id())?;
                let sid = source["id"].as_str().unwrap();
                let credit = if kind=="RCP" { f.ar } else { f.income };
                let role = if kind=="RCP" { "receivables" } else { "income" };
                documents::snapshot_source_accounts(tx,f.actor,f.company,sid,&json!({"destination":other.id.to_db(),format!("{role}:{}",credit.to_db()):credit.to_db()}))?;
                let j = documents::post_source(tx,f.actor,f.company,sid,"2026-10-02","Invalid synthetic destination",entries(&other.id.to_db(),100,BTreeMap::from([(credit.to_db(),100)])))?;
                let key = new_id();
                let data = json!({"id":key,"identity_id":sid,"kind":kind,"customer_id":f.customer,"customer_snapshot":party(&directory::get(tx,f.company,Model::Customer,&f.customer)?),"company_snapshot":company_snapshot(tx,f.company)?,"date":"2026-10-02","amount":"1.00","method":"CASH","reference":"","memo":"","destination":destination,"destination_account_id":other.id.to_db(),"journal_id":j.id.to_db(),"creation_digest":"0".repeat(64),"review_digest":"1".repeat(64),"actor_name":"Sales tester"});
                // Assert immediate rejection: no incomplete-document COMMIT can
                // accidentally make this destination test pass.
                let error = tx.execute("INSERT INTO sales_receipt VALUES(?1,?2,?3,?4,?5,'2026-10-02',100,?6,?7,?8,?9,?10)",params![key,f.company.to_db(),sid,kind,f.customer,destination,other.id.to_db(),j.id.to_db(),data.to_string(),f.actor.to_db()]).unwrap_err();
                assert_eq!(fb_db::constraint_of(&error).as_deref(),Some("receipt_destination"));
                Err(FbError::validation("roll back synthetic rejected journal"))
            });
            assert!(result.is_err());
            assert_eq!(snapshot(&f.book), before);
            let mut input = if kind == "RCP" {
                f.payment(inv["id"].as_str().unwrap(), "1.00")
            } else {
                f.sale_input()
            };
            input["destination"] = json!(destination);
            input["bank_id"] = if destination == "BANK" {
                json!(f.bank.to_db())
            } else {
                Value::Null
            };
            let receipt = if kind == "RCP" {
                let review = review_receipt(&mut f.book, f.actor, f.company, &input).unwrap();
                record_receipt(&mut f.book, f.actor, f.company, &input, Some(&review)).unwrap()
            } else {
                let review = review_sales_receipt(&mut f.book, f.actor, f.company, &input).unwrap();
                record_sales_receipt(&mut f.book, f.actor, f.company, &input, Some(&review))
                    .unwrap()
            };
            documents::save_setup(&mut f.book,f.actor,f.company,&json!({"receivables":f.ar.to_db(),"undeposited":other.id.to_db(),"collected_tax":f.tax.to_db(),"banks":[]}),Some(1)).unwrap();
            assert_eq!(
                f.book
                    .read(|r| super::super::receipt(r, f.company, receipt["id"].as_str().unwrap()))
                    .unwrap(),
                receipt
            );
            assert!(f.book.check().unwrap().is_empty());
            let path: String = f
                .book
                .connection()
                .query_row(
                    "SELECT file FROM pragma_database_list WHERE name='main'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(
                Book::open(std::path::Path::new(&path), &[13; 32])
                    .unwrap()
                    .check()
                    .unwrap()
                    .is_empty()
            );
            if kind == "RCP" {
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
            } else {
                void_sales_receipt(
                    &mut f.book,
                    f.actor,
                    f.company,
                    receipt["id"].as_str().unwrap(),
                    Some(1),
                    &json!("2026-10-03"),
                    &json!("Correction"),
                )
                .unwrap();
            }
            assert!(f.book.check().unwrap().is_empty());
        }
    }
}
