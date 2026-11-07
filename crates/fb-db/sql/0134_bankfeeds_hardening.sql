-- Bank feeds FIX1 (review of 0133): byte-exact text, canonical UUIDs, Python-exact dates and round-trip
-- timestamps; permanently retired transaction identities; decision eligibility equal to the services;
-- activation checked like mapping; and an open-time check that accepts legitimate account history.
-- fb_bankfeeds_uuid/date/timestamp/text are the Rust UDFs in fb-db/src/bankfeeds_text.rs (NULL passes the
-- uuid/date/timestamp forms; nullable columns keep their own NULL rules from 0133).

-- 1. Text, keys, dates and timestamps, at every write.
CREATE TRIGGER fb_bankfeeds_connection_text_insert BEFORE INSERT ON bankfeeds_connection BEGIN
 SELECT RAISE(ABORT,'bankfeeds_text: Invalid bank-feed text, key, date or timestamp') WHERE NOT (fb_bankfeeds_uuid(NEW.id) AND NEW.id IS NOT NULL AND fb_bankfeeds_uuid(NEW.company_id) AND fb_bankfeeds_uuid(NEW.created_by_id)
  AND fb_bankfeeds_text(NEW.label,1,120) AND fb_bankfeeds_text(NEW.host,1,253) AND fb_bankfeeds_text(NEW.last_sync_summary,0,500)
  AND fb_bankfeeds_timestamp(NEW.created_at) AND NEW.created_at IS NOT NULL AND fb_bankfeeds_timestamp(NEW.forgotten_at) AND fb_bankfeeds_timestamp(NEW.last_sync_at));
END;
CREATE TRIGGER fb_bankfeeds_connection_text_update BEFORE UPDATE ON bankfeeds_connection BEGIN
 SELECT RAISE(ABORT,'bankfeeds_text: Invalid bank-feed text, key, date or timestamp') WHERE NOT (fb_bankfeeds_uuid(NEW.id) AND NEW.id IS NOT NULL AND fb_bankfeeds_uuid(NEW.company_id) AND fb_bankfeeds_uuid(NEW.created_by_id)
  AND fb_bankfeeds_text(NEW.label,1,120) AND fb_bankfeeds_text(NEW.host,1,253) AND fb_bankfeeds_text(NEW.last_sync_summary,0,500)
  AND fb_bankfeeds_timestamp(NEW.created_at) AND NEW.created_at IS NOT NULL AND fb_bankfeeds_timestamp(NEW.forgotten_at) AND fb_bankfeeds_timestamp(NEW.last_sync_at));
END;
CREATE TRIGGER fb_bankfeeds_account_text_insert BEFORE INSERT ON bankfeeds_account BEGIN
 SELECT RAISE(ABORT,'bankfeeds_text: Invalid bank-feed text, key, date or timestamp') WHERE NOT (fb_bankfeeds_uuid(NEW.id) AND NEW.id IS NOT NULL AND fb_bankfeeds_uuid(NEW.connection_id) AND fb_bankfeeds_uuid(NEW.ledger_account_id)
  AND fb_bankfeeds_text(NEW.conn_key,0,200) AND fb_bankfeeds_text(NEW.remote_id,1,200) AND fb_bankfeeds_text(NEW.name,1,120) AND fb_bankfeeds_text(NEW.org_name,0,120) AND fb_bankfeeds_text(NEW.currency,1,200)
  AND fb_bankfeeds_timestamp(NEW.balance_at) AND fb_bankfeeds_timestamp(NEW.mapped_at) AND fb_bankfeeds_timestamp(NEW.first_seen_at) AND NEW.first_seen_at IS NOT NULL AND fb_bankfeeds_timestamp(NEW.last_seen_at));
