-- Customer deposits share the receipt root, preserving every prior receipt and key.
PRAGMA defer_foreign_keys=ON;
CREATE TEMP TABLE pp_receipt_copy AS SELECT * FROM sales_receipt;
DROP TABLE sales_receipt;

CREATE TABLE sales_receipt (
 id TEXT PRIMARY KEY,company_id TEXT NOT NULL REFERENCES ledger_company(id),identity_id TEXT NOT NULL UNIQUE REFERENCES documents_documentidentity(id),
 kind TEXT NOT NULL CHECK(kind IN('RCP','SR','CDP')),customer_id TEXT NOT NULL REFERENCES directory_customer(id),date TEXT NOT NULL CHECK(date(date) IS date),
 amount INTEGER NOT NULL CHECK(amount BETWEEN 1 AND 9999999999999999),destination TEXT NOT NULL CHECK(destination IN('BANK','UNDEPOSITED')),
 destination_account_id TEXT NOT NULL REFERENCES ledger_account(id),journal_id TEXT NOT NULL UNIQUE REFERENCES ledger_journal(id),
 data TEXT NOT NULL CHECK(json_valid(data)),actor_id TEXT NOT NULL REFERENCES fb_user(id)
) STRICT, WITHOUT ROWID;
INSERT INTO sales_receipt SELECT * FROM pp_receipt_copy;
DROP TABLE pp_receipt_copy;
CREATE TRIGGER fb_sales_receipt_insert BEFORE INSERT ON sales_receipt
BEGIN
 SELECT RAISE(ABORT,'receipt_immutable: Receipt history is immutable') WHERE EXISTS(SELECT 1 FROM sales_receipt WHERE id=NEW.id OR identity_id=NEW.identity_id OR journal_id=NEW.journal_id);
 SELECT RAISE(ABORT,'receipt_scope: Invalid receipt ownership/posting') WHERE NOT EXISTS(SELECT 1 FROM documents_documentidentity d JOIN directory_customer c ON c.id=NEW.customer_id JOIN ledger_account a ON a.id=NEW.destination_account_id JOIN ledger_journal j ON j.id=NEW.journal_id WHERE d.id=NEW.identity_id AND d.company_id=NEW.company_id AND d.kind=NEW.kind AND c.company_id=NEW.company_id AND c.active=1 AND a.company_id=NEW.company_id AND a.kind='ASSET' AND a.active=1 AND a.postable=1 AND j.company_id=NEW.company_id AND j.source_id=d.id AND j.status='POSTED' AND j.reversal_of_id IS NULL AND j.date=NEW.date);
 SELECT RAISE(ABORT,'receipt_fields: Invalid receipt fields') WHERE json_extract(NEW.data,'$.id') IS NOT NEW.id OR json_extract(NEW.data,'$.identity_id') IS NOT NEW.identity_id OR json_extract(NEW.data,'$.customer_id') IS NOT NEW.customer_id OR json_extract(NEW.data,'$.kind') IS NOT NEW.kind OR json_extract(NEW.data,'$.date') IS NOT NEW.date OR json_extract(NEW.data,'$.destination') IS NOT NEW.destination OR json_extract(NEW.data,'$.destination_account_id') IS NOT NEW.destination_account_id OR json_extract(NEW.data,'$.journal_id') IS NOT NEW.journal_id OR fb_sales_money(json_quote(json_extract(NEW.data,'$.amount'))) IS NOT NEW.amount OR json_extract(NEW.data,'$.method') NOT IN('CHECK','CASH','CARD','TRANSFER','OTHER') OR length(json_extract(NEW.data,'$.reference'))>120 OR length(json_extract(NEW.data,'$.memo'))>2000 OR length(json_extract(NEW.data,'$.creation_digest'))<>64 OR length(json_extract(NEW.data,'$.review_digest'))<>64;
END;
CREATE TRIGGER fb_sales_receipt_due AFTER INSERT ON sales_receipt
BEGIN INSERT INTO sales_seal_due(kind,doc_id) VALUES('sales_receipt',NEW.id); END;
CREATE TRIGGER fb_sales_receipt_update BEFORE UPDATE ON sales_receipt
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales history is immutable'); END;
CREATE TRIGGER fb_sales_receipt_delete BEFORE DELETE ON sales_receipt
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales history is immutable'); END;
CREATE TRIGGER fb_sales_receipt_shape BEFORE INSERT ON sales_receipt
BEGIN SELECT RAISE(ABORT,'receipt_fields: Invalid receipt fields') WHERE NOT fb_sales_header_ok('receipt',NEW.data); END;
CREATE TRIGGER fb_sales_receipt_permanent_id BEFORE UPDATE OF id ON sales_receipt
BEGIN SELECT RAISE(ABORT,'sales_history_immutable: Sales primary keys are permanent'); END;

