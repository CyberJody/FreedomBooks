-- Persist service line provenance. Historical rows predate this evidence and
-- remain NULL; all new retained selections must identify a unique prior line.
ALTER TABLE payables_billline ADD COLUMN origin_position INTEGER
 CHECK(origin_position IS NULL OR origin_position BETWEEN 1 AND 50);
CREATE UNIQUE INDEX fb_payables_line_origin ON payables_billline(revision_id,origin_position);

CREATE TRIGGER fb_payables_line_origin_insert BEFORE INSERT ON payables_billline
WHEN NEW.origin_position IS NOT NULL
BEGIN
 SELECT RAISE(ABORT,'payables_line_origin: Retained line origin already used')
 WHERE EXISTS(SELECT 1 FROM payables_billline WHERE revision_id=NEW.revision_id AND origin_position=NEW.origin_position);
 SELECT RAISE(ABORT,'payables_line_origin: Retained line must exist in the previous revision')
 WHERE NOT EXISTS(SELECT 1 FROM payables_billrevision r
 JOIN payables_billrevision p ON p.bill_id=r.bill_id AND p.number=r.number-1
 JOIN payables_billline l ON l.revision_id=p.id AND l.position=NEW.origin_position
 WHERE r.id=NEW.revision_id);
END;

CREATE TRIGGER fb_payables_billline_snapshot_shape BEFORE INSERT ON payables_billline
BEGIN
 SELECT RAISE(ABORT,'payables_snapshot_shape: Snapshots must be objects')
 WHERE (CASE WHEN json_valid(NEW.item_snapshot) THEN json_type(NEW.item_snapshot) END) IS NOT 'object'
 OR (CASE WHEN json_valid(NEW.account_snapshot) THEN json_type(NEW.account_snapshot) END) IS NOT 'object'
 OR EXISTS(SELECT 1 FROM json_each(CASE WHEN json_valid(NEW.account_snapshot) THEN NEW.account_snapshot ELSE '{}' END) WHERE type<>'object');
END;

CREATE TRIGGER fb_payables_billrevision_snapshot_shape BEFORE INSERT ON payables_billrevision
BEGIN
 SELECT RAISE(ABORT,'payables_snapshot_shape: Snapshots must be objects')
 WHERE (CASE WHEN json_valid(NEW.vendor_snapshot) THEN json_type(NEW.vendor_snapshot) END) IS NOT 'object'
 OR (CASE WHEN json_valid(NEW.company_snapshot) THEN json_type(NEW.company_snapshot) END) IS NOT 'object';
END;

CREATE TRIGGER fb_payables_billpayment_snapshot_shape BEFORE INSERT ON payables_billpayment
BEGIN
 SELECT RAISE(ABORT,'payables_snapshot_shape: Snapshots must be objects')
 WHERE (CASE WHEN json_valid(NEW.vendor_snapshot) THEN json_type(NEW.vendor_snapshot) END) IS NOT 'object'
 OR (CASE WHEN json_valid(NEW.company_snapshot) THEN json_type(NEW.company_snapshot) END) IS NOT 'object';
END;

DROP TRIGGER fb_payables_draft_item_eligible;
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
       WHERE p.bill_id=b.id AND p.number=r.number-1 AND l.position=NEW.origin_position AND l.item_id=i.id)));
END;

DROP TRIGGER fb_payables_draft_line_eligible;
CREATE TRIGGER fb_payables_draft_line_eligible BEFORE INSERT ON payables_billline
WHEN NEW.expense_account_id IS NOT NULL
 AND EXISTS(SELECT 1 FROM payables_billrevision WHERE id=NEW.revision_id AND state='DRAFT')
 AND NOT EXISTS(
 SELECT 1 FROM payables_billrevision r JOIN payables_bill b ON b.id=r.bill_id
 JOIN ledger_account a ON a.id=NEW.expense_account_id AND a.company_id=b.company_id
 WHERE r.id=NEW.revision_id
 AND a.detail_type IN('BANK','OTHER_CURRENT_ASSET','FIXED_ASSET','OTHER_ASSET','CREDIT_CARD','OTHER_CURRENT_LIABILITY','LONG_TERM_LIABILITY','EQUITY','COST_OF_GOODS_SOLD','EXPENSE','OTHER_EXPENSE')
 AND NOT EXISTS(SELECT 1 FROM fb_control_accounts WHERE company_id=b.company_id AND account_id=a.id)
 AND ((a.active=1 AND a.postable=1) OR EXISTS(
 SELECT 1 FROM payables_billrevision previous JOIN payables_billline line ON line.revision_id=previous.id
 WHERE previous.bill_id=b.id AND previous.number=r.number-1 AND line.position=NEW.origin_position AND line.expense_account_id=a.id)))
BEGIN SELECT RAISE(ABORT,'payables_line_eligible: New draft lines require existing eligible purchase accounts'); END;
