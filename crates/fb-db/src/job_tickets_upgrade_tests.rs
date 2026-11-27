//! Migration 0205 (job tickets and time entries): a book at the previous schema (book format 91) upgrades after a
//! verified backup, keeps every row and prior object, adds exactly the job-ticket objects, authenticates and
//! reopens. The SQL guards hold on their own, without the core service in front of them.
use super::*;
use rusqlite::params;
use std::collections::BTreeSet;

/// Tables 0205 adds (later upgrade tests count them).
pub(crate) const JOB_TICKET_TABLES: usize = 4;
/// Every schema object 0205 adds. `view:fb_value_invalid` is replaced, keeping its prior text before the new arms.
pub(crate) const JOB_TICKET_OBJECTS: &[&str] = &[
    "table:jobs_ticket", "table:jobs_timeentry", "table:jobs_billing", "table:jobs_billingstep",
    "index:jobs_ticket_customer", "index:jobs_timeentry_ticket", "index:jobs_timeentry_customer",
    "index:jobs_billing_entry", "index:jobs_billing_invoice", "index:jobs_billingstep_invoice",
    "view:jobs_billingline", "view:jobs_livebilling", "view:jobs_invoice_mismatch", "view:jobs_invalid",
    "trigger:jobs_ticket_insert", "trigger:jobs_ticket_update", "trigger:jobs_ticket_delete",
    "trigger:jobs_timeentry_insert", "trigger:jobs_timeentry_update", "trigger:jobs_timeentry_delete",
    "trigger:jobs_billing_insert", "trigger:jobs_billing_update", "trigger:jobs_billing_delete",
    "trigger:jobs_billingstep_insert", "trigger:jobs_billingstep_update", "trigger:jobs_billingstep_delete",
    "trigger:jobs_invoice_seal", "trigger:jobs_item_kind",
    "trigger:fb_valid_jobs_ticket_insert", "trigger:fb_valid_jobs_ticket_update",
    "trigger:fb_valid_jobs_timeentry_insert", "trigger:fb_valid_jobs_timeentry_update",
    "trigger:fb_valid_jobs_billing_insert", "trigger:fb_valid_jobs_billing_update",
    "trigger:fb_valid_jobs_billingstep_insert", "trigger:fb_valid_jobs_billingstep_update",
];

const CO: &str = "00000000-0000-7000-8000-0000000002c1";
const OTHER: &str = "00000000-0000-7000-8000-0000000002c9";
const USER: &str = "00000000-0000-7000-8000-0000000002c2";
const CUSTOMER: &str = "00000000-0000-7000-8000-0000000002c3";
const ITEM: &str = "00000000-0000-7000-8000-0000000002c4";
const GOODS: &str = "00000000-0000-7000-8000-0000000002c5";
const FOREIGN: &str = "00000000-0000-7000-8000-0000000002c6";
const TS: &str = "2026-11-15T10:00:00.000000Z";

fn boundary() -> usize {
    let n = MIGRATIONS.iter().position(|(name, _)| *name == "0205_job_tickets").unwrap();
    assert_eq!(n, 91);
    assert_eq!(MIGRATIONS[n - 1].0, "0204_recurring_bills");
    n
}

fn prior_book(path: &Path, key: &Key) -> (test_support::Snapshot, SchemaHashes, String) {
    let n = boundary();
    let q = Connection::open(path).unwrap();
    q.execute_batch(&format!("PRAGMA key=\"x'{}'\"", hex(key))).unwrap();
    Book::configure(&q).unwrap();
    q.execute_batch("BEGIN IMMEDIATE").unwrap();
    for (_, sql) in &MIGRATIONS[..n] {
        q.execute_batch(sql).unwrap();
    }
    q.execute_batch(&format!("PRAGMA user_version={n}")).unwrap();
    q.execute("INSERT INTO fb_user(id,username) VALUES(?1,'Synthetic technician')", [USER]).unwrap();
    for (id, name) in [(CO, "Synthetic service company"), (OTHER, "Synthetic other company")] {
        q.execute("INSERT INTO ledger_company(id,name,currency) VALUES(?1,?2,'USD')", params![id, name]).unwrap();
        q.execute("INSERT INTO ledger_company_members(company_id,user_id) VALUES(?1,?2)", params![id, USER]).unwrap();
    }
    q.execute("INSERT INTO directory_customer(id,company_id,display_name) VALUES(?1,?2,'Harbor Lane Tavern'),(?3,?4,'Elsewhere Diner')", params![CUSTOMER, CO, FOREIGN, OTHER]).unwrap();
    q.execute("INSERT INTO directory_item(id,company_id,code,kind,sale_rate,taxable) VALUES(?1,?2,'On-Site Support','SERVICE',12500,0),(?3,?2,'Router','NONSTOCK',9900,1)", params![ITEM, CO, GOODS]).unwrap();
    q.execute_batch("COMMIT").unwrap();
    let before = test_support::snapshot(&q).unwrap();
    let schema = schema_hashes(&q).unwrap();
    let view: String = q.query_row("SELECT sql FROM sqlite_schema WHERE name='fb_value_invalid'", [], |r| r.get(0)).unwrap();
    (before, schema, view)
}

