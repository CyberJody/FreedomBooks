-- Review FIX1. Configuration eligibility is a creation rule, not an
-- open-time predicate: historical evidence survives setup/designation changes.
CREATE TRIGGER pp_receipt_destination BEFORE INSERT ON sales_receipt WHEN NEW.kind='CDP'
BEGIN
 SELECT RAISE(ABORT,'receipt_destination: Choose a configured receipt destination')
 WHERE NOT EXISTS(SELECT 1 FROM documents_accountingsetup s JOIN ledger_account a ON a.id=NEW.destination_account_id
 WHERE s.company_id=NEW.company_id AND a.company_id=s.company_id AND a.kind='ASSET' AND a.active=1 AND a.postable=1
 AND ((NEW.destination='UNDEPOSITED' AND s.undeposited_id=a.id)
 OR (NEW.destination='BANK' AND EXISTS(SELECT 1 FROM documents_designatedbank b WHERE b.setup_id=s.id AND b.account_id=a.id)
 AND NOT EXISTS(SELECT 1 FROM fb_control_accounts c WHERE c.company_id=s.company_id AND c.account_id=a.id))));
END;

CREATE TRIGGER pp_deposit_evidence BEFORE INSERT ON sales_receipt WHEN NEW.kind='CDP'
BEGIN
 SELECT RAISE(ABORT,'prepayment_fields: Invalid deposit evidence') WHERE NOT fb_pp_snapshot_ok('deposit',NEW.data)
 OR json_extract(NEW.data,'$.id') IS NOT NEW.id
 OR json_extract(NEW.data,'$.company_id') IS NOT NEW.company_id
 OR json_extract(NEW.data,'$.identity_id') IS NOT NEW.identity_id
 OR json_extract(NEW.data,'$.kind') IS NOT NEW.kind
 OR json_extract(NEW.data,'$.customer_id') IS NOT NEW.customer_id
 OR json_extract(NEW.data,'$.date') IS NOT NEW.date
 OR json_extract(NEW.data,'$.destination') IS NOT NEW.destination
 OR json_extract(NEW.data,'$.destination_account_id') IS NOT NEW.destination_account_id
 OR json_extract(NEW.data,'$.journal_id') IS NOT NEW.journal_id
 OR json_extract(NEW.data,'$.actor_id') IS NOT NEW.actor_id
 OR fb_sales_money(json_quote(json_extract(NEW.data,'$.amount'))) IS NOT NEW.amount
 OR NOT EXISTS(SELECT 1 FROM fb_user u WHERE u.id=NEW.actor_id);
END;

CREATE TRIGGER pp_application_evidence BEFORE INSERT ON prepayments_depositapplication WHEN 1
BEGIN
 SELECT RAISE(ABORT,'prepayment_fields: Invalid application evidence') WHERE NOT fb_pp_snapshot_ok('application',NEW.data)
 OR json_extract(NEW.data,'$.id') IS NOT NEW.id
 OR json_extract(NEW.data,'$.company_id') IS NOT NEW.company_id
 OR json_extract(NEW.data,'$.identity_id') IS NOT NEW.identity_id
 OR json_extract(NEW.data,'$.deposit_id') IS NOT NEW.deposit_id
 OR json_extract(NEW.data,'$.invoice_id') IS NOT NEW.invoice_id
 OR json_extract(NEW.data,'$.invoice_revision_id') IS NOT NEW.invoice_revision_id
 OR json_extract(NEW.data,'$.receivables_id') IS NOT NEW.receivables_id
 OR json_extract(NEW.data,'$.liability_id') IS NOT NEW.liability_id
 OR json_extract(NEW.data,'$.date') IS NOT NEW.date
 OR json_extract(NEW.data,'$.journal_id') IS NOT NEW.journal_id
 OR json_extract(NEW.data,'$.actor_id') IS NOT NEW.actor_id
 OR fb_sales_money(json_quote(json_extract(NEW.data,'$.amount'))) IS NOT NEW.amount
 OR NOT EXISTS(SELECT 1 FROM fb_user u WHERE u.id=NEW.actor_id);
