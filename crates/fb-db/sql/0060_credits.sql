-- Credits: immutable evidence, exact Rust arithmetic, deferred seal obligations.
CREATE TABLE credits_creditmemo(id TEXT PRIMARY KEY,company_id TEXT NOT NULL REFERENCES ledger_company(id),identity_id TEXT NOT NULL UNIQUE REFERENCES documents_documentidentity(id),customer_id TEXT NOT NULL REFERENCES directory_customer(id),date TEXT NOT NULL CHECK(date(date) IS date AND date>='0001-01-01' AND date<='9999-12-31'),receivables_id TEXT NOT NULL REFERENCES ledger_account(id),collected_tax_id TEXT REFERENCES ledger_account(id),tax_item_id TEXT REFERENCES salestax_taxitem(id),total INTEGER NOT NULL CHECK(total BETWEEN 1 AND 9999999999999999),journal_id TEXT NOT NULL UNIQUE REFERENCES ledger_journal(id),actor_id TEXT NOT NULL REFERENCES fb_user(id),data TEXT NOT NULL CHECK(json_valid(data) AND json_type(data)='object')) STRICT, WITHOUT ROWID;
CREATE TABLE credits_creditmemoline(id TEXT PRIMARY KEY,credit_memo_id TEXT NOT NULL REFERENCES credits_creditmemo(id),position INTEGER NOT NULL CHECK(position BETWEEN 1 AND 50),item_id TEXT REFERENCES directory_item(id),income_account_id TEXT NOT NULL REFERENCES ledger_account(id),amount INTEGER NOT NULL CHECK(amount BETWEEN 0 AND 9999999999999999),taxable INTEGER NOT NULL CHECK(taxable IN(0,1)),data TEXT NOT NULL CHECK(json_valid(data) AND json_type(data)='object'),UNIQUE(credit_memo_id,position)) STRICT, WITHOUT ROWID;
CREATE TABLE credits_creditmemovoid(id TEXT PRIMARY KEY,credit_memo_id TEXT NOT NULL UNIQUE REFERENCES credits_creditmemo(id),journal_id TEXT NOT NULL UNIQUE REFERENCES ledger_journal(id),date TEXT NOT NULL CHECK(date(date) IS date AND date>='0001-01-01' AND date<='9999-12-31'),actor_id TEXT NOT NULL REFERENCES fb_user(id),data TEXT NOT NULL CHECK(json_valid(data) AND json_type(data)='object')) STRICT, WITHOUT ROWID;
CREATE TABLE credits_creditapplication(id TEXT PRIMARY KEY,company_id TEXT NOT NULL REFERENCES ledger_company(id),credit_memo_id TEXT NOT NULL REFERENCES credits_creditmemo(id),request_key TEXT NOT NULL,date TEXT NOT NULL CHECK(date(date) IS date AND date>='0001-01-01' AND date<='9999-12-31'),amount INTEGER NOT NULL CHECK(amount BETWEEN 1 AND 9999999999999999),actor_id TEXT NOT NULL REFERENCES fb_user(id),data TEXT NOT NULL CHECK(json_valid(data) AND json_type(data)='object'),UNIQUE(company_id,request_key)) STRICT, WITHOUT ROWID;
CREATE TABLE credits_creditapplicationline(id TEXT PRIMARY KEY,application_id TEXT NOT NULL REFERENCES credits_creditapplication(id),invoice_id TEXT NOT NULL REFERENCES sales_invoice(id),invoice_revision_id TEXT NOT NULL REFERENCES sales_invoicerevision(id),receivables_id TEXT NOT NULL REFERENCES ledger_account(id),amount INTEGER NOT NULL CHECK(amount BETWEEN 1 AND 9999999999999999),data TEXT NOT NULL CHECK(json_valid(data) AND json_type(data)='object'),UNIQUE(application_id,invoice_id)) STRICT, WITHOUT ROWID;
CREATE TABLE credits_creditunapplication(id TEXT PRIMARY KEY,application_id TEXT NOT NULL UNIQUE REFERENCES credits_creditapplication(id),date TEXT NOT NULL CHECK(date(date) IS date AND date>='0001-01-01' AND date<='9999-12-31'),actor_id TEXT NOT NULL REFERENCES fb_user(id),data TEXT NOT NULL CHECK(json_valid(data) AND json_type(data)='object')) STRICT, WITHOUT ROWID;
CREATE TABLE credits_creditrefund(id TEXT PRIMARY KEY,company_id TEXT NOT NULL REFERENCES ledger_company(id),identity_id TEXT NOT NULL UNIQUE REFERENCES documents_documentidentity(id),credit_memo_id TEXT NOT NULL REFERENCES credits_creditmemo(id),customer_id TEXT NOT NULL REFERENCES directory_customer(id),date TEXT NOT NULL CHECK(date(date) IS date AND date>='0001-01-01' AND date<='9999-12-31'),amount INTEGER NOT NULL CHECK(amount BETWEEN 1 AND 9999999999999999),bank_id TEXT NOT NULL REFERENCES ledger_account(id),receivables_id TEXT NOT NULL REFERENCES ledger_account(id),method TEXT NOT NULL CHECK(method IN('CHECK','CASH','OTHER')),check_number TEXT NOT NULL CHECK(length(check_number)<=20 AND check_number=trim(check_number) AND (method='CHECK' OR check_number='')),journal_id TEXT NOT NULL UNIQUE REFERENCES ledger_journal(id),actor_id TEXT NOT NULL REFERENCES fb_user(id),data TEXT NOT NULL CHECK(json_valid(data) AND json_type(data)='object')) STRICT, WITHOUT ROWID;
CREATE TABLE credits_creditrefundvoid(id TEXT PRIMARY KEY,refund_id TEXT NOT NULL UNIQUE REFERENCES credits_creditrefund(id),journal_id TEXT NOT NULL UNIQUE REFERENCES ledger_journal(id),date TEXT NOT NULL CHECK(date(date) IS date AND date>='0001-01-01' AND date<='9999-12-31'),actor_id TEXT NOT NULL REFERENCES fb_user(id),data TEXT NOT NULL CHECK(json_valid(data) AND json_type(data)='object')) STRICT, WITHOUT ROWID;
CREATE TABLE credits_seal(kind TEXT NOT NULL CHECK(kind IN('memo','memo_void','application','unapplication','refund','refund_void','journal')),doc_id TEXT NOT NULL,PRIMARY KEY(kind,doc_id)) STRICT, WITHOUT ROWID;
CREATE TABLE credits_seal_due(kind TEXT NOT NULL,doc_id TEXT NOT NULL,PRIMARY KEY(kind,doc_id),FOREIGN KEY(kind,doc_id) REFERENCES credits_seal(kind,doc_id) DEFERRABLE INITIALLY DEFERRED) STRICT, WITHOUT ROWID;
CREATE TRIGGER fb_credits_seal_insert BEFORE INSERT ON credits_seal
BEGIN SELECT RAISE(ABORT,'credit_immutable: Credit evidence cannot be replaced') WHERE EXISTS(SELECT 1 FROM credits_seal WHERE kind=NEW.kind AND doc_id=NEW.doc_id); END;
CREATE TRIGGER fb_credits_seal_due_insert BEFORE INSERT ON credits_seal_due
BEGIN SELECT RAISE(ABORT,'credit_immutable: Credit evidence cannot be replaced') WHERE EXISTS(SELECT 1 FROM credits_seal_due WHERE kind=NEW.kind AND doc_id=NEW.doc_id); END;
CREATE TRIGGER fb_credits_creditmemo_update BEFORE UPDATE ON credits_creditmemo BEGIN SELECT RAISE(ABORT,'credit_immutable: Credit history and primary identities are immutable'); END;
CREATE TRIGGER fb_credits_creditmemo_delete BEFORE DELETE ON credits_creditmemo BEGIN SELECT RAISE(ABORT,'credit_immutable: Credit history and primary identities are immutable'); END;
CREATE TRIGGER fb_credits_creditmemoline_update BEFORE UPDATE ON credits_creditmemoline BEGIN SELECT RAISE(ABORT,'credit_immutable: Credit history and primary identities are immutable'); END;
CREATE TRIGGER fb_credits_creditmemoline_delete BEFORE DELETE ON credits_creditmemoline BEGIN SELECT RAISE(ABORT,'credit_immutable: Credit history and primary identities are immutable'); END;
CREATE TRIGGER fb_credits_creditmemovoid_update BEFORE UPDATE ON credits_creditmemovoid BEGIN SELECT RAISE(ABORT,'credit_immutable: Credit history and primary identities are immutable'); END;
CREATE TRIGGER fb_credits_creditmemovoid_delete BEFORE DELETE ON credits_creditmemovoid BEGIN SELECT RAISE(ABORT,'credit_immutable: Credit history and primary identities are immutable'); END;
CREATE TRIGGER fb_credits_creditapplication_update BEFORE UPDATE ON credits_creditapplication BEGIN SELECT RAISE(ABORT,'credit_immutable: Credit history and primary identities are immutable'); END;
CREATE TRIGGER fb_credits_creditapplication_delete BEFORE DELETE ON credits_creditapplication BEGIN SELECT RAISE(ABORT,'credit_immutable: Credit history and primary identities are immutable'); END;
CREATE TRIGGER fb_credits_creditapplicationline_update BEFORE UPDATE ON credits_creditapplicationline BEGIN SELECT RAISE(ABORT,'credit_immutable: Credit history and primary identities are immutable'); END;
CREATE TRIGGER fb_credits_creditapplicationline_delete BEFORE DELETE ON credits_creditapplicationline BEGIN SELECT RAISE(ABORT,'credit_immutable: Credit history and primary identities are immutable'); END;
CREATE TRIGGER fb_credits_creditunapplication_update BEFORE UPDATE ON credits_creditunapplication BEGIN SELECT RAISE(ABORT,'credit_immutable: Credit history and primary identities are immutable'); END;
CREATE TRIGGER fb_credits_creditunapplication_delete BEFORE DELETE ON credits_creditunapplication BEGIN SELECT RAISE(ABORT,'credit_immutable: Credit history and primary identities are immutable'); END;
CREATE TRIGGER fb_credits_creditrefund_update BEFORE UPDATE ON credits_creditrefund BEGIN SELECT RAISE(ABORT,'credit_immutable: Credit history and primary identities are immutable'); END;
CREATE TRIGGER fb_credits_creditrefund_delete BEFORE DELETE ON credits_creditrefund BEGIN SELECT RAISE(ABORT,'credit_immutable: Credit history and primary identities are immutable'); END;
CREATE TRIGGER fb_credits_creditrefundvoid_update BEFORE UPDATE ON credits_creditrefundvoid BEGIN SELECT RAISE(ABORT,'credit_immutable: Credit history and primary identities are immutable'); END;
CREATE TRIGGER fb_credits_creditrefundvoid_delete BEFORE DELETE ON credits_creditrefundvoid BEGIN SELECT RAISE(ABORT,'credit_immutable: Credit history and primary identities are immutable'); END;
CREATE TRIGGER fb_credits_seal_update BEFORE UPDATE ON credits_seal BEGIN SELECT RAISE(ABORT,'credit_immutable: Credit history and primary identities are immutable'); END;
CREATE TRIGGER fb_credits_seal_delete BEFORE DELETE ON credits_seal BEGIN SELECT RAISE(ABORT,'credit_immutable: Credit history and primary identities are immutable'); END;
CREATE TRIGGER fb_credits_seal_due_update BEFORE UPDATE ON credits_seal_due BEGIN SELECT RAISE(ABORT,'credit_immutable: Credit history and primary identities are immutable'); END;
CREATE TRIGGER fb_credits_seal_due_delete BEFORE DELETE ON credits_seal_due BEGIN SELECT RAISE(ABORT,'credit_immutable: Credit history and primary identities are immutable'); END;
CREATE TRIGGER fb_credits_creditmemo_insert BEFORE INSERT ON credits_creditmemo
BEGIN
 SELECT RAISE(ABORT,'credit_immutable: Credit history cannot be replaced') WHERE EXISTS(SELECT 1 FROM credits_creditmemo WHERE id=NEW.id OR identity_id=NEW.identity_id OR journal_id=NEW.journal_id);
 SELECT RAISE(ABORT,'credit_fields: Credit columns and typed evidence must agree') WHERE json_extract(NEW.data,'$.id') IS NOT NEW.id OR json_extract(NEW.data,'$.company_id') IS NOT NEW.company_id OR json_extract(NEW.data,'$.identity_id') IS NOT NEW.identity_id OR json_extract(NEW.data,'$.customer_id') IS NOT NEW.customer_id OR json_extract(NEW.data,'$.date') IS NOT NEW.date OR json_extract(NEW.data,'$.receivables_id') IS NOT NEW.receivables_id OR json_extract(NEW.data,'$.collected_tax_id') IS NOT NEW.collected_tax_id OR json_extract(NEW.data,'$.tax_item_id') IS NOT NEW.tax_item_id OR fb_sales_money(json_quote(json_extract(NEW.data,'$.total'))) IS NOT NEW.total OR json_extract(NEW.data,'$.journal_id') IS NOT NEW.journal_id OR json_extract(NEW.data,'$.actor_id') IS NOT NEW.actor_id OR NOT fb_credits_fields('creditmemo',NEW.data);
