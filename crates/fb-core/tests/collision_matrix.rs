//! Exhaustive displaced-row / hidden-rowid regression matrix required by GUARD-PATTERNS.
mod common;
use common::*;
use fb_core::{
    services::{self, DraftInput, line},
    users,
};
use fb_db::rusqlite::Connection;

fn snapshot(conn: &Connection, table: &str) -> Vec<Vec<fb_db::rusqlite::types::Value>> {
    let mut stmt = conn
        .prepare(&format!("SELECT * FROM {table} ORDER BY 1,2"))
        .unwrap();
    let width = stmt.column_count();
    stmt.query_map([], |row| (0..width).map(|i| row.get(i)).collect())
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn every_update_unique_key_refuses_displacement_directly_and_through_views() {
    let mut fx = Fx::new("all-update-keys");
    let (c, a, b) = fx.chart();
    let oc = fx.company("Other");
    let spare_user = users::create_user(&mut fx.book, "spare", false).unwrap();
    users::add_member(&mut fx.book, c, spare_user).unwrap();
    let spare = fx.account(c, "spare", "Spare", "INCOME");
    let p = fx.draft(c, a, b, "10", "2026-01-01");
    fx.post(c, p.id);
    let d = fx.draft(c, a, b, "20", "2026-01-01");
    let retry = services::save_draft(
        &mut fx.book,
        fx.user,
        c,
        DraftInput {
            request_key: Some("00000000-0000-4000-8000-000000000001".into()),
            ..DraftInput::new(
                "2026-01-01",
                "Retry",
                vec![line(a).debit("1").build(), line(b).credit("1").build()],
            )
        },
    )
    .unwrap();
    let rev =
        services::reverse_journal(&mut fx.book, fx.user, c, p.id, "2026-01-01", "Void").unwrap();
    fx.book.connection().execute_batch("INSERT INTO fb_seal VALUES('matrix','victim'),('matrix','source'); INSERT INTO fb_seal_due SELECT * FROM fb_seal").unwrap();
    let audit_ids: Vec<String> = fx
        .book
        .connection()
        .prepare("SELECT id FROM ledger_auditevent ORDER BY id LIMIT 2")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let lines: Vec<String> = fx
        .book
        .connection()
        .prepare(&format!(
            "SELECT id FROM ledger_journalline WHERE journal_id='{}' ORDER BY position",
            d.id
        ))
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let cases = vec![
        (
            "fb_user",
            format!("id='{spare_user}'"),
            format!("id='{}'", fx.user),
        ),
        (
            "fb_user",
            format!("id='{spare_user}'"),
            "username='tester'".into(),
        ),
        ("ledger_company", format!("id='{oc}'"), format!("id='{c}'")),
        (
            "ledger_account",
            format!("id='{spare}'"),
            format!("id='{a}'"),
        ),
        (
            "ledger_account",
            format!("id='{spare}'"),
            "code='1000'".into(),
        ),
        (
            "ledger_account",
            format!("id='{spare}'"),
            format!("id='{a}',code='1000',kind='INCOME',detail_type='INCOME'"),
        ),
        (
            "ledger_auditevent",
            format!("id='{}'", audit_ids[1]),
            format!("id='{}'", audit_ids[0]),
        ),
        (
            "ledger_journal",
            format!("id='{}'", d.id),
            format!("id='{}'", p.id),
        ),
        (
            "ledger_journal",
            format!("id='{}'", d.id),
            format!("number={}", p.number),
        ),
        (
            "ledger_journal",
            format!("id='{}'", d.id),
            format!("entry_number='{}'", p.entry_number),
        ),
        (
            "ledger_journal",
            format!("id='{}'", d.id),
            format!(
                "request_key='{}',creation_digest='{}'",
                retry.request_key.unwrap(),
                retry.creation_digest
            ),
        ),
        (
            "ledger_journal",
            format!("id='{}'", d.id),
            format!(
                "entry_number='',reversal_of_id='{}'",
                rev.reversal_of_id.unwrap()
            ),
        ),
        (
            "ledger_journalline",
            format!("id='{}'", lines[1]),
            format!("id='{}'", lines[0]),
        ),
        (
            "ledger_journalline",
            format!("id='{}'", lines[1]),
            "position=1".into(),
        ),
        (
            "ledger_company_members",
            format!("company_id='{c}' AND user_id='{spare_user}'"),
            format!("user_id='{}'", fx.user),
        ),
        (
            "fb_seal",
            "kind='matrix' AND doc_id='source'".into(),
            "doc_id='victim'".into(),
        ),
        (
            "fb_seal_due",
            "kind='matrix' AND doc_id='source'".into(),
            "doc_id='victim'".into(),
        ),
    ];
    let tables = [
        "fb_user",
        "ledger_company",
        "ledger_account",
        "ledger_auditevent",
        "ledger_journal",
        "ledger_journalline",
        "ledger_company_members",
        "fb_seal",
        "fb_seal_due",
    ];
    for table in tables {
        let cols: Vec<String> = fx
            .book
            .connection()
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |r| r.get(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let sets = cols
            .iter()
            .map(|c| format!("{c}=NEW.{c}"))
            .collect::<Vec<_>>()
            .join(",");
        let pk = if table == "ledger_company_members" {
            "company_id=OLD.company_id AND user_id=OLD.user_id"
        } else if table.starts_with("fb_seal") {
            "kind=OLD.kind AND doc_id=OLD.doc_id"
        } else {
            "id=OLD.id"
        };
        fx.book.connection().execute_batch(&format!("CREATE TEMP VIEW v_{table} AS SELECT * FROM main.{table}; CREATE TEMP TRIGGER v_{table}_update INSTEAD OF UPDATE ON v_{table} BEGIN UPDATE {table} SET {sets} WHERE {pk}; END")).unwrap();
    }
    for recursive in [true, false] {
        fx.book
            .connection()
            .execute_batch(&format!(
                "PRAGMA recursive_triggers={}",
                i32::from(recursive)
            ))
            .unwrap();
        for (table, predicate, set) in &cases {
            for via_view in [false, true] {
                let target = if via_view {
                    format!("v_{table}")
                } else {
                    table.to_string()
                };
                for op in ["UPDATE", "UPDATE OR REPLACE"] {
                    let before = snapshot(fx.book.connection(), table);
                    let sql = format!("{op} {target} SET {set} WHERE {predicate}");
                    let error = fx.book.connection().execute_batch(&sql).expect_err(&sql);
                    assert!(fb_db::is_constraint(&error), "{sql}: {error}");
                    assert_eq!(snapshot(fx.book.connection(), table), before, "{sql}");
                }
            }
        }
    }
    fx.book
        .connection()
        .execute_batch("PRAGMA recursive_triggers=ON")
        .unwrap();
    assert!(fx.book.check().unwrap().is_empty());
}

#[test]
fn every_protected_table_has_no_hidden_rowid_alias_to_replace() {
    let mut fx = Fx::new("all-hidden-rowids");
    let (c, a, b) = fx.chart();
    let j = fx.draft(c, a, b, "10", "2026-01-01");
    fx.post(c, j.id);
    fx.book.connection().execute_batch("INSERT INTO fb_seal VALUES('matrix','x'); INSERT INTO fb_seal_due VALUES('matrix','x')").unwrap();
    for table in [
        "fb_user",
        "ledger_company",
        "ledger_account",
        "ledger_auditevent",
        "ledger_journal",
        "ledger_journalline",
        "ledger_company_members",
        "fb_seal",
        "fb_seal_due",
    ] {
        let without: i64 = fx
            .book
            .connection()
            .query_row(
                "SELECT wr FROM pragma_table_list WHERE schema='main' AND name=?1",
                [table],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(without, 1, "{table}");
        for recursive in [true, false] {
            fx.book
                .connection()
                .execute_batch(&format!(
                    "PRAGMA recursive_triggers={}",
                    i32::from(recursive)
                ))
                .unwrap();
            for alias in ["rowid", "_rowid_", "oid"] {
                let before = snapshot(fx.book.connection(), table);
                let columns: Vec<String> = fx
                    .book
                    .connection()
                    .prepare(&format!("PRAGMA table_info({table})"))
                    .unwrap()
                    .query_map([], |r| r.get(1))
                    .unwrap()
                    .collect::<Result<_, _>>()
                    .unwrap();
                let names = columns.join(",");
                let values = columns
                    .iter()
                    .map(|col| {
                        if col == "id" {
                            format!("'{}'", fb_core::UserId::new())
                        } else {
                            col.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                let sql = format!(
                    "INSERT OR REPLACE INTO {table}({alias},{names}) SELECT 1,{values} FROM {table} LIMIT 1"
                );
                let error = fx.book.connection().execute_batch(&sql).expect_err(&sql);
                assert!(
                    error.to_string().contains("no column named"),
                    "{sql}: {error}"
                );
                assert_eq!(snapshot(fx.book.connection(), table), before);
                assert!(
                    fx.book
                        .connection()
                        .prepare(&format!("UPDATE OR REPLACE {table} SET {alias}=1"))
                        .is_err()
                );
            }
        }
    }
}