CREATE TABLE prepayments_customerdeposit (
 id TEXT PRIMARY KEY, receipt_id TEXT NOT NULL UNIQUE REFERENCES sales_receipt(id),
 company_id TEXT NOT NULL REFERENCES ledger_company(id), liability_id TEXT NOT NULL REFERENCES ledger_account(id),
 estimate_id TEXT CHECK(estimate_id IS NULL), order_reference TEXT NOT NULL CHECK(length(order_reference)<=120)
) STRICT, WITHOUT ROWID;
CREATE TABLE prepayments_depositapplication (
 id TEXT PRIMARY KEY, company_id TEXT NOT NULL REFERENCES ledger_company(id),
 identity_id TEXT NOT NULL UNIQUE REFERENCES documents_documentidentity(id),
 deposit_id TEXT NOT NULL REFERENCES sales_receipt(id),
 invoice_id TEXT NOT NULL REFERENCES sales_invoice(id), invoice_revision_id TEXT NOT NULL REFERENCES sales_invoicerevision(id),
 receivables_id TEXT NOT NULL REFERENCES ledger_account(id), liability_id TEXT NOT NULL REFERENCES ledger_account(id),
 date TEXT NOT NULL CHECK(date(date) IS date), amount INTEGER NOT NULL CHECK(amount BETWEEN 1 AND 9999999999999999),
 journal_id TEXT NOT NULL UNIQUE REFERENCES ledger_journal(id), actor_id TEXT NOT NULL REFERENCES fb_user(id),
 data TEXT NOT NULL CHECK(json_valid(data))
) STRICT, WITHOUT ROWID;
CREATE TABLE prepayments_depositrefund (
 id TEXT PRIMARY KEY, company_id TEXT NOT NULL REFERENCES ledger_company(id),
 identity_id TEXT NOT NULL UNIQUE REFERENCES documents_documentidentity(id),
 deposit_id TEXT NOT NULL REFERENCES sales_receipt(id), customer_id TEXT NOT NULL REFERENCES directory_customer(id),
 bank_id TEXT NOT NULL REFERENCES ledger_account(id), liability_id TEXT NOT NULL REFERENCES ledger_account(id),
 date TEXT NOT NULL CHECK(date(date) IS date), amount INTEGER NOT NULL CHECK(amount BETWEEN 1 AND 9999999999999999),
 method TEXT NOT NULL CHECK(method IN('CHECK','CASH','OTHER')), check_number TEXT NOT NULL CHECK(length(check_number)<=20 AND (method='CHECK' OR check_number='')),
 journal_id TEXT NOT NULL UNIQUE REFERENCES ledger_journal(id), actor_id TEXT NOT NULL REFERENCES fb_user(id),
 data TEXT NOT NULL CHECK(json_valid(data))
) STRICT, WITHOUT ROWID;
CREATE TABLE prepayments_depositunapplication (
 id TEXT PRIMARY KEY, application_id TEXT NOT NULL UNIQUE REFERENCES prepayments_depositapplication(id),
 journal_id TEXT NOT NULL UNIQUE REFERENCES ledger_journal(id), date TEXT NOT NULL CHECK(date(date) IS date),
 actor_id TEXT NOT NULL REFERENCES fb_user(id), data TEXT NOT NULL CHECK(json_valid(data))
) STRICT, WITHOUT ROWID;
CREATE TABLE prepayments_depositrefundvoid (
 id TEXT PRIMARY KEY, refund_id TEXT NOT NULL UNIQUE REFERENCES prepayments_depositrefund(id),
 journal_id TEXT NOT NULL UNIQUE REFERENCES ledger_journal(id), date TEXT NOT NULL CHECK(date(date) IS date),
 actor_id TEXT NOT NULL REFERENCES fb_user(id), data TEXT NOT NULL CHECK(json_valid(data))
) STRICT, WITHOUT ROWID;
CREATE TABLE prepayments_seal(kind TEXT NOT NULL,doc_id TEXT NOT NULL,PRIMARY KEY(kind,doc_id)) STRICT, WITHOUT ROWID;
CREATE TABLE prepayments_due(kind TEXT NOT NULL,doc_id TEXT NOT NULL,PRIMARY KEY(kind,doc_id),
 FOREIGN KEY(kind,doc_id) REFERENCES prepayments_seal(kind,doc_id) DEFERRABLE INITIALLY DEFERRED) STRICT, WITHOUT ROWID;

CREATE VIEW prepayments_live_use AS
 SELECT deposit_id,amount FROM prepayments_depositapplication a WHERE NOT EXISTS(SELECT 1 FROM prepayments_depositunapplication u WHERE u.application_id=a.id)
 UNION ALL SELECT deposit_id,amount FROM prepayments_depositrefund r WHERE NOT EXISTS(SELECT 1 FROM prepayments_depositrefundvoid v WHERE v.refund_id=r.id);
CREATE VIEW prepayments_live_settlements AS
 SELECT invoice_id,amount FROM prepayments_depositapplication a WHERE NOT EXISTS(SELECT 1 FROM prepayments_depositunapplication u WHERE u.application_id=a.id)
 UNION ALL SELECT invoice_id,amount FROM sales_receiptallocation a WHERE NOT EXISTS(SELECT 1 FROM sales_receiptreversal r WHERE r.receipt_id=a.receipt_id);
