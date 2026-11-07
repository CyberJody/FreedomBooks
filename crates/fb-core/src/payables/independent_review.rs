use super::*;

// Raw evidence capture must also work on intentionally corrupt JSON.
fn snap(f: &Fx) -> Value {
    let c = f.book.connection();
    let tables=c.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT GLOB 'sqlite_*' ORDER BY name").unwrap()
        .query_map([],|r|r.get::<_,String>(0)).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
    let mut out = json!({});
    for table in tables {
        let mut stmt = c.prepare(&format!("SELECT * FROM {table}")).unwrap();
        let count = stmt.column_count();
        let mut data = stmt
            .query_map([], |r| {
                (0..count)
                    .map(|i| r.get::<_, SqlValue>(i))
                    .collect::<Result<Vec<_>, _>>()
            })
            .unwrap()
            .map(|row| format!("{:?}", row.unwrap()))
            .collect::<Vec<_>>();
        data.sort();
        out[table] = json!(data);
    }
    out
}
fn reopened(f: &Fx) {
    assert!(f.book.check().unwrap().is_empty());
    let path = std::path::PathBuf::from(f.book.connection().path().unwrap());
    let b = Book::open(&path, &[42; 32]).unwrap();
    assert!(b.check().unwrap().is_empty());
}

#[test]
fn independent_optional_bill_inputs_fail_without_mutation() {
    let mut f = Fx::new();
    let valid = f.payload();
    for field in [
        "memo",
        "reference",
        "reentry_reason",
        "refresh_vendor",
        "keep_archived",
        "bill_id",
        "supersedes_id",
        "expected_revision",
    ] {
        for bad in [
            Value::Null,
            json!(true),
            json!(12),
            json!([]),
            json!({}),
            json!("invalid"),
        ] {
            if (bad.is_null() && ["bill_id", "supersedes_id"].contains(&field))
                || (bad.is_string() && ["memo", "reference", "reentry_reason"].contains(&field))
                || (bad.is_boolean() && ["refresh_vendor", "keep_archived"].contains(&field))
            {
                continue;
            }
            let mut p = valid.clone();
            p[field] = bad.clone();
            let before = snap(&f);
            assert!(
                save_bill(&mut f.book, f.actor, f.co, &p).is_err(),
                "accepted {field}={bad}"
            );
            assert_eq!(snap(&f), before);
        }
    }
    reopened(&f);
}

#[test]
fn independent_optional_payment_inputs_fail_without_mutation() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let b = f.post(&b);
    let valid = f.payment(&b, "1.01");
    for field in [
        "memo",
        "reference",
        "request_key",
        "vendor_id",
        "bank_id",
        "date",
        "amount",
        "method",
        "allocations",
    ] {
        for bad in [Value::Null, json!(true), json!(12), json!([]), json!({})] {
            if field == "amount" && bad == 12 {
                continue;
            }
            let mut p = valid.clone();
            p[field] = bad.clone();
            let before = snap(&f);
            assert!(
                review_payment(&mut f.book, f.actor, f.co, &p).is_err(),
                "accepted {field}={bad}"
            );
            assert_eq!(snap(&f), before);
        }
    }
    reopened(&f);
}

