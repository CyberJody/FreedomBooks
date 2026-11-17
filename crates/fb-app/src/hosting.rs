//! "Host these books": the host's sealed record (book format 93, table `remote_host`).
//!
//! The record is kept in the encrypted book by the book process alone. It holds what must survive
//! someone putting back older copies of the files that live beside the book (the remote-access audit
//! log and its checkpoint, the gateway's list of paired computers):
//!
//! - whether this book has been hosted before (so a missing audit log or key is noticed, never
//!   taken for a first start);
//! - the hosting choice (on/off) and the address an Administrator chose;
//! - the audit log's key (32 random bytes made here; never given to the gateway) and the head of the
//!   chain as last recorded;
//! - the list of paired computers' key (random, never derived from the TLS key), its counter as last
//!   recorded, and every computer ever revoked;
//! - the host's TLS identity, handed to the gateway at start and never written by it;
//! - when each paired computer was last seen.
//!
//! Network code is never here: fb-node starts the `fb-remote` gateway from this record.
use crate::{App, AppError, AppResult, identity::Permission};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Most revoked ids and last-seen entries kept (far above the gateway's 256 paired computers).
pub const MAX_REVOKED: usize = 4096;
pub const MAX_SEEN: usize = 1024;

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    /// Certificate and PKCS#8 private key, DER as hex.
    pub cert: String,
    pub key: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Head {
    pub seq: u64,
    pub hash: String,
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostRecord {
    pub version: u32,
    /// Random; names this book's hosting folder (audit log, gateway state).
    pub host_id: String,
    pub hosted_before: bool,
    pub enabled: bool,
    /// The address and port an Administrator chose (`192.168.1.20:47100`).
    pub bind: String,
    /// Who turned hosting on last, for the screen.
    pub enabled_by: String,
    /// Hex, 32 bytes. None until the first start makes it.
    pub audit_key: Option<String>,
    pub audit_head: Option<Head>,
    /// Hex, 32 bytes.
    pub registry_key: String,
    pub device_counter: u64,
    pub revoked: Vec<String>,
    pub identity: Option<Identity>,
    /// Device id → seconds since 1970 when it last connected.
    pub last_seen: BTreeMap<String, u64>,
}

impl std::fmt::Debug for HostRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostRecord")
            .field("host_id", &self.host_id)
            .field("hosted_before", &self.hosted_before)
            .field("enabled", &self.enabled)
            .field("bind", &self.bind)
            .field("audit_head", &self.audit_head)
            .field("device_counter", &self.device_counter)
            .field("revoked", &self.revoked.len())
            .finish_non_exhaustive()
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn random<const N: usize>() -> AppResult<[u8; N]> {
    let mut out = [0u8; N];
    getrandom::fill(&mut out).map_err(|_| AppError::Invalid("This computer has no random source.".into()))?;
    Ok(out)
}

fn damaged() -> AppError {
    AppError::Invalid("The book's hosting record is damaged. Hosting cannot start; use Report a problem.".into())
}

impl HostRecord {
    /// A record for a book that has never been hosted: fresh random ids and keys.
    pub fn fresh() -> AppResult<HostRecord> {
        Ok(HostRecord { version: 1, host_id: hex(&random::<16>()?), registry_key: hex(&random::<32>()?), ..Default::default() })
    }

    fn checked(self) -> AppResult<HostRecord> {
        let hex32 = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());
        let ok = self.version == 1
            && self.host_id.len() == 32
            && self.host_id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            && hex32(&self.registry_key)
            && self.audit_key.as_deref().is_none_or(hex32)
            && self.revoked.len() <= MAX_REVOKED
            && self.last_seen.len() <= MAX_SEEN
            && self.bind.len() <= 64;
        if ok { Ok(self) } else { Err(damaged()) }
    }

    pub fn audit_key_bytes(&self) -> Option<[u8; 32]> {
        self.audit_key.as_deref().and_then(unhex32)
    }

    pub fn registry_key_bytes(&self) -> AppResult<[u8; 32]> {
        unhex32(&self.registry_key).ok_or_else(damaged)
    }

    /// Remember a revoked computer for good (oldest dropped past [`MAX_REVOKED`]).
    pub fn note_revoked(&mut self, id: &str) {
        if !self.revoked.iter().any(|r| r == id) {
            if self.revoked.len() >= MAX_REVOKED {
                self.revoked.remove(0);
            }
            self.revoked.push(id.to_owned());
        }
        self.last_seen.remove(id);
    }

    pub fn note_seen(&mut self, id: &str, at: u64) {
        if self.last_seen.len() >= MAX_SEEN
            && !self.last_seen.contains_key(id)
            && let Some(oldest) = self.last_seen.iter().min_by_key(|(_, t)| **t).map(|(k, _)| k.clone())
        {
            self.last_seen.remove(&oldest);
        }
        self.last_seen.insert(id.to_owned(), at);
    }
}

pub fn unhex32(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, pair) in text.as_bytes().chunks(2).enumerate() {
        let d = |c: u8| match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        };
        out[i] = d(pair[0])? << 4 | d(pair[1])?;
    }
    Some(out)
}

