//! Python edge behaviours retained rather than redesigned.
mod common;
use common::*;
use fb_core::{
    directory::{self, Model},
    documents as d, salestax as t,
};
use serde_json::{Value, json};
#[test]
fn keep_and_exact_retry_digest_do_not_resolve_live_tax() {
    let f = Fx::new("keep");
    let c = fb_core::CompanyId::new();
    let previous =
        json!({"tax_mode":"MANUAL","tax_rate":"3.0000","tax_label":"Old","tax_snapshot":{},"tax_item_id":null,"unrelated":"excluded"});
    let kept = t::document_tax(f.book.connection(), c, &json!({"tax_mode":"KEEP"}), Some(&previous), true).unwrap();
    assert!(kept["tax_snapshot"].is_null());
    assert!(kept.get("unrelated").is_none());
    let id = fb_ids::new_v7().to_string();
    let a = json!({"tax_mode":"ITEM","tax_item_id":id,"tax_rate":"3","tax_label":"ignored","lines":[]});
    let mut b = a.clone();
    b["tax_rate"] = json!("99");
    b["tax_label"] = json!("also ignored");
    let da = t::creation_digest(&a, || panic!("ITEM must not expand live state")).unwrap();
    let db = t::creation_digest(&b, || panic!("ITEM must not expand live state")).unwrap();
    assert_eq!(da, db);
    b["lines"] = json!([{"rate":"1"}]);
    assert_ne!(da, t::creation_digest(&b, || panic!("no live lookup")).unwrap());
    assert!(t::creation_digest(&json!({"tax_mode":"ITEM","tax_item_id":0}), || Ok(Value::Null)).is_err());
    let manual = t::creation_digest(&json!({"tax_mode":"MANUAL"}), || Ok(json!({"expanded":true}))).unwrap();
    assert_eq!(manual.len(), 64);
}
#[test]
fn picker_hints_and_audit_snapshot_shapes_match_python() {
    let mut f = Fx::new("hints");
    let (c, asset, _) = f.chart();
    let cust = directory::save_record(
        &mut f.book,
        f.user,
        c,
        Model::Customer,
        &json!({"display_name":"Exempt","tax_status":"EXEMPT","exemption_number":"RESALE"}),
        None,
    )
    .unwrap();
    let hints = directory::customer_tax_hints(f.book.connection(), c).unwrap();
    assert_eq!(hints[cust["id"].as_str().unwrap()], json!({"exempt":true,"certificate":"RESALE"}));
    let v = directory::save_record(
        &mut f.book,
        f.user,
        c,
        Model::Vendor,
        &json!({"display_name":"Vendor","default_expense_account":asset.to_db(),"default_terms":"15"}),
        None,
    )
    .unwrap();
    let vid = v["id"].as_str().unwrap();
    assert_eq!(
        directory::prefill_catalog(f.book.connection(), c).unwrap()[vid]["expense_account"],
        asset.to_db()
    );
    d::save_setup(&mut f.book, f.user, c, &json!({"receivables":asset.to_db()}), Some(0)).unwrap();
    assert_eq!(
        directory::prefill_catalog(f.book.connection(), c).unwrap()[vid]["expense_account"],
        ""
    );
    let detail: String = f
        .book
        .connection()
        .query_row("SELECT detail FROM ledger_auditevent WHERE action='accounting.setup'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let detail: Value = serde_json::from_str(&detail).unwrap();
    assert!(detail["after"].get("id").is_none());
    assert_eq!(detail["after"]["receivables"], asset.to_db());
    let source = f
        .book
        .write(|tx| d::create_identity(tx, f.user, c, "EST", "00000000-0000-0000-0000-000000000001"))
        .unwrap();
    let sid = source["id"].as_str().unwrap();
    f.book
        .write(|tx| d::snapshot_source_accounts(tx, f.user, c, sid, &json!({"asset":asset.to_db()})))
        .unwrap();
    let detail: String = f
        .book
        .connection()
        .query_row("SELECT detail FROM ledger_auditevent WHERE action='document.accounts'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let detail: Value = serde_json::from_str(&detail).unwrap();
    assert_eq!(detail["accounts"][0]["id"], asset.to_db());
    assert!(detail["accounts"][0].get("account_id").is_none());
}
#[test]
fn editing_a_member_rate_can_make_an_existing_group_exceed_100_percent() {
    let mut f = Fx::new("tax-oddity");
    let c = f.company("C");
    let a = t::save_rate(
        &mut f.book,
        f.user,
        c,
        &json!({"name":"A","description":"A","rate":"50","new_agency_name":"A"}),
    )
    .unwrap();
    let b = t::save_rate(
        &mut f.book,
        f.user,
        c,
        &json!({"name":"B","description":"B","rate":"50","new_agency_name":"B"}),
    )
    .unwrap();
    let group = t::save_group(
        &mut f.book,
        f.user,
        c,
        &json!({"name":"Group","description":"Group","member_ids":[a["id"],b["id"]]}),
    )
    .unwrap();
    t::save_rate(
        &mut f.book,
        f.user,
        c,
        &json!({"item_id":a["id"],"expected_revision":1,"name":"A","description":"A","rate":"60","agency_id":a["agency_id"]}),
    )
    .unwrap();
    assert_eq!(t::group_rate(f.book.connection(), c, &group).unwrap().to_string(), "110.0000");
    let snapshot = t::tax_snapshot(
        f.book.connection(),
        c,
        &t::item(f.book.connection(), c, group["id"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    assert!(fb_core::calculations::totals(&[], "GROUP", None, &json!(""), Some(&snapshot), false).is_err());
}
