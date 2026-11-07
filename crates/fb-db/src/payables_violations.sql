-- Row invariants that schema hashes and foreign keys alone cannot establish.
-- All sums verify persisted integer evidence; exact multiplication stays in Rust.
SELECT 'line calculation:'||l.id FROM payables_billline l
WHERE NOT fb_payables_line_valid(l.quantity,l.rate,l.amount)
UNION ALL
SELECT 'bill current revision:'||b.id FROM payables_bill b
WHERE NOT EXISTS(SELECT 1 FROM payables_billrevision r JOIN documents_documentidentity i ON i.id=b.identity_id
 WHERE r.bill_id=b.id AND r.number=b.revision AND r.vendor_id=b.vendor_id AND r.state=b.state
 AND b.reference_key=fb_lower(trim(r.reference,char(32,9,10,13,12,11)))
 AND i.company_id=b.company_id AND i.kind='BILL')
 OR b.revision<>(SELECT count(*) FROM payables_billrevision WHERE bill_id=b.id)
UNION ALL
SELECT 'revision evidence:'||r.id FROM payables_billrevision r
WHERE NOT EXISTS(SELECT 1 FROM fb_seal WHERE kind='bill_revision' AND doc_id=r.bill_id||':'||r.number)
 OR (SELECT count(*) FROM payables_billline WHERE revision_id=r.id) NOT BETWEEN 1 AND 50
 OR (SELECT min(position) FROM payables_billline WHERE revision_id=r.id)<>1
 OR (SELECT max(position) FROM payables_billline WHERE revision_id=r.id)<>(SELECT count(*) FROM payables_billline WHERE revision_id=r.id)
 OR r.total<>coalesce((SELECT sum(amount) FROM payables_billline WHERE revision_id=r.id),0)
 OR NOT EXISTS(SELECT 1 FROM payables_bill b JOIN directory_vendor v ON v.company_id=b.company_id WHERE b.id=r.bill_id AND v.id=r.vendor_id)
 OR (r.state='DRAFT' AND (r.journal_id IS NOT NULL OR r.reversal_id IS NOT NULL OR r.review_digest<>''))
 OR (r.state IN('OPEN','VOID') AND NOT EXISTS(SELECT 1 FROM ledger_journal j JOIN payables_bill b ON b.identity_id=j.source_id WHERE b.id=r.bill_id AND j.id=r.journal_id AND j.status='POSTED' AND j.reversal_of_id IS NULL AND j.date=r.date))
 OR (r.state='OPEN' AND (length(r.review_digest)<>64 OR r.reversal_id IS NOT NULL))
 OR (r.state='VOID' AND NOT EXISTS(SELECT 1 FROM ledger_journal j WHERE j.id=r.reversal_id AND j.status='POSTED' AND j.reversal_of_id=r.journal_id))
UNION ALL
SELECT 'line parent scope:'||l.id FROM payables_billline l JOIN payables_billrevision r ON r.id=l.revision_id JOIN payables_bill b ON b.id=r.bill_id
WHERE (l.item_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM directory_item WHERE id=l.item_id AND company_id=b.company_id))
 OR (l.income_account_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=l.income_account_id AND company_id=b.company_id))
 OR (l.expense_account_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=l.expense_account_id AND company_id=b.company_id))
UNION ALL
SELECT 'payment evidence:'||p.id FROM payables_billpayment p
WHERE NOT EXISTS(SELECT 1 FROM fb_seal WHERE kind='bill_payment' AND doc_id=p.id)
 OR (SELECT count(*) FROM payables_paymentallocation WHERE payment_id=p.id) NOT BETWEEN 1 AND 50
 OR p.amount<>coalesce((SELECT sum(amount) FROM payables_paymentallocation WHERE payment_id=p.id),0)
 OR NOT EXISTS(SELECT 1 FROM ledger_journal j JOIN documents_documentidentity i ON i.id=j.source_id WHERE j.id=p.journal_id AND j.company_id=p.company_id AND j.status='POSTED' AND j.reversal_of_id IS NULL AND j.date=p.date AND i.id=p.identity_id AND i.kind='BPAY')
 OR NOT EXISTS(SELECT 1 FROM directory_vendor v JOIN ledger_account a ON a.company_id=v.company_id WHERE v.id=p.vendor_id AND a.id=p.bank_id AND a.company_id=p.company_id)