CREATE VIEW prepayments_releases AS
 SELECT a.deposit_id,u.date FROM prepayments_depositunapplication u JOIN prepayments_depositapplication a ON a.id=u.application_id
 UNION ALL SELECT r.deposit_id,v.date FROM prepayments_depositrefundvoid v JOIN prepayments_depositrefund r ON r.id=v.refund_id;

DROP VIEW sales_expected_entries;
CREATE VIEW sales_expected_entries AS
 SELECT i.identity_id source_id,r.journal_id,d.account_id,r.total debit,0 credit FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.state='OPEN' JOIN documents_documentaccount d ON d.source_id=i.identity_id AND d.role='receivables'
 UNION ALL SELECT i.identity_id,r.journal_id,l.income_account_id,0,sum(l.amount) FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.state='OPEN' JOIN sales_invoiceline l ON l.revision_id=r.id GROUP BY r.id,l.income_account_id HAVING sum(l.amount)>0
 UNION ALL SELECT i.identity_id,r.journal_id,d.account_id,0,fb_sales_money(json_quote(json_extract(r.data,'$.tax'))) FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.state='OPEN' JOIN documents_documentaccount d ON d.source_id=i.identity_id AND d.role='collected_tax' WHERE fb_sales_money(json_quote(json_extract(r.data,'$.tax')))>0
 UNION ALL SELECT r.identity_id,r.journal_id,r.destination_account_id,r.amount,0 FROM sales_receipt r
 UNION ALL SELECT r.identity_id,r.journal_id,a.receivables_id,0,sum(a.amount) FROM sales_receipt r JOIN sales_receiptallocation a ON a.receipt_id=r.id WHERE r.kind='RCP' GROUP BY r.id,a.receivables_id
 UNION ALL SELECT r.identity_id,r.journal_id,l.income_account_id,0,sum(l.amount) FROM sales_receipt r JOIN sales_salesreceipt s ON s.receipt_id=r.id JOIN sales_salesreceiptline l ON l.sale_id=s.id GROUP BY r.id,l.income_account_id HAVING sum(l.amount)>0
 UNION ALL SELECT r.identity_id,r.journal_id,json_extract(s.data,'$.collected_tax_id'),0,fb_sales_money(json_quote(json_extract(s.data,'$.tax'))) FROM sales_receipt r JOIN sales_salesreceipt s ON s.receipt_id=r.id WHERE fb_sales_money(json_quote(json_extract(s.data,'$.tax')))>0 UNION ALL SELECT r.identity_id,r.journal_id,d.liability_id,0,r.amount FROM sales_receipt r JOIN prepayments_customerdeposit d ON d.receipt_id=r.id;
DROP VIEW sales_expected_references;
CREATE VIEW sales_expected_references AS
 SELECT i.identity_id source_id,'receivables' role,d.account_id FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.state='OPEN' JOIN documents_documentaccount d ON d.source_id=i.identity_id AND d.role='receivables'
 UNION ALL SELECT i.identity_id,'income:'||l.position,l.income_account_id FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.state='OPEN' JOIN sales_invoiceline l ON l.revision_id=r.id
 UNION ALL SELECT i.identity_id,'collected_tax',d.account_id FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.state='OPEN' JOIN documents_documentaccount d ON d.source_id=i.identity_id AND d.role='collected_tax' WHERE fb_sales_money(json_quote(json_extract(r.data,'$.tax')))>0
 UNION ALL SELECT identity_id,'destination',destination_account_id FROM sales_receipt
 UNION SELECT r.identity_id,'receivables:'||a.receivables_id,a.receivables_id FROM sales_receipt r JOIN sales_receiptallocation a ON a.receipt_id=r.id
 UNION SELECT r.identity_id,'income:'||l.income_account_id,l.income_account_id FROM sales_receipt r JOIN sales_salesreceipt s ON s.receipt_id=r.id JOIN sales_salesreceiptline l ON l.sale_id=s.id
 UNION ALL SELECT r.identity_id,'collected_tax',json_extract(s.data,'$.collected_tax_id') FROM sales_receipt r JOIN sales_salesreceipt s ON s.receipt_id=r.id WHERE json_extract(s.data,'$.collected_tax_id') IS NOT NULL UNION ALL SELECT r.identity_id,'customer_deposits',d.liability_id FROM sales_receipt r JOIN prepayments_customerdeposit d ON d.receipt_id=r.id;

CREATE TRIGGER pp_customerdeposit_collision BEFORE INSERT ON prepayments_customerdeposit
BEGIN SELECT RAISE(ABORT,'customer_deposit_immutable: Customer deposit history is immutable') WHERE EXISTS(SELECT 1 FROM prepayments_customerdeposit WHERE id=NEW.id OR receipt_id=NEW.receipt_id); END;
CREATE TRIGGER pp_customerdeposit_update BEFORE UPDATE ON prepayments_customerdeposit
BEGIN SELECT RAISE(ABORT,'customer_deposit_immutable: Customer deposit history is immutable'); END;
CREATE TRIGGER pp_customerdeposit_delete BEFORE DELETE ON prepayments_customerdeposit
BEGIN SELECT RAISE(ABORT,'customer_deposit_immutable: Customer deposit history is immutable'); END;


