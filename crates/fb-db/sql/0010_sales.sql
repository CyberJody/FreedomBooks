-- L3 sales: cents are INTEGER. Exact arithmetic is evaluated by Rust callbacks,
-- never SQLite REAL/round(). Build -> validate -> seal, with deferred obligations.
CREATE TABLE sales_invoice (
 id TEXT PRIMARY KEY, company_id TEXT NOT NULL REFERENCES ledger_company(id),
 identity_id TEXT NOT NULL UNIQUE REFERENCES documents_documentidentity(id),
 creation_digest TEXT NOT NULL CHECK(length(creation_digest)=64), revision INTEGER NOT NULL CHECK(revision>=1)
) STRICT, WITHOUT ROWID;
CREATE TABLE sales_invoicerevision (
 id TEXT PRIMARY KEY, invoice_id TEXT NOT NULL REFERENCES sales_invoice(id), number INTEGER NOT NULL,
 customer_id TEXT NOT NULL REFERENCES directory_customer(id), date TEXT NOT NULL CHECK(date(date) IS date),
 due_date TEXT NOT NULL CHECK(date(due_date) IS due_date AND due_date>=date),
 state TEXT NOT NULL CHECK(state IN('DRAFT','OPEN','VOID')),
 total INTEGER NOT NULL CHECK(total BETWEEN 0 AND 9999999999999999),
 journal_id TEXT REFERENCES ledger_journal(id), reversal_id TEXT REFERENCES ledger_journal(id),
 data TEXT NOT NULL CHECK(json_valid(data)), actor_id TEXT NOT NULL REFERENCES fb_user(id), UNIQUE(invoice_id,number)
) STRICT, WITHOUT ROWID;
CREATE TABLE sales_invoiceline (
 id TEXT PRIMARY KEY, revision_id TEXT NOT NULL REFERENCES sales_invoicerevision(id),
 position INTEGER NOT NULL CHECK(position BETWEEN 1 AND 50),item_id TEXT REFERENCES directory_item(id),
 income_account_id TEXT REFERENCES ledger_account(id),expense_account_id TEXT REFERENCES ledger_account(id),
 amount INTEGER NOT NULL CHECK(amount BETWEEN 0 AND 9999999999999999),taxable INTEGER NOT NULL CHECK(taxable IN(0,1)),
 data TEXT NOT NULL CHECK(json_valid(data)),UNIQUE(revision_id,position)
) STRICT, WITHOUT ROWID;
CREATE TABLE sales_receipt (
 id TEXT PRIMARY KEY,company_id TEXT NOT NULL REFERENCES ledger_company(id),identity_id TEXT NOT NULL UNIQUE REFERENCES documents_documentidentity(id),
 kind TEXT NOT NULL CHECK(kind IN('RCP','SR')),customer_id TEXT NOT NULL REFERENCES directory_customer(id),date TEXT NOT NULL CHECK(date(date) IS date),
 amount INTEGER NOT NULL CHECK(amount BETWEEN 1 AND 9999999999999999),destination TEXT NOT NULL CHECK(destination IN('BANK','UNDEPOSITED')),
 destination_account_id TEXT NOT NULL REFERENCES ledger_account(id),journal_id TEXT NOT NULL UNIQUE REFERENCES ledger_journal(id),
 data TEXT NOT NULL CHECK(json_valid(data)),actor_id TEXT NOT NULL REFERENCES fb_user(id)
) STRICT, WITHOUT ROWID;
CREATE TABLE sales_receiptallocation (
 id TEXT PRIMARY KEY,receipt_id TEXT NOT NULL REFERENCES sales_receipt(id),invoice_id TEXT NOT NULL REFERENCES sales_invoice(id),
 invoice_revision_id TEXT NOT NULL REFERENCES sales_invoicerevision(id),receivables_id TEXT NOT NULL REFERENCES ledger_account(id),
 amount INTEGER NOT NULL CHECK(amount BETWEEN 1 AND 9999999999999999),UNIQUE(receipt_id,invoice_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE sales_receiptreversal (
 id TEXT PRIMARY KEY,receipt_id TEXT NOT NULL UNIQUE REFERENCES sales_receipt(id),journal_id TEXT NOT NULL UNIQUE REFERENCES ledger_journal(id),
 date TEXT NOT NULL CHECK(date(date) IS date),data TEXT NOT NULL CHECK(json_valid(data)),actor_id TEXT NOT NULL REFERENCES fb_user(id)
) STRICT, WITHOUT ROWID;
CREATE TABLE sales_salesreceipt (
 id TEXT PRIMARY KEY,receipt_id TEXT NOT NULL UNIQUE REFERENCES sales_receipt(id),
 total INTEGER NOT NULL CHECK(total BETWEEN 1 AND 9999999999999999),data TEXT NOT NULL CHECK(json_valid(data))
) STRICT, WITHOUT ROWID;
CREATE TABLE sales_salesreceiptline (
 id TEXT PRIMARY KEY,sale_id TEXT NOT NULL REFERENCES sales_salesreceipt(id),position INTEGER NOT NULL CHECK(position BETWEEN 1 AND 50),
 item_id TEXT REFERENCES directory_item(id),income_account_id TEXT NOT NULL REFERENCES ledger_account(id),
 amount INTEGER NOT NULL CHECK(amount BETWEEN 0 AND 9999999999999999),taxable INTEGER NOT NULL CHECK(taxable IN(0,1)),
 data TEXT NOT NULL CHECK(json_valid(data)),UNIQUE(sale_id,position)
) STRICT, WITHOUT ROWID;

CREATE TRIGGER fb_sales_invoice_insert BEFORE INSERT ON sales_invoice
BEGIN
 SELECT RAISE(ABORT,'invoice_immutable_ownership: Invoices cannot be replaced') WHERE EXISTS(SELECT 1 FROM sales_invoice WHERE id=NEW.id OR identity_id=NEW.identity_id);
 SELECT RAISE(ABORT,'invoice_identity_scope: Invalid invoice identity') WHERE NEW.revision<>1 OR NOT EXISTS(SELECT 1 FROM documents_documentidentity WHERE id=NEW.identity_id AND company_id=NEW.company_id AND kind='INV');
END;
CREATE TRIGGER fb_sales_invoice_update BEFORE UPDATE ON sales_invoice
BEGIN
 SELECT RAISE(ABORT,'invoice_immutable_ownership: Invoice ownership is immutable; revisions advance by one') WHERE NEW.id IS NOT OLD.id OR NEW.company_id IS NOT OLD.company_id OR NEW.identity_id IS NOT OLD.identity_id OR NEW.creation_digest IS NOT OLD.creation_digest OR NEW.revision<>OLD.revision+1 OR NOT EXISTS(SELECT 1 FROM sales_invoicerevision r JOIN fb_seal s ON s.kind='sales_invoice_revision' AND s.doc_id=r.id WHERE r.invoice_id=OLD.id AND r.number=OLD.revision);
END;
CREATE TRIGGER fb_sales_invoice_delete BEFORE DELETE ON sales_invoice
BEGIN SELECT RAISE(ABORT,'invoice_immutable_ownership: Invoices cannot be deleted'); END;
CREATE TRIGGER fb_sales_invoice_due_insert AFTER INSERT ON sales_invoice
BEGIN INSERT INTO fb_seal_due(kind,doc_id) VALUES('sales_invoice_head',NEW.id||':'||NEW.revision); END;
CREATE TRIGGER fb_sales_invoice_due_update AFTER UPDATE ON sales_invoice
BEGIN INSERT INTO fb_seal_due(kind,doc_id) VALUES('sales_invoice_head',NEW.id||':'||NEW.revision); END;

CREATE TRIGGER fb_sales_revision_insert BEFORE INSERT ON sales_invoicerevision
BEGIN
 SELECT RAISE(ABORT,'invoice_history_immutable: Invoice revisions are immutable') WHERE EXISTS(SELECT 1 FROM sales_invoicerevision WHERE id=NEW.id OR (invoice_id=NEW.invoice_id AND number=NEW.number));
 SELECT RAISE(ABORT,'invoice_revision_scope: Invalid revision ownership') WHERE NOT EXISTS(SELECT 1 FROM sales_invoice i JOIN directory_customer c ON c.id=NEW.customer_id WHERE i.id=NEW.invoice_id AND i.revision=NEW.number AND c.company_id=i.company_id);
 SELECT RAISE(ABORT,'invoice_revision_fields: Invalid invoice revision fields') WHERE json_extract(NEW.data,'$.id') IS NOT NEW.id OR json_extract(NEW.data,'$.number') IS NOT NEW.number OR json_extract(NEW.data,'$.customer_id') IS NOT NEW.customer_id OR json_extract(NEW.data,'$.date') IS NOT NEW.date OR json_extract(NEW.data,'$.due_date') IS NOT NEW.due_date OR json_extract(NEW.data,'$.state') IS NOT NEW.state OR json_extract(NEW.data,'$.journal_id') IS NOT NEW.journal_id OR json_extract(NEW.data,'$.reversal_id') IS NOT NEW.reversal_id OR fb_sales_money(json_quote(json_extract(NEW.data,'$.total'))) IS NOT NEW.total OR length(json_extract(NEW.data,'$.memo'))>2000 OR length(json_extract(NEW.data,'$.reason'))>500;
 SELECT RAISE(ABORT,'invoice_transition: Invalid invoice state transition') WHERE (NEW.number=1 AND (NEW.state<>'DRAFT' OR json_extract(NEW.data,'$.action') IS NOT 'created')) OR (NEW.number>1 AND NOT EXISTS(SELECT 1 FROM sales_invoicerevision r WHERE r.invoice_id=NEW.invoice_id AND r.number=NEW.number-1 AND ((r.state='DRAFT' AND NEW.state IN('DRAFT','OPEN')) OR (r.state='OPEN' AND NEW.state='VOID'))));
 SELECT RAISE(ABORT,'invoice_financial_immutable: Posting and void preserve financial evidence') WHERE NEW.state IN('OPEN','VOID') AND json_remove(NEW.data,'$.id','$.number','$.state','$.action','$.reason','$.actor_name','$.journal_id','$.reversal_id','$.review_digest') IS NOT (SELECT json_remove(data,'$.id','$.number','$.state','$.action','$.reason','$.actor_name','$.journal_id','$.reversal_id','$.review_digest') FROM sales_invoicerevision WHERE invoice_id=NEW.invoice_id AND number=NEW.number-1);
 SELECT RAISE(ABORT,'invoice_posting_link: Invalid invoice posting linkage') WHERE
 (NEW.state='DRAFT' AND (NEW.journal_id IS NOT NULL OR NEW.reversal_id IS NOT NULL OR json_extract(NEW.data,'$.review_digest') IS NOT '')) OR
 (NEW.state IN('OPEN','VOID') AND (NEW.total<=0 OR NOT EXISTS(SELECT 1 FROM ledger_journal j JOIN sales_invoice i ON i.identity_id=j.source_id WHERE i.id=NEW.invoice_id AND j.id=NEW.journal_id AND j.company_id=i.company_id AND j.date=NEW.date AND j.status='POSTED' AND j.reversal_of_id IS NULL))) OR
 (NEW.state='OPEN' AND (NEW.reversal_id IS NOT NULL OR length(json_extract(NEW.data,'$.review_digest'))<>64)) OR
 (NEW.state='VOID' AND (trim(json_extract(NEW.data,'$.reason'))='' OR NEW.journal_id IS NOT (SELECT journal_id FROM sales_invoicerevision WHERE invoice_id=NEW.invoice_id AND number=NEW.number-1) OR json_extract(NEW.data,'$.review_digest') IS NOT (SELECT json_extract(data,'$.review_digest') FROM sales_invoicerevision WHERE invoice_id=NEW.invoice_id AND number=NEW.number-1) OR NOT EXISTS(SELECT 1 FROM ledger_journal j WHERE j.id=NEW.reversal_id AND j.reversal_of_id=NEW.journal_id AND j.status='POSTED')));
 SELECT RAISE(ABORT,'invoice_live_receipts: Reverse live receipts before voiding an invoice') WHERE NEW.state='VOID' AND EXISTS(SELECT 1 FROM sales_receiptallocation a WHERE a.invoice_id=NEW.invoice_id AND NOT EXISTS(SELECT 1 FROM sales_receiptreversal z WHERE z.receipt_id=a.receipt_id));
END;
CREATE TRIGGER fb_sales_revision_due AFTER INSERT ON sales_invoicerevision
BEGIN INSERT INTO fb_seal_due(kind,doc_id) VALUES('sales_invoice_revision',NEW.id); END;
CREATE TRIGGER fb_sales_invoice_line_insert BEFORE INSERT ON sales_invoiceline
BEGIN
 SELECT RAISE(ABORT,'invoice_history_immutable: Invoice lines are immutable') WHERE EXISTS(SELECT 1 FROM sales_invoiceline WHERE id=NEW.id OR (revision_id=NEW.revision_id AND position=NEW.position));
 SELECT RAISE(ABORT,'invoice_line_scope: Invalid invoice line scope or committed history') WHERE NOT EXISTS(SELECT 1 FROM sales_invoicerevision r JOIN sales_invoice i ON i.id=r.invoice_id WHERE r.id=NEW.revision_id AND r.number=i.revision AND NOT EXISTS(SELECT 1 FROM fb_seal s WHERE s.kind='sales_invoice_revision' AND s.doc_id=r.id) AND (NEW.item_id IS NULL OR EXISTS(SELECT 1 FROM directory_item WHERE id=NEW.item_id AND company_id=i.company_id)) AND (NEW.income_account_id IS NULL OR EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.income_account_id AND company_id=i.company_id)) AND (NEW.expense_account_id IS NULL OR EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.expense_account_id AND company_id=i.company_id)));
 SELECT RAISE(ABORT,'invoice_line_calculation: Invalid invoice line calculation') WHERE NOT fb_sales_line_ok(NEW.data) OR json_extract(NEW.data,'$.position') IS NOT NEW.position OR json_extract(NEW.data,'$.item_id') IS NOT NEW.item_id OR json_extract(NEW.data,'$.income_account_id') IS NOT NEW.income_account_id OR json_extract(NEW.data,'$.expense_account_id') IS NOT NEW.expense_account_id OR json_extract(NEW.data,'$.taxable') IS NOT NEW.taxable OR fb_sales_money(json_quote(json_extract(NEW.data,'$.amount'))) IS NOT NEW.amount;
