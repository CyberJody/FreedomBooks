//! Row reads and writes (prepared SQL; no ORM). Reads take a Query surface; crate-private writes require a Writer.

use fb_db::Query;
use crate::error::{FbError, FbResult};
use crate::model::*;
use fb_db::rusqlite::{OptionalExtension, Row, params};
use fb_ids::{AccountId, CompanyId, JournalId, JournalLineId, UserId};
use fb_money::Money;
use fb_db::Writer;
use std::str::FromStr;

pub struct User {
    pub id: UserId,
    pub username: String,
    pub is_active: bool,
    pub is_staff: bool,
    pub is_superuser: bool,
}

fn id<T: FromStr>(row: &Row<'_>, i: usize) -> fb_db::rusqlite::Result<T> {
    let text: String = row.get(i)?;
    text.parse::<T>().map_err(|_| fb_db::rusqlite::Error::InvalidColumnType(i, "uuid".into(), fb_db::rusqlite::types::Type::Text))
}

fn opt_id<T: FromStr>(row: &Row<'_>, i: usize) -> fb_db::rusqlite::Result<Option<T>> {
    let text: Option<String> = row.get(i)?;
    match text {
        None => Ok(None),
        Some(t) => t.parse::<T>().map(Some).map_err(|_| fb_db::rusqlite::Error::InvalidColumnType(i, "uuid".into(), fb_db::rusqlite::types::Type::Text)),
    }
}

pub fn user(conn: &(impl Query + ?Sized), user: UserId) -> FbResult<Option<User>> {
    Ok(conn
        .query_row(
            "SELECT id, username, is_active, is_staff, is_superuser FROM fb_user WHERE id = ?1",
            [user.to_db()],
            |r| Ok(User { id: id(r, 0)?, username: r.get(1)?, is_active: r.get(2)?, is_staff: r.get(3)?, is_superuser: r.get(4)? }),
        )
        .optional()?)
}

pub fn is_member(conn: &(impl Query + ?Sized), company: CompanyId, user: UserId) -> FbResult<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM ledger_company_members WHERE company_id = ?1 AND user_id = ?2)",
        [company.to_db(), user.to_db()],
        |r| r.get(0),
    )?)
}

const COMPANY_COLS: &str = "id, name, currency, first_run_open, bill_reminder_days";

fn company_row(r: &Row<'_>) -> fb_db::rusqlite::Result<Company> {
    Ok(Company { id: id(r, 0)?, name: r.get(1)?, currency: r.get(2)?, first_run_open: r.get(3)?, bill_reminder_days: r.get(4)? })
}

pub fn company(conn: &(impl Query + ?Sized), company: CompanyId) -> FbResult<Option<Company>> {
    Ok(conn.query_row(&format!("SELECT {COMPANY_COLS} FROM ledger_company WHERE id = ?1"), [company.to_db()], company_row).optional()?)
}

pub fn get_company(conn: &(impl Query + ?Sized), id: CompanyId) -> FbResult<Company> {
    company(conn, id)?.ok_or_else(|| FbError::missing("Company"))
}

pub(crate) fn insert_company(conn: &Writer<'_>, c: &Company) -> FbResult<()> {
    conn.execute(
        "INSERT INTO ledger_company (id, name, currency, first_run_open, bill_reminder_days) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![c.id.to_db(), c.name, c.currency, c.first_run_open, c.bill_reminder_days],
    )?;
    Ok(())
}

pub(crate) fn update_company(conn: &Writer<'_>, c: &Company) -> FbResult<()> {
    conn.execute(
        "UPDATE ledger_company SET name = ?2, currency = ?3, first_run_open = ?4, bill_reminder_days = ?5 WHERE id = ?1",
        params![c.id.to_db(), c.name, c.currency, c.first_run_open, c.bill_reminder_days],
    )?;
    Ok(())
}

pub(crate) fn add_member(conn: &Writer<'_>, company: CompanyId, user: UserId) -> FbResult<()> {
    conn.execute("INSERT OR IGNORE INTO ledger_company_members (company_id, user_id) VALUES (?1, ?2)", [company.to_db(), user.to_db()])?;
    Ok(())
}

pub const ACCOUNT_COLS: &str = "id, company_id, code, name, kind, detail_type, is_contra, description, active, postable";

