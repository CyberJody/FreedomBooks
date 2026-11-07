use super::*;

#[allow(clippy::too_many_arguments)] // Raw SQL fixture, not a production command.
pub(super) fn raw_receipt(
    tx: &Writer<'_>,
    actor: UserId,
    c: CompanyId,
    customer: &str,
    dest: AccountId,
    credit: AccountId,
    kind: &str,
    amount: i64,
    change: impl FnOnce(&mut Value),
) -> Result<Value, fb_db::rusqlite::Error> {
    let source = documents::create_identity(tx, actor, c, kind, &new_id()).unwrap();
    let sid = source["id"].as_str().unwrap();
    documents::snapshot_source_accounts(
        tx,
        actor,
        c,
        sid,
        &json!({"destination":dest.to_db(),format!("income:{}",credit.to_db()):credit.to_db()}),
    )
    .unwrap();
    let j = documents::post_source(
        tx,
        actor,
        c,
        sid,
        "2026-10-02",
        "Synthetic raw receipt",
        entries(
            &dest.to_db(),
            amount,
            BTreeMap::from([(credit.to_db(), amount)]),
        ),
    )
    .unwrap();
    let key = new_id();
    let mut v = json!({"id":key,"identity_id":sid,"kind":kind,"customer_id":customer,"date":"2026-10-02","amount":cash(amount),"method":"CASH","reference":"","memo":"Synthetic","destination":"BANK","destination_account_id":dest.to_db(),"journal_id":j.id.to_db(),"customer_snapshot":party(&directory::get(tx,c,Model::Customer,customer).unwrap()),"company_snapshot":company_snapshot(tx,c).unwrap(),"creation_digest":"0".repeat(64),"review_digest":"0".repeat(64),"actor_name":"Sales tester"});
    change(&mut v);
    v["customer_snapshot"]["id"] = v["customer_id"].clone();
    tx.execute(
        "INSERT INTO sales_receipt VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![
            key,
            c.to_db(),
            sid,
            kind,
            v["customer_id"].as_str(),
            v["date"].as_str(),
            amount,
            v["destination"].as_str(),
            dest.to_db(),
            j.id.to_db(),
            v.to_string(),
            actor.to_db()
        ],
    )?;
    Ok(v)
}
fn detail(base: &Value, tax: AccountId) -> Value {
    let mut d =
        json!({"tax_item_id":base["current"]["tax_item_id"],"collected_tax_id":tax.to_db()});
    copy_calc(&mut d, &base["current"]);
    d
}
#[test]
pub(super) fn receipts_allocations_details_totals_and_postings_have_independent_backstops() {
    let mut f = Fixture::new();
    let inv = f.posted();
    let state = snapshot(&f.book);
    let actor = f.actor;
    let c = f.company;
    let customer = f.customer.clone();
    let bank = f.bank;
    let income = f.income;
    let ar = f.ar;
    let tax = f.tax;
    for case in 0..12 {
        let guard: String = f
            .book
            .connection()
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='fb_sales_receipt_seal'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        if case == 10 {
            f.book
                .connection()
                .execute_batch("DROP TRIGGER fb_sales_receipt_seal")
                .unwrap();
        }
        let result: FbResult<()> = f.book.write(|tx| {
            if case == 0 || case == 1 {
                let result = raw_receipt(tx, actor, c, &customer, bank, income, "RCP", 100, |v| {
                    if case == 0 {
                        v["method"] = json!("WIRE")
                    } else {
                        v["customer_id"] = json!(new_id())
                    }
                });
                refusal(
                    result.unwrap_err(),
                    if case == 0 {
                        "receipt_fields"
                    } else {
                        "receipt_scope"
                    },
                );
                return Err(FbError::validation("rolled-back fixture"));
            }
            let sale = case >= 6;
            let amount = if sale {
                426
            } else {
                if case == 4 { 427 } else { 100 }
            };
            let r = raw_receipt(
                tx,
                actor,
                c,
                &customer,
                bank,
                income,
                if sale { "SR" } else { "RCP" },
                amount,
                |_| {},
            )
            .unwrap();
            let rid = r["id"].as_str().unwrap();
            if case == 2 {
                refusal(
                    tx.execute("INSERT INTO sales_seal VALUES('sales_receipt',?1)", [rid])
                        .unwrap_err(),
                    "receipt_complete",
                );
            }
            if case == 3 || case == 4 || case == 5 || case == 6 {
                let result = tx.execute(
                    "INSERT INTO sales_receiptallocation VALUES(?1,?2,?3,?4,?5,?6)",
                    params![
                        new_id(),
                        rid,
                        inv["id"].as_str(),
                        inv["current"]["id"].as_str(),
                        if case == 3 {
                            income.to_db()
                        } else {
                            ar.to_db()
                        },
                        amount
                    ],
                );
                match case {
                    3 => refusal(result.unwrap_err(), "allocation_scope"),
                    4 => refusal(result.unwrap_err(), "allocation_outstanding"),
                    6 => refusal(result.unwrap_err(), "sales_receipt_no_allocations"),
                    5 => {
                        result.unwrap();
                        refusal(
                            tx.execute("INSERT INTO sales_seal VALUES('sales_receipt',?1)", [rid])
                                .unwrap_err(),
                            "receipt_journal_split",
                        );
                    }
                    _ => unreachable!(),
                }
            }
            if case == 7 {
                refusal(
                    tx.execute("INSERT INTO sales_seal VALUES('sales_receipt',?1)", [rid])
                        .unwrap_err(),
                    "sales_receipt_complete",
                );
            }
            if case >= 8 {
                let mut d = detail(&inv, tax);
                if case == 8 {
                    d["total"] = json!("4.27")
                }
                if case == 9 {
                    d["collected_tax_id"] = json!(bank.to_db())
                }
                if case == 10 {
                    // A committed, incomplete SR cannot exist normally. Forge that state inside a
                    // rolled-back attack transaction to exercise the independent late-detail guard.
                    tx.execute("INSERT INTO sales_seal VALUES('sales_receipt',?1)", [rid])
                        .unwrap();
                }
                let sid = new_id();
                let result = tx.execute(
                    "INSERT INTO sales_salesreceipt VALUES(?1,?2,?3,?4)",
                    params![sid, rid, if case == 8 { 427 } else { 426 }, d.to_string()],
                );
                match case {
                    8 => refusal(result.unwrap_err(), "sales_receipt_fields"),
                    9 => refusal(result.unwrap_err(), "sales_tax_account"),
                    10 => refusal(result.unwrap_err(), "sales_receipt_late_detail"),
                    11 => {
                        result.unwrap();
                        let mut l = inv["lines"][0].clone();
                        l["amount"] = json!("3.00");
                        refusal(
                        tx.execute(
                            "INSERT INTO sales_salesreceiptline VALUES(?1,?2,1,NULL,?3,300,1,?4)",
                            params![new_id(), sid, income.to_db(), l.to_string()],
                        )
                        .unwrap_err(),
                        "sales_receipt_line_scope",
                    );
                    }
                    _ => unreachable!(),
                }
            }
            Err(FbError::validation("rolled-back fixture"))
        });
        assert!(result.is_err());
        if case == 10 {
            f.book.connection().execute_batch(&guard).unwrap();
        }
        assert_eq!(snapshot(&f.book), state);
    }
    let s = f.sale_input();
    let review = review_sales_receipt(&mut f.book, actor, c, &s).unwrap();
    let r = record_sales_receipt(&mut f.book, actor, c, &s, Some(&review)).unwrap();
    refusal(
        f.book
            .connection()
            .execute(
                "INSERT INTO sales_receiptreversal VALUES(?1,?2,?3,'2026-10-03',?4,?5)",
                params![
                    new_id(),
                    r["id"].as_str(),
                    new_id(),
                    json!({"reason":"Bad link"}).to_string(),
                    actor.to_db()
                ],
            )
            .unwrap_err(),
        "receipt_reversal_link",
    );
}
#[test]
pub(super) fn posting_financial_copy_split_eligibility_and_void_dependencies_are_guarded() {
    let mut f = Fixture::new();
    let draft = f.save();
    let actor = f.actor;
    let c = f.company;
    let ar = f.ar;
    let income = f.income;
    let tax = f.tax;
    for case in 0..4 {
        let result: FbResult<()> = f.book.write(|tx| {
        let source = draft["identity_id"].as_str().unwrap();
        let mut h = draft["current"].clone();
        h["id"] = json!(new_id());
        h["number"] = json!(2);
        h["state"] = json!("OPEN");
        h["action"] = json!("posted");
        h["review_digest"] = json!("0".repeat(64));
        documents::snapshot_source_accounts(tx,actor,c,source,&json!({"receivables":ar.to_db(),"income:1":income.to_db(),"collected_tax":tax.to_db()})).unwrap();
        let credits = if case == 2 {
            BTreeMap::from([(income.to_db(), 426)])
        } else {
            BTreeMap::from([(income.to_db(), 401), (tax.to_db(), 25)])
        };
        let j = documents::post_source(
            tx,
            actor,
            c,
            source,
            "2026-10-01",
            "Synthetic post",
            entries(&ar.to_db(), 426, credits),
        )
        .unwrap();
        h["journal_id"] = json!(j.id.to_db());
        if case == 0 {
            h["memo"] = json!("Changed financial evidence")
        }
        if case == 1 {
            tx.execute(
                "UPDATE directory_customer SET active=0 WHERE id=?1",
                [&f.customer],
            )
            .unwrap();
        }
        let key = draft["id"].as_str().unwrap();
        tx.execute("UPDATE sales_invoice SET revision=2 WHERE id=?1", [key])
            .unwrap();
        let inserted=tx.execute("INSERT INTO sales_invoicerevision VALUES(?1,?2,2,?3,'2026-10-01','2026-10-31','OPEN',426,?4,NULL,?5,?6)",params![h["id"].as_str(),key,h["customer_id"].as_str(),j.id.to_db(),h.to_string(),actor.to_db()]);
        match case {
            0 => refusal(inserted.unwrap_err(), "invoice_financial_immutable"),
            1 => refusal(inserted.unwrap_err(), "sales_posting_eligible"),
            2 | 3 => {
                inserted.unwrap();
                let mut line = draft["lines"][0].clone();
                if case == 3 {
                    line["description"] = json!("Changed after review")
                }
                insert_line(tx, h["id"].as_str().unwrap(), &line).unwrap();
                refusal(
                    tx.execute(
                        "INSERT INTO sales_seal VALUES('sales_invoice_revision',?1)",
                        [h["id"].as_str().unwrap()],
                    )
                    .unwrap_err(),
                    if case == 2 {
                        "invoice_posting_complete"
                    } else {
                        "invoice_financial_immutable"
                    },
                );
            }
            _ => unreachable!(),
        }
        Err(FbError::validation("rolled-back fixture"))
        });
        assert!(result.is_err());
    }
    let review = review_invoice(
        &mut f.book,
        actor,
        c,
        draft["id"].as_str().unwrap(),
        Some(1),
    )
    .unwrap();
    let inv = post_invoice(
        &mut f.book,
        actor,
        c,
        draft["id"].as_str().unwrap(),
        Some(1),
        Some(&review),
    )
    .unwrap();
    let p = f.payment(inv["id"].as_str().unwrap(), "1");
    let review = review_receipt(&mut f.book, actor, c, &p).unwrap();
    record_receipt(&mut f.book, actor, c, &p, Some(&review)).unwrap();
    let result:FbResult<()>=f.book.write(|tx|{
  let j=documents::reverse_source(tx,actor,c,inv["identity_id"].as_str().unwrap(),"2026-10-03","Correction")?;let mut h=inv["current"].clone();h["id"]=json!(new_id());h["number"]=json!(3);h["state"]=json!("VOID");h["action"]=json!("voided");h["reason"]=json!("Correction");h["reversal_id"]=json!(j.id.to_db());tx.execute("UPDATE sales_invoice SET revision=3 WHERE id=?1",[inv["id"].as_str().unwrap()])?;
  refusal(tx.execute("INSERT INTO sales_invoicerevision VALUES(?1,?2,3,?3,'2026-10-01','2026-10-31','VOID',426,?4,?5,?6,?7)",params![h["id"].as_str(),inv["id"].as_str(),h["customer_id"].as_str(),h["journal_id"].as_str(),j.id.to_db(),h.to_string(),actor.to_db()]).unwrap_err(),"invoice_live_receipts");Err(FbError::validation("rolled-back attack"))
 });
    assert!(result.is_err());
}
