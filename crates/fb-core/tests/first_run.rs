//! Ported documents/test_first_run.py and test_corefx_tax_name.py service assertions.
mod common;
use common::*;
use fb_core::{
    CompanyId, documents, first_run,
    services::{self, AccountInput},
    store, users,
};
use serde_json::{Value, json};

fn account(f: &mut Fx, c: CompanyId, code: &str, name: &str, detail: &str) -> fb_core::AccountId {
    services::save_account(
        &mut f.book,
        f.user,
        c,
        AccountInput::new(code, name).detail(detail),
    )
    .unwrap()
    .id
}
fn proposal(f: &mut Fx, c: CompanyId) -> Value {
    f.book.read(|tx| first_run::propose(tx, c)).unwrap()
}

#[test]
fn live_checklist_clear_ambiguous_stale_and_idempotent_dismiss() {
    let mut f = Fx::new("first-run");
    let c = services::create_company(&mut f.book, f.user, "Setup books", "USD", true)
        .unwrap()
        .id;
    let empty = proposal(&mut f, c);
    assert_eq!(empty["empty"], true);
    assert!(
        first_run::apply_proposal(
            &mut f.book,
            f.user,
            c,
            empty["digest"].as_str().unwrap(),
            Some(0)
        )
        .unwrap_err()
        .message()
        .contains("Nothing on the chart")
    );
    let ar = account(&mut f, c, "1100", "Accounts receivable", "RECEIVABLE");
    let ap = account(&mut f, c, "2000", "Accounts payable", "PAYABLE");
    let uf = account(
        &mut f,
        c,
        "1200",
        "Undeposited funds",
        "OTHER_CURRENT_ASSET",
    );
    let tax = account(
        &mut f,
        c,
        "2200",
        "Sales tax payable",
        "OTHER_CURRENT_LIABILITY",
    );
    account(
        &mut f,
        c,
        "2300",
        "Income tax payable",
        "OTHER_CURRENT_LIABILITY",
    );
    account(
        &mut f,
        c,
        "2400",
        "Payroll tax payable",
        "OTHER_CURRENT_LIABILITY",
    );
    let bank = account(&mut f, c, "1000", "Operating checking", "BANK");
    let clear = proposal(&mut f, c);
    for (role, id) in [
        ("receivables", ar),
        ("payables", ap),
        ("undeposited", uf),
        ("collected_tax", tax),
    ] {
        assert_eq!(clear["chosen"][role], id.to_db());
    }
    assert_eq!(clear["chosen"]["banks"], json!([bank.to_db()]));
    assert_eq!(
        f.audit_count("accounting.setup"),
        0,
        "preview writes nothing"
    );
    account(&mut f, c, "1101", "Other receivables", "RECEIVABLE");
    assert!(
        first_run::apply_proposal(
            &mut f.book,
            f.user,
            c,
            clear["digest"].as_str().unwrap(),
            Some(0)
        )
        .is_err(),
        "changed chart refused"
    );
    assert_eq!(f.audit_count("accounting.setup"), 0);
    let mixed = proposal(&mut f, c);
    assert!(mixed["chosen"].get("receivables").is_none());
    assert!(
        mixed["reasons"]["receivables"]
            .as_str()
            .unwrap()
            .starts_with("More than one")
    );
    first_run::apply_proposal(
        &mut f.book,
        f.user,
        c,
        mixed["digest"].as_str().unwrap(),
        Some(0),
    )
    .unwrap();
    assert_eq!(f.audit_count("accounting.setup"), 1);
    assert_eq!(f.audit_count("setup.filled"), 1);
    assert!(
        first_run::apply_proposal(
            &mut f.book,
            f.user,
            c,
            mixed["digest"].as_str().unwrap(),
            Some(0)
        )
        .is_err()
    );
    account(&mut f, c, "4000", "Sales", "OTHER_INCOME");
    account(&mut f, c, "5000", "Purchases", "COST_OF_GOODS_SOLD");
    let state = f.book.read(|tx| first_run::checklist(tx, c)).unwrap();
    assert_eq!(state["chart"], true);
    assert_eq!(state["special"], false);
    assert_eq!(state["tax"], false);
    first_run::dismiss_first_run(&mut f.book, f.user, c).unwrap();
    first_run::dismiss_first_run(&mut f.book, f.user, c).unwrap();
    assert_eq!(f.audit_count("setup.checklist.dismissed"), 1);
    assert!(
        !store::get_company(f.book.connection(), c)
            .unwrap()
            .first_run_open
    );
}

