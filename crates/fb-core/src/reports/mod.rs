//! Port of `ledger/reports.py` (the account register and the trial balance), with the financial reports of
//! `ledger/financial.py` in [`financial`] and the report-control data logic of `ledger/report_views.py` in [`params`].

pub mod financial;
pub mod params;
#[cfg(test)]
mod tests;

use crate::error::{FbError, FbResult};
use crate::model::*;
use crate::py;
use crate::services::access;
use crate::store;
use fb_db::Book;
use fb_db::rusqlite::params;
use fb_ids::{AccountId, CompanyId, JournalId, UserId};
use fb_money::Money;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub struct RegisterRow {
    pub line: JournalLine,
    pub journal_id: JournalId,
    pub journal_number: i64,
    pub date: String,
    pub memo: String,
    pub entry_number: String,
    pub balance: Money,
    /// Layer 2: the owning document's provenance; never invented for manual journals.
    pub source: Option<String>,
    pub reversal: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Register {
    pub account: Account,
    pub opening: Money,
    pub closing: Money,
    pub rows: Vec<RegisterRow>,
    pub start: Option<String>,
    pub end: Option<String>,
}

/// Posted lines of one account, in (date, journal, position, line) order, with a running balance on
/// the account's section side (a contra account reads negative). Lines before `start` fold into
/// `opening`; `closing` is the balance through `end`.
pub fn register(book: &mut Book, actor: UserId, company: CompanyId, account_id: AccountId, start: Option<&str>, end: Option<&str>) -> FbResult<Register> {
    book.read(|tx| {
        access(tx, actor, company)?;
        let account = match store::account(tx, account_id)? {
            Some(a) if a.company_id == company => a,
            _ => return Err(FbError::missing("Account")),
        };
        let parse = |v: Option<&str>| -> FbResult<Option<chrono::NaiveDate>> {
            match v {
                Some(s) if !s.is_empty() => py::form_date(s).map(Some).map_err(FbError::validation),
                _ => Ok(None),
            }
        };
        let start = parse(start)?;
        let end = parse(end)?;
        if let (Some(s), Some(e)) = (start, end) && s > e {
            return Err(FbError::validation("Start date must not be after end date."));
        }
        let end_text = end.map(|d| d.to_string());
        let mut stmt = tx.prepare(
            "SELECT l.id, l.journal_id, l.account_id, l.position, l.debit, l.credit, l.description, l.name,
                    j.number, j.date, j.memo, j.entry_number, j.reversal_of_id
             FROM ledger_journalline l JOIN ledger_journal j ON j.id = l.journal_id
             WHERE l.account_id = ?1 AND j.company_id = ?2 AND j.status = 'POSTED' AND (?3 IS NULL OR j.date <= ?3)
             ORDER BY j.date, l.journal_id, l.position, l.id",
        )?;
        let fetched = stmt
            .query_map(params![account.id.to_db(), company.to_db(), end_text], |r| {
                Ok((store::line_row(r)?, r.get::<_, i64>(8)?, r.get::<_, String>(9)?, r.get::<_, String>(10)?, r.get::<_, String>(11)?, r.get::<_, Option<String>>(12)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let sign: i128 = if account.section_balance() == Side::Debit { 1 } else { -1 };
        let start_text = start.map(|d| d.to_string());
        let mut opening = Money::ZERO;
        let mut balance = Money::ZERO;
        let mut rows = Vec::new();
        for (line, number, date, memo, entry_number, reversal_of) in fetched {
            balance += Money::from_cents(sign * (line.debit - line.credit).cents());
            if start_text.as_deref().is_some_and(|s| date.as_str() < s) {
                opening = balance;
            } else {
                rows.push(RegisterRow {
                    journal_id: line.journal_id,
                    line,
                    journal_number: number,
                    date,
                    memo,
                    entry_number,
                    balance,
                    source: None,
                    reversal: reversal_of.is_some(),
                });
            }
        }
        Ok(Register { account, opening, closing: balance, rows, start: start_text, end: end_text })
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrialBalanceRow {
    pub account: Account,
    pub debit: Money,
    pub credit: Money,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrialBalance {
    pub rows: Vec<TrialBalanceRow>,
    pub debit: Money,
    pub credit: Money,
    pub as_of: String,
}

/// Every account of the company with its net posted balance through `as_of` on its natural column.
/// The sums are integer cents read per line and added in Rust (no SQL arithmetic).
pub fn trial_balance(book: &mut Book, actor: UserId, company: CompanyId, as_of: &str) -> FbResult<TrialBalance> {
    book.read(|tx| {
        access(tx, actor, company)?;
        let as_of = py::form_date(as_of).map_err(FbError::validation)?.to_string();
        let mut stmt = tx.prepare(
            "SELECT l.account_id, l.debit, l.credit FROM ledger_journalline l
             JOIN ledger_journal j ON j.id = l.journal_id JOIN ledger_account a ON a.id = l.account_id
             WHERE j.company_id = ?1 AND a.company_id = ?1 AND j.status = 'POSTED' AND j.date <= ?2",
        )?;
        let mut nets: HashMap<String, Money> = HashMap::new();
        let lines = stmt.query_map(params![company.to_db(), as_of], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))?;
        for line in lines {
            let (account, debit, credit) = line?;
            *nets.entry(account).or_default() += Money::from_db(debit) - Money::from_db(credit);
        }
        let mut rows = Vec::new();
        let (mut debit, mut credit) = (Money::ZERO, Money::ZERO);
        for account in store::accounts(tx, company)? {
            let net = nets.get(&account.id.to_db()).copied().unwrap_or(Money::ZERO);
            let (dr, cr) = (net.max(Money::ZERO), (-net).max(Money::ZERO));
            debit += dr;
            credit += cr;
            rows.push(TrialBalanceRow { account, debit: dr, credit: cr });
        }
        Ok(TrialBalance { rows, debit, credit, as_of })
    })
}
