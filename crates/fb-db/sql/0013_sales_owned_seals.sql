-- Isolate exact-money callbacks from L1/L2 seal writes. SQLite resolves functions
-- in every trigger on a table even when its WHEN clause would be false. Sales
-- therefore owns its seal/obligation tables; the same deferred-FK protocol holds.
CREATE TABLE sales_seal(kind TEXT NOT NULL CHECK(kind IN('sales_invoice_head','sales_invoice_revision','sales_receipt','sales_receipt_reversal','sales_journal')),doc_id TEXT NOT NULL,PRIMARY KEY(kind,doc_id)) WITHOUT ROWID, STRICT;
CREATE TABLE sales_seal_due(kind TEXT NOT NULL,doc_id TEXT NOT NULL,PRIMARY KEY(kind,doc_id),FOREIGN KEY(kind,doc_id) REFERENCES sales_seal(kind,doc_id) DEFERRABLE INITIALLY DEFERRED) WITHOUT ROWID, STRICT;
-- Upgrade the initial L3 checkpoint without rewriting or deleting its old seals.
INSERT INTO sales_seal SELECT kind,doc_id FROM fb_seal WHERE kind LIKE 'sales_%';
INSERT INTO sales_seal_due SELECT kind,doc_id FROM fb_seal_due WHERE kind LIKE 'sales_%';
DROP TRIGGER fb_sales_invoice_update;
CREATE TRIGGER fb_sales_invoice_update BEFORE UPDATE ON sales_invoice
BEGIN
 SELECT RAISE(ABORT,'invoice_immutable_ownership: Invoice ownership is immutable; revisions advance by one') WHERE NEW.id IS NOT OLD.id OR NEW.company_id IS NOT OLD.company_id OR NEW.identity_id IS NOT OLD.identity_id OR NEW.creation_digest IS NOT OLD.creation_digest OR NEW.revision<>OLD.revision+1 OR NOT EXISTS(SELECT 1 FROM sales_invoicerevision r JOIN sales_seal s ON s.kind='sales_invoice_revision' AND s.doc_id=r.id WHERE r.invoice_id=OLD.id AND r.number=OLD.revision);
END;
DROP TRIGGER fb_sales_invoice_due_insert;
CREATE TRIGGER fb_sales_invoice_due_insert AFTER INSERT ON sales_invoice
BEGIN INSERT INTO sales_seal_due(kind,doc_id) VALUES('sales_invoice_head',NEW.id||':'||NEW.revision); END;
DROP TRIGGER fb_sales_invoice_due_update;
CREATE TRIGGER fb_sales_invoice_due_update AFTER UPDATE ON sales_invoice
BEGIN INSERT INTO sales_seal_due(kind,doc_id) VALUES('sales_invoice_head',NEW.id||':'||NEW.revision); END;
DROP TRIGGER fb_sales_revision_due;
CREATE TRIGGER fb_sales_revision_due AFTER INSERT ON sales_invoicerevision
BEGIN INSERT INTO sales_seal_due(kind,doc_id) VALUES('sales_invoice_revision',NEW.id); END;
DROP TRIGGER fb_sales_invoice_line_insert;
CREATE TRIGGER fb_sales_invoice_line_insert BEFORE INSERT ON sales_invoiceline
BEGIN
 SELECT RAISE(ABORT,'invoice_history_immutable: Invoice lines are immutable') WHERE EXISTS(SELECT 1 FROM sales_invoiceline WHERE id=NEW.id OR (revision_id=NEW.revision_id AND position=NEW.position));
 SELECT RAISE(ABORT,'invoice_line_scope: Invalid invoice line scope or committed history') WHERE NOT EXISTS(SELECT 1 FROM sales_invoicerevision r JOIN sales_invoice i ON i.id=r.invoice_id WHERE r.id=NEW.revision_id AND r.number=i.revision AND NOT EXISTS(SELECT 1 FROM sales_seal s WHERE s.kind='sales_invoice_revision' AND s.doc_id=r.id) AND (NEW.item_id IS NULL OR EXISTS(SELECT 1 FROM directory_item WHERE id=NEW.item_id AND company_id=i.company_id)) AND (NEW.income_account_id IS NULL OR EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.income_account_id AND company_id=i.company_id)) AND (NEW.expense_account_id IS NULL OR EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.expense_account_id AND company_id=i.company_id)));
 SELECT RAISE(ABORT,'invoice_line_calculation: Invalid invoice line calculation') WHERE NOT fb_sales_line_ok(NEW.data) OR json_extract(NEW.data,'$.position') IS NOT NEW.position OR json_extract(NEW.data,'$.item_id') IS NOT NEW.item_id OR json_extract(NEW.data,'$.income_account_id') IS NOT NEW.income_account_id OR json_extract(NEW.data,'$.expense_account_id') IS NOT NEW.expense_account_id OR json_extract(NEW.data,'$.taxable') IS NOT NEW.taxable OR fb_sales_money(json_quote(json_extract(NEW.data,'$.amount'))) IS NOT NEW.amount;
