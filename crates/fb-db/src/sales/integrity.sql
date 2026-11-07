-- Read-only invariant scan after registering the exact-money callbacks.
-- Ineligible (archived) historical selections are valid; foreign/missing parents are not.
SELECT 'invoice ownership: '||i.id FROM sales_invoice i
LEFT JOIN documents_documentidentity d ON d.id=i.identity_id
WHERE d.id IS NULL OR d.company_id<>i.company_id OR d.kind<>'INV'
 OR i.revision<>(SELECT max(number) FROM sales_invoicerevision WHERE invoice_id=i.id)
 OR i.revision<>(SELECT count(*) FROM sales_invoicerevision WHERE invoice_id=i.id)
UNION ALL
SELECT 'invoice revision scope: '||r.id FROM sales_invoicerevision r
JOIN sales_invoice i ON i.id=r.invoice_id LEFT JOIN directory_customer c ON c.id=r.customer_id
WHERE c.id IS NULL OR c.company_id<>i.company_id
 OR json_extract(r.data,'$.id') IS NOT r.id OR json_extract(r.data,'$.number') IS NOT r.number
 OR json_extract(r.data,'$.customer_id') IS NOT r.customer_id OR json_extract(r.data,'$.state') IS NOT r.state
 OR json_extract(r.data,'$.date') IS NOT r.date OR json_extract(r.data,'$.due_date') IS NOT r.due_date
 OR json_extract(r.data,'$.journal_id') IS NOT r.journal_id OR json_extract(r.data,'$.reversal_id') IS NOT r.reversal_id
 OR NOT fb_sales_header_ok('invoice',r.data)
UNION ALL
SELECT 'invoice line scope: '||l.id FROM sales_invoiceline l
JOIN sales_invoicerevision r ON r.id=l.revision_id JOIN sales_invoice i ON i.id=r.invoice_id
LEFT JOIN directory_item t ON t.id=l.item_id
LEFT JOIN ledger_account a ON a.id=l.income_account_id LEFT JOIN ledger_account e ON e.id=l.expense_account_id
WHERE (l.item_id IS NOT NULL AND (t.id IS NULL OR t.company_id<>i.company_id))
 OR (l.income_account_id IS NOT NULL AND (a.id IS NULL OR a.company_id<>i.company_id))
 OR (l.expense_account_id IS NOT NULL AND (e.id IS NULL OR e.company_id<>i.company_id))
 OR json_extract(l.data,'$.position') IS NOT l.position OR json_extract(l.data,'$.item_id') IS NOT l.item_id
 OR json_extract(l.data,'$.income_account_id') IS NOT l.income_account_id OR json_extract(l.data,'$.expense_account_id') IS NOT l.expense_account_id
 OR json_extract(l.data,'$.taxable') IS NOT l.taxable OR fb_sales_money(json_quote(json_extract(l.data,'$.amount'))) IS NOT l.amount
 OR NOT fb_sales_line_ok(l.data)
UNION ALL
SELECT 'receipt ownership: '||r.id FROM sales_receipt r
LEFT JOIN documents_documentidentity d ON d.id=r.identity_id LEFT JOIN directory_customer c ON c.id=r.customer_id
LEFT JOIN ledger_account a ON a.id=r.destination_account_id LEFT JOIN ledger_journal j ON j.id=r.journal_id
WHERE d.id IS NULL OR d.company_id<>r.company_id OR d.kind<>r.kind
 OR c.id IS NULL OR c.company_id<>r.company_id OR a.id IS NULL OR a.company_id<>r.company_id OR a.kind<>'ASSET'
 OR j.id IS NULL OR j.status<>'POSTED' OR j.source_id IS NOT r.identity_id OR j.company_id<>r.company_id OR j.date<>r.date OR j.reversal_of_id IS NOT NULL
 OR json_extract(r.data,'$.id') IS NOT r.id OR json_extract(r.data,'$.identity_id') IS NOT r.identity_id
 OR json_extract(r.data,'$.customer_id') IS NOT r.customer_id OR json_extract(r.data,'$.kind') IS NOT r.kind
 OR json_extract(r.data,'$.date') IS NOT r.date OR json_extract(r.data,'$.destination') IS NOT r.destination
 OR json_extract(r.data,'$.destination_account_id') IS NOT r.destination_account_id OR json_extract(r.data,'$.journal_id') IS NOT r.journal_id
 OR fb_sales_money(json_quote(json_extract(r.data,'$.amount'))) IS NOT r.amount OR NOT fb_sales_header_ok('receipt',r.data)
