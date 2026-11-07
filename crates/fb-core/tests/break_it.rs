//! D2's "try to break it" suite for the ledger family: raw SQL on a core-opened connection must be
//! refused by the database even where a Rust path could forget a check. Every RAISE message in the
//! ledger schema must be produced by at least one attack here (checked at the end).
mod common;

use common::*;
use fb_core::services::{self, AccountInput, DraftInput, line};
use std::collections::BTreeSet;

struct World {
    fx: Fx,
    company: fb_core::CompanyId,
    other: fb_core::CompanyId,
    cash: fb_core::AccountId,
    equity: fb_core::AccountId,
    foreign: fb_core::AccountId,
    inactive: fb_core::AccountId,
    posted: fb_core::model::Journal,
    draft: fb_core::model::Journal,
    reversal: fb_core::model::Journal,
}

fn world() -> World {
    let mut fx = Fx::new("break");
    let (company, cash, equity) = fx.chart();
    let other = fx.company("Other");
    let foreign = fx.account(other, "9", "Foreign", "ASSET");
    let inactive = services::save_account(&mut fx.book, fx.user, company, AccountInput::new("8", "Inactive").kind("ASSET").active(false)).unwrap().id;
    let p = fx.draft(company, cash, equity, "10", "2026-01-05");
    let posted = fx.post(company, p.id);
    let r = fx.draft(company, cash, equity, "7", "2026-01-06");
    let r = fx.post(company, r.id);
    let reversal = services::reverse_journal(&mut fx.book, fx.user, company, r.id, "2026-01-07", "Break test").unwrap();
    let draft = fx.draft(company, cash, equity, "3", "2026-01-08");
    World { fx, company, other, cash, equity, foreign, inactive, posted, draft, reversal }
}

fn new_journal_sql(w: &World, extra_cols: &str, extra_vals: &str) -> String {
    format!(
        "INSERT INTO ledger_journal (id, number, company_id, date, memo, status, created_by_id, created_at{extra_cols})
         VALUES ('{}', 9000 + (SELECT count(*) FROM ledger_journal), '{}', '2026-02-01', 'Raw', 'DRAFT', '{}', '2026-02-01T00:00:00Z'{extra_vals})",
        fb_core::JournalId::new(),
        w.company,
        w.fx.user
    )
}

fn line_sql(journal: impl std::fmt::Display, account: impl std::fmt::Display, position: i64, debit: i64, credit: i64) -> String {
    format!(
        "INSERT INTO ledger_journalline (id, journal_id, account_id, position, debit, credit) VALUES ('{}', '{journal}', '{account}', {position}, {debit}, {credit})",
        fb_ids::JournalLineId::new()
    )
}

fn post_sql(journal: impl std::fmt::Display, user: impl std::fmt::Display) -> String {
    format!("UPDATE ledger_journal SET status = 'POSTED', posted_by_id = '{user}', posted_at = '2026-02-01T00:00:00Z' WHERE id = '{journal}'")
}

/// (label, sql) attacks; each must be refused. Returns the set of refusal messages seen.
fn run(w: &World, attacks: &[(&str, String)]) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    for (label, sql) in attacks {
        match attack_message(&w.fx.book, sql) {
            Some(message) => {
                assert!(!message.starts_with("fb_valid"), "attack refused by fb_valid before its intended guard: {label}: {sql}");
                seen.insert(message);
            }
            None => panic!("attack accepted: {label}: {sql}"),
        }
    }
    seen
}

