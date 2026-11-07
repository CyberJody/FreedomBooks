-- L2a replacement and parent hardening. Every displaced unique-key row is
-- checked BEFORE INSERT/UPDATE, independently of recursive_triggers and FKs.
-- 0004 UUID tables are WITHOUT ROWID (pre-release lineage; recreate old L2a books).

CREATE TRIGGER fb_l2_directory_customer_insert BEFORE INSERT ON directory_customer
BEGIN
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM directory_customer WHERE id=NEW.id);
END;

CREATE TRIGGER fb_l2_directory_customer_update BEFORE UPDATE ON directory_customer
BEGIN
  SELECT RAISE(ABORT, 'l2_identity: Row identity is permanent') WHERE NEW.id IS NOT OLD.id;
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM directory_customer WHERE id<>OLD.id AND (id=NEW.id));
  SELECT RAISE(ABORT, 'l2_company: Row company is permanent') WHERE NEW.company_id IS NOT OLD.company_id;
END;

CREATE TRIGGER fb_l2_directory_customer_company_insert BEFORE INSERT ON directory_customer
BEGIN
  SELECT RAISE(ABORT, 'l2_parent: Company must exist') WHERE NOT EXISTS(SELECT 1 FROM ledger_company WHERE id=NEW.company_id);
END;

CREATE TRIGGER fb_l2_directory_customer_company_update BEFORE UPDATE ON directory_customer
BEGIN
  SELECT RAISE(ABORT, 'l2_parent: Company must exist') WHERE NOT EXISTS(SELECT 1 FROM ledger_company WHERE id=NEW.company_id);
END;

CREATE TRIGGER fb_l2_directory_vendor_insert BEFORE INSERT ON directory_vendor
BEGIN
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM directory_vendor WHERE id=NEW.id);
END;

CREATE TRIGGER fb_l2_directory_vendor_update BEFORE UPDATE ON directory_vendor
BEGIN
  SELECT RAISE(ABORT, 'l2_identity: Row identity is permanent') WHERE NEW.id IS NOT OLD.id;
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM directory_vendor WHERE id<>OLD.id AND (id=NEW.id));
  SELECT RAISE(ABORT, 'l2_company: Row company is permanent') WHERE NEW.company_id IS NOT OLD.company_id;
END;

CREATE TRIGGER fb_l2_directory_vendor_company_insert BEFORE INSERT ON directory_vendor
BEGIN
  SELECT RAISE(ABORT, 'l2_parent: Company must exist') WHERE NOT EXISTS(SELECT 1 FROM ledger_company WHERE id=NEW.company_id);
END;

CREATE TRIGGER fb_l2_directory_vendor_company_update BEFORE UPDATE ON directory_vendor
BEGIN
  SELECT RAISE(ABORT, 'l2_parent: Company must exist') WHERE NOT EXISTS(SELECT 1 FROM ledger_company WHERE id=NEW.company_id);
END;

CREATE TRIGGER fb_l2_directory_item_insert BEFORE INSERT ON directory_item
BEGIN
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM directory_item WHERE id=NEW.id OR (company_id=NEW.company_id AND fb_lower(code)=fb_lower(NEW.code)));
END;

CREATE TRIGGER fb_l2_directory_item_update BEFORE UPDATE ON directory_item
BEGIN
  SELECT RAISE(ABORT, 'l2_identity: Row identity is permanent') WHERE NEW.id IS NOT OLD.id;
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM directory_item WHERE id<>OLD.id AND (id=NEW.id OR (company_id=NEW.company_id AND fb_lower(code)=fb_lower(NEW.code))));
  SELECT RAISE(ABORT, 'l2_company: Row company is permanent') WHERE NEW.company_id IS NOT OLD.company_id;
END;

CREATE TRIGGER fb_l2_directory_item_company_insert BEFORE INSERT ON directory_item
BEGIN
  SELECT RAISE(ABORT, 'l2_parent: Company must exist') WHERE NOT EXISTS(SELECT 1 FROM ledger_company WHERE id=NEW.company_id);
END;

