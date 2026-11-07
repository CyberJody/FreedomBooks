//! D2 raw-SQL attacks: every new L2a RAISE message must actually refuse a write.
mod common;
use common::*;
use fb_core::{
    directory::{self, Model},
    documents, salestax,
    services::{self, AccountInput},
};
use serde_json::json;
use std::collections::BTreeSet;

// Attacker-chosen ids for raw inserts (0130 requires canonical UUIDs and timestamps).
const ATTACK_GROUP_MEMBER: &str = "00000000-0000-7000-8000-0000000a0001"; // salestax_taxgroupmember row
const ATTACK_IDENTITY_EUR: &str = "00000000-0000-7000-8000-0000000a0002"; // documentidentity, wrong currency
const ATTACK_IDENTITY_GAP: &str = "00000000-0000-7000-8000-0000000a0003"; // documentidentity, wrong number
const ATTACK_REQUEST_EUR: &str = "00000000-0000-7000-8000-0000000a0004"; // its request_key
const ATTACK_REQUEST_GAP: &str = "00000000-0000-7000-8000-0000000a0005"; // its request_key
const ATTACK_SNAPSHOT: &str = "00000000-0000-7000-8000-0000000a0006"; // documentaccount row
const ATTACK_AT: &str = "2026-01-01T00:00:00Z";
#[test]
fn every_l2a_guard_message_has_a_real_refusing_attack() {
    let mut f = Fx::new("l2-break");
    let c = f.company("C");
    let o = f.company("O");
    let a = f.account(c, "1", "Asset", "ASSET");
    let v = directory::save_record(&mut f.book, f.user, c, Model::Vendor, &json!({"display_name":"Agency"}), None).unwrap();
    let foreign = directory::save_record(&mut f.book, f.user, o, Model::Vendor, &json!({"display_name":"Foreign"}), None).unwrap();
    let rate = salestax::save_rate(
        &mut f.book,
        f.user,
        c,
        &json!({"name":"Rate","description":"Rate","rate":"3","agency_id":v["id"]}),
    )
    .unwrap();
    let other = salestax::save_rate(
        &mut f.book,
        f.user,
        o,
        &json!({"name":"Other","description":"Other","rate":"4","agency_id":foreign["id"]}),
    )
    .unwrap();
    let rate2 = salestax::save_rate(
        &mut f.book,
        f.user,
        c,
        &json!({"name":"State","description":"State","rate":"4","agency_id":v["id"]}),
    )
    .unwrap();
    let group = salestax::save_group(
        &mut f.book,
        f.user,
        c,
        &json!({"name":"Group","description":"Group","member_ids":[rate["id"],rate2["id"]]}),
    )
    .unwrap();
    salestax::save_settings(
        &mut f.book,
        f.user,
        c,
        &json!({"charge_tax":true,"default_item_id":null,"owed_basis":"ACCRUAL","filing_frequency":"MONTHLY","expected_revision":0}),
    )
    .unwrap();
    let identity = f
        .book
        .write(|tx| documents::create_identity(tx, f.user, c, "EST", "00000000-0000-0000-0000-000000000001"))
        .unwrap();
    let id = identity["id"].as_str().unwrap();
    f.book
        .write(|tx| documents::snapshot_source_accounts(tx, f.user, c, id, &json!({"asset":a.to_db()})))
        .unwrap();
    let t = rate["id"].as_str().unwrap();
    let g = group["id"].as_str().unwrap();
    let ot = other["id"].as_str().unwrap();
    let ov = foreign["id"].as_str().unwrap();
    let attacks = vec![
        format!("DELETE FROM salestax_taxitem WHERE id='{t}'"),
        format!("UPDATE salestax_taxitem SET company_id='{o}' WHERE id='{t}'"),
        format!("UPDATE salestax_taxitem SET kind='GROUP' WHERE id='{t}'"),
        format!("UPDATE salestax_taxitem SET agency_id='{ov}' WHERE id='{t}'"),
        format!("INSERT INTO salestax_taxgroupmember VALUES('{ATTACK_GROUP_MEMBER}','{g}','{ot}',3)"),
        format!("UPDATE salestax_taxgroupmember SET member_id='{ot}' WHERE group_id='{g}' AND position=1"),
        format!("UPDATE salestax_salestaxsettings SET default_item_id='{ot}' WHERE company_id='{c}'"),
        format!("UPDATE salestax_salestaxsettings SET company_id='{o}' WHERE company_id='{c}'"),
        format!("DELETE FROM salestax_salestaxsettings WHERE company_id='{c}'"),
        format!("DELETE FROM documents_documentsequence WHERE company_id='{c}'"),
        format!("UPDATE documents_documentsequence SET last_number=0 WHERE company_id='{c}'"),
        format!("UPDATE documents_documentsequence SET kind='INV' WHERE company_id='{c}'"),
        format!("UPDATE documents_documentidentity SET number='EST-999999' WHERE id='{id}'"),
        format!("DELETE FROM documents_documentidentity WHERE id='{id}'"),
        format!("INSERT INTO documents_documentidentity VALUES('{ATTACK_IDENTITY_EUR}','{c}','EST',2,'EST-000002','{ATTACK_REQUEST_EUR}','EUR','{ATTACK_AT}')"),
        format!("INSERT INTO documents_documentidentity VALUES('{ATTACK_IDENTITY_GAP}','{c}','EST',2,'EST-000099','{ATTACK_REQUEST_GAP}','USD','{ATTACK_AT}')"),
        format!("UPDATE documents_documentaccount SET account_name='Forged' WHERE source_id='{id}'"),
        format!("DELETE FROM documents_documentaccount WHERE source_id='{id}'"),
        format!("INSERT INTO documents_documentaccount VALUES('{ATTACK_SNAPSHOT}','{id}','other','{a}','1','Forged','ASSET')"),
        format!("UPDATE ledger_account SET kind='INCOME',detail_type='INCOME' WHERE id='{a}'"),
        format!("UPDATE ledger_company SET currency='EUR' WHERE id='{c}'"),
        // REPLACE must not bypass deletes or immutable snapshots.
        format!("INSERT OR REPLACE INTO documents_documentidentity SELECT * FROM documents_documentidentity WHERE id='{id}'"),
        format!("INSERT OR REPLACE INTO salestax_taxitem SELECT * FROM salestax_taxitem WHERE id='{t}'"),
    ];
    let mut seen = BTreeSet::new();
    for sql in attacks {
        let message = legacy_l2a_attack_message(&f.book, &sql).unwrap_or_else(|| panic!("accepted: {sql}"));
        assert!(!message.starts_with("fb_valid"), "refused by fb_valid before its intended guard: {sql}");
        seen.insert(message);
    }
    let expected = fb_db::MIGRATIONS[3]
        .1
        .split("RAISE(ABORT, '")
        .skip(1)
        .map(|x| x.split("')").next().unwrap().to_string())
        .collect::<BTreeSet<_>>();
    let missing = expected.difference(&seen).collect::<Vec<_>>();
    assert!(missing.is_empty(), "uncovered: {missing:?}; seen {seen:?}");
    assert_eq!(expected.len(), 15);
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn defaults_partial_edits_archive_and_stale_reactivation() {
    let mut f = Fx::new("l2-directory");
    let c = f.company("C");
    let a = f.account(c, "4", "Sales", "INCOME");
    let customer = directory::save_record(
        &mut f.book,
        f.user,
        c,
        Model::Customer,
        &json!({"display_name":"  Buyer ","tax_status":"EXEMPT","exemption_number":" ABC "}),
        None,
    )
    .unwrap();
    assert_eq!(customer["display_name"], "Buyer");
    assert_eq!(customer["exemption_number"], "ABC");
    let item = directory::save_record(
        &mut f.book,
        f.user,
        c,
        Model::Item,
        &json!({"code":"svc","kind":"SERVICE","sale_rate":"12.345","income_account":a.to_db()}),
        None,
    )
    .unwrap();
    assert_eq!(item["sale_rate"], "12.35");
    assert_eq!(item["taxable"], false);
    let id = item["id"].as_str().unwrap();
    let item = directory::save_record(&mut f.book, f.user, c, Model::Item, &json!({"kind":"NONSTOCK"}), Some(id)).unwrap();
    assert_eq!(item["taxable"], false);
    assert!(!directory::line_taxable_default(Some(&customer), Some(&item)));
    assert_eq!(
        directory::save_record(
            &mut f.book,
            f.user,
            c,
            Model::Item,
            &json!({"code":"SVC","kind":"SERVICE","sale_rate":"1"}),
            None
        )
        .unwrap_err()
        .message(),
        "An item with this code / name already exists in this company (including archived items)."
    );
    services::save_account(
        &mut f.book,
        f.user,
        c,
        AccountInput::new("4", "Sales").kind("INCOME").edit(a).active(false),
    )
    .unwrap();
    directory::set_active(&mut f.book, f.user, c, Model::Item, id, false).unwrap();
    assert_eq!(directory::set_active(&mut f.book, f.user, c, Model::Item, id, true).unwrap_err().message(), "Choose an active, postable income account in this company, or clear this default.");
    directory::save_record(&mut f.book, f.user, c, Model::Item, &json!({"income_account":null}), Some(id)).unwrap();
    directory::set_active(&mut f.book, f.user, c, Model::Item, id, true).unwrap();
}
#[test]
fn numbering_retry_rollback_kinds_and_currency_freeze() {
    let mut f = Fx::new("l2-numbering");
    let c = f.company("C");
    for (i, k) in documents::KINDS.iter().enumerate() {
        let key = format!("00000000-0000-0000-0000-{i:012}");
        let a = f.book.write(|tx| documents::create_identity(tx, f.user, c, k, &key)).unwrap();
        let b = f.book.write(|tx| documents::create_identity(tx, f.user, c, k, &key)).unwrap();
        assert_eq!(a, b);
        assert_eq!(a["number"], format!("{k}-000001"));
    }
    let result: fb_core::FbResult<()> = f.book.write(|tx| {
        documents::create_identity(tx, f.user, c, "INV", "00000000-0000-0000-0001-000000000001")?;
        Err(fb_core::FbError::validation("rollback"))
    });
    assert_eq!(result.unwrap_err().message(), "rollback");
    let a = f
        .book
        .write(|tx| documents::create_identity(tx, f.user, c, "INV", "00000000-0000-0000-0001-000000000002"))
        .unwrap();
    assert_eq!(a["number"], "INV-000002");
    assert_eq!(err_text(services::edit_company(&mut f.book, f.user, c, "C", "EUR")), "Currency cannot change after journals or commercial document identities have been created.");
}
#[test]
fn group_rounding_bulk_and_snapshot_survive_edits() {
    let mut f = Fx::new("l2-tax");
    let c = f.company("C");
    let cust = directory::save_record(
        &mut f.book,
        f.user,
        c,
        Model::Customer,
        &json!({"display_name":"Buyer","tax_status":"EXEMPT"}),
        None,
    )
    .unwrap();
    let good = directory::save_record(
        &mut f.book,
        f.user,
        c,
        Model::Item,
        &json!({"code":"Goods","kind":"NONSTOCK","sale_rate":"1","taxable":false}),
        None,
    )
    .unwrap();
    let svc = directory::save_record(
        &mut f.book,
        f.user,
        c,
        Model::Item,
        &json!({"code":"Svc","kind":"SERVICE","sale_rate":"1"}),
        None,
    )
    .unwrap();
    let a = salestax::save_rate(
        &mut f.book,
        f.user,
        c,
        &json!({"name":"A","description":"A","rate":"3","new_agency_name":"Agency A"}),
    )
    .unwrap();
    let b = salestax::save_rate(
        &mut f.book,
        f.user,
        c,
        &json!({"name":"B","description":"B","rate":"4","new_agency_name":"Agency B"}),
    )
    .unwrap();
    let g = salestax::save_group(
        &mut f.book,
        f.user,
        c,
        &json!({"name":"G","description":"Group tax","member_ids":[a["id"],b["id"]]}),
    )
    .unwrap();
    let (_, offer) = salestax::save_settings(
        &mut f.book,
        f.user,
        c,
        &json!({"charge_tax":true,"default_item_id":g["id"],"owed_basis":"ACCRUAL","filing_frequency":"MONTHLY","expected_revision":0}),
    )
    .unwrap();
    assert!(offer);
    let resolved = salestax::document_tax(
        f.book.connection(),
        c,
        &json!({"tax_mode":"ITEM","tax_item_id":g["id"]}),
        None,
        true,
    )
    .unwrap();
    let sum = fb_core::calculations::totals(
        &[json!({"quantity":"1","rate":"112.50","taxable":true})],
        "GROUP",
        None,
        &json!(""),
        Some(&resolved["tax_snapshot"]),
        false,
    )
    .unwrap();
    assert_eq!(sum["tax"], "7.88");
    assert_eq!(sum["tax_snapshot"]["members"][0]["tax"], "3.38");
    salestax::save_rate(
        &mut f.book,
        f.user,
        c,
        &json!({"item_id":a["id"],"expected_revision":1,"name":"A","description":"A","rate":"5","agency_id":a["agency_id"]}),
    )
    .unwrap();
    assert_eq!(sum["tax"], "7.88");
    assert!(
        !salestax::same_snapshot(
            f.book.connection(),
            c,
            &sum["tax_snapshot"],
            &salestax::item(f.book.connection(), c, g["id"].as_str().unwrap()).unwrap()
        )
        .unwrap()
    );
    let preview = salestax::bulk_preview(f.book.connection(), c).unwrap();
    salestax::finish_bulk_step(&mut f.book, f.user, c, true, true, preview["digest"].as_str().unwrap()).unwrap();
    assert_eq!(
        directory::get(f.book.connection(), c, Model::Customer, cust["id"].as_str().unwrap()).unwrap()["tax_status"],
        "TAXABLE"
    );
    assert_eq!(
        directory::get(f.book.connection(), c, Model::Item, good["id"].as_str().unwrap()).unwrap()["taxable"],
        true
    );
    assert_eq!(
        directory::get(f.book.connection(), c, Model::Item, svc["id"].as_str().unwrap()).unwrap()["taxable"],
        false
    );
}
