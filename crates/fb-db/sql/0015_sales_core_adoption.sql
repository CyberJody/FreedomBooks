-- New draft selections must satisfy the service's active-directory rule.
-- An archived customer already selected on the preceding revision may be kept;
-- OPEN eligibility and immutable VOID copies have separate existing guards.
CREATE TRIGGER fb_sales_draft_customer_eligible BEFORE INSERT ON sales_invoicerevision
WHEN NEW.state='DRAFT'
 AND NOT EXISTS(SELECT 1 FROM sales_invoicerevision WHERE id=NEW.id OR (invoice_id=NEW.invoice_id AND number=NEW.number))
 AND EXISTS(SELECT 1 FROM sales_invoice i JOIN directory_customer c ON c.id=NEW.customer_id
   WHERE i.id=NEW.invoice_id AND i.revision=NEW.number AND c.company_id=i.company_id)
BEGIN
 SELECT RAISE(ABORT,'invoice_customer_eligible: New draft customers must be active')
 WHERE NOT EXISTS(SELECT 1 FROM sales_invoice i JOIN directory_customer c
   ON c.id=NEW.customer_id AND c.company_id=i.company_id
   WHERE i.id=NEW.invoice_id AND (c.active=1 OR EXISTS(
     SELECT 1 FROM sales_invoicerevision p WHERE p.invoice_id=i.id
       AND p.number=NEW.number-1 AND p.customer_id=c.id)));
END;

-- Identity assignments that do not change the key may accompany a revision
-- advance. All ownership/revision/seal guards still apply independently.
DROP TRIGGER fb_sales_invoice_permanent_id;
CREATE TRIGGER fb_sales_invoice_permanent_id BEFORE UPDATE OF id ON sales_invoice
WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT,'invoice_immutable_ownership: Sales primary keys are permanent'); END;
