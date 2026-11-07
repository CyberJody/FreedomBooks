-- Extend exact open-time verification to banking relationships and optional statement entries.
CREATE VIEW banking_charges_invalid AS
SELECT r.id FROM banking_reconciliation r JOIN ledger_account a ON a.id=r.account_id
WHERE EXISTS(
 SELECT 1 FROM (SELECT 'service' prefix,'service_charge' amount,'EXPENSE' kind UNION ALL SELECT 'interest','interest','INCOME') x
 WHERE (json_extract(r.data,'$.'||x.amount) IS NULL AND (json_extract(r.data,'$.'||x.prefix||'_account_id') IS NOT NULL OR json_extract(r.data,'$.'||x.prefix||'_journal_id') IS NOT NULL))
 OR (json_extract(r.data,'$.'||x.amount) IS NOT NULL AND NOT EXISTS(
 SELECT 1 FROM ledger_journal j JOIN ledger_account b ON b.id=json_extract(r.data,'$.'||x.prefix||'_account_id')
 WHERE j.id=json_extract(r.data,'$.'||x.prefix||'_journal_id') AND j.company_id=r.company_id AND j.status='POSTED' AND j.source_id IS NULL AND j.reversal_of_id IS NULL
 AND b.company_id=r.company_id AND b.id<>a.id AND b.kind=x.kind
 AND fb_banking_money(json_quote(json_extract(r.data,'$.'||x.amount)))>0
 AND (x.prefix<>'interest' OR a.kind='ASSET')
 AND (SELECT count(*) FROM ledger_journalline WHERE journal_id=j.id)=2
 AND EXISTS(SELECT 1 FROM ledger_journalline l WHERE l.journal_id=j.id AND l.account_id=r.account_id AND l.debit=CASE WHEN x.prefix='interest' THEN fb_banking_money(json_quote(json_extract(r.data,'$.'||x.amount))) ELSE 0 END AND l.credit=CASE WHEN x.prefix='service' THEN fb_banking_money(json_quote(json_extract(r.data,'$.'||x.amount))) ELSE 0 END)
 AND EXISTS(SELECT 1 FROM ledger_journalline l WHERE l.journal_id=j.id AND l.account_id=b.id AND l.credit=CASE WHEN x.prefix='interest' THEN fb_banking_money(json_quote(json_extract(r.data,'$.'||x.amount))) ELSE 0 END AND l.debit=CASE WHEN x.prefix='service' THEN fb_banking_money(json_quote(json_extract(r.data,'$.'||x.amount))) ELSE 0 END)
 )));
CREATE TRIGGER fb_banking_charges_fixed BEFORE UPDATE ON banking_reconciliation BEGIN
 SELECT RAISE(ABORT,'reconciliation_charges_fixed: A recorded service charge or interest entry never changes') WHERE EXISTS(SELECT 1 FROM (SELECT 'service' prefix,'service_charge' amount UNION ALL SELECT 'interest','interest') x WHERE json_extract(OLD.data,'$.'||x.prefix||'_journal_id') IS NOT NULL AND (json_extract(OLD.data,'$.'||x.prefix||'_journal_id') IS NOT json_extract(NEW.data,'$.'||x.prefix||'_journal_id') OR json_extract(OLD.data,'$.'||x.amount) IS NOT json_extract(NEW.data,'$.'||x.amount) OR json_extract(OLD.data,'$.'||x.prefix||'_account_id') IS NOT json_extract(NEW.data,'$.'||x.prefix||'_account_id')));
END;
CREATE TRIGGER fb_banking_charges_seal BEFORE INSERT ON banking_seal WHEN NEW.kind='reconciliation' BEGIN
 SELECT RAISE(ABORT,'reconciliation_charge_posting: Invalid service charge or interest posting') WHERE EXISTS(SELECT 1 FROM banking_reconciliation r JOIN banking_charges_invalid v ON v.id=r.id WHERE r.id||':'||r.revision=NEW.doc_id);
