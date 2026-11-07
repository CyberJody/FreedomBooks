-- Source-document hooks for the L1 ledger. Generic primitives only: each owning
-- module adds its own completeness seals and late-reference rules in its migration.
ALTER TABLE ledger_journal ADD COLUMN source_id TEXT REFERENCES documents_documentidentity(id) DEFERRABLE INITIALLY DEFERRED;
CREATE UNIQUE INDEX journal_original_per_source ON ledger_journal(source_id) WHERE source_id IS NOT NULL AND reversal_of_id IS NULL;
DROP TRIGGER fb_journal_guard_insert;
CREATE TRIGGER fb_journal_guard_insert BEFORE INSERT ON ledger_journal
BEGIN
  SELECT RAISE(ABORT, 'fb_journal_guard: Journal key already exists') WHERE EXISTS (
    SELECT 1 FROM ledger_journal WHERE id=NEW.id OR number=NEW.number
      OR (company_id=NEW.company_id AND entry_number=NEW.entry_number AND NEW.entry_number<>'')
      OR (company_id=NEW.company_id AND request_key=NEW.request_key)
      OR reversal_of_id=NEW.reversal_of_id
      OR (source_id=NEW.source_id AND NEW.source_id IS NOT NULL AND reversal_of_id IS NULL AND NEW.reversal_of_id IS NULL));
  SELECT RAISE(ABORT, 'fb_journal_guard: Journals must start as drafts') WHERE NEW.status IS NOT 'DRAFT';
  SELECT RAISE(ABORT, 'journal_retry_evidence: Invalid journal revision or retry evidence')
  WHERE NEW.revision NOT BETWEEN 1 AND 9223372036854775806 OR (NEW.request_key IS NOT NULL AND length(NEW.creation_digest) <> 64);
  SELECT RAISE(ABORT, 'fb_journal_guard: Invalid journal state')
  WHERE NEW.posted_at IS NOT NULL OR NEW.posted_by_id IS NOT NULL;
  SELECT RAISE(ABORT, 'journal_entry_kind: Only manual original journal entries carry an entry number or retry key; document journals are never adjusting')
  WHERE ((NEW.source_id IS NOT NULL OR NEW.reversal_of_id IS NOT NULL) AND (NEW.entry_number <> '' OR NEW.request_key IS NOT NULL)) OR (NEW.source_id IS NOT NULL AND NEW.adjusting);
END;
DROP TRIGGER fb_journal_guard_update;
CREATE TRIGGER fb_journal_guard_update BEFORE UPDATE ON ledger_journal
BEGIN
  SELECT RAISE(ABORT, 'fb_journal_guard: Update would replace an existing row') WHERE EXISTS (SELECT 1 FROM ledger_journal WHERE id IS NOT OLD.id AND (
    id=NEW.id OR number=NEW.number OR (company_id=NEW.company_id AND entry_number=NEW.entry_number AND NEW.entry_number<>'')
    OR (company_id=NEW.company_id AND request_key=NEW.request_key) OR reversal_of_id=NEW.reversal_of_id
    OR (source_id=NEW.source_id AND NEW.source_id IS NOT NULL AND reversal_of_id IS NULL AND NEW.reversal_of_id IS NULL)));
  SELECT RAISE(ABORT, 'fb_journal_guard: Posted journals are immutable; create a reversal') WHERE OLD.status = 'POSTED';
  SELECT RAISE(ABORT, 'fb_journal_guard: Journal ownership cannot change')
  WHERE NEW.company_id IS NOT OLD.company_id OR NEW.created_by_id IS NOT OLD.created_by_id
     OR NEW.created_at IS NOT OLD.created_at OR NEW.id IS NOT OLD.id OR NEW.number IS NOT OLD.number;
  SELECT RAISE(ABORT, 'journal_retry_evidence: Journal retry evidence and revision cannot change or go back')
  WHERE NEW.request_key IS NOT OLD.request_key OR NEW.creation_digest IS NOT OLD.creation_digest OR NEW.revision < OLD.revision;
  SELECT RAISE(ABORT, 'journal_retry_evidence: Invalid journal revision or retry evidence')
  WHERE NEW.revision NOT BETWEEN 1 AND 9223372036854775806 OR (NEW.request_key IS NOT NULL AND length(NEW.creation_digest) <> 64);
  SELECT RAISE(ABORT, 'fb_journal_guard: Invalid journal state')
  WHERE NEW.status IS NULL OR NEW.status NOT IN ('DRAFT', 'POSTED')
     OR (NEW.status = 'DRAFT' AND (NEW.posted_at IS NOT NULL OR NEW.posted_by_id IS NOT NULL));
  SELECT RAISE(ABORT, 'fb_journal_guard: Posting requires balanced nonzero lines and an identified operator')
  WHERE NEW.status = 'POSTED' AND (
        (SELECT count(*) FROM ledger_journalline WHERE journal_id = NEW.id) < 2
     OR coalesce((SELECT sum(debit) FROM ledger_journalline WHERE journal_id = NEW.id), 0) <= 0
     OR coalesce((SELECT sum(debit) FROM ledger_journalline WHERE journal_id = NEW.id), 0)
        <> coalesce((SELECT sum(credit) FROM ledger_journalline WHERE journal_id = NEW.id), 0)
     OR NEW.posted_by_id IS NULL OR NEW.posted_at IS NULL);
  SELECT RAISE(ABORT, 'fb_journal_guard: Invalid posting account')
  WHERE NEW.status = 'POSTED' AND EXISTS (
        SELECT 1 FROM ledger_journalline l LEFT JOIN ledger_account a ON a.id = l.account_id
        WHERE l.journal_id = NEW.id AND (a.id IS NULL OR a.company_id <> NEW.company_id OR a.active = 0 OR a.postable = 0));
  SELECT RAISE(ABORT, 'journal_entry_bounds: A manual journal entry needs an entry number, at most 50 lines and totals within 99999999999999.99')
  WHERE NEW.status = 'POSTED' AND NEW.source_id IS NULL AND NEW.reversal_of_id IS NULL AND (
        NEW.entry_number = ''
     OR (SELECT count(*) FROM ledger_journalline WHERE journal_id = NEW.id) > 50
     OR coalesce((SELECT sum(debit) FROM ledger_journalline WHERE journal_id = NEW.id), 0) > 9999999999999999);
  SELECT RAISE(ABORT, 'fb_journal_guard: Invalid reversal')
  WHERE NEW.status = 'POSTED' AND NEW.reversal_of_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM ledger_journal o WHERE o.id = NEW.reversal_of_id AND o.status = 'POSTED'
          AND o.company_id = NEW.company_id AND o.reversal_of_id IS NULL AND NEW.date >= o.date AND NEW.adjusting = o.adjusting);
  -- (journal_id, position) is unique, so EXCEPT both ways is exact (SQLite has no EXCEPT ALL; probe F4).
  SELECT RAISE(ABORT, 'fb_journal_guard: Reversal must exactly compensate original lines')
  WHERE NEW.status = 'POSTED' AND NEW.reversal_of_id IS NOT NULL AND (
        EXISTS (SELECT position, account_id, debit, credit FROM ledger_journalline WHERE journal_id = NEW.reversal_of_id
                EXCEPT SELECT position, account_id, credit, debit FROM ledger_journalline WHERE journal_id = NEW.id)
     OR EXISTS (SELECT position, account_id, credit, debit FROM ledger_journalline WHERE journal_id = NEW.id
                EXCEPT SELECT position, account_id, debit, credit FROM ledger_journalline WHERE journal_id = NEW.reversal_of_id));
  SELECT RAISE(ABORT, 'journal_entry_kind: Only manual original journal entries carry an entry number or retry key; document journals are never adjusting')
  WHERE ((NEW.source_id IS NOT NULL OR NEW.reversal_of_id IS NOT NULL) AND (NEW.entry_number <> '' OR NEW.request_key IS NOT NULL)) OR (NEW.source_id IS NOT NULL AND NEW.adjusting);