END;
CREATE TRIGGER fb_bankfeeds_account_text_update BEFORE UPDATE ON bankfeeds_account BEGIN
 SELECT RAISE(ABORT,'bankfeeds_text: Invalid bank-feed text, key, date or timestamp') WHERE NOT (fb_bankfeeds_uuid(NEW.id) AND NEW.id IS NOT NULL AND fb_bankfeeds_uuid(NEW.connection_id) AND fb_bankfeeds_uuid(NEW.ledger_account_id)
  AND fb_bankfeeds_text(NEW.conn_key,0,200) AND fb_bankfeeds_text(NEW.remote_id,1,200) AND fb_bankfeeds_text(NEW.name,1,120) AND fb_bankfeeds_text(NEW.org_name,0,120) AND fb_bankfeeds_text(NEW.currency,1,200)
  AND fb_bankfeeds_timestamp(NEW.balance_at) AND fb_bankfeeds_timestamp(NEW.mapped_at) AND fb_bankfeeds_timestamp(NEW.first_seen_at) AND NEW.first_seen_at IS NOT NULL AND fb_bankfeeds_timestamp(NEW.last_seen_at));
END;
CREATE TRIGGER fb_bankfeeds_transaction_text_insert BEFORE INSERT ON bankfeeds_transaction BEGIN
 SELECT RAISE(ABORT,'bankfeeds_text: Invalid bank-feed text, key, date or timestamp') WHERE NOT (fb_bankfeeds_uuid(NEW.id) AND NEW.id IS NOT NULL AND fb_bankfeeds_uuid(NEW.feed_account_id) AND fb_bankfeeds_uuid(NEW.journal_id) AND fb_bankfeeds_uuid(NEW.journal_line_id) AND fb_bankfeeds_uuid(NEW.decided_by_id)
  AND fb_bankfeeds_text(NEW.remote_id,1,200) AND fb_bankfeeds_text(NEW.description,0,500) AND fb_bankfeeds_text(NEW.reference,0,60) AND fb_bankfeeds_text(NEW.reference_url,0,200)
  AND fb_bankfeeds_date(NEW.posted_on) AND fb_bankfeeds_date(NEW.transacted_on)
  AND fb_bankfeeds_timestamp(NEW.first_seen_at) AND NEW.first_seen_at IS NOT NULL AND fb_bankfeeds_timestamp(NEW.updated_at) AND NEW.updated_at IS NOT NULL AND fb_bankfeeds_timestamp(NEW.decided_at));
END;
CREATE TRIGGER fb_bankfeeds_transaction_text_update BEFORE UPDATE ON bankfeeds_transaction BEGIN
 SELECT RAISE(ABORT,'bankfeeds_text: Invalid bank-feed text, key, date or timestamp') WHERE NOT (fb_bankfeeds_uuid(NEW.id) AND NEW.id IS NOT NULL AND fb_bankfeeds_uuid(NEW.feed_account_id) AND fb_bankfeeds_uuid(NEW.journal_id) AND fb_bankfeeds_uuid(NEW.journal_line_id) AND fb_bankfeeds_uuid(NEW.decided_by_id)
  AND fb_bankfeeds_text(NEW.remote_id,1,200) AND fb_bankfeeds_text(NEW.description,0,500) AND fb_bankfeeds_text(NEW.reference,0,60) AND fb_bankfeeds_text(NEW.reference_url,0,200)
  AND fb_bankfeeds_date(NEW.posted_on) AND fb_bankfeeds_date(NEW.transacted_on)
  AND fb_bankfeeds_timestamp(NEW.first_seen_at) AND NEW.first_seen_at IS NOT NULL AND fb_bankfeeds_timestamp(NEW.updated_at) AND NEW.updated_at IS NOT NULL AND fb_bankfeeds_timestamp(NEW.decided_at));
