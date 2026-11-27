//! Consent in the encrypted book, a secret in the OS-sealed approval, and a revocable local anchor.
use super::{Permission, Refusal, equal};
use crate::{App, AppError, AppResult, identity::Permission as UserPermission};
use fb_core::{CompanyId, FbError, UserId};
use fb_db::{
    Writer,
    rusqlite::{OptionalExtension, params},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};
use uuid::Uuid;
use zeroize::Zeroize;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Proof {
    pub id: String,
    epoch: String,
    actor: String,
    company: String,
    pub name: String,
    pub permission: Permission,
    secret: String,
}

impl Drop for Proof {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

fn gone() -> AppError {
    AppError::Ai(Refusal::ApprovalGone)
}

fn random() -> AppResult<[u8; 32]> {
    crate::keys::random_bytes().map_err(|_| gone())
}

// Bind to the actual book on this computer. A snapshot and a replacement at the same path have
// different file identities. The separate anchor also prevents restoring a revoked row in place.
fn scope(book: &Path) -> AppResult<[u8; 32]> {
    let book = book.canonicalize().map_err(|_| gone())?;
    let meta = book.metadata().map_err(|_| gone())?;
    if !meta.is_file() {
        return Err(gone());
    }
    let mut h = Sha256::new();
    let path = book.as_os_str().as_encoded_bytes();
    h.update((path.len() as u64).to_le_bytes());
    h.update(path);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        h.update(meta.dev().to_le_bytes());
        h.update(meta.ino().to_le_bytes());
    }
    if let Ok(created) = meta.created().and_then(|t| {
        t.duration_since(std::time::UNIX_EPOCH)
            .map_err(std::io::Error::other)
    }) {
        h.update(created.as_nanos().to_le_bytes());
    } else if !cfg!(unix) {
        return Err(gone());
    }
    Ok(h.finalize().into())
}

impl Proof {
    pub(super) fn actor(&self) -> AppResult<UserId> {
        UserId::parse(&self.actor).ok_or_else(gone)
    }

    pub(super) fn company(&self) -> AppResult<CompanyId> {
        CompanyId::parse(&self.company).ok_or_else(gone)
    }

    fn verifier(&self, book: [u8; 32], anchor: &[u8; 32]) -> AppResult<[u8; 32]> {
        let secret = crate::keys::master_from_hex(&self.secret).map_err(|_| gone())?;
        let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_ref());
        let mut h = ring::hmac::Context::with_key(&key);
        h.update(b"FreedomBooks unattended consent v2");
        h.update(&book);
        h.update(Uuid::parse_str(&self.id).map_err(|_| gone())?.as_bytes());
        h.update(Uuid::parse_str(&self.epoch).map_err(|_| gone())?.as_bytes());
        h.update(self.actor.as_bytes());
        h.update(self.company.as_bytes());
        h.update(match self.permission {
            Permission::ReadOnly => b"r",
            Permission::AccountingWrite => b"w",
        });
        h.update(self.name.as_bytes());
        h.update(anchor);
        Ok(h.sign().as_ref().try_into().expect("SHA-256 HMAC"))
    }

    pub(super) fn token(&self) -> AppResult<String> {
        let secret = crate::keys::master_from_hex(&self.secret).map_err(|_| gone())?;
        let mut h = Sha256::new();
        h.update(b"FreedomBooks unattended worker v1");
        h.update(Uuid::parse_str(&self.id).map_err(|_| gone())?.as_bytes());
        h.update(secret.as_ref());
        Ok(crate::keys::to_hex(&h.finalize()))
    }
}

/// The local file an approval's row in the book must agree with.
///
/// `Active`: byte 1, then the 32-byte anchor. Locked access is on when the row holds the verifier
/// for this anchor.
///
/// `Revoked`: byte 2, then the hash of the anchor that was in place, the event (one byte for the
/// reason, eight for the time) and the value the account change wrote to the row. It records that
/// locked access was switched off and why, so the approval stays remembered and its approver can
/// turn it on again. It authorizes nothing: no file can make a row that an account change wrote
/// (a random value) pass as active.
enum Anchor {
    Active([u8; 32]),
    Revoked { hash: [u8; 32], event: [u8; 9], row: [u8; 32] },
}

const ACTIVE_LEN: u64 = 33;
const REVOKED_LEN: u64 = 74;
const NOT_RETIRED: &str = "Could not retire the local AI approval. Check access to the application data folder, or choose Forget all approvals in Manage AI connections and try again.";

fn anchor_error(error: std::io::Error) -> AppError {
    if error.kind() == std::io::ErrorKind::NotFound {
        gone()
    } else {
        AppError::Ai(Refusal::AnchorUnavailable)
    }
}

