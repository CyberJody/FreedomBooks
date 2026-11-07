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
}
fn columns(c: &rusqlite::Connection) -> String {
    c.prepare("SELECT * FROM attachments_attachment")
        .unwrap()
        .column_names()
        .collect::<Vec<_>>()
        .join(",")
}
/// Run one raw statement with a single trigger dropped inside a savepoint, then restore everything.
/// Used only for the dual-variant value cases: the shared 0130 guard (`fb_valid`) fires first on these
/// values, so dropping just that table's guard proves the attachments' own 0111 check also refuses them.
fn without_trigger(book: &Book, trigger: &str, sql: &str) -> Option<String> {
    let conn = book.connection();
    conn.execute_batch(&format!("SAVEPOINT isolate; DROP TRIGGER {trigger}")).unwrap();
    let outcome = conn.execute_batch(sql).err().map(|e| fb_db::constraint_of(&e).unwrap_or_else(|| format!("other: {e}")));
    conn.execute_batch("ROLLBACK TO isolate; RELEASE isolate").unwrap();
    outcome
}
const INSERT_GUARD: &str = "fb_valid_attachments_attachment_insert";
const UPDATE_GUARD: &str = "fb_valid_attachments_attachment_update";
#[test]
fn review_invalid_upload_timestamp_must_be_refused() {
    let mut f = F::new();
    let row = f.add("one.pdf", PDF);
    let cols = columns(f.fx.book.connection());
    let id = uuid::Uuid::new_v4().to_string();
    let key = uuid::Uuid::new_v4().to_string();
    let select = cols
        .split(',')
        .map(|c| match c {
            "id" => format!("'{id}'"),
            "request_key" => format!("'{key}'"),
            "uploaded_at" => "'2026-02-30T12:00:00Z'".into(),
            _ => c.into(),
        })
        .collect::<Vec<String>>()
        .join(",");
    let outcome: fb_core::FbResult<()> = f.fx.book.write(|tx| {
        tx.execute(&format!("INSERT INTO attachments_attachment({cols}) SELECT {select} FROM attachments_attachment WHERE id=?"), [row["id"].as_str().unwrap()])?;
        let user = services::access(tx, f.fx.user, f.co)?;
        services::audit(tx, &user, f.co, "attachment.added", format!("journal:{}",f.owner.key().1), json!({"attachment":id,"name":row["original_name"],"number":row["uploaded_number"],"content_type":row["content_type"],"size":row["size"],"sha256":row["sha256"]}))?;
        tx.execute("INSERT INTO fb_seal VALUES('attachment_added',?)", [&id])?;
        Ok(())
    });
    if outcome.is_ok() {
        assert!(f.fx.book.check().unwrap().is_empty());
        assert!(Book::open(&f.path, &KEY).is_ok());
    }
    assert!(
        outcome.is_err(),
        "impossible upload timestamp committed, passed check, and reopened"
    );
    // Dual variant: (a) the shared value guard refuses it; (b) so does the attachments' own timestamp check.
    assert_eq!(outcome.unwrap_err().constraint(), Some("fb_valid"));
    let sql = format!("INSERT INTO attachments_attachment({cols}) SELECT {select} FROM attachments_attachment WHERE id='{}'", row["id"].as_str().unwrap());
    assert_eq!(without_trigger(&f.fx.book, INSERT_GUARD, &sql).as_deref(), Some("attachment_text"));
    assert!(f.fx.book.check().unwrap().is_empty());
}

#[test]
fn review_removed_timestamp_must_be_refused() {
    let mut f = F::new();
    let row = f.add("one.pdf", PDF);
    let sql = format!(
        "UPDATE attachments_attachment SET deleted=1,deleted_by_id='{}',deleted_by_name='tester',deleted_at='2026-02-30T12:00:00Z',delete_reason='wrong',deleted_number=uploaded_number WHERE id='{}'",
        f.fx.user,
        row["id"].as_str().unwrap()
    );
    // Dual variant: (a) the shared value guard refuses it; (b) so does the attachments' own timestamp check.
    assert_eq!(
        attack(&f.fx.book, &sql).as_deref(),
        Some("fb_valid"),
        "SQL accepted impossible removal timestamp"
    );
    assert_eq!(without_trigger(&f.fx.book, UPDATE_GUARD, &sql).as_deref(), Some("attachment_text"));
    assert!(f.fx.book.check().unwrap().is_empty());
}