CREATE TRIGGER fb_l2_directory_item_company_update BEFORE UPDATE ON directory_item
BEGIN
  SELECT RAISE(ABORT, 'l2_parent: Company must exist') WHERE NOT EXISTS(SELECT 1 FROM ledger_company WHERE id=NEW.company_id);
END;

CREATE TRIGGER fb_l2_salestax_taxitem_insert BEFORE INSERT ON salestax_taxitem
BEGIN
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM salestax_taxitem WHERE id=NEW.id OR (company_id=NEW.company_id AND fb_lower(name)=fb_lower(NEW.name)));
END;

CREATE TRIGGER fb_l2_salestax_taxitem_update BEFORE UPDATE ON salestax_taxitem
BEGIN
  SELECT RAISE(ABORT, 'l2_identity: Row identity is permanent') WHERE NEW.id IS NOT OLD.id;
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM salestax_taxitem WHERE id<>OLD.id AND (id=NEW.id OR (company_id=NEW.company_id AND fb_lower(name)=fb_lower(NEW.name))));
  SELECT RAISE(ABORT, 'l2_company: Row company is permanent') WHERE NEW.company_id IS NOT OLD.company_id;
END;

CREATE TRIGGER fb_l2_salestax_taxitem_company_insert BEFORE INSERT ON salestax_taxitem
BEGIN
  SELECT RAISE(ABORT, 'l2_parent: Company must exist') WHERE NOT EXISTS(SELECT 1 FROM ledger_company WHERE id=NEW.company_id);
END;

CREATE TRIGGER fb_l2_salestax_taxitem_company_update BEFORE UPDATE ON salestax_taxitem
BEGIN
  SELECT RAISE(ABORT, 'l2_parent: Company must exist') WHERE NOT EXISTS(SELECT 1 FROM ledger_company WHERE id=NEW.company_id);
END;

CREATE TRIGGER fb_l2_salestax_taxgroupmember_insert BEFORE INSERT ON salestax_taxgroupmember
BEGIN
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM salestax_taxgroupmember WHERE id=NEW.id OR (group_id=NEW.group_id AND member_id=NEW.member_id) OR (group_id=NEW.group_id AND position=NEW.position));
END;

CREATE TRIGGER fb_l2_salestax_taxgroupmember_update BEFORE UPDATE ON salestax_taxgroupmember
BEGIN
  SELECT RAISE(ABORT, 'l2_identity: Row identity is permanent') WHERE NEW.id IS NOT OLD.id;
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM salestax_taxgroupmember WHERE id<>OLD.id AND (id=NEW.id OR (group_id=NEW.group_id AND member_id=NEW.member_id) OR (group_id=NEW.group_id AND position=NEW.position)));
END;

CREATE TRIGGER fb_l2_salestax_salestaxsettings_insert BEFORE INSERT ON salestax_salestaxsettings
BEGIN
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM salestax_salestaxsettings WHERE id=NEW.id OR (company_id=NEW.company_id));
END;

CREATE TRIGGER fb_l2_salestax_salestaxsettings_update BEFORE UPDATE ON salestax_salestaxsettings
BEGIN
  SELECT RAISE(ABORT, 'l2_identity: Row identity is permanent') WHERE NEW.id IS NOT OLD.id;
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM salestax_salestaxsettings WHERE id<>OLD.id AND (id=NEW.id OR (company_id=NEW.company_id)));
  SELECT RAISE(ABORT, 'l2_company: Row company is permanent') WHERE NEW.company_id IS NOT OLD.company_id;
END;

CREATE TRIGGER fb_l2_salestax_salestaxsettings_company_insert BEFORE INSERT ON salestax_salestaxsettings
BEGIN
  SELECT RAISE(ABORT, 'l2_parent: Company must exist') WHERE NOT EXISTS(SELECT 1 FROM ledger_company WHERE id=NEW.company_id);
END;

CREATE TRIGGER fb_l2_salestax_salestaxsettings_company_update BEFORE UPDATE ON salestax_salestaxsettings
BEGIN
  SELECT RAISE(ABORT, 'l2_parent: Company must exist') WHERE NOT EXISTS(SELECT 1 FROM ledger_company WHERE id=NEW.company_id);
END;

