//! `backups.views._audit` (DESKTOP-BACKUPS): the Backups page's audit events. Django records them for
//! "Create backup now" and each download; the desktop host has no raw writer, so they go through here.
use crate::{CompanyId, FbError, FbResult, UserId, services};
use fb_db::Book;
use serde_json::Value;

pub const ACTIONS: [&str; 2] = ["backup.create", "backup.download"];

/// One audit event on `company` with target `backup:<file>` (Django's `[:100]`, by characters).
/// Same access rule as the page: a member who is staff or superuser.
pub fn record(book: &mut Book, actor: UserId, company: CompanyId, action: &str, file: &str, detail: Value) -> FbResult<()> {
    if !ACTIONS.contains(&action) {
        return Err(FbError::refused("Unknown backup audit action."));
    }
    book.write(|tx| {
        let user = services::access(tx, actor, company)?;
        if !(user.is_staff || user.is_superuser) {
            return Err(FbError::denied("Backups are limited to staff operators: a backup contains every company."));
        }
        let target: String = format!("backup:{file}").chars().take(100).collect();
        services::audit(tx, &user, company, action, target, detail)
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
        let c = services::create_company(&mut book, owner, "Pond", "USD", true).unwrap().id;
        let long = format!("{}.fbbackup", "x".repeat(120));
        record(&mut book, owner, c, "backup.create", &long, serde_json::json!({"file": long})).unwrap();
        let (action, target): (String, String) = book
            .read(|tx| tx.query_row("SELECT action, target FROM ledger_auditevent WHERE action LIKE 'backup.%'", [], |r| Ok((r.get(0)?, r.get(1)?))))
            .unwrap();
        assert_eq!(action, "backup.create");
        assert_eq!(target.chars().count(), 100);
        assert!(target.starts_with("backup:xxx"));
        assert!(record(&mut book, owner, c, "backup.restore", "f", Value::Null).is_err());
        assert!(record(&mut book, clerk, c, "backup.download", "f", Value::Null).is_err());
    }
}
