SELECT 'posting '||id FROM prepayments_journal_mismatch
UNION ALL SELECT 'overused '||r.id FROM sales_receipt r WHERE r.kind='CDP' AND
 NOT fb_pp_within(r.amount,0,(SELECT json_group_array(amount) FROM prepayments_live_use WHERE deposit_id=r.id))
UNION ALL SELECT 'missing detail '||r.id FROM sales_receipt r WHERE r.kind='CDP' AND NOT EXISTS(SELECT 1 FROM prepayments_customerdeposit d WHERE d.receipt_id=r.id AND d.company_id=r.company_id)
UNION ALL SELECT 'unsealed '||j.id FROM ledger_journal j JOIN documents_documentidentity d ON d.id=j.source_id WHERE d.kind IN('CDP','CDA','CDR') AND NOT EXISTS(SELECT 1 FROM prepayments_seal s WHERE s.kind='journal' AND s.doc_id=j.id)
UNION ALL SELECT 'void deposit in use '||z.receipt_id FROM sales_receiptreversal z WHERE EXISTS(SELECT 1 FROM prepayments_live_use WHERE deposit_id=z.receipt_id)
UNION ALL SELECT 'oversettled '||i.id FROM sales_invoice i JOIN sales_invoicerevision v ON v.invoice_id=i.id AND v.number=i.revision WHERE NOT fb_pp_within(v.total,0,(SELECT json_group_array(amount) FROM prepayments_live_settlements WHERE invoice_id=i.id))
UNION ALL SELECT 'void invoice in use '||i.id FROM sales_invoice i JOIN sales_invoicerevision v ON v.invoice_id=i.id AND v.number=i.revision WHERE v.state='VOID' AND EXISTS(SELECT 1 FROM prepayments_depositapplication a WHERE a.invoice_id=i.id AND NOT EXISTS(SELECT 1 FROM prepayments_depositunapplication u WHERE u.application_id=a.id))
UNION ALL SELECT 'depositapplication fields '||r.id FROM prepayments_depositapplication r WHERE json_extract(r.data,'$.id') IS NOT r.id OR json_extract(r.data,'$.company_id') IS NOT r.company_id OR json_extract(r.data,'$.identity_id') IS NOT r.identity_id OR json_extract(r.data,'$.deposit_id') IS NOT r.deposit_id OR json_extract(r.data,'$.invoice_id') IS NOT r.invoice_id OR json_extract(r.data,'$.invoice_revision_id') IS NOT r.invoice_revision_id OR json_extract(r.data,'$.receivables_id') IS NOT r.receivables_id OR json_extract(r.data,'$.liability_id') IS NOT r.liability_id OR json_extract(r.data,'$.date') IS NOT r.date OR json_extract(r.data,'$.journal_id') IS NOT r.journal_id OR json_extract(r.data,'$.actor_id') IS NOT r.actor_id OR fb_sales_money(json_quote(json_extract(r.data,'$.amount'))) IS NOT r.amount OR NOT fb_pp_fields(r.data)
UNION ALL SELECT 'depositrefund fields '||r.id FROM prepayments_depositrefund r WHERE json_extract(r.data,'$.id') IS NOT r.id OR json_extract(r.data,'$.company_id') IS NOT r.company_id OR json_extract(r.data,'$.identity_id') IS NOT r.identity_id OR json_extract(r.data,'$.deposit_id') IS NOT r.deposit_id OR json_extract(r.data,'$.customer_id') IS NOT r.customer_id OR json_extract(r.data,'$.bank_id') IS NOT r.bank_id OR json_extract(r.data,'$.liability_id') IS NOT r.liability_id OR json_extract(r.data,'$.date') IS NOT r.date OR json_extract(r.data,'$.method') IS NOT r.method OR json_extract(r.data,'$.check_number') IS NOT r.check_number OR json_extract(r.data,'$.journal_id') IS NOT r.journal_id OR json_extract(r.data,'$.actor_id') IS NOT r.actor_id OR fb_sales_money(json_quote(json_extract(r.data,'$.amount'))) IS NOT r.amount OR NOT fb_pp_fields(r.data)
UNION ALL SELECT 'application scope '||a.id FROM prepayments_depositapplication a
 LEFT JOIN sales_receipt r ON r.id=a.deposit_id LEFT JOIN prepayments_customerdeposit d ON d.receipt_id=r.id
 LEFT JOIN documents_documentidentity identity ON identity.id=a.identity_id LEFT JOIN ledger_journal j ON j.id=a.journal_id
 LEFT JOIN sales_invoice i ON i.id=a.invoice_id LEFT JOIN sales_invoicerevision v ON v.id=a.invoice_revision_id
 WHERE r.id IS NULL OR d.id IS NULL OR r.company_id<>a.company_id OR d.liability_id<>a.liability_id OR r.kind<>'CDP'
 OR identity.id IS NULL OR identity.company_id<>a.company_id OR identity.kind<>'CDA'
 OR j.id IS NULL OR j.company_id<>a.company_id OR j.source_id IS NOT a.identity_id OR j.status<>'POSTED' OR j.date<>a.date OR j.reversal_of_id IS NOT NULL
 OR i.id IS NULL OR v.id IS NULL OR i.company_id<>a.company_id OR v.invoice_id<>i.id OR v.state<>'OPEN' OR v.customer_id<>r.customer_id OR v.date>a.date OR r.date>a.date
 OR NOT EXISTS(SELECT 1 FROM documents_documentaccount ar WHERE ar.source_id=i.identity_id AND ar.role='receivables' AND ar.account_id=a.receivables_id)
 OR (NOT EXISTS(SELECT 1 FROM prepayments_depositunapplication u WHERE u.application_id=a.id) AND (EXISTS(SELECT 1 FROM sales_receiptreversal z WHERE z.receipt_id=r.id) OR v.number<>i.revision))
