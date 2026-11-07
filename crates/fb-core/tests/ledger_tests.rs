//! Translation of ledger/tests.py (LedgerTests and ConcurrentPostingTests). The HTTP classes
//! (WorkflowTests, BusinessConflictTests' HTTP paths, PreviewTests) belong to the window layer.
mod common;

use common::*;
use fb_core::model::{Side, Status};
use fb_core::services::{self, AccountInput, DraftInput, line};
use fb_core::{FbError, reports, users};
use serde_json::json;

#[test]
fn company_chart_and_normal_balances() {
    let mut fx = Fx::new("chart");
    let company = services::create_company(&mut fx.book, fx.user, "Synthetic Workshop", "USD", false).unwrap();
    assert!(fb_core::store::is_member(fx.book.connection(), company.id, fx.user).unwrap());
    for (kind, normal) in [("ASSET", Side::Debit), ("LIABILITY", Side::Credit), ("EQUITY", Side::Credit), ("INCOME", Side::Credit), ("EXPENSE", Side::Debit)] {
        let account = services::save_account(&mut fx.book, fx.user, company.id, AccountInput::new(kind, kind).kind(kind)).unwrap();
        assert_eq!(account.normal_balance(), normal);
    }
    assert_eq!(fx.count("SELECT count(*) FROM ledger_auditevent"), 6);
    let actor: String = fx.book.connection().query_row("SELECT actor_id FROM ledger_auditevent ORDER BY id LIMIT 1", [], |r| r.get(0)).unwrap();
    assert_eq!(actor, fx.user.to_string());
}