END;

CREATE TRIGGER pp_refund_evidence BEFORE INSERT ON prepayments_depositrefund WHEN 1
BEGIN
 SELECT RAISE(ABORT,'prepayment_fields: Invalid refund evidence') WHERE NOT fb_pp_snapshot_ok('refund',NEW.data)
 OR json_extract(NEW.data,'$.id') IS NOT NEW.id
 OR json_extract(NEW.data,'$.company_id') IS NOT NEW.company_id
 OR json_extract(NEW.data,'$.identity_id') IS NOT NEW.identity_id
 OR json_extract(NEW.data,'$.deposit_id') IS NOT NEW.deposit_id
 OR json_extract(NEW.data,'$.customer_id') IS NOT NEW.customer_id
 OR json_extract(NEW.data,'$.bank_id') IS NOT NEW.bank_id
 OR json_extract(NEW.data,'$.liability_id') IS NOT NEW.liability_id
 OR json_extract(NEW.data,'$.date') IS NOT NEW.date
 OR json_extract(NEW.data,'$.method') IS NOT NEW.method
 OR json_extract(NEW.data,'$.check_number') IS NOT NEW.check_number
 OR json_extract(NEW.data,'$.journal_id') IS NOT NEW.journal_id
 OR json_extract(NEW.data,'$.actor_id') IS NOT NEW.actor_id
 OR fb_sales_money(json_quote(json_extract(NEW.data,'$.amount'))) IS NOT NEW.amount
 OR NOT EXISTS(SELECT 1 FROM fb_user u WHERE u.id=NEW.actor_id);
END;

CREATE TRIGGER pp_unapplication_evidence BEFORE INSERT ON prepayments_depositunapplication WHEN 1
BEGIN
 SELECT RAISE(ABORT,'prepayment_fields: Invalid unapplication evidence') WHERE NOT fb_pp_snapshot_ok('unapplication',NEW.data)
 OR json_extract(NEW.data,'$.id') IS NOT NEW.id
 OR json_extract(NEW.data,'$.application_id') IS NOT NEW.application_id
 OR json_extract(NEW.data,'$.journal_id') IS NOT NEW.journal_id
 OR json_extract(NEW.data,'$.date') IS NOT NEW.date
 OR json_extract(NEW.data,'$.actor_id') IS NOT NEW.actor_id
 OR NOT EXISTS(SELECT 1 FROM fb_user u WHERE u.id=NEW.actor_id);
END;

CREATE TRIGGER pp_refundvoid_evidence BEFORE INSERT ON prepayments_depositrefundvoid WHEN 1
BEGIN
 SELECT RAISE(ABORT,'prepayment_fields: Invalid refundvoid evidence') WHERE NOT fb_pp_snapshot_ok('refundvoid',NEW.data)
 OR json_extract(NEW.data,'$.id') IS NOT NEW.id
 OR json_extract(NEW.data,'$.refund_id') IS NOT NEW.refund_id
 OR json_extract(NEW.data,'$.journal_id') IS NOT NEW.journal_id
 OR json_extract(NEW.data,'$.date') IS NOT NEW.date
 OR json_extract(NEW.data,'$.actor_id') IS NOT NEW.actor_id
 OR NOT EXISTS(SELECT 1 FROM fb_user u WHERE u.id=NEW.actor_id);
END;

CREATE TRIGGER pp_depositvoid_evidence BEFORE INSERT ON sales_receiptreversal WHEN EXISTS(SELECT 1 FROM sales_receipt p WHERE p.id=NEW.receipt_id AND p.kind='CDP')
BEGIN
 SELECT RAISE(ABORT,'prepayment_fields: Invalid depositvoid evidence') WHERE NOT fb_pp_snapshot_ok('depositvoid',NEW.data)
 OR json_extract(NEW.data,'$.id') IS NOT NEW.id
 OR json_extract(NEW.data,'$.receipt_id') IS NOT NEW.receipt_id
 OR json_extract(NEW.data,'$.journal_id') IS NOT NEW.journal_id
 OR json_extract(NEW.data,'$.date') IS NOT NEW.date
 OR NOT EXISTS(SELECT 1 FROM fb_user u WHERE u.id=NEW.actor_id);