#[test]
fn review_concurrent_duplicate_uploads_keep_one_valid_blob() {
    let f = F::new();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let joins = (0..2)
        .map(|_| {
            let path = f.path.clone();
            let owner = f.owner.clone();
            let co = f.co;
            let actor = f.fx.user;
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut book = Book::open(&path, &KEY).unwrap();
                let store = Store::for_book(&path, &KEY).unwrap();
                barrier.wait();
                a::add(
                    &mut book,
                    &store,
                    actor,
                    co,
                    &owner,
                    PDF,
                    "same.pdf",
                    &uuid::Uuid::new_v4().to_string(),
                    &Limits::default(),
                )
                .unwrap()
                .0
            })
        })
        .collect::<Vec<_>>();
    let rows = joins
        .into_iter()
        .map(|j| j.join().unwrap())
        .collect::<Vec<_>>();
    assert_ne!(rows[0]["id"], rows[1]["id"]);
    assert_eq!(rows[0]["stored_name"], rows[1]["stored_name"]);
    assert_eq!(fs::read_dir(f.store.folder()).unwrap().count(), 1);
    let file: Stored = serde_json::from_value(rows[0].clone()).unwrap();
    assert_eq!(f.store.open_read(f.co, &file).unwrap(), PDF);
    assert!(f.fx.book.check().unwrap().is_empty());
}

#[test]
fn review_uuid_checks_must_reject_embedded_nul_suffixes() {
    let mut f = F::new();
    let row = f.add("one.pdf", PDF);
    let cols = columns(f.fx.book.connection());
    let mut accepted = vec![];
    for field in ["id", "request_key"] {
        let select = cols
            .split(',')
            .map(|c| {
                if c == "id" || c == "request_key" {
                    if c == field {
                        format!("'{}'||char(0)||'suffix'", uuid::Uuid::new_v4())
                    } else {
                        format!("'{}'", uuid::Uuid::new_v4())
                    }
                } else {
                    c.into()
                }
            })
            .collect::<Vec<String>>()
            .join(",");
        let sql = format!(
            "INSERT INTO attachments_attachment({cols}) SELECT {select} FROM attachments_attachment WHERE id='{}'",
            row["id"].as_str().unwrap()
        );
        // Dual variant: (a) the shared value guard refuses it; (b) so does the attachments' own UUID check.
        if attack(&f.fx.book, &sql).as_deref() != Some("fb_valid") {
            accepted.push(field);
        }
        if without_trigger(&f.fx.book, INSERT_GUARD, &sql).as_deref() != Some("attachment_text") {
            accepted.push(field);
        }
    }
    assert!(
        accepted.is_empty(),
        "UUID CHECK accepted NUL suffix on {accepted:?}"
    );
    assert!(f.fx.book.check().unwrap().is_empty());
}

#[test]
fn review_nul_request_key_must_not_commit_and_reopen() {
    let mut f = F::new();
    let row = f.add("one.pdf", PDF);
    let cols = columns(f.fx.book.connection());
    let id = uuid::Uuid::new_v4().to_string();
    let select = cols
        .split(',')
        .map(|c| match c {
            "id" => format!("'{id}'"),
            "request_key" => format!("'{}'||char(0)||'suffix'", uuid::Uuid::new_v4()),
            _ => c.into(),
        })
        .collect::<Vec<String>>()
        .join(",");
    let outcome:fb_core::FbResult<()>=f.fx.book.write(|tx|{
        tx.execute(&format!("INSERT INTO attachments_attachment({cols}) SELECT {select} FROM attachments_attachment WHERE id=?"),[row["id"].as_str().unwrap()])?;
        let user=services::access(tx,f.fx.user,f.co)?;
        services::audit(tx,&user,f.co,"attachment.added",format!("journal:{}",f.owner.key().1),json!({"attachment":id,"name":row["original_name"],"number":row["uploaded_number"],"content_type":row["content_type"],"size":row["size"],"sha256":row["sha256"]}))?;
        tx.execute("INSERT INTO fb_seal VALUES('attachment_added',?)",[&id])?;
        Ok(())
    });
    if outcome.is_ok() {
        assert!(f.fx.book.check().unwrap().is_empty());
        assert!(Book::open(&f.path, &KEY).is_ok());
    }
    assert!(
        outcome.is_err(),
        "invalid NUL-suffixed request key committed and reopened"
    );
    // Dual variant: (a) the shared value guard refuses it; (b) so does the attachments' own text check.
    assert_eq!(outcome.unwrap_err().constraint(), Some("fb_valid"));
    let sql = format!("INSERT INTO attachments_attachment({cols}) SELECT {select} FROM attachments_attachment WHERE id='{}'", row["id"].as_str().unwrap());
    assert_eq!(without_trigger(&f.fx.book, INSERT_GUARD, &sql).as_deref(), Some("attachment_text"));
    assert!(f.fx.book.check().unwrap().is_empty());
}

