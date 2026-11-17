-- Sales tax payments (lane taxpay). Two parts:
-- 1. Allow the new document kind STP in documents_documentsequence and documents_documentidentity. SQLite cannot
--    alter a CHECK constraint, so both tables are rebuilt with the one value added, every row copied back unchanged,
--    and every trigger on them recreated with its exact prior text, in its prior order. Foreign keys that point at
--    the identities are deferred for the copy and checked at commit. fb_numbering keeps its kind list: sales tax
--    payments number from 1 with no numbering setting.
-- 2. The payment and void tables, their guards, the journal seal, value guards and the open-time evidence view.
PRAGMA defer_foreign_keys=ON;
CREATE TEMP TABLE taxpay_sequence_copy AS SELECT * FROM documents_documentsequence;
CREATE TEMP TABLE taxpay_identity_copy AS SELECT * FROM documents_documentidentity;
DROP TABLE documents_documentidentity;
DROP TABLE documents_documentsequence;
CREATE TABLE documents_documentsequence (
id TEXT PRIMARY KEY, company_id TEXT REFERENCES ledger_company(id) DEFERRABLE INITIALLY DEFERRED NOT NULL, kind TEXT NOT NULL CONSTRAINT document_sequence_kind CHECK(kind IN('EST','INV','RCP','BILL','BPAY','DEP','CHK','SR','TRF','CM','RFD','CDP','CDA','CDR','STP')),
last_number INTEGER NOT NULL DEFAULT 0 CONSTRAINT document_sequence_limit CHECK(last_number BETWEEN 0 AND 999999999999),
CONSTRAINT document_sequence_per_company_kind UNIQUE(company_id,kind)) STRICT, WITHOUT ROWID;
CREATE TABLE documents_documentidentity (
id TEXT PRIMARY KEY, company_id TEXT REFERENCES ledger_company(id) DEFERRABLE INITIALLY DEFERRED NOT NULL, kind TEXT NOT NULL CONSTRAINT document_identity_kind CHECK(kind IN('EST','INV','RCP','BILL','BPAY','DEP','CHK','SR','TRF','CM','RFD','CDP','CDA','CDR','STP')),
ordinal INTEGER NOT NULL CONSTRAINT document_identity_ordinal CHECK(ordinal BETWEEN 1 AND 999999999999), number TEXT NOT NULL CHECK(length(number)<=17),
request_key TEXT NOT NULL, currency TEXT NOT NULL, created_at TEXT NOT NULL,
CONSTRAINT document_number_per_company UNIQUE(company_id,number),CONSTRAINT document_ordinal_per_company_kind UNIQUE(company_id,kind,ordinal),CONSTRAINT document_request_per_company UNIQUE(company_id,request_key)) STRICT, WITHOUT ROWID;
INSERT INTO documents_documentsequence SELECT * FROM temp.taxpay_sequence_copy;
INSERT INTO documents_documentidentity SELECT * FROM temp.taxpay_identity_copy;
DROP TABLE temp.taxpay_sequence_copy;
DROP TABLE temp.taxpay_identity_copy;
-- The prior triggers on both tables, verbatim (indented one space so the value-guard inventory, which reads lines that start with CREATE TRIGGER, lists each guard once; SQLite stores the text from CREATE).
 CREATE TRIGGER fb_document_sequence_delete BEFORE DELETE ON documents_documentsequence
BEGIN SELECT RAISE(ABORT, 'document_sequence_delete: Document counters cannot be deleted'); END;
 CREATE TRIGGER fb_document_sequence_update BEFORE UPDATE ON documents_documentsequence
WHEN NEW.company_id<>OLD.company_id OR NEW.kind<>OLD.kind OR NEW.last_number<OLD.last_number
BEGIN SELECT RAISE(ABORT, 'document_sequence_forward: Document counters cannot be reassigned or decreased'); END;
 CREATE TRIGGER fb_document_identity_update BEFORE UPDATE ON documents_documentidentity
BEGIN SELECT RAISE(ABORT, 'document_identity_immutable: Document identities cannot change or be deleted, including cancelled documents'); END;
 CREATE TRIGGER fb_document_identity_delete BEFORE DELETE ON documents_documentidentity
BEGIN SELECT RAISE(ABORT, 'document_identity_immutable: Document identities cannot change or be deleted, including cancelled documents'); END;
 CREATE TRIGGER fb_document_identity_insert BEFORE INSERT ON documents_documentidentity
WHEN NEW.currency IS NOT (SELECT currency FROM ledger_company WHERE id=NEW.company_id) OR NEW.number<>NEW.kind||'-'||printf('%06d',NEW.ordinal)
BEGIN SELECT RAISE(ABORT, 'document_identity: Document identity must match company currency and allocated number'); END;
 CREATE TRIGGER fb_l2_documents_documentsequence_insert BEFORE INSERT ON documents_documentsequence
BEGIN
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM documents_documentsequence WHERE id=NEW.id OR (company_id=NEW.company_id AND kind=NEW.kind));
END;
 CREATE TRIGGER fb_l2_documents_documentsequence_update BEFORE UPDATE ON documents_documentsequence
BEGIN
  SELECT RAISE(ABORT, 'l2_identity: Row identity is permanent') WHERE NEW.id IS NOT OLD.id;
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM documents_documentsequence WHERE id<>OLD.id AND (id=NEW.id OR (company_id=NEW.company_id AND kind=NEW.kind)));
  SELECT RAISE(ABORT, 'l2_company: Row company is permanent') WHERE NEW.company_id IS NOT OLD.company_id;
END;
 CREATE TRIGGER fb_l2_documents_documentsequence_company_insert BEFORE INSERT ON documents_documentsequence
BEGIN
  SELECT RAISE(ABORT, 'l2_parent: Company must exist') WHERE NOT EXISTS(SELECT 1 FROM ledger_company WHERE id=NEW.company_id);
END;
 CREATE TRIGGER fb_l2_documents_documentsequence_company_update BEFORE UPDATE ON documents_documentsequence
BEGIN
  SELECT RAISE(ABORT, 'l2_parent: Company must exist') WHERE NOT EXISTS(SELECT 1 FROM ledger_company WHERE id=NEW.company_id);
END;
 CREATE TRIGGER fb_l2_documents_documentidentity_insert BEFORE INSERT ON documents_documentidentity
BEGIN
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM documents_documentidentity WHERE id=NEW.id OR (company_id=NEW.company_id AND number=NEW.number) OR (company_id=NEW.company_id AND kind=NEW.kind AND ordinal=NEW.ordinal) OR (company_id=NEW.company_id AND request_key=NEW.request_key));
END;
 CREATE TRIGGER fb_l2_documents_documentidentity_update BEFORE UPDATE ON documents_documentidentity
BEGIN
  SELECT RAISE(ABORT, 'l2_identity: Row identity is permanent') WHERE NEW.id IS NOT OLD.id;
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM documents_documentidentity WHERE id<>OLD.id AND (id=NEW.id OR (company_id=NEW.company_id AND number=NEW.number) OR (company_id=NEW.company_id AND kind=NEW.kind AND ordinal=NEW.ordinal) OR (company_id=NEW.company_id AND request_key=NEW.request_key)));
  SELECT RAISE(ABORT, 'l2_company: Row company is permanent') WHERE NEW.company_id IS NOT OLD.company_id;
END;
 CREATE TRIGGER fb_l2_documents_documentidentity_company_insert BEFORE INSERT ON documents_documentidentity
BEGIN
  SELECT RAISE(ABORT, 'l2_parent: Company must exist') WHERE NOT EXISTS(SELECT 1 FROM ledger_company WHERE id=NEW.company_id);
