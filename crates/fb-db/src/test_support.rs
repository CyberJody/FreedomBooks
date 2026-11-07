//! Synthetic upgrade fixtures only. Construct the real prior schema first, then
//! populate every table/column it contained; never relabel a newer table definition.
use super::*;
use rusqlite::{params_from_iter, types::Value};

pub type Snapshot = BTreeMap<String, Vec<Vec<Value>>>;
pub fn snapshot(conn: &Connection) -> rusqlite::Result<Snapshot> {
    let tables: Vec<String> = conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT GLOB 'sqlite_*' ORDER BY name")?
        .query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    tables
        .into_iter()
        .map(|name| {
            let mut statement = conn.prepare(&format!("SELECT * FROM {name}"))?;
            let columns = statement.column_count();
            let mut rows: Vec<Vec<Value>> = statement
                .query_map([], |r| (0..columns).map(|i| r.get(i)).collect())?
                .collect::<Result<_, _>>()?;
            rows.sort_by_key(|r| format!("{r:?}"));
            Ok((name, rows))
        })
        .collect()
}

/// Identity columns of migration 0136 (`(table, column, domain, text column)`).
const IDENTITY_COLUMNS: &[(&str, &str, &str, &str)] = &[
    ("bankfeeds_account", "conn_key_identity", "connection-key", "conn_key"),
    ("bankfeeds_account", "remote_identity", "account", "remote_id"),
    ("bankfeeds_transaction", "remote_identity", "transaction", "remote_id"),
];
/// A book upgraded across 0136 gets a new random identity secret, so its keys cannot equal the source
/// book's. Checks that every key is the HMAC of the stored text under the book's secret (the documented
/// backfill), then blanks the secret and the keys so everything else still compares cell for cell.
pub fn identity_blind(conn: &Connection, mut snap: Snapshot) -> rusqlite::Result<Snapshot> {
    let has_secret: i64 = conn.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='bankfeeds_identity_secret'", [], |r| r.get(0))?;
    if has_secret == 0 {
        return Ok(snap);
    }
    for (table, column, domain, text) in IDENTITY_COLUMNS {
        let wrong: i64 = conn.query_row(
            &format!("SELECT count(*) FROM {table} WHERE {column} IS NOT fb_bankfeeds_identity((SELECT secret FROM bankfeeds_identity_secret),'{domain}',{text})"),
            [],
            |r| r.get(0),
        )?;
        assert_eq!(wrong, 0, "{table}.{column} is not derived from the stored {text}");
        let index = conn.prepare(&format!("SELECT * FROM {table}"))?.column_names().position(|n| n == *column).expect("identity column");
        if let Some(rows) = snap.get_mut(*table) {
            for row in rows.iter_mut() {
                row[index] = Value::Null;
            }
            rows.sort_by_key(|r| format!("{r:?}"));
        }
    }
    snap.insert("bankfeeds_identity_secret".into(), vec![]);
    Ok(snap)
}

pub struct PriorCopy {
    pub before: Snapshot,
    pub expected: Snapshot,
}

