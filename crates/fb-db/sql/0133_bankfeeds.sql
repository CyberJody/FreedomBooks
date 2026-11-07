-- Bank feeds (frozen bankfeeds/models.py): connections, downloaded accounts, staged transactions, rules.
-- The access URL is a plugin credential (plugin_credential, wave 4); no secret column exists here.
-- Nothing here posts: decisions only reference journals written by the checks/deposits/transfers/ledger services.
CREATE TABLE bankfeeds_connection(
 id TEXT PRIMARY KEY CHECK(length(id)=36),
 company_id TEXT NOT NULL REFERENCES ledger_company(id) DEFERRABLE INITIALLY DEFERRED,
 label TEXT NOT NULL CHECK(length(label) BETWEEN 1 AND 120),
 host TEXT NOT NULL CHECK(length(host) BETWEEN 1 AND 253 AND host=lower(host)),
 active INTEGER NOT NULL CHECK(active IN(0,1)),
 created_by_id TEXT NOT NULL REFERENCES fb_user(id) DEFERRABLE INITIALLY DEFERRED,
 created_at TEXT NOT NULL CHECK(length(created_at) BETWEEN 20 AND 40),
 forgotten_at TEXT CHECK(forgotten_at IS NULL OR length(forgotten_at) BETWEEN 20 AND 40),
 last_sync_at TEXT CHECK(last_sync_at IS NULL OR length(last_sync_at) BETWEEN 20 AND 40),
 last_sync_ok INTEGER CHECK(last_sync_ok IS NULL OR last_sync_ok IN(0,1)),
 last_sync_summary TEXT NOT NULL CHECK(length(last_sync_summary)<=500),
 sync_count INTEGER NOT NULL CHECK(sync_count BETWEEN 0 AND 2147483647),
 CHECK((last_sync_at IS NULL)=(last_sync_ok IS NULL))
) WITHOUT ROWID, STRICT;
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
 CHECK((ledger_account_id IS NULL)=(mapped_at IS NULL)),
 UNIQUE(connection_id,conn_key,remote_id)
) WITHOUT ROWID, STRICT;
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
 CHECK((status='NEW' AND accepted_as='' AND journal_id IS NULL AND journal_line_id IS NULL)
    OR (status='IGNORED' AND accepted_as='' AND journal_id IS NULL AND journal_line_id IS NULL)
    OR (status='MATCHED' AND accepted_as='MATCH' AND journal_id IS NOT NULL AND journal_line_id IS NOT NULL)
    OR (status='ACCEPTED' AND journal_id IS NOT NULL AND accepted_as NOT IN('','MATCH'))),
 CHECK((decided_at IS NULL)=(decided_by_id IS NULL)),
 UNIQUE(feed_account_id,remote_id)
) WITHOUT ROWID, STRICT;
CREATE INDEX bankfeeds_transaction_review ON bankfeeds_transaction(feed_account_id,status);
CREATE TABLE bankfeeds_rule(
 id TEXT PRIMARY KEY CHECK(length(id)=36),
 company_id TEXT NOT NULL REFERENCES ledger_company(id) DEFERRABLE INITIALLY DEFERRED,
 position INTEGER NOT NULL CHECK(position BETWEEN 0 AND 2147483647),
 name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 120),
 match_text TEXT NOT NULL CHECK(length(match_text) BETWEEN 1 AND 200),
 sign TEXT NOT NULL CHECK(sign IN('ANY','IN','OUT')),
 amount_min INTEGER CHECK(amount_min IS NULL OR amount_min BETWEEN 0 AND 9999999999999999),
 amount_max INTEGER CHECK(amount_max IS NULL OR amount_max BETWEEN 0 AND 9999999999999999),
 account_id TEXT NOT NULL REFERENCES ledger_account(id) DEFERRABLE INITIALLY DEFERRED,
 payee_name TEXT NOT NULL CHECK(length(payee_name)<=120),
 active INTEGER NOT NULL CHECK(active IN(0,1)),
 created_at TEXT NOT NULL CHECK(length(created_at) BETWEEN 20 AND 40),
 updated_at TEXT NOT NULL CHECK(length(updated_at) BETWEEN 20 AND 40),
 CHECK(amount_min IS NULL OR amount_max IS NULL OR amount_min<=amount_max)
) WITHOUT ROWID, STRICT;
CREATE INDEX bankfeeds_rule_order ON bankfeeds_rule(company_id,position);
-- Python's integer primary key: never reused, also after a withdrawn pending row is deleted. The row
-- appears with the first downloaded transaction.
CREATE TABLE bankfeeds_counter(name TEXT PRIMARY KEY CHECK(name='transaction'),value INTEGER NOT NULL CHECK(value BETWEEN 0 AND 9223372036854775806)) WITHOUT ROWID, STRICT;

