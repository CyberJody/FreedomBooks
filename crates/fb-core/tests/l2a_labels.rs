//! Directory label and shared purchase-line rule tests from the frozen app.
mod common;
use common::*;
use fb_core::{
    directory::{self, Model},
    documents as d, party_display as p,
    services::{self, AccountInput},
};
use serde_json::json;
#[test]
fn duplicate_names_use_town_then_email_then_id_and_include_archived_peers() {
    let mut f = Fx::new("labels");
    let c = f.company("C");
    let a = directory::save_record(
        &mut f.book,
        f.user,
        c,
        Model::Customer,
        &json!({"display_name":"Same","billing_address":"Street\u{2028}Town A, NY","email":"a@example.com"}),
        None,
    )
    .unwrap();
    let b = directory::save_record(
        &mut f.book,
        f.user,
        c,
        Model::Customer,
        &json!({"display_name":"Same","billing_address":"Street\u{2028}Town B, NY"}),
        None,
    )
    .unwrap();
    let aid = a["id"].as_str().unwrap();
    let bid = b["id"].as_str().unwrap();
    let map = p::contact_label_map(f.book.connection(), c, Model::Customer, None).unwrap();
    assert_eq!(map[aid], "Same (Town A)");
    assert_eq!(map[bid], "Same (Town B)");
    directory::save_record(
        &mut f.book,
        f.user,
        c,
        Model::Customer,
        &json!({"billing_address":"Street\u{2028}Town A, NY"}),
        Some(bid),
    )
    .unwrap();
    let map = p::contact_label_map(f.book.connection(), c, Model::Customer, Some(std::slice::from_ref(&a))).unwrap();
    assert_eq!(map[aid], "Same (a@example.com)");
    let all = p::contact_label_map(f.book.connection(), c, Model::Customer, None).unwrap();
    assert_eq!(all[bid], format!("Same (#{bid})"));
    directory::set_active(&mut f.book, f.user, c, Model::Customer, bid, false).unwrap();
    assert!(
        p::duplicate_active_display_names(f.book.connection(), c, Model::Customer)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        p::contact_label_map(f.book.connection(), c, Model::Customer, None).unwrap()[aid],
        "Same"
    );
    assert_eq!(p::posting_role_label(Some("income:999"), Some("SR")), "Income account");
    assert_eq!(p::posting_role_label(Some("income:2"), Some("INV")), "Income account (line 2)");
}
#[test]
fn purchase_line_rules_allow_retained_inactive_but_not_controls_or_the_paying_bank() {
    let mut f = Fx::new("purchase-lines");
    let (c, bank, _) = f.chart();
    let exp = f.account(c, "5", "Expense", "EXPENSE");
    let account = fb_core::store::account(f.book.connection(), exp).unwrap().unwrap();
    assert!(d::purchase_line_allowed(f.book.connection(), c, &account, None).unwrap());
    services::save_account(
        &mut f.book,
        f.user,
        c,
        AccountInput::new("5", "Expense").kind("EXPENSE").edit(exp).active(false),
    )
    .unwrap();
    let account = fb_core::store::account(f.book.connection(), exp).unwrap().unwrap();
    assert!(
        d::line_account_error(f.book.connection(), c, Some(&account), None, true)
            .unwrap()
            .is_none()
    );
    assert!(
        d::line_account_error(f.book.connection(), c, Some(&account), None, false)
            .unwrap()
            .is_some()
    );
    let account = fb_core::store::account(f.book.connection(), bank).unwrap().unwrap();
    assert!(
        d::line_account_error(f.book.connection(), c, Some(&account), Some(bank), true)
            .unwrap()
            .unwrap()
            .contains("bank this check is paid from")
    );
    d::save_setup(&mut f.book, f.user, c, &json!({"receivables":bank.to_db()}), Some(0)).unwrap();
    assert!(
        d::line_account_error(f.book.connection(), c, Some(&account), None, true)
            .unwrap()
            .unwrap()
            .contains("receivables")
    );
    assert!(
        d::eligible_line_accounts(f.book.connection(), c)
            .unwrap()
            .iter()
            .all(|a| a.id != bank && a.id != exp)
    );
}
