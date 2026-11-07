//! Restore resource limits, including before a portable stream's manifest is available.
use super::*;
use std::io;

pub(super) const ABSOLUTE: u64 = 8 * 1024 * 1024 * 1024;
pub(super) const METADATA: u64 = 64 * 1024 * 1024;

pub(super) fn budget(data: &Manifest) -> Result<u64> {
    let bytes = data.attachments.iter().try_fold(data.book_bytes, |n, r| n.checked_add(r.bytes))
        .ok_or_else(|| BackupError::refused("Backup size exceeds the restore limit."))?;
    // Twice the declared bytes, with bounded metadata and tar-header/padding allowance.
    let cap = bytes.checked_mul(2)
        .and_then(|n| n.checked_add(METADATA + 1024 * 1024))
        .and_then(|n| n.checked_add((data.attachments.len() as u64).saturating_mul(1024)))
        .ok_or_else(|| BackupError::refused("Backup size exceeds the restore limit."))?;
    if bytes > ABSOLUTE { return Err(BackupError::refused("Backup size exceeds the restore limit.")); }
    Ok(cap.min(ABSOLUTE))
}

/// Hold the first authenticated record(s) in bounded memory until the leading
/// manifest gives a size budget. Never persist a plaintext prefix on failure.
pub(super) struct Payload {
    file: tempfile::NamedTempFile,
    pending: Vec<u8>,
    cap: Option<u64>,
    written: u64,
}
impl Payload {
    pub(super) fn new(work: &Path) -> Result<Self> {
        Ok(Self { file: tempfile::NamedTempFile::new_in(work)?, pending: Vec::new(), cap: None, written: 0 })
    }
    pub(super) fn publish(self, path: &Path) -> Result<()> {
        if self.cap.is_none() { return Err(BackupError::refused("Truncated backup manifest.")); }
        self.file.as_file().sync_all()?;
        self.file.persist_noclobber(path).map_err(|e| BackupError::from(e.error))?;
        Ok(())
    }
    fn write_inner(&mut self, bytes: &[u8]) -> Result<()> {
        if self.cap.is_none() {
            if self.pending.len() as u64 + bytes.len() as u64 > METADATA + crypto::CHUNK as u64 + 512 {
                return Err(BackupError::refused("Backup metadata is too large."));
            }
            self.pending.extend_from_slice(bytes);
            if self.pending.len() < 512 { return Ok(()); }
            let header = tar::Header::from_byte_slice(&self.pending[..512]);
            if header.path_bytes().as_ref() != b"manifest.json" || !header.entry_type().is_file() {
                return Err(BackupError::refused("A portable backup must begin with its manifest."));
            }
            let len = header.size()?;
            if len > METADATA { return Err(BackupError::refused("Backup metadata is too large.")); }
            let end = 512 + len as usize;
            if self.pending.len() < end { return Ok(()); }
            let data: Manifest = serde_json::from_value(parse_json(&self.pending[512..end])?)?;
            validate_manifest(&data)?;
            self.cap = Some(budget(&data)?);
            let pending = std::mem::take(&mut self.pending);
            return self.write_inner(&pending);
        }
        let size = self.written.checked_add(bytes.len() as u64)
            .filter(|n| *n <= self.cap.unwrap()).ok_or_else(|| BackupError::refused("Backup size exceeds the restore limit."))?;
        self.file.write_all(bytes)?;
        self.written = size;
        Ok(())
    }
}
impl Write for Payload {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.write_inner(bytes).map_err(io::Error::other)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> { self.file.flush() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn truncated_stream_discards_the_authenticated_plaintext_prefix() {
        let dir=tempfile::tempdir().unwrap();
        let book=Book::create(&dir.path().join("source.fbbook"),&[42;32]).unwrap();
        fs::create_dir(dir.path().join("backups")).unwrap();
        let data=super::super::create_backup(&book,&[42;32],&BackupOptions{
            output:&dir.path().join("backups/source.fbbackup"),work_root:&dir.path().join("work"),
            receipts_root:None,verify_receipt:None,label:"test",app_version:"test",created_at:"2026-10-30T12:00:00Z",passphrase:None,keep:None,
        }).unwrap().manifest;
        let raw=serde_json::to_vec(&data).unwrap();
        let mut tar=tar::Builder::new(Vec::new());let mut h=tar::Header::new_ustar();
        h.set_size(raw.len() as u64);h.set_cksum();
        tar.append_data(&mut h,"manifest.json",raw.as_slice()).unwrap();
        let mut plain=tar.into_inner().unwrap();plain.resize(crypto::CHUNK*3,0);
        let header=crypto::new_parameters();let mut sealed=Vec::new();
        crypto::encrypt_stream(&mut plain.as_slice(),&mut sealed,"synthetic passphrase",&header,b"test").unwrap();
        // Keep two valid non-final records and a broken final one: some plaintext
        // has actually reached disk before the final authentication failure.
        sealed.truncate(2*(crypto::CHUNK+crypto::TAG)+8);
        let work=dir.path().join("decrypt");fs::create_dir(&work).unwrap();
        let mut dst=Payload::new(&work).unwrap();
        assert!(crypto::decrypt_stream(&mut sealed.as_slice(),&mut dst,"synthetic passphrase",&header,b"test").is_err());
        assert!(dst.written>0);
        drop(dst);
        assert_eq!(fs::read_dir(work).unwrap().count(),0);
    }
}
