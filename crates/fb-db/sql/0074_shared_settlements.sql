-- Credits replaces this provider at INT-L3b with live application lines.
-- Keep amount in integer cents and release rows visible through their owning family.
CREATE VIEW fb_credit_live_settlements AS SELECT '' AS invoice_id, 0 AS amount WHERE 0;
DROP VIEW prepayments_live_settlements;
CREATE VIEW prepayments_live_settlements AS
 SELECT invoice_id,amount FROM prepayments_depositapplication a
 WHERE NOT EXISTS(SELECT 1 FROM prepayments_depositunapplication u WHERE u.application_id=a.id)
 UNION ALL SELECT invoice_id,amount FROM sales_receiptallocation a
 WHERE NOT EXISTS(SELECT 1 FROM sales_receiptreversal r WHERE r.receipt_id=a.receipt_id)
 UNION ALL SELECT invoice_id,amount FROM fb_credit_live_settlements;
-- Existing sales and prepayment void guards retain their family-specific messages
-- and release dates. This common guard also includes the credits provider.
CREATE TRIGGER fb_invoice_shared_settlements BEFORE INSERT ON sales_invoicerevision
WHEN NEW.state='VOID' BEGIN
 SELECT RAISE(ABORT,'invoice_live_receipts: Reverse live receipts before voiding an invoice')
 WHERE EXISTS(SELECT 1 FROM sales_receiptallocation a WHERE a.invoice_id=NEW.invoice_id AND NOT EXISTS(SELECT 1 FROM sales_receiptreversal v WHERE v.receipt_id=a.receipt_id));
 SELECT RAISE(ABORT,'invoice_live_deposits: Unapply customer deposits before voiding an invoice')
 WHERE EXISTS(SELECT 1 FROM prepayments_depositapplication a WHERE a.invoice_id=NEW.invoice_id AND NOT EXISTS(SELECT 1 FROM prepayments_depositunapplication v WHERE v.application_id=a.id));
 SELECT RAISE(ABORT,'invoice_live_settlements: Release every live settlement before voiding an invoice')
 WHERE EXISTS(SELECT 1 FROM prepayments_live_settlements WHERE invoice_id=NEW.invoice_id);
END;
