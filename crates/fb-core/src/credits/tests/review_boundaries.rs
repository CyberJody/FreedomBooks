#[test]
fn review_designations_reject_wrong_tax_and_preserve_historical_roles() {
    let mut f = Fixture::new();
    let p = f.input();
    let plan = review_credit_memo(&mut f.book, f.actor, f.company, &p).unwrap();
    let base = record_credit_memo(&mut f.book, f.actor, f.company, &p, Some(&plan)).unwrap();
    let mut a = AccountInput::new("211", "Other liability");
    a.kind = Some("LIABILITY".into());
    let other_tax = services::save_account(&mut f.book, f.actor, f.company, a)
        .unwrap()
        .id;
    let actor = f.actor;
    let company = f.company;
    let before = snapshot(&f);
    let result: FbResult<()> = f.book.write(|tx| {
        let mut m = memo_shell(tx, actor, company, &base, &plan.display, false, false)?;
        m["collected_tax_id"] = json!(other_tax.to_db());
        insert(tx, "credits_creditmemo", &m)
    });
    assert!(
        matches!(result, Err(FbError::Integrity { constraint: Some(ref s), .. }) if s == "credit_memo_designation")
    );
    assert_eq!(snapshot(&f), before);
    documents::save_setup(&mut f.book, actor, company,
        &json!({"receivables":f.bank.to_db(),"collected_tax":other_tax.to_db(),"banks":[f.ar.to_db()]}), Some(1)).unwrap();
    f.clean();
    void_credit_memo(
        &mut f.book,
        actor,
        company,
        base["id"].as_str().unwrap(),
        Some(1),
        &json!("2026-10-20"),
        &json!("Historical role change"),
    )
    .unwrap();
    f.clean();
}

#[test]
fn review_complete_nested_snapshot_matrix_rejects_at_insert() {
    let mut f = Fixture::new();
    let p = f.input();
    let plan = review_credit_memo(&mut f.book, f.actor, f.company, &p).unwrap();
    let base = record_credit_memo(&mut f.book, f.actor, f.company, &p, Some(&plan)).unwrap();
    let full = get_credit_memo(
        &mut f.book,
        f.actor,
        f.company,
        base["id"].as_str().unwrap(),
    )
    .unwrap();
    let (actor, company) = (f.actor, f.company);
    for (path, value) in [
        ("/customer_snapshot", Value::Null),
        ("/customer_snapshot/phone", json!(false)),
        ("/customer_snapshot/id", json!(new_id())),
        ("/company_snapshot", json!({})),
        ("/company_snapshot/currency", json!([])),
        ("/company_snapshot/id", json!(new_id())),
    ] {
        let before = snapshot(&f);
        let result: FbResult<()> = f.book.write(|tx| {
            let mut m = memo_shell(tx, actor, company, &base, &plan.display, false, false)?;
            *m.pointer_mut(path).unwrap() = value;
            insert(tx, "credits_creditmemo", &m)
        });
        assert!(result.is_err(), "accepted {path}");
        assert_eq!(snapshot(&f), before);
    }
    for (path, value) in [
        ("/account_snapshot", json!([])),
        ("/account_snapshot/income_account", json!(false)),
        ("/account_snapshot/income_account/code", json!(12)),
        ("/account_snapshot/income_account/id", json!(new_id())),
        ("/account_snapshot/income_account/kind", json!("ASSET")),
        ("/item_snapshot", json!({"id":"unrelated"})),
    ] {
        let before = snapshot(&f);
        let result: FbResult<()> = f.book.write(|tx| {
            let m = memo_shell(tx, actor, company, &base, &plan.display, false, false)?;
            insert(tx, "credits_creditmemo", &m)?;
            let mut ls = full["lines"].as_array().unwrap().clone();
            *ls[0].pointer_mut(path).unwrap() = value;
            lines(tx, &m, &ls)
        });
        assert!(result.is_err(), "accepted {path}");
        assert_eq!(snapshot(&f), before);
    }
    f.clean();
}

