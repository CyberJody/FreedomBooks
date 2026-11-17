//! The host's remote-access audit: pairing, connections, refusals, sign-in and sign-out, and
//! revocation, as an append-only JSON-lines file (0600). It is kept beside the book, not in it:
//! device and session columns in the book's audit table would change the book format. It is
//! written only by the book process; the gateway reports its events up the pipe.
//!
//! Tamper evidence:
//! - Each line carries an HMAC-SHA-256 (keyed with an [`AuditKey`] the book process holds, never
//!   written beside the log) over its content and the previous line's MAC. Editing, removing or
//!   reordering lines, even with every MAC recomputed, needs the key.
//! - A checkpoint file beside the log (`<log>.head`, MAC'd with the same key) records the last
//!   sequence number and MAC. Removing lines from the end, deleting the log, or deleting the
//!   checkpoint is detected when the log is opened or verified. Opening with
//!   [`Expect::Existing`] (the host has used this key before) refuses a missing log.
//! - Not detected by these files alone: putting back an earlier copy of the checkpoint and cutting
//!   the log back to it (the log is append-only, so only the small checkpoint file needs to be
//!   kept). The host closes that gap by keeping the head in its sealed store too
//!   (`AuditLog::head` gives what to keep, `open_with_head` checks against it).
//!
//! Failures and repair:
//! - After any failed write (line, sync or checkpoint) the log refuses every later append until it
//!   is opened again, so a full or read-only disk never leaves lines in an order that cannot be
//!   checked. The dispatcher stops serving remote requests at the same moment.
//! - Lines past the checkpoint (written before a checkpoint write failed or a crash) are accepted
//!   when each verifies under the key, which cannot be done without it; the checkpoint is brought
//!   up to them and `audit.repaired` records how many.
//! - A crash during an append can leave a last line without its newline. Opening the log sets that
//!   fragment aside in `<log>.torn-<time>` (only when everything before it verifies against the
//!   checkpoint), truncates the log to its last whole line and records `audit.repaired`.
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Event {
    pub kind: String,
    pub peer: String,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub user: Option<String>,
    pub detail: String,
}

pub trait Sink: Send {
    fn record(&mut self, event: &Event) -> io::Result<()>;
}

/// Tests and tools: events kept in memory.
#[derive(Default)]
pub struct Memory(pub std::sync::Arc<std::sync::Mutex<Vec<Event>>>);

impl Sink for Memory {
    fn record(&mut self, event: &Event) -> io::Result<()> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(event.clone());
        Ok(())
    }
}

/// The key the audit chain and its checkpoint are MAC'd with: 32 random bytes the book process
/// makes ([`AuditKey::generate`]) and keeps sealed itself. It is never written beside the log, never
/// sent to the gateway (which holds the TLS key, so nothing may be derived from that), and never
/// logged; `Debug` hides it.
#[derive(Clone)]
pub struct AuditKey([u8; 32]);

impl std::fmt::Debug for AuditKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuditKey { .. }")
    }
}

impl AuditKey {
    pub fn generate() -> io::Result<AuditKey> {
        crate::random::<32>().map(AuditKey).map_err(|_| io::Error::other("no random source"))
    }

    pub fn from_bytes(bytes: [u8; 32]) -> AuditKey {
        AuditKey(bytes)
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }

    fn mac(&self) -> io::Result<HmacSha256> {
        <HmacSha256 as Mac>::new_from_slice(&self.0).map_err(io::Error::other)
    }
}

/// Whether the host has kept a log under this key before.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expect {
    /// A key made just now: no log or checkpoint may exist yet; both are created.
    New,
    /// A key the host has used before: the log and its checkpoint must both be there.
    Existing,
}

/// The end of the chain: what the checkpoint holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Head {
    pub seq: u64,
    pub hash: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    seq: u64,
    at: u64,
    kind: String,
    peer: String,
    device_id: Option<String>,
    device_name: Option<String>,
    user: Option<String>,
    detail: String,
    prev: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Line {
    seq: u64,
    at: u64,
    kind: String,
    peer: String,
    device_id: Option<String>,
    device_name: Option<String>,
    user: Option<String>,
    detail: String,
    prev: String,
    hash: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    version: u32,
    seq: u64,
    hash: String,
    mac: String,
}

const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// One line of plain text: control and invisible formatting characters (bidirectional overrides,
/// zero-width characters) dropped, at most 200 characters.
pub fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control() && !crate::is_format_char(*c)).take(200).collect()
}

fn broken(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn digest(key: &AuditKey, record: &Record) -> io::Result<String> {
    let text = serde_json::to_string(record).map_err(io::Error::other)?;
    let mut m = key.mac()?;
    m.update(text.as_bytes());
    Ok(crate::hex(&m.finalize().into_bytes()))
}

fn head_mac(key: &AuditKey, seq: u64, hash: &str) -> io::Result<HmacSha256> {
    let mut m = key.mac()?;
    m.update(b"fb-remote audit head v1\0");
    m.update(&seq.to_be_bytes());
    m.update(hash.as_bytes());
    Ok(m)
}

fn head_path(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".head");
    PathBuf::from(p)
}

