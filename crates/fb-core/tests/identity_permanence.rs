//! Source identities must survive ordinary writes as well as replacement collisions.
mod common;
use common::*;
use fb_core::{Book, UserId, services, users};
use fb_db::rusqlite::{self, Connection, types::Value};

const TABLES: &[&str] = &[
    "fb_user",
    "desktop_user",
    "ledger_company",
    "ledger_account",
    "ledger_journal",
    "ledger_journalline",
    "ledger_auditevent",
    "ledger_company_members",
    "fb_seal",
    "fb_seal_due",
];

fn snapshot(conn: &Connection) -> Vec<Vec<Vec<Value>>> {
    TABLES
        .iter()
        .map(|table| {
            let mut stmt = conn
                .prepare(&format!("SELECT * FROM {table} ORDER BY 1,2"))
                .unwrap();
            let width = stmt.column_count();
            stmt.query_map([], |r| (0..width).map(|i| r.get(i)).collect())
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        })
        .collect()
}

#[test]
fn referenced_operator_cannot_be_replaced_by_rekey_then_reinsert() {
    let path = book_path("operator-rekey");
    let mut book = Book::create(&path, &KEY).unwrap();
    let original = users::create_user(&mut book, "original-operator", false).unwrap();
    let mut fx = Fx {
        book,
        user: original,
    };
    let (company, a, b) = fx.chart();
    let journal = fx.draft(company, a, b, "10", "2026-01-01");
    fx.post(company, journal.id);
    let before = snapshot(fx.book.connection());
    let result: rusqlite::Result<()> = fx.book.write(|w| {
        w.execute(
            "UPDATE fb_user SET id=?1 WHERE id=?2",
            [UserId::new().to_db(), original.to_db()],
        )?;
        w.execute(
            "INSERT INTO fb_user(id,username) VALUES(?1,'replacement-operator')",
            [original.to_db()],
        )?;
        Ok(())
    });
    assert!(
        result.is_err(),
        "referenced operator replacement must be refused"
    );
    assert_eq!(snapshot(fx.book.connection()), before);
    fx.book.read(|r| -> rusqlite::Result<()> {
        let actor: String = r.query_row("SELECT u.username FROM ledger_journal j JOIN fb_user u ON u.id=j.posted_by_id WHERE j.id=?1", [journal.id.to_db()], |r| r.get(0))?;
        assert_eq!(actor, "original-operator");
        Ok(())
    }).unwrap();
    assert_eq!(
        fx.book
            .read(|r| services::access(r, original, company))
            .unwrap()
            .username,
        "original-operator"
    );
    assert!(fx.book.check().unwrap().is_empty());
    drop(fx);
    let reopened = Book::open(&path, &KEY).unwrap();
    assert_eq!(snapshot(reopened.connection()), before);
}

