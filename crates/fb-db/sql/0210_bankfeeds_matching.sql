-- FY2025 banking fixes (2026-11-26, book format 97):
-- 1. Late-clearing written checks: a posted live check (Check, bill-payment check, or refund check in
--    fb_check_number_live) may match a bank withdrawal dated up to 180 days after the check date (and at
--    most 5 days before). When the bank description names a check number ("CHECK 1003"), a written check
--    with a different check number is refused even within 5 days; non-check entries keep the 5-day window.
-- 2. Multi-entry matches: one bank transaction may match 2 to 20 posted, unreversed lines on the mapped
--    account in the same direction whose amounts sum exactly to the bank row. Single-entry matches stay
--    on bankfeeds_transaction alone (no rows in bankfeeds_transaction_match). Multi-entry matches record
--    all matched lines in bankfeeds_transaction_match and keep the first line in
--    bankfeeds_transaction.(journal_id, journal_line_id). Each matched journal line stays exclusive to one
--    bank transaction across both tables.

CREATE UNIQUE INDEX bankfeeds_transaction_id_status ON bankfeeds_transaction(id, status);

CREATE TABLE bankfeeds_transaction_match(
 transaction_id TEXT NOT NULL REFERENCES bankfeeds_transaction(id) DEFERRABLE INITIALLY DEFERRED,
 parent_status TEXT NOT NULL DEFAULT 'MATCHED' CHECK(parent_status='MATCHED'),
 journal_id TEXT NOT NULL REFERENCES ledger_journal(id) DEFERRABLE INITIALLY DEFERRED,
 journal_line_id TEXT NOT NULL UNIQUE REFERENCES ledger_journalline(id) DEFERRABLE INITIALLY DEFERRED,
 PRIMARY KEY (transaction_id, journal_line_id),
 FOREIGN KEY (transaction_id, parent_status) REFERENCES bankfeeds_transaction(id, status) DEFERRABLE INITIALLY DEFERRED
) WITHOUT ROWID, STRICT;
CREATE INDEX bankfeeds_transaction_match_journal ON bankfeeds_transaction_match(journal_id);

CREATE TRIGGER fb_valid_bankfeeds_transaction_match_insert BEFORE INSERT ON bankfeeds_transaction_match
BEGIN
 SELECT RAISE(ABORT,'fb_valid: Invalid stored text, identifier, date or timestamp')
  WHERE NOT fb_valid_uuid(NEW.transaction_id) OR NOT fb_valid_text(NEW.parent_status) OR NOT fb_valid_uuid(NEW.journal_id) OR NOT fb_valid_uuid(NEW.journal_line_id);
END;
CREATE TRIGGER fb_valid_bankfeeds_transaction_match_update BEFORE UPDATE ON bankfeeds_transaction_match
BEGIN
 SELECT RAISE(ABORT,'fb_valid: Invalid stored text, identifier, date or timestamp')
  WHERE NOT fb_valid_uuid(NEW.transaction_id) OR NOT fb_valid_text(NEW.parent_status) OR NOT fb_valid_uuid(NEW.journal_id) OR NOT fb_valid_uuid(NEW.journal_line_id);
END;

CREATE TRIGGER fb_bankfeeds_transaction_match_insert BEFORE INSERT ON bankfeeds_transaction_match BEGIN
 SELECT RAISE(ABORT,'bankfeeds_text: Invalid bank-feed text, key, date or timestamp')
  WHERE NOT (NEW.transaction_id IS NOT NULL AND NEW.journal_id IS NOT NULL AND NEW.journal_line_id IS NOT NULL
   AND fb_bankfeeds_uuid(NEW.transaction_id) AND fb_bankfeeds_text(NEW.parent_status,1,20) AND fb_bankfeeds_uuid(NEW.journal_id) AND fb_bankfeeds_uuid(NEW.journal_line_id));
 SELECT RAISE(ABORT,'bankfeeds_key: Protected key already exists')
  WHERE EXISTS(SELECT 1 FROM bankfeeds_transaction_match WHERE journal_line_id=NEW.journal_line_id)
   OR EXISTS(SELECT 1 FROM bankfeeds_transaction WHERE journal_line_id=NEW.journal_line_id);
 SELECT RAISE(ABORT,'bankfeeds_match_limit: A bank transaction matches at most 20 posted entries')
  WHERE (SELECT count(*) FROM bankfeeds_transaction_match WHERE transaction_id=NEW.transaction_id)>=20;
 SELECT RAISE(ABORT,'bankfeeds_decision: Each matched entry must be an unmatched posted line on the mapped account in the same direction')
  WHERE NOT EXISTS(
   SELECT 1 FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_connection c ON c.id=f.connection_id
    JOIN ledger_company y ON y.id=c.company_id JOIN ledger_journal j ON j.id=NEW.journal_id JOIN ledger_journalline l ON l.id=NEW.journal_line_id
   WHERE t.id=NEW.transaction_id AND t.status='NEW' AND t.pending=0 AND coalesce(t.posted_on,t.transacted_on) IS NOT NULL AND t.amount<>0
    AND f.active=1 AND f.ledger_account_id IS NOT NULL AND upper(f.currency)=y.currency
    AND j.company_id=c.company_id AND j.status='POSTED' AND j.reversal_of_id IS NULL
    AND NOT EXISTS(SELECT 1 FROM ledger_journal r WHERE r.reversal_of_id=j.id)
    AND l.journal_id=j.id AND l.account_id=f.ledger_account_id
    AND ((t.amount>0 AND l.debit>0 AND l.credit=0) OR (t.amount<0 AND l.credit>0 AND l.debit=0)));
 SELECT RAISE(ABORT,'bankfeeds_decision: A match names an entry dated within 5 days of the transaction, or a matching check within 180 days')
  WHERE NOT EXISTS(
   SELECT 1 FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN ledger_journal j ON j.id=NEW.journal_id
   WHERE t.id=NEW.transaction_id AND fb_bankfeeds_date(j.date) AND fb_bankfeeds_date(coalesce(t.posted_on,t.transacted_on))
    AND NOT (t.amount<0 AND fb_bankfeeds_check_number(t.description) IS NOT NULL
      AND EXISTS(SELECT 1 FROM fb_check_number_live h WHERE h.company_id=j.company_id AND h.bank_id=f.ledger_account_id AND h.identity_id=j.source_id AND h.number<>'' AND h.number NOT GLOB '*[^0-9]*')
      AND NOT EXISTS(SELECT 1 FROM fb_check_number_live h WHERE h.company_id=j.company_id AND h.bank_id=f.ledger_account_id AND h.identity_id=j.source_id AND h.number<>'' AND h.number NOT GLOB '*[^0-9]*' AND lower(h.number)=lower(fb_bankfeeds_check_number(t.description))))
    AND (abs(julianday(j.date)-julianday(coalesce(t.posted_on,t.transacted_on)))<=5
      OR (t.amount<0
        AND EXISTS(SELECT 1 FROM fb_check_number_live h WHERE h.company_id=j.company_id AND h.bank_id=f.ledger_account_id AND h.identity_id=j.source_id AND h.number<>'' AND h.number NOT GLOB '*[^0-9]*'
          AND (fb_bankfeeds_check_number(t.description) IS NULL OR lower(h.number)=lower(fb_bankfeeds_check_number(t.description))))
        AND (julianday(coalesce(t.posted_on,t.transacted_on))-julianday(j.date)) BETWEEN -5 AND 180)));
END;

CREATE TRIGGER fb_bankfeeds_transaction_match_update BEFORE UPDATE ON bankfeeds_transaction_match BEGIN
 SELECT RAISE(ABORT,'bankfeeds_match_fixed: Return the transaction to review to change its matched entries');
END;

CREATE TRIGGER fb_bankfeeds_transaction_match_delete BEFORE DELETE ON bankfeeds_transaction_match BEGIN
 SELECT RAISE(ABORT,'bankfeeds_match_fixed: Return the transaction to review to change its matched entries')
  WHERE EXISTS(SELECT 1 FROM bankfeeds_transaction WHERE id=OLD.transaction_id AND status<>'NEW');
END;

