-- L2a review: exact purchase eligibility, Python unused-account semantics, explicit revision bounds.

DROP TRIGGER fb_l2_directory_vendor_default_expense_account_id_insert;
CREATE TRIGGER fb_l2_directory_vendor_default_expense_account_id_insert BEFORE INSERT ON directory_vendor
BEGIN
  SELECT RAISE(ABORT, 'l2_account_parent: Default account must exist in this company') WHERE NEW.default_expense_account_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.default_expense_account_id AND company_id=NEW.company_id);
  SELECT RAISE(ABORT, 'l2_account_parent: Default requires an existing eligible account') WHERE NEW.default_expense_account_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=NEW.default_expense_account_id AND a.company_id=NEW.company_id AND a.active=1 AND a.postable=1 AND a.detail_type IN('BANK','OTHER_CURRENT_ASSET','FIXED_ASSET','OTHER_ASSET','CREDIT_CARD','OTHER_CURRENT_LIABILITY','LONG_TERM_LIABILITY','EQUITY','COST_OF_GOODS_SOLD','EXPENSE','OTHER_EXPENSE') AND NOT EXISTS(SELECT 1 FROM fb_control_accounts c WHERE c.company_id=NEW.company_id AND c.account_id=a.id));
END;

DROP TRIGGER fb_l2_directory_vendor_default_expense_account_id_update;
CREATE TRIGGER fb_l2_directory_vendor_default_expense_account_id_update BEFORE UPDATE ON directory_vendor
BEGIN
  SELECT RAISE(ABORT, 'l2_account_parent: Default account must exist in this company') WHERE NEW.default_expense_account_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.default_expense_account_id AND company_id=NEW.company_id);
  SELECT RAISE(ABORT, 'l2_account_parent: Default requires an existing eligible account') WHERE NEW.default_expense_account_id IS NOT NULL AND (NEW.default_expense_account_id IS NOT OLD.default_expense_account_id) AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=NEW.default_expense_account_id AND a.company_id=NEW.company_id AND a.active=1 AND a.postable=1 AND a.detail_type IN('BANK','OTHER_CURRENT_ASSET','FIXED_ASSET','OTHER_ASSET','CREDIT_CARD','OTHER_CURRENT_LIABILITY','LONG_TERM_LIABILITY','EQUITY','COST_OF_GOODS_SOLD','EXPENSE','OTHER_EXPENSE') AND NOT EXISTS(SELECT 1 FROM fb_control_accounts c WHERE c.company_id=NEW.company_id AND c.account_id=a.id));
END;

DROP TRIGGER fb_l2_account_identity;
CREATE TRIGGER fb_l2_account_identity BEFORE UPDATE ON ledger_account
WHEN NEW.kind IS NOT OLD.kind AND EXISTS(SELECT 1 FROM documents_documentaccount WHERE account_id=OLD.id)
BEGIN
  SELECT RAISE(ABORT, 'l2_account_reference: Accounts used in source document snapshots cannot change kind');
END;

CREATE TRIGGER fb_l2_revision_documents_accountingsetup_insert BEFORE INSERT ON documents_accountingsetup
WHEN typeof(NEW.revision)<>'integer' OR NEW.revision NOT BETWEEN 0 AND 9223372036854775807
BEGIN
  SELECT RAISE(ABORT, 'l2_revision_bounds: Revision must be a nonnegative signed 64-bit integer');
END;

CREATE TRIGGER fb_l2_revision_documents_accountingsetup_update BEFORE UPDATE ON documents_accountingsetup
WHEN typeof(NEW.revision)<>'integer' OR NEW.revision NOT BETWEEN 0 AND 9223372036854775807
BEGIN
  SELECT RAISE(ABORT, 'l2_revision_bounds: Revision must be a nonnegative signed 64-bit integer');
END;

CREATE TRIGGER fb_l2_revision_salestax_taxitem_insert BEFORE INSERT ON salestax_taxitem
WHEN typeof(NEW.revision)<>'integer' OR NEW.revision NOT BETWEEN 0 AND 9223372036854775807
BEGIN
  SELECT RAISE(ABORT, 'l2_revision_bounds: Revision must be a nonnegative signed 64-bit integer');
END;

CREATE TRIGGER fb_l2_revision_salestax_taxitem_update BEFORE UPDATE ON salestax_taxitem
WHEN typeof(NEW.revision)<>'integer' OR NEW.revision NOT BETWEEN 0 AND 9223372036854775807
BEGIN
  SELECT RAISE(ABORT, 'l2_revision_bounds: Revision must be a nonnegative signed 64-bit integer');
END;

CREATE TRIGGER fb_l2_revision_salestax_salestaxsettings_insert BEFORE INSERT ON salestax_salestaxsettings
WHEN typeof(NEW.revision)<>'integer' OR NEW.revision NOT BETWEEN 0 AND 9223372036854775807
BEGIN
  SELECT RAISE(ABORT, 'l2_revision_bounds: Revision must be a nonnegative signed 64-bit integer');
END;

CREATE TRIGGER fb_l2_revision_salestax_salestaxsettings_update BEFORE UPDATE ON salestax_salestaxsettings
WHEN typeof(NEW.revision)<>'integer' OR NEW.revision NOT BETWEEN 0 AND 9223372036854775807
BEGIN
  SELECT RAISE(ABORT, 'l2_revision_bounds: Revision must be a nonnegative signed 64-bit integer');
END;