END;

CREATE TRIGGER fb_sales_receipt_insert BEFORE INSERT ON sales_receipt
BEGIN
 SELECT RAISE(ABORT,'receipt_immutable: Receipt history is immutable') WHERE EXISTS(SELECT 1 FROM sales_receipt WHERE id=NEW.id OR identity_id=NEW.identity_id OR journal_id=NEW.journal_id);
 SELECT RAISE(ABORT,'receipt_scope: Invalid receipt ownership/posting') WHERE NOT EXISTS(SELECT 1 FROM documents_documentidentity d JOIN directory_customer c ON c.id=NEW.customer_id JOIN ledger_account a ON a.id=NEW.destination_account_id JOIN ledger_journal j ON j.id=NEW.journal_id WHERE d.id=NEW.identity_id AND d.company_id=NEW.company_id AND d.kind=NEW.kind AND c.company_id=NEW.company_id AND c.active=1 AND a.company_id=NEW.company_id AND a.kind='ASSET' AND a.active=1 AND a.postable=1 AND j.company_id=NEW.company_id AND j.source_id=d.id AND j.status='POSTED' AND j.reversal_of_id IS NULL AND j.date=NEW.date);
 SELECT RAISE(ABORT,'receipt_fields: Invalid receipt fields') WHERE json_extract(NEW.data,'$.id') IS NOT NEW.id OR json_extract(NEW.data,'$.identity_id') IS NOT NEW.identity_id OR json_extract(NEW.data,'$.customer_id') IS NOT NEW.customer_id OR json_extract(NEW.data,'$.kind') IS NOT NEW.kind OR json_extract(NEW.data,'$.date') IS NOT NEW.date OR json_extract(NEW.data,'$.destination') IS NOT NEW.destination OR json_extract(NEW.data,'$.destination_account_id') IS NOT NEW.destination_account_id OR json_extract(NEW.data,'$.journal_id') IS NOT NEW.journal_id OR fb_sales_money(json_quote(json_extract(NEW.data,'$.amount'))) IS NOT NEW.amount OR json_extract(NEW.data,'$.method') NOT IN('CHECK','CASH','CARD','TRANSFER','OTHER') OR length(json_extract(NEW.data,'$.reference'))>120 OR length(json_extract(NEW.data,'$.memo'))>2000 OR length(json_extract(NEW.data,'$.creation_digest'))<>64 OR length(json_extract(NEW.data,'$.review_digest'))<>64;
