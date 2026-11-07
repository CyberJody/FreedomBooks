//! fb-core backups: archive reading on the verify path (`backups::verify_backup`), the recovery-key
//! sidecar (`backups::recovery::WrappedKey::read` + `unlock`) and the retention file-name parser.
//!
//! Two shapes, chosen by the first byte:
//!   * raw: the remaining bytes are the whole `.fbbackup` tar (tar header parsing, entry allow-list,
//!     header.json decoding, manifest decrypt failure path);
//!   * structured: the bytes are split into a manifest JSON and a `book.fbbook`; a valid
//!     `header.json` is generated and the manifest is encrypted correctly under the master key so
//!     the manifest deserializer, `validate_manifest`, the SHA-256 checks and the SQLCipher snapshot
//!     inspection all see fuzzer-controlled content.
//! Asserts: no panic; verification of fuzzer bytes never succeeds (nothing random is a backup of a
//! book, so an Ok here would be a false accept).
#![no_main]
use libfuzzer_sys::fuzz_target;
use fb_core::backups::{self, manifest};
use std::io::Write;

const KEY: [u8; 32] = [9u8; 32];

fn tar_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, bytes) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, name, *bytes).unwrap();
    }
    builder.into_inner().unwrap()
}

fuzz_target!(|data: &[u8]| {
    let Some((&selector, rest)) = data.split_first() else { return };
    let work = tempfile::tempdir().unwrap();
    let archive = work.path().join("in.fbbackup");
    let bytes = if selector % 2 == 0 {
        rest.to_vec()
    } else {
        let cut = rest.len() / 2;
        let (manifest_json, book) = rest.split_at(cut);
        let header = serde_json::json!({
            "format": backups::FORMAT,
            "format_version": backups::FORMAT_VERSION,
            "encrypted": false,
            "database": "SQLCipher",
            "manifest_nonce": manifest::new_nonce(),
        });
        let raw = serde_json::to_vec(&header).unwrap();
        // Let the fuzzer control the manifest text, but splice in the real book hash so the
        // sha256 gate passes when the JSON is otherwise valid.
        let mut text = String::from_utf8_lossy(manifest_json).into_owned();
        let digest = {
            use sha2::Digest;
            format!("{:x}", sha2::Sha256::digest(book))
        };
        text = text.replace("@SHA@", &digest).replace("@LEN@", &book.len().to_string());
        let sealed = manifest::encrypt(text.as_bytes(), &KEY, &header, &raw).unwrap();
        tar_with(&[("header.json", &raw), ("manifest.enc", &sealed), ("book.fbbook", book)])
    };
    std::fs::File::create(&archive).unwrap().write_all(&bytes).unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let result = backups::verify_backup(&archive, &KEY, scratch.path(), None, false);
    assert!(result.is_err(), "fuzzer bytes verified as a backup");
    let result = backups::verify_backup(&archive, &KEY, scratch.path(), Some("passphrase-with-twelve"), false);
    assert!(result.is_err(), "fuzzer bytes verified as an encrypted backup");
    // Recovery-key sidecar: the same bytes as a JSON file.
    let sidecar = work.path().join("recovery.json");
    std::fs::write(&sidecar, rest).unwrap();
    if let Ok(w) = backups::recovery::WrappedKey::read(&sidecar) {
        let _ = w.unlock("passphrase-with-twelve");
    }
    let text = String::from_utf8_lossy(rest);
    let _ = backups::retention::filename(&text, &text);
    let _ = backups::crypto::check_new_passphrase(&text);
});