END;
CREATE TRIGGER fb_credits_creditmemoline_insert BEFORE INSERT ON credits_creditmemoline
BEGIN
 SELECT RAISE(ABORT,'credit_immutable: Credit history cannot be replaced') WHERE EXISTS(SELECT 1 FROM credits_creditmemoline WHERE id=NEW.id OR (credit_memo_id=NEW.credit_memo_id AND position=NEW.position));
 SELECT RAISE(ABORT,'credit_fields: Credit columns and typed evidence must agree') WHERE json_extract(NEW.data,'$.id') IS NOT NEW.id OR json_extract(NEW.data,'$.credit_memo_id') IS NOT NEW.credit_memo_id OR json_extract(NEW.data,'$.position') IS NOT NEW.position OR json_extract(NEW.data,'$.item_id') IS NOT NEW.item_id OR json_extract(NEW.data,'$.income_account_id') IS NOT NEW.income_account_id OR fb_sales_money(json_quote(json_extract(NEW.data,'$.amount'))) IS NOT NEW.amount OR json_extract(NEW.data,'$.taxable') IS NOT NEW.taxable OR NOT fb_credits_fields('creditmemoline',NEW.data);
END;
CREATE TRIGGER fb_credits_creditmemovoid_insert BEFORE INSERT ON credits_creditmemovoid
BEGIN
 SELECT RAISE(ABORT,'credit_immutable: Credit history cannot be replaced') WHERE EXISTS(SELECT 1 FROM credits_creditmemovoid WHERE id=NEW.id OR credit_memo_id=NEW.credit_memo_id OR journal_id=NEW.journal_id);
 SELECT RAISE(ABORT,'credit_fields: Credit columns and typed evidence must agree') WHERE json_extract(NEW.data,'$.id') IS NOT NEW.id OR json_extract(NEW.data,'$.credit_memo_id') IS NOT NEW.credit_memo_id OR json_extract(NEW.data,'$.journal_id') IS NOT NEW.journal_id OR json_extract(NEW.data,'$.date') IS NOT NEW.date OR json_extract(NEW.data,'$.actor_id') IS NOT NEW.actor_id OR NOT fb_credits_fields('creditmemovoid',NEW.data);