CREATE TRIGGER fb_l2_documents_documentsequence_insert BEFORE INSERT ON documents_documentsequence
BEGIN
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM documents_documentsequence WHERE id=NEW.id OR (company_id=NEW.company_id AND kind=NEW.kind));
END;

CREATE TRIGGER fb_l2_documents_documentsequence_update BEFORE UPDATE ON documents_documentsequence
BEGIN
  SELECT RAISE(ABORT, 'l2_identity: Row identity is permanent') WHERE NEW.id IS NOT OLD.id;
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM documents_documentsequence WHERE id<>OLD.id AND (id=NEW.id OR (company_id=NEW.company_id AND kind=NEW.kind)));
  SELECT RAISE(ABORT, 'l2_company: Row company is permanent') WHERE NEW.company_id IS NOT OLD.company_id;
END;

CREATE TRIGGER fb_l2_documents_documentsequence_company_insert BEFORE INSERT ON documents_documentsequence
BEGIN
  SELECT RAISE(ABORT, 'l2_parent: Company must exist') WHERE NOT EXISTS(SELECT 1 FROM ledger_company WHERE id=NEW.company_id);
END;

CREATE TRIGGER fb_l2_documents_documentsequence_company_update BEFORE UPDATE ON documents_documentsequence
BEGIN
  SELECT RAISE(ABORT, 'l2_parent: Company must exist') WHERE NOT EXISTS(SELECT 1 FROM ledger_company WHERE id=NEW.company_id);
END;

CREATE TRIGGER fb_l2_documents_documentidentity_insert BEFORE INSERT ON documents_documentidentity
BEGIN
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM documents_documentidentity WHERE id=NEW.id OR (company_id=NEW.company_id AND number=NEW.number) OR (company_id=NEW.company_id AND kind=NEW.kind AND ordinal=NEW.ordinal) OR (company_id=NEW.company_id AND request_key=NEW.request_key));
END;

CREATE TRIGGER fb_l2_documents_documentidentity_update BEFORE UPDATE ON documents_documentidentity
BEGIN
  SELECT RAISE(ABORT, 'l2_identity: Row identity is permanent') WHERE NEW.id IS NOT OLD.id;
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM documents_documentidentity WHERE id<>OLD.id AND (id=NEW.id OR (company_id=NEW.company_id AND number=NEW.number) OR (company_id=NEW.company_id AND kind=NEW.kind AND ordinal=NEW.ordinal) OR (company_id=NEW.company_id AND request_key=NEW.request_key)));
  SELECT RAISE(ABORT, 'l2_company: Row company is permanent') WHERE NEW.company_id IS NOT OLD.company_id;
END;

CREATE TRIGGER fb_l2_documents_documentidentity_company_insert BEFORE INSERT ON documents_documentidentity
BEGIN
  SELECT RAISE(ABORT, 'l2_parent: Company must exist') WHERE NOT EXISTS(SELECT 1 FROM ledger_company WHERE id=NEW.company_id);
END;

CREATE TRIGGER fb_l2_documents_documentidentity_company_update BEFORE UPDATE ON documents_documentidentity
BEGIN
  SELECT RAISE(ABORT, 'l2_parent: Company must exist') WHERE NOT EXISTS(SELECT 1 FROM ledger_company WHERE id=NEW.company_id);
END;

CREATE TRIGGER fb_l2_documents_documentaccount_insert BEFORE INSERT ON documents_documentaccount
BEGIN
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM documents_documentaccount WHERE id=NEW.id OR (source_id=NEW.source_id AND role=NEW.role));
END;

CREATE TRIGGER fb_l2_documents_documentaccount_update BEFORE UPDATE ON documents_documentaccount
BEGIN
  SELECT RAISE(ABORT, 'l2_identity: Row identity is permanent') WHERE NEW.id IS NOT OLD.id;
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM documents_documentaccount WHERE id<>OLD.id AND (id=NEW.id OR (source_id=NEW.source_id AND role=NEW.role)));
END;

CREATE TRIGGER fb_l2_documents_accountingsetup_insert BEFORE INSERT ON documents_accountingsetup
BEGIN
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM documents_accountingsetup WHERE id=NEW.id OR (company_id=NEW.company_id));
END;