END;

CREATE VIEW prepayments_evidence_violations AS
SELECT 'deposit evidence '||r.id problem FROM sales_receipt r WHERE (r.kind='CDP') AND (NOT fb_pp_snapshot_ok('deposit',r.data)
 OR json_extract(r.data,'$.id') IS NOT r.id
 OR json_extract(r.data,'$.company_id') IS NOT r.company_id
 OR json_extract(r.data,'$.identity_id') IS NOT r.identity_id
 OR json_extract(r.data,'$.kind') IS NOT r.kind
 OR json_extract(r.data,'$.customer_id') IS NOT r.customer_id
 OR json_extract(r.data,'$.date') IS NOT r.date
 OR json_extract(r.data,'$.destination') IS NOT r.destination
 OR json_extract(r.data,'$.destination_account_id') IS NOT r.destination_account_id
 OR json_extract(r.data,'$.journal_id') IS NOT r.journal_id
 OR json_extract(r.data,'$.actor_id') IS NOT r.actor_id
 OR fb_sales_money(json_quote(json_extract(r.data,'$.amount'))) IS NOT r.amount
 OR NOT EXISTS(SELECT 1 FROM fb_user u WHERE u.id=r.actor_id))
UNION ALL
SELECT 'application evidence '||r.id problem FROM prepayments_depositapplication r WHERE (1) AND (NOT fb_pp_snapshot_ok('application',r.data)
 OR json_extract(r.data,'$.id') IS NOT r.id
 OR json_extract(r.data,'$.company_id') IS NOT r.company_id
 OR json_extract(r.data,'$.identity_id') IS NOT r.identity_id
 OR json_extract(r.data,'$.deposit_id') IS NOT r.deposit_id
 OR json_extract(r.data,'$.invoice_id') IS NOT r.invoice_id
 OR json_extract(r.data,'$.invoice_revision_id') IS NOT r.invoice_revision_id
 OR json_extract(r.data,'$.receivables_id') IS NOT r.receivables_id
 OR json_extract(r.data,'$.liability_id') IS NOT r.liability_id
 OR json_extract(r.data,'$.date') IS NOT r.date
 OR json_extract(r.data,'$.journal_id') IS NOT r.journal_id
 OR json_extract(r.data,'$.actor_id') IS NOT r.actor_id
 OR fb_sales_money(json_quote(json_extract(r.data,'$.amount'))) IS NOT r.amount
 OR NOT EXISTS(SELECT 1 FROM fb_user u WHERE u.id=r.actor_id))
UNION ALL
SELECT 'refund evidence '||r.id problem FROM prepayments_depositrefund r WHERE (1) AND (NOT fb_pp_snapshot_ok('refund',r.data)
 OR json_extract(r.data,'$.id') IS NOT r.id
 OR json_extract(r.data,'$.company_id') IS NOT r.company_id
 OR json_extract(r.data,'$.identity_id') IS NOT r.identity_id
 OR json_extract(r.data,'$.deposit_id') IS NOT r.deposit_id
 OR json_extract(r.data,'$.customer_id') IS NOT r.customer_id
 OR json_extract(r.data,'$.bank_id') IS NOT r.bank_id
 OR json_extract(r.data,'$.liability_id') IS NOT r.liability_id
 OR json_extract(r.data,'$.date') IS NOT r.date
 OR json_extract(r.data,'$.method') IS NOT r.method
 OR json_extract(r.data,'$.check_number') IS NOT r.check_number
 OR json_extract(r.data,'$.journal_id') IS NOT r.journal_id
 OR json_extract(r.data,'$.actor_id') IS NOT r.actor_id
 OR fb_sales_money(json_quote(json_extract(r.data,'$.amount'))) IS NOT r.amount
 OR NOT EXISTS(SELECT 1 FROM fb_user u WHERE u.id=r.actor_id))