END;
CREATE TRIGGER fb_credits_creditapplication_insert BEFORE INSERT ON credits_creditapplication
BEGIN
 SELECT RAISE(ABORT,'credit_immutable: Credit history cannot be replaced') WHERE EXISTS(SELECT 1 FROM credits_creditapplication WHERE id=NEW.id OR (company_id=NEW.company_id AND request_key=NEW.request_key));
 SELECT RAISE(ABORT,'credit_fields: Credit columns and typed evidence must agree') WHERE json_extract(NEW.data,'$.id') IS NOT NEW.id OR json_extract(NEW.data,'$.company_id') IS NOT NEW.company_id OR json_extract(NEW.data,'$.credit_memo_id') IS NOT NEW.credit_memo_id OR json_extract(NEW.data,'$.request_key') IS NOT NEW.request_key OR json_extract(NEW.data,'$.date') IS NOT NEW.date OR fb_sales_money(json_quote(json_extract(NEW.data,'$.amount'))) IS NOT NEW.amount OR json_extract(NEW.data,'$.actor_id') IS NOT NEW.actor_id OR NOT fb_credits_fields('creditapplication',NEW.data);
END;
CREATE TRIGGER fb_credits_creditapplicationline_insert BEFORE INSERT ON credits_creditapplicationline
BEGIN
 SELECT RAISE(ABORT,'credit_immutable: Credit history cannot be replaced') WHERE EXISTS(SELECT 1 FROM credits_creditapplicationline WHERE id=NEW.id OR (application_id=NEW.application_id AND invoice_id=NEW.invoice_id));
 SELECT RAISE(ABORT,'credit_fields: Credit columns and typed evidence must agree') WHERE json_extract(NEW.data,'$.id') IS NOT NEW.id OR json_extract(NEW.data,'$.application_id') IS NOT NEW.application_id OR json_extract(NEW.data,'$.invoice_id') IS NOT NEW.invoice_id OR json_extract(NEW.data,'$.invoice_revision_id') IS NOT NEW.invoice_revision_id OR json_extract(NEW.data,'$.receivables_id') IS NOT NEW.receivables_id OR fb_sales_money(json_quote(json_extract(NEW.data,'$.amount'))) IS NOT NEW.amount OR NOT fb_credits_fields('creditapplicationline',NEW.data);
END;
CREATE TRIGGER fb_credits_creditunapplication_insert BEFORE INSERT ON credits_creditunapplication
BEGIN
 SELECT RAISE(ABORT,'credit_immutable: Credit history cannot be replaced') WHERE EXISTS(SELECT 1 FROM credits_creditunapplication WHERE id=NEW.id OR application_id=NEW.application_id);
 SELECT RAISE(ABORT,'credit_fields: Credit columns and typed evidence must agree') WHERE json_extract(NEW.data,'$.id') IS NOT NEW.id OR json_extract(NEW.data,'$.application_id') IS NOT NEW.application_id OR json_extract(NEW.data,'$.date') IS NOT NEW.date OR json_extract(NEW.data,'$.actor_id') IS NOT NEW.actor_id OR NOT fb_credits_fields('creditunapplication',NEW.data);