#[test]
fn fill_preserves_unproposed_defaults_and_rolls_back_on_final_audit_failure() {
    let mut f = Fx::new("first-run-atomic");
    let c = f.company("Setup");
    let bank = account(&mut f, c, "1000", "Bank", "BANK");
    let ar = account(&mut f, c, "1100", "Receivable", "RECEIVABLE");
    documents::save_setup(
        &mut f.book,
        f.user,
        c,
        &json!({"receivables":ar.to_db(),"banks":[bank.to_db()]}),
        Some(0),
    )
    .unwrap();
    account(&mut f, c, "1101", "Other receivable", "RECEIVABLE");
    let offered = proposal(&mut f, c);
    f.book.connection().execute_batch("CREATE TEMP TRIGGER fail_fill BEFORE INSERT ON ledger_auditevent WHEN NEW.action='setup.filled' BEGIN SELECT RAISE(ABORT,'injected final fill audit failure'); END;").unwrap();
    assert!(
        first_run::apply_proposal(
            &mut f.book,
            f.user,
            c,
            offered["digest"].as_str().unwrap(),
            Some(1)
        )
        .unwrap_err()
        .message()
        .contains("injected")
    );
    let current = f.book.read(|tx| documents::setup_snapshot(tx, c)).unwrap();
    assert_eq!(current["revision"], 1);
    assert_eq!(f.audit_count("accounting.setup"), 1);
    assert_eq!(f.audit_count("setup.filled"), 0);
    f.book
        .connection()
        .execute_batch("DROP TRIGGER fail_fill")
        .unwrap();
    first_run::apply_proposal(
        &mut f.book,
        f.user,
        c,
        offered["digest"].as_str().unwrap(),
        Some(1),
    )
    .unwrap();
    assert_eq!(
        f.book.read(|tx| documents::setup_snapshot(tx, c)).unwrap()["receivables"],
        ar.to_db()
    );
}

#[test]
fn company_scope_inactive_candidates_and_bounded_scans() {
    let mut f = Fx::new("first-run-scope");
    let c = f.company("First");
    let other = f.company("Other");
    let outsider = users::create_user(&mut f.book, "outsider", false).unwrap();
    let dead = services::save_account(
        &mut f.book,
        f.user,
        c,
        AccountInput::new("1100", "Receivable")
            .detail("RECEIVABLE")
            .active(false),
    )
    .unwrap();
    assert!(
        proposal(&mut f, c)["reasons"]["receivables"]
            .as_str()
            .unwrap()
            .contains("inactive")
    );
    assert!(first_run::fill_setup(&mut f.book,f.user,c,&json!({"receivables":dead.id.to_db(),"payables":null,"undeposited":null,"collected_tax":null,"banks":[]}),Some(0)).is_err());
    assert_eq!(f.audit_count("setup.filled"), 0);
    account(&mut f, c, "1000", "Bank", "BANK");
    let p = proposal(&mut f, c);
    assert!(
        first_run::apply_proposal(
            &mut f.book,
            f.user,
            other,
            p["digest"].as_str().unwrap(),
            Some(0)
        )
        .is_err()
    );
    assert!(first_run::dismiss_first_run(&mut f.book, outsider, c).is_err());
    for n in 0..21 {
        account(
            &mut f,
            c,
            &format!("A{n:02}"),
            "Other asset",
            "OTHER_CURRENT_ASSET",
        );
    }
    assert!(
        proposal(&mut f, c)["reasons"]["undeposited"]
            .as_str()
            .unwrap()
            .starts_with("Too many")
    );
}

#[test]
fn sales_tax_name_oracle_and_python_word_boundaries() {
    let vectors: Value = serde_json::from_str(include_str!("data/d2-tax-names.json")).unwrap();
    for row in vectors.as_array().unwrap() {
        let name = row[0].as_str().unwrap();
        assert_eq!(
            first_run::sales_tax_name(name),
            row[1].as_bool().unwrap(),
            "{name}"
        );
    }
}
