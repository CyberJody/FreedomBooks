-- Invoice-only adjustments; stored line amounts remain unsigned magnitudes.
DROP TRIGGER fb_sales_invoice_line_insert;
CREATE TRIGGER fb_sales_invoice_line_insert BEFORE INSERT ON sales_invoiceline
BEGIN
 SELECT RAISE(ABORT,'invoice_history_immutable: Invoice lines are immutable') WHERE EXISTS(SELECT 1 FROM sales_invoiceline WHERE id=NEW.id OR (revision_id=NEW.revision_id AND position=NEW.position));
 SELECT RAISE(ABORT,'invoice_line_scope: Invalid invoice line scope or committed history') WHERE NOT EXISTS(SELECT 1 FROM sales_invoicerevision r JOIN sales_invoice i ON i.id=r.invoice_id WHERE r.id=NEW.revision_id AND r.number=i.revision AND NOT EXISTS(SELECT 1 FROM sales_seal s WHERE s.kind='sales_invoice_revision' AND s.doc_id=r.id) AND (NEW.item_id IS NULL OR EXISTS(SELECT 1 FROM directory_item WHERE id=NEW.item_id AND company_id=i.company_id)) AND (NEW.income_account_id IS NULL OR EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.income_account_id AND company_id=i.company_id)) AND (NEW.expense_account_id IS NULL OR EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.expense_account_id AND company_id=i.company_id)));
 SELECT RAISE(ABORT,'invoice_line_calculation: Invalid invoice line calculation') WHERE NOT fb_invoice_line_ok(NEW.data) OR json_extract(NEW.data,'$.position') IS NOT NEW.position OR json_extract(NEW.data,'$.item_id') IS NOT NEW.item_id OR json_extract(NEW.data,'$.income_account_id') IS NOT NEW.income_account_id OR json_extract(NEW.data,'$.expense_account_id') IS NOT NEW.expense_account_id OR json_extract(NEW.data,'$.taxable') IS NOT NEW.taxable OR fb_sales_money(json_quote(json_extract(NEW.data,'$.amount'))) IS NOT NEW.amount;
END;
DROP TRIGGER fb_sales_invoice_seal;
CREATE TRIGGER fb_sales_invoice_seal BEFORE INSERT ON sales_seal WHEN NEW.kind='sales_invoice_revision'
BEGIN
 SELECT RAISE(ABORT,'invoice_complete: Invoice requires real revision') WHERE NOT EXISTS(SELECT 1 FROM sales_invoicerevision WHERE id=NEW.doc_id);
 SELECT RAISE(ABORT,'invoice_complete: Incomplete or inconsistent invoice totals') WHERE EXISTS(SELECT 1 FROM sales_invoicerevision r WHERE r.id=NEW.doc_id AND NOT fb_invoice_document_ok(r.data,coalesce((SELECT json_group_array(json(l.data)) FROM (SELECT data FROM sales_invoiceline WHERE revision_id=r.id ORDER BY position) l),'[]'),r.total));
 SELECT RAISE(ABORT,'invoice_financial_immutable: Posting and void must copy exact lines') WHERE EXISTS(SELECT 1 FROM sales_invoicerevision r JOIN sales_invoicerevision p ON p.invoice_id=r.invoice_id AND p.number=r.number-1 WHERE r.id=NEW.doc_id AND r.state IN('OPEN','VOID') AND (EXISTS(SELECT data FROM sales_invoiceline WHERE revision_id=r.id EXCEPT SELECT data FROM sales_invoiceline WHERE revision_id=p.id) OR EXISTS(SELECT data FROM sales_invoiceline WHERE revision_id=p.id EXCEPT SELECT data FROM sales_invoiceline WHERE revision_id=r.id)));
 SELECT RAISE(ABORT,'invoice_posting_complete: Invoice posting must exactly match its lines and account evidence') WHERE EXISTS(SELECT 1 FROM sales_invoicerevision r JOIN sales_invoice i ON i.id=r.invoice_id WHERE r.id=NEW.doc_id AND r.state IN('OPEN','VOID') AND (EXISTS(SELECT 1 FROM sales_journal_mismatch WHERE id=r.journal_id) OR EXISTS(SELECT 1 FROM sales_reference_mismatch WHERE id=i.identity_id) OR NOT EXISTS(SELECT 1 FROM documents_documentaccount d JOIN ledger_account a ON a.id=d.account_id WHERE d.source_id=i.identity_id AND d.role='receivables' AND a.kind='ASSET') OR EXISTS(SELECT 1 FROM sales_invoiceline l LEFT JOIN ledger_account a ON a.id=l.income_account_id WHERE l.revision_id=r.id AND coalesce(json_extract(l.data,'$.line_kind'),'CHARGE')<>'NOTE' AND (a.id IS NULL OR a.kind<>'INCOME'))));