#[test]
fn every_ledger_guard_refuses_raw_sql_and_every_message_is_covered() {
    let w = world();
    let (c, o, u) = (w.company, w.other, w.fx.user);
    let (p, d, r) = (w.posted.id, w.draft.id, w.reversal.id);
    let mut attacks: Vec<(&str, String)> = vec![
        // seals
        ("update seal", "INSERT INTO fb_seal VALUES ('k', 'a'); UPDATE fb_seal SET doc_id = 'b'".into()),
        ("delete seal", "INSERT INTO fb_seal VALUES ('k', 'a'); DELETE FROM fb_seal".into()),
        ("update obligation", "INSERT INTO fb_seal VALUES ('k', 'a'); INSERT INTO fb_seal_due VALUES ('k', 'a'); UPDATE fb_seal_due SET kind = 'x'".into()),
        ("delete obligation", "INSERT INTO fb_seal VALUES ('k', 'a'); INSERT INTO fb_seal_due VALUES ('k', 'a'); DELETE FROM fb_seal_due".into()),
        ("replace operator", format!("INSERT OR REPLACE INTO fb_user SELECT * FROM fb_user WHERE id='{u}'")),
        ("delete operator", format!("DELETE FROM fb_user WHERE id='{u}'")),
        ("replace company", format!("INSERT OR REPLACE INTO ledger_company SELECT * FROM ledger_company WHERE id='{c}'")),
        ("delete company", format!("DELETE FROM ledger_company WHERE id='{c}'")),
        ("replace account", format!("INSERT OR REPLACE INTO ledger_account SELECT * FROM ledger_account WHERE id='{}'",w.cash)),
        ("delete account", format!("DELETE FROM ledger_account WHERE id='{}'",w.cash)),
        ("replace journal", format!("INSERT OR REPLACE INTO ledger_journal SELECT * FROM ledger_journal WHERE id='{p}'")),
        ("update replace company victim", format!("UPDATE OR REPLACE ledger_company SET id='{c}' WHERE id='{o}'")),
        ("update replace user victim", format!("INSERT INTO fb_user(id,username) VALUES('{}','spare'); UPDATE OR REPLACE fb_user SET username='tester' WHERE username='spare'",fb_core::UserId::new())),
        ("update replace membership victim", format!("UPDATE OR REPLACE ledger_company_members SET company_id='{c}' WHERE company_id='{o}'")),
        ("update replace line victim", format!("UPDATE OR REPLACE ledger_journalline SET position=1 WHERE journal_id='{d}' AND position=2")),
        // company
        ("lowercase currency", format!("INSERT INTO ledger_company (id, name, currency) VALUES ('{}', 'x', 'usd')", fb_core::CompanyId::new())),
        ("2-letter currency", format!("UPDATE ledger_company SET currency = 'US' WHERE id = '{c}'")),
        ("relabel currency with books", format!("UPDATE ledger_company SET currency = 'EUR' WHERE id = '{c}'")),
        ("change company id", format!("UPDATE ledger_company SET id = '{}' WHERE id = '{o}'", fb_core::CompanyId::new())),
        ("reminder days 91", format!("UPDATE ledger_company SET bill_reminder_days = 91 WHERE id = '{c}'")),
        // accounts
        ("invalid kind insert", format!("INSERT INTO ledger_account (id, company_id, code, name, kind) VALUES ('{}', '{c}', 'K', 'K', 'ASSETS')", fb_core::AccountId::new())),
        ("invalid kind update", format!("UPDATE ledger_account SET kind = 'INVALID' WHERE id = '{}'", w.inactive)),
        ("used account to income", format!("UPDATE ledger_account SET kind = 'INCOME', detail_type = 'INCOME' WHERE id = '{}'", w.cash)),
        ("used account to other company", format!("UPDATE ledger_account SET company_id = '{o}' WHERE id = '{}'", w.cash)),
        ("type of another kind", format!("UPDATE ledger_account SET detail_type = 'PAYABLE' WHERE id = '{}'", w.cash)),
        ("contra on a bank", format!("UPDATE ledger_account SET detail_type = 'BANK', is_contra = 1 WHERE id = '{}'", w.inactive)),
        ("duplicate code", format!("UPDATE ledger_account SET code = '1000' WHERE id = '{}'", w.inactive)),
        // audit
        ("audit update", "UPDATE ledger_auditevent SET action = 'concealed'".into()),
        ("audit delete", "DELETE FROM ledger_auditevent".into()),
        ("audit replace", "INSERT OR REPLACE INTO ledger_auditevent SELECT id, company_id, actor_id, actor_name, occurred_at, 'x', target, detail FROM ledger_auditevent LIMIT 1".into()),
        // journal insert
        ("journal born posted", new_journal_sql(&w, ", posted_by_id, posted_at", &format!(", '{u}', '2026-02-01T00:00:00Z'")).replace("'DRAFT'", "'POSTED'")),
        ("revision 0", new_journal_sql(&w, ", revision", ", 0")),
        ("retry key without digest", new_journal_sql(&w, ", request_key", &format!(", '{}'", uuid::Uuid::new_v4()))),
        ("draft with posting fields", new_journal_sql(&w, ", posted_by_id", &format!(", '{u}'"))),
        ("reversal with entry number", new_journal_sql(&w, ", reversal_of_id, entry_number", &format!(", '{}', 'X1'", w.draft.id))),
        ("real into integer revision", new_journal_sql(&w, ", revision", ", 1.5")),
        // journal update
        ("edit posted header", format!("UPDATE ledger_journal SET memo = 'x' WHERE id = '{p}'")),
        ("delete posted", format!("DELETE FROM ledger_journal WHERE id = '{p}'")),
        ("move draft to other company", format!("UPDATE ledger_journal SET company_id = '{o}' WHERE id = '{d}'")),
        ("renumber J-number", format!("UPDATE ledger_journal SET number = 99999 WHERE id = '{d}'")),
        ("revision back", format!("UPDATE ledger_journal SET revision = 0 WHERE id = '{d}'")),
        ("revision 0 via jump", format!("UPDATE ledger_journal SET revision = -1 WHERE id = '{d}'")),
        ("unknown status", format!("UPDATE ledger_journal SET status = 'VOID' WHERE id = '{d}'")),
        ("post without operator", format!("UPDATE ledger_journal SET status = 'POSTED' WHERE id = '{d}'")),
        ("draft gains entry kind clash", format!("UPDATE ledger_journal SET reversal_of_id = '{p}' WHERE id = '{d}'")),
        ("duplicate entry number", format!("UPDATE ledger_journal SET entry_number = '{}' WHERE id = '{d}'", w.posted.entry_number)),
        ("second reversal of the same original", format!("UPDATE ledger_journal SET entry_number = '', reversal_of_id = '{}' WHERE id = '{d}'", w.reversal.reversal_of_id.unwrap())),
        // lines
        ("line on posted journal", line_sql(p, w.cash, 9, 100, 0)),
        ("line on foreign account", line_sql(d, w.foreign, 9, 100, 0)),
        ("line on inactive account", line_sql(d, w.inactive, 9, 100, 0)),
        ("line with two sides", line_sql(d, w.cash, 9, 100, 100)),
        ("line with no side", line_sql(d, w.cash, 9, 0, 0)),
        ("negative line", line_sql(d, w.cash, 9, -100, 0)),
        ("over-bound line", line_sql(d, w.cash, 9, 10_000_000_000_000_000, 0)),
        ("float amount", line_sql(d, w.cash, 9, 0, 0).replace(", 0, 0)", ", 1.5, 0)")),
        ("duplicate position", line_sql(d, w.cash, 1, 100, 0)),
        ("edit posted line", format!("UPDATE ledger_journalline SET debit = 1 WHERE journal_id = '{p}'")),
        ("delete posted line", format!("DELETE FROM ledger_journalline WHERE journal_id = '{p}'")),
        ("move draft line into posted journal", format!("UPDATE ledger_journalline SET journal_id = '{p}', position = 99 WHERE journal_id = '{d}' AND position = 1")),
        ("move draft line to foreign account", format!("UPDATE ledger_journalline SET account_id = '{}' WHERE journal_id = '{d}' AND position = 1", w.foreign)),
        ("replace posted line", format!("INSERT OR REPLACE INTO ledger_journalline SELECT id, journal_id, account_id, position, debit + 1, credit, description, name FROM ledger_journalline WHERE journal_id = '{p}' AND debit > 0")),
        ("edit posted reversal", format!("UPDATE ledger_journal SET memo = 'x' WHERE id = '{r}'")),
    ];
    // Posting attacks need a fresh draft shape each, built inside the savepoint.
    let fresh = |label: &'static str, body: String| (label, body);
    let j = fb_core::JournalId::new();
    let header = |entry: &str, extra: &str| {
        format!(
            "INSERT INTO ledger_journal (id, number, company_id, date, memo, status, created_by_id, created_at, entry_number{extra_cols}) \
             VALUES ('{j}', 8000, '{c}', '2026-02-01', 'Raw', 'DRAFT', '{u}', '2026-02-01T00:00:00Z', '{entry}'{extra});",
            extra_cols = if extra.is_empty() { "" } else { ", reversal_of_id, adjusting" }
        )
    };
    attacks.push(fresh("post one line", format!("{}{}; {}", header("R1", ""), line_sql(j, w.cash, 1, 100, 0), post_sql(j, u))));
    attacks.push(fresh("post unequal", format!("{}{}; {}; {}", header("R2", ""), line_sql(j, w.cash, 1, 100, 0), line_sql(j, w.equity, 2, 0, 99), post_sql(j, u))));
    attacks.push(fresh(
        "post on account deactivated after drafting",
        format!(
            "{}{}; {}; UPDATE ledger_account SET active = 0 WHERE id = '{}'; {}",
            header("R3", ""), line_sql(j, w.cash, 1, 100, 0), line_sql(j, w.equity, 2, 0, 100), w.cash, post_sql(j, u)
        ),
    ));
    attacks.push(fresh("post manual without number", format!("{}{}; {}; {}", header("", ""), line_sql(j, w.cash, 1, 100, 0), line_sql(j, w.equity, 2, 0, 100), post_sql(j, u))));
    let mut many = header("R4", "");
    for n in 1..=50 {
        many.push_str(&line_sql(j, w.cash, n, 1, 0));
        many.push(';');
    }
    many.push_str(&line_sql(j, w.equity, 51, 0, 50));
    many.push(';');
    many.push_str(&post_sql(j, u));
    attacks.push(fresh("post 51 lines", many));
    attacks.push(fresh(
        "post over the total bound",
        format!(
            "{}{}; {}; {}; {}; {}",
            header("R5", ""), line_sql(j, w.cash, 1, 9_999_999_999_999_999, 0), line_sql(j, w.cash, 2, 1, 0),
            line_sql(j, w.equity, 3, 0, 9_999_999_999_999_999), line_sql(j, w.equity, 4, 0, 1), post_sql(j, u)
        ),
    ));
    attacks.push(fresh(
        "reverse a draft",
        format!("{}{}; {}; {}", header("", &format!(", '{d}', 0")), line_sql(j, w.equity, 1, 300, 0), line_sql(j, w.cash, 2, 0, 300), post_sql(j, u)),
    ));
    attacks.push(fresh(
        "reverse a reversal",
        format!("{}{}; {}; {}", header("", &format!(", '{r}', 0")), line_sql(j, w.equity, 1, 0, 700), line_sql(j, w.cash, 2, 700, 0), post_sql(j, u)),
    ));
    attacks.push(fresh(
        "reverse before the original date",
        format!(
            "{}{}; {}; UPDATE ledger_journal SET date = '2026-01-01' WHERE id = '{j}'; {}",
            header("", &format!(", '{p}', 0")).replace("'2026-02-01'", "'2026-01-01'"), line_sql(j, w.equity, 1, 1000, 0), line_sql(j, w.cash, 2, 0, 1000), post_sql(j, u)
        ),
    ));
    attacks.push(fresh(
        "reversal lines don't mirror",
        format!("{}{}; {}; {}", header("", &format!(", '{p}', 0")), line_sql(j, w.equity, 1, 999, 0), line_sql(j, w.cash, 2, 0, 999), post_sql(j, u)),
    ));
    let seen = run(&w, &attacks);
    // Dual variant (0130): an impossible date is refused by fb_valid first, and the older
    // date CHECK still refuses it when only this table's fb_valid insert trigger is removed.
    let bad_date = new_journal_sql(&w, "", "").replace("'2026-02-01', 'Raw'", "'2026-02-30', 'Raw'");
    assert_eq!(
        attack_message(&w.fx.book, &bad_date).as_deref(),
        Some("fb_valid: Invalid stored text, identifier, date or timestamp")
    );
    {
        let conn = w.fx.book.connection();
        conn.execute_batch("SAVEPOINT older_date_check; DROP TRIGGER fb_valid_ledger_journal_insert").unwrap();
        let older = attack_message(&w.fx.book, &bad_date);
        conn.execute_batch("ROLLBACK TO older_date_check; RELEASE older_date_check").unwrap();
        assert_eq!(older.as_deref(), Some("CHECK constraint failed: date(date) IS date"));
    }
    // L2a has its own exhaustive guard-message gate. Permanent-key refusals
    // may subsume older ledger errors, independently of trigger firing order.
    let sql = [fb_db::MIGRATIONS[0].1, fb_db::MIGRATIONS[6].1].concat();
    let mut expected = BTreeSet::new();
    for chunk in sql.split("RAISE(ABORT, '").skip(1) {
        let message = chunk.split("')").next().unwrap();
        if !message.starts_with('<') {
            expected.insert(message.to_string()); // (the header comment's template is not a message)
        }
    }
    let subsumed_by_permanent_keys = [
        "fb_company_guard: Company ids are permanent",
        "fb_company_guard: Update would replace an existing row",
        "fb_members_guard: Update would replace an existing membership",
    ];
    let missing: Vec<_> = expected.iter().filter(|m| {
        !seen.contains(*m) && !(subsumed_by_permanent_keys.contains(&m.as_str())
            && seen.contains("primary_key_immutable: Primary keys never change"))
    }).collect();
    assert!(missing.is_empty(), "guard messages with no refusing test: {missing:#?}\nseen: {seen:#?}");
    assert!(expected.len() >= 20, "{}", expected.len());
    // The guarded data is untouched.
    assert_eq!(w.fx.journal(p).memo, "Synthetic contribution");
    assert_eq!(w.fx.count("SELECT count(*) FROM ledger_journal WHERE status = 'POSTED'"), 3);
}

