use super::*;
use fb_db::{SnapshotSource, test_support::snapshot};

// Hyphens join numeric groups for card-number detection. Keep synthetic references unique
// without allowing UUID groups to accidentally form a Luhn-valid payment card number.
fn synthetic_payment_reference(id: &str) -> String {
    format!("synthetic-{}", id.replace('-', "x"))
}

#[test]
fn synthetic_payment_reference_is_card_free_even_for_numeric_uuids() {
    let id = "00000000-0000-7000-8111-111111111112";
    assert!(crate::card_numbers::has_card_number(&format!("synthetic-{id}")));
    assert!(!crate::card_numbers::has_card_number(&synthetic_payment_reference(id)));
}

fn source(f: &Fixture, state: &str, amount: &str) -> Value {
    let mut invoice = f.input();
    invoice["tax_rate"] = json!("0");
    invoice["tax_mode"] = json!("NONE");
    invoice["due_date"] = json!("2026-10-01");
    invoice["lines"][0]["rate"] = json!(amount);
    let id = new_id();
    let pay = new_id();
    let number = format!("OLD-{}", &id[24..]);
    let posted = ["paid", "sent", "partial"].contains(&state);
    let original = json!({"id":id,"number":17,"display_number":number,"status":state,"issue_date":"2026-10-01","due_date":"2026-10-01",
        "created_at":"2026-10-01T10:00:00.000Z","updated_at":"2026-10-02T10:00:00.000Z","sent_at":if posted {json!("2026-10-01T11:00:00.000Z")}else{Value::Null},"paid_at":if state=="paid"{json!("2026-10-02T10:00:00.000Z")}else{Value::Null},"voided_at":if state=="void"{json!("2026-10-03T10:00:00.000Z")}else{Value::Null},
        "tax_rate":0,"subtotal":amount,"gc_discount":"0","tax_amount":"0","total":amount,"amount_paid":if state=="paid"{amount}else{"0"},"bill_to":"Synthetic customer\nOriginal address","notes":"Synthetic invoice","internal_notes":"Preserved source history",
        "lines":[{"id":new_id(),"invoice_id":id,"quantity":"1","unit_price":amount,"amount":amount,"taxable":1,"description":"Work","item":"Synthetic service"}],
        "payments":if state=="paid" {json!([{"id":pay,"invoice_id":id,"date":"2026-10-02","created_at":"2026-10-02T10:00:00.000Z","amount":amount,"method":"card","reference":synthetic_payment_reference(&pay),"external_id":synthetic_payment_reference(&pay),"notes":"Imported record only"}])}else{json!([])} });
    json!({"invoice":invoice,"source":original})
}
#[test]
fn preview_rolls_back_then_import_preserves_paid_draft_and_both_void_histories() {
    let mut f = Fixture::new();
    let mut sent_void = source(&f, "void", "13");
    sent_void["source"]["sent_at"] = json!("2026-10-01T11:00:00.000Z");
    let rows = vec![
        source(&f, "paid", "10"),
        source(&f, "draft", "11"),
        source(&f, "void", "12"),
        sent_void,
    ];
    let before = snapshot(f.book.connection()).unwrap();
    let preview = historical::run(&mut f.book, f.actor, f.company, &rows, true, None).unwrap();
    assert_eq!(
        snapshot(f.book.connection()).unwrap(),
        before,
        "preview must roll back ALL domain/audit/sequence writes"
    );
    let saved = historical::run(
        &mut f.book,
        f.actor,
        f.company,
        &rows,
        false,
        preview["review_digest"].as_str(),
    )
    .unwrap();
    let balances = invoice_balances(&mut f.book, f.actor, f.company).unwrap();
    assert_eq!(
        balances
            .iter()
            .filter(|v| v["payment_state"] == "PAID")
            .count(),
        1
    );
    assert_eq!(balances.iter().filter(|v| v["state"] == "DRAFT").count(), 1);
    assert_eq!(balances.iter().filter(|v| v["state"] == "VOID").count(), 2);
    f.book.read(|tx|->FbResult<()> {
        let count:i64=tx.query_row("SELECT count(*) FROM ledger_journal",[],|r|r.get(0))?;
        assert_eq!(count,4,"paid invoice + receipt + sent void's invoice/reversal; no draft cancellation journal");
        for row in saved["invoices"].as_array().unwrap() {
            let source:String=tx.query_row("SELECT data FROM sales_importinvoice WHERE invoice_id=?1",[row["invoice_id"].as_str()],|r|r.get(0))?;
            let v:Value=serde_json::from_str(&source).unwrap();assert_eq!(v["source"]["internal_notes"],"Preserved source history");
        }
        Ok(())
    }).unwrap();
    assert!(f.book.check().unwrap().is_empty());
    let after = snapshot(f.book.connection()).unwrap();
    let retry = historical::run(
        &mut f.book,
        f.actor,
        f.company,
        &rows,
        false,
        preview["review_digest"].as_str(),
    )
    .unwrap();
    assert_eq!(retry["already_imported"], true);
    assert_eq!(snapshot(f.book.connection()).unwrap(), after);
    let mut changed = rows.clone();
    changed[0]["source"]["internal_notes"] = json!("Changed history");
    assert!(historical::run(&mut f.book, f.actor, f.company, &changed, true, None).is_err());
    assert_eq!(snapshot(f.book.connection()).unwrap(), after);
}
#[test]
fn import_rejects_changed_mappings_duplicates_and_late_failures_atomically() {
    let mut f = Fixture::new();
    let first = source(&f, "paid", "10");
    let rows = vec![first.clone()];
    let preview = historical::run(&mut f.book, f.actor, f.company, &rows, true, None).unwrap();
    directory::save_record(
        &mut f.book,
        f.actor,
        f.company,
        Model::Customer,
        &json!({"display_name":"Revised customer"}),
        Some(&f.customer),
    )
    .unwrap();
    let before = snapshot(f.book.connection()).unwrap();
    assert!(
        historical::run(
            &mut f.book,
            f.actor,
            f.company,
            &rows,
            false,
            preview["review_digest"].as_str()
        )
        .is_err()
    );
    assert_eq!(snapshot(f.book.connection()).unwrap(), before);
    let mut second = source(&f, "paid", "11");
    second["source"]["payments"][0]["reference"] =
        first["source"]["payments"][0]["reference"].clone();
    assert!(
        historical::run(
            &mut f.book,
            f.actor,
            f.company,
            &[first.clone(), second],
            true,
            None
        )
        .is_err()
    );
    assert_eq!(snapshot(f.book.connection()).unwrap(), before);
    let second = source(&f, "paid", "10");
    assert!(
        historical::run(
            &mut f.book,
            f.actor,
            f.company,
            &[first, second],
            true,
            None
        )
        .is_err()
    );
    assert_eq!(snapshot(f.book.connection()).unwrap(), before);
}
#[test]
fn import_requires_exact_source_tax_totals_lines_and_company_scope() {
    let mut f = Fixture::new();
    let row = source(&f, "paid", "10");
    for pointer in [
        "/source/total",
        "/source/tax_amount",
        "/source/subtotal",
        "/source/amount_paid",
        "/source/lines/0/amount",
        "/source/lines/0/unit_price",
        "/source/lines/0/quantity",
    ] {
        let mut changed = row.clone();
        *changed.pointer_mut(pointer).unwrap() = json!("22");
        assert!(
            historical::run(&mut f.book, f.actor, f.company, &[changed], true, None).is_err(),
            "{pointer}"
        );
    }
    let other = services::create_company(&mut f.book, f.actor, "Other company", "USD", false)
        .unwrap()
        .id;
    assert!(historical::run(&mut f.book, f.actor, other, &[row], true, None).is_err());
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn cancellation_keeps_unposted_zero_draft_out_of_the_ledger() {
    let mut f = Fixture::new();
    let mut v = f.input();
    v["lines"][0]["rate"] = json!("0");
    let inv = save_invoice(&mut f.book, f.actor, f.company, &v).unwrap();
    let key = inv["id"].as_str().unwrap();
    let cancel = void_invoice(
        &mut f.book,
        f.actor,
        f.company,
        key,
        Some(1),
        &json!("2026-10-02"),
        &json!("Never issued"),
    )
    .unwrap();
    assert_eq!(cancel["current"]["state"], "VOID");
    assert_eq!(cancel["current"]["journal_id"], Value::Null);
    assert!(
        void_invoice(
            &mut f.book,
            f.actor,
            f.company,
            key,
            Some(1),
            &json!("2026-10-02"),
            &json!("Never issued")
        )
        .is_ok()
    );
    assert!(review_invoice(&mut f.book, f.actor, f.company, key, Some(2)).is_err());
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn original_nontaxable_credit_does_not_reclassify_tax_and_source_display_survives() {
    let mut f = Fixture::new();
    let mut row = source(&f, "paid", "410.00");
    row["invoice"]["tax_mode"] = json!("MANUAL");
    row["invoice"]["tax_rate"] = json!("7");
    row["invoice"]["lines"] = json!([
        {"line_kind":"NOTE","description":"Work history\nContinued","quantity":"0","rate":"0","taxable":false},
        {"description":"Programming","quantity":"5","rate":"100","taxable":false,"income_account_id":f.income.to_db()},
        {"line_kind":"DISCOUNT","description":"Credit for prior invoice tax","quantity":"1","rate":"40","taxable":false,"income_account_id":f.income.to_db()},
        {"line_kind":"DISCOUNT","description":"Preferred customer discount","quantity":"1","rate":"50","taxable":false,"income_account_id":f.income.to_db()}]);
    row["source"]["tax_rate"] = json!(7);
    row["source"]["subtotal"] = json!("460.00");
    row["source"]["gc_discount"] = json!("-50.00");
    row["source"]["lines"] = json!([
        {"item":"On-site support","description":"Work history\nContinued","quantity":0,"unit_price":0,"amount":0,"taxable":0},
        {"item":"Programming","description":"Programming","quantity":5,"unit_price":100,"amount":500,"taxable":0},
        {"item":"Credit","description":"Credit for prior invoice tax","quantity":1,"unit_price":-40,"amount":-40,"taxable":0},
        {"item":"Discount","description":"Preferred customer discount","quantity":1,"unit_price":-50,"amount":-50,"taxable":0}]);
    let rows = vec![row];
    let preview = historical::run(&mut f.book, f.actor, f.company, &rows, true, None).unwrap();
    let result = historical::run(
        &mut f.book,
        f.actor,
        f.company,
        &rows,
        false,
        preview["review_digest"].as_str(),
    )
    .unwrap();
    let id = result["invoices"][0]["invoice_id"].as_str().unwrap();
    f.book
        .read(|q| -> FbResult<()> {
            let inv = invoice(q, f.company, id)?;
            assert_eq!(inv["current"]["tax"], "0.00");
            assert_eq!(inv["current"]["total"], "410.00");
            assert_eq!(
                q.query_row(
                    "SELECT count(*) FROM ledger_journalline WHERE account_id=?1",
                    [f.tax.to_db()],
                    |r| r.get::<_, i64>(0)
                )?,
                0,
                "source credit must never adjust tax liability"
            );
            let view = historical::presentation(
                q,
                f.company,
                id,
                &inv["current"],
                inv["lines"].as_array().unwrap(),
            )?;
            assert_eq!(view["matches_revision"], true);
            assert_eq!(view["subtotal"], "460.00");
            assert_eq!(view["discount"], "-50.00");
            assert_eq!(view["payments"][0]["amount"], "410.00");
            let mut edited = inv["current"].clone();
            edited["customer_snapshot"]["display_name"] = json!("Different customer");
            assert_eq!(
                historical::presentation(
                    q,
                    f.company,
                    id,
                    &edited,
                    inv["lines"].as_array().unwrap()
                )?["matches_revision"],
                false
            );
            Ok(())
        })
        .unwrap();
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn import_state_rate_bounds_and_stale_role_account_fail_closed() {
    let mut f = Fixture::new();
    let row = source(&f, "paid", "10");
    for (pointer, value) in [
        ("/source/tax_rate", json!(7)),
        ("/source/status", json!("sent")),
        ("/source/status", json!("partial")),
        ("/source/notes", json!("Different memo")),
        ("/source/id", json!("not-a-uuid")),
    ] {
        let mut changed = row.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        assert!(
            historical::run(&mut f.book, f.actor, f.company, &[changed], true, None).is_err(),
            "{pointer}"
        );
    }
    let p = historical::run(&mut f.book, f.actor, f.company, std::slice::from_ref(&row), true, None).unwrap();
    let mut a = services::AccountInput::new("110", "Changed receivables");
    a.account = Some(f.ar);
    a.kind = Some("ASSET".into());
    services::save_account(&mut f.book, f.actor, f.company, a).unwrap();
    let before = snapshot(f.book.connection()).unwrap();
    assert!(
        historical::run(
            &mut f.book,
            f.actor,
            f.company,
            &[row],
            false,
            p["review_digest"].as_str()
        )
        .is_err()
    );
    assert_eq!(snapshot(f.book.connection()).unwrap(), before);
}

#[test]
fn genuine_populated_preimport_book_backup_upgrade_and_reopen_preserve_every_old_row() {
    let mut f = Fixture::new();
    let input = f.input();
    let inv = save_invoice(&mut f.book, f.actor, f.company, &input).unwrap();
    let id = inv["id"].as_str().unwrap();
    let review = review_invoice(&mut f.book, f.actor, f.company, id, Some(1)).unwrap();
    post_invoice(&mut f.book, f.actor, f.company, id, Some(1), Some(&review)).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old-0192.fbbook");
    let backup = dir.path().join("verified-0192.fbbook");
    let prior = fb_db::test_support::populated_copy(
        f.book.connection(),
        &path,
        &[13; 32],
        "0192_bank_rule_autopost_off",
        true,
    )
    .unwrap();
    let old = match Book::open_staged(&path, &[13; 32]).unwrap() {
        fb_db::Opened::Behind(old) => old,
        _ => panic!("must open the actual previous schema"),
    };
    old.snapshot(&backup, &[13; 32]).unwrap();
    let old_snapshot = fb_db::test_support::snapshot_file(&backup, &[13; 32]).unwrap();
    let mut upgraded = old.upgrade().unwrap();
    assert_eq!(snapshot(upgraded.connection()).unwrap(), prior.expected);
    for (table, rows) in old_snapshot {
        assert_eq!(
            snapshot(upgraded.connection()).unwrap()[&table],
            rows,
            "{table}"
        );
    }
    let row = source(&f, "paid", "25");
    let preview = historical::run(
        &mut upgraded,
        f.actor,
        f.company,
        std::slice::from_ref(&row),
        true,
        None,
    )
    .unwrap();
    let saved = historical::run(
        &mut upgraded,
        f.actor,
        f.company,
        &[row],
        false,
        preview["review_digest"].as_str(),
    )
    .unwrap();
    let after = snapshot(upgraded.connection()).unwrap();
    drop(upgraded);
    let mut reopened = Book::open(&path, &[13; 32]).unwrap();
    assert_eq!(snapshot(reopened.connection()).unwrap(), after);
    assert!(reopened.check().unwrap().is_empty());
    reopened
        .read(|q| -> FbResult<()> {
            let inv = invoice(
                q,
                f.company,
                saved["invoices"][0]["invoice_id"].as_str().unwrap(),
            )?;
            assert_eq!(inv["current"]["total"], "25.00");
            assert_eq!(inv["source_history"]["source"]["status"], "paid");
            Ok(())
        })
        .unwrap();
}

#[test]
fn sql_evidence_guards_reject_status_conflicts_and_open_refuses_tampered_history() {
    let mut f = Fixture::new();
    let row = source(&f, "paid", "10");
    let p = historical::run(&mut f.book, f.actor, f.company, std::slice::from_ref(&row), true, None).unwrap();
    let saved = historical::run(
        &mut f.book,
        f.actor,
        f.company,
        &[row],
        false,
        p["review_digest"].as_str(),
    )
    .unwrap();
    let key = saved["invoices"][0]["invoice_id"].as_str().unwrap();
    f.book.read(|q|->FbResult<()> {
        for status in ["sent","partial","draft","void","unknown"] {
            let valid:bool=q.query_row("SELECT fb_invoice_import_matches(json_set(h.data,'$.source.status',?1),r.data,(SELECT json_group_array(json(data)) FROM (SELECT data FROM sales_invoiceline WHERE revision_id=r.id ORDER BY position))) FROM sales_importinvoice h JOIN sales_invoicerevision r ON r.id=h.revision_id WHERE h.invoice_id=?2",params![status,key],|r|r.get(0))?;
            assert!(!valid,"raw SQL status {status} must fail independently of the digest check");
        }
        for field in ["issue_date","due_date"] {
            let valid:bool=q.query_row("SELECT fb_invoice_import_matches(json_set(h.data,?1,'2026-09-01'),r.data,(SELECT json_group_array(json(data)) FROM (SELECT data FROM sales_invoiceline WHERE revision_id=r.id ORDER BY position))) FROM sales_importinvoice h JOIN sales_invoicerevision r ON r.id=h.revision_id WHERE h.invoice_id=?2",params![format!("$.source.{field}"),key],|r|r.get(0))?;
            assert!(!valid,"source {field} must match native accounting dates");
        }
        Ok(())
    }).unwrap();
    let before = snapshot(f.book.connection()).unwrap();
    for sql in [
        "DELETE FROM sales_importseal",
        "UPDATE sales_importinvoice SET source_number='changed'",
        "DELETE FROM sales_importpayment",
    ] {
        assert!(f.book.write(|q| q.execute(sql, [])).is_err());
    }
    assert_eq!(snapshot(f.book.connection()).unwrap(), before);
    let path: String = f
        .book
        .connection()
        .query_row("PRAGMA database_list", [], |r| r.get(2))
        .unwrap();
    let guard: String = f
        .book
        .connection()
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='sales_importinvoice_update'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    f.book.connection().execute_batch("DROP TRIGGER sales_importinvoice_update; UPDATE sales_importinvoice SET data=json_set(data,'$.source.payments[0].method','cash');").unwrap();
    f.book.connection().execute_batch(&guard).unwrap();
    assert!(!f.book.check().unwrap().is_empty());
    drop(f.book);
    assert!(matches!(
        Book::open(std::path::Path::new(&path), &[13; 32]),
        Err(fb_db::DbError::Integrity(_))
    ));
}

/// Populated identity matrix for the 0195 import tables (registered in
/// fb-core/tests/identity_permanence.rs): the recorded source of an imported invoice, its payment
/// and its seal cannot move to another key, by UPDATE or UPDATE OR REPLACE.
#[test]
fn imported_invoice_payment_and_seal_identities_are_permanent() {
    let mut f = Fixture::new();
    let rows = vec![source(&f, "paid", "10"), source(&f, "sent", "11")];
    let preview = historical::run(&mut f.book, f.actor, f.company, &rows, true, None).unwrap();
    historical::run(&mut f.book, f.actor, f.company, &rows, false, preview["review_digest"].as_str()).unwrap();
    let tables = ["sales_importinvoice", "sales_importpayment", "sales_importseal"];
    let accepted = fb_db::test_support::primary_key_changes_accepted(f.book.connection(), &tables);
    assert!(accepted.is_empty(), "accepted primary-key changes: {accepted:#?}");
    assert!(f.book.check().unwrap().is_empty());
}
