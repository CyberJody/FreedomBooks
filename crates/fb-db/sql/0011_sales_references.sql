-- Sales-owned reference protection. INSERT collision checks cover REPLACE even
-- with recursive_triggers disabled; immediate FK constraints reject DELETE/reinsert.
CREATE TRIGGER fb_sales_seal_collision BEFORE INSERT ON fb_seal
WHEN NEW.kind LIKE 'sales_%'
BEGIN SELECT RAISE(ABORT,'sales_seal_immutable: Sales seals cannot be replaced') WHERE EXISTS(SELECT 1 FROM fb_seal WHERE kind=NEW.kind AND doc_id=NEW.doc_id); END;
CREATE TRIGGER fb_sales_due_collision BEFORE INSERT ON fb_seal_due
WHEN NEW.kind LIKE 'sales_%'
BEGIN SELECT RAISE(ABORT,'sales_seal_immutable: Sales obligations cannot be replaced') WHERE EXISTS(SELECT 1 FROM fb_seal_due WHERE kind=NEW.kind AND doc_id=NEW.doc_id); END;
CREATE TRIGGER fb_sales_directory_customer_collision BEFORE INSERT ON directory_customer
BEGIN SELECT RAISE(ABORT,'sales_reference_protected: Referenced sales evidence cannot be replaced') WHERE EXISTS(SELECT 1 FROM directory_customer o WHERE (o.id=NEW.id) AND (EXISTS(SELECT 1 FROM sales_invoicerevision WHERE customer_id=o.id) OR EXISTS(SELECT 1 FROM sales_receipt WHERE customer_id=o.id))); END;
CREATE TRIGGER fb_sales_directory_customer_company BEFORE UPDATE ON directory_customer
BEGIN SELECT RAISE(ABORT,'sales_reference_protected: Referenced sales records cannot change company') WHERE NEW.id IS NOT OLD.id OR (NEW.company_id IS NOT OLD.company_id AND EXISTS(SELECT 1 FROM directory_customer o WHERE o.id=OLD.id AND (EXISTS(SELECT 1 FROM sales_invoicerevision WHERE customer_id=o.id) OR EXISTS(SELECT 1 FROM sales_receipt WHERE customer_id=o.id)))); END;
CREATE TRIGGER fb_sales_directory_customer_kept BEFORE DELETE ON directory_customer
BEGIN SELECT RAISE(ABORT,'sales_reference_protected: Referenced sales evidence cannot be deleted') WHERE EXISTS(SELECT 1 FROM directory_customer o WHERE o.id=OLD.id AND (EXISTS(SELECT 1 FROM sales_invoicerevision WHERE customer_id=o.id) OR EXISTS(SELECT 1 FROM sales_receipt WHERE customer_id=o.id))); END;
CREATE TRIGGER fb_sales_directory_item_collision BEFORE INSERT ON directory_item
BEGIN SELECT RAISE(ABORT,'sales_reference_protected: Referenced sales evidence cannot be replaced') WHERE EXISTS(SELECT 1 FROM directory_item o WHERE (o.id=NEW.id OR (o.company_id=NEW.company_id AND fb_lower(o.code)=fb_lower(NEW.code))) AND (EXISTS(SELECT 1 FROM sales_invoiceline WHERE item_id=o.id) OR EXISTS(SELECT 1 FROM sales_salesreceiptline WHERE item_id=o.id))); END;
CREATE TRIGGER fb_sales_directory_item_company BEFORE UPDATE ON directory_item
BEGIN SELECT RAISE(ABORT,'sales_reference_protected: Referenced sales records cannot change company') WHERE NEW.id IS NOT OLD.id OR (NEW.company_id IS NOT OLD.company_id AND EXISTS(SELECT 1 FROM directory_item o WHERE o.id=OLD.id AND (EXISTS(SELECT 1 FROM sales_invoiceline WHERE item_id=o.id) OR EXISTS(SELECT 1 FROM sales_salesreceiptline WHERE item_id=o.id)))); END;
CREATE TRIGGER fb_sales_directory_item_kept BEFORE DELETE ON directory_item
BEGIN SELECT RAISE(ABORT,'sales_reference_protected: Referenced sales evidence cannot be deleted') WHERE EXISTS(SELECT 1 FROM directory_item o WHERE o.id=OLD.id AND (EXISTS(SELECT 1 FROM sales_invoiceline WHERE item_id=o.id) OR EXISTS(SELECT 1 FROM sales_salesreceiptline WHERE item_id=o.id))); END;
CREATE TRIGGER fb_sales_ledger_account_collision BEFORE INSERT ON ledger_account
BEGIN SELECT RAISE(ABORT,'sales_reference_protected: Referenced sales evidence cannot be replaced') WHERE EXISTS(SELECT 1 FROM ledger_account o WHERE (o.id=NEW.id OR (o.company_id=NEW.company_id AND o.code=NEW.code)) AND (EXISTS(SELECT 1 FROM sales_invoiceline WHERE o.id IN(income_account_id,expense_account_id)) OR EXISTS(SELECT 1 FROM sales_receipt WHERE destination_account_id=o.id) OR EXISTS(SELECT 1 FROM sales_receiptallocation WHERE receivables_id=o.id) OR EXISTS(SELECT 1 FROM sales_salesreceiptline WHERE income_account_id=o.id))); END;
CREATE TRIGGER fb_sales_ledger_account_company BEFORE UPDATE ON ledger_account
BEGIN SELECT RAISE(ABORT,'sales_reference_protected: Referenced sales records cannot change company') WHERE NEW.id IS NOT OLD.id OR (NEW.company_id IS NOT OLD.company_id AND EXISTS(SELECT 1 FROM ledger_account o WHERE o.id=OLD.id AND (EXISTS(SELECT 1 FROM sales_invoiceline WHERE o.id IN(income_account_id,expense_account_id)) OR EXISTS(SELECT 1 FROM sales_receipt WHERE destination_account_id=o.id) OR EXISTS(SELECT 1 FROM sales_receiptallocation WHERE receivables_id=o.id) OR EXISTS(SELECT 1 FROM sales_salesreceiptline WHERE income_account_id=o.id)))); END;
CREATE TRIGGER fb_sales_ledger_account_kept BEFORE DELETE ON ledger_account
BEGIN SELECT RAISE(ABORT,'sales_reference_protected: Referenced sales evidence cannot be deleted') WHERE EXISTS(SELECT 1 FROM ledger_account o WHERE o.id=OLD.id AND (EXISTS(SELECT 1 FROM sales_invoiceline WHERE o.id IN(income_account_id,expense_account_id)) OR EXISTS(SELECT 1 FROM sales_receipt WHERE destination_account_id=o.id) OR EXISTS(SELECT 1 FROM sales_receiptallocation WHERE receivables_id=o.id) OR EXISTS(SELECT 1 FROM sales_salesreceiptline WHERE income_account_id=o.id))); END;
CREATE TRIGGER fb_sales_documents_documentidentity_collision BEFORE INSERT ON documents_documentidentity
BEGIN SELECT RAISE(ABORT,'sales_reference_protected: Referenced sales evidence cannot be replaced') WHERE EXISTS(SELECT 1 FROM documents_documentidentity o WHERE (o.id=NEW.id OR (o.company_id=NEW.company_id AND (o.request_key=NEW.request_key OR (o.kind=NEW.kind AND o.ordinal=NEW.ordinal) OR o.number=NEW.number))) AND (EXISTS(SELECT 1 FROM sales_invoice WHERE identity_id=o.id) OR EXISTS(SELECT 1 FROM sales_receipt WHERE identity_id=o.id))); END;
CREATE TRIGGER fb_sales_documents_documentidentity_kept BEFORE DELETE ON documents_documentidentity
BEGIN SELECT RAISE(ABORT,'sales_reference_protected: Referenced sales evidence cannot be deleted') WHERE EXISTS(SELECT 1 FROM documents_documentidentity o WHERE o.id=OLD.id AND (EXISTS(SELECT 1 FROM sales_invoice WHERE identity_id=o.id) OR EXISTS(SELECT 1 FROM sales_receipt WHERE identity_id=o.id))); END;
CREATE TRIGGER fb_sales_ledger_journal_collision BEFORE INSERT ON ledger_journal
BEGIN SELECT RAISE(ABORT,'sales_reference_protected: Referenced sales evidence cannot be replaced') WHERE EXISTS(SELECT 1 FROM ledger_journal o WHERE (o.id=NEW.id OR o.number=NEW.number OR (o.company_id=NEW.company_id AND o.entry_number=NEW.entry_number AND o.entry_number<>'') OR (o.source_id=NEW.source_id AND o.reversal_of_id IS NULL AND NEW.reversal_of_id IS NULL) OR o.reversal_of_id=NEW.reversal_of_id) AND (EXISTS(SELECT 1 FROM sales_invoicerevision WHERE o.id IN(journal_id,reversal_id)) OR EXISTS(SELECT 1 FROM sales_receipt WHERE journal_id=o.id) OR EXISTS(SELECT 1 FROM sales_receiptreversal WHERE journal_id=o.id))); END;
CREATE TRIGGER fb_sales_ledger_journal_kept BEFORE DELETE ON ledger_journal
BEGIN SELECT RAISE(ABORT,'sales_reference_protected: Referenced sales evidence cannot be deleted') WHERE EXISTS(SELECT 1 FROM ledger_journal o WHERE o.id=OLD.id AND (EXISTS(SELECT 1 FROM sales_invoicerevision WHERE o.id IN(journal_id,reversal_id)) OR EXISTS(SELECT 1 FROM sales_receipt WHERE journal_id=o.id) OR EXISTS(SELECT 1 FROM sales_receiptreversal WHERE journal_id=o.id))); END;
CREATE TRIGGER fb_sales_documents_documentaccount_collision BEFORE INSERT ON documents_documentaccount
BEGIN SELECT RAISE(ABORT,'sales_reference_protected: Referenced sales evidence cannot be replaced') WHERE EXISTS(SELECT 1 FROM documents_documentaccount o WHERE (o.id=NEW.id OR (o.source_id=NEW.source_id AND o.role=NEW.role)) AND (EXISTS(SELECT 1 FROM sales_invoice WHERE identity_id=o.source_id) OR EXISTS(SELECT 1 FROM sales_receipt WHERE identity_id=o.source_id))); END;
CREATE TRIGGER fb_sales_documents_documentaccount_kept BEFORE DELETE ON documents_documentaccount
BEGIN SELECT RAISE(ABORT,'sales_reference_protected: Referenced sales evidence cannot be deleted') WHERE EXISTS(SELECT 1 FROM documents_documentaccount o WHERE o.id=OLD.id AND (EXISTS(SELECT 1 FROM sales_invoice WHERE identity_id=o.source_id) OR EXISTS(SELECT 1 FROM sales_receipt WHERE identity_id=o.source_id))); END;
CREATE TRIGGER fb_sales_ledger_journalline_collision BEFORE INSERT ON ledger_journalline
BEGIN SELECT RAISE(ABORT,'sales_reference_protected: Referenced sales evidence cannot be replaced') WHERE EXISTS(SELECT 1 FROM ledger_journalline o WHERE (o.id=NEW.id OR (o.journal_id=NEW.journal_id AND o.position=NEW.position)) AND (EXISTS(SELECT 1 FROM sales_invoicerevision WHERE o.journal_id IN(journal_id,reversal_id)) OR EXISTS(SELECT 1 FROM sales_receipt WHERE journal_id=o.journal_id) OR EXISTS(SELECT 1 FROM sales_receiptreversal WHERE journal_id=o.journal_id))); END;
CREATE TRIGGER fb_sales_ledger_journalline_kept BEFORE DELETE ON ledger_journalline
BEGIN SELECT RAISE(ABORT,'sales_reference_protected: Referenced sales evidence cannot be deleted') WHERE EXISTS(SELECT 1 FROM ledger_journalline o WHERE o.id=OLD.id AND (EXISTS(SELECT 1 FROM sales_invoicerevision WHERE o.journal_id IN(journal_id,reversal_id)) OR EXISTS(SELECT 1 FROM sales_receipt WHERE journal_id=o.journal_id) OR EXISTS(SELECT 1 FROM sales_receiptreversal WHERE journal_id=o.journal_id))); END;

