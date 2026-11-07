-- CORE-INT2: permanent primary keys and ownership identities for every L2a table.
-- Keep 0001-0007 unchanged. These independent guards apply even before references
-- exist, with recursive_triggers OFF and with deferred foreign keys.
-- No-op identity assignments are allowed; existing immutable-snapshot guards remain.
-- Mutable tax-member/bank sets still use DELETE + INSERT, never reassignment.

CREATE TRIGGER fb_l2_directory_customer_primary_key_permanent BEFORE UPDATE OF id ON directory_customer
WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT, 'primary_key_immutable: Primary keys never change'); END;

CREATE TRIGGER fb_l2_directory_vendor_primary_key_permanent BEFORE UPDATE OF id ON directory_vendor
WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT, 'primary_key_immutable: Primary keys never change'); END;

CREATE TRIGGER fb_l2_directory_item_primary_key_permanent BEFORE UPDATE OF id ON directory_item
WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT, 'primary_key_immutable: Primary keys never change'); END;

CREATE TRIGGER fb_l2_salestax_taxitem_primary_key_permanent BEFORE UPDATE OF id ON salestax_taxitem
WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT, 'primary_key_immutable: Primary keys never change'); END;

CREATE TRIGGER fb_l2_salestax_taxgroupmember_primary_key_permanent BEFORE UPDATE OF id ON salestax_taxgroupmember
WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT, 'primary_key_immutable: Primary keys never change'); END;

CREATE TRIGGER fb_l2_salestax_salestaxsettings_primary_key_permanent BEFORE UPDATE OF id ON salestax_salestaxsettings
WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT, 'primary_key_immutable: Primary keys never change'); END;

CREATE TRIGGER fb_l2_documents_documentsequence_primary_key_permanent BEFORE UPDATE OF id ON documents_documentsequence
WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT, 'primary_key_immutable: Primary keys never change'); END;

CREATE TRIGGER fb_l2_documents_documentidentity_primary_key_permanent BEFORE UPDATE OF id ON documents_documentidentity
WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT, 'primary_key_immutable: Primary keys never change'); END;

CREATE TRIGGER fb_l2_documents_documentaccount_primary_key_permanent BEFORE UPDATE OF id ON documents_documentaccount
WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT, 'primary_key_immutable: Primary keys never change'); END;

CREATE TRIGGER fb_l2_documents_accountingsetup_primary_key_permanent BEFORE UPDATE OF id ON documents_accountingsetup
WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT, 'primary_key_immutable: Primary keys never change'); END;

CREATE TRIGGER fb_l2_documents_designatedbank_primary_key_permanent BEFORE UPDATE OF id ON documents_designatedbank
WHEN NEW.id IS NOT OLD.id
BEGIN SELECT RAISE(ABORT, 'primary_key_immutable: Primary keys never change'); END;

CREATE TRIGGER fb_l2_directory_customer_ownership_permanent BEFORE UPDATE OF company_id ON directory_customer
WHEN NEW.company_id IS NOT OLD.company_id
BEGIN SELECT RAISE(ABORT, 'identity_immutable: Ownership identities never change'); END;

CREATE TRIGGER fb_l2_directory_vendor_ownership_permanent BEFORE UPDATE OF company_id ON directory_vendor
WHEN NEW.company_id IS NOT OLD.company_id
BEGIN SELECT RAISE(ABORT, 'identity_immutable: Ownership identities never change'); END;

CREATE TRIGGER fb_l2_directory_item_ownership_permanent BEFORE UPDATE OF company_id ON directory_item
WHEN NEW.company_id IS NOT OLD.company_id
BEGIN SELECT RAISE(ABORT, 'identity_immutable: Ownership identities never change'); END;

CREATE TRIGGER fb_l2_salestax_taxitem_ownership_permanent BEFORE UPDATE OF company_id, kind ON salestax_taxitem
WHEN NEW.company_id IS NOT OLD.company_id OR NEW.kind IS NOT OLD.kind
BEGIN SELECT RAISE(ABORT, 'identity_immutable: Ownership identities never change'); END;

CREATE TRIGGER fb_l2_salestax_taxgroupmember_ownership_permanent BEFORE UPDATE OF group_id, member_id ON salestax_taxgroupmember
WHEN NEW.group_id IS NOT OLD.group_id OR NEW.member_id IS NOT OLD.member_id
BEGIN SELECT RAISE(ABORT, 'identity_immutable: Ownership identities never change'); END;

CREATE TRIGGER fb_l2_salestax_salestaxsettings_ownership_permanent BEFORE UPDATE OF company_id ON salestax_salestaxsettings
WHEN NEW.company_id IS NOT OLD.company_id
BEGIN SELECT RAISE(ABORT, 'identity_immutable: Ownership identities never change'); END;

CREATE TRIGGER fb_l2_documents_documentsequence_ownership_permanent BEFORE UPDATE OF company_id, kind ON documents_documentsequence
WHEN NEW.company_id IS NOT OLD.company_id OR NEW.kind IS NOT OLD.kind
BEGIN SELECT RAISE(ABORT, 'identity_immutable: Ownership identities never change'); END;

CREATE TRIGGER fb_l2_documents_documentidentity_ownership_permanent BEFORE UPDATE OF company_id ON documents_documentidentity
WHEN NEW.company_id IS NOT OLD.company_id
BEGIN SELECT RAISE(ABORT, 'identity_immutable: Ownership identities never change'); END;

CREATE TRIGGER fb_l2_documents_documentaccount_ownership_permanent BEFORE UPDATE OF source_id, account_id, role ON documents_documentaccount
WHEN NEW.source_id IS NOT OLD.source_id OR NEW.account_id IS NOT OLD.account_id OR NEW.role IS NOT OLD.role
BEGIN SELECT RAISE(ABORT, 'identity_immutable: Ownership identities never change'); END;

CREATE TRIGGER fb_l2_documents_accountingsetup_ownership_permanent BEFORE UPDATE OF company_id ON documents_accountingsetup
WHEN NEW.company_id IS NOT OLD.company_id
BEGIN SELECT RAISE(ABORT, 'identity_immutable: Ownership identities never change'); END;

CREATE TRIGGER fb_l2_documents_designatedbank_ownership_permanent BEFORE UPDATE OF setup_id, account_id ON documents_designatedbank
WHEN NEW.setup_id IS NOT OLD.setup_id OR NEW.account_id IS NOT OLD.account_id
BEGIN SELECT RAISE(ABORT, 'identity_immutable: Ownership identities never change'); END;

CREATE TRIGGER fb_l2_ledger_journal_ownership_permanent BEFORE UPDATE OF source_id ON ledger_journal
WHEN NEW.source_id IS NOT OLD.source_id
BEGIN SELECT RAISE(ABORT, 'source_journal_ownership: Journal source ownership is immutable'); END;
