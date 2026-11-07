-- Additive hardening: retain old snapshots/identities; legacy provenance is NULL.
ALTER TABLE checks_checkline ADD COLUMN origin_position INTEGER
 CHECK(origin_position IS NULL OR origin_position BETWEEN 1 AND 50);
CREATE UNIQUE INDEX fb_checks_line_origin ON checks_checkline(revision_id,origin_position);
CREATE TRIGGER fb_checks_line_origin_insert BEFORE INSERT ON checks_checkline
WHEN NEW.origin_position IS NOT NULL BEGIN
 SELECT RAISE(ABORT,'check_line_origin: Retained origin already used') WHERE EXISTS(
 SELECT 1 FROM checks_checkline WHERE revision_id=NEW.revision_id AND origin_position=NEW.origin_position);
 SELECT RAISE(ABORT,'check_line_origin: Origin must be in the immediately preceding revision') WHERE NOT EXISTS(
 SELECT 1 FROM checks_checkrevision r JOIN checks_checkrevision p ON p.document_id=r.document_id AND p.number=r.number-1
 JOIN checks_checkline l ON l.revision_id=p.id AND l.position=NEW.origin_position WHERE r.id=NEW.revision_id);
END;
DROP TRIGGER fb_check_line_insert;
CREATE TRIGGER fb_check_line_insert BEFORE INSERT ON checks_checkline BEGIN
 SELECT RAISE(ABORT,'check_history_immutable: Check lines are immutable') WHERE EXISTS(SELECT 1 FROM checks_checkline WHERE id=NEW.id OR (revision_id=NEW.revision_id AND position=NEW.position));
 SELECT RAISE(ABORT,'check_line_scope: Invalid check line scope or committed history') WHERE NOT EXISTS(SELECT 1 FROM checks_checkrevision r JOIN checks_check c ON c.id=r.document_id WHERE r.id=NEW.revision_id AND r.number=c.revision AND NOT EXISTS(SELECT 1 FROM fb_seal WHERE kind='check_revision' AND doc_id=c.id||':'||c.revision) AND (NEW.item_id IS NULL OR EXISTS(SELECT 1 FROM directory_item WHERE id=NEW.item_id AND company_id=c.company_id)) AND (NEW.account_id IS NULL OR EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.account_id AND company_id=c.company_id)));
 SELECT RAISE(ABORT,'check_line_calculation: Invalid check line calculation or shape') WHERE fb_payables_line_valid(NEW.quantity,NEW.rate,NEW.amount)=0 OR (NEW.tab='EXPENSE' AND (NEW.item_id IS NOT NULL OR NEW.quantity<>10000)) OR (NEW.tab='ITEM' AND NEW.item_id IS NULL);
 SELECT RAISE(ABORT,'check_item_eligible: New draft items must be active') WHERE NEW.item_id IS NOT NULL AND EXISTS(SELECT 1 FROM checks_checkrevision WHERE id=NEW.revision_id AND state='DRAFT') AND NOT EXISTS(SELECT 1 FROM directory_item i JOIN checks_checkrevision r ON r.id=NEW.revision_id WHERE i.id=NEW.item_id AND (i.active=1 OR EXISTS(SELECT 1 FROM checks_checkrevision p JOIN checks_checkline l ON l.revision_id=p.id WHERE p.document_id=r.document_id AND p.number=r.number-1 AND l.position=NEW.origin_position AND l.item_id=i.id)));
 SELECT RAISE(ABORT,'check_line_eligible: Invalid draft purchase account') WHERE NEW.account_id IS NOT NULL AND EXISTS(SELECT 1 FROM checks_checkrevision WHERE id=NEW.revision_id AND state='DRAFT') AND NOT EXISTS(SELECT 1 FROM checks_checkrevision r JOIN checks_check c ON c.id=r.document_id JOIN ledger_account a ON a.company_id=c.company_id WHERE r.id=NEW.revision_id AND a.id=NEW.account_id AND a.id<>r.bank_id AND a.detail_type IN('BANK','OTHER_CURRENT_ASSET','FIXED_ASSET','OTHER_ASSET','CREDIT_CARD','OTHER_CURRENT_LIABILITY','LONG_TERM_LIABILITY','EQUITY','COST_OF_GOODS_SOLD','EXPENSE','OTHER_EXPENSE') AND NOT EXISTS(SELECT 1 FROM fb_control_accounts WHERE company_id=c.company_id AND account_id=a.id) AND ((a.active=1 AND a.postable=1) OR EXISTS(SELECT 1 FROM checks_checkrevision p JOIN checks_checkline l ON l.revision_id=p.id WHERE p.document_id=c.id AND p.number=r.number-1 AND l.position=NEW.origin_position AND l.account_id=a.id)));