END;
DROP VIEW sales_snapshot_violations;
CREATE VIEW sales_snapshot_violations AS
 SELECT 'invoice snapshot: '||r.id problem FROM sales_invoicerevision r
 JOIN sales_invoice i ON i.id=r.invoice_id
 WHERE NOT json_valid(r.data) OR json_type(r.data) IS NOT 'object'
   OR json_type(r.data,'$.customer_snapshot') IS NOT 'object'
   OR json_type(r.data,'$.company_snapshot') IS NOT 'object'
   OR json_extract(r.data,'$.company_snapshot.id') IS NOT i.company_id
   OR NOT fb_sales_header_ok('invoice',r.data)
 UNION ALL
 SELECT 'invoice line snapshot: '||l.id FROM sales_invoiceline l
 WHERE NOT json_valid(l.data) OR json_type(l.data) IS NOT 'object'
   OR json_type(l.data,'$.expense_account_id') IS NULL
   OR json_type(l.data,'$.account_snapshot') IS NOT 'object'
   OR json_type(l.data,'$.item_snapshot') IS NOT 'object' OR NOT fb_invoice_line_ok(l.data)
 UNION ALL
 SELECT 'receipt snapshot: '||r.id FROM sales_receipt r
 WHERE NOT json_valid(r.data) OR json_type(r.data) IS NOT 'object'
   OR json_type(r.data,'$.customer_snapshot') IS NOT 'object'
   OR json_type(r.data,'$.company_snapshot') IS NOT 'object'
   OR json_extract(r.data,'$.company_snapshot.id') IS NOT r.company_id
   OR NOT fb_sales_header_ok('receipt',r.data)
 UNION ALL
 SELECT 'sale snapshot: '||s.id FROM sales_salesreceipt s
 WHERE NOT json_valid(s.data) OR json_type(s.data) IS NOT 'object'
   OR json_type(s.data,'$.tax_snapshot') IS NOT 'object' OR NOT fb_sales_snapshot_ok('detail',s.data)
 UNION ALL
 SELECT 'sale line snapshot: '||l.id FROM sales_salesreceiptline l
 WHERE NOT json_valid(l.data) OR json_type(l.data) IS NOT 'object'
   OR json_type(l.data,'$.account_snapshot') IS NOT 'object'
   OR json_type(l.data,'$.item_snapshot') IS NOT 'object' OR NOT fb_sales_line_ok(l.data)
 UNION ALL
 SELECT 'receipt reversal snapshot: '||z.id FROM sales_receiptreversal z
 WHERE NOT json_valid(z.data) OR json_type(z.data) IS NOT 'object' OR NOT fb_sales_reversal_ok(z.data)
   OR json_extract(z.data,'$.id') IS NOT z.id;
