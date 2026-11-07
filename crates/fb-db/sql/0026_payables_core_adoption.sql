-- Match new draft directory selections to service eligibility, while preserving
-- archived selections on the previous revision. OPEN and VOID copies retain
-- their existing posting and exact-financial-copy guards.
CREATE TRIGGER fb_payables_draft_vendor_eligible BEFORE INSERT ON payables_billrevision
WHEN NEW.state='DRAFT'
 AND NOT EXISTS(SELECT 1 FROM payables_billrevision WHERE id=NEW.id OR (bill_id=NEW.bill_id AND number=NEW.number))
 AND EXISTS(SELECT 1 FROM payables_bill b JOIN directory_vendor v ON v.id=NEW.vendor_id
   WHERE b.id=NEW.bill_id AND b.revision=NEW.number AND v.company_id=b.company_id)
BEGIN
 SELECT RAISE(ABORT,'bill_vendor_eligible: New draft vendors must be active')
 WHERE NOT EXISTS(SELECT 1 FROM payables_bill b JOIN directory_vendor v
   ON v.id=NEW.vendor_id AND v.company_id=b.company_id
   WHERE b.id=NEW.bill_id AND (v.active=1 OR EXISTS(
     SELECT 1 FROM payables_billrevision p WHERE p.bill_id=b.id
       AND p.number=NEW.number-1 AND p.vendor_id=v.id)));
END;

CREATE TRIGGER fb_payables_draft_item_eligible BEFORE INSERT ON payables_billline
WHEN NEW.item_id IS NOT NULL
 AND NOT EXISTS(SELECT 1 FROM payables_billline WHERE id=NEW.id OR (revision_id=NEW.revision_id AND position=NEW.position))
 AND EXISTS(SELECT 1 FROM payables_billrevision WHERE id=NEW.revision_id AND state='DRAFT')
 AND EXISTS(SELECT 1 FROM payables_billrevision r JOIN payables_bill b ON b.id=r.bill_id
   JOIN directory_item i ON i.id=NEW.item_id AND i.company_id=b.company_id
   WHERE r.id=NEW.revision_id AND r.number=b.revision)
BEGIN
 SELECT RAISE(ABORT,'bill_item_eligible: New draft items must be active')
 WHERE NOT EXISTS(SELECT 1 FROM payables_billrevision r JOIN payables_bill b ON b.id=r.bill_id
   JOIN directory_item i ON i.id=NEW.item_id AND i.company_id=b.company_id
   WHERE r.id=NEW.revision_id AND (i.active=1 OR EXISTS(
     SELECT 1 FROM payables_billrevision p JOIN payables_billline l ON l.revision_id=p.id
       WHERE p.bill_id=b.id AND p.number=r.number-1 AND l.item_id=i.id)));
END;

-- Identity assignments that do not change the key may accompany a revision
-- advance. All ownership/revision/seal guards still apply independently.
DROP TRIGGER fb_payables_bill_permanent_id;
CREATE TRIGGER fb_payables_bill_permanent_id BEFORE UPDATE OF id ON payables_bill
WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT,'payables_permanent_id: Primary keys are permanent'); END;
