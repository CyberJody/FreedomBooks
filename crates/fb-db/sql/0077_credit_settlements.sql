-- Every settlement family participates in both over-allocation boundaries.
DROP VIEW fb_credit_live_settlements;
CREATE VIEW fb_credit_live_settlements AS
 SELECT invoice_id,amount FROM credits_creditapplicationline l
 WHERE NOT EXISTS(SELECT 1 FROM credits_creditunapplication u WHERE u.application_id=l.application_id);
DROP VIEW credits_invoice_settlements;
CREATE VIEW credits_invoice_settlements AS SELECT invoice_id,amount FROM prepayments_live_settlements;