END;
CREATE TRIGGER fb_sales_receipt_due AFTER INSERT ON sales_receipt
BEGIN INSERT INTO fb_seal_due(kind,doc_id) VALUES('sales_receipt',NEW.id); END;
CREATE TRIGGER fb_sales_allocation_insert BEFORE INSERT ON sales_receiptallocation
BEGIN
 SELECT RAISE(ABORT,'receipt_immutable: Allocation history is immutable') WHERE EXISTS(SELECT 1 FROM sales_receiptallocation WHERE id=NEW.id OR (receipt_id=NEW.receipt_id AND invoice_id=NEW.invoice_id));
 SELECT RAISE(ABORT,'receipt_late_allocation: Cannot append to committed allocations') WHERE NOT EXISTS(SELECT 1 FROM sales_receipt WHERE id=NEW.receipt_id) OR EXISTS(SELECT 1 FROM fb_seal WHERE kind='sales_receipt' AND doc_id=NEW.receipt_id);
 SELECT RAISE(ABORT,'sales_receipt_no_allocations: Sales receipts carry no invoice allocations') WHERE (SELECT kind FROM sales_receipt WHERE id=NEW.receipt_id) IS NOT 'RCP';
 SELECT RAISE(ABORT,'allocation_scope: Invalid allocation target or original AR') WHERE NOT EXISTS(SELECT 1 FROM sales_receipt r JOIN sales_invoice i ON i.id=NEW.invoice_id JOIN sales_invoicerevision v ON v.id=NEW.invoice_revision_id WHERE r.id=NEW.receipt_id AND i.company_id=r.company_id AND v.invoice_id=i.id AND v.number=i.revision AND v.customer_id=r.customer_id AND v.state='OPEN' AND v.date<=r.date AND NEW.receivables_id<>r.destination_account_id AND EXISTS(SELECT 1 FROM documents_documentaccount d WHERE d.source_id=i.identity_id AND d.role='receivables' AND d.account_id=NEW.receivables_id));
 SELECT RAISE(ABORT,'allocation_outstanding: Live allocations cannot overpay or target void invoices') WHERE NEW.amount+(SELECT coalesce(sum(a.amount),0) FROM sales_receiptallocation a WHERE a.invoice_id=NEW.invoice_id AND NOT EXISTS(SELECT 1 FROM sales_receiptreversal z WHERE z.receipt_id=a.receipt_id))>(SELECT total FROM sales_invoicerevision WHERE id=NEW.invoice_revision_id);