UNION ALL
SELECT 'allocation ownership:'||a.id FROM payables_paymentallocation a
WHERE NOT EXISTS(SELECT 1 FROM payables_billpayment p JOIN payables_bill b ON b.company_id=p.company_id
 JOIN payables_billrevision r ON r.bill_id=b.id JOIN documents_documentaccount d ON d.source_id=b.identity_id AND d.role='payables'
 WHERE p.id=a.payment_id AND b.id=a.bill_id AND r.id=a.bill_revision_id AND r.vendor_id=p.vendor_id AND r.state='OPEN' AND r.date<=p.date AND d.account_id=a.payables_id)
UNION ALL
SELECT 'live allocation balance:'||b.id FROM payables_bill b JOIN payables_billrevision r ON r.bill_id=b.id AND r.number=b.revision
WHERE (SELECT sum(amount) FROM fb_payables_live_allocations WHERE bill_id=b.id)>r.total
 OR (b.state<>'OPEN' AND EXISTS(SELECT 1 FROM fb_payables_live_allocations WHERE bill_id=b.id))
UNION ALL
SELECT 'payment reversal:'||z.id FROM payables_paymentreversal z
WHERE NOT EXISTS(SELECT 1 FROM payables_billpayment p JOIN ledger_journal j ON j.reversal_of_id=p.journal_id AND j.company_id=p.company_id AND j.source_id=p.identity_id
 WHERE p.id=z.payment_id AND j.id=z.journal_id AND j.status='POSTED' AND j.date=z.date AND z.date>=p.date)
UNION ALL
SELECT 'unowned payables journal:'||j.id FROM ledger_journal j JOIN documents_documentidentity i ON i.id=j.source_id
WHERE i.kind IN('BILL','BPAY') AND (
 NOT EXISTS(SELECT 1 FROM fb_seal WHERE kind='payables_journal' AND doc_id=j.id)
 OR NOT (EXISTS(SELECT 1 FROM payables_billrevision WHERE journal_id=j.id OR reversal_id=j.id)
 OR EXISTS(SELECT 1 FROM payables_billpayment WHERE journal_id=j.id)
 OR EXISTS(SELECT 1 FROM payables_paymentreversal WHERE journal_id=j.id)))
UNION ALL
SELECT 'billline snapshot shape:'||id FROM payables_billline
WHERE (CASE WHEN json_valid(item_snapshot) THEN json_type(item_snapshot) END) IS NOT 'object'
 OR (CASE WHEN json_valid(account_snapshot) THEN json_type(account_snapshot) END) IS NOT 'object'
 OR EXISTS(SELECT 1 FROM json_each(CASE WHEN json_valid(account_snapshot) THEN account_snapshot ELSE '{}' END) WHERE type<>'object')
UNION ALL
SELECT 'billrevision snapshot shape:'||id FROM payables_billrevision
WHERE (CASE WHEN json_valid(vendor_snapshot) THEN json_type(vendor_snapshot) END) IS NOT 'object'
 OR (CASE WHEN json_valid(company_snapshot) THEN json_type(company_snapshot) END) IS NOT 'object'
UNION ALL
SELECT 'billpayment snapshot shape:'||id FROM payables_billpayment
WHERE (CASE WHEN json_valid(vendor_snapshot) THEN json_type(vendor_snapshot) END) IS NOT 'object'
 OR (CASE WHEN json_valid(company_snapshot) THEN json_type(company_snapshot) END) IS NOT 'object'
UNION ALL
SELECT 'line origin:'||l.id FROM payables_billline l
WHERE l.origin_position IS NOT NULL AND (
 NOT EXISTS(SELECT 1 FROM payables_billrevision r
 JOIN payables_billrevision p ON p.bill_id=r.bill_id AND p.number=r.number-1
 JOIN payables_billline prior ON prior.revision_id=p.id AND prior.position=l.origin_position
 WHERE r.id=l.revision_id)
 OR EXISTS(SELECT 1 FROM payables_billline other WHERE other.revision_id=l.revision_id AND other.origin_position=l.origin_position AND other.id<>l.id))