#[test]
fn independent_sql_rejects_duplicate_archived_item_onto_new_line() {
    let mut f = Fx::new();
    let item = directory::save_record(
        &mut f.book,
        f.actor,
        f.co,
        Model::Item,
        &json!({"code":"OLD","kind":"SERVICE","sale_rate":"1.00"}),
        None,
    )
    .unwrap();
    let mut p = f.payload();
    p["lines"][0]["item_id"] = item["id"].clone();
    let b = f.save(&p);
    directory::set_active(
        &mut f.book,
        f.actor,
        f.co,
        Model::Item,
        s(&item, "id"),
        false,
    )
    .unwrap();
    p["bill_id"] = b["id"].clone();
    p["expected_revision"] = json!(1);
    p["lines"][0]["keep_archived"] = json!(true);
    let mut extra = p["lines"][0].clone();
    extra["origin_position"] = Value::Null;
    p["lines"].as_array_mut().unwrap().push(extra);
    let before = snap(&f);
    let err = save_bill(&mut f.book, f.actor, f.co, &p).unwrap_err();
    assert!(err.message().contains("Archived selection"));
    assert_eq!(snap(&f), before);
    let raw = bill(f.book.connection(), f.co, s(&b, "id")).unwrap();
    f.book
        .write(|tx| -> FbResult<()> {
            let mut r = super::attacks::new_revision(tx, &raw, "DRAFT")?;
            r["total"] = json!(202);
            r["action"] = json!("revised");
            insert(tx, "payables_billrevision", &r)?;
            for pos in 1..=2 {
                let mut l = raw["current"]["lines"][0].clone();
                l["id"] = json!(fb_ids::new_v7().to_string());
                l["revision_id"] = r["id"].clone();
                l["position"] = json!(pos);
                insert(tx, "payables_billline", &l)?;
            }
            tx.execute(
                "INSERT INTO fb_seal VALUES('bill_revision',?)",
                [format!("{}:2", s(&b, "id"))],
            )?;
            Ok(())
        })
        .expect_err("SQL must reject a new archived selection rejected by the service");
    assert_eq!(snap(&f), before);
    reopened(&f);
}

#[test]
fn independent_sql_rejects_duplicate_inactive_expense_onto_new_line() {
    let mut f = Fx::new();
    let mut p = f.payload();
    let b = f.save(&p);
    f.book
        .connection()
        .execute(
            "UPDATE ledger_account SET active=0 WHERE id=?",
            [f.expense.to_db()],
        )
        .unwrap();
    p["bill_id"] = b["id"].clone();
    p["expected_revision"] = json!(1);
    let mut extra = p["lines"][0].clone();
    extra["origin_position"] = Value::Null;
    p["lines"].as_array_mut().unwrap().push(extra);
    let before = snap(&f);
    assert!(save_bill(&mut f.book, f.actor, f.co, &p).is_err());
    assert_eq!(snap(&f), before);
    let raw = bill(f.book.connection(), f.co, s(&b, "id")).unwrap();
    f.book
        .write(|tx| -> FbResult<()> {
            let mut r = super::attacks::new_revision(tx, &raw, "DRAFT")?;
            r["total"] = json!(202);
            r["action"] = json!("revised");
            insert(tx, "payables_billrevision", &r)?;
            for pos in 1..=2 {
                let mut l = raw["current"]["lines"][0].clone();
                l["id"] = json!(fb_ids::new_v7().to_string());
                l["revision_id"] = r["id"].clone();
                l["position"] = json!(pos);
                insert(tx, "payables_billline", &l)?;
            }
            tx.execute(
                "INSERT INTO fb_seal VALUES('bill_revision',?)",
                [format!("{}:2", s(&b, "id"))],
            )?;
            Ok(())
        })
        .expect_err("SQL must reject a new inactive selection rejected by the service");
    assert_eq!(snap(&f), before);
    reopened(&f);
}

#[test]
fn independent_tracker_search_must_match_python_uppercase_rule() {
    let mut f = Fx::new();
    let mut p = f.payload();
    p["memo"] = json!("I");
    let b = f.save(&p);
    f.post(&b);
    for q in ["ı", "i", "I"] {
        let t = bill_tracker(
            &mut f.book,
            f.actor,
            f.co,
            &json!({"q":q}),
            &json!("2026-11-23"),
        )
        .unwrap();
        assert_eq!(
            t["totals"]["open_money"], "1.01",
            "search {q} must match frozen PostgreSQL UPPER semantics"
        );
        assert_eq!(t["rows"].as_array().unwrap().len(), 1);
    }
}

#[test]
fn independent_malformed_snapshot_is_rejected_at_insert() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let raw = bill(f.book.connection(), f.co, s(&b, "id")).unwrap();
    let before = snap(&f);
    let result = f.book.write(|tx| -> FbResult<()> {
        let mut r = super::attacks::new_revision(tx, &raw, "DRAFT")?;
        r["action"] = json!("revised");
        insert(tx, "payables_billrevision", &r)?;
        let mut l = raw["current"]["lines"][0].clone();
        l["id"] = json!(fb_ids::new_v7().to_string());
        l["revision_id"] = r["id"].clone();
        l["account_snapshot"] = json!([]);
        insert(tx, "payables_billline", &l)?;
        tx.execute(
            "INSERT INTO fb_seal VALUES('bill_revision',?)",
            [format!("{}:2", s(&b, "id"))],
        )?;
        Ok(())
    });
    assert!(
        result.is_err(),
        "malformed stored snapshot must be refused at INSERT"
    );
    assert_eq!(snap(&f), before);
    reopened(&f);
}