#[test]
fn every_protected_primary_key_is_permanent_even_without_references() {
    let mut fx = Fx::new("primary-key-matrix");
    let (company, a, b) = fx.chart();
    fx.draft(company, a, b, "10", "2026-01-01");
    users::create_user(&mut fx.book, "unused-operator", false).unwrap();
    fx.account(company, "unused", "Unused", "ASSET");
    fx.book.write(|w| -> rusqlite::Result<()> {
        w.execute_batch("INSERT INTO fb_seal VALUES('matrix','original'); INSERT INTO fb_seal_due VALUES('matrix','original')")
    }).unwrap();
    fx.book.write(|w| w.execute("INSERT INTO desktop_user(user_id,role) VALUES(?1,'bookkeeper')", [fx.user.to_db()])).unwrap();
    let actual: Vec<String> = fx.book.connection().prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT GLOB 'sqlite_*' ORDER BY name")
        .unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
    let mut expected = TABLES.iter().chain([
        "attachments_attachment",
        "directory_customer", "directory_vendor", "directory_item",
        "salestax_taxitem", "salestax_taxgroupmember", "salestax_salestaxsettings",
        "documents_documentsequence", "documents_documentidentity", "documents_documentaccount",
        "documents_accountingsetup", "documents_designatedbank",
        // Each later layer owns its populated primary-key/ownership matrix.
        // See sales/rebase_tests.rs and payables/rebase_tests.rs.
        // D4 populated key/REPLACE matrix: sales/estimate_tests.rs.
        "sales_estimate", "sales_estimaterevision", "sales_estimateline",
        "sales_estimateconversion", "estimate_seal", "estimate_seal_due",
        "sales_invoice", "sales_invoicerevision", "sales_invoiceline",
        "sales_receipt", "sales_receiptallocation", "sales_receiptreversal",
        "sales_salesreceipt", "sales_salesreceiptline", "sales_seal", "sales_seal_due",
        "payables_bill", "payables_billrevision", "payables_billline",
        "payables_billpayment", "payables_paymentallocation", "payables_paymentreversal",
        // checks/guards_tests.rs owns the populated checks/transfers matrix.
        "checks_check", "checks_checkrevision", "checks_checkline",
        "transfers_transfer", "transfers_transfervoid",
        // Populated ownership/key attacks: prepayments/guard_tests.rs.
        "prepayments_customerdeposit", "prepayments_depositapplication",
        "prepayments_depositunapplication", "prepayments_depositrefund",
        "prepayments_depositrefundvoid", "prepayments_seal", "prepayments_due",
        // Populated credits identity/replacement matrix: credits/tests.rs.
        "credits_creditmemo", "credits_creditmemoline", "credits_creditmemovoid",
        "credits_creditapplication", "credits_creditapplicationline", "credits_creditunapplication",
        "credits_creditrefund", "credits_creditrefundvoid", "credits_seal", "credits_seal_due",
        // Assistant's populated matrix lives in assistant/tests.rs.
        "assistant_connection", "assistant_reading",
        // Banking's populated matrix lives in banking/{tests,matrix_tests}.rs.
        "banking_deposit", "banking_depositline", "banking_depositreversal",
        "banking_reconciliation", "banking_banklinestatus", "banking_reconciliationline",
        "banking_seal", "banking_seal_due", "banking_reconciliationactor",
        // Populated singleton/owner-parent identity matrix: tests/accounts.rs.
        "accounts_owner",
        // Bank feeds' populated key/identity attacks: bankfeeds/tests.rs.
        "bankfeeds_connection", "bankfeeds_account", "bankfeeds_transaction", "bankfeeds_rule", "bankfeeds_counter",
        "bankfeeds_retired",
        // FIX5: the one permanent identity secret (bankfeeds/tests.rs fix5_identity_keys_*).
        "bankfeeds_identity_secret",
        // Vault scope/AAD tamper matrix: fb-app/tests/credential_store.rs.
        "plugin_credential",
        // Populated permanent singleton, REPLACE and ownership matrix: tests/company_documents.rs.
        "company_document_details",
        // Populated numbering composite-key/replacement matrix: tests/numbering.rs.
        "fb_numbering",
        // Mutable self-service MFA enrollment and single-use recovery: fb-app/tests/desktop_mfa.rs.
        "desktop_mfa_totp", "desktop_mfa_recovery",
    ].iter()).map(|s| s.to_string()).collect::<Vec<_>>();
    expected.sort();
    assert_eq!(
        actual, expected,
        "extend the identity matrix for every new table"
    );
    let before = snapshot(fx.book.connection());
    let mut accepted = Vec::new();
    for recursion in [true, false] {
        fx.book
            .connection()
            .execute_batch(&format!(
                "PRAGMA recursive_triggers={}",
                i32::from(recursion)
            ))
            .unwrap();
        for table in TABLES {
            let keys: Vec<String> = fx
                .book
                .connection()
                .prepare(&format!(
                    "SELECT name FROM pragma_table_info('{table}') WHERE pk>0 ORDER BY pk"
                ))
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert!(!keys.is_empty(), "{table}");
            let predicate = if *table == "fb_user" {
                " WHERE username='unused-operator'".to_string()
            } else if *table == "ledger_account" {
                " WHERE code='unused'".to_string()
            } else {
                let columns = keys.join(",");
                let values: Vec<String> = fx
                    .book
                    .connection()
                    .query_row(
                        &format!("SELECT {columns} FROM {table} ORDER BY {columns} LIMIT 1"),
                        [],
                        |r| (0..keys.len()).map(|i| r.get(i)).collect(),
                    )
                    .unwrap();
                format!(
                    " WHERE {}",
                    keys.iter()
                        .zip(values)
                        .map(|(k, v)| format!("{k}='{}'", v.replace('\'', "''")))
                        .collect::<Vec<_>>()
                        .join(" AND ")
                )
            };
            for key in keys {
                for operation in ["UPDATE", "UPDATE OR REPLACE"] {
                    let sql = format!(
                        "{operation} {table} SET {key}='{}'{predicate}",
                        UserId::new()
                    );
                    // A savepoint restores even an incorrectly accepted statement before the next case.
                    fx.book
                        .connection()
                        .execute_batch("SAVEPOINT identity_case")
                        .unwrap();
                    match fx.book.connection().execute_batch(&sql) {
                        Ok(()) => accepted.push(format!("recursion={recursion}: {sql}")),
                        Err(e) => {
                            assert!(fb_db::is_constraint(&e), "{sql}: {e}");
                            assert_eq!(snapshot(fx.book.connection()), before, "{sql}");
                        }
                    }
                    fx.book
                        .connection()
                        .execute_batch("ROLLBACK TO identity_case; RELEASE identity_case")
                        .unwrap();
                }
            }
        }
    }
    fx.book
        .connection()
        .execute_batch("PRAGMA recursive_triggers=ON")
        .unwrap();
    assert!(
        accepted.is_empty(),
        "accepted primary-key changes: {accepted:#?}"
    );
    assert!(fx.book.check().unwrap().is_empty());
}