END;
DROP TRIGGER fb_sales_receipt_due;
CREATE TRIGGER fb_sales_receipt_due AFTER INSERT ON sales_receipt
BEGIN INSERT INTO sales_seal_due(kind,doc_id) VALUES('sales_receipt',NEW.id); END;
DROP TRIGGER fb_sales_allocation_insert;
CREATE TRIGGER fb_sales_allocation_insert BEFORE INSERT ON sales_receiptallocation
BEGIN
 SELECT RAISE(ABORT,'receipt_immutable: Allocation history is immutable') WHERE EXISTS(SELECT 1 FROM sales_receiptallocation WHERE id=NEW.id OR (receipt_id=NEW.receipt_id AND invoice_id=NEW.invoice_id));
 SELECT RAISE(ABORT,'receipt_late_allocation: Cannot append to committed allocations') WHERE NOT EXISTS(SELECT 1 FROM sales_receipt WHERE id=NEW.receipt_id) OR EXISTS(SELECT 1 FROM sales_seal WHERE kind='sales_receipt' AND doc_id=NEW.receipt_id);
 SELECT RAISE(ABORT,'sales_receipt_no_allocations: Sales receipts carry no invoice allocations') WHERE (SELECT kind FROM sales_receipt WHERE id=NEW.receipt_id) IS NOT 'RCP';
 SELECT RAISE(ABORT,'allocation_scope: Invalid allocation target or original AR') WHERE NOT EXISTS(SELECT 1 FROM sales_receipt r JOIN sales_invoice i ON i.id=NEW.invoice_id JOIN sales_invoicerevision v ON v.id=NEW.invoice_revision_id WHERE r.id=NEW.receipt_id AND i.company_id=r.company_id AND v.invoice_id=i.id AND v.number=i.revision AND v.customer_id=r.customer_id AND v.state='OPEN' AND v.date<=r.date AND NEW.receivables_id<>r.destination_account_id AND EXISTS(SELECT 1 FROM documents_documentaccount d WHERE d.source_id=i.identity_id AND d.role='receivables' AND d.account_id=NEW.receivables_id));
 SELECT RAISE(ABORT,'allocation_outstanding: Live allocations cannot overpay or target void invoices') WHERE NEW.amount+(SELECT coalesce(sum(a.amount),0) FROM sales_receiptallocation a WHERE a.invoice_id=NEW.invoice_id AND NOT EXISTS(SELECT 1 FROM sales_receiptreversal z WHERE z.receipt_id=a.receipt_id))>(SELECT total FROM sales_invoicerevision WHERE id=NEW.invoice_revision_id);