-- Mapping eligibility (services.map_account / mappable_accounts): an active postable ASSET or LIABILITY account of
-- the feed's company that is not a control account, mapped by at most one active feed account of the company.
CREATE VIEW bankfeeds_mappable AS
 SELECT a.id account_id,c.id connection_id FROM bankfeeds_connection c JOIN ledger_account a ON a.company_id=c.company_id
 WHERE a.kind IN('ASSET','LIABILITY') AND a.active=1 AND a.postable=1
   AND NOT EXISTS(SELECT 1 FROM fb_control_accounts x WHERE x.company_id=a.company_id AND x.account_id=a.id);
-- Rule categories (services.rule_accounts): active postable non-control accounts of the company.
CREATE VIEW bankfeeds_rule_account AS
 SELECT a.id account_id,a.company_id FROM ledger_account a WHERE a.active=1 AND a.postable=1
   AND NOT EXISTS(SELECT 1 FROM fb_control_accounts x WHERE x.company_id=a.company_id AND x.account_id=a.id);

-- 1. Collisions: every unique key on INSERT; displaced rows on UPDATE.
CREATE TRIGGER fb_bankfeeds_connection_insert BEFORE INSERT ON bankfeeds_connection BEGIN
 SELECT RAISE(ABORT,'bankfeeds_key: Protected key already exists') WHERE EXISTS(SELECT 1 FROM bankfeeds_connection WHERE id=NEW.id);
 SELECT RAISE(ABORT,'bankfeeds_connection_scope: A new connection is active, never synced, and made by a member') WHERE NEW.active<>1 OR NEW.forgotten_at IS NOT NULL OR NEW.last_sync_at IS NOT NULL OR NEW.sync_count<>0 OR NEW.last_sync_summary<>''
  OR NOT EXISTS(SELECT 1 FROM ledger_company_members m JOIN fb_user u ON u.id=m.user_id WHERE m.company_id=NEW.company_id AND m.user_id=NEW.created_by_id AND u.is_active=1);
END;
CREATE TRIGGER fb_bankfeeds_connection_update BEFORE UPDATE ON bankfeeds_connection BEGIN
 SELECT RAISE(ABORT,'bankfeeds_key: Update would replace an existing row') WHERE EXISTS(SELECT 1 FROM bankfeeds_connection WHERE id IS NOT OLD.id AND id=NEW.id);
 SELECT RAISE(ABORT,'bankfeeds_connection_fixed: A connection keeps its company, label, host and creator') WHERE NEW.company_id IS NOT OLD.company_id OR NEW.label IS NOT OLD.label OR NEW.host IS NOT OLD.host OR NEW.created_by_id IS NOT OLD.created_by_id OR NEW.created_at IS NOT OLD.created_at;
 SELECT RAISE(ABORT,'bankfeeds_connection_forgotten: A forgotten connection stays forgotten') WHERE (OLD.active=0 AND NEW.active=1) OR (OLD.forgotten_at IS NOT NULL AND NEW.forgotten_at IS NOT OLD.forgotten_at) OR (NEW.active=0 AND NEW.forgotten_at IS NULL) OR (NEW.active=1 AND NEW.forgotten_at IS NOT NULL);
 SELECT RAISE(ABORT,'bankfeeds_connection_sync: Sync results are recorded forward only') WHERE NEW.sync_count<OLD.sync_count OR NEW.sync_count>OLD.sync_count+1 OR (NEW.sync_count=OLD.sync_count AND (NEW.last_sync_at IS NOT OLD.last_sync_at OR NEW.last_sync_ok IS NOT OLD.last_sync_ok OR NEW.last_sync_summary IS NOT OLD.last_sync_summary)) OR (NEW.sync_count=OLD.sync_count+1 AND NEW.last_sync_at IS NULL);
