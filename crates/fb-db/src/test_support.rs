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

/// The only data added by 0185: unconfigured roles derived from existing staff flags.
pub fn desktop_user_backfill(conn: &Connection) -> rusqlite::Result<Vec<Vec<Value>>> {
    conn.prepare("SELECT id,CASE WHEN is_staff=1 OR is_superuser=1 THEN 'administrator' ELSE 'bookkeeper' END,NULL FROM fb_user ORDER BY id")?
        .query_map([], |r| Ok(vec![r.get(0)?, r.get(1)?, r.get(2)?]))?.collect()
}

pub fn assert_desktop_user_backfill(conn: &Connection) {
    let actual: Vec<Vec<Value>> = conn.prepare("SELECT user_id,role,password_hash FROM desktop_user ORDER BY user_id").unwrap()
        .query_map([], |r| Ok(vec![r.get(0)?, r.get(1)?, r.get(2)?])).unwrap().collect::<Result<_,_>>().unwrap();
    assert_eq!(actual, desktop_user_backfill(conn).unwrap(), "upgrade must derive every role and leave every password unconfigured");
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
    conn.execute_batch("BEGIN IMMEDIATE")?;
    for (_, sql) in &migrations {
        conn.execute_batch(sql)?;
    }
    conn.execute_batch("COMMIT")?;
    let mut expected = snapshot(source)?;
    let prior_empty = snapshot(&conn)?;
    if !prior_empty.contains_key("desktop_user") {
        // Old fixtures may reproduce an unconfigured roster, never discard a configured password.
        let has_desktop: bool=source.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='desktop_user')",[],|r|r.get(0))?;
        let enrolled: i64 = if has_desktop { source.query_row("SELECT count(*) FROM desktop_user WHERE password_hash IS NOT NULL",[],|r|r.get(0))? } else { 0 };
        if enrolled != 0 { return Err(DbError::Integrity(vec!["cannot discard enrolled desktop users".into()])); }
        let backfill = source.prepare("SELECT id,CASE WHEN is_staff=1 OR is_superuser=1 THEN 'administrator' ELSE 'bookkeeper' END,NULL FROM fb_user ORDER BY id")?
            .query_map([],|r|Ok(vec![r.get(0)?,r.get(1)?,r.get(2)?]))?.collect::<Result<Vec<_>,_>>()?;
        expected.insert("desktop_user".into(),backfill);
    }
    for (table, rows) in &expected {
        if !prior_empty.contains_key(table)
            && !rows.is_empty()
            && !matches!(table.as_str(), "sales_seal" | "sales_seal_due" | "desktop_user")
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

/// Synthetic tests: register normal read callbacks on an encrypted snapshot connection.
pub fn snapshot_file(path:&Path,key:&Key)->Result<Snapshot,DbError> {
 let conn=Connection::open(path)?; conn.execute_batch(&format!("PRAGMA key=\"x'{}'\"",hex(key)))?; Book::configure(&conn)?; Ok(snapshot(&conn)?)
}

/// One migrated, checked, empty book per key and test process, held in memory as file bytes.
/// The bytes come from the real `Book::create`, so a copy is the same schema, rows and pragmas
/// a fresh creation gives; only the SQLCipher salt is shared between copies.
fn template_bytes(key: &Key) -> Result<Arc<Vec<u8>>, DbError> {
    static TEMPLATES: OnceLock<std::sync::Mutex<BTreeMap<Key, Arc<Vec<u8>>>>> = OnceLock::new();
    let mut templates = TEMPLATES.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(bytes) = templates.get(key) { return Ok(bytes.clone()); }
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("fb-book-template-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    std::fs::create_dir(&dir).map_err(DbError::Io)?;
    let built = (|| {
        let path = dir.join("template.fbbook");
        let book = Book::create(&path, key)?;
        // Fold the WAL into the main file so the bytes alone are the whole book.
        book.connection().query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get::<_, i64>(0))?;
        drop(book);
        let wal = dir.join("template.fbbook-wal");
        if std::fs::metadata(&wal).is_ok_and(|m| m.len() != 0) {
            return Err(DbError::Pragma("template book left a non-empty WAL".into()));
        }
        std::fs::read(&path).map_err(DbError::Io)
    })();
    let _ = std::fs::remove_dir_all(&dir);
    let bytes = Arc::new(built?);
    templates.insert(*key, bytes.clone());
    Ok(bytes)
}

/// A new, fully migrated book at `path` for a test that needs a book rather than its creation.
/// It copies a per-process template made by the real `Book::create` with the same key, then opens
/// it through `Book::open`, so every open-time check (schema hash, guard triggers, integrity) runs
/// on the copy. Tests of creation, migration or upgrade must keep calling `Book::create`.
/// Like `Book::create`, it refuses a path that already exists.
///
/// The copy and every file FreedomBooks or SQLite keeps beside it are deleted when the thread that
/// made it ends: libtest runs each test on its own thread, so a test can drop its fixture and still
/// reopen (or prove it cannot reopen) the same file before it finishes.
pub fn template_book(path: &Path, key: &Key) -> Result<Book, DbError> {
    use std::io::Write;
    let bytes = template_bytes(key)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
    let mut file = options.open(path).map_err(|e| if e.kind() == std::io::ErrorKind::AlreadyExists {
        DbError::Pragma(format!("{} already exists", path.display()))
    } else { DbError::Io(e) })?;
    remove_when_test_ends(path);
    file.write_all(&bytes).and_then(|()| file.sync_all()).map_err(DbError::Io)?;
    drop(file);
    Book::open(path, key)
}

/// Files a test thread made, deleted when that thread ends (thread-local destructors run on exit,
/// after a panic too). Should a test ever run on the process's main thread, whose thread-locals are not
/// destroyed at exit, its files are left to the gate's private temp folder.
struct ThreadFiles(std::cell::RefCell<Vec<std::path::PathBuf>>);
impl Drop for ThreadFiles {
    fn drop(&mut self) {
        for path in self.0.borrow_mut().drain(..) { remove_book_files(&path); }
    }
}
thread_local! { static THREAD_FILES: ThreadFiles = const { ThreadFiles(std::cell::RefCell::new(Vec::new())) }; }

fn remove_when_test_ends(path: &Path) {
    let path = path.to_path_buf();
    // During thread teardown the list may already be gone; then delete nothing rather than panic.
    let _ = THREAD_FILES.try_with(|files| files.0.borrow_mut().push(path));
}

/// Delete a book and every file FreedomBooks or SQLite keeps beside it.
pub fn remove_book_files(path: &Path) {
    for suffix in ["", "-wal", "-shm", "-journal", ".lock", ".rekey.json"] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        let _ = std::fs::remove_file(std::path::PathBuf::from(name));
    }
}