UNION ALL
SELECT 'receipt completeness: '||r.id FROM sales_receipt r
WHERE (r.kind='RCP' AND (EXISTS(SELECT 1 FROM sales_salesreceipt WHERE receipt_id=r.id)
 OR (SELECT count(*) FROM sales_receiptallocation WHERE receipt_id=r.id) NOT BETWEEN 1 AND 50
 OR (SELECT coalesce(sum(amount),0) FROM sales_receiptallocation WHERE receipt_id=r.id)<>r.amount))
 OR (r.kind='SR' AND (EXISTS(SELECT 1 FROM sales_receiptallocation WHERE receipt_id=r.id)
 OR NOT EXISTS(SELECT 1 FROM sales_salesreceipt WHERE receipt_id=r.id AND total=r.amount)))
UNION ALL
SELECT 'allocation scope: '||a.id FROM sales_receiptallocation a
JOIN sales_receipt r ON r.id=a.receipt_id JOIN sales_invoice i ON i.id=a.invoice_id JOIN sales_invoicerevision v ON v.id=a.invoice_revision_id
WHERE r.kind<>'RCP' OR r.company_id<>i.company_id OR v.invoice_id<>i.id OR v.customer_id<>r.customer_id OR v.state<>'OPEN' OR v.date>r.date
 OR a.receivables_id=r.destination_account_id OR NOT EXISTS(SELECT 1 FROM documents_documentaccount d WHERE d.source_id=i.identity_id AND d.role='receivables' AND d.account_id=a.receivables_id)
 OR (NOT EXISTS(SELECT 1 FROM sales_receiptreversal z WHERE z.receipt_id=r.id) AND (
 NOT EXISTS(SELECT 1 FROM sales_invoicerevision current WHERE current.invoice_id=i.id AND current.number=i.revision AND current.state='OPEN')
 OR (SELECT sum(x.amount) FROM sales_receiptallocation x WHERE x.invoice_id=i.id AND NOT EXISTS(SELECT 1 FROM sales_receiptreversal z WHERE z.receipt_id=x.receipt_id))>v.total))
UNION ALL
SELECT 'receipt reversal link: '||z.id FROM sales_receiptreversal z
JOIN sales_receipt r ON r.id=z.receipt_id LEFT JOIN ledger_journal j ON j.id=z.journal_id
WHERE j.id IS NULL OR j.source_id IS NOT r.identity_id OR j.company_id<>r.company_id OR j.status<>'POSTED' OR j.reversal_of_id IS NOT r.journal_id OR j.date<>z.date OR z.date<r.date
 OR json_extract(z.data,'$.date') IS NOT z.date OR json_extract(z.data,'$.journal_id') IS NOT z.journal_id OR json_extract(z.data,'$.receipt_id') IS NOT r.id
 OR length(trim(json_extract(z.data,'$.reason'))) NOT BETWEEN 1 AND 500
UNION ALL
SELECT 'sales line scope: '||l.id FROM sales_salesreceiptline l JOIN sales_salesreceipt s ON s.id=l.sale_id JOIN sales_receipt r ON r.id=s.receipt_id
LEFT JOIN ledger_account a ON a.id=l.income_account_id LEFT JOIN directory_item t ON t.id=l.item_id
WHERE a.id IS NULL OR a.company_id<>r.company_id OR a.kind<>'INCOME' OR (l.item_id IS NOT NULL AND (t.id IS NULL OR t.company_id<>r.company_id))
 OR json_extract(l.data,'$.position') IS NOT l.position OR json_extract(l.data,'$.item_id') IS NOT l.item_id
 OR json_extract(l.data,'$.income_account_id') IS NOT l.income_account_id OR json_extract(l.data,'$.taxable') IS NOT l.taxable
 OR fb_sales_money(json_quote(json_extract(l.data,'$.amount'))) IS NOT l.amount OR NOT fb_sales_line_ok(l.data)
UNION ALL SELECT 'sales account evidence: '||id FROM sales_reference_mismatch
UNION ALL SELECT 'sales tax scope: '||company_id FROM sales_tax_scope_violations;