DROP TRIGGER fb_bankfeeds_transaction_update;
CREATE TRIGGER fb_bankfeeds_transaction_update BEFORE UPDATE ON bankfeeds_transaction BEGIN
 SELECT RAISE(ABORT,'bankfeeds_key: Update would replace an existing row') WHERE EXISTS(SELECT 1 FROM bankfeeds_transaction WHERE id IS NOT OLD.id AND (id=NEW.id OR ordinal=NEW.ordinal OR (feed_account_id=NEW.feed_account_id AND remote_identity=NEW.remote_identity) OR (NEW.journal_line_id IS NOT NULL AND journal_line_id=NEW.journal_line_id)))
  OR (NEW.journal_line_id IS NOT NULL AND EXISTS(SELECT 1 FROM bankfeeds_transaction_match m WHERE m.journal_line_id=NEW.journal_line_id AND m.transaction_id<>NEW.id));
 SELECT RAISE(ABORT,'bankfeeds_transaction_fixed: A downloaded transaction keeps its account, number and remote identity') WHERE NEW.ordinal IS NOT OLD.ordinal OR NEW.feed_account_id IS NOT OLD.feed_account_id OR NEW.remote_id IS NOT OLD.remote_id OR NEW.first_seen_at IS NOT OLD.first_seen_at
   OR NEW.remote_identity IS NOT OLD.remote_identity;
 SELECT RAISE(ABORT,'bankfeeds_decided_values: A decided transaction keeps the values the operator accepted') WHERE OLD.status IN('ACCEPTED','MATCHED') AND (NEW.posted_on IS NOT OLD.posted_on OR NEW.transacted_on IS NOT OLD.transacted_on OR NEW.amount IS NOT OLD.amount OR NEW.description IS NOT OLD.description OR NEW.pending IS NOT OLD.pending);
 SELECT RAISE(ABORT,'bankfeeds_return_count: Returning to review counts once') WHERE NEW.return_count<>OLD.return_count+(CASE WHEN OLD.status<>'NEW' AND NEW.status='NEW' THEN 1 ELSE 0 END);
 SELECT RAISE(ABORT,'bankfeeds_transition: Invalid review decision') WHERE (OLD.status IN('ACCEPTED','MATCHED') AND NEW.status NOT IN(OLD.status,'NEW')) OR (OLD.status='IGNORED' AND NEW.status NOT IN('IGNORED','NEW'))
  OR (OLD.status IN('ACCEPTED','MATCHED') AND NEW.status=OLD.status AND (NEW.accepted_as IS NOT OLD.accepted_as OR NEW.journal_id IS NOT OLD.journal_id OR NEW.journal_line_id IS NOT OLD.journal_line_id OR NEW.reference IS NOT OLD.reference OR NEW.reference_url IS NOT OLD.reference_url OR NEW.decided_at IS NOT OLD.decided_at OR NEW.decided_by_id IS NOT OLD.decided_by_id))
  OR (NEW.status='NEW' AND (NEW.decided_at IS NOT NULL OR NEW.reference<>'' OR NEW.reference_url<>''))
  OR (NEW.status<>'NEW' AND NEW.decided_at IS NULL)
  OR (NEW.status IN('ACCEPTED','IGNORED') AND EXISTS(SELECT 1 FROM bankfeeds_transaction_match WHERE transaction_id=NEW.id));
 SELECT RAISE(ABORT,'bankfeeds_ignore: Only a posted-or-pending transaction still to review can be ignored') WHERE NEW.status='IGNORED' AND OLD.status<>'IGNORED' AND (OLD.status<>'NEW' OR NEW.reference<>'');
 SELECT RAISE(ABORT,'bankfeeds_return: An accepted transaction returns to review only after its entry was reversed') WHERE OLD.status='ACCEPTED' AND NEW.status='NEW' AND NOT EXISTS(SELECT 1 FROM ledger_journal WHERE reversal_of_id=OLD.journal_id);
 SELECT RAISE(ABORT,'bankfeeds_decision: A decision names a posted entry line on the mapped account, in this direction and amount')
  WHERE NEW.status IN('ACCEPTED','MATCHED') AND OLD.status NOT IN('ACCEPTED','MATCHED')
   AND NOT (NEW.status='MATCHED' AND EXISTS(SELECT 1 FROM bankfeeds_transaction_match WHERE transaction_id=NEW.id))
   AND (OLD.status<>'NEW' OR OLD.pending<>0 OR NEW.pending<>0 OR coalesce(NEW.posted_on,NEW.transacted_on) IS NULL OR NOT EXISTS(
   SELECT 1 FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id JOIN ledger_journal j ON j.id=NEW.journal_id JOIN ledger_journalline l ON l.id=NEW.journal_line_id JOIN ledger_company y ON y.id=c.company_id
   WHERE f.id=NEW.feed_account_id AND f.active=1 AND f.ledger_account_id IS NOT NULL AND upper(f.currency)=y.currency AND j.company_id=c.company_id AND j.status='POSTED' AND l.journal_id=j.id AND l.account_id=f.ledger_account_id
     AND l.debit=(CASE WHEN NEW.amount>0 THEN NEW.amount ELSE 0 END) AND l.credit=(CASE WHEN NEW.amount>0 THEN 0 ELSE -NEW.amount END)
     AND EXISTS(SELECT 1 FROM ledger_company_members m JOIN fb_user u ON u.id=m.user_id WHERE m.company_id=c.company_id AND m.user_id=NEW.decided_by_id AND u.is_active=1)
     AND (CASE NEW.accepted_as
       WHEN 'MATCH' THEN j.reversal_of_id IS NULL AND NOT EXISTS(SELECT 1 FROM ledger_journal r WHERE r.reversal_of_id=j.id)
       WHEN 'JOURNAL' THEN j.source_id IS NULL AND j.reversal_of_id IS NULL
       WHEN 'CHECK' THEN j.reversal_of_id IS NULL AND (SELECT kind FROM documents_documentidentity WHERE id=j.source_id)='CHK'
       WHEN 'DEPOSIT' THEN j.reversal_of_id IS NULL AND (SELECT kind FROM documents_documentidentity WHERE id=j.source_id)='DEP'
       WHEN 'TRANSFER' THEN j.reversal_of_id IS NULL AND (SELECT kind FROM documents_documentidentity WHERE id=j.source_id)='TRF'
       ELSE 0 END)));
 SELECT RAISE(ABORT,'bankfeeds_decision: Matched entries must be posted lines on the mapped account totaling this transaction amount')
  WHERE NEW.status='MATCHED' AND OLD.status NOT IN('ACCEPTED','MATCHED')
   AND EXISTS(SELECT 1 FROM bankfeeds_transaction_match WHERE transaction_id=NEW.id)
   AND (OLD.status<>'NEW' OR OLD.pending<>0 OR NEW.pending<>0 OR coalesce(NEW.posted_on,NEW.transacted_on) IS NULL OR NEW.amount=0 OR NEW.accepted_as<>'MATCH'
    OR (SELECT count(*) FROM bankfeeds_transaction_match WHERE transaction_id=NEW.id) NOT BETWEEN 2 AND 20
    OR NOT EXISTS(SELECT 1 FROM bankfeeds_transaction_match WHERE transaction_id=NEW.id AND journal_id=NEW.journal_id AND journal_line_id=NEW.journal_line_id)
    OR NOT EXISTS(
      SELECT 1 FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id JOIN ledger_company y ON y.id=c.company_id
      WHERE f.id=NEW.feed_account_id AND f.active=1 AND f.ledger_account_id IS NOT NULL AND upper(f.currency)=y.currency
        AND EXISTS(SELECT 1 FROM ledger_company_members m JOIN fb_user u ON u.id=m.user_id WHERE m.company_id=c.company_id AND m.user_id=NEW.decided_by_id AND u.is_active=1))
    OR EXISTS(
      SELECT 1 FROM bankfeeds_transaction_match m JOIN bankfeeds_account f ON f.id=NEW.feed_account_id JOIN bankfeeds_connection c ON c.id=f.connection_id
       LEFT JOIN ledger_journal j ON j.id=m.journal_id LEFT JOIN ledger_journalline l ON l.id=m.journal_line_id
      WHERE m.transaction_id=NEW.id AND NOT (
        j.id IS NOT NULL AND l.id IS NOT NULL AND j.company_id=c.company_id AND j.status='POSTED' AND j.reversal_of_id IS NULL
        AND NOT EXISTS(SELECT 1 FROM ledger_journal r WHERE r.reversal_of_id=j.id)
        AND l.journal_id=j.id AND l.account_id=f.ledger_account_id
        AND ((NEW.amount>0 AND l.debit>0 AND l.credit=0) OR (NEW.amount<0 AND l.credit>0 AND l.debit=0))))
    OR (SELECT coalesce(sum(l.debit),0) FROM bankfeeds_transaction_match m JOIN ledger_journalline l ON l.id=m.journal_line_id WHERE m.transaction_id=NEW.id)<>(CASE WHEN NEW.amount>0 THEN NEW.amount ELSE 0 END)
    OR (SELECT coalesce(sum(l.credit),0) FROM bankfeeds_transaction_match m JOIN ledger_journalline l ON l.id=m.journal_line_id WHERE m.transaction_id=NEW.id)<>(CASE WHEN NEW.amount>0 THEN 0 ELSE -NEW.amount END));
