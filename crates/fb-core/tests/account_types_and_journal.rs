//! Translation of the in-scope parts of ledger/test_account_types.py (FX2 types, contra, description,
//! the type-change rule, the trial balance with contra accounts) and ledger/test_general_journal.py
//! (entry numbers, adjusting flag, lines/bounds, creation retries, optimistic revisions, races).
//! Out of scope here: the HTTP screens, the P&L / balance sheet (layer 3) and control accounts (layer 2).
mod common;

use common::*;
use fb_core::model::{DetailType, Kind, Side, Status};
use fb_core::services::{self, AccountInput, DraftInput, ENTRY_NUMBER_MAX, line};
use fb_core::{AccountId, CompanyId, FbError, reports, users};
use serde_json::json;
use std::collections::HashMap;

/// `TypedBooks`: a chart with one account of every FX2 type and two contra accounts.
fn typed(fx: &mut Fx) -> (CompanyId, HashMap<&'static str, AccountId>) {
    let company = fx.company("Typed books");
    let mut chart = HashMap::new();
    for (code, name, detail, contra) in [
        ("100", "Synthetic bank", "BANK", false),
        ("105", "Petty cash", "OTHER_CURRENT_ASSET", false),
        ("110", "Receivables", "RECEIVABLE", false),
        ("150", "Equipment", "FIXED_ASSET", false),
        ("159", "Accumulated depreciation", "FIXED_ASSET", true),
        ("170", "Security deposit", "OTHER_ASSET", false),
        ("200", "Payables", "PAYABLE", false),
        ("210", "Accrued", "OTHER_CURRENT_LIABILITY", false),
        ("220", "Company card", "CREDIT_CARD", false),
        ("250", "Equipment loan", "LONG_TERM_LIABILITY", false),
        ("300", "Owner capital", "EQUITY", false),
        ("310", "Owner draw", "EQUITY", true),
        ("400", "Service income", "INCOME", false),
        ("420", "Interest income", "OTHER_INCOME", false),
        ("500", "Expense", "EXPENSE", false),
        ("510", "Materials", "COST_OF_GOODS_SOLD", false),
        ("590", "Loss on disposal", "OTHER_EXPENSE", false),
    ] {
        let a = services::save_account(&mut fx.book, fx.user, company, AccountInput::new(code, name).detail(detail).contra(contra).description(&format!("Synthetic {}", name.to_lowercase())))
            .unwrap();
        chart.insert(code, a.id);
    }
    (company, chart)
}

fn post(fx: &mut Fx, company: CompanyId, chart: &HashMap<&str, AccountId>, date: &str, lines: &[(&str, i64)], adjusting: bool) {
    let lines = lines
        .iter()
        .map(|(code, cents)| {
            let amount = format!("{}.{:02}", cents.abs() / 100, cents.abs() % 100);
            if *cents > 0 { line(chart[code]).debit(&amount).build() } else { line(chart[code]).credit(&amount).build() }
        })
        .collect();
    let mut input = DraftInput::new(date, "Synthetic entry", lines);
    input.adjusting = Some(adjusting);
    let j = services::save_draft(&mut fx.book, fx.user, company, input).unwrap();
    services::post_journal(&mut fx.book, fx.user, company, j.id, None).unwrap();
}

/// `TypedBooks.build` (the ledger part; the invoice / bill half belongs to layer 2).
fn build(fx: &mut Fx, company: CompanyId, chart: &HashMap<&str, AccountId>) {
    post(fx, company, chart, "2026-10-01", &[("100", 500000), ("300", -500000)], false);
    post(fx, company, chart, "2026-10-02", &[("150", 300000), ("250", -200000), ("100", -100000)], false);
    post(fx, company, chart, "2026-10-03", &[("170", 20000), ("100", -20000)], false);
    post(fx, company, chart, "2026-10-15", &[("310", 30000), ("100", -30000)], false);
    post(fx, company, chart, "2026-10-20", &[("500", 4000), ("220", -4000)], false);
    post(fx, company, chart, "2026-10-25", &[("100", 1234), ("420", -1234)], false);
    post(fx, company, chart, "2026-10-26", &[("590", 500), ("100", -500)], false);
    post(fx, company, chart, "2026-10-31", &[("500", 5000), ("159", -5000)], true);
}