#[test]
fn review_seal_rechecks_snapshots_even_if_insert_guard_was_bypassed() {
    let mut f = Fixture::new();
    let base = f.memo();
    let id = base["id"].as_str().unwrap().to_owned();
    let c = f.book.connection();
    let def: String = c
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='fb_credits_creditmemoline_update'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let original: String = c
        .query_row(
            "SELECT data FROM credits_creditmemoline WHERE credit_memo_id=?1 AND position=1",
            [&id],
            |r| r.get(0),
        )
        .unwrap();
    c.execute_batch("BEGIN IMMEDIATE; DROP TRIGGER fb_credits_creditmemoline_update;")
        .unwrap();
    c.execute("UPDATE credits_creditmemoline SET data=json_set(data,'$.account_snapshot.income_account',false) WHERE credit_memo_id=?1 AND position=1", [&id]).unwrap();
    c.execute_batch(&def).unwrap();
    c.execute_batch("COMMIT").unwrap();
    let result: FbResult<()> = f.book.write(|tx| seal(tx, "memo", &id));
    assert!(
        matches!(result, Err(FbError::Integrity {constraint: Some(ref s), ..}) if s == "credit_snapshot"),
        "{result:?}"
    );
    let c = f.book.connection();
    c.execute_batch("BEGIN IMMEDIATE; DROP TRIGGER fb_credits_creditmemoline_update;")
        .unwrap();
    c.execute(
        "UPDATE credits_creditmemoline SET data=?1 WHERE credit_memo_id=?2 AND position=1",
        params![original, id],
    )
    .unwrap();
    c.execute_batch(&def).unwrap();
    c.execute_batch("COMMIT").unwrap();
    f.clean();
}

#[test]
fn review_open_and_reader_refuse_corrupt_nested_snapshots_with_exact_schema_restored() {
    for (table, field, value) in [
        ("creditmemo", "$.company_snapshot.id", json!(new_id())),
        ("creditmemo", "$.customer_snapshot.phone", json!(false)),
        (
            "creditmemoline",
            "$.account_snapshot.income_account",
            json!(false),
        ),
    ] {
        let mut f = Fixture::new();
        let base = f.memo();
        let c = f.book.connection();
        let name = format!("fb_credits_{table}_update");
        let definition: String = c
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name=?1",
                [&name],
                |r| r.get(0),
            )
            .unwrap();
        c.execute_batch(&format!("BEGIN IMMEDIATE; DROP TRIGGER {name};"))
            .unwrap();
        c.execute(
            &format!("UPDATE credits_{table} SET data=json_set(data,?1,json(?2))"),
            params![field, value.to_string()],
        )
        .unwrap();
        c.execute_batch(&definition).unwrap();
        c.execute_batch("COMMIT").unwrap();
        assert!(!f.book.check().unwrap().is_empty());
        assert!(Book::open(&f.path, &[24; 32]).is_err());
        let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            get_credit_memo(
                &mut f.book,
                f.actor,
                f.company,
                base["id"].as_str().unwrap(),
            )
        }));
        assert!(read.is_ok_and(|r| r.is_err()));
    }
}

#[test]
fn review_date_layouts_reuse_python_conversion_without_django_fallback() {
    for (input, expected) in [
        ("2026-01-01", "2026-01-01"),
        ("20260101", "2026-01-01"),
        ("2026W01", "2025-12-29"),
        ("2026W011", "2025-12-29"),
        ("2026-W01", "2025-12-29"),
        ("2026-W01-1", "2025-12-29"),
        ("0001-01-01", "0001-01-01"),
        ("9999-12-31", "9999-12-31"),
    ] {
        assert_eq!(date(&json!(input)).unwrap(), expected);
    }
    for input in [
        "2026-1-01",
        "2026-01-1",
        "2026-01-01\n",
        "２０２６-01-01",
        "0000-01-01",
        "2026-W00-1",
        "2026-W54-1",
        "2026-W01-0",
        "2026-W01-8",
        "2026-02-29",
        "9999-W52-7",
    ] {
        assert!(date(&json!(input)).is_err(), "accepted {input}");
    }
}

