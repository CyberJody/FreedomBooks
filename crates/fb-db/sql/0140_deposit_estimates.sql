-- SEC-FIX1: estimate-linked deposits; preserve the full 98100e0 trigger inventory.
-- No shipped migration is edited. Legacy views/triggers keep referring to the final table name.
PRAGMA defer_foreign_keys=ON;
CREATE TEMP TABLE secfix1_deposit_copy AS SELECT * FROM prepayments_customerdeposit;
DROP TABLE prepayments_customerdeposit;
CREATE TABLE prepayments_customerdeposit (
 id TEXT PRIMARY KEY, receipt_id TEXT NOT NULL UNIQUE REFERENCES sales_receipt(id),
 company_id TEXT NOT NULL REFERENCES ledger_company(id), liability_id TEXT NOT NULL REFERENCES ledger_account(id),
 estimate_id TEXT REFERENCES sales_estimate(id), order_reference TEXT NOT NULL CHECK(length(order_reference)<=120)
) STRICT, WITHOUT ROWID;
INSERT INTO prepayments_customerdeposit SELECT * FROM secfix1_deposit_copy;
DROP TABLE secfix1_deposit_copy;
CREATE TRIGGER fb_valid_prepayments_customerdeposit_insert BEFORE INSERT ON prepayments_customerdeposit BEGIN
 SELECT RAISE(ABORT,'fb_valid: Invalid stored text, identifier, date or timestamp') WHERE
 NOT fb_valid_uuid(NEW.id) OR
 NOT fb_valid_uuid(NEW.receipt_id) OR
 NOT fb_valid_uuid(NEW.company_id) OR
 NOT fb_valid_uuid(NEW.liability_id) OR
 NOT fb_valid_uuid(NEW.estimate_id) OR
 NOT fb_valid_text(NEW.order_reference);
END;
CREATE TRIGGER fb_valid_prepayments_customerdeposit_update BEFORE UPDATE ON prepayments_customerdeposit BEGIN
 SELECT RAISE(ABORT,'fb_valid: Invalid stored text, identifier, date or timestamp') WHERE
 NOT fb_valid_uuid(NEW.id) OR
 NOT fb_valid_uuid(NEW.receipt_id) OR
 NOT fb_valid_uuid(NEW.company_id) OR
 NOT fb_valid_uuid(NEW.liability_id) OR
 NOT fb_valid_uuid(NEW.estimate_id) OR
 NOT fb_valid_text(NEW.order_reference);
END;
CREATE TRIGGER pp_customerdeposit_collision BEFORE INSERT ON prepayments_customerdeposit
BEGIN SELECT RAISE(ABORT,'customer_deposit_immutable: Customer deposit history is immutable') WHERE EXISTS(SELECT 1 FROM prepayments_customerdeposit WHERE id=NEW.id OR receipt_id=NEW.receipt_id); END;
CREATE TRIGGER pp_customerdeposit_delete BEFORE DELETE ON prepayments_customerdeposit
BEGIN SELECT RAISE(ABORT,'customer_deposit_immutable: Customer deposit history is immutable'); END;
CREATE TRIGGER pp_customerdeposit_update BEFORE UPDATE ON prepayments_customerdeposit
BEGIN SELECT RAISE(ABORT,'customer_deposit_immutable: Customer deposit history is immutable'); END;
CREATE TRIGGER pp_detail BEFORE INSERT ON prepayments_customerdeposit
BEGIN
 SELECT RAISE(ABORT,'customer_deposit_scope: Invalid customer deposit detail') WHERE NOT EXISTS(
 SELECT 1 FROM sales_receipt r JOIN ledger_account a ON a.id=NEW.liability_id JOIN documents_accountingsetup s ON s.company_id=r.company_id
 WHERE r.id=NEW.receipt_id AND r.kind='CDP' AND r.company_id=NEW.company_id AND s.customer_deposits_id=a.id
 AND a.company_id=r.company_id AND a.kind='LIABILITY' AND a.detail_type='OTHER_CURRENT_LIABILITY' AND a.active=1 AND a.postable=1
 AND NOT EXISTS(SELECT 1 FROM sales_seal WHERE kind='sales_receipt' AND doc_id=r.id));
END;
CREATE TRIGGER pp_customerdeposit_estimate_scope BEFORE INSERT ON prepayments_customerdeposit
WHEN NEW.estimate_id IS NOT NULL
BEGIN
 SELECT RAISE(ABORT,'customer_deposit_estimate_scope: Choose an estimate from this company for this customer.')
 WHERE NOT EXISTS(SELECT 1 FROM sales_estimate e
 JOIN sales_estimaterevision v ON v.estimate_id=e.id AND v.number=e.revision
 JOIN sales_receipt r ON r.id=NEW.receipt_id
 WHERE e.id=NEW.estimate_id AND e.company_id=NEW.company_id
 AND v.customer_id=r.customer_id AND v.state NOT IN('DECLINED','CANCELLED'));
END;
