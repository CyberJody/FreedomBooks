use super::primitive::{refuse, revision, transfer_row};
use super::*;

#[test]
pub(super) fn new_guards_have_independent_refusals() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let b = check(f.book.connection(), f.co, s(&b, "id")).unwrap();
    let (actor, co, bank, bank2) = (f.actor, f.co, f.bank, f.bank2);
    for field in ["bank_snapshot", "payee_snapshot", "company_snapshot"] {
        for bad in [
            json!("null"),
            json!("1.5"),
            json!({}),
            json!({"id":"wrong"}),
        ] {
            refuse(&mut f, "Invalid header snapshot", |tx| {
                let mut r = revision(tx, &b, "DRAFT")?;
                r[field] = bad;
                insert(tx, "checks_checkrevision", &r)
            });
        }
    }
    for field in ["account_snapshot", "item_snapshot"] {
        refuse(&mut f, "Invalid line snapshot", |tx| {
            let r = revision(tx, &b, "DRAFT")?;
            insert(tx, "checks_checkrevision", &r)?;
            let mut l = b["current"]["lines"][0].clone();
            l["revision_id"] = r["id"].clone();
            l["id"] = json!(fb_ids::new_v7().to_string());
            l[field] = json!("1.5");
            insert(tx, "checks_checkline", &l)
        });
    }
    for field in ["from_snapshot", "to_snapshot", "company_snapshot"] {
        refuse(&mut f, "Invalid transfer snapshot", |tx| {
            let mut t = transfer_row(tx, actor, co, bank, bank2, false)?;
            t[field] = json!({});
            insert(tx, "transfers_transfer", &t)
        });
    }
    for duplicate in [false, true] {
        refuse(
            &mut f,
            if duplicate {
                "Retained origin already used"
            } else {
                "Origin must be in the immediately preceding revision"
            },
            |tx| {
                let r = revision(tx, &b, "DRAFT")?;
                insert(tx, "checks_checkrevision", &r)?;
                let mut l = b["current"]["lines"][0].clone();
                l["revision_id"] = r["id"].clone();
                l["id"] = json!(fb_ids::new_v7().to_string());
                l["origin_position"] = json!(if duplicate { 1 } else { 50 });
                insert(tx, "checks_checkline", &l)?;
                l["id"] = json!(fb_ids::new_v7().to_string());
                l["position"] = json!(2);
                insert(tx, "checks_checkline", &l)
            },
        );
    }
    for update in [false, true] {
        refuse(&mut f, "Invalid check number", |tx| {
            if update {
                tx.execute(
                    "UPDATE checks_check SET revision=revision+1,number_key=? WHERE id=?",
                    params!["a\u{200b}", s(&b, "id")],
                )?;
                Ok(())
            } else {
                let mut root = b.clone();
                root.as_object_mut().unwrap().remove("current");
                root.as_object_mut().unwrap().remove("number");
                root["number_key"] = json!("a\u{e000}");
                insert(tx, "checks_check", &root)
            }
        });
    }
    for header in [true, false] {
        let name = if header {
            "fb_checks_revision_snapshot_insert"
        } else {
            "fb_checks_line_snapshot_insert"
        };
        let definition: String = f
            .book
            .connection()
            .query_row("SELECT sql FROM sqlite_master WHERE name=?", [name], |r| {
                r.get(0)
            })
            .unwrap();
        f.book
            .connection()
            .execute_batch(&format!("DROP TRIGGER {name}"))
            .unwrap();
        refuse(
            &mut f,
            if header {
                "Invalid sealed header evidence"
            } else {
                "Invalid sealed line evidence"
            },
            |tx| {
                let mut r = revision(tx, &b, "DRAFT")?;
                if header {
                    r["bank_snapshot"] = json!([]);
                }
                insert(tx, "checks_checkrevision", &r)?;
                let mut l = b["current"]["lines"][0].clone();
                l["revision_id"] = r["id"].clone();
                l["id"] = json!(fb_ids::new_v7().to_string());
                if !header {
                    l["account_snapshot"] = json!([]);
                }
                insert(tx, "checks_checkline", &l)?;
                tx.execute(
                    "INSERT INTO fb_seal VALUES('check_revision',?)",
                    [format!("{}:2", s(&b, "id"))],
                )?;
                Ok(())
            },
        );
        f.book.connection().execute_batch(&definition).unwrap();
    }
    let definition: String = f
        .book
        .connection()
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='fb_checks_transfer_snapshot_insert'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    f.book
        .connection()
        .execute_batch("DROP TRIGGER fb_checks_transfer_snapshot_insert")
        .unwrap();
    refuse(&mut f, "Invalid sealed transfer evidence", |tx| {
        let mut t = transfer_row(tx, actor, co, bank, bank2, false)?;
        t["from_snapshot"] = json!([]);
        insert(tx, "transfers_transfer", &t)?;
        tx.execute(
            "INSERT INTO fb_seal VALUES('transfer_journal',?)",
            [s(&t, "journal_id")],
        )?;
        Ok(())
    });
    f.book.connection().execute_batch(&definition).unwrap();
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn snapshot_and_number_corruption_is_refused_with_exact_schema_restored() {
    for (table, field, bad) in [
        ("checks_checkrevision", "bank_snapshot", "1.5"),
        ("checks_checkrevision", "payee_snapshot", "{}"),
        ("checks_checkrevision", "company_snapshot", "[]"),
        ("checks_checkline", "account_snapshot", "{}"),
        ("checks_checkline", "item_snapshot", "false"),
        ("checks_checkrevision", "check_number", "a\u{200b}"),
        ("checks_checkrevision", "check_number", "a\u{e000}"),
        ("checks_checkrevision", "check_number", "a\u{2028}"),
        ("checks_checkrevision", "check_number", "a\u{1}"),
    ] {
        let mut f = Fx::new();
        let p = f.payload();
        let b = f.save(&p);
        let review = f.review(&b);
        let c = f.book.connection();
        let name = format!("fb_{table}_update");
        let definition: String = c
            .query_row("SELECT sql FROM sqlite_master WHERE name=?", [&name], |r| {
                r.get(0)
            })
            .unwrap();
        c.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
        c.execute(&format!("UPDATE {table} SET {field}=?"), [bad])
            .unwrap();
        c.execute_batch(&definition).unwrap();
        assert!(!f.book.check().unwrap().is_empty(), "{table}.{field}={bad}");
        let path = std::path::PathBuf::from(c.path().unwrap());
        let before = f.counts();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            record_check(&mut f.book, f.actor, f.co, s(&b, "id"), Some(1), &review)
        }));
        assert!(result.is_ok());
        assert!(result.unwrap().is_err());
        assert_eq!(before, f.counts());
        drop(f);
        assert!(Book::open(&path, &[42; 32]).is_err());
    }
}

