-- Bank feeds FIX5 (review 5 of FIX4): provider identity is kept apart from provider display text.
-- FIX4 stores every provider id redacted, so two ids that differ only in URL userinfo became one stored id,
-- and the UNIQUE keys and lookups on that text merged them (a second transaction vanished; a second bank
-- account inherited the first one's ledger mapping). Every identifier now carries a stable, credential-free
-- identity key: lowercase hex HMAC-SHA256 of the RAW provider value (fb_db::bankfeeds_text::identity_key)
-- under a per-book random secret kept in this encrypted book. Uniqueness, dedupe, mapping and lookup use
-- the keys; conn_key / remote_id keep the redacted text, for display and audit only. Raw values are never
-- stored or audited.
-- Domains: connection-key (bankfeeds_account.conn_key), account (bankfeeds_account.remote_id),
-- transaction (bankfeeds_transaction.remote_id).
-- fb_bankfeeds_identity(secret, domain, value) and fb_bankfeeds_identity_form(text) are Rust UDFs
-- (fb-db/src/bankfeeds_text.rs).

-- 1. The per-book identity secret: one row, created by the core on first use (or below, when rows already
--    exist), never replaced, changed or deleted (a REPLACE deletes first, so it is refused too).
CREATE TABLE bankfeeds_identity_secret(
 name TEXT PRIMARY KEY CHECK(name='identity'),
 secret BLOB NOT NULL CHECK(length(secret)=32)
) WITHOUT ROWID, STRICT;
CREATE TRIGGER fb_bankfeeds_identity_secret_insert BEFORE INSERT ON bankfeeds_identity_secret BEGIN
 SELECT RAISE(ABORT,'bankfeeds_identity: The book keeps one permanent identity secret') WHERE EXISTS(SELECT 1 FROM bankfeeds_identity_secret);
END;
CREATE TRIGGER fb_bankfeeds_identity_secret_update BEFORE UPDATE ON bankfeeds_identity_secret
BEGIN SELECT RAISE(ABORT,'bankfeeds_identity: The book keeps one permanent identity secret'); END;
CREATE TRIGGER fb_bankfeeds_identity_secret_delete BEFORE DELETE ON bankfeeds_identity_secret
BEGIN SELECT RAISE(ABORT,'bankfeeds_identity: The book keeps one permanent identity secret'); END;
-- Backfill: rows stored before this migration hold FIX4's already-redacted ids (or, before FIX4, the
-- parser's text), and the raw value is gone. Their keys are derived from what is stored, so ids that FIX4
-- already merged stay merged (one row each: nothing to split) and every later pull of a raw id whose
-- redacted form equals the stored text and holds no userinfo (every ordinary id) finds its row again.
INSERT INTO bankfeeds_identity_secret(name,secret) SELECT 'identity',randomblob(32) WHERE EXISTS(SELECT 1 FROM bankfeeds_account);

-- 2. Rebuild the account and transaction tables: their UNIQUE keys were on the redacted text. The rows are
--    copied aside, the tables (with their triggers and index) dropped, child first, so no foreign key
--    ever points at a missing parent, and recreated under the same names with identity keys.
DROP VIEW bankfeeds_invalid;
DROP VIEW bankfeeds_text_invalid;
CREATE TABLE fbtmp_bankfeeds_account AS SELECT * FROM bankfeeds_account;
CREATE TABLE fbtmp_bankfeeds_transaction AS SELECT * FROM bankfeeds_transaction;
DROP TABLE bankfeeds_transaction;
DROP TABLE bankfeeds_account;
CREATE TABLE bankfeeds_account(
 id TEXT PRIMARY KEY CHECK(length(id)=36),
 connection_id TEXT NOT NULL REFERENCES bankfeeds_connection(id) DEFERRABLE INITIALLY DEFERRED,
 conn_key TEXT NOT NULL CHECK(length(conn_key)<=200),
 remote_id TEXT NOT NULL CHECK(length(remote_id) BETWEEN 1 AND 200),
 name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 120),
 org_name TEXT NOT NULL CHECK(length(org_name)<=120),
 currency TEXT NOT NULL CHECK(length(currency) BETWEEN 1 AND 200),
 balance INTEGER CHECK(balance IS NULL OR balance BETWEEN -9999999999999999 AND 9999999999999999),
 available_balance INTEGER CHECK(available_balance IS NULL OR available_balance BETWEEN -9999999999999999 AND 9999999999999999),
 balance_at TEXT CHECK(balance_at IS NULL OR length(balance_at) BETWEEN 20 AND 40),
 ledger_account_id TEXT REFERENCES ledger_account(id) DEFERRABLE INITIALLY DEFERRED,
 mapped_at TEXT CHECK(mapped_at IS NULL OR length(mapped_at) BETWEEN 20 AND 40),
 first_seen_at TEXT NOT NULL CHECK(length(first_seen_at) BETWEEN 20 AND 40),
 last_seen_at TEXT CHECK(last_seen_at IS NULL OR length(last_seen_at) BETWEEN 20 AND 40),
 active INTEGER NOT NULL CHECK(active IN(0,1)),
 conn_key_identity TEXT NOT NULL CHECK(fb_bankfeeds_identity_form(conn_key_identity)),
 remote_identity TEXT NOT NULL CHECK(fb_bankfeeds_identity_form(remote_identity)),
 CHECK((ledger_account_id IS NULL)=(mapped_at IS NULL)),
 UNIQUE(connection_id,conn_key_identity,remote_identity)
) WITHOUT ROWID, STRICT;
INSERT INTO bankfeeds_account(id,connection_id,conn_key,remote_id,name,org_name,currency,balance,available_balance,balance_at,ledger_account_id,mapped_at,first_seen_at,last_seen_at,active,conn_key_identity,remote_identity)
 SELECT id,connection_id,conn_key,remote_id,name,org_name,currency,balance,available_balance,balance_at,ledger_account_id,mapped_at,first_seen_at,last_seen_at,active,
  fb_bankfeeds_identity((SELECT secret FROM bankfeeds_identity_secret),'connection-key',conn_key),fb_bankfeeds_identity((SELECT secret FROM bankfeeds_identity_secret),'account',remote_id)
 FROM fbtmp_bankfeeds_account;