#[test]
fn draft_then_atomic_balanced_post_with_half_up_and_audit() {
    let mut fx = Fx::new("post");
    let (company, cash, equity) = fx.chart();
    let journal = fx.draft(company, cash, equity, "125.005", "2026-01-01");
    assert_eq!(journal.status, Status::Draft);
    assert_eq!(fx.line_amounts(journal.id, cash).0, "125.01");
    let journal = fx.post(company, journal.id);
    assert_eq!(journal.status, Status::Posted);
    assert_eq!(journal.posted_by_id, Some(fx.user));
    assert!(journal.posted_at.is_some());
    let (target, detail): (String, String) = fx
        .book
        .connection()
        .query_row("SELECT target, detail FROM ledger_auditevent WHERE action = 'journal.posted'", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(target, format!("journal:{}", journal.id));
    let detail: serde_json::Value = serde_json::from_str(&detail).unwrap();
    assert_eq!(detail["debit"], "125.01");
}

#[test]
fn draft_rejects_invalid_accounts_and_rolls_back_edits() {
    let mut fx = Fx::new("invalid");
    let (company, cash, equity) = fx.chart();
    let draft = fx.draft(company, cash, equity, "125.005", "2026-01-01");
    let other = fx.company("Other synthetic");
    let foreign = fx.account(other, "1", "Foreign", "ASSET");
    let inactive = services::save_account(&mut fx.book, fx.user, company, AccountInput::new("2", "Inactive").kind("ASSET").active(false)).unwrap().id;
    let summary = services::save_account(&mut fx.book, fx.user, company, AccountInput::new("3", "Summary").kind("ASSET").postable(false)).unwrap().id;
    for invalid in [foreign.to_string(), inactive.to_string(), summary.to_string(), "999999".into(), fb_core::AccountId::new().to_string()] {
        let mut input = DraftInput::new("2026-01-02", "Bad edit", vec![line(cash).debit("50").credit("0").build(), json!({"account": invalid, "credit": "50", "debit": "0"})]);
        input.journal_id = Some(draft.id);
        let result = services::save_draft(&mut fx.book, fx.user, company, input);
        assert!(matches!(result, Err(FbError::Validation(_))), "{invalid}");
        let stored = fx.journal(draft.id);
        assert_eq!(stored.memo, "Synthetic contribution");
        assert_eq!(fx.line_amounts(draft.id, cash).0, "125.01");
    }
}

#[test]
fn unbalanced_and_missing_lines_never_partially_post() {
    let mut fx = Fx::new("unbalanced");
    let (company, cash, equity) = fx.chart();
    for lines in [vec![], vec![line(cash).debit("2").build()], vec![line(cash).debit("2").build(), line(equity).credit("1").build()]] {
        let draft = services::save_draft(&mut fx.book, fx.user, company, DraftInput::new("2026-01-01", "Incomplete", lines)).unwrap();
        assert!(matches!(services::post_journal(&mut fx.book, fx.user, company, draft.id, None), Err(FbError::Validation(_))));
        let stored = fx.journal(draft.id);
        assert_eq!(stored.status, Status::Draft);
        assert!(stored.posted_at.is_none());
        let posted: i64 = fx
            .book
            .connection()
            .query_row("SELECT count(*) FROM ledger_auditevent WHERE action = 'journal.posted' AND target = ?1", [format!("journal:{}", draft.id)], |r| r.get(0))
            .unwrap();
        assert_eq!(posted, 0);
    }
}

#[test]
fn post_revalidates_account_status() {
    let mut fx = Fx::new("revalidate");
    let (company, cash, equity) = fx.chart();
    let draft = fx.draft(company, cash, equity, "125.005", "2026-01-01");
    // `cash.active = False; cash.save()` (a direct model save, no service).
    fx.book.connection().execute("UPDATE ledger_account SET active = 0 WHERE id = ?1", [cash.to_db()]).unwrap();
    let message = err_text(services::post_journal(&mut fx.book, fx.user, company, draft.id, None));
    assert_eq!(message, "Every line must use an active posting account in this company.");
}

#[test]
fn company_write_access_and_identified_actor() {
    let mut fx = Fx::new("access");
    let (company, cash, _) = fx.chart();
    let stranger = users::create_user(&mut fx.book, "stranger", false).unwrap();
    assert!(matches!(
        services::save_account(&mut fx.book, stranger, company, AccountInput::new("x", "x").kind("ASSET")),
        Err(FbError::PermissionDenied(m)) if m == "Company access denied."
    ));
    // AnonymousUser: no operator row.
    assert!(matches!(
        services::create_company(&mut fx.book, fb_core::UserId::new(), "Anonymous", "USD", false),
        Err(FbError::PermissionDenied(m)) if m == "An identified active operator is required."
    ));
    let other = fx.company("Other");
    assert_eq!(
        err_text(services::save_account(&mut fx.book, fx.user, other, AccountInput::new("x", "x").kind("ASSET").edit(cash))),
        "Account belongs to another company."
    );
    // An inactive operator is refused too.
    users::set_active(&mut fx.book, stranger, false).unwrap();
    assert!(matches!(services::create_company(&mut fx.book, stranger, "Inactive", "USD", false), Err(FbError::PermissionDenied(_))));
}

#[test]
fn line_requires_exactly_one_positive_side() {
    let mut fx = Fx::new("oneside");
    let (company, cash, _) = fx.chart();
    for (debit, credit) in [("0", "0"), ("1", "1"), ("0.001", "0"), ("-1", "0")] {
        let result = services::save_draft(&mut fx.book, fx.user, company, DraftInput::new("2026-01-01", "Invalid", vec![line(cash).debit(debit).credit(credit).build()]));
        assert!(matches!(result, Err(FbError::Validation(_))), "{debit}/{credit}");
    }
    assert_eq!(fx.count("SELECT count(*) FROM ledger_journal"), 0);
}

#[test]
fn posted_history_protected_through_every_raw_path() {
    let mut fx = Fx::new("history");
    let (company, cash, equity) = fx.chart();
    let draft = fx.draft(company, cash, equity, "125.005", "2026-01-01");
    let journal = fx.post(company, draft.id);
    let j = journal.id.to_db();
    let line_id: String = fx.book.connection().query_row("SELECT id FROM ledger_journalline WHERE journal_id = ?1 ORDER BY position LIMIT 1", [&j], |r| r.get(0)).unwrap();
    let operations = [
        format!("UPDATE ledger_journalline SET debit = 900 WHERE id = '{line_id}'"),
        format!("UPDATE ledger_journal SET memo = 'Changed' WHERE id = '{j}'"),
        format!("UPDATE ledger_journalline SET debit = 99900 WHERE journal_id = '{j}'"),
        format!("DELETE FROM ledger_journalline WHERE id = '{line_id}'"),
        format!("DELETE FROM ledger_journalline WHERE journal_id = '{j}'"),
        format!("DELETE FROM ledger_journal WHERE id = '{j}'"),
        format!("UPDATE ledger_journal SET status = 'DRAFT' WHERE id = '{j}'"),
        format!("INSERT INTO ledger_journalline (id, journal_id, account_id, position, debit, credit) VALUES ('{}', '{j}', '{}', 9, 100, 0)", fb_ids::JournalLineId::new(), cash),
        format!("INSERT OR REPLACE INTO ledger_journalline (id, journal_id, account_id, position, debit, credit) VALUES ('{line_id}', '{j}', '{cash}', 1, 1, 0)"),
        format!("UPDATE ledger_account SET kind = 'INCOME', detail_type = 'INCOME' WHERE id = '{cash}'"),
        "UPDATE ledger_auditevent SET action = 'concealed'".to_string(),
        "DELETE FROM ledger_auditevent".to_string(),
        "INSERT OR REPLACE INTO ledger_auditevent SELECT id, company_id, actor_id, actor_name, occurred_at, 'concealed', target, detail FROM ledger_auditevent".to_string(),
    ];
    for (index, sql) in operations.iter().enumerate() {
        assert!(attack(&fx.book, sql).is_some(), "operation {index} was accepted: {sql}");
    }
    let stored = fx.journal(journal.id);
    assert_eq!(stored.memo, "Synthetic contribution");
    assert_eq!(fx.line_amounts(journal.id, cash).0, "125.01");
}

#[test]
fn database_rejects_cross_company_line_and_unbalanced_direct_post() {
    let mut fx = Fx::new("crossco");
    let (company, cash, equity) = fx.chart();
    let draft = fx.draft(company, cash, equity, "125.005", "2026-01-01");
    let other = fx.company("Other");
    let foreign = fx.account(other, "f", "Foreign", "ASSET");
    let d = draft.id.to_db();
    assert_eq!(
        attack(&fx.book, &format!("UPDATE ledger_journalline SET account_id = '{foreign}' WHERE journal_id = '{d}' AND account_id = '{cash}'")).as_deref(),
        Some("fb_line_guard")
    );
    fx.book.connection().execute(&format!("UPDATE ledger_journalline SET debit = 100 WHERE journal_id = '{d}' AND account_id = '{cash}'"), []).unwrap();
    let post = format!("UPDATE ledger_journal SET status = 'POSTED', posted_by_id = '{}', posted_at = '2026-01-01T00:00:00Z' WHERE id = '{d}'", fx.user);
    assert_eq!(attack(&fx.book, &post).as_deref(), Some("fb_journal_guard"));
}

#[test]
fn reversal_is_linked_exact_compensation_preserving_original_and_audit() {
    let mut fx = Fx::new("reversal");
    let (company, cash, equity) = fx.chart();
    let draft = fx.draft(company, cash, equity, "125.005", "2026-01-01");
    let original = fx.post(company, draft.id);
    let reversal = services::reverse_journal(&mut fx.book, fx.user, company, original.id, "2026-02-01", "Synthetic correction").unwrap();
    assert_eq!(reversal.reversal_of_id, Some(original.id));
    assert_eq!(reversal.status, Status::Posted);
    assert_eq!(fx.line_amounts(reversal.id, cash).1, "125.01");
    assert_eq!(reversal.memo, format!("Reversal of {}: Synthetic correction", original.display()));
    let stored = fx.journal(original.id);
    assert_eq!(stored.status, Status::Posted);
    assert_eq!(fx.line_amounts(original.id, cash).0, "125.01");
    let (actor_name, detail): (String, String) = fx
        .book
        .connection()
        .query_row("SELECT actor_name, detail FROM ledger_auditevent WHERE action = 'journal.reversed'", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(actor_name, "tester");
    let detail: serde_json::Value = serde_json::from_str(&detail).unwrap();
    assert_eq!(detail["reversal_id"], reversal.id.to_string());
    assert!(matches!(services::reverse_journal(&mut fx.book, fx.user, company, original.id, "2026-02-02", "Duplicate"), Err(FbError::Validation(_))));
    assert_eq!(fx.count("SELECT count(*) FROM ledger_journal"), 2);
}

#[test]
fn reversal_rejects_drafts_earlier_dates_and_empty_reason() {
    let mut fx = Fx::new("reversal-bad");
    let (company, cash, equity) = fx.chart();
    let draft = fx.draft(company, cash, equity, "125.005", "2026-01-01");
    assert_eq!(
        err_text(services::reverse_journal(&mut fx.book, fx.user, company, draft.id, "2026-01-02", "Draft")),
        "Only an unreversed original posted journal can be reversed."
    );
    fx.post(company, draft.id);
    for (date, reason) in [("2025-12-31", "Early"), ("2026-02-01", "  ")] {
        assert_eq!(
            err_text(services::reverse_journal(&mut fx.book, fx.user, company, draft.id, date, reason)),
            "Supply a reason and a reversal date on or after the original date."
        );
    }
    assert_eq!(fx.count("SELECT count(*) FROM ledger_journal"), 1);
}

#[test]
fn register_opening_balance_order_dates_and_credit_normal() {
    let mut fx = Fx::new("register");
    let (company, cash, equity) = fx.chart();
    let first = fx.draft(company, cash, equity, "100", "2026-01-01");
    fx.post(company, first.id);
    let second = fx.draft(company, cash, equity, "20", "2026-02-01");
    fx.post(company, second.id);
    let third = fx.draft(company, cash, equity, "30", "2026-02-01");
    fx.post(company, third.id);
    fx.draft(company, cash, equity, "999", "2026-02-01");
    let later = fx.draft(company, cash, equity, "999", "2026-03-01");
    fx.post(company, later.id);
    for account in [cash, equity] {
        let result = reports::register(&mut fx.book, fx.user, company, account, Some("2026-02-01"), Some("2026-02-28")).unwrap();
        assert_eq!(result.opening.to_string(), "100.00");
        assert_eq!(result.rows.iter().map(|r| r.journal_id).collect::<Vec<_>>(), vec![second.id, third.id]);
        assert_eq!(result.rows.iter().map(|r| r.balance.to_string()).collect::<Vec<_>>(), vec!["120.00", "150.00"]);
        assert_eq!(result.closing.to_string(), "150.00");
    }
    let empty = reports::register(&mut fx.book, fx.user, company, cash, Some("2026-01-02"), Some("2026-01-31")).unwrap();
    assert!(empty.rows.is_empty());
    assert_eq!(empty.closing.to_string(), "100.00");
}

#[test]
fn trial_balance_as_of_reversals_drafts_and_company_scoping() {
    let mut fx = Fx::new("tb");
    let (company, cash, equity) = fx.chart();
    let draft = fx.draft(company, cash, equity, "100", "2026-01-01");
    let journal = fx.post(company, draft.id);
    fx.draft(company, cash, equity, "999", "2026-01-01");
    services::reverse_journal(&mut fx.book, fx.user, company, journal.id, "2026-02-01", "Correction").unwrap();
    let before = reports::trial_balance(&mut fx.book, fx.user, company, "2025-12-31").unwrap();
    assert_eq!(before.debit.to_string(), "0.00");
    let during = reports::trial_balance(&mut fx.book, fx.user, company, "2026-01-01").unwrap();
    assert_eq!((during.debit.to_string(), during.credit.to_string()), ("100.00".into(), "100.00".into()));
    let after = reports::trial_balance(&mut fx.book, fx.user, company, "2026-02-01").unwrap();
    assert_eq!((after.debit.to_string(), after.credit.to_string()), ("0.00".into(), "0.00".into()));
    let other = fx.company("Empty company");
    assert!(reports::trial_balance(&mut fx.book, fx.user, other, "2026-01-01").unwrap().rows.is_empty());
    assert!(matches!(reports::register(&mut fx.book, fx.user, other, cash, None, None), Err(FbError::DoesNotExist(_))));
    let stranger = users::create_user(&mut fx.book, "report-stranger", false).unwrap();
    assert!(matches!(reports::trial_balance(&mut fx.book, stranger, company, "2026-01-01"), Err(FbError::PermissionDenied(_))));
    assert_eq!(
        err_text(reports::register(&mut fx.book, fx.user, company, cash, Some("2026-02-01"), Some("2026-01-01"))),
        "Start date must not be after end date."
    );
}

#[test]
fn draft_edit_audit_keeps_before_after_line_amounts() {
    let mut fx = Fx::new("draft-audit");
    let (company, cash, equity) = fx.chart();
    let journal = fx.draft(company, cash, equity, "10", "2026-01-01");
    let mut input = DraftInput::new("2026-01-02", "Changed", vec![line(cash).debit("20").build(), line(equity).credit("20").build()]);
    input.journal_id = Some(journal.id);
    services::save_draft(&mut fx.book, fx.user, company, input).unwrap();
    let detail: String = fx
        .book
        .connection()
        .query_row("SELECT detail FROM ledger_auditevent WHERE action = 'journal.drafted' AND target = ?1 ORDER BY id DESC LIMIT 1", [format!("journal:{}", journal.id)], |r| r.get(0))
        .unwrap();
    let detail: serde_json::Value = serde_json::from_str(&detail).unwrap();
    assert_eq!(detail["before"]["lines"][0]["debit"], "10.00");
    assert_eq!(detail["after"]["lines"][0]["debit"], "20.00");
    assert_eq!(detail["after"]["lines"][0]["account_id"], cash.to_string());
}

#[test]
fn currency_and_used_account_type_edits_are_rejected_cleanly() {
    let mut fx = Fx::new("currency");
    let (company, cash, equity) = fx.chart();
    fx.draft(company, cash, equity, "1", "2026-01-01");
    assert!(matches!(services::edit_company(&mut fx.book, fx.user, company, "Same books", "EUR"), Err(FbError::Validation(_))));
    assert!(matches!(
        services::save_account(&mut fx.book, fx.user, company, AccountInput::new("1000", "Cash").kind("INCOME").edit(cash)),
        Err(FbError::Validation(_))
    ));
    let stored = fb_core::store::get_company(fx.book.connection(), company).unwrap();
    assert_eq!(stored.currency, "USD");
    assert_eq!(fb_core::store::account(fx.book.connection(), cash).unwrap().unwrap().kind.as_str(), "ASSET");
}

#[test]
fn currency_validation_is_consistent_at_service_boundary() {
    let mut fx = Fx::new("currency-bad");
    for currency in ["xx", "12$", "ééé"] {
        assert!(matches!(services::create_company(&mut fx.book, fx.user, "Invalid currency", currency, false), Err(FbError::Validation(_))), "{currency}");
    }
}

#[test]
fn database_reversal_must_exactly_compensate_original() {
    let mut fx = Fx::new("db-reversal");
    let (company, cash, equity) = fx.chart();
    let d = fx.draft(company, cash, equity, "10", "2026-01-01");
    let original = fx.post(company, d.id);
    // Reversal identity is fixed at INSERT; retain the independent posting-total check.
    let wrong = fx.book.write(|tx| services::save_draft_tx(
        tx, fx.user, company,
        &DraftInput::new("2026-01-02", "Wrong reversal", vec![line(cash).credit("11").build(), line(equity).debit("11").build()]),
        Some(&original),
    )).unwrap();
    let sql = format!(
        "UPDATE ledger_journal SET status = 'POSTED', posted_by_id = '{}', posted_at = '2026-01-02T00:00:00Z' WHERE id = '{}'",
        fx.user, wrong.id
    );
    assert_eq!(attack_message(&fx.book, &sql).as_deref(), Some("fb_journal_guard: Reversal must exactly compensate original lines"));
}

#[test]
fn database_prevents_relabelling_currency_of_recorded_books() {
    let mut fx = Fx::new("db-currency");
    let (company, cash, equity) = fx.chart();
    let d = fx.draft(company, cash, equity, "1", "2026-01-01");
    fx.post(company, d.id);
    assert_eq!(attack(&fx.book, &format!("UPDATE ledger_company SET currency = 'EUR' WHERE id = '{company}'")).as_deref(), Some("fb_company_guard"));
}

#[test]
fn post_rolls_back_if_audit_write_fails_after_status_update() {
    // Python patches ledger.services.audit to raise after the real status update. Here the audit insert is
    // made to fail at the database: a temporary trigger refuses journal.posted events on this connection.
    let mut fx = Fx::new("audit-fail");
    let (company, cash, equity) = fx.chart();
    let draft = fx.draft(company, cash, equity, "125.005", "2026-01-01");
    fx.book
        .connection()
        .execute_batch("CREATE TEMP TRIGGER fail_audit BEFORE INSERT ON main.ledger_auditevent WHEN NEW.action = 'journal.posted' BEGIN SELECT RAISE(ABORT, 'injected: audit failure'); END;")
        .unwrap();
    assert!(services::post_journal(&mut fx.book, fx.user, company, draft.id, None).is_err());
    fx.book.connection().execute_batch("DROP TRIGGER temp.fail_audit").unwrap();
    let stored = fx.journal(draft.id);
    assert_eq!(stored.status, Status::Draft);
    assert!(stored.posted_at.is_none());
    assert!(stored.posted_by_id.is_none());
    assert_eq!(fx.audit_count("journal.posted"), 0);
    assert_eq!(fb_core::store::lines(fx.book.connection(), draft.id).unwrap().len(), 2);
}

#[test]
fn debit_normal_account_can_have_credit_balance() {
    let mut fx = Fx::new("overdraw");
    let (company, cash, equity) = fx.chart();
    let draft = services::save_draft(
        &mut fx.book,
        fx.user,
        company,
        DraftInput::new("2026-01-01", "Synthetic overdraw", vec![line(cash).credit("5").build(), line(equity).debit("5").build()]),
    )
    .unwrap();
    fx.post(company, draft.id);
    assert_eq!(reports::register(&mut fx.book, fx.user, company, cash, None, None).unwrap().closing.to_string(), "-5.00");
    let balance = reports::trial_balance(&mut fx.book, fx.user, company, "2026-01-01").unwrap();
    let row = balance.rows.iter().find(|r| r.account.id == cash).unwrap();
    assert_eq!((row.debit.to_string(), row.credit.to_string()), ("0.00".into(), "5.00".into()));
}

/// ConcurrentPostingTests: two threads, two connections, one writer. One posts, the other is refused.
fn race(path: std::path::PathBuf, op: fn(&mut fb_core::Book, fb_core::UserId, fb_core::CompanyId, fb_core::JournalId) -> fb_core::FbResult<()>, user: fb_core::UserId, company: fb_core::CompanyId, journal: fb_core::JournalId) -> Vec<&'static str> {
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut book = fb_core::Book::open(&path, &KEY).unwrap();
                barrier.wait();
                match op(&mut book, user, company, journal) {
                    Ok(()) => "success",
                    Err(FbError::Validation(_)) => "rejected",
                    Err(e) => panic!("unexpected {e}"),
                }
            })
        })
        .collect();
    let mut results: Vec<&str> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    results.sort();
    results
}