/// A unique book path in the system temp folder for one test. [`TempBookPath::book`] makes a template
/// copy there, removed with its side files when the test's thread ends (see [`template_book`]).
pub struct TempBookPath { path: std::path::PathBuf }

impl TempBookPath {
    pub fn new(tag: &str) -> TempBookPath {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let name = format!("fb-test-{tag}-{}-{}.fbbook", std::process::id(), N.fetch_add(1, Ordering::SeqCst));
        TempBookPath { path: std::env::temp_dir().join(name) }
    }
    pub fn path(&self) -> &Path { &self.path }
    /// A template copy at this path (see [`template_book`]).
    pub fn book(&self, key: &Key) -> Book { template_book(&self.path, key).expect("template book") }
}

/// `SELECT * FROM table` ordered by every column, so a dump has one order whatever the table's width
/// (a one-column table has no second sort key) and rows equal in their first columns still sort.
pub fn select_all_ordered(conn: &Connection, table: &str) -> rusqlite::Result<String> {
    let width = conn.prepare(&format!("SELECT * FROM {table}"))?.column_count();
    let keys = (1..=width).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    Ok(format!("SELECT * FROM {table} ORDER BY {keys}"))
}

/// The case count of a seeded fuzz loop. `scripts/gate.sh` (the release gate, no flag) sets
/// `FB_TEST_FULL_CASES=1` and every loop runs its full `full` count; otherwise, as in the quick gate and
/// in focused `cargo test` runs, it runs the first `quick` cases of the same seeded sequence.
pub fn seeded_cases(full: usize, quick: usize) -> usize {
    if full_cases() { full } else { quick.min(full) }
}

/// True when `FB_TEST_FULL_CASES=1`: the release gate's full seeded case counts.
pub fn full_cases() -> bool {
    std::env::var("FB_TEST_FULL_CASES").is_ok_and(|v| v == "1")
}

