use super::*;

#[test]
fn noop_bill_identity_can_accompany_a_valid_revision_advance() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let c = f.book.connection();
    let before = super::rebase_tests::snapshot(c);
    for recursion in [0, 1] {
        c.execute_batch(&format!(
            "PRAGMA recursive_triggers={recursion}; BEGIN IMMEDIATE"
        ))
        .unwrap();
        let result = c.execute("UPDATE payables_bill SET id=id,identity_id=identity_id,company_id=company_id,revision=revision+1 WHERE id=?", [s(&b,"id")]);
        c.execute_batch("ROLLBACK").unwrap();
        result.expect("unchanged keys must not block a legitimate revision advance");
        assert_eq!(super::rebase_tests::snapshot(c), before);
    }
}

#[test]
fn every_purchase_detail_type_matches_at_service_and_sql_boundaries() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let raw = bill(f.book.connection(), f.co, s(&b, "id")).unwrap();
    for (index, detail) in crate::model::DetailType::ALL.into_iter().enumerate() {
        let a = services::save_account(
            &mut f.book,
            f.actor,
            f.co,
            AccountInput::new(&format!("D{index}"), detail.as_str())
                .kind(detail.kind().as_str())
                .detail(detail.as_str()),
        )
        .unwrap();
        let expected =
            !["RECEIVABLE", "PAYABLE", "INCOME", "OTHER_INCOME"].contains(&detail.as_str());
        let mut p = f.payload();
        p["reference"] = json!(detail.as_str());
        p["lines"][0]["expense_account_id"] = json!(a.id.to_db());
        let saved = save_bill(&mut f.book, f.actor, f.co, &p);
        assert_eq!(
            saved.is_ok(),
            expected,
            "service {}: {saved:?}",
            detail.as_str()
        );
        if let Ok(saved) = saved {
            f.post(&saved);
        }
        let before = super::rebase_tests::snapshot(f.book.connection());
        f.book
            .write(|tx| -> FbResult<()> {
                let r = super::attacks::new_revision(tx, &raw, "DRAFT")?;
                insert(tx, "payables_billrevision", &r)?;
                let mut l = raw["current"]["lines"][0].clone();
                l["id"] = json!(fb_ids::new_v7().to_string());
                l["revision_id"] = r["id"].clone();
                l["expense_account_id"] = json!(a.id.to_db());
                let result = insert(tx, "payables_billline", &l);
                assert_eq!(
                    result.is_ok(),
                    expected,
                    "SQL {}: {result:?}",
                    detail.as_str()
                );
                Err(err("rollback fixture"))
            })
            .unwrap_err();
        assert_eq!(super::rebase_tests::snapshot(f.book.connection()), before);
    }
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn new_draft_vendor_eligibility_matches_service_and_sql() {
    let mut f = Fx::new();
    let p = f.payload();
    let b = f.save(&p);
    let raw = bill(f.book.connection(), f.co, s(&b, "id")).unwrap();
    f.book
        .connection()
        .execute(
            "UPDATE directory_vendor SET active=0 WHERE id=?",
            [&f.vendor],
        )
        .unwrap();
    let mut p = f.payload();
    p["reference"] = json!("Other");
    assert!(save_bill(&mut f.book, f.actor, f.co, &p).is_err());
    let before = super::rebase_tests::snapshot(f.book.connection());
    f.book
        .write(|tx| -> FbResult<()> {
            let identity = documents::create_identity(
                tx,
                f.actor,
                f.co,
                "BILL",
                &fb_ids::new_v7().to_string(),
            )?;
            let mut root = raw.clone();
            root.as_object_mut().unwrap().retain(|k, _| {
                [
                    "id",
                    "company_id",
                    "identity_id",
                    "creation_digest",
                    "revision",
                    "vendor_id",
                    "reference_key",
                    "state",
                ]
                .contains(&k.as_str())
            });
            root["id"] = json!(fb_ids::new_v7().to_string());
            root["identity_id"] = identity["id"].clone();
            root["reference_key"] = json!("other");
            insert(tx, "payables_bill", &root)?;
            let mut r = raw["current"].clone();
            r.as_object_mut().unwrap().remove("lines");
            r["id"] = json!(fb_ids::new_v7().to_string());
            r["bill_id"] = root["id"].clone();
            r["reference"] = json!("Other");
            let error = insert(tx, "payables_billrevision", &r)
                .expect_err("SQL accepted archived new vendor");
            assert_eq!(error.constraint(), Some("bill_vendor_eligible"));
            Err(err("rollback fixture"))
        })
        .unwrap_err();
    assert_eq!(super::rebase_tests::snapshot(f.book.connection()), before);
}