UNION ALL SELECT 'refund scope '||f.id FROM prepayments_depositrefund f
 LEFT JOIN sales_receipt r ON r.id=f.deposit_id LEFT JOIN prepayments_customerdeposit d ON d.receipt_id=r.id
 LEFT JOIN documents_documentidentity i ON i.id=f.identity_id LEFT JOIN ledger_journal j ON j.id=f.journal_id
 LEFT JOIN ledger_account b ON b.id=f.bank_id
 WHERE r.id IS NULL OR d.id IS NULL OR r.kind<>'CDP' OR r.company_id<>f.company_id OR r.customer_id<>f.customer_id OR d.liability_id<>f.liability_id OR r.date>f.date
 OR i.id IS NULL OR i.company_id<>f.company_id OR i.kind<>'CDR' OR b.id IS NULL OR b.company_id<>f.company_id OR b.kind<>'ASSET'
 OR j.id IS NULL OR j.company_id<>f.company_id OR j.source_id IS NOT f.identity_id OR j.status<>'POSTED' OR j.date<>f.date OR j.reversal_of_id IS NOT NULL
 OR (NOT EXISTS(SELECT 1 FROM prepayments_depositrefundvoid v WHERE v.refund_id=f.id) AND EXISTS(SELECT 1 FROM sales_receiptreversal z WHERE z.receipt_id=r.id))
UNION ALL SELECT 'detail scope '||d.id FROM prepayments_customerdeposit d LEFT JOIN sales_receipt r ON r.id=d.receipt_id LEFT JOIN ledger_account a ON a.id=d.liability_id
 WHERE r.id IS NULL OR r.kind<>'CDP' OR r.company_id<>d.company_id OR a.id IS NULL OR a.company_id<>d.company_id OR a.kind<>'LIABILITY'
UNION ALL SELECT 'early deposit reversal '||z.id FROM sales_receiptreversal z WHERE EXISTS(SELECT 1 FROM prepayments_releases r WHERE r.deposit_id=z.receipt_id AND r.date>z.date)

UNION ALL SELECT 'depositunapplication link '||v.id FROM prepayments_depositunapplication v LEFT JOIN prepayments_depositapplication p ON p.id=v.application_id LEFT JOIN ledger_journal j ON j.id=v.journal_id
 WHERE p.id IS NULL OR j.id IS NULL OR j.source_id IS NOT p.identity_id OR j.company_id<>p.company_id OR j.status<>'POSTED' OR j.reversal_of_id IS NOT p.journal_id OR j.date<>v.date OR v.date<p.date
 OR NOT fb_pp_reversal_fields(v.data,v.id,v.application_id,v.journal_id,v.date) OR json_extract(v.data,'$.actor_id') IS NOT v.actor_id