END;
CREATE TRIGGER fb_credits_creditrefund_insert BEFORE INSERT ON credits_creditrefund
BEGIN
 SELECT RAISE(ABORT,'credit_immutable: Credit history cannot be replaced') WHERE EXISTS(SELECT 1 FROM credits_creditrefund WHERE id=NEW.id OR identity_id=NEW.identity_id OR journal_id=NEW.journal_id);
 SELECT RAISE(ABORT,'credit_fields: Credit columns and typed evidence must agree') WHERE json_extract(NEW.data,'$.id') IS NOT NEW.id OR json_extract(NEW.data,'$.company_id') IS NOT NEW.company_id OR json_extract(NEW.data,'$.identity_id') IS NOT NEW.identity_id OR json_extract(NEW.data,'$.credit_memo_id') IS NOT NEW.credit_memo_id OR json_extract(NEW.data,'$.customer_id') IS NOT NEW.customer_id OR json_extract(NEW.data,'$.date') IS NOT NEW.date OR fb_sales_money(json_quote(json_extract(NEW.data,'$.amount'))) IS NOT NEW.amount OR json_extract(NEW.data,'$.bank_id') IS NOT NEW.bank_id OR json_extract(NEW.data,'$.receivables_id') IS NOT NEW.receivables_id OR json_extract(NEW.data,'$.method') IS NOT NEW.method OR json_extract(NEW.data,'$.check_number') IS NOT NEW.check_number OR json_extract(NEW.data,'$.journal_id') IS NOT NEW.journal_id OR json_extract(NEW.data,'$.actor_id') IS NOT NEW.actor_id OR NOT fb_credits_fields('creditrefund',NEW.data);
END;
CREATE TRIGGER fb_credits_creditrefundvoid_insert BEFORE INSERT ON credits_creditrefundvoid
BEGIN
 SELECT RAISE(ABORT,'credit_immutable: Credit history cannot be replaced') WHERE EXISTS(SELECT 1 FROM credits_creditrefundvoid WHERE id=NEW.id OR refund_id=NEW.refund_id OR journal_id=NEW.journal_id);
 SELECT RAISE(ABORT,'credit_fields: Credit columns and typed evidence must agree') WHERE json_extract(NEW.data,'$.id') IS NOT NEW.id OR json_extract(NEW.data,'$.refund_id') IS NOT NEW.refund_id OR json_extract(NEW.data,'$.journal_id') IS NOT NEW.journal_id OR json_extract(NEW.data,'$.date') IS NOT NEW.date OR json_extract(NEW.data,'$.actor_id') IS NOT NEW.actor_id OR NOT fb_credits_fields('creditrefundvoid',NEW.data);
END;
CREATE TRIGGER fb_credits_creditmemo_due AFTER INSERT ON credits_creditmemo BEGIN INSERT INTO credits_seal_due VALUES('memo',NEW.id); END;
CREATE TRIGGER fb_credits_creditmemovoid_due AFTER INSERT ON credits_creditmemovoid BEGIN INSERT INTO credits_seal_due VALUES('memo_void',NEW.id); END;
CREATE TRIGGER fb_credits_creditapplication_due AFTER INSERT ON credits_creditapplication BEGIN INSERT INTO credits_seal_due VALUES('application',NEW.id); END;
CREATE TRIGGER fb_credits_creditunapplication_due AFTER INSERT ON credits_creditunapplication BEGIN INSERT INTO credits_seal_due VALUES('unapplication',NEW.id); END;
CREATE TRIGGER fb_credits_creditrefund_due AFTER INSERT ON credits_creditrefund BEGIN INSERT INTO credits_seal_due VALUES('refund',NEW.id); END;
CREATE TRIGGER fb_credits_creditrefundvoid_due AFTER INSERT ON credits_creditrefundvoid BEGIN INSERT INTO credits_seal_due VALUES('refund_void',NEW.id); END;
CREATE VIEW credits_live_use AS
 SELECT id,credit_memo_id,amount FROM credits_creditapplication a WHERE NOT EXISTS(SELECT 1 FROM credits_creditunapplication u WHERE u.application_id=a.id)
 UNION ALL SELECT id,credit_memo_id,amount FROM credits_creditrefund r WHERE NOT EXISTS(SELECT 1 FROM credits_creditrefundvoid v WHERE v.refund_id=r.id);
CREATE VIEW credits_invoice_settlements AS
 SELECT invoice_id,amount FROM sales_receiptallocation a WHERE NOT EXISTS(SELECT 1 FROM sales_receiptreversal u WHERE u.receipt_id=a.receipt_id)
 UNION ALL SELECT invoice_id,amount FROM credits_creditapplicationline l WHERE NOT EXISTS(SELECT 1 FROM credits_creditunapplication u WHERE u.application_id=l.application_id);
CREATE VIEW credits_overused AS
 SELECT m.id FROM credits_creditmemo m WHERE fb_credits_sum((SELECT json_group_array(amount) FROM credits_live_use WHERE credit_memo_id=m.id)) IS NULL OR fb_credits_sum((SELECT json_group_array(amount) FROM credits_live_use WHERE credit_memo_id=m.id))>m.total OR (EXISTS(SELECT 1 FROM credits_creditmemovoid v WHERE v.credit_memo_id=m.id) AND EXISTS(SELECT 1 FROM credits_live_use WHERE credit_memo_id=m.id));
CREATE VIEW credits_overcredited AS
 SELECT i.id FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.number=i.revision WHERE fb_credits_sum((SELECT json_group_array(amount) FROM credits_invoice_settlements WHERE invoice_id=i.id)) IS NULL OR fb_credits_sum((SELECT json_group_array(amount) FROM credits_invoice_settlements WHERE invoice_id=i.id))>r.total;
CREATE VIEW credits_releases AS
 SELECT a.credit_memo_id,u.date FROM credits_creditunapplication u JOIN credits_creditapplication a ON a.id=u.application_id
 UNION ALL SELECT r.credit_memo_id,v.date FROM credits_creditrefundvoid v JOIN credits_creditrefund r ON r.id=v.refund_id;
CREATE VIEW credits_expected_entries AS
 SELECT m.identity_id source_id,m.journal_id,l.income_account_id account_id,fb_credits_sum(json_group_array(l.amount)) debit,0 credit FROM credits_creditmemo m JOIN credits_creditmemoline l ON l.credit_memo_id=m.id GROUP BY m.id,l.income_account_id HAVING fb_credits_sum(json_group_array(l.amount))>0
 UNION ALL SELECT identity_id,journal_id,collected_tax_id,fb_sales_money(json_quote(json_extract(data,'$.tax'))),0 FROM credits_creditmemo WHERE collected_tax_id IS NOT NULL
 UNION ALL SELECT identity_id,journal_id,receivables_id,0,total FROM credits_creditmemo
 UNION ALL SELECT identity_id,journal_id,receivables_id,amount,0 FROM credits_creditrefund
 UNION ALL SELECT identity_id,journal_id,bank_id,0,amount FROM credits_creditrefund;
