-- L2a: directory and document framework, ported from the frozen Python schema.
-- No financial arithmetic in SQL: money is integer cents, rates are 4-place integer percent.
CREATE TABLE directory_customer (id TEXT PRIMARY KEY, company_id TEXT REFERENCES ledger_company(id) DEFERRABLE INITIALLY DEFERRED NOT NULL,
display_name TEXT NOT NULL COLLATE fb_text CHECK(length(display_name)<=120), business_name TEXT NOT NULL CHECK(length(business_name)<=120) DEFAULT '', contact_name TEXT NOT NULL CHECK(length(contact_name)<=120) DEFAULT '',
email TEXT NOT NULL CHECK(length(email)<=254) DEFAULT '', phone TEXT NOT NULL CHECK(length(phone)<=50) DEFAULT '', billing_address TEXT NOT NULL DEFAULT '', notes TEXT NOT NULL DEFAULT '',
active INTEGER NOT NULL DEFAULT 1 CHECK(active IN (0,1)),
tax_status TEXT NOT NULL DEFAULT 'TAXABLE' CONSTRAINT customer_tax_status CHECK(tax_status IN ('TAXABLE','EXEMPT')),
exemption_number TEXT NOT NULL CHECK(length(exemption_number)<=40) DEFAULT '') STRICT, WITHOUT ROWID;
CREATE TABLE directory_vendor (id TEXT PRIMARY KEY, company_id TEXT REFERENCES ledger_company(id) DEFERRABLE INITIALLY DEFERRED NOT NULL,
display_name TEXT NOT NULL COLLATE fb_text CHECK(length(display_name)<=120), business_name TEXT NOT NULL CHECK(length(business_name)<=120) DEFAULT '', contact_name TEXT NOT NULL CHECK(length(contact_name)<=120) DEFAULT '',
email TEXT NOT NULL CHECK(length(email)<=254) DEFAULT '', phone TEXT NOT NULL CHECK(length(phone)<=50) DEFAULT '', billing_address TEXT NOT NULL DEFAULT '', notes TEXT NOT NULL DEFAULT '',
active INTEGER NOT NULL DEFAULT 1 CHECK(active IN (0,1)),
default_terms TEXT CONSTRAINT vendor_default_terms CHECK(default_terms IS NULL OR default_terms IN ('0','15','30','60')),
default_expense_account_id TEXT REFERENCES ledger_account(id) DEFERRABLE INITIALLY DEFERRED) STRICT, WITHOUT ROWID;
CREATE TABLE directory_item (
id TEXT PRIMARY KEY, company_id TEXT REFERENCES ledger_company(id) DEFERRABLE INITIALLY DEFERRED NOT NULL, code TEXT COLLATE fb_text NOT NULL CHECK(length(code)<=120), description TEXT NOT NULL DEFAULT '',
kind TEXT NOT NULL CONSTRAINT item_supported_kind CHECK(kind IN ('SERVICE','NONSTOCK')),
sale_rate INTEGER NOT NULL CONSTRAINT item_sale_rate_range CHECK(sale_rate BETWEEN 0 AND 9999999999999999),
purchase_cost INTEGER CONSTRAINT item_purchase_cost_range CHECK(purchase_cost IS NULL OR purchase_cost BETWEEN 0 AND 9999999999999999),
income_account_id TEXT REFERENCES ledger_account(id) DEFERRABLE INITIALLY DEFERRED, expense_account_id TEXT REFERENCES ledger_account(id) DEFERRABLE INITIALLY DEFERRED,
taxable INTEGER NOT NULL DEFAULT 1 CHECK(taxable IN (0,1)), active INTEGER NOT NULL DEFAULT 1 CHECK(active IN (0,1))) STRICT, WITHOUT ROWID;
CREATE UNIQUE INDEX item_code_ci_per_company ON directory_item(fb_lower(code),company_id);
CREATE TABLE salestax_taxitem (
id TEXT PRIMARY KEY, company_id TEXT REFERENCES ledger_company(id) DEFERRABLE INITIALLY DEFERRED NOT NULL,
kind TEXT NOT NULL CONSTRAINT salestax_item_kind CHECK(kind IN ('RATE','GROUP')), name TEXT NOT NULL COLLATE fb_text CHECK(length(name)<=60), description TEXT NOT NULL CHECK(length(description)<=120),
rate INTEGER, agency_id TEXT REFERENCES directory_vendor(id) DEFERRABLE INITIALLY DEFERRED, active INTEGER NOT NULL DEFAULT 1 CHECK(active IN(0,1)), revision INTEGER NOT NULL DEFAULT 1 CHECK(revision>=0),
CONSTRAINT salestax_item_shape CHECK((kind='RATE' AND rate IS NOT NULL AND rate BETWEEN 0 AND 1000000 AND agency_id IS NOT NULL) OR (kind='GROUP' AND rate IS NULL AND agency_id IS NULL))) STRICT, WITHOUT ROWID;
CREATE UNIQUE INDEX salestax_item_name_ci_per_company ON salestax_taxitem(fb_lower(name),company_id);
CREATE TABLE salestax_taxgroupmember (
id TEXT PRIMARY KEY, group_id TEXT REFERENCES salestax_taxitem(id) DEFERRABLE INITIALLY DEFERRED NOT NULL, member_id TEXT REFERENCES salestax_taxitem(id) DEFERRABLE INITIALLY DEFERRED NOT NULL,
position INTEGER NOT NULL CONSTRAINT salestax_group_position_range CHECK(position BETWEEN 1 AND 20),
CONSTRAINT salestax_group_member_once UNIQUE(group_id,member_id), CONSTRAINT salestax_group_position UNIQUE(group_id,position)) STRICT, WITHOUT ROWID;
CREATE TABLE salestax_salestaxsettings (
id TEXT PRIMARY KEY, company_id TEXT REFERENCES ledger_company(id) DEFERRABLE INITIALLY DEFERRED NOT NULL UNIQUE,
charge_tax INTEGER NOT NULL DEFAULT 0 CHECK(charge_tax IN(0,1)), default_item_id TEXT REFERENCES salestax_taxitem(id) DEFERRABLE INITIALLY DEFERRED,
owed_basis TEXT NOT NULL DEFAULT 'ACCRUAL' CONSTRAINT salestax_settings_basis CHECK(owed_basis IN('ACCRUAL','CASH')),
filing_frequency TEXT NOT NULL DEFAULT 'MONTHLY' CONSTRAINT salestax_settings_frequency CHECK(filing_frequency IN('MONTHLY','QUARTERLY','ANNUALLY')),
bulk_step TEXT NOT NULL DEFAULT '' CONSTRAINT salestax_settings_bulk_step CHECK(bulk_step IN('','APPLIED','SKIPPED')),
revision INTEGER NOT NULL DEFAULT 0 CHECK(revision>=0)) STRICT, WITHOUT ROWID;
CREATE TRIGGER fb_salestax_item_delete BEFORE DELETE ON salestax_taxitem
BEGIN SELECT RAISE(ABORT, 'salestax_item_no_delete: Sales tax rates and groups are never deleted; deactivate instead'); END;
CREATE TRIGGER fb_salestax_item_identity BEFORE UPDATE ON salestax_taxitem
WHEN NEW.company_id IS NOT OLD.company_id OR NEW.kind IS NOT OLD.kind
BEGIN SELECT RAISE(ABORT, 'salestax_item_identity: Sales tax company and type are permanent'); END;
CREATE TRIGGER fb_salestax_item_agency_insert BEFORE INSERT ON salestax_taxitem
WHEN NEW.agency_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM directory_vendor WHERE id=NEW.agency_id AND company_id=NEW.company_id)
BEGIN SELECT RAISE(ABORT, 'salestax_item_agency_scope: Tax agency must be a vendor of the same company'); END;
CREATE TRIGGER fb_salestax_member_insert BEFORE INSERT ON salestax_taxgroupmember
WHEN NOT EXISTS(SELECT 1 FROM salestax_taxitem g JOIN salestax_taxitem m ON m.company_id=g.company_id WHERE g.id=NEW.group_id AND g.kind='GROUP' AND m.id=NEW.member_id AND m.kind='RATE')
BEGIN SELECT RAISE(ABORT, 'salestax_member_scope: A tax group holds tax rates of the same company'); END;
CREATE TRIGGER fb_salestax_settings_default_insert BEFORE INSERT ON salestax_salestaxsettings
WHEN NEW.default_item_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM salestax_taxitem WHERE id=NEW.default_item_id AND company_id=NEW.company_id)
BEGIN SELECT RAISE(ABORT, 'salestax_settings_default_scope: Default sales tax must belong to the same company'); END;
CREATE TRIGGER fb_salestax_item_agency_update BEFORE UPDATE ON salestax_taxitem
WHEN NEW.agency_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM directory_vendor WHERE id=NEW.agency_id AND company_id=NEW.company_id)
BEGIN SELECT RAISE(ABORT, 'salestax_item_agency_scope: Tax agency must be a vendor of the same company'); END;
CREATE TRIGGER fb_salestax_member_update BEFORE UPDATE ON salestax_taxgroupmember
WHEN NOT EXISTS(SELECT 1 FROM salestax_taxitem g JOIN salestax_taxitem m ON m.company_id=g.company_id WHERE g.id=NEW.group_id AND g.kind='GROUP' AND m.id=NEW.member_id AND m.kind='RATE')
BEGIN SELECT RAISE(ABORT, 'salestax_member_scope: A tax group holds tax rates of the same company'); END;
CREATE TRIGGER fb_salestax_settings_default_update BEFORE UPDATE ON salestax_salestaxsettings
WHEN NEW.default_item_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM salestax_taxitem WHERE id=NEW.default_item_id AND company_id=NEW.company_id)
BEGIN SELECT RAISE(ABORT, 'salestax_settings_default_scope: Default sales tax must belong to the same company'); END;
CREATE TRIGGER fb_salestax_settings_delete BEFORE DELETE ON salestax_salestaxsettings
BEGIN SELECT RAISE(ABORT, 'salestax_settings_no_delete: Sales tax settings are never deleted'); END;
CREATE TRIGGER fb_salestax_settings_company BEFORE UPDATE ON salestax_salestaxsettings
WHEN NEW.company_id IS NOT OLD.company_id
BEGIN SELECT RAISE(ABORT, 'salestax_settings_company: Settings company is permanent'); END;
CREATE TABLE documents_documentsequence (
id TEXT PRIMARY KEY, company_id TEXT REFERENCES ledger_company(id) DEFERRABLE INITIALLY DEFERRED NOT NULL, kind TEXT NOT NULL CONSTRAINT document_sequence_kind CHECK(kind IN('EST','INV','RCP','BILL','BPAY','DEP','CHK','SR','TRF','CM','RFD','CDP','CDA','CDR')),
last_number INTEGER NOT NULL DEFAULT 0 CONSTRAINT document_sequence_limit CHECK(last_number BETWEEN 0 AND 999999999999),
CONSTRAINT document_sequence_per_company_kind UNIQUE(company_id,kind)) STRICT, WITHOUT ROWID;
CREATE TABLE documents_documentidentity (
id TEXT PRIMARY KEY, company_id TEXT REFERENCES ledger_company(id) DEFERRABLE INITIALLY DEFERRED NOT NULL, kind TEXT NOT NULL CONSTRAINT document_identity_kind CHECK(kind IN('EST','INV','RCP','BILL','BPAY','DEP','CHK','SR','TRF','CM','RFD','CDP','CDA','CDR')),
ordinal INTEGER NOT NULL CONSTRAINT document_identity_ordinal CHECK(ordinal BETWEEN 1 AND 999999999999), number TEXT NOT NULL CHECK(length(number)<=17),
request_key TEXT NOT NULL, currency TEXT NOT NULL, created_at TEXT NOT NULL,
CONSTRAINT document_number_per_company UNIQUE(company_id,number),CONSTRAINT document_ordinal_per_company_kind UNIQUE(company_id,kind,ordinal),CONSTRAINT document_request_per_company UNIQUE(company_id,request_key)) STRICT, WITHOUT ROWID;
CREATE TABLE documents_documentaccount (
id TEXT PRIMARY KEY, source_id TEXT REFERENCES documents_documentidentity(id) DEFERRABLE INITIALLY DEFERRED NOT NULL, role TEXT NOT NULL COLLATE fb_text CHECK(length(role)<=80),
account_id TEXT REFERENCES ledger_account(id) DEFERRABLE INITIALLY DEFERRED NOT NULL, account_code TEXT NOT NULL CHECK(length(account_code)<=20), account_name TEXT NOT NULL CHECK(length(account_name)<=120), account_kind TEXT NOT NULL CHECK(length(account_kind)<=12),
CONSTRAINT document_account_role UNIQUE(source_id,role)) STRICT, WITHOUT ROWID;
CREATE TABLE documents_accountingsetup (
id TEXT PRIMARY KEY, company_id TEXT REFERENCES ledger_company(id) DEFERRABLE INITIALLY DEFERRED NOT NULL UNIQUE,
receivables_id TEXT REFERENCES ledger_account(id) DEFERRABLE INITIALLY DEFERRED, payables_id TEXT REFERENCES ledger_account(id) DEFERRABLE INITIALLY DEFERRED, undeposited_id TEXT REFERENCES ledger_account(id) DEFERRABLE INITIALLY DEFERRED,
collected_tax_id TEXT REFERENCES ledger_account(id) DEFERRABLE INITIALLY DEFERRED, customer_deposits_id TEXT REFERENCES ledger_account(id) DEFERRABLE INITIALLY DEFERRED, revision INTEGER NOT NULL DEFAULT 0 CHECK(revision>=0)) STRICT, WITHOUT ROWID;
CREATE TABLE documents_designatedbank (
id TEXT PRIMARY KEY, setup_id TEXT NOT NULL REFERENCES documents_accountingsetup(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
account_id TEXT REFERENCES ledger_account(id) DEFERRABLE INITIALLY DEFERRED NOT NULL, CONSTRAINT designated_bank_per_setup UNIQUE(setup_id,account_id)) STRICT, WITHOUT ROWID;
CREATE TRIGGER fb_document_sequence_delete BEFORE DELETE ON documents_documentsequence
BEGIN SELECT RAISE(ABORT, 'document_sequence_delete: Document counters cannot be deleted'); END;
CREATE TRIGGER fb_document_sequence_update BEFORE UPDATE ON documents_documentsequence
WHEN NEW.company_id<>OLD.company_id OR NEW.kind<>OLD.kind OR NEW.last_number<OLD.last_number
BEGIN SELECT RAISE(ABORT, 'document_sequence_forward: Document counters cannot be reassigned or decreased'); END;
CREATE TRIGGER fb_document_identity_update BEFORE UPDATE ON documents_documentidentity
BEGIN SELECT RAISE(ABORT, 'document_identity_immutable: Document identities cannot change or be deleted, including cancelled documents'); END;
CREATE TRIGGER fb_document_identity_delete BEFORE DELETE ON documents_documentidentity
BEGIN SELECT RAISE(ABORT, 'document_identity_immutable: Document identities cannot change or be deleted, including cancelled documents'); END;
CREATE TRIGGER fb_document_account_update BEFORE UPDATE ON documents_documentaccount
BEGIN SELECT RAISE(ABORT, 'document_account_immutable: Document account snapshots are immutable'); END;
CREATE TRIGGER fb_document_account_delete BEFORE DELETE ON documents_documentaccount
BEGIN SELECT RAISE(ABORT, 'document_account_immutable: Document account snapshots are immutable'); END;
CREATE TRIGGER fb_document_identity_insert BEFORE INSERT ON documents_documentidentity
WHEN NEW.currency IS NOT (SELECT currency FROM ledger_company WHERE id=NEW.company_id) OR NEW.number<>NEW.kind||'-'||printf('%06d',NEW.ordinal)
BEGIN SELECT RAISE(ABORT, 'document_identity: Document identity must match company currency and allocated number'); END;
CREATE TRIGGER fb_document_account_insert BEFORE INSERT ON documents_documentaccount
WHEN NOT EXISTS(SELECT 1 FROM documents_documentidentity s JOIN ledger_account a ON a.company_id=s.company_id WHERE s.id=NEW.source_id AND a.id=NEW.account_id AND a.active AND a.postable AND NEW.account_code=a.code AND NEW.account_name=a.name AND NEW.account_kind=a.kind)
BEGIN SELECT RAISE(ABORT, 'document_account_snapshot: Invalid source account snapshot'); END;
CREATE TRIGGER fb_account_document_reference BEFORE UPDATE ON ledger_account
WHEN (NEW.company_id<>OLD.company_id OR NEW.kind<>OLD.kind) AND EXISTS(SELECT 1 FROM documents_documentaccount WHERE account_id=OLD.id)
BEGIN SELECT RAISE(ABORT, 'account_document_reference: Source-referenced accounts cannot change company or type'); END;
CREATE TRIGGER fb_company_document_currency BEFORE UPDATE ON ledger_company
WHEN NEW.currency<>OLD.currency AND EXISTS(SELECT 1 FROM documents_documentidentity WHERE company_id=OLD.id)
BEGIN SELECT RAISE(ABORT, 'company_currency_frozen: Recorded books and commercial documents cannot change currency'); END;
CREATE VIEW fb_control_accounts AS
SELECT company_id, receivables_id account_id, 'receivables' role, 0 priority FROM documents_accountingsetup WHERE receivables_id IS NOT NULL
UNION SELECT company_id,payables_id,'payables',1 FROM documents_accountingsetup WHERE payables_id IS NOT NULL
UNION SELECT company_id,undeposited_id,'undeposited',2 FROM documents_accountingsetup WHERE undeposited_id IS NOT NULL
UNION SELECT company_id,collected_tax_id,'collected_tax',3 FROM documents_accountingsetup WHERE collected_tax_id IS NOT NULL
UNION SELECT company_id,customer_deposits_id,'customer_deposits',4 FROM documents_accountingsetup WHERE customer_deposits_id IS NOT NULL
UNION SELECT s.company_id,d.account_id,substr(d.role,1,instr(d.role||':',':')-1),
 CASE substr(d.role,1,instr(d.role||':',':')-1) WHEN 'receivables' THEN 0 WHEN 'payables' THEN 1 WHEN 'undeposited' THEN 2 WHEN 'collected_tax' THEN 3 ELSE 4 END
FROM documents_documentaccount d JOIN documents_documentidentity s ON s.id=d.source_id
WHERE substr(d.role,1,instr(d.role||':',':')-1) IN('receivables','payables','undeposited','collected_tax','customer_deposits');
