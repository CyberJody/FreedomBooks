use super::*;
/// Bank-register reversal boundary using the ledger's public composable writer.
pub fn reverse_journal(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    key: crate::JournalId,
    d: &str,
    reason: &str,
) -> FbResult<crate::model::Journal> {
    command(book, |tx| {
        services::access(tx, actor, c)?;
        refuse_cleared_reversal(tx, c, &key.to_db())?;
        services::reverse_journal_tx(tx, actor, c, key, d, reason)
    })
}
pub fn is_register_account(q: &(impl Query + ?Sized), c: CompanyId, key: &str) -> FbResult<bool> {
    Ok(q.query_row(
        "SELECT EXISTS(SELECT 1 FROM banking_register_account WHERE id=?1 AND company_id=?2)",
        params![key, c.to_db()],
        |r| r.get(0),
    )?)
}
pub(super) fn line(q: &(impl Query + ?Sized), c: CompanyId, key: &str) -> FbResult<Value> {
    q.query_row("SELECT l.id,l.account_id,l.journal_id,l.debit,l.credit,j.date,coalesce(s.state,'U'),coalesce(s.revision,0),s.reconciliation_id,s.id,l.description,l.name,l.position,j.memo,j.reversal_of_id,(SELECT id FROM ledger_journal z WHERE z.reversal_of_id=j.id) FROM ledger_journalline l JOIN ledger_journal j ON j.id=l.journal_id LEFT JOIN banking_banklinestatus s ON s.journal_line_id=l.id WHERE l.id=?1 AND j.company_id=?2 AND j.status='POSTED'",params![key,c.to_db()],|r|{
  let description:String=r.get(10)?;let journal_memo:String=r.get(13)?;
  let memo=if description.is_empty(){journal_memo}else{description.clone()};
  Ok(json!({"id":r.get::<_,String>(0)?,"account_id":r.get::<_,String>(1)?,"journal_id":r.get::<_,String>(2)?,"debit":cash(r.get(3)?),"credit":cash(r.get(4)?),"date":r.get::<_,String>(5)?,"state":r.get::<_,String>(6)?,"revision":r.get::<_,i64>(7)?,"reconciliation_id":r.get::<_,Option<String>>(8)?,"status_id":r.get::<_,Option<String>>(9)?,"description":description,"name":r.get::<_,String>(11)?,"position":r.get::<_,i64>(12)?,"memo":memo,"reversal_of_id":r.get::<_,Option<String>>(14)?,"voided_by_id":r.get::<_,Option<String>>(15)?}))
 }).optional()?.ok_or_else(||FbError::missing("JournalLine"))
}
#[allow(clippy::too_many_arguments)] // Preserve the frozen service argument contract.
pub(super) fn write_state(
    tx: &Writer<'_>,
    actor: UserId,
    c: CompanyId,
    row: &Value,
    state: &str,
    rec: Option<&str>,
    extra: Value,
    action: Option<&str>,
) -> FbResult<Value> {
    let user = services::access(tx, actor, c)?;
    let a = account(tx, c, s(row, "account_id")?)?;
    let key = s(row, "id")?;
    let old = row["revision"]
        .as_i64()
        .ok_or_else(|| FbError::validation("Invalid status revision."))?;
    let rev = documents::next_revision(old)?;
    if old == 0 {
        tx.execute("INSERT INTO banking_banklinestatus(id,company_id,journal_line_id,account_id,state,revision,changed_by_id,reconciliation_id) VALUES(?1,?2,?3,?4,?5,1,?6,?7)",params![new_id(),c.to_db(),key,a.id.to_db(),state,actor.to_db(),rec])?;
    } else {
        tx.execute("UPDATE banking_banklinestatus SET state=?1,revision=?2,changed_by_id=?3,reconciliation_id=?4 WHERE journal_line_id=?5",params![state,rev,actor.to_db(),rec,key])?;
    }
    let mut detail = json!({"line_id":key,"account_id":a.id.to_db(),"account":format!("{} · {}",a.code,a.name),"date":row["date"],"debit":row["debit"],"credit":row["credit"],"before":row["state"],"after":state,"revision":rev});
    if let Some(m) = extra.as_object() {
        for (k, v) in m {
            detail[k] = v.clone();
        }
    }
    services::audit(
        tx,
        &user,
        c,
        action.unwrap_or(if state == "C" {
            "bank_line.cleared"
        } else {
            "bank_line.uncleared"
        }),
        format!("journal:{}", s(row, "journal_id")?),
        detail,
    )?;
    let mut out = row.clone();
    out["state"] = json!(state);
    out["revision"] = json!(rev);
    out["reconciliation_id"] = json!(rec);
    Ok(out)
}
pub fn set_cleared(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    key: &str,
    state: &str,
    expected: Option<i64>,
    account_id: Option<&str>,
) -> FbResult<Value> {
    command(book, |tx| {
        set_cleared_tx(tx, actor, c, key, state, expected, account_id)
    })
}
/// Composable clearing writer: both the state and its audit roll back on error.
/// ```compile_fail
/// fn invalid(q: &fb_db::rusqlite::Connection, u: fb_core::UserId, c: fb_core::CompanyId) {
///     fb_core::banking::set_cleared_tx(q, u, c, "line", "C", Some(0), None);
/// }
/// ```
/// ```compile_fail
/// fn invalid(q: &fb_db::Reader<'_>, u: fb_core::UserId, c: fb_core::CompanyId) {
///     fb_core::banking::set_cleared_tx(q, u, c, "line", "C", Some(0), None);
/// }
/// ```
pub fn set_cleared_tx(
    tx: &Writer<'_>,
    actor: UserId,
    c: CompanyId,
    key: &str,
    state: &str,
    expected: Option<i64>,
    account_id: Option<&str>,
) -> FbResult<Value> {
    tx.atomic(||{
 services::access(tx,actor,c)?;if !["U","C"].contains(&state){return Err(FbError::validation("Choose cleared or not cleared. Reconciled marks are set by Reconcile."));}
if expected.is_none(){return Err(FbError::validation("Reload the register and try again."));}
 let row=line(tx,c,key)?;let aid=s(&row,"account_id")?;if account_id.is_some_and(|a|a!=aid){return Err(FbError::missing("JournalLine"));}
if !is_register_account(tx,c,aid)?{return Err(FbError::validation("Only bank and credit card lines can be marked cleared."));}
 if row["state"]=="R"{return Err(FbError::validation("This line is reconciled. It can change only by undoing that reconciliation."));}
if row["state"]==state{return Ok(row);}
 if row["revision"].as_i64()!=expected{return Err(FbError::validation("This line changed after the register was opened. Reload the register and try again."));}
 write_state(tx,actor,c,&row,state,None,json!({}),None)
})
}
pub fn refuse_cleared_reversal(
    q: &(impl Query + ?Sized),
    c: CompanyId,
    journal: &str,
) -> FbResult<()> {
    let marked:Option<(String,String,String,Option<String>)>=q.query_row("SELECT s.state,a.code,a.name,r.statement_date FROM banking_banklinestatus s JOIN ledger_journalline l ON l.id=s.journal_line_id JOIN ledger_account a ON a.id=s.account_id LEFT JOIN banking_reconciliation r ON r.id=s.reconciliation_id WHERE s.company_id=?1 AND l.journal_id=?2 AND s.state IN('C','R') ORDER BY s.state DESC,l.id LIMIT 1",params![c.to_db(),journal],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    if let Some((state, code, name, d)) = marked {
        let j = store::journal_in(
            q,
            journal.parse().map_err(|_| FbError::missing("Journal"))?,
            c,
        )?;
        let what = if let Some(source) = j.source_id {
            q.query_row(
                "SELECT number FROM documents_documentidentity WHERE id=?1",
                [source],
                |r| r.get::<_, String>(0),
            )?
        } else if !j.entry_number.is_empty() {
            format!("Journal entry {}", j.entry_number)
        } else {
            j.display()
        };
        return Err(FbError::validation(if state == "R" {
            format!(
                "{what} is reconciled in the {code} · {name} register{}. It cannot be voided unless that reconciliation is undone: Banking → Reconcile → Undo last reconciliation, then mark it not cleared and void it.",
                d.map(|s| format!(" (statement {s})")).unwrap_or_default()
            )
        } else {
            format!(
                "{what} is marked cleared in the {code} · {name} register. Mark it not cleared there first, then void it."
            )
        }));
    }
    Ok(())
}
