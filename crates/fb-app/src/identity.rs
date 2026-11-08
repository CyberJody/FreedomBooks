//! Named desktop sessions and the central authorization boundary. A book key opens the file;
//! a verified individual password and any enabled MFA grant application authority. No renderer-selected actor.
use crate::{App, AppError, AppResult};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use fb_core::{CompanyId, UserId};
use fb_db::rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::{Duration, Instant};

pub const SIGN_IN: &str = "/desktop/sign-in/";
pub const USERS: &str = "/desktop/users/";
pub const SESSION: &str = "/desktop/session/";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Administrator,
    Bookkeeper,
    Readonly,
}
impl Role {
    pub fn parse(s: &str) -> AppResult<Self> {
        match s {
            "administrator" => Ok(Self::Administrator),
            "bookkeeper" => Ok(Self::Bookkeeper),
            "readonly" => Ok(Self::Readonly),
            _ => Err(invalid(
                "Choose an administrator, bookkeeper or read-only role.",
            )),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Administrator => "administrator",
            Self::Bookkeeper => "bookkeeper",
            Self::Readonly => "readonly",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    Read,
    Write,
    Administer,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthenticatedSession {
    pub actor: UserId,
    pub generation: u64,
    pub role: Role,
}

#[derive(Default)]
pub(crate) struct Identity {
    pub(crate) actor: Option<UserId>,
    pub(crate) mfa: crate::mfa::State,
    generation: u64,
    failures: u32,
    retry_after: Option<Instant>,
    pub(crate) pending_signins: std::collections::HashMap<CompanyId, u64>,
}
fn invalid(s: &str) -> AppError {
    AppError::Invalid(s.into())
}
fn password_hash(password: &str) -> AppResult<String> {
    if password.chars().count() < 8 || password.len() > 1024 {
        return Err(invalid(
            "Use a user password of at least 8 characters (at most 1024 bytes).",
        ));
    }
    let salt = SaltString::encode_b64(
        &crate::keys::random_bytes::<16>()
            .map_err(|_| invalid("Could not obtain a password salt."))?,
    )
    .map_err(|_| invalid("Could not obtain a password salt."))?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|_| invalid("Could not secure the user password."))
}
fn verifies(password: &str, hash: &str) -> bool {
    password.len() <= 1024
        && PasswordHash::new(hash).is_ok_and(|h| {
            Argon2::default()
                .verify_password(password.as_bytes(), &h)
                .is_ok()
        })
}

impl App {
    /// Invalidates form, review, dialog, recovery and agent capabilities. Monotonic for this App's
    /// entire lifetime, including book close/reopen. Never cache authority across this generation.
    pub(crate) fn invalidate_identity(&mut self) {
        self.ai.invalidate();
        self.identity.mfa=Default::default();
        // A browser callback begun by this person must never attach to the next person's session.
        for (company, _) in self.identity.pending_signins.drain() {
            if let Some(net) = self.net.as_mut() {
                let _ = net.call(&json!({"op":"chatgpt_cancel","company":company.to_db()}));
            }
        }
        self.identity.actor = None;
        self.identity.generation = self
            .identity
            .generation
            .checked_add(1)
            .expect("session generation exhausted");
        self.tokens.clear();
        self.d7a_reviews = Default::default();
        self.d5_reviews = Default::default();
        self.d6_reviews = Default::default();
        self.sales_reviews.clear();
        self.attachment_notes.clear();
        self.messages.clear();
        self.recovery_authorized = false;
        self.backups.invalidate_identity();
    }
    pub fn session_generation(&self) -> u64 {
        self.identity.generation
    }
    /// Rechecks active status, role and password enrollment against the book on every call.
    pub fn authenticated_session(&mut self) -> AppResult<AuthenticatedSession> {
        let open = self.open.as_mut().ok_or(AppError::Locked)?;
        let actor = self
            .identity
            .actor
            .ok_or_else(|| invalid("Sign in with your own user name and password."))?;
        if open.owner != actor {
            return Err(invalid("The active user has changed. Sign in again."));
        }
        let role: String = open.book.read(|q| q.query_row(
            "SELECT d.role FROM desktop_user d JOIN fb_user u ON u.id=d.user_id WHERE d.user_id=?1 AND u.is_active=1 AND d.password_hash IS NOT NULL",
            [actor.to_db()], |r| r.get(0))) .map_err(|_| invalid("This user is disabled or needs a password. Sign in again."))?;
        Ok(AuthenticatedSession {
            actor,
            generation: self.session_generation(),
            role: Role::parse(&role)?,
        })
    }
    /// AI/native integrations must retain the generation when accepting work and check it again
    /// under the APP mutex immediately before execution. Company authority is always explicit.
    pub fn require_session(
        &mut self,
        generation: u64,
        company: Option<CompanyId>,
        permission: Permission,
    ) -> AppResult<AuthenticatedSession> {
        if generation != self.session_generation() {
            return Err(invalid("The active user changed. Open the screen again."));
        }
        self.authorize(company, permission)
    }
    pub fn authorize(
        &mut self,
        company: Option<CompanyId>,
        permission: Permission,
    ) -> AppResult<AuthenticatedSession> {
        let session = self.authenticated_session()?;
        if permission == Permission::Administer && session.role != Role::Administrator {
            return Err(invalid("An administrator is required for this action."));
        }
        if permission == Permission::Write && session.role == Role::Readonly {
            return Err(invalid(
                "Read-only users can view and print, but cannot save changes.",
            ));
        }
        if let Some(c) = company {
            self.open
                .as_mut()
                .ok_or(AppError::Locked)?
                .book
                .read(|q| fb_core::services::access(q, session.actor, c))?;
        }
        Ok(session)
    }
    pub fn sign_out(&mut self) {
        self.invalidate_identity();
    }
    pub fn identity_location(&mut self) -> Option<&'static str> {
        self.authenticated_session().is_err().then_some(SIGN_IN)
    }
    /// First enrollment is deliberately limited to the retained owner, or the sole active staff
    /// operator of old desktop books. Interrupted enrollment leaves no partial hash or authority.
    pub fn bootstrap_owner(&mut self) -> AppResult<Option<(UserId, String)>> {
        let open = self.open.as_mut().ok_or(AppError::Locked)?;
        open.book.read(|q| {
            let enrolled: i64 = q.query_row("SELECT count(*) FROM desktop_user WHERE password_hash IS NOT NULL", [], |r| r.get(0))?;
            if enrolled != 0 { return Ok::<_,fb_db::rusqlite::Error>(None); }
            let rows: Vec<(String, String)> = q.prepare("SELECT u.id,u.username FROM fb_user u WHERE u.is_active=1 AND (u.is_staff=1 OR u.is_superuser=1) AND (u.id=(SELECT user_id FROM accounts_owner WHERE singleton=1) OR NOT EXISTS(SELECT 1 FROM accounts_owner)) ORDER BY u.id")?
                .query_map([], |r| Ok((r.get(0)?,r.get(1)?)))?.collect::<Result<_,_>>()?;
            Ok(if rows.len()==1 { UserId::parse(&rows[0].0).map(|id| (id,rows[0].1.clone())) } else { None })
        }).map_err(Into::into)
    }
    pub fn enroll_owner(&mut self, password: &str) -> AppResult<()> {
        let (owner, _) = self.bootstrap_owner()?.ok_or_else(|| invalid("Owner setup is unavailable. Ask a signed-in administrator to reset your user password."))?;
        let hash = password_hash(password)?;
        // Explicit first-owner enrollment authorizes connection-mask restoration before its first Writer.
        if let Err(error) = self.restore_connections_for(owner) {
            self.lock();
            return Err(error);
        }
        let open = self.open.as_mut().ok_or(AppError::Locked)?;
        open.book.write(|tx| {
            let inaccessible: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM ledger_company c WHERE NOT EXISTS(SELECT 1 FROM ledger_company_members m WHERE m.company_id=c.id AND m.user_id=?1))",[owner.to_db()],|r|r.get(0))?;
            if inaccessible { return Err(fb_core::FbError::validation("Owner setup needs an administrator membership in every company. No user password was changed.")); }
            tx.execute("INSERT INTO desktop_user(user_id,role,password_hash) VALUES(?1,'administrator',?2) ON CONFLICT(user_id) DO UPDATE SET role='administrator',password_hash=excluded.password_hash", params![owner.to_db(), hash])?;
            let user = fb_core::services::authenticated(tx, owner)?;
            let companies: Vec<String> = tx.prepare("SELECT company_id FROM ledger_company_members WHERE user_id=?1")?.query_map([owner.to_db()],|r|r.get(0))?.collect::<Result<_,_>>()?;
            for c in companies { let c=CompanyId::parse(&c).expect("canonical company id"); fb_core::services::audit(tx, &user, c, "user.enrolled", format!("user:{}",owner.to_db()), json!({"role":"administrator"}))?; }
            Ok::<_,fb_core::FbError>(())
        })?;
        // Enrollment sets a credential, then uses the same verification path as subsequent sign-ins.
        let name = self.bootstrap_name(owner)?;
        self.sign_in(&name, password)
    }
    fn bootstrap_name(&mut self, user: UserId) -> AppResult<String> {
        Ok(self.open.as_mut().ok_or(AppError::Locked)?.book.read(|q| {
            q.query_row(
                "SELECT username FROM fb_user WHERE id=?1",
                [user.to_db()],
                |r| r.get(0),
            )
        })?)
    }
    pub fn sign_in(&mut self, username: &str, password: &str) -> AppResult<()> {
        if self.identity.actor.is_some() {
            return Err(invalid("Sign out before switching to another user."));
        }
        self.identity_retry_allowed()?;
        self.identity.mfa.pending=None;
        let open = self.open.as_mut().ok_or(AppError::Locked)?;
        let row: Option<(String,String,String,bool)> = open.book.read(|q| q.query_row(
            "SELECT u.id,u.username,d.password_hash,u.is_active FROM fb_user u JOIN desktop_user d ON d.user_id=u.id WHERE u.username=?1 AND d.password_hash IS NOT NULL",
            [username], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional())?;
        let valid = row
            .as_ref()
            .is_some_and(|(_, _, h, active)| verifies(password, h) && *active);
        if !valid {
            // Unknown/unconfigured users also pay the KDF cost; generic errors do not reveal status.
            if row.is_none() {
                let _ = password_hash("Synthetic timing padding password");
            }
            self.identity_failure();
            return Err(invalid(
                "The user name or password is incorrect, or this user is disabled.",
            ));
        }
        let (id, name, hash, _) = row.expect("verified row");
        let actor = UserId::parse(&id).ok_or_else(|| invalid("Invalid user identity."))?;
        if self.mfa_factors(actor)?.enabled() {
            self.invalidate_identity();
            self.mfa_begin_signin(actor,name,hash);
            return Ok(());
        }
        self.finish_sign_in(actor,name)
    }
    pub(crate) fn finish_sign_in(&mut self, actor: UserId, name: String) -> AppResult<()> {
        self.invalidate_identity();
        self.identity_success();
        self.identity.actor = Some(actor);
        let session = self.open.as_mut().ok_or(AppError::Locked)?;
        session.owner = actor;
        session.owner_name = name;
        let role: String = session.book.read(|q| {
            q.query_row(
                "SELECT role FROM desktop_user WHERE user_id=?1",
                [actor.to_db()],
                |r| r.get(0),
            )
        })?;
        session.staff = role == "administrator";
        if let Err(error) = self.restore_connections_for(actor) {
            self.lock();
            return Err(error);
        }
        Ok(())
    }
    pub(crate) fn identity_retry_allowed(&self) -> AppResult<()> {
        if self.identity.retry_after.is_some_and(|t|t>Instant::now()) {
            return Err(invalid("Too many unsuccessful attempts. Wait one minute, then try again."));
        }
        Ok(())
    }
    pub(crate) fn identity_failure(&mut self) {
        self.identity.failures=self.identity.failures.saturating_add(1);
        if self.identity.failures>=5 {
            self.identity.retry_after=Some(Instant::now()+Duration::from_secs(60));
            self.identity.failures=0;
            self.identity.mfa=Default::default();
        }
    }
    pub(crate) fn identity_success(&mut self) {
        self.identity.failures=0;
        self.identity.retry_after=None;
    }
    pub(crate) fn verify_current_password(&mut self, actor:UserId, password:&str) -> AppResult<bool> {
        let hash:String=self.open.as_mut().ok_or(AppError::Locked)?.book.read(|q|q.query_row("SELECT d.password_hash FROM desktop_user d JOIN fb_user u ON u.id=d.user_id WHERE d.user_id=?1 AND u.is_active=1",[actor.to_db()],|r|r.get(0)))?;
        Ok(verifies(password,&hash))
    }
    /// A global role with explicit company memberships. Administrators manage the local book's
    /// roster; membership still gates their ordinary accounting reads and writes.
    #[allow(clippy::too_many_arguments)] // One atomic roster change includes role, credential and memberships.
    pub fn save_desktop_user(
        &mut self,
        user: Option<UserId>,
        name: &str,
        role: Role,
        active: bool,
        password: Option<&str>,
        companies: &[CompanyId],
    ) -> AppResult<UserId> {
        let session = self.authorize(None, Permission::Administer)?;
        fb_core::py::clean_free_text(name)?;
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 150 {
            return Err(invalid("Enter a unique user name of 1 to 150 characters."));
        }
        if companies.is_empty() {
            return Err(invalid("Choose at least one company membership."));
        }
        let hash = password.map(password_hash).transpose()?;
        if user.is_none() && hash.is_none() {
            return Err(invalid("Set an individual password for the new user."));
        }
        let id = user.unwrap_or_default();
        let open = self.open.as_mut().ok_or(AppError::Locked)?;
        open.book.write(|tx| {
            let actor=fb_core::services::authenticated(tx,session.actor)?;
            if user.is_none() {
                tx.execute("INSERT INTO fb_user(id,username,is_active,is_staff,is_superuser) VALUES(?1,?2,?3,?4,0)",params![id.to_db(),name,active,role==Role::Administrator])?;
                tx.execute("INSERT INTO desktop_user(user_id,role,password_hash) VALUES(?1,?2,?3)",params![id.to_db(),role.as_str(),hash])?;
            } else {
                let exists: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM fb_user WHERE id=?1)",[id.to_db()],|r|r.get(0))?;
                if !exists { return Err(fb_core::FbError::validation("Unknown user.")); }
                tx.execute("UPDATE desktop_user SET role=?2,password_hash=coalesce(?3,password_hash) WHERE user_id=?1",params![id.to_db(),role.as_str(),hash])?;
                tx.execute("UPDATE fb_user SET username=?2,is_active=?3,is_staff=?4,is_superuser=0 WHERE id=?1",params![id.to_db(),name,active,role==Role::Administrator])?;
            }
            // Add first, remove only the differences: never transiently remove a last admin.
            for c in companies { tx.execute("INSERT OR IGNORE INTO ledger_company_members(company_id,user_id) VALUES(?1,?2)",params![c.to_db(),id.to_db()])?; }
            let old: Vec<String>=tx.prepare("SELECT company_id FROM ledger_company_members WHERE user_id=?1")?.query_map([id.to_db()],|r|r.get(0))?.collect::<Result<_,_>>()?;
            for c in &old {
                if !companies.iter().any(|n|n.to_db()==*c) { tx.execute("DELETE FROM ledger_company_members WHERE user_id=?1 AND company_id=?2",params![id.to_db(),c])?; }
                let c=CompanyId::parse(c).ok_or_else(||fb_core::FbError::validation("Invalid company."))?;
                fb_core::services::audit(tx,&actor,c,if user.is_none(){"user.created"}else{"user.changed"},format!("user:{}",id.to_db()),json!({"username":name,"role":role.as_str(),"active":active,"password_changed":hash.is_some(),"companies":companies.iter().map(|c|c.to_db()).collect::<Vec<_>>()}))?;
            }
            Ok::<_,fb_core::FbError>(id)
        })?;
        // Any roster change revokes outstanding authority, including self-demotion and password reset.
        self.invalidate_identity();
        Ok(id)
    }
}