CREATE TRIGGER pp_depositapplication_collision BEFORE INSERT ON prepayments_depositapplication
BEGIN SELECT RAISE(ABORT,'customer_deposit_immutable: Customer deposit history is immutable') WHERE EXISTS(SELECT 1 FROM prepayments_depositapplication WHERE id=NEW.id OR identity_id=NEW.identity_id OR journal_id=NEW.journal_id); END;
CREATE TRIGGER pp_depositapplication_update BEFORE UPDATE ON prepayments_depositapplication
BEGIN SELECT RAISE(ABORT,'customer_deposit_immutable: Customer deposit history is immutable'); END;
CREATE TRIGGER pp_depositapplication_delete BEFORE DELETE ON prepayments_depositapplication
BEGIN SELECT RAISE(ABORT,'customer_deposit_immutable: Customer deposit history is immutable'); END;


CREATE TRIGGER pp_depositrefund_collision BEFORE INSERT ON prepayments_depositrefund
BEGIN SELECT RAISE(ABORT,'customer_deposit_immutable: Customer deposit history is immutable') WHERE EXISTS(SELECT 1 FROM prepayments_depositrefund WHERE id=NEW.id OR identity_id=NEW.identity_id OR journal_id=NEW.journal_id); END;
CREATE TRIGGER pp_depositrefund_update BEFORE UPDATE ON prepayments_depositrefund
BEGIN SELECT RAISE(ABORT,'customer_deposit_immutable: Customer deposit history is immutable'); END;
CREATE TRIGGER pp_depositrefund_delete BEFORE DELETE ON prepayments_depositrefund
BEGIN SELECT RAISE(ABORT,'customer_deposit_immutable: Customer deposit history is immutable'); END;


CREATE TRIGGER pp_depositunapplication_collision BEFORE INSERT ON prepayments_depositunapplication
BEGIN SELECT RAISE(ABORT,'customer_deposit_immutable: Customer deposit history is immutable') WHERE EXISTS(SELECT 1 FROM prepayments_depositunapplication WHERE id=NEW.id OR application_id=NEW.application_id OR journal_id=NEW.journal_id); END;
CREATE TRIGGER pp_depositunapplication_update BEFORE UPDATE ON prepayments_depositunapplication
BEGIN SELECT RAISE(ABORT,'customer_deposit_immutable: Customer deposit history is immutable'); END;
CREATE TRIGGER pp_depositunapplication_delete BEFORE DELETE ON prepayments_depositunapplication
BEGIN SELECT RAISE(ABORT,'customer_deposit_immutable: Customer deposit history is immutable'); END;


CREATE TRIGGER pp_depositrefundvoid_collision BEFORE INSERT ON prepayments_depositrefundvoid
BEGIN SELECT RAISE(ABORT,'customer_deposit_immutable: Customer deposit history is immutable') WHERE EXISTS(SELECT 1 FROM prepayments_depositrefundvoid WHERE id=NEW.id OR refund_id=NEW.refund_id OR journal_id=NEW.journal_id); END;
CREATE TRIGGER pp_depositrefundvoid_update BEFORE UPDATE ON prepayments_depositrefundvoid
BEGIN SELECT RAISE(ABORT,'customer_deposit_immutable: Customer deposit history is immutable'); END;
CREATE TRIGGER pp_depositrefundvoid_delete BEFORE DELETE ON prepayments_depositrefundvoid
BEGIN SELECT RAISE(ABORT,'customer_deposit_immutable: Customer deposit history is immutable'); END;


CREATE TRIGGER pp_prepayments_seal_collision BEFORE INSERT ON prepayments_seal
BEGIN SELECT RAISE(ABORT,'prepayment_seal_immutable: Prepayment seals are permanent') WHERE EXISTS(SELECT 1 FROM prepayments_seal WHERE kind=NEW.kind AND doc_id=NEW.doc_id); END;
CREATE TRIGGER pp_prepayments_seal_update BEFORE UPDATE ON prepayments_seal
BEGIN SELECT RAISE(ABORT,'prepayment_seal_immutable: Prepayment seals are permanent'); END;
CREATE TRIGGER pp_prepayments_seal_delete BEFORE DELETE ON prepayments_seal
BEGIN SELECT RAISE(ABORT,'prepayment_seal_immutable: Prepayment seals are permanent'); END;


