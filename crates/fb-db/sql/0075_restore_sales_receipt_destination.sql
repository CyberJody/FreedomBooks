-- 0070 rebuilds sales_receipt, which drops the sibling sales-review trigger.
-- Restore its reviewed definition unchanged after all receipt rebuilds.
CREATE TRIGGER fb_sales_receipt_destination BEFORE INSERT ON sales_receipt
BEGIN
 SELECT RAISE(ABORT,'receipt_destination: Choose a configured receipt destination')
 WHERE NOT EXISTS(SELECT 1 FROM documents_accountingsetup s
   JOIN ledger_account a ON a.id=NEW.destination_account_id
   WHERE s.company_id=NEW.company_id AND a.company_id=s.company_id
     AND a.kind='ASSET' AND a.active=1 AND a.postable=1
     AND ((NEW.destination='BANK' AND EXISTS(SELECT 1 FROM documents_designatedbank b
           WHERE b.setup_id=s.id AND b.account_id=a.id))
       OR (NEW.destination='UNDEPOSITED' AND s.undeposited_id=a.id)));
END;
