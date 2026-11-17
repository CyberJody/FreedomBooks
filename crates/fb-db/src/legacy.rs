//! Exact pre-INT-L3a schema layouts. Ordinal user_version predates named history;
//! authenticate every schema object before filling the newly inserted migrations.
use super::*;

// Each layout is a complete known historical ordering. Recognition still hashes
// every installed object; an ordinal or a subset of known names never suffices.
pub(super) fn histories() -> Vec<Vec<(&'static str, &'static str)>> {
    let frozen = MIGRATIONS.iter().copied().filter(|(name,_)| {
        *name != "0016_sales_review_fixes" && *name <= "0026_payables_core_adoption"
    }).collect();
    let l3a = MIGRATIONS.iter().copied().filter(|(n,_)| !n.starts_with("005") && !n.starts_with("006") && *n <= "0075_restore_sales_receipt_destination").collect();
    // The reviewed document lane branched before numbering. Its genuine ordinal
    // history is authenticated before inserting the missing numbering migration.
    let documents = MIGRATIONS.iter().copied().filter(|(n,_)| *n != "0162_numbering" && *n <= "0183_company_document_json_ownership").collect();
    let prefix = || MIGRATIONS.iter().copied().take_while(|(name, _)| *name <= "0195_invoice_import").collect::<Vec<_>>();
    let mut purchasing = prefix();
    purchasing.push(("0196_bank_accounts_by_type", include_str!("../sql/0196_bank_accounts_by_type.sql")));
    let mut tax = prefix();
    tax.push(("0200_salestax_payments", include_str!("../sql/0200_salestax_payments.sql")));
    vec![frozen,l3a,documents,purchasing,tax]
}

pub(super) fn recognize(conn: &Connection, applied: usize) -> Result<Option<Vec<(&'static str, &'static str)>>, DbError> {
    for layout in histories() {
        if applied <= 15 || applied > layout.len() { continue; }
        let scratch = Connection::open_in_memory()?;
        Book::configure_connection(&scratch)?;
        let candidate: Vec<_> = layout.into_iter().take(applied).collect();
        for (_, sql) in &candidate { scratch.execute_batch(sql)?; }
        if schema_hashes(conn)? == schema_hashes(&scratch)? { return Ok(Some(candidate)); }
    }
    Ok(None)
}

pub(super) fn upgrade_frozen_prefix(conn: &mut Connection, applied: usize) -> Result<bool, DbError> {
    let Some(installed) = recognize(conn, applied)? else { return Ok(false); };
    // One atomic transition: a crash cannot leave an ambiguous mixed ordinal.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (name, sql) in MIGRATIONS {
        if !installed.iter().any(|(present, _)| present == name
            || (*name == "0200_bank_accounts_by_type" && *present == "0196_bank_accounts_by_type")
            || (*name == "0201_salestax_payments" && *present == "0200_salestax_payments")) {
            tx.execute_batch(sql)?;
        }
    }
    tx.execute_batch(&format!("PRAGMA user_version={}", MIGRATIONS.len()))?;
    tx.commit()?;
    Ok(true)
}