END;
 CREATE TRIGGER fb_l2_documents_documentidentity_company_update BEFORE UPDATE ON documents_documentidentity
BEGIN
  SELECT RAISE(ABORT, 'l2_parent: Company must exist') WHERE NOT EXISTS(SELECT 1 FROM ledger_company WHERE id=NEW.company_id);
END;
 CREATE TRIGGER fb_l2_documents_documentsequence_primary_key_permanent BEFORE UPDATE OF id ON documents_documentsequence
WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT, 'primary_key_immutable: Primary keys never change'); END;
 CREATE TRIGGER fb_l2_documents_documentidentity_primary_key_permanent BEFORE UPDATE OF id ON documents_documentidentity
WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT, 'primary_key_immutable: Primary keys never change'); END;
 CREATE TRIGGER fb_l2_documents_documentsequence_ownership_permanent BEFORE UPDATE OF company_id, kind ON documents_documentsequence
WHEN NEW.company_id IS NOT OLD.company_id OR NEW.kind IS NOT OLD.kind
BEGIN SELECT RAISE(ABORT, 'identity_immutable: Ownership identities never change'); END;
 CREATE TRIGGER fb_l2_documents_documentidentity_ownership_permanent BEFORE UPDATE OF company_id ON documents_documentidentity
WHEN NEW.company_id IS NOT OLD.company_id
BEGIN SELECT RAISE(ABORT, 'identity_immutable: Ownership identities never change'); END;
 CREATE TRIGGER fb_sales_documents_documentidentity_collision BEFORE INSERT ON documents_documentidentity
BEGIN SELECT RAISE(ABORT,'sales_reference_protected: Referenced sales evidence cannot be replaced') WHERE EXISTS(SELECT 1 FROM documents_documentidentity o WHERE (o.id=NEW.id OR (o.company_id=NEW.company_id AND (o.request_key=NEW.request_key OR (o.kind=NEW.kind AND o.ordinal=NEW.ordinal) OR o.number=NEW.number))) AND (EXISTS(SELECT 1 FROM sales_invoice WHERE identity_id=o.id) OR EXISTS(SELECT 1 FROM sales_receipt WHERE identity_id=o.id))); END;
 CREATE TRIGGER fb_sales_documents_documentidentity_kept BEFORE DELETE ON documents_documentidentity
BEGIN SELECT RAISE(ABORT,'sales_reference_protected: Referenced sales evidence cannot be deleted') WHERE EXISTS(SELECT 1 FROM documents_documentidentity o WHERE o.id=OLD.id AND (EXISTS(SELECT 1 FROM sales_invoice WHERE identity_id=o.id) OR EXISTS(SELECT 1 FROM sales_receipt WHERE identity_id=o.id))); END;
 CREATE TRIGGER fb_payables_documents_documentidentity_kept BEFORE DELETE ON documents_documentidentity WHEN EXISTS(SELECT 1 FROM payables_bill WHERE identity_id=OLD.id) OR EXISTS(SELECT 1 FROM payables_billpayment WHERE identity_id=OLD.id) BEGIN SELECT RAISE(ABORT,'payables_parent: Referenced parents cannot be deleted or replaced'); END;
 CREATE TRIGGER fb_payables_documents_documentidentity_replace BEFORE INSERT ON documents_documentidentity
WHEN EXISTS(SELECT 1 FROM documents_documentidentity old WHERE (old.id=NEW.id OR (old.company_id=NEW.company_id AND (old.number=NEW.number OR old.request_key=NEW.request_key OR (old.kind=NEW.kind AND old.ordinal=NEW.ordinal)))) AND (EXISTS(SELECT 1 FROM payables_bill WHERE identity_id=old.id) OR EXISTS(SELECT 1 FROM payables_billpayment WHERE identity_id=old.id)))
BEGIN SELECT RAISE(ABORT,'payables_parent: Referenced parents cannot be deleted or replaced'); END;
 CREATE TRIGGER fb_checks_documents_documentidentity_kept BEFORE DELETE ON documents_documentidentity WHEN EXISTS(SELECT 1 FROM checks_check WHERE identity_id=OLD.id) OR EXISTS(SELECT 1 FROM transfers_transfer WHERE identity_id=OLD.id) BEGIN SELECT RAISE(ABORT,'checks_parent_retained: Referenced parents cannot be deleted'); END;
 CREATE TRIGGER fb_checks_documents_documentidentity_insert BEFORE INSERT ON documents_documentidentity WHEN EXISTS(SELECT 1 FROM documents_documentidentity a WHERE (a.id=NEW.id OR (a.company_id=NEW.company_id AND ((a.kind=NEW.kind AND a.number=NEW.number) OR a.request_key=NEW.request_key))) AND (EXISTS(SELECT 1 FROM checks_check WHERE identity_id=a.id) OR EXISTS(SELECT 1 FROM transfers_transfer WHERE identity_id=a.id))) BEGIN SELECT RAISE(ABORT,'checks_parent_retained: Replacement would discard a referenced parent'); END;
 CREATE TRIGGER fb_checks_documents_documentidentity_update BEFORE UPDATE ON documents_documentidentity WHEN EXISTS(SELECT 1 FROM documents_documentidentity a WHERE (a.id=NEW.id OR (a.company_id=NEW.company_id AND ((a.kind=NEW.kind AND a.number=NEW.number) OR a.request_key=NEW.request_key))) AND a.id<>OLD.id AND (EXISTS(SELECT 1 FROM checks_check WHERE identity_id=a.id) OR EXISTS(SELECT 1 FROM transfers_transfer WHERE identity_id=a.id))) BEGIN SELECT RAISE(ABORT,'checks_parent_retained: Replacement would discard a referenced parent'); END;
 CREATE TRIGGER pp_retain_documents_documentidentity BEFORE DELETE ON documents_documentidentity
WHEN EXISTS(SELECT 1 FROM prepayments_depositapplication WHERE identity_id=OLD.id) OR
 EXISTS(SELECT 1 FROM prepayments_depositrefund WHERE identity_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'prepayment_parent_retained: Referenced prepayment parents cannot be deleted'); END;
 CREATE TRIGGER attachment_document_id_kept BEFORE DELETE ON documents_documentidentity WHEN EXISTS(SELECT 1 FROM attachments_attachment WHERE document_id=OLD.id) BEGIN SELECT RAISE(ABORT,'attachment_parent: Attachment parents cannot be deleted'); END;
 CREATE TRIGGER fb_valid_documents_documentidentity_insert BEFORE INSERT ON documents_documentidentity BEGIN
 SELECT RAISE(ABORT,'fb_valid: Invalid stored text, identifier, date or timestamp') WHERE
 NOT fb_valid_uuid(NEW.id) OR
 NOT fb_valid_uuid(NEW.company_id) OR
 NOT fb_valid_text(NEW.kind) OR
 NOT fb_valid_text(NEW.number) OR
 NOT fb_valid_uuid(NEW.request_key) OR
 NOT fb_valid_text(NEW.currency) OR
 NOT fb_valid_timestamp(NEW.created_at);
END;
 CREATE TRIGGER fb_valid_documents_documentidentity_update BEFORE UPDATE ON documents_documentidentity BEGIN
 SELECT RAISE(ABORT,'fb_valid: Invalid stored text, identifier, date or timestamp') WHERE
 NOT fb_valid_uuid(NEW.id) OR
 NOT fb_valid_uuid(NEW.company_id) OR
 NOT fb_valid_text(NEW.kind) OR
 NOT fb_valid_text(NEW.number) OR
 NOT fb_valid_uuid(NEW.request_key) OR
 NOT fb_valid_text(NEW.currency) OR
 NOT fb_valid_timestamp(NEW.created_at);