END;
CREATE TRIGGER fb_sales_reversal_insert BEFORE INSERT ON sales_receiptreversal
BEGIN
 SELECT RAISE(ABORT,'receipt_immutable: Reversal history is immutable') WHERE EXISTS(SELECT 1 FROM sales_receiptreversal WHERE id=NEW.id OR receipt_id=NEW.receipt_id OR journal_id=NEW.journal_id);
 SELECT RAISE(ABORT,'receipt_reversal_link: Invalid receipt reversal') WHERE NOT EXISTS(SELECT 1 FROM sales_receipt r JOIN ledger_journal j ON j.id=NEW.journal_id WHERE r.id=NEW.receipt_id AND j.source_id=r.identity_id AND j.company_id=r.company_id AND j.status='POSTED' AND j.reversal_of_id=r.journal_id AND j.date=NEW.date AND NEW.date>=r.date AND json_extract(NEW.data,'$.date')=NEW.date AND json_extract(NEW.data,'$.journal_id')=NEW.journal_id AND json_extract(NEW.data,'$.receipt_id')=r.id AND length(trim(json_extract(NEW.data,'$.reason'))) BETWEEN 1 AND 500);
END;
CREATE TRIGGER fb_sales_reversal_due AFTER INSERT ON sales_receiptreversal
BEGIN INSERT INTO fb_seal_due(kind,doc_id) VALUES('sales_receipt_reversal',NEW.id); END;
CREATE TRIGGER fb_sales_detail_insert BEFORE INSERT ON sales_salesreceipt
BEGIN
 SELECT RAISE(ABORT,'receipt_immutable: Sales receipt history is immutable') WHERE EXISTS(SELECT 1 FROM sales_salesreceipt WHERE id=NEW.id OR receipt_id=NEW.receipt_id);
 SELECT RAISE(ABORT,'sales_receipt_scope: Sale detail requires a sales receipt') WHERE (SELECT kind FROM sales_receipt WHERE id=NEW.receipt_id) IS NOT 'SR';
 SELECT RAISE(ABORT,'sales_receipt_late_detail: Cannot append sale detail to a committed receipt') WHERE EXISTS(SELECT 1 FROM fb_seal WHERE kind='sales_receipt' AND doc_id=NEW.receipt_id);
 SELECT RAISE(ABORT,'sales_receipt_fields: Invalid sales receipt detail') WHERE NEW.total IS NOT (SELECT amount FROM sales_receipt WHERE id=NEW.receipt_id) OR fb_sales_money(json_quote(json_extract(NEW.data,'$.total'))) IS NOT NEW.total;