#[test]
fn independent_corrupt_snapshot_cannot_make_public_save_panic() {
    let mut f = Fx::new();
    let mut p = f.payload();
    let b = f.save(&p);
    let c = f.book.connection();
    let guard: String = c
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='fb_payables_billline_update'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    c.execute_batch("DROP TRIGGER fb_payables_billline_update; UPDATE payables_billline SET account_snapshot='[]';").unwrap();
    c.execute_batch(&guard).unwrap();
    p["bill_id"] = b["id"].clone();
    p["expected_revision"] = json!(1);
    p["lines"][0]["expense_account_id"] = Value::Null;
    let before = snap(&f);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        save_bill(&mut f.book, f.actor, f.co, &p)
    }));
    assert_eq!(snap(&f), before);
    assert!(
        result.is_ok(),
        "malformed stored snapshot must return an error, never panic"
    );
    assert!(result.unwrap().is_err());
    assert!(
        !f.book.check().unwrap().is_empty(),
        "integrity check must report invalid snapshot shape"
    );
    let path = std::path::PathBuf::from(f.book.connection().path().unwrap());
    assert!(Book::open(&path, &[42; 32]).is_err());
}

#[test]
fn independent_last_audit_failure_rolls_back_every_public_mutation() {
    for (action, event) in [
        ("create", "bill.created"),
        ("edit", "bill.revised"),
        ("receipt", "bill.created"),
        ("post", "bill.posted"),
        ("pay", "payment.recorded"),
        ("reverse", "payment.reversed"),
        ("void", "bill.voided"),
    ] {
        let mut f = Fx::new();
        let p = f.payload();
        let b = f.save(&p);
        let bill_review = f.review(&b);
        let b = if ["pay", "reverse", "void"].contains(&action) {
            f.post(&b)
        } else {
            b
        };
        let pay_input = f.payment(&b, "1.01");
        let pay_review = if action == "pay" {
            Some(review_payment(&mut f.book, f.actor, f.co, &pay_input).unwrap())
        } else {
            None
        };
        let payment = if action == "reverse" {
            Some(f.pay(&pay_input))
        } else {
            None
        };
        let before = snap(&f);
        f.book.connection().execute_batch(&format!("CREATE TEMP TRIGGER independent_late_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='{event}' BEGIN SELECT RAISE(ABORT,'independent final audit failure'); END;")).unwrap();
        let result = match action {
            "create" => {
                let mut q = p.clone();
                q["request_key"] = json!(fb_ids::new_v7().to_string());
                q["reference"] = json!("Independent new");
                save_bill(&mut f.book, f.actor, f.co, &q)
            }
            "edit" => {
                let mut q = p.clone();
                q["bill_id"] = b["id"].clone();
                q["expected_revision"] = json!(1);
                save_bill(&mut f.book, f.actor, f.co, &q)
            }
            "receipt" => save_receipt_draft(
                &mut f.book,
                f.actor,
                f.co,
                &f.vendor,
                &json!({"document_date":"2026-10-23","document_number":"Receipt independent","lines":[{"description":"Sample","quantity":"1","rate":"1","amount":"1"}]}),
                &fb_ids::new_v7().to_string(),
            ),
            "post" => post_bill(
                &mut f.book,
                f.actor,
                f.co,
                s(&b, "id"),
                Some(1),
                &bill_review,
            ),
            "pay" => record_payment(
                &mut f.book,
                f.actor,
                f.co,
                &pay_input,
                pay_review.as_ref().unwrap(),
            ),
            "reverse" => reverse_payment(
                &mut f.book,
                f.actor,
                f.co,
                s(payment.as_ref().unwrap(), "id"),
                Some(1),
                &json!("2026-10-25"),
                &json!("Correction"),
            ),
            _ => void_bill(
                &mut f.book,
                f.actor,
                f.co,
                s(&b, "id"),
                Some(2),
                &json!("2026-10-25"),
                &json!("Correction"),
            ),
        };
        let error = result.expect_err(action);
        assert!(
            error.message().contains("independent final audit failure"),
            "{action}: {error:?}"
        );
        f.book
            .connection()
            .execute_batch("DROP TRIGGER independent_late_audit")
            .unwrap();
        assert_eq!(snap(&f), before, "partial result for {action}");
        reopened(&f);
    }
}

