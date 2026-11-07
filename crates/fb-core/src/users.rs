//! The operator rows the ledger needs. The Python app uses Django's auth users; the desktop core keeps
//! one `fb_user` row per operator because every audit event needs an actor (ARCHITECTURE section 7).
//! There is no Python service for these: `create_user` stands in for `User.objects.create_user` and
//! `add_member` for `company.members.add` (what the sample-company seeder does).

use crate::error::FbResult;
use crate::store;
use fb_db::Book;
use fb_db::rusqlite::params;
use fb_ids::{CompanyId, UserId};

pub fn create_user(book: &mut Book, username: &str, is_staff: bool) -> FbResult<UserId> {
    crate::py::clean_free_text(username)?;
    book.write(|tx| {
        let id = UserId::new();
        tx.execute(
            "INSERT INTO fb_user (id, username, is_active, is_staff, is_superuser) VALUES (?1, ?2, 1, ?3, 0)",
            params![id.to_db(), username, is_staff],
        )?;
        Ok(id)
    })
}

pub fn set_active(book: &mut Book, user: UserId, active: bool) -> FbResult<()> {
    book.write(|tx| {
        tx.execute("UPDATE fb_user SET is_active = ?2 WHERE id = ?1", params![user.to_db(), active])?;
        Ok(())
    })
}

pub fn add_member(book: &mut Book, company: CompanyId, user: UserId) -> FbResult<()> {
    book.write(|tx| store::add_member(tx, company, user))
}