END;
DROP TRIGGER fb_sales_reversal_due;
CREATE TRIGGER fb_sales_reversal_due AFTER INSERT ON sales_receiptreversal
BEGIN INSERT INTO sales_seal_due(kind,doc_id) VALUES('sales_receipt_reversal',NEW.id); END;
DROP TRIGGER fb_sales_detail_insert;
CREATE TRIGGER fb_sales_detail_insert BEFORE INSERT ON sales_salesreceipt
BEGIN
 SELECT RAISE(ABORT,'receipt_immutable: Sales receipt history is immutable') WHERE EXISTS(SELECT 1 FROM sales_salesreceipt WHERE id=NEW.id OR receipt_id=NEW.receipt_id);
 SELECT RAISE(ABORT,'sales_receipt_scope: Sale detail requires a sales receipt') WHERE (SELECT kind FROM sales_receipt WHERE id=NEW.receipt_id) IS NOT 'SR';
 SELECT RAISE(ABORT,'sales_receipt_late_detail: Cannot append sale detail to a committed receipt') WHERE EXISTS(SELECT 1 FROM sales_seal WHERE kind='sales_receipt' AND doc_id=NEW.receipt_id);
 SELECT RAISE(ABORT,'sales_receipt_fields: Invalid sales receipt detail') WHERE NEW.total IS NOT (SELECT amount FROM sales_receipt WHERE id=NEW.receipt_id) OR fb_sales_money(json_quote(json_extract(NEW.data,'$.total'))) IS NOT NEW.total;
END;
DROP TRIGGER fb_sales_sale_line_insert;
CREATE TRIGGER fb_sales_sale_line_insert BEFORE INSERT ON sales_salesreceiptline
BEGIN
 SELECT RAISE(ABORT,'receipt_immutable: Sales receipt lines are immutable') WHERE EXISTS(SELECT 1 FROM sales_salesreceiptline WHERE id=NEW.id OR (sale_id=NEW.sale_id AND position=NEW.position));
 SELECT RAISE(ABORT,'sales_receipt_late_line: Cannot append lines to a committed sales receipt') WHERE NOT EXISTS(SELECT 1 FROM sales_salesreceipt s JOIN sales_receipt r ON r.id=s.receipt_id WHERE s.id=NEW.sale_id AND NOT EXISTS(SELECT 1 FROM sales_seal z WHERE z.kind='sales_receipt' AND z.doc_id=r.id));
 SELECT RAISE(ABORT,'sales_receipt_line_scope: Invalid sales receipt line') WHERE NOT fb_sales_line_ok(NEW.data) OR json_extract(NEW.data,'$.position') IS NOT NEW.position OR json_extract(NEW.data,'$.item_id') IS NOT NEW.item_id OR json_extract(NEW.data,'$.income_account_id') IS NOT NEW.income_account_id OR json_extract(NEW.data,'$.taxable') IS NOT NEW.taxable OR fb_sales_money(json_quote(json_extract(NEW.data,'$.amount'))) IS NOT NEW.amount OR NOT EXISTS(SELECT 1 FROM sales_salesreceipt s JOIN sales_receipt r ON r.id=s.receipt_id JOIN ledger_account a ON a.id=NEW.income_account_id WHERE s.id=NEW.sale_id AND a.company_id=r.company_id AND a.kind='INCOME' AND a.active=1 AND a.postable=1 AND (NEW.item_id IS NULL OR EXISTS(SELECT 1 FROM directory_item WHERE id=NEW.item_id AND company_id=r.company_id AND active=1)));
