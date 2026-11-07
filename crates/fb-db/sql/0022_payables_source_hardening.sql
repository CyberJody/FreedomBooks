-- Payables' own protection around source journal and frozen account evidence.
-- Does not replace L1 guards; these additive guards also hold on the pre-hardening L2a base.
CREATE TRIGGER fb_payables_journal_line_replace BEFORE INSERT ON ledger_journalline
WHEN EXISTS(SELECT 1 FROM ledger_journalline old JOIN ledger_journal j ON j.id=old.journal_id JOIN documents_documentidentity i ON i.id=j.source_id WHERE (old.id=NEW.id OR (old.journal_id=NEW.journal_id AND old.position=NEW.position)) AND j.status='POSTED' AND i.kind IN('BILL','BPAY'))
BEGIN SELECT RAISE(ABORT,'payables_posted_line: Posted source lines cannot be replaced'); END;
CREATE TRIGGER fb_payables_documentaccount_replace BEFORE INSERT ON documents_documentaccount
WHEN EXISTS(SELECT 1 FROM documents_documentaccount old JOIN documents_documentidentity i ON i.id=old.source_id WHERE (old.id=NEW.id OR (old.source_id=NEW.source_id AND old.role=NEW.role)) AND i.kind IN('BILL','BPAY'))
BEGIN SELECT RAISE(ABORT,'payables_frozen_account: Frozen source accounts cannot be replaced'); END;
CREATE TRIGGER fb_payables_post_eligibility BEFORE UPDATE ON ledger_journal
WHEN NEW.status='POSTED' AND EXISTS(SELECT 1 FROM documents_documentidentity WHERE id=NEW.source_id AND kind IN('BILL','BPAY')) AND EXISTS(SELECT 1 FROM ledger_journalline l WHERE l.journal_id=NEW.id AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=l.account_id AND a.company_id=NEW.company_id AND a.active=1 AND a.postable=1))
BEGIN SELECT RAISE(ABORT,'payables_post_account: Existing eligible accounts required at posting'); END;
-- Guard line parent order immediately, even with the base ledger's deferred foreign keys.
CREATE TRIGGER fb_payables_line_parent BEFORE INSERT ON payables_billline
WHEN NOT EXISTS(SELECT 1 FROM payables_billrevision WHERE id=NEW.revision_id)
BEGIN SELECT RAISE(ABORT,'bill_line_scope: Invalid bill line scope or committed history'); END;
