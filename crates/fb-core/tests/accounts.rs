mod common;
use common::*;
use fb_core::{
    Book,
    accounts::{self, AppLock, LockPolicy, UnlockOutcome},
    users,
};

#[test]
fn original_owner_remains_an_audit_actor_and_named_users_can_be_added() {
    let mut book = book("owner");
    assert!(accounts::create_owner(&mut book, "\n").is_err());
    assert!(accounts::create_owner(&mut book, &"x".repeat(151)).is_err());
    assert!(accounts::create_owner(&mut book, "Joe\u{0000}Owner").is_err());
    let id = accounts::create_owner(&mut book, "  Synthetic Owner  ").unwrap();
    assert_eq!(book.read(|q| accounts::owner(q)).unwrap(), Some(id));
    let co =
        fb_core::services::create_company(&mut book, id, "Owner Company", "USD", false).unwrap();
    let name: String = book
        .connection()
        .query_row(
            "SELECT actor_name FROM ledger_auditevent WHERE company_id=?1",
            [co.id.to_db()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(name, "Synthetic Owner");
    assert!(accounts::create_owner(&mut book, "Another").is_err());
    assert!(users::create_user(&mut book, "Another", false).is_ok());
    // An unconfigured legacy owner can be disabled; enrolled administrators have the 0185 guards.
    assert!(users::set_active(&mut book, id, false).is_ok());
    assert_eq!(book.read(|q| accounts::owner(q)).unwrap(), Some(id));
    assert!(book.check().unwrap().is_empty());
}

#[test]
fn owner_identity_matrix_and_parent_protection() {
    let mut book = book("owner-matrix");
    let id = accounts::create_owner(&mut book, "Owner").unwrap();
    let other = fb_core::UserId::new();
    let before = fb_db::test_support::snapshot(book.connection()).unwrap();
    for recursive in ["ON", "OFF"] {
        book.connection()
            .execute_batch(&format!("PRAGMA recursive_triggers={recursive}"))
            .unwrap();
        for sql in [
            "DELETE FROM accounts_owner".into(),
            "UPDATE accounts_owner SET singleton=2".into(),
            format!("UPDATE accounts_owner SET user_id='{other}'"),
            format!("INSERT OR REPLACE INTO accounts_owner VALUES(1,'{id}')"),
            format!("UPDATE OR REPLACE accounts_owner SET user_id='{other}'"),
            format!("DELETE FROM fb_user WHERE id='{id}'"),
            format!("UPDATE OR REPLACE fb_user SET id='{other}' WHERE id='{id}'"),
            format!("INSERT OR REPLACE INTO fb_user(id,username) VALUES('{other}','Owner')"),
            format!(
                "INSERT INTO accounts_owner VALUES(1,'{id}') ON CONFLICT DO UPDATE SET user_id=excluded.user_id"
            ),
            "INSERT OR REPLACE INTO accounts_owner(rowid,singleton,user_id) VALUES(1,1,'x')".into(),
        ] {
            assert!(attack_message(&book, &sql).is_some(), "{sql}");
            assert_eq!(
                fb_db::test_support::snapshot(book.connection()).unwrap(),
                before
            );
        }
    }
    book.connection()
        .execute_batch("PRAGMA recursive_triggers=ON")
        .unwrap();
    assert!(book.check().unwrap().is_empty());
}

#[test]
fn owner_guard_messages_missing_parent_and_legacy_refusal() {
    let mut book = book("owner-empty");
    assert!(
        attack_message(
            &book,
            "INSERT INTO accounts_owner VALUES(1,'00000000-0000-0000-0000-000000000000')"
        )
        .unwrap()
        .contains("One active staff")
    );
    users::create_user(&mut book, "Legacy", false).unwrap();
    assert!(accounts::create_owner(&mut book, "Owner").is_err());
    assert!(
        attack_message(&book, "INSERT INTO accounts_owner SELECT 1,id FROM fb_user")
            .unwrap()
            .contains("One active staff")
    );
    assert!(book.check().unwrap().is_empty());
}

#[test]
fn create_owner_atomic_even_if_outer_catches_failure() {
    let mut book = book("owner-atomic");
    // Isolate a controlled late failure in the second insert.
    book.connection().execute_batch("CREATE TRIGGER fail_owner BEFORE INSERT ON accounts_owner BEGIN SELECT RAISE(ABORT,'injected late failure');END").unwrap();
    book.write(|tx| -> fb_core::FbResult<()> {
        assert!(accounts::create_owner_tx(tx, "Owner").is_err());
        Ok(())
    })
    .unwrap();
    assert_eq!(
        book.connection()
            .query_row("SELECT count(*) FROM fb_user", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    book.connection()
        .execute_batch("DROP TRIGGER fail_owner")
        .unwrap();
    accounts::create_owner(&mut book, "Owner").unwrap();
    assert!(book.check().unwrap().is_empty());
}

#[test]
fn every_owner_trigger_is_required_and_hash_checked() {
    let mut book = book("owner-schema");
    accounts::create_owner(&mut book, "Owner").unwrap();
    for name in [
        "fb_owner_insert",
        "fb_owner_update",
        "fb_owner_delete",
        "fb_owner_user_insert",
        "fb_owner_user_update",
        "fb_owner_user_delete",
    ] {
        book.connection().execute_batch("SAVEPOINT test").unwrap();
        book.connection()
            .execute_batch(&format!("DROP TRIGGER {name}"))
            .unwrap();
        assert!(!book.check().unwrap().is_empty());
        book.connection()
            .execute_batch(&format!(
                "CREATE TRIGGER {name} BEFORE INSERT ON accounts_owner BEGIN SELECT 1;END"
            ))
            .unwrap();
        assert!(!book.check().unwrap().is_empty());
        book.connection()
            .execute_batch("ROLLBACK TO test;RELEASE test")
            .unwrap();
    }
    assert!(book.check().unwrap().is_empty());
}

#[test]
fn populated_base_50c3f74_upgrades_without_losing_any_row_or_trigger() {
    let mut fx = Fx::new("owner-upgrade");
    let (co, a, e) = fx.chart();
    let j = fx.draft(co, a, e, "10.10", "2026-10-01");
    fx.post(co, j.id);
    let _reverse = fb_core::services::reverse_journal(
        &mut fx.book,
        fx.user,
        co,
        j.id,
        "2026-10-02",
        "Synthetic reversal",
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.fbbook");
    let prior = fb_db::test_support::populated_copy(
        fx.book.connection(),
        &path,
        &KEY,
        "0078_shared_unique_json",
        true,
    )
    .unwrap();
    assert!(!prior.before.contains_key("accounts_owner"));
    let book = Book::open(&path, &KEY).unwrap();
    assert!(book.check().unwrap().is_empty());
    assert_eq!(
        fb_db::test_support::snapshot(book.connection()).unwrap(),
        prior.expected
    );
    drop(book);
    assert!(Book::open(&path, &KEY).unwrap().check().unwrap().is_empty());
}

#[test]
fn lock_threshold_expiry_success_reset_idle_and_sleep() {
    let pass = "synthetic lock passphrase";
    let policy = LockPolicy {
        max_failures: 3,
        lockout_seconds: 10,
        idle_seconds: 20,
    };
    let mut lock = AppLock::new(KEY, Some(pass), true, policy, 100).unwrap();
    lock.on_sleep();
    assert!(lock.is_locked());
    assert!(lock.with_key(|k| k[0]).is_err());
    assert_eq!(lock.unlock("wrong", 100), UnlockOutcome::Failed);
    assert_eq!(lock.unlock("wrong", 101), UnlockOutcome::Failed);
    assert_eq!(lock.unlock("wrong", 102), UnlockOutcome::Throttled);
    assert_eq!(lock.unlock(pass, 111), UnlockOutcome::Throttled);
    assert_eq!(lock.unlock(pass, 112), UnlockOutcome::Unlocked);
    assert_eq!(lock.with_key(|k| *k).unwrap(), KEY);
    lock.activity(120);
    lock.idle_tick(139);
    assert!(!lock.is_locked());
    lock.idle_tick(140);
    assert!(lock.is_locked());
    assert_eq!(lock.unlock(pass, 140), UnlockOutcome::Unlocked);
    lock.on_sleep();
    assert!(lock.is_locked());
    assert!(lock.unlock_os(KEY, 141).is_err());
    let wrapped = lock.wrapped_key().unwrap().clone();
    let mut restarted = AppLock::from_wrapped(wrapped, true, policy, 200).unwrap();
    assert!(restarted.is_locked());
    assert_eq!(restarted.unlock(pass, 200), UnlockOutcome::Unlocked);
}

#[test]
fn password_required_on_basic_text_os_store_and_policy_validation() {
    assert!(AppLock::new(KEY, None, true, LockPolicy::default(), 0).is_err());
    assert!(AppLock::new(KEY, Some("short"), true, LockPolicy::default(), 0).is_err());
    assert!(
        AppLock::new(
            KEY,
            None,
            false,
            LockPolicy {
                max_failures: 0,
                ..LockPolicy::default()
            },
            0
        )
        .is_err()
    );
    let mut lock = AppLock::new(KEY, None, false, LockPolicy::default(), 0).unwrap();
    lock.lock();
    lock.unlock_os(KEY, 1).unwrap();
    assert!(!lock.is_locked());
}

#[test]
fn backward_clock_does_not_shorten_lockout_and_window_boundary_resets_failures() {
    let policy = LockPolicy {
        max_failures: 2,
        lockout_seconds: 10,
        idle_seconds: 20,
    };
    let mut lock = AppLock::new(KEY, Some("synthetic passphrase"), false, policy, 100).unwrap();
    lock.lock();
    assert_eq!(lock.unlock("wrong", 100), UnlockOutcome::Failed);
    assert_eq!(lock.unlock("wrong", 110), UnlockOutcome::Failed); // <= now-window resets, exactly Python
    assert_eq!(lock.unlock("wrong", 111), UnlockOutcome::Throttled);
    assert_eq!(
        lock.unlock("synthetic passphrase", 90),
        UnlockOutcome::Throttled
    );
    assert_eq!(
        lock.unlock("synthetic passphrase", 121),
        UnlockOutcome::Unlocked
    );
}