END;
CREATE TRIGGER fb_sales_sale_line_insert BEFORE INSERT ON sales_salesreceiptline
BEGIN
 SELECT RAISE(ABORT,'receipt_immutable: Sales receipt lines are immutable') WHERE EXISTS(SELECT 1 FROM sales_salesreceiptline WHERE id=NEW.id OR (sale_id=NEW.sale_id AND position=NEW.position));
 SELECT RAISE(ABORT,'sales_receipt_late_line: Cannot append lines to a committed sales receipt') WHERE NOT EXISTS(SELECT 1 FROM sales_salesreceipt s JOIN sales_receipt r ON r.id=s.receipt_id WHERE s.id=NEW.sale_id AND NOT EXISTS(SELECT 1 FROM fb_seal z WHERE z.kind='sales_receipt' AND z.doc_id=r.id));
 SELECT RAISE(ABORT,'sales_receipt_line_scope: Invalid sales receipt line') WHERE NOT fb_sales_line_ok(NEW.data) OR json_extract(NEW.data,'$.position') IS NOT NEW.position OR json_extract(NEW.data,'$.item_id') IS NOT NEW.item_id OR json_extract(NEW.data,'$.income_account_id') IS NOT NEW.income_account_id OR json_extract(NEW.data,'$.taxable') IS NOT NEW.taxable OR fb_sales_money(json_quote(json_extract(NEW.data,'$.amount'))) IS NOT NEW.amount OR NOT EXISTS(SELECT 1 FROM sales_salesreceipt s JOIN sales_receipt r ON r.id=s.receipt_id JOIN ledger_account a ON a.id=NEW.income_account_id WHERE s.id=NEW.sale_id AND a.company_id=r.company_id AND a.kind='INCOME' AND a.active=1 AND a.postable=1 AND (NEW.item_id IS NULL OR EXISTS(SELECT 1 FROM directory_item WHERE id=NEW.item_id AND company_id=r.company_id AND active=1)));
END;