CREATE TABLE bankfeeds_transaction(
 id TEXT PRIMARY KEY CHECK(length(id)=36),
 ordinal INTEGER NOT NULL UNIQUE CHECK(ordinal BETWEEN 1 AND 9223372036854775806),
 feed_account_id TEXT NOT NULL REFERENCES bankfeeds_account(id) DEFERRABLE INITIALLY DEFERRED,
 remote_id TEXT NOT NULL CHECK(length(remote_id) BETWEEN 1 AND 200),
 posted_on TEXT CHECK(posted_on IS NULL OR (date(posted_on) IS NOT NULL AND date(posted_on)=posted_on)),
 transacted_on TEXT CHECK(transacted_on IS NULL OR (date(transacted_on) IS NOT NULL AND date(transacted_on)=transacted_on)),
 amount INTEGER NOT NULL CHECK(amount BETWEEN -9999999999999999 AND 9999999999999999),
 description TEXT NOT NULL CHECK(length(description)<=500),
 pending INTEGER NOT NULL CHECK(pending IN(0,1)),
 first_seen_at TEXT NOT NULL CHECK(length(first_seen_at) BETWEEN 20 AND 40),
 updated_at TEXT NOT NULL CHECK(length(updated_at) BETWEEN 20 AND 40),
 status TEXT NOT NULL CHECK(status IN('NEW','ACCEPTED','MATCHED','IGNORED')),
 accepted_as TEXT NOT NULL CHECK(accepted_as IN('','CHECK','DEPOSIT','TRANSFER','JOURNAL','MATCH')),
 journal_id TEXT REFERENCES ledger_journal(id) DEFERRABLE INITIALLY DEFERRED,
 journal_line_id TEXT UNIQUE REFERENCES ledger_journalline(id) DEFERRABLE INITIALLY DEFERRED,
 reference TEXT NOT NULL CHECK(length(reference)<=60),
 reference_url TEXT NOT NULL CHECK(length(reference_url)<=200),
 decided_at TEXT CHECK(decided_at IS NULL OR length(decided_at) BETWEEN 20 AND 40),
 decided_by_id TEXT REFERENCES fb_user(id) DEFERRABLE INITIALLY DEFERRED,
 return_count INTEGER NOT NULL CHECK(return_count BETWEEN 0 AND 2147483647),
 remote_identity TEXT NOT NULL CHECK(fb_bankfeeds_identity_form(remote_identity)),
 CHECK((status='NEW' AND accepted_as='' AND journal_id IS NULL AND journal_line_id IS NULL)
    OR (status='IGNORED' AND accepted_as='' AND journal_id IS NULL AND journal_line_id IS NULL)
    OR (status='MATCHED' AND accepted_as='MATCH' AND journal_id IS NOT NULL AND journal_line_id IS NOT NULL)
    OR (status='ACCEPTED' AND journal_id IS NOT NULL AND accepted_as NOT IN('','MATCH'))),
 CHECK((decided_at IS NULL)=(decided_by_id IS NULL)),
 UNIQUE(feed_account_id,remote_identity)
) WITHOUT ROWID, STRICT;
INSERT INTO bankfeeds_transaction(id,ordinal,feed_account_id,remote_id,posted_on,transacted_on,amount,description,pending,first_seen_at,updated_at,status,accepted_as,journal_id,journal_line_id,reference,reference_url,decided_at,decided_by_id,return_count,remote_identity)
 SELECT id,ordinal,feed_account_id,remote_id,posted_on,transacted_on,amount,description,pending,first_seen_at,updated_at,status,accepted_as,journal_id,journal_line_id,reference,reference_url,decided_at,decided_by_id,return_count,
  fb_bankfeeds_identity((SELECT secret FROM bankfeeds_identity_secret),'transaction',remote_id)
 FROM fbtmp_bankfeeds_transaction;
