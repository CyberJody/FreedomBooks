//! Independent review regressions. First run against 92985af is recorded in L1-FIX1 evidence.
mod common;
use common::*;
use fb_core::{
    AccountId,
    services::{self, AccountInput, DraftInput, line},
    users,
};
use fb_db::rusqlite::Connection;

/// The refusing guard of a raw statement; the refusal must exist and must not be 0130's `fb_valid`,
/// so a placeholder value can never mask the guard a test was written for.
fn refused_by(book: &fb_core::Book, label: &str, sql: &str) -> String {
    let guard = attack(book, sql).unwrap_or_else(|| panic!("attack accepted: {label}: {sql}"));
    assert!(!guard.starts_with("fb_valid"), "refused by fb_valid before its intended guard: {label}: {sql}");
    guard
}

fn replace_row(conn: &Connection, table: &str, predicate: &str, changes: &[(&str, String)]) -> String {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})")).unwrap();
    let cols: Vec<String> = stmt.query_map([], |r| r.get(1)).unwrap().collect::<Result<_, _>>().unwrap();
    let values: Vec<String> = cols.iter().map(|c| changes.iter().find(|(k, _)| k == c).map(|(_, v)| v.clone()).unwrap_or_else(|| c.clone())).collect();
    format!("INSERT OR REPLACE INTO {table} ({}) SELECT {} FROM {table} WHERE {predicate}", cols.join(","), values.join(","))
}

