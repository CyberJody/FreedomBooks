mod common;
use common::*;
use fb_core::backups::{
    self, BackupOptions, RestoreOptions, crypto, recovery::WrappedKey, retention,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

struct Files {
    _tmp: tempfile::TempDir,
    work: PathBuf,
    out: PathBuf,
    restore: PathBuf,
}
impl Files {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("private");
        let out = tmp.path().join("backups");
        let restore = tmp.path().join("restores");
        fs::create_dir(&out).unwrap();
        fs::create_dir(&restore).unwrap();
        Self {
            _tmp: tmp,
            work,
            out,
            restore,
        }
    }
    fn options<'a>(&'a self, path: &'a Path, passphrase: Option<&'a str>) -> BackupOptions<'a> {
        BackupOptions {
            output: path,
            work_root: &self.work,
            receipts_root: None,
            verify_receipt: None,
            label: "synthetic",
            app_version: "test",
            created_at: "2026-10-30T12:00:00Z",
            passphrase,
            keep: None,
        }
    }
    fn roptions<'a>(&'a self, target: &'a Path) -> RestoreOptions<'a> {
        RestoreOptions {
            verify_receipt: None,
            target,
            receipts_target: None,
            work_root: &self.work,
            protected: &[],
            passphrase: None,
            require_encrypted: false,
        }
    }
}
fn archive(path: &Path) -> Vec<(String, Vec<u8>)> {
    tar::Archive::new(fs::File::open(path).unwrap())
        .entries()
        .unwrap()
        .map(|e| {
            let mut e = e.unwrap();
            let name = e.path().unwrap().to_str().unwrap().to_owned();
            let mut bytes = Vec::new();
            e.read_to_end(&mut bytes).unwrap();
            (name, bytes)
        })
        .collect()
}
fn write_archive(path: &Path, members: Vec<(String, Vec<u8>)>) {
    let mut tar = tar::Builder::new(fs::File::create(path).unwrap());
    for (name, bytes) in members {
        let mut h = tar::Header::new_ustar();
        h.set_size(bytes.len() as u64);
        h.set_mode(0o600);
        h.set_cksum();
        tar.append_data(&mut h, name, bytes.as_slice()).unwrap();
    }
    tar.finish().unwrap();
}

fn read_fixture_manifest(members: &[(String, Vec<u8>)]) -> Value {
    let header: Value = serde_json::from_slice(&members[0].1).unwrap();
    let plain = backups::manifest::decrypt(&members[1].1, &KEY, &header, &members[0].1).unwrap();
    serde_json::from_slice(&plain).unwrap()
}
fn write_fixture_manifest(members: &mut [(String, Vec<u8>)], data: &Value) {
    let header: Value = serde_json::from_slice(&members[0].1).unwrap();
    members[1].1 = backups::manifest::encrypt(
        &serde_json::to_vec(data).unwrap(),
        &KEY,
        &header,
        &members[0].1,
    )
    .unwrap();
}

#[test]
fn secfix1_restore_refuses_declared_oversize_and_manifest_mismatch_before_copy() {
    let mut fx=Fx::new("secfix1-restore");fx.chart();
    let f=Files::new();let backup=f.out.join("limits.fbbackup");
    backups::create_backup(&fx.book,&KEY,&f.options(&backup,None)).unwrap();
    let original=archive(&backup);
    // A short physical tar with a huge declared payload cannot allocate or copy it.
    let huge=f.out.join("huge.fbbackup");
    let mut tar=tar::Builder::new(fs::File::create(&huge).unwrap());
    for (name,bytes) in &original[..2] {
        let mut h=tar::Header::new_ustar();h.set_size(bytes.len() as u64);h.set_mode(0o600);h.set_cksum();
        tar.append_data(&mut h,name,bytes.as_slice()).unwrap();
    }
    let mut h=tar::Header::new_ustar();h.set_path("book.fbbook").unwrap();h.set_size(9*1024*1024*1024);h.set_cksum();
    use std::io::Write;
    tar.get_mut().write_all(h.as_bytes()).unwrap();drop(tar);
    let target=f.restore.join("oversize.fbbook");
    let e=backups::restore_backup(&huge,&KEY,&f.roptions(&target)).err().unwrap();
    assert!(e.to_string().contains("large") || e.to_string().contains("limit"),"{e}");
    assert!(!target.exists());
    let mut changed=original.clone();let mut data=read_fixture_manifest(&changed);
    data["book_bytes"]=json!(1);write_fixture_manifest(&mut changed,&data);
    write_archive(&backup,changed);
    let e=backups::restore_backup(&backup,&KEY,&f.roptions(&target)).err().unwrap();
    assert!(e.to_string().contains("size"),"{e}");
    assert!(!target.exists());
    assert_eq!(fs::read_dir(&f.work).unwrap().count(),0,"no plaintext staging files remain");
}