pub fn account_row(r: &Row<'_>) -> fb_db::rusqlite::Result<Account> {
    let kind: String = r.get(4)?;
    let detail: String = r.get(5)?;
    let bad = |i: usize| fb_db::rusqlite::Error::InvalidColumnType(i, "enum".into(), fb_db::rusqlite::types::Type::Text);
    Ok(Account {
        id: id(r, 0)?,
        company_id: id(r, 1)?,
        code: r.get(2)?,
        name: r.get(3)?,
        kind: Kind::parse(&kind).ok_or_else(|| bad(4))?,
        detail_type: DetailType::parse(&detail).ok_or_else(|| bad(5))?,
        is_contra: r.get(6)?,
        description: r.get(7)?,
        active: r.get(8)?,
        postable: r.get(9)?,
    })
}

pub fn account(conn: &(impl Query + ?Sized), account: AccountId) -> FbResult<Option<Account>> {
    Ok(conn.query_row(&format!("SELECT {ACCOUNT_COLS} FROM ledger_account WHERE id = ?1"), [account.to_db()], account_row).optional()?)
}

/// Accounts in Python Meta.ordering = ["code", "pk"], using the PostgreSQL/glibc fb_text collation.
pub fn accounts(conn: &(impl Query + ?Sized), company: CompanyId) -> FbResult<Vec<Account>> {
    let mut stmt = conn.prepare(&format!("SELECT {ACCOUNT_COLS} FROM ledger_account WHERE company_id = ?1 ORDER BY code COLLATE fb_text, id"))?;
    let rows = stmt.query_map([company.to_db()], account_row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub(crate) fn insert_account(conn: &Writer<'_>, a: &Account) -> FbResult<()> {
    conn.execute(
        "INSERT INTO ledger_account (id, company_id, code, name, kind, detail_type, is_contra, description, active, postable)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![a.id.to_db(), a.company_id.to_db(), a.code, a.name, a.kind.as_str(), a.detail_type.as_str(), a.is_contra, a.description, a.active, a.postable],
    )?;
    Ok(())
}

pub(crate) fn update_account(conn: &Writer<'_>, a: &Account) -> FbResult<()> {
    conn.execute(
        "UPDATE ledger_account SET company_id = ?2, code = ?3, name = ?4, kind = ?5, detail_type = ?6, is_contra = ?7,
         description = ?8, active = ?9, postable = ?10 WHERE id = ?1",
        params![a.id.to_db(), a.company_id.to_db(), a.code, a.name, a.kind.as_str(), a.detail_type.as_str(), a.is_contra, a.description, a.active, a.postable],
    )?;
    Ok(())
}

pub fn account_has_lines(conn: &(impl Query + ?Sized), account: AccountId) -> FbResult<bool> {
    Ok(conn.query_row("SELECT EXISTS (SELECT 1 FROM ledger_journalline WHERE account_id = ?1)", [account.to_db()], |r| r.get(0))?)
}

pub fn account_code_taken(conn: &(impl Query + ?Sized), company: CompanyId, code: &str, except: Option<AccountId>) -> FbResult<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM ledger_account WHERE company_id = ?1 AND code = ?2 AND id IS NOT ?3)",
        params![company.to_db(), code, except.map(|a| a.to_db())],
        |r| r.get(0),
    )?)
}

pub const JOURNAL_COLS: &str = "id, number, company_id, date, memo, status, created_by_id, created_at, posted_by_id, posted_at, \
    reversal_of_id, entry_number, adjusting, request_key, creation_digest, revision, source_id";

pub fn journal_row(r: &Row<'_>) -> fb_db::rusqlite::Result<Journal> {
    let status: String = r.get(5)?;
    Ok(Journal {
        id: id(r, 0)?,
        number: r.get(1)?,
        company_id: id(r, 2)?,
        date: r.get(3)?,
        memo: r.get(4)?,
        status: Status::parse(&status).ok_or(fb_db::rusqlite::Error::InvalidColumnType(5, "status".into(), fb_db::rusqlite::types::Type::Text))?,
        created_by_id: id(r, 6)?,
        created_at: r.get(7)?,
        posted_by_id: opt_id(r, 8)?,
        posted_at: r.get(9)?,
        reversal_of_id: opt_id(r, 10)?,
        entry_number: r.get(11)?,
        adjusting: r.get(12)?,
        request_key: r.get(13)?,
        creation_digest: r.get(14)?,
        revision: r.get(15)?,
        source_id: r.get(16)?,
    })
}