END;
 CREATE TRIGGER fb_valid_documents_documentsequence_insert BEFORE INSERT ON documents_documentsequence BEGIN
 SELECT RAISE(ABORT,'fb_valid: Invalid stored text, identifier, date or timestamp') WHERE
 NOT fb_valid_uuid(NEW.id) OR
 NOT fb_valid_uuid(NEW.company_id) OR
 NOT fb_valid_text(NEW.kind);
END;
 CREATE TRIGGER fb_valid_documents_documentsequence_update BEFORE UPDATE ON documents_documentsequence BEGIN
 SELECT RAISE(ABORT,'fb_valid: Invalid stored text, identifier, date or timestamp') WHERE
 NOT fb_valid_uuid(NEW.id) OR
 NOT fb_valid_uuid(NEW.company_id) OR
 NOT fb_valid_text(NEW.kind);
END;
 CREATE TRIGGER fb_document_import_number BEFORE INSERT ON documents_documentidentity
WHEN NEW.kind='INV' AND EXISTS(SELECT 1 FROM sales_importinvoice WHERE company_id=NEW.company_id AND source_number=NEW.number)
BEGIN SELECT RAISE(ABORT,'invoice_import_number: This number belongs to an imported invoice. Choose a different invoice numbering sequence.'); END;
-- Sales tax payments: the one document allowed to reduce collected sales tax.
CREATE TABLE salestax_payment (
 id TEXT PRIMARY KEY, company_id TEXT NOT NULL REFERENCES ledger_company(id),
 identity_id TEXT NOT NULL UNIQUE REFERENCES documents_documentidentity(id),
 creation_digest TEXT NOT NULL CHECK(length(creation_digest)=64), review_digest TEXT NOT NULL CHECK(length(review_digest)=64),
 agency_id TEXT NOT NULL REFERENCES directory_vendor(id), agency_name TEXT NOT NULL CHECK(length(agency_name) BETWEEN 1 AND 500),
 bank_id TEXT NOT NULL REFERENCES ledger_account(id), tax_account_id TEXT REFERENCES ledger_account(id), expense_account_id TEXT REFERENCES ledger_account(id),
 date TEXT NOT NULL CHECK(date(date) IS date),
 tax_amount INTEGER NOT NULL CHECK(tax_amount BETWEEN 0 AND 9999999999999999), penalty_amount INTEGER NOT NULL CHECK(penalty_amount BETWEEN 0 AND 9999999999999999),
 period_start_date TEXT CHECK(period_start_date IS NULL OR date(period_start_date) IS period_start_date),
 period_end_date TEXT CHECK(period_end_date IS NULL OR date(period_end_date) IS period_end_date),
 reference TEXT NOT NULL CHECK(length(reference)<=40), memo TEXT NOT NULL CHECK(length(memo)<=2000),
 journal_id TEXT NOT NULL UNIQUE REFERENCES ledger_journal(id), actor_id TEXT NOT NULL REFERENCES fb_user(id), actor_name TEXT NOT NULL, created_at TEXT NOT NULL,
 CHECK(tax_amount+penalty_amount BETWEEN 1 AND 9999999999999999),
 CHECK((tax_amount>0)=(tax_account_id IS NOT NULL)),
 CHECK((penalty_amount>0)=(expense_account_id IS NOT NULL)),
 CHECK((period_start_date IS NULL)=(period_end_date IS NULL) AND (period_start_date IS NULL OR period_start_date<=period_end_date))
) STRICT, WITHOUT ROWID;
CREATE TABLE salestax_paymentvoid (
 id TEXT PRIMARY KEY, payment_id TEXT NOT NULL UNIQUE REFERENCES salestax_payment(id), journal_id TEXT NOT NULL UNIQUE REFERENCES ledger_journal(id),
 date TEXT NOT NULL CHECK(date(date) IS date), reason TEXT NOT NULL CHECK(length(trim(reason)) BETWEEN 1 AND 500),
 actor_id TEXT NOT NULL REFERENCES fb_user(id), actor_name TEXT NOT NULL, created_at TEXT NOT NULL
) STRICT, WITHOUT ROWID;
CREATE TRIGGER fb_salestax_payment_insert BEFORE INSERT ON salestax_payment BEGIN
 SELECT RAISE(ABORT,'salestax_payment_immutable: Sales tax payment history is immutable') WHERE EXISTS(SELECT 1 FROM salestax_payment WHERE id=NEW.id OR identity_id=NEW.identity_id OR journal_id=NEW.journal_id);
 SELECT RAISE(ABORT,'salestax_payment_scope: Invalid sales tax payment ownership, agency or posting') WHERE
  NOT EXISTS(SELECT 1 FROM documents_documentidentity WHERE id=NEW.identity_id AND company_id=NEW.company_id AND kind='STP')
  OR NOT EXISTS(SELECT 1 FROM directory_vendor v WHERE v.id=NEW.agency_id AND v.company_id=NEW.company_id AND EXISTS(SELECT 1 FROM salestax_taxitem t WHERE t.company_id=NEW.company_id AND t.agency_id=v.id))
  OR NOT EXISTS(SELECT 1 FROM ledger_journal WHERE id=NEW.journal_id AND company_id=NEW.company_id AND source_id=NEW.identity_id AND status='POSTED' AND reversal_of_id IS NULL AND date=NEW.date)
  OR NOT EXISTS(SELECT 1 FROM fb_user WHERE id=NEW.actor_id);
 SELECT RAISE(ABORT,'salestax_payment_bank: A sales tax payment is paid from an active designated bank account') WHERE NOT EXISTS(SELECT 1 FROM ledger_account a JOIN documents_designatedbank b ON b.account_id=a.id JOIN documents_accountingsetup s ON s.id=b.setup_id WHERE a.id=NEW.bank_id AND a.company_id=NEW.company_id AND s.company_id=NEW.company_id AND a.kind='ASSET' AND a.active=1 AND a.postable=1);
 SELECT RAISE(ABORT,'salestax_payment_tax_account: Only the configured collected sales tax account can be paid down') WHERE NEW.tax_account_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM documents_accountingsetup s JOIN ledger_account a ON a.id=s.collected_tax_id WHERE s.company_id=NEW.company_id AND s.collected_tax_id=NEW.tax_account_id AND a.company_id=NEW.company_id AND a.kind='LIABILITY' AND a.active=1 AND a.postable=1);
 SELECT RAISE(ABORT,'salestax_payment_expense: Penalty and interest go to an active expense account that is not a control account') WHERE NEW.expense_account_id IS NOT NULL AND (NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=NEW.expense_account_id AND a.company_id=NEW.company_id AND a.kind='EXPENSE' AND a.active=1 AND a.postable=1) OR EXISTS(SELECT 1 FROM fb_control_accounts WHERE company_id=NEW.company_id AND account_id=NEW.expense_account_id));
 SELECT RAISE(ABORT,'salestax_payment_journal_split: The payment journal credits the bank with the total and debits only collected tax and the penalty account') WHERE
  (SELECT count(*) FROM ledger_journalline WHERE journal_id=NEW.journal_id)<>1+(NEW.tax_amount>0)+(NEW.penalty_amount>0)
  OR NOT EXISTS(SELECT 1 FROM ledger_journalline WHERE journal_id=NEW.journal_id AND account_id=NEW.bank_id AND debit=0 AND credit=NEW.tax_amount+NEW.penalty_amount)
  OR (NEW.tax_amount>0 AND NOT EXISTS(SELECT 1 FROM ledger_journalline WHERE journal_id=NEW.journal_id AND account_id=NEW.tax_account_id AND debit=NEW.tax_amount AND credit=0))
  OR (NEW.penalty_amount>0 AND NOT EXISTS(SELECT 1 FROM ledger_journalline WHERE journal_id=NEW.journal_id AND account_id=NEW.expense_account_id AND debit=NEW.penalty_amount AND credit=0));
 SELECT RAISE(ABORT,'salestax_payment_account_evidence: Payment account evidence is exactly the bank, collected tax and penalty accounts') WHERE
  (SELECT count(*) FROM documents_documentaccount WHERE source_id=NEW.identity_id)<>1+(NEW.tax_amount>0)+(NEW.penalty_amount>0)
  OR NOT EXISTS(SELECT 1 FROM documents_documentaccount WHERE source_id=NEW.identity_id AND role='bank' AND account_id=NEW.bank_id)
  OR (NEW.tax_amount>0 AND NOT EXISTS(SELECT 1 FROM documents_documentaccount WHERE source_id=NEW.identity_id AND role='collected_tax' AND account_id=NEW.tax_account_id))
  OR (NEW.penalty_amount>0 AND NOT EXISTS(SELECT 1 FROM documents_documentaccount WHERE source_id=NEW.identity_id AND role='expense' AND account_id=NEW.expense_account_id));