#[test]
fn consistent_encrypted_snapshot_and_manifest_roundtrip() {
    let mut fx = Fx::new("backup-roundtrip");
    let (co, a, e) = fx.chart();
    let j = fx.draft(co, a, e, "123.45", "2026-10-01");
    fx.post(co, j.id);
    let draft = fx.draft(co, a, e, "25.00", "2026-10-02");
    let before = fb_db::test_support::snapshot(fx.book.connection()).unwrap();
    let files = Files::new();
    let output = files.out.join("chosen.fbbackup");
    let result = backups::create_backup(&fx.book, &KEY, &files.options(&output, None)).unwrap();
    assert_eq!(
        result.manifest.contents.companies[0].trial_balance["debit"],
        "123.45"
    );
    assert_eq!(result.manifest.contents.companies[0].journals["draft"], 1);
    assert_eq!(result.manifest.contents.companies[0].journals["posted"], 1);
    assert_eq!(
        archive(&output)
            .iter()
            .map(|r| r.0.as_str())
            .collect::<Vec<_>>(),
        ["header.json", "manifest.enc", "book.fbbook"]
    );
    let full = fs::read(&output).unwrap();
    assert!(
        !full
            .windows(b"Synthetic Company".len())
            .any(|w| w == b"Synthetic Company")
    );
    let bytes = archive(&output)
        .into_iter()
        .find(|r| r.0 == "book.fbbook")
        .unwrap()
        .1;
    assert_ne!(&bytes[..16], b"SQLite format 3\0");
    assert!(
        !bytes
            .windows(b"Synthetic Company".len())
            .any(|w| w == b"Synthetic Company")
    );
    // Mutating live books after the snapshot cannot change the saved manifest.
    fx.post(co, draft.id);
    let target = files.restore.join("restored.fbbook");
    let restored = backups::restore_backup(&output, &KEY, &files.roptions(&target)).unwrap();
    assert!(!restored.upgraded);
    let book = fb_core::Book::open(&target, &KEY).unwrap();
    assert_eq!(
        fb_db::test_support::snapshot(book.connection()).unwrap(),
        before
    );
    assert!(book.check().unwrap().is_empty());
    drop(book);
    assert!(backups::verify_backup(&output, &KEY, &files.work, None, true).is_ok());
    assert!(retention::verified_note(&output));
    assert!(backups::create_backup(&fx.book, &KEY, &files.options(&output, None)).is_err());
    assert!(backups::restore_backup(&output, &KEY, &files.roptions(&target)).is_err());
    assert_eq!(fs::read_dir(&files.work).unwrap().count(), 0);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(output).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&files.work).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}