END;
DROP TRIGGER fb_sales_invoice_seal;
CREATE TRIGGER fb_sales_invoice_seal BEFORE INSERT ON sales_seal WHEN NEW.kind='sales_invoice_revision'
BEGIN
 SELECT RAISE(ABORT,'invoice_complete: Invoice requires real revision') WHERE NOT EXISTS(SELECT 1 FROM sales_invoicerevision WHERE id=NEW.doc_id);
 SELECT RAISE(ABORT,'invoice_complete: Incomplete or inconsistent invoice totals') WHERE EXISTS(SELECT 1 FROM sales_invoicerevision r WHERE r.id=NEW.doc_id AND NOT fb_sales_document_ok(r.data,coalesce((SELECT json_group_array(json(l.data)) FROM (SELECT data FROM sales_invoiceline WHERE revision_id=r.id ORDER BY position) l),'[]'),r.total));
 SELECT RAISE(ABORT,'invoice_financial_immutable: Posting and void must copy exact lines') WHERE EXISTS(SELECT 1 FROM sales_invoicerevision r JOIN sales_invoicerevision p ON p.invoice_id=r.invoice_id AND p.number=r.number-1 WHERE r.id=NEW.doc_id AND r.state IN('OPEN','VOID') AND (EXISTS(SELECT data FROM sales_invoiceline WHERE revision_id=r.id EXCEPT SELECT data FROM sales_invoiceline WHERE revision_id=p.id) OR EXISTS(SELECT data FROM sales_invoiceline WHERE revision_id=p.id EXCEPT SELECT data FROM sales_invoiceline WHERE revision_id=r.id)));
 SELECT RAISE(ABORT,'invoice_posting_complete: Invoice posting must exactly match its lines and account evidence') WHERE EXISTS(SELECT 1 FROM sales_invoicerevision r JOIN sales_invoice i ON i.id=r.invoice_id WHERE r.id=NEW.doc_id AND r.state IN('OPEN','VOID') AND (EXISTS(SELECT 1 FROM sales_journal_mismatch WHERE id=r.journal_id) OR EXISTS(SELECT 1 FROM sales_reference_mismatch WHERE id=i.identity_id) OR NOT EXISTS(SELECT 1 FROM documents_documentaccount d JOIN ledger_account a ON a.id=d.account_id WHERE d.source_id=i.identity_id AND d.role='receivables' AND a.kind='ASSET') OR EXISTS(SELECT 1 FROM sales_invoiceline l LEFT JOIN ledger_account a ON a.id=l.income_account_id WHERE l.revision_id=r.id AND (a.id IS NULL OR a.kind<>'INCOME'))));
END;
DROP TRIGGER fb_sales_invoice_head_seal;
CREATE TRIGGER fb_sales_invoice_head_seal AFTER INSERT ON sales_seal WHEN NEW.kind='sales_invoice_revision'
BEGIN INSERT INTO sales_seal(kind,doc_id) SELECT 'sales_invoice_head',invoice_id||':'||number FROM sales_invoicerevision WHERE id=NEW.doc_id; END;
DROP TRIGGER fb_sales_receipt_seal;
CREATE TRIGGER fb_sales_receipt_seal BEFORE INSERT ON sales_seal WHEN NEW.kind='sales_receipt'
BEGIN
 SELECT RAISE(ABORT,'receipt_complete: Receipt allocations must equal amount') WHERE NOT EXISTS(SELECT 1 FROM sales_receipt WHERE id=NEW.doc_id) OR EXISTS(SELECT 1 FROM sales_receipt r WHERE r.id=NEW.doc_id AND r.kind='RCP' AND ((SELECT count(*) FROM sales_receiptallocation WHERE receipt_id=r.id) NOT BETWEEN 1 AND 50 OR (SELECT coalesce(sum(amount),0) FROM sales_receiptallocation WHERE receipt_id=r.id)<>r.amount));
 SELECT RAISE(ABORT,'sales_receipt_complete: Sales receipt detail and lines must equal amount') WHERE EXISTS(SELECT 1 FROM sales_receipt r WHERE r.id=NEW.doc_id AND r.kind='SR' AND NOT EXISTS(SELECT 1 FROM sales_salesreceipt s WHERE s.receipt_id=r.id AND s.total=r.amount AND fb_sales_document_ok(s.data,coalesce((SELECT json_group_array(json(l.data)) FROM (SELECT data FROM sales_salesreceiptline WHERE sale_id=s.id ORDER BY position) l),'[]'),s.total) AND ((fb_sales_money(json_quote(json_extract(s.data,'$.tax')))>0)=(json_extract(s.data,'$.collected_tax_id') IS NOT NULL))));
 SELECT RAISE(ABORT,'receipt_journal_split: Receipt journal must exactly match its amount and account evidence') WHERE EXISTS(SELECT 1 FROM sales_receipt r WHERE r.id=NEW.doc_id AND (EXISTS(SELECT 1 FROM sales_journal_mismatch WHERE id=r.journal_id) OR EXISTS(SELECT 1 FROM sales_reference_mismatch WHERE id=r.identity_id)));