CREATE TRIGGER fb_l2_documents_accountingsetup_update BEFORE UPDATE ON documents_accountingsetup
BEGIN
  SELECT RAISE(ABORT, 'l2_identity: Row identity is permanent') WHERE NEW.id IS NOT OLD.id;
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM documents_accountingsetup WHERE id<>OLD.id AND (id=NEW.id OR (company_id=NEW.company_id)));
  SELECT RAISE(ABORT, 'l2_company: Row company is permanent') WHERE NEW.company_id IS NOT OLD.company_id;
END;

CREATE TRIGGER fb_l2_documents_accountingsetup_company_insert BEFORE INSERT ON documents_accountingsetup
BEGIN
  SELECT RAISE(ABORT, 'l2_parent: Company must exist') WHERE NOT EXISTS(SELECT 1 FROM ledger_company WHERE id=NEW.company_id);
END;

CREATE TRIGGER fb_l2_documents_accountingsetup_company_update BEFORE UPDATE ON documents_accountingsetup
BEGIN
  SELECT RAISE(ABORT, 'l2_parent: Company must exist') WHERE NOT EXISTS(SELECT 1 FROM ledger_company WHERE id=NEW.company_id);
END;

CREATE TRIGGER fb_l2_documents_designatedbank_insert BEFORE INSERT ON documents_designatedbank
BEGIN
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM documents_designatedbank WHERE id=NEW.id OR (setup_id=NEW.setup_id AND account_id=NEW.account_id));
END;

CREATE TRIGGER fb_l2_documents_designatedbank_update BEFORE UPDATE ON documents_designatedbank
BEGIN
  SELECT RAISE(ABORT, 'l2_identity: Row identity is permanent') WHERE NEW.id IS NOT OLD.id;
  SELECT RAISE(ABORT, 'l2_key_collision: Protected key already exists') WHERE EXISTS(SELECT 1 FROM documents_designatedbank WHERE id<>OLD.id AND (id=NEW.id OR (setup_id=NEW.setup_id AND account_id=NEW.account_id)));
END;

CREATE TRIGGER fb_l2_directory_customer_delete BEFORE DELETE ON directory_customer
BEGIN
  SELECT RAISE(ABORT, 'l2_no_delete: Archive or edit this record instead of deleting it');
END;

CREATE TRIGGER fb_l2_directory_vendor_delete BEFORE DELETE ON directory_vendor
BEGIN
  SELECT RAISE(ABORT, 'l2_no_delete: Archive or edit this record instead of deleting it');
END;

CREATE TRIGGER fb_l2_directory_item_delete BEFORE DELETE ON directory_item
BEGIN
  SELECT RAISE(ABORT, 'l2_no_delete: Archive or edit this record instead of deleting it');
END;

CREATE TRIGGER fb_l2_documents_accountingsetup_delete BEFORE DELETE ON documents_accountingsetup
BEGIN
  SELECT RAISE(ABORT, 'l2_no_delete: Archive or edit this record instead of deleting it');
END;

CREATE TRIGGER fb_l2_directory_item_income_account_id_insert BEFORE INSERT ON directory_item
BEGIN
  SELECT RAISE(ABORT, 'l2_account_parent: Default account must exist in this company') WHERE NEW.income_account_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.income_account_id AND company_id=NEW.company_id);
  SELECT RAISE(ABORT, 'l2_account_parent: Default requires an existing eligible account') WHERE NEW.income_account_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=NEW.income_account_id AND a.company_id=NEW.company_id AND a.active=1 AND a.postable=1 AND a.kind='INCOME');
END;

CREATE TRIGGER fb_l2_directory_item_expense_account_id_insert BEFORE INSERT ON directory_item
BEGIN
  SELECT RAISE(ABORT, 'l2_account_parent: Default account must exist in this company') WHERE NEW.expense_account_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.expense_account_id AND company_id=NEW.company_id);
  SELECT RAISE(ABORT, 'l2_account_parent: Default requires an existing eligible account') WHERE NEW.expense_account_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=NEW.expense_account_id AND a.company_id=NEW.company_id AND a.active=1 AND a.postable=1 AND a.kind='EXPENSE');
END;

