-- Reopen must require the same ownership and account evidence as the retained insert guards.
DROP VIEW salestax_payment_invalid;
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
  OR NOT EXISTS(SELECT 1 FROM documents_documentaccount WHERE source_id=t.identity_id AND role='bank' AND account_id=t.bank_id)
  OR (t.tax_amount>0 AND NOT EXISTS(SELECT 1 FROM documents_documentaccount WHERE source_id=t.identity_id AND role='collected_tax' AND account_id=t.tax_account_id))
  OR (t.penalty_amount>0 AND NOT EXISTS(SELECT 1 FROM documents_documentaccount WHERE source_id=t.identity_id AND role='expense' AND account_id=t.expense_account_id))
  OR NOT EXISTS(SELECT 1 FROM fb_seal WHERE kind='salestax_journal' AND doc_id=t.journal_id)
 UNION ALL SELECT v.id FROM salestax_paymentvoid v LEFT JOIN salestax_payment t ON t.id=v.payment_id WHERE t.id IS NULL OR v.date<t.date
  OR NOT EXISTS(SELECT 1 FROM ledger_journal j WHERE j.id=v.journal_id AND j.reversal_of_id=t.journal_id AND j.source_id=t.identity_id AND j.status='POSTED' AND j.date=v.date)
  OR NOT EXISTS(SELECT 1 FROM fb_seal WHERE kind='salestax_journal' AND doc_id=v.journal_id)
 UNION ALL SELECT j.id FROM ledger_journal j JOIN documents_documentidentity i ON i.id=j.source_id WHERE i.kind='STP' AND (
  NOT EXISTS(SELECT 1 FROM fb_seal WHERE kind='salestax_journal' AND doc_id=j.id)
  OR NOT EXISTS(SELECT 1 FROM salestax_payment t WHERE t.identity_id=j.source_id AND t.company_id=j.company_id AND j.status='POSTED' AND
   ((j.reversal_of_id IS NULL AND t.journal_id=j.id) OR (t.journal_id=j.reversal_of_id AND EXISTS(SELECT 1 FROM salestax_paymentvoid v WHERE v.payment_id=t.id AND v.journal_id=j.id)))));
