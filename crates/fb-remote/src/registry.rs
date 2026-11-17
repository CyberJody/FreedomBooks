//! Paired computers. The registry holds only public certificate pins, never a secret. The file
//! format (`fb_remote_proto::registry_file`) is authenticated with an HMAC under a random key the
//! book process keeps in its sealed host record (never derived from the TLS key), so an edited
//! file (a pin added by hand) is refused rather than trusted, and it carries a counter that the
//! book process records: an older copy of the file put back is refused at start. Written
//! atomically, 0600.
use fb_remote_proto::registry_file::{self, Contents};
use fb_remote_proto::wire::{DeviceRecord, DeviceRef};
use std::io;
use std::path::{Path, PathBuf};

#[cfg(test)]
const FILE: &str = registry_file::FILE;
pub const MAX_DEVICES: usize = 256;

/// The list on disk is older than the host's sealed record says it should be.
pub const ROLLED_BACK: &str = "The list of paired computers is older than the one this host recorded: an earlier copy was put back, which could let a revoked computer in again. An Administrator can set it aside on the host's Hosting screen; every computer then pairs again.";

pub struct Registry {
    dir: PathBuf,
    key: [u8; 32],
    counter: u64,
    devices: Vec<DeviceRecord>,
    /// A change (a revocation) is in force but not yet written: retried on every later change.
    unsaved: bool,
}

/// The result of a revocation.
#[derive(Debug, PartialEq, Eq)]
pub enum Revoked {
    /// No such active device.
    Unknown,
    /// Revoked and saved.
    Saved(DeviceRef),
    /// Revoked in this run (connections closed, new ones refused) but the file could not be
    /// written: it is tried again on every later change, and lost on a restart until then.
    NotSaved(DeviceRef, String),
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A device name as shown in lists and the audit: one line, 1–60 characters.
pub fn clean_name(name: &str) -> Option<String> {
    let text: String = name.chars().filter(|c| !fb_remote_proto::is_format_char(*c)).map(|c| if c.is_whitespace() { ' ' } else { c }).filter(|c| !c.is_control()).collect();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty() && text.chars().count() <= 60).then_some(text)
}

impl Registry {
    /// Load the list in `dir` with `key`. A missing list starts at `min_counter`; a list whose
    /// counter is below it is refused ([`ROLLED_BACK`]).
    pub fn load(dir: &Path, key: [u8; 32], min_counter: u64) -> io::Result<Registry> {
        let (counter, devices) = match registry_file::read(dir, &key)? {
            Some(c) if c.counter < min_counter => return Err(io::Error::new(io::ErrorKind::InvalidData, ROLLED_BACK)),
            Some(c) => (c.counter, c.devices),
            None => (min_counter, vec![]),
        };
        Ok(Registry { dir: dir.to_owned(), key, counter, devices, unsaved: false })
    }