END;

DROP TRIGGER fb_bankfeeds_transaction_eligible;
CREATE TRIGGER fb_bankfeeds_transaction_eligible BEFORE UPDATE ON bankfeeds_transaction
WHEN NEW.status IN('IGNORED','ACCEPTED','MATCHED') AND NEW.status IS NOT OLD.status AND OLD.status NOT IN('ACCEPTED','MATCHED') BEGIN
 SELECT RAISE(ABORT,'bankfeeds_decision: A decision is made by an active member of the company') WHERE NOT EXISTS(
   SELECT 1 FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id JOIN ledger_company_members m ON m.company_id=c.company_id JOIN fb_user u ON u.id=m.user_id
   WHERE f.id=NEW.feed_account_id AND m.user_id=NEW.decided_by_id AND u.is_active=1);
 SELECT RAISE(ABORT,'bankfeeds_decision: A match names an entry dated within 5 days of the transaction, or a matching check within 180 days')
  WHERE NEW.status='MATCHED' AND NOT EXISTS(SELECT 1 FROM bankfeeds_transaction_match WHERE transaction_id=NEW.id) AND NOT EXISTS(
   SELECT 1 FROM bankfeeds_account f JOIN ledger_journal j ON j.id=NEW.journal_id
   WHERE f.id=NEW.feed_account_id AND fb_bankfeeds_date(j.date) AND fb_bankfeeds_date(coalesce(NEW.posted_on,NEW.transacted_on))
    AND NOT (NEW.amount<0 AND fb_bankfeeds_check_number(NEW.description) IS NOT NULL
      AND EXISTS(SELECT 1 FROM fb_check_number_live h WHERE h.company_id=j.company_id AND h.bank_id=f.ledger_account_id AND h.identity_id=j.source_id AND h.number<>'' AND h.number NOT GLOB '*[^0-9]*')
      AND NOT EXISTS(SELECT 1 FROM fb_check_number_live h WHERE h.company_id=j.company_id AND h.bank_id=f.ledger_account_id AND h.identity_id=j.source_id AND h.number<>'' AND h.number NOT GLOB '*[^0-9]*' AND lower(h.number)=lower(fb_bankfeeds_check_number(NEW.description))))
    AND (abs(julianday(j.date)-julianday(coalesce(NEW.posted_on,NEW.transacted_on)))<=5
      OR (NEW.amount<0
        AND EXISTS(SELECT 1 FROM fb_check_number_live h WHERE h.company_id=j.company_id AND h.bank_id=f.ledger_account_id AND h.identity_id=j.source_id AND h.number<>'' AND h.number NOT GLOB '*[^0-9]*'
          AND (fb_bankfeeds_check_number(NEW.description) IS NULL OR lower(h.number)=lower(fb_bankfeeds_check_number(NEW.description))))
        AND (julianday(coalesce(NEW.posted_on,NEW.transacted_on))-julianday(j.date)) BETWEEN -5 AND 180)));
 SELECT RAISE(ABORT,'bankfeeds_decision: A match names entries dated within 5 days of the transaction, or matching checks within 180 days')
  WHERE NEW.status='MATCHED' AND EXISTS(SELECT 1 FROM bankfeeds_transaction_match WHERE transaction_id=NEW.id) AND EXISTS(
   SELECT 1 FROM bankfeeds_transaction_match m JOIN bankfeeds_account f ON f.id=NEW.feed_account_id JOIN ledger_journal j ON j.id=m.journal_id
   WHERE m.transaction_id=NEW.id AND NOT (
    fb_bankfeeds_date(j.date) AND fb_bankfeeds_date(coalesce(NEW.posted_on,NEW.transacted_on))
    AND NOT (NEW.amount<0 AND fb_bankfeeds_check_number(NEW.description) IS NOT NULL
      AND EXISTS(SELECT 1 FROM fb_check_number_live h WHERE h.company_id=j.company_id AND h.bank_id=f.ledger_account_id AND h.identity_id=j.source_id AND h.number<>'' AND h.number NOT GLOB '*[^0-9]*')
      AND NOT EXISTS(SELECT 1 FROM fb_check_number_live h WHERE h.company_id=j.company_id AND h.bank_id=f.ledger_account_id AND h.identity_id=j.source_id AND h.number<>'' AND h.number NOT GLOB '*[^0-9]*' AND lower(h.number)=lower(fb_bankfeeds_check_number(NEW.description))))
    AND (abs(julianday(j.date)-julianday(coalesce(NEW.posted_on,NEW.transacted_on)))<=5
      OR (NEW.amount<0
        AND EXISTS(SELECT 1 FROM fb_check_number_live h WHERE h.company_id=j.company_id AND h.bank_id=f.ledger_account_id AND h.identity_id=j.source_id AND h.number<>'' AND h.number NOT GLOB '*[^0-9]*'
          AND (fb_bankfeeds_check_number(NEW.description) IS NULL OR lower(h.number)=lower(fb_bankfeeds_check_number(NEW.description))))
        AND (julianday(coalesce(NEW.posted_on,NEW.transacted_on))-julianday(j.date)) BETWEEN -5 AND 180))));
 SELECT RAISE(ABORT,'bankfeeds_decision: A decision never names a reversed entry')
  WHERE NEW.status IN('ACCEPTED','MATCHED') AND (
   EXISTS(SELECT 1 FROM ledger_journal r WHERE r.reversal_of_id=NEW.journal_id)
   OR (NEW.status='MATCHED' AND EXISTS(SELECT 1 FROM bankfeeds_transaction_match m JOIN ledger_journal r ON r.reversal_of_id=m.journal_id WHERE m.transaction_id=NEW.id)));
END;

DROP TRIGGER fb_bankfeeds_link_insert;
CREATE TRIGGER fb_bankfeeds_link_insert AFTER INSERT ON bankfeeds_link BEGIN
 SELECT RAISE(ABORT,'bankfeeds_link: A linked transaction is ignored and names an earlier accepted transaction of another downloaded account with the same amount on the same ledger account')
 WHERE EXISTS(SELECT 1 FROM bankfeeds_safety_invalid WHERE id='link:'||NEW.item_ordinal)
  OR NOT EXISTS(SELECT 1 FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_transaction e ON e.ordinal=NEW.earlier_ordinal
   WHERE t.ordinal=NEW.item_ordinal AND e.status IN('ACCEPTED','MATCHED') AND e.amount=t.amount AND (
    (NOT EXISTS(SELECT 1 FROM bankfeeds_transaction_match m WHERE m.transaction_id=e.id) AND EXISTS(
      SELECT 1 FROM ledger_journalline l JOIN ledger_journal j ON j.id=l.journal_id
      WHERE l.id=e.journal_line_id AND l.account_id IS f.ledger_account_id
       AND l.debit=CASE WHEN t.amount>0 THEN t.amount ELSE 0 END AND l.credit=CASE WHEN t.amount<0 THEN -t.amount ELSE 0 END
       AND j.status='POSTED' AND NOT EXISTS(SELECT 1 FROM ledger_journal r WHERE r.reversal_of_id=j.id)))
    OR (e.status='MATCHED' AND EXISTS(SELECT 1 FROM bankfeeds_transaction_match m WHERE m.transaction_id=e.id)
      AND NOT EXISTS(
        SELECT 1 FROM bankfeeds_transaction_match m LEFT JOIN ledger_journalline l ON l.id=m.journal_line_id LEFT JOIN ledger_journal j ON j.id=m.journal_id
        WHERE m.transaction_id=e.id AND NOT (l.id IS NOT NULL AND j.id IS NOT NULL AND l.journal_id=j.id AND l.account_id IS f.ledger_account_id
          AND j.status='POSTED' AND NOT EXISTS(SELECT 1 FROM ledger_journal r WHERE r.reversal_of_id=j.id)))
      AND (SELECT coalesce(sum(l.debit),0) FROM bankfeeds_transaction_match m JOIN ledger_journalline l ON l.id=m.journal_line_id WHERE m.transaction_id=e.id)=CASE WHEN t.amount>0 THEN t.amount ELSE 0 END
      AND (SELECT coalesce(sum(l.credit),0) FROM bankfeeds_transaction_match m JOIN ledger_journalline l ON l.id=m.journal_line_id WHERE m.transaction_id=e.id)=CASE WHEN t.amount<0 THEN -t.amount ELSE 0 END)));
