//! Migration 0208 (book format 95) adds scoped unattended AI approval records. A book at the
//! installed format 93 takes the bank-feed migration 0207 on the way and starts with none.
use super::*;
use rusqlite::params;
use std::collections::BTreeSet;

pub(crate) const AI_UNATTENDED_TABLES: usize = 1;
pub(crate) const AI_UNATTENDED_OBJECTS: &[&str] = &[
    "table:desktop_ai_unattended",
    "index:desktop_ai_unattended_actor",
    "trigger:fb_valid_desktop_ai_unattended_insert",
    "trigger:fb_valid_desktop_ai_unattended_update",
    "trigger:desktop_ai_unattended_identity",
    "trigger:desktop_ai_unattended_role",
    "trigger:desktop_ai_unattended_active",
    "trigger:desktop_ai_unattended_membership",
    "trigger:desktop_ai_unattended_mfa_enable",
    "trigger:desktop_ai_unattended_mfa_replace",
    "trigger:desktop_ai_unattended_mfa_disable",
];

const CO: &str = "00000000-0000-7000-8000-0000000004a1";
const ACTOR: &str = "00000000-0000-7000-8000-0000000004a2";
const ID: &str = "00000000-0000-7000-8000-0000000004a3";
const ANCHOR: &str = "00000000-0000-7000-8000-0000000004a4";

#[test]
fn format_93_upgrades_with_no_approvals_and_preserves_the_old_book() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("format-93.fbbook");
    let key = [93; 32];
    // The installed format is 93. Two migrations follow it, and this one is the last.
    let boundary = 93;
    assert_eq!(MIGRATIONS[boundary - 1].0, "0206_remote_host");
    assert_eq!(MIGRATIONS[boundary].0, "0207_bankfeed_id_provenance");
    assert_eq!(MIGRATIONS[boundary + 1].0, "0208_ai_unattended");
    assert_eq!(MIGRATIONS[boundary + 2].0, "0209_deposit_adjustments");
    assert_eq!(MIGRATIONS[boundary + 3].0, "0210_bankfeeds_matching");
    assert_eq!(MIGRATIONS[boundary + 4].0, "0211_company_and_bankfeed_defaults");
    assert_eq!(MIGRATIONS.len(), boundary + 5);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(&format!("PRAGMA key=\"x'{}'\"", hex(&key)))
        .unwrap();
    Book::configure(&conn).unwrap();
    for (_, sql) in &MIGRATIONS[..boundary] {
        conn.execute_batch(sql).unwrap();
    }
    conn.execute_batch("PRAGMA user_version=93").unwrap();
    conn.execute(
        "INSERT INTO fb_user(id,username) VALUES(?1,'Synthetic approving administrator')",
        [ACTOR],
    )
    .unwrap();
    conn.execute("INSERT INTO ledger_company(id,name,currency) VALUES(?1,'Synthetic approved company','USD')", [CO]).unwrap();
    conn.execute(
        "INSERT INTO ledger_company_members(company_id,user_id) VALUES(?1,?2)",
        params![CO, ACTOR],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO desktop_user(user_id,role) VALUES(?1,'administrator')",
        [ACTOR],
    )
    .unwrap();
    let before = test_support::snapshot(&conn).unwrap();
    let schema = schema_hashes(&conn).unwrap();
    let view = |c: &Connection| -> String {
        c.query_row(
            "SELECT sql FROM sqlite_schema WHERE name='fb_value_invalid'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    let prior_view = view(&conn);
    drop(conn);

    let old = match Book::open_staged(&path, &key).unwrap() {
        Opened::Behind(old) => old,
        Opened::Current(_) => panic!("expected a format-93 book"),
    };
    assert_eq!((old.from(), old.to()), (93, 98));
    let backup = dir.path().join("verified-format-93.fbbook");
    old.snapshot(&backup, &key).unwrap();
    assert_eq!(test_support::snapshot_file(&backup, &key).unwrap(), before);
    let book = old.upgrade().unwrap();
    let after = test_support::snapshot(&book.conn).unwrap();
    assert_eq!(after.len(), before.len() + AI_UNATTENDED_TABLES + crate::bankfeeds_matching_upgrade_tests::TABLES);
    let mut before = before;
    test_support::company_period_defaults(&mut before, &book.conn);
    for (table, rows) in &before {
        assert_eq!(&after[table], rows, "changed {table}");
    }
    assert!(after["desktop_ai_unattended"].is_empty());
    let upgraded = schema_hashes(&book.conn).unwrap();
    for (object, hash) in &schema {
        if crate::company_periods_upgrade_tests::CHANGED.contains(&object.as_str()) {
            assert_ne!(&upgraded[object], hash, "0211 extends {object}");
            continue;
        }
        if ["table:bankfeeds_account", "table:bankfeeds_transaction"].contains(&object.as_str()) {
            assert_ne!(&upgraded[object], hash, "0207 must extend {object}");
        } else if object != "view:fb_value_invalid"
            && !crate::deposit_adjustments_upgrade_tests::CHANGED.contains(&object.as_str())
            && !crate::bankfeeds_matching_upgrade_tests::CHANGED.contains(&object.as_str())
        {
            assert_eq!(&upgraded[object], hash, "changed {object}");
        }
    }
    let added: BTreeSet<&str> = upgraded
        .keys()
        .filter(|k| !schema.contains_key(*k))
        .map(String::as_str)
        .filter(|k| !crate::deposit_adjustments_upgrade_tests::OBJECTS.contains(k) && !crate::bankfeeds_matching_upgrade_tests::OBJECTS.contains(k))
        .collect();
    assert_eq!(added, AI_UNATTENDED_OBJECTS.iter().chain(crate::company_periods_upgrade_tests::OBJECTS).copied().collect());
    let next_view = view(&book.conn);
    let (kept, _) = next_view
        .split_once("\nUNION ALL\nSELECT 'invalid stored value in desktop_ai_unattended'")
        .unwrap();
    assert_eq!(kept, prior_view.trim_end_matches(';'));
    drop(book);

    let book = Book::open(&path, &key).unwrap();
    assert!(book.check().unwrap().is_empty());
    assert_eq!(test_support::snapshot(&book.conn).unwrap(), after);
    book.conn
        .execute(
            "INSERT INTO desktop_ai_unattended VALUES(?1,?2,?3,?4,zeroblob(32))",
            params![ID, ACTOR, CO, ANCHOR],
        )
        .unwrap();
    assert!(
        book.conn
            .execute("UPDATE desktop_ai_unattended SET anchor_id='malformed'", [])
            .is_err()
    );
    assert!(
        book.conn
            .execute("UPDATE desktop_ai_unattended SET id=?1", [ANCHOR])
            .is_err()
    );
    assert!(
        book.conn
            .execute("UPDATE desktop_ai_unattended SET verifier=zeroblob(31)", [])
            .is_err()
    );
    book.conn
        .execute(
            "DELETE FROM ledger_company_members WHERE user_id=?1 AND company_id=?2",
            params![ACTOR, CO],
        )
        .unwrap();
    assert!(test_support::snapshot(&book.conn).unwrap()["desktop_ai_unattended"].is_empty());
}