CREATE VIEW sales_tax_scope_violations AS
 WITH headers AS (
 SELECT i.company_id,r.data FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id
 UNION ALL SELECT r.company_id,s.data FROM sales_receipt r JOIN sales_salesreceipt s ON s.receipt_id=r.id)
 SELECT h.company_id FROM headers h WHERE json_extract(h.data,'$.tax_mode') IN('RATE','GROUP') AND (
 NOT EXISTS(SELECT 1 FROM salestax_taxitem t WHERE t.id=json_extract(h.data,'$.tax_item_id') AND t.company_id=h.company_id AND t.kind=json_extract(h.data,'$.tax_mode')) OR
 EXISTS(SELECT 1 FROM json_each(h.data,'$.tax_snapshot.members') m WHERE NOT EXISTS(SELECT 1 FROM salestax_taxitem t WHERE t.id=json_extract(m.value,'$.id') AND t.company_id=h.company_id AND t.kind='RATE')) OR
 (json_extract(h.data,'$.tax_mode')='RATE' AND json_extract(h.data,'$.tax_item_id') IS NOT json_extract(h.data,'$.tax_snapshot.members[0].id')));
CREATE TRIGGER fb_sales_tax_revision_scope BEFORE INSERT ON fb_seal WHEN NEW.kind='sales_invoice_revision'
BEGIN SELECT RAISE(ABORT,'sales_tax_scope: Sales tax snapshot must refer to this company') WHERE EXISTS(SELECT 1 FROM sales_invoicerevision r JOIN sales_invoice i ON i.id=r.invoice_id WHERE r.id=NEW.doc_id AND EXISTS(SELECT 1 FROM sales_tax_scope_violations v WHERE v.company_id=i.company_id)); END;
CREATE TRIGGER fb_sales_tax_receipt_scope BEFORE INSERT ON fb_seal WHEN NEW.kind='sales_receipt'
BEGIN SELECT RAISE(ABORT,'sales_tax_scope: Sales tax snapshot must refer to this company') WHERE EXISTS(SELECT 1 FROM sales_receipt r WHERE r.id=NEW.doc_id AND EXISTS(SELECT 1 FROM sales_tax_scope_violations v WHERE v.company_id=r.company_id)); END;
CREATE TRIGGER fb_sales_group_members BEFORE INSERT ON sales_salesreceipt
WHEN json_extract(NEW.data,'$.tax_mode')='GROUP'
BEGIN
 SELECT RAISE(ABORT,'sales_tax_members: Sales receipt tax group members must match the selected group') WHERE
 (SELECT count(*) FROM json_each(NEW.data,'$.tax_snapshot.members'))<>(SELECT count(*) FROM salestax_taxgroupmember WHERE group_id=json_extract(NEW.data,'$.tax_item_id')) OR
 (SELECT count(DISTINCT json_extract(value,'$.id')) FROM json_each(NEW.data,'$.tax_snapshot.members'))<>(SELECT count(*) FROM json_each(NEW.data,'$.tax_snapshot.members')) OR
 EXISTS(SELECT 1 FROM json_each(NEW.data,'$.tax_snapshot.members') m WHERE NOT EXISTS(SELECT 1 FROM salestax_taxgroupmember g WHERE g.group_id=json_extract(NEW.data,'$.tax_item_id') AND g.member_id=json_extract(m.value,'$.id')));