CREATE TRIGGER fb_l2_directory_vendor_default_expense_account_id_insert BEFORE INSERT ON directory_vendor
BEGIN
  SELECT RAISE(ABORT, 'l2_account_parent: Default account must exist in this company') WHERE NEW.default_expense_account_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.default_expense_account_id AND company_id=NEW.company_id);
  SELECT RAISE(ABORT, 'l2_account_parent: Default requires an existing eligible account') WHERE NEW.default_expense_account_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=NEW.default_expense_account_id AND a.company_id=NEW.company_id AND a.active=1 AND a.postable=1 AND a.kind IN('EXPENSE','ASSET','LIABILITY','EQUITY') AND NOT EXISTS(SELECT 1 FROM fb_control_accounts c WHERE c.company_id=NEW.company_id AND c.account_id=a.id));
END;

CREATE TRIGGER fb_l2_tax_agency_insert BEFORE INSERT ON salestax_taxitem
BEGIN
  SELECT RAISE(ABORT, 'l2_tax_agency: New agency must be active') WHERE NEW.agency_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM directory_vendor WHERE id=NEW.agency_id AND company_id=NEW.company_id AND active=1);
END;

CREATE TRIGGER fb_l2_tax_default_insert BEFORE INSERT ON salestax_salestaxsettings
BEGIN
  SELECT RAISE(ABORT, 'l2_tax_default: New default tax item must be active') WHERE NEW.default_item_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM salestax_taxitem WHERE id=NEW.default_item_id AND company_id=NEW.company_id AND active=1);
END;

CREATE TRIGGER fb_l2_tax_member_insert BEFORE INSERT ON salestax_taxgroupmember
BEGIN
  SELECT RAISE(ABORT, 'l2_tax_member: Active groups need active member rates') WHERE EXISTS(SELECT 1 FROM salestax_taxitem WHERE id=NEW.group_id AND active=1) AND NOT EXISTS(SELECT 1 FROM salestax_taxitem WHERE id=NEW.member_id AND active=1);
END;

CREATE TRIGGER fb_l2_setup_insert BEFORE INSERT ON documents_accountingsetup
BEGIN
  SELECT RAISE(ABORT, 'l2_setup_account: Setup account must exist in this company') WHERE NEW.receivables_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.receivables_id AND company_id=NEW.company_id);
  SELECT RAISE(ABORT, 'l2_setup_account: Setup account must exist in this company') WHERE NEW.payables_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.payables_id AND company_id=NEW.company_id);
  SELECT RAISE(ABORT, 'l2_setup_account: Setup account must exist in this company') WHERE NEW.undeposited_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.undeposited_id AND company_id=NEW.company_id);
  SELECT RAISE(ABORT, 'l2_setup_account: Setup account must exist in this company') WHERE NEW.collected_tax_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.collected_tax_id AND company_id=NEW.company_id);
  SELECT RAISE(ABORT, 'l2_setup_account: Setup account must exist in this company') WHERE NEW.customer_deposits_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.customer_deposits_id AND company_id=NEW.company_id);
  SELECT RAISE(ABORT, 'l2_setup_account: Setup requires an existing eligible account') WHERE NEW.receivables_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=NEW.receivables_id AND a.company_id=NEW.company_id AND a.active=1 AND a.postable=1 AND a.kind='ASSET');
  SELECT RAISE(ABORT, 'l2_setup_account: Setup requires an existing eligible account') WHERE NEW.undeposited_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=NEW.undeposited_id AND a.company_id=NEW.company_id AND a.active=1 AND a.postable=1 AND a.kind='ASSET');
  SELECT RAISE(ABORT, 'l2_setup_account: Setup requires an existing eligible account') WHERE NEW.payables_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=NEW.payables_id AND a.company_id=NEW.company_id AND a.active=1 AND a.postable=1 AND a.kind='LIABILITY');
  SELECT RAISE(ABORT, 'l2_setup_account: Setup requires an existing eligible account') WHERE NEW.collected_tax_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=NEW.collected_tax_id AND a.company_id=NEW.company_id AND a.active=1 AND a.postable=1 AND a.kind='LIABILITY');
  SELECT RAISE(ABORT, 'l2_setup_account: Setup requires an existing eligible account') WHERE NEW.customer_deposits_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=NEW.customer_deposits_id AND a.company_id=NEW.company_id AND a.active=1 AND a.postable=1 AND a.kind='LIABILITY' AND a.detail_type='OTHER_CURRENT_LIABILITY');