#[test]
fn shared_number_sql_function_matches_service_and_is_read_only() {
    let mut f = Fx::new();
    for value in [
        "",
        "123",
        "001",
        "é-20",
        "１２３",
        "a\u{200b}",
        "a\u{e000}",
        "a\u{2028}",
        "a\u{1}",
        " 123 ",
        "123456789012345678901",
    ] {
        let expected = number_text(&json!(value)).is_ok_and(|v| v == value);
        assert_eq!(fb_db::check_numbers::valid(value), expected);
        f.book
            .read(|tx| -> FbResult<()> {
                let valid: bool =
                    tx.query_row("SELECT fb_check_number_valid(?)", [value], |r| r.get(0))?;
                assert_eq!(valid, expected);
                Ok(())
            })
            .unwrap();
        f.book
            .write(|tx| -> FbResult<()> {
                let valid: bool =
                    tx.query_row("SELECT fb_check_number_valid(?)", [value], |r| r.get(0))?;
                assert_eq!(valid, expected);
                Ok(())
            })
            .unwrap();
    }
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn typed_snapshot_fields_and_linked_identities_are_required() {
    let mut f = Fx::new();
    let item=directory::save_record(&mut f.book,f.actor,f.co,Model::Item,
        &json!({"code":"Typed","kind":"NONSTOCK","sale_rate":"1","purchase_cost":"2","expense_account":f.expense.to_db()}),None).unwrap();
    let mut p = f.payload();
    p["lines"][0] = json!({"tab":"ITEM","item_id":item["id"],"account_id":f.expense.to_db(),"quantity":"1","rate":"1"});
    let b = f.save(&p);
    let r = &b["current"];
    for (kind, value, expected) in [
        ("account", &r["bank_snapshot"], r["bank_id"].as_str()),
        ("party", &r["payee_snapshot"], r["vendor_id"].as_str()),
        (
            "company",
            &r["company_snapshot"],
            Some(f.co.to_db()).as_deref(),
        ),
        (
            "item",
            &r["lines"][0]["item_snapshot"],
            r["lines"][0]["item_id"].as_str(),
        ),
    ] {
        assert!(fb_db::checks_snapshots::valid(kind, value, expected));
        let invalid = |v: &Value| {
            assert!(
                !fb_db::checks_snapshots::valid(kind, v, expected),
                "{kind}: {v}"
            );
            let accepted: bool = f
                .book
                .connection()
                .query_row(
                    "SELECT fb_checks_snapshot_valid(?1,?2,?3)",
                    params![kind, v.to_string(), expected],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(!accepted);
        };
        for field in value.as_object().unwrap().keys() {
            let mut bad = value.clone();
            bad.as_object_mut().unwrap().remove(field);
            invalid(&bad);
            for bad_value in [json!(false), json!(1.5), json!([])] {
                let mut bad = value.clone();
                bad[field] = bad_value;
                invalid(&bad);
            }
        }
        let mut bad = value.clone();
        bad["id"] = json!("different-parent");
        invalid(&bad);
    }
    for cost in ["-1.00", "0.001", "NaN", "100000000000000.00"] {
        let mut bad = r["lines"][0]["item_snapshot"].clone();
        bad["purchase_cost"] = json!(cost);
        assert!(!fb_db::checks_snapshots::valid(
            "item",
            &bad,
            r["lines"][0]["item_id"].as_str()
        ));
    }
    let before = f.counts();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        f.book
            .write(|tx| insert(tx, "checks_check", &json!({"revision":1.5})))
    }));
    assert!(result.is_ok());
    assert!(result.unwrap().is_err());
    assert_eq!(before, f.counts());
}