#[test]
fn concurrent_post_and_reversal_are_serialized() {
    let path = book_path("race");
    let (user, company, journal) = {
        let mut book = fb_core::Book::create(&path, &KEY).unwrap();
        let user = users::create_user(&mut book, "concurrent-operator", false).unwrap();
        let company = services::create_company(&mut book, user, "Concurrency synthetic", "USD", false).unwrap().id;
        let cash = services::save_account(&mut book, user, company, AccountInput::new("1", "Cash").kind("ASSET")).unwrap().id;
        let equity = services::save_account(&mut book, user, company, AccountInput::new("3", "Equity").kind("EQUITY")).unwrap().id;
        let journal = services::save_draft(&mut book, user, company, DraftInput::new("2026-01-01", "Race", vec![line(cash).debit("10").build(), line(equity).credit("10").build()])).unwrap().id;
        (user, company, journal)
    };
    let results = race(path.clone(), |b, u, c, j| services::post_journal(b, u, c, j, None).map(|_| ()), user, company, journal);
    assert_eq!(results, vec!["rejected", "success"]);
    let results = race(path.clone(), |b, u, c, j| services::reverse_journal(b, u, c, j, "2026-02-01", "Race").map(|_| ()), user, company, journal);
    assert_eq!(results, vec!["rejected", "success"]);
    let book = fb_core::Book::open(&path, &KEY).unwrap();
    let count = |sql: &str| -> i64 { book.connection().query_row(sql, [], |r| r.get(0)).unwrap() };
    assert_eq!(count("SELECT count(*) FROM ledger_auditevent WHERE action = 'journal.posted'"), 2); // original + reversal
    assert_eq!(count("SELECT count(*) FROM ledger_auditevent WHERE action = 'journal.reversed'"), 1);
    assert_eq!(count("SELECT count(*) FROM ledger_journal WHERE reversal_of_id IS NOT NULL"), 1);
}

#[test]
fn duplicate_account_code_is_a_clean_validation_error() {
    // BusinessConflictTests.test_duplicate_account_http_conflict_after_both_validate, minus HTTP: under one
    // writer the second save sees the first and is refused by full_clean's unique check.
    let mut fx = Fx::new("dup-code");
    let company = fx.company("Conflict synthetic");
    services::save_account(&mut fx.book, fx.user, company, AccountInput::new("RACE", "Concurrent cash").detail("BANK")).unwrap();
    let message = err_text(services::save_account(&mut fx.book, fx.user, company, AccountInput::new("RACE", "Concurrent cash").detail("BANK")));
    assert!(message.contains("already exists"), "{message}");
    assert_eq!(fx.audit_count("account.saved"), 1);
}