#[test]
fn review_upgrade_from_populated_0060_preserves_every_row_and_key() {
    let mut f = Fixture::new();
    let m = f.memo();
    let p = f.refund_input("1.00");
    let plan = review_refund(
        &mut f.book,
        f.actor,
        f.company,
        m["id"].as_str().unwrap(),
        &p,
    )
    .unwrap();
    record_refund(
        &mut f.book,
        f.actor,
        f.company,
        m["id"].as_str().unwrap(),
        &p,
        Some(&plan),
    )
    .unwrap();
    let path=f.path.with_extension("0060-upgrade.fbbook");
    let copy=fb_db::test_support::populated_copy(f.book.connection(),&path,&[24;32],"0061_credits_review_fixes",false).unwrap();
    f.path = path.clone();
    f.book=Book::open(&f.path,&[24;32]).unwrap();
    assert_eq!(f.book.connection().path(), f.path.to_str(), "fixture must follow upgraded copy");
    for _ in 0..2 {
        assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(),copy.expected);
        f.clean();
        f.book = Book::open(&f.path, &[24; 32]).unwrap();
    }
    assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(),copy.expected);
}

#[test]
fn review_item_and_tax_snapshots_validate_every_nested_field() {
    let mut f = Fixture::new();
    crate::salestax::save_settings(
        &mut f.book,
        f.actor,
        f.company,
        &json!({"charge_tax":true,"default_item_id":null,"owed_basis":"ACCRUAL","filing_frequency":"MONTHLY","expected_revision":0}),
    ).unwrap();
    let item = directory::save_record(&mut f.book, f.actor, f.company, Model::Item,
        &json!({"code":"RETURN","description":"Synthetic item","kind":"NONSTOCK","sale_rate":"99999999999999.99","income_account":f.income.to_db()}), None).unwrap();
    let tax = crate::salestax::save_rate(&mut f.book, f.actor, f.company,
        &json!({"name":"Return tax","description":"Return tax","rate":"8","new_agency_name":"Synthetic agency"})).unwrap();
    let mut p = f.input();
    p["lines"][0]["item_id"] = item["id"].clone();
    p["tax_mode"] = json!("ITEM");
    p["tax_item_id"] = tax["id"].clone();
    let plan = review_credit_memo(&mut f.book, f.actor, f.company, &p).unwrap();
    let m = record_credit_memo(&mut f.book, f.actor, f.company, &p, Some(&plan)).unwrap();
    let full = get_credit_memo(&mut f.book, f.actor, f.company, m["id"].as_str().unwrap()).unwrap();
    assert!(fb_db::credit_fields_valid("creditmemo", &m));
    for (path, value) in [
        ("/tax_snapshot/name", json!(false)),
        ("/tax_snapshot/kind", json!("BOGUS")),
        ("/tax_snapshot/id", json!(new_id())),
        ("/tax_snapshot/rate", json!("NaN")),
        ("/tax_snapshot/members/0/name", json!([])),
        ("/tax_snapshot/members/0/agency_name", Value::Null),
        ("/tax_snapshot/members/0/agency_id", json!({})),
        ("/tax_snapshot/members/0/rate", json!("101.0000")),
        ("/tax_snapshot/members/0/tax", json!("not-money")),
    ] {
        let mut bad = m.clone();
        *bad.pointer_mut(path).unwrap() = value;
        assert!(
            !fb_db::credit_fields_valid("creditmemo", &bad),
            "accepted {path}"
        );
    }
    let line = &full["lines"][0];
    assert!(fb_db::credit_fields_valid("creditmemoline", line));
    for (path, value) in [
        ("/item_snapshot/id", json!(new_id())),
        ("/item_snapshot/code", json!(false)),
        ("/item_snapshot/kind", json!("BOGUS")),
        ("/item_snapshot/sale_rate", json!([])),
        ("/item_snapshot/sale_rate", json!("NaN")),
        ("/item_snapshot/sale_rate", json!("100000000000000.00")),
    ] {
        let mut bad = line.clone();
        *bad.pointer_mut(path).unwrap() = value;
        assert!(
            !fb_db::credit_fields_valid("creditmemoline", &bad),
            "accepted {path}"
        );
    }
    f.clean();
}