CREATE VIEW credits_expected_accounts AS
 SELECT identity_id source_id,'receivables' role,receivables_id account_id FROM credits_creditmemo
 UNION ALL SELECT identity_id,'collected_tax',collected_tax_id FROM credits_creditmemo WHERE collected_tax_id IS NOT NULL
 UNION ALL SELECT DISTINCT m.identity_id,'income:'||l.income_account_id,l.income_account_id FROM credits_creditmemo m JOIN credits_creditmemoline l ON l.credit_memo_id=m.id
 UNION ALL SELECT identity_id,'receivables',receivables_id FROM credits_creditrefund
 UNION ALL SELECT identity_id,'bank',bank_id FROM credits_creditrefund;
CREATE VIEW credits_journal_mismatch AS
 SELECT j.id FROM ledger_journal j WHERE j.id IN(SELECT journal_id FROM credits_expected_entries) AND
 ((SELECT count(*) FROM credits_expected_entries e WHERE e.journal_id=j.id)<>(SELECT count(*) FROM ledger_journalline l WHERE l.journal_id=j.id)
 OR EXISTS(SELECT account_id,debit,credit FROM credits_expected_entries e WHERE e.journal_id=j.id EXCEPT SELECT account_id,debit,credit FROM ledger_journalline l WHERE l.journal_id=j.id)
 OR EXISTS(SELECT account_id,debit,credit FROM ledger_journalline l WHERE l.journal_id=j.id EXCEPT SELECT account_id,debit,credit FROM credits_expected_entries e WHERE e.journal_id=j.id));
CREATE VIEW credits_account_mismatch AS
 SELECT d.id FROM documents_documentidentity d WHERE d.id IN(SELECT source_id FROM credits_expected_accounts) AND
 ((SELECT count(*) FROM credits_expected_accounts e WHERE e.source_id=d.id)<>(SELECT count(*) FROM documents_documentaccount a WHERE a.source_id=d.id)
 OR EXISTS(SELECT role,account_id FROM credits_expected_accounts e WHERE e.source_id=d.id EXCEPT SELECT role,account_id FROM documents_documentaccount a WHERE a.source_id=d.id)
 OR EXISTS(SELECT role,account_id FROM documents_documentaccount a WHERE a.source_id=d.id EXCEPT SELECT role,account_id FROM credits_expected_accounts e WHERE e.source_id=d.id));
CREATE TRIGGER fb_credits_memo_scope BEFORE INSERT ON credits_creditmemo WHEN NOT EXISTS(SELECT 1 FROM credits_creditmemo WHERE id=NEW.id OR identity_id=NEW.identity_id OR journal_id=NEW.journal_id)
BEGIN
 SELECT RAISE(ABORT,'credit_memo_scope: Invalid credit memo ownership or posting') WHERE NOT EXISTS(SELECT 1 FROM documents_documentidentity d JOIN directory_customer c ON c.id=NEW.customer_id JOIN ledger_account a ON a.id=NEW.receivables_id JOIN ledger_journal j ON j.id=NEW.journal_id WHERE d.id=NEW.identity_id AND d.company_id=NEW.company_id AND d.kind='CM' AND c.company_id=NEW.company_id AND c.active=1 AND a.company_id=NEW.company_id AND a.kind='ASSET' AND a.active=1 AND a.postable=1 AND j.company_id=NEW.company_id AND j.source_id=d.id AND j.status='POSTED' AND j.reversal_of_id IS NULL AND j.date=NEW.date);
 SELECT RAISE(ABORT,'credit_memo_tax: Invalid credit memo tax account or picked rate') WHERE (fb_sales_money(json_quote(json_extract(NEW.data,'$.tax')))>0) IS NOT (NEW.collected_tax_id IS NOT NULL) OR (NEW.collected_tax_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.collected_tax_id AND company_id=NEW.company_id AND kind='LIABILITY' AND active=1 AND postable=1)) OR (NEW.tax_item_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM salestax_taxitem t WHERE t.id=NEW.tax_item_id AND t.company_id=NEW.company_id AND t.active=1 AND (t.kind<>'GROUP' OR NOT EXISTS(SELECT 1 FROM salestax_taxgroupmember g LEFT JOIN salestax_taxitem r ON r.id=g.member_id WHERE g.group_id=t.id AND (r.id IS NULL OR r.active<>1)))));
END;
CREATE TRIGGER fb_credits_line_scope BEFORE INSERT ON credits_creditmemoline WHEN NOT EXISTS(SELECT 1 FROM credits_creditmemoline WHERE id=NEW.id OR (credit_memo_id=NEW.credit_memo_id AND position=NEW.position))
BEGIN
 SELECT RAISE(ABORT,'credit_memo_late_line: Cannot append credit memo lines after sealing') WHERE NOT EXISTS(SELECT 1 FROM credits_creditmemo m WHERE m.id=NEW.credit_memo_id AND NOT EXISTS(SELECT 1 FROM credits_seal s WHERE s.kind='memo' AND s.doc_id=m.id));
 SELECT RAISE(ABORT,'credit_memo_line_scope: Invalid credit memo line account or item') WHERE NOT EXISTS(SELECT 1 FROM credits_creditmemo m JOIN ledger_account a ON a.id=NEW.income_account_id WHERE m.id=NEW.credit_memo_id AND a.company_id=m.company_id AND a.kind='INCOME' AND a.active=1 AND a.postable=1 AND (NEW.item_id IS NULL OR EXISTS(SELECT 1 FROM directory_item WHERE id=NEW.item_id AND company_id=m.company_id AND active=1))) OR NOT fb_sales_line_ok(NEW.data);