#[test]
fn review_replace_and_delete_reinsert_cannot_rewrite_protected_rows() {
    let mut fx = Fx::new("replace");
    let (c, a, b) = fx.chart();
    let j = fx.draft(c, a, b, "10", "2026-01-01");
    fx.post(c, j.id);
    fx.book.connection().execute_batch("INSERT INTO fb_seal VALUES ('test','x'); INSERT INTO fb_seal_due VALUES ('test','x');").unwrap();
    let mut accepted = Vec::new();
    let mut hit = std::collections::BTreeSet::new();
    for recursive in [true, false] {
        fx.book.connection().execute_batch(&format!("PRAGMA recursive_triggers={}", i32::from(recursive))).unwrap();
        let cases = vec![
            ("ledger_company", format!("id='{c}'"), vec![("currency", "'EUR'".into())]),
            ("ledger_account", format!("id='{a}'"), vec![("kind", "'INCOME'".into()), ("detail_type", "'INCOME'".into())]),
            (
                "ledger_account",
                format!("id='{a}'"),
                vec![("id", format!("'{}'", AccountId::new())), ("kind", "'INCOME'".into()), ("detail_type", "'INCOME'".into())],
            ),
            ("fb_user", format!("id='{}'", fx.user), vec![("username", "'rewritten'".into())]),
            ("ledger_journal", format!("id='{}'", j.id), vec![("status", "'DRAFT'".into()), ("posted_at", "NULL".into()), ("posted_by_id", "NULL".into())]),
            (
                "ledger_journal",
                format!("id='{}'", j.id),
                vec![
                    ("id", format!("'{}'", fb_core::JournalId::new())),
                    ("status", "'DRAFT'".into()),
                    ("posted_at", "NULL".into()),
                    ("posted_by_id", "NULL".into()),
                ],
            ),
            ("ledger_journalline", format!("journal_id='{}' AND debit>0", j.id), vec![("journal_id", format!("'{}'", fb_core::JournalId::new()))]),
            ("ledger_auditevent", "1=1".into(), vec![("detail", "'{\"rewritten\":true}'".into())]),
            ("fb_seal", "1=1".into(), vec![]),
            ("fb_seal_due", "1=1".into(), vec![]),
        ];
        for (table, predicate, changes) in cases {
            let sql = replace_row(fx.book.connection(), table, &predicate, &changes);
            match attack(&fx.book, &sql) {
                None => accepted.push(format!("REPLACE {table} recursive={recursive}")),
                Some(guard) => {
                    hit.insert(format!("REPLACE {table}: {guard}"));
                }
            }
            // Capture row before deletion, then try deleting and reinserting it in one transaction.
            let sql = format!(
                "CREATE TEMP TABLE saved AS SELECT * FROM {table} WHERE {predicate}; DELETE FROM {table} WHERE {predicate}; INSERT INTO {table} SELECT * FROM saved; DROP TABLE saved;"
            );
            match attack(&fx.book, &sql) {
                None => accepted.push(format!("DELETE/reinsert {table} recursive={recursive}")),
                Some(guard) => {
                    hit.insert(format!("DELETE/reinsert {table}: {guard}"));
                }
            }
        }
    }
    assert!(accepted.is_empty(), "accepted attacks: {accepted:?}");
    let expected: std::collections::BTreeSet<String> = [
        "DELETE/reinsert fb_seal: seal_immutable",
        "DELETE/reinsert fb_seal_due: seal_immutable",
        "DELETE/reinsert fb_user: fb_user_guard",
        "DELETE/reinsert ledger_account: fb_account_guard",
        "DELETE/reinsert ledger_auditevent: fb_audit_guard",
        "DELETE/reinsert ledger_company: fb_company_guard",
        "DELETE/reinsert ledger_journal: fb_journal_guard",
        "DELETE/reinsert ledger_journalline: fb_line_guard",
        "REPLACE fb_seal: seal_immutable",
        "REPLACE fb_seal_due: seal_immutable",
        "REPLACE fb_user: fb_user_guard",
        "REPLACE ledger_account: fb_account_guard",
        "REPLACE ledger_auditevent: fb_audit_guard",
        "REPLACE ledger_company: fb_company_guard",
        "REPLACE ledger_journal: fb_journal_guard",
        "REPLACE ledger_journalline: fb_line_guard",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(hit, expected, "refusing guards changed (no attack may be refused by fb_valid)");
}

#[test]
fn review_every_write_refuses_disabled_required_pragmas() {
    let mut fx = Fx::new("pragma");
    for pragma in ["recursive_triggers", "foreign_keys"] {
        fx.book.connection().execute_batch(&format!("PRAGMA {pragma}=OFF")).unwrap();
        let result: Result<(), fb_db::rusqlite::Error> = fx.book.write(|tx| {
            tx.execute("INSERT INTO fb_seal VALUES ('bad','bad')", [])?;
            Ok(())
        });
        fx.book.connection().execute_batch(&format!("PRAGMA {pragma}=ON")).unwrap();
        let message = result.expect_err("write accepted with a required pragma off").to_string();
        assert_eq!(message, format!("Required PRAGMA {pragma} is 0, expected 1"));
    }
}

#[test]
fn review_missing_parents_rejected_at_line_insert_and_posting() {
    let mut fx = Fx::new("parent");
    let (c, a, b) = fx.chart();
    let j = fx.draft(c, a, b, "10", "2026-01-01");
    let missing = AccountId::new();
    let sql = format!(
        "INSERT INTO ledger_journalline(id,journal_id,account_id,position,debit) VALUES ('{}','{}','{missing}',3,100)",
        fb_ids::JournalLineId::new(),
        j.id
    );
    assert_eq!(refused_by(&fx.book, "line on missing account", &sql), "fb_line_guard");
    // Tamper before posting as a schema owner, restore the guard, then posting must reject the absent account.
    let trigger: String = fx.book.connection().query_row("SELECT sql FROM sqlite_master WHERE name='fb_line_guard_update'", [], |r| r.get(0)).unwrap();
    fx.book.connection().execute_batch("PRAGMA foreign_keys=OFF; DROP TRIGGER fb_line_guard_update;").unwrap();
    fx.book.connection().execute("UPDATE ledger_journalline SET account_id=?1 WHERE debit>0", [missing.to_db()]).unwrap();
    fx.book.connection().execute_batch(&trigger).unwrap();
    let sql = format!("UPDATE ledger_journal SET status='POSTED',posted_by_id='{}',posted_at='2026-01-01T00:00:00Z' WHERE id='{}'", fx.user, j.id);
    assert_eq!(refused_by(&fx.book, "post with missing account", &sql), "fb_journal_guard");
}

#[test]
fn review_same_name_noop_trigger_is_detected() {
    let fx = Fx::new("definition");
    fx.book
        .connection()
        .execute_batch("DROP TRIGGER fb_audit_no_delete; CREATE TRIGGER fb_audit_no_delete BEFORE DELETE ON ledger_auditevent BEGIN SELECT 1; END;")
        .unwrap();
    assert!(!fx.book.check().unwrap().is_empty());
}

#[test]
fn review_open_detects_over_limit_total_and_bad_retry_evidence() {
    let path = book_path("over-limit-reopen");
    let mut book = fb_core::Book::create(&path, &KEY).unwrap();
    let user = users::create_user(&mut book, "tester", false).unwrap();
    let mut fx = Fx { book, user };
    let (c, a, b) = fx.chart();
    let j = fx.draft(c, a, b, "99999999999999.99", "2026-01-01");
    fx.book
        .connection()
        .execute(
            &format!(
                "INSERT INTO ledger_journalline(id,journal_id,account_id,position,debit) VALUES ('{}','{}','{a}',3,1)",
                fb_ids::JournalLineId::new(),
                j.id
            ),
            [],
        )
        .unwrap();
    fx.book
        .connection()
        .execute(
            &format!(
                "INSERT INTO ledger_journalline(id,journal_id,account_id,position,credit) VALUES ('{}','{}','{b}',4,1)",
                fb_ids::JournalLineId::new(),
                j.id
            ),
            [],
        )
        .unwrap();
    let trigger: String = fx.book.connection().query_row("SELECT sql FROM sqlite_master WHERE name='fb_journal_guard_update'", [], |r| r.get(0)).unwrap();
    fx.book.connection().execute_batch("DROP TRIGGER fb_journal_guard_update").unwrap();
    fx.book
        .connection()
        .execute(&format!("UPDATE ledger_journal SET status='POSTED',posted_by_id='{}',posted_at='2026-01-01T00:00:00Z' WHERE id='{}'", fx.user, j.id), [])
        .unwrap();
    fx.book.connection().execute_batch(&trigger).unwrap();
    assert!(fx.book.check().unwrap().iter().any(|p| p.contains("bounds")), "over-limit total not detected");
    drop(fx);
    assert!(matches!(fb_core::Book::open(&path, &KEY), Err(fb_db::DbError::Integrity(_))));
}

#[test]
fn review_retry_evidence_is_checked_on_open() {
    let path = book_path("retry-reopen");
    let mut book = fb_core::Book::create(&path, &KEY).unwrap();
    let user = users::create_user(&mut book, "tester", false).unwrap();
    let mut fx = Fx { book, user };
    let (c, a, b) = fx.chart();
    let j = fx.draft(c, a, b, "10", "2026-01-01");
    let trigger: String = fx.book.connection().query_row("SELECT sql FROM sqlite_master WHERE name='fb_journal_guard_update'", [], |r| r.get(0)).unwrap();
    fx.book.connection().execute_batch("DROP TRIGGER fb_journal_guard_update").unwrap();
    fx.book
        .connection()
        .execute(&format!("UPDATE ledger_journal SET request_key='00000000-0000-0000-0000-000000000000',creation_digest='' WHERE id='{}'", j.id), [])
        .unwrap();
    fx.book.connection().execute_batch(&trigger).unwrap();
    assert!(fx.book.check().unwrap().iter().any(|p| p.contains("retry")));
    drop(fx);
    assert!(matches!(fb_core::Book::open(&path, &KEY), Err(fb_db::DbError::Integrity(_))));
}

#[test]
fn review_autocommit_helper_does_not_partially_commit() {
    let mut fx = Fx::new("autocommit");
    let (c, a, b) = fx.chart();
    fx.book.connection().execute_batch("CREATE TRIGGER injected BEFORE INSERT ON ledger_auditevent BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    let input = DraftInput::new("2026-01-01", "Test", vec![line(a).debit("10").build(), line(b).credit("10").build()]);
    assert!(fx.book.write(|tx| services::save_draft_tx(tx, fx.user, c, &input, None)).is_err());
    assert_eq!(fx.count("SELECT count(*) FROM ledger_journal"), 0);
}

#[test]
fn review_revision_max_is_rejected_without_panic() {
    let mut fx = Fx::new("revision");
    let (c, a, b) = fx.chart();
    let j = fx.draft(c, a, b, "10", "2026-01-01");
    let sql = format!("UPDATE ledger_journal SET revision=9223372036854775807 WHERE id='{}'", j.id);
    assert_eq!(refused_by(&fx.book, "revision max", &sql), "journal_retry_evidence");
}

#[test]
fn review_date_formats_match_python() {
    for value in ["2026-W44-5", "2026W445", "2026-W44", "2026W44"] {
        assert!(fb_core::py::model_date(value).is_ok(), "{value}");
    }
    assert_eq!(fb_core::py::model_date("２０２６-１-５").unwrap().to_string(), "2026-01-05");
    for value in ["Oct 31 2026", "October 31, 2026", "31 Oct 2026", "31 October, 2026"] {
        assert_eq!(fb_core::py::form_date(value).unwrap().to_string(), "2026-10-31");
    }
}

#[test]
fn public_commands_rollback_on_audit_failure() {
    let mut fx = Fx::new("rollback");
    let (c, a, b) = fx.chart();
    users::set_active(&mut fx.book, fx.user, true).unwrap();
    let draft = fx.draft(c, a, b, "10", "2026-01-01");
    let posted = fx.draft(c, a, b, "20", "2026-01-01");
    fx.post(c, posted.id);
    fx.book.connection().execute("UPDATE fb_user SET is_staff=1", []).unwrap();
    let before = (
        fx.count("SELECT count(*) FROM ledger_journal"),
        fx.count("SELECT count(*) FROM ledger_journalline"),
        fx.count("SELECT count(*) FROM ledger_auditevent"),
    );
    fx.book.connection().execute_batch("CREATE TRIGGER injected BEFORE INSERT ON ledger_auditevent BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(services::create_company(&mut fx.book, fx.user, "New", "USD", false).is_err());
    assert!(services::edit_company(&mut fx.book, fx.user, c, "Changed", "USD").is_err());
    assert!(services::set_bill_reminder_days(&mut fx.book, fx.user, c, 7, Some(10)).is_err());
    assert!(services::save_account(&mut fx.book, fx.user, c, AccountInput::new("extra", "Extra").kind("ASSET")).is_err());
    assert!(services::save_account(&mut fx.book, fx.user, c, AccountInput::new("1000", "Changed").kind("ASSET").edit(a)).is_err());
    let input = DraftInput::new("2026-01-02", "Changed", vec![line(a).debit("30").build(), line(b).credit("30").build()]);
    assert!(services::save_draft(&mut fx.book, fx.user, c, input.clone()).is_err());
    assert!(services::save_draft(&mut fx.book, fx.user, c, DraftInput { journal_id: Some(draft.id), ..input }).is_err());
    assert!(services::post_journal(&mut fx.book, fx.user, c, draft.id, None).is_err());
    assert!(services::reverse_journal(&mut fx.book, fx.user, c, posted.id, "2026-01-02", "Reason").is_err());
    assert_eq!(
        before,
        (
            fx.count("SELECT count(*) FROM ledger_journal"),
            fx.count("SELECT count(*) FROM ledger_journalline"),
            fx.count("SELECT count(*) FROM ledger_auditevent")
        )
    );
    assert_eq!(fx.count("SELECT count(*) FROM ledger_company WHERE name='Changed' OR name='New'"), 0);
    assert_eq!(fx.count("SELECT count(*) FROM ledger_company WHERE bill_reminder_days<>10"), 0);
    assert_eq!(fx.count("SELECT count(*) FROM ledger_account WHERE name='Changed' OR code='extra'"), 0);
    assert_eq!(fx.journal(draft.id).revision, 1);
    assert_eq!(fx.journal(draft.id).status, fb_core::model::Status::Draft);
    assert_eq!(fx.line_amounts(draft.id, a).0, "10.00");
}

#[test]
fn revision_overflow_from_a_corrupt_book_is_a_controlled_error() {
    let mut fx = Fx::new("revision-corrupt");
    let (c, a, b) = fx.chart();
    let j = fx.draft(c, a, b, "10", "2026-01-01");
    let trigger: String = fx.book.connection().query_row("SELECT sql FROM sqlite_master WHERE name='fb_journal_guard_update'", [], |r| r.get(0)).unwrap();
    fx.book.connection().execute_batch("DROP TRIGGER fb_journal_guard_update").unwrap();
    fx.book.connection().execute("UPDATE ledger_journal SET revision=9223372036854775807 WHERE id=?1", [j.id.to_db()]).unwrap();
    fx.book.connection().execute_batch(&trigger).unwrap();
    let result = services::save_draft(&mut fx.book, fx.user, c, DraftInput { journal_id: Some(j.id), ..DraftInput::new("2026-01-02", "Edit", vec![]) });
    assert_eq!(result.unwrap_err().message(), "Journal revision limit reached.");
    assert!(fx.book.check().unwrap().iter().any(|p| p.contains("revision")));
}

#[test]
fn all_public_user_writes_rollback_on_injected_late_failure() {
    let mut fx = Fx::new("user-rollback");
    let (c, _, _) = fx.chart();
    let other = users::create_user(&mut fx.book, "other", false).unwrap();
    fx.book.connection().execute_batch("CREATE TRIGGER injected_user AFTER INSERT ON fb_user BEGIN SELECT RAISE(ABORT,'injected'); END; CREATE TRIGGER injected_active AFTER UPDATE ON fb_user BEGIN SELECT RAISE(ABORT,'injected'); END; CREATE TRIGGER injected_member AFTER INSERT ON ledger_company_members BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(users::create_user(&mut fx.book, "refused", false).is_err());
    assert!(users::set_active(&mut fx.book, fx.user, false).is_err());
    assert!(users::add_member(&mut fx.book, c, other).is_err());
    assert_eq!(fx.count("SELECT count(*) FROM fb_user WHERE username='refused' OR is_active=0"), 0);
    assert_eq!(fx.count("SELECT count(*) FROM ledger_company_members"), 1);
}

#[test]
fn typed_public_tx_helpers_rollback_with_their_writer() {
    let mut fx = Fx::new("typed-rollback");
    let (c, a, b) = fx.chart();
    let j = fx.draft(c, a, b, "10", "2026-01-01");
    let p = fx.draft(c, a, b, "20", "2026-01-01");
    fx.post(c, p.id);
    fx.book.connection().execute_batch("CREATE TRIGGER injected BEFORE INSERT ON ledger_auditevent BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(fx.book.write(|tx| services::post_journal_tx(tx, fx.user, c, j.id, None)).is_err());
    assert!(fx.book.write(|tx| services::reverse_journal_tx(tx, fx.user, c, p.id, "2026-01-02", "Test")).is_err());
    assert!(
        fx.book
            .write(|tx| {
                let actor = services::access(tx, fx.user, c)?;
                services::audit(tx, &actor, c, "test", "company:test".into(), serde_json::json!({}))
            })
            .is_err()
    );
    assert_eq!(fx.count("SELECT count(*) FROM ledger_journal"), 2);
    assert_eq!(fx.journal(j.id).status, fb_core::model::Status::Draft);
}

#[test]
fn trigger_creation_order_does_not_change_protections() {
    let mut fx = Fx::new("trigger-order");
    let conn = fx.book.connection();
    let triggers: Vec<(String, String)> = conn
        .prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger' ORDER BY rowid DESC")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for (name, _) in &triggers {
        conn.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
    }
    for (_, sql) in &triggers {
        conn.execute_batch(sql).unwrap();
    }
    let (c, a, b) = fx.chart();
    let j = fx.draft(c, a, b, "10", "2026-01-01");
    fx.post(c, j.id);
    let replace_company = format!("INSERT OR REPLACE INTO ledger_company SELECT * FROM ledger_company WHERE id='{c}'");
    assert_eq!(refused_by(&fx.book, "replace company", &replace_company), "fb_company_guard");
    let edit_posted = format!("UPDATE ledger_journal SET memo='changed' WHERE id='{}'", j.id);
    assert_eq!(refused_by(&fx.book, "edit posted", &edit_posted), "fb_journal_guard");
    let replace_audit = "INSERT OR REPLACE INTO ledger_auditevent SELECT * FROM ledger_auditevent LIMIT 1";
    assert_eq!(refused_by(&fx.book, "replace audit", replace_audit), "fb_audit_guard");
    services::reverse_journal(&mut fx.book, fx.user, c, j.id, "2026-01-01", "Void").unwrap();
    assert!(fb_core::reports::register(&mut fx.book, fx.user, c, a, None, None).unwrap().closing.is_zero());
    assert!(fx.book.check().unwrap().is_empty());
}

#[test]
fn every_alternate_unique_collision_is_refused_without_delete_trigger_help() {
    let mut fx = Fx::new("unique-collisions");
    let (c, a, b) = fx.chart();
    let posted = fx.draft(c, a, b, "10", "2026-01-01");
    fx.post(c, posted.id);
    let other = fx.draft(c, a, b, "20", "2026-01-01");
    let retry = services::save_draft(
        &mut fx.book,
        fx.user,
        c,
        DraftInput {
            request_key: Some("00000000-0000-4000-8000-000000000001".into()),
            ..DraftInput::new("2026-01-01", "Retry", vec![line(a).debit("1").build(), line(b).credit("1").build()])
        },
    )
    .unwrap();
    let reversal = services::reverse_journal(&mut fx.book, fx.user, c, posted.id, "2026-01-01", "Void").unwrap();
    let mut hit = std::collections::BTreeSet::new();
    for recursive in [true, false] {
        fx.book.connection().execute_batch(&format!("PRAGMA recursive_triggers={}", i32::from(recursive))).unwrap();
        let q = |id: String| format!("'{id}'");
        let cases = vec![
            ("fb_user", format!("id='{}'", fx.user), vec![("id", q(fb_core::UserId::new().to_string()))]),
            ("ledger_account", format!("id='{a}'"), vec![("id", q(AccountId::new().to_string()))]),
            // Isolate each journal unique key: other primary/alternate values are made fresh.
            ("ledger_journal", format!("id='{}'", other.id), vec![("id", q(fb_core::JournalId::new().to_string())), ("entry_number", "'fresh-number'".into())]),
            ("ledger_journal", format!("id='{}'", other.id), vec![("id", q(fb_core::JournalId::new().to_string())), ("number", "9001".into())]),
            (
                "ledger_journal",
                format!("id='{}'", retry.id),
                vec![("id", q(fb_core::JournalId::new().to_string())), ("number", "9002".into()), ("entry_number", "'fresh-retry'".into())],
            ),
            (
                "ledger_journal",
                format!("id='{}'", reversal.id),
                vec![
                    ("id", q(fb_core::JournalId::new().to_string())),
                    ("number", "9003".into()),
                    ("status", "'DRAFT'".into()),
                    ("posted_at", "NULL".into()),
                    ("posted_by_id", "NULL".into()),
                ],
            ),
            // A real eligible draft destination: without an old-key guard the posted line could move.
            ("ledger_journalline", format!("journal_id='{}' AND debit>0", posted.id), vec![("journal_id", q(other.id.to_string())), ("position", "3".into())]),
            ("ledger_journalline", format!("journal_id='{}' AND debit>0", other.id), vec![("id", q(fb_ids::JournalLineId::new().to_string()))]),
        ];
        for (n, (table, predicate, changes)) in cases.into_iter().enumerate() {
            let sql = replace_row(fx.book.connection(), table, &predicate, &changes);
            let guard = refused_by(&fx.book, &format!("{table} recursive={recursive}"), &sql);
            hit.insert(format!("{n} {table}: {guard}"));
        }
    }
    let expected: std::collections::BTreeSet<String> = [
        "0 fb_user: fb_user_guard",
        "1 ledger_account: fb_account_guard",
        "2 ledger_journal: fb_journal_guard",
        "3 ledger_journal: fb_journal_guard",
        "4 ledger_journal: fb_journal_guard",
        "5 ledger_journal: fb_journal_guard",
        "6 ledger_journalline: fb_line_guard",
        "7 ledger_journalline: fb_line_guard",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(hit, expected, "refusing guards changed");
}

#[test]
fn missing_parent_ordering_cannot_commit_ineligible_or_foreign_accounts() {
    let mut fx = Fx::new("late-parent-profiles");
    let (c, _, _) = fx.chart();
    let other = fx.company("Other");
    for (company, active, postable) in [(c, 0, 1), (c, 1, 0), (other, 1, 1)] {
        let j = fb_core::JournalId::new();
        let a = AccountId::new();
        let b = AccountId::new();
        let sql=format!("INSERT INTO ledger_journal(id,number,company_id,date,memo,created_by_id,created_at,entry_number) VALUES('{j}',9000,'{c}','2026-01-01','Late','{}','2026-01-01T00:00:00Z','late');
          INSERT INTO ledger_journalline(id,journal_id,account_id,position,debit,credit) VALUES('{}','{j}','{a}',1,100,0),('{}','{j}','{b}',2,0,100);
          UPDATE ledger_journal SET status='POSTED',posted_by_id='{}',posted_at='2026-01-01T00:00:00Z' WHERE id='{j}';
          INSERT INTO ledger_account(id,company_id,code,name,kind,active,postable) VALUES('{a}','{company}','late-a','Late A','ASSET',{active},{postable}),('{b}','{company}','late-b','Late B','EQUITY',{active},{postable});",fx.user,fb_ids::JournalLineId::new(),fb_ids::JournalLineId::new(),fx.user);
        assert_eq!(attack(&fx.book, &sql).as_deref(), Some("fb_line_guard"));
    }
    assert_eq!(fx.count("SELECT count(*) FROM ledger_journal"), 0);
}

#[test]
fn reversal_rolls_back_even_when_only_its_final_audit_fails() {
    let mut fx = Fx::new("late-reversal-audit");
    let (c, a, b) = fx.chart();
    let original = fx.draft(c, a, b, "10", "2026-01-01");
    fx.post(c, original.id);
    let audits = fx.count("SELECT count(*) FROM ledger_auditevent");
    fx.book.connection().execute_batch("CREATE TRIGGER injected_final BEFORE INSERT ON ledger_auditevent WHEN NEW.action='journal.reversed' BEGIN SELECT RAISE(ABORT,'injected final audit'); END;").unwrap();
    assert!(services::reverse_journal(&mut fx.book, fx.user, c, original.id, "2026-01-02", "Test").is_err());
    assert_eq!(fx.count("SELECT count(*) FROM ledger_journal"), 1);
    assert_eq!(fx.count("SELECT count(*) FROM ledger_journalline"), 2);
    assert_eq!(fx.count("SELECT count(*) FROM ledger_auditevent"), audits);
    assert_eq!(fx.journal(original.id).status, fb_core::model::Status::Posted);
    assert_eq!(fx.line_amounts(original.id, a).0, "10.00");
}
