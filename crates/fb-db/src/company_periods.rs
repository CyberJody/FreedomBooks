//! Fixed Writer capability for revising accounting periods and appending their audit.
use crate::{CallbackScope, Query, Writer, check_numbers, validators};
use fb_ids::{AuditEventId, CompanyId, UserId};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeriodChange {
    pub fiscal_year_start_month: u32,
    pub closing_date: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PeriodPolicy {
    #[serde(flatten)]
    pub values: PeriodChange,
    pub revision: i64,
}

#[derive(Debug)]
pub enum PeriodPolicyError {
    AdministratorRequired,
    Stale,
    InvalidMonth,
    InvalidDate,
    InvalidReason,
    RevisionExhausted,
    Sql(rusqlite::Error),
}

impl From<rusqlite::Error> for PeriodPolicyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sql(error)
    }
}

pub fn period_policy(
    query: &(impl Query + ?Sized),
    company: CompanyId,
) -> rusqlite::Result<PeriodPolicy> {
    query.query_row(
        "SELECT fiscal_year_start_month,closing_date,period_policy_revision FROM ledger_company WHERE id=?1",
        [company.to_db()],
        |row| Ok(PeriodPolicy {
            values: PeriodChange {
                fiscal_year_start_month: row.get(0)?,
                closing_date: row.get(1)?,
            },
            revision: row.get(2)?,
        }),
    )
}

pub(crate) fn register(conn: &Connection) -> rusqlite::Result<Arc<AtomicU8>> {
    let state = Arc::new(AtomicU8::new(0));
    let captured = state.clone();
    conn.create_scalar_function(
        "fb_company_period_command",
        0,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8,
        move |_| Ok(captured.load(Ordering::Relaxed) == 1),
    )?;
    Ok(state)
}

impl Writer<'_> {
    pub fn set_company_period_policy(
        &self,
        actor: UserId,
        company: CompanyId,
        expected_revision: i64,
        change: PeriodChange,
        reason: Option<&str>,
    ) -> Result<PeriodPolicy, PeriodPolicyError> {
        use PeriodPolicyError::*;
        self.atomic(|| {
            let company_id = company.to_db();
            let actor_id = actor.to_db();
            let actor_name: Option<String> = self.query_row(
                "SELECT u.username FROM ledger_company_members m
                 JOIN fb_user u ON u.id=m.user_id
                 JOIN desktop_user d ON d.user_id=u.id
                 WHERE m.company_id=?1 AND u.id=?2 AND u.is_active=1
                   AND d.role='administrator' AND d.password_hash IS NOT NULL",
                params![company_id, actor_id], |r| r.get(0),
            ).optional()?;
            let actor_name = actor_name.ok_or(AdministratorRequired)?;
            let before = period_policy(self, company)?;
            if expected_revision != before.revision {
                return Err(Stale);
            }
            if !(1..=12).contains(&change.fiscal_year_start_month) {
                return Err(InvalidMonth);
            }
            if change.closing_date.as_deref().is_some_and(|date| !validators::date(date)) {
                return Err(InvalidDate);
            }
            if before.values == change {
                return Ok(before);
            }
            let reopened = change.closing_date < before.values.closing_date;
            let trimmed = reason.map(str::trim).filter(|s| !s.is_empty());
            if (reopened && trimmed.is_none())
                || reason.is_some_and(|s| !check_numbers::free_text_valid(s))
                || trimmed.is_some_and(|s| s.chars().count() > 500)
            {
                return Err(InvalidReason);
            }
            let revision = before.revision.checked_add(1).filter(|n| *n < i64::MAX).ok_or(RevisionExhausted)?;

            // Text policy code can run user callbacks. Run it with the capability off, check
            // that it preserves stored identities/dates, and scope the capability to this UPDATE.
            const UPDATE: &str = "UPDATE ledger_company SET fiscal_year_start_month=?1,closing_date=?2,period_policy_revision=?3 WHERE id=?4 AND period_policy_revision=?5";
            if self.guarded_text(UPDATE)? != UPDATE || self.guarded_text(&company_id)? != company_id
                || change.closing_date.as_deref().map(|s| self.guarded_text(s)).transpose()?.as_deref() != change.closing_date.as_deref()
            {
                return Err(Self::guarded_sql_error().into());
            }
            let updated = {
                let _scope = CallbackScope::enter(&self.company_period_command, 1);
                self.tx.execute(UPDATE, params![change.fiscal_year_start_month, change.closing_date, revision, company_id, expected_revision])?
            };
            if updated != 1 {
                return Err(Stale);
            }

            // Guard only free-text reason (allowing credential redaction there) while
            // requiring every structured audit field to pass text guards verbatim, then
            // bind those exact validated strings once via `self.tx.execute`.
            let guarded_reason = trimmed.map(|s| self.guarded_text(s)).transpose()?;
            let detail = serde_json::json!({
                "before": before.values,
                "after": change,
                "reason": guarded_reason,
                "revision": revision
            })
            .to_string();
            let date_changed = change.closing_date != before.values.closing_date;
            let fiscal_changed = change.fiscal_year_start_month != before.values.fiscal_year_start_month;
            let date_action = if reopened { "company.period_reopened" } else { "company.period_closed" };
            let target = format!("company:{company}");
            const INSERT_AUDIT: &str = "INSERT INTO ledger_auditevent(id,company_id,actor_id,actor_name,occurred_at,action,target,detail) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)";
            if self.guarded_text(INSERT_AUDIT)? != INSERT_AUDIT
                || self.guarded_text(&actor_id)? != actor_id
                || self.guarded_text(&actor_name)? != actor_name
                || self.guarded_text(&target)? != target
                || self.guarded_text(&detail)? != detail
            {
                return Err(Self::guarded_sql_error().into());
            }
            for (changed, action) in [(fiscal_changed, "company.fiscal_year_changed"), (date_changed, date_action)] {
                if changed {
                    let audit_id = AuditEventId::new().to_db();
                    let occurred_at = chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now())
                        .format("%Y-%m-%dT%H:%M:%S%.6fZ")
                        .to_string();
                    if self.guarded_text(&audit_id)? != audit_id
                        || self.guarded_text(&occurred_at)? != occurred_at
                        || self.guarded_text(action)? != action
                    {
                        return Err(Self::guarded_sql_error().into());
                    }
                    let inserted = self.tx.execute(
                        INSERT_AUDIT,
                        params![audit_id, company_id, actor_id, actor_name, occurred_at, action, target, detail],
                    )?;
                    if inserted != 1 {
                        return Err(rusqlite::Error::StatementChangedRows(inserted).into());
                    }
                }
            }
            Ok(PeriodPolicy { values: change, revision })
        })
    }
}