-- Expected postings are multisets represented by distinct accounts and exact count.
CREATE VIEW sales_expected_entries AS
 SELECT i.identity_id source_id,r.journal_id,d.account_id,r.total debit,0 credit FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.state='OPEN' JOIN documents_documentaccount d ON d.source_id=i.identity_id AND d.role='receivables'
 UNION ALL SELECT i.identity_id,r.journal_id,l.income_account_id,0,sum(l.amount) FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.state='OPEN' JOIN sales_invoiceline l ON l.revision_id=r.id GROUP BY r.id,l.income_account_id HAVING sum(l.amount)>0
 UNION ALL SELECT i.identity_id,r.journal_id,d.account_id,0,fb_sales_money(json_quote(json_extract(r.data,'$.tax'))) FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.state='OPEN' JOIN documents_documentaccount d ON d.source_id=i.identity_id AND d.role='collected_tax' WHERE fb_sales_money(json_quote(json_extract(r.data,'$.tax')))>0
 UNION ALL SELECT r.identity_id,r.journal_id,r.destination_account_id,r.amount,0 FROM sales_receipt r
 UNION ALL SELECT r.identity_id,r.journal_id,a.receivables_id,0,sum(a.amount) FROM sales_receipt r JOIN sales_receiptallocation a ON a.receipt_id=r.id WHERE r.kind='RCP' GROUP BY r.id,a.receivables_id
 UNION ALL SELECT r.identity_id,r.journal_id,l.income_account_id,0,sum(l.amount) FROM sales_receipt r JOIN sales_salesreceipt s ON s.receipt_id=r.id JOIN sales_salesreceiptline l ON l.sale_id=s.id GROUP BY r.id,l.income_account_id HAVING sum(l.amount)>0
 UNION ALL SELECT r.identity_id,r.journal_id,json_extract(s.data,'$.collected_tax_id'),0,fb_sales_money(json_quote(json_extract(s.data,'$.tax'))) FROM sales_receipt r JOIN sales_salesreceipt s ON s.receipt_id=r.id WHERE fb_sales_money(json_quote(json_extract(s.data,'$.tax')))>0;
CREATE VIEW sales_journal_mismatch AS
 SELECT j.id FROM ledger_journal j WHERE EXISTS(SELECT 1 FROM sales_expected_entries e WHERE e.journal_id=j.id) AND (
 (SELECT count(*) FROM ledger_journalline l WHERE l.journal_id=j.id)<>(SELECT count(*) FROM sales_expected_entries e WHERE e.journal_id=j.id) OR
 EXISTS(SELECT account_id,debit,credit FROM ledger_journalline WHERE journal_id=j.id EXCEPT SELECT account_id,debit,credit FROM sales_expected_entries WHERE journal_id=j.id) OR
 EXISTS(SELECT account_id,debit,credit FROM sales_expected_entries WHERE journal_id=j.id EXCEPT SELECT account_id,debit,credit FROM ledger_journalline WHERE journal_id=j.id));
CREATE VIEW sales_expected_references AS
 SELECT i.identity_id source_id,'receivables' role,d.account_id FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.state='OPEN' JOIN documents_documentaccount d ON d.source_id=i.identity_id AND d.role='receivables'
 UNION ALL SELECT i.identity_id,'income:'||l.position,l.income_account_id FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.state='OPEN' JOIN sales_invoiceline l ON l.revision_id=r.id
 UNION ALL SELECT i.identity_id,'collected_tax',d.account_id FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.state='OPEN' JOIN documents_documentaccount d ON d.source_id=i.identity_id AND d.role='collected_tax' WHERE fb_sales_money(json_quote(json_extract(r.data,'$.tax')))>0
 UNION ALL SELECT identity_id,'destination',destination_account_id FROM sales_receipt
 UNION SELECT r.identity_id,'receivables:'||a.receivables_id,a.receivables_id FROM sales_receipt r JOIN sales_receiptallocation a ON a.receipt_id=r.id
 UNION SELECT r.identity_id,'income:'||l.income_account_id,l.income_account_id FROM sales_receipt r JOIN sales_salesreceipt s ON s.receipt_id=r.id JOIN sales_salesreceiptline l ON l.sale_id=s.id
 UNION ALL SELECT r.identity_id,'collected_tax',json_extract(s.data,'$.collected_tax_id') FROM sales_receipt r JOIN sales_salesreceipt s ON s.receipt_id=r.id WHERE json_extract(s.data,'$.collected_tax_id') IS NOT NULL;
CREATE VIEW sales_reference_mismatch AS
 SELECT d.id FROM documents_documentidentity d WHERE (EXISTS(SELECT 1 FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.state='OPEN' WHERE i.identity_id=d.id) OR EXISTS(SELECT 1 FROM sales_receipt WHERE identity_id=d.id)) AND (
 EXISTS(SELECT role,account_id FROM documents_documentaccount WHERE source_id=d.id EXCEPT SELECT role,account_id FROM sales_expected_references WHERE source_id=d.id) OR
 EXISTS(SELECT role,account_id FROM sales_expected_references WHERE source_id=d.id EXCEPT SELECT role,account_id FROM documents_documentaccount WHERE source_id=d.id));

