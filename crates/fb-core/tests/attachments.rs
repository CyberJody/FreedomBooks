//! Service, storage and direct-SQL regressions from the frozen attachment tests.
mod common;
use common::*;
use fb_core::{
    Book,
    attachments::{
        self as a, Limits, Owner,
        storage::{Store, Stored},
    },
    services, users,
};
use fb_db::rusqlite::{self};
use serde_json::{Value, json};
use std::fs;
const PDF: &[u8] = b"%PDF-1.4\n1 0 obj<</Type/Catalog>>endobj\n%%EOF\n";
struct F {
    fx: Fx,
    _dir: tempfile::TempDir,
    store: Store,
    co: fb_core::CompanyId,
    owner: Owner,
    path: std::path::PathBuf,
}
impl F {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.fbbook");
        let mut book = Book::create(&path, &KEY).unwrap();
        let user = users::create_user(&mut book, "tester", false).unwrap();
        let mut fx = Fx { book, user };
        let (co, cash, eq) = fx.chart();
        let j = fx.draft(co, cash, eq, "1.00", "2026-01-01");
        let store = Store::for_book(&path, &KEY).unwrap();
        Self {
            fx,
            _dir: dir,
            store,
            co,
            owner: Owner::Journal(j.id),
            path,
        }
    }
    fn add(&mut self, name: &str, bytes: &[u8]) -> Value {
        a::add(
            &mut self.fx.book,
            &self.store,
            self.fx.user,
            self.co,
            &self.owner,
            bytes,
            name,
            &uuid::Uuid::new_v4().to_string(),
            &Limits::default(),
        )
        .unwrap()
        .0
    }
    fn snapshot(&self) -> fb_db::test_support::Snapshot {
        fb_db::test_support::snapshot(self.fx.book.connection()).unwrap()
    }
}
#[test]
fn add_retry_remove_and_download_preserve_bytes_and_audit() {
    let mut f = F::new();
    let key = uuid::Uuid::new_v4().to_string();
    let (row, new) = a::add(
        &mut f.fx.book,
        &f.store,
        f.fx.user,
        f.co,
        &f.owner,
        PDF,
        "scan",
        &key,
        &Limits::default(),
    )
    .unwrap();
    assert!(new);
    assert_eq!(row["original_name"], "scan.pdf");
    let before = f.snapshot();
    assert!(
        !a::add(
            &mut f.fx.book,
            &f.store,
            f.fx.user,
            f.co,
            &f.owner,
            PDF,
            "scan",
            &key,
            &Limits::default()
        )
        .unwrap()
        .1
    );
    assert_eq!(before, f.snapshot());
    let id = row["id"].as_str().unwrap();
    assert_eq!(
        a::download(&mut f.fx.book, &f.store, f.fx.user, f.co, id).unwrap(),
        PDF
    );
    assert!(a::remove(&mut f.fx.book, f.fx.user, f.co, id, "  ").is_err());
    assert!(a::remove(&mut f.fx.book, f.fx.user, f.co, id, "wrong\nfile").is_err());
    let removed = a::remove(&mut f.fx.book, f.fx.user, f.co, id, " wrong\t file ").unwrap();
    assert!(removed.2);
    assert_eq!(removed.0["delete_reason"], "wrong file");
    assert!(
        !a::remove(&mut f.fx.book, f.fx.user, f.co, id, "again")
            .unwrap()
            .2
    );
    assert_eq!(
        a::download(&mut f.fx.book, &f.store, f.fx.user, f.co, id).unwrap(),
        PDF
    );
    assert!(
        a::add(
            &mut f.fx.book,
            &f.store,
            f.fx.user,
            f.co,
            &f.owner,
            PDF,
            "scan",
            &key,
            &Limits::default()
        )
        .is_err()
    );
    assert_eq!(f.fx.audit_count("attachment.added"), 1);
    assert_eq!(f.fx.audit_count("attachment.removed"), 1);
    assert!(f.fx.book.check().unwrap().is_empty());
    drop(f.fx);
    assert!(
        Book::open(&f.path, &KEY)
            .unwrap()
            .check()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn dedupe_retained_references_and_encryption() {
    let mut f = F::new();
    let one = f.add("one.pdf", PDF);
    let two = f.add("two.pdf", PDF);
    assert_eq!(one["stored_name"], two["stored_name"]);
    assert_eq!(fs::read_dir(f.store.folder()).unwrap().count(), 1);
    let ciphertext = fs::read(f.store.folder().join(one["stored_name"].as_str().unwrap())).unwrap();
    assert!(!ciphertext.windows(5).any(|s| s == b"%PDF-"));
    assert_eq!(ciphertext.len(), PDF.len() + 16);
    let n: i64 =
        f.fx.book
            .connection()
            .query_row(
                "SELECT reference_count FROM attachment_file_refs",
                [],
                |r| r.get(0),
            )
            .unwrap();
    assert_eq!(n, 2);
}
#[test]
fn caps_empty_invalid_extension_rate_and_company_scope() {
    let mut f = F::new();
    for _ in 0..20 {
        f.add("a.pdf", PDF);
    }
    assert!(
        a::add(
            &mut f.fx.book,
            &f.store,
            f.fx.user,
            f.co,
            &f.owner,
            PDF,
            "next.pdf",
            &uuid::Uuid::new_v4().to_string(),
            &Limits::default()
        )
        .unwrap_err()
        .message()
        .contains("20 attachments")
    );
    let id = a::all(f.fx.book.connection(), f.co).unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    a::remove(&mut f.fx.book, f.fx.user, f.co, &id, "space").unwrap();
    f.add("next.pdf", PDF);
    for (data, name) in [
        (b"".as_slice(), "empty"),
        (b"<html>not a file", "fake.pdf"),
        (PDF, "fake.exe"),
    ] {
        assert!(a::inspect(data, name, &Limits::default()).is_err());
    }
    let other = f.fx.company("Other");
    assert!(a::download(&mut f.fx.book, &f.store, f.fx.user, other, &id).is_err());
    let outsider = users::create_user(&mut f.fx.book, "outsider", false).unwrap();
    assert!(a::download(&mut f.fx.book, &f.store, outsider, f.co, &id).is_err());
}
#[test]
fn size_and_throttle_caps_include_removed_rows() {
    let mut f = F::new();
    let r = f.add("one.pdf", PDF);
    a::remove(
        &mut f.fx.book,
        f.fx.user,
        f.co,
        r["id"].as_str().unwrap(),
        "wrong",
    )
    .unwrap();
    let l = Limits {
        uploads_per_window: 1,
        ..Limits::default()
    };
    assert!(
        a::add(
            &mut f.fx.book,
            &f.store,
            f.fx.user,
            f.co,
            &f.owner,
            PDF,
            "two.pdf",
            &uuid::Uuid::new_v4().to_string(),
            &l
        )
        .unwrap_err()
        .message()
        .contains("Too many")
    );
    let l = Limits {
        max_mb: 1,
        ..Limits::default()
    };
    let mut big = b"%PDF-1.4\n".to_vec();
    big.resize(1024 * 1024 - 6, b'x');
    big.extend(b"%%EOF\n");
    assert_eq!(big.len(), 1024 * 1024);
    a::inspect(&big, "big.pdf", &l).unwrap();
    big.push(0);
    assert!(a::inspect(&big, "big.pdf", &l).is_err());
    assert!(
        a::check_limits(
            f.fx.book.connection(),
            f.fx.user,
            f.co,
            1024 * 1024,
            &Limits {
                company_max_mb: 1,
                ..Limits::default()
            }
        )
        .is_err()
    );
}
#[test]
fn adopt_authenticates_all_bytes_and_metadata() {
    let mut f = F::new();
    let file = f.store.write_atomic(f.co, PDF, "application/pdf").unwrap();
    let key = uuid::Uuid::new_v4().to_string();
    let row =
        f.fx.book
            .write(|tx| {
                a::adopt_tx(
                    tx,
                    &f.store,
                    f.fx.user,
                    f.co,
                    &f.owner,
                    &file,
                    "receipt.pdf",
                    &key,
                    &Limits::default(),
                )
            })
            .unwrap()
            .0;
    assert_eq!(row["sha256"], file.sha256);
    let mut wrong = file.clone();
    wrong.sha256 = "0".repeat(64);
    assert!(
        f.fx.book
            .write(|tx| a::adopt_tx(
                tx,
                &f.store,
                f.fx.user,
                f.co,
                &f.owner,
                &wrong,
                "receipt.pdf",
                &uuid::Uuid::new_v4().to_string(),
                &Limits::default()
            ))
            .is_err()
    );
}
#[test]
fn every_sql_identity_and_replacement_entrypoint_is_refused() {
    let mut f = F::new();
    let row = f.add("one.pdf", PDF);
    let second = f.add("two.pdf", PDF);
    let id = row["id"].as_str().unwrap();
    let before = f.snapshot();
    let mut hits = std::collections::BTreeSet::new();
    for recursion in ["ON", "OFF"] {
        f.fx.book
            .connection()
            .execute_batch(&format!("PRAGMA recursive_triggers={recursion}"))
            .unwrap();
        for policy in ["", " OR REPLACE"] {
            for field in [
                "id",
                "company_id",
                "document_id",
                "journal_id",
                "original_name",
                "stored_name",
                "content_type",
                "size",
                "sha256",
                "original_sha256",
                "wrapped_key",
                "wrap_nonce",
                "nonce",
                "uploaded_by_id",
                "uploaded_by_name",
                "uploaded_at",
                "request_key",
            ] {
                // A changed value of the column's own class: the attack is the change, not a malformed value.
                let value = match field {
                    "size" => "size+1".into(),
                    "uploaded_at" => "'2026-01-01T00:00:00.000000Z'".into(),
                    _ => format!("'{}'", uuid::Uuid::new_v4()),
                };
                let sql = format!(
                    "UPDATE{policy} attachments_attachment SET {field}={value} WHERE id='{id}'"
                );
                hits.insert((format!("UPDATE{policy} {field}"), attack(&f.fx.book, &sql)));
            }
            let sql = format!(
                "UPDATE{policy} attachments_attachment SET request_key='{}' WHERE id='{id}'",
                second["request_key"].as_str().unwrap()
            );
            hits.insert((format!("UPDATE{policy} request_key=second"), attack(&f.fx.book, &sql)));
        }
        for alias in ["rowid", "_rowid_", "oid"] {
            let sql = format!("UPDATE OR REPLACE attachments_attachment SET {alias}=1 WHERE id='{id}'");
            hits.insert((format!("UPDATE OR REPLACE {alias}"), attack(&f.fx.book, &sql)));
        }
        let sql = format!("DELETE FROM attachments_attachment WHERE id='{id}'");
        hits.insert(("DELETE".into(), attack(&f.fx.book, &sql)));
        let cols = columns(f.fx.book.connection());
        for policy in ["OR REPLACE", "OR IGNORE"] {
            let sql = format!("INSERT {policy} INTO attachments_attachment({cols}) SELECT {cols} FROM attachments_attachment WHERE id='{id}'");
            hits.insert((format!("INSERT {policy}"), attack(&f.fx.book, &sql)));
        }
        assert_eq!(before, f.snapshot());
    }
    // Every entrypoint is refused by the attachment guard written for it (both recursion settings agree),
    // never by the shared value guard.
    // Non-removal updates meet the owner-number snapshot guard (it fires before attachment_immutable);
    // the table is WITHOUT ROWID, so the rowid aliases do not exist at all.
    let hits = hits
        .into_iter()
        .map(|(k, v)| format!("{k} -> {}", v.unwrap_or_else(|| "ACCEPTED".into())))
        .collect::<std::collections::BTreeSet<_>>();
    let mut expected = std::collections::BTreeSet::new();
    for policy in ["", " OR REPLACE"] {
        for field in [
            "id", "company_id", "document_id", "journal_id", "original_name", "stored_name", "content_type", "size",
            "sha256", "original_sha256", "wrapped_key", "wrap_nonce", "nonce", "uploaded_by_id", "uploaded_by_name",
            "uploaded_at", "request_key", "request_key=second",
        ] {
            expected.insert(format!("UPDATE{policy} {field} -> attachment_number"));
        }
    }
    for alias in ["rowid", "_rowid_", "oid"] {
        expected.insert(format!("UPDATE OR REPLACE {alias} -> other: no such column: {alias}"));
    }
    expected.insert("DELETE -> attachment_immutable".into());
    expected.insert("INSERT OR REPLACE -> attachment_keys".into());
    expected.insert("INSERT OR IGNORE -> attachment_keys".into());
    assert_eq!(hits, expected);
    f.fx.book
        .connection()
        .execute_batch("PRAGMA recursive_triggers=ON")
        .unwrap();
    assert!(f.fx.book.check().unwrap().is_empty());
}
fn columns(c: &rusqlite::Connection) -> String {
    c.prepare("SELECT * FROM attachments_attachment")
        .unwrap()
        .column_names()
        .collect::<Vec<_>>()
        .join(",")
}
#[test]
fn missing_cross_company_owners_removed_insert_and_cap_fail_at_sql_boundary() {
    let mut f = F::new();
    let r = f.add("one.pdf", PDF);
    let id = r["id"].as_str().unwrap();
    let cols = columns(f.fx.book.connection());
    let other = f.fx.company("Other");
    let before = f.snapshot();
    let mut hits = vec![];
    // The missing uploader is a well-formed id with no fb_user row.
    for (changes, guard) in [
        (format!("company_id='{}'", other.to_db()), "attachment_owner"),
        (format!("journal_id='{}'", fb_core::JournalId::new()), "attachment_owner"),
        ("deleted=1".into(), "attachment_insert_active"),
        (format!("uploaded_by_id='{}'", fb_core::UserId::new()), "attachment_actor"),
        ("original_name=''".into(), "length(trim(original_name)) BETWEEN 1 AND 120"),
        ("stored_name='../evil.bin'".into(), "attachment_blob"),
        ("sha256='bad'".into(), "attachment_blob"),
        ("wrapped_key='bad'".into(), "attachment_blob"),
    ] {
        let mut select = cols.split(',').map(str::to_string).collect::<Vec<_>>();
        for pair in changes.split(',') {
            let (k, v) = pair.split_once('=').unwrap();
            let i = select.iter().position(|s| s == k).unwrap();
            select[i] = v.into();
        }
        select[0] = format!("'{}'", uuid::Uuid::new_v4());
        let req = cols.split(',').position(|k| k == "request_key").unwrap();
        select[req] = format!("'{}'", uuid::Uuid::new_v4());
        let got = attack(&f.fx.book,&format!("INSERT INTO attachments_attachment({cols}) SELECT {} FROM attachments_attachment WHERE id='{id}'",select.join(",")));
        hits.push((changes.split('=').next().unwrap().to_owned(), got.unwrap_or_else(|| "ACCEPTED".into()), guard));
        assert_eq!(before, f.snapshot());
    }
    let mismatched = hits.iter().filter(|(_, got, want)| got != want).collect::<Vec<_>>();
    assert!(mismatched.is_empty(), "{mismatched:#?}");
}
#[test]
fn late_audit_failure_rolls_back_add_remove_and_caught_adopt() {
    let mut f = F::new();
    let row = f.add("keep.pdf", PDF);
    let before = f.snapshot();
    f.fx.book.connection().execute_batch("CREATE TRIGGER fail_attachment_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action GLOB 'attachment.*' BEGIN SELECT RAISE(ABORT,'late_audit'); END;").unwrap();
    assert!(
        a::add(
            &mut f.fx.book,
            &f.store,
            f.fx.user,
            f.co,
            &f.owner,
            PDF,
            "second.pdf",
            &uuid::Uuid::new_v4().to_string(),
            &Limits::default()
        )
        .is_err()
    );
    assert!(
        a::remove(
            &mut f.fx.book,
            f.fx.user,
            f.co,
            row["id"].as_str().unwrap(),
            "no"
        )
        .is_err()
    );
    let file: Stored = serde_json::from_value(row.clone()).unwrap();
    f.fx.book
        .write(|tx| -> fb_core::FbResult<()> {
            assert!(
                a::adopt_tx(
                    tx,
                    &f.store,
                    f.fx.user,
                    f.co,
                    &f.owner,
                    &file,
                    "adopt.pdf",
                    &uuid::Uuid::new_v4().to_string(),
                    &Limits::default()
                )
                .is_err()
            );
            assert!(a::remove_tx(tx, f.fx.user, f.co, row["id"].as_str().unwrap(), "no").is_err());
            Ok(())
        })
        .unwrap();
    f.fx.book
        .connection()
        .execute_batch("DROP TRIGGER fail_attachment_audit")
        .unwrap();
    assert_eq!(before, f.snapshot());
    assert_eq!(fs::read_dir(f.store.folder()).unwrap().count(), 1);
}
#[test]
fn reads_refuse_tamper_wrong_master_symlinks_and_traversal() {
    let mut f = F::new();
    let r = f.add("keep.pdf", PDF);
    let file: Stored = serde_json::from_value(r).unwrap();
    let wrong = Store::for_book(&f.path, &[7; 32]).unwrap();
    assert!(wrong.open_read(f.co, &file).is_err());
    for name in [
        "../foo",
        "",
        "a.pdf",
        "a",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.bin/other",
    ] {
        assert!(!a::storage::valid_name(name));
    }
    let path = f.store.folder().join(&file.stored_name);
    let mut bytes = fs::read(&path).unwrap();
    bytes[0] ^= 1;
    fs::write(&path, &bytes).unwrap();
    assert!(f.store.open_read(f.co, &file).is_err());
    fs::remove_file(&path).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&f.path, &path).unwrap();
        assert!(f.store.open_read(f.co, &file).is_err());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(f.store.folder()).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}
#[test]
fn store_refuses_symlink_directory_and_sets_private_modes() {
    let dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};
        symlink(target.path(), dir.path().join("receipts")).unwrap();
        assert!(Store::for_book(&dir.path().join("a.fbbook"), &KEY).is_err());
        fs::remove_file(dir.path().join("receipts")).unwrap();
        let store = Store::for_book(&dir.path().join("a.fbbook"), &KEY).unwrap();
        let file = store
            .write_atomic(fb_core::CompanyId::new(), PDF, "application/pdf")
            .unwrap();
        assert_eq!(
            fs::metadata(store.folder().join(file.stored_name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
#[test]
fn populated_50c3f74_book_upgrades_without_altering_any_old_row() {
    let mut f = F::new();
    if let Owner::Journal(j) = f.owner {
        f.fx.post(f.co, j);
        services::reverse_journal(&mut f.fx.book, f.fx.user, f.co, j, "2026-02-01", "wrong")
            .unwrap();
    }
    let copy_path = f._dir.path().join("prior.fbbook");
    let copy = fb_db::test_support::populated_copy(
        f.fx.book.connection(),
        &copy_path,
        &KEY,
        "0078_shared_unique_json",
        true,
    )
    .unwrap();
    let upgraded = Book::open(&copy_path, &KEY).unwrap();
    assert_eq!(
        fb_db::test_support::snapshot(upgraded.connection()).unwrap(),
        copy.expected
    );
    assert!(upgraded.check().unwrap().is_empty());
    drop(upgraded);
    assert!(
        Book::open(&copy_path, &KEY)
            .unwrap()
            .check()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn structure_and_names_match_frozen_python_vectors() {
    let vectors: Value = serde_json::from_str(include_str!("attachments_vectors.json")).unwrap();
    for v in vectors["files"].as_array().unwrap() {
        let raw = v["hex"].as_str().unwrap();
        let bytes = raw
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        let t = a::sniff::sniff(&bytes);
        assert_eq!(json!(t), v["type"]);
        if let Some(t) = t {
            assert_eq!(
                json!(a::sniff::problem(&bytes, t)),
                v["problem"],
                "{}",
                v["label"]
            );
        }
    }
    for v in vectors["names"].as_array().unwrap() {
        assert_eq!(a::sniff::clean_name(v["raw"].as_str().unwrap()), v["clean"]);
        let result =
            a::sniff::display_name(v["raw"].as_str().unwrap(), v["type"].as_str().unwrap());
        match result {
            Ok(s) => assert_eq!(json!(s), v["display"]),
            Err(e) => assert_eq!(e.message(), v["error"]),
        }
    }
}

#[test]
fn all_new_schema_guards_are_hashed_and_required() {
    let f = F::new();
    let guards = f
        .fx
        .book
        .connection()
        .prepare(
            "SELECT name,sql FROM sqlite_master WHERE type='trigger' AND name GLOB 'attachment_*'",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    for (name, sql) in guards {
        f.fx.book
            .connection()
            .execute_batch(&format!("DROP TRIGGER {name}"))
            .unwrap();
        assert!(!f.fx.book.check().unwrap().is_empty(), "{name}");
        assert!(Book::open(&f.path, &KEY).is_err(), "{name}");
        f.fx.book.connection().execute_batch(&sql).unwrap();
    }
    assert!(f.fx.book.check().unwrap().is_empty());
}

#[test]
fn unaudited_insert_cannot_seal_or_commit() {
    let mut f = F::new();
    let r = f.add("one.pdf", PDF);
    let columns = columns(f.fx.book.connection());
    let original = r["id"].as_str().unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let key = uuid::Uuid::new_v4().to_string();
    let select = columns
        .split(',')
        .map(|c| match c {
            "id" => format!("'{id}'"),
            "request_key" => format!("'{key}'"),
            _ => c.into(),
        })
        .collect::<Vec<_>>()
        .join(",");
    let before = f.snapshot();
    let result=f.fx.book.write(|w|->fb_core::FbResult<()>{w.execute(&format!("INSERT INTO attachments_attachment({columns}) SELECT {select} FROM attachments_attachment WHERE id=?"),[original])?;w.execute("INSERT INTO fb_seal VALUES('attachment_added',?)",[&id])?;Ok(())});
    assert_eq!(result.unwrap_err().constraint(), Some("attachment_audit"));
    assert_eq!(before, f.snapshot());
    let result=f.fx.book.write(|w|->fb_core::FbResult<()>{w.execute(&format!("INSERT INTO attachments_attachment({columns}) SELECT {select} FROM attachments_attachment WHERE id=?"),[original])?;Ok(())});
    assert!(result.is_err());
    assert_eq!(before, f.snapshot());
}

#[test]
fn photo_reduction_is_bounded_keeps_original_fingerprint_and_retries() {
    let mut f = F::new();
    let mut pixels = vec![0; 2300 * 2100 * 3];
    let mut state = 19u64;
    for p in &mut pixels {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *p = state as u8;
    }
    let photo = image::RgbImage::from_raw(2300, 2100, pixels).unwrap();
    let mut png = std::io::Cursor::new(vec![]);
    image::DynamicImage::ImageRgb8(photo)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let bytes = png.into_inner();
    assert!(bytes.len() > 1536 * 1024);
    let key = uuid::Uuid::new_v4().to_string();
    let limits = Limits {
        max_mb: 100,
        ..Limits::default()
    };
    let (row, created) = a::add(
        &mut f.fx.book,
        &f.store,
        f.fx.user,
        f.co,
        &f.owner,
        &bytes,
        "phone.png",
        &key,
        &limits,
    )
    .unwrap();
    assert!(created);
    assert_eq!(row["original_name"], "phone.jpg");
    assert_eq!(row["content_type"], "image/jpeg");
    assert_ne!(row["sha256"], row["original_sha256"]);
    let data = a::download(
        &mut f.fx.book,
        &f.store,
        f.fx.user,
        f.co,
        row["id"].as_str().unwrap(),
    )
    .unwrap();
    let image = image::load_from_memory(&data).unwrap();
    assert!(image.width() <= 2000 && image.height() <= 2000);
    assert!(
        !a::add(
            &mut f.fx.book,
            &f.store,
            f.fx.user,
            f.co,
            &f.owner,
            &bytes,
            "phone.png",
            &key,
            &limits
        )
        .unwrap()
        .1
    );
    let detail: String =
        f.fx.book
            .connection()
            .query_row(
                "SELECT detail FROM ledger_auditevent WHERE action='attachment.added'",
                [],
                |r| r.get(0),
            )
            .unwrap();
    let detail: Value = serde_json::from_str(&detail).unwrap();
    assert_eq!(detail["original_sha256"], row["original_sha256"]);
    let (staged, name) = a::stage(&f.store, f.co, &bytes, "phone.png", &limits).unwrap();
    assert_eq!(name, "phone.jpg");
    assert_eq!(staged.original_sha256, row["original_sha256"]);
    let (adopted, created) = a::adopt(
        &mut f.fx.book,
        &f.store,
        f.fx.user,
        f.co,
        &f.owner,
        &staged,
        &name,
        &uuid::Uuid::new_v4().to_string(),
        &limits,
    )
    .unwrap();
    assert!(created);
    assert_eq!(adopted["original_sha256"], row["original_sha256"]);
    assert_eq!(adopted["sha256"], row["sha256"]);
    assert_eq!(adopted["stored_name"], row["stored_name"]);
    assert!(f.fx.book.check().unwrap().is_empty());
}

#[test]
fn streaming_cipher_rejects_reordered_chunks_and_roundtrips_boundaries() {
    let f = F::new();
    for size in [
        1024 * 1024 - 1,
        1024 * 1024,
        1024 * 1024 + 1,
        2 * 1024 * 1024,
    ] {
        let mut bytes = b"%PDF-1.4\n".to_vec();
        bytes.resize(size - 6, b'x');
        bytes.extend(b"%%EOF\n");
        let file = f
            .store
            .write_atomic(f.co, &bytes, "application/pdf")
            .unwrap();
        assert_eq!(f.store.open_read(f.co, &file).unwrap(), bytes);
        let path = f.store.folder().join(&file.stored_name);
        let mut encrypted = fs::read(&path).unwrap();
        if encrypted.len() > 1024 * 1024 + 16 {
            let split = 1024 * 1024 + 16;
            encrypted.rotate_left(split);
            fs::write(&path, encrypted).unwrap();
            assert!(f.store.open_read(f.co, &file).is_err());
        }
    }
}

#[test]
fn attachment_uuid_is_checked_at_the_storage_boundary() {
    let mut f = F::new();
    let r = f.add("one.pdf", PDF);
    let cols = columns(f.fx.book.connection());
    let select = cols
        .split(',')
        .map(|c| match c {
            "id" => "'invalid-id'".into(),
            "request_key" => format!("'{}'", uuid::Uuid::new_v4()),
            _ => c.into(),
        })
        .collect::<Vec<String>>()
        .join(",");
    let before = f.snapshot();
    let sql = format!("INSERT INTO attachments_attachment({cols}) SELECT {select} FROM attachments_attachment WHERE id='{}'", r["id"].as_str().unwrap());
    // Dual variant: (a) the shared value guard refuses the malformed id first; (b) without it, the
    // attachments' own reviewed UUID check (0111 fb_attachment_uuid) still refuses it.
    assert_eq!(attack(&f.fx.book, &sql).as_deref(), Some("fb_valid"));
    assert_eq!(
        without_trigger(&f.fx.book, "fb_valid_attachments_attachment_insert", &sql).as_deref(),
        Some("attachment_text")
    );
    assert_eq!(before, f.snapshot());
    assert!(f.fx.book.check().unwrap().is_empty());
}

/// Run one raw statement with a single trigger dropped inside a savepoint, then restore everything.
fn without_trigger(book: &Book, trigger: &str, sql: &str) -> Option<String> {
    let conn = book.connection();
    conn.execute_batch(&format!("SAVEPOINT isolate; DROP TRIGGER {trigger}")).unwrap();
    let outcome = conn.execute_batch(sql).err().map(|e| fb_db::constraint_of(&e).unwrap_or_else(|| format!("other: {e}")));
    conn.execute_batch("ROLLBACK TO isolate; RELEASE isolate").unwrap();
    outcome
}

#[test]
fn audit_seal_requires_the_real_owner_and_number_and_original_hash() {
    for field in ["target", "number", "original_sha256"] {
        let mut f = F::new();
        let row = f.add("one.pdf", PDF);
        let id = row["id"].as_str().unwrap();
        let trigger: String =
            f.fx.book
                .connection()
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE name='fb_audit_no_update'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
        f.fx.book
            .connection()
            .execute_batch("DROP TRIGGER fb_audit_no_update")
            .unwrap();
        let update = if field == "target" {
            "UPDATE ledger_auditevent SET target='journal:wrong' WHERE action='attachment.added'"
                .to_string()
        } else {
            format!(
                "UPDATE ledger_auditevent SET detail=json_set(detail,'$.{field}','wrong') WHERE action='attachment.added'"
            )
        };
        f.fx.book.connection().execute_batch(&update).unwrap();
        f.fx.book.connection().execute_batch(&trigger).unwrap();
        // A fresh seal must verify every immutable audit field, independently of any prior seal.
        let result: fb_core::FbResult<()> = f.fx.book.write(|w| {
            w.execute("INSERT INTO fb_seal VALUES('attachment_added',?)", [id])?;
            Ok(())
        });
        assert_eq!(
            result.unwrap_err().constraint(),
            Some("attachment_audit"),
            "seal accepted corrupt {field}"
        );
        assert!(
            !f.fx.book.check().unwrap().is_empty(),
            "open scan missed {field}"
        );
        assert!(
            Book::open(&f.path, &KEY).is_err(),
            "reopen accepted {field}"
        );
    }
}

#[test]
fn raw_insert_cap_blob_consistency_and_parent_deletion_are_independent() {
    let mut f = F::new();
    let row = f.add("one.pdf", PDF);
    let id = row["id"].as_str().unwrap();
    let cols = columns(f.fx.book.connection());
    for (column, value, constraint) in [
        ("nonce", format!("'{}'", "0".repeat(14)), "attachment_blob"),
        (
            "uploaded_by_id",
            format!("'{}'", fb_core::UserId::new()),
            "attachment_actor",
        ),
        ("uploaded_number", "'wrong'".into(), "attachment_number"),
    ] {
        let select = cols
            .split(',')
            .map(|c| {
                if c == "id" || c == "request_key" {
                    format!("'{}'", uuid::Uuid::new_v4())
                } else if c == column {
                    value.clone()
                } else {
                    c.into()
                }
            })
            .collect::<Vec<String>>()
            .join(",");
        let sql = format!(
            "INSERT INTO attachments_attachment({cols}) SELECT {select} FROM attachments_attachment WHERE id='{id}'"
        );
        let error = attack(&f.fx.book, &sql).unwrap();
        assert!(error.to_string().contains(constraint), "{error}");
    }
    for _ in 1..20 {
        f.add("same.pdf", PDF);
    }
    let before = f.snapshot();
    let select = cols
        .split(',')
        .map(|c| {
            if c == "id" || c == "request_key" {
                format!("'{}'", uuid::Uuid::new_v4())
            } else {
                c.into()
            }
        })
        .collect::<Vec<String>>()
        .join(",");
    assert!(attack(&f.fx.book, &format!("INSERT INTO attachments_attachment({cols}) SELECT {select} FROM attachments_attachment WHERE id='{id}'")).unwrap().to_string().contains("attachment_limit"));
    for (table, key) in [
        ("ledger_company", f.co.to_db()),
        ("ledger_journal", f.owner.key().1),
        ("fb_user", f.fx.user.to_db()),
    ] {
        assert!(attack(&f.fx.book, &format!("DELETE FROM {table} WHERE id='{key}'")).is_some());
    }
    assert_eq!(before, f.snapshot());
}

#[test]
fn manual_journal_number_edits_keep_both_attachment_action_snapshots() {
    let mut f = F::new();
    let row = f.add("one.pdf", PDF);
    let before_number = row["uploaded_number"].clone();
    let id = f.owner.key().1;
    let (debit,credit):(String,String)=f.fx.book.connection().query_row("SELECT (SELECT account_id FROM ledger_journalline WHERE journal_id=?1 AND debit>0),(SELECT account_id FROM ledger_journalline WHERE journal_id=?1 AND credit>0)",[&id],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    services::save_draft(
        &mut f.fx.book,
        f.fx.user,
        f.co,
        services::DraftInput {
            journal_id: fb_core::JournalId::parse(&id),
            date: "2026-01-01".into(),
            memo: "Edited entry label".into(),
            entry_number: Some("RENAMED".into()),
            expected_revision: Some(1),
            lines: vec![
                json!({"account":debit,"debit":"1.00","credit":"0"}),
                json!({"account":credit,"debit":"0","credit":"1.00"}),
            ],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(f.fx.book.check().unwrap().is_empty());
    assert!(Book::open(&f.path, &KEY).is_ok());
    let r = a::remove(
        &mut f.fx.book,
        f.fx.user,
        f.co,
        row["id"].as_str().unwrap(),
        "wrong file",
    )
    .unwrap()
    .0;
    assert_eq!(r["uploaded_number"], before_number);
    assert_eq!(r["deleted_number"], "RENAMED");
    assert!(f.fx.book.check().unwrap().is_empty());
    assert!(Book::open(&f.path, &KEY).is_ok());
}