#[test]
fn encrypted_export_restore_and_refusals() {
    let fx = Fx::new("backup-encrypted");
    let f = Files::new();
    let output = f.out.join("chosen.fbbackup");
    let pass = "synthetic secret passphrase";
    backups::create_backup(&fx.book, &KEY, &f.options(&output, Some(pass))).unwrap();
    assert_eq!(
        archive(&output)
            .iter()
            .map(|r| r.0.as_str())
            .collect::<Vec<_>>(),
        ["header.json", "payload.enc"]
    );
    let bytes = fs::read(&output).unwrap();
    assert!(!bytes.windows(pass.len()).any(|w| w == pass.as_bytes()));
    let target = f.restore.join("restore.fbbook");
    let mut opt = f.roptions(&target);
    assert!(backups::restore_backup(&output, &KEY, &opt).is_err());
    opt.passphrase = Some("synthetic wrong passphrase");
    assert!(backups::restore_backup(&output, &KEY, &opt).is_err());
    opt.passphrase = Some(pass);
    opt.require_encrypted = true;
    backups::restore_backup(&output, &KEY, &opt).unwrap();
    backups::verify_backup(&output, &KEY, &f.work, Some(pass), true).unwrap();
    let note: Value =
        serde_json::from_slice(&fs::read(retention::sidecar(&output)).unwrap()).unwrap();
    assert_eq!(note["encrypted"], true);
    assert!(retention::verified_note(&output));
    let badkey = f.restore.join("wrong-key.fbbook");
    opt.target = &badkey;
    assert!(backups::restore_backup(&output, &[9; 32], &opt).is_err());
    assert!(!badkey.exists());
    let short = f.out.join("short.fbbackup");
    let mut opt = f.options(&short, Some("short"));
    opt.keep = Some(0);
    assert!(backups::create_backup(&fx.book, &KEY, &opt).is_err());
}

#[test]
fn refuses_damage_duplicates_schema_mismatch_and_downgrade_without_creating_target() {
    let fx = Fx::new("backup-refusals");
    let f = Files::new();
    let output = f.out.join("good.fbbackup");
    backups::create_backup(&fx.book, &KEY, &f.options(&output, None)).unwrap();
    for case in 0..9 {
        let mut members = archive(&output);
        match case {
            0 => members.push(members[0].clone()),
            1 => {
                members[2].1[100] ^= 1;
            }
            2 => {
                let mut m = read_fixture_manifest(&members);
                m["contents"]["tables"]["fb_user"] = json!(999);
                write_fixture_manifest(&mut members, &m);
            }
            3 => {
                let mut m = read_fixture_manifest(&members);
                m["migrations"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!("9999_future"));
                write_fixture_manifest(&mut members, &m);
            }
            4 => {
                members[0].1 = br#"{"format":"bad","format":"freedombooks-backup","format_version":3,"encrypted":false,"database":"SQLCipher"}"#.to_vec();
            }
            5 => members.push((
                "receipts/11111111111111111111111111111111.bin".into(),
                vec![0; 20],
            )),
            6 => {
                let mut m: Value = serde_json::from_slice(&members[0].1).unwrap();
                m["encrypted"] = json!(0);
                members[0].1 = serde_json::to_vec(&m).unwrap();
            }
            7 => members[1].1 = b"[]".to_vec(),
            _ => {
                members.remove(2);
            }
        }
        let bad = f.out.join(format!("bad-{case}.fbbackup"));
        write_archive(&bad, members);
        let target = f.restore.join(format!("bad-{case}.fbbook"));
        assert!(
            backups::restore_backup(&bad, &KEY, &f.roptions(&target)).is_err(),
            "case {case}"
        );
        assert!(!target.exists());
        assert_eq!(fs::read_dir(&f.work).unwrap().count(), 0);
    }
    let target = f.restore.join("require-encrypted.fbbook");
    let mut opt = f.roptions(&target);
    opt.require_encrypted = true;
    assert!(backups::restore_backup(&output, &KEY, &opt).is_err());
}

#[test]
fn plaintext_work_overlap_and_live_restore_refused_before_writes() {
    let fx = Fx::new("backup-paths");
    let f = Files::new();
    let output = f.out.join("b.fbbackup");
    let mut opt = f.options(&output, None);
    opt.work_root = &f.out;
    assert!(backups::create_backup(&fx.book, &KEY, &opt).is_err());
    assert!(!output.exists());
    backups::create_backup(&fx.book, &KEY, &f.options(&output, None)).unwrap();
    let target = f.restore.join("live.fbbook");
    let protected = [f.restore.clone()];
    let mut opt = f.roptions(&target);
    opt.protected = &protected;
    assert!(backups::restore_backup(&output, &KEY, &opt).is_err());
    assert!(!target.exists());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(f.restore.join("absent"), &target).unwrap();
        assert!(backups::restore_backup(&output, &KEY, &f.roptions(&target)).is_err());
        let sym = f.out.join("loop");
        std::os::unix::fs::symlink(&sym, &sym).unwrap();
        let nested = sym.join("b.fbbackup");
        assert!(backups::create_backup(&fx.book, &KEY, &f.options(&nested, None)).is_err());
    }
}