DROP TABLE fbtmp_bankfeeds_transaction;
DROP TABLE fbtmp_bankfeeds_account;
CREATE INDEX bankfeeds_transaction_review ON bankfeeds_transaction(feed_account_id,status);

-- 3. The guards of 0133-0135 on these two tables, unchanged except that collisions and fixed identity are
--    decided on the identity keys, and a downloaded row needs the book's identity secret.
CREATE TRIGGER fb_bankfeeds_account_insert BEFORE INSERT ON bankfeeds_account BEGIN
 SELECT RAISE(ABORT,'bankfeeds_key: Protected key already exists') WHERE EXISTS(SELECT 1 FROM bankfeeds_account WHERE id=NEW.id OR (connection_id=NEW.connection_id AND conn_key_identity=NEW.conn_key_identity AND remote_identity=NEW.remote_identity));
 SELECT RAISE(ABORT,'bankfeeds_identity: A downloaded row carries identity keys under the book''s secret') WHERE NOT EXISTS(SELECT 1 FROM bankfeeds_identity_secret);
 SELECT RAISE(ABORT,'bankfeeds_account_parent: A downloaded account belongs to an active connection') WHERE NOT EXISTS(SELECT 1 FROM bankfeeds_connection WHERE id=NEW.connection_id AND active=1);
 SELECT RAISE(ABORT,'bankfeeds_mapping: Map a feed to an eligible unmapped bank or credit card account of its company') WHERE NEW.ledger_account_id IS NOT NULL;