#[test]
fn the_service_layer_refuses_the_same_things_with_friendly_messages() {
    let mut w = world();
    let (c, u) = (w.company, w.fx.user);
    let book = &mut w.fx.book;
    let msg = |r: fb_core::FbResult<fb_core::model::Journal>| err_text(r);
    assert_eq!(msg(services::post_journal(book, u, c, w.posted.id, None)), "Only drafts can be posted.");
    let mut edit = DraftInput::new("2026-01-05", "Edit posted", vec![]);
    edit.journal_id = Some(w.posted.id);
    assert_eq!(msg(services::save_draft(book, u, c, edit)), "Posted journals cannot be edited.");
    let mut edit = DraftInput::new("2026-01-07", "Edit reversal", vec![]);
    edit.journal_id = Some(w.reversal.id);
    assert_eq!(msg(services::save_draft(book, u, c, edit)), "Posted journals cannot be edited.");
    assert_eq!(msg(services::reverse_journal(book, u, c, w.reversal.id, "2026-02-01", "Again")), "Only an unreversed original posted journal can be reversed.");
    assert_eq!(msg(services::reverse_journal(book, u, c, w.posted.id, "bad", "x")), "Enter a valid date.");
    let foreign_line = DraftInput::new("2026-01-09", "Foreign", vec![line(w.cash).debit("1").build(), line(w.foreign).credit("1").build()]);
    assert_eq!(msg(services::save_draft(book, u, c, foreign_line)), "Choose an active posting account in this company.");
    let inactive_line = DraftInput::new("2026-01-09", "Inactive", vec![line(w.cash).debit("1").build(), line(w.inactive).credit("1").build()]);
    assert_eq!(msg(services::save_draft(book, u, c, inactive_line)), "Choose an active posting account in this company.");
    let blank = DraftInput::new("bad-date", "", vec![]);
    assert_eq!(
        msg(services::save_draft(book, u, c, blank)),
        "“bad-date” value has an invalid date format. It must be in YYYY-MM-DD format.; This field cannot be blank."
    );
    // Another company's journal is not found through this company.
    assert!(matches!(services::post_journal(book, u, w.other, w.draft.id, None), Err(fb_core::FbError::DoesNotExist(_))));
    assert_eq!(
        err_text(services::edit_company(book, u, c, "Renamed", "eur")),
        "Currency cannot change after journals or commercial document identities have been created."
    );
    assert_eq!(
        err_text(services::create_company(book, u, " ", "usd", false)),
        "This field cannot be blank."
    );
}