#[test]
fn a_format_91_book_is_backed_up_upgraded_and_reopens_with_only_the_job_ticket_objects_added() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("format-91.fbbook");
    let key = [91; 32];
    let (before, schema, view) = prior_book(&path, &key);
    let old = match Book::open_staged(&path, &key).unwrap() {
        Opened::Behind(old) => old,
        Opened::Current(_) => panic!("expected the format-91 schema"),
    };
    assert_eq!((old.from(), old.to()), (91, 97)); // 0205 through 0210
    let backup = dir.path().join("verified-format-91.fbbook");
    old.snapshot(&backup, &key).unwrap();
    assert_eq!(test_support::snapshot_file(&backup, &key).unwrap(), before);
    let book = old.upgrade().unwrap();
    let after = test_support::snapshot(&book.conn).unwrap();
    for (table, rows) in &before {
        assert_eq!(&after[table], rows, "upgrade changed {table}");
    }
    assert!(after["remote_host"].is_empty());
    assert!(after["desktop_ai_unattended"].is_empty());
    for table in ["jobs_ticket", "jobs_timeentry", "jobs_billing", "jobs_billingstep"] {
        assert!(after[table].is_empty(), "{table} starts empty");
    }
    let upgraded = schema_hashes(&book.conn).unwrap();
    for (object, hash) in &schema {
        if !["view:fb_value_invalid", "table:bankfeeds_account", "table:bankfeeds_transaction"].contains(&object.as_str())
            && !crate::deposit_adjustments_upgrade_tests::CHANGED.contains(&object.as_str())
            && !crate::bankfeeds_matching_upgrade_tests::CHANGED.contains(&object.as_str())
        {
            assert_eq!(&upgraded[object], hash, "changed prior object {object}");
        }
    }
    let added: BTreeSet<&str> = upgraded.keys().filter(|k| !schema.contains_key(*k)).map(String::as_str)
        .filter(|k| !crate::remote_host_upgrade_tests::REMOTE_HOST_OBJECTS.contains(k)
            && !crate::ai_unattended_upgrade_tests::AI_UNATTENDED_OBJECTS.contains(k)
            && !crate::deposit_adjustments_upgrade_tests::OBJECTS.contains(k)
            && !crate::bankfeeds_matching_upgrade_tests::OBJECTS.contains(k)).collect();
    assert_eq!(added, JOB_TICKET_OBJECTS.iter().copied().collect::<BTreeSet<_>>());
    // The shared value view keeps its prior text and gains the four job arms before the AI arm.
    let next: String = book.conn.query_row("SELECT sql FROM sqlite_schema WHERE name='fb_value_invalid'", [], |r| r.get(0)).unwrap();
    let (kept, tail) = next.split_once("\nUNION ALL\nSELECT 'invalid stored value in jobs_billing'").unwrap();
    assert_eq!(kept, view.trim_end_matches(';'));
    assert_eq!(tail.matches("invalid stored value in jobs_").count(), 3);
    let fresh = Book::create(&dir.path().join("fresh.fbbook"), &key).unwrap();
    assert_eq!(upgraded, schema_hashes(&fresh.conn).unwrap());
    assert!(book.check().unwrap().is_empty());
    drop(book);
    let reopened = Book::open(&path, &key).unwrap();
    assert_eq!(test_support::snapshot(&reopened.conn).unwrap(), after);
    assert!(reopened.check().unwrap().is_empty());
}

fn ticket(conn: &Connection, id: &str, ordinal: i64, customer: &str, company: &str, status: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT INTO jobs_ticket(id,company_id,ordinal,number,request_key,customer_id,title,description,status,opened_date,finished_date,revision,created_by_id,created_at,updated_at)
         VALUES(?1,?2,?3,'JT-'||printf('%06d',?3),?4,?5,'Network outage','',?6,'2026-11-15',CASE WHEN ?6='OPEN' THEN NULL ELSE '2026-11-15' END,1,?7,?8,?8)",
        params![id, company, ordinal, id.replace("-8000-", "-9000-"), customer, status, USER, TS],
    )
}
fn entry(conn: &Connection, id: &str, ticket: Option<&str>, item: &str, hours: i64, rate: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT INTO jobs_timeentry(id,company_id,request_key,customer_id,ticket_id,item_id,date,hours,rate,amount,rate_override,billable,notes,worker_id,revision,created_by_id,created_at,updated_at)
         VALUES(?1,?2,?3,?4,?5,?6,'2026-11-15',?7,?8,(?7*?8+5000)/10000,0,1,'',?9,1,?9,?10,?10)",
        params![id, CO, id.replace("-8000-", "-a000-"), CUSTOMER, ticket, item, hours, rate, USER, TS],
    )
}
fn refused(result: rusqlite::Result<usize>, constraint: &str) {
    let error = result.expect_err("refused");
    assert!(error.to_string().contains(constraint), "{constraint}: {error}");
}