END;

CREATE TRIGGER fb_checks_revision_snapshot_insert BEFORE INSERT ON checks_checkrevision BEGIN
 SELECT RAISE(ABORT,'check_snapshot_shape: Invalid header snapshot or number') WHERE
 fb_checks_snapshot_valid('account',NEW.bank_snapshot,NEW.bank_id)=0
 OR fb_checks_snapshot_valid('party',NEW.payee_snapshot,NEW.vendor_id)=0
 OR fb_checks_snapshot_valid('company',NEW.company_snapshot,(SELECT company_id FROM checks_check WHERE id=NEW.document_id))=0
 OR fb_check_number_valid(NEW.check_number)=0;
END;
CREATE TRIGGER fb_checks_line_snapshot_insert BEFORE INSERT ON checks_checkline BEGIN
 SELECT RAISE(ABORT,'check_snapshot_shape: Invalid line snapshot') WHERE fb_checks_snapshot_valid('account',NEW.account_snapshot,NEW.account_id)=0
 OR fb_checks_snapshot_valid('item',NEW.item_snapshot,NEW.item_id)=0;
END;
CREATE TRIGGER fb_checks_transfer_snapshot_insert BEFORE INSERT ON transfers_transfer BEGIN
 SELECT RAISE(ABORT,'check_snapshot_shape: Invalid transfer snapshot') WHERE fb_checks_snapshot_valid('account',NEW.from_snapshot,NEW.from_account_id)=0
 OR fb_checks_snapshot_valid('account',NEW.to_snapshot,NEW.to_account_id)=0
 OR fb_checks_snapshot_valid('company',NEW.company_snapshot,NEW.company_id)=0;
END;
CREATE TRIGGER fb_checks_number_insert BEFORE INSERT ON checks_check
WHEN fb_check_number_valid(NEW.number_key)=0 BEGIN SELECT RAISE(ABORT,'check_number: Invalid check number'); END;
CREATE TRIGGER fb_checks_number_update BEFORE UPDATE OF number_key ON checks_check
WHEN fb_check_number_valid(NEW.number_key)=0 BEGIN SELECT RAISE(ABORT,'check_number: Invalid check number'); END;
CREATE TRIGGER fb_checks_evidence_seal BEFORE INSERT ON fb_seal WHEN NEW.kind='check_revision' BEGIN
 SELECT RAISE(ABORT,'check_snapshot_shape: Invalid sealed header evidence') WHERE EXISTS(
 SELECT 1 FROM checks_checkrevision r JOIN checks_check c ON c.id=r.document_id
 WHERE NEW.doc_id=r.document_id||':'||r.number AND (fb_checks_snapshot_valid('account',r.bank_snapshot,r.bank_id)=0
 OR fb_checks_snapshot_valid('party',r.payee_snapshot,r.vendor_id)=0
 OR fb_checks_snapshot_valid('company',r.company_snapshot,c.company_id)=0
 OR fb_check_number_valid(r.check_number)=0));
 SELECT RAISE(ABORT,'check_snapshot_shape: Invalid sealed line evidence') WHERE EXISTS(
 SELECT 1 FROM checks_checkline l JOIN checks_checkrevision r ON r.id=l.revision_id
 WHERE NEW.doc_id=r.document_id||':'||r.number AND (fb_checks_snapshot_valid('account',l.account_snapshot,l.account_id)=0
 OR fb_checks_snapshot_valid('item',l.item_snapshot,l.item_id)=0 OR (l.origin_position IS NOT NULL AND NOT EXISTS(
 SELECT 1 FROM checks_checkrevision p JOIN checks_checkline old ON old.revision_id=p.id
 WHERE p.document_id=r.document_id AND p.number=r.number-1 AND old.position=l.origin_position))));
END;

CREATE TRIGGER fb_checks_transfer_evidence_seal BEFORE INSERT ON fb_seal
WHEN NEW.kind='transfer_journal' BEGIN
 SELECT RAISE(ABORT,'check_snapshot_shape: Invalid sealed transfer evidence') WHERE EXISTS(
 SELECT 1 FROM transfers_transfer t JOIN ledger_journal j ON j.source_id=t.identity_id
 WHERE j.id=NEW.doc_id AND (
 fb_checks_snapshot_valid('account',t.from_snapshot,t.from_account_id)=0
 OR fb_checks_snapshot_valid('account',t.to_snapshot,t.to_account_id)=0
 OR fb_checks_snapshot_valid('company',t.company_snapshot,t.company_id)=0));
END;