END;
CREATE TRIGGER fb_salestax_payment_update BEFORE UPDATE ON salestax_payment BEGIN SELECT RAISE(ABORT,'salestax_payment_immutable: Sales tax payment history is immutable'); END;
CREATE TRIGGER fb_salestax_payment_delete BEFORE DELETE ON salestax_payment BEGIN SELECT RAISE(ABORT,'salestax_payment_immutable: Sales tax payment history cannot be deleted'); END;
CREATE TRIGGER fb_salestax_paymentvoid_insert BEFORE INSERT ON salestax_paymentvoid BEGIN
 SELECT RAISE(ABORT,'salestax_payment_immutable: Sales tax payment void history is immutable') WHERE EXISTS(SELECT 1 FROM salestax_paymentvoid WHERE id=NEW.id OR payment_id=NEW.payment_id OR journal_id=NEW.journal_id);
 SELECT RAISE(ABORT,'salestax_payment_void_link: A void posts the reversal of its payment, dated on or after the payment') WHERE
  NOT EXISTS(SELECT 1 FROM salestax_payment t JOIN ledger_journal j ON j.source_id=t.identity_id AND j.company_id=t.company_id AND j.reversal_of_id=t.journal_id WHERE t.id=NEW.payment_id AND NEW.date>=t.date AND j.id=NEW.journal_id AND j.date=NEW.date AND j.status='POSTED')
  OR NOT EXISTS(SELECT 1 FROM fb_user WHERE id=NEW.actor_id);
END;
CREATE TRIGGER fb_salestax_paymentvoid_update BEFORE UPDATE ON salestax_paymentvoid BEGIN SELECT RAISE(ABORT,'salestax_payment_immutable: Sales tax payment void history is immutable'); END;
CREATE TRIGGER fb_salestax_paymentvoid_delete BEFORE DELETE ON salestax_paymentvoid BEGIN SELECT RAISE(ABORT,'salestax_payment_immutable: Sales tax payment void history cannot be deleted'); END;
-- Every STP journal must be owned by its payment (original) or that payment's void (reversal) before commit.
CREATE TRIGGER fb_salestax_journal_due AFTER INSERT ON ledger_journal WHEN EXISTS(SELECT 1 FROM documents_documentidentity WHERE id=NEW.source_id AND kind='STP') BEGIN INSERT INTO fb_seal_due VALUES('salestax_journal',NEW.id); END;
CREATE TRIGGER fb_salestax_journal_seal BEFORE INSERT ON fb_seal WHEN NEW.kind='salestax_journal' BEGIN
 SELECT RAISE(ABORT,'salestax_journal_owner: Sales tax payment journals require the owning payment lifecycle') WHERE NOT EXISTS(SELECT 1 FROM ledger_journal j JOIN salestax_payment t ON t.identity_id=j.source_id WHERE j.id=NEW.doc_id AND j.status='POSTED' AND ((j.reversal_of_id IS NULL AND t.journal_id=j.id) OR (t.journal_id=j.reversal_of_id AND EXISTS(SELECT 1 FROM salestax_paymentvoid v WHERE v.payment_id=t.id AND v.journal_id=j.id))));