END;
CREATE VIEW banking_relationship_invalid AS
SELECT d.id FROM banking_deposit d WHERE NOT EXISTS(SELECT 1 FROM documents_documentidentity i JOIN ledger_journal j ON j.id=d.journal_id WHERE i.id=d.identity_id AND i.company_id=d.company_id AND i.kind='DEP' AND j.company_id=d.company_id AND j.source_id=i.id AND j.status='POSTED' AND j.reversal_of_id IS NULL AND j.date=d.date)
UNION ALL SELECT l.id FROM banking_depositline l JOIN banking_deposit d ON d.id=l.deposit_id WHERE NOT EXISTS(SELECT 1 FROM sales_receipt r WHERE r.id=l.receipt_id AND r.company_id=d.company_id AND r.destination='UNDEPOSITED' AND l.undeposited_id=r.destination_account_id AND l.amount=r.amount AND r.date<=d.date AND l.undeposited_id<>d.bank_id)
UNION ALL SELECT l.id FROM banking_depositline l WHERE NOT EXISTS(SELECT 1 FROM banking_depositreversal WHERE deposit_id=l.deposit_id) AND (EXISTS(SELECT 1 FROM sales_receiptreversal WHERE receipt_id=l.receipt_id) OR EXISTS(SELECT 1 FROM banking_depositline x WHERE x.receipt_id=l.receipt_id AND x.deposit_id<>l.deposit_id AND NOT EXISTS(SELECT 1 FROM banking_depositreversal WHERE deposit_id=x.deposit_id)))
UNION ALL SELECT z.id FROM banking_depositreversal z JOIN banking_deposit d ON d.id=z.deposit_id WHERE z.date<d.date OR NOT EXISTS(SELECT 1 FROM ledger_journal j WHERE j.id=z.journal_id AND j.company_id=d.company_id AND j.source_id=d.identity_id AND j.status='POSTED' AND j.reversal_of_id=d.journal_id AND j.date=z.date)
UNION ALL SELECT s.id FROM banking_banklinestatus s WHERE NOT EXISTS(SELECT 1 FROM ledger_journalline l JOIN ledger_journal j ON j.id=l.journal_id JOIN ledger_account a ON a.id=l.account_id WHERE l.id=s.journal_line_id AND j.company_id=s.company_id AND j.status='POSTED' AND l.account_id=s.account_id AND a.company_id=s.company_id)
 OR (s.state='R' AND NOT EXISTS(SELECT 1 FROM banking_reconciliation r JOIN banking_reconciliationline l ON l.reconciliation_id=r.id WHERE r.id=s.reconciliation_id AND r.status='DONE' AND r.company_id=s.company_id AND r.account_id=s.account_id AND l.journal_line_id=s.journal_line_id))
UNION ALL SELECT l.id FROM banking_reconciliationline l WHERE NOT EXISTS(SELECT 1 FROM banking_reconciliation r JOIN ledger_journalline x ON x.id=l.journal_line_id JOIN ledger_journal j ON j.id=x.journal_id WHERE r.id=l.reconciliation_id AND r.status IN('DONE','UNDONE') AND j.status='POSTED' AND j.company_id=r.company_id AND x.account_id=r.account_id AND x.debit=l.debit AND x.credit=l.credit)
UNION ALL SELECT r.id FROM banking_reconciliation r WHERE NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=r.account_id AND company_id=r.company_id)
 OR json_extract(r.data,'$.id') IS NOT r.id OR json_extract(r.data,'$.company_id') IS NOT r.company_id OR json_extract(r.data,'$.account_id') IS NOT r.account_id OR json_extract(r.data,'$.status') IS NOT r.status OR json_extract(r.data,'$.statement_date') IS NOT r.statement_date OR json_extract(r.data,'$.revision') IS NOT r.revision OR fb_banking_money(json_quote(json_extract(r.data,'$.ending_balance'))) IS NOT r.ending_balance;
-- Referenced identities may not be deleted and reinserted within deferred-FK transactions.
CREATE TRIGGER fb_banking_account_kept BEFORE DELETE ON ledger_account WHEN EXISTS(SELECT 1 FROM banking_reconciliation WHERE account_id=OLD.id) OR EXISTS(SELECT 1 FROM banking_deposit WHERE bank_id=OLD.id) BEGIN SELECT RAISE(ABORT,'banking_parent_kept: Referenced banking parents cannot be deleted'); END;
CREATE TRIGGER fb_banking_user_kept BEFORE DELETE ON fb_user WHEN EXISTS(SELECT 1 FROM banking_reconciliation WHERE started_by_id=OLD.id) OR EXISTS(SELECT 1 FROM banking_banklinestatus WHERE changed_by_id=OLD.id) OR EXISTS(SELECT 1 FROM banking_deposit WHERE actor_id=OLD.id) OR EXISTS(SELECT 1 FROM banking_depositreversal WHERE actor_id=OLD.id) BEGIN SELECT RAISE(ABORT,'banking_parent_kept: Referenced banking parents cannot be deleted'); END;