END;

DROP VIEW bankfeeds_invalid;
CREATE VIEW bankfeeds_invalid AS
 SELECT t.id FROM bankfeeds_transaction t WHERE t.status IN('ACCEPTED','MATCHED')
  AND NOT (t.status='MATCHED' AND EXISTS(SELECT 1 FROM bankfeeds_transaction_match m WHERE m.transaction_id=t.id))
  AND NOT EXISTS(
   SELECT 1 FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id JOIN ledger_journal j ON j.id=t.journal_id JOIN ledger_journalline l ON l.id=t.journal_line_id
   WHERE f.id=t.feed_account_id AND j.company_id=c.company_id AND j.status='POSTED' AND l.journal_id=j.id
     AND l.debit=(CASE WHEN t.amount>0 THEN t.amount ELSE 0 END) AND l.credit=(CASE WHEN t.amount>0 THEN 0 ELSE -t.amount END))
 UNION ALL SELECT t.id FROM bankfeeds_transaction t WHERE t.status='MATCHED' AND EXISTS(SELECT 1 FROM bankfeeds_transaction_match m WHERE m.transaction_id=t.id) AND (
   (SELECT count(*) FROM bankfeeds_transaction_match m WHERE m.transaction_id=t.id) NOT BETWEEN 2 AND 20
   OR NOT EXISTS(SELECT 1 FROM bankfeeds_transaction_match m WHERE m.transaction_id=t.id AND m.journal_id=t.journal_id AND m.journal_line_id=t.journal_line_id)
   OR EXISTS(
     SELECT 1 FROM bankfeeds_transaction_match m JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_connection c ON c.id=f.connection_id
      LEFT JOIN ledger_journal j ON j.id=m.journal_id LEFT JOIN ledger_journalline l ON l.id=m.journal_line_id
     WHERE m.transaction_id=t.id AND NOT (j.id IS NOT NULL AND l.id IS NOT NULL AND j.company_id=c.company_id AND j.status='POSTED' AND l.journal_id=j.id
       AND l.account_id=(SELECT account_id FROM ledger_journalline WHERE id=t.journal_line_id)
       AND ((t.amount>0 AND l.debit>0 AND l.credit=0) OR (t.amount<0 AND l.credit>0 AND l.debit=0))))
   OR (SELECT coalesce(sum(l.debit),0) FROM bankfeeds_transaction_match m JOIN ledger_journalline l ON l.id=m.journal_line_id WHERE m.transaction_id=t.id)<>(CASE WHEN t.amount>0 THEN t.amount ELSE 0 END)
   OR (SELECT coalesce(sum(l.credit),0) FROM bankfeeds_transaction_match m JOIN ledger_journalline l ON l.id=m.journal_line_id WHERE m.transaction_id=t.id)<>(CASE WHEN t.amount>0 THEN 0 ELSE -t.amount END))
 UNION ALL SELECT m.transaction_id FROM bankfeeds_transaction_match m LEFT JOIN bankfeeds_transaction t ON t.id=m.transaction_id WHERE t.id IS NULL OR t.status<>'MATCHED'
 UNION ALL SELECT m.transaction_id FROM bankfeeds_transaction_match m JOIN bankfeeds_transaction t ON t.journal_line_id=m.journal_line_id WHERE t.id<>m.transaction_id
 UNION ALL SELECT t.id FROM bankfeeds_transaction t WHERE t.ordinal>coalesce((SELECT value FROM bankfeeds_counter WHERE name='transaction'),0)
 UNION ALL SELECT f.id FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id LEFT JOIN ledger_account a ON a.id=f.ledger_account_id WHERE f.ledger_account_id IS NOT NULL AND (a.id IS NULL OR a.company_id<>c.company_id)
 UNION ALL SELECT f.id FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE f.active=1 AND f.ledger_account_id IS NOT NULL AND EXISTS(
   SELECT 1 FROM bankfeeds_account g JOIN bankfeeds_connection d ON d.id=g.connection_id WHERE d.company_id=c.company_id AND g.id<>f.id AND g.active=1 AND g.ledger_account_id=f.ledger_account_id)
 UNION ALL SELECT r.id FROM bankfeeds_rule r LEFT JOIN ledger_account a ON a.id=r.account_id WHERE a.id IS NULL OR a.company_id<>r.company_id
 UNION ALL SELECT c.id FROM bankfeeds_connection c WHERE NOT EXISTS(SELECT 1 FROM fb_user WHERE id=c.created_by_id)
 UNION ALL SELECT t.id FROM bankfeeds_transaction t WHERE t.decided_by_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM fb_user WHERE id=t.decided_by_id)
 UNION ALL SELECT t.id FROM bankfeeds_transaction t WHERE EXISTS(SELECT 1 FROM bankfeeds_retired x WHERE x.id=t.id)
 UNION ALL SELECT x.id FROM bankfeeds_retired x WHERE NOT (length(CAST(x.id AS BLOB))=36 AND fb_bankfeeds_uuid(x.id))
 UNION ALL SELECT f.id FROM bankfeeds_account f WHERE EXISTS(SELECT 1 FROM bankfeeds_account g WHERE g.id<>f.id AND g.connection_id=f.connection_id AND g.conn_key_identity=f.conn_key_identity AND g.remote_identity=f.remote_identity)
 UNION ALL SELECT t.id FROM bankfeeds_transaction t WHERE EXISTS(SELECT 1 FROM bankfeeds_transaction u WHERE u.id<>t.id AND u.feed_account_id=t.feed_account_id AND u.remote_identity=t.remote_identity)
 UNION ALL SELECT f.id FROM bankfeeds_account f WHERE NOT EXISTS(SELECT 1 FROM bankfeeds_identity_secret)
 UNION ALL SELECT s.name FROM bankfeeds_identity_secret s WHERE NOT (s.name='identity' AND typeof(s.secret)='blob' AND length(s.secret)=32);