#[test]
fn snapshot_shapes_are_enforced_for_all_six_columns() {
    let mut f = Fx::new();
    // Keep the older shape guards independently exercised: SQL NULL also fails
    // the newer shared JSON boundary before reaching those shape predicates.
    let shared:Vec<(String,String)>=f.book.connection().prepare("SELECT name,sql FROM sqlite_master WHERE name IN('fb_json_payables_billline_insert','fb_json_payables_billrevision_insert','fb_json_payables_billpayment_insert')").unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<Result<_,_>>().unwrap();
    for (name,_) in &shared {f.book.connection().execute_batch(&format!("DROP TRIGGER {name}")).unwrap();}
    let p = f.payload();
    let b = f.save(&p);
    let raw = bill(f.book.connection(), f.co, s(&b, "id")).unwrap();
    for (table, fields) in [
        ("billline", vec!["item_snapshot", "account_snapshot"]),
        ("billrevision", vec!["vendor_snapshot", "company_snapshot"]),
    ] {
        for field in fields {
            for bad in [
                json!([]),
                json!(null),
                json!(true),
                json!(7),
                json!("text"),
                json!({"expense_account":[]}),
            ] {
                if bad.is_object() && field != "account_snapshot" {
                    continue;
                }
                let before = snap(&f);
                let e = f
                    .book
                    .write(|tx| -> FbResult<()> {
                        let mut r = super::attacks::new_revision(tx, &raw, "DRAFT")?;
                        r["action"] = json!("revised");
                        if table == "billrevision" {
                            r[field] = bad.clone();
                        }
                        insert(tx, "payables_billrevision", &r)?;
                        let mut l = raw["current"]["lines"][0].clone();
                        l["id"] = json!(fb_ids::new_v7().to_string());
                        l["revision_id"] = r["id"].clone();
                        if table == "billline" {
                            l[field] = bad.clone();
                        }
                        insert(tx, "payables_billline", &l)?;
                        Ok(())
                    })
                    .unwrap_err();
                assert!(
                    e.message().contains("Snapshots must be objects"),
                    "{table}.{field}={bad}: {e:?}"
                );
                assert_eq!(snap(&f), before);
            }
        }
    }
    let b = f.post(&b);
    let raw = bill(f.book.connection(), f.co, s(&b, "id")).unwrap();
    for field in ["vendor_snapshot", "company_snapshot"] {
        for bad in [json!([]), json!(null), json!(true), json!(7), json!("text")] {
            let before = snap(&f);
            let e = f
                .book
                .write(|tx| -> FbResult<()> {
                    let mut p = super::attacks::raw_payment_data(
                        tx, f.actor, f.co, &raw, f.bank, f.ap, false, false,
                    )?;
                    p[field] = bad.clone();
                    insert(tx, "payables_billpayment", &p)
                })
                .unwrap_err();
            assert!(
                e.message().contains("Snapshots must be objects"),
                "payment.{field}: {e:?}"
            );
            assert_eq!(snap(&f), before);
        }
    }
    for (_,sql) in shared {f.book.connection().execute_batch(&sql).unwrap();}
    reopened(&f);
}