DROP VIEW sales_expected_entries;
CREATE VIEW sales_expected_entries AS
 SELECT i.identity_id source_id,r.journal_id,d.account_id,r.total debit,0 credit FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.state='OPEN' JOIN documents_documentaccount d ON d.source_id=i.identity_id AND d.role='receivables'
 UNION ALL SELECT i.identity_id,r.journal_id,l.income_account_id,max(0,-sum(CASE WHEN json_extract(l.data,'$.line_kind')='DISCOUNT' THEN -l.amount ELSE l.amount END)),max(0,sum(CASE WHEN json_extract(l.data,'$.line_kind')='DISCOUNT' THEN -l.amount ELSE l.amount END)) FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.state='OPEN' JOIN sales_invoiceline l ON l.revision_id=r.id WHERE coalesce(json_extract(l.data,'$.line_kind'),'CHARGE')<>'NOTE' GROUP BY r.id,l.income_account_id HAVING sum(CASE WHEN json_extract(l.data,'$.line_kind')='DISCOUNT' THEN -l.amount ELSE l.amount END)<>0
 UNION ALL SELECT i.identity_id,r.journal_id,d.account_id,0,fb_sales_money(json_quote(json_extract(r.data,'$.tax'))) FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.state='OPEN' JOIN documents_documentaccount d ON d.source_id=i.identity_id AND d.role='collected_tax' WHERE fb_sales_money(json_quote(json_extract(r.data,'$.tax')))>0
 UNION ALL SELECT r.identity_id,r.journal_id,r.destination_account_id,r.amount,0 FROM sales_receipt r
 UNION ALL SELECT r.identity_id,r.journal_id,a.receivables_id,0,sum(a.amount) FROM sales_receipt r JOIN sales_receiptallocation a ON a.receipt_id=r.id WHERE r.kind='RCP' GROUP BY r.id,a.receivables_id
 UNION ALL SELECT r.identity_id,r.journal_id,l.income_account_id,0,sum(l.amount) FROM sales_receipt r JOIN sales_salesreceipt s ON s.receipt_id=r.id JOIN sales_salesreceiptline l ON l.sale_id=s.id GROUP BY r.id,l.income_account_id HAVING sum(l.amount)>0
 UNION ALL SELECT r.identity_id,r.journal_id,json_extract(s.data,'$.collected_tax_id'),0,fb_sales_money(json_quote(json_extract(s.data,'$.tax'))) FROM sales_receipt r JOIN sales_salesreceipt s ON s.receipt_id=r.id WHERE fb_sales_money(json_quote(json_extract(s.data,'$.tax')))>0 UNION ALL SELECT r.identity_id,r.journal_id,d.liability_id,0,r.amount FROM sales_receipt r JOIN prepayments_customerdeposit d ON d.receipt_id=r.id;
DROP VIEW sales_expected_references;
CREATE VIEW sales_expected_references AS
 SELECT i.identity_id source_id,'receivables' role,d.account_id FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.state='OPEN' JOIN documents_documentaccount d ON d.source_id=i.identity_id AND d.role='receivables'
 UNION ALL SELECT i.identity_id,'income:'||l.position,l.income_account_id FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.state='OPEN' JOIN sales_invoiceline l ON l.revision_id=r.id WHERE coalesce(json_extract(l.data,'$.line_kind'),'CHARGE')<>'NOTE'
 UNION ALL SELECT i.identity_id,'collected_tax',d.account_id FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.state='OPEN' JOIN documents_documentaccount d ON d.source_id=i.identity_id AND d.role='collected_tax' WHERE fb_sales_money(json_quote(json_extract(r.data,'$.tax')))>0
 UNION ALL SELECT identity_id,'destination',destination_account_id FROM sales_receipt
 UNION SELECT r.identity_id,'receivables:'||a.receivables_id,a.receivables_id FROM sales_receipt r JOIN sales_receiptallocation a ON a.receipt_id=r.id
 UNION SELECT r.identity_id,'income:'||l.income_account_id,l.income_account_id FROM sales_receipt r JOIN sales_salesreceipt s ON s.receipt_id=r.id JOIN sales_salesreceiptline l ON l.sale_id=s.id
 UNION ALL SELECT r.identity_id,'collected_tax',json_extract(s.data,'$.collected_tax_id') FROM sales_receipt r JOIN sales_salesreceipt s ON s.receipt_id=r.id WHERE json_extract(s.data,'$.collected_tax_id') IS NOT NULL UNION ALL SELECT r.identity_id,'customer_deposits',d.liability_id FROM sales_receipt r JOIN prepayments_customerdeposit d ON d.receipt_id=r.id;