CREATE TRIGGER pp_prepayments_due_collision BEFORE INSERT ON prepayments_due
BEGIN SELECT RAISE(ABORT,'prepayment_seal_immutable: Prepayment seals are permanent') WHERE EXISTS(SELECT 1 FROM prepayments_due WHERE kind=NEW.kind AND doc_id=NEW.doc_id); END;
CREATE TRIGGER pp_prepayments_due_update BEFORE UPDATE ON prepayments_due
BEGIN SELECT RAISE(ABORT,'prepayment_seal_immutable: Prepayment seals are permanent'); END;
CREATE TRIGGER pp_prepayments_due_delete BEFORE DELETE ON prepayments_due
BEGIN SELECT RAISE(ABORT,'prepayment_seal_immutable: Prepayment seals are permanent'); END;


CREATE TRIGGER pp_detail BEFORE INSERT ON prepayments_customerdeposit
BEGIN
 SELECT RAISE(ABORT,'customer_deposit_scope: Invalid customer deposit detail') WHERE NOT EXISTS(
 SELECT 1 FROM sales_receipt r JOIN ledger_account a ON a.id=NEW.liability_id JOIN documents_accountingsetup s ON s.company_id=r.company_id
 WHERE r.id=NEW.receipt_id AND r.kind='CDP' AND r.company_id=NEW.company_id AND s.customer_deposits_id=a.id
 AND a.company_id=r.company_id AND a.kind='LIABILITY' AND a.detail_type='OTHER_CURRENT_LIABILITY' AND a.active=1 AND a.postable=1
 AND NOT EXISTS(SELECT 1 FROM sales_seal WHERE kind='sales_receipt' AND doc_id=r.id));
END;
CREATE TRIGGER pp_receipt_complete BEFORE INSERT ON sales_seal WHEN NEW.kind='sales_receipt'
BEGIN
 SELECT RAISE(ABORT,'customer_deposit_complete: A customer deposit needs exactly one deposit detail row') WHERE EXISTS(
 SELECT 1 FROM sales_receipt r WHERE r.id=NEW.doc_id AND r.kind='CDP' AND (NOT EXISTS(SELECT 1 FROM prepayments_customerdeposit d WHERE d.receipt_id=r.id AND d.company_id=r.company_id)
 OR EXISTS(SELECT 1 FROM sales_receiptallocation WHERE receipt_id=r.id) OR EXISTS(SELECT 1 FROM sales_salesreceipt WHERE receipt_id=r.id)));
END;
CREATE TRIGGER pp_application_scope BEFORE INSERT ON prepayments_depositapplication
BEGIN
 SELECT RAISE(ABORT,'deposit_application_scope: Invalid deposit application') WHERE NOT EXISTS(
 SELECT 1 FROM sales_receipt r JOIN prepayments_customerdeposit d ON d.receipt_id=r.id
 JOIN documents_documentidentity i ON i.id=NEW.identity_id JOIN ledger_journal j ON j.id=NEW.journal_id
 JOIN ledger_account l ON l.id=NEW.liability_id JOIN ledger_account ar ON ar.id=NEW.receivables_id
 WHERE r.id=NEW.deposit_id AND r.company_id=NEW.company_id AND r.kind='CDP' AND d.liability_id=NEW.liability_id
 AND i.company_id=r.company_id AND i.kind='CDA' AND j.company_id=r.company_id AND j.source_id=i.id AND j.status='POSTED' AND j.date=NEW.date AND j.reversal_of_id IS NULL
 AND l.active=1 AND l.postable=1 AND ar.active=1 AND ar.postable=1 AND l.company_id=r.company_id AND ar.company_id=r.company_id AND l.id<>ar.id
 AND NEW.date>=r.date AND NOT EXISTS(SELECT 1 FROM sales_receiptreversal z WHERE z.receipt_id=r.id));
 SELECT RAISE(ABORT,'deposit_application_target: Apply deposits only to the same customer open invoice and original AR') WHERE NOT EXISTS(
 SELECT 1 FROM sales_invoice i JOIN sales_invoicerevision v ON v.invoice_id=i.id AND v.number=i.revision
 JOIN sales_receipt r ON r.id=NEW.deposit_id JOIN documents_documentaccount a ON a.source_id=i.identity_id AND a.role='receivables'
 WHERE i.id=NEW.invoice_id AND i.company_id=NEW.company_id AND v.id=NEW.invoice_revision_id AND v.state='OPEN' AND v.customer_id=r.customer_id AND v.date<=NEW.date AND a.account_id=NEW.receivables_id);
 SELECT RAISE(ABORT,'customer_deposit_overused: Customer deposit cannot be overused') WHERE NOT fb_pp_within((SELECT amount FROM sales_receipt WHERE id=NEW.deposit_id),NEW.amount,(SELECT json_group_array(amount) FROM prepayments_live_use WHERE deposit_id=NEW.deposit_id));
 SELECT RAISE(ABORT,'invoice_overcredited: Invoice cannot be oversettled') WHERE NOT fb_pp_within((SELECT total FROM sales_invoicerevision WHERE id=NEW.invoice_revision_id),NEW.amount,(SELECT json_group_array(amount) FROM prepayments_live_settlements WHERE invoice_id=NEW.invoice_id));