#[test]
fn origins_follow_immediate_prior_lines_across_reorder_and_posting() {
    let mut f = Fx::new();
    let mut p = f.payload();
    p["lines"]
        .as_array_mut()
        .unwrap()
        .push(json!({"tab":"EXPENSE","account_id":f.bank2.to_db(),"rate":"2"}));
    let b = f.save(&p);
    f.book
        .write(|tx| -> FbResult<()> {
            tx.execute(
                "UPDATE ledger_account SET active=0,postable=0 WHERE id IN (?,?)",
                params![f.expense.to_db(), f.bank2.to_db()],
            )?;
            Ok(())
        })
        .unwrap();
    p["check_id"] = b["id"].clone();
    p["expected_revision"] = json!(1);
    p["lines"][0]["origin_position"] = json!(1);
    p["lines"][1]["origin_position"] = json!(2);
    p["lines"].as_array_mut().unwrap().swap(0, 1);
    let b = f.save(&p);
    let raw = check(f.book.connection(), f.co, s(&b, "id")).unwrap();
    assert_eq!(raw["current"]["lines"][0]["origin_position"], 2);
    for wrong in [Value::Null, json!(1)] {
        refuse(&mut f, "Invalid draft purchase account", |tx| {
            let r = revision(tx, &raw, "DRAFT")?;
            insert(tx, "checks_checkrevision", &r)?;
            let mut l = raw["current"]["lines"][1].clone();
            l["revision_id"] = r["id"].clone();
            l["id"] = json!(fb_ids::new_v7().to_string());
            l["origin_position"] = wrong;
            insert(tx, "checks_checkline", &l)
        });
    }
    p["expected_revision"] = json!(2);
    p["lines"][0]["origin_position"] = json!(1);
    p["lines"][1]["origin_position"] = json!(2);
    let b = f.save(&p);
    f.book
        .write(|tx| -> FbResult<()> {
            tx.execute(
                "UPDATE ledger_account SET active=1,postable=1 WHERE id IN (?,?)",
                params![f.expense.to_db(), f.bank2.to_db()],
            )?;
            Ok(())
        })
        .unwrap();
    let b = f.post(&b);
    f.void(&b);
    assert!(f.book.check().unwrap().is_empty());
    let path = std::path::PathBuf::from(f.book.connection().path().unwrap());
    drop(f);
    assert!(Book::open(&path, &[42; 32]).is_ok());
}