#[test]
fn python_crypto_vectors_cover_empty_exact_and_multichunk_nfc() {
    let vectors: Value = serde_json::from_str(include_str!("data/backups-crypto.json")).unwrap();
    let header = &vectors["header"];
    let aad = vectors["aad"].as_str().unwrap().as_bytes();
    for vector in vectors["vectors"].as_array().unwrap() {
        let size = vector["bytes"].as_u64().unwrap() as usize;
        let plain: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
        let mut sealed = Vec::new();
        crypto::encrypt_stream(
            &mut plain.as_slice(),
            &mut sealed,
            "synthetic cafe\u{301} passphrase",
            header,
            aad,
        )
        .unwrap();
        assert_eq!(
            sealed.len() as u64,
            vector["ciphertext_bytes"].as_u64().unwrap()
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(&sealed)),
            vector["sha256"].as_str().unwrap()
        );
        let mut opened = Vec::new();
        crypto::decrypt_stream(
            &mut sealed.as_slice(),
            &mut opened,
            "synthetic café passphrase",
            header,
            aad,
        )
        .unwrap();
        assert_eq!(opened, plain);
    }
    assert!(crypto::check_new_passphrase("            ").is_ok()); // copied Python oddity
    assert!(crypto::check_new_passphrase("ééééééééééé").is_err()); // characters, not UTF-8 bytes
}

#[test]
fn stream_tampering_and_resource_parameters_fail_closed() {
    let h = crypto::new_parameters();
    let aad = b"exact header bytes";
    let pass = "synthetic passphrase";
    let plain = vec![77; crypto::CHUNK + 1];
    let mut sealed = Vec::new();
    crypto::encrypt_stream(&mut plain.as_slice(), &mut sealed, pass, &h, aad).unwrap();
    for case in 0..6 {
        let mut bad = sealed.clone();
        match case {
            0 => bad[0] ^= 1,
            1 => {
                bad.pop();
            }
            2 => bad.truncate(crypto::CHUNK + crypto::TAG),
            3 => bad.extend_from_slice(&[0; 16]),
            4 => bad.rotate_left(crypto::CHUNK + crypto::TAG),
            _ => bad.clear(),
        }
        assert!(
            crypto::decrypt_stream(&mut bad.as_slice(), &mut Vec::new(), pass, &h, aad).is_err(),
            "case {case}"
        );
    }
    assert!(
        crypto::decrypt_stream(
            &mut sealed.as_slice(),
            &mut Vec::new(),
            pass,
            &h,
            b"edited header"
        )
        .is_err()
    );
    for path in ["n", "r", "p"] {
        let mut bad = h.clone();
        bad["kdf"][path] = json!(1u64 << 40);
        assert!(
            crypto::decrypt_stream(&mut sealed.as_slice(), &mut Vec::new(), pass, &bad, aad)
                .is_err()
        );
    }
    let mut bad = h.clone();
    bad["kdf"]["n"] = json!(true);
    assert!(
        crypto::decrypt_stream(&mut sealed.as_slice(), &mut Vec::new(), pass, &bad, aad).is_err()
    );
}