END;
CREATE TRIGGER fb_source_journal_insert BEFORE INSERT ON ledger_journal
BEGIN
SELECT RAISE(ABORT, 'source_journal_scope: Invalid journal source company or type') WHERE NEW.source_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM documents_documentidentity WHERE id=NEW.source_id AND company_id=NEW.company_id AND kind<>'EST');
SELECT RAISE(ABORT, 'source_journal_reversal: Reversal must retain original source ownership') WHERE NEW.reversal_of_id IS NOT NULL AND NEW.source_id IS NOT (SELECT source_id FROM ledger_journal WHERE id=NEW.reversal_of_id);
END;
CREATE TRIGGER fb_source_journal_update BEFORE UPDATE ON ledger_journal
BEGIN
SELECT RAISE(ABORT, 'source_journal_ownership: Journal source ownership is immutable') WHERE NEW.source_id IS NOT OLD.source_id;
SELECT RAISE(ABORT, 'source_journal_scope: Invalid journal source company or type') WHERE NEW.source_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM documents_documentidentity WHERE id=NEW.source_id AND company_id=NEW.company_id AND kind<>'EST');
SELECT RAISE(ABORT, 'source_journal_reversal: Reversal must retain original source ownership') WHERE NEW.reversal_of_id IS NOT NULL AND NEW.source_id IS NOT (SELECT source_id FROM ledger_journal WHERE id=NEW.reversal_of_id);
END;
CREATE TRIGGER fb_journal_control_account BEFORE UPDATE ON ledger_journal
WHEN NEW.status='POSTED' AND OLD.status='DRAFT' AND NEW.source_id IS NULL AND NEW.reversal_of_id IS NULL
AND EXISTS(SELECT 1 FROM ledger_journalline l JOIN fb_control_accounts c ON c.company_id=NEW.company_id AND c.account_id=l.account_id WHERE l.journal_id=NEW.id)
BEGIN SELECT RAISE(ABORT, 'journal_control_account: Manual journal entries cannot post to a receivables, payables, undeposited-funds or collected-tax control account (current setup or frozen by a document)'); END;
CREATE TRIGGER fb_account_configured_company BEFORE UPDATE ON ledger_account
WHEN NEW.company_id<>OLD.company_id AND (EXISTS(SELECT 1 FROM documents_accountingsetup WHERE OLD.id IN(receivables_id,payables_id,undeposited_id,collected_tax_id)) OR EXISTS(SELECT 1 FROM documents_designatedbank WHERE account_id=OLD.id))
BEGIN SELECT RAISE(ABORT, 'account_configured_company: Configured accounts cannot change company'); END;
