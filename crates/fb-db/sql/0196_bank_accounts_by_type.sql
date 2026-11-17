-- Every active, postable asset account of detail type Bank is a bank account for checks, bill
-- payments, deposits, transfers and refunds, without a separate designation step. Add the ones a
-- book has not listed yet. Accounts used in an accounting role (receivables, undeposited funds…),
-- now or by posted documents, stay out. Nothing else changes: the guards that allow bank
-- documents only on listed bank accounts are kept as they are.
INSERT INTO documents_designatedbank(id, setup_id, account_id)
SELECT lower(substr(h, 1, 8) || '-' || substr(h, 9, 4) || '-7' || substr(h, 14, 3) || '-' || substr('89AB', 1 + (unicode(substr(h, 17, 1)) % 4), 1) || substr(h, 18, 3) || '-' || substr(h, 21, 12)),
       setup_id, account_id
FROM (
  SELECT hex(randomblob(16)) AS h, s.id AS setup_id, a.id AS account_id
  FROM documents_accountingsetup s
  JOIN ledger_account a ON a.company_id = s.company_id
  WHERE a.detail_type = 'BANK' AND a.kind = 'ASSET' AND a.active = 1 AND a.postable = 1
    AND a.id NOT IN (coalesce(s.receivables_id, ''), coalesce(s.payables_id, ''), coalesce(s.undeposited_id, ''), coalesce(s.collected_tax_id, ''), coalesce(s.customer_deposits_id, ''))
    AND NOT EXISTS (SELECT 1 FROM documents_designatedbank d WHERE d.setup_id = s.id AND d.account_id = a.id)
    AND NOT EXISTS (SELECT 1 FROM fb_control_accounts c WHERE c.company_id = s.company_id AND c.account_id = a.id)
);
