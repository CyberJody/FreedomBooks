//! Deliberate desktop-sync action, separate from staging and the AI intake capability.
//! Every eligibility decision and accounting write shares one Writer transaction.
use super::*;
use sha2::{Digest, Sha256};
fn fingerprint(item: &Item, rule: &Rule, feed: &Feed) -> String {
    format!("{:x}",Sha256::digest(json!({"item":item.id,"feed":item.feed_account_id,"ledger_account":feed.ledger_account_id,"company":feed.company_id,"connection":feed.connection_id,"currency":feed.currency,"active":feed.active,"date":item.date(),"amount":item.amount,"description":item.description,"pending":item.pending,"return_count":item.return_count,"rule":rule.id,"category":rule.account_id,"payee":rule.payee_name,"policy":rule.policy}).to_string().as_bytes()))
}
pub(super) fn enqueue(w: &Writes<'_, '_>, co: CompanyId, id: &str) -> FbResult<()> {
    let item = item(w.tx, co, id)?;
    if item.status != "NEW" || item.pending || item.return_count != 0 {
        return Ok(());
    }
    let Some(feed) = feed(w.tx, &item.feed_account_id)? else {
        return Ok(());
    };
    if !feed.active || feed.ledger_account_id.is_none() {
        return Ok(());
    }
    let rules = rules::active_rules_in(w.tx, co)?;
    if let Some(rule) =
        rule_for(&rules, &item).filter(|r| r.policy.as_ref().is_some_and(|p| p.auto_post))
    {
        w.execute("INSERT INTO bankfeeds_auto_queue(item_id,rule_id,fingerprint) VALUES(?1,?2,?3) ON CONFLICT(item_id) DO NOTHING",params![id,rule.id,fingerprint(&item,rule,&feed)])?;
    }
    Ok(())
}
pub fn pending_automatic(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    connection: &str,
) -> FbResult<Vec<String>> {
    book.read(|q|{services::access(q,actor,co)?;let mut st=q.prepare("SELECT a.item_id FROM bankfeeds_auto_queue a JOIN bankfeeds_transaction t ON t.id=a.item_id JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE c.company_id=?1 AND c.id=?2 ORDER BY t.ordinal LIMIT 5000")?;Ok(st.query_map(params![co.to_db(),connection],|r|r.get(0))?.collect::<Result<Vec<_>,_>>()?)})
}