END;
CREATE TRIGGER fb_salestax_late_reference BEFORE INSERT ON documents_documentaccount WHEN EXISTS(SELECT 1 FROM ledger_journal j JOIN fb_seal s ON s.kind='salestax_journal' AND s.doc_id=j.id WHERE j.source_id=NEW.source_id) BEGIN SELECT RAISE(ABORT,'salestax_late_reference: Cannot append posted account evidence'); END;
CREATE TRIGGER fb_valid_salestax_payment_insert BEFORE INSERT ON salestax_payment WHEN NOT fb_valid_uuid(NEW.id) OR NOT fb_valid_uuid(NEW.company_id) OR NOT fb_valid_uuid(NEW.identity_id) OR NOT fb_valid_text(NEW.creation_digest) OR NOT fb_valid_text(NEW.review_digest) OR NOT fb_valid_uuid(NEW.agency_id) OR NOT fb_valid_text(NEW.agency_name) OR NOT fb_valid_uuid(NEW.bank_id) OR NOT fb_valid_uuid(NEW.tax_account_id) OR NOT fb_valid_uuid(NEW.expense_account_id) OR NOT fb_valid_date(NEW.date) OR NOT fb_valid_date(NEW.period_start_date) OR NOT fb_valid_date(NEW.period_end_date) OR NOT fb_valid_text(NEW.reference) OR NOT fb_valid_text(NEW.memo) OR NOT fb_valid_uuid(NEW.journal_id) OR NOT fb_valid_uuid(NEW.actor_id) OR NOT fb_valid_text(NEW.actor_name) OR NOT fb_valid_timestamp(NEW.created_at) BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value'); END;
CREATE TRIGGER fb_valid_salestax_payment_update BEFORE UPDATE ON salestax_payment WHEN NOT fb_valid_uuid(NEW.id) OR NOT fb_valid_uuid(NEW.company_id) OR NOT fb_valid_uuid(NEW.identity_id) OR NOT fb_valid_text(NEW.creation_digest) OR NOT fb_valid_text(NEW.review_digest) OR NOT fb_valid_uuid(NEW.agency_id) OR NOT fb_valid_text(NEW.agency_name) OR NOT fb_valid_uuid(NEW.bank_id) OR NOT fb_valid_uuid(NEW.tax_account_id) OR NOT fb_valid_uuid(NEW.expense_account_id) OR NOT fb_valid_date(NEW.date) OR NOT fb_valid_date(NEW.period_start_date) OR NOT fb_valid_date(NEW.period_end_date) OR NOT fb_valid_text(NEW.reference) OR NOT fb_valid_text(NEW.memo) OR NOT fb_valid_uuid(NEW.journal_id) OR NOT fb_valid_uuid(NEW.actor_id) OR NOT fb_valid_text(NEW.actor_name) OR NOT fb_valid_timestamp(NEW.created_at) BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value'); END;
CREATE TRIGGER fb_valid_salestax_paymentvoid_insert BEFORE INSERT ON salestax_paymentvoid WHEN NOT fb_valid_uuid(NEW.id) OR NOT fb_valid_uuid(NEW.payment_id) OR NOT fb_valid_uuid(NEW.journal_id) OR NOT fb_valid_date(NEW.date) OR NOT fb_valid_text(NEW.reason) OR NOT fb_valid_uuid(NEW.actor_id) OR NOT fb_valid_text(NEW.actor_name) OR NOT fb_valid_timestamp(NEW.created_at) BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value'); END;
CREATE TRIGGER fb_valid_salestax_paymentvoid_update BEFORE UPDATE ON salestax_paymentvoid WHEN NOT fb_valid_uuid(NEW.id) OR NOT fb_valid_uuid(NEW.payment_id) OR NOT fb_valid_uuid(NEW.journal_id) OR NOT fb_valid_date(NEW.date) OR NOT fb_valid_text(NEW.reason) OR NOT fb_valid_uuid(NEW.actor_id) OR NOT fb_valid_text(NEW.actor_name) OR NOT fb_valid_timestamp(NEW.created_at) BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value'); END;
-- Open-time evidence: the same rules, for books that a connection without these triggers could have changed.
CREATE VIEW salestax_payment_invalid AS
 SELECT t.id FROM salestax_payment t WHERE
  NOT EXISTS(SELECT 1 FROM documents_documentidentity i WHERE i.id=t.identity_id AND i.company_id=t.company_id AND i.kind='STP')
  OR NOT EXISTS(SELECT 1 FROM ledger_journal j WHERE j.id=t.journal_id AND j.company_id=t.company_id AND j.source_id=t.identity_id AND j.status='POSTED' AND j.reversal_of_id IS NULL AND j.date=t.date)
  OR NOT EXISTS(SELECT 1 FROM directory_vendor v WHERE v.id=t.agency_id AND v.company_id=t.company_id)
  OR NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=t.bank_id AND a.company_id=t.company_id AND a.kind='ASSET')
  OR (t.tax_account_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=t.tax_account_id AND a.company_id=t.company_id AND a.kind='LIABILITY'))
  OR (t.expense_account_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=t.expense_account_id AND a.company_id=t.company_id AND a.kind='EXPENSE'))
  OR t.tax_amount<0 OR t.penalty_amount<0 OR t.tax_amount+t.penalty_amount<=0 OR (t.tax_amount>0)<>(t.tax_account_id IS NOT NULL) OR (t.penalty_amount>0)<>(t.expense_account_id IS NOT NULL)
  OR (SELECT count(*) FROM ledger_journalline WHERE journal_id=t.journal_id)<>1+(t.tax_amount>0)+(t.penalty_amount>0)
  OR NOT EXISTS(SELECT 1 FROM ledger_journalline WHERE journal_id=t.journal_id AND account_id=t.bank_id AND debit=0 AND credit=t.tax_amount+t.penalty_amount)
  OR (t.tax_amount>0 AND NOT EXISTS(SELECT 1 FROM ledger_journalline WHERE journal_id=t.journal_id AND account_id=t.tax_account_id AND debit=t.tax_amount AND credit=0))
  OR (t.penalty_amount>0 AND NOT EXISTS(SELECT 1 FROM ledger_journalline WHERE journal_id=t.journal_id AND account_id=t.expense_account_id AND debit=t.penalty_amount AND credit=0))
  OR (SELECT count(*) FROM documents_documentaccount WHERE source_id=t.identity_id)<>1+(t.tax_amount>0)+(t.penalty_amount>0)
  OR NOT EXISTS(SELECT 1 FROM fb_seal WHERE kind='salestax_journal' AND doc_id=t.journal_id)
 UNION ALL SELECT v.id FROM salestax_paymentvoid v LEFT JOIN salestax_payment t ON t.id=v.payment_id WHERE t.id IS NULL OR v.date<t.date
  OR NOT EXISTS(SELECT 1 FROM ledger_journal j WHERE j.id=v.journal_id AND j.reversal_of_id=t.journal_id AND j.source_id=t.identity_id AND j.status='POSTED' AND j.date=v.date)
  OR NOT EXISTS(SELECT 1 FROM fb_seal WHERE kind='salestax_journal' AND doc_id=v.journal_id)
 UNION ALL SELECT j.id FROM ledger_journal j JOIN documents_documentidentity i ON i.id=j.source_id WHERE i.kind='STP' AND NOT EXISTS(SELECT 1 FROM fb_seal WHERE kind='salestax_journal' AND doc_id=j.id);
DROP VIEW fb_value_invalid;
CREATE VIEW fb_value_invalid AS
SELECT 'invalid stored value in accounts_owner' AS problem FROM accounts_owner WHERE
 NOT fb_valid_uuid(user_id)
