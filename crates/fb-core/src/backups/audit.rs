//! `backups.views._audit` (DESKTOP-BACKUPS): the Backups page's audit events. Django records them for
//! "Create backup now" and each download; the desktop host has no raw writer, so they go through here.
use crate::{CompanyId, FbError, FbResult, UserId, services};
use fb_db::Book;
use serde_json::Value;

pub const ACTIONS: [&str; 2] = ["backup.create", "backup.download"];

/// One audit event on `company` with target `backup:<file>` (Django's `[:100]`, by characters).
/// Same access rule as the page: a member who is staff or superuser.
pub fn record(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    action: &str,
    file: &str,
    detail: Value,
) -> FbResult<()> {
    if !ACTIONS.contains(&action) {
        return Err(FbError::refused("Unknown backup audit action."));
    }
    book.write(|tx| {
        let user = services::access(tx, actor, company)?;
        if !(user.is_staff || user.is_superuser) {
            return Err(FbError::denied(
                "Backups are limited to staff operators: a backup contains every company.",
            ));
        }
        let target: String = format!("backup:{file}").chars().take(100).collect();
        services::audit(tx, &user, company, action, target, detail)
    })
}

const SCHEDULED: &str = "backup.scheduled";

pub(super) fn scheduled_administrator(
    q: &(impl fb_db::Query + ?Sized),
    actor: UserId,
    company: CompanyId,
) -> FbResult<crate::store::User> {
    let user = services::access(q, actor, company)?;
    let admin: bool = q.query_row(
        "SELECT EXISTS(SELECT 1 FROM desktop_user WHERE user_id=? AND role='administrator' AND password_hash IS NOT NULL)",
        [actor.to_db()],
        |row| row.get(0),
    )?;
    if !admin {
        return Err(FbError::denied(
            "An enrolled Administrator is required for automatic backups.",
        ));
    }
    Ok(user)
}

use super::{
    BackupError, Result,
    scheduled::{Evidence, Slot},
};
use fb_db::rusqlite::params;
use std::collections::BTreeMap;

fn evidence_rows(
    q: &(impl fb_db::Query + ?Sized),
    schedule_id: Option<&str>,
    stamp: Option<&str>,
) -> Result<BTreeMap<Slot, Evidence>> {
    let mut st = q.prepare(
        "SELECT target, detail FROM ledger_auditevent
         WHERE action=?1 AND (?2 IS NULL OR json_extract(detail,'$.schedule_id')=?2)
           AND (?3 IS NULL OR json_extract(detail,'$.slot')=?3)",
    )?;
    let rows = st.query_map(params![SCHEDULED, schedule_id, stamp], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let invalid = || BackupError::conflict("The automatic backup audit evidence is inconsistent.");
    let mut evidence = BTreeMap::new();
    for row in rows {
        let (target, raw) = row?;
        let detail: Value = fb_db::parse_unique_json(&raw).map_err(|_| invalid())?;
        let file = detail["file"].as_str().ok_or_else(invalid)?;
        let slot = Slot::from_basename(file).map_err(|_| invalid())?;
        let bytes = detail["bytes"]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or_else(invalid)?;
        let digest = |field: &str| {
            detail[field]
                .as_str()
                .filter(|d| super::digest_valid(d))
                .map(str::to_owned)
                .ok_or_else(invalid)
        };
        if target != format!("backup:{file}")
            || detail["schedule_id"] != slot.schedule_id()
            || detail["slot"] != slot.stamp()
        {
            return Err(invalid());
        }
        let row = Evidence {
            slot: slot.clone(),
            bytes,
            sha256: digest("archive_sha256")?,
            book_sha256: digest("book_sha256")?,
        };
        if evidence.insert(slot, row).is_some() {
            return Err(invalid());
        }
    }
    Ok(evidence)
}

pub(super) fn scheduled_evidence(
    book: &mut Book,
    schedule_id: Option<&str>,
    stamp: Option<&str>,
) -> Result<BTreeMap<Slot, Evidence>> {
    book.read(|q| evidence_rows(q, schedule_id, stamp))
}

/// One Writer checks slot idempotency, verifies the actor, and appends audit
/// evidence. A retry can authenticate as another Administrator of this company.
pub(super) fn record_scheduled(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    archive: &Evidence,
) -> Result<()> {
    let slot = &archive.slot;
    let file = slot.basename()?;
    book.write(|tx| {
        let user = scheduled_administrator(tx, actor, company)
            .map_err(|e| BackupError::retryable(e.to_string()))?;
        if let Some(prior) =
            evidence_rows(tx, Some(slot.schedule_id()), Some(&slot.stamp()))?.get(slot)
        {
            if prior != archive {
                return Err(BackupError::conflict(
                    "The automatic backup conflicts with its existing audit record.",
                ));
            }
            return Ok(());
        }
        services::audit(
            tx,
            &user,
            company,
            SCHEDULED,
            format!("backup:{file}"),
            serde_json::json!({
                "schedule_id": slot.schedule_id(),
                "slot": slot.stamp(),
                "file": file,
                "bytes": archive.bytes,
                "archive_sha256": archive.sha256,
                "book_sha256": archive.book_sha256,
            }),
        )
        .map_err(|e| BackupError::retryable(e.to_string()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_django_shaped_events_for_staff_only() {
        let d = tempfile::tempdir().unwrap();
        let mut book = Book::create(&d.path().join("b.fbbook"), &[7; 32]).unwrap();
        let owner = crate::users::create_user(&mut book, "owner", true).unwrap();
        let clerk = crate::users::create_user(&mut book, "clerk", false).unwrap();
        let c = services::create_company(&mut book, owner, "Pond", "USD", true)
            .unwrap()
            .id;
        let long = format!("{}.fbbackup", "x".repeat(120));
        record(
            &mut book,
            owner,
            c,
            "backup.create",
            &long,
            serde_json::json!({"file": long}),
        )
        .unwrap();
        let (action, target): (String, String) = book
            .read(|tx| {
                tx.query_row(
                    "SELECT action, target FROM ledger_auditevent WHERE action LIKE 'backup.%'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
            })
            .unwrap();
        assert_eq!(action, "backup.create");
        assert_eq!(target.chars().count(), 100);
        assert!(target.starts_with("backup:xxx"));
        assert!(record(&mut book, owner, c, "backup.restore", "f", Value::Null).is_err());
        assert!(record(&mut book, clerk, c, "backup.download", "f", Value::Null).is_err());
    }
}
