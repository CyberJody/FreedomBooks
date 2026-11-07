SELECT 'invalid creditmemo fields '||r.id FROM credits_creditmemo r WHERE json_extract(r.data,'$.id') IS NOT r.id OR json_extract(r.data,'$.company_id') IS NOT r.company_id OR json_extract(r.data,'$.identity_id') IS NOT r.identity_id OR json_extract(r.data,'$.customer_id') IS NOT r.customer_id OR json_extract(r.data,'$.date') IS NOT r.date OR json_extract(r.data,'$.receivables_id') IS NOT r.receivables_id OR json_extract(r.data,'$.collected_tax_id') IS NOT r.collected_tax_id OR json_extract(r.data,'$.tax_item_id') IS NOT r.tax_item_id OR fb_sales_money(json_quote(json_extract(r.data,'$.total'))) IS NOT r.total OR json_extract(r.data,'$.journal_id') IS NOT r.journal_id OR json_extract(r.data,'$.actor_id') IS NOT r.actor_id OR NOT fb_credits_fields('creditmemo',r.data)
UNION ALL
SELECT 'invalid creditmemoline fields '||r.id FROM credits_creditmemoline r WHERE json_extract(r.data,'$.id') IS NOT r.id OR json_extract(r.data,'$.credit_memo_id') IS NOT r.credit_memo_id OR json_extract(r.data,'$.position') IS NOT r.position OR json_extract(r.data,'$.item_id') IS NOT r.item_id OR json_extract(r.data,'$.income_account_id') IS NOT r.income_account_id OR fb_sales_money(json_quote(json_extract(r.data,'$.amount'))) IS NOT r.amount OR json_extract(r.data,'$.taxable') IS NOT r.taxable OR NOT fb_credits_fields('creditmemoline',r.data)
UNION ALL
SELECT 'invalid creditmemovoid fields '||r.id FROM credits_creditmemovoid r WHERE json_extract(r.data,'$.id') IS NOT r.id OR json_extract(r.data,'$.credit_memo_id') IS NOT r.credit_memo_id OR json_extract(r.data,'$.journal_id') IS NOT r.journal_id OR json_extract(r.data,'$.date') IS NOT r.date OR json_extract(r.data,'$.actor_id') IS NOT r.actor_id OR NOT fb_credits_fields('creditmemovoid',r.data)
UNION ALL
SELECT 'invalid creditapplication fields '||r.id FROM credits_creditapplication r WHERE json_extract(r.data,'$.id') IS NOT r.id OR json_extract(r.data,'$.company_id') IS NOT r.company_id OR json_extract(r.data,'$.credit_memo_id') IS NOT r.credit_memo_id OR json_extract(r.data,'$.request_key') IS NOT r.request_key OR json_extract(r.data,'$.date') IS NOT r.date OR fb_sales_money(json_quote(json_extract(r.data,'$.amount'))) IS NOT r.amount OR json_extract(r.data,'$.actor_id') IS NOT r.actor_id OR NOT fb_credits_fields('creditapplication',r.data)
UNION ALL
SELECT 'invalid creditapplicationline fields '||r.id FROM credits_creditapplicationline r WHERE json_extract(r.data,'$.id') IS NOT r.id OR json_extract(r.data,'$.application_id') IS NOT r.application_id OR json_extract(r.data,'$.invoice_id') IS NOT r.invoice_id OR json_extract(r.data,'$.invoice_revision_id') IS NOT r.invoice_revision_id OR json_extract(r.data,'$.receivables_id') IS NOT r.receivables_id OR fb_sales_money(json_quote(json_extract(r.data,'$.amount'))) IS NOT r.amount OR NOT fb_credits_fields('creditapplicationline',r.data)
UNION ALL
SELECT 'invalid creditunapplication fields '||r.id FROM credits_creditunapplication r WHERE json_extract(r.data,'$.id') IS NOT r.id OR json_extract(r.data,'$.application_id') IS NOT r.application_id OR json_extract(r.data,'$.date') IS NOT r.date OR json_extract(r.data,'$.actor_id') IS NOT r.actor_id OR NOT fb_credits_fields('creditunapplication',r.data)
UNION ALL
SELECT 'invalid creditrefund fields '||r.id FROM credits_creditrefund r WHERE json_extract(r.data,'$.id') IS NOT r.id OR json_extract(r.data,'$.company_id') IS NOT r.company_id OR json_extract(r.data,'$.identity_id') IS NOT r.identity_id OR json_extract(r.data,'$.credit_memo_id') IS NOT r.credit_memo_id OR json_extract(r.data,'$.customer_id') IS NOT r.customer_id OR json_extract(r.data,'$.date') IS NOT r.date OR fb_sales_money(json_quote(json_extract(r.data,'$.amount'))) IS NOT r.amount OR json_extract(r.data,'$.bank_id') IS NOT r.bank_id OR json_extract(r.data,'$.receivables_id') IS NOT r.receivables_id OR json_extract(r.data,'$.method') IS NOT r.method OR json_extract(r.data,'$.check_number') IS NOT r.check_number OR json_extract(r.data,'$.journal_id') IS NOT r.journal_id OR json_extract(r.data,'$.actor_id') IS NOT r.actor_id OR NOT fb_credits_fields('creditrefund',r.data)
UNION ALL
SELECT 'invalid creditrefundvoid fields '||r.id FROM credits_creditrefundvoid r WHERE json_extract(r.data,'$.id') IS NOT r.id OR json_extract(r.data,'$.refund_id') IS NOT r.refund_id OR json_extract(r.data,'$.journal_id') IS NOT r.journal_id OR json_extract(r.data,'$.date') IS NOT r.date OR json_extract(r.data,'$.actor_id') IS NOT r.actor_id OR NOT fb_credits_fields('creditrefundvoid',r.data)
UNION ALL
SELECT 'unsealed creditmemo '||r.id FROM credits_creditmemo r WHERE NOT EXISTS(SELECT 1 FROM credits_seal s WHERE s.kind='memo' AND s.doc_id=r.id)
UNION ALL
SELECT 'unsealed creditmemovoid '||r.id FROM credits_creditmemovoid r WHERE NOT EXISTS(SELECT 1 FROM credits_seal s WHERE s.kind='memo_void' AND s.doc_id=r.id)
UNION ALL
SELECT 'unsealed creditapplication '||r.id FROM credits_creditapplication r WHERE NOT EXISTS(SELECT 1 FROM credits_seal s WHERE s.kind='application' AND s.doc_id=r.id)
UNION ALL
SELECT 'unsealed creditunapplication '||r.id FROM credits_creditunapplication r WHERE NOT EXISTS(SELECT 1 FROM credits_seal s WHERE s.kind='unapplication' AND s.doc_id=r.id)
UNION ALL
SELECT 'unsealed creditrefund '||r.id FROM credits_creditrefund r WHERE NOT EXISTS(SELECT 1 FROM credits_seal s WHERE s.kind='refund' AND s.doc_id=r.id)
UNION ALL
SELECT 'unsealed creditrefundvoid '||r.id FROM credits_creditrefundvoid r WHERE NOT EXISTS(SELECT 1 FROM credits_seal s WHERE s.kind='refund_void' AND s.doc_id=r.id)
UNION ALL
SELECT 'credit memo overused '||id FROM credits_overused
UNION ALL
SELECT 'invoice overcredited '||id FROM credits_overcredited
UNION ALL
SELECT 'credit journal mismatch '||id FROM credits_journal_mismatch
UNION ALL
SELECT 'credit account mismatch '||id FROM credits_account_mismatch
UNION ALL
SELECT 'credit tax scope '||id FROM credits_tax_scope_violations
UNION ALL
SELECT 'credit memo scope '||m.id FROM credits_creditmemo m WHERE NOT EXISTS(SELECT 1 FROM directory_customer c WHERE c.id=m.customer_id AND c.company_id=m.company_id) OR NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=m.receivables_id AND a.company_id=m.company_id AND a.kind='ASSET') OR (m.collected_tax_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=m.collected_tax_id AND a.company_id=m.company_id AND a.kind='LIABILITY')) OR (fb_sales_money(json_quote(json_extract(m.data,'$.tax')))>0) IS NOT (m.collected_tax_id IS NOT NULL)
UNION ALL
SELECT 'credit memo line scope '||l.id FROM credits_creditmemoline l JOIN credits_creditmemo m ON m.id=l.credit_memo_id WHERE NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=l.income_account_id AND a.company_id=m.company_id AND a.kind='INCOME') OR (l.item_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM directory_item t WHERE t.id=l.item_id AND t.company_id=m.company_id)) OR NOT fb_sales_line_ok(l.data)
UNION ALL
SELECT 'credit application scope '||a.id FROM credits_creditapplication a JOIN credits_creditmemo m ON m.id=a.credit_memo_id WHERE a.company_id<>m.company_id OR a.date<m.date
UNION ALL
SELECT 'credit refund account scope '||r.id FROM credits_creditrefund r WHERE r.bank_id=r.receivables_id OR NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=r.bank_id AND a.company_id=r.company_id AND a.kind='ASSET') OR NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=r.receivables_id AND a.company_id=r.company_id AND a.kind='ASSET')
UNION ALL
SELECT 'orphan credit journal '||j.id FROM ledger_journal j JOIN documents_documentidentity d ON d.id=j.source_id WHERE d.kind IN('CM','RFD') AND (j.status<>'POSTED' OR NOT EXISTS(SELECT 1 FROM credits_seal s WHERE s.kind='journal' AND s.doc_id=j.id) OR NOT ((j.reversal_of_id IS NULL AND (EXISTS(SELECT 1 FROM credits_creditmemo m WHERE m.identity_id=j.source_id AND m.journal_id=j.id) OR EXISTS(SELECT 1 FROM credits_creditrefund r WHERE r.identity_id=j.source_id AND r.journal_id=j.id))) OR (j.reversal_of_id IS NOT NULL AND (EXISTS(SELECT 1 FROM credits_creditmemovoid v JOIN credits_creditmemo m ON m.id=v.credit_memo_id WHERE m.identity_id=j.source_id AND m.journal_id=j.reversal_of_id AND v.journal_id=j.id) OR EXISTS(SELECT 1 FROM credits_creditrefundvoid v JOIN credits_creditrefund r ON r.id=v.refund_id WHERE r.identity_id=j.source_id AND r.journal_id=j.reversal_of_id AND v.journal_id=j.id)))))
UNION ALL
SELECT 'credit memo arithmetic '||m.id FROM credits_creditmemo m WHERE NOT fb_sales_document_ok(m.data,(SELECT json_group_array(json(data)) FROM (SELECT data FROM credits_creditmemoline WHERE credit_memo_id=m.id ORDER BY position)),m.total)
UNION ALL
SELECT 'credit application arithmetic '||a.id FROM credits_creditapplication a WHERE (SELECT count(*) FROM credits_creditapplicationline l WHERE l.application_id=a.id) NOT BETWEEN 1 AND 50 OR fb_credits_sum((SELECT json_group_array(amount) FROM credits_creditapplicationline l WHERE l.application_id=a.id)) IS NOT a.amount
UNION ALL
SELECT 'credit application target '||l.id FROM credits_creditapplicationline l JOIN credits_creditapplication a ON a.id=l.application_id JOIN credits_creditmemo m ON m.id=a.credit_memo_id LEFT JOIN sales_invoice i ON i.id=l.invoice_id LEFT JOIN sales_invoicerevision r ON r.id=l.invoice_revision_id WHERE i.company_id IS NOT a.company_id OR r.invoice_id IS NOT i.id OR r.state IS NOT 'OPEN' OR r.customer_id IS NOT m.customer_id OR r.date>a.date OR a.date<m.date OR l.receivables_id IS NOT m.receivables_id OR NOT EXISTS(SELECT 1 FROM documents_documentaccount d WHERE d.source_id=i.identity_id AND d.role='receivables' AND d.account_id=l.receivables_id) OR (NOT EXISTS(SELECT 1 FROM credits_creditunapplication WHERE application_id=a.id) AND i.revision IS NOT r.number)
UNION ALL
SELECT 'credit unapply date '||u.id FROM credits_creditunapplication u JOIN credits_creditapplication a ON a.id=u.application_id WHERE u.date<a.date
UNION ALL
SELECT 'credit memo void date '||v.id FROM credits_creditmemovoid v JOIN credits_creditmemo m ON m.id=v.credit_memo_id WHERE v.date<m.date OR EXISTS(SELECT 1 FROM credits_releases x WHERE x.credit_memo_id=m.id AND x.date>v.date)
UNION ALL
SELECT 'credit refund date '||r.id FROM credits_creditrefund r JOIN credits_creditmemo m ON m.id=r.credit_memo_id WHERE r.date<m.date OR r.company_id<>m.company_id OR r.customer_id<>m.customer_id OR r.receivables_id<>m.receivables_id
UNION ALL
SELECT 'credit refund void date '||v.id FROM credits_creditrefundvoid v JOIN credits_creditrefund r ON r.id=v.refund_id WHERE v.date<r.date
UNION ALL
SELECT 'invoice credit release date '||r.id FROM sales_invoicerevision r JOIN ledger_journal j ON j.id=r.reversal_id WHERE r.state='VOID' AND EXISTS(SELECT 1 FROM credits_creditapplicationline l JOIN credits_creditunapplication u ON u.application_id=l.application_id WHERE l.invoice_id=r.invoice_id AND u.date>j.date)
UNION ALL
SELECT 'credit posting ownership '||r.id FROM credits_creditmemo r WHERE NOT EXISTS(SELECT 1 FROM documents_documentidentity d JOIN ledger_journal j ON j.id=r.journal_id WHERE d.id=r.identity_id AND d.kind='CM' AND d.company_id=r.company_id AND j.source_id=d.id AND j.company_id=r.company_id AND j.date=r.date AND j.status='POSTED' AND j.reversal_of_id IS NULL)
UNION ALL
SELECT 'credit posting ownership '||r.id FROM credits_creditrefund r WHERE NOT EXISTS(SELECT 1 FROM documents_documentidentity d JOIN ledger_journal j ON j.id=r.journal_id WHERE d.id=r.identity_id AND d.kind='RFD' AND d.company_id=r.company_id AND j.source_id=d.id AND j.company_id=r.company_id AND j.date=r.date AND j.status='POSTED' AND j.reversal_of_id IS NULL)
UNION ALL
SELECT 'credit reversal ownership '||v.id FROM credits_creditmemovoid v JOIN credits_creditmemo p ON p.id=v.credit_memo_id WHERE NOT EXISTS(SELECT 1 FROM ledger_journal j WHERE j.id=v.journal_id AND j.source_id=p.identity_id AND j.company_id=p.company_id AND j.status='POSTED' AND j.reversal_of_id=p.journal_id AND j.date=v.date)
UNION ALL
SELECT 'credit reversal ownership '||v.id FROM credits_creditrefundvoid v JOIN credits_creditrefund p ON p.id=v.refund_id WHERE NOT EXISTS(SELECT 1 FROM ledger_journal j WHERE j.id=v.journal_id AND j.source_id=p.identity_id AND j.company_id=p.company_id AND j.status='POSTED' AND j.reversal_of_id=p.journal_id AND j.date=v.date)
UNION ALL
SELECT 'duplicate live refund number '||r.id FROM credits_creditrefund r
 WHERE r.check_number<>'' AND NOT EXISTS(SELECT 1 FROM credits_creditrefundvoid v WHERE v.refund_id=r.id)
 AND (EXISTS(SELECT 1 FROM credits_creditrefund other
      WHERE other.id<>r.id AND other.company_id=r.company_id AND other.bank_id=r.bank_id
      AND other.check_number=r.check_number
      AND NOT EXISTS(SELECT 1 FROM credits_creditrefundvoid v WHERE v.refund_id=other.id))
   OR (r.check_number NOT GLOB '*[^0-9]*' AND EXISTS(SELECT 1 FROM payables_billpayment p
      WHERE p.company_id=r.company_id AND p.bank_id=r.bank_id AND p.method='CHECK'
      AND p.reference=r.check_number AND length(p.reference) BETWEEN 1 AND 20
      AND NOT EXISTS(SELECT 1 FROM payables_paymentreversal z WHERE z.payment_id=p.id))))
UNION ALL
SELECT problem FROM credits_snapshot_violations;
