//! Raw-SQL attacks; these deliberately do not go through friendly service validation.
use super::*;
use fb_db::rusqlite::types::ValueRef;
#[path = "rebase_tests.rs"]
mod rebase;

// Fresh/absent ids: canonical UUIDs, so 0130 passes and the named sales guard decides.
const MISSING_REVISION: &str = "00000000-0000-7000-8000-0000000c5001"; // absent invoice line parent
const LATE_INVOICE_LINE: &str = "00000000-0000-7000-8000-0000000c5002"; // attacker's late invoice line
const LATE_ALLOCATION: &str = "00000000-0000-7000-8000-0000000c5003"; // attacker's late allocation
const WRONG_KIND_SALE: &str = "00000000-0000-7000-8000-0000000c5004"; // sales-receipt detail on a payment
const LATE_SALE_LINE: &str = "00000000-0000-7000-8000-0000000c5005"; // attacker's late sales-receipt line
const LATE_ACCOUNT: &str = "00000000-0000-7000-8000-0000000c5006"; // attacker's late frozen account row
static SEEN: std::sync::LazyLock<std::sync::Mutex<BTreeSet<String>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(BTreeSet::new()));
fn seen(name: &str) {
    SEEN.lock().unwrap().insert(name.to_string());
}
fn expect(conn: &Connection, sql: &str, want: &str) -> String {
    let err = conn.execute_batch(sql).expect_err(sql);
    let name =
        fb_db::constraint_of(&err).unwrap_or_else(|| panic!("not a constraint: {err}, SQL {sql}"));
    assert_eq!(name, want, "{err}, SQL {sql}");
    seen(&name);
    name
}
fn tables(conn: &Connection) -> Vec<String> {
    let mut st=conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").unwrap();
    st.query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}