END;
CREATE TRIGGER fb_bankfeeds_account_update BEFORE UPDATE ON bankfeeds_account BEGIN
 SELECT RAISE(ABORT,'bankfeeds_key: Update would replace an existing row') WHERE EXISTS(SELECT 1 FROM bankfeeds_account WHERE id IS NOT OLD.id AND (id=NEW.id OR (connection_id=NEW.connection_id AND conn_key_identity=NEW.conn_key_identity AND remote_identity=NEW.remote_identity)));
 SELECT RAISE(ABORT,'bankfeeds_account_fixed: A downloaded account keeps its connection and remote identity') WHERE NEW.connection_id IS NOT OLD.connection_id OR NEW.conn_key IS NOT OLD.conn_key OR NEW.remote_id IS NOT OLD.remote_id OR NEW.first_seen_at IS NOT OLD.first_seen_at
   OR NEW.conn_key_identity IS NOT OLD.conn_key_identity OR NEW.remote_identity IS NOT OLD.remote_identity;
 SELECT RAISE(ABORT,'bankfeeds_mapping: Map a feed to an eligible unmapped bank or credit card account of its company') WHERE NEW.ledger_account_id IS NOT NULL AND NEW.ledger_account_id IS NOT OLD.ledger_account_id AND (
   NOT EXISTS(SELECT 1 FROM bankfeeds_mappable WHERE account_id=NEW.ledger_account_id AND connection_id=NEW.connection_id)
   OR NOT EXISTS(SELECT 1 FROM bankfeeds_connection c JOIN ledger_company y ON y.id=c.company_id WHERE c.id=NEW.connection_id AND upper(NEW.currency)=y.currency)
   OR EXISTS(SELECT 1 FROM bankfeeds_account f JOIN bankfeeds_connection fc ON fc.id=f.connection_id JOIN bankfeeds_connection nc ON nc.company_id=fc.company_id WHERE nc.id=NEW.connection_id AND f.id IS NOT NEW.id AND f.ledger_account_id=NEW.ledger_account_id AND f.active=1));
END;
CREATE TRIGGER fb_bankfeeds_account_id BEFORE UPDATE OF id ON bankfeeds_account WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT,'primary_key_immutable: Primary keys never change'); END;
CREATE TRIGGER fb_bankfeeds_account_delete BEFORE DELETE ON bankfeeds_account
BEGIN SELECT RAISE(ABORT,'bankfeeds_retained: Bank feed history is kept; forget the connection instead'); END;

CREATE TRIGGER fb_bankfeeds_transaction_insert BEFORE INSERT ON bankfeeds_transaction BEGIN
 SELECT RAISE(ABORT,'bankfeeds_key: Protected key already exists') WHERE EXISTS(SELECT 1 FROM bankfeeds_transaction WHERE id=NEW.id OR ordinal=NEW.ordinal OR (feed_account_id=NEW.feed_account_id AND remote_identity=NEW.remote_identity) OR (NEW.journal_line_id IS NOT NULL AND journal_line_id=NEW.journal_line_id));
 SELECT RAISE(ABORT,'bankfeeds_identity: A downloaded row carries identity keys under the book''s secret') WHERE NOT EXISTS(SELECT 1 FROM bankfeeds_identity_secret);
 SELECT RAISE(ABORT,'bankfeeds_staged: A downloaded transaction starts to review on an account of an active connection') WHERE NEW.status<>'NEW' OR NEW.decided_at IS NOT NULL OR NEW.return_count<>0 OR NEW.reference<>'' OR NEW.reference_url<>''
  OR NOT EXISTS(SELECT 1 FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE f.id=NEW.feed_account_id AND c.active=1);
 SELECT RAISE(ABORT,'bankfeeds_ordinal: Transaction numbers are never reused') WHERE NEW.ordinal<>coalesce((SELECT value FROM bankfeeds_counter WHERE name='transaction'),0)+1;
END;
CREATE TRIGGER fb_bankfeeds_transaction_count AFTER INSERT ON bankfeeds_transaction BEGIN
 INSERT INTO bankfeeds_counter SELECT 'transaction',NEW.ordinal WHERE NOT EXISTS(SELECT 1 FROM bankfeeds_counter WHERE name='transaction');
 UPDATE bankfeeds_counter SET value=NEW.ordinal WHERE name='transaction' AND value<>NEW.ordinal;
