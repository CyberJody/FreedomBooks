//! Local-profile backup policy and trusted desktop automation. Times are UTC;
//! eligibility and the book's keys come from the already-open desktop session.
use crate::{
    App, AppError, AppResult,
    identity::{AuthenticatedSession, Role},
};
use chrono::{DateTime, Datelike, Days, Local, NaiveTime, TimeZone, Utc};
use fb_core::{
    CompanyId,
    backups::{Directory, scheduled as core},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const MAX_SETTINGS: u64 = 16 * 1024;
const RETRY_DELAY: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Frequency {
    Daily,
    Weekly,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Verified,
    RetentionFailed,
    Retry,
    PublishedRetry,
    Conflict,
    MissingReceipts,
    ProtectionChanged,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    version: u32,
    enabled: bool,
    frequency: Frequency,
    weekday: Option<u32>,
    time_utc: String,
    keep_count: usize,
    schedule_id: String,
    company_id: String,
    book_path: PathBuf,
    backup_root: PathBuf,
    first_slot: i64,
    last_completed_slot: Option<i64>,
    in_flight_slot: Option<i64>,
    in_flight_key: Option<String>,
    status: Option<Status>,
}

pub(crate) struct Backoff {
    schedule_id: String,
    until: Instant,
}

#[derive(Clone, Copy)]
struct Cadence {
    weekday: Option<u32>,
    time: NaiveTime,
}

impl Cadence {
    fn parse(frequency: Frequency, time: &str, weekday: Option<u32>) -> Option<Self> {
        let parsed = NaiveTime::parse_from_str(time, "%H:%M").ok()?;
        if parsed.format("%H:%M").to_string() != time {
            return None;
        }
        let weekday = match (frequency, weekday) {
            (Frequency::Daily, None) => None,
            (Frequency::Weekly, Some(day @ 1..=7)) => Some(day),
            _ => return None,
        };
        Some(Self {
            weekday,
            time: parsed,
        })
    }

    fn period(self) -> Days {
        Days::new(if self.weekday.is_some() { 7 } else { 1 })
    }

    fn latest(self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let today = now.date_naive();
        let back = self.weekday.map_or(0, |day| {
            (today.weekday().number_from_monday() + 7 - day) % 7
        });
        let date = today.checked_sub_days(Days::new(u64::from(back)))?;
        let slot = date.and_time(self.time).and_utc();
        if slot <= now {
            Some(slot)
        } else {
            slot.checked_sub_days(self.period())
        }
    }

    fn next(self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.latest(now)?.checked_add_days(self.period())
    }
}

fn invalid(message: &str) -> AppError {
    AppError::Invalid(message.into())
}
fn damaged() -> AppError {
    invalid("Automatic backup settings could not be read safely.")
}
fn save_failed() -> AppError {
    invalid("Automatic backup settings could not be saved.")
}

impl Settings {
    fn cadence(&self) -> AppResult<Cadence> {
        Cadence::parse(self.frequency, &self.time_utc, self.weekday).ok_or_else(damaged)
    }

    fn validate(&self) -> AppResult<()> {
        let cadence = self.cadence()?;
        let valid_slot =
            |stamp| instant(stamp).is_ok_and(|slot| cadence.latest(slot) == Some(slot));
        if self.version != 1
            || !core::valid_id(&self.schedule_id)
            || CompanyId::parse(&self.company_id).is_none()
            || !(1..=90).contains(&self.keep_count)
            || !self.book_path.is_absolute()
            || !self.backup_root.is_absolute()
            || !valid_slot(self.first_slot)
            || !self.last_completed_slot.is_none_or(valid_slot)
            || !self.in_flight_slot.is_none_or(valid_slot)
            || self.in_flight_slot.is_some() != self.in_flight_key.is_some()
            || !self.in_flight_key.as_ref().is_none_or(|tag| {
                tag.len() == 64
                    && tag
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
            || self
                .in_flight_slot
                .zip(self.last_completed_slot)
                .is_some_and(|(flight, done)| flight <= done)
        {
            return Err(damaged());
        }
        Ok(())
    }

    fn restart(&mut self, now: DateTime<Utc>) -> AppResult<()> {
        self.schedule_id = uuid::Uuid::new_v4().simple().to_string();
        self.first_slot = self.cadence()?.next(now).ok_or_else(damaged)?.timestamp();
        self.last_completed_slot = None;
        self.clear_flight();
        self.status = None;
        Ok(())
    }

    fn clear_flight(&mut self) {
        self.in_flight_slot = None;
        self.in_flight_key = None;
    }

    fn retire_flight(&mut self, stamp: i64, status: Status) -> AppResult<()> {
        self.first_slot = self.first_slot.max(
            self.cadence()?
                .next(instant(stamp)?)
                .ok_or_else(damaged)?
                .timestamp(),
        );
        self.clear_flight();
        self.status = Some(status);
        Ok(())
    }

    fn due(&self, now: DateTime<Utc>) -> AppResult<Option<i64>> {
        if let Some(stamp) = self.in_flight_slot {
            let slot =
                core::Slot::new(&self.schedule_id, instant(stamp)?).map_err(|_| damaged())?;
            if core::published(&self.backup_root, &slot).map_err(crate::protection::error)? {
                return Ok((stamp <= now.timestamp()).then_some(stamp));
            }
        }
        let due = self.cadence()?.latest(now).ok_or_else(damaged)?.timestamp();
        Ok((due >= self.first_slot
            && self.in_flight_slot.is_none_or(|old| due >= old)
            && self.last_completed_slot.is_none_or(|old| due > old))
        .then_some(due))
    }

    fn display_next(&self, now: DateTime<Utc>) -> AppResult<DateTime<Utc>> {
        if let Ok(Some(stamp)) = self.due(now) {
            return instant(stamp);
        }
        let after = now
            .timestamp()
            .max(self.last_completed_slot.unwrap_or(i64::MIN))
            .max(self.in_flight_slot.unwrap_or(i64::MIN));
        let next = self.cadence()?.next(instant(after)?).ok_or_else(damaged)?;
        instant(next.timestamp().max(self.first_slot))
    }
}

fn instant(stamp: i64) -> AppResult<DateTime<Utc>> {
    DateTime::from_timestamp(stamp, 0).ok_or_else(damaged)
}

const SETTINGS_FILE: &str = "automatic-backups.json";

fn load(root: &Path) -> AppResult<Option<Settings>> {
    let dir = Directory::open(root).map_err(|_| damaged())?;
    let Some(file) = dir.read_file(SETTINGS_FILE).map_err(|_| damaged())? else {
        return Ok(None);
    };
    let meta = file.metadata().map_err(|_| damaged())?;
    if meta.len() > MAX_SETTINGS {
        return Err(damaged());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(damaged());
        }
    }
    let mut raw = String::new();
    file.take(MAX_SETTINGS + 1)
        .read_to_string(&mut raw)
        .map_err(|_| damaged())?;
    if raw.len() as u64 > MAX_SETTINGS {
        return Err(damaged());
    }
    let settings: Settings =
        serde_json::from_value(fb_db::parse_unique_json(&raw).map_err(|_| damaged())?)
            .map_err(|_| damaged())?;
    settings.validate()?;
    Ok(Some(settings))
}

fn save(root: &Path, settings: &Settings) -> AppResult<()> {
    settings.validate()?;
    let raw = serde_json::to_vec(settings).map_err(|_| save_failed())?;
    if raw.len() as u64 > MAX_SETTINGS {
        return Err(save_failed());
    }
    Directory::open(root)
        .and_then(|dir| dir.replace_private_file(SETTINGS_FILE, &raw))
        .map_err(|_| save_failed())
}

pub(crate) fn reset_for_main(root: &Path, main: &Path, now: DateTime<Utc>) -> AppResult<()> {
    if let Some(mut settings) = load(root)?
        && settings.book_path != main
    {
        settings.book_path = main.to_owned();
        settings.restart(now)?;
        save(root, &settings)?;
    }
    Ok(())
}

pub(crate) fn folder_changed(root: &Path, folder: &Path, now: DateTime<Utc>) -> AppResult<()> {
    if let Some(mut settings) = load(root)?
        && settings.backup_root != folder
    {
        settings.backup_root = folder.to_owned();
        settings.restart(now)?;
        save(root, &settings)?;
    }
    Ok(())
}

fn main_book(app: &App) -> AppResult<bool> {
    let (Ok(root), Some(book)) = (app.backups.root(), app.book_path()) else {
        return Ok(false);
    };
    Ok(app.backups.viewing_restored().is_none() && crate::books::is_main_book(root, book)?)
}

fn local_administrator(app: &mut App) -> Option<AuthenticatedSession> {
    if app.remote_session {
        return None;
    }
    app.authenticated_session()
        .ok()
        .filter(|session| session.role == Role::Administrator)
}

pub(crate) fn may_manage(app: &mut App) -> bool {
    local_administrator(app).is_some() && main_book(app).unwrap_or(false)
}

pub(crate) fn configure(
    app: &mut App,
    company: CompanyId,
    fields: &[(String, String)],
) -> AppResult<()> {
    if !may_manage(app) {
        return Err(invalid(
            "Automatic backups require a local Administrator in the main books.",
        ));
    }
    app.authorize(Some(company), crate::identity::Permission::Administer)?;
    let field = |name| {
        fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    let enabled = match field("enabled") {
        None | Some("0") => false,
        Some("1") => true,
        _ => return Err(invalid("Choose whether automatic backups are enabled.")),
    };
    let (frequency, weekday) = match field("frequency") {
        Some("daily") => (Frequency::Daily, None),
        Some("weekly") => (
            Frequency::Weekly,
            field("weekday").and_then(|day| day.parse().ok()),
        ),
        _ => return Err(invalid("Choose daily or weekly automatic backups.")),
    };
    let time = field("time_utc").unwrap_or("");
    let cadence = Cadence::parse(frequency, time, weekday)
        .ok_or_else(|| invalid("Choose a valid UTC time and a day for weekly backups."))?;
    let keep_count = field("keep_count")
        .and_then(|n| n.parse().ok())
        .filter(|n| (1..=90).contains(n))
        .ok_or_else(|| invalid("Keep between 1 and 90 automatic backups."))?;
    let root = app.backups.root()?;
    let book_path = app.book_path().ok_or(AppError::Locked)?.to_owned();
    let backup_root = app.backups.folder()?;
    let now = (app.now)();
    let next = cadence.next(now).ok_or_else(damaged)?.timestamp();
    let prior = load(root)?;
    let rotate = prior.as_ref().is_none_or(|s| {
        s.frequency != frequency
            || s.weekday != weekday
            || s.time_utc != time
            || s.book_path != book_path
            || s.backup_root != backup_root
    });
    let mut settings = prior.unwrap_or_else(|| Settings {
        version: 1,
        enabled: false,
        frequency,
        weekday,
        time_utc: time.into(),
        keep_count,
        schedule_id: String::new(),
        company_id: company.to_db(),
        book_path: book_path.clone(),
        backup_root: backup_root.clone(),
        first_slot: next,
        last_completed_slot: None,
        in_flight_slot: None,
        in_flight_key: None,
        status: None,
    });
    if enabled && !settings.enabled {
        settings.first_slot = settings.first_slot.max(next);
    }
    settings.enabled = enabled;
    settings.frequency = frequency;
    settings.weekday = weekday;
    settings.time_utc = time.into();
    settings.keep_count = keep_count;
    settings.company_id = company.to_db();
    settings.book_path = book_path;
    settings.backup_root = backup_root;
    if rotate {
        settings.restart(now)?;
    }
    save(root, &settings)?;
    app.scheduled_retry = None;
    Ok(())
}

fn shown_local<Tz: TimeZone>(at: DateTime<Utc>, zone: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    at.with_timezone(zone)
        .format("%Y-%m-%d %H:%M (%:z)")
        .to_string()
}

pub(crate) fn view(app: &mut App) -> AppResult<Value> {
    let root = app.backups.root()?;
    let folder = app.backups.folder()?;
    let settings = load(root)?;
    let target = folder.join("automatic");
    let Some(settings) = settings else {
        return Ok(json!({"frequency":"daily", "time_utc":"02:00", "weekday":1,
            "keep_count":7, "folder":target.display().to_string()}));
    };
    let current =
        app.book_path() == Some(settings.book_path.as_path()) && folder == settings.backup_root;
    let now = (app.now)();
    let next = (settings.enabled && current)
        .then(|| settings.display_next(now))
        .transpose()?;
    let company = CompanyId::parse(&settings.company_id).ok_or_else(damaged)?;
    let can_audit = app
        .authorize(Some(company), crate::identity::Permission::Administer)
        .is_ok();
    let status = match settings.status.filter(|_| current && settings.enabled) {
        Some(Status::Verified) | None => None,
        Some(Status::RetentionFailed) => {
            Some("The backup was verified. Older automatic backups could not be removed. Check the schedule folder, including hidden .fb-retention- folders, for retained files.")
        }
        Some(Status::Retry) => Some(
            "The automatic backup could not be verified or recorded. FreedomBooks will retry. Check the backup folder and available space.",
        ),
        Some(Status::PublishedRetry) => Some(
            "The scheduled backup file could not be verified or recorded. The file was kept and FreedomBooks will check it again.",
        ),
        Some(Status::Conflict) => Some(
            "An interrupted automatic backup conflicts with its slot or verification record. Its file was kept. Later backups can still run.",
        ),
        Some(Status::MissingReceipts) => Some(
            "Attached files are missing or damaged. Restore those files so the automatic backup can run again.",
        ),
        Some(Status::ProtectionChanged) => Some(
            "Book protection changed before an interrupted backup was recorded. Its file was kept. The next backup uses the current protection settings.",
        ),
    };
    let status = if settings.enabled && !can_audit {
        Some("Save the schedule on a Backups page for a company you can access.")
    } else {
        status
    };
    let folder = if current {
        target.join(&settings.schedule_id)
    } else {
        target
    };
    Ok(json!({
        "enabled":settings.enabled, "frequency":settings.frequency, "weekday":settings.weekday.unwrap_or(1),
        "time_utc":settings.time_utc, "keep_count":settings.keep_count, "folder":folder.display().to_string(),
        "next_utc":next.map(|at| at.format("%Y-%m-%d %H:%M UTC").to_string()),
        "next_local":next.map(|at| shown_local(at, &Local)),
        "status":status,
    }))
}

/// Runs under the native APP lease, outside the hosting bridge. No code here
/// opens a book or retrieves an OS key.
pub fn desktop_tick(app: &mut App, generation: u64) -> AppResult<()> {
    let Some(session) = local_administrator(app).filter(|s| s.generation == generation) else {
        return Ok(());
    };
    if !main_book(app)? || app.backups.maintenance_pending() {
        return Ok(());
    }

    let root = app.backups.root()?.to_owned();
    let Some(mut settings) = load(&root)? else {
        return Ok(());
    };
    if !settings.enabled || app.host_record()?.is_some_and(|r| r.enabled) {
        return Ok(());
    }
    if app
        .scheduled_retry
        .as_ref()
        .is_some_and(|r| r.schedule_id == settings.schedule_id && Instant::now() < r.until)
    {
        return Ok(());
    }
    let book = app.book_path().ok_or(AppError::Locked)?.to_owned();
    for path in [
        crate::books::change_journal(&app.backups.books()?),
        crate::protection::sidecar(&book, ".rekey.json"),
        crate::protection::sidecar(&book, ".rekey-before"),
    ] {
        if crate::protection::present_path(&path)? {
            return Ok(());
        }
    }
    let now = (app.now)();
    let folder = app.backups.folder()?;
    if book != settings.book_path || folder != settings.backup_root {
        settings.book_path = book;
        settings.backup_root = folder;
        settings.restart(now)?;
        return save(&root, &settings);
    }
    let company = CompanyId::parse(&settings.company_id).ok_or_else(damaged)?;
    if app
        .authorize(Some(company), crate::identity::Permission::Administer)
        .is_err()
    {
        return Ok(());
    }
    let stamp = match settings.due(now) {
        Ok(Some(stamp)) => stamp,
        Ok(None) => return Ok(()),
        Err(_) => return retry(app, &root, &mut settings, Status::Retry),
    };
    let slot = core::Slot::new(&settings.schedule_id, instant(stamp)?).map_err(|_| damaged())?;
    let key_tag = core::key_tag(app.master_key()?, &settings.schedule_id)
        .map_err(crate::protection::error)?;
    if settings.in_flight_slot == Some(stamp)
        && settings
            .in_flight_key
            .as_ref()
            .is_some_and(|tag| tag != &key_tag)
        && core::published(&settings.backup_root, &slot).map_err(crate::protection::error)?
        && match core::resolve(&mut app.open_mut()?.book, &settings.backup_root, &slot) {
            Ok(archive) => archive.is_none(),
            Err(error) if error.is_conflict() => true,
            Err(_) => return retry(app, &root, &mut settings, Status::PublishedRetry),
        }
    {
        // A confirmed archive can settle through audit after a rekey. An
        // unaudited old-key archive stays untouched and cannot block future slots.
        settings.retire_flight(stamp, Status::ProtectionChanged)?;
        return save(&root, &settings);
    }
    settings.in_flight_slot = Some(stamp);
    settings.in_flight_key = Some(key_tag);
    if let Err(e) = save(&root, &settings) {
        back_off(app, &settings);
        return Err(e);
    }

    match crate::screens::backups::scheduled_copy(app, session.actor, company, &slot) {
        Ok(()) => {
            settings.last_completed_slot = Some(stamp);
            settings.clear_flight();
            settings.status = Some(Status::Verified);
            if let Err(e) = save(&root, &settings) {
                back_off(app, &settings);
                return Err(e);
            }
            app.scheduled_retry = None;
            let book = &mut app.open_mut()?.book;
            if core::prune(book, &settings.backup_root, &slot, settings.keep_count).is_err() {
                settings.status = Some(Status::RetentionFailed);
                save(&root, &settings)?;
            }
            Ok(())
        }
        Err(failure) if failure.is_conflict() => {
            settings.retire_flight(stamp, Status::Conflict)?;
            save(&root, &settings)
        }
        Err(failure) => {
            let status = if !failure.absent().is_empty() {
                Status::MissingReceipts
            } else if core::published(&settings.backup_root, &slot).unwrap_or(false) {
                Status::PublishedRetry
            } else {
                Status::Retry
            };
            retry(app, &root, &mut settings, status)
        }
    }
}

fn back_off(app: &mut App, settings: &Settings) {
    app.scheduled_retry = Some(Backoff {
        schedule_id: settings.schedule_id.clone(),
        until: Instant::now() + RETRY_DELAY,
    });
}

fn retry(app: &mut App, root: &Path, settings: &mut Settings, status: Status) -> AppResult<()> {
    back_off(app, settings);
    if !matches!(
        settings.status,
        Some(Status::Conflict | Status::ProtectionChanged)
    ) {
        settings.status = Some(status);
    }
    save(root, settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Outcome, hosting::HostRecord};
    use std::fs;

    fn at(date: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(date)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn before() -> DateTime<Utc> {
        at("2026-11-01T01:00:00Z")
    }
    fn after() -> DateTime<Utc> {
        at("2026-11-01T03:00:00Z")
    }
    fn missed() -> DateTime<Utc> {
        at("2026-11-05T19:00:00Z")
    }
    fn next_day() -> DateTime<Utc> {
        at("2026-11-06T19:00:00Z")
    }

    fn book() -> (tempfile::TempDir, App, CompanyId) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let mut app = App::new();
        app.backups.configure(&root).unwrap();
        let created = app
            .create_protected_book(
                &root.join("books/default.fbbook"),
                "Local Owner",
                "Synthetic Books",
                None,
            )
            .unwrap();
        app.enroll_owner("a synthetic password").unwrap();
        app.now = before;
        (tmp, app, created.company_id)
    }

    fn fields(time: &str, keep: &str) -> Vec<(String, String)> {
        [
            ("action", "automatic"),
            ("enabled", "1"),
            ("frequency", "daily"),
            ("time_utc", time),
            ("keep_count", keep),
        ]
        .into_iter()
        .map(|(name, value)| (name.into(), value.into()))
        .collect()
    }

    fn current(app: &App) -> Settings {
        load(app.backups.root().unwrap()).unwrap().unwrap()
    }

    fn tick(app: &mut App) {
        desktop_tick(app, app.session_generation()).unwrap();
    }

    fn audits(app: &App) -> i64 {
        app.open
            .as_ref()
            .unwrap()
            .book
            .connection()
            .query_row(
                "SELECT count(*) FROM ledger_auditevent WHERE action='backup.scheduled'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn daily_weekly_utc_rollover_and_local_dst_display() {
        let daily = Cadence::parse(Frequency::Daily, "02:00", None).unwrap();
        assert_eq!(daily.latest(before()), Some(at("2026-10-31T02:00:00Z")));
        assert_eq!(daily.next(before()), Some(at("2026-11-01T02:00:00Z")));
        assert_eq!(daily.latest(missed()), Some(at("2026-11-05T02:00:00Z")));
        let weekly = Cadence::parse(Frequency::Weekly, "02:00", Some(7)).unwrap();
        assert_eq!(weekly.latest(before()), Some(at("2026-10-25T02:00:00Z")));
        assert_eq!(weekly.latest(missed()), Some(at("2026-11-01T02:00:00Z")));
        assert_eq!(weekly.next(missed()), Some(at("2026-11-08T02:00:00Z")));
        assert_eq!(
            weekly.next(at("2026-12-31T23:59:59Z")),
            Some(at("2027-01-03T02:00:00Z"))
        );
        let ny = chrono_tz::America::New_York;
        assert_eq!(
            shown_local(at("2026-11-01T05:30:00Z"), &ny),
            "2026-11-01 01:30 (-04:00)"
        );
        assert_eq!(
            shown_local(at("2026-11-01T06:30:00Z"), &ny),
            "2026-11-01 01:30 (-05:00)"
        );
        assert!(Cadence::parse(Frequency::Weekly, "02:00", Some(0)).is_none());
        assert!(Cadence::parse(Frequency::Daily, "2:00", None).is_none());
        assert!(Cadence::parse(Frequency::Daily, "24:00", None).is_none());
    }

    #[test]
    fn starts_at_next_slot_coalesces_sleep_and_suppresses_rollback() {
        let (_dir, mut app, company) = book();
        configure(&mut app, company, &fields("02:00", "7")).unwrap();
        let first = at("2026-11-01T02:00:00Z").timestamp();
        assert_eq!(current(&app).first_slot, first);
        tick(&mut app);
        assert_eq!(audits(&app), 0);
        app.now = missed;
        tick(&mut app);
        let latest = at("2026-11-05T02:00:00Z").timestamp();
        assert_eq!(current(&app).last_completed_slot, Some(latest));
        assert_eq!(audits(&app), 1);
        for time in [after, before, missed] {
            app.now = time;
            tick(&mut app);
            assert_eq!(audits(&app), 1);
        }
        app.now = next_day;
        tick(&mut app);
        assert_eq!(audits(&app), 2);
    }

    #[test]
    fn resumes_published_in_flight_before_coalescing_missed_slots() {
        let (_dir, mut app, company) = book();
        configure(&mut app, company, &fields("02:00", "7")).unwrap();
        app.now = after;
        tick(&mut app);
        let root = app.backups.root().unwrap().to_owned();
        let mut state = current(&app);
        let first = state.last_completed_slot.take().unwrap();
        state.in_flight_slot = Some(first);
        state.in_flight_key =
            Some(core::key_tag(app.master_key().unwrap(), &state.schedule_id).unwrap());
        save(&root, &state).unwrap();
        app.now = missed;
        tick(&mut app);
        assert_eq!(audits(&app), 1);
        assert_eq!(current(&app).last_completed_slot, Some(first));
        assert!(current(&app).in_flight_slot.is_none());
        tick(&mut app);
        assert_eq!(audits(&app), 2);
        assert_eq!(
            current(&app).last_completed_slot,
            Some(at("2026-11-05T02:00:00Z").timestamp())
        );
    }

    #[test]
    fn defers_for_changed_session_remote_hosted_restored_or_maintenance() {
        let (_dir, mut app, company) = book();
        let root = app.backups.root().unwrap().to_owned();
        let main = app.book_path().unwrap().to_owned();
        configure(&mut app, company, &fields("02:00", "7")).unwrap();
        app.now = after;
        let old_generation = app.session_generation();
        app.sign_out();
        tick(&mut app);
        assert_eq!(audits(&app), 0);
        app.sign_in("Local Owner", "a synthetic password").unwrap();
        desktop_tick(&mut app, old_generation).unwrap();
        assert_eq!(audits(&app), 0);
        app.remote_session = true;
        tick(&mut app);
        assert!(!may_manage(&mut app));
        assert!(configure(&mut app, company, &fields("02:00", "7")).is_err());
        app.remote_session = false;
        assert_eq!(audits(&app), 0);
        app.mark_copy(true);
        tick(&mut app);
        app.mark_copy(false);
        assert_eq!(audits(&app), 0);
        let mut hosted = HostRecord::fresh().unwrap();
        hosted.enabled = true;
        app.save_host_record(&hosted).unwrap();
        tick(&mut app);
        assert_eq!(audits(&app), 0);
        hosted.enabled = false;
        app.save_host_record(&hosted).unwrap();
        let marker = crate::protection::sidecar(&main, ".rekey.json");
        fs::write(&marker, "synthetic interruption").unwrap();
        tick(&mut app);
        assert_eq!(audits(&app), 0);
        fs::remove_file(marker).unwrap();
        let route = format!("/c/{}/backups/", company.to_db());
        let restore_fields = vec![
            ("action".into(), "restore".into()),
            ("new_book".into(), "Synthetic Restored".into()),
        ];
        let restore = crate::screens::submit(&mut app, &route, &restore_fields).unwrap();
        let Outcome::Done { data } = restore else {
            panic!("expected native restore picker")
        };
        tick(&mut app);
        assert_eq!(audits(&app), 0);
        crate::screens::backups::dialog_result(&mut app, data["ticket"].as_str().unwrap(), "")
            .unwrap();
        app.lock();
        tick(&mut app);
        assert!(!app.is_open());
        assert!(load(&root).unwrap().unwrap().last_completed_slot.is_none());
        app.open_book(&main, &crate::protection::builtin_key())
            .unwrap();
        app.sign_in("Local Owner", "a synthetic password").unwrap();
        tick(&mut app);
        assert_eq!(audits(&app), 1);
    }

    #[test]
    fn remote_transport_and_bookkeeper_cannot_manage_automatic_backups() {
        use crate::multiuser::{ClientInfo, Policy, Request, Response, SessionHost};
        let (_dir, mut app, company) = book();
        let route = format!("/c/{}/backups/", company.to_db());
        app.save_desktop_user(
            None,
            "Synthetic Bookkeeper",
            Role::Bookkeeper,
            true,
            Some("another synthetic password"),
            &[company],
        )
        .unwrap();
        app.sign_in("Local Owner", "a synthetic password").unwrap();
        let (mut host, local) = SessionHost::host_with_local(
            app,
            Policy {
                remote_maintenance: true,
                ..Policy::default()
            },
            "Main computer",
        );
        let remote = host
            .open_session(ClientInfo::remote("Another computer", None))
            .unwrap();
        host.dispatch(
            &remote,
            Request::SignIn {
                username: "Local Owner".into(),
                password: "a synthetic password".into(),
            },
        )
        .unwrap();
        let Response::Page { html, .. } = host
            .dispatch(
                &remote,
                Request::Render {
                    target: route.clone(),
                },
            )
            .unwrap()
        else {
            panic!("expected the Backups page")
        };
        assert!(!html.contains("Automatic backups"));
        let token = host
            .with_session(&remote, |app| app.tokens.mint(&route))
            .unwrap();
        let command = Command::SubmitForm {
            path: route.clone(),
            token,
            fields: fields("02:00", "7"),
        };
        let response = host.dispatch(&remote, Request::Run { command }).unwrap();
        assert!(
            matches!(response, Response::Outcome { outcome: Outcome::Retry { message, token: None }, .. } if message.contains("local Administrator"))
        );
        assert!(
            !host
                .app()
                .backups
                .root()
                .unwrap()
                .join(SETTINGS_FILE)
                .exists()
        );
        host.with_session(&local, |app| {
            assert!(may_manage(app));
            app.sign_out();
            app.sign_in("Synthetic Bookkeeper", "another synthetic password")
                .unwrap();
            assert!(!may_manage(app));
            assert!(configure(app, company, &fields("02:00", "7")).is_err());
        })
        .unwrap();
    }

    #[test]
    fn folder_failure_backs_off_and_leaves_a_safe_retry_status() {
        let (_dir, mut app, company) = book();
        configure(&mut app, company, &fields("02:00", "7")).unwrap();
        app.now = after;
        let folder = current(&app).backup_root;
        fs::write(&folder, "this is a file, not a folder").unwrap();
        tick(&mut app);
        assert_eq!(current(&app).status, Some(Status::Retry));
        assert_eq!(audits(&app), 0);
        fs::remove_file(folder).unwrap();
        tick(&mut app);
        assert_eq!(
            audits(&app),
            0,
            "a fixed path still waits for the retry delay"
        );
        app.scheduled_retry.as_mut().unwrap().until = Instant::now();
        tick(&mut app);
        assert_eq!(audits(&app), 1);
        assert_eq!(current(&app).status, Some(Status::Verified));
    }

    #[test]
    fn configuration_form_and_managed_download_and_restore_use_real_paths() {
        let (_dir, mut app, company) = book();
        let route = format!("/c/{}/backups/", company.to_db());
        let root = app.backups.root().unwrap().to_owned();
        let page = crate::screens::render(&mut app, &route).unwrap().html;
        assert!(page.contains("Automatic backups") && !root.join(SETTINGS_FILE).exists());
        let token = app.tokens.mint(&route);
        app.run(Command::SubmitForm {
            path: route.clone(),
            token,
            fields: fields("02:00", "7"),
        })
        .unwrap();
        app.now = after;
        tick(&mut app);
        let state = current(&app);
        let slot = core::Slot::new(
            &state.schedule_id,
            instant(state.last_completed_slot.unwrap()).unwrap(),
        )
        .unwrap();
        let archive = slot.basename().unwrap();
        let page = crate::screens::render(&mut app, &route).unwrap().html;
        assert!(page.contains("2026-11-02 02:00 UTC") && page.contains(&archive));
        assert!(page.contains("Verified automatic backups") && page.contains("2026-11-01 02:00"));
        let export = app
            .run(Command::ExportBackup {
                company_id: company.to_db(),
                name: format!("scheduled.{archive}"),
            })
            .unwrap();
        let Outcome::Done { data } = export else {
            panic!("expected native download")
        };
        let destination = root.join("synthetic-export.fbbackup");
        crate::screens::backups::dialog_result(
            &mut app,
            data["ticket"].as_str().unwrap(),
            destination.to_str().unwrap(),
        )
        .unwrap();
        assert!(destination.is_file());
        let restore_fields = vec![
            ("action".into(), "restore".into()),
            ("new_book".into(), "Synthetic Restored".into()),
            ("scheduled_archive".into(), archive.clone()),
        ];
        let token = app.tokens.mint(&route);
        assert!(matches!(
            app.run(Command::SubmitForm {
                path: route.clone(),
                token,
                fields: restore_fields
            }),
            Ok(Outcome::Redirect { .. })
        ));
        assert!(
            crate::screens::render(&mut app, &route)
                .unwrap()
                .html
                .contains("Open restored book")
        );
        let original = state
            .backup_root
            .join("automatic")
            .join(&state.schedule_id)
            .join(&archive);
        assert_eq!(
            fs::read(&original).unwrap(),
            fs::read(&destination).unwrap()
        );
        assert!(
            fs::read_dir(root.join("work/managed-archives"))
                .unwrap()
                .next()
                .is_none()
        );
        let Outcome::Done { data } = app
            .run(Command::ExportBackup {
                company_id: company.to_db(),
                name: format!("scheduled.{archive}"),
            })
            .unwrap()
        else {
            panic!("expected native download");
        };
        fs::write(&original, b"synthetic swapped archive").unwrap();
        let refused_destination = root.join("refused-export.fbbackup");
        assert!(
            crate::screens::backups::dialog_result(
                &mut app,
                data["ticket"].as_str().unwrap(),
                refused_destination.to_str().unwrap(),
            )
            .is_err()
        );
        assert!(!refused_destination.exists());
        fs::remove_file(fb_core::backups::retention::sidecar(&original)).unwrap();
        let page = crate::screens::render(&mut app, &route).unwrap().html;
        assert!(!page.contains(&format!("scheduled.{archive}")));
        assert!(
            app.run(Command::ExportBackup {
                company_id: company.to_db(),
                name: format!("scheduled.{archive}")
            })
            .is_err()
        );
    }

    #[test]
    fn configuration_is_private_bounded_unique_and_rotates_only_for_new_cadence_or_folder() {
        let (_dir, mut app, company) = book();
        configure(&mut app, company, &fields("02:00", "7")).unwrap();
        let initial = current(&app);
        configure(&mut app, company, &fields("02:00", "1")).unwrap();
        assert_eq!(current(&app).schedule_id, initial.schedule_id);
        assert!(configure(&mut app, company, &fields("02:00", "91")).is_err());
        assert_eq!(current(&app).keep_count, 1);
        configure(&mut app, company, &fields("08:00", "1")).unwrap();
        let cadence_id = current(&app).schedule_id;
        assert_ne!(cadence_id, initial.schedule_id);
        let root = app.backups.root().unwrap();
        folder_changed(root, &root.join("different"), before()).unwrap();
        assert_ne!(current(&app).schedule_id, cadence_id);
        let path = root.join(SETTINGS_FILE);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let raw = fs::read_to_string(&path).unwrap();
        fs::write(&path, raw.replacen('{', "{\"version\":1,", 1)).unwrap();
        assert!(load(root).is_err());
        fs::write(&path, vec![b'x'; MAX_SETTINGS as usize + 1]).unwrap();
        assert!(load(root).is_err());
        #[cfg(unix)]
        {
            let displaced = root.join("displaced.json");
            fs::rename(&path, &displaced).unwrap();
            std::os::unix::fs::symlink(displaced, path).unwrap();
            assert!(load(root).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_profile_cannot_read_or_replace_schedule_settings() {
        let (_dir, mut app, company) = book();
        configure(&mut app, company, &fields("02:00", "7")).unwrap();
        let root = app.backups.root().unwrap().to_owned();
        let parent = tempfile::tempdir().unwrap();
        let link = parent.path().join("profile-link");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        let original = fs::read(root.join(SETTINGS_FILE)).unwrap();
        let mut changed = current(&app);
        changed.keep_count = 4;
        assert!(load(&link).is_err());
        assert!(save(&link, &changed).is_err());
        assert_eq!(fs::read(root.join(SETTINGS_FILE)).unwrap(), original);
    }

    fn password_prompt(app: &mut App, company: CompanyId) -> (String, PathBuf) {
        app.change_protection("", Some("Previous synthetic password"))
            .unwrap();
        configure(app, company, &fields("02:00", "7")).unwrap();
        app.now = after;
        tick(app);
        let state = current(app);
        let slot = core::Slot::new(
            &state.schedule_id,
            instant(state.last_completed_slot.unwrap()).unwrap(),
        )
        .unwrap();
        app.change_protection(
            "Previous synthetic password",
            Some("Current synthetic password"),
        )
        .unwrap();
        app.now = next_day;
        let archive = slot.basename().unwrap();
        let original = state
            .backup_root
            .join("automatic")
            .join(&state.schedule_id)
            .join(&archive);
        let route = format!("/c/{}/backups/", company.to_db());
        let fields = vec![
            ("action".into(), "restore".into()),
            ("new_book".into(), "Password Protected Copy".into()),
            ("scheduled_archive".into(), archive),
        ];
        let Outcome::Redirect { location } = crate::screens::submit(app, &route, &fields).unwrap()
        else {
            panic!("expected the restore password prompt");
        };
        assert_eq!(location, "/settings/restore-backup/");
        assert_eq!(crate::screens::render(app, &location).unwrap().status, 200);
        tick(app);
        assert_eq!(
            audits(app),
            1,
            "a live restore prompt defers the due backup"
        );
        (route, original)
    }

    #[test]
    fn automatic_backups_resume_when_the_restore_prompt_expires() {
        let (_dir, mut app, company) = book();
        password_prompt(&mut app, company);
        app.backups.expire_restore_prompt_for_test();
        tick(&mut app);
        assert_eq!(audits(&app), 2);
        assert!(crate::screens::render(&mut app, "/settings/restore-backup/").is_err());
    }

    #[test]
    fn leaving_the_restore_password_prompt_resumes_automatic_backups() {
        let (_dir, mut app, company) = book();
        let (route, _) = password_prompt(&mut app, company);
        assert_eq!(
            crate::screens::render(&mut app, &route).unwrap().status,
            200
        );
        tick(&mut app);
        assert_eq!(audits(&app), 2);
        assert!(crate::screens::render(&mut app, "/settings/restore-backup/").is_err());
    }

    #[test]
    fn a_managed_restore_refuses_an_archive_swapped_during_the_password_prompt() {
        let (_dir, mut app, company) = book();
        let (_, archive) = password_prompt(&mut app, company);
        fs::write(&archive, b"synthetic swapped archive").unwrap();
        let result = crate::screens::submit(
            &mut app,
            "/settings/restore-backup/",
            &[
                ("action".into(), "restore".into()),
                ("secret".into(), "Previous synthetic password".into()),
            ],
        )
        .unwrap();
        assert!(matches!(result, Outcome::Invalid { .. }));
        assert!(
            !app.backups
                .books()
                .unwrap()
                .join("restored-password-protected-copy")
                .exists()
        );
    }

    #[test]
    fn adopting_the_restored_copy_rotates_the_profile_schedule_and_preserves_archives() {
        let (_dir, mut app, company) = book();
        configure(&mut app, company, &fields("02:00", "1")).unwrap();
        app.now = after;
        tick(&mut app);
        let original = current(&app);
        let route = format!("/c/{}/backups/", company.to_db());
        let slot = core::Slot::new(
            &original.schedule_id,
            instant(original.last_completed_slot.unwrap()).unwrap(),
        )
        .unwrap();
        let archive = original
            .backup_root
            .join("automatic")
            .join(&original.schedule_id)
            .join(slot.basename().unwrap());
        crate::screens::submit(
            &mut app,
            &route,
            &[
                ("action".into(), "restore".into()),
                ("new_book".into(), "Replacement Books".into()),
                ("scheduled_archive".into(), slot.basename().unwrap()),
            ],
        )
        .unwrap();
        crate::screens::submit(&mut app, &route, &[("action".into(), "open".into())]).unwrap();
        app.sign_in("Local Owner", "a synthetic password").unwrap();
        app.make_open_book_main(Some(&original.book_path)).unwrap();
        let changed = current(&app);
        assert_ne!(changed.schedule_id, original.schedule_id);
        assert_eq!(changed.book_path, app.book_path().unwrap());
        assert_eq!(changed.first_slot, at("2026-11-02T02:00:00Z").timestamp());
        assert_eq!(changed.last_completed_slot, None);
        assert!(archive.exists());
    }

    fn publish_interrupted(
        app: &mut App,
        company: CompanyId,
        root: &Path,
        state: &mut Settings,
    ) -> PathBuf {
        let old_key = crate::keys::MasterKey::new(**app.master_key().unwrap());
        let receipts = app.receipt_key().unwrap();
        let slot = core::Slot::new(&state.schedule_id, instant(state.first_slot).unwrap()).unwrap();
        let replica_path = root.join("interrupted-capture.fbbook");
        let session = app.authenticated_session().unwrap();
        app.open
            .as_ref()
            .unwrap()
            .book
            .snapshot(&replica_path, &old_key)
            .unwrap();
        let mut replica = fb_db::Book::open(&replica_path, &old_key).unwrap();
        // The clone publishes with the current keys and company IDs. Its Writer does not
        // put an event in the live book, modelling interruption before the live audit.
        let worker = core::Worker {
            root: &state.backup_root,
            protected: std::slice::from_ref(&state.book_path),
            copy: fb_core::backups::CopyWorker {
                key: &old_key,
                receipt_key: &receipts,
                receipts_root: None,
                envelope: app.protection.as_ref().map(|record| &record.keys),
                app_version: "test",
            },
        };
        let previous = worker
            .run(&mut replica, session.actor, company, &slot, after())
            .unwrap()
            .path;
        fs::remove_file(fb_core::backups::retention::sidecar(&previous)).unwrap();
        state.in_flight_slot = Some(state.first_slot);
        state.in_flight_key = Some(core::key_tag(&old_key, &state.schedule_id).unwrap());
        save(root, state).unwrap();
        previous
    }

    #[test]
    fn a_conflicting_published_slot_is_preserved_and_later_slots_can_complete() {
        let (_dir, mut app, company) = book();
        configure(&mut app, company, &fields("02:00", "1")).unwrap();
        let root = app.backups.root().unwrap().to_owned();
        let mut state = current(&app);
        let previous = publish_interrupted(&mut app, company, &root, &mut state);
        let original = fs::read(&previous).unwrap();
        app.lock();
        app.open_book(&state.book_path, &crate::protection::builtin_key())
            .unwrap();
        app.sign_in("Local Owner", "a synthetic password").unwrap();
        let actor = app.authenticated_session().unwrap().actor;
        fb_core::services::create_company(
            &mut app.open_mut().unwrap().book,
            actor,
            "A new company",
            "USD",
            false,
        )
        .unwrap();

        app.now = after;
        tick(&mut app);
        let retired = current(&app);
        assert!(retired.in_flight_slot.is_none());
        assert!(retired.last_completed_slot.is_none());
        assert_eq!(retired.status, Some(Status::Conflict));
        assert!(
            view(&mut app).unwrap()["status"]
                .as_str()
                .unwrap()
                .contains("Its file was kept.")
        );
        tick(&mut app);
        assert_eq!(current(&app).status, Some(Status::Conflict));
        assert_eq!(audits(&app), 0);
        assert_eq!(fs::read(&previous).unwrap(), original);

        app.now = next_day;
        tick(&mut app);
        let settled = current(&app);
        assert_eq!(settled.status, Some(Status::Verified));
        assert_eq!(audits(&app), 1);
        let newer = core::Slot::new(
            &settled.schedule_id,
            instant(settled.last_completed_slot.unwrap()).unwrap(),
        )
        .unwrap();
        assert!(
            core::resolve(
                &mut app.open_mut().unwrap().book,
                &settled.backup_root,
                &newer
            )
            .unwrap()
            .is_some()
        );
        assert_eq!(fs::read(&previous).unwrap(), original);
    }

    #[test]
    fn rekey_preserves_an_unaudited_old_key_archive_and_allows_later_slots() {
        let (_dir, mut app, company) = book();
        configure(&mut app, company, &fields("02:00", "1")).unwrap();
        let root = app.backups.root().unwrap().to_owned();
        let mut state = current(&app);
        let previous = publish_interrupted(&mut app, company, &root, &mut state);
        app.change_protection("", Some("Synthetic new book password"))
            .unwrap();
        app.now = missed;
        tick(&mut app);
        assert_eq!(current(&app).status, Some(Status::ProtectionChanged));
        assert_eq!(audits(&app), 0);
        tick(&mut app);
        assert_eq!(audits(&app), 1);
        assert_eq!(current(&app).schedule_id, state.schedule_id);
        assert!(
            previous.exists(),
            "retention leaves the unaudited old-key file alone"
        );
    }

    #[test]
    fn rekey_keeps_the_schedule_and_the_next_archive_uses_the_new_envelope() {
        let (_dir, mut app, company) = book();
        configure(&mut app, company, &fields("02:00", "2")).unwrap();
        app.now = after;
        tick(&mut app);
        let old = current(&app);
        app.change_protection("", Some("Synthetic new book password"))
            .unwrap();
        assert_eq!(current(&app).schedule_id, old.schedule_id);
        app.now = missed;
        tick(&mut app);
        assert_eq!(audits(&app), 2);
        let archives =
            core::inventory(&mut app.open.as_mut().unwrap().book, &old.backup_root, 10).unwrap();
        let newest = crate::screens::backups::archive_keys(&archives[0].path)
            .unwrap()
            .unwrap();
        assert!(newest.unlock("Synthetic new book password").is_ok());
        let first = crate::screens::backups::archive_keys(&archives[1].path)
            .unwrap()
            .unwrap();
        assert_eq!(first.mode, fb_core::backups::portable::Mode::BuiltIn);
    }
}