    /// Make sure the folder the registry lives in is private to this user (0700 on Unix).
    pub fn private_dir(dir: &Path) -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::symlink_metadata(dir)?;
            if !meta.is_dir() {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "the remote-access folder is not a folder"));
            }
            if meta.permissions().mode() & 0o077 != 0 {
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        #[cfg(not(unix))]
        let _ = dir;
        Ok(())
    }

    /// The list's counter (goes up by one with every write).
    pub fn counter(&self) -> u64 {
        self.counter
    }

    /// Whether a change is in force but not written yet.
    pub fn unsaved(&self) -> bool {
        self.unsaved
    }

    /// Try again to write a change that could not be saved.
    pub fn retry_save(&mut self) -> io::Result<()> {
        if self.unsaved {
            self.save()?;
            self.unsaved = false;
        }
        Ok(())
    }

    fn save(&mut self) -> io::Result<()> {
        let next = self.counter.checked_add(1).ok_or_else(|| io::Error::other("the list of paired computers cannot be changed any more"))?;
        // A fresh 0600 temporary file every time: one left behind (with any mode) is never reused.
        registry_file::write(&self.dir, &self.key, &Contents { counter: next, devices: self.devices.clone() })?;
        self.counter = next;
        Ok(())
    }

    /// Refuse the devices the host's sealed record lists as revoked, whatever this file says (it
    /// may be an older copy that still has them). Returns whether anything changed and was saved.
    pub fn enforce(&mut self, revoked: &[String]) -> io::Result<bool> {
        let at = now();
        let mut changed = false;
        for d in self.devices.iter_mut().filter(|d| d.revoked_at.is_none() && revoked.contains(&d.id)) {
            d.revoked_at = Some(at);
            changed = true;
        }
        if changed {
            self.save()?;
        }
        Ok(changed)
    }

    pub fn active(&self, pin: &crate::tls::Pin) -> Option<&DeviceRecord> {
        let hex = fb_remote_proto::hex(pin);
        self.devices.iter().find(|d| d.revoked_at.is_none() && d.pin == hex)
    }

    pub fn is_active_id(&self, id: &str) -> bool {
        self.devices.iter().any(|d| d.id == id && d.revoked_at.is_none())
    }

    pub fn add(&mut self, name: &str, pin: &crate::tls::Pin, paired_by: &str) -> io::Result<DeviceRecord> {
        let name = clean_name(name).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Give this computer a name of up to 60 characters."))?;
        let hex = fb_remote_proto::hex(pin);
        if self.devices.iter().any(|d| d.pin == hex) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "This computer is already paired."));
        }
        if self.devices.iter().filter(|d| d.revoked_at.is_none()).count() >= MAX_DEVICES {
            return Err(io::Error::new(io::ErrorKind::OutOfMemory, "Too many paired computers. Revoke one first."));
        }
        let id = fb_remote_proto::hex(&fb_remote_proto::random::<16>().map_err(|_| io::Error::other("no random source"))?);
        let record = DeviceRecord { id, name, pin: hex, paired_at: now(), paired_by: paired_by.into(), revoked_at: None };
        self.devices.push(record.clone());
        if let Err(e) = self.save() {
            self.devices.pop();
            return Err(e);
        }
        self.unsaved = false;
        Ok(record)
    }

    /// Revoke a device. The revocation is in force at once, whether or not it can be written.
    pub fn revoke(&mut self, id: &str) -> Revoked {
        let Some(i) = self.devices.iter().position(|d| d.id == id && d.revoked_at.is_none()) else { return Revoked::Unknown };
        self.devices[i].revoked_at = Some(now());
        let device = DeviceRef { id: self.devices[i].id.clone(), name: self.devices[i].name.clone() };
        match self.save() {
            Ok(()) => {
                self.unsaved = false;
                Revoked::Saved(device)
            }
            Err(e) => {
                self.unsaved = true;
                Revoked::NotSaved(device, e.to_string())
            }
        }
    }

    pub fn list(&self) -> Vec<DeviceRecord> {
        self.devices.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reviewer's finding S7: a revocation that cannot be saved stays in force.
    #[cfg(unix)]
    #[test]
    fn a_revocation_that_cannot_be_saved_stays_in_force() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let key = [3u8; 32];
        let mut r = Registry::load(dir.path(), key, 0).unwrap();
        let pin = [7u8; 32];
        let d = r.add("Stolen laptop", &pin, "Admin").unwrap();
        // The folder cannot be written (a full or read-only disk).
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        if std::fs::write(dir.path().join("probe"), b"x").is_ok() {
            return; // running as root: permissions do not stop writes
        }
        assert!(matches!(r.revoke(&d.id), Revoked::NotSaved(ref x, _) if x.id == d.id));
        assert!(r.active(&pin).is_none() && !r.is_active_id(&d.id), "refused at once");
        assert!(r.unsaved());
        assert!(r.retry_save().is_err());
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        r.retry_save().unwrap();
        assert!(!r.unsaved());
        assert!(Registry::load(dir.path(), key, 0).unwrap().active(&pin).is_none(), "saved once the disk allows");
    }

    #[cfg(unix)]
    #[test]
    fn a_leftover_temporary_file_does_not_pass_on_its_mode_and_the_folder_is_made_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        Registry::private_dir(dir.path()).unwrap();
        assert_eq!(std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777, 0o700);
        let tmp = dir.path().join("remote-devices.json.tmp");
        std::fs::write(&tmp, b"junk").unwrap();
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o666)).unwrap();
        let mut r = Registry::load(dir.path(), [4u8; 32], 0).unwrap();
        r.add("Laptop", &[1u8; 32], "x").unwrap();
        assert_eq!(std::fs::metadata(dir.path().join(FILE)).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn names_drop_invisible_formatting() {
        assert_eq!(clean_name("Front\u{202e}desk\u{200b} PC").unwrap(), "Frontdesk PC");
    }

    #[test]
    fn devices_persist_revoke_and_refuse_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let key = [3u8; 32];
        let mut r = Registry::load(dir.path(), key, 0).unwrap();
        let pin = [7u8; 32];
        let d = r.add("Front desk\n", &pin, "Jordan Admin").unwrap();
        assert_eq!(d.name, "Front desk");
        assert!(r.add("again", &pin, "x").is_err(), "one record per certificate");
        assert!(r.add("", &[8u8; 32], "x").is_err());
        let r = Registry::load(dir.path(), key, 0).unwrap();
        assert_eq!(r.active(&pin).unwrap().id, d.id);
        assert!(Registry::load(dir.path(), [5u8; 32], 0).is_err(), "another host's key cannot read it");
        let mut r = r;
        assert!(matches!(r.revoke(&d.id), Revoked::Saved(ref x) if x.name == "Front desk"));
        assert_eq!(r.revoke(&d.id), Revoked::Unknown);
        let mut r = Registry::load(dir.path(), key, 0).unwrap();
        assert!(r.active(&pin).is_none(), "revocation persists");
        assert!(!r.is_active_id(&d.id));
        r.add("Laptop", &[9u8; 32], "x").unwrap();
        let path = dir.path().join(FILE);
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace(&fb_remote_proto::hex(&[9u8; 32]), &fb_remote_proto::hex(&[1u8; 32]))).unwrap();
        assert!(Registry::load(dir.path(), key, 0).is_err(), "a pin added by hand is refused");
        #[cfg(unix)]
        {
            std::fs::write(&path, &text).unwrap();
            let r = Registry::load(dir.path(), key, 0).unwrap();
            drop(r);
        }
    }

    /// Section 10 item 2: an older copy of the list put back (with a revoked computer still active)
    /// is refused once the host has recorded a later counter, and the sealed record's revoked ids
    /// are refused whatever the file says.
    #[test]
    fn an_older_list_put_back_is_refused_and_recorded_revocations_hold() {
        let dir = tempfile::tempdir().unwrap();
        let key = [6u8; 32];
        let mut r = Registry::load(dir.path(), key, 0).unwrap();
        let d = r.add("Stolen laptop", &[7u8; 32], "Owner").unwrap();
        assert_eq!(r.counter(), 1);
        let older = std::fs::read(dir.path().join(FILE)).unwrap();
        assert!(matches!(r.revoke(&d.id), Revoked::Saved(_)));
        assert_eq!(r.counter(), 2);
        std::fs::write(dir.path().join(FILE), &older).unwrap();
        let err = Registry::load(dir.path(), key, 2).err().unwrap();
        assert_eq!(err.to_string(), ROLLED_BACK);
        // Without the counter (an older record), the sealed revoked ids still refuse it.
        let mut r = Registry::load(dir.path(), key, 1).unwrap();
        assert!(r.is_active_id(&d.id));
        assert!(r.enforce(std::slice::from_ref(&d.id)).unwrap());
        assert!(!r.is_active_id(&d.id) && r.counter() == 2);
        assert!(!r.enforce(std::slice::from_ref(&d.id)).unwrap(), "nothing more to change");
        // A missing list starts at the recorded counter, so it cannot pass for an older one.
        std::fs::remove_file(dir.path().join(FILE)).unwrap();
        let mut r = Registry::load(dir.path(), key, 5).unwrap();
        assert_eq!(r.counter(), 5);
        r.add("New laptop", &[8u8; 32], "Owner").unwrap();
        assert_eq!(Registry::load(dir.path(), key, 6).unwrap().counter(), 6);
    }
}