END;
CREATE TRIGGER pp_refund_scope BEFORE INSERT ON prepayments_depositrefund
BEGIN
 SELECT RAISE(ABORT,'deposit_refund_scope: Invalid deposit refund') WHERE NOT EXISTS(
 SELECT 1 FROM sales_receipt r JOIN prepayments_customerdeposit d ON d.receipt_id=r.id JOIN documents_documentidentity i ON i.id=NEW.identity_id
 JOIN ledger_journal j ON j.id=NEW.journal_id JOIN ledger_account l ON l.id=NEW.liability_id
 WHERE r.id=NEW.deposit_id AND r.company_id=NEW.company_id AND r.customer_id=NEW.customer_id AND d.liability_id=NEW.liability_id
 AND i.company_id=r.company_id AND i.kind='CDR' AND j.source_id=i.id AND j.company_id=r.company_id AND j.status='POSTED' AND j.reversal_of_id IS NULL AND j.date=NEW.date
 AND l.active=1 AND l.postable=1 AND NEW.date>=r.date AND NOT EXISTS(SELECT 1 FROM sales_receiptreversal z WHERE z.receipt_id=r.id));
 SELECT RAISE(ABORT,'deposit_refund_bank: A refund requires an eligible designated bank') WHERE NOT EXISTS(
 SELECT 1 FROM ledger_account a JOIN documents_designatedbank b ON b.account_id=a.id JOIN documents_accountingsetup setup ON setup.id=b.setup_id WHERE a.id=NEW.bank_id AND a.company_id=NEW.company_id AND setup.company_id=NEW.company_id
 AND a.kind='ASSET' AND a.active=1 AND a.postable=1 AND NOT EXISTS(SELECT 1 FROM fb_control_accounts c WHERE c.company_id=NEW.company_id AND c.account_id=a.id));
 SELECT RAISE(ABORT,'customer_deposit_overused: Customer deposit cannot be overused') WHERE NOT fb_pp_within((SELECT amount FROM sales_receipt WHERE id=NEW.deposit_id),NEW.amount,(SELECT json_group_array(amount) FROM prepayments_live_use WHERE deposit_id=NEW.deposit_id));
 SELECT RAISE(ABORT,'deposit_refund_number_in_use: Check number is already in use') WHERE NEW.check_number<>'' AND (
 EXISTS(SELECT 1 FROM prepayments_depositrefund r WHERE r.company_id=NEW.company_id AND r.bank_id=NEW.bank_id AND r.check_number=NEW.check_number AND NOT EXISTS(SELECT 1 FROM prepayments_depositrefundvoid v WHERE v.refund_id=r.id))
 OR EXISTS(SELECT 1 FROM fb_payables_live_checks p WHERE p.company_id=NEW.company_id AND p.bank_id=NEW.bank_id AND p.number=NEW.check_number));
END;
CREATE TRIGGER pp_payment_number BEFORE INSERT ON payables_billpayment
BEGIN SELECT RAISE(ABORT,'bill_payment_deposit_number: Check number is already used by a deposit refund') WHERE NEW.method='CHECK' AND NEW.reference<>'' AND EXISTS(
 SELECT 1 FROM prepayments_depositrefund r WHERE r.company_id=NEW.company_id AND r.bank_id=NEW.bank_id AND r.check_number=NEW.reference AND NOT EXISTS(SELECT 1 FROM prepayments_depositrefundvoid v WHERE v.refund_id=r.id)); END;
CREATE TRIGGER pp_allocation_bound BEFORE INSERT ON sales_receiptallocation
WHEN EXISTS(SELECT 1 FROM prepayments_depositapplication a WHERE a.invoice_id=NEW.invoice_id AND NOT EXISTS(SELECT 1 FROM prepayments_depositunapplication u WHERE u.application_id=a.id))
BEGIN SELECT RAISE(ABORT,'invoice_overcredited: Invoice cannot be oversettled') WHERE NOT fb_pp_within((SELECT total FROM sales_invoicerevision WHERE id=NEW.invoice_revision_id),NEW.amount,(SELECT json_group_array(amount) FROM prepayments_live_settlements WHERE invoice_id=NEW.invoice_id)); END;
CREATE TRIGGER pp_deposit_reversal BEFORE INSERT ON sales_receiptreversal WHEN (SELECT kind FROM sales_receipt WHERE id=NEW.receipt_id)='CDP'
BEGIN
 SELECT RAISE(ABORT,'customer_deposit_live_dependency: Unapply and void refunds before voiding a customer deposit') WHERE EXISTS(SELECT 1 FROM prepayments_live_use WHERE deposit_id=NEW.receipt_id);
 SELECT RAISE(ABORT,'customer_deposit_release_date: Customer deposit void cannot precede its last release') WHERE EXISTS(SELECT 1 FROM prepayments_releases WHERE deposit_id=NEW.receipt_id AND date>NEW.date);