#[test]
fn review_panel_retains_python_combined_cap_and_ordering() {
    let mut f = F::new();
    let limits = Limits {
        uploads_per_window: 1000,
        ..Limits::default()
    };
    let mut removed_ids = vec![];
    for _ in 0..71 {
        let row = a::add(
            &mut f.fx.book,
            &f.store,
            f.fx.user,
            f.co,
            &f.owner,
            PDF,
            "one.pdf",
            &uuid::Uuid::new_v4().to_string(),
            &limits,
        )
        .unwrap()
        .0;
        let id = row["id"].as_str().unwrap().to_owned();
        a::remove(&mut f.fx.book, f.fx.user, f.co, &id, "old file").unwrap();
        removed_ids.push(id);
    }
    for _ in 0..3 {
        a::add(
            &mut f.fx.book,
            &f.store,
            f.fx.user,
            f.co,
            &f.owner,
            PDF,
            "active.pdf",
            &uuid::Uuid::new_v4().to_string(),
            &limits,
        )
        .unwrap();
    }
    let panel =
        f.fx.book
            .read(|c| a::panel_rows(c, f.co, &f.owner))
            .unwrap();
    assert_eq!(panel.len(), 70);
    assert!(panel[..3].iter().all(|r| r["deleted"] == false));
    assert_eq!(panel[3]["id"], removed_ids[0]);
    assert_eq!(panel[69]["id"], removed_ids[66]);
    let removed = f.fx.book.read(|c| a::removed(c, f.co, &f.owner)).unwrap();
    assert_eq!(removed.len(), 71);
    assert_eq!(removed[0]["id"], removed_ids[70]);
    assert_eq!(
        f.fx.book
            .read(|c| a::active(c, f.co, &f.owner))
            .unwrap()
            .len(),
        3
    );
    assert_eq!(fs::read_dir(f.store.folder()).unwrap().count(), 1);
    assert!(f.fx.book.check().unwrap().is_empty());
    assert!(Book::open(&f.path, &KEY).is_ok());
}

