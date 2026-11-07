-- Close alternate-UNIQUE REPLACE collisions while recursive_triggers is off.
DROP TRIGGER fb_payables_documents_documentidentity_replace;
CREATE TRIGGER fb_payables_documents_documentidentity_replace BEFORE INSERT ON documents_documentidentity
WHEN EXISTS(SELECT 1 FROM documents_documentidentity old WHERE (old.id=NEW.id OR (old.company_id=NEW.company_id AND (old.number=NEW.number OR old.request_key=NEW.request_key OR (old.kind=NEW.kind AND old.ordinal=NEW.ordinal)))) AND (EXISTS(SELECT 1 FROM payables_bill WHERE identity_id=old.id) OR EXISTS(SELECT 1 FROM payables_billpayment WHERE identity_id=old.id)))
BEGIN SELECT RAISE(ABORT,'payables_parent: Referenced parents cannot be deleted or replaced'); END;
DROP TRIGGER fb_payables_ledger_journal_replace;
CREATE TRIGGER fb_payables_ledger_journal_replace BEFORE INSERT ON ledger_journal
WHEN EXISTS(SELECT 1 FROM ledger_journal old WHERE (old.id=NEW.id OR old.number=NEW.number OR (NEW.reversal_of_id IS NOT NULL AND old.reversal_of_id=NEW.reversal_of_id) OR (old.company_id=NEW.company_id AND ((NEW.entry_number<>'' AND old.entry_number=NEW.entry_number) OR (NEW.request_key IS NOT NULL AND old.request_key=NEW.request_key)))) AND (EXISTS(SELECT 1 FROM payables_billrevision WHERE old.id IN(journal_id,reversal_id)) OR EXISTS(SELECT 1 FROM payables_billpayment WHERE journal_id=old.id) OR EXISTS(SELECT 1 FROM payables_paymentreversal WHERE journal_id=old.id)))
BEGIN SELECT RAISE(ABORT,'payables_parent: Referenced parents cannot be deleted or replaced'); END;