END;
CREATE TRIGGER pp_invoice_void BEFORE INSERT ON sales_invoicerevision WHEN NEW.state='VOID'
BEGIN
 SELECT RAISE(ABORT,'invoice_live_deposits: Unapply customer deposits before voiding an invoice') WHERE EXISTS(SELECT 1 FROM prepayments_depositapplication a WHERE a.invoice_id=NEW.invoice_id AND NOT EXISTS(SELECT 1 FROM prepayments_depositunapplication u WHERE u.application_id=a.id));
 SELECT RAISE(ABORT,'invoice_deposit_release_date: Invoice void cannot precede its last deposit unapply') WHERE EXISTS(SELECT 1 FROM prepayments_depositapplication a JOIN prepayments_depositunapplication u ON u.application_id=a.id JOIN ledger_journal j ON j.id=NEW.reversal_id WHERE a.invoice_id=NEW.invoice_id AND u.date>j.date);
END;


CREATE TRIGGER pp_depositunapplication_link BEFORE INSERT ON prepayments_depositunapplication
BEGIN SELECT RAISE(ABORT,'deposit_unapplication_link: Invalid linked prepayment reversal') WHERE NOT EXISTS(
 SELECT 1 FROM prepayments_depositapplication p JOIN ledger_journal j ON j.id=NEW.journal_id
 WHERE p.id=NEW.application_id AND j.source_id=p.identity_id AND j.company_id=p.company_id AND j.status='POSTED' AND j.reversal_of_id=p.journal_id AND j.date=NEW.date AND NEW.date>=p.date
 AND fb_pp_reversal_fields(NEW.data,NEW.id,NEW.application_id,NEW.journal_id,NEW.date)); END;


CREATE TRIGGER pp_depositrefundvoid_link BEFORE INSERT ON prepayments_depositrefundvoid
BEGIN SELECT RAISE(ABORT,'deposit_refund_void_link: Invalid linked prepayment reversal') WHERE NOT EXISTS(
 SELECT 1 FROM prepayments_depositrefund p JOIN ledger_journal j ON j.id=NEW.journal_id
 WHERE p.id=NEW.refund_id AND j.source_id=p.identity_id AND j.company_id=p.company_id AND j.status='POSTED' AND j.reversal_of_id=p.journal_id AND j.date=NEW.date AND NEW.date>=p.date
 AND fb_pp_reversal_fields(NEW.data,NEW.id,NEW.refund_id,NEW.journal_id,NEW.date)); END;


CREATE TRIGGER pp_depositapplication_fields BEFORE INSERT ON prepayments_depositapplication
BEGIN SELECT RAISE(ABORT,'prepayment_fields: Invalid prepayment evidence') WHERE json_extract(NEW.data,'$.id') IS NOT NEW.id OR json_extract(NEW.data,'$.company_id') IS NOT NEW.company_id OR json_extract(NEW.data,'$.identity_id') IS NOT NEW.identity_id OR json_extract(NEW.data,'$.deposit_id') IS NOT NEW.deposit_id OR json_extract(NEW.data,'$.invoice_id') IS NOT NEW.invoice_id OR json_extract(NEW.data,'$.invoice_revision_id') IS NOT NEW.invoice_revision_id OR json_extract(NEW.data,'$.receivables_id') IS NOT NEW.receivables_id OR json_extract(NEW.data,'$.liability_id') IS NOT NEW.liability_id OR json_extract(NEW.data,'$.date') IS NOT NEW.date OR json_extract(NEW.data,'$.journal_id') IS NOT NEW.journal_id OR json_extract(NEW.data,'$.actor_id') IS NOT NEW.actor_id OR fb_sales_money(json_quote(json_extract(NEW.data,'$.amount'))) IS NOT NEW.amount OR NOT fb_pp_fields(NEW.data); END;


CREATE TRIGGER pp_depositrefund_fields BEFORE INSERT ON prepayments_depositrefund
BEGIN SELECT RAISE(ABORT,'prepayment_fields: Invalid prepayment evidence') WHERE json_extract(NEW.data,'$.id') IS NOT NEW.id OR json_extract(NEW.data,'$.company_id') IS NOT NEW.company_id OR json_extract(NEW.data,'$.identity_id') IS NOT NEW.identity_id OR json_extract(NEW.data,'$.deposit_id') IS NOT NEW.deposit_id OR json_extract(NEW.data,'$.customer_id') IS NOT NEW.customer_id OR json_extract(NEW.data,'$.bank_id') IS NOT NEW.bank_id OR json_extract(NEW.data,'$.liability_id') IS NOT NEW.liability_id OR json_extract(NEW.data,'$.date') IS NOT NEW.date OR json_extract(NEW.data,'$.method') IS NOT NEW.method OR json_extract(NEW.data,'$.check_number') IS NOT NEW.check_number OR json_extract(NEW.data,'$.journal_id') IS NOT NEW.journal_id OR json_extract(NEW.data,'$.actor_id') IS NOT NEW.actor_id OR fb_sales_money(json_quote(json_extract(NEW.data,'$.amount'))) IS NOT NEW.amount OR NOT fb_pp_fields(NEW.data); END;


