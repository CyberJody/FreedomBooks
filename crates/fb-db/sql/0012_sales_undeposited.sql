-- Extend the L2a control-role hook once receipts exist; historical destinations survive setup changes.
DROP VIEW fb_control_accounts;
CREATE VIEW fb_control_accounts AS
SELECT company_id, receivables_id account_id, 'receivables' role, 0 priority FROM documents_accountingsetup WHERE receivables_id IS NOT NULL
UNION SELECT company_id,payables_id,'payables',1 FROM documents_accountingsetup WHERE payables_id IS NOT NULL
UNION SELECT company_id,undeposited_id,'undeposited',2 FROM documents_accountingsetup WHERE undeposited_id IS NOT NULL
UNION SELECT company_id,collected_tax_id,'collected_tax',3 FROM documents_accountingsetup WHERE collected_tax_id IS NOT NULL
UNION SELECT company_id,customer_deposits_id,'customer_deposits',4 FROM documents_accountingsetup WHERE customer_deposits_id IS NOT NULL
UNION SELECT s.company_id,d.account_id,substr(d.role,1,instr(d.role||':',':')-1),
 CASE substr(d.role,1,instr(d.role||':',':')-1) WHEN 'receivables' THEN 0 WHEN 'payables' THEN 1 WHEN 'undeposited' THEN 2 WHEN 'collected_tax' THEN 3 ELSE 4 END
FROM documents_documentaccount d JOIN documents_documentidentity s ON s.id=d.source_id
WHERE substr(d.role,1,instr(d.role||':',':')-1) IN('receivables','payables','undeposited','collected_tax','customer_deposits')
UNION SELECT r.company_id,r.destination_account_id,'undeposited',2
FROM sales_receipt r WHERE r.destination='UNDEPOSITED';