#[test]
fn new_draft_item_eligibility_matches_service_and_sql() {
    let mut f = Fx::new();
    let item = directory::save_record(
        &mut f.book,
        f.actor,
        f.co,
        Model::Item,
        &json!({"code":"Archived item","kind":"SERVICE","sale_rate":"1.00"}),
        None,
    )
    .unwrap();
    directory::set_active(
        &mut f.book,
        f.actor,
        f.co,
        Model::Item,
        s(&item, "id"),
        false,
    )
    .unwrap();
    let p = f.payload();
    let b = f.save(&p);
    let raw = bill(f.book.connection(), f.co, s(&b, "id")).unwrap();
    let mut p = p.clone();
    p["bill_id"] = b["id"].clone();
    p["expected_revision"] = json!(1);
    p["lines"][0]["item_id"] = item["id"].clone();
    assert!(save_bill(&mut f.book, f.actor, f.co, &p).is_err());
    let before = super::rebase_tests::snapshot(f.book.connection());
    f.book
        .write(|tx| -> FbResult<()> {
            let r = super::attacks::new_revision(tx, &raw, "DRAFT")?;
            insert(tx, "payables_billrevision", &r)?;
            let mut l = raw["current"]["lines"][0].clone();
            l["id"] = json!(fb_ids::new_v7().to_string());
            l["revision_id"] = r["id"].clone();
            l["item_id"] = item["id"].clone();
            let error =
                insert(tx, "payables_billline", &l).expect_err("SQL accepted archived new item");
            assert_eq!(error.constraint(), Some("bill_item_eligible"));
            Err(err("rollback fixture"))
        })
        .unwrap_err();
    assert_eq!(super::rebase_tests::snapshot(f.book.connection()), before);
}

fn exhaust(f: &Fx, key: &str) {
    let c = f.book.connection();
    let definitions: Vec<(String, String)> = c.prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger' AND tbl_name IN('payables_bill','payables_billrevision')").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
    for (name, _) in &definitions {
        c.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
    }
    c.execute("UPDATE payables_billrevision SET number=?1 WHERE bill_id=?2 AND number=(SELECT revision FROM payables_bill WHERE id=?2)", params![i64::MAX,key]).unwrap();
    c.execute(
        "UPDATE payables_bill SET revision=? WHERE id=?",
        params![i64::MAX, key],
    )
    .unwrap();
    for (_, sql) in definitions {
        c.execute_batch(&sql).unwrap();
    }
}

#[test]
fn exhausted_bill_revisions_fail_validation_without_mutation_or_panic() {
    for action in ["save", "post", "void"] {
        let mut f = Fx::new();
        let p = f.payload();
        let b = f.save(&p);
        let b = if action == "void" { f.post(&b) } else { b };
        let key = s(&b, "id");
        exhaust(&f, key);
        let review = if action == "post" {
            Some(review_bill(&mut f.book, f.actor, f.co, key, Some(i64::MAX)).unwrap())
        } else {
            None
        };
        let before = super::rebase_tests::snapshot(f.book.connection());
        let result = match action {
            "save" => {
                let mut p = p.clone();
                p["bill_id"] = json!(key);
                p["expected_revision"] = json!(i64::MAX);
                save_bill(&mut f.book, f.actor, f.co, &p)
            }
            "post" => post_bill(
                &mut f.book,
                f.actor,
                f.co,
                key,
                Some(i64::MAX),
                review.as_ref().unwrap(),
            ),
            _ => void_bill(
                &mut f.book,
                f.actor,
                f.co,
                key,
                Some(i64::MAX),
                &json!("2026-10-24"),
                &json!("Correction"),
            ),
        };
        let error = result.unwrap_err();
        assert_eq!(error.kind(), "ValidationError", "{action}: {error:?}");
        assert!(error.message().contains("exhausted"), "{action}: {error:?}");
        assert_eq!(super::rebase_tests::snapshot(f.book.connection()), before);
    }
}