END;
CREATE TRIGGER fb_bankfeeds_connection_id BEFORE UPDATE OF id ON bankfeeds_connection WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT,'primary_key_immutable: Primary keys never change'); END;
CREATE TRIGGER fb_bankfeeds_connection_delete BEFORE DELETE ON bankfeeds_connection
BEGIN SELECT RAISE(ABORT,'bankfeeds_retained: Bank feed history is kept; forget the connection instead'); END;

CREATE TRIGGER fb_bankfeeds_account_insert BEFORE INSERT ON bankfeeds_account BEGIN
 SELECT RAISE(ABORT,'bankfeeds_key: Protected key already exists') WHERE EXISTS(SELECT 1 FROM bankfeeds_account WHERE id=NEW.id OR (connection_id=NEW.connection_id AND conn_key=NEW.conn_key AND remote_id=NEW.remote_id));
 SELECT RAISE(ABORT,'bankfeeds_account_parent: A downloaded account belongs to an active connection') WHERE NOT EXISTS(SELECT 1 FROM bankfeeds_connection WHERE id=NEW.connection_id AND active=1);
 SELECT RAISE(ABORT,'bankfeeds_mapping: Map a feed to an eligible unmapped bank or credit card account of its company') WHERE NEW.ledger_account_id IS NOT NULL;
END;
CREATE TRIGGER fb_bankfeeds_account_update BEFORE UPDATE ON bankfeeds_account BEGIN
 SELECT RAISE(ABORT,'bankfeeds_key: Update would replace an existing row') WHERE EXISTS(SELECT 1 FROM bankfeeds_account WHERE id IS NOT OLD.id AND (id=NEW.id OR (connection_id=NEW.connection_id AND conn_key=NEW.conn_key AND remote_id=NEW.remote_id)));
 SELECT RAISE(ABORT,'bankfeeds_account_fixed: A downloaded account keeps its connection and remote identity') WHERE NEW.connection_id IS NOT OLD.connection_id OR NEW.conn_key IS NOT OLD.conn_key OR NEW.remote_id IS NOT OLD.remote_id OR NEW.first_seen_at IS NOT OLD.first_seen_at;
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
 SELECT RAISE(ABORT,'bankfeeds_key: Protected key already exists') WHERE EXISTS(SELECT 1 FROM bankfeeds_transaction WHERE id=NEW.id OR ordinal=NEW.ordinal OR (feed_account_id=NEW.feed_account_id AND remote_id=NEW.remote_id) OR (NEW.journal_line_id IS NOT NULL AND journal_line_id=NEW.journal_line_id));
 SELECT RAISE(ABORT,'bankfeeds_staged: A downloaded transaction starts to review on an account of an active connection') WHERE NEW.status<>'NEW' OR NEW.decided_at IS NOT NULL OR NEW.return_count<>0 OR NEW.reference<>'' OR NEW.reference_url<>''
  OR NOT EXISTS(SELECT 1 FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE f.id=NEW.feed_account_id AND c.active=1);
 SELECT RAISE(ABORT,'bankfeeds_ordinal: Transaction numbers are never reused') WHERE NEW.ordinal<>coalesce((SELECT value FROM bankfeeds_counter WHERE name='transaction'),0)+1;
END;
CREATE TRIGGER fb_bankfeeds_transaction_count AFTER INSERT ON bankfeeds_transaction BEGIN
 INSERT INTO bankfeeds_counter SELECT 'transaction',NEW.ordinal WHERE NOT EXISTS(SELECT 1 FROM bankfeeds_counter WHERE name='transaction');
 UPDATE bankfeeds_counter SET value=NEW.ordinal WHERE name='transaction' AND value<>NEW.ordinal;