UNION ALL
SELECT 'invalid stored value in assistant_connection' AS problem FROM assistant_connection WHERE
 NOT fb_valid_text(account_email) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(created_by_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(key_last4) OR
 NOT fb_valid_text(last_error) OR
 NOT fb_valid_text(mode) OR
 NOT fb_valid_text(model) OR
 NOT fb_valid_text(model_choices) OR
 NOT fb_valid_text(status)
UNION ALL
SELECT 'invalid stored value in assistant_reading' AS problem FROM assistant_reading WHERE
 NOT fb_valid_uuid(bill_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(content_type) OR
 NOT fb_valid_uuid(created_by_id) OR
 NOT fb_valid_text(error) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(mode) OR
 NOT fb_valid_text(model) OR
 NOT fb_valid_uuid(origin_bill_id) OR
 NOT fb_valid_text(original_name) OR
 NOT fb_valid_uuid(request_key) OR
 NOT fb_valid_text(result) OR
 NOT fb_valid_text(sha256) OR
 NOT fb_valid_uuid(source_file_id) OR
 NOT fb_valid_text(status)
UNION ALL
SELECT 'invalid stored value in attachments_attachment' AS problem FROM attachments_attachment WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(content_type) OR
 NOT fb_valid_text(delete_reason) OR
 NOT fb_valid_timestamp(deleted_at) OR
 NOT fb_valid_uuid(deleted_by_id) OR
 NOT fb_valid_text(deleted_by_name) OR
 NOT fb_valid_text(deleted_number) OR
 NOT fb_valid_uuid(document_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(nonce) OR
 NOT fb_valid_text(original_name) OR
 NOT fb_valid_text(original_sha256) OR
 NOT fb_valid_uuid(request_key) OR
 NOT fb_valid_text(sha256) OR
 NOT fb_valid_text(stored_name) OR
 NOT fb_valid_timestamp(uploaded_at) OR
 NOT fb_valid_uuid(uploaded_by_id) OR
 NOT fb_valid_text(uploaded_by_name) OR
 NOT fb_valid_text(uploaded_number) OR
 NOT fb_valid_text(wrap_nonce) OR
 NOT fb_valid_text(wrapped_key)
UNION ALL
SELECT 'invalid stored value in bankfeeds_account' AS problem FROM bankfeeds_account WHERE
 NOT fb_valid_timestamp(balance_at) OR
 NOT fb_valid_text(conn_key) OR
 NOT fb_valid_text(conn_key_identity) OR
 NOT fb_valid_uuid(connection_id) OR
 NOT fb_valid_text(currency) OR
 NOT fb_valid_timestamp(first_seen_at) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_timestamp(last_seen_at) OR
 NOT fb_valid_uuid(ledger_account_id) OR
 NOT fb_valid_timestamp(mapped_at) OR
 NOT fb_valid_text(name) OR
 NOT fb_valid_text(org_name) OR
 NOT fb_valid_text(remote_id) OR
 NOT fb_valid_text(remote_identity)
UNION ALL
SELECT 'invalid stored value in bankfeeds_connection' AS problem FROM bankfeeds_connection WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_uuid(created_by_id) OR
 NOT fb_valid_timestamp(forgotten_at) OR
 NOT fb_valid_text(host) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(label) OR
 NOT fb_valid_timestamp(last_sync_at) OR
 NOT fb_valid_text(last_sync_summary)
UNION ALL
SELECT 'invalid stored value in bankfeeds_counter' AS problem FROM bankfeeds_counter WHERE
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in bankfeeds_identity_secret' AS problem FROM bankfeeds_identity_secret WHERE
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in bankfeeds_retired' AS problem FROM bankfeeds_retired WHERE
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in bankfeeds_rule' AS problem FROM bankfeeds_rule WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(match_text) OR
 NOT fb_valid_text(name) OR
 NOT fb_valid_text(payee_name) OR
 NOT fb_valid_text(sign) OR
 NOT fb_valid_timestamp(updated_at)
UNION ALL
SELECT 'invalid stored value in bankfeeds_transaction' AS problem FROM bankfeeds_transaction WHERE
 NOT fb_valid_text(accepted_as) OR
 NOT fb_valid_timestamp(decided_at) OR
 NOT fb_valid_uuid(decided_by_id) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(feed_account_id) OR
 NOT fb_valid_timestamp(first_seen_at) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(journal_line_id) OR
 NOT fb_valid_text(posted_on) OR
 NOT fb_valid_text(reference) OR
 NOT fb_valid_text(reference_url) OR
 NOT fb_valid_text(remote_id) OR
 NOT fb_valid_text(remote_identity) OR
 NOT fb_valid_text(status) OR
 NOT fb_valid_text(transacted_on) OR
 NOT fb_valid_timestamp(updated_at)
UNION ALL
SELECT 'invalid stored value in banking_banklinestatus' AS problem FROM banking_banklinestatus WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_uuid(changed_by_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_line_id) OR
 NOT fb_valid_uuid(reconciliation_id) OR
 NOT fb_valid_text(state)
UNION ALL
SELECT 'invalid stored value in banking_deposit' AS problem FROM banking_deposit WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id)
UNION ALL
SELECT 'invalid stored value in banking_depositline' AS problem FROM banking_depositline WHERE
 NOT fb_valid_uuid(deposit_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(receipt_id) OR
 NOT fb_valid_uuid(undeposited_id)
UNION ALL
SELECT 'invalid stored value in banking_depositreversal' AS problem FROM banking_depositreversal WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(deposit_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id)
UNION ALL
SELECT 'invalid stored value in banking_reconciliation' AS problem FROM banking_reconciliation WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(request_key) OR
 NOT fb_valid_uuid(started_by_id) OR
 NOT fb_valid_date(statement_date) OR
 NOT fb_valid_text(status)
UNION ALL
SELECT 'invalid stored value in banking_reconciliationactor' AS problem FROM banking_reconciliationactor WHERE
 NOT fb_valid_text(phase) OR
 NOT fb_valid_uuid(reconciliation_id) OR
 NOT fb_valid_uuid(user_id)
UNION ALL
SELECT 'invalid stored value in banking_reconciliationline' AS problem FROM banking_reconciliationline WHERE
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_line_id) OR
 NOT fb_valid_uuid(reconciliation_id)
UNION ALL
SELECT 'invalid stored value in banking_seal' AS problem FROM banking_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in banking_seal_due' AS problem FROM banking_seal_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in checks_check' AS problem FROM checks_check WHERE
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_text(number_key) OR
 NOT fb_valid_text(state) OR
 NOT fb_valid_uuid(vendor_id)
UNION ALL
SELECT 'invalid stored value in checks_checkline' AS problem FROM checks_checkline WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_text(account_snapshot) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(item_id) OR
 NOT fb_valid_text(item_snapshot) OR
 NOT fb_valid_uuid(revision_id) OR
 NOT fb_valid_text(tab)
UNION ALL
SELECT 'invalid stored value in checks_checkrevision' AS problem FROM checks_checkrevision WHERE
 NOT fb_valid_text(action) OR
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_text(address) OR
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_text(bank_snapshot) OR
 NOT fb_valid_text(check_number) OR
 NOT fb_valid_text(company_snapshot) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(document_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(memo) OR
 NOT fb_valid_text(payee_kind) OR
 NOT fb_valid_text(payee_name) OR
 NOT fb_valid_text(payee_snapshot) OR
 NOT fb_valid_text(reason) OR
 NOT fb_valid_uuid(reversal_id) OR
 NOT fb_valid_text(review_digest) OR
 NOT fb_valid_text(state) OR
 NOT fb_valid_uuid(vendor_id)
UNION ALL
SELECT 'invalid stored value in credits_creditapplication' AS problem FROM credits_creditapplication WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(credit_memo_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(request_key)
UNION ALL
SELECT 'invalid stored value in credits_creditapplicationline' AS problem FROM credits_creditapplicationline WHERE
 NOT fb_valid_uuid(application_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(invoice_revision_id) OR
 NOT fb_valid_uuid(receivables_id)
UNION ALL
SELECT 'invalid stored value in credits_creditmemo' AS problem FROM credits_creditmemo WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(collected_tax_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(receivables_id) OR
 NOT fb_valid_uuid(tax_item_id)
UNION ALL
SELECT 'invalid stored value in credits_creditmemoline' AS problem FROM credits_creditmemoline WHERE
 NOT fb_valid_uuid(credit_memo_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_uuid(item_id)
UNION ALL
SELECT 'invalid stored value in credits_creditmemovoid' AS problem FROM credits_creditmemovoid WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(credit_memo_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id)
UNION ALL
SELECT 'invalid stored value in credits_creditrefund' AS problem FROM credits_creditrefund WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_text(check_number) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(credit_memo_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(method) OR
 NOT fb_valid_uuid(receivables_id)
UNION ALL
SELECT 'invalid stored value in credits_creditrefundvoid' AS problem FROM credits_creditrefundvoid WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(refund_id)
UNION ALL
SELECT 'invalid stored value in credits_creditunapplication' AS problem FROM credits_creditunapplication WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(application_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id)
UNION ALL
SELECT 'invalid stored value in credits_seal' AS problem FROM credits_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in credits_seal_due' AS problem FROM credits_seal_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in directory_customer' AS problem FROM directory_customer WHERE
 NOT fb_valid_text(billing_address) OR
 NOT fb_valid_text(business_name) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(contact_name) OR
 NOT fb_valid_text(display_name) OR
 NOT fb_valid_text(email) OR
 NOT fb_valid_text(exemption_number) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(notes) OR
 NOT fb_valid_text(phone) OR
 NOT fb_valid_text(tax_status)
UNION ALL
SELECT 'invalid stored value in directory_item' AS problem FROM directory_item WHERE
 NOT fb_valid_text(code) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(expense_account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in directory_vendor' AS problem FROM directory_vendor WHERE
 NOT fb_valid_text(billing_address) OR
 NOT fb_valid_text(business_name) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(contact_name) OR
 NOT fb_valid_uuid(default_expense_account_id) OR
 NOT fb_valid_text(default_terms) OR
 NOT fb_valid_text(display_name) OR
 NOT fb_valid_text(email) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(notes) OR
 NOT fb_valid_text(phone)
UNION ALL
SELECT 'invalid stored value in documents_accountingsetup' AS problem FROM documents_accountingsetup WHERE
 NOT fb_valid_uuid(collected_tax_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(customer_deposits_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(payables_id) OR
 NOT fb_valid_uuid(receivables_id) OR
 NOT fb_valid_uuid(undeposited_id)
UNION ALL
SELECT 'invalid stored value in documents_designatedbank' AS problem FROM documents_designatedbank WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(setup_id)
UNION ALL
SELECT 'invalid stored value in documents_documentaccount' AS problem FROM documents_documentaccount WHERE
 NOT fb_valid_text(account_code) OR
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_text(account_kind) OR
 NOT fb_valid_text(account_name) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(role) OR
 NOT fb_valid_uuid(source_id)
UNION ALL
SELECT 'invalid stored value in documents_documentidentity' AS problem FROM documents_documentidentity WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_text(currency) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(kind) OR
 NOT fb_valid_text(number) OR
 NOT fb_valid_uuid(request_key)
UNION ALL
SELECT 'invalid stored value in documents_documentsequence' AS problem FROM documents_documentsequence WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in estimate_seal' AS problem FROM estimate_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in estimate_seal_due' AS problem FROM estimate_seal_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in fb_seal' AS problem FROM fb_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in fb_seal_due' AS problem FROM fb_seal_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in fb_user' AS problem FROM fb_user WHERE
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(username)
UNION ALL
SELECT 'invalid stored value in ledger_account' AS problem FROM ledger_account WHERE
 NOT fb_valid_text(code) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_text(detail_type) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(kind) OR
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in ledger_auditevent' AS problem FROM ledger_auditevent WHERE
 NOT fb_valid_text(action) OR
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(detail) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_timestamp(occurred_at) OR
 NOT fb_valid_text(target)
UNION ALL
SELECT 'invalid stored value in ledger_company' AS problem FROM ledger_company WHERE
 NOT fb_valid_text(currency) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in ledger_company_members' AS problem FROM ledger_company_members WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(user_id)
UNION ALL
SELECT 'invalid stored value in ledger_journal' AS problem FROM ledger_journal WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_uuid(created_by_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_text(entry_number) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(memo) OR
 NOT fb_valid_timestamp(posted_at) OR
 NOT fb_valid_uuid(posted_by_id) OR
 NOT fb_valid_uuid(request_key) OR
 NOT fb_valid_uuid(reversal_of_id) OR
 NOT fb_valid_uuid(source_id) OR
 NOT fb_valid_text(status)
UNION ALL
SELECT 'invalid stored value in ledger_journalline' AS problem FROM ledger_journalline WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in payables_bill' AS problem FROM payables_bill WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_text(reference_key) OR
 NOT fb_valid_text(state) OR
 NOT fb_valid_uuid(vendor_id)
UNION ALL
SELECT 'invalid stored value in payables_billline' AS problem FROM payables_billline WHERE
 NOT fb_valid_text(account_snapshot) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(expense_account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_uuid(item_id) OR
 NOT fb_valid_text(item_snapshot) OR
 NOT fb_valid_uuid(revision_id)
UNION ALL
SELECT 'invalid stored value in payables_billpayment' AS problem FROM payables_billpayment WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(company_snapshot) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(memo) OR
 NOT fb_valid_text(method) OR
 NOT fb_valid_text(reference) OR
 NOT fb_valid_text(review_digest) OR
 NOT fb_valid_uuid(vendor_id) OR
 NOT fb_valid_text(vendor_snapshot)
UNION ALL
SELECT 'invalid stored value in payables_billrevision' AS problem FROM payables_billrevision WHERE
 NOT fb_valid_text(action) OR
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_uuid(bill_id) OR
 NOT fb_valid_text(company_snapshot) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_date(due_date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(memo) OR
 NOT fb_valid_text(reason) OR
 NOT fb_valid_text(reentry_reason) OR
 NOT fb_valid_text(reference) OR
 NOT fb_valid_uuid(reversal_id) OR
 NOT fb_valid_text(review_digest) OR
 NOT fb_valid_text(state) OR
 NOT fb_valid_uuid(supersedes_id) OR
 NOT fb_valid_uuid(vendor_id) OR
 NOT fb_valid_text(vendor_snapshot)
UNION ALL
SELECT 'invalid stored value in payables_paymentallocation' AS problem FROM payables_paymentallocation WHERE
 NOT fb_valid_uuid(bill_id) OR
 NOT fb_valid_uuid(bill_revision_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(payables_id) OR
 NOT fb_valid_uuid(payment_id)
UNION ALL
SELECT 'invalid stored value in payables_paymentreversal' AS problem FROM payables_paymentreversal WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(payment_id) OR
 NOT fb_valid_text(reason)
UNION ALL
SELECT 'invalid stored value in plugin_credential' AS problem FROM plugin_credential WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(plugin_id) OR
 NOT fb_valid_uuid(connection_id)
UNION ALL
SELECT 'invalid stored value in prepayments_customerdeposit' AS problem FROM prepayments_customerdeposit WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(estimate_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(liability_id) OR
 NOT fb_valid_text(order_reference) OR
 NOT fb_valid_uuid(receipt_id)
UNION ALL
SELECT 'invalid stored value in prepayments_depositapplication' AS problem FROM prepayments_depositapplication WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(deposit_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(invoice_revision_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(liability_id) OR
 NOT fb_valid_uuid(receivables_id)
UNION ALL
SELECT 'invalid stored value in prepayments_depositrefund' AS problem FROM prepayments_depositrefund WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_text(check_number) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(deposit_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(liability_id) OR
 NOT fb_valid_text(method)
UNION ALL
SELECT 'invalid stored value in prepayments_depositrefundvoid' AS problem FROM prepayments_depositrefundvoid WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(refund_id)
UNION ALL
SELECT 'invalid stored value in prepayments_depositunapplication' AS problem FROM prepayments_depositunapplication WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(application_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id)
UNION ALL
SELECT 'invalid stored value in prepayments_due' AS problem FROM prepayments_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in prepayments_seal' AS problem FROM prepayments_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in sales_estimate' AS problem FROM sales_estimate WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id)
UNION ALL
SELECT 'invalid stored value in sales_estimateconversion' AS problem FROM sales_estimateconversion WHERE
 NOT fb_valid_text(digest) OR
 NOT fb_valid_uuid(estimate_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(revision_id)
UNION ALL
SELECT 'invalid stored value in sales_estimateline' AS problem FROM sales_estimateline WHERE
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(expense_account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_uuid(item_id) OR
 NOT fb_valid_uuid(revision_id)
UNION ALL
SELECT 'invalid stored value in sales_estimaterevision' AS problem FROM sales_estimaterevision WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(estimate_id) OR
 NOT fb_valid_text(expiry) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(state)
UNION ALL
SELECT 'invalid stored value in sales_invoice' AS problem FROM sales_invoice WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id)
UNION ALL
SELECT 'invalid stored value in sales_invoiceline' AS problem FROM sales_invoiceline WHERE
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(expense_account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_uuid(item_id) OR
 NOT fb_valid_uuid(revision_id)
UNION ALL
SELECT 'invalid stored value in sales_invoicerevision' AS problem FROM sales_invoicerevision WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_date(due_date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(reversal_id) OR
 NOT fb_valid_text(state)
UNION ALL
SELECT 'invalid stored value in sales_receipt' AS problem FROM sales_receipt WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_text(destination) OR
 NOT fb_valid_uuid(destination_account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in sales_receiptallocation' AS problem FROM sales_receiptallocation WHERE
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(invoice_revision_id) OR
 NOT fb_valid_uuid(receipt_id) OR
 NOT fb_valid_uuid(receivables_id)
UNION ALL
SELECT 'invalid stored value in sales_receiptreversal' AS problem FROM sales_receiptreversal WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(receipt_id)
UNION ALL
SELECT 'invalid stored value in sales_salesreceipt' AS problem FROM sales_salesreceipt WHERE
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(receipt_id)
UNION ALL
SELECT 'invalid stored value in sales_salesreceiptline' AS problem FROM sales_salesreceiptline WHERE
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_uuid(item_id) OR
 NOT fb_valid_uuid(sale_id)
UNION ALL
SELECT 'invalid stored value in sales_seal' AS problem FROM sales_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in sales_seal_due' AS problem FROM sales_seal_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in salestax_salestaxsettings' AS problem FROM salestax_salestaxsettings WHERE
 NOT fb_valid_text(bulk_step) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(default_item_id) OR
 NOT fb_valid_text(filing_frequency) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(owed_basis)
UNION ALL
SELECT 'invalid stored value in salestax_taxgroupmember' AS problem FROM salestax_taxgroupmember WHERE
 NOT fb_valid_uuid(group_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(member_id)
UNION ALL
SELECT 'invalid stored value in salestax_taxitem' AS problem FROM salestax_taxitem WHERE
 NOT fb_valid_uuid(agency_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(kind) OR
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in transfers_transfer' AS problem FROM transfers_transfer WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(company_snapshot) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(from_account_id) OR
 NOT fb_valid_text(from_snapshot) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(memo) OR
 NOT fb_valid_text(review_digest) OR
 NOT fb_valid_uuid(to_account_id) OR
 NOT fb_valid_text(to_snapshot)
UNION ALL
SELECT 'invalid stored value in transfers_transfervoid' AS problem FROM transfers_transfervoid WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(reason) OR
 NOT fb_valid_uuid(transfer_id)
UNION ALL SELECT 'invalid stored value in company_document_details' AS problem FROM company_document_details WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_text(logo_mime) OR (fb_json_unique_keys(data) AND NOT fb_company_document_valid(data))
UNION ALL
SELECT 'invalid stored value in fb_numbering' AS problem FROM fb_numbering WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(kind) OR
 NOT fb_valid_text(scope) OR
 NOT fb_valid_text(start) OR
 NOT fb_valid_uuid(audit_id)
UNION ALL
SELECT 'invalid stored value in desktop_user' AS problem FROM desktop_user WHERE
 NOT fb_valid_uuid(user_id) OR
 NOT fb_valid_text(role) OR
 NOT fb_valid_text(password_hash)
UNION ALL
SELECT 'invalid stored value in desktop_mfa_totp' FROM desktop_mfa_totp WHERE NOT fb_valid_uuid(user_id)
UNION ALL
SELECT 'invalid stored value in desktop_mfa_recovery' FROM desktop_mfa_recovery WHERE NOT fb_valid_uuid(user_id)
UNION ALL
SELECT 'invalid stored value in invoice_preferences' AS problem FROM invoice_preferences WHERE
 NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(data)
UNION ALL
SELECT 'invalid stored value in invoice_email_connection' AS problem FROM invoice_email_connection WHERE
 NOT fb_valid_text(client_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(from_address) OR
 NOT fb_valid_text(status) OR
 NOT fb_valid_timestamp(updated_at) OR
 NOT fb_valid_uuid(updated_by_id)
UNION ALL
SELECT 'invalid stored value in invoice_email_delivery' AS problem FROM invoice_email_delivery WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_timestamp(completed_at) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_uuid(created_by_id) OR
 NOT fb_valid_text(document_hash) OR
 NOT fb_valid_text(error_code) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(invoice_revision_id) OR
 NOT fb_valid_text(message_key) OR
 NOT fb_valid_text(mode) OR
 NOT fb_valid_text(pdf_sha256) OR
 NOT fb_valid_text(provider_draft) OR
 NOT fb_valid_text(provider_message) OR
 NOT fb_valid_text(state) OR
 NOT fb_valid_text(subject) OR
 NOT fb_valid_text(to_address)
UNION ALL
SELECT 'invalid stored value in invoice_stripe_connection' AS problem FROM invoice_stripe_connection WHERE NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(account_key) OR NOT fb_valid_text(mode) OR NOT fb_valid_text(key_last4) OR NOT fb_valid_text(status) OR NOT fb_valid_timestamp(created_at) OR NOT fb_valid_timestamp(updated_at) OR NOT fb_valid_uuid(updated_by_id)
UNION ALL
SELECT 'invalid stored value in invoice_stripe_link' AS problem FROM invoice_stripe_link WHERE NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_uuid(connection_id) OR NOT fb_valid_uuid(invoice_id) OR NOT fb_valid_uuid(invoice_revision_id) OR NOT fb_valid_uuid(created_by_id) OR NOT fb_valid_text(description) OR NOT fb_valid_text(currency) OR NOT fb_valid_text(state) OR NOT fb_valid_text(provider_key) OR NOT fb_valid_text(payment_url) OR NOT fb_valid_text(cursor) OR NOT fb_valid_timestamp(created_at) OR NOT fb_valid_timestamp(checked_at)
UNION ALL
SELECT 'invalid stored value in invoice_stripe_payment' AS problem FROM invoice_stripe_payment WHERE NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_uuid(connection_id) OR NOT fb_valid_uuid(link_id) OR NOT fb_valid_text(account_key) OR NOT fb_valid_text(mode) OR NOT fb_valid_text(session_key) OR NOT fb_valid_text(intent_key) OR NOT fb_valid_text(currency) OR NOT fb_valid_text(provider_state) OR NOT fb_valid_timestamp(observed_at) OR NOT fb_valid_uuid(receipt_id)
UNION ALL
SELECT 'invalid stored value in bankfeeds_rule_policy' AS problem FROM bankfeeds_rule_policy WHERE NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(data) OR NOT fb_valid_uuid(rule_id) OR NOT fb_valid_text(source_hash) OR NOT fb_valid_text(source_key)
UNION ALL
SELECT 'invalid stored value in bankfeeds_rule_review' AS problem FROM bankfeeds_rule_review WHERE NOT fb_valid_uuid(item_id) OR NOT fb_valid_text(reason)
UNION ALL
SELECT 'invalid stored value in bankfeeds_auto_queue' AS problem FROM bankfeeds_auto_queue WHERE NOT fb_valid_text(fingerprint) OR NOT fb_valid_uuid(item_id) OR NOT fb_valid_uuid(rule_id)
UNION ALL SELECT 'sales_importinvoice:'||invoice_id FROM sales_importinvoice WHERE NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(source) OR NOT fb_valid_uuid(source_id) OR NOT fb_valid_text(source_number) OR NOT fb_valid_uuid(invoice_id) OR NOT fb_valid_uuid(revision_id) OR NOT fb_valid_text(digest) OR NOT fb_valid_text(data) OR NOT fb_valid_uuid(actor_id) OR NOT fb_valid_uuid(audit_id)
UNION ALL SELECT 'sales_importpayment:'||invoice_id FROM sales_importpayment WHERE NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(source) OR NOT fb_valid_uuid(source_id) OR NOT fb_valid_uuid(invoice_id) OR NOT fb_valid_uuid(receipt_id) OR NOT fb_valid_text(external_key)
UNION ALL SELECT 'sales_importseal:'||invoice_id FROM sales_importseal WHERE NOT fb_valid_uuid(invoice_id)
UNION ALL SELECT 'salestax_payment:'||id FROM salestax_payment WHERE NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_uuid(identity_id) OR NOT fb_valid_text(creation_digest) OR NOT fb_valid_text(review_digest) OR NOT fb_valid_uuid(agency_id) OR NOT fb_valid_text(agency_name) OR NOT fb_valid_uuid(bank_id) OR NOT fb_valid_uuid(tax_account_id) OR NOT fb_valid_uuid(expense_account_id) OR NOT fb_valid_date(date) OR NOT fb_valid_date(period_start_date) OR NOT fb_valid_date(period_end_date) OR NOT fb_valid_text(reference) OR NOT fb_valid_text(memo) OR NOT fb_valid_uuid(journal_id) OR NOT fb_valid_uuid(actor_id) OR NOT fb_valid_text(actor_name) OR NOT fb_valid_timestamp(created_at)
UNION ALL SELECT 'salestax_paymentvoid:'||id FROM salestax_paymentvoid WHERE NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(payment_id) OR NOT fb_valid_uuid(journal_id) OR NOT fb_valid_date(date) OR NOT fb_valid_text(reason) OR NOT fb_valid_uuid(actor_id) OR NOT fb_valid_text(actor_name) OR NOT fb_valid_timestamp(created_at);
