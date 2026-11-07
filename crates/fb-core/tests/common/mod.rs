#![allow(dead_code)]
//! Shared fixtures for the translated ledger tests.

use fb_core::services::{self, AccountInput, DraftInput, line};
use fb_core::{AccountId, Book, CompanyId, JournalId, UserId, users};
use fb_core::model::Journal;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static N: AtomicUsize = AtomicUsize::new(0);

pub const KEY: [u8; 32] = [42u8; 32];

pub fn book_path(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fb-core-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let n = N.fetch_add(1, Ordering::SeqCst);
    let path = dir.join(format!("{tag}-{n}.fbbook"));
    let _ = std::fs::remove_file(&path);
    path
}

pub fn book(tag: &str) -> Book {
    Book::create(&book_path(tag), &KEY).unwrap()
}

pub struct Fx {
    pub book: Book,
    pub user: UserId,
}

impl Fx {
    pub fn new(tag: &str) -> Fx {
        let mut book = book(tag);
        let user = users::create_user(&mut book, "tester", false).unwrap();
        Fx { book, user }
    }

    pub fn company(&mut self, name: &str) -> CompanyId {
        services::create_company(&mut self.book, self.user, name, "USD", false).unwrap().id
    }

    pub fn account(&mut self, company: CompanyId, code: &str, name: &str, kind: &str) -> AccountId {
        services::save_account(&mut self.book, self.user, company, AccountInput::new(code, name).kind(kind)).unwrap().id
    }

    /// `chart()`: a company with 1000 Cash (asset) and 3000 Capital (equity).
    pub fn chart(&mut self) -> (CompanyId, AccountId, AccountId) {
        let company = self.company("Synthetic Company");
        let cash = self.account(company, "1000", "Cash", "ASSET");
        let equity = self.account(company, "3000", "Capital", "EQUITY");
        (company, cash, equity)
    }

    /// `draft()`: debit cash / credit equity for `amount` on `date`.
    pub fn draft(&mut self, company: CompanyId, cash: AccountId, equity: AccountId, amount: &str, date: &str) -> Journal {
        services::save_draft(
            &mut self.book,
            self.user,
            company,
            DraftInput::new(date, "Synthetic contribution", vec![line(cash).debit(amount).credit("0").build(), line(equity).debit("0").credit(amount).build()]),
        )
        .unwrap()
    }

    pub fn post(&mut self, company: CompanyId, journal: JournalId) -> Journal {
        services::post_journal(&mut self.book, self.user, company, journal, None).unwrap()
    }

    pub fn count(&self, sql: &str) -> i64 {
        self.book.connection().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    pub fn audit_count(&self, action: &str) -> i64 {
        self.book.connection().query_row("SELECT count(*) FROM ledger_auditevent WHERE action = ?1", [action], |r| r.get(0)).unwrap()
    }

    pub fn journal(&self, id: JournalId) -> Journal {
        fb_core::store::journal(self.book.connection(), id).unwrap().unwrap()
    }

    pub fn line_amounts(&self, journal: JournalId, account: AccountId) -> (String, String) {
        let l = fb_core::store::lines(self.book.connection(), journal).unwrap().into_iter().find(|l| l.account_id == account).unwrap();
        (l.debit.to_string(), l.credit.to_string())
    }
}

/// Run raw SQL on the core's own connection inside a savepoint that is always rolled back; returns the
/// guard/constraint name of the refusal (or None if the statement was accepted).
pub fn attack(book: &Book, sql: &str) -> Option<String> {
    let conn = book.connection();
    conn.execute_batch("SAVEPOINT attack").unwrap();
    let result = conn.execute_batch(sql);
    let outcome = match &result {
        Ok(()) => None,
        Err(e) => Some(fb_db::constraint_of(e).unwrap_or_else(|| format!("other: {e}"))),
    };
    conn.execute_batch("ROLLBACK TO attack; RELEASE attack").unwrap();
    outcome
}

/// The full message of a refused raw statement (for message-coverage tests).
pub fn attack_message(book: &Book, sql: &str) -> Option<String> {
    let conn = book.connection();
    conn.execute_batch("SAVEPOINT attack").unwrap();
    let result = conn.execute_batch(sql);
    let outcome = result.err().map(|e| e.to_string());
    conn.execute_batch("ROLLBACK TO attack; RELEASE attack").unwrap();
    outcome
}

/// Exercise the earlier guard messages independently of newer overlapping guards.
/// Replacement/parent tests run against the complete schema in l2a_replacement.rs.
pub fn legacy_l2a_attack_message(book: &Book, sql: &str) -> Option<String> {
    let conn = book.connection();
    conn.execute_batch("SAVEPOINT isolate_legacy").unwrap();
    // Isolate the original L2a messages; current-chain identity coverage lives in
    // identity_permanence and l2a_identity_permanence, without removing guards.
    let names = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='trigger' AND (name GLOB 'fb_l2_*' OR name='fb_account_company_permanent')")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    for name in names {
        conn.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
    }
    let result = attack_message(book, sql);
    conn.execute_batch("ROLLBACK TO isolate_legacy; RELEASE isolate_legacy")
        .unwrap();
    result
}

pub fn err_text<T: std::fmt::Debug>(result: fb_core::FbResult<T>) -> String {
    match result {
        Err(e) => e.message(),
        Ok(v) => panic!("expected a refusal, got {v:?}"),
    }
}
