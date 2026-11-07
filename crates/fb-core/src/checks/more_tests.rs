use super::guards::snapshot;
use super::*;
use crate::transfers;

#[test]
fn archived_vendor_item_retention_refresh_and_reactivation() {
    let mut f = Fx::new();
    let item=directory::save_record(&mut f.book,f.actor,f.co,Model::Item,&json!({"code":"Part","kind":"NONSTOCK","sale_rate":"1","purchase_cost":"2","expense_account":f.expense.to_db()}),None).unwrap();
    let mut p = f.payload();
    p["lines"] = json!([{"tab":"ITEM","item_id":item["id"],"account_id":f.expense.to_db(),"quantity":"2","rate":"3"}]);
    let b = f.save(&p);
    directory::save_record(
        &mut f.book,
        f.actor,
        f.co,
        Model::Vendor,
        &json!({"display_name":"Changed directory"}),
        Some(&f.vendor),
    )
    .unwrap();
    directory::set_active(&mut f.book, f.actor, f.co, Model::Vendor, &f.vendor, false).unwrap();
    directory::save_record(
        &mut f.book,
        f.actor,
        f.co,
        Model::Item,
        &json!({"description":"Changed item"}),
        Some(s(&item, "id")),
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
    p["check_id"] = b["id"].clone();
    p["expected_revision"] = json!(1);
    assert!(save_check(&mut f.book, f.actor, f.co, &p).is_err());
    p["keep_archived"] = json!(true);
    p["lines"][0]["keep_archived"] = json!(true);
    let b2 = f.save(&p);
    assert_eq!(
        b2["current"]["payee_snapshot"],
        b["current"]["payee_snapshot"]
    );
    assert_eq!(
        b2["current"]["lines"][0]["item_snapshot"],
        b["current"]["lines"][0]["item_snapshot"]
    );
    assert!(review_check(&mut f.book, f.actor, f.co, s(&b, "id"), Some(2)).is_err());
    p["expected_revision"] = json!(2);
    p["refresh_payee"] = json!(true);
    assert!(save_check(&mut f.book, f.actor, f.co, &p).is_err());
    directory::set_active(&mut f.book, f.actor, f.co, Model::Vendor, &f.vendor, true).unwrap();
    directory::set_active(
        &mut f.book,
        f.actor,
        f.co,
        Model::Item,
        s(&item, "id"),
        true,
    )
    .unwrap();
    p["lines"][0]["refresh_item"] = json!(true);
    let b = f.save(&p);
    assert_eq!(b["current"]["reason"], "Explicit snapshot refresh");
    f.post(&b);
}
#[test]
fn frozen_controls_and_bank_destination_distinction() {
    let mut f = Fx::new();
    let (actor, co, bank2) = (f.actor, f.co, f.bank2);
    f.book
        .write(|tx| -> FbResult<()> {
            let i =
                documents::create_identity(tx, actor, co, "EST", &fb_ids::new_v7().to_string())?;
            documents::snapshot_source_accounts(
                tx,
                actor,
                co,
                s(&i, "id"),
                &json!({"undeposited":bank2.to_db()}),
            )?;
            Ok(())
        })
        .unwrap();
    let mut p = f.payload();
    p["lines"][0]["account_id"] = json!(bank2.to_db());
    assert!(save_check(&mut f.book, actor, co, &p).is_err());
    let tp = f.transfer();
    assert!(transfers::review_transfer(&mut f.book, actor, co, &tp).is_err());
    let mut g = Fx::new();
    let (actor, co, bank2) = (g.actor, g.co, g.bank2);
    g.book
        .write(|tx| -> FbResult<()> {
            let i =
                documents::create_identity(tx, actor, co, "RCP", &fb_ids::new_v7().to_string())?;
            documents::snapshot_source_accounts(
                tx,
                actor,
                co,
                s(&i, "id"),
                &json!({"destination":bank2.to_db()}),
            )?;
            Ok(())
        })
        .unwrap();
    let mut p = g.payload();
    p["lines"][0]["account_id"] = json!(bank2.to_db());
    let b = g.save(&p);
    g.post(&b);
    let tp = g.transfer();
    assert!(transfers::review_transfer(&mut g.book, actor, co, &tp).is_ok());
}
#[test]
fn company_permissions_cross_company_targets_and_no_generic_source_bypass() {
    let mut f = Fx::new();
    let outsider = users::create_user(&mut f.book, "outsider", false).unwrap();
    let co2 = services::create_company(&mut f.book, outsider, "Other", "USD", false)
        .unwrap()
        .id;
    let p = f.payload();
    assert_eq!(
        save_check(&mut f.book, outsider, f.co, &p)
            .unwrap_err()
            .kind(),
        "PermissionDenied"
    );
    let b = f.save(&p);
    assert_eq!(
        get_check(&mut f.book, outsider, co2, s(&b, "id"))
            .unwrap_err()
            .kind(),
        "DoesNotExist"
    );
    let b = f.post(&b);
    let j = crate::JournalId::parse(s(&b["current"], "journal_id")).unwrap();
    assert!(
        services::reverse_journal(
            &mut f.book,
            f.actor,
            f.co,
            j,
            "2026-10-24",
            "Wrong boundary"
        )
        .is_err()
    );
    let tp = f.transfer();
    assert_eq!(
        transfers::review_transfer(&mut f.book, outsider, f.co, &tp)
            .unwrap_err()
            .kind(),
        "PermissionDenied"
    );
    let r = transfers::review_transfer(&mut f.book, f.actor, f.co, &tp).unwrap();
    let t = transfers::record_transfer(&mut f.book, f.actor, f.co, &tp, &r).unwrap();
    assert_eq!(
        transfers::void_transfer(
            &mut f.book,
            outsider,
            co2,
            s(&t, "id"),
            Some(1),
            &json!("2026-10-24"),
            &json!("Wrong company")
        )
        .unwrap_err()
        .kind(),
        "DoesNotExist"
    );
}
#[test]
fn duplicate_commands_serialize_across_independent_connections() {
    for transfer in [false, true] {
        let mut f = Fx::new();
        let path = std::path::PathBuf::from(f.book.connection().path().unwrap());
        let other = Book::open(&path, &[42; 32]).unwrap();
        let (actor, co) = (f.actor, f.co);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        if transfer {
            let p = f.transfer();
            let r = transfers::review_transfer(&mut f.book, actor, co, &p).unwrap();
            let p2 = p.clone();
            let r2 = r.clone();
            let gate = barrier.clone();
            let child = std::thread::spawn(move || {
                let mut book = other;
                gate.wait();
                transfers::record_transfer(&mut book, actor, co, &p2, &r2).unwrap()
            });
            barrier.wait();
            let first = transfers::record_transfer(&mut f.book, actor, co, &p, &r).unwrap();
            assert_eq!(first["id"], child.join().unwrap()["id"]);
        } else {
            let p = f.payload();
            let b = f.save(&p);
            let r = f.review(&b);
            let r2 = r.clone();
            let cid = s(&b, "id").to_owned();
            let cid2 = cid.clone();
            let gate = barrier.clone();
            let child = std::thread::spawn(move || {
                let mut book = other;
                gate.wait();
                record_check(&mut book, actor, co, &cid2, Some(1), &r2).unwrap()
            });
            barrier.wait();
            let first = record_check(&mut f.book, actor, co, &cid, Some(1), &r).unwrap();
            assert_eq!(first["id"], child.join().unwrap()["id"]);
        }
        assert_eq!(f.counts()["ledger_journal"], 1);
        assert!(f.book.check().unwrap().is_empty());
    }
}
#[test]
fn numbering_and_transfer_void_late_audits_rollback() {
    let mut f = Fx::new();
    let mut p = f.payload();
    p["check_number"] = json!("");
    p["print_later"] = json!(true);
    let b = f.save(&p);
    let b = f.post(&b);
    f.book.connection().execute_batch("CREATE TEMP TRIGGER late_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='check.numbered' BEGIN SELECT RAISE(ABORT,'late audit'); END").unwrap();
    let before = snapshot(f.book.connection());
    assert!(
        assign_number(
            &mut f.book,
            f.actor,
            f.co,
            s(&b, "id"),
            Some(2),
            &json!("200")
        )
        .is_err()
    );
    assert_eq!(before, snapshot(f.book.connection()));
    f.book
        .connection()
        .execute_batch("DROP TRIGGER late_audit")
        .unwrap();
    let p = f.transfer();
    let r = transfers::review_transfer(&mut f.book, f.actor, f.co, &p).unwrap();
    let t = transfers::record_transfer(&mut f.book, f.actor, f.co, &p, &r).unwrap();
    f.book.connection().execute_batch("CREATE TEMP TRIGGER late_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='transfer.voided' BEGIN SELECT RAISE(ABORT,'late audit'); END").unwrap();
    let before = snapshot(f.book.connection());
    assert!(
        transfers::void_transfer(
            &mut f.book,
            f.actor,
            f.co,
            s(&t, "id"),
            Some(1),
            &json!("2026-10-24"),
            &json!("Fix")
        )
        .is_err()
    );
    assert_eq!(before, snapshot(f.book.connection()));
    f.book
        .connection()
        .execute_batch("DROP TRIGGER late_audit")
        .unwrap();
}
#[test]
fn fifty_lines_money_limit_and_display_helpers() {
    let mut f = Fx::new();
    let mut p = f.payload();
    p["lines"] = json!(vec![
        json!({"tab":"EXPENSE","account_id":f.expense.to_db(),"rate":"1"});
        50
    ]);
    let b = f.save(&p);
    assert_eq!(b["current"]["tab_totals"]["EXPENSE"], "50.00");
    assert_eq!(b["current"]["number_display"], "101");
    f.post(&b);
    p["request_key"] = json!(fb_ids::new_v7().to_string());
    p["check_number"] = json!("102");
    let line = p["lines"][0].clone();
    p["lines"].as_array_mut().unwrap().push(line);
    assert!(save_check(&mut f.book, f.actor, f.co, &p).is_err());
    p["lines"] =
        json!([{"tab":"EXPENSE","account_id":f.expense.to_db(),"rate":"99999999999999.99"}]);
    let b = f.save(&p);
    assert_eq!(b["current"]["total"], "99999999999999.99");
    assert_eq!(current_checks(&mut f.book, f.actor, f.co).unwrap().len(), 2);
}
