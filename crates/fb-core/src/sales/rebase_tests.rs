use super::*;

const PROTECTED: &[&str] = &[
    "sales_invoice",
    "sales_invoicerevision",
    "sales_invoiceline",
    "sales_receipt",
    "sales_receiptallocation",
    "sales_receiptreversal",
    "sales_salesreceipt",
    "sales_salesreceiptline",
    "sales_seal",
    "sales_seal_due",
];

#[test]
fn new_invoice_lines_require_eligible_accounts_before_the_parent_can_be_sealed() {
    let mut f = Fixture::new();
    let base = f.save();
    let actor = f.actor;
    let c = f.company;
    for case in 0..3 {
        let bad = if case == 0 { f.bank } else { f.income };
        let before = snapshot(&f.book);
        let result:FbResult<()>=f.book.write(|tx|{
            if case>0 {tx.execute(&format!("UPDATE ledger_account SET {}=0 WHERE id=?1",if case==1 {"active"}else{"postable"}),[bad.to_db()])?;}
            let d=documents::create_identity(tx,actor,c,"INV",&new_id())?;
            let key=new_id(); let rid=new_id(); let mut h=base["current"].clone(); h["id"]=json!(rid);
            tx.execute("INSERT INTO sales_invoice VALUES(?1,?2,?3,?4,1)",params![key,c.to_db(),d["id"].as_str(),"0".repeat(64)])?;
            tx.execute("INSERT INTO sales_invoicerevision VALUES(?1,?2,1,?3,'2026-10-01','2026-10-31','DRAFT',426,NULL,NULL,?4,?5)",params![rid,key,h["customer_id"].as_str(),h.to_string(),actor.to_db()])?;
            let mut l=base["lines"][0].clone();l["income_account_id"]=json!(bad.to_db());l["account_snapshot"]["income_account"]=account_snapshot(&account(tx,c,&bad.to_db())?);
            let err=insert_line(tx,&rid,&l).expect_err("ineligible new account must fail immediately");
            assert_eq!(fb_db::constraint_of(&err).as_deref(),Some("invoice_line_scope"));
            Err(FbError::validation("rollback fixture"))
        });
        assert!(result.is_err());
        assert_eq!(snapshot(&f.book), before);
    }
}

#[test]
fn every_sales_table_has_no_hidden_identity_and_permanent_primary_keys() {
    let f = prepare_history();
    let conn = f.book.connection();
    for table in PROTECTED {
        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name=?1",
                [table],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            sql.to_uppercase().contains("WITHOUT ROWID"),
            "{table} has hidden identity"
        );
        if !table.starts_with("sales_seal") {
            let count: i64 = conn.query_row("SELECT count(*) FROM sqlite_master WHERE type='trigger' AND tbl_name=?1 AND upper(sql) LIKE '%BEFORE UPDATE OF ID%'", [table], |r| r.get(0)).unwrap();
            assert_eq!(
                count, 1,
                "{table} must explicitly forbid primary-key updates"
            );
        }
        for recursive in [0, 1] {
            conn.execute_batch(&format!("PRAGMA recursive_triggers={recursive}"))
                .unwrap();
            for alias in ["rowid", "_rowid_", "oid"] {
                let before = snapshot(&f.book);
                for sql in [
                    format!("INSERT OR REPLACE INTO {table}({alias}) VALUES(1)"),
                    format!("UPDATE OR REPLACE {table} SET {alias}=1"),
                ] {
                    let err = conn.execute_batch(&sql).unwrap_err();
                    assert!(err.to_string().contains(alias), "{table}: {err}");
                    assert_eq!(snapshot(&f.book), before);
                }
            }
        }
    }
}