CREATE TRIGGER fb_sales_invoice_seal BEFORE INSERT ON fb_seal WHEN NEW.kind='sales_invoice_revision'
BEGIN
 SELECT RAISE(ABORT,'invoice_complete: Invoice requires real revision') WHERE NOT EXISTS(SELECT 1 FROM sales_invoicerevision WHERE id=NEW.doc_id);
 SELECT RAISE(ABORT,'invoice_complete: Incomplete or inconsistent invoice totals') WHERE EXISTS(SELECT 1 FROM sales_invoicerevision r WHERE r.id=NEW.doc_id AND NOT fb_sales_document_ok(r.data,coalesce((SELECT json_group_array(json(l.data)) FROM (SELECT data FROM sales_invoiceline WHERE revision_id=r.id ORDER BY position) l),'[]'),r.total));
 SELECT RAISE(ABORT,'invoice_financial_immutable: Posting and void must copy exact lines') WHERE EXISTS(SELECT 1 FROM sales_invoicerevision r JOIN sales_invoicerevision p ON p.invoice_id=r.invoice_id AND p.number=r.number-1 WHERE r.id=NEW.doc_id AND r.state IN('OPEN','VOID') AND (EXISTS(SELECT data FROM sales_invoiceline WHERE revision_id=r.id EXCEPT SELECT data FROM sales_invoiceline WHERE revision_id=p.id) OR EXISTS(SELECT data FROM sales_invoiceline WHERE revision_id=p.id EXCEPT SELECT data FROM sales_invoiceline WHERE revision_id=r.id)));
 SELECT RAISE(ABORT,'invoice_posting_complete: Invoice posting must exactly match its lines and account evidence') WHERE EXISTS(SELECT 1 FROM sales_invoicerevision r JOIN sales_invoice i ON i.id=r.invoice_id WHERE r.id=NEW.doc_id AND r.state IN('OPEN','VOID') AND (EXISTS(SELECT 1 FROM sales_journal_mismatch WHERE id=r.journal_id) OR EXISTS(SELECT 1 FROM sales_reference_mismatch WHERE id=i.identity_id) OR NOT EXISTS(SELECT 1 FROM documents_documentaccount d JOIN ledger_account a ON a.id=d.account_id WHERE d.source_id=i.identity_id AND d.role='receivables' AND a.kind='ASSET') OR EXISTS(SELECT 1 FROM sales_invoiceline l LEFT JOIN ledger_account a ON a.id=l.income_account_id WHERE l.revision_id=r.id AND (a.id IS NULL OR a.kind<>'INCOME'))));
END;
CREATE TRIGGER fb_sales_invoice_head_seal AFTER INSERT ON fb_seal WHEN NEW.kind='sales_invoice_revision'
BEGIN INSERT INTO fb_seal(kind,doc_id) SELECT 'sales_invoice_head',invoice_id||':'||number FROM sales_invoicerevision WHERE id=NEW.doc_id; END;
CREATE TRIGGER fb_sales_receipt_seal BEFORE INSERT ON fb_seal WHEN NEW.kind='sales_receipt'
BEGIN
 SELECT RAISE(ABORT,'receipt_complete: Receipt allocations must equal amount') WHERE NOT EXISTS(SELECT 1 FROM sales_receipt WHERE id=NEW.doc_id) OR EXISTS(SELECT 1 FROM sales_receipt r WHERE r.id=NEW.doc_id AND r.kind='RCP' AND ((SELECT count(*) FROM sales_receiptallocation WHERE receipt_id=r.id) NOT BETWEEN 1 AND 50 OR (SELECT coalesce(sum(amount),0) FROM sales_receiptallocation WHERE receipt_id=r.id)<>r.amount));
 SELECT RAISE(ABORT,'sales_receipt_complete: Sales receipt detail and lines must equal amount') WHERE EXISTS(SELECT 1 FROM sales_receipt r WHERE r.id=NEW.doc_id AND r.kind='SR' AND NOT EXISTS(SELECT 1 FROM sales_salesreceipt s WHERE s.receipt_id=r.id AND s.total=r.amount AND fb_sales_document_ok(s.data,coalesce((SELECT json_group_array(json(l.data)) FROM (SELECT data FROM sales_salesreceiptline WHERE sale_id=s.id ORDER BY position) l),'[]'),s.total) AND ((fb_sales_money(json_quote(json_extract(s.data,'$.tax')))>0)=(json_extract(s.data,'$.collected_tax_id') IS NOT NULL))));
 SELECT RAISE(ABORT,'receipt_journal_split: Receipt journal must exactly match its amount and account evidence') WHERE EXISTS(SELECT 1 FROM sales_receipt r WHERE r.id=NEW.doc_id AND (EXISTS(SELECT 1 FROM sales_journal_mismatch WHERE id=r.journal_id) OR EXISTS(SELECT 1 FROM sales_reference_mismatch WHERE id=r.identity_id)));