impl App {
    /// The open book's hosting record, if it has one. Host-level: no session is needed.
    pub fn host_record(&mut self) -> AppResult<Option<HostRecord>> {
        let open = self.open.as_mut().ok_or(AppError::Locked)?;
        let bytes: Option<Vec<u8>> = open.book.read(|q| {
            use fb_db::rusqlite::OptionalExtension;
            q.query_row("SELECT record FROM remote_host WHERE id=1", [], |r| r.get(0)).optional()
        })?;
        match bytes {
            None => Ok(None),
            Some(b) => serde_json::from_slice::<HostRecord>(&b).map_err(|_| damaged())?.checked().map(Some),
        }
    }

    /// Write the open book's hosting record. Host-level: the caller (fb-node) decided who may.
    pub fn save_host_record(&mut self, record: &HostRecord) -> AppResult<()> {
        let record = record.clone().checked()?;
        let bytes = serde_json::to_vec(&record).map_err(|e| AppError::Invalid(e.to_string()))?;
        let open = self.open.as_mut().ok_or(AppError::Locked)?;
        open.book.write(|tx| tx.execute("INSERT INTO remote_host(id,record) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET record=excluded.record", [bytes]).map(drop))?;
        Ok(())
    }

    /// A restored copy is about to become the books in use: hosting is switched off in it, so it
    /// never starts by itself from a backup. Nothing is written if it is already off.
    pub(crate) fn hosting_off_for_restored(&mut self) -> AppResult<()> {
        match self.host_record()? {
            Some(mut r) if r.enabled => {
                r.enabled = false;
                self.save_host_record(&r)
            }
            _ => Ok(()),
        }
    }

    /// The signed-in Administrator's name, for hosting controls (turn hosting on or off, pair,
    /// revoke, repair). Anyone else is refused.
    pub fn hosting_administrator(&mut self) -> AppResult<String> {
        self.authorize(None, Permission::Administer)?;
        Ok(self.open.as_ref().map(|o| o.owner_name.clone()).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Role;
    use crate::keys::MasterKey;

    fn book() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new();
        app.backups.configure(dir.path()).unwrap();
        app.device_key = Some(MasterKey::new([43; 32]));
        app.create_protected_book(&dir.path().join("books/default.fbbook"), "Synthetic owner", "Synthetic hosted company", Some("Synthetic book password")).unwrap();
        app.enroll_owner("Synthetic owner user password").unwrap();
        (dir, app)
    }

    #[test]
    fn the_record_round_trips_in_the_book_and_hides_its_keys() {
        let (_dir, mut app) = book();
        assert_eq!(app.host_record().unwrap(), None);
        let mut r = HostRecord::fresh().unwrap();
        r.audit_key = Some(hex(&[9u8; 32]));
        r.identity = Some(Identity { cert: "aa".into(), key: "secret-key-hex".into() });
        r.audit_head = Some(Head { seq: 3, hash: "ab".repeat(32) });
        r.note_revoked("dev1");
        r.note_revoked("dev1");
        r.note_seen("dev2", 10);
        app.save_host_record(&r).unwrap();
        let back = app.host_record().unwrap().unwrap();
        assert_eq!(back, r);
        assert_eq!(back.revoked, vec!["dev1".to_owned()]);
        assert_eq!(back.audit_key_bytes(), Some([9u8; 32]));
        let shown = format!("{back:?}");
        assert!(!shown.contains("secret-key-hex") && !shown.contains(&hex(&[9u8; 32])) && !shown.contains(&r.registry_key), "{shown}");
        // A damaged record is refused, not trusted.
        let mut bad = r.clone();
        bad.registry_key = "short".into();
        assert!(app.save_host_record(&bad).is_err());
        app.open.as_mut().unwrap().book.write(|tx| tx.execute("UPDATE remote_host SET record=?1", [b"{\"version\":1}".to_vec()]).map(drop)).unwrap();
        assert!(app.host_record().is_err());
    }

    #[test]
    fn only_an_administrator_may_control_hosting() {
        let (_dir, mut app) = book();
        assert_eq!(app.hosting_administrator().unwrap(), "Synthetic owner");
        let company = app.first_company().unwrap().unwrap();
        let keeper = app.save_desktop_user(None, "Synthetic bookkeeper", Role::Bookkeeper, true, Some("Synthetic keeper password"), &[company]).unwrap();
        let _ = keeper;
        app.sign_out();
        assert!(app.hosting_administrator().is_err(), "signed out");
        app.sign_in("Synthetic bookkeeper", "Synthetic keeper password").unwrap();
        assert!(app.hosting_administrator().is_err(), "a bookkeeper");
    }
}