#[test]
fn protected_foreign_identities_cannot_move_to_fresh_or_different_parents() {
    let mut fx = Fx::new("foreign-identity-matrix");
    let (c, a, b) = fx.chart();
    let other = fx.company("Other");
    let unused = fx.account(c, "unused", "Unused", "ASSET");
    let operator = users::create_user(&mut fx.book, "other-operator", false).unwrap();
    let posted = fx.draft(c, a, b, "10", "2026-01-01");
    fx.post(c, posted.id);
    let draft = fx.draft(c, a, b, "20", "2026-01-01");
    let other_draft = fx.draft(c, a, b, "30", "2026-01-01");
    fx.book
        .write(|w| -> rusqlite::Result<()> {
            w.execute(
                "DELETE FROM ledger_journalline WHERE journal_id=?1",
                [other_draft.id.to_db()],
            )?;
            Ok(())
        })
        .unwrap();
    fx.book
        .write(|w| -> rusqlite::Result<()> {
            w.execute(
                "UPDATE ledger_journal SET entry_number='' WHERE id=?1",
                [draft.id.to_db()],
            )?;
            Ok(())
        })
        .unwrap();
    let cases = [
        (
            "ledger_account",
            "company_id",
            format!("id='{unused}'"),
            other.to_db(),
        ),
        (
            "ledger_journal",
            "company_id",
            format!("id='{}'", draft.id),
            other.to_db(),
        ),
        (
            "ledger_journal",
            "created_by_id",
            format!("id='{}'", draft.id),
            operator.to_db(),
        ),
        (
            "ledger_journal",
            "posted_by_id",
            format!("id='{}'", posted.id),
            operator.to_db(),
        ),
        (
            "ledger_journal",
            "reversal_of_id",
            format!("id='{}'", draft.id),
            posted.id.to_db(),
        ),
        (
            "ledger_journalline",
            "journal_id",
            format!("journal_id='{}' AND position=1", draft.id),
            other_draft.id.to_db(),
        ),
        (
            "ledger_journalline",
            "journal_id",
            format!("journal_id='{}'", posted.id),
            draft.id.to_db(),
        ),
        (
            "ledger_journalline",
            "account_id",
            format!("journal_id='{}'", posted.id),
            unused.to_db(),
        ),
        ("ledger_auditevent", "company_id", "1".into(), other.to_db()),
        (
            "ledger_auditevent",
            "actor_id",
            "1".into(),
            operator.to_db(),
        ),
        (
            "ledger_company_members",
            "company_id",
            format!("company_id='{c}'"),
            UserId::new().to_db(),
        ),
        (
            "ledger_company_members",
            "user_id",
            format!("company_id='{c}'"),
            operator.to_db(),
        ),
    ];
    let before = snapshot(fx.book.connection());
    let mut accepted = Vec::new();
    for (table, column, predicate, value) in cases {
        let sql = format!("UPDATE {table} SET {column}='{value}' WHERE {predicate}");
        let result: rusqlite::Result<()> = fx.book.write(|w| {
            match w.execute_batch(&sql) {
                Ok(()) => accepted.push(sql.clone()),
                Err(e) => {
                    assert!(fb_db::is_constraint(&e), "{sql}: {e}");
                    return Err(e);
                }
            }
            // Roll back deliberately even if the rule was absent; continue the inventory.
            Err(rusqlite::Error::InvalidQuery)
        });
        assert!(result.is_err());
        assert_eq!(snapshot(fx.book.connection()), before, "{sql}");
    }
    assert!(
        accepted.is_empty(),
        "accepted identity changes: {accepted:#?}"
    );
    assert!(fx.book.check().unwrap().is_empty());
}

#[test]
fn draft_content_and_operator_labels_remain_editable_with_permanent_keys() {
    let mut fx = Fx::new("editable-content");
    let (c, a, b) = fx.chart();
    let draft = fx.draft(c, a, b, "10", "2026-01-01");
    fx.book.write(|w| -> rusqlite::Result<()> {
        assert_eq!(w.execute("UPDATE fb_user SET id=id,username='renamed' WHERE id=?1", [fx.user.to_db()])?, 1);
        assert_eq!(w.execute("UPDATE ledger_account SET id=id,company_id=company_id,name='Renamed cash' WHERE id=?1", [a.to_db()])?, 1);
        assert_eq!(w.execute("UPDATE ledger_journal SET id=id,reversal_of_id=reversal_of_id,memo='Edited draft' WHERE id=?1", [draft.id.to_db()])?, 1);
        assert_eq!(w.execute("UPDATE ledger_journalline SET id=id,journal_id=journal_id,account_id=?1 WHERE journal_id=?2 AND position=1", [b.to_db(), draft.id.to_db()])?, 1);
        Ok(())
    }).unwrap();
    assert_eq!(fx.journal(draft.id).memo, "Edited draft");
    assert!(fx.book.check().unwrap().is_empty());
}