/// `boundary` is exclusive unless `inclusive` is true; fdd2fa6 names the exact
/// frozen base layout whose ordinal list did not yet contain sales migration 0016.
pub fn populated_copy(
    source: &Connection,
    path: &Path,
    key: &[u8; 32],
    boundary: &str,
    inclusive: bool,
) -> Result<PriorCopy, DbError> {
    let conn = Connection::open(path)?;
    conn.execute_batch(&format!("PRAGMA key=\"x'{}'\"", hex(key)))?;
    Book::configure(&conn)?;
    let migrations: Vec<_> = if boundary == "fdd2fa6" {
        MIGRATIONS
            .iter()
            .filter(|(n, _)| *n != "0016_sales_review_fixes" && *n <= "0026_payables_core_adoption")
            .collect()
    } else if boundary == "a2be4d8" {
        MIGRATIONS.iter().filter(|(n,_)| !n.starts_with("005") && !n.starts_with("006") && *n <= "0075_restore_sales_receipt_destination").collect()
    } else {
        let end = MIGRATIONS
            .iter()
            .position(|(n, _)| *n == boundary)
            .expect("named schema boundary")
            + usize::from(inclusive);
        MIGRATIONS[..end].iter().collect()
    };
    for (_, sql) in &migrations {
        conn.execute_batch(sql)?;
    }
    let mut expected = snapshot(source)?;
    let prior_empty = snapshot(&conn)?;
    for (table, rows) in &expected {
        if !prior_empty.contains_key(table)
            && !rows.is_empty()
            && !matches!(table.as_str(), "sales_seal" | "sales_seal_due")
            // 0053 reconstructs actor evidence from the canonical audit trail.
            && !(table == "banking_reconciliationactor" && migrations.iter().all(|(n,_)| *n < "0053_banking_actors"))
            // 0134 retires withdrawn bank-feed identities from the audit trail.
            && !(table == "bankfeeds_retired" && migrations.iter().all(|(n,_)| *n < "0134_bankfeeds_hardening"))
            // 0136 makes a new random identity secret for a book that already holds downloaded rows.
            && !(table == "bankfeeds_identity_secret" && migrations.iter().all(|(n,_)| *n < "0136_bankfeeds_identity"))
        {
            return Err(DbError::Integrity(vec![format!(
                "cannot discard later populated table {table}"
            )]));
        }
    }
    let triggers: Vec<(String, String)> = conn
        .prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger'")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    conn.execute_batch("PRAGMA foreign_keys=OFF;BEGIN IMMEDIATE")?;
    for (name, _) in &triggers {
        conn.execute_batch(&format!("DROP TRIGGER {name}"))?;
    }
    for table in prior_empty.keys() {
        let columns: Vec<String> = conn
            .prepare(&format!("SELECT * FROM {table}"))?
            .column_names()
            .map(str::to_owned)
            .collect();
        let mut stmt = source.prepare(&format!("SELECT {} FROM {table}", columns.join(",")))?;
        let data = stmt.query_map([], |r| {
            (0..columns.len())
                .map(|i| r.get::<_, Value>(i))
                .collect::<Result<Vec<_>, _>>()
        })?;
        for row in data {
            conn.execute(
                &format!(
                    "INSERT INTO {table} VALUES({})",
                    vec!["?"; columns.len()].join(",")
                ),
                params_from_iter(row?),
            )?;
        }
    }
    // Early sales used the shared seal tables. The real 0013 copies them to the
    // sales-owned tables while preserving the original shared evidence as well.
    for (old, new) in [("fb_seal", "sales_seal"), ("fb_seal_due", "sales_seal_due")] {
        if !prior_empty.contains_key(new) {
            for row in &expected[new] {
                conn.execute(
                    &format!("INSERT INTO {old} VALUES(?,?)"),
                    params_from_iter(row),
                )?;
            }
            let extra = expected[new].clone();
            expected.get_mut(old).unwrap().extend(extra);
            expected
                .get_mut(old)
                .unwrap()
                .sort_by_key(|r| format!("{r:?}"));
        }
    }
    // The provenance column did not exist before 0027. Every other cell and key
    // is identical to the populated service-created source, including empty tables.
    if !conn
        .prepare("SELECT * FROM payables_billline")
        .ok()
        .is_some_and(|s| s.column_names().any(|n| n == "origin_position"))
        && let Some(rows) = expected.get_mut("payables_billline")
    {
        let index = source
            .prepare("SELECT * FROM payables_billline")?
            .column_names()
            .position(|n| n == "origin_position")
            .unwrap();
        for row in rows.iter_mut() {
            row[index] = Value::Null;
        }
        rows.sort_by_key(|r| format!("{r:?}"));
    }
    for (_, sql) in triggers {
        conn.execute_batch(&sql)?;
    }
    conn.execute_batch(&format!(
        "PRAGMA user_version={};COMMIT;PRAGMA foreign_keys=ON",
        migrations.len()
    ))?;
    let before = snapshot(&conn)?;
    // Prove every table at the prior boundary was copied, including zero-row tables.
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        prior_empty.keys().collect::<Vec<_>>()
    );
    Ok(PriorCopy { before, expected })
}