UNION ALL SELECT 'depositrefundvoid link '||v.id FROM prepayments_depositrefundvoid v LEFT JOIN prepayments_depositrefund p ON p.id=v.refund_id LEFT JOIN ledger_journal j ON j.id=v.journal_id
 WHERE p.id IS NULL OR j.id IS NULL OR j.source_id IS NOT p.identity_id OR j.company_id<>p.company_id OR j.status<>'POSTED' OR j.reversal_of_id IS NOT p.journal_id OR j.date<>v.date OR v.date<p.date
 OR NOT fb_pp_reversal_fields(v.data,v.id,v.refund_id,v.journal_id,v.date) OR json_extract(v.data,'$.actor_id') IS NOT v.actor_id
UNION ALL SELECT 'refund number collision '||r.id FROM prepayments_depositrefund r
 WHERE r.check_number<>'' AND NOT EXISTS(SELECT 1 FROM prepayments_depositrefundvoid v WHERE v.refund_id=r.id)
 AND (EXISTS(SELECT 1 FROM prepayments_depositrefund other WHERE other.id<>r.id
 AND other.company_id=r.company_id AND other.bank_id=r.bank_id AND other.check_number=r.check_number
 AND NOT EXISTS(SELECT 1 FROM prepayments_depositrefundvoid v WHERE v.refund_id=other.id))
 OR EXISTS(SELECT 1 FROM fb_payables_live_checks p WHERE p.company_id=r.company_id AND p.bank_id=r.bank_id AND p.number=r.check_number))
UNION ALL SELECT 'early invoice reversal '||v.id FROM sales_invoicerevision v JOIN ledger_journal j ON j.id=v.reversal_id
 WHERE v.state='VOID' AND EXISTS(SELECT 1 FROM prepayments_depositapplication a
 JOIN prepayments_depositunapplication u ON u.application_id=a.id WHERE a.invoice_id=v.invoice_id AND u.date>j.date)
UNION ALL SELECT 'invalid seal '||s.doc_id FROM prepayments_seal s WHERE s.kind<>'journal'
 OR NOT EXISTS(SELECT 1 FROM prepayments_due d WHERE d.kind=s.kind AND d.doc_id=s.doc_id)
 OR NOT EXISTS(SELECT 1 FROM ledger_journal j JOIN documents_documentidentity i ON i.id=j.source_id
 WHERE j.id=s.doc_id AND j.status='POSTED' AND i.kind IN('CDP','CDA','CDR'))
UNION ALL SELECT 'missing journal owner '||j.id FROM ledger_journal j JOIN documents_documentidentity i ON i.id=j.source_id
 WHERE i.kind IN('CDP','CDA','CDR') AND NOT (
 EXISTS(SELECT 1 FROM sales_receipt r JOIN prepayments_customerdeposit d ON d.receipt_id=r.id WHERE r.journal_id=j.id AND r.identity_id=j.source_id)
 OR EXISTS(SELECT 1 FROM sales_receiptreversal z JOIN sales_receipt r ON r.id=z.receipt_id WHERE z.journal_id=j.id AND r.kind='CDP' AND r.identity_id=j.source_id)
 OR EXISTS(SELECT 1 FROM prepayments_depositapplication a WHERE a.journal_id=j.id AND a.identity_id=j.source_id)
 OR EXISTS(SELECT 1 FROM prepayments_depositrefund r WHERE r.journal_id=j.id AND r.identity_id=j.source_id)
 OR EXISTS(SELECT 1 FROM prepayments_depositunapplication u JOIN prepayments_depositapplication a ON a.id=u.application_id WHERE u.journal_id=j.id AND a.identity_id=j.source_id)
 OR EXISTS(SELECT 1 FROM prepayments_depositrefundvoid v JOIN prepayments_depositrefund r ON r.id=v.refund_id WHERE v.journal_id=j.id AND r.identity_id=j.source_id))

UNION ALL SELECT 'deposit estimate scope '||d.id FROM prepayments_customerdeposit d
 WHERE d.estimate_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM sales_estimate e WHERE e.id=d.estimate_id AND e.company_id=d.company_id)
UNION ALL SELECT problem FROM prepayments_evidence_violations
;