pub(super) fn snapshot(book: &Book) -> Vec<(String, Vec<Vec<String>>)> {
    let conn = book.connection();
    tables(conn)
        .into_iter()
        .map(|table| {
            let mut st = conn
                .prepare(&format!("SELECT * FROM {table} ORDER BY 1"))
                .unwrap();
            let count = st.column_count();
            let rows = st
                .query_map([], |row| {
                    (0..count)
                        .map(|i| {
                            Ok(match row.get_ref(i)? {
                                ValueRef::Null => "NULL".into(),
                                v => format!("{v:?}"),
                            })
                        })
                        .collect::<Result<Vec<_>, fb_db::rusqlite::Error>>()
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            (table, rows)
        })
        .collect()
}
fn sqlq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}
fn prepare_history() -> Fixture {
    let mut f = Fixture::new();
    let inv = f.posted();
    let p = f.payment(inv["id"].as_str().unwrap(), "1.00");
    let r = review_receipt(&mut f.book, f.actor, f.company, &p).unwrap();
    let r = record_receipt(&mut f.book, f.actor, f.company, &p, Some(&r)).unwrap();
    reverse_receipt(
        &mut f.book,
        f.actor,
        f.company,
        r["id"].as_str().unwrap(),
        Some(1),
        &json!("2026-10-03"),
        &json!("Synthetic reversal"),
    )
    .unwrap();
    let s = f.sale_input();
    let r = review_sales_receipt(&mut f.book, f.actor, f.company, &s).unwrap();
    record_sales_receipt(&mut f.book, f.actor, f.company, &s, Some(&r)).unwrap();
    f
}
#[test]
fn all_protected_tables_reject_updates_deletes_and_every_unique_key_replace() {
    let f = prepare_history();
    let conn = f.book.connection();
    let protected = [
        ("sales_invoice", "invoice_immutable_ownership"),
        ("sales_invoicerevision", "invoice_history_immutable"),
        ("sales_invoiceline", "invoice_history_immutable"),
        ("sales_receipt", "receipt_immutable"),
        ("sales_receiptallocation", "receipt_immutable"),
        ("sales_receiptreversal", "receipt_immutable"),
        ("sales_salesreceipt", "receipt_immutable"),
        ("sales_salesreceiptline", "receipt_immutable"),
    ];
    let mut tested = 0;
    for recursive in [true, false] {
        conn.execute_batch(&format!(
            "PRAGMA recursive_triggers={}",
            i32::from(recursive)
        ))
        .unwrap();
        for (table, collision) in protected {
            let state = snapshot(&f.book);
            expect(
                conn,
                &format!("UPDATE {table} SET id=id"),
                if table == "sales_invoice" {
                    "invoice_immutable_ownership"
                } else {
                    "sales_history_immutable"
                },
            );
            expect(
                conn,
                &format!("DELETE FROM {table}"),
                if table == "sales_invoice" {
                    "invoice_immutable_ownership"
                } else {
                    "sales_history_immutable"
                },
            );
            let mut st = conn
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap();
            let columns = st
                .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            let mut st = conn
                .prepare(&format!("PRAGMA index_list({table})"))
                .unwrap();
            let indexes = st
                .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, bool>(2)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            let mut unique = Vec::new();
            for (index, yes) in indexes {
                if yes {
                    let mut st = conn
                        .prepare(&format!("PRAGMA index_info({index})"))
                        .unwrap();
                    unique.push(
                        st.query_map([], |r| r.get::<_, String>(2))
                            .unwrap()
                            .collect::<Result<Vec<_>, _>>()
                            .unwrap(),
                    )
                }
            }
            let all_keys = unique.iter().flatten().collect::<BTreeSet<_>>();
            for kept in &unique {
                let projection = columns
                    .iter()
                    .map(|(col, kind)| {
                        if all_keys.contains(col) && !kept.contains(col) {
                            if kind == "INTEGER" {
                                format!("{col}+100000")
                            } else {
                                sqlq(&new_id())
                            }
                        } else {
                            col.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                expect(
                    conn,
                    &format!(
                        "INSERT OR REPLACE INTO {table} SELECT {projection} FROM {table} LIMIT 1"
                    ),
                    collision,
                );
                tested += 1;
            }
            assert_eq!(snapshot(&f.book), state);
        }
    }
    conn.execute_batch("PRAGMA recursive_triggers=ON").unwrap();
    assert_eq!(tested, 36, "unique-key attack count");
}
#[test]
fn referenced_directory_accounts_sources_and_lines_cannot_be_replaced() {
    let mut f = prepare_history();
    let mut input = f.input();
    let item = directory::save_record(
        &mut f.book,
        f.actor,
        f.company,
        Model::Item,
        &json!({"code":"REF","description":"Referenced","kind":"SERVICE","sale_rate":"1"}),
        None,
    )
    .unwrap();
    input["lines"][0]["item_id"] = item["id"].clone();
    save_invoice(&mut f.book, f.actor, f.company, &input).unwrap();
    let conn = f.book.connection();
    for recursive in [true, false] {
        conn.execute_batch(&format!(
            "PRAGMA recursive_triggers={}",
            i32::from(recursive)
        ))
        .unwrap();
        for table in [
            "directory_customer",
            "directory_item",
            "ledger_account",
            "documents_documentidentity",
            "ledger_journal",
            "documents_documentaccount",
            "ledger_journalline",
        ] {
            let where_clause = if table == "ledger_account" {
                " WHERE code='400'"
            } else {
                ""
            };
            expect(
                conn,
                &format!(
                    "INSERT OR REPLACE INTO {table} SELECT * FROM {table}{where_clause} LIMIT 1"
                ),
                if table == "documents_documentaccount" {
                    "sales_late_reference"
                } else {
                    "sales_reference_protected"
                },
            );
            expect(
                conn,
                &format!("DELETE FROM {table}{where_clause}"),
                "sales_reference_protected",
            );
        }
        for table in ["directory_customer", "directory_item", "ledger_account"] {
            expect(
                conn,
                &format!("UPDATE {table} SET company_id={}", sqlq(&new_id())),
                "sales_reference_protected",
            );
        }
        expect(
            conn,
            "INSERT OR REPLACE INTO sales_seal SELECT * FROM sales_seal WHERE kind LIKE 'sales_%' LIMIT 1",
            "sales_seal_immutable",
        );
        expect(
            conn,
            "INSERT OR REPLACE INTO sales_seal_due SELECT * FROM sales_seal_due WHERE kind LIKE 'sales_%' LIMIT 1",
            "sales_seal_immutable",
        );
    }
    conn.execute_batch("PRAGMA recursive_triggers=ON").unwrap();
}

#[test]
fn owned_seals_cannot_be_changed_or_deleted() {
    let f = prepare_history();
    for table in ["sales_seal", "sales_seal_due"] {
        expect(
            f.book.connection(),
            &format!("UPDATE {table} SET doc_id=doc_id"),
            "sales_seal_immutable",
        );
        expect(
            f.book.connection(),
            &format!("DELETE FROM {table}"),
            "sales_seal_immutable",
        );
    }
}
pub(super) fn insert_line(tx: &Writer<'_>, rid: &str, l: &Value) -> Result<usize, fb_db::rusqlite::Error> {
    tx.execute(
        "INSERT INTO sales_invoiceline VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            new_id(),
            rid,
            l["position"].as_i64(),
            l["item_id"].as_str(),
            l["income_account_id"].as_str(),
            l["expense_account_id"].as_str(),
            cents(&l["amount"]).unwrap(),
            l["taxable"].as_bool(),
            l.to_string()
        ],
    )
}
fn refusal(err: fb_db::rusqlite::Error, want: &str) {
    seen(want);
    assert_eq!(fb_db::constraint_of(&err).as_deref(), Some(want), "{err}")
}
#[test]
fn incomplete_revisions_missing_parents_line_calculations_and_seals_are_refused() {
    let mut f = Fixture::new();
    let base = f.save();
    let state = snapshot(&f.book);
    // Open the writer directly, but keep an independent immutable fixture view for helper metadata.
    let actor = f.actor;
    let c = f.company;
    for which in 0..9 {
        let result: FbResult<()> = f.book.write(|tx| {
            // Build inline to avoid borrowing the book and fixture at the same time.
            let source = documents::create_identity(tx, actor, c, "INV", &new_id()).unwrap();
            let key = new_id();
            let rid = new_id();
            tx.execute(
                "INSERT INTO sales_invoice VALUES(?1,?2,?3,?4,1)",
                params![key, c.to_db(), source["id"].as_str(), "0".repeat(64)],
            )
            .unwrap();
            let mut h = base["current"].clone();
            h["id"] = json!(rid);
            let sql =
                "INSERT INTO sales_invoicerevision VALUES(?1,?2,1,?3,?4,?5,?6,?7,NULL,NULL,?8,?9)";
            if which == 0 {
                h["memo"] = json!("x".repeat(2001))
            }
            if which == 1 {
                h["action"] = json!("posted")
            }
            if which == 2 {
                h["review_digest"] = json!("x".repeat(64))
            }
            let result = tx.execute(
                sql,
                params![
                    rid,
                    if which == 3 { new_id() } else { key.clone() },
                    h["customer_id"].as_str(),
                    h["date"].as_str(),
                    h["due_date"].as_str(),
                    h["state"].as_str(),
                    cents(&h["total"]).unwrap(),
                    h.to_string(),
                    actor.to_db()
                ],
            );
            match which {
                0 => refusal(result.unwrap_err(), "invoice_revision_fields"),
                1 => refusal(result.unwrap_err(), "invoice_transition"),
                2 => refusal(result.unwrap_err(), "invoice_posting_link"),
                3 => refusal(result.unwrap_err(), "invoice_revision_scope"),
                4 | 5 => {
                    result.unwrap();
                    let mut line = base["lines"][0].clone();
                    if which == 4 {
                        line["amount"] = json!("2.00")
                    }
                    let err =
                        insert_line(tx, if which == 5 { MISSING_REVISION } else { &rid }, &line)
                            .unwrap_err();
                    refusal(
                        err,
                        if which == 5 {
                            "invoice_line_scope"
                        } else {
                            "invoice_line_calculation"
                        },
                    );
                }
                6 => {
                    result.unwrap();
                    refusal(
                        tx.execute(
                            "INSERT INTO sales_seal VALUES('sales_invoice_revision',?1)",
                            [&rid],
                        )
                        .unwrap_err(),
                        "invoice_complete",
                    );
                }
                7 => {
                    result.unwrap();
                    refusal(
                        tx.execute(
                            "INSERT INTO sales_seal VALUES('sales_invoice_revision',?1)",
                            [new_id()],
                        )
                        .unwrap_err(),
                        "invoice_complete",
                    );
                }
                8 => {
                    result.unwrap();
                    return Ok(());
                }
                _ => unreachable!(),
            }
            Err(FbError::validation("rolled-back fixture"))
        });
        let e = result.unwrap_err();
        if which == 8 {
            // A complete-looking but unsealed revision: the deferred seal-due foreign key refuses commit.
            assert_eq!(e.constraint(), Some(ORPHAN_INVOICE_GUARD), "{e}");
        } else {
            assert_eq!(e.message(), "rolled-back fixture", "{e}");
        }
        assert_eq!(snapshot(&f.book), state);
    }
}
#[test]
fn late_lines_references_allocations_and_cross_kind_details_are_refused() {
    let mut f = prepare_history();
    let second = f.posted();
    let key = second["id"].as_str().unwrap();
    let state = snapshot(&f.book);
    let conn = f.book.connection();
    expect(
        conn,
        &format!("INSERT INTO sales_invoiceline SELECT '{LATE_INVOICE_LINE}',revision_id,50,item_id,income_account_id,expense_account_id,amount,taxable,json_set(data,'$.position',50) FROM sales_invoiceline LIMIT 1"),
        "invoice_line_scope",
    );
    expect(
        conn,
        &format!(
            "INSERT INTO sales_receiptallocation SELECT {},receipt_id,{},invoice_revision_id,receivables_id,1 FROM sales_receiptallocation LIMIT 1",
            sqlq(LATE_ALLOCATION),
            sqlq(key)
        ),
        "receipt_late_allocation",
    );
    expect(
        conn,
        &format!("INSERT INTO sales_salesreceipt SELECT '{WRONG_KIND_SALE}',r.id,r.amount,'{{}}' FROM sales_receipt r WHERE r.kind='RCP' LIMIT 1"),
        "sales_receipt_scope",
    );
    expect(
        conn,
        &format!("INSERT INTO sales_salesreceiptline SELECT '{LATE_SALE_LINE}',sale_id,50,item_id,income_account_id,amount,taxable,json_set(data,'$.position',50) FROM sales_salesreceiptline LIMIT 1"),
        "sales_receipt_late_line",
    );
    expect(
        conn,
        &format!("INSERT INTO documents_documentaccount SELECT '{LATE_ACCOUNT}',source_id,'extra',account_id,account_code,account_name,account_kind FROM documents_documentaccount LIMIT 1"),
        "sales_late_reference",
    );
    assert_eq!(snapshot(&f.book), state);
}
#[test]
fn orphan_receipt_journals_and_invoice_roots_cannot_commit() {
    let mut f = Fixture::new();
    let state = snapshot(&f.book);
    let a = f.actor;
    let c = f.company;
    let bank = f.bank;
    let income = f.income;
    let result: FbResult<()> = f.book.write(|tx| {
        let d = documents::create_identity(tx, a, c, "RCP", &new_id())?;
        let j = documents::post_source(
            tx,
            a,
            c,
            d["id"].as_str().unwrap(),
            "2026-10-01",
            "Synthetic orphan",
            vec![
                json!({"account":bank.to_db(),"debit":"1"}),
                json!({"account":income.to_db(),"credit":"1"}),
            ],
        )?;
        refusal(
            tx.execute(
                "INSERT INTO sales_seal VALUES('sales_journal',?1)",
                [j.id.to_db()],
            )
            .unwrap_err(),
            "sales_journal_owner",
        );
        Ok(())
    });
    // The unsealed orphan journal is refused at commit by the deferred seal-due foreign key.
    let e = result.unwrap_err();
    assert_eq!(e.constraint(), Some(ORPHAN_INVOICE_GUARD), "{e}");
    assert_eq!(snapshot(&f.book), state);
    let result: FbResult<()> = f.book.write(|tx| {
        let d = documents::create_identity(tx, a, c, "INV", &new_id())?;
        tx.execute(
            "INSERT INTO sales_invoice VALUES(?1,?2,?3,?4,1)",
            params![new_id(), c.to_db(), d["id"].as_str(), "0".repeat(64)],
        )?;
        Ok(())
    });
    let e = result.unwrap_err();
    assert_ne!(e.constraint(), Some("fb_valid"), "masked by fb_valid: {e}");
    assert_eq!(e.constraint(), Some(ORPHAN_INVOICE_GUARD), "{e}");
    assert_eq!(snapshot(&f.book), state);
}

// The unsealed head leaves an fb_seal_due row whose deferred foreign key refuses the commit.
const ORPHAN_INVOICE_GUARD: &str = "foreign_key";
/// The public write must fail at the injected late audit trigger, not at any earlier guard.
fn injected<T: std::fmt::Debug>(r: FbResult<T>) {
    let e = r.unwrap_err();
    assert_eq!(e.constraint(), Some("injected"), "{e}");
}
fn inject(book: &Book, action: &str) {
    book.connection().execute_batch(&format!("CREATE TRIGGER sales_injected_failure BEFORE INSERT ON ledger_auditevent WHEN NEW.action={} BEGIN SELECT RAISE(ABORT,'injected: synthetic late audit failure'); END;",sqlq(action))).unwrap();
}
fn clear(book: &Book) {
    book.connection()
        .execute_batch("DROP TRIGGER sales_injected_failure")
        .unwrap()
}
#[test]
fn every_public_write_rolls_back_after_a_late_partial_write() {
    let mut f = Fixture::new();
    let input = f.input();
    let before = snapshot(&f.book);
    inject(&f.book, "invoice.created");
    injected(save_invoice(&mut f.book, f.actor, f.company, &input));
    clear(&f.book);
    assert_eq!(snapshot(&f.book), before);
    let inv = save_invoice(&mut f.book, f.actor, f.company, &input).unwrap();
    let key = inv["id"].as_str().unwrap();
    let mut edit = input.clone();
    edit["invoice_id"] = json!(key);
    edit["expected_revision"] = json!(1);
    edit["memo"] = json!("Edited");
    let before = snapshot(&f.book);
    inject(&f.book, "invoice.revised");
    injected(save_invoice(&mut f.book, f.actor, f.company, &edit));
    clear(&f.book);
    assert_eq!(snapshot(&f.book), before);
    let review = review_invoice(&mut f.book, f.actor, f.company, key, Some(1)).unwrap();
    inject(&f.book, "invoice.posted");
    injected(post_invoice(&mut f.book, f.actor, f.company, key, Some(1), Some(&review)));
    clear(&f.book);
    assert_eq!(snapshot(&f.book), before);
    post_invoice(&mut f.book, f.actor, f.company, key, Some(1), Some(&review)).unwrap();
    let before = snapshot(&f.book);
    inject(&f.book, "invoice.voided");
    injected(void_invoice(
        &mut f.book,
        f.actor,
        f.company,
        key,
        Some(2),
        &json!("2026-10-03"),
        &json!("Correction"),
    ));
    clear(&f.book);
    assert_eq!(snapshot(&f.book), before);
    let input = f.payment(key, "1");
    let review = review_receipt(&mut f.book, f.actor, f.company, &input).unwrap();
    inject(&f.book, "receipt.recorded");
    injected(record_receipt(&mut f.book, f.actor, f.company, &input, Some(&review)));
    clear(&f.book);
    assert_eq!(snapshot(&f.book), before);
    let r = record_receipt(&mut f.book, f.actor, f.company, &input, Some(&review)).unwrap();
    let before = snapshot(&f.book);
    inject(&f.book, "receipt.reversed");
    injected(reverse_receipt(
        &mut f.book,
        f.actor,
        f.company,
        r["id"].as_str().unwrap(),
        Some(1),
        &json!("2026-10-03"),
        &json!("Correction"),
    ));
    clear(&f.book);
    assert_eq!(snapshot(&f.book), before);
    let input = f.sale_input();
    let review = review_sales_receipt(&mut f.book, f.actor, f.company, &input).unwrap();
    inject(&f.book, "sales_receipt.recorded");
    injected(record_sales_receipt(&mut f.book, f.actor, f.company, &input, Some(&review)));
    clear(&f.book);
    assert_eq!(snapshot(&f.book), before);
    let r = record_sales_receipt(&mut f.book, f.actor, f.company, &input, Some(&review)).unwrap();
    let before = snapshot(&f.book);
    inject(&f.book, "sales_receipt.voided");
    injected(void_sales_receipt(
        &mut f.book,
        f.actor,
        f.company,
        r["id"].as_str().unwrap(),
        Some(1),
        &json!("2026-10-03"),
        &json!("Correction"),
    ));
    clear(&f.book);
    assert_eq!(snapshot(&f.book), before);
    // Reviews have no writes: both successful and refused paths leave every row unchanged.
    let before = snapshot(&f.book);
    let _ = review_receipt(&mut f.book, f.actor, f.company, &input);
    let _ = review_sales_receipt(&mut f.book, f.actor, f.company, &input);
    let _ = review_invoice(&mut f.book, f.actor, f.company, key, Some(900));
    assert_eq!(snapshot(&f.book), before);
}

#[path = "attacks_lifecycle.rs"]
mod lifecycle;

#[path = "attacks_tax.rs"]
mod tax_attacks;