END;
CREATE TRIGGER fb_credits_application_scope BEFORE INSERT ON credits_creditapplication WHEN NOT EXISTS(SELECT 1 FROM credits_creditapplication WHERE id=NEW.id OR (company_id=NEW.company_id AND request_key=NEW.request_key))
BEGIN SELECT RAISE(ABORT,'credit_application_scope: Invalid credit application') WHERE NOT EXISTS(SELECT 1 FROM credits_creditmemo m WHERE m.id=NEW.credit_memo_id AND m.company_id=NEW.company_id AND NEW.date>=m.date AND NOT EXISTS(SELECT 1 FROM credits_creditmemovoid WHERE credit_memo_id=m.id)); END;
CREATE TRIGGER fb_credits_application_target BEFORE INSERT ON credits_creditapplicationline WHEN NOT EXISTS(SELECT 1 FROM credits_creditapplicationline WHERE id=NEW.id OR (application_id=NEW.application_id AND invoice_id=NEW.invoice_id))
BEGIN
 SELECT RAISE(ABORT,'credit_application_late_line: Cannot append sealed credit allocations') WHERE NOT EXISTS(SELECT 1 FROM credits_creditapplication a WHERE a.id=NEW.application_id AND NOT EXISTS(SELECT 1 FROM credits_seal s WHERE s.kind='application' AND s.doc_id=a.id));
 SELECT RAISE(ABORT,'credit_application_target: Invalid current open invoice target') WHERE NOT EXISTS(SELECT 1 FROM credits_creditapplication a JOIN credits_creditmemo m ON m.id=a.credit_memo_id JOIN sales_invoice i ON i.id=NEW.invoice_id JOIN sales_invoicerevision r ON r.id=NEW.invoice_revision_id WHERE a.id=NEW.application_id AND i.company_id=a.company_id AND r.invoice_id=i.id AND r.number=i.revision AND r.state='OPEN' AND r.customer_id=m.customer_id AND r.date<=a.date);
 SELECT RAISE(ABORT,'credit_application_cross_ar: Credit and invoice original receivables must agree') WHERE NOT EXISTS(SELECT 1 FROM credits_creditapplication a JOIN credits_creditmemo m ON m.id=a.credit_memo_id JOIN sales_invoice i ON i.id=NEW.invoice_id JOIN documents_documentaccount d ON d.source_id=i.identity_id AND d.role='receivables' WHERE a.id=NEW.application_id AND m.receivables_id=NEW.receivables_id AND d.account_id=NEW.receivables_id);
END;
CREATE TRIGGER fb_credits_application_bound AFTER INSERT ON credits_creditapplicationline
BEGIN SELECT RAISE(ABORT,'invoice_overcredited: Invoice settlements cannot exceed the invoice total') WHERE EXISTS(SELECT 1 FROM credits_overcredited WHERE id=NEW.invoice_id); END;
CREATE TRIGGER fb_credits_receipt_bound AFTER INSERT ON sales_receiptallocation
BEGIN SELECT RAISE(ABORT,'invoice_overcredited: Invoice settlements cannot exceed the invoice total') WHERE EXISTS(SELECT 1 FROM credits_overcredited WHERE id=NEW.invoice_id); END;
CREATE TRIGGER fb_credits_use_application AFTER INSERT ON credits_creditapplication
BEGIN SELECT RAISE(ABORT,'credit_memo_overused: Available credit cannot go negative') WHERE EXISTS(SELECT 1 FROM credits_overused WHERE id=NEW.credit_memo_id); END;
CREATE TRIGGER fb_credits_use_refund AFTER INSERT ON credits_creditrefund
BEGIN SELECT RAISE(ABORT,'credit_memo_overused: Available credit cannot go negative') WHERE EXISTS(SELECT 1 FROM credits_overused WHERE id=NEW.credit_memo_id); END;
CREATE TRIGGER fb_credits_unapplication_scope BEFORE INSERT ON credits_creditunapplication WHEN NOT EXISTS(SELECT 1 FROM credits_creditunapplication WHERE id=NEW.id OR application_id=NEW.application_id)
BEGIN SELECT RAISE(ABORT,'credit_unapplication_fields: Invalid unapply parent or date') WHERE NOT EXISTS(SELECT 1 FROM credits_creditapplication a WHERE a.id=NEW.application_id AND NEW.date>=a.date); END;
CREATE TRIGGER fb_credits_refund_scope BEFORE INSERT ON credits_creditrefund WHEN NOT EXISTS(SELECT 1 FROM credits_creditrefund WHERE id=NEW.id OR identity_id=NEW.identity_id OR journal_id=NEW.journal_id)
BEGIN
 SELECT RAISE(ABORT,'credit_refund_scope: Invalid credit refund ownership or posting') WHERE NOT EXISTS(SELECT 1 FROM credits_creditmemo m JOIN documents_documentidentity d ON d.id=NEW.identity_id JOIN directory_customer c ON c.id=NEW.customer_id JOIN ledger_journal j ON j.id=NEW.journal_id WHERE m.id=NEW.credit_memo_id AND m.company_id=NEW.company_id AND NOT EXISTS(SELECT 1 FROM credits_creditmemovoid WHERE credit_memo_id=m.id) AND NEW.customer_id=m.customer_id AND c.company_id=m.company_id AND NEW.receivables_id=m.receivables_id AND NEW.date>=m.date AND d.kind='RFD' AND d.company_id=NEW.company_id AND j.company_id=NEW.company_id AND j.source_id=d.id AND j.status='POSTED' AND j.reversal_of_id IS NULL AND j.date=NEW.date);
 SELECT RAISE(ABORT,'credit_refund_bank: Refund requires a designated bank and active original receivables') WHERE NEW.bank_id=NEW.receivables_id OR NOT EXISTS(SELECT 1 FROM ledger_account a JOIN documents_designatedbank b ON b.account_id=a.id JOIN documents_accountingsetup s ON s.id=b.setup_id WHERE a.id=NEW.bank_id AND a.company_id=NEW.company_id AND s.company_id=NEW.company_id AND a.kind='ASSET' AND a.active=1 AND a.postable=1) OR NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.receivables_id AND company_id=NEW.company_id AND kind='ASSET' AND active=1 AND postable=1) OR EXISTS(SELECT 1 FROM fb_control_accounts WHERE company_id=NEW.company_id AND account_id=NEW.bank_id);
 SELECT RAISE(ABORT,'credit_refund_number_in_use: Check number already used on this bank') WHERE NEW.check_number<>'' AND (EXISTS(SELECT 1 FROM credits_creditrefund r WHERE r.company_id=NEW.company_id AND r.bank_id=NEW.bank_id AND r.check_number=NEW.check_number AND NOT EXISTS(SELECT 1 FROM credits_creditrefundvoid v WHERE v.refund_id=r.id)) OR (NEW.check_number NOT GLOB '*[^0-9]*' AND EXISTS(SELECT 1 FROM payables_billpayment p WHERE p.company_id=NEW.company_id AND p.bank_id=NEW.bank_id AND p.method='CHECK' AND p.reference=NEW.check_number AND NOT EXISTS(SELECT 1 FROM payables_paymentreversal z WHERE z.payment_id=p.id))));