DROP VIEW bankfeeds_text_invalid;
CREATE VIEW bankfeeds_text_invalid AS
 SELECT id FROM bankfeeds_connection WHERE NOT (fb_bankfeeds_uuid(id) AND fb_bankfeeds_uuid(company_id) AND fb_bankfeeds_uuid(created_by_id)
  AND fb_bankfeeds_text(label,1,120) AND fb_bankfeeds_text(host,1,253) AND fb_bankfeeds_text(last_sync_summary,0,500)
  AND fb_bankfeeds_timestamp(created_at) AND fb_bankfeeds_timestamp(forgotten_at) AND fb_bankfeeds_timestamp(last_sync_at))
 UNION ALL SELECT id FROM bankfeeds_account WHERE NOT (fb_bankfeeds_uuid(id) AND fb_bankfeeds_uuid(connection_id) AND fb_bankfeeds_uuid(ledger_account_id)
  AND fb_bankfeeds_text(conn_key,0,200) AND fb_bankfeeds_text(remote_id,1,200) AND fb_bankfeeds_text(name,1,120) AND fb_bankfeeds_text(org_name,0,120) AND fb_bankfeeds_text(currency,1,200)
  AND fb_bankfeeds_timestamp(balance_at) AND fb_bankfeeds_timestamp(mapped_at) AND fb_bankfeeds_timestamp(first_seen_at) AND fb_bankfeeds_timestamp(last_seen_at)
  AND fb_bankfeeds_identity_form(conn_key_identity) AND fb_bankfeeds_identity_form(remote_identity))
 UNION ALL SELECT id FROM bankfeeds_transaction WHERE NOT (fb_bankfeeds_uuid(id) AND fb_bankfeeds_uuid(feed_account_id) AND fb_bankfeeds_uuid(journal_id) AND fb_bankfeeds_uuid(journal_line_id) AND fb_bankfeeds_uuid(decided_by_id)
  AND fb_bankfeeds_text(remote_id,1,200) AND fb_bankfeeds_text(description,0,500) AND fb_bankfeeds_text(reference,0,60) AND fb_bankfeeds_text(reference_url,0,200)
  AND fb_bankfeeds_date(posted_on) AND fb_bankfeeds_date(transacted_on) AND fb_bankfeeds_timestamp(first_seen_at) AND fb_bankfeeds_timestamp(updated_at) AND fb_bankfeeds_timestamp(decided_at)
  AND fb_bankfeeds_identity_form(remote_identity))
 UNION ALL SELECT transaction_id FROM bankfeeds_transaction_match WHERE NOT (fb_bankfeeds_uuid(transaction_id) AND fb_bankfeeds_text(parent_status,1,20) AND fb_bankfeeds_uuid(journal_id) AND fb_bankfeeds_uuid(journal_line_id))
 UNION ALL SELECT id FROM bankfeeds_rule WHERE NOT (fb_bankfeeds_uuid(id) AND fb_bankfeeds_uuid(company_id) AND fb_bankfeeds_uuid(account_id)
  AND fb_bankfeeds_text(name,1,120) AND fb_bankfeeds_text(match_text,1,200) AND fb_bankfeeds_text(payee_name,0,120)
  AND fb_bankfeeds_timestamp(created_at) AND fb_bankfeeds_timestamp(updated_at));

DROP VIEW fb_value_invalid;
CREATE VIEW fb_value_invalid AS
SELECT 'invalid stored value in accounts_owner' AS problem FROM accounts_owner WHERE
 NOT fb_valid_uuid(user_id)