#[test]
fn sales_update_replacement_view_upsert_and_delete_matrix_preserves_entire_book() {
    let mut f = prepare_history();
    f.posted();
    let conn = f.book.connection();
    for recursive in [0, 1] {
        conn.execute_batch(&format!("PRAGMA recursive_triggers={recursive}"))
            .unwrap();
        for table in PROTECTED {
            let before = snapshot(&f.book);
            let key = if table.starts_with("sales_seal") {
                "doc_id"
            } else {
                "id"
            };
            let sqls = [
                format!(
                    "UPDATE OR REPLACE {table} SET {key}=(SELECT {key} FROM {table} ORDER BY {key} LIMIT 1)"
                ),
                format!(
                    "INSERT INTO {table} SELECT * FROM {table} WHERE true ON CONFLICT DO UPDATE SET {key}=excluded.{key}"
                ),
                format!("DELETE FROM {table}"),
            ];
            for sql in sqls {
                assert!(conn.execute_batch(&sql).is_err(), "{sql}");
                assert_eq!(snapshot(&f.book), before);
            }
            conn.execute_batch(&format!("CREATE VIEW sales_matrix AS SELECT * FROM {table}; CREATE TRIGGER sales_matrix_update INSTEAD OF UPDATE ON sales_matrix BEGIN UPDATE {table} SET {key}=NEW.{key} WHERE {key}=OLD.{key}; END;")).unwrap();
            let view_state = snapshot(&f.book);
            assert!(
                conn.execute_batch(&format!("UPDATE OR REPLACE sales_matrix SET {key}={key}"))
                    .is_err()
            );
            assert_eq!(snapshot(&f.book), view_state);
            conn.execute_batch("DROP TRIGGER sales_matrix_update; DROP VIEW sales_matrix")
                .unwrap();
        }
    }
}