fn read_anchor(dir: &Path, id: Uuid) -> AppResult<Anchor> {
    let file = dir.join(id.to_string());
    let meta = fs::symlink_metadata(&file).map_err(anchor_error)?;
    if !meta.is_file() || !matches!(meta.len(), ACTIVE_LEN | REVOKED_LEN) {
        return Err(gone());
    }
    let mut bytes = vec![0; meta.len() as usize];
    fs::File::open(file)
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(anchor_error)?;
    match bytes.as_slice() {
        [1, anchor @ ..] if anchor.len() == 32 => Ok(Anchor::Active(anchor.try_into().unwrap())),
        [2, data @ ..] if data.len() == 73 => Ok(Anchor::Revoked {
            hash: data[..32].try_into().unwrap(),
            event: data[32..41].try_into().unwrap(),
            row: data[41..].try_into().unwrap(),
        }),
        _ => Err(gone()),
    }
}

pub(crate) struct AccountApproval {
    id: String,
    company: CompanyId,
    anchor: Uuid,
}

pub(crate) fn account_approvals(
    tx: &Writer<'_>,
    actor: UserId,
    revoke: impl Fn(CompanyId) -> bool,
) -> Result<Vec<AccountApproval>, FbError> {
    let mut statement =
        tx.prepare("SELECT anchor_id,company_id,id FROM desktop_ai_unattended WHERE actor_id=?1")?;
    let mut rows = statement.query([actor.to_db()])?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let company: String = row.get(1)?;
        let invalid = || FbError::validation("An AI approval is damaged.");
        let company = CompanyId::parse(&company).ok_or_else(invalid)?;
        if revoke(company) {
            ids.push(AccountApproval {
                id: row.get(2)?,
                company,
                anchor: Uuid::parse_str(&id).map_err(|_| invalid())?,
            });
        }
    }
    Ok(ids)
}

/// An account or second-factor change switches locked access off for every approval of that person.
///
/// Nothing about the local folder can stop it or change what the row becomes. The row is always
/// replaced by a fresh random value, which no anchor can match, or removed when there is no anchor
/// to remember (missing, unreadable, or not a file this app writes). The file is read for one thing
/// only, the hash of the anchor the receipt should name, so that an attempt that wrote the receipt
/// and then did not commit can still be recognized as switched off (`ai_check_unattended`). The
/// receipt is written if it can be; without it the anchor is removed, or emptied where the folder
/// does not allow that, and the approval is simply gone.
pub(crate) fn revoke_account_approvals(
    tx: &Writer<'_>,
    dir: Option<&Path>,
    actor: UserId,
    approvals: Vec<AccountApproval>,
    mfa: bool,
) -> Result<(), FbError> {
    let mut event = [u8::from(mfa); 9];
    event[1..].copy_from_slice(&chrono::Utc::now().timestamp_millis().to_le_bytes());
    for approval in approvals {
        let hash: Option<[u8; 32]> = dir.and_then(|dir| match read_anchor(dir, approval.anchor) {
            Ok(Anchor::Active(anchor)) => Some(Sha256::digest(anchor).into()),
            Ok(Anchor::Revoked { hash, .. }) => Some(hash),
            Err(_) => None,
        });
        let (Some(dir), Some(hash)) = (dir, hash) else {
            if let Some(dir) = dir {
                discard_anchors(dir, &[approval.anchor]);
            }
            tx.execute("DELETE FROM desktop_ai_unattended WHERE id=?1", [&approval.id])?;
            continue;
        };
        let row: [u8; 32] = crate::keys::random_bytes()
            .map_err(|_| FbError::validation("Could not switch off the AI approval."))?;
        let mut receipt = [2; REVOKED_LEN as usize];
        receipt[1..33].copy_from_slice(&hash);
        receipt[33..42].copy_from_slice(&event);
        receipt[42..].copy_from_slice(&row);
        if crate::protection::write_private(&dir.join(approval.anchor.to_string()), &receipt).is_err() {
            // No receipt: the active anchor must not stay as it was for older book contents to match.
            discard_anchors(dir, &[approval.anchor]);
        }
        // The triggers on the account tables may already have removed the row: put it back, switched off.
        tx.execute(
            "INSERT INTO desktop_ai_unattended(id,actor_id,company_id,anchor_id,verifier) VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(id) DO UPDATE SET verifier=excluded.verifier",
            params![approval.id, actor.to_db(), approval.company.to_db(), approval.anchor.to_string(), row.as_slice()],
        )?;
    }
    Ok(())
}