END;
CREATE TRIGGER fb_sales_post_eligible BEFORE INSERT ON sales_invoicerevision WHEN NEW.state='OPEN'
BEGIN
 SELECT RAISE(ABORT,'sales_posting_eligible: Posting requires an active customer and items') WHERE NOT EXISTS(SELECT 1 FROM directory_customer WHERE id=NEW.customer_id AND active=1) OR EXISTS(SELECT 1 FROM sales_invoicerevision p JOIN sales_invoiceline l ON l.revision_id=p.id JOIN directory_item t ON t.id=l.item_id WHERE p.invoice_id=NEW.invoice_id AND p.number=NEW.number-1 AND t.active=0);
END;
CREATE TRIGGER fb_sales_tax_account BEFORE INSERT ON sales_salesreceipt
WHEN NOT EXISTS(SELECT 1 FROM sales_salesreceipt WHERE id=NEW.id OR receipt_id=NEW.receipt_id)
BEGIN
 SELECT RAISE(ABORT,'sales_tax_account: Collected tax requires a liability account in this company') WHERE json_extract(NEW.data,'$.collected_tax_id') IS NOT NULL AND NOT EXISTS(SELECT 1 FROM sales_receipt r JOIN ledger_account a ON a.id=json_extract(NEW.data,'$.collected_tax_id') WHERE r.id=NEW.receipt_id AND a.company_id=r.company_id AND a.kind='LIABILITY' AND a.active=1 AND a.postable=1);