#[test]
fn each_sales_unique_key_rejects_displacing_a_distinct_victim_directly_and_through_views() {
    let mut f = prepare_history();
    let inv = f.posted();
    let input = f.payment(inv["id"].as_str().unwrap(), "1");
    let review = review_receipt(&mut f.book, f.actor, f.company, &input).unwrap();
    let receipt = record_receipt(&mut f.book, f.actor, f.company, &input, Some(&review)).unwrap();
    reverse_receipt(
        &mut f.book,
        f.actor,
        f.company,
        receipt["id"].as_str().unwrap(),
        Some(1),
        &json!("2026-10-03"),
        &json!("Second synthetic reversal"),
    )
    .unwrap();
    let input = f.sale_input();
    let review = review_sales_receipt(&mut f.book, f.actor, f.company, &input).unwrap();
    record_sales_receipt(&mut f.book, f.actor, f.company, &input, Some(&review)).unwrap();
    let conn = f.book.connection();
    let mut tested = 0;
    for recursive in [0, 1] {
        conn.execute_batch(&format!("PRAGMA recursive_triggers={recursive}"))
            .unwrap();
        for table in PROTECTED {
            let mut st = conn
                .prepare(&format!("PRAGMA index_list({table})"))
                .unwrap();
            let indexes: Vec<String> = st
                .query_map([], |r| r.get(1))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            for index in indexes {
                let mut st = conn
                    .prepare(&format!("PRAGMA index_info({index})"))
                    .unwrap();
                let cols: Vec<String> = st
                    .query_map([], |r| r.get(2))
                    .unwrap()
                    .collect::<Result<_, _>>()
                    .unwrap();
                let pk = if table.starts_with("sales_seal") {
                    "kind||':'||doc_id"
                } else {
                    "id"
                };
                let keys: Vec<String> = conn
                    .prepare(&format!("SELECT {pk} FROM {table} ORDER BY {pk} LIMIT 2"))
                    .unwrap()
                    .query_map([], |r| r.get(0))
                    .unwrap()
                    .collect::<Result<_, _>>()
                    .unwrap();
                assert_eq!(
                    keys.len(),
                    2,
                    "need distinct source/victim for {table} {cols:?}"
                );
                let set = cols
                    .iter()
                    .map(|col| {
                        format!(
                            "{col}=(SELECT {col} FROM {table} WHERE {pk}={})",
                            sqlq(&keys[1])
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                let before = snapshot(&f.book);
                for op in ["UPDATE", "UPDATE OR REPLACE"] {
                    let sql = format!("{op} {table} SET {set} WHERE {pk}={}", sqlq(&keys[0]));
                    assert!(conn.execute_batch(&sql).is_err(), "{sql}");
                    assert_eq!(snapshot(&f.book), before);
                }
                let view_set = cols
                    .iter()
                    .map(|col| format!("{col}=NEW.{col}"))
                    .collect::<Vec<_>>()
                    .join(",");
                let old_pk = if table.starts_with("sales_seal") {
                    "OLD.kind||':'||OLD.doc_id"
                } else {
                    "OLD.id"
                };
                conn.execute_batch(&format!("CREATE VIEW sales_matrix AS SELECT * FROM {table}; CREATE TRIGGER sales_matrix_update INSTEAD OF UPDATE ON sales_matrix BEGIN UPDATE {table} SET {view_set} WHERE {pk}={old_pk}; END;")).unwrap();
                let before = snapshot(&f.book);
                assert!(
                    conn.execute_batch(&format!(
                        "UPDATE OR REPLACE sales_matrix SET {set} WHERE {pk}={}",
                        sqlq(&keys[0])
                    ))
                    .is_err()
                );
                assert_eq!(snapshot(&f.book), before);
                conn.execute_batch("DROP TRIGGER sales_matrix_update;DROP VIEW sales_matrix")
                    .unwrap();
                tested += 1;
            }
        }
    }
    assert!(tested >= 40);
    println!(
        "{tested} isolated sales unique-key collision cases preserved complete book snapshots"
    );
}

#[test]
fn every_sales_schema_object_is_definition_hashed() {
    let mut f = prepare_history();
    let path: String = f
        .book
        .connection()
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name='main'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let objects: Vec<(String, String, String)> = {
        let mut st=f.book.connection().prepare("SELECT type,name,sql FROM sqlite_master WHERE name GLOB 'sales_*' OR name GLOB 'fb_sales_*' ORDER BY type,name").unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert!(objects.len() > 80);
    for (kind, name, definition) in objects {
        // Preserve a parseable schema so every object's own hash is tested,
        // including tables/CHECKs, without an unrelated missing-view error.
        f.book
            .connection()
            .execute_batch("PRAGMA writable_schema=ON")
            .unwrap();
        f.book
            .connection()
            .execute(
                "UPDATE sqlite_master SET sql=sql||' /* altered */' WHERE type=?1 AND name=?2",
                params![kind, name],
            )
            .unwrap();
        f.book
            .connection()
            .execute_batch("PRAGMA writable_schema=OFF")
            .unwrap();
        assert!(
            f.book.check().unwrap().iter().any(|p| p.contains(&name)),
            "{name}"
        );
        assert!(
            Book::open(std::path::Path::new(&path), &[13; 32]).is_err(),
            "{name}"
        );
        f.book
            .connection()
            .execute_batch("PRAGMA writable_schema=ON")
            .unwrap();
        f.book
            .connection()
            .execute(
                "UPDATE sqlite_master SET sql=?1 WHERE type=?2 AND name=?3",
                params![definition, kind, name],
            )
            .unwrap();
        f.book
            .connection()
            .execute_batch("PRAGMA writable_schema=OFF")
            .unwrap();
    }
    assert!(f.book.check().unwrap().is_empty());
    let read: FbResult<i64> = f
        .book
        .read(|r| Ok(r.query_row("SELECT fb_sales_money('\"4.26\"')", [], |r| r.get(0))?));
    assert_eq!(read.unwrap(), 426);
    let write: FbResult<i64> = f
        .book
        .write(|w| Ok(w.query_row("SELECT fb_sales_money('\"4.26\"')", [], |r| r.get(0))?));
    assert_eq!(write.unwrap(), 426);
}

#[test]
fn populated_sales_prefix_migrates_without_changing_any_existing_row_or_key() {
    for prefix in ["0010_sales", "0026_payables_core_adoption"] {
        populated_sales_upgrade(prefix);
    }
}
fn populated_sales_upgrade(prefix: &str) {
    let mut f = Fixture::new();
    let mut input = f.input();
    input["tax_mode"] = json!("NONE");
    input["tax_rate"] = json!("0");
    input["tax_label"] = json!("No tax");
    input["lines"][0]["rate"] = json!("10");
    let inv = save_invoice(&mut f.book, f.actor, f.company, &input).unwrap();
    let review = review_invoice(
        &mut f.book,
        f.actor,
        f.company,
        inv["id"].as_str().unwrap(),
        Some(1),
    )
    .unwrap();
    post_invoice(
        &mut f.book,
        f.actor,
        f.company,
        inv["id"].as_str().unwrap(),
        Some(1),
        Some(&review),
    )
    .unwrap();
    let input = f.payment(inv["id"].as_str().unwrap(), "3");
    let review = review_receipt(&mut f.book, f.actor, f.company, &input).unwrap();
    record_receipt(&mut f.book, f.actor, f.company, &input, Some(&review)).unwrap();
    let path=std::env::temp_dir().join(format!("sales-prior-{}.fbbook",new_id()));
    let copy=fb_db::test_support::populated_copy(f.book.connection(),&path,&[13;32],prefix,true).unwrap();
    assert!(!copy.before["sales_invoice"].is_empty());
    for _ in 0..2 {
        let mut book=Book::open(&path,&[13;32]).unwrap();
        assert_eq!(fb_db::test_support::snapshot(book.connection()).unwrap(),copy.expected);
        assert!(book.check().unwrap().is_empty());
        assert_eq!(invoice_balances(&mut book,f.actor,f.company).unwrap()[0]["outstanding"],"7.00");
    }
}

#[test]
fn open_time_guard_math_matches_domain_decimal_limits_rounding_and_strict_types() {
    let mut f = Fixture::new();
    for quantity in [
        json!("0.0001"),
        json!("0.1250"),
        json!("1"),
        json!("99999999999999.9999"),
        json!(true),
        json!("NaN"),
        json!("1e999999"),
    ] {
        for rate in [
            json!("0.01"),
            json!("4.01"),
            json!("99999999999999.99"),
            json!("-1"),
            json!(1.5),
        ] {
            let expected = calculations::line_amount(&quantity, &rate).ok();
            let line = json!({"quantity":quantity,"rate":rate,"description":"Work","taxable":true,"item_id":null,"item_snapshot":{},"income_account_id":null,"expense_account_id":null,"account_snapshot":{},"amount":expected.map(|m|m.to_string()).unwrap_or_else(||"0.00".into())});
            let actual: FbResult<bool> = f.book.read(|r| {
                Ok(
                    r.query_row("SELECT fb_sales_line_ok(?1)", [line.to_string()], |r| {
                        r.get(0)
                    })?,
                )
            });
            assert_eq!(actual.unwrap(), expected.is_some(), "{line}");
        }
    }
    for percent in ["0", "6.2500", "100", "100.0001", "NaN"] {
        let lines = vec![
            json!({"position":1,"quantity":"0.5","rate":"0.01","taxable":true,"amount":"0.01","item_id":null,"item_snapshot":{},"income_account_id":null,"expense_account_id":null,"account_snapshot":{}}),
        ];
        let header = json!({"state":"DRAFT","tax_mode":"MANUAL","tax_rate":percent,"tax_label":"Rate","tax_snapshot":{},"tax_item_id":null});
        let calc = calculate(&lines, &header, false);
        let mut good = header.clone();
        if let Ok(calc) = &calc {
            copy_calc(&mut good, calc);
        }
        let total = cents(&good["total"]).unwrap_or(0);
        let actual: FbResult<bool> = f.book.read(|r| {
            Ok(r.query_row(
                "SELECT fb_sales_document_ok(?1,?2,?3)",
                params![good.to_string(), json!(lines).to_string(), total],
                |r| r.get(0),
            )?)
        });
        assert_eq!(actual.unwrap(), calc.is_ok(), "{good}");
    }
}