pub fn journal(conn: &(impl Query + ?Sized), journal: JournalId) -> FbResult<Option<Journal>> {
    Ok(conn.query_row(&format!("SELECT {JOURNAL_COLS} FROM ledger_journal WHERE id = ?1"), [journal.to_db()], journal_row).optional()?)
}

pub fn journal_in(conn: &(impl Query + ?Sized), journal_id: JournalId, company: CompanyId) -> FbResult<Journal> {
    match journal(conn, journal_id)? {
        Some(j) if j.company_id == company => Ok(j),
        _ => Err(FbError::missing("Journal")),
    }
}

pub(crate) fn next_journal_number(conn: &Writer<'_>) -> FbResult<i64> {
    Ok(conn.query_row("SELECT coalesce(max(number), 0) + 1 FROM ledger_journal", [], |r| r.get(0))?)
}

pub(crate) fn insert_journal(conn: &Writer<'_>, j: &Journal) -> FbResult<()> {
    conn.execute(
        &format!("INSERT INTO ledger_journal ({JOURNAL_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)"),
        params![
            j.id.to_db(), j.number, j.company_id.to_db(), j.date, j.memo, j.status.as_str(), j.created_by_id.to_db(), j.created_at,
            j.posted_by_id.map(|u| u.to_db()), j.posted_at, j.reversal_of_id.map(|r| r.to_db()), j.entry_number, j.adjusting,
            j.request_key, j.creation_digest, j.revision, j.source_id
        ],
    )?;
    Ok(())
}

/// Django `journal.save()` on an existing row: every column is written back.
pub(crate) fn update_journal(conn: &Writer<'_>, j: &Journal) -> FbResult<()> {
    conn.execute(
        "UPDATE ledger_journal SET number = ?2, company_id = ?3, date = ?4, memo = ?5, status = ?6, created_by_id = ?7, created_at = ?8,
         posted_by_id = ?9, posted_at = ?10, reversal_of_id = ?11, entry_number = ?12, adjusting = ?13, request_key = ?14,
         creation_digest = ?15, revision = ?16, source_id = ?17 WHERE id = ?1",
        params![
            j.id.to_db(), j.number, j.company_id.to_db(), j.date, j.memo, j.status.as_str(), j.created_by_id.to_db(), j.created_at,
            j.posted_by_id.map(|u| u.to_db()), j.posted_at, j.reversal_of_id.map(|r| r.to_db()), j.entry_number, j.adjusting,
            j.request_key, j.creation_digest, j.revision, j.source_id
        ],
    )?;
    Ok(())
}

pub const LINE_COLS: &str = "id, journal_id, account_id, position, debit, credit, description, name";

pub fn line_row(r: &Row<'_>) -> fb_db::rusqlite::Result<JournalLine> {
    Ok(JournalLine {
        id: id(r, 0)?,
        journal_id: id(r, 1)?,
        account_id: id(r, 2)?,
        position: r.get(3)?,
        debit: Money::from_db(r.get(4)?),
        credit: Money::from_db(r.get(5)?),
        description: r.get(6)?,
        name: r.get(7)?,
    })
}

/// Lines in `Meta.ordering = ["position", "pk"]`.
pub fn lines(conn: &(impl Query + ?Sized), journal: JournalId) -> FbResult<Vec<JournalLine>> {
    let mut stmt = conn.prepare(&format!("SELECT {LINE_COLS} FROM ledger_journalline WHERE journal_id = ?1 ORDER BY position, id"))?;
    let rows = stmt.query_map([journal.to_db()], line_row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub(crate) fn insert_line(conn: &Writer<'_>, l: &JournalLine) -> FbResult<()> {
    let debit = l.debit.to_db().ok_or_else(|| FbError::validation(fb_money::INVALID_AMOUNT))?;
    let credit = l.credit.to_db().ok_or_else(|| FbError::validation(fb_money::INVALID_AMOUNT))?;
    conn.execute(
        &format!("INSERT INTO ledger_journalline ({LINE_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"),
        params![l.id.to_db(), l.journal_id.to_db(), l.account_id.to_db(), l.position, debit, credit, l.description, l.name],
    )?;
    Ok(())
}

pub(crate) fn delete_lines(conn: &Writer<'_>, journal: JournalId) -> FbResult<()> {
    conn.execute("DELETE FROM ledger_journalline WHERE journal_id = ?1", [journal.to_db()])?;
    Ok(())
}

pub fn new_line_id() -> JournalLineId {
    JournalLineId::new()
}