struct Short<'a>(&'a [u8]);
impl Read for Short<'_> {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        let n = b.len().min(7).min(self.0.len());
        b[..n].copy_from_slice(&self.0[..n]);
        self.0 = &self.0[n..];
        Ok(n)
    }
}
#[test]
fn short_read_streams_match_the_same_bytes() {
    let h = crypto::new_parameters();
    let mut normal = Vec::new();
    let mut short = Vec::new();
    let p = vec![41; 65536];
    crypto::encrypt_stream(
        &mut p.as_slice(),
        &mut normal,
        "synthetic passphrase",
        &h,
        b"aad",
    )
    .unwrap();
    crypto::encrypt_stream(
        &mut Short(&p),
        &mut short,
        "synthetic passphrase",
        &h,
        b"aad",
    )
    .unwrap();
    assert_eq!(normal, short);
    let mut out = Vec::new();
    crypto::decrypt_stream(
        &mut Short(&short),
        &mut out,
        "synthetic passphrase",
        &h,
        b"aad",
    )
    .unwrap();
    assert_eq!(out, p);
}

#[test]
fn recovery_sidecar_cross_machine_roundtrip_never_overwrites_or_serializes_master_key() {
    let f = Files::new();
    let path = f.restore.join("recovery-key.txt");
    let pass = "synthetic recovery passphrase";
    let wrapped = WrappedKey::new(&KEY, pass).unwrap();
    wrapped.write_once(&path).unwrap();
    assert!(wrapped.write_once(&path).is_err());
    let recovered = WrappedKey::read(&path).unwrap().unlock(pass).unwrap();
    assert_eq!(*recovered, KEY);
    assert!(WrappedKey::read(&path).unwrap().unlock("wrong").is_err());
    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains(pass));
    let mut value: Value = serde_json::from_str(&text).unwrap();
    value["nonce"] = json!("bad");
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(WrappedKey::read(&path).unwrap().unlock(pass).is_err());
}

#[test]
fn retention_exact_labels_verified_protection_clock_rollback_and_note_binding() {
    let f = Files::new();
    let mut mine = Vec::new();
    for day in 1..=5 {
        let p = f.out.join(format!(
            "freedombooks-synthetic-2020010{day}T000000Z.fbbackup"
        ));
        fs::write(&p, b"fake").unwrap();
        mine.push(p);
    }
    retention::record_verified(&mine[1]).unwrap();
    let other = f
        .out
        .join("freedombooks-synthetic_extra-20200101T000000Z.fbbackup");
    fs::write(&other, b"keep").unwrap();
    let removed = retention::prune(&f.out, "synthetic", 1, Some(&mine[0])).unwrap();
    assert_eq!(removed.len(), 2);
    assert!(mine[0].exists());
    assert!(mine[1].exists());
    assert!(mine[4].exists());
    assert!(other.exists());
    fs::write(&mine[1], b"same").unwrap();
    assert!(!retention::verified_note(&mine[1]));
    assert!(retention::prune(&f.out, "synthetic", 0, None).is_err());
    for label in ["bad/name", "", "a b", "é", "synthetic-foo"] {
        let actual = retention::safe_db_label(label);
        assert!(actual.len() <= 63);
        if label == "bad/name" {
            assert!(actual.starts_with("bad_name."));
        }
    }
    #[cfg(unix)]
    {
        let link = f
            .out
            .join("freedombooks-synthetic-20200106T000000Z.fbbackup");
        std::os::unix::fs::symlink(&other, &link).unwrap();
        assert!(
            !retention::backup_files(&f.out, Some("synthetic"))
                .unwrap()
                .contains(&link)
        );
    }
}

#[test]
fn raw_manifest_counts_schema_and_diff_limit() {
    let mut fx = Fx::new("backup-manifest");
    fx.chart();
    let contents = backups::manifest::from_book(&mut fx.book).unwrap();
    assert_eq!(contents.tables["fb_user"], 1);
    assert!(contents.schema.contains_key("table:fb_user"));
    let a = json!({"a":1,"b":2,"c":3});
    let b = json!({"a":2,"b":3,"c":4});
    assert_eq!(backups::manifest::differences(&a, &b, 2).len(), 2);
}

