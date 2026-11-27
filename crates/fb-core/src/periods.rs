//! Company fiscal calendar and the inclusive close on new accounting activity.
use crate::{Book, CompanyId, FbError, FbResult, UserId, services};
use fb_db::{Query, Writer, company_periods::PeriodPolicyError, rusqlite::params};

pub use fb_db::company_periods::{PeriodChange, PeriodPolicy};

pub fn policy(query: &(impl Query + ?Sized), company: CompanyId) -> FbResult<PeriodPolicy> {
    Ok(fb_db::company_periods::period_policy(query, company)?)
}

pub fn can_set_policy(
    query: &(impl Query + ?Sized),
    actor: UserId,
    company: CompanyId,
) -> FbResult<bool> {
    Ok(query.query_row(
        "SELECT EXISTS(SELECT 1 FROM ledger_company_members m
         JOIN fb_user u ON u.id=m.user_id JOIN desktop_user d ON d.user_id=u.id
         WHERE m.company_id=?1 AND u.id=?2 AND u.is_active=1 AND d.role='administrator' AND d.password_hash IS NOT NULL)",
        params![company.to_db(), actor.to_db()], |row| row.get(0),
    )?)
}

pub fn closed_message(
    query: &(impl Query + ?Sized),
    company: CompanyId,
    date: &str,
) -> FbResult<Option<String>> {
    let closing_date: Option<String> = query.query_row(
        "SELECT closing_date FROM ledger_company WHERE id=?1",
        [company.to_db()],
        |row| row.get(0),
    )?;
    Ok(closing_date.filter(|close| date <= close.as_str()).map(|close| {
        format!("Date {date} is in a closed period. The books are closed through {close}. See Settings → Accounting periods (/c/{company}/settings/accounting-periods/).")
    }))
}

pub fn is_closed_message(message: &str) -> bool {
    message.starts_with("Date ")
        && message.contains(" is in a closed period. The books are closed through ")
}

pub fn require_open(writer: &Writer<'_>, company: CompanyId, date: &str) -> FbResult<()> {
    if let Some(message) = closed_message(writer, company, date)? {
        return Err(FbError::validation(message));
    }
    Ok(())
}

pub const STALE: &str = "Accounting periods changed. Review the current settings and submit again.";
pub const MONTH: &str = "Choose a fiscal-year start month from January through December.";
pub const DATE: &str = "Enter a valid closing date.";
pub const REASON: &str = "Enter a reason of 1–500 characters when reopening dates.";
const ADMINISTRATOR: &str =
    "An active company administrator with a configured password is required.";

pub fn set_policy(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    expected_revision: i64,
    change: PeriodChange,
    reason: Option<&str>,
) -> FbResult<PeriodPolicy> {
    if crate::agent_provenance::is_active() {
        return Err(FbError::denied(ADMINISTRATOR));
    }
    book.write(|writer| {
        services::access(writer, actor, company)?;
        if !can_set_policy(writer, actor, company)? {
            return Err(FbError::denied(ADMINISTRATOR));
        }
        writer
            .set_company_period_policy(actor, company, expected_revision, change, reason)
            .map_err(|error| match error {
                PeriodPolicyError::AdministratorRequired => FbError::denied(ADMINISTRATOR),
                PeriodPolicyError::Stale => FbError::validation(STALE),
                PeriodPolicyError::InvalidMonth => FbError::validation(MONTH),
                PeriodPolicyError::InvalidDate => FbError::validation(DATE),
                PeriodPolicyError::InvalidReason => FbError::validation(REASON),
                PeriodPolicyError::RevisionExhausted => {
                    FbError::validation("The accounting periods revision limit was reached.")
                }
                PeriodPolicyError::Sql(error) => error.into(),
            })
    })
}