END;

CREATE TRIGGER fb_l2_bank_insert BEFORE INSERT ON documents_designatedbank
BEGIN
  SELECT RAISE(ABORT, 'l2_bank_account: Bank requires an existing setup and eligible asset account') WHERE NOT EXISTS(SELECT 1 FROM documents_accountingsetup s JOIN ledger_account a ON a.company_id=s.company_id WHERE s.id=NEW.setup_id AND a.id=NEW.account_id AND a.active=1 AND a.postable=1 AND a.kind='ASSET');
END;

CREATE TRIGGER fb_l2_directory_item_income_account_id_update BEFORE UPDATE ON directory_item
BEGIN
  SELECT RAISE(ABORT, 'l2_account_parent: Default account must exist in this company') WHERE NEW.income_account_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.income_account_id AND company_id=NEW.company_id);
  SELECT RAISE(ABORT, 'l2_account_parent: Default requires an existing eligible account') WHERE NEW.income_account_id IS NOT NULL AND (NEW.income_account_id IS NOT OLD.income_account_id) AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=NEW.income_account_id AND a.company_id=NEW.company_id AND a.active=1 AND a.postable=1 AND a.kind='INCOME');
END;

CREATE TRIGGER fb_l2_directory_item_expense_account_id_update BEFORE UPDATE ON directory_item
BEGIN
  SELECT RAISE(ABORT, 'l2_account_parent: Default account must exist in this company') WHERE NEW.expense_account_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.expense_account_id AND company_id=NEW.company_id);
  SELECT RAISE(ABORT, 'l2_account_parent: Default requires an existing eligible account') WHERE NEW.expense_account_id IS NOT NULL AND (NEW.expense_account_id IS NOT OLD.expense_account_id) AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=NEW.expense_account_id AND a.company_id=NEW.company_id AND a.active=1 AND a.postable=1 AND a.kind='EXPENSE');
END;

CREATE TRIGGER fb_l2_directory_vendor_default_expense_account_id_update BEFORE UPDATE ON directory_vendor
BEGIN
  SELECT RAISE(ABORT, 'l2_account_parent: Default account must exist in this company') WHERE NEW.default_expense_account_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.default_expense_account_id AND company_id=NEW.company_id);
  SELECT RAISE(ABORT, 'l2_account_parent: Default requires an existing eligible account') WHERE NEW.default_expense_account_id IS NOT NULL AND (NEW.default_expense_account_id IS NOT OLD.default_expense_account_id) AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=NEW.default_expense_account_id AND a.company_id=NEW.company_id AND a.active=1 AND a.postable=1 AND a.kind IN('EXPENSE','ASSET','LIABILITY','EQUITY') AND NOT EXISTS(SELECT 1 FROM fb_control_accounts c WHERE c.company_id=NEW.company_id AND c.account_id=a.id));
END;

CREATE TRIGGER fb_l2_tax_agency_update BEFORE UPDATE ON salestax_taxitem
BEGIN
  SELECT RAISE(ABORT, 'l2_tax_agency: New agency must be active') WHERE NEW.agency_id IS NOT NULL AND NEW.agency_id IS NOT OLD.agency_id AND NOT EXISTS(SELECT 1 FROM directory_vendor WHERE id=NEW.agency_id AND company_id=NEW.company_id AND active=1);
END;

CREATE TRIGGER fb_l2_tax_default_update BEFORE UPDATE ON salestax_salestaxsettings
BEGIN
  SELECT RAISE(ABORT, 'l2_tax_default: New default tax item must be active') WHERE NEW.default_item_id IS NOT NULL AND NEW.default_item_id IS NOT OLD.default_item_id AND NOT EXISTS(SELECT 1 FROM salestax_taxitem WHERE id=NEW.default_item_id AND company_id=NEW.company_id AND active=1);
END;