UNION ALL
SELECT 'invalid stored value in assistant_connection' AS problem FROM assistant_connection WHERE
 NOT fb_valid_text(account_email) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(created_by_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(key_last4) OR
 NOT fb_valid_text(last_error) OR
 NOT fb_valid_text(mode) OR
 NOT fb_valid_text(model) OR
 NOT fb_valid_text(model_choices) OR
 NOT fb_valid_text(status)
UNION ALL
SELECT 'invalid stored value in assistant_reading' AS problem FROM assistant_reading WHERE
 NOT fb_valid_uuid(bill_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(content_type) OR
 NOT fb_valid_uuid(created_by_id) OR
 NOT fb_valid_text(error) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(mode) OR
 NOT fb_valid_text(model) OR
 NOT fb_valid_uuid(origin_bill_id) OR
 NOT fb_valid_text(original_name) OR
 NOT fb_valid_uuid(request_key) OR
 NOT fb_valid_text(result) OR
 NOT fb_valid_text(sha256) OR
 NOT fb_valid_uuid(source_file_id) OR
 NOT fb_valid_text(status)
UNION ALL
SELECT 'invalid stored value in attachments_attachment' AS problem FROM attachments_attachment WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(content_type) OR
 NOT fb_valid_text(delete_reason) OR
 NOT fb_valid_timestamp(deleted_at) OR
 NOT fb_valid_uuid(deleted_by_id) OR
 NOT fb_valid_text(deleted_by_name) OR
 NOT fb_valid_text(deleted_number) OR
 NOT fb_valid_uuid(document_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(nonce) OR
 NOT fb_valid_text(original_name) OR
 NOT fb_valid_text(original_sha256) OR
 NOT fb_valid_uuid(request_key) OR
 NOT fb_valid_text(sha256) OR
 NOT fb_valid_text(stored_name) OR
 NOT fb_valid_timestamp(uploaded_at) OR
 NOT fb_valid_uuid(uploaded_by_id) OR
 NOT fb_valid_text(uploaded_by_name) OR
 NOT fb_valid_text(uploaded_number) OR
 NOT fb_valid_text(wrap_nonce) OR
 NOT fb_valid_text(wrapped_key)
UNION ALL
SELECT 'invalid stored value in bankfeeds_account' AS problem FROM bankfeeds_account WHERE
 NOT fb_valid_timestamp(balance_at) OR
 NOT fb_valid_text(conn_key) OR
 NOT fb_valid_text(conn_key_identity) OR
 NOT fb_valid_uuid(connection_id) OR
 NOT fb_valid_text(currency) OR
 NOT fb_valid_timestamp(first_seen_at) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_timestamp(last_seen_at) OR
 NOT fb_valid_uuid(ledger_account_id) OR
 NOT fb_valid_timestamp(mapped_at) OR
 NOT fb_valid_text(name) OR
 NOT fb_valid_text(org_name) OR
 NOT fb_valid_text(remote_id) OR
 NOT fb_valid_text(remote_identity)
UNION ALL
SELECT 'invalid stored value in bankfeeds_connection' AS problem FROM bankfeeds_connection WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_uuid(created_by_id) OR
 NOT fb_valid_timestamp(forgotten_at) OR
 NOT fb_valid_text(host) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(label) OR
 NOT fb_valid_timestamp(last_sync_at) OR
 NOT fb_valid_text(last_sync_summary)
UNION ALL
SELECT 'invalid stored value in bankfeeds_counter' AS problem FROM bankfeeds_counter WHERE
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in bankfeeds_identity_secret' AS problem FROM bankfeeds_identity_secret WHERE
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in bankfeeds_retired' AS problem FROM bankfeeds_retired WHERE
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in bankfeeds_rule' AS problem FROM bankfeeds_rule WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(match_text) OR
 NOT fb_valid_text(name) OR
 NOT fb_valid_text(payee_name) OR
 NOT fb_valid_text(sign) OR
 NOT fb_valid_timestamp(updated_at)
UNION ALL
SELECT 'invalid stored value in bankfeeds_transaction' AS problem FROM bankfeeds_transaction WHERE
 NOT fb_valid_text(accepted_as) OR
 NOT fb_valid_timestamp(decided_at) OR
 NOT fb_valid_uuid(decided_by_id) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(feed_account_id) OR
 NOT fb_valid_timestamp(first_seen_at) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(journal_line_id) OR
 NOT fb_valid_text(posted_on) OR
 NOT fb_valid_text(reference) OR
 NOT fb_valid_text(reference_url) OR
 NOT fb_valid_text(remote_id) OR
 NOT fb_valid_text(remote_identity) OR
 NOT fb_valid_text(status) OR
 NOT fb_valid_text(transacted_on) OR
 NOT fb_valid_timestamp(updated_at)
UNION ALL
SELECT 'invalid stored value in banking_banklinestatus' AS problem FROM banking_banklinestatus WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_uuid(changed_by_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_line_id) OR
 NOT fb_valid_uuid(reconciliation_id) OR
 NOT fb_valid_text(state)
UNION ALL
SELECT 'invalid stored value in banking_deposit' AS problem FROM banking_deposit WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id)
UNION ALL
SELECT 'invalid stored value in banking_depositline' AS problem FROM banking_depositline WHERE
 NOT fb_valid_uuid(deposit_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(receipt_id) OR
 NOT fb_valid_uuid(undeposited_id)
UNION ALL
SELECT 'invalid stored value in banking_depositreversal' AS problem FROM banking_depositreversal WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(deposit_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id)
UNION ALL
SELECT 'invalid stored value in banking_reconciliation' AS problem FROM banking_reconciliation WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(request_key) OR
 NOT fb_valid_uuid(started_by_id) OR
 NOT fb_valid_date(statement_date) OR
 NOT fb_valid_text(status)
UNION ALL
SELECT 'invalid stored value in banking_reconciliationactor' AS problem FROM banking_reconciliationactor WHERE
 NOT fb_valid_text(phase) OR
 NOT fb_valid_uuid(reconciliation_id) OR
 NOT fb_valid_uuid(user_id)
UNION ALL
SELECT 'invalid stored value in banking_reconciliationline' AS problem FROM banking_reconciliationline WHERE
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_line_id) OR
 NOT fb_valid_uuid(reconciliation_id)
UNION ALL
SELECT 'invalid stored value in banking_seal' AS problem FROM banking_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in banking_seal_due' AS problem FROM banking_seal_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in checks_check' AS problem FROM checks_check WHERE
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_text(number_key) OR
 NOT fb_valid_text(state) OR
 NOT fb_valid_uuid(vendor_id)
UNION ALL
SELECT 'invalid stored value in checks_checkline' AS problem FROM checks_checkline WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_text(account_snapshot) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(item_id) OR
 NOT fb_valid_text(item_snapshot) OR
 NOT fb_valid_uuid(revision_id) OR
 NOT fb_valid_text(tab)
UNION ALL
SELECT 'invalid stored value in checks_checkrevision' AS problem FROM checks_checkrevision WHERE
 NOT fb_valid_text(action) OR
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_text(address) OR
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_text(bank_snapshot) OR
 NOT fb_valid_text(check_number) OR
 NOT fb_valid_text(company_snapshot) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(document_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(memo) OR
 NOT fb_valid_text(payee_kind) OR
 NOT fb_valid_text(payee_name) OR
 NOT fb_valid_text(payee_snapshot) OR
 NOT fb_valid_text(reason) OR
 NOT fb_valid_uuid(reversal_id) OR
 NOT fb_valid_text(review_digest) OR
 NOT fb_valid_text(state) OR
 NOT fb_valid_uuid(vendor_id)
UNION ALL
SELECT 'invalid stored value in credits_creditapplication' AS problem FROM credits_creditapplication WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(credit_memo_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(request_key)
UNION ALL
SELECT 'invalid stored value in credits_creditapplicationline' AS problem FROM credits_creditapplicationline WHERE
 NOT fb_valid_uuid(application_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(invoice_revision_id) OR
 NOT fb_valid_uuid(receivables_id)
UNION ALL
SELECT 'invalid stored value in credits_creditmemo' AS problem FROM credits_creditmemo WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(collected_tax_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(receivables_id) OR
 NOT fb_valid_uuid(tax_item_id)
UNION ALL
SELECT 'invalid stored value in credits_creditmemoline' AS problem FROM credits_creditmemoline WHERE
 NOT fb_valid_uuid(credit_memo_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_uuid(item_id)
UNION ALL
SELECT 'invalid stored value in credits_creditmemovoid' AS problem FROM credits_creditmemovoid WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(credit_memo_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id)
UNION ALL
SELECT 'invalid stored value in credits_creditrefund' AS problem FROM credits_creditrefund WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_text(check_number) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(credit_memo_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(method) OR
 NOT fb_valid_uuid(receivables_id)
UNION ALL
SELECT 'invalid stored value in credits_creditrefundvoid' AS problem FROM credits_creditrefundvoid WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(refund_id)
UNION ALL
SELECT 'invalid stored value in credits_creditunapplication' AS problem FROM credits_creditunapplication WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(application_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id)
UNION ALL
SELECT 'invalid stored value in credits_seal' AS problem FROM credits_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in credits_seal_due' AS problem FROM credits_seal_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in directory_customer' AS problem FROM directory_customer WHERE
 NOT fb_valid_text(billing_address) OR
 NOT fb_valid_text(business_name) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(contact_name) OR
 NOT fb_valid_text(display_name) OR
 NOT fb_valid_text(email) OR
 NOT fb_valid_text(exemption_number) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(notes) OR
 NOT fb_valid_text(phone) OR
 NOT fb_valid_text(tax_status)
UNION ALL
SELECT 'invalid stored value in directory_item' AS problem FROM directory_item WHERE
 NOT fb_valid_text(code) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(expense_account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in directory_vendor' AS problem FROM directory_vendor WHERE
 NOT fb_valid_text(billing_address) OR
 NOT fb_valid_text(business_name) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(contact_name) OR
 NOT fb_valid_uuid(default_expense_account_id) OR
 NOT fb_valid_text(default_terms) OR
 NOT fb_valid_text(display_name) OR
 NOT fb_valid_text(email) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(notes) OR
 NOT fb_valid_text(phone)
UNION ALL
SELECT 'invalid stored value in documents_accountingsetup' AS problem FROM documents_accountingsetup WHERE
 NOT fb_valid_uuid(collected_tax_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(customer_deposits_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(payables_id) OR
 NOT fb_valid_uuid(receivables_id) OR
 NOT fb_valid_uuid(undeposited_id)
UNION ALL
SELECT 'invalid stored value in documents_designatedbank' AS problem FROM documents_designatedbank WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(setup_id)
UNION ALL
SELECT 'invalid stored value in documents_documentaccount' AS problem FROM documents_documentaccount WHERE
 NOT fb_valid_text(account_code) OR
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_text(account_kind) OR
 NOT fb_valid_text(account_name) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(role) OR
 NOT fb_valid_uuid(source_id)
UNION ALL
SELECT 'invalid stored value in documents_documentidentity' AS problem FROM documents_documentidentity WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_text(currency) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(kind) OR
 NOT fb_valid_text(number) OR
 NOT fb_valid_uuid(request_key)
UNION ALL
SELECT 'invalid stored value in documents_documentsequence' AS problem FROM documents_documentsequence WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in estimate_seal' AS problem FROM estimate_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in estimate_seal_due' AS problem FROM estimate_seal_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in fb_seal' AS problem FROM fb_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in fb_seal_due' AS problem FROM fb_seal_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in fb_user' AS problem FROM fb_user WHERE
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(username)
UNION ALL
SELECT 'invalid stored value in ledger_account' AS problem FROM ledger_account WHERE
 NOT fb_valid_text(code) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_text(detail_type) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(kind) OR
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in ledger_auditevent' AS problem FROM ledger_auditevent WHERE
 NOT fb_valid_text(action) OR
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(detail) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_timestamp(occurred_at) OR
 NOT fb_valid_text(target)
UNION ALL
SELECT 'invalid stored value in ledger_company' AS problem FROM ledger_company WHERE
 NOT fb_valid_text(currency) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in ledger_company_members' AS problem FROM ledger_company_members WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(user_id)
UNION ALL
SELECT 'invalid stored value in ledger_journal' AS problem FROM ledger_journal WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_uuid(created_by_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_text(entry_number) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(memo) OR
 NOT fb_valid_timestamp(posted_at) OR
 NOT fb_valid_uuid(posted_by_id) OR
 NOT fb_valid_uuid(request_key) OR
 NOT fb_valid_uuid(reversal_of_id) OR
 NOT fb_valid_uuid(source_id) OR
 NOT fb_valid_text(status)
UNION ALL
SELECT 'invalid stored value in ledger_journalline' AS problem FROM ledger_journalline WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in payables_bill' AS problem FROM payables_bill WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_text(reference_key) OR
 NOT fb_valid_text(state) OR
 NOT fb_valid_uuid(vendor_id)
UNION ALL
SELECT 'invalid stored value in payables_billline' AS problem FROM payables_billline WHERE
 NOT fb_valid_text(account_snapshot) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(expense_account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_uuid(item_id) OR
 NOT fb_valid_text(item_snapshot) OR
 NOT fb_valid_uuid(revision_id)
UNION ALL
SELECT 'invalid stored value in payables_billpayment' AS problem FROM payables_billpayment WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(company_snapshot) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(memo) OR
 NOT fb_valid_text(method) OR
 NOT fb_valid_text(reference) OR
 NOT fb_valid_text(review_digest) OR
 NOT fb_valid_uuid(vendor_id) OR
 NOT fb_valid_text(vendor_snapshot)
UNION ALL
SELECT 'invalid stored value in payables_billrevision' AS problem FROM payables_billrevision WHERE
 NOT fb_valid_text(action) OR
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_uuid(bill_id) OR
 NOT fb_valid_text(company_snapshot) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_date(due_date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(memo) OR
 NOT fb_valid_text(reason) OR
 NOT fb_valid_text(reentry_reason) OR
 NOT fb_valid_text(reference) OR
 NOT fb_valid_uuid(reversal_id) OR
 NOT fb_valid_text(review_digest) OR
 NOT fb_valid_text(state) OR
 NOT fb_valid_uuid(supersedes_id) OR
 NOT fb_valid_uuid(vendor_id) OR
 NOT fb_valid_text(vendor_snapshot)
UNION ALL
SELECT 'invalid stored value in payables_paymentallocation' AS problem FROM payables_paymentallocation WHERE
 NOT fb_valid_uuid(bill_id) OR
 NOT fb_valid_uuid(bill_revision_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(payables_id) OR
 NOT fb_valid_uuid(payment_id)
UNION ALL
SELECT 'invalid stored value in payables_paymentreversal' AS problem FROM payables_paymentreversal WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(payment_id) OR
 NOT fb_valid_text(reason)
UNION ALL
SELECT 'invalid stored value in plugin_credential' AS problem FROM plugin_credential WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(plugin_id) OR
 NOT fb_valid_uuid(connection_id)
UNION ALL
SELECT 'invalid stored value in prepayments_customerdeposit' AS problem FROM prepayments_customerdeposit WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(estimate_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(liability_id) OR
 NOT fb_valid_text(order_reference) OR
 NOT fb_valid_uuid(receipt_id)
UNION ALL
SELECT 'invalid stored value in prepayments_depositapplication' AS problem FROM prepayments_depositapplication WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(deposit_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(invoice_revision_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(liability_id) OR
 NOT fb_valid_uuid(receivables_id)
UNION ALL
SELECT 'invalid stored value in prepayments_depositrefund' AS problem FROM prepayments_depositrefund WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_text(check_number) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(deposit_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(liability_id) OR
 NOT fb_valid_text(method)
UNION ALL
SELECT 'invalid stored value in prepayments_depositrefundvoid' AS problem FROM prepayments_depositrefundvoid WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(refund_id)
UNION ALL
SELECT 'invalid stored value in prepayments_depositunapplication' AS problem FROM prepayments_depositunapplication WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(application_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id)
UNION ALL
SELECT 'invalid stored value in prepayments_due' AS problem FROM prepayments_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in prepayments_seal' AS problem FROM prepayments_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in sales_estimate' AS problem FROM sales_estimate WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id)
UNION ALL
SELECT 'invalid stored value in sales_estimateconversion' AS problem FROM sales_estimateconversion WHERE
 NOT fb_valid_text(digest) OR
 NOT fb_valid_uuid(estimate_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(revision_id)
UNION ALL
SELECT 'invalid stored value in sales_estimateline' AS problem FROM sales_estimateline WHERE
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(expense_account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_uuid(item_id) OR
 NOT fb_valid_uuid(revision_id)
UNION ALL
SELECT 'invalid stored value in sales_estimaterevision' AS problem FROM sales_estimaterevision WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(estimate_id) OR
 NOT fb_valid_text(expiry) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(state)
UNION ALL
SELECT 'invalid stored value in sales_invoice' AS problem FROM sales_invoice WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id)
UNION ALL
SELECT 'invalid stored value in sales_invoiceline' AS problem FROM sales_invoiceline WHERE
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(expense_account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_uuid(item_id) OR
 NOT fb_valid_uuid(revision_id)
UNION ALL
SELECT 'invalid stored value in sales_invoicerevision' AS problem FROM sales_invoicerevision WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_date(due_date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(reversal_id) OR
 NOT fb_valid_text(state)
UNION ALL
SELECT 'invalid stored value in sales_receipt' AS problem FROM sales_receipt WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_text(destination) OR
 NOT fb_valid_uuid(destination_account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in sales_receiptallocation' AS problem FROM sales_receiptallocation WHERE
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(invoice_revision_id) OR
 NOT fb_valid_uuid(receipt_id) OR
 NOT fb_valid_uuid(receivables_id)
UNION ALL
SELECT 'invalid stored value in sales_receiptreversal' AS problem FROM sales_receiptreversal WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(receipt_id)
UNION ALL
SELECT 'invalid stored value in sales_salesreceipt' AS problem FROM sales_salesreceipt WHERE
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(receipt_id)
UNION ALL
SELECT 'invalid stored value in sales_salesreceiptline' AS problem FROM sales_salesreceiptline WHERE
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_uuid(item_id) OR
 NOT fb_valid_uuid(sale_id)
UNION ALL
SELECT 'invalid stored value in sales_seal' AS problem FROM sales_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in sales_seal_due' AS problem FROM sales_seal_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in salestax_salestaxsettings' AS problem FROM salestax_salestaxsettings WHERE
 NOT fb_valid_text(bulk_step) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(default_item_id) OR
 NOT fb_valid_text(filing_frequency) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(owed_basis)
UNION ALL
SELECT 'invalid stored value in salestax_taxgroupmember' AS problem FROM salestax_taxgroupmember WHERE
 NOT fb_valid_uuid(group_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(member_id)
UNION ALL
SELECT 'invalid stored value in salestax_taxitem' AS problem FROM salestax_taxitem WHERE
 NOT fb_valid_uuid(agency_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(kind) OR
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in transfers_transfer' AS problem FROM transfers_transfer WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(company_snapshot) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(from_account_id) OR
 NOT fb_valid_text(from_snapshot) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(memo) OR
 NOT fb_valid_text(review_digest) OR
 NOT fb_valid_uuid(to_account_id) OR
 NOT fb_valid_text(to_snapshot)
UNION ALL
SELECT 'invalid stored value in transfers_transfervoid' AS problem FROM transfers_transfervoid WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(reason) OR
 NOT fb_valid_uuid(transfer_id)
UNION ALL SELECT 'invalid stored value in company_document_details' AS problem FROM company_document_details WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_text(logo_mime) OR (fb_json_unique_keys(data) AND NOT fb_company_document_valid(data))
UNION ALL
SELECT 'invalid stored value in fb_numbering' AS problem FROM fb_numbering WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(kind) OR
 NOT fb_valid_text(scope) OR
 NOT fb_valid_text(start) OR
 NOT fb_valid_uuid(audit_id)
UNION ALL
SELECT 'invalid stored value in desktop_user' AS problem FROM desktop_user WHERE
 NOT fb_valid_uuid(user_id) OR
 NOT fb_valid_text(role) OR
 NOT fb_valid_text(password_hash)
UNION ALL
SELECT 'invalid stored value in desktop_mfa_totp' FROM desktop_mfa_totp WHERE NOT fb_valid_uuid(user_id)
UNION ALL
SELECT 'invalid stored value in desktop_mfa_recovery' FROM desktop_mfa_recovery WHERE NOT fb_valid_uuid(user_id)
UNION ALL
SELECT 'invalid stored value in invoice_preferences' AS problem FROM invoice_preferences WHERE
 NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(data)
UNION ALL
SELECT 'invalid stored value in invoice_email_connection' AS problem FROM invoice_email_connection WHERE
 NOT fb_valid_text(client_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(from_address) OR
 NOT fb_valid_text(status) OR
 NOT fb_valid_timestamp(updated_at) OR
 NOT fb_valid_uuid(updated_by_id)
UNION ALL
SELECT 'invalid stored value in invoice_email_delivery' AS problem FROM invoice_email_delivery WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_timestamp(completed_at) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_uuid(created_by_id) OR
 NOT fb_valid_text(document_hash) OR
 NOT fb_valid_text(error_code) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(invoice_revision_id) OR
 NOT fb_valid_text(message_key) OR
 NOT fb_valid_text(mode) OR
 NOT fb_valid_text(pdf_sha256) OR
 NOT fb_valid_text(provider_draft) OR
 NOT fb_valid_text(provider_message) OR
 NOT fb_valid_text(state) OR
 NOT fb_valid_text(subject) OR
 NOT fb_valid_text(to_address)
UNION ALL
SELECT 'invalid stored value in invoice_stripe_connection' AS problem FROM invoice_stripe_connection WHERE NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(account_key) OR NOT fb_valid_text(mode) OR NOT fb_valid_text(key_last4) OR NOT fb_valid_text(status) OR NOT fb_valid_timestamp(created_at) OR NOT fb_valid_timestamp(updated_at) OR NOT fb_valid_uuid(updated_by_id)
UNION ALL
SELECT 'invalid stored value in invoice_stripe_link' AS problem FROM invoice_stripe_link WHERE NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_uuid(connection_id) OR NOT fb_valid_uuid(invoice_id) OR NOT fb_valid_uuid(invoice_revision_id) OR NOT fb_valid_uuid(created_by_id) OR NOT fb_valid_text(description) OR NOT fb_valid_text(currency) OR NOT fb_valid_text(state) OR NOT fb_valid_text(provider_key) OR NOT fb_valid_text(payment_url) OR NOT fb_valid_text(cursor) OR NOT fb_valid_timestamp(created_at) OR NOT fb_valid_timestamp(checked_at)
UNION ALL
SELECT 'invalid stored value in invoice_stripe_payment' AS problem FROM invoice_stripe_payment WHERE NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_uuid(connection_id) OR NOT fb_valid_uuid(link_id) OR NOT fb_valid_text(account_key) OR NOT fb_valid_text(mode) OR NOT fb_valid_text(session_key) OR NOT fb_valid_text(intent_key) OR NOT fb_valid_text(currency) OR NOT fb_valid_text(provider_state) OR NOT fb_valid_timestamp(observed_at) OR NOT fb_valid_uuid(receipt_id)
UNION ALL
SELECT 'invalid stored value in bankfeeds_rule_policy' AS problem FROM bankfeeds_rule_policy WHERE NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(data) OR NOT fb_valid_uuid(rule_id) OR NOT fb_valid_text(source_hash) OR NOT fb_valid_text(source_key)
UNION ALL
SELECT 'invalid stored value in bankfeeds_rule_review' AS problem FROM bankfeeds_rule_review WHERE NOT fb_valid_uuid(item_id) OR NOT fb_valid_text(reason)
UNION ALL
SELECT 'invalid stored value in bankfeeds_auto_queue' AS problem FROM bankfeeds_auto_queue WHERE NOT fb_valid_text(fingerprint) OR NOT fb_valid_uuid(item_id) OR NOT fb_valid_uuid(rule_id)
UNION ALL SELECT 'sales_importinvoice:'||invoice_id FROM sales_importinvoice WHERE NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(source) OR NOT fb_valid_uuid(source_id) OR NOT fb_valid_text(source_number) OR NOT fb_valid_uuid(invoice_id) OR NOT fb_valid_uuid(revision_id) OR NOT fb_valid_text(digest) OR NOT fb_valid_text(data) OR NOT fb_valid_uuid(actor_id) OR NOT fb_valid_uuid(audit_id)
UNION ALL SELECT 'sales_importpayment:'||invoice_id FROM sales_importpayment WHERE NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(source) OR NOT fb_valid_uuid(source_id) OR NOT fb_valid_uuid(invoice_id) OR NOT fb_valid_uuid(receipt_id) OR NOT fb_valid_text(external_key)
UNION ALL SELECT 'sales_importseal:'||invoice_id FROM sales_importseal WHERE NOT fb_valid_uuid(invoice_id)
UNION ALL SELECT 'salestax_payment:'||id FROM salestax_payment WHERE NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_uuid(identity_id) OR NOT fb_valid_text(creation_digest) OR NOT fb_valid_text(review_digest) OR NOT fb_valid_uuid(agency_id) OR NOT fb_valid_text(agency_name) OR NOT fb_valid_uuid(bank_id) OR NOT fb_valid_uuid(tax_account_id) OR NOT fb_valid_uuid(expense_account_id) OR NOT fb_valid_date(date) OR NOT fb_valid_date(period_start_date) OR NOT fb_valid_date(period_end_date) OR NOT fb_valid_text(reference) OR NOT fb_valid_text(memo) OR NOT fb_valid_uuid(journal_id) OR NOT fb_valid_uuid(actor_id) OR NOT fb_valid_text(actor_name) OR NOT fb_valid_timestamp(created_at)
UNION ALL SELECT 'salestax_paymentvoid:'||id FROM salestax_paymentvoid WHERE NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(payment_id) OR NOT fb_valid_uuid(journal_id) OR NOT fb_valid_date(date) OR NOT fb_valid_text(reason) OR NOT fb_valid_uuid(actor_id) OR NOT fb_valid_text(actor_name) OR NOT fb_valid_timestamp(created_at)
UNION ALL SELECT 'attachments_keywrap:'||company_id||':'||stored_name FROM attachments_keywrap WHERE NOT fb_valid_uuid(audit_id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(stored_name) OR NOT fb_valid_text(wrap_nonce) OR NOT fb_valid_text(wrapped_key)
UNION ALL SELECT 'invalid stored value in payables_recurring' AS problem FROM payables_recurring WHERE NOT fb_valid_uuid(source_bill_id) OR NOT fb_valid_date(source_date) OR NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(name) OR NOT fb_valid_text(frequency) OR NOT fb_valid_date(start_date) OR NOT fb_valid_date(end_date) OR NOT fb_valid_text(data)
UNION ALL SELECT 'invalid stored value in payables_recurring_occurrence' AS problem FROM payables_recurring_occurrence WHERE NOT fb_valid_uuid(company_id) OR NOT fb_valid_uuid(template_id) OR NOT fb_valid_date(occurrence_date) OR NOT fb_valid_uuid(bill_id)
UNION ALL
SELECT 'invalid stored value in jobs_billing' AS problem FROM jobs_billing WHERE
 NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_uuid(entry_id) OR NOT fb_valid_uuid(invoice_id) OR NOT fb_valid_uuid(ticket_id) OR NOT fb_valid_uuid(customer_id) OR NOT fb_valid_uuid(created_by_id) OR NOT fb_valid_timestamp(created_at)
UNION ALL
SELECT 'invalid stored value in jobs_billingstep' AS problem FROM jobs_billingstep WHERE
 NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_uuid(billing_id) OR NOT fb_valid_uuid(invoice_id) OR NOT fb_valid_uuid(created_by_id) OR NOT fb_valid_timestamp(created_at)
UNION ALL
SELECT 'invalid stored value in jobs_ticket' AS problem FROM jobs_ticket WHERE
 NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(number) OR NOT fb_valid_uuid(request_key) OR NOT fb_valid_uuid(customer_id) OR NOT fb_valid_text(title) OR NOT fb_valid_text(description) OR NOT fb_valid_text(status) OR NOT fb_valid_date(opened_date) OR NOT fb_valid_date(due_date) OR NOT fb_valid_date(finished_date) OR NOT fb_valid_uuid(created_by_id) OR NOT fb_valid_timestamp(created_at) OR NOT fb_valid_timestamp(updated_at)
UNION ALL
SELECT 'invalid stored value in jobs_timeentry' AS problem FROM jobs_timeentry WHERE
 NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_uuid(request_key) OR NOT fb_valid_uuid(customer_id) OR NOT fb_valid_uuid(ticket_id) OR NOT fb_valid_uuid(item_id) OR NOT fb_valid_date(date) OR NOT fb_valid_text(notes) OR NOT fb_valid_uuid(worker_id) OR NOT fb_valid_uuid(created_by_id) OR NOT fb_valid_timestamp(created_at) OR NOT fb_valid_timestamp(updated_at)
UNION ALL
SELECT 'invalid stored value in desktop_ai_unattended' AS problem FROM desktop_ai_unattended WHERE
 NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(actor_id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_uuid(anchor_id)
 OR length(verifier) <> 32
UNION ALL
SELECT 'invalid stored value in bankfeeds_transaction_match' AS problem FROM bankfeeds_transaction_match WHERE
 NOT fb_valid_uuid(journal_id) OR NOT fb_valid_uuid(journal_line_id) OR NOT fb_valid_text(parent_status) OR NOT fb_valid_uuid(transaction_id);