END;
CREATE TRIGGER fb_bankfeeds_counter_update BEFORE UPDATE ON bankfeeds_counter BEGIN
 SELECT RAISE(ABORT,'bankfeeds_ordinal: Transaction numbers are never reused') WHERE NEW.name IS NOT OLD.name OR NEW.value<>OLD.value+1 OR NOT EXISTS(SELECT 1 FROM bankfeeds_transaction WHERE ordinal=NEW.value);
END;
CREATE TRIGGER fb_bankfeeds_counter_insert BEFORE INSERT ON bankfeeds_counter BEGIN SELECT RAISE(ABORT,'bankfeeds_ordinal: Transaction numbers are never reused') WHERE NEW.value<>1 OR EXISTS(SELECT 1 FROM bankfeeds_counter) OR NOT EXISTS(SELECT 1 FROM bankfeeds_transaction WHERE ordinal=1); END;
CREATE TRIGGER fb_bankfeeds_counter_delete BEFORE DELETE ON bankfeeds_counter BEGIN SELECT RAISE(ABORT,'bankfeeds_ordinal: Transaction numbers are never reused'); END;
CREATE TRIGGER fb_bankfeeds_transaction_update BEFORE UPDATE ON bankfeeds_transaction BEGIN
 SELECT RAISE(ABORT,'bankfeeds_key: Update would replace an existing row') WHERE EXISTS(SELECT 1 FROM bankfeeds_transaction WHERE id IS NOT OLD.id AND (id=NEW.id OR ordinal=NEW.ordinal OR (feed_account_id=NEW.feed_account_id AND remote_id=NEW.remote_id) OR (NEW.journal_line_id IS NOT NULL AND journal_line_id=NEW.journal_line_id)));
 SELECT RAISE(ABORT,'bankfeeds_transaction_fixed: A downloaded transaction keeps its account, number and remote identity') WHERE NEW.ordinal IS NOT OLD.ordinal OR NEW.feed_account_id IS NOT OLD.feed_account_id OR NEW.remote_id IS NOT OLD.remote_id OR NEW.first_seen_at IS NOT OLD.first_seen_at;
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

CREATE TRIGGER fb_bankfeeds_rule_insert BEFORE INSERT ON bankfeeds_rule BEGIN
 SELECT RAISE(ABORT,'bankfeeds_key: Protected key already exists') WHERE EXISTS(SELECT 1 FROM bankfeeds_rule WHERE id=NEW.id);
 SELECT RAISE(ABORT,'bankfeeds_rule_account: A rule names an active posting account of its company that is not a control account') WHERE NOT EXISTS(SELECT 1 FROM bankfeeds_rule_account WHERE account_id=NEW.account_id AND company_id=NEW.company_id);
 SELECT RAISE(ABORT,'bankfeeds_rule_limit: A company has at most 200 rules') WHERE (SELECT count(*) FROM bankfeeds_rule WHERE company_id=NEW.company_id)>=200;
END;
CREATE TRIGGER fb_bankfeeds_rule_update BEFORE UPDATE ON bankfeeds_rule BEGIN
 SELECT RAISE(ABORT,'bankfeeds_key: Update would replace an existing row') WHERE EXISTS(SELECT 1 FROM bankfeeds_rule WHERE id IS NOT OLD.id AND id=NEW.id);
 SELECT RAISE(ABORT,'bankfeeds_rule_fixed: A rule keeps its company') WHERE NEW.company_id IS NOT OLD.company_id OR NEW.created_at IS NOT OLD.created_at;
 SELECT RAISE(ABORT,'bankfeeds_rule_account: A rule names an active posting account of its company that is not a control account') WHERE NEW.account_id IS NOT OLD.account_id AND NOT EXISTS(SELECT 1 FROM bankfeeds_rule_account WHERE account_id=NEW.account_id AND company_id=NEW.company_id);
END;
CREATE TRIGGER fb_bankfeeds_rule_id BEFORE UPDATE OF id ON bankfeeds_rule WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT,'primary_key_immutable: Primary keys never change'); END;
CREATE TRIGGER fb_bankfeeds_rule_delete BEFORE DELETE ON bankfeeds_rule
BEGIN SELECT RAISE(ABORT,'bankfeeds_retained: Rules are deactivated, never deleted'); END;

