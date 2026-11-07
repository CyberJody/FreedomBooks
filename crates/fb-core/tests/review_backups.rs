// Reviewer tests (R-L4-backups). Temporary copy only; never committed.
mod common;
use common::*;
use fb_core::{
    accounts,
    backups::{self, BackupOptions, RestoreOptions, retention},
    users,
};
use std::fs;

fn opts<'a>(
    out: &'a std::path::Path,
    work: &'a std::path::Path,
    label: &'a str,
    at: &'a str,
    keep: Option<usize>,
) -> BackupOptions<'a> {
    BackupOptions {
        output: out,
        work_root: work,
        receipts_root: None,
        verify_receipt: None,
        label,
        app_version: "review",
        created_at: at,
        passphrase: None,
        keep,
    }
}

/// Python prunes with safe_db_label(name). Rust prunes with the raw label.
#[test]
fn review_prune_with_unsafe_label_removes_older_backups() {
    for label in ["plain_label", "a b"] {
        let book = book(&format!("rv-prune-{}", label.len()));
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("backups");
        let work = tmp.path().join("private");
        fs::create_dir(&out).unwrap();
        let t1 = "2026-10-30T12:00:00Z";
        let t2 = "2026-10-30T13:00:00Z";
        let first = out.join(retention::filename(label, t1).unwrap());
        let second = out.join(retention::filename(label, t2).unwrap());
        backups::create_backup(&book, &[42; 32], &opts(&first, &work, label, t1, None)).unwrap();
        let r = backups::create_backup(&book, &[42; 32], &opts(&second, &work, label, t2, Some(1)))
            .unwrap();
        eprintln!("label {label:?}: pruned {:?}", r.pruned);
        assert_eq!(
            r.pruned,
            vec![first.clone()],
            "label {label:?} did not prune"
        );
        assert!(!first.exists());
    }
}

/// Service refuses an owner when a legacy operator exists; the SQL guard must agree.
#[test]
fn review_owner_sql_eligibility_matches_service_with_one_legacy_staff_operator() {
    let mut book = book("rv-owner-legacy");
    let legacy = users::create_user(&mut book, "Legacy", true).unwrap();
    assert!(accounts::create_owner(&mut book, "Owner").is_err());
    let sql = format!("INSERT INTO accounts_owner VALUES(1,'{}')", legacy.to_db());
    book.write(|tx| -> fb_core::FbResult<()> {
        assert!(
            tx.execute_batch(&sql).is_err(),
            "writer callback adopted legacy operator"
        );
        Ok(())
    })
    .unwrap();
    assert!(
        attack_message(&book, &sql).is_some(),
        "raw SQL selected a legacy operator as owner that the service refuses"
    );
}

/// Restore of a snapshot must leave a self-contained target (no WAL needed).
#[test]
fn review_restore_target_is_self_contained() {
    let mut fx = Fx::new("rv-restore-wal");
    let (co, cash, equity) = fx.chart();
    let j = fx.draft(co, cash, equity, "12.34", "2026-01-02");
    fx.post(co, j.id);
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir(tmp.path().join("o")).unwrap();
    let out = tmp.path().join("o").join("b.fbbackup");
    let work = tmp.path().join("private");
    backups::create_backup(
        &fx.book,
        &[42; 32],
        &opts(&out, &work, "x", "2026-10-30T12:00:00Z", None),
    )
    .unwrap();
    let target = tmp.path().join("restored.fbbook");
    let _ = backups::restore_backup(
        &out,
        &[42; 32],
        &RestoreOptions {
            verify_receipt: None,
            target: &target,
            receipts_target: None,
            work_root: &work,
            protected: &[],
            passphrase: None,
            require_encrypted: false,
        },
    )
    .unwrap();
    for s in ["-wal", "-shm"] {
        let mut p = target.as_os_str().to_owned();
        p.push(s);
        assert!(!std::path::PathBuf::from(p).exists(), "restore left {s}");
    }
}