/// The populated primary-key attack of `fb-core/tests/identity_permanence.rs`, for any tables, made
/// stricter: on the first row of each (which must exist), every primary-key column is moved by `UPDATE`
/// and by `UPDATE OR REPLACE`, with recursive triggers on and off, to
/// - a fresh value of the column's own type (a type check cannot stand in for the guard);
/// - when the column references a parent, a parent no row of this table uses yet (a foreign-key refusal
///   cannot stand in for the guard);
/// - another row's value of that column (the replacement collision that deletes the other row).
///
/// Each statement runs in a savepoint that is rolled back. A refusal must be a constraint and leave every
/// table unchanged; the returned list names every statement the database accepted (empty when every
/// identity is permanent).
pub fn primary_key_changes_accepted(conn: &Connection, tables: &[&str]) -> Vec<String> {
    let before = snapshot(conn).unwrap();
    let mut accepted = Vec::new();
    let literal = |v: &Value| match v {
        Value::Integer(i) => i.to_string(),
        Value::Text(t) => format!("'{}'", t.replace('\'', "''")),
        other => panic!("unexpected key value {other:?}"),
    };
    for recursion in [true, false] {
        conn.execute_batch(&format!("PRAGMA recursive_triggers={}", i32::from(recursion))).unwrap();
        for table in tables {
            let keys: Vec<String> = conn.prepare(&format!("SELECT name FROM pragma_table_info('{table}') WHERE pk>0 ORDER BY pk")).unwrap()
                .query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
            assert!(!keys.is_empty(), "{table} has no primary key");
            let columns = keys.join(",");
            let values: Vec<Value> = conn
                .query_row(&format!("SELECT {columns} FROM {table} ORDER BY {columns} LIMIT 1"), [], |r| (0..keys.len()).map(|i| r.get(i)).collect())
                .unwrap_or_else(|e| panic!("{table} must hold a row for the identity attack: {e}"));
            let predicate = keys.iter().zip(&values).map(|(k, v)| format!("{k}={}", literal(v))).collect::<Vec<_>>().join(" AND ");
            for (key, value) in keys.iter().zip(&values) {
                let mut targets = vec![match value {
                    Value::Integer(i) => (i + 1_000_003).to_string(),
                    _ => format!("'{}'", fb_ids_free_uuid()),
                }];
                // A parent this table does not use yet.
                let parent: Option<(String, Option<String>)> = conn
                    .query_row(&format!("SELECT \"table\", \"to\" FROM pragma_foreign_key_list('{table}') WHERE \"from\"='{key}'"), [], |r| Ok((r.get(0)?, r.get(1)?)))
                    .ok();
                if let Some((parent, to)) = parent {
                    let to = to.unwrap_or_else(|| conn.query_row(&format!("SELECT name FROM pragma_table_info('{parent}') WHERE pk=1"), [], |r| r.get(0)).unwrap());
                    let unused: Option<Value> = conn
                        .query_row(&format!("SELECT {to} FROM {parent} WHERE {to} IS NOT NULL AND {to} NOT IN (SELECT {key} FROM {table} WHERE {key} IS NOT NULL) ORDER BY {to} LIMIT 1"), [], |r| r.get(0))
                        .ok();
                    targets.extend(unused.as_ref().map(literal));
                }
                // Another row's value: UPDATE OR REPLACE would delete that row.
                let other: Option<Value> = conn
                    .query_row(&format!("SELECT {key} FROM {table} WHERE {key} IS NOT {} ORDER BY {key} LIMIT 1", literal(value)), [], |r| r.get(0))
                    .ok();
                targets.extend(other.as_ref().map(literal));
                for target in &targets {
                    for operation in ["UPDATE", "UPDATE OR REPLACE"] {
                        let sql = format!("{operation} {table} SET {key}={target} WHERE {predicate}");
                        conn.execute_batch("SAVEPOINT identity_case").unwrap();
                        match conn.execute_batch(&sql) {
                            Ok(()) => accepted.push(format!("recursion={recursion}: {sql}")),
                            Err(e) => {
                                assert!(crate::is_constraint(&e), "{sql}: {e}");
                                assert_eq!(snapshot(conn).unwrap(), before, "{sql}");
                            }
                        }
                        conn.execute_batch("ROLLBACK TO identity_case; RELEASE identity_case").unwrap();
                    }
                }
            }
        }
    }
    conn.execute_batch("PRAGMA recursive_triggers=ON").unwrap();
    accepted
}

/// A fresh, never-used version-4 UUID in canonical text form (deterministic, per process).
fn fb_ids_free_uuid() -> String {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    format!("0f1e2d3c-4b5a-4968-8776-{:012x}", N.fetch_add(1, Ordering::SeqCst))
}

impl PriorCopy {
    /// The rows of `expected` for the tables that existed at the copy's boundary, so a test can compare
    /// them with `before` cell for cell. The tables later migrations added are derived from the two
    /// schemas rather than listed: each must be absent from `before` and empty in `expected`, except
    /// `desktop_user`, whose rows 0185 derives from `fb_user` (checked against that derivation).
    /// `known` names later tables the caller relies on, so a boundary that silently stops adding them fails.
    /// (`populated_copy` already refuses to discard any later table holding rows.)
    pub fn expected_at_boundary(&self, source: &Connection, known: &[&str]) -> Snapshot {
        let later: Vec<&String> = self.expected.keys().filter(|t| !self.before.contains_key(*t)).collect();
        for table in known {
            assert!(later.iter().any(|t| t == table), "{table} must be one of the tables added after the boundary");
        }
        assert!(self.before.keys().all(|t| self.expected.contains_key(t)), "no table at the boundary may disappear");
        let mut at_boundary = self.expected.clone();
        for table in later {
            let rows = at_boundary.remove(table).unwrap();
            if table == "desktop_user" {
                assert_eq!(rows, desktop_user_backfill(source).unwrap(), "0185 derives every role from fb_user");
            } else {
                assert!(rows.is_empty(), "the later {table} table must be empty");
            }
        }
        at_boundary
    }
}