UNION ALL
SELECT 'unapplication evidence '||r.id problem FROM prepayments_depositunapplication r WHERE (1) AND (NOT fb_pp_snapshot_ok('unapplication',r.data)
 OR json_extract(r.data,'$.id') IS NOT r.id
 OR json_extract(r.data,'$.application_id') IS NOT r.application_id
 OR json_extract(r.data,'$.journal_id') IS NOT r.journal_id
 OR json_extract(r.data,'$.date') IS NOT r.date
 OR json_extract(r.data,'$.actor_id') IS NOT r.actor_id
 OR NOT EXISTS(SELECT 1 FROM fb_user u WHERE u.id=r.actor_id))
UNION ALL
SELECT 'refundvoid evidence '||r.id problem FROM prepayments_depositrefundvoid r WHERE (1) AND (NOT fb_pp_snapshot_ok('refundvoid',r.data)
 OR json_extract(r.data,'$.id') IS NOT r.id
 OR json_extract(r.data,'$.refund_id') IS NOT r.refund_id
 OR json_extract(r.data,'$.journal_id') IS NOT r.journal_id
 OR json_extract(r.data,'$.date') IS NOT r.date
 OR json_extract(r.data,'$.actor_id') IS NOT r.actor_id
 OR NOT EXISTS(SELECT 1 FROM fb_user u WHERE u.id=r.actor_id))
UNION ALL
SELECT 'depositvoid evidence '||r.id problem FROM sales_receiptreversal r WHERE (EXISTS(SELECT 1 FROM sales_receipt p WHERE p.id=r.receipt_id AND p.kind='CDP')) AND (NOT fb_pp_snapshot_ok('depositvoid',r.data)
 OR json_extract(r.data,'$.id') IS NOT r.id
 OR json_extract(r.data,'$.receipt_id') IS NOT r.receipt_id
 OR json_extract(r.data,'$.journal_id') IS NOT r.journal_id
 OR json_extract(r.data,'$.date') IS NOT r.date
 OR NOT EXISTS(SELECT 1 FROM fb_user u WHERE u.id=r.actor_id));

CREATE TRIGGER pp_evidence_seal BEFORE INSERT ON prepayments_seal
BEGIN SELECT RAISE(ABORT,'prepayment_fields: Invalid stored prepayment evidence')
 WHERE EXISTS(SELECT 1 FROM prepayments_evidence_violations); END;
CREATE TRIGGER pp_evidence_sales_seal BEFORE INSERT ON sales_seal
WHEN NEW.kind IN('sales_receipt','sales_receipt_reversal')
BEGIN SELECT RAISE(ABORT,'prepayment_fields: Invalid stored prepayment evidence')
 WHERE EXISTS(SELECT 1 FROM prepayments_evidence_violations); END;

CREATE TRIGGER pp_retain_fb_user BEFORE DELETE ON fb_user
WHEN EXISTS(SELECT 1 FROM prepayments_depositapplication WHERE actor_id=OLD.id) OR
 EXISTS(SELECT 1 FROM prepayments_depositrefund WHERE actor_id=OLD.id) OR
 EXISTS(SELECT 1 FROM prepayments_depositunapplication WHERE actor_id=OLD.id) OR
 EXISTS(SELECT 1 FROM prepayments_depositrefundvoid WHERE actor_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'prepayment_parent_retained: Referenced prepayment parents cannot be deleted'); END;

CREATE TRIGGER pp_retain_ledger_company BEFORE DELETE ON ledger_company
WHEN EXISTS(SELECT 1 FROM prepayments_customerdeposit WHERE company_id=OLD.id) OR
 EXISTS(SELECT 1 FROM prepayments_depositapplication WHERE company_id=OLD.id) OR
 EXISTS(SELECT 1 FROM prepayments_depositrefund WHERE company_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'prepayment_parent_retained: Referenced prepayment parents cannot be deleted'); END;