END;
CREATE TRIGGER fb_credits_payment_number BEFORE INSERT ON payables_billpayment
BEGIN SELECT RAISE(ABORT,'bill_payment_check_number: Check number already used by a live refund') WHERE NEW.method='CHECK' AND length(NEW.reference) BETWEEN 1 AND 20 AND NEW.reference NOT GLOB '*[^0-9]*' AND EXISTS(SELECT 1 FROM credits_creditrefund r WHERE r.company_id=NEW.company_id AND r.bank_id=NEW.bank_id AND r.check_number=NEW.reference AND NOT EXISTS(SELECT 1 FROM credits_creditrefundvoid v WHERE v.refund_id=r.id)); END;
CREATE TRIGGER fb_credits_memo_void_scope BEFORE INSERT ON credits_creditmemovoid WHEN NOT EXISTS(SELECT 1 FROM credits_creditmemovoid WHERE id=NEW.id OR credit_memo_id=NEW.credit_memo_id OR journal_id=NEW.journal_id)
BEGIN
 SELECT RAISE(ABORT,'credit_memo_void_link: Invalid credit memo reversal or date') WHERE NOT EXISTS(SELECT 1 FROM credits_creditmemo m JOIN ledger_journal j ON j.id=NEW.journal_id WHERE m.id=NEW.credit_memo_id AND j.source_id=m.identity_id AND j.company_id=m.company_id AND j.status='POSTED' AND j.reversal_of_id=m.journal_id AND j.date=NEW.date AND NEW.date>=m.date);
 SELECT RAISE(ABORT,'credit_memo_live_dependency: Unapply and void refunds before voiding a credit memo') WHERE EXISTS(SELECT 1 FROM credits_live_use WHERE credit_memo_id=NEW.credit_memo_id);
 SELECT RAISE(ABORT,'credit_memo_release_date: Credit memo void cannot precede its last release') WHERE EXISTS(SELECT 1 FROM credits_releases WHERE credit_memo_id=NEW.credit_memo_id AND date>NEW.date);
END;
CREATE TRIGGER fb_credits_refund_void_scope BEFORE INSERT ON credits_creditrefundvoid WHEN NOT EXISTS(SELECT 1 FROM credits_creditrefundvoid WHERE id=NEW.id OR refund_id=NEW.refund_id OR journal_id=NEW.journal_id)
BEGIN SELECT RAISE(ABORT,'credit_refund_void_link: Invalid refund reversal or date') WHERE NOT EXISTS(SELECT 1 FROM credits_creditrefund r JOIN ledger_journal j ON j.id=NEW.journal_id WHERE r.id=NEW.refund_id AND j.source_id=r.identity_id AND j.company_id=r.company_id AND j.status='POSTED' AND j.reversal_of_id=r.journal_id AND j.date=NEW.date AND NEW.date>=r.date); END;
CREATE TRIGGER fb_credits_invoice_void BEFORE INSERT ON sales_invoicerevision WHEN NEW.state='VOID'
BEGIN
 SELECT RAISE(ABORT,'invoice_live_credits: Unapply live credits before voiding an invoice') WHERE EXISTS(SELECT 1 FROM credits_creditapplicationline l WHERE l.invoice_id=NEW.invoice_id AND NOT EXISTS(SELECT 1 FROM credits_creditunapplication u WHERE u.application_id=l.application_id));
 SELECT RAISE(ABORT,'invoice_credit_release_date: Invoice void cannot precede latest credit unapply') WHERE EXISTS(SELECT 1 FROM credits_creditapplicationline l JOIN credits_creditunapplication u ON u.application_id=l.application_id JOIN ledger_journal j ON j.id=NEW.reversal_id WHERE l.invoice_id=NEW.invoice_id AND u.date>j.date);
END;
CREATE TRIGGER fb_credits_journal_due AFTER INSERT ON ledger_journal WHEN (SELECT kind FROM documents_documentidentity WHERE id=NEW.source_id) IN('CM','RFD')
BEGIN INSERT INTO credits_seal_due VALUES('journal',NEW.id); END;
CREATE TRIGGER fb_credits_late_reference BEFORE INSERT ON documents_documentaccount
BEGIN SELECT RAISE(ABORT,'credit_late_reference: Cannot append account evidence after sealing') WHERE EXISTS(SELECT 1 FROM credits_creditmemo m JOIN credits_seal s ON s.kind='memo' AND s.doc_id=m.id WHERE m.identity_id=NEW.source_id) OR EXISTS(SELECT 1 FROM credits_creditrefund r JOIN credits_seal s ON s.kind='refund' AND s.doc_id=r.id WHERE r.identity_id=NEW.source_id); END;
CREATE TRIGGER fb_credits_seal_memo BEFORE INSERT ON credits_seal WHEN NEW.kind='memo'
BEGIN SELECT RAISE(ABORT,'credit_complete: Credit seal requires a real record') WHERE NOT EXISTS(SELECT 1 FROM credits_creditmemo WHERE id=NEW.doc_id);
 SELECT RAISE(ABORT,'credit_memo_complete: Lines must match memo totals') WHERE EXISTS(SELECT 1 FROM credits_creditmemo m WHERE m.id=NEW.doc_id AND NOT fb_sales_document_ok(m.data,(SELECT json_group_array(json(data)) FROM (SELECT data FROM credits_creditmemoline WHERE credit_memo_id=m.id ORDER BY position)),m.total));
 SELECT RAISE(ABORT,'credit_memo_journal_split: Credit journal must exactly debit income and tax and credit receivables') WHERE EXISTS(SELECT 1 FROM credits_creditmemo m JOIN credits_journal_mismatch x ON x.id=m.journal_id WHERE m.id=NEW.doc_id);
 SELECT RAISE(ABORT,'credit_memo_account_evidence: Credit account snapshots must match exactly') WHERE EXISTS(SELECT 1 FROM credits_creditmemo m JOIN credits_account_mismatch x ON x.id=m.identity_id WHERE m.id=NEW.doc_id);
 END;
