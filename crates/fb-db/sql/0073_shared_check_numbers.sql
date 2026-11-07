-- One namespace. Credits registers by replacing the two empty provider views;
-- retain their column contract and add credits write guards using fb_check_number_live.
CREATE VIEW fb_check_number_credit_live AS SELECT '' AS company_id, '' AS bank_id,
 '' AS number, '' AS holder_id, '' AS identity_id, 'refund' AS kind, 2 AS priority WHERE 0;
CREATE VIEW fb_check_number_credit_used AS SELECT '' AS company_id, '' AS bank_id, '' AS number WHERE 0;
CREATE VIEW fb_check_number_live AS
 SELECT company_id,bank_id,number_key AS number,id AS holder_id,identity_id,'check' AS kind,1 AS priority
 FROM checks_check WHERE state<>'VOID' AND number_key<>''
 UNION ALL SELECT * FROM fb_check_number_credit_live
 UNION ALL SELECT p.company_id,p.bank_id,p.reference,p.id,p.identity_id,'bill payment',3
 FROM payables_billpayment p WHERE p.method='CHECK' AND length(p.reference) BETWEEN 1 AND 20
 AND p.reference NOT GLOB '*[^0-9]*' AND NOT EXISTS(SELECT 1 FROM payables_paymentreversal v WHERE v.payment_id=p.id)
 UNION ALL SELECT r.company_id,r.bank_id,r.check_number,r.id,r.identity_id,'deposit refund',4
 FROM prepayments_depositrefund r WHERE r.check_number<>''
 AND NOT EXISTS(SELECT 1 FROM prepayments_depositrefundvoid v WHERE v.refund_id=r.id);
CREATE VIEW fb_check_number_used AS
 SELECT company_id,bank_id,number_key AS number FROM checks_check WHERE number_key<>''
 UNION ALL SELECT * FROM fb_check_number_credit_used
 UNION ALL SELECT company_id,bank_id,reference FROM payables_billpayment
 WHERE method='CHECK' AND length(reference) BETWEEN 1 AND 20 AND reference NOT GLOB '*[^0-9]*'
 UNION ALL SELECT company_id,bank_id,check_number FROM prepayments_depositrefund WHERE check_number<>'';
-- Same-family check uniqueness retains the reviewed root/index guards and their
-- ownership-before-collision ordering. These boundaries cover all other families.
CREATE TRIGGER fb_number_checks_check_insert BEFORE INSERT ON checks_check
WHEN NEW.state<>'VOID' AND NEW.number_key<>'' BEGIN
 SELECT RAISE(ABORT,'check_payment_number: Check number already used by a live bill payment on this bank')
 WHERE EXISTS(SELECT 1 FROM fb_check_number_live h WHERE h.company_id=NEW.company_id
 AND h.bank_id=NEW.bank_id AND h.number=NEW.number_key AND h.kind<>'check'); END;
CREATE TRIGGER fb_number_checks_check_update BEFORE UPDATE ON checks_check
WHEN NEW.state<>'VOID' AND NEW.number_key<>'' BEGIN
 SELECT RAISE(ABORT,'check_payment_number: Check number already used by a live bill payment on this bank')
 WHERE EXISTS(SELECT 1 FROM fb_check_number_live h WHERE h.company_id=NEW.company_id
 AND h.bank_id=NEW.bank_id AND h.number=NEW.number_key AND h.kind<>'check'); END;
CREATE TRIGGER fb_number_payables_billpayment_insert BEFORE INSERT ON payables_billpayment
WHEN NEW.method='CHECK' AND length(NEW.reference) BETWEEN 1 AND 20 AND NEW.reference NOT GLOB '*[^0-9]*' BEGIN
 SELECT RAISE(ABORT,'bill_payment_deposit_number: Check number is already used by a deposit refund')
 WHERE EXISTS(SELECT 1 FROM fb_check_number_live WHERE company_id=NEW.company_id AND bank_id=NEW.bank_id AND number=NEW.reference AND kind='deposit refund');
 SELECT RAISE(ABORT,'check_payment_number: Check number already used on this bank')
 WHERE EXISTS(SELECT 1 FROM fb_check_number_live WHERE company_id=NEW.company_id AND bank_id=NEW.bank_id AND number=NEW.reference AND kind IN('check','refund'));
 SELECT RAISE(ABORT,'bill_payment_check_number: Check number already used on this bank')
 WHERE EXISTS(SELECT 1 FROM fb_check_number_live WHERE company_id=NEW.company_id AND bank_id=NEW.bank_id AND number=NEW.reference AND kind='bill payment');
END;
DROP TRIGGER pp_refund_scope;
CREATE TRIGGER pp_refund_scope BEFORE INSERT ON prepayments_depositrefund
BEGIN
 SELECT RAISE(ABORT,'deposit_refund_scope: Invalid deposit refund') WHERE NOT EXISTS(
 SELECT 1 FROM sales_receipt r JOIN prepayments_customerdeposit d ON d.receipt_id=r.id JOIN documents_documentidentity i ON i.id=NEW.identity_id
 JOIN ledger_journal j ON j.id=NEW.journal_id JOIN ledger_account l ON l.id=NEW.liability_id
 WHERE r.id=NEW.deposit_id AND r.company_id=NEW.company_id AND r.customer_id=NEW.customer_id AND d.liability_id=NEW.liability_id
 AND i.company_id=r.company_id AND i.kind='CDR' AND j.source_id=i.id AND j.company_id=r.company_id AND j.status='POSTED' AND j.reversal_of_id IS NULL AND j.date=NEW.date
 AND l.active=1 AND l.postable=1 AND NEW.date>=r.date AND NOT EXISTS(SELECT 1 FROM sales_receiptreversal z WHERE z.receipt_id=r.id));
 SELECT RAISE(ABORT,'deposit_refund_bank: A refund requires an eligible designated bank') WHERE NOT EXISTS(
 SELECT 1 FROM ledger_account a JOIN documents_designatedbank b ON b.account_id=a.id JOIN documents_accountingsetup setup ON setup.id=b.setup_id WHERE a.id=NEW.bank_id AND a.company_id=NEW.company_id AND setup.company_id=NEW.company_id
 AND a.kind='ASSET' AND a.active=1 AND a.postable=1 AND NOT EXISTS(SELECT 1 FROM fb_control_accounts c WHERE c.company_id=NEW.company_id AND c.account_id=a.id));
 SELECT RAISE(ABORT,'customer_deposit_overused: Customer deposit cannot be overused') WHERE NOT fb_pp_within((SELECT amount FROM sales_receipt WHERE id=NEW.deposit_id),NEW.amount,(SELECT json_group_array(amount) FROM prepayments_live_use WHERE deposit_id=NEW.deposit_id));
 SELECT RAISE(ABORT,'deposit_refund_number_in_use: Check number is already in use') WHERE NEW.check_number<>'' AND EXISTS(
 SELECT 1 FROM fb_check_number_live WHERE company_id=NEW.company_id AND bank_id=NEW.bank_id AND number=NEW.check_number);
END;
