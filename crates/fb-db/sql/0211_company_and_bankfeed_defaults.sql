ALTER TABLE ledger_company ADD COLUMN fiscal_year_start_month INTEGER NOT NULL
  DEFAULT 1 CHECK(fiscal_year_start_month BETWEEN 1 AND 12);
ALTER TABLE ledger_company ADD COLUMN closing_date TEXT DEFAULT NULL
  CHECK(closing_date IS NULL OR fb_valid_date(closing_date)=1);
ALTER TABLE ledger_company ADD COLUMN period_policy_revision INTEGER NOT NULL
  DEFAULT 0 CHECK(period_policy_revision BETWEEN 0 AND 9223372036854775806);

ALTER TABLE directory_customer ADD COLUMN default_terms TEXT DEFAULT NULL
  CONSTRAINT customer_default_terms CHECK(default_terms IS NULL OR default_terms IN ('0','15','30','60'));

CREATE TRIGGER fb_period_journal_post BEFORE UPDATE ON ledger_journal
WHEN NEW.status='POSTED' AND OLD.status IS NOT 'POSTED'
BEGIN
  SELECT RAISE(ABORT,'period_closed: The journal date is in a closed period')
  WHERE EXISTS (SELECT 1 FROM ledger_company c
    WHERE c.id=NEW.company_id AND NEW.date<=c.closing_date);
END;

CREATE TRIGGER fb_period_credit_apply BEFORE INSERT ON credits_creditapplication
BEGIN
  SELECT RAISE(ABORT,'period_closed: The application date is in a closed period')
  WHERE EXISTS (SELECT 1 FROM ledger_company c
    WHERE c.id=NEW.company_id AND NEW.date<=c.closing_date);
END;

CREATE TRIGGER fb_period_credit_unapply BEFORE INSERT ON credits_creditunapplication
BEGIN
  SELECT RAISE(ABORT,'period_closed: The unapplication date is in a closed period')
  WHERE EXISTS (SELECT 1 FROM credits_creditapplication a JOIN ledger_company c
    ON c.id=a.company_id WHERE a.id=NEW.application_id AND NEW.date<=c.closing_date);
END;

CREATE TRIGGER fb_period_feed_accept BEFORE UPDATE OF status ON bankfeeds_transaction
WHEN OLD.status='NEW' AND NEW.status='ACCEPTED'
BEGIN
  SELECT RAISE(ABORT,'period_closed: The transaction date is in a closed period')
  WHERE EXISTS (SELECT 1 FROM bankfeeds_account f
    JOIN bankfeeds_connection b ON b.id=f.connection_id
    JOIN ledger_company c ON c.id=b.company_id
    WHERE f.id=NEW.feed_account_id
      AND (coalesce(NEW.posted_on,NEW.transacted_on)<=c.closing_date
        OR EXISTS (SELECT 1 FROM ledger_journal j
          WHERE j.id=NEW.journal_id
            AND (j.date<=c.closing_date
              OR j.date IS NOT coalesce(NEW.posted_on,NEW.transacted_on)))));
END;

CREATE TRIGGER fb_period_company_insert BEFORE INSERT ON ledger_company
WHEN NEW.fiscal_year_start_month IS NOT 1 OR NEW.closing_date IS NOT NULL
  OR NEW.period_policy_revision IS NOT 0
BEGIN
  SELECT RAISE(ABORT,'period_policy: A new company starts with the default accounting periods');
END;

CREATE TRIGGER fb_period_company_update
BEFORE UPDATE OF fiscal_year_start_month,closing_date,period_policy_revision ON ledger_company
WHEN NEW.fiscal_year_start_month IS NOT OLD.fiscal_year_start_month
  OR NEW.closing_date IS NOT OLD.closing_date
  OR NEW.period_policy_revision IS NOT OLD.period_policy_revision