#[test]
fn review_empty_label_is_refused_before_backup_publication() {
    let book = book("rv-empty-label");
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("out")).unwrap();
    let out = dir.path().join("out/empty.fbbackup");
    let work = dir.path().join("private");
    assert!(
        backups::create_backup(
            &book,
            &KEY,
            &opts(&out, &work, "", "2026-10-30T12:00:00Z", None)
        )
        .is_err()
    );
    assert!(!out.exists());
    assert!(retention::filename("", "2026-10-30T12:00:00Z").is_err());
}

#[test]
fn review_verify_note_has_restored_summary_fields() {
    let mut fx = Fx::new("rv-verify-note");
    fx.chart();
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("out")).unwrap();
    let out = dir.path().join("out/note.fbbackup");
    let work = dir.path().join("private");
    backups::create_backup(
        &fx.book,
        &KEY,
        &opts(&out, &work, "note", "2026-10-30T12:00:00Z", None),
    )
    .unwrap();
    backups::verify_backup(&out, &KEY, &work, None, true).unwrap();
    let note: serde_json::Value =
        serde_json::from_slice(&fs::read(retention::sidecar(&out)).unwrap()).unwrap();
    for name in [
        "encrypted",
        "companies",
        "tables",
        "needs_migrate",
        "app_version",
        "lifecycle",
        "attachments",
        "attachment_bytes",
    ] {
        assert!(note.get(name).is_some(), "missing verify-note field {name}");
    }
    assert_eq!(note["encrypted"], false);
    assert_eq!(note["companies"], 1);
    assert_eq!(note["needs_migrate"], false);
    assert_eq!(note["app_version"], "review");
    assert_eq!(note["lifecycle"]["run"], false);
    assert_eq!(note["attachments"], 0);
    assert_eq!(note["attachment_bytes"], 0);
    assert!(retention::verified_note(&out));
}

#[test]
fn review_money_check_runs_read_only_on_staged_book_and_failure_leaves_no_note() {
    use fb_core::backups::{BackupError, VerifyOptions};
    use std::cell::Cell;
    let mut fx = Fx::new("rv-money-check");
    fx.chart();
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("out")).unwrap();
    let out = dir.path().join("out/money.fbbackup");
    let work = dir.path().join("private");
    backups::create_backup(
        &fx.book,
        &KEY,
        &opts(&out, &work, "money", "2026-10-30T12:00:00Z", None),
    )
    .unwrap();
    // Live changes must not appear in the private restored verification copy.
    fx.company("Live only");
    let calls = Cell::new(0);
    let failing =
        |q: &fb_db::Reader<'_>, _: &backups::Manifest| -> backups::Result<serde_json::Value> {
            calls.set(calls.get() + 1);
            let n: i64 = q.query_row("SELECT count(*) FROM ledger_company", [], |r| r.get(0))?;
            assert_eq!(n, 1);
            assert!(q.prepare("DELETE FROM fb_user").is_err());
            Err(BackupError::refused("independent lifecycle mismatch"))
        };
    let mut options = VerifyOptions {
        work_root: &work,
        passphrase: None,
        record: true,
        verify_receipt: None,
        money_check: Some(&failing),
    };
    let result = backups::verify_backup_with_options(&out, &KEY, &options);
    assert!(result.is_err(), "money check failure was ignored");
    assert_eq!(calls.get(), 1);
    assert!(!retention::sidecar(&out).exists());
    let good = |_: &fb_db::Reader<'_>, _: &backups::Manifest| {
        Ok(serde_json::json!({"run":true,"verifier":"test lifecycle","companies":1}))
    };
    options.money_check = Some(&good);
    backups::verify_backup_with_options(&out, &KEY, &options).unwrap();
    let note: serde_json::Value =
        serde_json::from_slice(&fs::read(retention::sidecar(&out)).unwrap()).unwrap();
    assert_eq!(note["lifecycle"]["run"], true);
    assert_eq!(note["lifecycle"]["verifier"], "test lifecycle");
    assert!(retention::verified_note(&out));
    assert!(fs::read_dir(&work).unwrap().next().is_none());
}