CREATE VIEW prepayments_expected_entries AS
 SELECT identity_id source_id,journal_id,liability_id account_id,amount debit,0 credit FROM prepayments_depositapplication
 UNION ALL SELECT identity_id,journal_id,receivables_id,0,amount FROM prepayments_depositapplication
 UNION ALL SELECT identity_id,journal_id,liability_id,amount,0 FROM prepayments_depositrefund
 UNION ALL SELECT identity_id,journal_id,bank_id,0,amount FROM prepayments_depositrefund;
CREATE VIEW prepayments_expected_references AS
 SELECT identity_id source_id,'customer_deposits' role,liability_id account_id FROM prepayments_depositapplication
 UNION ALL SELECT identity_id,'receivables',receivables_id FROM prepayments_depositapplication
 UNION ALL SELECT identity_id,'customer_deposits',liability_id FROM prepayments_depositrefund
 UNION ALL SELECT identity_id,'bank',bank_id FROM prepayments_depositrefund;
CREATE VIEW prepayments_journal_mismatch AS
 SELECT j.id FROM ledger_journal j WHERE EXISTS(SELECT 1 FROM prepayments_expected_entries e WHERE e.journal_id=j.id) AND (
 (SELECT count(*) FROM ledger_journalline WHERE journal_id=j.id)<>2
 OR EXISTS(SELECT account_id,debit,credit FROM ledger_journalline WHERE journal_id=j.id EXCEPT SELECT account_id,debit,credit FROM prepayments_expected_entries WHERE journal_id=j.id)
 OR EXISTS(SELECT account_id,debit,credit FROM prepayments_expected_entries WHERE journal_id=j.id EXCEPT SELECT account_id,debit,credit FROM ledger_journalline WHERE journal_id=j.id)
 OR EXISTS(SELECT role,account_id FROM documents_documentaccount WHERE source_id=j.source_id EXCEPT SELECT role,account_id FROM prepayments_expected_references WHERE source_id=j.source_id)
 OR EXISTS(SELECT role,account_id FROM prepayments_expected_references WHERE source_id=j.source_id EXCEPT SELECT role,account_id FROM documents_documentaccount WHERE source_id=j.source_id));
CREATE TRIGGER pp_journal_due AFTER INSERT ON ledger_journal WHEN (SELECT kind FROM documents_documentidentity WHERE id=NEW.source_id) IN('CDP','CDA','CDR')
BEGIN INSERT INTO prepayments_due VALUES('journal',NEW.id); END;
CREATE TRIGGER pp_journal_seal BEFORE INSERT ON prepayments_seal
BEGIN
 SELECT RAISE(ABORT,'prepayment_journal_owner: A prepayment journal requires its complete owning lifecycle') WHERE NEW.kind<>'journal' OR NOT EXISTS(
 SELECT 1 FROM ledger_journal j WHERE j.id=NEW.doc_id AND j.status='POSTED' AND (
 EXISTS(SELECT 1 FROM sales_receipt r JOIN prepayments_customerdeposit d ON d.receipt_id=r.id JOIN sales_seal s ON s.kind='sales_receipt' AND s.doc_id=r.id WHERE r.journal_id=j.id AND r.identity_id=j.source_id)
 OR EXISTS(SELECT 1 FROM sales_receiptreversal z JOIN sales_receipt r ON r.id=z.receipt_id JOIN sales_seal s ON s.kind='sales_receipt_reversal' AND s.doc_id=z.id WHERE z.journal_id=j.id AND r.kind='CDP' AND r.identity_id=j.source_id)
 OR EXISTS(SELECT 1 FROM prepayments_depositapplication a WHERE a.journal_id=j.id AND a.identity_id=j.source_id)
 OR EXISTS(SELECT 1 FROM prepayments_depositrefund r WHERE r.journal_id=j.id AND r.identity_id=j.source_id)
 OR EXISTS(SELECT 1 FROM prepayments_depositunapplication u JOIN prepayments_depositapplication a ON a.id=u.application_id WHERE u.journal_id=j.id AND a.identity_id=j.source_id)
 OR EXISTS(SELECT 1 FROM prepayments_depositrefundvoid v JOIN prepayments_depositrefund r ON r.id=v.refund_id WHERE v.journal_id=j.id AND r.identity_id=j.source_id)));
 SELECT RAISE(ABORT,'prepayment_journal_split: Prepayment posting and account evidence must match exactly') WHERE EXISTS(SELECT 1 FROM prepayments_journal_mismatch WHERE id=NEW.doc_id);
END;
CREATE TRIGGER pp_late_reference BEFORE INSERT ON documents_documentaccount
BEGIN SELECT RAISE(ABORT,'deposit_late_reference: Cannot append to committed prepayment evidence') WHERE EXISTS(
 SELECT 1 FROM ledger_journal j JOIN prepayments_seal s ON s.kind='journal' AND s.doc_id=j.id WHERE j.source_id=NEW.source_id); END;