#[test]
fn the_schema_alone_keeps_tickets_and_time_in_scope_and_history_permanent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("guards.fbbook");
    let key = [92; 32];
    prior_book(&path, &key);
    let book = match Book::open_staged(&path, &key).unwrap() { Opened::Behind(old) => old.upgrade().unwrap(), _ => panic!() };
    let conn = &book.conn;
    conn.execute_batch("BEGIN").unwrap();
    let t1 = "00000000-0000-7000-8000-000000000301";
    // Numbers are allocated in order and match the ordinal; the customer is from the company.
    refused(ticket(conn, t1, 2, CUSTOMER, CO, "OPEN"), "jobs_ticket_number");
    refused(ticket(conn, t1, 1, FOREIGN, CO, "OPEN"), "jobs_ticket_scope");
    ticket(conn, t1, 1, CUSTOMER, CO, "OPEN").unwrap();
    refused(conn.execute("UPDATE jobs_ticket SET number='JT-000009' WHERE id=?1", [t1]), "jobs_ticket_identity");
    refused(conn.execute("UPDATE jobs_ticket SET customer_id=?2, revision=2 WHERE id=?1", params![t1, FOREIGN]), "jobs_ticket_identity");
    refused(conn.execute("UPDATE jobs_ticket SET title='x', revision=5 WHERE id=?1", [t1]), "jobs_ticket_revision");
    refused(conn.execute("DELETE FROM jobs_ticket WHERE id=?1", [t1]), "jobs_ticket_kept");
    // Hours are exact ten-thousandths; the amount is the invoice line rule; only service items.
    let e1 = "00000000-0000-7000-8000-000000000311";
    refused(entry(conn, e1, Some(t1), GOODS, 35000, 12500), "jobs_entry_scope");
    refused(conn.execute(
        "INSERT INTO jobs_timeentry(id,company_id,request_key,customer_id,item_id,date,hours,rate,amount,rate_override,billable,notes,worker_id,revision,created_by_id,created_at,updated_at)
         VALUES(?1,?2,?3,?4,?5,'2026-11-15',35000,12500,43751,0,1,'',?6,1,?6,?7,?7)",
        params![e1, CO, "00000000-0000-7000-8000-0000000003e1", CUSTOMER, ITEM, USER, TS]), "CHECK");
    entry(conn, e1, Some(t1), ITEM, 35000, 12500).unwrap();
    let amount: i64 = conn.query_row("SELECT amount FROM jobs_timeentry WHERE id=?1", [e1], |r| r.get(0)).unwrap();
    assert_eq!(amount, 43750, "3.5 h at 125.00 is 437.50");
    // Time joins only an open ticket of its own customer.
    let t2 = "00000000-0000-7000-8000-000000000302";
    ticket(conn, t2, 2, CUSTOMER, CO, "COMPLETED").unwrap();
    refused(conn.execute("UPDATE jobs_timeentry SET ticket_id=?2, revision=2 WHERE id=?1", params![e1, t2]), "jobs_entry_ticket");
    let t3 = "00000000-0000-7000-8000-000000000303";
    refused(ticket(conn, t3, 1, FOREIGN, OTHER, "OPEN").and_then(|_| conn.execute("UPDATE jobs_timeentry SET ticket_id=?2, revision=2 WHERE id=?1", params![e1, t3])), "jobs_entry_ticket");
    // A service with recorded time stays a service; an item without time may change kind.
    refused(conn.execute("UPDATE directory_item SET kind='NONSTOCK' WHERE id=?1", [ITEM]), "jobs_item_kind");
    conn.execute("UPDATE directory_item SET kind='SERVICE' WHERE id=?1", [GOODS]).unwrap();
    conn.execute("UPDATE directory_item SET kind='NONSTOCK' WHERE id=?1", [GOODS]).unwrap();
    // A NUL in free text is refused by the shared value guard, and flagged on open if written around it.
    refused(conn.execute("UPDATE jobs_timeentry SET notes='a'||char(0), revision=2 WHERE id=?1", [e1]), "fb_valid");
    // An entry without billing history may be removed; billing history itself is append-only.
    conn.execute("DELETE FROM jobs_timeentry WHERE id=?1", [e1]).unwrap();
    conn.execute("UPDATE directory_item SET kind='NONSTOCK' WHERE id=?1", [ITEM]).unwrap();
    // Billing steps follow only live billings, for the invoice revision being written.
    refused(conn.execute("INSERT INTO jobs_billingstep(id,company_id,billing_id,invoice_id,revision,line_position,created_by_id,created_at) VALUES(?1,?2,?1,?1,2,NULL,?3,?4)",
        params!["00000000-0000-7000-8000-000000000321", CO, USER, TS]), "jobs_billing_step");
    assert!(conn.query_row("SELECT count(*) FROM jobs_invalid", [], |r| r.get::<_, i64>(0)).unwrap() == 0);
    conn.execute_batch("ROLLBACK").unwrap();
}
