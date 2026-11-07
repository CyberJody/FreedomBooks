-- L1 review hardening: replacement collisions, parent existence, revision bounds.

DROP TRIGGER fb_company_guard_insert;
CREATE TRIGGER fb_company_guard_insert BEFORE INSERT ON ledger_company
BEGIN
  SELECT RAISE(ABORT, 'fb_company_guard: Company key already exists') WHERE EXISTS (SELECT 1 FROM ledger_company WHERE id=NEW.id);
  SELECT RAISE(ABORT, 'fb_company_guard: Currency must be three uppercase letters') WHERE NEW.currency IS NULL OR length(NEW.currency) <> 3 OR NEW.currency GLOB '*[^A-Z]*';
END;

DROP TRIGGER fb_account_guard_insert;
CREATE TRIGGER fb_account_guard_insert BEFORE INSERT ON ledger_account
BEGIN
  SELECT RAISE(ABORT, 'fb_account_guard: Account key already exists') WHERE EXISTS (SELECT 1 FROM ledger_account WHERE id=NEW.id OR (company_id=NEW.company_id AND code=NEW.code));
  SELECT RAISE(ABORT, 'fb_account_guard: Invalid account type') WHERE NEW.kind IS NULL OR NEW.kind NOT IN ('ASSET', 'LIABILITY', 'EQUITY', 'INCOME', 'EXPENSE');
END;

DROP TRIGGER fb_journal_guard_insert;
CREATE TRIGGER fb_journal_guard_insert BEFORE INSERT ON ledger_journal
BEGIN
  SELECT RAISE(ABORT, 'fb_journal_guard: Journal key already exists') WHERE EXISTS (
    SELECT 1 FROM ledger_journal WHERE id=NEW.id OR number=NEW.number
      OR (company_id=NEW.company_id AND entry_number=NEW.entry_number AND NEW.entry_number<>'')
      OR (company_id=NEW.company_id AND request_key=NEW.request_key)
      OR reversal_of_id=NEW.reversal_of_id);
  SELECT RAISE(ABORT, 'fb_journal_guard: Journals must start as drafts') WHERE NEW.status IS NOT 'DRAFT';
  SELECT RAISE(ABORT, 'journal_retry_evidence: Invalid journal revision or retry evidence')
  WHERE NEW.revision NOT BETWEEN 1 AND 9223372036854775806 OR (NEW.request_key IS NOT NULL AND length(NEW.creation_digest) <> 64);
  SELECT RAISE(ABORT, 'fb_journal_guard: Invalid journal state')
  WHERE NEW.posted_at IS NOT NULL OR NEW.posted_by_id IS NOT NULL;
  SELECT RAISE(ABORT, 'journal_entry_kind: Only manual original journal entries carry an entry number or retry key; document journals are never adjusting')
  WHERE NEW.reversal_of_id IS NOT NULL AND (NEW.entry_number <> '' OR NEW.request_key IS NOT NULL);
END;

