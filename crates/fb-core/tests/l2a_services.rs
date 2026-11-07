//! Directory/vendor and accounting-setup service tests translated from the Python answer key.
mod common;
use common::*;
use fb_core::{directory as d, directory::Model, documents as doc, salestax as tax, services as l, services::AccountInput};
use serde_json::json;
#[test]
fn contact_roles_duplicates_whitespace_lengths_and_mass_assignment() {
    let mut f = Fx::new("contacts");
    let c = f.company("C");
    for model in [Model::Customer, Model::Vendor] {
        let a = d::save_record(
            &mut f.book,
            f.user,
            c,
            model,
            &json!({"display_name":" Same ","notes":" notes "}),
            None,
        )
        .unwrap();
        let b = d::save_record(&mut f.book, f.user, c, model, &json!({"display_name":"Same"}), None).unwrap();
        assert_ne!(a["id"], b["id"]);
        assert_eq!(a["notes"], "notes");
        for values in [
            json!({"display_name":" "}),
            json!({"display_name":"x".repeat(121)}),
            json!({"display_name":"x","notes":"x".repeat(4001)}),
            json!({"display_name":"x","company_id":c.to_db()}),
            json!({"active":false}),
        ] {
            assert!(d::save_record(&mut f.book, f.user, c, model, &values, None).is_err());
        }
    }
    assert!(
        d::save_record(
            &mut f.book,
            f.user,
            c,
            Model::Vendor,
            &json!({"display_name":"Vendor","tax_status":"EXEMPT"}),
            None
        )
        .is_err()
    );
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal"), 0);
}
#[test]
fn archive_noop_unauthorized_and_foreign_record_tampering() {
    let mut f = Fx::new("access");
    let c = f.company("C");
    let o = f.company("O");
    let outsider = fb_core::users::create_user(&mut f.book, "outsider", false).unwrap();
    let obj = d::save_record(&mut f.book, f.user, c, Model::Customer, &json!({"display_name":"Buyer"}), None).unwrap();
    let id = obj["id"].as_str().unwrap();
    let before = f.count("SELECT count(*) FROM ledger_auditevent");
    d::set_active(&mut f.book, f.user, c, Model::Customer, id, true).unwrap();
    assert_eq!(before, f.count("SELECT count(*) FROM ledger_auditevent"));
    assert!(matches!(
        d::set_active(&mut f.book, outsider, c, Model::Customer, id, false),
        Err(fb_core::FbError::PermissionDenied(_))
    ));
    assert!(matches!(
        d::save_record(&mut f.book, f.user, o, Model::Customer, &json!({"notes":"attack"}), Some(id)),
        Err(fb_core::FbError::DoesNotExist(_))
    ));
}
#[test]
fn vendor_defaults_omission_clearing_eligibility_and_stale_choices() {
    let mut f = Fx::new("vendor");
    let (c, asset, _) = f.chart();
    let exp = f.account(c, "5", "Expense", "EXPENSE");
    let income = f.account(c, "4", "Income", "INCOME");
    let foreign_company = f.company("O");
    let foreign = f.account(foreign_company, "5", "Other", "EXPENSE");
    let obj = d::save_record(
        &mut f.book,
        f.user,
        c,
        Model::Vendor,
        &json!({"display_name":"Vendor","default_expense_account":asset.to_db(),"default_terms":"30"}),
        None,
    )
    .unwrap();
    let id = obj["id"].as_str().unwrap();
    let kept = d::save_record(&mut f.book, f.user, c, Model::Vendor, &json!({"notes":"edit"}), Some(id)).unwrap();
    assert_eq!(kept["default_expense_account"], asset.to_db());
    assert_eq!(kept["default_terms"], "30");
    for a in [income, foreign] {
        assert!(
            d::save_record(
                &mut f.book,
                f.user,
                c,
                Model::Vendor,
                &json!({"default_expense_account":a.to_db()}),
                Some(id)
            )
            .is_err()
        );
    }
    for term in [json!(30), json!(true), json!("NET30"), json!("45")] {
        assert!(d::save_record(&mut f.book, f.user, c, Model::Vendor, &json!({"default_terms":term}), Some(id)).is_err());
    }
    d::save_record(
        &mut f.book,
        f.user,
        c,
        Model::Vendor,
        &json!({"default_expense_account":exp.to_db()}),
        Some(id),
    )
    .unwrap();
    l::save_account(
        &mut f.book,
        f.user,
        c,
        AccountInput::new("5", "Expense").kind("EXPENSE").edit(exp).active(false),
    )
    .unwrap();
    assert!(d::save_record(&mut f.book, f.user, c, Model::Vendor, &json!({"notes":"unrelated edit"}), Some(id)).is_err());
    d::set_active(&mut f.book, f.user, c, Model::Vendor, id, false).unwrap();
    assert!(d::set_active(&mut f.book, f.user, c, Model::Vendor, id, true).is_err());
    let cleared = d::save_record(
        &mut f.book,
        f.user,
        c,
        Model::Vendor,
        &json!({"default_expense_account":null,"default_terms":""}),
        Some(id),
    )
    .unwrap();
    assert!(cleared["default_terms"].is_null());
    d::set_active(&mut f.book, f.user, c, Model::Vendor, id, true).unwrap();
}
#[test]
fn audit_failure_rolls_back_directory_and_tax_quick_add() {
    let mut f = Fx::new("rollback");
    let c = f.company("C");
    f.book.connection().execute_batch("CREATE TRIGGER fail_directory BEFORE INSERT ON ledger_auditevent WHEN NEW.action<>'company.created' BEGIN SELECT RAISE(ABORT,'injected'); END").unwrap();
    assert!(d::save_record(&mut f.book, f.user, c, Model::Customer, &json!({"display_name":"Buyer"}), None).is_err());
    assert_eq!(f.count("SELECT count(*) FROM directory_customer"), 0);
    assert!(
        tax::save_rate(
            &mut f.book,
            f.user,
            c,
            &json!({"name":"Tax","description":"Tax","rate":"3","new_agency_name":"Agency"})
        )
        .is_err()
    );
    assert_eq!(f.count("SELECT count(*) FROM directory_vendor"), 0);
    assert_eq!(f.count("SELECT count(*) FROM salestax_taxitem"), 0);
}
#[test]
fn setup_revision_roles_customer_deposit_keep_and_additional_union() {
    let mut f = Fx::new("setup");
    let (c, a, b) = f.chart();
    let payable = f.account(c, "2", "Payable", "LIABILITY");
    let deposit = f.account(c, "21", "Prepay", "LIABILITY");
    let bank = f.account(c, "11", "Bank", "ASSET");
    let input = json!({"receivables":a.to_db(),"payables":payable.to_db(),"customer_deposits":deposit.to_db(),"banks":[bank.to_db()]});
    doc::save_setup(&mut f.book, f.user, c, &input, Some(0)).unwrap();
    assert!(doc::save_setup(&mut f.book, f.user, c, &input, Some(0)).is_err());
    for invalid in [
        json!({"receivables":a.to_db(),"undeposited":a.to_db()}),
        json!({"receivables":a.to_db(),"banks":[a.to_db()]}),
        json!({"payables":a.to_db()}),
        json!({"banks":[bank.to_db(),bank.to_db()]}),
    ] {
        assert!(doc::save_setup(&mut f.book, f.user, c, &invalid, Some(1)).is_err());
    }
    let keep = doc::save_setup(
        &mut f.book,
        f.user,
        c,
        &json!({"payables":payable.to_db(),"banks":[bank.to_db()]}),
        Some(1),
    )
    .unwrap();
    assert_eq!(keep["customer_deposits"], deposit.to_db());
    assert!(keep["receivables"].is_null());
    f.book
        .write(|tx| {
            let config = doc::lock_configuration(tx, f.user, c, &["payables"], Some(bank), &[b])?;
            assert_eq!(config["payables"], payable.to_db());
            Ok::<_, fb_core::FbError>(())
        })
        .unwrap();
    let cleared = doc::save_setup(&mut f.book, f.user, c, &json!({"customer_deposits":null}), Some(2)).unwrap();
    assert!(cleared["customer_deposits"].is_null());
}
#[test]
fn tax_inactive_default_group_and_revision_rules() {
    let mut f = Fx::new("tax-rules");
    let c = f.company("C");
    let a = tax::save_rate(
        &mut f.book,
        f.user,
        c,
        &json!({"name":"A","description":"A","rate":"3","new_agency_name":"A"}),
    )
    .unwrap();
    let b = tax::save_rate(
        &mut f.book,
        f.user,
        c,
        &json!({"name":"B","description":"B","rate":"4","new_agency_name":"B"}),
    )
    .unwrap();
    let group = tax::save_group(
        &mut f.book,
        f.user,
        c,
        &json!({"name":"G","description":"G","member_ids":[a["id"],b["id"]]}),
    )
    .unwrap();
    let g = group["id"].as_str().unwrap();
    let aid = a["id"].as_str().unwrap();
    assert!(tax::set_item_active(&mut f.book, f.user, c, aid, false, Some(1)).is_err());
    tax::set_item_active(&mut f.book, f.user, c, g, false, Some(1)).unwrap();
    tax::set_item_active(&mut f.book, f.user, c, aid, false, Some(1)).unwrap();
    assert!(tax::set_item_active(&mut f.book, f.user, c, g, true, Some(2)).is_err());
    assert!(
        tax::save_rate(
            &mut f.book,
            f.user,
            c,
            &json!({"name":"a","description":"duplicate","rate":"2","new_agency_name":"unused"})
        )
        .is_err()
    );
    assert!(
        tax::save_group(
            &mut f.book,
            f.user,
            c,
            &json!({"name":"Too small","description":"x","member_ids":[b["id"]]})
        )
        .is_err()
    );
    tax::set_item_active(&mut f.book, f.user, c, aid, true, Some(2)).unwrap();
    tax::set_item_active(&mut f.book, f.user, c, g, true, Some(2)).unwrap();
    let settings = json!({"charge_tax":true,"default_item_id":g,"owed_basis":"CASH","filing_frequency":"QUARTERLY","expected_revision":0});
    tax::save_settings(&mut f.book, f.user, c, &settings).unwrap();
    assert!(tax::set_item_active(&mut f.book, f.user, c, g, false, Some(3)).is_err());
}
