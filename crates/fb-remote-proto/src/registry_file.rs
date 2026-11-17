//! The list of paired computers as a file: public certificate pins only, never a secret. The
//! gateway writes it; the book process reads it too, to check it against its sealed host record.
//!
//! - It is authenticated with an HMAC under a random key the book process makes and keeps in its
//!   sealed record (never derived from the TLS key), so a pin added by hand is refused.
//! - It carries a counter that goes up with every write. The book process keeps the latest counter
//!   in its sealed record; a file whose counter is lower is an older copy put back (which could make
//!   a revoked computer active again) and is refused at start, with a repair for the Administrator.
//! - Written atomically through a fresh 0600 temporary file, in a 0700 folder.
use crate::wire::DeviceRecord;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::io;
use std::path::{Path, PathBuf};

pub const FILE: &str = "remote-devices.json";
const VERSION: u32 = 2;

/// What the file holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Contents {
    /// Goes up by one with every write.
    pub counter: u64,
    pub devices: Vec<DeviceRecord>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u32,
    counter: u64,
    devices: Vec<DeviceRecord>,
    mac: String,
}

pub fn path(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

fn mac(key: &[u8; 32], counter: u64, devices: &[DeviceRecord]) -> io::Result<String> {
    let text = serde_json::to_vec(devices).map_err(io::Error::other)?;
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(key).map_err(io::Error::other)?;
    m.update(b"fb-remote registry v2\0");
    m.update(&counter.to_be_bytes());
    m.update(&text);
    Ok(crate::hex(&m.finalize().into_bytes()))
}

fn damaged(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

/// Read and authenticate the file in `dir`. None: there is no file.
pub fn read(dir: &Path, key: &[u8; 32]) -> io::Result<Option<Contents>> {
    let bytes = match std::fs::read(path(dir)) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let text = std::str::from_utf8(&bytes).map_err(|_| damaged("the list of paired computers is not text"))?;
    let stored: Stored = crate::strict::parse(text).map_err(|_| damaged("the list of paired computers is damaged"))?;
    let expected = mac(key, stored.counter, &stored.devices)?;
    let given = crate::unhex(&stored.mac).unwrap_or_default();
    let expected = crate::unhex(&expected).unwrap_or_default();
    if stored.version != VERSION || !constant_eq(&given, &expected) {
        return Err(damaged("the list of paired computers was changed outside FreedomBooks or belongs to another host"));
    }
    Ok(Some(Contents { counter: stored.counter, devices: stored.devices }))
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn write(dir: &Path, key: &[u8; 32], contents: &Contents) -> io::Result<()> {
    let stored = Stored { version: VERSION, counter: contents.counter, devices: contents.devices.clone(), mac: mac(key, contents.counter, &contents.devices)? };
    let bytes = serde_json::to_vec_pretty(&stored).map_err(io::Error::other)?;
    crate::audit::write_private(&path(dir), &bytes)
}

/// Move the file aside as `<file>.<label>-<ms>` (the Administrator's repair for a list that is
/// older than the host recorded). Returns the new name, or None if there was no file.
pub fn set_aside(dir: &Path, label: &str) -> io::Result<Option<PathBuf>> {
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let from = path(dir);
    let to = dir.join(format!("{FILE}.{label}-{at}"));
    match std::fs::rename(&from, &to) {
        Ok(()) => Ok(Some(to)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str) -> DeviceRecord {
        DeviceRecord { id: id.into(), name: "Front desk".into(), pin: "ab".repeat(32), paired_at: 1, paired_by: "Owner".into(), revoked_at: None }
    }

    #[test]
    fn the_counter_and_the_devices_are_authenticated_together() {
        let dir = tempfile::tempdir().unwrap();
        let key = [7u8; 32];
        assert_eq!(read(dir.path(), &key).unwrap(), None);
        let c = Contents { counter: 3, devices: vec![device("a")] };
        write(dir.path(), &key, &c).unwrap();
        assert_eq!(read(dir.path(), &key).unwrap(), Some(c));
        assert!(read(dir.path(), &[8u8; 32]).is_err(), "another key");
        // An edited counter (to pass an older list off as newer) is refused.
        let text = std::fs::read_to_string(path(dir.path())).unwrap().replace("\"counter\": 3", "\"counter\": 9");
        std::fs::write(path(dir.path()), text).unwrap();
        assert!(read(dir.path(), &key).is_err());
        let aside = set_aside(dir.path(), "older").unwrap().unwrap();
        assert!(aside.exists() && read(dir.path(), &key).unwrap().is_none());
    }
}