#[test]
fn review_owner_candidate_with_audit_or_membership_is_refused_and_scope_resets() {
    for kind in ["audit", "membership"] {
        let mut book = book(kind);
        let user = fb_core::UserId::new().to_db();
        let co = fb_core::CompanyId::new().to_db();
        book.write(|tx| -> fb_core::FbResult<()> {
            tx.execute("INSERT INTO fb_user(id,username,is_staff) VALUES(?1,'Candidate',1)", [&user])?;
            tx.execute("INSERT INTO ledger_company(id,name) VALUES(?1,'Synthetic')", [&co])?;
            if kind == "membership" {
                tx.execute("INSERT INTO ledger_company_members VALUES(?1,?2)", [&co,&user])?;
            } else {
                tx.execute("INSERT INTO ledger_auditevent(id,company_id,actor_id,actor_name,occurred_at,action,target) VALUES(?1,?2,?3,'Candidate','2026-10-30T12:00:00Z','synthetic','synthetic')", [fb_ids::new_v7().to_string(), co.clone(), user.clone()])?;
            }
            assert!(tx.execute("INSERT INTO accounts_owner VALUES(1,?1)", [&user]).is_err());
            Err(fb_core::FbError::validation("rollback fixture"))
        }).unwrap_err();
        // A failed command must not leak the empty-book privilege to raw SQL.
        book.connection()
            .execute(
                "INSERT INTO fb_user(id,username,is_staff) VALUES(?1,'Outside command',1)",
                [&user],
            )
            .unwrap();
        assert!(
            attack_message(
                &book,
                &format!("INSERT INTO accounts_owner VALUES(1,'{user}')")
            )
            .is_some()
        );
        book.connection()
            .execute("DELETE FROM fb_user WHERE id=?1", [&user])
            .unwrap();
        accounts::create_owner(&mut book, "Real first-run owner").unwrap();
    }
}

#[test]
fn review_existing_0120_owner_survives_eligibility_upgrade() {
    let mut current = book("rv-owner-0120");
    let owner = accounts::create_owner(&mut current, "Existing owner").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("0120.fbbook");
    let prior =
        fb_db::test_support::populated_copy(current.connection(), &old, &KEY, "0120_owner", true)
            .unwrap();
    let mut upgraded = fb_core::Book::open(&old, &KEY).unwrap();
    assert_eq!(
        fb_db::test_support::snapshot(upgraded.connection()).unwrap(),
        prior.expected
    );
    assert_eq!(upgraded.read(|q| accounts::owner(q)).unwrap(), Some(owner));
    assert!(upgraded.check().unwrap().is_empty());
}

#[test]
fn review_raw_manifest_excludes_corrupt_cross_company_line() {
    let mut fx = Fx::new("rv-cross-manifest");
    let (a, cash, equity) = fx.chart();
    let b = fx.company("Other company");
    let foreign = fx.account(b, "1000", "Foreign cash", "ASSET");
    let journal = fx.draft(a, cash, equity, "23.45", "2026-10-01");
    fx.post(a, journal.id);
    let q = fx.book.connection();
    q.execute_batch("SAVEPOINT corrupt_fixture").unwrap();
    let triggers: Vec<String> = q
        .prepare(
            "SELECT name FROM sqlite_master WHERE type='trigger' AND tbl_name='ledger_journalline'",
        )
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for trigger in triggers {
        q.execute_batch(&format!("DROP TRIGGER {trigger}")).unwrap();
    }
    q.execute(
        "UPDATE ledger_journalline SET account_id=?1 WHERE journal_id=?2 AND account_id=?3",
        [foreign.to_db(), journal.id.to_db(), cash.to_db()],
    )
    .unwrap();
    let manifest = backups::manifest::contents(q).unwrap();
    let other = manifest
        .companies
        .iter()
        .find(|c| c.id == b.to_db())
        .unwrap();
    assert_eq!(other.accounts[0].posted_debits, "0.00");
    assert_eq!(other.accounts[0].posted_credits, "0.00");
    q.execute_batch("ROLLBACK TO corrupt_fixture;RELEASE corrupt_fixture")
        .unwrap();
    assert!(fx.book.check().unwrap().is_empty());
}