/// Write a small private file atomically: never reuse a temporary file someone else left (its
/// mode would carry over).
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    match std::fs::remove_file(&tmp) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut f = options.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    if let Some(dir) = path.parent()
        && let Ok(d) = File::open(dir)
    {
        let _ = d.sync_all();
    }
    Ok(())
}

fn read_head(path: &Path, key: &AuditKey) -> io::Result<Option<Head>> {
    let bytes = match std::fs::read(head_path(path)) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let text = std::str::from_utf8(&bytes).map_err(|_| broken("the remote-access audit checkpoint is damaged"))?;
    let c: Checkpoint = crate::strict::parse(text).map_err(|_| broken("the remote-access audit checkpoint is damaged"))?;
    let mac = crate::unhex(&c.mac).unwrap_or_default();
    if c.version != 1 || head_mac(key, c.seq, &c.hash)?.verify_slice(&mac).is_err() {
        return Err(broken("the remote-access audit checkpoint was changed outside FreedomBooks or belongs to another key"));
    }
    Ok(Some(Head { seq: c.seq, hash: c.hash }))
}

fn write_head(path: &Path, key: &AuditKey, head: &Head) -> io::Result<()> {
    let mac = crate::hex(&head_mac(key, head.seq, &head.hash)?.finalize().into_bytes());
    let bytes = serde_json::to_vec(&Checkpoint { version: 1, seq: head.seq, hash: head.hash.clone(), mac }).map_err(io::Error::other)?;
    write_private(&head_path(path), &bytes)
}

/// What a log file holds: whole lines verified, their chain end, the MAC at each of the asked-for
/// lines (if reached; line 0 is the genesis value), and the length and bytes of a final fragment
/// without a newline.
struct Scan {
    seq: u64,
    prev: String,
    hash_at: Vec<Option<String>>,
    whole: u64,
    torn: Vec<u8>,
}

impl Scan {
    fn empty(marks: &[u64]) -> Scan {
        Scan { seq: 0, prev: GENESIS.to_owned(), hash_at: marks.iter().map(|m| (*m == 0).then(|| GENESIS.to_owned())).collect(), whole: 0, torn: vec![] }
    }
}

fn scan(bytes: &[u8], key: &AuditKey, marks: &[u64]) -> io::Result<Scan> {
    let whole = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    let (body, torn) = bytes.split_at(whole);
    let mut seq = 0;
    let mut prev = GENESIS.to_owned();
    let mut hash_at: Vec<Option<String>> = marks.iter().map(|m| (*m == 0).then(|| GENESIS.to_owned())).collect();
    for raw in body.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let fail = || broken(format!("the remote-access audit log is broken at line {}", seq + 1));
        let text = std::str::from_utf8(raw).map_err(|_| fail())?;
        let l: Line = crate::strict::parse(text).map_err(|_| fail())?;
        if l.seq != seq + 1 || l.prev != prev {
            return Err(fail());
        }
        let hash = l.hash.clone();
        let record = Record { seq: l.seq, at: l.at, kind: l.kind, peer: l.peer, device_id: l.device_id, device_name: l.device_name, user: l.user, detail: l.detail, prev: l.prev };
        if !constant_eq(digest(key, &record)?.as_bytes(), hash.as_bytes()) {
            return Err(fail());
        }
        seq += 1;
        for (mark, slot) in marks.iter().zip(hash_at.iter_mut()) {
            if *mark == seq {
                *slot = Some(hash.clone());
            }
        }
        prev = hash;
    }
    Ok(Scan { seq, prev, hash_at, whole: whole as u64, torn: torn.to_vec() })
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Check a scanned log (scanned with the checkpoint's line as its first mark) against its
/// checkpoint. The log may be ahead (lines written before a checkpoint write failed or a crash:
/// each verified under the key by the scan), never behind, and must pass through the checkpoint.
fn matches(scan: &Scan, head: &Head) -> io::Result<()> {
    if scan.seq < head.seq {
        return Err(broken(format!("the remote-access audit log ends at line {} but its checkpoint says {}: lines were removed", scan.seq, head.seq)));
    }
    if scan.hash_at[0].as_deref() != Some(head.hash.as_str()) {
        return Err(broken("the remote-access audit log does not match its checkpoint"));
    }
    Ok(())
}

pub struct AuditLog {
    path: PathBuf,
    key: AuditKey,
    file: File,
    seq: u64,
    prev: String,
    /// Set by the first failed write: no later line may be added until the log is opened again.
    failed: Option<String>,
}

impl std::fmt::Debug for AuditLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditLog").field("path", &self.path).field("seq", &self.seq).finish_non_exhaustive()
    }
}