enum Status {
    Enabled(String),
    Revoked(serde_json::Value),
}

// Retire before the account transaction commits, so a crash or restored book cannot revive it.
pub(crate) fn retire_anchors(dir: Option<&Path>, ids: &[Uuid]) -> Result<(), FbError> {
    if ids.is_empty() {
        return Ok(());
    }
    let failed = || FbError::validation(NOT_RETIRED);
    let dir = dir.ok_or_else(failed)?;
    for id in ids {
        match fs::remove_file(dir.join(id.to_string())) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err(failed()),
        }
    }
    if dir.is_dir() {
        crate::protection::sync_dir(dir).map_err(|_| failed())?;
    }
    Ok(())
}

/// For the switches nothing in the folder may stop. The file is removed. One that can be neither
/// removed nor replaced (the folder does not allow it) is emptied in place when the file itself can
/// be written, so it no longer matches anything. Only a file that could be an anchor is emptied: a
/// regular file with no other name and no longer than a receipt, checked on the opened file. A hard
/// link at an anchor's name is the same file as something elsewhere (the book, for one), and
/// emptying it would destroy that. Returns whether a regular file is still there; a folder or
/// symbolic link left at an anchor's name is no anchor and is not counted.
fn discard_anchor(file: &Path) -> bool {
    match fs::remove_file(file) {
        Ok(()) => return false,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => (),
    }
    if fs::remove_dir(file).is_ok() {
        return false;
    }
    let Ok(found) = fs::symlink_metadata(file) else { return false };
    if !found.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let could_be_anchor = |meta: &fs::Metadata| meta.is_file() && meta.nlink() == 1 && meta.len() <= REVOKED_LEN;
        // Anything else is not opened at all: opening and closing a second handle on the book
        // would also drop this process's locks on it.
        if could_be_anchor(&found) {
            // Opened for appending, like the app's other private files, and cut to nothing. The
            // open does not follow a link and does not wait on a pipe put there in the meantime;
            // the opened file is checked again before it is cut.
            let mut options = fs::OpenOptions::new();
            options.append(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            if let Ok(emptied) = options.open(file)
                && emptied.metadata().is_ok_and(|meta| could_be_anchor(&meta))
            {
                let _ = emptied.set_len(0);
                let _ = emptied.sync_all();
            }
        }
    }
    true
}

fn discard_anchors(dir: &Path, ids: &[Uuid]) {
    for id in ids {
        discard_anchor(&dir.join(id.to_string()));
    }
    if dir.is_dir() {
        let _ = crate::protection::sync_dir(dir);
    }
}

impl App {
    pub(crate) fn ai_anchor_directory(&self) -> AppResult<PathBuf> {
        Ok(self
            .backups
            .root()?
            .join("ai-connections")
            .join("unattended"))
    }

    pub(super) fn ai_validate_unattended(&mut self, proof: &Proof) -> AppResult<String> {
        match self.ai_check_unattended(proof)? {
            Status::Enabled(name) => Ok(name),
            Status::Revoked(_) => Err(AppError::Ai(Refusal::PausedSignedOut)),
        }
    }

    fn ai_check_unattended(&mut self, proof: &Proof) -> AppResult<Status> {
        self.ai_require_books()?;
        let scope = scope(self.book_path().ok_or_else(gone)?)?;
        let open = self.open.as_mut().ok_or(AppError::Locked)?;
        let row: Option<(Vec<u8>, Option<String>, String)> = open.book.read(|tx| {
            tx.query_row(
                "SELECT a.verifier,(SELECT u.username FROM desktop_user d
                 JOIN fb_user u ON u.id=d.user_id
                 JOIN ledger_company_members m ON m.user_id=u.id AND m.company_id=a.company_id
                 WHERE d.user_id=a.actor_id AND u.is_active=1 AND d.role='administrator' AND d.password_hash IS NOT NULL),a.anchor_id
                 FROM desktop_ai_unattended a
                 WHERE a.id=?1 AND a.actor_id=?2 AND a.company_id=?3",
                params![proof.id, proof.actor, proof.company],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
        })?;
        let (stored, name, anchor_id) = row.ok_or_else(gone)?;
        let anchor = read_anchor(
            &self.ai_anchor_directory()?,
            Uuid::parse_str(&anchor_id).map_err(|_| gone())?,
        )?;
        let stored = <[u8; 32]>::try_from(stored).map_err(|_| gone())?;
        match anchor {
            // On: the row holds this proof's verifier for the anchor on disk.
            Anchor::Active(anchor) => {
                let expected = proof.verifier(scope, &Sha256::digest(anchor).into())?;
                if !equal(&stored, &expected) {
                    return Err(gone());
                }
                Ok(Status::Enabled(name.ok_or_else(gone)?))
            }
            // Switched off by an account change. The row is the value that change wrote; or the
            // change wrote this file and then did not commit, and the row is still the verifier
            // for the anchor the file names. Neither is on, and both can be turned on again.
            Anchor::Revoked { hash, event, row } => {
                let unfinished = proof.verifier(scope, &hash)?;
                if !(equal(&stored, &row) | equal(&stored, &unfinished)) {
                    return Err(gone());
                }
                let millis = i64::from_le_bytes(event[1..].try_into().unwrap());
                let at = chrono::DateTime::from_timestamp_millis(millis).ok_or_else(gone)?.to_rfc3339();
                let reason = if event[0] == 1 { "mfa_changed" } else { "account_changed" };
                Ok(Status::Revoked(serde_json::json!({"at": at, "reason": reason})))
            }
        }
    }