END;
DROP TRIGGER fb_sales_reversal_seal;
CREATE TRIGGER fb_sales_reversal_seal BEFORE INSERT ON sales_seal WHEN NEW.kind='sales_receipt_reversal'
BEGIN SELECT RAISE(ABORT,'receipt_reversal_link: Invalid receipt reversal') WHERE NOT EXISTS(SELECT 1 FROM sales_receiptreversal WHERE id=NEW.doc_id); END;
DROP TRIGGER fb_sales_journal_due;
CREATE TRIGGER fb_sales_journal_due AFTER INSERT ON ledger_journal
WHEN (SELECT kind FROM documents_documentidentity WHERE id=NEW.source_id) IN('RCP','SR') OR EXISTS(SELECT 1 FROM sales_invoice WHERE identity_id=NEW.source_id)
BEGIN INSERT INTO sales_seal_due(kind,doc_id) VALUES('sales_journal',NEW.id); END;
DROP TRIGGER fb_sales_journal_seal;
CREATE TRIGGER fb_sales_journal_seal BEFORE INSERT ON sales_seal WHEN NEW.kind='sales_journal'
BEGIN
 SELECT RAISE(ABORT,'sales_journal_owner: Sales journals require complete owning lifecycle') WHERE NOT EXISTS(SELECT 1 FROM ledger_journal j WHERE j.id=NEW.doc_id AND j.status='POSTED' AND (
 (j.reversal_of_id IS NULL AND (EXISTS(SELECT 1 FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id WHERE i.identity_id=j.source_id AND r.journal_id=j.id AND r.state='OPEN' AND EXISTS(SELECT 1 FROM sales_seal WHERE kind='sales_invoice_revision' AND doc_id=r.id)) OR EXISTS(SELECT 1 FROM sales_receipt r WHERE r.identity_id=j.source_id AND r.journal_id=j.id AND EXISTS(SELECT 1 FROM sales_seal WHERE kind='sales_receipt' AND doc_id=r.id)))) OR
 (j.reversal_of_id IS NOT NULL AND (EXISTS(SELECT 1 FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id WHERE i.identity_id=j.source_id AND r.reversal_id=j.id AND r.state='VOID' AND EXISTS(SELECT 1 FROM sales_seal WHERE kind='sales_invoice_revision' AND doc_id=r.id)) OR EXISTS(SELECT 1 FROM sales_receiptreversal z JOIN sales_receipt r ON r.id=z.receipt_id WHERE r.identity_id=j.source_id AND z.journal_id=j.id AND EXISTS(SELECT 1 FROM sales_seal WHERE kind='sales_receipt_reversal' AND doc_id=z.id))))));
END;
DROP TRIGGER fb_sales_reference_insert;
CREATE TRIGGER fb_sales_reference_insert BEFORE INSERT ON documents_documentaccount
BEGIN
 SELECT RAISE(ABORT,'sales_late_reference: Cannot append sales account evidence after sealing') WHERE EXISTS(SELECT 1 FROM sales_receipt r JOIN sales_seal s ON s.kind='sales_receipt' AND s.doc_id=r.id WHERE r.identity_id=NEW.source_id) OR EXISTS(SELECT 1 FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id JOIN sales_seal s ON s.kind='sales_invoice_revision' AND s.doc_id=r.id WHERE i.identity_id=NEW.source_id AND r.state IN('OPEN','VOID'));
