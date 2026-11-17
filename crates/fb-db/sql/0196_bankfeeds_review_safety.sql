-- Additive (review 2026-11-10): bank-feed review safety. No existing row or object changes.
-- Rows are keyed by the book-wide transaction number (`ordinal`, UNIQUE and never reused), so these tables
-- hold integers only.
--
-- bankfeeds_link: "Link to existing". A re-downloaded transaction (for example after a connection was
-- replaced) that an earlier feed row of another downloaded account already recorded is marked handled
-- without posting: the row is IGNORED and linked to that earlier ACCEPTED / MATCHED row, whose posted line
-- is on the same ledger account with the same amount. Returning the row to review removes the link.
CREATE TABLE bankfeeds_link(
 item_ordinal INTEGER PRIMARY KEY REFERENCES bankfeeds_transaction(ordinal) DEFERRABLE INITIALLY DEFERRED,
 earlier_ordinal INTEGER NOT NULL REFERENCES bankfeeds_transaction(ordinal) DEFERRABLE INITIALLY DEFERRED,
 CHECK(earlier_ordinal<>item_ordinal)
) STRICT, WITHOUT ROWID;
-- bankfeeds_bank_change: the bank's later amount for a decided row (accepted, matched or linked), which the
-- books keep as decided. `kept`=1 once the user chose to keep the recorded amount for this bank amount.
CREATE TABLE bankfeeds_bank_change(
 item_ordinal INTEGER PRIMARY KEY REFERENCES bankfeeds_transaction(ordinal) DEFERRABLE INITIALLY DEFERRED,
 amount INTEGER NOT NULL CHECK(amount BETWEEN -9999999999999999 AND 9999999999999999),
 kept INTEGER NOT NULL CHECK(kept IN(0,1))
) STRICT, WITHOUT ROWID;
-- Open-time structure: a link's row is IGNORED, both rows exist in one company on different downloaded
-- accounts. (The earlier entry may later be voided or returned; the link then stays as the user's decision
-- and the review screen offers to return the row.) A bank change belongs to a decided row and differs.
CREATE VIEW bankfeeds_safety_invalid AS
 SELECT 'link:'||k.item_ordinal AS id FROM bankfeeds_link k
  LEFT JOIN bankfeeds_transaction t ON t.ordinal=k.item_ordinal LEFT JOIN bankfeeds_account f ON f.id=t.feed_account_id LEFT JOIN bankfeeds_connection c ON c.id=f.connection_id
  LEFT JOIN bankfeeds_transaction e ON e.ordinal=k.earlier_ordinal LEFT JOIN bankfeeds_account ef ON ef.id=e.feed_account_id LEFT JOIN bankfeeds_connection ec ON ec.id=ef.connection_id
  WHERE t.status IS NOT 'IGNORED' OR c.company_id IS NULL OR ec.company_id IS NOT c.company_id OR e.feed_account_id IS t.feed_account_id
 UNION ALL
 SELECT 'change:'||g.item_ordinal FROM bankfeeds_bank_change g LEFT JOIN bankfeeds_transaction t ON t.ordinal=g.item_ordinal
  WHERE t.id IS NULL OR (t.status NOT IN('ACCEPTED','MATCHED') AND NOT EXISTS(SELECT 1 FROM bankfeeds_link k WHERE k.item_ordinal=g.item_ordinal)) OR g.amount=t.amount;
-- At link time the earlier row is accepted or matched and its posted, unreversed line is on this row's mapped
-- ledger account with the same amount in the same direction.
CREATE TRIGGER fb_bankfeeds_link_insert AFTER INSERT ON bankfeeds_link BEGIN
 SELECT RAISE(ABORT,'bankfeeds_link: A linked transaction is ignored and names an earlier accepted transaction of another downloaded account with the same amount on the same ledger account')
 WHERE EXISTS(SELECT 1 FROM bankfeeds_safety_invalid WHERE id='link:'||NEW.item_ordinal)
  OR NOT EXISTS(SELECT 1 FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_transaction e ON e.ordinal=NEW.earlier_ordinal
   JOIN ledger_journalline l ON l.id=e.journal_line_id JOIN ledger_journal j ON j.id=l.journal_id
   WHERE t.ordinal=NEW.item_ordinal AND e.status IN('ACCEPTED','MATCHED') AND e.amount=t.amount AND l.account_id IS f.ledger_account_id
    AND l.debit=CASE WHEN t.amount>0 THEN t.amount ELSE 0 END AND l.credit=CASE WHEN t.amount<0 THEN -t.amount ELSE 0 END
    AND j.status='POSTED' AND NOT EXISTS(SELECT 1 FROM ledger_journal r WHERE r.reversal_of_id=j.id));
END;
CREATE TRIGGER fb_bankfeeds_link_fixed BEFORE UPDATE ON bankfeeds_link BEGIN
 SELECT RAISE(ABORT,'bankfeeds_link_fixed: Return the transaction to review to change its link');
END;
-- A linked row cannot leave IGNORED while its link remains: returning it removes the link first.
CREATE TRIGGER fb_bankfeeds_link_status BEFORE UPDATE OF status ON bankfeeds_transaction
 WHEN OLD.status='IGNORED' AND NEW.status<>'IGNORED' AND EXISTS(SELECT 1 FROM bankfeeds_link WHERE item_ordinal=OLD.ordinal) BEGIN
 SELECT RAISE(ABORT,'bankfeeds_link_status: Remove the link in the same change that returns the transaction to review');
END;
CREATE TRIGGER fb_bankfeeds_bank_change_valid AFTER INSERT ON bankfeeds_bank_change BEGIN
 SELECT RAISE(ABORT,'bankfeeds_bank_change: A bank change records a different amount for a decided transaction')
 WHERE EXISTS(SELECT 1 FROM bankfeeds_safety_invalid WHERE id='change:'||NEW.item_ordinal);
END;
CREATE TRIGGER fb_bankfeeds_bank_change_update AFTER UPDATE ON bankfeeds_bank_change BEGIN
 SELECT RAISE(ABORT,'bankfeeds_bank_change: A bank change records a different amount for a decided transaction')
 WHERE NEW.item_ordinal<>OLD.item_ordinal OR EXISTS(SELECT 1 FROM bankfeeds_safety_invalid WHERE id='change:'||NEW.item_ordinal);
END;
