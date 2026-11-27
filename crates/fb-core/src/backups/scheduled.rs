//! Verified, no-clobber publication of one UTC slot. The desktop persists the
//! in-flight slot before calling this worker and marks it complete after audit.
use super::{BackupError, CopyTarget, CopyWorker, Manifest, Result, audit, paths, retention};
use crate::{CompanyId, UserId};
use chrono::{DateTime, Datelike, NaiveDateTime, SecondsFormat, Timelike, Utc};
use fb_db::Book;
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Slot {
    schedule_id: String,
    at: DateTime<Utc>,
}

pub fn valid_id(id: &str) -> bool {
    super::hex_valid(id, 32)
}

impl Slot {
    pub fn new(id: &str, at: DateTime<Utc>) -> Result<Self> {
        if !valid_id(id)
            || !(0..=9999).contains(&at.year())
            || at.second() != 0
            || at.nanosecond() != 0
        {
            return Err(BackupError::refused("Invalid automatic backup slot."));
        }
        Ok(Self {
            schedule_id: id.into(),
            at,
        })
    }

    pub fn from_basename(name: &str) -> Result<Self> {
        let (id, stamp) = name
            .strip_prefix("freedombooks-auto-")
            .and_then(|name| name.strip_suffix(".fbbackup"))
            .and_then(|name| name.split_once('-'))
            .ok_or_else(|| BackupError::refused("Invalid automatic backup filename."))?;
        let at = NaiveDateTime::parse_from_str(stamp, "%Y%m%dT%H%M%SZ")
            .map_err(|_| BackupError::refused("Invalid automatic backup filename."))?;
        let slot = Self::new(id, at.and_utc())?;
        if slot.basename()? != name {
            return Err(BackupError::refused("Invalid automatic backup filename."));
        }
        Ok(slot)
    }

    pub(super) fn identity(&self) -> Value {
        json!({"schedule_id": self.schedule_id, "slot": self.stamp()})
    }

    pub fn schedule_id(&self) -> &str {
        &self.schedule_id
    }

    pub fn stamp(&self) -> String {
        utc_stamp(self.at)
    }

    fn label(&self) -> String {
        format!("auto-{}", self.schedule_id)
    }

    pub fn basename(&self) -> Result<String> {
        retention::filename(&self.label(), &self.stamp())
    }

    pub(super) fn check_capture(
        &self,
        header: &Value,
        manifest: &Manifest,
        companies: &[String],
    ) -> Result<()> {
        // Same key (authenticated manifest), exact slot marker, and current live company IDs.
        let mut captured: Vec<_> = manifest.contents.companies.iter().map(|c| &c.id).collect();
        captured.sort_unstable();
        if !manifest.complete()
            || header["scheduled"] != self.identity()
            || !captured.into_iter().eq(companies)
        {
            return Err(BackupError::refused(
                "The automatic backup does not match this slot and these books.",
            ));
        }
        Ok(())
    }
}

/// Private profile tag identifying the encryption key an interrupted capture used.
pub fn key_tag(key: &fb_db::Key, schedule_id: &str) -> Result<String> {
    use sha2::Sha256;
    let mut tag = [0u8; 32];
    hkdf::Hkdf::<Sha256>::new(Some(schedule_id.as_bytes()), key)
        .expand(b"FreedomBooks automatic backup key v1", &mut tag)
        .map_err(|_| BackupError::refused("Automatic backup key binding failed."))?;
    Ok(tag.iter().map(|b| format!("{b:02x}")).collect())
}

pub fn utc_stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Checked evidence for the exact archive and its book snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Evidence {
    pub slot: Slot,
    pub bytes: u64,
    pub sha256: String,
    pub book_sha256: String,
}

pub struct Verified {
    pub path: PathBuf,
    pub verified_at: String,
    evidence: Evidence,
}

pub struct StagedArchive {
    path: PathBuf,
    _work: tempfile::TempDir,
}

impl StagedArchive {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Verified {
    /// Consume a private copy of the audited bytes. Only the safely opened
    /// source descriptor is used for copying, even if its path is replaced.
    pub fn stage(&self, work: &Path) -> Result<StagedArchive> {
        let evidence = &self.evidence;
        let mismatch = || {
            BackupError::conflict(
                "The automatic backup changed or no longer matches its verification record.",
            )
        };
        let mut source = paths::regular(&self.path)?;
        let meta = source.metadata()?;
        let note = retention::read_note(&self.path)?;
        if meta.len() != evidence.bytes
            || checked_note(&note, evidence, &evidence.slot.basename()?).is_none()
            || !retention::note_matches(&note, &meta, evidence.bytes, &evidence.sha256)?
        {
            return Err(mismatch());
        }

        let work = super::stage(work, &[&self.path])?;
        let path = work.path().join(evidence.slot.basename()?);
        let mut file = paths::new_file(&path)?;
        io::copy(
            &mut source.by_ref().take(evidence.bytes.saturating_add(1)),
            &mut file,
        )?;
        drop(file);
        let (bytes, sha256) = super::hash_file(&path)?;
        if bytes != evidence.bytes
            || sha256 != evidence.sha256
            || !checked_header(&path, &evidence.slot)?
        {
            return Err(mismatch());
        }
        Ok(StagedArchive { path, _work: work })
    }
}

pub struct Worker<'a> {
    pub root: &'a Path,
    pub protected: &'a [PathBuf],
    pub copy: CopyWorker<'a>,
}