impl AuditLog {
    /// Open (or, with [`Expect::New`], create) the log and verify the whole chain against its
    /// checkpoint. A broken, shortened or missing log is an error: hosting does not start on a log
    /// someone edited. A torn last line is set aside and the repair recorded.
    pub fn open(path: &Path, key: &AuditKey, expect: Expect) -> io::Result<AuditLog> {
        Self::open_with_head(path, key, expect, None)
    }

    /// As [`AuditLog::open`], also checking the chain against a head the host kept elsewhere
    /// (its sealed store): the log must reach that line and have the kept MAC there, so an earlier
    /// copy put back, or a different chain (even a longer one), is refused.
    pub fn open_with_head(path: &Path, key: &AuditKey, expect: Expect, kept: Option<&Head>) -> io::Result<AuditLog> {
        let bytes = match std::fs::read(path) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        let head = read_head(path, key)?;
        let marks = |head: &Head| [head.seq, kept.map_or(0, |k| k.seq)];
        let (head, scan) = match (expect, bytes, head) {
            (Expect::New, None, None) => {
                let head = Head { seq: 0, hash: GENESIS.to_owned() };
                write_head(path, key, &head)?;
                let scan = Scan::empty(&marks(&head));
                (head, scan)
            }
            // A crash on the first start, after the empty checkpoint and before the log: nothing
            // was recorded yet, so this is an empty log in either mode.
            (_, None, Some(head)) if head.seq == 0 && head.hash == GENESIS => {
                let scan = Scan::empty(&marks(&head));
                (head, scan)
            }
            (Expect::New, _, _) => return Err(broken("a remote-access audit log already exists here; it was kept under another key. Move it aside before hosting with a new key.")),
            (Expect::Existing, None, _) => return Err(broken("the remote-access audit log is missing")),
            (Expect::Existing, Some(_), None) => return Err(broken("the remote-access audit checkpoint is missing")),
            (Expect::Existing, Some(bytes), Some(head)) => {
                let scan = scan(&bytes, key, &marks(&head))?;
                matches(&scan, &head)?;
                (head, scan)
            }
        };
        if let Some(kept) = kept
            && (scan.seq < kept.seq || scan.hash_at[1].as_deref() != Some(kept.hash.as_str()))
        {
            return Err(broken("the remote-access audit log is not the one the host recorded: an earlier copy was put back or the log was replaced"));
        }
        let torn = scan.torn.len();
        if torn > 0 {
            // Keep the fragment for whoever investigates, then cut the log back to whole lines.
            let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
            let mut aside = path.as_os_str().to_owned();
            aside.push(format!(".torn-{at}"));
            write_private(Path::new(&aside), &scan.torn)?;
            let f = OpenOptions::new().write(true).open(path)?;
            f.set_len(scan.whole)?;
            f.sync_all()?;
        }
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let file = options.open(path)?;
        let mut log = AuditLog { path: path.to_owned(), key: key.clone(), file, seq: scan.seq, prev: scan.prev, failed: None };
        let behind = log.seq - head.seq;
        if behind > 0 {
            // Lines were written but the checkpoint was not (a crash, or a failed checkpoint write
            // that stopped the log): each verified under the key, so bring the checkpoint up to them.
            log.checkpoint()?;
            log.append(&Event { kind: "audit.repaired".into(), detail: format!("the checkpoint was {behind} line{} behind the log; each line verified and the checkpoint was brought up to date", if behind == 1 { "" } else { "s" }), ..Default::default() })?;
        }
        if torn > 0 {
            log.append(&Event { kind: "audit.repaired".into(), detail: format!("an incomplete last line ({torn} bytes) was set aside"), ..Default::default() })?;
        }
        Ok(log)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The chain's end, for a host that also keeps it in its sealed store.
    pub fn head(&self) -> Head {
        Head { seq: self.seq, hash: self.prev.clone() }
    }

    fn checkpoint(&self) -> io::Result<()> {
        write_head(&self.path, &self.key, &self.head())
    }

    /// Why the log stopped taking lines, if a write failed since it was opened.
    pub fn failed(&self) -> Option<&str> {
        self.failed.as_deref()
    }

    /// Add one line, then its checkpoint. Any failure stops the log: later appends are refused
    /// until it is opened again (which accepts and records what did reach the disk).
    pub fn append(&mut self, event: &Event) -> io::Result<()> {
        if let Some(why) = &self.failed {
            return Err(io::Error::other(format!("the remote-access audit log stopped after a failed write: {why}")));
        }
        let result = self.append_once(event);
        if let Err(e) = &result {
            self.failed = Some(e.to_string());
        }
        result
    }

    fn append_once(&mut self, event: &Event) -> io::Result<()> {
        let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let record = Record {
            seq: self.seq + 1,
            at,
            kind: clean(&event.kind),
            peer: clean(&event.peer),
            device_id: event.device_id.as_deref().map(clean),
            device_name: event.device_name.as_deref().map(clean),
            user: event.user.as_deref().map(clean),
            detail: clean(&event.detail),
            prev: self.prev.clone(),
        };
        let hash = digest(&self.key, &record)?;
        let line = Line { seq: record.seq, at, kind: record.kind, peer: record.peer, device_id: record.device_id, device_name: record.device_name, user: record.user, detail: record.detail, prev: record.prev, hash: hash.clone() };
        let mut text = serde_json::to_string(&line).map_err(io::Error::other)?;
        text.push('\n');
        self.file.write_all(text.as_bytes())?;
        self.file.sync_data()?;
        self.seq += 1;
        self.prev = hash;
        self.checkpoint()
    }
}

impl Sink for AuditLog {
    fn record(&mut self, event: &Event) -> io::Result<()> {
        self.append(event)
    }
}

/// Where the book process keeps the audit key sealed (the OS keyring, through the desktop app).
/// Built with the hosting toggle; the gateway never sees it.
pub trait KeyStore {
    fn load(&self) -> io::Result<Option<[u8; 32]>>;
    fn store(&self, key: &[u8; 32]) -> io::Result<()>;
}

/// The key the host's audit log was kept under is gone (the keyring was reset, or the key lost).
pub const LOST_KEY: &str = "The key that protects the remote-access audit log is missing (for example the system keyring was reset). Remote access cannot start on that log. An Administrator can set the old log aside and start a new one; the old log can no longer be checked.";

/// Open the host's audit log for hosting, with the key from `store` (a new random key is made and
/// sealed on the first start). `hosted_before` and `kept` come from the host's sealed record:
/// whether this host has kept a log before, and the head it kept. A host that has hosted before
/// and has no key gets [`LOST_KEY`]; its repair is [`start_over`].
pub fn open_for_hosting(path: &Path, store: &dyn KeyStore, hosted_before: bool, kept: Option<&Head>) -> io::Result<AuditLog> {
    let key = match store.load()? {
        Some(bytes) => AuditKey::from_bytes(bytes),
        None if hosted_before => return Err(io::Error::new(io::ErrorKind::NotFound, LOST_KEY)),
        None => {
            let key = AuditKey::generate()?;
            store.store(&key.to_bytes())?;
            key
        }
    };
    let exists = path.exists() || head_path(path).exists();
    let expect = if hosted_before || exists { Expect::Existing } else { Expect::New };
    AuditLog::open_with_head(path, &key, expect, kept)
}

/// The Administrator's repair when the key is lost (`by`, already authorized by the caller): the
/// old log and checkpoint are set aside under a dated name, a new random key is made and sealed,
/// and the new log's first line names the old log's last line and MAC as found (they cannot be
/// checked without the old key). The host must show the Administrator what was set aside.
pub fn start_over(path: &Path, store: &dyn KeyStore, by: &str) -> io::Result<AuditLog> {
    set_aside(path, store, by, "lost-key", "the key was lost").map(|(log, _)| log)
}

/// Set the log and its checkpoint aside as `<log>.<label>-<time>` and start a new log under a new
/// random key (sealed in `store`), whose first line (`audit.key_replaced`, by the Administrator
/// `by`) says `why` and names the old log's last line and MAC as found. Used when the key is lost
/// ([`start_over`]) and when the log no longer matches the head the host recorded (an older copy
/// was put back). Returns the new log and the name the old log was set aside under.
pub fn set_aside(path: &Path, store: &dyn KeyStore, by: &str, label: &str, why: &str) -> io::Result<(AuditLog, PathBuf)> {
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let aside = |p: &Path| {
        let mut name = p.as_os_str().to_owned();
        name.push(format!(".{label}-{at}"));
        PathBuf::from(name)
    };
    let mut last = None;
    if let Ok(text) = std::fs::read_to_string(path) {
        last = text.lines().rev().find_map(|l| serde_json::from_str::<Line>(l).ok()).map(|l| (l.seq, l.hash));
    }
    let old = aside(path);
    for p in [path.to_owned(), head_path(path)] {
        match std::fs::rename(&p, aside(&p)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    let key = AuditKey::generate()?;
    store.store(&key.to_bytes())?;
    let mut log = AuditLog::open(path, &key, Expect::New)?;
    let name = old.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let detail = match last {
        Some((seq, hash)) => format!("{why}; the old log was set aside as {name}; it ended at line {seq} with MAC {}", hash.get(..16).unwrap_or(&hash)),
        None => format!("{why}; the old log was set aside as {name}; no line could be read from it"),
    };
    log.append(&Event { kind: "audit.key_replaced".into(), user: Some(by.into()), detail, ..Default::default() })?;
    Ok((log, old))
}

/// Verify a log against its checkpoint (no repair); returns the number of lines. Safe while the
/// log is in use: the checkpoint is read before the log (an append writes its line first), and a
/// check that fails is tried once more in case a line was being written.
pub fn verify(path: &Path, key: &AuditKey) -> io::Result<u64> {
    verify_once(path, key).or_else(|_| {
        std::thread::sleep(std::time::Duration::from_millis(50));
        verify_once(path, key)
    })
}

fn verify_once(path: &Path, key: &AuditKey) -> io::Result<u64> {
    let head = read_head(path, key)?.ok_or_else(|| broken("the remote-access audit checkpoint is missing"))?;
    let bytes = std::fs::read(path)?;
    let scan = scan(&bytes, key, &[head.seq])?;
    if !scan.torn.is_empty() {
        return Err(broken("the remote-access audit log ends with an incomplete line"));
    }
    matches(&scan, &head)?;
    Ok(scan.seq)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: &str) -> Event {
        Event { kind: kind.into(), peer: "10.0.0.2".into(), ..Default::default() }
    }

    /// A log of `n` lines under `key`, closed.
    fn log_of(path: &Path, key: &AuditKey, n: usize) {
        let mut log = AuditLog::open(path, key, Expect::New).unwrap();
        for i in 0..n {
            log.append(&event(if i % 3 == 0 { "sign_in.failed" } else { "connect" })).unwrap();
        }
    }

    /// Recompute every line's chain with `key` (an attacker's rewrite), keeping only lines `keep`.
    fn rewrite(path: &Path, key: &AuditKey, mut keep: impl FnMut(&Line) -> bool) -> String {
        let text = std::fs::read_to_string(path).unwrap();
        let mut out = String::new();
        let (mut seq, mut prev) = (0, GENESIS.to_owned());
        for raw in text.lines() {
            let l: Line = serde_json::from_str(raw).unwrap();
            if !keep(&l) {
                continue;
            }
            seq += 1;
            let record = Record { seq, at: l.at, kind: l.kind, peer: l.peer, device_id: l.device_id, device_name: l.device_name, user: l.user, detail: l.detail, prev: prev.clone() };
            let hash = digest(key, &record).unwrap();
            let line = Line { seq, at: record.at, kind: record.kind, peer: record.peer, device_id: record.device_id, device_name: record.device_name, user: record.user, detail: record.detail, prev: record.prev, hash: hash.clone() };
            out.push_str(&serde_json::to_string(&line).unwrap());
            out.push('\n');
            prev = hash;
        }
        std::fs::write(path, &out).unwrap();
        prev
    }

    #[test]
    fn the_chain_survives_reopen_and_detects_edits_and_removals() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote-audit.jsonl");
        let key = AuditKey::generate().unwrap();
        let mut log = AuditLog::open(&path, &key, Expect::New).unwrap();
        log.append(&Event { kind: "pair.succeeded".into(), peer: "10.0.0.2".into(), device_name: Some("Front\ndesk\u{202e}".into()), ..Default::default() }).unwrap();
        log.append(&Event { kind: "sign_in.succeeded".into(), user: Some("dana".into()), ..Default::default() }).unwrap();
        drop(log);
        assert!(AuditLog::open(&path, &key, Expect::New).is_err(), "a new key never adopts an existing log");
        let mut log = AuditLog::open(&path, &key, Expect::Existing).unwrap();
        log.append(&Event { kind: "sign_out".into(), ..Default::default() }).unwrap();
        assert_eq!(log.head().seq, 3);
        drop(log);
        assert_eq!(verify(&path, &key).unwrap(), 3);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"Frontdesk\""), "control and bidi characters are dropped");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(head_path(&path)).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert!(verify(&path, &AuditKey::generate().unwrap()).is_err(), "another key cannot verify it");
        std::fs::write(&path, text.replace("dana", "eve!")).unwrap();
        assert!(verify(&path, &key).is_err(), "an edited line breaks the chain");
        assert!(AuditLog::open(&path, &key, Expect::Existing).is_err());
        let lines: Vec<&str> = text.lines().collect();
        std::fs::write(&path, format!("{}\n{}\n", lines[0], lines[2])).unwrap();
        assert!(verify(&path, &key).is_err(), "a removed line breaks the chain");
    }

    /// The reviewer's probe (`probe-audit.out`): each of these verified before the fix.
    #[test]
    fn truncation_deletion_and_recomputed_rewrites_are_detected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote-audit.jsonl");
        let key = AuditKey::generate().unwrap();
        log_of(&path, &key, 38);
        let original = std::fs::read_to_string(&path).unwrap();
        let original_head = std::fs::read(head_path(&path)).unwrap();
        assert_eq!(verify(&path, &key).unwrap(), 38);

        // The newest 20 lines removed.
        let kept: Vec<&str> = original.lines().take(18).collect();
        std::fs::write(&path, kept.join("\n") + "\n").unwrap();
        assert!(verify(&path, &key).unwrap_err().to_string().contains("lines were removed"));
        assert!(AuditLog::open(&path, &key, Expect::Existing).is_err(), "hosting does not reopen a shortened log");

        // The log deleted (its checkpoint left), and both deleted.
        std::fs::remove_file(&path).unwrap();
        assert!(AuditLog::open(&path, &key, Expect::Existing).unwrap_err().to_string().contains("missing"));
        std::fs::remove_file(head_path(&path)).unwrap();
        assert!(AuditLog::open(&path, &key, Expect::Existing).is_err(), "a host that has used this key expects its log");

        // Failures dropped and every MAC recomputed without the key.
        std::fs::write(&path, &original).unwrap();
        std::fs::write(head_path(&path), &original_head).unwrap();
        assert_eq!(verify(&path, &key).unwrap(), 38, "restored");
        let wrong = AuditKey::generate().unwrap();
        rewrite(&path, &wrong, |l| l.kind != "sign_in.failed");
        assert!(verify(&path, &key).is_err(), "recomputed without the key");
        assert!(AuditLog::open(&path, &key, Expect::Existing).is_err());
    }

    #[test]
    fn a_rewrite_with_the_key_but_without_the_checkpoint_is_still_caught_by_the_kept_head() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote-audit.jsonl");
        let key = AuditKey::generate().unwrap();
        log_of(&path, &key, 10);
        let early_log = std::fs::read(&path).unwrap();
        let early_head = std::fs::read(head_path(&path)).unwrap();
        let mut log = AuditLog::open(&path, &key, Expect::Existing).unwrap();
        log.append(&event("device.revoked")).unwrap();
        let kept = log.head();
        drop(log);
        // An older copy of both files put back verifies on its own...
        std::fs::write(&path, &early_log).unwrap();
        std::fs::write(head_path(&path), &early_head).unwrap();
        assert_eq!(verify(&path, &key).unwrap(), 10);
        // ...but not against the head the host kept in its sealed store.
        assert!(AuditLog::open_with_head(&path, &key, Expect::Existing, Some(&kept)).unwrap_err().to_string().contains("earlier copy"));
    }

    /// The re-check's A4 (`probe-audit2.out` [6]): a different chain under the same key, one line
    /// longer than the kept head, was accepted.
    #[test]
    fn a_different_chain_longer_than_the_kept_head_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote-audit.jsonl");
        let key = AuditKey::generate().unwrap();
        log_of(&path, &key, 10);
        let kept = AuditLog::open(&path, &key, Expect::Existing).unwrap().head();
        assert_eq!(kept.seq, 10);
        let real = std::fs::read(&path).unwrap();
        let real_head = std::fs::read(head_path(&path)).unwrap();
        // The same history with line 4 dropped and two lines added, every MAC and the checkpoint
        // recomputed with the key: 11 lines, a valid chain, a valid checkpoint.
        for extra in [1usize, 2, 5] {
            std::fs::write(&path, &real).unwrap();
            std::fs::write(head_path(&path), &real_head).unwrap();
            let mut n = 0;
            rewrite(&path, &key, |_| {
                n += 1;
                n != 4
            });
            std::fs::remove_file(head_path(&path)).unwrap();
            let (seq, prev) = {
                let text = std::fs::read_to_string(&path).unwrap();
                let last: Line = serde_json::from_str(text.lines().last().unwrap()).unwrap();
                (last.seq, last.hash)
            };
            write_head(&path, &key, &Head { seq, hash: prev }).unwrap();
            let mut log = AuditLog::open(&path, &key, Expect::Existing).unwrap();
            for _ in 0..extra + 1 {
                log.append(&event("connect")).unwrap();
            }
            drop(log);
            assert!(verify(&path, &key).unwrap() > kept.seq, "a valid chain on its own");
            let refused = AuditLog::open_with_head(&path, &key, Expect::Existing, Some(&kept)).unwrap_err().to_string();
            assert!(refused.contains("not the one the host recorded"), "{refused}");
        }
        // The real log, and the real log grown past the kept head, are accepted.
        std::fs::write(&path, &real).unwrap();
        std::fs::write(head_path(&path), &real_head).unwrap();
        let mut log = AuditLog::open_with_head(&path, &key, Expect::Existing, Some(&kept)).unwrap();
        log.append(&event("connect")).unwrap();
        drop(log);
        AuditLog::open_with_head(&path, &key, Expect::Existing, Some(&kept)).unwrap();
    }

    /// The re-check's A1 (`probe-audit2.out` [10]): with the folder read-only the checkpoint could
    /// not be written, later events kept adding lines, and the log could not be opened again.
    #[cfg(unix)]
    #[test]
    fn a_failed_write_stops_the_log_and_the_next_open_repairs_and_records_it() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("remote");
        std::fs::create_dir(&folder).unwrap();
        let path = folder.join("remote-audit.jsonl");
        let key = AuditKey::generate().unwrap();
        log_of(&path, &key, 5);
        let mut log = AuditLog::open(&path, &key, Expect::Existing).unwrap();
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o500)).unwrap();
        if std::fs::write(folder.join("probe"), b"x").is_ok() {
            return; // running as root: permissions do not stop writes
        }
        // The line reaches the (still writable) file; its checkpoint cannot be written.
        assert!(log.append(&event("sign_in.succeeded")).is_err());
        assert!(log.failed().is_some());
        let lines = std::fs::read_to_string(&path).unwrap().lines().count();
        for kind in ["disconnect", "session.ended", "host.stopped"] {
            assert!(log.append(&event(kind)).is_err(), "nothing more is added after a failed write");
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), lines);
        drop(log);
        // The Administrator frees the disk and starts hosting again.
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).unwrap();
        let log = AuditLog::open(&path, &key, Expect::Existing).unwrap();
        assert_eq!(log.head().seq, 7, "the line past the checkpoint is kept and the repair recorded");
        drop(log);
        assert_eq!(verify(&path, &key).unwrap(), 7);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.lines().nth(5).unwrap().contains("sign_in.succeeded"));
        assert!(text.lines().last().unwrap().contains("checkpoint was 1 line behind"));
    }

    /// Several lines past the checkpoint (each verified under the key) are accepted; one line
    /// past it that does not verify is not.
    #[test]
    fn lines_past_the_checkpoint_are_accepted_only_when_each_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote-audit.jsonl");
        let key = AuditKey::generate().unwrap();
        log_of(&path, &key, 5);
        let head5 = std::fs::read(head_path(&path)).unwrap();
        let mut log = AuditLog::open(&path, &key, Expect::Existing).unwrap();
        for _ in 0..3 {
            log.append(&event("connect")).unwrap();
        }
        drop(log);
        let eight = std::fs::read_to_string(&path).unwrap();
        std::fs::write(head_path(&path), &head5).unwrap();
        assert_eq!(verify(&path, &key).unwrap(), 8);
        // A forged line (no key) after the checkpoint.
        let mut lines: Vec<String> = eight.lines().map(str::to_owned).collect();
        lines[6] = lines[6].replace("connect", "conneXt");
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        assert!(AuditLog::open(&path, &key, Expect::Existing).is_err());
        std::fs::write(&path, &eight).unwrap();
        let log = AuditLog::open(&path, &key, Expect::Existing).unwrap();
        assert_eq!(log.head().seq, 9);
        drop(log);
        assert!(std::fs::read_to_string(&path).unwrap().contains("3 lines behind"));
    }

    /// A write that fails part-way leaves a fragment; the log stops, so no later line follows it,
    /// and the next open sets it aside (`probe-audit2.out` [11]).
    #[test]
    fn a_write_that_fails_part_way_is_followed_by_nothing_and_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote-audit.jsonl");
        let key = AuditKey::generate().unwrap();
        log_of(&path, &key, 5);
        let mut log = AuditLog::open(&path, &key, Expect::Existing).unwrap();
        // The failed write: a fragment reached the file, then the file handle stopped working.
        OpenOptions::new().append(true).open(&path).unwrap().write_all(br#"{"seq":6,"at":1,"kind":"conn"#).unwrap();
        log.file = File::open(&path).unwrap();
        assert!(log.append(&event("connect")).is_err());
        log.file = OpenOptions::new().append(true).open(&path).unwrap();
        assert!(log.append(&event("host.stopped")).is_err(), "the log stays stopped");
        drop(log);
        let log = AuditLog::open(&path, &key, Expect::Existing).unwrap();
        assert_eq!(log.head().seq, 6);
        drop(log);
        assert!(std::fs::read_to_string(&path).unwrap().lines().last().unwrap().contains("incomplete last line"));
    }

    /// The re-check's B1 (`probe-audit2.out` [8]): a crash on first start left an empty
    /// checkpoint and no log, which neither mode accepted.
    #[test]
    fn an_empty_checkpoint_without_a_log_is_an_empty_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote-audit.jsonl");
        let key = AuditKey::generate().unwrap();
        write_head(&path, &key, &Head { seq: 0, hash: GENESIS.to_owned() }).unwrap();
        let mut log = AuditLog::open(&path, &key, Expect::New).unwrap();
        log.append(&event("host.started")).unwrap();
        drop(log);
        std::fs::remove_file(&path).unwrap();
        write_head(&path, &key, &Head { seq: 0, hash: GENESIS.to_owned() }).unwrap();
        AuditLog::open(&path, &key, Expect::Existing).unwrap();
        // Another key's empty checkpoint is still refused.
        std::fs::remove_file(&path).unwrap();
        write_head(&path, &AuditKey::generate().unwrap(), &Head { seq: 0, hash: GENESIS.to_owned() }).unwrap();
        assert!(AuditLog::open(&path, &key, Expect::New).is_err());
    }

    #[test]
    fn a_torn_last_line_is_set_aside_and_the_repair_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote-audit.jsonl");
        let key = AuditKey::generate().unwrap();
        log_of(&path, &key, 5);
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(br#"{"seq":6,"at":1,"kind":"conn"#).unwrap();
        drop(f);
        assert!(verify(&path, &key).is_err(), "verify reports it");
        let log = AuditLog::open(&path, &key, Expect::Existing).unwrap();
        assert_eq!(log.head().seq, 6);
        drop(log);
        assert_eq!(verify(&path, &key).unwrap(), 6);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.lines().last().unwrap().contains("audit.repaired"));
        let aside: Vec<_> = std::fs::read_dir(dir.path()).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().contains(".torn-")).collect();
        assert_eq!(aside.len(), 1);
        assert_eq!(std::fs::read(aside[0].path()).unwrap(), br#"{"seq":6,"at":1,"kind":"conn"#);
        // A torn line after lines were removed is not repaired: the log is refused.
        let lines: Vec<&str> = text.lines().take(3).collect();
        std::fs::write(&path, lines.join("\n") + "\n{\"seq\"").unwrap();
        assert!(AuditLog::open(&path, &key, Expect::Existing).is_err());
    }

    #[test]
    fn a_line_written_before_its_checkpoint_is_accepted_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote-audit.jsonl");
        let key = AuditKey::generate().unwrap();
        log_of(&path, &key, 4);
        let head4 = std::fs::read(head_path(&path)).unwrap();
        let mut log = AuditLog::open(&path, &key, Expect::Existing).unwrap();
        log.append(&event("disconnect")).unwrap();
        drop(log);
        // A crash between the line and its checkpoint: the checkpoint still says 4.
        std::fs::write(head_path(&path), &head4).unwrap();
        let log = AuditLog::open(&path, &key, Expect::Existing).unwrap();
        assert_eq!(log.head().seq, 6, "the line is kept and the repair recorded");
        drop(log);
        assert_eq!(verify(&path, &key).unwrap(), 6, "the checkpoint caught up");
        assert!(std::fs::read_to_string(&path).unwrap().lines().last().unwrap().contains("checkpoint was 1 line behind"));
    }

    #[derive(Default)]
    struct Sealed(std::sync::Mutex<Option<[u8; 32]>>);

    impl KeyStore for Sealed {
        fn load(&self) -> io::Result<Option<[u8; 32]>> {
            Ok(*self.0.lock().unwrap())
        }
        fn store(&self, key: &[u8; 32]) -> io::Result<()> {
            *self.0.lock().unwrap() = Some(*key);
            Ok(())
        }
    }

    /// A5: the key is random, made and sealed by the book process on the first start; a host
    /// that lost it cannot start on the old log, and the Administrator's repair sets it aside and
    /// starts a new log that names where the old one ended.
    #[test]
    fn the_audit_key_is_made_and_sealed_on_first_start_and_a_lost_key_has_a_repair() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote-audit.jsonl");
        let store = Sealed::default();
        let mut log = open_for_hosting(&path, &store, false, None).unwrap();
        let key = store.load().unwrap().expect("a key was sealed");
        assert_ne!(key, [0u8; 32]);
        log.append(&event("host.started")).unwrap();
        let kept = log.head();
        drop(log);
        // Later starts use the sealed key and the kept head; deleting both files is refused.
        open_for_hosting(&path, &store, true, Some(&kept)).unwrap();
        assert_eq!(store.load().unwrap(), Some(key), "the key is not replaced");
        let lost = Sealed::default();
        assert_eq!(open_for_hosting(&path, &lost, true, Some(&kept)).unwrap_err().to_string(), LOST_KEY);
        let log = start_over(&path, &lost, "Jordan Admin").unwrap();
        assert_eq!(log.head().seq, 1);
        drop(log);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("audit.key_replaced") && text.contains("Jordan Admin") && text.contains(&format!("ended at line {} with MAC {}", kept.seq, &kept.hash[..16])), "{text}");
        let aside: Vec<String> = std::fs::read_dir(dir.path()).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.contains(".lost-key-")).collect();
        assert_eq!(aside.len(), 2, "log and checkpoint set aside: {aside:?}");
        assert!(lost.load().unwrap().is_some_and(|k| k != key), "a new key was sealed");
        open_for_hosting(&path, &lost, true, None).unwrap();
    }
}