END;
CREATE TRIGGER fb_bankfeeds_rule_text_insert BEFORE INSERT ON bankfeeds_rule BEGIN
 SELECT RAISE(ABORT,'bankfeeds_text: Invalid bank-feed text, key, date or timestamp') WHERE NOT (fb_bankfeeds_uuid(NEW.id) AND NEW.id IS NOT NULL AND fb_bankfeeds_uuid(NEW.company_id) AND fb_bankfeeds_uuid(NEW.account_id)
  AND fb_bankfeeds_text(NEW.name,1,120) AND fb_bankfeeds_text(NEW.match_text,1,200) AND fb_bankfeeds_text(NEW.payee_name,0,120)
  AND fb_bankfeeds_timestamp(NEW.created_at) AND NEW.created_at IS NOT NULL AND fb_bankfeeds_timestamp(NEW.updated_at) AND NEW.updated_at IS NOT NULL);
END;
CREATE TRIGGER fb_bankfeeds_rule_text_update BEFORE UPDATE ON bankfeeds_rule BEGIN
 SELECT RAISE(ABORT,'bankfeeds_text: Invalid bank-feed text, key, date or timestamp') WHERE NOT (fb_bankfeeds_uuid(NEW.id) AND NEW.id IS NOT NULL AND fb_bankfeeds_uuid(NEW.company_id) AND fb_bankfeeds_uuid(NEW.account_id)
  AND fb_bankfeeds_text(NEW.name,1,120) AND fb_bankfeeds_text(NEW.match_text,1,200) AND fb_bankfeeds_text(NEW.payee_name,0,120)
  AND fb_bankfeeds_timestamp(NEW.created_at) AND NEW.created_at IS NOT NULL AND fb_bankfeeds_timestamp(NEW.updated_at) AND NEW.updated_at IS NOT NULL);
END;
CREATE VIEW bankfeeds_text_invalid AS
 SELECT id FROM bankfeeds_connection WHERE NOT (fb_bankfeeds_uuid(id) AND fb_bankfeeds_uuid(company_id) AND fb_bankfeeds_uuid(created_by_id)
  AND fb_bankfeeds_text(label,1,120) AND fb_bankfeeds_text(host,1,253) AND fb_bankfeeds_text(last_sync_summary,0,500)
  AND fb_bankfeeds_timestamp(created_at) AND fb_bankfeeds_timestamp(forgotten_at) AND fb_bankfeeds_timestamp(last_sync_at))
 UNION ALL SELECT id FROM bankfeeds_account WHERE NOT (fb_bankfeeds_uuid(id) AND fb_bankfeeds_uuid(connection_id) AND fb_bankfeeds_uuid(ledger_account_id)
  AND fb_bankfeeds_text(conn_key,0,200) AND fb_bankfeeds_text(remote_id,1,200) AND fb_bankfeeds_text(name,1,120) AND fb_bankfeeds_text(org_name,0,120) AND fb_bankfeeds_text(currency,1,200)
  AND fb_bankfeeds_timestamp(balance_at) AND fb_bankfeeds_timestamp(mapped_at) AND fb_bankfeeds_timestamp(first_seen_at) AND fb_bankfeeds_timestamp(last_seen_at))
 UNION ALL SELECT id FROM bankfeeds_transaction WHERE NOT (fb_bankfeeds_uuid(id) AND fb_bankfeeds_uuid(feed_account_id) AND fb_bankfeeds_uuid(journal_id) AND fb_bankfeeds_uuid(journal_line_id) AND fb_bankfeeds_uuid(decided_by_id)
  AND fb_bankfeeds_text(remote_id,1,200) AND fb_bankfeeds_text(description,0,500) AND fb_bankfeeds_text(reference,0,60) AND fb_bankfeeds_text(reference_url,0,200)
  AND fb_bankfeeds_date(posted_on) AND fb_bankfeeds_date(transacted_on) AND fb_bankfeeds_timestamp(first_seen_at) AND fb_bankfeeds_timestamp(updated_at) AND fb_bankfeeds_timestamp(decided_at))
 UNION ALL SELECT id FROM bankfeeds_rule WHERE NOT (fb_bankfeeds_uuid(id) AND fb_bankfeeds_uuid(company_id) AND fb_bankfeeds_uuid(account_id)
  AND fb_bankfeeds_text(name,1,120) AND fb_bankfeeds_text(match_text,1,200) AND fb_bankfeeds_text(payee_name,0,120)
  AND fb_bankfeeds_timestamp(created_at) AND fb_bankfeeds_timestamp(updated_at));

