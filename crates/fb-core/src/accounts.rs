//! Desktop mapping of accounts/: one audit owner and an optional app lock.
//! No HTTP address, usernames, sessions or throttle database tables. The local
//! unlock counter preserves Python's exact window/threshold/reset rules.
use crate::backups::{self, recovery::WrappedKey};
use crate::{Book, FbError, FbResult, UserId};
use fb_db::{
    Query, Writer,
    rusqlite::{OptionalExtension, params},
};
use zeroize::Zeroizing;

pub fn owner(q: &(impl Query + ?Sized)) -> FbResult<Option<UserId>> {
    let id: Option<String> = q
        .query_row(
            "SELECT user_id FROM accounts_owner WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    id.map(|id| UserId::parse(&id).ok_or_else(|| FbError::validation("Invalid owner identity.")))
        .transpose()
}
pub fn create_owner_tx(tx: &Writer<'_>, name: &str) -> FbResult<UserId> {
    crate::py::clean_free_text(name)?;
    tx.atomic(|| {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 150 {
            return Err(FbError::validation(
                "Enter an owner name of 1 to 150 characters.",
            ));
        }
        let count: i64 = tx.query_row("SELECT count(*) FROM fb_user", [], |r| r.get(0))?;
        if count != 0 {
            return Err(FbError::validation(
                "This book already has an owner or legacy operators.",
            ));
        }
        let id = UserId::new();
        tx.execute(
            "INSERT INTO fb_user(id,username,is_active,is_staff,is_superuser) VALUES(?1,?2,1,1,0)",
            params![id.to_db(), name],
        )?;
        tx.execute(
            "INSERT INTO accounts_owner(singleton,user_id) VALUES(1,?1)",
            [id.to_db()],
        )?;
        Ok(id)
    })
}
pub fn create_owner(book: &mut Book, name: &str) -> FbResult<UserId> {
    book.write(|tx| create_owner_tx(tx, name))
}

#[derive(Clone, Copy)]
pub struct LockPolicy {
    pub max_failures: u32,
    pub lockout_seconds: u64,
    pub idle_seconds: u64,
}
impl Default for LockPolicy {
    fn default() -> Self {
        Self {
            max_failures: 5,
            lockout_seconds: 900,
            idle_seconds: 900,
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum UnlockOutcome {
    Unlocked,
    Failed,
    Throttled,
}

/// Keep ONE instance in the main process, behind its command mutex. `now` is
/// monotonic seconds supplied by the bridge. Restart persistence and OS unlock
/// are host responsibilities; this is not an offline key-guessing boundary.
pub struct AppLock {
    key: Option<Zeroizing<[u8; 32]>>,
    wrapped: Option<WrappedKey>,
    password_required: bool,
    policy: LockPolicy,
    throttle: UnlockThrottle,
    last_activity: u64,
    last_now: u64,
}
impl AppLock {
    pub fn new(
        key: [u8; 32],
        passphrase: Option<&str>,
        password_required: bool,
        policy: LockPolicy,
        now: u64,
    ) -> backups::Result<Self> {
        let key = Zeroizing::new(key);
        if policy.max_failures == 0 || policy.lockout_seconds == 0 || policy.idle_seconds == 0 {
            return Err(backups::BackupError::refused("Invalid app-lock policy."));
        }
        if password_required && passphrase.is_none() {
            return Err(backups::BackupError::refused(
                "An app-lock password is required when the OS store uses basic_text.",
            ));
        }
        let wrapped = passphrase.map(|p| WrappedKey::new(&key, p)).transpose()?;
        Ok(Self {
            key: Some(key),
            wrapped,
            password_required,
            policy,
            throttle: UnlockThrottle::new(policy)?,
            last_activity: now,
            last_now: now,
        })
    }
    pub fn from_wrapped(
        wrapped: WrappedKey,
        password_required: bool,
        policy: LockPolicy,
        now: u64,
    ) -> backups::Result<Self> {
        if policy.max_failures == 0 || policy.lockout_seconds == 0 || policy.idle_seconds == 0 {
            return Err(backups::BackupError::refused("Invalid app-lock policy."));
        }
        Ok(Self {
            key: None,
            wrapped: Some(wrapped),
            password_required,
            policy,
            throttle: UnlockThrottle::new(policy)?,
            last_activity: now,
            last_now: now,
        })
    }
    pub fn wrapped_key(&self) -> Option<&WrappedKey> {
        self.wrapped.as_ref()
    }
    pub fn is_locked(&self) -> bool {
        self.key.is_none()
    }
    pub fn lock(&mut self) {
        self.key = None;
    }
    pub fn on_sleep(&mut self) {
        self.lock();
    }
    fn now(&mut self, now: u64) -> u64 {
        self.last_now = self.last_now.max(now);
        self.last_now
    }
    pub fn idle_tick(&mut self, now: u64) {
        let now = self.now(now);
        if now.saturating_sub(self.last_activity) >= self.policy.idle_seconds {
            self.lock();
        }
    }
    pub fn activity(&mut self, now: u64) {
        let now = self.now(now);
        self.idle_tick(now);
        if !self.is_locked() {
            self.last_activity = now;
        }
    }
    /// The bridge drops its open Book before lock. Access is closure-scoped so
    /// no borrowed key can outlive this call or survive a subsequent lock.
    pub fn with_key<T>(&self, f: impl FnOnce(&[u8; 32]) -> T) -> backups::Result<T> {
        self.key
            .as_ref()
            .map(|k| f(k))
            .ok_or_else(|| backups::BackupError::refused("The app is locked."))
    }
    pub fn unlock(&mut self, passphrase: &str, now: u64) -> UnlockOutcome {
        let now = self.now(now);
        let (key, throttled) = self.throttle.attempt(now, || {
            self.wrapped
                .as_ref()
                .and_then(|w| w.unlock(passphrase).ok())
        });
        if let Some(key) = key {
            self.key = Some(key);
            self.last_activity = now;
            UnlockOutcome::Unlocked
        } else if throttled {
            UnlockOutcome::Throttled
        } else {
            UnlockOutcome::Failed
        }
    }

    /// Only available when no app password was configured. The bridge is
    /// responsible for obtaining this key from an actually secure OS store.
    pub fn unlock_os(&mut self, key: [u8; 32], now: u64) -> backups::Result<()> {
        let key = Zeroizing::new(key);
        if self.password_required || self.wrapped.is_some() {
            return Err(backups::BackupError::refused(
                "Unlock with the app password.",
            ));
        }
        self.last_activity = self.now(now);
        self.key = Some(key);
        Ok(())
    }
}

/// Single-process replacement for Python's per-(user,address) throttle. Keep it
/// across lock/unlock cycles and serialize attempts with the password check.
pub struct UnlockThrottle {
    policy: LockPolicy,
    failures: u32,
    last_failure: Option<u64>,
    locked_until: Option<u64>,
    last_now: u64,
}
impl UnlockThrottle {
    pub fn new(policy: LockPolicy) -> backups::Result<Self> {
        if policy.max_failures == 0 || policy.lockout_seconds == 0 {
            return Err(backups::BackupError::refused("Invalid app-lock policy."));
        }
        Ok(Self {
            policy,
            failures: 0,
            last_failure: None,
            locked_until: None,
            last_now: 0,
        })
    }
    pub fn attempt<T>(&mut self, now: u64, check: impl FnOnce() -> Option<T>) -> (Option<T>, bool) {
        self.last_now = self.last_now.max(now);
        let now = self.last_now;
        if self.locked_until.is_some_and(|until| until > now) {
            return (None, true);
        }
        if let Some(value) = check() {
            self.failures = 0;
            self.last_failure = None;
            self.locked_until = None;
            return (Some(value), false);
        }
        if self
            .last_failure
            .is_none_or(|last| now.saturating_sub(last) >= self.policy.lockout_seconds)
        {
            self.failures = 0;
            self.locked_until = None;
        }
        self.failures = self.failures.saturating_add(1);
        self.last_failure = Some(now);
        if self.failures >= self.policy.max_failures {
            self.locked_until = Some(now.saturating_add(self.policy.lockout_seconds));
        }
        (None, self.locked_until.is_some_and(|until| until > now))
    }
}
