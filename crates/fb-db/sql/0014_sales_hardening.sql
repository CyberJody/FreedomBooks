-- Permanent UUID identities. Append-only tables also refuse every payload UPDATE.
CREATE TRIGGER fb_sales_invoice_permanent_id BEFORE UPDATE OF id ON sales_invoice
BEGIN SELECT RAISE(ABORT,'invoice_immutable_ownership: Sales primary keys are permanent'); END;
CREATE TRIGGER fb_sales_invoicerevision_permanent_id BEFORE UPDATE OF id ON sales_invoicerevision
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales primary keys are permanent'); END;
CREATE TRIGGER fb_sales_invoiceline_permanent_id BEFORE UPDATE OF id ON sales_invoiceline
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales primary keys are permanent'); END;
CREATE TRIGGER fb_sales_receipt_permanent_id BEFORE UPDATE OF id ON sales_receipt
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales primary keys are permanent'); END;
CREATE TRIGGER fb_sales_receiptallocation_permanent_id BEFORE UPDATE OF id ON sales_receiptallocation
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales primary keys are permanent'); END;
CREATE TRIGGER fb_sales_receiptreversal_permanent_id BEFORE UPDATE OF id ON sales_receiptreversal
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales primary keys are permanent'); END;
CREATE TRIGGER fb_sales_salesreceipt_permanent_id BEFORE UPDATE OF id ON sales_salesreceipt
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales primary keys are permanent'); END;
CREATE TRIGGER fb_sales_salesreceiptline_permanent_id BEFORE UPDATE OF id ON sales_salesreceiptline
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales primary keys are permanent'); END;
CREATE TRIGGER fb_sales_invoice_update_collision BEFORE UPDATE ON sales_invoice
BEGIN SELECT RAISE(ABORT,'invoice_immutable_ownership: Update would replace an existing invoice')
 WHERE EXISTS(SELECT 1 FROM sales_invoice WHERE id IS NOT OLD.id AND (id=NEW.id OR identity_id=NEW.identity_id)); END;

-- Historical draft/void lines may retain an archived selection from the prior
-- revision. New choices and posting lines require eligible parents immediately.
CREATE TRIGGER fb_sales_invoice_line_eligible BEFORE INSERT ON sales_invoiceline
WHEN NOT EXISTS(SELECT 1 FROM sales_invoiceline WHERE id=NEW.id OR (revision_id=NEW.revision_id AND position=NEW.position))
BEGIN
 SELECT RAISE(ABORT,'invoice_line_scope: New lines require eligible income accounts and items')
 WHERE NOT EXISTS(SELECT 1 FROM sales_invoicerevision r WHERE r.id=NEW.revision_id
 AND (NEW.income_account_id IS NULL OR EXISTS(SELECT 1 FROM ledger_account a
      WHERE a.id=NEW.income_account_id AND a.kind='INCOME' AND a.active=1 AND a.postable=1)
      OR (r.state<>'OPEN' AND EXISTS(SELECT 1 FROM sales_invoicerevision p JOIN sales_invoiceline l ON l.revision_id=p.id
          WHERE p.invoice_id=r.invoice_id AND p.number=r.number-1 AND l.position=NEW.position AND l.income_account_id=NEW.income_account_id)))
 AND (NEW.item_id IS NULL OR EXISTS(SELECT 1 FROM directory_item t WHERE t.id=NEW.item_id AND t.active=1)
      OR (r.state<>'OPEN' AND EXISTS(SELECT 1 FROM sales_invoicerevision p JOIN sales_invoiceline l ON l.revision_id=p.id
          WHERE p.invoice_id=r.invoice_id AND p.number=r.number-1 AND l.position=NEW.position AND l.item_id=NEW.item_id))));
END;