pub fn auto_post(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    item_id: &str,
    secrets: &SecretSet,
) -> FbResult<bool> {
    let result = command(book, secrets, |w| {
        let tx = w.tx;
        let user = services::access(tx, actor, co)?;
        let original = item(tx, co, item_id)?;
        let queued: Option<(String, String)> = tx
            .query_row(
                "SELECT rule_id,fingerprint FROM bankfeeds_auto_queue WHERE item_id=?1",
                [item_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((expected_rule, expected_fingerprint)) = queued else {
            return Ok(false);
        };
        w.execute(
            "DELETE FROM bankfeeds_auto_queue WHERE item_id=?1",
            params![item_id],
        )?;
        if original.status != "NEW" || original.pending || original.return_count != 0 {
            return Ok(false);
        }
        let rules = rules::active_rules_in(tx, co)?;
        let Some(rule) = rule_for(&rules, &original) else {
            return Err(err("The matching rule changed. Review this transaction."));
        };
        let current_feed = feed(tx, &original.feed_account_id)?
            .ok_or_else(|| err("The bank feed is unavailable. Review before posting."))?;
        if connection_row(tx, co, &current_feed.connection_id)?["active"] != true {
            return Err(err(
                "The bank connection is inactive. Review this transaction.",
            ));
        }
        if rule.id != expected_rule
            || fingerprint(&original, rule, &current_feed) != expected_fingerprint
        {
            return Err(err(
                "The downloaded transaction or matching rule changed. Review before posting.",
            ));
        }
        let Some(policy) = &rule.policy else {
            return Ok(false);
        };
        if !policy.auto_post {
            return Ok(false);
        }
        let (item, f) = accept::validated(w, co, item_id)?;
        let Some(f) = f else { return Ok(false) };
        crate::periods::require_open(tx, co, item.date().unwrap_or_default())?;
        let mapped = f.ledger_account_id.as_deref().unwrap_or("");
        let name = qbo::validate_policy(tx, co, policy, &rule.account_id)?;
        if mapped == rule.account_id || item.amount == 0 {
            return Err(err(
                "This rule needs a different category and a nonzero amount.",
            ));
        }
        let abs_amount = magnitude(&item)?;
        if !match_candidates(tx, co, &item, 1)?.is_empty()
            || !receipt_candidates(tx, co, &item, 1)?.is_empty()
            || has_multi_match_group(&multi_match_candidates(tx, co, &item, MULTI_MATCH_POOL_LIMIT)?, abs_amount)
        {
            return Err(err(
                "Possible existing transaction or receipt. Review and match it before creating anything new.",
            ));
        }
        if already_recorded(tx, co, &item)?.is_some() {
            return Err(err(
                "Possible duplicate or already-recorded transaction. Review before posting.",
            ));
        }
        // Include already-linked entries: match_candidates deliberately omits them.
        let (debit, credit) = direction(&item)?;
        let duplicate:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM ledger_journalline l JOIN ledger_journal j ON j.id=l.journal_id WHERE j.company_id=?1 AND l.account_id=?2 AND j.status='POSTED' AND l.debit=?3 AND l.credit=?4 AND (abs(julianday(j.date)-julianday(?5))<=5 OR (?8<0 AND (julianday(?5)-julianday(j.date)) BETWEEN -5 AND 180 AND EXISTS(SELECT 1 FROM fb_check_number_live h WHERE h.company_id=j.company_id AND h.bank_id=?2 AND h.identity_id=j.source_id AND h.number<>'' AND h.number NOT GLOB '*[^0-9]*')))) OR EXISTS(SELECT 1 FROM bankfeeds_transaction b WHERE b.feed_account_id=?6 AND b.id<>?7 AND b.amount=?8 AND coalesce(b.posted_on,b.transacted_on)=?5 AND b.description=?9) OR EXISTS(SELECT 1 FROM bankfeeds_transaction b JOIN bankfeeds_account bf ON bf.id=b.feed_account_id JOIN bankfeeds_connection bc ON bc.id=bf.connection_id WHERE bc.company_id=?1 AND b.id<>?7 AND b.status='MATCHED' AND b.amount=?8 AND abs(julianday(coalesce(b.posted_on,b.transacted_on))-julianday(?5))<=5 AND EXISTS(SELECT 1 FROM bankfeeds_transaction_match m JOIN ledger_journalline ml ON ml.id=m.journal_line_id JOIN ledger_journal mj ON mj.id=m.journal_id WHERE m.transaction_id=b.id AND ml.account_id=?2 AND mj.company_id=?1 AND mj.status='POSTED'))",params![co.to_db(),mapped,debit,credit,item.date(),item.feed_account_id,item.id,item.amount,item.description],|r|r.get(0))?;
        if duplicate {
            return Err(err(
                "Possible duplicate or already-recorded transaction. Review before posting.",
            ));
        }
        let mapped_account = AccountId::parse(mapped)
            .map(|id| store::account(tx, id))
            .transpose()?
            .flatten()
            .ok_or_else(|| err("Choose a mapped bank or credit card account."))?;
        if !["BANK", "CREDIT_CARD"].contains(&mapped_account.detail_type.as_str()) {
            return Err(err(
                "Automatic rules require a bank or credit card feed mapping.",
            ));
        }
        // A two-line journal correctly records both bank transfers and card payments;
        // keep the imported policy on its existing journal path.
        let kind = "JOURNAL";
        let input = json!({"account_id":rule.account_id,"transfer_account_id":rule.account_id,"payee_name":name});
        let posted = accept::finish(w, actor, co, &item, &f, kind, &input, None)?;
        w.audit(
            &user,
            co,
            "bankfeed.rule_auto_posted",
            format!("bankfeedtransaction:{}", item.id),
            json!({"rule_id":rule.id,"journal_id":posted.journal_id,"kind":policy.kind}),
        )?;
        w.execute(
            "DELETE FROM bankfeeds_rule_review WHERE item_id=?1",
            params![item.id],
        )?;
        Ok(true)
    });
    match result {
        Ok(done) => Ok(done),
        Err(e) => {
            // Refusal is visible; posting and decision were rolled back together.
            // Never turn authorization/database failures into successful results.
            let message = match &e {
                FbError::Validation(messages) if messages.first().is_some_and(|m| crate::periods::is_closed_message(m)) => e.message(),
                FbError::Validation(_) => e.to_string(),
                _ => return Err(e),
            };
            command(book, secrets, |w| {
                services::access(w.tx, actor, co)?;
                let i = item(w.tx, co, item_id)?;
                w.execute(
                    "DELETE FROM bankfeeds_auto_queue WHERE item_id=?1",
                    params![item_id],
                )?;
                if i.status == "NEW" {
                    w.execute("INSERT INTO bankfeeds_rule_review(item_id,reason) VALUES(?1,?2) ON CONFLICT(item_id) DO UPDATE SET reason=excluded.reason",params![item_id,text::sanitize(&message,500)])?;
                }
                Ok(())
            })?;
            Ok(false)
        }
    }
}