CREATE TRIGGER pp_retain_ledger_account BEFORE DELETE ON ledger_account
WHEN EXISTS(SELECT 1 FROM prepayments_customerdeposit WHERE liability_id=OLD.id) OR
 EXISTS(SELECT 1 FROM prepayments_depositapplication WHERE receivables_id=OLD.id) OR
 EXISTS(SELECT 1 FROM prepayments_depositapplication WHERE liability_id=OLD.id) OR
 EXISTS(SELECT 1 FROM prepayments_depositrefund WHERE bank_id=OLD.id) OR
 EXISTS(SELECT 1 FROM prepayments_depositrefund WHERE liability_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'prepayment_parent_retained: Referenced prepayment parents cannot be deleted'); END;

CREATE TRIGGER pp_retain_documents_documentidentity BEFORE DELETE ON documents_documentidentity
WHEN EXISTS(SELECT 1 FROM prepayments_depositapplication WHERE identity_id=OLD.id) OR
 EXISTS(SELECT 1 FROM prepayments_depositrefund WHERE identity_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'prepayment_parent_retained: Referenced prepayment parents cannot be deleted'); END;

CREATE TRIGGER pp_retain_directory_customer BEFORE DELETE ON directory_customer
WHEN EXISTS(SELECT 1 FROM prepayments_depositrefund WHERE customer_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'prepayment_parent_retained: Referenced prepayment parents cannot be deleted'); END;

CREATE TRIGGER pp_retain_sales_receipt BEFORE DELETE ON sales_receipt
WHEN EXISTS(SELECT 1 FROM prepayments_customerdeposit WHERE receipt_id=OLD.id) OR
 EXISTS(SELECT 1 FROM prepayments_depositapplication WHERE deposit_id=OLD.id) OR
 EXISTS(SELECT 1 FROM prepayments_depositrefund WHERE deposit_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'prepayment_parent_retained: Referenced prepayment parents cannot be deleted'); END;

CREATE TRIGGER pp_retain_sales_invoice BEFORE DELETE ON sales_invoice
WHEN EXISTS(SELECT 1 FROM prepayments_depositapplication WHERE invoice_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'prepayment_parent_retained: Referenced prepayment parents cannot be deleted'); END;

CREATE TRIGGER pp_retain_sales_invoicerevision BEFORE DELETE ON sales_invoicerevision
WHEN EXISTS(SELECT 1 FROM prepayments_depositapplication WHERE invoice_revision_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'prepayment_parent_retained: Referenced prepayment parents cannot be deleted'); END;

CREATE TRIGGER pp_retain_ledger_journal BEFORE DELETE ON ledger_journal
WHEN EXISTS(SELECT 1 FROM prepayments_depositapplication WHERE journal_id=OLD.id) OR
 EXISTS(SELECT 1 FROM prepayments_depositrefund WHERE journal_id=OLD.id) OR
 EXISTS(SELECT 1 FROM prepayments_depositunapplication WHERE journal_id=OLD.id) OR
 EXISTS(SELECT 1 FROM prepayments_depositrefundvoid WHERE journal_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'prepayment_parent_retained: Referenced prepayment parents cannot be deleted'); END;

CREATE TRIGGER pp_retain_prepayments_depositapplication BEFORE DELETE ON prepayments_depositapplication
WHEN EXISTS(SELECT 1 FROM prepayments_depositunapplication WHERE application_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'prepayment_parent_retained: Referenced prepayment parents cannot be deleted'); END;

CREATE TRIGGER pp_retain_prepayments_depositrefund BEFORE DELETE ON prepayments_depositrefund
WHEN EXISTS(SELECT 1 FROM prepayments_depositrefundvoid WHERE refund_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'prepayment_parent_retained: Referenced prepayment parents cannot be deleted'); END;