END;
CREATE TRIGGER fb_sales_invoice_no_old_journal BEFORE INSERT ON sales_invoice
WHEN NOT EXISTS(SELECT 1 FROM sales_invoice WHERE id=NEW.id OR identity_id=NEW.identity_id)
BEGIN SELECT RAISE(ABORT,'invoice_identity_scope: Invalid invoice identity') WHERE EXISTS(SELECT 1 FROM ledger_journal WHERE source_id=NEW.identity_id); END;

CREATE TRIGGER fb_sales_invoice_tax_account BEFORE INSERT ON fb_seal WHEN NEW.kind='sales_invoice_revision'
BEGIN SELECT RAISE(ABORT,'sales_tax_account: Collected tax requires a liability account in this company') WHERE EXISTS(SELECT 1 FROM sales_invoicerevision r JOIN sales_invoice i ON i.id=r.invoice_id WHERE r.id=NEW.doc_id AND r.state IN('OPEN','VOID') AND fb_sales_money(json_quote(json_extract(r.data,'$.tax')))>0 AND NOT EXISTS(SELECT 1 FROM documents_documentaccount d JOIN ledger_account a ON a.id=d.account_id WHERE d.source_id=i.identity_id AND d.role='collected_tax' AND a.kind='LIABILITY' AND a.company_id=i.company_id)); END;
CREATE TRIGGER fb_sales_post_seal_eligible BEFORE INSERT ON fb_seal WHEN NEW.kind='sales_invoice_revision'
BEGIN SELECT RAISE(ABORT,'sales_posting_eligible: Posting requires an active customer and items') WHERE EXISTS(SELECT 1 FROM sales_invoicerevision r WHERE r.id=NEW.doc_id AND r.state='OPEN' AND (NOT EXISTS(SELECT 1 FROM directory_customer WHERE id=r.customer_id AND active=1) OR EXISTS(SELECT 1 FROM sales_invoiceline l JOIN directory_item t ON t.id=l.item_id WHERE l.revision_id=r.id AND t.active=0))); END;

CREATE TRIGGER fb_sales_head_seal BEFORE INSERT ON fb_seal WHEN NEW.kind='sales_invoice_head'
BEGIN SELECT RAISE(ABORT,'invoice_complete: Invoice requires real sealed revision') WHERE NOT EXISTS(SELECT 1 FROM sales_invoicerevision r JOIN fb_seal s ON s.kind='sales_invoice_revision' AND s.doc_id=r.id WHERE r.invoice_id||':'||r.number=NEW.doc_id); END;
CREATE TRIGGER fb_sales_revision_shape BEFORE INSERT ON sales_invoicerevision
BEGIN SELECT RAISE(ABORT,'invoice_revision_fields: Invalid invoice revision fields') WHERE NOT fb_sales_header_ok('invoice',NEW.data); END;
CREATE TRIGGER fb_sales_receipt_shape BEFORE INSERT ON sales_receipt
BEGIN SELECT RAISE(ABORT,'receipt_fields: Invalid receipt fields') WHERE NOT fb_sales_header_ok('receipt',NEW.data); END;
