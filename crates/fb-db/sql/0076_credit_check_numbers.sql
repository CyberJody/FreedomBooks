-- Credits completes the shared, Python checks.numbering namespace.
DROP VIEW fb_check_number_credit_live;
CREATE VIEW fb_check_number_credit_live AS
 SELECT r.company_id,r.bank_id,r.check_number AS number,r.id AS holder_id,r.identity_id,'refund' AS kind,2 AS priority
 FROM credits_creditrefund r WHERE r.check_number<>''
 AND NOT EXISTS(SELECT 1 FROM credits_creditrefundvoid v WHERE v.refund_id=r.id);
DROP VIEW fb_check_number_credit_used;
CREATE VIEW fb_check_number_credit_used AS
 SELECT company_id,bank_id,check_number AS number FROM credits_creditrefund WHERE check_number<>'';
CREATE TRIGGER fb_number_credits_refund_insert BEFORE INSERT ON credits_creditrefund BEGIN
 SELECT RAISE(ABORT,'credit_fields: Invalid check number') WHERE NOT fb_check_number_valid(NEW.check_number);
 SELECT RAISE(ABORT,'credit_refund_number_in_use: Check number already used on this bank')
 WHERE NEW.check_number<>'' AND EXISTS(SELECT 1 FROM fb_check_number_live
 WHERE company_id=NEW.company_id AND bank_id=NEW.bank_id AND number=NEW.check_number);
END;