DROP TRIGGER fb_journal_guard_update;
CREATE TRIGGER fb_journal_guard_update BEFORE UPDATE ON ledger_journal
BEGIN
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
  WHERE NEW.status = 'POSTED' AND NEW.reversal_of_id IS NULL AND (
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
  WHERE NEW.reversal_of_id IS NOT NULL AND (NEW.entry_number <> '' OR NEW.request_key IS NOT NULL);
END;

DROP TRIGGER fb_journal_guard_delete;
CREATE TRIGGER fb_journal_guard_delete BEFORE DELETE ON ledger_journal WHEN OLD.status = 'POSTED' OR EXISTS (SELECT 1 FROM ledger_journalline WHERE journal_id=OLD.id) OR EXISTS (SELECT 1 FROM ledger_journal WHERE reversal_of_id=OLD.id)
BEGIN SELECT RAISE(ABORT, 'fb_journal_guard: Posted journals are immutable; create a reversal'); END;

DROP TRIGGER fb_line_guard_insert;
CREATE TRIGGER fb_line_guard_insert BEFORE INSERT ON ledger_journalline
BEGIN
  SELECT RAISE(ABORT, 'fb_line_guard: Line key already exists') WHERE EXISTS (SELECT 1 FROM ledger_journalline WHERE id=NEW.id OR (journal_id=NEW.journal_id AND position=NEW.position));
  SELECT RAISE(ABORT, 'fb_line_guard: Cannot add or move lines to posted journals')
  WHERE (SELECT status FROM ledger_journal WHERE id = NEW.journal_id) IS NOT 'DRAFT';
  SELECT RAISE(ABORT, 'fb_line_guard: Line must use an active posting account in its company')
  WHERE NOT EXISTS (SELECT 1 FROM ledger_account a, ledger_journal j WHERE a.id = NEW.account_id AND j.id = NEW.journal_id
                AND a.company_id = j.company_id AND a.active = 1 AND a.postable = 1);
END;

DROP TRIGGER fb_line_guard_update;
CREATE TRIGGER fb_line_guard_update BEFORE UPDATE ON ledger_journalline
BEGIN
  SELECT RAISE(ABORT, 'fb_line_guard: Posted lines are immutable')
  WHERE (SELECT status FROM ledger_journal WHERE id = OLD.journal_id) = 'POSTED';
  SELECT RAISE(ABORT, 'fb_line_guard: Cannot add or move lines to posted journals')
  WHERE (SELECT status FROM ledger_journal WHERE id = NEW.journal_id) IS NOT 'DRAFT';
  SELECT RAISE(ABORT, 'fb_line_guard: Line must use an active posting account in its company')
  WHERE NOT EXISTS (SELECT 1 FROM ledger_account a, ledger_journal j WHERE a.id = NEW.account_id AND j.id = NEW.journal_id
                AND a.company_id = j.company_id AND a.active = 1 AND a.postable = 1);
END;


CREATE TRIGGER fb_company_guard_delete BEFORE DELETE ON ledger_company
WHEN EXISTS (SELECT 1 FROM ledger_journal WHERE company_id=OLD.id)
  OR EXISTS (SELECT 1 FROM ledger_account WHERE company_id=OLD.id)
  OR EXISTS (SELECT 1 FROM ledger_auditevent WHERE company_id=OLD.id)
  OR EXISTS (SELECT 1 FROM ledger_company_members WHERE company_id=OLD.id)
BEGIN SELECT RAISE(ABORT, 'fb_company_guard: Referenced companies cannot be deleted'); END;
CREATE TRIGGER fb_account_guard_delete BEFORE DELETE ON ledger_account
WHEN EXISTS (SELECT 1 FROM ledger_journalline WHERE account_id=OLD.id)
BEGIN SELECT RAISE(ABORT, 'fb_account_guard: Used accounts cannot be deleted'); END;
CREATE TRIGGER fb_audit_guard_insert BEFORE INSERT ON ledger_auditevent
WHEN EXISTS (SELECT 1 FROM ledger_auditevent WHERE id=NEW.id)
BEGIN SELECT RAISE(ABORT, 'fb_audit_guard: Audit events are append-only'); END;
CREATE TRIGGER fb_user_guard_insert BEFORE INSERT ON fb_user
WHEN EXISTS (SELECT 1 FROM fb_user WHERE id=NEW.id OR username=NEW.username)
BEGIN SELECT RAISE(ABORT, 'fb_user_guard: Operator key already exists'); END;
CREATE TRIGGER fb_user_guard_delete BEFORE DELETE ON fb_user
WHEN EXISTS (SELECT 1 FROM ledger_auditevent WHERE actor_id=OLD.id)
  OR EXISTS (SELECT 1 FROM ledger_journal WHERE created_by_id=OLD.id OR posted_by_id=OLD.id)
  OR EXISTS (SELECT 1 FROM ledger_company_members WHERE user_id=OLD.id)
BEGIN SELECT RAISE(ABORT, 'fb_user_guard: Referenced operators cannot be deleted'); END;
CREATE TRIGGER fb_seal_guard_insert BEFORE INSERT ON fb_seal
WHEN EXISTS (SELECT 1 FROM fb_seal WHERE kind=NEW.kind AND doc_id=NEW.doc_id)
BEGIN SELECT RAISE(ABORT, 'seal_immutable: Seals are permanent'); END;
CREATE TRIGGER fb_seal_due_guard_insert BEFORE INSERT ON fb_seal_due
WHEN EXISTS (SELECT 1 FROM fb_seal_due WHERE kind=NEW.kind AND doc_id=NEW.doc_id)
BEGIN SELECT RAISE(ABORT, 'seal_immutable: Seal obligations are permanent'); END;