BEGIN
  SELECT RAISE(ABORT,'period_policy: Use Accounting periods to change this policy')
  WHERE fb_company_period_command() IS NOT 1
    OR NEW.period_policy_revision IS NOT OLD.period_policy_revision+1
    OR (NEW.fiscal_year_start_month IS OLD.fiscal_year_start_month AND NEW.closing_date IS OLD.closing_date);
END;

ALTER TABLE bankfeeds_account ADD COLUMN import_from_date TEXT DEFAULT NULL
  CHECK(import_from_date IS NULL OR fb_valid_date(import_from_date)=1);
ALTER TABLE bankfeeds_account ADD COLUMN import_state TEXT NOT NULL DEFAULT 'ACTIVE'
  CHECK(import_state IN ('PENDING','READY','ACTIVE') AND (import_state<>'PENDING' OR import_from_date IS NULL));

CREATE TRIGGER fb_feed_import_account_insert BEFORE INSERT ON bankfeeds_account
WHEN NEW.import_state NOT IN ('PENDING','ACTIVE') OR NEW.import_from_date IS NOT NULL
BEGIN
  SELECT RAISE(ABORT,'feed_import_policy: A new feed starts with no import date');
END;

CREATE TRIGGER fb_feed_import_account_update BEFORE UPDATE OF import_state,import_from_date ON bankfeeds_account
WHEN NEW.import_state IS NOT OLD.import_state OR NEW.import_from_date IS NOT OLD.import_from_date
BEGIN
  SELECT RAISE(ABORT,'feed_import_policy: Invalid bank-feed import policy change')
  WHERE NOT (
    (OLD.import_state='PENDING' AND NEW.import_state='READY')
    OR (OLD.import_state='READY' AND (NEW.import_state='READY'
      OR (NEW.import_state='ACTIVE' AND NEW.import_from_date IS OLD.import_from_date)))
  );
END;

CREATE TRIGGER fb_feed_import_insert BEFORE INSERT ON bankfeeds_transaction
BEGIN
  SELECT RAISE(ABORT,'feed_import_policy: The transaction is outside this feed’s import policy')
  WHERE NOT EXISTS (
    SELECT 1 FROM bankfeeds_account f WHERE f.id=NEW.feed_account_id
      AND f.import_state IN ('READY','ACTIVE')
      AND (f.import_from_date IS NULL OR coalesce(NEW.posted_on,NEW.transacted_on)>=f.import_from_date)
  );
END;

ALTER TABLE bankfeeds_rule ADD COLUMN feed_account_id TEXT
  REFERENCES bankfeeds_account(id) DEFERRABLE INITIALLY DEFERRED
  CHECK(feed_account_id IS NULL OR fb_valid_uuid(feed_account_id)=1);

CREATE TRIGGER fb_rule_feed_scope_insert BEFORE INSERT ON bankfeeds_rule
WHEN NEW.feed_account_id IS NOT NULL
BEGIN
  SELECT RAISE(ABORT,'rule_feed_scope: Choose a feed account of this company')
  WHERE NOT EXISTS (SELECT 1 FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id
    WHERE f.id=NEW.feed_account_id AND c.company_id=NEW.company_id);
END;

CREATE TRIGGER fb_rule_feed_scope_update BEFORE UPDATE OF feed_account_id,company_id ON bankfeeds_rule
WHEN NEW.feed_account_id IS NOT NULL
BEGIN
  SELECT RAISE(ABORT,'rule_feed_scope: Choose a feed account of this company')
  WHERE NOT EXISTS (SELECT 1 FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id
    WHERE f.id=NEW.feed_account_id AND c.company_id=NEW.company_id);
  SELECT RAISE(ABORT,'rule_feed_scope: Imported rules store their own scope')
  WHERE EXISTS (SELECT 1 FROM bankfeeds_rule_policy p WHERE p.rule_id=NEW.id);
END;