fn schedule_dir(root: &Path, id: &str) -> Result<PathBuf> {
    if !valid_id(id) || !root.is_absolute() {
        return Err(BackupError::refused("Invalid automatic backup folder."));
    }
    paths::resolve(&root.join("automatic").join(id))
}

fn check_private_dir(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
        Ok(meta) => {
            if !meta.is_dir() {
                return Err(BackupError::refused(
                    "Expected a private automatic backup folder.",
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if meta.uid() != rustix::process::getuid().as_raw() || meta.mode() & 0o077 != 0 {
                    return Err(BackupError::refused(
                        "The automatic backup folder must be private.",
                    ));
                }
            }
            Ok(true)
        }
    }
}

fn candidate_path(root: &Path, slot: &Slot) -> Result<Option<PathBuf>> {
    let dir = schedule_dir(root, slot.schedule_id())?;
    if !check_private_dir(&root.join("automatic"))? || !check_private_dir(&dir)? {
        return Ok(None);
    }
    Ok(Some(dir.join(slot.basename()?)))
}

/// Includes conflicting files so an interrupted slot will report them before
/// skipping on to a newer slot.
pub fn published(root: &Path, slot: &Slot) -> Result<bool> {
    let Some(path) = candidate_path(root, slot)? else {
        return Ok(false);
    };
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

impl Worker<'_> {
    pub fn run(
        &self,
        book: &mut Book,
        actor: UserId,
        company: CompanyId,
        slot: &Slot,
        captured_at: DateTime<Utc>,
    ) -> Result<Verified> {
        let companies = book
            .read(|q| {
                audit::scheduled_administrator(q, actor, company)?;
                let mut st = q.prepare("SELECT id FROM ledger_company ORDER BY id")?;
                Ok::<_, crate::FbError>(
                    st.query_map([], |row| row.get::<_, String>(0))?
                        .collect::<std::result::Result<Vec<_>, _>>()?,
                )
            })
            .map_err(|e| BackupError::refused(e.to_string()))?;
        super::check_backup_folder(
            self.root,
            self.protected
                .iter()
                .map(PathBuf::as_path)
                .chain(self.copy.receipts_root),
        )?;
        let dir = schedule_dir(self.root, slot.schedule_id())?;
        let root = paths::private_dir(self.root)?;
        let automatic = root.join("automatic");
        paths::private_dir(&automatic)?;
        paths::private_dir(&dir)?;
        paths::sync_dir(&automatic)?;
        paths::sync_dir(&root)?;
        if let Some(parent) = root.parent() {
            paths::sync_dir(parent)?;
        }
        let lock = paths::lock_file(&dir.join(".fb-backup.lock"))?;
        fs2::FileExt::try_lock_exclusive(&lock)
            .map_err(|_| BackupError::refused("Another automatic backup is already running."))?;
        if !super::cleanup::cleanup_stale(&dir)?.is_empty() {
            paths::sync_dir(&dir)?;
        }

        let path = dir.join(slot.basename()?);
        let existing =
            audit::scheduled_evidence(book, Some(slot.schedule_id()), Some(&slot.stamp()))?
                .remove(slot);
        let (evidence, verified_at) = match existing {
            Some(evidence) if let Some(verified_at) = verify_evidence(&path, &evidence)? => {
                (evidence, verified_at)
            }
            existing => self.capture(
                book,
                slot,
                captured_at,
                &companies,
                &path,
                existing.as_ref(),
            )?,
        };
        audit::record_scheduled(book, actor, company, &evidence)?;
        Ok(Verified {
            path,
            verified_at,
            evidence,
        })
    }

    fn capture(
        &self,
        book: &dyn fb_db::SnapshotSource,
        slot: &Slot,
        captured_at: DateTime<Utc>,
        companies: &[String],
        path: &Path,
        prior: Option<&Evidence>,
    ) -> Result<(Evidence, String)> {
        let already_published = match fs::symlink_metadata(path) {
            Ok(meta) if meta.is_file() => true,
            Ok(_) => {
                return Err(BackupError::conflict(
                    "The automatic backup filename is occupied by an unexpected file.",
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(e) => return Err(e.into()),
        };
        if !already_published && prior.is_some() {
            return Err(BackupError::conflict(
                "The recorded automatic backup is missing.",
            ));
        }
        let staging = super::stage_in(self.root)?;
        let result = (|| {
            let work = staging.path().join("work");
            let archive = if already_published {
                path.to_owned()
            } else {
                let output =
                    paths::private_dir(&staging.path().join("archive"))?.join(slot.basename()?);
                self.copy.write_slot(
                    book,
                    &CopyTarget {
                        output: &output,
                        work_root: &work,
                        label: &slot.label(),
                        created_at: &utc_stamp(captured_at),
                        allow_incomplete: false,
                    },
                    Some(slot),
                )?;
                output
            };
            let super::CheckedBackup {
                manifest,
                fingerprint,
                summary,
            } = self
                .copy
                .check_copy(&archive, &work, Some((slot, companies)))
                .map_err(|e| {
                    if already_published {
                        e.archive_conflict()
                    } else {
                        e
                    }
                })?;
            let evidence = Evidence {
                slot: slot.clone(),
                bytes: fingerprint.0,
                sha256: fingerprint.1.clone(),
                book_sha256: manifest.book_sha256,
            };
            if prior.is_some_and(|prior| prior != &evidence) {
                return Err(BackupError::conflict(
                    "The automatic backup conflicts with its existing audit record.",
                ));
            }
            if !already_published {
                super::publish(&archive, path)?;
            } else {
                paths::sync_dir(
                    path.parent()
                        .ok_or_else(|| BackupError::refused("Missing backup folder."))?,
                )?;
            }
            let mut note = summary
                .ok_or_else(|| BackupError::refused("Missing automatic backup verification."))?;
            note["scheduled"] = slot.identity();
            note["book_sha256"] = json!(evidence.book_sha256);
            let verified_at = note["verified_at"].as_str().unwrap_or_default().to_owned();
            retention::record_verified_noclobber(path, &fingerprint, note)?;
            Ok((evidence, verified_at))
        })();
        // Only this operation's staging directory is removed. On interruption
        // the same dead-process cleanup as normal backup can retire it later.
        let cleanup = staging
            .close()
            .map_err(BackupError::from)
            .and_then(|()| paths::sync_dir(self.root));
        result.and_then(|value| cleanup.map(|()| value))
    }
}

fn checked_header(path: &Path, slot: &Slot) -> Result<bool> {
    let header = super::read_header(path)?;
    Ok(header.get("incomplete").is_none() && header["scheduled"] == slot.identity())
}

fn checked_note<'a>(note: &'a Value, evidence: &Evidence, basename: &str) -> Option<&'a str> {
    if note["complete"] != true
        || note["scheduled"] != evidence.slot.identity()
        || note["file"] != basename
        || note["sha256"] != evidence.sha256
        || note["bytes"] != evidence.bytes
        || note["book_sha256"] != evidence.book_sha256
    {
        return None;
    }
    let time = note["verified_at"].as_str()?;
    DateTime::parse_from_rfc3339(time).ok()?;
    Some(time)
}

fn verify_evidence(path: &Path, evidence: &Evidence) -> Result<Option<String>> {
    let metadata = |path: &Path| match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => Ok(Some(meta)),
        Ok(_) => Ok(None),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(BackupError::from(e)),
    };
    let Some(meta) = metadata(path)? else {
        return Ok(None);
    };
    if metadata(&retention::sidecar(path))?.is_none() {
        return Ok(None);
    }
    let result = (|| {
        let note = retention::read_note(path)?;
        let Some(time) = checked_note(&note, evidence, &evidence.slot.basename()?) else {
            return Ok(None);
        };
        let (bytes, sha) = super::hash_file(path)?;
        if !retention::note_matches(&note, &meta, bytes, &sha)?
            || !checked_header(path, &evidence.slot)?
        {
            return Ok(None);
        }
        Ok(Some(time.to_owned()))
    })();
    match result {
        Err(BackupError {
            failure: super::Failure::Validation,
            ..
        }) => Ok(None),
        other => other,
    }
}

fn checked_header_file(file: fs::File, slot: &Slot) -> Result<bool> {
    let header = super::read_header_file(file)?;
    Ok(header.get("incomplete").is_none() && header["scheduled"] == slot.identity())
}

fn verify_evidence_in_dir(
    dir: &paths::Directory,
    archive_name: &str,
    note_name: &str,
    evidence: &Evidence,
) -> Result<Option<String>> {
    let result = (|| {
        let Some(note_file) = dir.read_file(note_name)? else {
            return Ok(None);
        };
        let note = retention::read_note_file(note_file)?;
        let Some(time) = checked_note(&note, evidence, &evidence.slot.basename()?) else {
            return Ok(None);
        };
        let Some(archive_file) = dir.read_file(archive_name)? else {
            return Ok(None);
        };
        let meta = archive_file.metadata()?;
        let (bytes, sha) = super::hash_open_file(archive_file)?;
        if !retention::note_matches(&note, &meta, bytes, &sha)? {
            return Ok(None);
        }
        let Some(header_file) = dir.read_file(archive_name)? else {
            return Ok(None);
        };
        let header_meta = header_file.metadata()?;
        if !retention::note_matches(&note, &header_meta, bytes, &sha)?
            || !checked_header_file(header_file, &evidence.slot)?
        {
            return Ok(None);
        }
        Ok(Some(time.to_owned()))
    })();
    match result {
        Err(BackupError {
            failure: super::Failure::Validation,
            ..
        }) => Ok(None),
        other => other,
    }
}

fn verified(path: &Path, evidence: &Evidence) -> Option<String> {
    verify_evidence(path, evidence).ok().flatten()
}

/// Resolve a typed selection using the same audit and file checks as retention.
pub fn resolve(book: &mut Book, root: &Path, slot: &Slot) -> Result<Option<Verified>> {
    let mut evidence =
        audit::scheduled_evidence(book, Some(slot.schedule_id()), Some(&slot.stamp()))?;
    let Some(path) = candidate_path(root, slot)? else {
        return Ok(None);
    };
    let Some(evidence) = evidence.remove(slot) else {
        return Ok(None);
    };
    Ok(
        verify_evidence(&path, &evidence)?.map(|verified_at| Verified {
            path,
            verified_at,
            evidence,
        }),
    )
}

/// Current book's verified automatic backups in the configured root, newest first.
pub fn inventory(book: &mut Book, root: &Path, limit: usize) -> Result<Vec<Verified>> {
    let mut evidence: Vec<_> = audit::scheduled_evidence(book, None, None)?
        .into_values()
        .collect();
    evidence.sort_unstable_by(|a, b| b.slot.at.cmp(&a.slot.at).then_with(|| a.slot.cmp(&b.slot)));
    let mut files = Vec::new();
    for e in evidence {
        if files.len() >= limit {
            break;
        }
        if let Some(path) = candidate_path(root, &e.slot)?
            && let Some(verified_at) = verified(&path, &e)
        {
            files.push(Verified {
                path,
                verified_at,
                evidence: e,
            });
        }
    }
    Ok(files)
}

/// Called only after a successful slot. Exact-label inventory and evidence form
/// the candidate filter, so unknown or incomplete files never consume retention.
pub fn prune(book: &mut Book, root: &Path, completed: &Slot, keep: usize) -> Result<Vec<PathBuf>> {
    let id = completed.schedule_id();
    if !(1..=90).contains(&keep) {
        return Err(BackupError::refused(
            "Keep between 1 and 90 automatic backups.",
        ));
    }
    let dir = schedule_dir(root, id)?;
    if !check_private_dir(&root.join("automatic"))? || !check_private_dir(&dir)? {
        return Ok(Vec::new());
    }
    let pinned = paths::Directory::open(&dir)?;
    let evidence = audit::scheduled_evidence(book, Some(id), None)?;
    let completed_name = completed.basename()?;
    let completed_note = format!("{completed_name}.verified.json");
    if !evidence.get(completed).is_some_and(|row| {
        verify_evidence_in_dir(&pinned, &completed_name, &completed_note, row)
            .ok()
            .flatten()
            .is_some()
    }) {
        return Err(BackupError::refused(
            "The just-completed automatic backup could not be confirmed for retention.",
        ));
    }
    let mut rows: Vec<&Evidence> = evidence.values().collect();
    rows.sort_unstable_by(|a, b| b.slot.at.cmp(&a.slot.at).then_with(|| b.slot.cmp(&a.slot)));
    let candidates = rows.into_iter().filter_map(|row| {
        let name = row.slot.basename().ok()?;
        let note_name = format!("{name}.verified.json");
        verify_evidence_in_dir(&pinned, &name, &note_name, row)
            .ok()
            .flatten()?;
        Some((name, row))
    });
    let mut removed = Vec::new();
    for (name, row) in candidates.skip(keep) {
        if prune_candidate(&pinned, &name, row)? {
            removed.push(dir.join(&name));
        }
    }
    pinned.sync()?;
    Ok(removed)
}

fn prune_candidate(dir: &paths::Directory, archive_name: &str, row: &Evidence) -> Result<bool> {
    retention::quarantine_remove(dir, archive_name, |q_dir, archive_name, note_name| {
        verify_evidence_in_dir(q_dir, archive_name, note_name, row)
            .ok()
            .flatten()
            .is_some()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        attachments::{self, Limits, Owner, storage::Store},
        services, users,
    };
    use fb_db::rusqlite::params;

    const SCHEDULE: &str = "abe171743484492cbbd287790b998823";
    const OTHER: &str = "5db74b6867dc458d8975d4b4517f6af2";
    const KEY: fb_db::Key = [42; 32];
    const RECEIPT_KEY: fb_db::Key = [28; 32];
    const PDF: &[u8] = b"%PDF-1.4\n1 0 obj<</Type/Catalog>>endobj\n%%EOF\n";

    fn time(day: u32, hour: u32) -> DateTime<Utc> {
        use chrono::TimeZone;
        Utc.with_ymd_and_hms(2026, 11, day, hour, 0, 0).unwrap()
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        live: PathBuf,
        receipts: PathBuf,
        book: Book,
        actor: UserId,
        company: CompanyId,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let base = dir.path().canonicalize().unwrap();
            let live = base.join("live");
            fs::create_dir(&live).unwrap();
            let mut book = Book::create(&live.join("main.fbbook"), &KEY).unwrap();
            book.connection()
                .execute_batch("PRAGMA wal_autocheckpoint=0")
                .unwrap();
            let actor = users::create_user(&mut book, "Synthetic Administrator", true).unwrap();
            book.write(|tx| tx.execute("INSERT INTO desktop_user(user_id,role,password_hash) VALUES(?,'administrator',?)",
                params![actor.to_db(), format!("$argon2id$v=19${}", "a".repeat(80))])).unwrap();
            let company =
                services::create_company(&mut book, actor, "Scheduled Books", "USD", false)
                    .unwrap()
                    .id;
            Self {
                _dir: dir,
                root: base.join("backups"),
                receipts: live.join("receipts"),
                live,
                book,
                actor,
                company,
            }
        }

        fn run(&mut self, slot: &Slot, at: DateTime<Utc>) -> Result<Verified> {
            Worker {
                root: &self.root,
                protected: std::slice::from_ref(&self.live),
                copy: CopyWorker {
                    key: &KEY,
                    receipt_key: &RECEIPT_KEY,
                    receipts_root: Some(&self.receipts),
                    envelope: Some(
                        &super::super::portable::KeyEnvelope::new(
                            &KEY,
                            &RECEIPT_KEY,
                            Some("a synthetic password"),
                        )
                        .unwrap(),
                    ),
                    app_version: "test",
                },
            }
            .run(&mut self.book, self.actor, self.company, slot, at)
        }

        fn archive(&self, slot: &Slot) -> PathBuf {
            self.root
                .join("automatic")
                .join(slot.schedule_id())
                .join(slot.basename().unwrap())
        }

        fn audit_count(&self) -> i64 {
            self.book
                .connection()
                .query_row(
                    "SELECT count(*) FROM ledger_auditevent WHERE action='backup.scheduled'",
                    [],
                    |r| r.get(0),
                )
                .unwrap()
        }

        fn add_receipt(&mut self) -> Value {
            let cash = services::save_account(
                &mut self.book,
                self.actor,
                self.company,
                services::AccountInput::new("1000", "Cash").kind("ASSET"),
            )
            .unwrap();
            let equity = services::save_account(
                &mut self.book,
                self.actor,
                self.company,
                services::AccountInput::new("3000", "Equity").kind("EQUITY"),
            )
            .unwrap();
            let journal = services::save_draft(
                &mut self.book,
                self.actor,
                self.company,
                services::DraftInput::new(
                    "2026-11-01",
                    "Synthetic draft",
                    vec![
                        services::line(cash.id).debit("12.34").credit("0").build(),
                        services::line(equity.id).debit("0").credit("12.34").build(),
                    ],
                ),
            )
            .unwrap();
            let store = Store::for_book(&self.live.join("main.fbbook"), &RECEIPT_KEY).unwrap();
            attachments::add(
                &mut self.book,
                &store,
                self.actor,
                self.company,
                &Owner::Journal(journal.id),
                PDF,
                "synthetic.pdf",
                &uuid::Uuid::new_v4().to_string(),
                &Limits::default(),
            )
            .unwrap()
            .0
        }
    }

    #[test]
    fn restart_reuses_both_crash_windows_and_audit_is_idempotent() {
        let mut f = Fixture::new();
        let envelope = super::super::portable::KeyEnvelope::new(
            &KEY,
            &RECEIPT_KEY,
            Some("a synthetic password"),
        )
        .unwrap();
        for (day, sidecar_survived) in [(2, false), (3, true)] {
            let slot = Slot::new(SCHEDULE, time(day, 2)).unwrap();
            let path = f.archive(&slot);
            paths::private_dir(path.parent().unwrap()).unwrap();
            let worker = Worker {
                root: &f.root,
                protected: std::slice::from_ref(&f.live),
                copy: CopyWorker {
                    key: &KEY,
                    receipt_key: &RECEIPT_KEY,
                    receipts_root: Some(&f.receipts),
                    envelope: Some(&envelope),
                    app_version: "test",
                },
            };
            assert!(
                worker
                    .capture(
                        &f.book,
                        &slot,
                        time(day, 3),
                        &[f.company.to_db()],
                        &path,
                        None
                    )
                    .is_ok()
            );
            let before = super::super::hash_file(&path).unwrap();
            if !sidecar_survived {
                fs::remove_file(retention::sidecar(&path)).unwrap();
            }
            assert!(
                resolve(&mut f.book, &f.root, &slot).unwrap().is_none(),
                "uncommitted files stay out of the picker"
            );
            let verified = f.run(&slot, time(day, 4)).unwrap();
            assert_eq!(verified.path, path);
            assert_eq!(super::super::hash_file(&path).unwrap(), before);
            assert_eq!(f.audit_count(), i64::from(day - 1));
            f.run(&slot, time(day, 5)).unwrap();
            assert_eq!(f.audit_count(), i64::from(day - 1));
            assert!(resolve(&mut f.book, &f.root, &slot).unwrap().is_some());
            let mut evidence =
                audit::scheduled_evidence(&mut f.book, Some(SCHEDULE), Some(&slot.stamp()))
                    .unwrap()
                    .remove(&slot)
                    .unwrap();
            evidence.sha256 = "0".repeat(64);
            assert!(audit::record_scheduled(&mut f.book, f.actor, f.company, &evidence).is_err());
        }
    }

    #[test]
    fn encrypted_receipts_and_committed_wal_restore_from_a_scheduled_archive() {
        let mut f = Fixture::new();
        let receipt = f.add_receipt();
        let slot = Slot::new(SCHEDULE, time(5, 2)).unwrap();
        let created = f.run(&slot, time(5, 8)).unwrap();
        assert_eq!(
            created.path.file_name().unwrap(),
            "freedombooks-auto-abe171743484492cbbd287790b998823-20261105T020000Z.fbbackup"
        );
        assert!(f.live.join("main.fbbook-wal").metadata().unwrap().len() > 0);
        let base = f.live.parent().unwrap();
        let restored_dir = base.join("restored");
        fs::create_dir(&restored_dir).unwrap();
        let target = restored_dir.join("restored.fbbook");
        let receipts = restored_dir.join("receipts");
        let verifier = super::super::attachment_receipt_verifier_with(&RECEIPT_KEY, Some(&KEY));
        let restore = super::super::restore_backup(
            &created.path,
            &KEY,
            &super::super::RestoreOptions {
                target: &target,
                work_root: &base.join("restore-work"),
                receipts_target: Some(&receipts),
                verify_receipt: Some(&verifier),
                protected: &[f.live.clone(), f.root.clone()],
                passphrase: None,
                require_encrypted: false,
            },
        )
        .unwrap();
        assert_eq!(restore.manifest.created_at, "2026-11-05T08:00:00Z");
        assert_eq!(restore.manifest.migrations.len(), 98);
        let mut restored = Book::open(&target, &KEY).unwrap();
        let stored = Store::for_book(&target, &RECEIPT_KEY).unwrap();
        let id = receipt["id"].as_str().unwrap();
        assert_eq!(
            attachments::download(&mut restored, &stored, f.actor, f.company, id).unwrap(),
            PDF
        );
        let (actor, company, target, bytes, digest, book_digest): (String, String, String, i64, String, String) =
            f.book.connection().query_row("SELECT actor_id, company_id, target, json_extract(detail,'$.bytes'), json_extract(detail,'$.archive_sha256'), json_extract(detail,'$.book_sha256') FROM ledger_auditevent WHERE action=?",
                params!["backup.scheduled"], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).unwrap();
        assert_eq!(actor, f.actor.to_db());
        assert_eq!(company, f.company.to_db());
        assert_eq!(target, format!("backup:{}", slot.basename().unwrap()));
        assert_eq!(
            (u64::try_from(bytes).unwrap(), digest),
            super::super::hash_file(&created.path).unwrap()
        );
        assert_eq!(book_digest, restore.manifest.book_sha256);
    }

    #[test]
    fn retention_counts_only_verified_and_audited_files_of_the_exact_schedule() {
        let mut f = Fixture::new();
        let old = Slot::new(SCHEDULE, time(8, 2)).unwrap();
        let latest = Slot::new(SCHEDULE, time(9, 2)).unwrap();
        let other = Slot::new(OTHER, time(7, 2)).unwrap();
        for slot in [&old, &latest, &other] {
            f.run(slot, time(9, 8)).unwrap();
        }
        let old_file = f.archive(&old);
        let latest_file = f.archive(&latest);
        let dir = latest_file.parent().unwrap();
        let manual = f
            .root
            .join(retention::filename("main", &utc_stamp(time(6, 2))).unwrap());
        let safety =
            dir.join(retention::filename("main-before-upgrade", &utc_stamp(time(6, 2))).unwrap());
        let fake = f.archive(&Slot::new(SCHEDULE, time(10, 2)).unwrap());
        for path in [&manual, &safety, &fake] {
            fs::copy(&latest_file, path).unwrap();
            retention::record_verified(path).unwrap();
        }
        let tampered = Slot::new(SCHEDULE, time(11, 2)).unwrap();
        f.run(&tampered, time(11, 8)).unwrap();
        fs::write(retention::sidecar(&f.archive(&tampered)), b"invalid").unwrap();
        assert_eq!(
            prune(&mut f.book, &f.root, &latest, 1).unwrap(),
            vec![old_file.clone()]
        );
        assert!(!old_file.exists() && !retention::sidecar(&old_file).exists());
        for path in [
            manual,
            safety,
            fake,
            latest_file,
            f.archive(&other),
            f.archive(&tampered),
        ] {
            assert!(path.exists(), "preserve {}", path.display());
        }
        assert_eq!(inventory(&mut f.book, &f.root, 20).unwrap().len(), 2);
    }

    #[test]
    fn a_staged_archive_is_bound_to_its_audited_bytes_after_lookup() {
        let mut f = Fixture::new();
        let slot = Slot::new(SCHEDULE, time(12, 2)).unwrap();
        f.run(&slot, time(12, 4)).unwrap();
        let selected = resolve(&mut f.book, &f.root, &slot).unwrap().unwrap();
        let path = &selected.path;
        let original = fs::read(path).unwrap();
        let restore_work = f.live.join("staged-restore");
        let download_work = f.live.join("staged-download");
        let ready = selected.stage(&restore_work).unwrap();
        let staged_path = ready.path().to_owned();
        replace_with_same_metadata(path, &original);
        assert_eq!(fs::read(ready.path()).unwrap(), original);
        for work in [&restore_work, &download_work] {
            let error = selected
                .stage(work)
                .err()
                .expect("swapped bytes must fail verification");
            assert!(error.is_conflict());
        }
        drop(ready);
        assert!(!staged_path.exists());
        assert!(fs::read_dir(&download_work).unwrap().next().is_none());
    }

    fn replace_with_same_metadata(path: &Path, original: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mtime = fs::metadata(path).unwrap().modified().unwrap();
        let mut altered = original.to_vec();
        altered[0] ^= 1;
        let mut swap = tempfile::NamedTempFile::new_in(path.parent().unwrap()).unwrap();
        swap.write_all(&altered).unwrap();
        swap.as_file()
            .set_times(fs::FileTimes::new().set_modified(mtime))
            .unwrap();
        swap.persist(path).unwrap();
        altered
    }

    #[test]
    fn a_replaced_retention_candidate_is_verified_in_quarantine_and_preserved() {
        let mut f = Fixture::new();
        let slot = Slot::new(SCHEDULE, time(12, 2)).unwrap();
        let path = f.run(&slot, time(12, 4)).unwrap().path;
        let row = audit::scheduled_evidence(&mut f.book, Some(SCHEDULE), Some(&slot.stamp()))
            .unwrap()
            .remove(&slot)
            .unwrap();
        assert!(
            verified(&path, &row).is_some(),
            "the initial inventory found this candidate"
        );
        let original = fs::read(&path).unwrap();
        let altered = replace_with_same_metadata(&path, &original);
        let dir = paths::Directory::open(path.parent().unwrap()).unwrap();
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(!prune_candidate(&dir, name, &row).unwrap());
        assert_eq!(fs::read(&path).unwrap(), altered);
        assert!(retention::sidecar(&path).is_file());
        assert!(fs::read_dir(path.parent().unwrap()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".fb-retention-")
        }));
    }

    #[test]
    fn refuses_missing_attachments_and_preserves_conflicting_publication() {
        let mut f = Fixture::new();
        let good = Slot::new(SCHEDULE, time(12, 2)).unwrap();
        f.run(&good, time(12, 4)).unwrap();
        let receipt = f.add_receipt();
        fs::remove_file(f.receipts.join(receipt["stored_name"].as_str().unwrap())).unwrap();
        let missing = Slot::new(SCHEDULE, time(13, 2)).unwrap();
        let error = f.run(&missing, time(13, 4)).err().unwrap();
        assert_eq!(error.absent().len(), 1);
        let target = f.archive(&missing);
        assert!(!target.exists());
        let good_file = f.archive(&good);
        fs::copy(&good_file, &target).unwrap();
        let before = fs::read(&target).unwrap();
        assert!(f.run(&missing, time(13, 5)).err().unwrap().is_conflict());
        assert_eq!(fs::read(&target).unwrap(), before);
        assert!(!retention::sidecar(&target).exists());
        assert_eq!(f.audit_count(), 1);
        assert!(resolve(&mut f.book, &f.root, &good).unwrap().is_some());
    }

    #[test]
    fn a_busy_worker_leaves_the_slot_for_later() {
        let mut f = Fixture::new();
        let slot = Slot::new(SCHEDULE, time(14, 2)).unwrap();
        let target = f.archive(&slot);
        let dir = paths::private_dir(target.parent().unwrap()).unwrap();
        let other = paths::lock_file(&dir.join(".fb-backup.lock")).unwrap();
        fs2::FileExt::try_lock_exclusive(&other).unwrap();
        let error = f.run(&slot, time(14, 3)).err().unwrap();
        assert!(error.to_string().contains("already running"));
        assert!(!error.is_conflict());
        assert!(!target.exists());
        assert_eq!(f.audit_count(), 0);
        drop(other);
        assert!(f.run(&slot, time(14, 4)).is_ok());
        assert_eq!(f.audit_count(), 1);
    }

    #[test]
    fn insufficient_space_keeps_the_last_verified_archive() {
        struct TooLarge;
        impl fb_db::SnapshotSource for TooLarge {
            fn snapshot_size(&self) -> std::result::Result<u64, fb_db::DbError> {
                Ok(u64::MAX)
            }
            fn snapshot(
                &self,
                _: &Path,
                _: &fb_db::Key,
            ) -> std::result::Result<(), fb_db::DbError> {
                panic!("space must be checked before writing a snapshot")
            }
            fn migrations(&self) -> &[(&'static str, &'static str)] {
                fb_db::MIGRATIONS
            }
        }
        let mut f = Fixture::new();
        let good = Slot::new(SCHEDULE, time(13, 2)).unwrap();
        let due = Slot::new(SCHEDULE, time(14, 2)).unwrap();
        let good_path = f.run(&good, time(13, 3)).unwrap().path;
        let target = f.archive(&due);
        let worker = Worker {
            root: &f.root,
            protected: std::slice::from_ref(&f.live),
            copy: CopyWorker {
                key: &KEY,
                receipt_key: &RECEIPT_KEY,
                receipts_root: Some(&f.receipts),
                envelope: None,
                app_version: "test",
            },
        };
        let error = worker
            .capture(
                &TooLarge,
                &due,
                time(14, 3),
                &[f.company.to_db()],
                &target,
                None,
            )
            .unwrap_err();
        assert!(error.to_string().contains("Not enough free space"));
        assert!(!error.is_conflict());
        assert!(!target.exists());
        assert!(retention::verified_note(&good_path));
        assert_eq!(f.audit_count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn dead_process_staging_is_cleaned_without_touching_foreign_staging() {
        let Some(host) = super::super::cleanup::host_tag() else {
            return;
        };
        let mut f = Fixture::new();
        paths::private_dir(&f.root).unwrap();
        let stale = f
            .root
            .join(format!(".fb-backup.{host}.{}.stale1", i32::MAX));
        let foreign = f
            .root
            .join(format!(".fb-backup.0000000000000000.{}.stale1", i32::MAX));
        for path in [&stale, &foreign] {
            fs::create_dir(path).unwrap();
            fs::write(path.join("book.fbbook"), b"interrupted staging").unwrap();
        }
        let slot = Slot::new(SCHEDULE, time(14, 2)).unwrap();
        f.run(&slot, time(14, 3)).unwrap();
        assert!(!stale.exists());
        assert!(foreign.exists());
        assert_eq!(f.audit_count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn overlap_and_symlinks_cannot_publish_or_prune() {
        use std::os::unix::fs::symlink;
        let mut f = Fixture::new();
        let slot = Slot::new(SCHEDULE, time(14, 2)).unwrap();
        let root = f.root.clone();
        f.root = f.live.join("overlap");
        assert!(f.run(&slot, time(14, 3)).is_err());
        assert!(!f.root.exists());
        f.root = root;
        fs::create_dir(&f.root).unwrap();
        symlink(&f.live, f.root.join("automatic")).unwrap();
        assert!(f.run(&slot, time(14, 3)).is_err());
        assert!(prune(&mut f.book, &f.root, &slot, 1).is_err());
        assert!(!f.live.join(SCHEDULE).exists());
        fs::remove_file(f.root.join("automatic")).unwrap();
        let target = f.archive(&slot);
        paths::private_dir(target.parent().unwrap()).unwrap();
        symlink(f.live.join("main.fbbook"), &target).unwrap();
        assert!(published(&f.root, &slot).unwrap());
        assert!(f.run(&slot, time(14, 3)).is_err());
        assert!(fs::symlink_metadata(&target).unwrap().is_symlink());
        assert_eq!(f.audit_count(), 0);
    }

    #[test]
    fn nonregular_sidecar_is_classified_as_conflict_and_preserves_both_entries() {
        let mut f = Fixture::new();
        let slot = Slot::new(SCHEDULE, time(14, 2)).unwrap();
        let target = f.archive(&slot);
        let note_path = retention::sidecar(&target);
        paths::private_dir(target.parent().unwrap()).unwrap();
        fs::create_dir(&note_path).unwrap();
        let err = f.run(&slot, time(14, 3)).err().expect("occupied nonregular sidecar must fail");
        assert!(err.is_conflict(), "occupied nonregular sidecar must be a slot conflict: {err}");
        assert!(target.is_file(), "valid archive must be preserved");
        assert!(note_path.is_dir(), "occupying directory must be preserved");
        assert_eq!(f.audit_count(), 0);
    }
}