    pub fn ai_unattended_status(&mut self, raw: &str) -> AppResult<Option<serde_json::Value>> {
        let proof = fb_db::parse_unique_json_as(raw).map_err(|_| gone())?;
        Ok(match self.ai_check_unattended(&proof)? {
            Status::Enabled(_) => None,
            Status::Revoked(event) => Some(event),
        })
    }

    pub(crate) fn ai_prune_missing_anchors(&mut self) -> AppResult<()> {
        if self.ai_require_books().is_err() { return Ok(()); }
        let dir = self.ai_anchor_directory()?;
        let book = &mut self.open.as_mut().ok_or(AppError::Locked)?.book;
        let missing = book.read(|tx| {
            let mut statement = tx.prepare("SELECT anchor_id FROM desktop_ai_unattended")?;
            let ids = statement.query_map([], |row| row.get::<_, String>(0))?;
            let mut missing = Vec::new();
            for id in ids {
                let id = id?;
                if Uuid::parse_str(&id).is_ok_and(|uuid| uuid.to_string() == id) {
                    match fs::symlink_metadata(dir.join(&id)) {
                        Ok(meta) if meta.is_file() && meta.len() > 0 => (),
                        // A folder or link in the anchor's place, or a file emptied in place, is no anchor either.
                        Ok(_) => missing.push(id),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => missing.push(id),
                        Err(_) => return Err(FbError::validation("Could not check the local AI approval directory.")),
                    }
                }
            }
            Ok::<_, FbError>(missing)
        })?;
        if !missing.is_empty() {
            book.write(|tx| {
                for id in missing {
                    tx.execute("DELETE FROM desktop_ai_unattended WHERE anchor_id=?1", [id])?;
                }
                Ok::<_, FbError>(())
            })?;
        }
        Ok(())
    }

