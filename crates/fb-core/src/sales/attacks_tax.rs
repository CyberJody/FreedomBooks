use super::*;
fn taxes(f: &mut Fixture) -> Value {
    let mut ids = Vec::new();
    for n in 1..=2 {
        let v=crate::salestax::save_rate(&mut f.book,f.actor,f.company,&json!({"name":format!("Rate {n}"),"description":format!("Rate {n}"),"rate":format!("{n}"),"new_agency_name":format!("Agency {n}"),"expected_revision":0})).unwrap();
        ids.push(v["id"].clone());
    }
    crate::salestax::save_settings(&mut f.book,f.actor,f.company,&json!({"charge_tax":true,"default_item_id":null,"owed_basis":"ACCRUAL","filing_frequency":"QUARTERLY","expected_revision":0})).unwrap();
    crate::salestax::save_group(
        &mut f.book,
        f.actor,
        f.company,
        &json!({"name":"Both","description":"Both rates","member_ids":ids,"expected_revision":0}),
    )
    .unwrap()
}
#[test]
pub(super) fn tax_scope_group_members_and_forged_heads_are_refused() {
    let mut f = Fixture::new();
    let group = taxes(&mut f);
    let base = f.save();
    let actor = f.actor;
    let c = f.company;
    let bank = f.bank;
    let income = f.income;
    let tax_account = f.tax;
    let picked = crate::salestax::document_tax(
        f.book.connection(),
        c,
        &json!({"tax_mode":"ITEM","tax_item_id":group["id"]}),
        None,
        true,
    )
    .unwrap();
    // Group detail with duplicate member IDs has exact decimal math, but invalid membership.
    let mut detail = calculate(base["lines"].as_array().unwrap(), &picked, true).unwrap();
    detail["tax_item_id"] = group["id"].clone();
    detail["collected_tax_id"] = json!(tax_account.to_db());
    detail["tax_snapshot"]["members"][1]["id"] = detail["tax_snapshot"]["members"][0]["id"].clone();
    let result: FbResult<()> = f.book.write(|tx| {
        let r = super::lifecycle::raw_receipt(
            tx,
            actor,
            c,
            &f.customer,
            bank,
            income,
            "SR",
            cents(&detail["total"]).unwrap(),
            |_| {},
        )
        .unwrap();
        refusal(
            tx.execute(
                "INSERT INTO sales_salesreceipt VALUES(?1,?2,?3,?4)",
                params![
                    new_id(),
                    r["id"].as_str(),
                    cents(&detail["total"]).unwrap(),
                    detail.to_string()
                ],
            )
            .unwrap_err(),
            "sales_tax_members",
        );
        Err(FbError::validation("rolled-back fixture"))
    });
    assert!(result.is_err());
    // A header claiming RATE for a GROUP id passes arithmetic, but not ownership/type.
    let mut wrong = picked.clone();
    wrong["tax_mode"] = json!("RATE");
    wrong["tax_snapshot"]["kind"] = json!("RATE");
    let member = wrong["tax_snapshot"]["members"][0].clone();
    wrong["tax_snapshot"]["members"] = json!([member]);
    let calc = calculate(base["lines"].as_array().unwrap(), &wrong, false).unwrap();
    let mut h = base["current"].clone();
    copy_calc(&mut h, &calc);
    h["tax_item_id"] = group["id"].clone();
    h["id"] = json!(new_id());
    let key = new_id();
    let result: FbResult<()> = f.book.write(|tx| {
    let source = documents::create_identity(tx, actor, c, "INV", &new_id()).unwrap();
    tx.execute(
        "INSERT INTO sales_invoice VALUES(?1,?2,?3,?4,1)",
        params![key, c.to_db(), source["id"].as_str(), "0".repeat(64)],
    )
    .unwrap();
    tx.execute("INSERT INTO sales_invoicerevision VALUES(?1,?2,1,?3,'2026-10-01','2026-10-31','DRAFT',?4,NULL,NULL,?5,?6)",params![h["id"].as_str(),key,f.customer,cents(&h["total"]).unwrap(),h.to_string(),actor.to_db()]).unwrap();
    insert_line(tx, h["id"].as_str().unwrap(), &base["lines"][0]).unwrap();
    refusal(
        tx.execute(
            "INSERT INTO sales_seal VALUES('sales_invoice_revision',?1)",
            [h["id"].as_str().unwrap()],
        )
        .unwrap_err(),
        "sales_tax_scope",
    );
    Err(FbError::validation("rolled-back fixture"))
    });
    assert!(result.is_err());
    let conn = f.book.connection();
    expect(
        conn,
        &format!(
            "INSERT INTO sales_seal VALUES('sales_invoice_head',{})",
            sqlq(&format!("{}:900", base["id"].as_str().unwrap()))
        ),
        "invoice_complete",
    );
    expect(
        conn,
        &format!(
            "INSERT INTO sales_invoice VALUES({}, {}, {}, {}, 1)",
            sqlq(&new_id()),
            sqlq(&c.to_db()),
            sqlq(&new_id()),
            sqlq(&"0".repeat(64))
        ),
        "invoice_identity_scope",
    );
}
#[test]
fn every_sales_guard_message_has_an_executed_raw_sql_attack() {
    // Execute each attack group here as well: the coverage assertion is independent of
    // Rust's parallel test ordering, and fails when a new named guard lacks an attack.
    super::all_protected_tables_reject_updates_deletes_and_every_unique_key_replace();
    super::referenced_directory_accounts_sources_and_lines_cannot_be_replaced();
    super::incomplete_revisions_missing_parents_line_calculations_and_seals_are_refused();
    super::late_lines_references_allocations_and_cross_kind_details_are_refused();
    super::orphan_receipt_journals_and_invoice_roots_cannot_commit();
    super::lifecycle::receipts_allocations_details_totals_and_postings_have_independent_backstops();
    super::lifecycle::posting_financial_copy_split_eligibility_and_void_dependencies_are_guarded();
    tax_scope_group_members_and_forged_heads_are_refused();
    let sql = concat!(
        include_str!("../../../fb-db/sql/0010_sales.sql"),
        include_str!("../../../fb-db/sql/0011_sales_references.sql")
    );
    let names = sql
        .split("RAISE(ABORT,")
        .skip(1)
        .filter_map(|s| s.trim_start().strip_prefix('\''))
        .filter_map(|s| s.split_once(':').map(|(n, _)| n.to_string()))
        .collect::<BTreeSet<_>>();
    let seen = SEEN.lock().unwrap();
    let missing = names.difference(&seen).collect::<Vec<_>>();
    assert!(missing.is_empty(), "unattacked guards: {missing:?}");
    println!("Sales guard messages attacked: {}", names.len());
}