END;
CREATE TRIGGER fb_sales_reversal_seal BEFORE INSERT ON fb_seal WHEN NEW.kind='sales_receipt_reversal'
BEGIN SELECT RAISE(ABORT,'receipt_reversal_link: Invalid receipt reversal') WHERE NOT EXISTS(SELECT 1 FROM sales_receiptreversal WHERE id=NEW.doc_id); END;
CREATE TRIGGER fb_sales_journal_due AFTER INSERT ON ledger_journal
WHEN (SELECT kind FROM documents_documentidentity WHERE id=NEW.source_id) IN('RCP','SR') OR EXISTS(SELECT 1 FROM sales_invoice WHERE identity_id=NEW.source_id)
BEGIN INSERT INTO fb_seal_due(kind,doc_id) VALUES('sales_journal',NEW.id); END;
CREATE TRIGGER fb_sales_journal_seal BEFORE INSERT ON fb_seal WHEN NEW.kind='sales_journal'
BEGIN
 SELECT RAISE(ABORT,'sales_journal_owner: Sales journals require complete owning lifecycle') WHERE NOT EXISTS(SELECT 1 FROM ledger_journal j WHERE j.id=NEW.doc_id AND j.status='POSTED' AND (
 (j.reversal_of_id IS NULL AND (EXISTS(SELECT 1 FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id WHERE i.identity_id=j.source_id AND r.journal_id=j.id AND r.state='OPEN' AND EXISTS(SELECT 1 FROM fb_seal WHERE kind='sales_invoice_revision' AND doc_id=r.id)) OR EXISTS(SELECT 1 FROM sales_receipt r WHERE r.identity_id=j.source_id AND r.journal_id=j.id AND EXISTS(SELECT 1 FROM fb_seal WHERE kind='sales_receipt' AND doc_id=r.id)))) OR
 (j.reversal_of_id IS NOT NULL AND (EXISTS(SELECT 1 FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id WHERE i.identity_id=j.source_id AND r.reversal_id=j.id AND r.state='VOID' AND EXISTS(SELECT 1 FROM fb_seal WHERE kind='sales_invoice_revision' AND doc_id=r.id)) OR EXISTS(SELECT 1 FROM sales_receiptreversal z JOIN sales_receipt r ON r.id=z.receipt_id WHERE r.identity_id=j.source_id AND z.journal_id=j.id AND EXISTS(SELECT 1 FROM fb_seal WHERE kind='sales_receipt_reversal' AND doc_id=z.id))))));
END;
CREATE TRIGGER fb_sales_reference_insert BEFORE INSERT ON documents_documentaccount
BEGIN
 SELECT RAISE(ABORT,'sales_late_reference: Cannot append sales account evidence after sealing') WHERE EXISTS(SELECT 1 FROM sales_receipt r JOIN fb_seal s ON s.kind='sales_receipt' AND s.doc_id=r.id WHERE r.identity_id=NEW.source_id) OR EXISTS(SELECT 1 FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id JOIN fb_seal s ON s.kind='sales_invoice_revision' AND s.doc_id=r.id WHERE i.identity_id=NEW.source_id AND r.state IN('OPEN','VOID'));
END;
-- Immutable-row and REPLACE guards are generated below; no reliance on recursive_triggers.
CREATE TRIGGER fb_sales_invoicerevision_update BEFORE UPDATE ON sales_invoicerevision
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales history is immutable'); END;
CREATE TRIGGER fb_sales_invoicerevision_delete BEFORE DELETE ON sales_invoicerevision
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales history is immutable'); END;
CREATE TRIGGER fb_sales_invoiceline_update BEFORE UPDATE ON sales_invoiceline
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales history is immutable'); END;
CREATE TRIGGER fb_sales_invoiceline_delete BEFORE DELETE ON sales_invoiceline
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales history is immutable'); END;
CREATE TRIGGER fb_sales_receipt_update BEFORE UPDATE ON sales_receipt
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales history is immutable'); END;
CREATE TRIGGER fb_sales_receipt_delete BEFORE DELETE ON sales_receipt
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales history is immutable'); END;
CREATE TRIGGER fb_sales_receiptallocation_update BEFORE UPDATE ON sales_receiptallocation
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales history is immutable'); END;
CREATE TRIGGER fb_sales_receiptallocation_delete BEFORE DELETE ON sales_receiptallocation
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales history is immutable'); END;
CREATE TRIGGER fb_sales_receiptreversal_update BEFORE UPDATE ON sales_receiptreversal
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales history is immutable'); END;
CREATE TRIGGER fb_sales_receiptreversal_delete BEFORE DELETE ON sales_receiptreversal
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales history is immutable'); END;
CREATE TRIGGER fb_sales_salesreceipt_update BEFORE UPDATE ON sales_salesreceipt
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales history is immutable'); END;
CREATE TRIGGER fb_sales_salesreceipt_delete BEFORE DELETE ON sales_salesreceipt
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales history is immutable'); END;
CREATE TRIGGER fb_sales_salesreceiptline_update BEFORE UPDATE ON sales_salesreceiptline
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales history is immutable'); END;
CREATE TRIGGER fb_sales_salesreceiptline_delete BEFORE DELETE ON sales_salesreceiptline
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales history is immutable'); END;
