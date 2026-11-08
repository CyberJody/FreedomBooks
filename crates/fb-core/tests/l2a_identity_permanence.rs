//! Permanent keys and ownership, including unreferenced rows and mutable link sets.
mod common;
use common::*;
use fb_core::UserId;
use fb_db::rusqlite::{self, Connection, types::Value};
use std::collections::{BTreeMap, BTreeSet};

// Fixture ids: canonical UUIDv7-shaped literals so 0130 (fb_valid) accepts them and every attack
// reaches the permanence/ownership guard it was written for.
const CUSTOMER: &str = "00000000-0000-7000-8000-00000000c001";
const VENDOR: &str = "00000000-0000-7000-8000-00000000a001"; // tax agency
const MOVED_VENDOR: &str = "00000000-0000-7000-8000-00000000a002"; // attacker's rekey target
const ITEM: &str = "00000000-0000-7000-8000-00000000b001";
const RATE: &str = "00000000-0000-7000-8000-00000000d001";
const RATE2: &str = "00000000-0000-7000-8000-00000000d002";
const GROUP: &str = "00000000-0000-7000-8000-00000000e001";
const GROUP2: &str = "00000000-0000-7000-8000-00000000e002";
const MEMBER: &str = "00000000-0000-7000-8000-00000000f001";
const REPLACEMENT_MEMBER: &str = "00000000-0000-7000-8000-00000000f002"; // attacker's reinsert
const NEW_MEMBER: &str = "00000000-0000-7000-8000-00000000f003"; // service-style replacement
const SETTINGS: &str = "00000000-0000-7000-8000-000000005e71";
const SEQUENCE: &str = "00000000-0000-7000-8000-000000005e01";
const SOURCE: &str = "00000000-0000-7000-8000-000000050001"; // document identity
const SOURCE2: &str = "00000000-0000-7000-8000-000000050002";
const REQUEST_KEY: &str = "00000000-0000-7000-8000-00000000ee01";
const REQUEST_KEY2: &str = "00000000-0000-7000-8000-00000000ee02";
const SNAPSHOT: &str = "00000000-0000-7000-8000-000000060001"; // document account snapshot
const SETUP: &str = "00000000-0000-7000-8000-000000070001";
const SETUP2: &str = "00000000-0000-7000-8000-000000070002";
const BANK: &str = "00000000-0000-7000-8000-000000080001";
const REPLACEMENT_BANK: &str = "00000000-0000-7000-8000-000000080002"; // attacker's reinsert
const NEW_BANK: &str = "00000000-0000-7000-8000-000000080003"; // service-style replacement
const NOW: &str = "2026-01-01T00:00:00Z";

const TABLES: &[&str] = &[
    "directory_customer",
    "directory_vendor",
    "directory_item",
    "salestax_taxitem",
    "salestax_taxgroupmember",
    "salestax_salestaxsettings",
    "documents_documentsequence",
    "documents_documentidentity",
    "documents_documentaccount",
    "documents_accountingsetup",
    "documents_designatedbank",
];