#[test]
fn genuine_base_native_backup_compares_before_upgrade_and_reopens() {
    let mut fx = Fx::new("backup-old");
    let (co, a, e) = fx.chart();
    let j = fx.draft(co, a, e, "77.77", "2026-10-01");
    fx.post(co, j.id);
    let f = Files::new();
    let old = f.restore.join("old.fbbook");
    let prior = fb_db::test_support::populated_copy(
        fx.book.connection(),
        &old,
        &KEY,
        "0078_shared_unique_json",
        true,
    )
    .unwrap();
    let contents =
        fb_core::Book::inspect_snapshot(&old, &KEY, |q| backups::manifest::contents(q)).unwrap();
    let bytes = fs::read(&old).unwrap();
    // The genuine prior list ends at the named boundary; later siblings (0079, 0100s, 0110s) sort before 0120.
    let boundary = fb_db::MIGRATIONS.iter().position(|(n, _)| *n == "0078_shared_unique_json").unwrap();
    let names: Vec<_> = fb_db::MIGRATIONS[..=boundary].iter().map(|(n, _)| *n).collect();
    let manifest = json!({"format":backups::FORMAT,"format_version":3,"created_at":"2026-10-30T12:00:00Z","app_version":"test-old","migrations":names,"book_bytes":bytes.len(),"book_sha256":format!("{:x}",Sha256::digest(&bytes)),"contents":contents,"attachments":[],"not_included":[]});
    let header = json!({"format":backups::FORMAT,"format_version":3,"encrypted":false,"database":"SQLCipher","manifest_nonce":backups::manifest::new_nonce()});
    let raw = serde_json::to_vec(&header).unwrap();
    let sealed =
        backups::manifest::encrypt(&serde_json::to_vec(&manifest).unwrap(), &KEY, &header, &raw)
            .unwrap();
    let output = f.out.join("old.fbbackup");
    write_archive(
        &output,
        vec![
            ("header.json".into(), raw),
            ("manifest.enc".into(), sealed),
            ("book.fbbook".into(), bytes),
        ],
    );
    backups::verify_backup(&output, &KEY, &f.work, None, true).unwrap();
    let note: Value =
        serde_json::from_slice(&fs::read(retention::sidecar(&output)).unwrap()).unwrap();
    assert_eq!(note["needs_migrate"], true);
    assert_eq!(note["app_version"], "test-old");
    let target = f.restore.join("upgraded.fbbook");
    let result = backups::restore_backup(&output, &KEY, &f.roptions(&target)).unwrap();
    assert!(result.upgraded);
    let book = fb_core::Book::open(&target, &KEY).unwrap();
    assert_eq!(
        fb_db::test_support::snapshot(book.connection()).unwrap(),
        prior.expected
    );
    drop(book);
    assert!(
        fb_core::Book::open(&target, &KEY)
            .unwrap()
            .check()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn encrypted_manifest_binds_header_and_wrong_master_key_before_book_access() {
    let fx = Fx::new("backup-meta");
    let f = Files::new();
    let output = f.out.join("good.fbbackup");
    backups::create_backup(&fx.book, &KEY, &f.options(&output, None)).unwrap();
    assert!(
        backups::verify_backup(&output, &[9; 32], &f.work, None, false)
            .unwrap_err()
            .to_string()
            .contains("manifest")
    );
    let mut members = archive(&output);
    let mut header: Value = serde_json::from_slice(&members[0].1).unwrap();
    header["note"] = json!("changed");
    members[0].1 = serde_json::to_vec(&header).unwrap();
    let bad = f.out.join("edited.fbbackup");
    write_archive(&bad, members);
    assert!(backups::verify_backup(&bad, &KEY, &f.work, None, false).is_err());
}

#[test]
fn restore_checks_existing_empty_nonempty_receipt_targets_and_stale_sidecars() {
    let fx = Fx::new("backup-targets");
    let f = Files::new();
    let output = f.out.join("good.fbbackup");
    backups::create_backup(&fx.book, &KEY, &f.options(&output, None)).unwrap();
    let target = f.restore.join("book.fbbook");
    let receipts = f.restore.join("receipts");
    fs::create_dir(&receipts).unwrap();
    let mut opt = f.roptions(&target);
    opt.receipts_target = Some(&receipts);
    fs::write(receipts.join("sentinel"), b"keep").unwrap();
    assert!(backups::restore_backup(&output, &KEY, &opt).is_err());
    assert!(!target.exists());
    assert!(receipts.join("sentinel").exists());
    fs::remove_file(receipts.join("sentinel")).unwrap();
    let side = f.restore.join("book.fbbook-wal");
    fs::write(&side, b"old wal").unwrap();
    assert!(backups::restore_backup(&output, &KEY, &opt).is_err());
    assert_eq!(fs::read(&side).unwrap(), b"old wal");
    fs::remove_file(side).unwrap();
    backups::restore_backup(&output, &KEY, &opt).unwrap();
    assert!(target.exists());
    assert!(receipts.is_dir());
}

#[test]
fn archive_links_traversal_extensions_and_oversized_metadata_are_refused() {
    let f = Files::new();
    for (case, name, kind, size) in [
        (0, "../escape", tar::EntryType::Regular, 0),
        (1, "header.json", tar::EntryType::Symlink, 0),
        (2, "header.json", tar::EntryType::Directory, 0),
        (3, "meta", tar::EntryType::XHeader, 0),
        (4, "header.json", tar::EntryType::Regular, 65537),
    ] {
        let path = f.out.join(format!("bad{case}.fbbackup"));
        let mut h = tar::Header::new_ustar();
        h.set_entry_type(kind);
        h.set_mode(0o600);
        h.set_size(size);
        let raw = h.as_mut_bytes();
        raw[..100].fill(0);
        raw[..name.len()].copy_from_slice(name.as_bytes());
        h.set_cksum();
        // Write raw header: tar's safe writer correctly refuses traversal names.
        fs::write(&path, h.as_bytes()).unwrap();
        let target = f.restore.join(format!("bad{case}.fbbook"));
        assert!(backups::restore_backup(&path, &KEY, &f.roptions(&target)).is_err());
        assert!(!target.exists());
        assert!(!f._tmp.path().join("escape").exists());
    }
}

#[test]
fn stale_cleanup_keeps_live_other_host_and_unrecognized_work() {
    let f = Files::new();
    fs::create_dir(&f.work).unwrap();
    let host = format!(
        "{:x}",
        Sha256::digest(
            fs::read_to_string("/etc/hostname")
                .unwrap()
                .trim()
                .as_bytes()
        )
    );
    let dead = f.work.join(format!(
        ".fb-backup.{}.4294967295.ABCDEF123456",
        &host[..16]
    ));
    fs::create_dir(&dead).unwrap();
    fs::write(dead.join("private"), b"synthetic").unwrap();
    let live = f.work.join(format!(
        ".fb-backup.{}.{}.ABCDEF123456",
        &host[..16],
        std::process::id()
    ));
    fs::create_dir(&live).unwrap();
    let unknown = f.work.join("notes");
    fs::write(&unknown, b"keep").unwrap();
    let other = f.work.join(".fb-backup.other.4294967295.ABCDEF123456");
    fs::create_dir(&other).unwrap();
    let removed = backups::cleanup::cleanup_stale(&f.work).unwrap();
    assert_eq!(removed, vec![dead]);
    assert!(live.exists());
    assert!(unknown.exists());
    assert!(other.exists());
}

#[test]
fn directory_lock_serializes_backups_without_touching_existing_files() {
    let fx = Fx::new("backup-lock");
    let f = Files::new();
    let output = f.out.join("good.fbbackup");
    let lock_path = f.out.join(".fb-backup.lock");
    let lock = fs::File::create(lock_path).unwrap();
    fs2::FileExt::lock_exclusive(&lock).unwrap();
    assert!(
        backups::create_backup(&fx.book, &KEY, &f.options(&output, None))
            .err()
            .unwrap()
            .to_string()
            .contains("Another backup")
    );
    assert!(!output.exists());
    fs2::FileExt::unlock(&lock).unwrap();
    backups::create_backup(&fx.book, &KEY, &f.options(&output, None)).unwrap();
}
