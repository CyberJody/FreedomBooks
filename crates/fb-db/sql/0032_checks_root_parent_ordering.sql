-- Root updates must not temporarily point to missing or foreign parents,
-- even when a raw SQL caller defers foreign keys and supplies the parent later.
CREATE TRIGGER fb_checks_check_update_existing_parent BEFORE UPDATE OF bank_id,vendor_id ON checks_check
WHEN NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.bank_id AND company_id=NEW.company_id)
 OR (NEW.vendor_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM directory_vendor WHERE id=NEW.vendor_id AND company_id=NEW.company_id))
BEGIN SELECT RAISE(ABORT,'checks_existing_parent: Referenced parent must already exist'); END;