CREATE TRIGGER fb_rule_policy_manual_scope BEFORE INSERT ON bankfeeds_rule_policy
BEGIN
  SELECT RAISE(ABORT,'bankfeeds_rule_policy_immutable: Imported rule policies cannot be replaced')
  WHERE EXISTS (SELECT 1 FROM bankfeeds_rule_policy p
    WHERE p.rule_id=NEW.rule_id OR (p.company_id=NEW.company_id AND p.source_key=NEW.source_key));
  SELECT RAISE(ABORT,'rule_feed_scope: Imported rules store their own scope')
  WHERE EXISTS (SELECT 1 FROM bankfeeds_rule r WHERE r.id=NEW.rule_id AND r.feed_account_id IS NOT NULL);
END;

CREATE VIEW fb_transfer_eligible(company_id,account_id) AS
SELECT a.company_id,a.id FROM ledger_account a WHERE a.active=1 AND a.postable=1
  AND NOT EXISTS (SELECT 1 FROM fb_control_accounts c WHERE c.company_id=a.company_id AND c.account_id=a.id)
  AND ((a.kind='LIABILITY' AND a.detail_type='CREDIT_CARD')
    OR (a.kind='ASSET' AND EXISTS (
      SELECT 1 FROM documents_designatedbank d JOIN documents_accountingsetup s ON s.id=d.setup_id
      WHERE d.account_id=a.id AND s.company_id=a.company_id
    )));

DROP TRIGGER fb_transfer_insert;
CREATE TRIGGER fb_transfer_insert BEFORE INSERT ON transfers_transfer BEGIN
 SELECT RAISE(ABORT,'transfer_immutable: Transfer history is immutable') WHERE EXISTS(SELECT 1 FROM transfers_transfer WHERE id=NEW.id OR identity_id=NEW.identity_id OR journal_id=NEW.journal_id);
 SELECT RAISE(ABORT,'transfer_scope: Invalid transfer ownership, accounts or posting') WHERE NOT EXISTS(SELECT 1 FROM documents_documentidentity WHERE id=NEW.identity_id AND company_id=NEW.company_id AND kind='TRF') OR
 NOT EXISTS(SELECT 1 FROM ledger_journal WHERE id=NEW.journal_id AND company_id=NEW.company_id AND source_id=NEW.identity_id AND status='POSTED' AND reversal_of_id IS NULL AND date=NEW.date);
 SELECT RAISE(ABORT,'transfer_control_account: Transfers cannot use a control account') WHERE EXISTS(SELECT 1 FROM fb_control_accounts WHERE company_id=NEW.company_id AND account_id IN(NEW.from_account_id,NEW.to_account_id));
 SELECT RAISE(ABORT,'transfer_scope: Choose two eligible bank or credit card accounts')
 WHERE NEW.from_account_id=NEW.to_account_id OR
 (SELECT count(*) FROM fb_transfer_eligible WHERE company_id=NEW.company_id AND account_id IN(NEW.from_account_id,NEW.to_account_id))<>2;
 SELECT RAISE(ABORT,'transfer_journal_split: Transfer journal must be exactly one credit on the from account and one debit on the to account') WHERE (SELECT count(*) FROM ledger_journalline WHERE journal_id=NEW.journal_id)<>2 OR NOT EXISTS(SELECT 1 FROM ledger_journalline WHERE journal_id=NEW.journal_id AND account_id=NEW.from_account_id AND debit=0 AND credit=NEW.amount) OR NOT EXISTS(SELECT 1 FROM ledger_journalline WHERE journal_id=NEW.journal_id AND account_id=NEW.to_account_id AND debit=NEW.amount AND credit=0);
 SELECT RAISE(ABORT,'transfer_account_evidence: Transfer account evidence must be exactly from and to') WHERE (SELECT count(*) FROM documents_documentaccount WHERE source_id=NEW.identity_id)<>2 OR NOT EXISTS(SELECT 1 FROM documents_documentaccount WHERE source_id=NEW.identity_id AND role='from' AND account_id=NEW.from_account_id) OR NOT EXISTS(SELECT 1 FROM documents_documentaccount WHERE source_id=NEW.identity_id AND role='to' AND account_id=NEW.to_account_id);
END;
