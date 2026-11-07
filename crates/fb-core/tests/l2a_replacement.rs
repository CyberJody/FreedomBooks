//! Replacement, displaced-row, delete/reinsert and rowid attacks for EVERY L2a table.
mod common;
use common::*;
use fb_db::rusqlite::{Connection, types::Value};
use std::collections::{BTreeMap, BTreeSet};

// Fixture ids: canonical UUIDv7-shaped literals so 0130 (fb_valid) accepts them and every attack reaches the
// L2a guard it was written for. The tail names the entity (old placeholder in the comment); groups sort before
// rates exactly as 'g*' sorted before 'r*', so the generated attacks still pick the same first/second rows.
const C1: &str = "00000000-0000-7000-8000-000000010001"; // customer c1
const C2: &str = "00000000-0000-7000-8000-000000010002"; // customer c2
const V1: &str = "00000000-0000-7000-8000-000000020001"; // vendor v1
const V2: &str = "00000000-0000-7000-8000-000000020002"; // vendor v2
const I1: &str = "00000000-0000-7000-8000-000000030001"; // item i1
const I2: &str = "00000000-0000-7000-8000-000000030002"; // item i2
const G1: &str = "00000000-0000-7000-8000-000000040001"; // tax group g1
const G2: &str = "00000000-0000-7000-8000-000000040002"; // tax group g2
const R1: &str = "00000000-0000-7000-8000-000000050001"; // tax rate r1
const R2: &str = "00000000-0000-7000-8000-000000050002"; // tax rate r2
const R3: &str = "00000000-0000-7000-8000-000000050003"; // tax rate r3
const M1: &str = "00000000-0000-7000-8000-000000060001"; // group member m1
const M2: &str = "00000000-0000-7000-8000-000000060002"; // group member m2
const M3: &str = "00000000-0000-7000-8000-000000060003"; // group member m3
const T1: &str = "00000000-0000-7000-8000-000000070001"; // sales tax settings t1
const T2: &str = "00000000-0000-7000-8000-000000070002"; // sales tax settings t2
const Q1: &str = "00000000-0000-7000-8000-000000080001"; // document sequence q1
const Q2: &str = "00000000-0000-7000-8000-000000080002"; // document sequence q2
const S1: &str = "00000000-0000-7000-8000-000000090001"; // document identity s1
const S2: &str = "00000000-0000-7000-8000-000000090002"; // document identity s2
const S3: &str = "00000000-0000-7000-8000-000000090003"; // document identity s3
const A1: &str = "00000000-0000-7000-8000-0000000a0001"; // document account a1
const A2: &str = "00000000-0000-7000-8000-0000000a0002"; // document account a2
const U1: &str = "00000000-0000-7000-8000-0000000b0001"; // accounting setup u1
const U2: &str = "00000000-0000-7000-8000-0000000b0002"; // accounting setup u2
const B1: &str = "00000000-0000-7000-8000-0000000c0001"; // designated bank b1
const B2: &str = "00000000-0000-7000-8000-0000000c0002"; // designated bank b2
const KEY1: &str = "00000000-0000-7000-8000-0000000d0001"; // request key key1
const KEY2: &str = "00000000-0000-7000-8000-0000000d0002"; // request key key2
const KEY3: &str = "00000000-0000-7000-8000-0000000d0003"; // request key key3
const FRESH_HIDDEN: &str = "00000000-0000-7000-8000-0000000e0001"; // attacker's rowid-alias row id 'fresh-hidden'
const FRESH: &str = "00000000-0000-7000-8000-0000000e0002"; // attacker's displacing row id 'fresh'
const FRESH_KEY: &str = "00000000-0000-7000-8000-0000000e0003"; // attacker's request key 'fresh-key'
const BAD: &str = "00000000-0000-7000-8000-0000000e0004"; // attacker's new row id 'bad'
const MISSING: &str = "00000000-0000-7000-8000-0000000e0005"; // well-formed id of no row ('missing')
const NOW: &str = "2026-01-01T00:00:00Z"; // created_at 'now'

