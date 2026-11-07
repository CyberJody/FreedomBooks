-- L1-FIX3: primary keys never change, even before they acquire references.
-- Deferred FKs cannot prevent rekey + reinsert from redirecting historical identity.
-- Add independent monotonic guards; leave migrations 0001-0003 and collision guards intact.
-- IS NOT permits no-op key assignments used by ordinary full-row UPDATE statements.

CREATE TRIGGER fb_user_primary_key_permanent BEFORE UPDATE OF id ON fb_user
WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT, 'primary_key_immutable: Primary keys never change'); END;

CREATE TRIGGER ledger_company_primary_key_permanent BEFORE UPDATE OF id ON ledger_company
WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT, 'primary_key_immutable: Primary keys never change'); END;

CREATE TRIGGER ledger_account_primary_key_permanent BEFORE UPDATE OF id ON ledger_account
WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT, 'primary_key_immutable: Primary keys never change'); END;

CREATE TRIGGER ledger_journal_primary_key_permanent BEFORE UPDATE OF id ON ledger_journal
WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT, 'primary_key_immutable: Primary keys never change'); END;

CREATE TRIGGER ledger_journalline_primary_key_permanent BEFORE UPDATE OF id ON ledger_journalline
WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT, 'primary_key_immutable: Primary keys never change'); END;

CREATE TRIGGER ledger_auditevent_primary_key_permanent BEFORE UPDATE OF id ON ledger_auditevent
WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT, 'primary_key_immutable: Primary keys never change'); END;

CREATE TRIGGER ledger_company_members_primary_key_permanent BEFORE UPDATE OF company_id, user_id ON ledger_company_members
WHEN NEW.company_id IS NOT OLD.company_id OR NEW.user_id IS NOT OLD.user_id
BEGIN SELECT RAISE(ABORT, 'primary_key_immutable: Primary keys never change'); END;

CREATE TRIGGER fb_seal_primary_key_permanent BEFORE UPDATE OF kind, doc_id ON fb_seal
WHEN NEW.kind IS NOT OLD.kind OR NEW.doc_id IS NOT OLD.doc_id
BEGIN SELECT RAISE(ABORT, 'primary_key_immutable: Primary keys never change'); END;

CREATE TRIGGER fb_seal_due_primary_key_permanent BEFORE UPDATE OF kind, doc_id ON fb_seal_due
WHEN NEW.kind IS NOT OLD.kind OR NEW.doc_id IS NOT OLD.doc_id
BEGIN SELECT RAISE(ABORT, 'primary_key_immutable: Primary keys never change'); END;

CREATE TRIGGER fb_account_company_permanent BEFORE UPDATE OF company_id ON ledger_account
WHEN NEW.company_id IS NOT OLD.company_id
BEGIN SELECT RAISE(ABORT, 'identity_immutable: Account ownership never changes'); END;

CREATE TRIGGER fb_line_journal_permanent BEFORE UPDATE OF journal_id ON ledger_journalline
WHEN NEW.journal_id IS NOT OLD.journal_id
BEGIN SELECT RAISE(ABORT, 'identity_immutable: Line ownership never changes'); END;

CREATE TRIGGER fb_journal_origin_permanent BEFORE UPDATE OF reversal_of_id ON ledger_journal
WHEN NEW.reversal_of_id IS NOT OLD.reversal_of_id
BEGIN SELECT RAISE(ABORT, 'identity_immutable: Reversal origin never changes'); END;