#[test]
fn corrupt_snapshots_are_reported_by_check_reopen_and_reads() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let b = f.post(&b);
    let p = f.payment(&b, "1.01");
    f.pay(&p);
    for (table, fields) in [
        ("billline", vec!["item_snapshot", "account_snapshot"]),
        ("billrevision", vec!["vendor_snapshot", "company_snapshot"]),
        ("billpayment", vec!["vendor_snapshot", "company_snapshot"]),
    ] {
        for field in fields {
            for bad in ["[]", "null", "true", "7", "\"text\""] {
                let c = f.book.connection();
                let guard: String = c
                    .query_row(
                        "SELECT sql FROM sqlite_master WHERE name=?",
                        [format!("fb_payables_{table}_update")],
                        |r| r.get(0),
                    )
                    .unwrap();
                c.execute_batch("BEGIN").unwrap();
                c.execute_batch(&format!("DROP TRIGGER fb_payables_{table}_update"))
                    .unwrap();
                c.execute(&format!("UPDATE payables_{table} SET {field}=?"), [bad])
                    .unwrap();
                c.execute_batch(&guard).unwrap();
                let issues = f.book.check().unwrap();
                assert!(
                    issues.iter().any(|x| x.contains("snapshot shape")),
                    "{table}.{field}: {issues:?}"
                );
                assert!(rows(c, &format!("SELECT * FROM payables_{table}"), &[]).is_err());
                c.execute_batch("ROLLBACK").unwrap();
            }
        }
    }
    reopened(&f);
}

#[test]
fn retained_origins_are_unique_and_reordering_still_works() {
    let mut f = Fx::new();
    let mut p = f.payload();
    let item = directory::save_record(
        &mut f.book,
        f.actor,
        f.co,
        Model::Item,
        &json!({"code":"OLD","kind":"SERVICE","sale_rate":"1.00"}),
        None,
    )
    .unwrap();
    p["lines"][0]["item_id"] = item["id"].clone();
    let mut second = p["lines"][0].clone();
    second["description"] = json!("Second line");
    p["lines"].as_array_mut().unwrap().push(second);
    let b = f.save(&p);
    directory::set_active(
        &mut f.book,
        f.actor,
        f.co,
        Model::Item,
        s(&item, "id"),
        false,
    )
    .unwrap();
    f.book
        .connection()
        .execute(
            "UPDATE ledger_account SET active=0 WHERE id=?",
            [f.expense.to_db()],
        )
        .unwrap();
    p["bill_id"] = b["id"].clone();
    p["expected_revision"] = json!(1);
    for (i, l) in p["lines"].as_array_mut().unwrap().iter_mut().enumerate() {
        l["origin_position"] = json!(i + 1);
        l["keep_archived"] = json!(true);
    }
    p["lines"].as_array_mut().unwrap().reverse();
    let b = f.save(&p);
    assert_eq!(b["current"]["lines"][0]["description"], "Second line");
    assert_eq!(b["current"]["lines"][0]["origin_position"], 2);
    let raw = bill(f.book.connection(), f.co, s(&b, "id")).unwrap();
    for origin in [json!(1), json!(null), json!(3)] {
        let before = snap(&f);
        let result = f.book.write(|tx| -> FbResult<()> {
            let r = super::attacks::new_revision(tx, &raw, "DRAFT")?;
            insert(tx, "payables_billrevision", &r)?;
            for pos in 1..=2 {
                let mut l = raw["current"]["lines"][0].clone();
                l["id"] = json!(fb_ids::new_v7().to_string());
                l["revision_id"] = r["id"].clone();
                l["position"] = json!(pos);
                l["origin_position"] = origin.clone();
                insert(tx, "payables_billline", &l)?;
            }
            tx.execute(
                "INSERT INTO fb_seal VALUES('bill_revision',?)",
                [format!("{}:3", s(&b, "id"))],
            )?;
            Ok(())
        });
        assert!(
            result.is_err(),
            "duplicated, missing, or invalid origin {origin} must fail"
        );
        assert_eq!(snap(&f), before);
    }
    // SQL also accepts the same legitimate reorder; position need not equal origin.
    f.book
        .write(|tx| -> FbResult<()> {
            let r = super::attacks::new_revision(tx, &raw, "DRAFT")?;
            insert(tx, "payables_billrevision", &r)?;
            for (i, prior) in raw["current"]["lines"]
                .as_array()
                .unwrap()
                .iter()
                .rev()
                .enumerate()
            {
                let mut l = prior.clone();
                l["id"] = json!(fb_ids::new_v7().to_string());
                l["revision_id"] = r["id"].clone();
                l["position"] = json!(i + 1);
                l["origin_position"] = prior["position"].clone();
                insert(tx, "payables_billline", &l)?;
            }
            tx.execute(
                "INSERT INTO fb_seal VALUES('bill_revision',?)",
                [format!("{}:3", s(&b, "id"))],
            )?;
            Ok(())
        })
        .unwrap();
    reopened(&f);
}