CREATE TRIGGER fb_l2_tax_member_update BEFORE UPDATE ON salestax_taxgroupmember
BEGIN
  SELECT RAISE(ABORT, 'l2_tax_member: Active groups need active member rates') WHERE EXISTS(SELECT 1 FROM salestax_taxitem WHERE id=NEW.group_id AND active=1) AND NOT EXISTS(SELECT 1 FROM salestax_taxitem WHERE id=NEW.member_id AND active=1);
END;

CREATE TRIGGER fb_l2_setup_update BEFORE UPDATE ON documents_accountingsetup
BEGIN
  SELECT RAISE(ABORT, 'l2_setup_account: Setup account must exist in this company') WHERE NEW.receivables_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.receivables_id AND company_id=NEW.company_id);
  SELECT RAISE(ABORT, 'l2_setup_account: Setup account must exist in this company') WHERE NEW.payables_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.payables_id AND company_id=NEW.company_id);
  SELECT RAISE(ABORT, 'l2_setup_account: Setup account must exist in this company') WHERE NEW.undeposited_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.undeposited_id AND company_id=NEW.company_id);
  SELECT RAISE(ABORT, 'l2_setup_account: Setup account must exist in this company') WHERE NEW.collected_tax_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.collected_tax_id AND company_id=NEW.company_id);
  SELECT RAISE(ABORT, 'l2_setup_account: Setup account must exist in this company') WHERE NEW.customer_deposits_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.customer_deposits_id AND company_id=NEW.company_id);
  SELECT RAISE(ABORT, 'l2_setup_account: Setup requires an existing eligible account') WHERE NEW.receivables_id IS NOT NULL AND NEW.receivables_id IS NOT OLD.receivables_id AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=NEW.receivables_id AND a.company_id=NEW.company_id AND a.active=1 AND a.postable=1 AND a.kind='ASSET');
  SELECT RAISE(ABORT, 'l2_setup_account: Setup requires an existing eligible account') WHERE NEW.undeposited_id IS NOT NULL AND NEW.undeposited_id IS NOT OLD.undeposited_id AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=NEW.undeposited_id AND a.company_id=NEW.company_id AND a.active=1 AND a.postable=1 AND a.kind='ASSET');
  SELECT RAISE(ABORT, 'l2_setup_account: Setup requires an existing eligible account') WHERE NEW.payables_id IS NOT NULL AND NEW.payables_id IS NOT OLD.payables_id AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=NEW.payables_id AND a.company_id=NEW.company_id AND a.active=1 AND a.postable=1 AND a.kind='LIABILITY');
  SELECT RAISE(ABORT, 'l2_setup_account: Setup requires an existing eligible account') WHERE NEW.collected_tax_id IS NOT NULL AND NEW.collected_tax_id IS NOT OLD.collected_tax_id AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=NEW.collected_tax_id AND a.company_id=NEW.company_id AND a.active=1 AND a.postable=1 AND a.kind='LIABILITY');
  SELECT RAISE(ABORT, 'l2_setup_account: Setup requires an existing eligible account') WHERE NEW.customer_deposits_id IS NOT NULL AND NEW.customer_deposits_id IS NOT OLD.customer_deposits_id AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=NEW.customer_deposits_id AND a.company_id=NEW.company_id AND a.active=1 AND a.postable=1 AND a.kind='LIABILITY' AND a.detail_type='OTHER_CURRENT_LIABILITY');
END;

CREATE TRIGGER fb_l2_bank_update BEFORE UPDATE ON documents_designatedbank
BEGIN
  SELECT RAISE(ABORT, 'l2_bank_account: Bank requires an existing setup and eligible asset account') WHERE NOT EXISTS(SELECT 1 FROM documents_accountingsetup s JOIN ledger_account a ON a.company_id=s.company_id WHERE s.id=NEW.setup_id AND a.id=NEW.account_id AND a.active=1 AND a.postable=1 AND a.kind='ASSET');
END;