-- 2. A withdrawn transaction's identity is retired for good: audit rows keep naming that one transaction.
CREATE TABLE bankfeeds_retired(id TEXT PRIMARY KEY CHECK(length(CAST(id AS BLOB))=36 AND fb_bankfeeds_uuid(id)), kind TEXT NOT NULL CHECK(kind='transaction')) WITHOUT ROWID, STRICT;
-- Rows withdrawn before this migration left audit references; retire those identities too.
INSERT INTO bankfeeds_retired(id,kind) SELECT DISTINCT substr(e.target,21),'transaction' FROM ledger_auditevent e
 WHERE substr(e.target,1,20)='bankfeedtransaction:' AND fb_bankfeeds_uuid(substr(e.target,21)) AND length(CAST(substr(e.target,21) AS BLOB))=36
   AND NOT EXISTS(SELECT 1 FROM bankfeeds_transaction t WHERE t.id=substr(e.target,21));
CREATE TRIGGER fb_bankfeeds_retired_insert BEFORE INSERT ON bankfeeds_retired BEGIN
 SELECT RAISE(ABORT,'bankfeeds_retired: Only a withdrawn transaction identity is retired, once') WHERE EXISTS(SELECT 1 FROM bankfeeds_transaction WHERE id=NEW.id) OR EXISTS(SELECT 1 FROM bankfeeds_retired WHERE id=NEW.id);
END;
CREATE TRIGGER fb_bankfeeds_retired_update BEFORE UPDATE ON bankfeeds_retired
BEGIN SELECT RAISE(ABORT,'bankfeeds_retired: Retired identities are permanent'); END;
CREATE TRIGGER fb_bankfeeds_retired_delete BEFORE DELETE ON bankfeeds_retired
BEGIN SELECT RAISE(ABORT,'bankfeeds_retired: Retired identities are permanent'); END;
CREATE TRIGGER fb_bankfeeds_transaction_withdrawn AFTER DELETE ON bankfeeds_transaction
BEGIN INSERT INTO bankfeeds_retired(id,kind) VALUES(OLD.id,'transaction'); END;
CREATE TRIGGER fb_bankfeeds_transaction_identity BEFORE INSERT ON bankfeeds_transaction BEGIN
 SELECT RAISE(ABORT,'bankfeeds_key: A withdrawn transaction identity is never reused') WHERE EXISTS(SELECT 1 FROM bankfeeds_retired WHERE id=NEW.id);
END;

-- 3. Decision eligibility equals the services: ignore/accept/match by an active member of the company;
--    a match within MATCH_WINDOW_DAYS of the item's date; never a new decision on a reversed entry.
--    Only the transition into a decision is checked, so a later void/reversal stays valid history.
CREATE TRIGGER fb_bankfeeds_transaction_eligible BEFORE UPDATE ON bankfeeds_transaction
WHEN NEW.status IN('IGNORED','ACCEPTED','MATCHED') AND NEW.status IS NOT OLD.status AND OLD.status NOT IN('ACCEPTED','MATCHED') BEGIN
 SELECT RAISE(ABORT,'bankfeeds_decision: A decision is made by an active member of the company') WHERE NOT EXISTS(
   SELECT 1 FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id JOIN ledger_company_members m ON m.company_id=c.company_id JOIN fb_user u ON u.id=m.user_id
   WHERE f.id=NEW.feed_account_id AND m.user_id=NEW.decided_by_id AND u.is_active=1);
 SELECT RAISE(ABORT,'bankfeeds_decision: A match names an entry dated within 5 days of the transaction') WHERE NEW.status='MATCHED' AND NOT EXISTS(
   SELECT 1 FROM ledger_journal j WHERE j.id=NEW.journal_id AND fb_bankfeeds_date(j.date) AND fb_bankfeeds_date(coalesce(NEW.posted_on,NEW.transacted_on))
     AND abs(julianday(j.date)-julianday(coalesce(NEW.posted_on,NEW.transacted_on)))<=5);
 SELECT RAISE(ABORT,'bankfeeds_decision: A decision never names a reversed entry') WHERE NEW.status IN('ACCEPTED','MATCHED') AND EXISTS(SELECT 1 FROM ledger_journal r WHERE r.reversal_of_id=NEW.journal_id);
