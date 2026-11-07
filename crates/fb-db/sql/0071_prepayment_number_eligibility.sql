-- Python bill-payment references reserve a check number only for ASCII digits.
DROP TRIGGER pp_payment_number;
CREATE TRIGGER pp_payment_number BEFORE INSERT ON payables_billpayment
BEGIN SELECT RAISE(ABORT,'bill_payment_deposit_number: Check number is already used by a deposit refund')
WHERE NEW.method='CHECK' AND length(NEW.reference) BETWEEN 1 AND 20
AND NEW.reference NOT GLOB '*[^0-9]*' AND EXISTS(
 SELECT 1 FROM prepayments_depositrefund r WHERE r.company_id=NEW.company_id
 AND r.bank_id=NEW.bank_id AND r.check_number=NEW.reference
 AND NOT EXISTS(SELECT 1 FROM prepayments_depositrefundvoid v WHERE v.refund_id=r.id)); END;