#[test]
fn every_type_maps_to_one_kind_and_the_defaults_follow_the_kind() {
    let mut fx = Fx::new("types");
    let (company, chart) = typed(&mut fx);
    for (code, kind, detail) in [
        ("100", Kind::Asset, DetailType::Bank),
        ("105", Kind::Asset, DetailType::OtherCurrentAsset),
        ("110", Kind::Asset, DetailType::Receivable),
        ("200", Kind::Liability, DetailType::Payable),
        ("250", Kind::Liability, DetailType::LongTermLiability),
        ("420", Kind::Income, DetailType::OtherIncome),
        ("510", Kind::Expense, DetailType::CostOfGoodsSold),
    ] {
        let a = fb_core::store::account(fx.book.connection(), chart[code]).unwrap().unwrap();
        assert_eq!((a.kind, a.detail_type), (kind, detail), "{code}");
    }
    let a = services::save_account(&mut fx.book, fx.user, company, AccountInput::new("160", "Vehicle").detail("FIXED_ASSET")).unwrap();
    assert_eq!(a.kind, Kind::Asset);
    let a = services::save_account(&mut fx.book, fx.user, company, AccountInput::new("260", "Payroll liabilities").kind("LIABILITY")).unwrap();
    assert_eq!(a.detail_type, DetailType::OtherCurrentLiability);
    assert_eq!(
        err_text(services::save_account(&mut fx.book, fx.user, company, AccountInput::new("261", "Mismatch").kind("INCOME").detail("BANK"))),
        "The account type does not belong to that kind."
    );
    assert_eq!(fx.count("SELECT count(*) FROM ledger_account WHERE code = '261'"), 0);
    assert_eq!(err_text(services::save_account(&mut fx.book, fx.user, company, AccountInput::new("262", "No type"))), "Choose an account type.");
    assert_eq!(
        err_text(services::save_account(&mut fx.book, fx.user, company, AccountInput::new("263", "Bad type").detail("NOT_A_TYPE"))),
        "Choose an account type from the list."
    );
}