#[test]
fn tracker_simple_uppercase_vectors_cover_both_operands() {
    for (text, query, matches) in [
        ("I", "ı", true),
        ("ı", "I", true),
        ("İ", "i", false),
        ("i", "İ", false),
        ("Σ", "ς", true),
        ("ς", "σ", true),
        ("ß", "SS", false),
        ("ﬃ", "FFI", false),
        ("É", "é", true),
        ("é", "é", false),
        ("%_\\", "%_\\", true),
    ] {
        let mut f = Fx::new();
        let mut p = f.payload();
        p["memo"] = json!(format!("before-{text}-after"));
        let b = f.save(&p);
        f.post(&b);
        let t = bill_tracker(
            &mut f.book,
            f.actor,
            f.co,
            &json!({"q":format!("before-{query}-after")}),
            &json!("2026-11-23"),
        )
        .unwrap();
        assert_eq!(
            t["rows"].as_array().unwrap().len(),
            usize::from(matches),
            "{text:?} / {query:?}"
        );
        assert_eq!(
            t["totals"]["open_money"],
            if matches { "1.01" } else { "0.00" }
        );
    }
}

fn raw_insert(
    c: &fb_db::rusqlite::Connection,
    table: &str,
    v: &Value,
    mode: &str,
) -> Result<usize, fb_db::rusqlite::Error> {
    let m = v.as_object().unwrap();
    let columns = m.keys().cloned().collect::<Vec<_>>();
    let vals = m
        .values()
        .map(|v| match v {
            Value::Null => SqlValue::Null,
            Value::Number(n) => SqlValue::Integer(n.as_i64().unwrap()),
            Value::String(s) => SqlValue::Text(s.clone()),
            v => SqlValue::Text(v.to_string()),
        })
        .collect::<Vec<_>>();
    c.execute(
        &format!(
            "INSERT {mode} INTO {table}({}) VALUES({})",
            columns.join(","),
            vec!["?"; columns.len()].join(",")
        ),
        fb_db::rusqlite::params_from_iter(vals),
    )
}

#[test]
fn retained_origin_collision_guard_runs_before_replace_with_recursion_on_or_off() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let raw = bill(f.book.connection(), f.co, s(&b, "id")).unwrap();
    let before = snap(&f);
    for mode in ["ON", "OFF"] {
        let c = f.book.connection();
        c.execute_batch(&format!("PRAGMA recursive_triggers={mode};BEGIN IMMEDIATE"))
            .unwrap();
        c.execute(
            "UPDATE payables_bill SET revision=2 WHERE id=?",
            [s(&b, "id")],
        )
        .unwrap();
        let mut r = raw["current"].clone();
        r.as_object_mut().unwrap().remove("lines");
        r["number"] = json!(2);
        r["id"] = json!(fb_ids::new_v7().to_string());
        r["action"] = json!("revised");
        raw_insert(c, "payables_billrevision", &r, "").unwrap();
        let mut line = raw["current"]["lines"][0].clone();
        line["id"] = json!(fb_ids::new_v7().to_string());
        line["revision_id"] = r["id"].clone();
        line["origin_position"] = json!(1);
        raw_insert(c, "payables_billline", &line, "").unwrap();
        let mut attack = line.clone();
        attack["id"] = json!(fb_ids::new_v7().to_string());
        attack["position"] = json!(2);
        for policy in ["", "OR REPLACE", "OR IGNORE"] {
            let e = raw_insert(c, "payables_billline", &attack, policy).unwrap_err();
            assert!(
                e.to_string().contains("Retained line origin already used"),
                "{mode}/{policy}: {e}"
            );
            assert_eq!(
                c.query_row(
                    "SELECT count(*) FROM payables_billline WHERE revision_id=?",
                    [s(&r, "id")],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
        }
        c.execute_batch("ROLLBACK;PRAGMA recursive_triggers=ON")
            .unwrap();
        assert_eq!(snap(&f), before);
    }
    reopened(&f);
}