impl App {
    pub(crate) fn restore_connections_for(&mut self, actor: UserId) -> AppResult<()> {
        let open = self.open.take().ok_or(AppError::Locked)?;
        let device = self
            .device_key
            .as_ref()
            .ok_or_else(|| invalid("Connection key storage is unavailable."))?;
        let legacy = if self.legacy_credentials {
            Some(&**self.backups.open_book()?.1)
        } else {
            None
        };
        let vault =
            crate::credentials::BookCredentials::with_device_key(open.book, actor, device, legacy)
                .map_err(|_| invalid("The stored connection credentials could not be restored."))?;
        let reconnect = vault
            .restore_bankfeeds()
            .map_err(|_| invalid("The stored connection credentials could not be restored."))?;
        let book = vault
            .into_book()
            .map_err(|_| invalid("The stored connection credentials could not be restored."))?;
        self.open = Some(crate::session::Open { book, ..open });
        self.vault_key = self
            .device_key
            .as_ref()
            .and_then(|k| crate::credentials::wrapping_key(k).ok());
        if self.bankfeeds_enabled() {
            for name in reconnect {
                self.messages.push(crate::session::reconnect_notice(&name));
            }
        }
        Ok(())
    }
}

impl App {
    pub(crate) fn authorize_command(&mut self, command: &crate::Command) -> AppResult<()> {
        use crate::Command;
        if let Command::SubmitForm { path, fields, .. } = command {
            if [SIGN_IN, SESSION, USERS, crate::mfa::SETTINGS].contains(&path.as_str()) {
                if path == crate::mfa::SETTINGS { self.authorize(None,Permission::Read)?; }
                if path == USERS {
                    self.authorize(None, Permission::Administer)?;
                }
                return Ok(());
            }
            return self.authorize_path(
                path,
                if fields == &[("action".into(), "desktop_print".into())] {
                    Permission::Read
                } else {
                    Permission::Write
                },
            );
        }
        let permission = match command {
            Command::ExportHistory { .. }
            | Command::ExportReport { .. }
            | Command::ExportBankRegister { .. }
            | Command::ExportReconciliation { .. } => Permission::Read,
            Command::SaveCompany { .. }
            | Command::SaveCompanyDetails { .. }
            | Command::ExportBackup { .. } => Permission::Administer,
            _ => Permission::Write,
        };
        // Only the typed command's company_id is consulted; the actor is never deserialized.
        let value = serde_json::to_value(command).map_err(|_| invalid("Invalid command."))?;
        let company = value
            .get("company_id")
            .and_then(|v| v.as_str())
            .map(crate::session::company)
            .transpose()?;
        self.authorize(company, permission)?;
        Ok(())
    }
    pub(crate) fn authorize_query(&mut self, query: &crate::Query) -> AppResult<()> {
        let value = serde_json::to_value(query).map_err(|_| invalid("Invalid query."))?;
        let company = value
            .get("company_id")
            .and_then(|v| v.as_str())
            .map(crate::session::company)
            .transpose()?;
        self.authorize(company, Permission::Read)?;
        Ok(())
    }
    pub(crate) fn authorize_path(&mut self, target: &str, permission: Permission) -> AppResult<()> {
        let path = target.split('?').next().unwrap_or(target);
        if [
            crate::protection::SETTINGS,
            crate::protection::RECOVERY,
            crate::protection::KIT,
            "/settings/restore-backup/",
        ]
        .contains(&path)
        {
            self.authorize(None, Permission::Administer)?;
            return Ok(());
        }
        let route = crate::screens::resolve(path).ok_or_else(|| AppError::NotFound(path.into()))?;
        let company = route
            .args
            .get("company_id")
            .map(|id| crate::session::company(id))
            .transpose()?;
        let admin = route.name.starts_with("backups:")
            || route.name.starts_with("assistant:")
            || matches!(route.name.as_str(), "company_new" | "company_edit");
        self.authorize(
            company,
            if admin {
                Permission::Administer
            } else {
                permission
            },
        )?;
        Ok(())
    }
}