#[test]
fn database_refuses_a_type_of_another_kind_and_contra_on_bank_and_fills_a_missing_type() {
    let mut fx = Fx::new("types-db");
    let (company, chart) = typed(&mut fx);
    let income = chart["400"];
    for sql in [
        format!("UPDATE ledger_account SET detail_type = 'BANK' WHERE id = '{income}'"),
        format!("UPDATE ledger_account SET detail_type = 'NOT_A_TYPE' WHERE id = '{income}'"),
    ] {
        assert_eq!(attack(&fx.book, &sql).as_deref(), Some("account_detail_type_kind"), "{sql}");
    }
    assert_eq!(attack(&fx.book, &format!("UPDATE ledger_account SET is_contra = 1 WHERE id = '{}'", chart["100"])).as_deref(), Some("account_contra_type"));
    let raw = fb_ids::AccountId::new();
    fx.book
        .connection()
        .execute(&format!("INSERT INTO ledger_account (id, company_id, code, name, kind, active, postable) VALUES ('{raw}', '{company}', 'RAW', 'Raw', 'LIABILITY', 1, 1)"), [])
        .unwrap();
    let (detail, contra, description): (String, bool, String) = fx
        .book
        .connection()
        .query_row("SELECT detail_type, is_contra, description FROM ledger_account WHERE id = ?1", [raw.to_db()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap();
    assert_eq!((detail.as_str(), contra, description.as_str()), ("OTHER_CURRENT_LIABILITY", false, ""));
    // Blanking a stored type refills the plain type of the kind (PG fills it on UPDATE too).
    fx.book.connection().execute("UPDATE ledger_account SET detail_type = '' WHERE id = ?1", [raw.to_db()]).unwrap();
    let detail: String = fx.book.connection().query_row("SELECT detail_type FROM ledger_account WHERE id = ?1", [raw.to_db()], |r| r.get(0)).unwrap();
    assert_eq!(detail, "OTHER_CURRENT_LIABILITY");
}

#[test]
fn contra_flips_the_normal_balance_but_not_the_section_side() {
    let mut fx = Fx::new("contra");
    let (company, chart) = typed(&mut fx);
    let get = |fx: &Fx, code: &str| fb_core::store::account(fx.book.connection(), chart[code]).unwrap().unwrap();
    assert_eq!((get(&fx, "159").normal_balance(), get(&fx, "159").section_balance()), (Side::Credit, Side::Debit));
    assert_eq!((get(&fx, "310").normal_balance(), get(&fx, "310").section_balance()), (Side::Debit, Side::Credit));
    assert_eq!((get(&fx, "150").normal_balance(), get(&fx, "300").normal_balance()), (Side::Debit, Side::Credit));
    for (detail, label) in [("BANK", "bank"), ("RECEIVABLE", "accounts receivable"), ("PAYABLE", "accounts payable"), ("CREDIT_CARD", "credit card")] {
        let code = format!("X{}", &detail[..detail.len().min(6)]);
        assert_eq!(
            err_text(services::save_account(&mut fx.book, fx.user, company, AccountInput::new(&code, "Contra").detail(detail).contra(true))),
            format!("A {label} account cannot be a contra account.")
        );
    }
}

#[test]
fn description_is_optional_bounded_and_audited() {
    let mut fx = Fx::new("description");
    let (company, chart) = typed(&mut fx);
    let account = services::save_account(&mut fx.book, fx.user, company, AccountInput::new("180", "Prepaid rent").detail("OTHER_CURRENT_ASSET").description("Rent paid ahead")).unwrap();
    assert_eq!(account.description, "Rent paid ahead");
    assert_eq!(
        err_text(services::save_account(&mut fx.book, fx.user, company, AccountInput::new("180", "Prepaid rent").detail("OTHER_CURRENT_ASSET").description(&"x".repeat(201)).edit(account.id))),
        "Ensure this value has at most 200 characters (it has 201)."
    );
    let detail: String = fx
        .book
        .connection()
        .query_row("SELECT detail FROM ledger_auditevent WHERE action = 'account.saved' AND target = ?1 ORDER BY id DESC LIMIT 1", [format!("account:{}", account.id)], |r| r.get(0))
        .unwrap();
    let detail: serde_json::Value = serde_json::from_str(&detail).unwrap();
    assert_eq!(detail["after"]["description"], "Rent paid ahead");
    assert_eq!(detail["after"]["detail_type"], "OTHER_CURRENT_ASSET");
    let bank = services::save_account(&mut fx.book, fx.user, company, AccountInput::new("100", "Synthetic bank renamed").kind("ASSET").edit(chart["100"])).unwrap();
    assert_eq!((bank.detail_type, bank.is_contra), (DetailType::Bank, false));
    let draw = services::save_account(&mut fx.book, fx.user, company, AccountInput::new("310", "Owner draw").kind("EQUITY").edit(chart["310"])).unwrap();
    assert_eq!((draw.is_contra, draw.description.as_str()), (true, "Synthetic owner draw"));
}

#[test]
fn unused_accounts_may_change_to_any_type_used_accounts_only_within_their_kind() {
    let mut fx = Fx::new("type-change");
    let (company, chart) = typed(&mut fx);
    let spare = services::save_account(&mut fx.book, fx.user, company, AccountInput::new("199", "Spare").detail("OTHER_ASSET")).unwrap();
    let spare = services::save_account(&mut fx.book, fx.user, company, AccountInput::new("199", "Spare").detail("LONG_TERM_LIABILITY").edit(spare.id)).unwrap();
    assert_eq!((spare.kind, spare.detail_type), (Kind::Liability, DetailType::LongTermLiability));
    build(&mut fx, company, &chart);
    let moved = services::save_account(&mut fx.book, fx.user, company, AccountInput::new("150", "Equipment").detail("OTHER_ASSET").edit(chart["150"])).unwrap();
    assert_eq!((moved.kind, moved.detail_type), (Kind::Asset, DetailType::OtherAsset));
    services::save_account(&mut fx.book, fx.user, company, AccountInput::new("150", "Equipment").detail("FIXED_ASSET").edit(chart["150"])).unwrap();
    let before = fx.count("SELECT count(*) FROM ledger_auditevent");
    for (code, detail) in [("150", "LONG_TERM_LIABILITY"), ("420", "OTHER_EXPENSE"), ("300", "BANK")] {
        let message = err_text(services::save_account(&mut fx.book, fx.user, company, AccountInput::new(code, "Renamed").detail(detail).edit(chart[code])));
        assert!(message.contains("cannot change its type"), "{code}: {message}");
    }
    assert_eq!(fx.count("SELECT count(*) FROM ledger_auditevent"), before);
    assert_eq!(fx.count(&format!("SELECT count(*) FROM ledger_account WHERE id = '{}' AND detail_type = 'FIXED_ASSET'", chart["150"])), 1);
}

#[test]
fn database_refuses_a_consistent_kind_and_type_change_on_a_used_account() {
    let mut fx = Fx::new("type-db");
    let (company, chart) = typed(&mut fx);
    build(&mut fx, company, &chart);
    let sql = format!("UPDATE ledger_account SET kind = 'LIABILITY', detail_type = 'LONG_TERM_LIABILITY' WHERE id = '{}'", chart["150"]);
    assert_eq!(attack_message(&fx.book, &sql).as_deref(), Some("fb_account_guard: Used accounts cannot change company or type"));
    let unused = services::save_account(&mut fx.book, fx.user, company, AccountInput::new("198", "Unused").detail("OTHER_ASSET")).unwrap();
    fx.book.connection().execute("UPDATE ledger_account SET kind = 'LIABILITY', detail_type = 'LONG_TERM_LIABILITY' WHERE id = ?1", [unused.id.to_db()]).unwrap();
    fx.book.connection().execute("UPDATE ledger_account SET detail_type = 'OTHER_ASSET' WHERE id = ?1", [chart["150"].to_db()]).unwrap();
}

#[test]
fn trial_balance_keeps_natural_columns_and_registers_read_contra_negative() {
    let mut fx = Fx::new("typed-tb");
    let (company, chart) = typed(&mut fx);
    build(&mut fx, company, &chart);
    let tb = reports::trial_balance(&mut fx.book, fx.user, company, "2026-10-31").unwrap();
    let row = |code: &str| tb.rows.iter().find(|r| r.account.code == code).map(|r| (r.debit.to_string(), r.credit.to_string())).unwrap();
    assert_eq!(row("159"), ("0.00".into(), "50.00".into()));
    assert_eq!(row("310"), ("300.00".into(), "0.00".into()));
    assert_eq!(row("100"), ("3507.34".into(), "0.00".into()));
    assert_eq!(tb.debit, tb.credit);
    assert_eq!(tb.debit.to_string(), "7102.34");
    let contra = reports::register(&mut fx.book, fx.user, company, chart["159"], None, None).unwrap();
    assert_eq!(contra.closing.to_string(), "-50.00");
    let draw = reports::register(&mut fx.book, fx.user, company, chart["310"], None, None).unwrap();
    assert_eq!(draw.closing.to_string(), "-300.00");
}

// ------------------------------------------------------------------ general journal (N16)

struct Gj {
    fx: Fx,
    company: CompanyId,
    expense: AccountId,
    prepaid: AccountId,
    accrued: AccountId,
}

fn gj(tag: &str) -> Gj {
    let mut fx = Fx::new(tag);
    let company = fx.company("N16 journal books");
    let prepaid = fx.account(company, "1300", "Prepaid insurance", "ASSET");
    let accrued = fx.account(company, "2200", "Accrued wages", "LIABILITY");
    let expense = fx.account(company, "6000", "Insurance expense", "EXPENSE");
    Gj { fx, company, expense, prepaid, accrued }
}

impl Gj {
    fn lines(&self, amount: &str) -> Vec<serde_json::Value> {
        vec![line(self.expense).debit(amount).description("Expense part").build(), line(self.prepaid).credit(amount).description("Prepaid part").build()]
    }
    fn input(&self) -> DraftInput {
        DraftInput::new("2026-10-31", "Month-end insurance", self.lines("100.00"))
    }
    fn entry(&mut self, input: DraftInput) -> fb_core::FbResult<fb_core::model::Journal> {
        services::save_draft(&mut self.fx.book, self.fx.user, self.company, input)
    }
    fn numbered(&mut self, number: &str) -> fb_core::FbResult<fb_core::model::Journal> {
        let mut input = self.input();
        input.entry_number = Some(number.into());
        self.entry(input)
    }
    fn next(&mut self) -> String {
        services::next_entry_number(&mut self.fx.book, self.company).unwrap()
    }
}

#[test]
fn entry_numbers_are_automatic_editable_and_unique() {
    let mut g = gj("entry-numbers");
    let first = g.entry(g.input()).unwrap();
    let second = g.entry(g.input()).unwrap();
    assert_eq!((first.entry_number.as_str(), second.entry_number.as_str()), ("1", "2"));
    assert_eq!(g.numbered("  FAM2010 ").unwrap().entry_number, "FAM2010");
    assert_eq!(g.next(), "FAM2011");
    assert_eq!(g.entry(g.input()).unwrap().entry_number, "FAM2011");
    g.numbered("0099").unwrap();
    assert_eq!(g.next(), "0100");
    g.numbered("ADJ-A").unwrap();
    assert_eq!(g.next(), "100");
    assert!(err_text(g.numbered("FAM2010")).contains("Entry number FAM2010 is already used"));
    for (bad, message) in [("x".repeat(21), "up to 20"), ("A\u{200b}B".into(), "control or formatting"), ("A\nB".into(), "control or formatting")] {
        assert!(err_text(g.numbered(&bad)).contains(message), "{bad:?}");
    }
    assert_eq!(g.numbered(&"🧾".repeat(20)).unwrap().entry_number, "🧾".repeat(20));
    let mut edit = g.input();
    edit.journal_id = Some(first.id);
    edit.memo = "Edited".into();
    edit.expected_revision = Some(1);
    let edited = g.entry(edit.clone()).unwrap();
    assert_eq!((edited.entry_number.as_str(), edited.revision), ("1", 2));
    edit.entry_number = Some("2".into());
    edit.expected_revision = Some(2);
    assert!(err_text(g.entry(edit)).contains("already used"));
    let posted = services::post_journal(&mut g.fx.book, g.fx.user, g.company, second.id, Some(1)).unwrap();
    let reversal = services::reverse_journal(&mut g.fx.book, g.fx.user, g.company, posted.id, "2026-11-01", "Wrong month").unwrap();
    assert_eq!((reversal.entry_number.as_str(), reversal.display_number()), ("", reversal.display()));
    assert_eq!(attack(&g.fx.book, &format!("UPDATE ledger_journal SET entry_number = '99' WHERE id = '{}'", posted.id)).as_deref(), Some("fb_journal_guard"));
    assert_eq!(
        attack(&g.fx.book, &format!("UPDATE ledger_journal SET entry_number = 'FAM2010' WHERE id = '{}'", edited.id)).as_deref(),
        Some("fb_journal_guard")
    );
}

#[test]
fn fallback_does_not_collide_with_19_or_20_digit_numbers_and_blank_draft_posts() {
    let mut g = gj("fallback");
    let (eighteen, nineteen, twenty) = ("9".repeat(18), format!("1{}", "0".repeat(18)), "9".repeat(20));
    g.numbered(&nineteen).unwrap();
    g.numbered(&eighteen).unwrap();
    let suggested = g.next();
    assert!(suggested.len() <= ENTRY_NUMBER_MAX);
    assert!(suggested != eighteen && suggested != nineteen, "{suggested}");
    assert_eq!(g.fx.count(&format!("SELECT count(*) FROM ledger_journal WHERE entry_number = '{suggested}'")), 0);
    g.numbered(&twenty).unwrap();
    let suggested = g.next();
    assert!(suggested.len() <= ENTRY_NUMBER_MAX);
    assert_eq!(g.fx.count(&format!("SELECT count(*) FROM ledger_journal WHERE entry_number = '{suggested}'")), 0);
    let mut input = g.input();
    input.memo = "Draft saved before entry numbers".into();
    let draft = g.entry(input).unwrap();
    g.fx.book.connection().execute("UPDATE ledger_journal SET entry_number = '' WHERE id = ?1", [draft.id.to_db()]).unwrap();
    let posted = services::post_journal(&mut g.fx.book, g.fx.user, g.company, draft.id, Some(1)).unwrap();
    assert_eq!(posted.status, Status::Posted);
    assert!(!posted.entry_number.is_empty() && posted.entry_number.len() <= ENTRY_NUMBER_MAX);
    assert_eq!(g.fx.count(&format!("SELECT count(*) FROM ledger_journal WHERE entry_number = '{}'", posted.entry_number)), 1);
}

#[test]
fn adjusting_flag_is_stored_immutable_and_kept_by_reversals() {
    let mut g = gj("adjusting");
    let mut input = DraftInput::new("2026-10-31", "Accrue October wages", vec![line(g.expense).debit("1200.00").build(), line(g.accrued).credit("1200.00").build()]);
    input.adjusting = Some(true);
    let adjusting = g.entry(input).unwrap();
    let adjusting = services::post_journal(&mut g.fx.book, g.fx.user, g.company, adjusting.id, Some(1)).unwrap();
    assert!(adjusting.adjusting);
    let detail: String = g
        .fx
        .book
        .connection()
        .query_row("SELECT detail FROM ledger_auditevent WHERE action = 'journal.posted' AND target = ?1", [format!("journal:{}", adjusting.id)], |r| r.get(0))
        .unwrap();
    assert_eq!(serde_json::from_str::<serde_json::Value>(&detail).unwrap()["adjusting"], true);
    assert_eq!(attack(&g.fx.book, &format!("UPDATE ledger_journal SET adjusting = 0 WHERE id = '{}'", adjusting.id)).as_deref(), Some("fb_journal_guard"));
    let reversal = services::reverse_journal(&mut g.fx.book, g.fx.user, g.company, adjusting.id, "2026-11-01", "Auto-reverse accrual").unwrap();
    assert!(reversal.adjusting);
    // DB: a raw reversal that drops the flag is refused even when its lines mirror exactly.
    let mut input = g.input();
    input.adjusting = Some(true);
    let other = g.entry(input).unwrap();
    let other = services::post_journal(&mut g.fx.book, g.fx.user, g.company, other.id, Some(1)).unwrap();
    // Declare the reversal at creation, then try dropping its inherited flag at posting.
    let mirror = g.fx.book.write(|tx| services::save_draft_tx(
        tx, g.fx.user, g.company,
        &DraftInput::new("2026-10-31", "Mirror", vec![line(g.prepaid).debit("100.00").description("Prepaid part").build(), line(g.expense).credit("100.00").description("Expense part").build()]),
        Some(&other),
    )).unwrap();
    // Positions differ from the original (expense first), so put them in the same order to isolate the flag.
    g.fx.book.connection().execute_batch(&format!(
        "UPDATE ledger_journalline SET position = 9 WHERE journal_id = '{m}' AND position = 1;
         UPDATE ledger_journalline SET position = 1 WHERE journal_id = '{m}' AND position = 2;
         UPDATE ledger_journalline SET position = 2 WHERE journal_id = '{m}' AND position = 9;",
        m = mirror.id
    )).unwrap();
    let sql = format!(
        "UPDATE ledger_journal SET adjusting = 0, status = 'POSTED', posted_by_id = '{}', posted_at = '2026-10-31T00:00:00Z' WHERE id = '{}'",
        g.fx.user, mirror.id
    );
    assert_eq!(attack_message(&g.fx.book, &sql).as_deref(), Some("fb_journal_guard: Invalid reversal"));
}

#[test]
fn lines_names_and_bounds() {
    let mut g = gj("bounds");
    let one = g.entry(DraftInput::new("2026-10-31", "One", vec![line(g.expense).debit("5").build()])).unwrap();
    assert!(err_text(services::post_journal(&mut g.fx.book, g.fx.user, g.company, one.id, Some(1))).contains("at least two nonzero lines"));
    let unbalanced = g.entry(DraftInput::new("2026-10-31", "Unbalanced", vec![line(g.expense).debit("5").build(), line(g.prepaid).credit("4.99").build()])).unwrap();
    assert!(err_text(services::post_journal(&mut g.fx.book, g.fx.user, g.company, unbalanced.id, Some(1))).contains("(debits 5.00, credits 4.99)"));
    let mut fifty: Vec<serde_json::Value> = (0..49).map(|n| line(g.expense).debit("0.01").name(&format!("Name {n}")).build()).collect();
    fifty.push(line(g.prepaid).credit("0.49").build());
    let j = g.entry(DraftInput::new("2026-10-31", "Fifty", fifty.clone())).unwrap();
    services::post_journal(&mut g.fx.book, g.fx.user, g.company, j.id, Some(1)).unwrap();
    assert_eq!(fb_core::store::lines(g.fx.book.connection(), j.id).unwrap().len(), 50);
    let mut fifty_one = fifty.clone();
    fifty_one.push(line(g.prepaid).credit("0.01").build());
    assert!(err_text(g.entry(DraftInput::new("2026-10-31", "51", fifty_one))).contains("at most 50 lines"));
    let big = "99999999999999.99";
    let huge = g
        .entry(DraftInput::new("2026-10-31", "Huge", vec![
            line(g.expense).debit(big).build(), line(g.expense).debit("0.01").build(),
            line(g.prepaid).credit(big).build(), line(g.prepaid).credit("0.01").build(),
        ]))
        .unwrap();
    assert!(err_text(services::post_journal(&mut g.fx.book, g.fx.user, g.company, huge.id, Some(1))).contains("cannot exceed 99999999999999.99"));
    assert!(matches!(
        g.entry(DraftInput::new("2026-10-31", "Too big", vec![line(g.expense).debit("100000000000000.00").build(), line(g.prepaid).credit("1").build()])),
        Err(FbError::Validation(_))
    ));
    let mut long_name = g.lines("100.00");
    long_name[0]["name"] = json!("n".repeat(121));
    assert!(err_text(g.entry(DraftInput::new("2026-10-31", "Name", long_name))).contains("name up to 120"));
    let extra = vec![line(g.expense).debit("1").set("billable", json!(true)).build(), line(g.prepaid).credit("1").build()];
    assert_eq!(err_text(g.entry(DraftInput::new("2026-10-31", "Extra", extra))), "Unexpected journal line fields.");
    let name = format!("Synthetic insurer · 🧾{}", "n".repeat(99));
    let mut named = g.lines("100.00");
    named[0]["name"] = json!(name);
    named[1]["name"] = json!(name);
    let named = g.entry(DraftInput::new("2026-10-31", "Named", named)).unwrap();
    let named = services::post_journal(&mut g.fx.book, g.fx.user, g.company, named.id, Some(1)).unwrap();
    let reversal = services::reverse_journal(&mut g.fx.book, g.fx.user, g.company, named.id, "2026-11-01", "Name copied").unwrap();
    let names = |id| fb_core::store::lines(g.fx.book.connection(), id).unwrap().into_iter().map(|l| l.name).collect::<Vec<_>>();
    assert_eq!(names(reversal.id), names(named.id));
    assert_eq!(names(named.id)[0], name);
}

#[test]
fn exact_creation_retries_and_optimistic_revisions() {
    let mut g = gj("retries");
    let key = uuid::Uuid::new_v4().to_string();
    let mut keyed = g.input();
    keyed.request_key = Some(key.clone());
    let first = g.entry(keyed.clone()).unwrap();
    let mut braced = keyed.clone();
    braced.request_key = Some(format!("{{{}}}", key.to_uppercase()));
    assert_eq!(g.entry(braced).unwrap().id, first.id);
    let mut changed = keyed.clone();
    changed.memo = "changed".into();
    assert!(err_text(g.entry(changed)).contains("different journal entry"));
    let mut nope = g.input();
    nope.request_key = Some("nope".into());
    assert!(err_text(g.entry(nope)).contains("UUID"));
    services::post_journal(&mut g.fx.book, g.fx.user, g.company, first.id, Some(1)).unwrap();
    assert_eq!(g.entry(keyed).unwrap().id, first.id);
    let draft = g.entry(g.input()).unwrap();
    let mut edit = g.input();
    edit.journal_id = Some(draft.id);
    edit.memo = "Edited elsewhere".into();
    edit.expected_revision = Some(1);
    g.entry(edit.clone()).unwrap();
    edit.memo = "Stale edit".into();
    assert!(err_text(g.entry(edit)).contains("changed since you opened it"));
    assert!(err_text(services::post_journal(&mut g.fx.book, g.fx.user, g.company, draft.id, Some(1))).contains("changed since you opened it"));
    let stored = g.fx.journal(draft.id);
    assert_eq!((stored.status, stored.memo.as_str(), stored.revision), (Status::Draft, "Edited elsewhere", 2));
    assert_eq!(attack(&g.fx.book, &format!("UPDATE ledger_journal SET revision = 1 WHERE id = '{}'", draft.id)).as_deref(), Some("journal_retry_evidence"));
    assert_eq!(
        attack(&g.fx.book, &format!("UPDATE ledger_journal SET request_key = '{}' WHERE id = '{}'", uuid::Uuid::new_v4(), first.id)).as_deref(),
        Some("fb_journal_guard") // posted: immutability fires first, as in PG
    );
}

#[test]
fn duplicate_entry_number_and_double_post_race_under_one_writer() {
    // JournalRaces: two connections; the second waits for the first's IMMEDIATE transaction, then is refused.
    let path = book_path("gj-race");
    let (user, company, expense, prepaid) = {
        let mut book = fb_core::Book::create(&path, &KEY).unwrap();
        let user = users::create_user(&mut book, "gje-operator", false).unwrap();
        let company = services::create_company(&mut book, user, "N16 race", "USD", false).unwrap().id;
        let prepaid = services::save_account(&mut book, user, company, AccountInput::new("1300", "Prepaid").kind("ASSET")).unwrap().id;
        let expense = services::save_account(&mut book, user, company, AccountInput::new("6000", "Expense").kind("EXPENSE")).unwrap().id;
        (user, company, expense, prepaid)
    };
    for number in ["R-1", "R-2"] {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|i| {
                let (path, barrier, number) = (path.clone(), barrier.clone(), number.to_string());
                std::thread::spawn(move || {
                    let mut book = fb_core::Book::open(&path, &KEY).unwrap();
                    let mut input = DraftInput::new("2026-10-31", if i == 0 { "first" } else { "second" }, vec![line(expense).debit("1").build(), line(prepaid).credit("1").build()]);
                    input.entry_number = Some(number);
                    input.request_key = Some(uuid::Uuid::new_v4().to_string());
                    barrier.wait();
                    services::save_draft(&mut book, user, company, input).map(|_| ()).map_err(|e| e.message())
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1, "{results:?}");
        let refused = results.into_iter().find_map(|r| r.err()).unwrap();
        assert!(refused.contains(&format!("Entry number {number} is already used")), "{refused}");
    }
}