END;

-- 4. Reactivating a mapped feed is a mapping change: one active feed per ledger account and company.
CREATE TRIGGER fb_bankfeeds_account_activation BEFORE UPDATE ON bankfeeds_account WHEN NEW.active=1 AND OLD.active IS NOT 1 AND NEW.ledger_account_id IS NOT NULL BEGIN
 SELECT RAISE(ABORT,'bankfeeds_mapping: Map a feed to an eligible unmapped bank or credit card account of its company') WHERE EXISTS(
   SELECT 1 FROM bankfeeds_account f JOIN bankfeeds_connection fc ON fc.id=f.connection_id JOIN bankfeeds_connection nc ON nc.company_id=fc.company_id
   WHERE nc.id=NEW.connection_id AND f.id IS NOT NEW.id AND f.ledger_account_id=NEW.ledger_account_id AND f.active=1);
END;

-- 5. Open-time evidence. Mapping eligibility (kind, active, postable, control role) is checked when a
--    mapping is made; the services let an unused account change kind or be deactivated afterwards, as
--    Python does, so history keeps only ownership. Live duplicates and reused identities are refused.
DROP VIEW bankfeeds_invalid;
CREATE VIEW bankfeeds_invalid AS
 SELECT t.id FROM bankfeeds_transaction t WHERE t.status IN('ACCEPTED','MATCHED') AND NOT EXISTS(
   SELECT 1 FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id JOIN ledger_journal j ON j.id=t.journal_id JOIN ledger_journalline l ON l.id=t.journal_line_id
   WHERE f.id=t.feed_account_id AND j.company_id=c.company_id AND j.status='POSTED' AND l.journal_id=j.id
     AND l.debit=(CASE WHEN t.amount>0 THEN t.amount ELSE 0 END) AND l.credit=(CASE WHEN t.amount>0 THEN 0 ELSE -t.amount END))
 UNION ALL SELECT t.id FROM bankfeeds_transaction t WHERE t.ordinal>coalesce((SELECT value FROM bankfeeds_counter WHERE name='transaction'),0)
 UNION ALL SELECT f.id FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id LEFT JOIN ledger_account a ON a.id=f.ledger_account_id WHERE f.ledger_account_id IS NOT NULL AND (a.id IS NULL OR a.company_id<>c.company_id)
 UNION ALL SELECT f.id FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE f.active=1 AND f.ledger_account_id IS NOT NULL AND EXISTS(
   SELECT 1 FROM bankfeeds_account g JOIN bankfeeds_connection d ON d.id=g.connection_id WHERE d.company_id=c.company_id AND g.id<>f.id AND g.active=1 AND g.ledger_account_id=f.ledger_account_id)
 UNION ALL SELECT r.id FROM bankfeeds_rule r LEFT JOIN ledger_account a ON a.id=r.account_id WHERE a.id IS NULL OR a.company_id<>r.company_id
 UNION ALL SELECT c.id FROM bankfeeds_connection c WHERE NOT EXISTS(SELECT 1 FROM fb_user WHERE id=c.created_by_id)
 UNION ALL SELECT t.id FROM bankfeeds_transaction t WHERE t.decided_by_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM fb_user WHERE id=t.decided_by_id)
 UNION ALL SELECT t.id FROM bankfeeds_transaction t WHERE EXISTS(SELECT 1 FROM bankfeeds_retired x WHERE x.id=t.id)
 UNION ALL SELECT x.id FROM bankfeeds_retired x WHERE NOT (length(CAST(x.id AS BLOB))=36 AND fb_bankfeeds_uuid(x.id));