-- 2. Every parent a bank-feed row references keeps its identity.
CREATE TRIGGER fb_bankfeeds_account_kept BEFORE DELETE ON ledger_account WHEN EXISTS(SELECT 1 FROM bankfeeds_account WHERE ledger_account_id=OLD.id) OR EXISTS(SELECT 1 FROM bankfeeds_rule WHERE account_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'bankfeeds_parent_kept: Referenced bank-feed parents cannot be deleted'); END;
CREATE TRIGGER fb_bankfeeds_account_moved BEFORE UPDATE OF id,company_id ON ledger_account WHEN (NEW.id IS NOT OLD.id OR NEW.company_id IS NOT OLD.company_id) AND (EXISTS(SELECT 1 FROM bankfeeds_account WHERE ledger_account_id=OLD.id) OR EXISTS(SELECT 1 FROM bankfeeds_rule WHERE account_id=OLD.id))
BEGIN SELECT RAISE(ABORT,'bankfeeds_parent_kept: Referenced bank-feed parents cannot be deleted'); END;
CREATE TRIGGER fb_bankfeeds_user_kept BEFORE DELETE ON fb_user WHEN EXISTS(SELECT 1 FROM bankfeeds_connection WHERE created_by_id=OLD.id) OR EXISTS(SELECT 1 FROM bankfeeds_transaction WHERE decided_by_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'bankfeeds_parent_kept: Referenced bank-feed parents cannot be deleted'); END;
CREATE TRIGGER fb_bankfeeds_company_kept BEFORE DELETE ON ledger_company WHEN EXISTS(SELECT 1 FROM bankfeeds_connection WHERE company_id=OLD.id) OR EXISTS(SELECT 1 FROM bankfeeds_rule WHERE company_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'bankfeeds_parent_kept: Referenced bank-feed parents cannot be deleted'); END;
CREATE TRIGGER fb_bankfeeds_journal_kept BEFORE DELETE ON ledger_journal WHEN EXISTS(SELECT 1 FROM bankfeeds_transaction WHERE journal_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'bankfeeds_parent_kept: Referenced bank-feed parents cannot be deleted'); END;
CREATE TRIGGER fb_bankfeeds_line_kept BEFORE DELETE ON ledger_journalline WHEN EXISTS(SELECT 1 FROM bankfeeds_transaction WHERE journal_line_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'bankfeeds_parent_kept: Referenced bank-feed parents cannot be deleted'); END;

-- Open-time evidence: rows that a connection without these triggers could have slipped in.
CREATE VIEW bankfeeds_invalid AS
 SELECT t.id FROM bankfeeds_transaction t WHERE t.status IN('ACCEPTED','MATCHED') AND NOT EXISTS(
   SELECT 1 FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id JOIN ledger_journal j ON j.id=t.journal_id JOIN ledger_journalline l ON l.id=t.journal_line_id
   WHERE f.id=t.feed_account_id AND j.company_id=c.company_id AND j.status='POSTED' AND l.journal_id=j.id
     AND l.debit=(CASE WHEN t.amount>0 THEN t.amount ELSE 0 END) AND l.credit=(CASE WHEN t.amount>0 THEN 0 ELSE -t.amount END))
 UNION ALL SELECT t.id FROM bankfeeds_transaction t WHERE t.ordinal>coalesce((SELECT value FROM bankfeeds_counter WHERE name='transaction'),0)
 UNION ALL SELECT f.id FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id LEFT JOIN ledger_account a ON a.id=f.ledger_account_id WHERE f.ledger_account_id IS NOT NULL AND (a.id IS NULL OR a.company_id<>c.company_id OR a.kind NOT IN('ASSET','LIABILITY'))
 UNION ALL SELECT r.id FROM bankfeeds_rule r LEFT JOIN ledger_account a ON a.id=r.account_id WHERE a.id IS NULL OR a.company_id<>r.company_id
 UNION ALL SELECT c.id FROM bankfeeds_connection c WHERE NOT EXISTS(SELECT 1 FROM fb_user WHERE id=c.created_by_id)
 UNION ALL SELECT t.id FROM bankfeeds_transaction t WHERE t.decided_by_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM fb_user WHERE id=t.decided_by_id);