CREATE TRIGGER fb_l2_account_delete BEFORE DELETE ON ledger_account
WHEN EXISTS(SELECT 1 FROM directory_vendor WHERE default_expense_account_id=OLD.id) OR EXISTS(SELECT 1 FROM directory_item WHERE income_account_id=OLD.id OR expense_account_id=OLD.id) OR EXISTS(SELECT 1 FROM documents_documentaccount WHERE account_id=OLD.id) OR EXISTS(SELECT 1 FROM documents_accountingsetup WHERE OLD.id IN(receivables_id,payables_id,undeposited_id,collected_tax_id,customer_deposits_id)) OR EXISTS(SELECT 1 FROM documents_designatedbank WHERE account_id=OLD.id)
BEGIN
  SELECT RAISE(ABORT, 'l2_account_reference: L2a-referenced accounts cannot be deleted');
END;

CREATE TRIGGER fb_l2_account_identity BEFORE UPDATE ON ledger_account
WHEN (NEW.id IS NOT OLD.id OR NEW.company_id IS NOT OLD.company_id OR NEW.kind IS NOT OLD.kind) AND (EXISTS(SELECT 1 FROM directory_vendor WHERE default_expense_account_id=OLD.id) OR EXISTS(SELECT 1 FROM directory_item WHERE income_account_id=OLD.id OR expense_account_id=OLD.id) OR EXISTS(SELECT 1 FROM documents_documentaccount WHERE account_id=OLD.id) OR EXISTS(SELECT 1 FROM documents_accountingsetup WHERE OLD.id IN(receivables_id,payables_id,undeposited_id,collected_tax_id,customer_deposits_id)) OR EXISTS(SELECT 1 FROM documents_designatedbank WHERE account_id=OLD.id))
BEGIN
  SELECT RAISE(ABORT, 'l2_account_reference: L2a-referenced accounts cannot change identity company or type');
END;

CREATE TRIGGER fb_l2_company_delete BEFORE DELETE ON ledger_company
WHEN EXISTS(SELECT 1 FROM directory_customer WHERE company_id=OLD.id) OR EXISTS(SELECT 1 FROM directory_vendor WHERE company_id=OLD.id) OR EXISTS(SELECT 1 FROM directory_item WHERE company_id=OLD.id) OR EXISTS(SELECT 1 FROM salestax_taxitem WHERE company_id=OLD.id) OR EXISTS(SELECT 1 FROM salestax_salestaxsettings WHERE company_id=OLD.id) OR EXISTS(SELECT 1 FROM documents_documentsequence WHERE company_id=OLD.id) OR EXISTS(SELECT 1 FROM documents_documentidentity WHERE company_id=OLD.id) OR EXISTS(SELECT 1 FROM documents_accountingsetup WHERE company_id=OLD.id)
BEGIN
  SELECT RAISE(ABORT, 'l2_company_reference: L2a-referenced companies cannot be deleted');
END;

CREATE TRIGGER fb_l2_account_collision BEFORE UPDATE ON ledger_account
BEGIN
  SELECT RAISE(ABORT, 'l2_key_collision: Account key already exists') WHERE EXISTS(SELECT 1 FROM ledger_account WHERE id<>OLD.id AND (id=NEW.id OR (company_id=NEW.company_id AND code=NEW.code)));
END;



-- A member's eligibility also matters when the parent group is reactivated, or
-- when a rate used by an active group/default is deactivated.
CREATE TRIGGER fb_l2_tax_activation BEFORE UPDATE ON salestax_taxitem
BEGIN
  SELECT RAISE(ABORT, 'l2_tax_member: Active groups need active member rates')
  WHERE NEW.active=1 AND NEW.kind='GROUP' AND EXISTS(
    SELECT 1 FROM salestax_taxgroupmember m LEFT JOIN salestax_taxitem r ON r.id=m.member_id
    WHERE m.group_id=OLD.id AND (r.id IS NULL OR r.active<>1 OR r.kind<>'RATE' OR r.company_id<>NEW.company_id));
  SELECT RAISE(ABORT, 'l2_tax_member: Rates used by active groups must remain active')
  WHERE NEW.active=0 AND EXISTS(
    SELECT 1 FROM salestax_taxgroupmember m JOIN salestax_taxitem g ON g.id=m.group_id
    WHERE m.member_id=OLD.id AND g.active=1);
  SELECT RAISE(ABORT, 'l2_tax_default: Company default tax items must remain active')
  WHERE NEW.active=0 AND EXISTS(SELECT 1 FROM salestax_salestaxsettings WHERE default_item_id=OLD.id);
END;