CREATE TRIGGER fb_credits_seal_memo_void BEFORE INSERT ON credits_seal WHEN NEW.kind='memo_void'
BEGIN SELECT RAISE(ABORT,'credit_complete: Credit seal requires a real record') WHERE NOT EXISTS(SELECT 1 FROM credits_creditmemovoid WHERE id=NEW.doc_id); END;
CREATE TRIGGER fb_credits_seal_application BEFORE INSERT ON credits_seal WHEN NEW.kind='application'
BEGIN SELECT RAISE(ABORT,'credit_complete: Credit seal requires a real record') WHERE NOT EXISTS(SELECT 1 FROM credits_creditapplication WHERE id=NEW.doc_id);
 SELECT RAISE(ABORT,'credit_application_complete: Lines must equal the application amount') WHERE EXISTS(SELECT 1 FROM credits_creditapplication a WHERE a.id=NEW.doc_id AND ((SELECT count(*) FROM credits_creditapplicationline WHERE application_id=a.id) NOT BETWEEN 1 AND 50 OR fb_credits_sum((SELECT json_group_array(amount) FROM credits_creditapplicationline WHERE application_id=a.id)) IS NOT a.amount));
 SELECT RAISE(ABORT,'credit_application_open_at_commit: Live credit requires its current open invoice') WHERE EXISTS(SELECT 1 FROM credits_creditapplicationline l JOIN sales_invoice i ON i.id=l.invoice_id LEFT JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.number=i.revision WHERE l.application_id=NEW.doc_id AND NOT EXISTS(SELECT 1 FROM credits_creditunapplication WHERE application_id=l.application_id) AND (r.id IS NOT l.invoice_revision_id OR r.state IS NOT 'OPEN'));
 END;
CREATE TRIGGER fb_credits_seal_unapplication BEFORE INSERT ON credits_seal WHEN NEW.kind='unapplication'
BEGIN SELECT RAISE(ABORT,'credit_complete: Credit seal requires a real record') WHERE NOT EXISTS(SELECT 1 FROM credits_creditunapplication WHERE id=NEW.doc_id); END;
CREATE TRIGGER fb_credits_seal_refund BEFORE INSERT ON credits_seal WHEN NEW.kind='refund'
BEGIN SELECT RAISE(ABORT,'credit_complete: Credit seal requires a real record') WHERE NOT EXISTS(SELECT 1 FROM credits_creditrefund WHERE id=NEW.doc_id);
 SELECT RAISE(ABORT,'credit_refund_journal_split: Refund must exactly debit receivables and credit bank') WHERE EXISTS(SELECT 1 FROM credits_creditrefund r JOIN credits_journal_mismatch x ON x.id=r.journal_id WHERE r.id=NEW.doc_id);
 SELECT RAISE(ABORT,'credit_refund_account_evidence: Refund account snapshots must match exactly') WHERE EXISTS(SELECT 1 FROM credits_creditrefund r JOIN credits_account_mismatch x ON x.id=r.identity_id WHERE r.id=NEW.doc_id);
 END;
CREATE TRIGGER fb_credits_seal_refund_void BEFORE INSERT ON credits_seal WHEN NEW.kind='refund_void'
BEGIN SELECT RAISE(ABORT,'credit_complete: Credit seal requires a real record') WHERE NOT EXISTS(SELECT 1 FROM credits_creditrefundvoid WHERE id=NEW.doc_id); END;
CREATE TRIGGER fb_credits_seal_journal BEFORE INSERT ON credits_seal WHEN NEW.kind='journal'
BEGIN SELECT RAISE(ABORT,'credit_journal_owner: Credit journals require their complete owning lifecycle') WHERE NOT EXISTS(SELECT 1 FROM ledger_journal j WHERE j.id=NEW.doc_id AND j.status='POSTED' AND
 ((j.reversal_of_id IS NULL AND (EXISTS(SELECT 1 FROM credits_creditmemo m JOIN credits_seal s ON s.kind='memo' AND s.doc_id=m.id WHERE m.identity_id=j.source_id AND m.journal_id=j.id) OR EXISTS(SELECT 1 FROM credits_creditrefund r JOIN credits_seal s ON s.kind='refund' AND s.doc_id=r.id WHERE r.identity_id=j.source_id AND r.journal_id=j.id)))
 OR (j.reversal_of_id IS NOT NULL AND (EXISTS(SELECT 1 FROM credits_creditmemovoid v JOIN credits_creditmemo m ON m.id=v.credit_memo_id JOIN credits_seal s ON s.kind='memo_void' AND s.doc_id=v.id WHERE m.identity_id=j.source_id AND m.journal_id=j.reversal_of_id AND v.journal_id=j.id) OR EXISTS(SELECT 1 FROM credits_creditrefundvoid v JOIN credits_creditrefund r ON r.id=v.refund_id JOIN credits_seal s ON s.kind='refund_void' AND s.doc_id=v.id WHERE r.identity_id=j.source_id AND r.journal_id=j.reversal_of_id AND v.journal_id=j.id))))); END;
CREATE VIEW credits_tax_scope_violations AS
 SELECT m.id FROM credits_creditmemo m WHERE json_extract(m.data,'$.tax_mode') IN('RATE','GROUP') AND (
 NOT EXISTS(SELECT 1 FROM salestax_taxitem t WHERE t.id=m.tax_item_id AND t.company_id=m.company_id AND t.kind=json_extract(m.data,'$.tax_mode')) OR
 EXISTS(SELECT 1 FROM json_each(m.data,'$.tax_snapshot.members') x WHERE NOT EXISTS(SELECT 1 FROM salestax_taxitem t WHERE t.id=json_extract(x.value,'$.id') AND t.company_id=m.company_id AND t.kind='RATE')) OR
 EXISTS(SELECT 1 FROM json_each(m.data,'$.tax_snapshot.members') x WHERE json_extract(x.value,'$.agency_id') IS NOT NULL AND NOT EXISTS(SELECT 1 FROM directory_vendor v WHERE v.id=json_extract(x.value,'$.agency_id') AND v.company_id=m.company_id)));
CREATE TRIGGER fb_credits_tax_scope AFTER INSERT ON credits_creditmemo
BEGIN SELECT RAISE(ABORT,'credit_memo_tax: Invalid credit memo tax account or picked rate') WHERE EXISTS(SELECT 1 FROM credits_tax_scope_violations WHERE id=NEW.id) OR (json_extract(NEW.data,'$.tax_mode')='GROUP' AND (
 (SELECT count(*) FROM json_each(NEW.data,'$.tax_snapshot.members'))<>(SELECT count(*) FROM salestax_taxgroupmember WHERE group_id=NEW.tax_item_id) OR
 EXISTS(SELECT 1 FROM json_each(NEW.data,'$.tax_snapshot.members') x WHERE NOT EXISTS(SELECT 1 FROM salestax_taxgroupmember g WHERE g.group_id=NEW.tax_item_id AND g.member_id=json_extract(x.value,'$.id'))))); END;