    /// The actor, company and permission come from a live attended grant belonging to this session.
    /// Only this signed-in action can create or replace the book verifier and local anchor.
    pub fn ai_set_unattended(
        &mut self,
        id: &str,
        epoch: &str,
        enabled: bool,
    ) -> AppResult<serde_json::Value> {
        self.ai_require_books()?;
        let id = Uuid::parse_str(id).map_err(|_| gone())?;
        let epoch = Uuid::parse_str(epoch).map_err(|_| gone())?;
        let id_string = id.to_string();
        let grant = self
            .ai
            .attended
            .grants
            .iter()
            .find(|g| {
                g.id == id_string
                    && g.expires > std::time::Instant::now()
                    && g.generation == self.ai.attended.generation
                    && g.session_generation == self.session_generation()
            })
            .ok_or_else(gone)?;
        let (company, actor, name, permission) = (
            grant.company,
            grant.actor,
            grant.name.clone(),
            grant.permission,
        );
        let session = self.authenticated_session()?;
        if session.actor != actor || self.ai.attended.company != Some(company) {
            return Err(gone());
        }
        self.authorize(Some(company), UserPermission::Administer)?;
        let dir = self.ai_anchor_directory()?;
        let open_scope = scope(self.book_path().ok_or_else(gone)?)?;
        let previous: Option<(String, String, String)> = self.open_mut()?.book.read(|tx| {
            tx.query_row(
                "SELECT actor_id,company_id,anchor_id FROM desktop_ai_unattended WHERE id=?1",
                [&id_string],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
        })?;
        // The receipt of an earlier switch-off is kept until the new row and anchor are in place, so
        // a failure while turning locked access on again leaves an approval that can be retried.
        let mut receipt = None;
        if let Some((previous_actor, previous_company, anchor)) = previous {
            if previous_actor != actor.to_db() || previous_company != company.to_db() {
                return Err(gone());
            }
            let anchor = Uuid::parse_str(&anchor).map_err(|_| gone())?;
            if !enabled {
                // Switching off: the row below goes whatever the folder allows.
                discard_anchors(&dir, &[anchor]);
            } else {
                match read_anchor(&dir, anchor) {
                    Ok(Anchor::Revoked { .. }) => receipt = Some(anchor),
                    // Invalidate any earlier enablement first. A failure can leave the approval attended.
                    Ok(_) | Err(AppError::Ai(Refusal::ApprovalGone)) => retire_anchors(Some(&dir), &[anchor])?,
                    Err(e) => return Err(e),
                }
            }
        }
        self.ai.unattended.retain(|g| g.id != id_string);
        if !enabled {
            self.open_mut()?.book.write(|tx| {
                tx.execute("DELETE FROM desktop_ai_unattended WHERE id=?1 AND actor_id=?2 AND company_id=?3",
                    params![id_string,actor.to_db(),company.to_db()])?;
                Ok::<_,FbError>(())
            })?;
            return Ok(serde_json::Value::Null);
        }
        let proof = Proof {
            id: id_string.clone(),
            epoch: epoch.to_string(),
            actor: actor.to_db(),
            company: company.to_db(),
            name,
            permission,
            secret: crate::keys::to_hex(&random()?),
        };
        let anchor = random()?;
        let anchor_id = Uuid::new_v4().to_string();
        let verifier = proof.verifier(open_scope, &Sha256::digest(anchor).into())?;
        let mut active = [1; ACTIVE_LEN as usize];
        active[1..].copy_from_slice(&anchor);
        // The new anchor exists before the row names it (until then nothing refers to it), and is
        // removed again if the row cannot be written.
        let file = dir.join(&anchor_id);
        crate::protection::write_private(&file, &active)?;
        let written = self.open_mut().and_then(|open| Ok(open.book.write(|tx| {
            tx.execute(
                "INSERT INTO desktop_ai_unattended(id,actor_id,company_id,anchor_id,verifier) VALUES(?1,?2,?3,?4,?5)
                 ON CONFLICT(id) DO UPDATE SET verifier=excluded.verifier,anchor_id=excluded.anchor_id
                 WHERE actor_id=excluded.actor_id AND company_id=excluded.company_id",
                params![id_string,actor.to_db(),company.to_db(),anchor_id,verifier.as_slice()],
            ).and_then(|n| if n == 1 { Ok(n) } else { Err(fb_db::rusqlite::Error::QueryReturnedNoRows) })?;
            Ok::<_,FbError>(())
        })?));
        if let Err(error) = written {
            let _ = fs::remove_file(&file);
            return Err(error);
        }
        // Nothing names the old receipt any more; a leftover grants nothing.
        if let Some(receipt) = receipt {
            let _ = retire_anchors(Some(&dir), &[receipt]);
        }
        Ok(serde_json::to_value(&proof).expect("typed approval proof"))
    }

    /// Forget removes the consent rows of the open books first, whatever the anchors folder allows,
    /// and then every anchor that can be removed, going on past one that cannot; such a file is
    /// emptied in place when it can be written. The result is an error when the rows could not be
    /// removed (the write to the book failed), and otherwise says whether an anchor file was left.
    /// With the approved books not open their rows cannot be reached, so a file that was left matters
    /// there; the desktop reports it in every case.
    pub fn ai_forget_all(&mut self) -> AppResult<bool> {
        self.ai.invalidate();
        self.ai.clear_unattended();
        let books = self.ai_require_books().is_ok().then(|| {
            self.prepare_connections_for_write()?;
            self.open.as_mut().ok_or(AppError::Locked)?.book.write(|tx| tx.execute("DELETE FROM desktop_ai_unattended", []).map(drop))?;
            Ok::<_, AppError>(())
        });
        let mut left = false;
        match self.ai_anchor_directory() {
            Ok(dir) => {
                match fs::read_dir(&dir) {
                    Ok(entries) => for entry in entries {
                        match entry {
                            Ok(entry) => {
                                if Uuid::parse_str(&entry.file_name().to_string_lossy()).is_ok() && discard_anchor(&entry.path()) {
                                    left = true;
                                }
                            }
                            Err(_) => left = true,
                        }
                    },
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(_) => left = true,
                }
                if dir.is_dir() && crate::protection::sync_dir(&dir).is_err() {
                    left = true;
                }
            }
            Err(_) => left = true,
        }
        books.unwrap_or(Ok(())).map(|()| left)
    }
}