const TABLES: &[(&str, &[&[&str]])] = &[
    ("directory_customer", &[]),
    ("directory_vendor", &[]),
    ("directory_item", &[&["company_id", "code"]]),
    ("salestax_taxitem", &[&["company_id", "name"]]),
    (
        "salestax_taxgroupmember",
        &[&["group_id", "member_id"], &["group_id", "position"]],
    ),
    ("salestax_salestaxsettings", &[&["company_id"]]),
    ("documents_documentsequence", &[&["company_id", "kind"]]),
    (
        "documents_documentidentity",
        &[
            &["company_id", "number"],
            &["company_id", "kind", "ordinal"],
            &["company_id", "request_key"],
        ],
    ),
    ("documents_documentaccount", &[&["source_id", "role"]]),
    ("documents_accountingsetup", &[&["company_id"]]),
    ("documents_designatedbank", &[&["setup_id", "account_id"]]),
];

fn fixture() -> Fx {
    let mut f = Fx::new("l2-replace");
    let c = f.company("C");
    let d = f.company("D");
    let a = f.account(c, "100", "A", "ASSET");
    let b = f.account(c, "101", "B", "ASSET");
    f.book.connection().execute_batch(&format!("
        INSERT INTO directory_customer(id,company_id,display_name) VALUES('{C1}','{c}','C1'),('{C2}','{c}','C2');
        INSERT INTO directory_vendor(id,company_id,display_name) VALUES('{V1}','{c}','V1'),('{V2}','{c}','V2');
        INSERT INTO directory_item(id,company_id,code,kind,sale_rate) VALUES('{I1}','{c}','I1','SERVICE',1),('{I2}','{c}','I2','SERVICE',1);
        INSERT INTO salestax_taxitem(id,company_id,kind,name,description,rate,agency_id) VALUES('{R1}','{c}','RATE','R1','R1',1,'{V1}'),('{R2}','{c}','RATE','R2','R2',2,'{V2}'),('{R3}','{c}','RATE','R3','R3',3,'{V1}');
        INSERT INTO salestax_taxitem(id,company_id,kind,name,description) VALUES('{G1}','{c}','GROUP','G1','G1'),('{G2}','{c}','GROUP','G2','G2');
        INSERT INTO salestax_taxgroupmember VALUES('{M1}','{G1}','{R1}',1),('{M2}','{G1}','{R2}',2);
        INSERT INTO salestax_salestaxsettings(id,company_id) VALUES('{T1}','{c}'),('{T2}','{d}');
        INSERT INTO documents_documentsequence(id,company_id,kind,last_number) VALUES('{Q1}','{c}','INV',2),('{Q2}','{c}','BILL',0);
        INSERT INTO documents_documentidentity VALUES('{S1}','{c}','INV',1,'INV-000001','{KEY1}','USD','{NOW}'),('{S2}','{c}','INV',2,'INV-000002','{KEY2}','USD','{NOW}');
        INSERT INTO documents_documentaccount VALUES('{A1}','{S1}','asset','{a}','100','A','ASSET'),('{A2}','{S2}','asset','{b}','101','B','ASSET');
        INSERT INTO documents_accountingsetup(id,company_id) VALUES('{U1}','{c}'),('{U2}','{d}');
        INSERT INTO documents_designatedbank VALUES('{B1}','{U1}','{a}'),('{B2}','{U1}','{b}');
    ")).unwrap();
    f
}

fn dump(conn: &Connection) -> BTreeMap<String, Vec<Vec<Value>>> {
    let tables=conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT GLOB 'sqlite_*' ORDER BY name").unwrap()
        .query_map([],|r|r.get::<_,String>(0)).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
    tables
        .into_iter()
        .map(|table| {
            let mut stmt = conn
                .prepare(&format!("SELECT * FROM {table} ORDER BY 1"))
                .unwrap();
            let n = stmt.column_count();
            let rows = stmt
                .query_map([], |r| {
                    (0..n).map(|i| r.get(i)).collect::<Result<Vec<Value>, _>>()
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            (table, rows)
        })
        .collect()
}

/// `sql` must be refused with the complete book unchanged, and never by 0130's shared `fb_valid` value guard
/// (that would mask the guard under test). Returns the refusing guard/constraint name so callers pin it.
fn refused_unchanged(conn: &Connection, sql: &str) -> String {
    let before = dump(conn);
    conn.execute_batch("SAVEPOINT attempt").unwrap();
    let error = conn.execute_batch(sql).expect_err(&format!("accepted {sql}"));
    let guard = fb_db::constraint_of(&error).unwrap_or_else(|| format!("other: {error}"));
    assert!(
        !guard.starts_with("fb_valid") && !error.to_string().contains("fb_valid"),
        "0130 fb_valid masked the intended guard: {error} in {sql}"
    );
    assert_eq!(dump(conn), before, "partially changed after refusal: {sql}");
    conn.execute_batch("ROLLBACK TO attempt; RELEASE attempt")
        .unwrap();
    guard
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| n.to_string()).collect()
}

#[test]
fn every_primary_and_alternate_key_resists_insert_and_update_replace() {
    let f = fixture();
    let conn = f.book.connection();
    let mut attacks = 0;
    let mut hit: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for recursive in ["ON", "OFF"] {
        conn.execute_batch(&format!("PRAGMA recursive_triggers={recursive}"))
            .unwrap();
        for (table, keys) in TABLES {
            let guards = hit.entry(*table).or_default();
            let mut refuse = |sql: &str| {
                guards.insert(refused_unchanged(conn, sql));
            };
            let columns = conn
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap()
                .query_map([], |r| r.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            let ids = conn
                .prepare(&format!("SELECT id FROM {table} ORDER BY id LIMIT 2"))
                .unwrap()
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            let (a, b) = (&ids[0], &ids[1]);
            let definition: String = conn.query_row("SELECT sql FROM sqlite_master WHERE name=?1", [table], |r| r.get(0)).unwrap();
            assert!(definition.contains("WITHOUT ROWID"), "{table}");
            let view = format!("attack_{table}");
            let all_set = columns.iter().map(|c| format!("{c}=NEW.{c}")).collect::<Vec<_>>().join(",");
            conn.execute_batch(&format!("CREATE TEMP VIEW {view} AS SELECT * FROM {table}; CREATE TEMP TRIGGER {view}_update INSTEAD OF UPDATE ON {view} BEGIN UPDATE {table} SET {all_set} WHERE id=OLD.id; END;")).unwrap();
            // Explicit rowid aliases cannot bypass UPDATE triggers on protected UUID tables.
            for alias in ["rowid", "_rowid_", "oid"] {
                refuse(&format!("UPDATE OR REPLACE {table} SET {alias}=2 WHERE id='{a}'"),
                );
                attacks += 1;
                let values = columns.iter().map(|c| if c == "id" { format!("'{FRESH_HIDDEN}'") } else { c.clone() }).collect::<Vec<_>>().join(",");
                refuse(&format!("INSERT OR REPLACE INTO {table}({alias},{}) SELECT 1,{values} FROM {table} WHERE id='{a}'", columns.join(",")));
                attacks += 1;
            }
            refuse(&format!("INSERT OR REPLACE INTO {table} SELECT * FROM {table} WHERE id='{a}'"),
            );
            attacks += 1;
            refuse(&format!("UPDATE OR REPLACE {table} SET id='{a}' WHERE id='{b}'"),
            );
            attacks += 1;
            for target in [table.to_string(), view.clone()] {
                for policy in ["UPDATE", "UPDATE OR REPLACE"] {
                    refuse(&format!("{policy} {target} SET id='{a}' WHERE id='{b}'"));
                    attacks += 1;
                }
            }
            refuse(&format!("INSERT INTO {table} SELECT * FROM {table} WHERE id='{a}' ON CONFLICT(id) DO UPDATE SET id=excluded.id"));
            attacks += 1;
            for (index, key) in keys.iter().enumerate() {
                let values = columns
                    .iter()
                    .map(|col| {
                        if col == "id" {
                            format!("'{FRESH}'")
                        } else if *table == "salestax_taxgroupmember"
                            && index == 0
                            && col == "position"
                        {
                            "3".to_string()
                        } else if *table == "salestax_taxgroupmember"
                            && index == 1
                            && col == "member_id"
                        {
                            format!("'{R3}'")
                        } else if *table == "documents_documentidentity"
                            && index == 2
                            && col == "number"
                        {
                            "'INV-000003'".to_string()
                        } else if *table == "documents_documentidentity"
                            && index == 2
                            && col == "ordinal"
                        {
                            "3".to_string()
                        } else if *table == "documents_documentidentity"
                            && index != 2
                            && col == "request_key"
                        {
                            format!("'{FRESH_KEY}'")
                        } else {
                            col.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                refuse(&format!(
                        "INSERT OR REPLACE INTO {table} SELECT {values} FROM {table} WHERE id='{a}'"
                    ),
                );
                attacks += 1;
                // Incoming row is distinct; UPDATE must inspect the displaced row's keys.
                let set = key
                    .iter()
                    .map(|col| format!("{col}=(SELECT {col} FROM {table} WHERE id='{a}')"))
                    .collect::<Vec<_>>()
                    .join(",");
                refuse(&format!("UPDATE OR REPLACE {table} SET {set} WHERE id='{b}'"),
                );
                attacks += 1;
                for target in [table.to_string(), view.clone()] {
                    for policy in ["UPDATE", "UPDATE OR REPLACE"] {
                        refuse(&format!("{policy} {target} SET {set} WHERE id='{b}'"));
                        attacks += 1;
                    }
                }
                let incoming = key.iter().map(|c| format!("{c}=excluded.{c}")).collect::<Vec<_>>().join(",");
                refuse(&format!("INSERT INTO {table} SELECT {values} FROM {table} WHERE id='{a}' ON CONFLICT DO UPDATE SET {incoming}"));
                attacks += 1;
            }
            conn.execute_batch(&format!("DROP VIEW {view}")).unwrap();
        }
    }
    conn.execute_batch("PRAGMA recursive_triggers=ON").unwrap();
    // WITHOUT ROWID tables have no rowid alias at all: those attacks fail at SQL compile time.
    let rowid = |table: &str| -> BTreeSet<String> {
        ["rowid", "_rowid_", "oid"]
            .iter()
            .flat_map(|alias| {
                [
                    format!("other: no such column: {alias}"),
                    format!("other: table {table} has no column named {alias}"),
                ]
            })
            .collect()
    };
    let identity: &[&str] = &["identity_immutable", "l2_key_collision", "primary_key_immutable"];
    let guards: [(&str, &[&str]); 11] = [
        ("directory_customer", &["l2_key_collision", "sales_reference_protected"]),
        ("directory_vendor", &["l2_key_collision", "primary_key_immutable"]),
        ("directory_item", &["l2_key_collision", "sales_reference_protected"]),
        ("salestax_taxitem", &["l2_key_collision", "primary_key_immutable"]),
        ("salestax_taxgroupmember", identity),
        ("salestax_salestaxsettings", identity),
        ("documents_documentsequence", identity),
        ("documents_documentidentity", &["l2_key_collision", "primary_key_immutable"]),
        ("documents_documentaccount", identity),
        ("documents_accountingsetup", identity),
        ("documents_designatedbank", identity),
    ];
    let expected: BTreeMap<&str, BTreeSet<String>> = guards
        .iter()
        .map(|(table, names)| (*table, set(names).union(&rowid(table)).cloned().collect()))
        .collect();
    assert_eq!(hit, expected, "refusing guards per table changed (none may be fb_valid)");
    assert!(f.book.check().unwrap().is_empty());
    println!("{attacks} replacement/rowid attacks refused with complete book unchanged");
}

#[test]
fn delete_reinsert_cannot_rewrite_protected_rows_and_mutable_links_stay_editable() {
    let f = fixture();
    let conn = f.book.connection();
    let mut hit: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for recursive in ["ON", "OFF"] {
        conn.execute_batch(&format!("PRAGMA recursive_triggers={recursive}"))
            .unwrap();
        for (table, _) in TABLES {
            let mutable = ["salestax_taxgroupmember", "documents_designatedbank"].contains(table);
            let sql = format!(
                "CREATE TEMP TABLE held AS SELECT * FROM {table}; DELETE FROM {table}; INSERT INTO {table} SELECT * FROM held; DROP TABLE held;"
            );
            if mutable {
                let before = dump(conn);
                conn.execute_batch(&sql).unwrap();
                assert_eq!(dump(conn), before);
            } else {
                // TEMP DDL is also rolled back, while main-table content is checked immediately.
                hit.entry(*table).or_default().insert(refused_unchanged(conn, &sql));
            }
        }
        // Immediate protection extends to parents referenced only by L2a.
        for table in ["ledger_account", "ledger_company"] {
            hit.entry(table).or_default().insert(refused_unchanged(conn, &format!("DELETE FROM {table}")));
        }
    }
    let expected: BTreeMap<&str, BTreeSet<String>> = [
        ("directory_customer", "l2_no_delete"),
        ("directory_vendor", "l2_no_delete"),
        ("directory_item", "l2_no_delete"),
        ("salestax_taxitem", "salestax_item_no_delete"),
        ("salestax_salestaxsettings", "salestax_settings_no_delete"),
        ("documents_documentsequence", "document_sequence_delete"),
        ("documents_documentidentity", "document_identity_immutable"),
        ("documents_documentaccount", "document_account_immutable"),
        ("documents_accountingsetup", "l2_no_delete"),
        ("ledger_account", "l2_account_reference"),
        ("ledger_company", "l2_company_reference"),
    ]
    .into_iter()
    .map(|(table, guard)| (table, set(&[guard])))
    .collect();
    assert_eq!(hit, expected, "refusing guards per table changed (none may be fb_valid)");
    conn.execute_batch("PRAGMA recursive_triggers=ON").unwrap();
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn missing_and_ineligible_parents_fail_before_deferred_commit() {
    let mut f = fixture();
    let c = f.company("Other");
    let conn = f.book.connection();
    let hit: Vec<String> = [
        format!("INSERT INTO directory_customer(id,company_id,display_name) VALUES('{BAD}','{MISSING}','bad')"),
        format!("INSERT INTO directory_vendor(id,company_id,display_name,default_expense_account_id) VALUES('{BAD}','{MISSING}','bad','{MISSING}')"),
        format!("INSERT INTO directory_item(id,company_id,code,kind,sale_rate,income_account_id) VALUES('{BAD}','{c}','bad','SERVICE',1,'{MISSING}')"),
        format!("INSERT INTO salestax_taxitem(id,company_id,kind,name,description,rate,agency_id) VALUES('{BAD}','{c}','RATE','bad','bad',1,'{V1}')"),
        format!("INSERT INTO salestax_taxgroupmember VALUES('{BAD}','{MISSING}','{R1}',1)"),
        format!("INSERT INTO salestax_salestaxsettings(id,company_id) VALUES('{BAD}','{MISSING}')"),
        format!("INSERT INTO documents_documentsequence(id,company_id,kind) VALUES('{BAD}','{MISSING}','INV')"),
        format!("INSERT INTO documents_documentidentity VALUES('{BAD}','{MISSING}','INV',1,'INV-000001','{FRESH_KEY}','USD','{NOW}')"),
        format!("INSERT INTO documents_documentaccount VALUES('{BAD}','{MISSING}','role','{MISSING}','100','A','ASSET')"),
        format!("INSERT INTO documents_accountingsetup(id,company_id) VALUES('{BAD}','{MISSING}')"),
        format!("INSERT INTO documents_designatedbank VALUES('{BAD}','{MISSING}','{MISSING}')"),
    ]
    .iter()
    .map(|sql| refused_unchanged(conn, sql))
    .collect();
    let expected = [
        "l2_parent",
        "l2_account_parent",
        "l2_account_parent",
        "l2_tax_agency",
        "salestax_member_scope",
        "l2_parent",
        "l2_parent",
        "l2_parent",
        "document_account_snapshot",
        "l2_parent",
        "l2_bank_account",
    ];
    assert_eq!(hit, expected, "refusing guard per statement changed (none may be fb_valid)");
}

#[test]
fn all_new_guard_definitions_are_hashed_not_just_named() {
    let f = fixture();
    let conn = f.book.connection();
    let objects = conn
        .prepare(
            "SELECT name,tbl_name FROM sqlite_master WHERE type='trigger' AND name GLOB 'fb_l2_*'",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(objects.len() > 60);
    for (name, table) in objects {
        conn.execute_batch("SAVEPOINT tamper").unwrap();
        conn.execute_batch(&format!("DROP TRIGGER {name}; CREATE TRIGGER {name} BEFORE INSERT ON {table} BEGIN SELECT 1; END;")).unwrap();
        assert!(
            f.book.check().unwrap().iter().any(|p| p.contains(&name)),
            "{name}"
        );
        conn.execute_batch("ROLLBACK TO tamper; RELEASE tamper")
            .unwrap();
    }
    for (table, _) in TABLES {
        let ddl: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |r| r.get(0),
            )
            .unwrap();
        assert!(ddl.contains("WITHOUT ROWID"), "{table}");
    }
    assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn parent_eligibility_and_reactivation_are_enforced_in_raw_sql() {
    let f=fixture();let conn=f.book.connection();
    let (company,asset):(String,String)=conn.query_row("SELECT company_id,id FROM ledger_account WHERE code='100'",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    let mut hit: Vec<String> = Vec::new();
    for recursive in ["ON","OFF"] {
        conn.execute_batch(&format!("PRAGMA recursive_triggers={recursive}")).unwrap();
        for sql in [
            format!("INSERT INTO directory_item(id,company_id,code,kind,sale_rate,income_account_id) VALUES('{BAD}','{company}','bad','SERVICE',1,'{asset}')"),
            format!("UPDATE directory_item SET expense_account_id='{asset}' WHERE id='{I1}'"),
            format!("UPDATE documents_accountingsetup SET payables_id='{asset}' WHERE id='{U1}'"),
            format!("UPDATE salestax_taxitem SET active=0 WHERE id='{R1}'"),
        ] {hit.push(refused_unchanged(conn,&sql));}
        // Retained inactive vendor links are allowed, new ones are refused.
        conn.execute_batch(&format!("SAVEPOINT eligibility; UPDATE directory_vendor SET active=0 WHERE id='{V2}'; UPDATE salestax_taxitem SET active=0 WHERE id='{R3}'")).unwrap();
        for sql in [
            format!("INSERT INTO salestax_taxitem(id,company_id,kind,name,description,rate,agency_id) VALUES('{BAD}','{company}','RATE','bad','bad',1,'{V2}')"),
            format!("UPDATE salestax_taxitem SET agency_id='{V2}' WHERE id='{R1}'"),
            format!("UPDATE salestax_salestaxsettings SET default_item_id='{R3}' WHERE id='{T1}'"),
            format!("INSERT INTO salestax_taxgroupmember VALUES('{BAD}','{G1}','{R3}',3)"),
        ] {hit.push(refused_unchanged(conn,&sql));}
        conn.execute_batch(&format!("UPDATE salestax_taxitem SET description='retained agency' WHERE id='{R2}'")).unwrap();
        conn.execute_batch(&format!("UPDATE salestax_taxitem SET active=0 WHERE id='{G2}'; INSERT INTO salestax_taxgroupmember VALUES('{M3}','{G2}','{R3}',1)")).unwrap();
        hit.push(refused_unchanged(conn,&format!("UPDATE salestax_taxitem SET active=1 WHERE id='{G2}'")));
        conn.execute_batch(&format!("UPDATE salestax_salestaxsettings SET default_item_id='{G1}' WHERE id='{T1}'")).unwrap();
        hit.push(refused_unchanged(conn,&format!("UPDATE salestax_taxitem SET active=0 WHERE id='{G1}'")));
        conn.execute_batch("ROLLBACK TO eligibility; RELEASE eligibility").unwrap();
    }
    // Per statement, in order, for recursive_triggers ON then OFF.
    let once = [
        "l2_account_parent",
        "l2_account_parent",
        "l2_setup_account",
        "l2_tax_member",
        "l2_tax_agency",
        "l2_tax_agency",
        "l2_tax_default",
        "l2_tax_member",
        "l2_tax_member",
        "l2_tax_default",
    ];
    let expected: Vec<&str> = once.iter().chain(once.iter()).copied().collect();
    assert_eq!(hit, expected, "refusing guard per statement changed (none may be fb_valid)");
    conn.execute_batch("PRAGMA recursive_triggers=ON").unwrap();assert!(f.book.check().unwrap().is_empty());
}

#[test]
fn every_l2a_write_entry_rolls_back_after_a_late_audit_failure() {
    use fb_core::{
        AccountId, CompanyId, directory as d, directory::Model, documents as doc, salestax as tax,
    };
    use serde_json::json;
    for action in [
        "customer",
        "vendor",
        "item",
        "directory_edit",
        "directory_active",
        "rate",
        "rate_edit",
        "group",
        "group_edit",
        "tax_active",
        "settings",
        "bulk",
        "setup",
        "identify",
        "snapshot",
        "post_source",
        "reverse_source",
    ] {
        let mut f = fixture();
        let c = CompanyId::parse(
            &f.book
                .connection()
                .query_row(
                    &format!("SELECT company_id FROM directory_customer WHERE id='{C1}'"),
                    [],
                    |r| r.get::<_, String>(0),
                )
                .unwrap(),
        )
        .unwrap();
        let a = AccountId::parse(
            &f.book
                .connection()
                .query_row(
                    "SELECT id FROM ledger_account WHERE company_id=?1 AND code='100'",
                    [c.to_db()],
                    |r| r.get::<_, String>(0),
                )
                .unwrap(),
        )
        .unwrap();
        let b = AccountId::parse(
            &f.book
                .connection()
                .query_row(
                    "SELECT id FROM ledger_account WHERE company_id=?1 AND code='101'",
                    [c.to_db()],
                    |r| r.get::<_, String>(0),
                )
                .unwrap(),
        )
        .unwrap();
        let lines = || {
            vec![
                fb_core::services::line(a).debit("1").build(),
                fb_core::services::line(b).credit("1").build(),
            ]
        };
        if action == "reverse_source" {
            f.book
                .write(|tx| doc::post_source(tx, f.user, c, S1, "2026-10-30", "Source", lines()))
                .unwrap();
        }
        if action == "snapshot" {
            f.book.connection().execute_batch(&format!("INSERT INTO documents_documentidentity VALUES('{S3}','{c}','INV',3,'INV-000003','{KEY3}','USD','{NOW}')")).unwrap();
        }
        if action == "bulk" {
            f.book.connection().execute_batch("UPDATE salestax_salestaxsettings SET charge_tax=1; UPDATE directory_customer SET tax_status='EXEMPT'; UPDATE directory_item SET kind='NONSTOCK',taxable=0;").unwrap();
        }
        let preview = tax::bulk_preview(f.book.connection(), c).unwrap();
        f.book.connection().execute_batch("CREATE TRIGGER fail_late BEFORE INSERT ON ledger_auditevent BEGIN SELECT RAISE(ABORT,'injected late audit failure'); END").unwrap();
        let before = dump(f.book.connection());
        let result:fb_core::FbResult<()>=match action {
            "customer"|"vendor"|"item" => {
                let model=match action {"customer"=>Model::Customer,"vendor"=>Model::Vendor,_=>Model::Item};
                let v=if action=="item" {json!({"code":"New","kind":"SERVICE","sale_rate":"1"})} else {json!({"display_name":"New"})};
                d::save_record(&mut f.book,f.user,c,model,&v,None).map(|_|())
            }
            "directory_edit"=>d::save_record(&mut f.book,f.user,c,Model::Customer,&json!({"notes":"edited"}),Some(C1)).map(|_|()),
            "directory_active"=>d::set_active(&mut f.book,f.user,c,Model::Customer,C1,false).map(|_|()),
            "rate"=>tax::save_rate(&mut f.book,f.user,c,&json!({"name":"New","description":"New","rate":"3","new_agency_name":"New agency"})).map(|_|()),
            "rate_edit"=>tax::save_rate(&mut f.book,f.user,c,&json!({"item_id":R1,"expected_revision":1,"name":"R1","description":"Changed","rate":"3","agency_id":V1})).map(|_|()),
            "group"=>tax::save_group(&mut f.book,f.user,c,&json!({"name":"New","description":"New","member_ids":[R1,R2]})).map(|_|()),
            "group_edit"=>tax::save_group(&mut f.book,f.user,c,&json!({"item_id":G1,"expected_revision":1,"name":"G1","description":"Changed","member_ids":[R2,R3]})).map(|_|()),
            "tax_active"=>tax::set_item_active(&mut f.book,f.user,c,G2,false,Some(1)).map(|_|()),
            "settings"=>tax::save_settings(&mut f.book,f.user,c,&json!({"charge_tax":true,"default_item_id":R1,"owed_basis":"CASH","filing_frequency":"MONTHLY","expected_revision":0})).map(|_|()),
            "bulk"=>tax::finish_bulk_step(&mut f.book,f.user,c,true,true,preview["digest"].as_str().unwrap()).map(|_|()),
            "setup"=>doc::save_setup(&mut f.book,f.user,c,&json!({"receivables":a.to_db(),"banks":[b.to_db()]}),Some(0)).map(|_|()),
            "identify"=>f.book.write(|tx|doc::create_identity(tx,f.user,c,"INV","00000000-0000-0000-0000-000000000001")).map(|_|()),
            "snapshot"=>f.book.write(|tx|doc::snapshot_source_accounts(tx,f.user,c,S3,&json!({"asset":a.to_db()}))).map(|_|()),
            "post_source"=>f.book.write(|tx|doc::post_source(tx,f.user,c,S1,"2026-10-30","Source",lines())).map(|_|()),
            "reverse_source"=>f.book.write(|tx|doc::reverse_source(tx,f.user,c,S1,"2026-10-31","Correction")).map(|_|()),
            _=>unreachable!()
        };
        let error = result.unwrap_err();
        assert!(
            error.message().contains("injected late audit failure"),
            "{action}: {error}"
        );
        assert_eq!(
            dump(f.book.connection()),
            before,
            "{action} leaked partial work"
        );
    }
}