fn corrupt_stored_text(f: &F, field: &str, value: &str) {
    let c = f.fx.book.connection();
    let triggers=c.prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger' AND tbl_name='attachments_attachment'").unwrap()
        .query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).unwrap()
        .collect::<Result<Vec<_>,_>>().unwrap();
    c.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA ignore_check_constraints=ON;")
        .unwrap();
    for (name, _) in &triggers {
        c.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
    }
    c.execute(
        &format!("UPDATE attachments_attachment SET {field}=?"),
        [value],
    )
    .unwrap();
    for (_, sql) in &triggers {
        c.execute_batch(sql).unwrap();
    }
    c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA ignore_check_constraints=OFF;")
        .unwrap();
}
#[test]
fn every_attachment_text_column_rejects_nul_on_write_and_open() {
    let f = F::new();
    let fields =
        f.fx.book
            .connection()
            .prepare("PRAGMA table_info(attachments_attachment)")
            .unwrap()
            .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .filter(|(_, t)| t == "TEXT")
            .map(|(n, _)| n)
            .collect::<Vec<_>>();
    assert_eq!(
        fields.len(),
        22,
        "update the migration sweep for new text columns"
    );
    for field in fields {
        let mut f = F::new();
        let row = f.add("one.pdf", PDF);
        let value = format!("{}\0suffix", row[&field].as_str().unwrap_or(""));
        let columns = columns(f.fx.book.connection());
        let select = columns
            .split(',')
            .map(|c| match c {
                "id" | "request_key" if c != field => format!("'{}'", uuid::Uuid::new_v4()),
                c if c == field => "?1".to_owned(),
                _ => c.to_owned(),
            })
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "INSERT INTO attachments_attachment({columns}) SELECT {select} FROM attachments_attachment"
        );
        // Dual variant: (a) the shared value guard refuses the NUL; (b) so does the attachments' own text check.
        let refusal = |e: rusqlite::Error| fb_db::constraint_of(&e);
        assert_eq!(
            f.fx.book.connection().execute(&sql, [&value]).err().and_then(refusal).as_deref(),
            Some("fb_valid"),
            "write accepted {field}"
        );
        let conn = f.fx.book.connection();
        conn.execute_batch(&format!("SAVEPOINT isolate; DROP TRIGGER {INSERT_GUARD}")).unwrap();
        let own = conn.execute(&sql, [&value]).err().and_then(refusal);
        conn.execute_batch("ROLLBACK TO isolate; RELEASE isolate").unwrap();
        assert_eq!(own.as_deref(), Some("attachment_text"), "own text guard missed {field}");
        corrupt_stored_text(&f, &field, &value);
        let problems = f.fx.book.check().unwrap();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("invalid attachment evidence")),
            "open-time text guard missed {field}: {problems:?}"
        );
        assert!(
            Book::open(&f.path, &KEY).is_err(),
            "reopened corrupt {field}"
        );
    }
}
#[test]
fn impossible_stored_timestamps_fail_check_and_open() {
    for field in ["uploaded_at", "deleted_at"] {
        let mut f = F::new();
        f.add("one.pdf", PDF);
        corrupt_stored_text(&f, field, "2026-02-30T12:00:00Z");
        let problems = f.fx.book.check().unwrap();
        assert!(
            problems.iter().any(|p| p.contains("invalid attachment evidence")),
            "{field}"
        );
        // Dual variant for the open-time check: (a) the shared 0130 view flags the row; (b) the attachments'
        // own 0111 view flags it by itself (queried directly, so neither check can stand in for the other).
        assert!(
            problems
                .iter()
                .any(|p| p.starts_with("invalid stored text, identifier, date or timestamp: invalid stored value in attachments_attachment")),
            "{field}: {problems:?}"
        );
        let own: i64 = f
            .fx
            .book
            .connection()
            .query_row("SELECT count(*) FROM attachment_text_invalid", [], |r| r.get(0))
            .unwrap();
        assert_eq!(own, 1, "{field}");
        match Book::open(&f.path, &KEY) {
            Err(fb_db::DbError::Integrity(problems)) => assert!(
                problems.iter().any(|p| p.contains("invalid attachment evidence")),
                "{field}: {problems:?}"
            ),
            other => panic!("{field}: {:?}", other.err()),
        }
    }
}

#[test]
fn populated_0110_attachment_history_upgrades_without_changing_a_cell() {
    let mut f = F::new();
    let row = f.add("one.pdf", PDF);
    a::remove(
        &mut f.fx.book,
        f.fx.user,
        f.co,
        row["id"].as_str().unwrap(),
        "Synthetic correction",
    )
    .unwrap();
    f.add("two.pdf", PDF);
    let path = f._dir.path().join("0110.fbbook");
    let prior = fb_db::test_support::populated_copy(
        f.fx.book.connection(),
        &path,
        &KEY,
        "0110_attachments",
        true,
    )
    .unwrap();
    let upgraded = Book::open(&path, &KEY).unwrap();
    assert_eq!(
        fb_db::test_support::snapshot(upgraded.connection()).unwrap(),
        prior.expected
    );
    assert!(upgraded.check().unwrap().is_empty());
    drop(upgraded);
    assert!(Book::open(&path, &KEY).unwrap().check().unwrap().is_empty());
}