END;
CREATE TRIGGER fb_bankfeeds_transaction_update BEFORE UPDATE ON bankfeeds_transaction BEGIN
 SELECT RAISE(ABORT,'bankfeeds_key: Update would replace an existing row') WHERE EXISTS(SELECT 1 FROM bankfeeds_transaction WHERE id IS NOT OLD.id AND (id=NEW.id OR ordinal=NEW.ordinal OR (feed_account_id=NEW.feed_account_id AND remote_identity=NEW.remote_identity) OR (NEW.journal_line_id IS NOT NULL AND journal_line_id=NEW.journal_line_id)));
 SELECT RAISE(ABORT,'bankfeeds_transaction_fixed: A downloaded transaction keeps its account, number and remote identity') WHERE NEW.ordinal IS NOT OLD.ordinal OR NEW.feed_account_id IS NOT OLD.feed_account_id OR NEW.remote_id IS NOT OLD.remote_id OR NEW.first_seen_at IS NOT OLD.first_seen_at
   OR NEW.remote_identity IS NOT OLD.remote_identity;
 SELECT RAISE(ABORT,'bankfeeds_decided_values: A decided transaction keeps the values the operator accepted') WHERE OLD.status IN('ACCEPTED','MATCHED') AND (NEW.posted_on IS NOT OLD.posted_on OR NEW.transacted_on IS NOT OLD.transacted_on OR NEW.amount IS NOT OLD.amount OR NEW.description IS NOT OLD.description OR NEW.pending IS NOT OLD.pending);
 SELECT RAISE(ABORT,'bankfeeds_return_count: Returning to review counts once') WHERE NEW.return_count<>OLD.return_count+(CASE WHEN OLD.status<>'NEW' AND NEW.status='NEW' THEN 1 ELSE 0 END);
 SELECT RAISE(ABORT,'bankfeeds_transition: Invalid review decision') WHERE (OLD.status IN('ACCEPTED','MATCHED') AND NEW.status NOT IN(OLD.status,'NEW')) OR (OLD.status='IGNORED' AND NEW.status NOT IN('IGNORED','NEW'))
  OR (OLD.status IN('ACCEPTED','MATCHED') AND NEW.status=OLD.status AND (NEW.accepted_as IS NOT OLD.accepted_as OR NEW.journal_id IS NOT OLD.journal_id OR NEW.journal_line_id IS NOT OLD.journal_line_id OR NEW.reference IS NOT OLD.reference OR NEW.reference_url IS NOT OLD.reference_url OR NEW.decided_at IS NOT OLD.decided_at OR NEW.decided_by_id IS NOT OLD.decided_by_id))
  OR (NEW.status='NEW' AND (NEW.decided_at IS NOT NULL OR NEW.reference<>'' OR NEW.reference_url<>''))
  OR (NEW.status<>'NEW' AND NEW.decided_at IS NULL);
 SELECT RAISE(ABORT,'bankfeeds_ignore: Only a posted-or-pending transaction still to review can be ignored') WHERE NEW.status='IGNORED' AND OLD.status<>'IGNORED' AND (OLD.status<>'NEW' OR NEW.reference<>'');
 SELECT RAISE(ABORT,'bankfeeds_return: An accepted transaction returns to review only after its entry was reversed') WHERE OLD.status='ACCEPTED' AND NEW.status='NEW' AND NOT EXISTS(SELECT 1 FROM ledger_journal WHERE reversal_of_id=OLD.journal_id);
 SELECT RAISE(ABORT,'bankfeeds_decision: A decision names a posted entry line on the mapped account, in this direction and amount') WHERE NEW.status IN('ACCEPTED','MATCHED') AND OLD.status NOT IN('ACCEPTED','MATCHED') AND (OLD.status<>'NEW' OR OLD.pending<>0 OR NEW.pending<>0 OR coalesce(NEW.posted_on,NEW.transacted_on) IS NULL OR NOT EXISTS(
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
END;
CREATE TRIGGER fb_bankfeeds_transaction_id BEFORE UPDATE OF id ON bankfeeds_transaction WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT,'primary_key_immutable: Primary keys never change'); END;
CREATE TRIGGER fb_bankfeeds_transaction_delete BEFORE DELETE ON bankfeeds_transaction WHEN OLD.status<>'NEW' OR OLD.pending<>1
BEGIN SELECT RAISE(ABORT,'bankfeeds_retained: Only a withdrawn pending transaction still to review is removed'); END;

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
CREATE TRIGGER fb_bankfeeds_transaction_withdrawn AFTER DELETE ON bankfeeds_transaction
BEGIN INSERT INTO bankfeeds_retired(id,kind) VALUES(OLD.id,'transaction'); END;
CREATE TRIGGER fb_bankfeeds_transaction_identity BEFORE INSERT ON bankfeeds_transaction BEGIN
 SELECT RAISE(ABORT,'bankfeeds_key: A withdrawn transaction identity is never reused') WHERE EXISTS(SELECT 1 FROM bankfeeds_retired WHERE id=NEW.id);
END;
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
CREATE TRIGGER fb_bankfeeds_account_activation BEFORE UPDATE ON bankfeeds_account WHEN NEW.active=1 AND OLD.active IS NOT 1 AND NEW.ledger_account_id IS NOT NULL BEGIN
 SELECT RAISE(ABORT,'bankfeeds_mapping: Map a feed to an eligible unmapped bank or credit card account of its company') WHERE
   NOT EXISTS(SELECT 1 FROM bankfeeds_mappable WHERE account_id=NEW.ledger_account_id AND connection_id=NEW.connection_id)
   OR NOT EXISTS(SELECT 1 FROM bankfeeds_connection c JOIN ledger_company y ON y.id=c.company_id WHERE c.id=NEW.connection_id AND upper(NEW.currency)=y.currency)
   OR EXISTS(SELECT 1 FROM bankfeeds_account f JOIN bankfeeds_connection fc ON fc.id=f.connection_id JOIN bankfeeds_connection nc ON nc.company_id=fc.company_id
     WHERE nc.id=NEW.connection_id AND f.id IS NOT NEW.id AND f.ledger_account_id=NEW.ledger_account_id AND f.active=1);
END;

-- 4. Open-time evidence: 0134's views, plus identity keys in their stored form, a duplicate identity, and
--    downloaded rows without the book's secret.
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
 UNION ALL SELECT x.id FROM bankfeeds_retired x WHERE NOT (length(CAST(x.id AS BLOB))=36 AND fb_bankfeeds_uuid(x.id))
 UNION ALL SELECT f.id FROM bankfeeds_account f WHERE EXISTS(SELECT 1 FROM bankfeeds_account g WHERE g.id<>f.id AND g.connection_id=f.connection_id AND g.conn_key_identity=f.conn_key_identity AND g.remote_identity=f.remote_identity)
 UNION ALL SELECT t.id FROM bankfeeds_transaction t WHERE EXISTS(SELECT 1 FROM bankfeeds_transaction u WHERE u.id<>t.id AND u.feed_account_id=t.feed_account_id AND u.remote_identity=t.remote_identity)
 UNION ALL SELECT f.id FROM bankfeeds_account f WHERE NOT EXISTS(SELECT 1 FROM bankfeeds_identity_secret)
 UNION ALL SELECT s.name FROM bankfeeds_identity_secret s WHERE NOT (s.name='identity' AND typeof(s.secret)='blob' AND length(s.secret)=32);
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
 UNION ALL SELECT id FROM bankfeeds_rule WHERE NOT (fb_bankfeeds_uuid(id) AND fb_bankfeeds_uuid(company_id) AND fb_bankfeeds_uuid(account_id)
  AND fb_bankfeeds_text(name,1,120) AND fb_bankfeeds_text(match_text,1,200) AND fb_bankfeeds_text(payee_name,0,120)
  AND fb_bankfeeds_timestamp(created_at) AND fb_bankfeeds_timestamp(updated_at));