fn fixture() -> Fx {
    let mut f = Fx::new("l2a-permanent");
    let c = f.company("C");
    let d = f.company("D");
    f.company("Unused company");
    let a = f.account(c, "100", "Cash", "ASSET");
    f.account(c, "101", "Other cash", "ASSET");
    let user = f.user;
    f.account(d, "100", "Other company cash", "ASSET");
    f.book.write(|w| -> rusqlite::Result<()> {
        w.execute_batch(&format!("
            INSERT INTO directory_customer(id,company_id,display_name) VALUES('{CUSTOMER}','{c}','Customer');
            INSERT INTO directory_vendor(id,company_id,display_name) VALUES('{VENDOR}','{c}','Vendor');
            INSERT INTO directory_item(id,company_id,code,kind,sale_rate) VALUES('{ITEM}','{c}','Item','SERVICE',1);
            INSERT INTO salestax_taxitem(id,company_id,kind,name,description,rate,agency_id) VALUES('{RATE}','{c}','RATE','Rate','Rate',1,'{VENDOR}'),('{RATE2}','{c}','RATE','Rate 2','Rate 2',2,'{VENDOR}');
            INSERT INTO salestax_taxitem(id,company_id,kind,name,description) VALUES('{GROUP}','{c}','GROUP','Group','Group'),('{GROUP2}','{c}','GROUP','Group 2','Group 2');
            INSERT INTO salestax_taxgroupmember VALUES('{MEMBER}','{GROUP}','{RATE}',1);
            INSERT INTO salestax_salestaxsettings(id,company_id) VALUES('{SETTINGS}','{c}');
            INSERT INTO documents_documentsequence(id,company_id,kind,last_number) VALUES('{SEQUENCE}','{c}','INV',2);
            INSERT INTO documents_documentidentity VALUES('{SOURCE}','{c}','INV',1,'INV-000001','{REQUEST_KEY}','USD','{NOW}'),('{SOURCE2}','{c}','INV',2,'INV-000002','{REQUEST_KEY2}','USD','{NOW}');
            INSERT INTO documents_documentaccount VALUES('{SNAPSHOT}','{SOURCE}','asset','{a}','100','Cash','ASSET');
            INSERT INTO documents_accountingsetup(id,company_id) VALUES('{SETUP}','{c}'),('{SETUP2}','{d}');
            INSERT INTO documents_designatedbank VALUES('{BANK}','{SETUP}','{a}');
            INSERT INTO ledger_journal(id,number,company_id,date,memo,created_by_id,created_at,source_id)
            VALUES('00000000-0000-7000-8000-000000000099',1,'{c}','2026-01-01','Source draft','{user}','{NOW}','{SOURCE}'),
                  ('00000000-0000-7000-8000-000000000098',2,'{c}','2026-01-01','Manual draft','{user}','{NOW}',NULL);
        "))?;
        Ok(())
    }).unwrap();
    f
}

fn dump(conn: &Connection) -> BTreeMap<String, Vec<Vec<Value>>> {
    let tables = conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT GLOB 'sqlite_*' ORDER BY name").unwrap()
        .query_map([], |r| r.get::<_, String>(0)).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
    tables
        .into_iter()
        .map(|table| {
            let mut stmt = conn
                .prepare(&format!("SELECT * FROM {table} ORDER BY 1,2"))
                .unwrap();
            let width = stmt.column_count();
            let rows = stmt
                .query_map([], |r| {
                    (0..width)
                        .map(|i| r.get(i))
                        .collect::<Result<Vec<Value>, _>>()
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            (table, rows)
        })
        .collect()
}

/// The guard name of a refused write; never the 0130 shared validator.
fn guard_of(sql: &str, e: &rusqlite::Error) -> String {
    assert!(fb_db::is_constraint(e), "{sql}: {e}");
    let guard = fb_db::constraint_of(e).unwrap_or_else(|| panic!("unnamed refusal for {sql}: {e}"));
    assert!(!guard.starts_with("fb_valid"), "0130 masked the intended guard for {sql}: {e}");
    guard
}

/// Run one attack in a rolled-back savepoint. Accepted statements are collected; a refusal must be a
/// named non-`fb_valid` guard with no partial change, and its guard name is recorded under `label`
/// so the caller can pin the exact set of refusing guards per attack.
fn attempt(
    conn: &Connection,
    label: &str,
    sql: &str,
    accepted: &mut Vec<String>,
    hit: &mut BTreeMap<String, BTreeSet<String>>,
) {
    let before = dump(conn);
    conn.execute_batch("SAVEPOINT permanent_case").unwrap();
    match conn.execute_batch(sql) {
        Ok(()) => accepted.push(sql.to_string()),
        Err(e) => {
            let guard = guard_of(sql, &e);
            assert_eq!(dump(conn), before, "partial change: {sql}");
            hit.entry(label.to_string()).or_default().insert(guard);
        }
    }
    conn.execute_batch("ROLLBACK TO permanent_case; RELEASE permanent_case")
        .unwrap();
    assert_eq!(dump(conn), before);
}

#[test]
fn every_l2a_primary_key_and_ownership_identity_is_permanent() {
    let f = fixture();
    let conn = f.book.connection();
    let actual = conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND (name GLOB 'directory_*' OR name GLOB 'salestax_*' OR name GLOB 'documents_*') ORDER BY name").unwrap()
        .query_map([], |r| r.get::<_, String>(0)).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
    let mut expected = TABLES.to_vec();
    expected.sort();
    assert_eq!(
        actual, expected,
        "extend the matrix for every new protected table"
    );
    let other_company: String = conn
        .query_row(
            "SELECT id FROM ledger_company WHERE name='Unused company'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let other_account: String = conn
        .query_row("SELECT id FROM ledger_account WHERE code='101'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let foreign_account: String = conn.query_row("SELECT a.id FROM ledger_account a JOIN ledger_company c ON c.id=a.company_id WHERE c.name='D'", [], |r| r.get(0)).unwrap();
    let mut accepted = Vec::new();
    let mut hit: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for recursion in ["ON", "OFF"] {
        conn.execute_batch(&format!("PRAGMA recursive_triggers={recursion}"))
            .unwrap();
        for operation in ["UPDATE", "UPDATE OR REPLACE"] {
            for table in TABLES {
                let keys = conn
                    .prepare(&format!(
                        "SELECT name FROM pragma_table_info('{table}') WHERE pk>0 ORDER BY pk"
                    ))
                    .unwrap()
                    .query_map([], |r| r.get::<_, String>(0))
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                assert_eq!(keys, ["id"], "inventory all primary-key parts");
                attempt(
                    conn,
                    &format!("{operation} {table}.id"),
                    &format!(
                        "{operation} {table} SET id='{}' WHERE id=(SELECT id FROM {table} ORDER BY id LIMIT 1)",
                        UserId::new()
                    ),
                    &mut accepted,
                    &mut hit,
                );
                let columns = conn
                    .prepare(&format!(
                        "SELECT name FROM pragma_table_info('{table}') WHERE name='company_id'"
                    ))
                    .unwrap()
                    .query_map([], |r| r.get::<_, String>(0))
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                if !columns.is_empty() {
                    attempt(
                        conn,
                        &format!("{operation} {table}.company_id"),
                        &format!(
                            "{operation} {table} SET company_id='{other_company}' WHERE company_id<>'{other_company}'"
                        ),
                        &mut accepted,
                        &mut hit,
                    );
                }
            }
            attempt(
                conn,
                &format!("{operation} documents_designatedbank.setup_id+account_id"),
                &format!(
                    "{operation} documents_designatedbank SET setup_id='{SETUP2}',account_id='{foreign_account}' WHERE id='{BANK}'"
                ),
                &mut accepted,
                &mut hit,
            );
            for (label, sql) in [
                (
                    "ledger_journal.source_id moved",
                    format!("{operation} ledger_journal SET source_id='{SOURCE2}' WHERE source_id='{SOURCE}'"),
                ),
                (
                    "ledger_journal.source_id cleared",
                    format!("{operation} ledger_journal SET source_id=NULL WHERE source_id='{SOURCE}'"),
                ),
                (
                    "ledger_journal.source_id attached",
                    format!("{operation} ledger_journal SET source_id='{SOURCE2}' WHERE source_id IS NULL"),
                ),
            ] {
                attempt(conn, &format!("{operation} {label}"), &sql, &mut accepted, &mut hit);
            }
            for (table, column, value, id) in [
                ("salestax_taxgroupmember", "group_id", GROUP2, MEMBER),
                ("salestax_taxgroupmember", "member_id", RATE2, MEMBER),
                ("documents_documentsequence", "kind", "BILL", SEQUENCE),
                ("documents_documentaccount", "source_id", SOURCE2, SNAPSHOT),
                ("documents_documentaccount", "account_id", other_account.as_str(), SNAPSHOT),
                ("documents_documentaccount", "role", "other-role", SNAPSHOT),
                ("documents_designatedbank", "setup_id", SETUP2, BANK),
                ("documents_designatedbank", "account_id", other_account.as_str(), BANK),
            ] {
                attempt(
                    conn,
                    &format!("{operation} {table}.{column}"),
                    &format!("{operation} {table} SET {column}='{value}' WHERE id='{id}'"),
                    &mut accepted,
                    &mut hit,
                );
            }
        }
    }
    conn.execute_batch("PRAGMA recursive_triggers=ON").unwrap();
    assert!(
        accepted.is_empty(),
        "accepted identity changes: {accepted:#?}"
    );
    // The exact refusing guard per attack (same for UPDATE and UPDATE OR REPLACE, recursive triggers
    // ON and OFF). Pinned so the 0130 shared validator can never again mask a permanence guard.
    let per_attack: &[(&str, &str)] = &[
        ("directory_customer.id", "sales_reference_protected"),
        ("directory_customer.company_id", "identity_immutable"),
        ("directory_vendor.id", "primary_key_immutable"),
        ("directory_vendor.company_id", "identity_immutable"),
        ("directory_item.id", "sales_reference_protected"),
        ("directory_item.company_id", "identity_immutable"),
        ("salestax_taxitem.id", "primary_key_immutable"),
        ("salestax_taxitem.company_id", "identity_immutable"),
        ("salestax_taxgroupmember.id", "primary_key_immutable"),
        ("salestax_taxgroupmember.group_id", "identity_immutable"),
        ("salestax_taxgroupmember.member_id", "identity_immutable"),
        ("salestax_salestaxsettings.id", "primary_key_immutable"),
        ("salestax_salestaxsettings.company_id", "identity_immutable"),
        ("documents_documentsequence.id", "primary_key_immutable"),
        ("documents_documentsequence.company_id", "identity_immutable"),
        ("documents_documentsequence.kind", "identity_immutable"),
        ("documents_documentidentity.id", "primary_key_immutable"),
        ("documents_documentidentity.company_id", "identity_immutable"),
        ("documents_documentaccount.id", "primary_key_immutable"),
        ("documents_documentaccount.source_id", "identity_immutable"),
        ("documents_documentaccount.account_id", "identity_immutable"),
        ("documents_documentaccount.role", "identity_immutable"),
        ("documents_accountingsetup.id", "primary_key_immutable"),
        ("documents_accountingsetup.company_id", "identity_immutable"),
        ("documents_designatedbank.id", "primary_key_immutable"),
        ("documents_designatedbank.setup_id", "identity_immutable"),
        ("documents_designatedbank.account_id", "identity_immutable"),
        ("documents_designatedbank.setup_id+account_id", "identity_immutable"),
        ("ledger_journal.source_id moved", "source_journal_ownership"),
        ("ledger_journal.source_id cleared", "source_journal_ownership"),
        ("ledger_journal.source_id attached", "source_journal_ownership"),
    ];
    let expected: BTreeMap<String, BTreeSet<String>> = ["UPDATE", "UPDATE OR REPLACE"]
        .iter()
        .flat_map(|op| {
            per_attack.iter().map(move |(label, guard)| {
                (format!("{op} {label}"), BTreeSet::from([guard.to_string()]))
            })
        })
        .collect();
    assert_eq!(hit, expected, "refusing guard per attack");
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn reparent_then_reinsert_cannot_replace_tax_or_bank_relationships() {
    let mut f = fixture();
    let before = dump(f.book.connection());
    let other_account: String = f
        .book
        .connection()
        .query_row("SELECT id FROM ledger_account WHERE code='101'", [], |r| {
            r.get(0)
        })
        .unwrap();
    for (sql, expected) in [
        (format!("UPDATE salestax_taxgroupmember SET group_id='{GROUP2}' WHERE id='{MEMBER}'; INSERT INTO salestax_taxgroupmember VALUES('{REPLACEMENT_MEMBER}','{GROUP}','{RATE}',1)"), "identity_immutable"),
        (format!("UPDATE salestax_taxgroupmember SET member_id='{RATE2}' WHERE id='{MEMBER}'; INSERT INTO salestax_taxgroupmember VALUES('{REPLACEMENT_MEMBER}','{GROUP}','{RATE}',2)"), "identity_immutable"),
        (format!("UPDATE documents_designatedbank SET account_id='{other_account}' WHERE id='{BANK}'; INSERT INTO documents_designatedbank SELECT '{REPLACEMENT_BANK}','{SETUP}',account_id FROM documents_documentaccount WHERE id='{SNAPSHOT}'"), "identity_immutable"),
    ] {
        let result: rusqlite::Result<()> = f.book.write(|w| w.execute_batch(&sql));
        let e = result.expect_err(&format!("accepted reparent/reinsert: {sql}"));
        assert_eq!(guard_of(&sql, &e), expected, "{sql}: {e}");
        assert_eq!(dump(f.book.connection()), before);
    }
}

#[test]
fn vendor_rekey_then_reinsert_cannot_redirect_recorded_tax_agency() {
    let mut f = fixture();
    let before = dump(f.book.connection());
    let sql = format!("UPDATE directory_vendor SET id='{MOVED_VENDOR}' WHERE id='{VENDOR}';
            INSERT INTO directory_vendor(id,company_id,display_name)
            SELECT '{VENDOR}',company_id,'Replacement agency' FROM directory_vendor WHERE id='{MOVED_VENDOR}'");
    let result: rusqlite::Result<()> = f.book.write(|w| w.execute_batch(&sql));
    let e = result.expect_err("deferred agency FK must never permit rekey/reinsert");
    assert_eq!(guard_of(&sql, &e), "primary_key_immutable", "{e}");
    assert_eq!(dump(f.book.connection()), before);
    let agency: String = f.book.connection().query_row("SELECT v.display_name FROM salestax_taxitem t JOIN directory_vendor v ON v.id=t.agency_id WHERE t.id=?", [RATE], |r| r.get(0)).unwrap();
    assert_eq!(agency, "Vendor");
}

#[test]
fn ordinary_edits_noop_keys_and_service_style_link_replacement_still_work() {
    let mut f = fixture();
    f.book.write(|w| -> rusqlite::Result<()> {
        assert_eq!(w.execute("UPDATE directory_customer SET id=id,company_id=company_id,display_name='Edited' WHERE id=?", [CUSTOMER])?, 1);
        assert_eq!(w.execute("UPDATE salestax_taxgroupmember SET id=id,group_id=group_id,member_id=member_id,position=2 WHERE id=?", [MEMBER])?, 1);
        assert_eq!(w.execute("UPDATE documents_designatedbank SET id=id,setup_id=setup_id,account_id=account_id WHERE id=?", [BANK])?, 1);
        w.execute_batch(&format!("DELETE FROM salestax_taxgroupmember WHERE id='{MEMBER}'; INSERT INTO salestax_taxgroupmember VALUES('{NEW_MEMBER}','{GROUP}','{RATE2}',1);
            DELETE FROM documents_designatedbank WHERE id='{BANK}';
            INSERT INTO documents_designatedbank SELECT '{NEW_BANK}','{SETUP}',id FROM ledger_account WHERE code='101'"))?;
        Ok(())
    }).unwrap();
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn populated_l2a_v6_v7_v8_upgrade_preserves_every_row_and_reopens() {
    for prior in [6, 7, 8] {
        let f = fixture();
        let before = dump(f.book.connection());
        let original = book_path("l2a-historical-prefix");
        let historical = Connection::open(&original).unwrap();
        historical.execute_batch("PRAGMA key=\"x'2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a'\"").unwrap();
        fb_db::collation::register(&historical).unwrap();
        for (_, sql) in &fb_db::MIGRATIONS[..prior] { historical.execute_batch(sql).unwrap(); }
        let definitions = historical.prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger'").unwrap()
            .query_map([], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?))).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
        historical.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE; PRAGMA defer_foreign_keys=ON").unwrap();
        for (name, _) in &definitions { historical.execute_batch(&format!("DROP TRIGGER {name}")).unwrap(); }
        for (table, rows) in &before {
            let exists: bool = historical.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)", [table], |r| r.get(0)).unwrap();
            if !exists {
                assert!(rows.is_empty(), "cannot discard populated later table {table}");
                continue;
            }
            for row in rows {
                historical.execute(&format!("INSERT INTO {table} VALUES({})", vec!["?"; row.len()].join(",")), fb_db::rusqlite::params_from_iter(row)).unwrap();
            }
        }
        for (_, sql) in definitions { historical.execute_batch(&sql).unwrap(); }
        historical.execute_batch(&format!("PRAGMA user_version={prior}; COMMIT; PRAGMA wal_checkpoint(TRUNCATE)")).unwrap();
        drop(historical);
        drop(f);
        let path = book_path("l2a-upgrade-copy");
        std::fs::copy(original, &path).unwrap();
        // Exercise and inspect the core-only v6/v7/v8 -> v9 upgrade in the
        // copied encrypted file before Book opens it through the full L3 chain.
        let mut upgrade = Connection::open(&path).unwrap();
        upgrade.execute_batch("PRAGMA key=\"x'2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a'\"; PRAGMA foreign_keys=ON; PRAGMA recursive_triggers=ON").unwrap();
        fb_db::collation::register(&upgrade).unwrap();
        let prefix_before = dump(&upgrade);
        for (index, (_, sql)) in fb_db::MIGRATIONS.iter().enumerate().take(9).skip(prior) {
            let tx = upgrade.transaction().unwrap();
            tx.execute_batch(sql).unwrap();
            tx.execute_batch(&format!("PRAGMA user_version={}", index + 1)).unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(dump(&upgrade), prefix_before, "core v{prior} -> v9 changed rows");
        assert_eq!(upgrade.query_row("PRAGMA user_version", [], |r| r.get::<_,i64>(0)).unwrap(), 9);
        drop(upgrade);
        let book = fb_core::Book::open(&path, &KEY).unwrap();
        fb_db::test_support::assert_desktop_user_backfill(book.connection());
        let mut before = before;
        assert!(before["desktop_user"].is_empty());
        before.insert("desktop_user".into(), fb_db::test_support::desktop_user_backfill(book.connection()).unwrap());
        assert_eq!(
            dump(book.connection()),
            before,
            "v{prior} upgrade changed data"
        );
        assert_eq!(
            book.connection()
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            fb_db::MIGRATIONS.len() as i64
        );
        assert!(book.check().unwrap().is_empty());
        drop(book);
        let book = fb_core::Book::open(&path, &KEY).unwrap();
        assert_eq!(dump(book.connection()), before);
        let sql = format!("UPDATE salestax_taxgroupmember SET member_id='{RATE2}' WHERE id='{MEMBER}'");
        let e = book
            .connection()
            .execute_batch(&sql)
            .expect_err("upgraded book must keep tax-group membership permanent");
        assert_eq!(guard_of(&sql, &e), "identity_immutable", "{e}");
    }
}