END;
DROP TRIGGER fb_sales_seal_collision;
CREATE TRIGGER fb_sales_seal_collision BEFORE INSERT ON sales_seal
WHEN NEW.kind LIKE 'sales_%'
BEGIN SELECT RAISE(ABORT,'sales_seal_immutable: Sales seals cannot be replaced') WHERE EXISTS(SELECT 1 FROM sales_seal WHERE kind=NEW.kind AND doc_id=NEW.doc_id); END;
DROP TRIGGER fb_sales_due_collision;
CREATE TRIGGER fb_sales_due_collision BEFORE INSERT ON sales_seal_due
WHEN NEW.kind LIKE 'sales_%'
BEGIN SELECT RAISE(ABORT,'sales_seal_immutable: Sales obligations cannot be replaced') WHERE EXISTS(SELECT 1 FROM sales_seal_due WHERE kind=NEW.kind AND doc_id=NEW.doc_id); END;
DROP TRIGGER fb_sales_tax_revision_scope;
CREATE TRIGGER fb_sales_tax_revision_scope BEFORE INSERT ON sales_seal WHEN NEW.kind='sales_invoice_revision'
BEGIN SELECT RAISE(ABORT,'sales_tax_scope: Sales tax snapshot must refer to this company') WHERE EXISTS(SELECT 1 FROM sales_invoicerevision r JOIN sales_invoice i ON i.id=r.invoice_id WHERE r.id=NEW.doc_id AND EXISTS(SELECT 1 FROM sales_tax_scope_violations v WHERE v.company_id=i.company_id)); END;
DROP TRIGGER fb_sales_tax_receipt_scope;
CREATE TRIGGER fb_sales_tax_receipt_scope BEFORE INSERT ON sales_seal WHEN NEW.kind='sales_receipt'
BEGIN SELECT RAISE(ABORT,'sales_tax_scope: Sales tax snapshot must refer to this company') WHERE EXISTS(SELECT 1 FROM sales_receipt r WHERE r.id=NEW.doc_id AND EXISTS(SELECT 1 FROM sales_tax_scope_violations v WHERE v.company_id=r.company_id)); END;
DROP TRIGGER fb_sales_invoice_tax_account;
CREATE TRIGGER fb_sales_invoice_tax_account BEFORE INSERT ON sales_seal WHEN NEW.kind='sales_invoice_revision'
BEGIN SELECT RAISE(ABORT,'sales_tax_account: Collected tax requires a liability account in this company') WHERE EXISTS(SELECT 1 FROM sales_invoicerevision r JOIN sales_invoice i ON i.id=r.invoice_id WHERE r.id=NEW.doc_id AND r.state IN('OPEN','VOID') AND fb_sales_money(json_quote(json_extract(r.data,'$.tax')))>0 AND NOT EXISTS(SELECT 1 FROM documents_documentaccount d JOIN ledger_account a ON a.id=d.account_id WHERE d.source_id=i.identity_id AND d.role='collected_tax' AND a.kind='LIABILITY' AND a.company_id=i.company_id)); END;
DROP TRIGGER fb_sales_post_seal_eligible;
CREATE TRIGGER fb_sales_post_seal_eligible BEFORE INSERT ON sales_seal WHEN NEW.kind='sales_invoice_revision'
BEGIN SELECT RAISE(ABORT,'sales_posting_eligible: Posting requires an active customer and items') WHERE EXISTS(SELECT 1 FROM sales_invoicerevision r WHERE r.id=NEW.doc_id AND r.state='OPEN' AND (NOT EXISTS(SELECT 1 FROM directory_customer WHERE id=r.customer_id AND active=1) OR EXISTS(SELECT 1 FROM sales_invoiceline l JOIN directory_item t ON t.id=l.item_id WHERE l.revision_id=r.id AND t.active=0))); END;
DROP TRIGGER fb_sales_head_seal;
CREATE TRIGGER fb_sales_head_seal BEFORE INSERT ON sales_seal WHEN NEW.kind='sales_invoice_head'
BEGIN SELECT RAISE(ABORT,'invoice_complete: Invoice requires real sealed revision') WHERE NOT EXISTS(SELECT 1 FROM sales_invoicerevision r JOIN sales_seal s ON s.kind='sales_invoice_revision' AND s.doc_id=r.id WHERE r.invoice_id||':'||r.number=NEW.doc_id); END;
CREATE TRIGGER fb_sales_seal_update BEFORE UPDATE ON sales_seal
BEGIN SELECT RAISE(ABORT,'sales_seal_immutable: Sales seals and obligations are permanent'); END;
CREATE TRIGGER fb_sales_seal_delete BEFORE DELETE ON sales_seal
BEGIN SELECT RAISE(ABORT,'sales_seal_immutable: Sales seals and obligations are permanent'); END;
CREATE TRIGGER fb_sales_seal_due_update BEFORE UPDATE ON sales_seal_due
BEGIN SELECT RAISE(ABORT,'sales_seal_immutable: Sales seals and obligations are permanent'); END;
CREATE TRIGGER fb_sales_seal_due_delete BEFORE DELETE ON sales_seal_due
BEGIN SELECT RAISE(ABORT,'sales_seal_immutable: Sales seals and obligations are permanent'); END;
