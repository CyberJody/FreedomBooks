-- L3 payables. Live PostgreSQL guard bodies captured in runs/2026-10-30/l3-payables.
-- Monetary arithmetic is performed by Rust. SQL sums only verify stored integer evidence.
CREATE TABLE payables_bill (
 id TEXT PRIMARY KEY, company_id TEXT NOT NULL REFERENCES ledger_company(id),
 identity_id TEXT NOT NULL UNIQUE REFERENCES documents_documentidentity(id),
 creation_digest TEXT NOT NULL CHECK(length(creation_digest)=64),
 revision INTEGER NOT NULL CHECK(revision>0), vendor_id TEXT NOT NULL REFERENCES directory_vendor(id),
 reference_key TEXT NOT NULL CHECK(length(reference_key)<=120), state TEXT NOT NULL CHECK(state IN('DRAFT','OPEN','VOID'))
) STRICT;
CREATE UNIQUE INDEX bill_live_vendor_reference ON payables_bill(company_id,vendor_id,reference_key) WHERE state<>'VOID' AND reference_key<>'';
CREATE TABLE payables_billrevision (
 id TEXT PRIMARY KEY,bill_id TEXT NOT NULL REFERENCES payables_bill(id),number INTEGER NOT NULL CHECK(number>0),
 state TEXT NOT NULL CHECK(state IN('DRAFT','OPEN','VOID')),action TEXT NOT NULL,reason TEXT NOT NULL CHECK(length(reason)<=500),
 actor_id TEXT NOT NULL REFERENCES fb_user(id),actor_name TEXT NOT NULL,created_at TEXT NOT NULL,
 vendor_id TEXT NOT NULL REFERENCES directory_vendor(id),vendor_snapshot TEXT NOT NULL CHECK(json_valid(vendor_snapshot)),company_snapshot TEXT NOT NULL CHECK(json_valid(company_snapshot)),
 date TEXT NOT NULL CHECK(date(date) IS date),due_date TEXT NOT NULL CHECK(date(due_date) IS due_date AND due_date>=date),
 reference TEXT NOT NULL CHECK(length(reference)<=120),supersedes_id TEXT REFERENCES payables_bill(id),reentry_reason TEXT NOT NULL CHECK(length(reentry_reason)<=500),
 journal_id TEXT REFERENCES ledger_journal(id),reversal_id TEXT REFERENCES ledger_journal(id),review_digest TEXT NOT NULL,
 memo TEXT NOT NULL CHECK(length(memo)<=2000),total INTEGER NOT NULL CHECK(total BETWEEN 0 AND 9999999999999999),
 UNIQUE(bill_id,number)
) STRICT;
CREATE TABLE payables_billline (
 id TEXT PRIMARY KEY,revision_id TEXT NOT NULL REFERENCES payables_billrevision(id),position INTEGER NOT NULL CHECK(position BETWEEN 1 AND 50),
 item_id TEXT REFERENCES directory_item(id),item_snapshot TEXT NOT NULL CHECK(json_valid(item_snapshot)),description TEXT NOT NULL CHECK(length(trim(description))>0 AND length(description)<=1000),
 quantity INTEGER NOT NULL CHECK(quantity BETWEEN 1 AND 999999999999999999),rate INTEGER NOT NULL CHECK(rate BETWEEN 0 AND 9999999999999999),amount INTEGER NOT NULL CHECK(amount BETWEEN 0 AND 9999999999999999),
 income_account_id TEXT REFERENCES ledger_account(id),expense_account_id TEXT REFERENCES ledger_account(id),account_snapshot TEXT NOT NULL CHECK(json_valid(account_snapshot)),UNIQUE(revision_id,position)
) STRICT;
CREATE TABLE payables_billpayment (
 id TEXT PRIMARY KEY,company_id TEXT NOT NULL REFERENCES ledger_company(id),identity_id TEXT NOT NULL UNIQUE REFERENCES documents_documentidentity(id),
 creation_digest TEXT NOT NULL CHECK(length(creation_digest)=64),review_digest TEXT NOT NULL CHECK(length(review_digest)=64),
 vendor_id TEXT NOT NULL REFERENCES directory_vendor(id),vendor_snapshot TEXT NOT NULL CHECK(json_valid(vendor_snapshot)),company_snapshot TEXT NOT NULL CHECK(json_valid(company_snapshot)),
 date TEXT NOT NULL CHECK(date(date) IS date),amount INTEGER NOT NULL CHECK(amount BETWEEN 1 AND 9999999999999999),
 method TEXT NOT NULL CHECK(method IN('CHECK','CASH','CARD','TRANSFER','OTHER')),reference TEXT NOT NULL CHECK(length(reference)<=120),memo TEXT NOT NULL CHECK(length(memo)<=2000),
 bank_id TEXT NOT NULL REFERENCES ledger_account(id),journal_id TEXT NOT NULL UNIQUE REFERENCES ledger_journal(id),
 actor_id TEXT NOT NULL REFERENCES fb_user(id),actor_name TEXT NOT NULL,created_at TEXT NOT NULL
) STRICT;
CREATE TABLE payables_paymentallocation (
 id TEXT PRIMARY KEY,payment_id TEXT NOT NULL REFERENCES payables_billpayment(id),bill_id TEXT NOT NULL REFERENCES payables_bill(id),
 bill_revision_id TEXT NOT NULL REFERENCES payables_billrevision(id),payables_id TEXT NOT NULL REFERENCES ledger_account(id),
 amount INTEGER NOT NULL CHECK(amount BETWEEN 1 AND 9999999999999999),UNIQUE(payment_id,bill_id)
) STRICT;
CREATE TABLE payables_paymentreversal (
 id TEXT PRIMARY KEY,payment_id TEXT NOT NULL UNIQUE REFERENCES payables_billpayment(id),journal_id TEXT NOT NULL UNIQUE REFERENCES ledger_journal(id),
 date TEXT NOT NULL CHECK(date(date) IS date),reason TEXT NOT NULL CHECK(length(trim(reason)) BETWEEN 1 AND 500),
 actor_id TEXT NOT NULL REFERENCES fb_user(id),actor_name TEXT NOT NULL,created_at TEXT NOT NULL
) STRICT;
CREATE VIEW fb_payables_live_allocations AS SELECT a.* FROM payables_paymentallocation a WHERE NOT EXISTS(SELECT 1 FROM payables_paymentreversal z WHERE z.payment_id=a.payment_id);
CREATE VIEW fb_payables_live_checks AS SELECT p.company_id,p.bank_id,p.reference AS number,p.id FROM payables_billpayment p WHERE p.method='CHECK' AND length(p.reference) BETWEEN 1 AND 20 AND p.reference NOT GLOB '*[^0-9]*' AND NOT EXISTS(SELECT 1 FROM payables_paymentreversal z WHERE z.payment_id=p.id);

CREATE TRIGGER fb_bill_insert BEFORE INSERT ON payables_bill BEGIN
 SELECT RAISE(ABORT,'bill_immutable_ownership: Bills cannot replace existing history') WHERE EXISTS(SELECT 1 FROM payables_bill WHERE id=NEW.id OR identity_id=NEW.identity_id OR (company_id=NEW.company_id AND vendor_id=NEW.vendor_id AND reference_key=NEW.reference_key AND reference_key<>'' AND state<>'VOID' AND NEW.state<>'VOID'));
 SELECT RAISE(ABORT,'bill_identity_scope: Invalid bill identity') WHERE NEW.revision<>1 OR NEW.state<>'DRAFT' OR NOT EXISTS(SELECT 1 FROM documents_documentidentity WHERE id=NEW.identity_id AND company_id=NEW.company_id AND kind='BILL') OR NOT EXISTS(SELECT 1 FROM directory_vendor WHERE id=NEW.vendor_id AND company_id=NEW.company_id);
END;
CREATE TRIGGER fb_bill_update BEFORE UPDATE ON payables_bill BEGIN
 SELECT RAISE(ABORT,'bill_immutable_ownership: Bill ownership is immutable; revisions advance by one') WHERE NEW.id IS NOT OLD.id OR NEW.company_id IS NOT OLD.company_id OR NEW.identity_id IS NOT OLD.identity_id OR NEW.creation_digest IS NOT OLD.creation_digest OR NEW.revision<>OLD.revision+1 OR NOT EXISTS(SELECT 1 FROM fb_seal WHERE kind='bill_revision' AND doc_id=OLD.id||':'||OLD.revision);
END;
CREATE TRIGGER fb_bill_delete BEFORE DELETE ON payables_bill BEGIN SELECT RAISE(ABORT,'bill_immutable_ownership: Bills cannot be deleted'); END;
CREATE TRIGGER fb_bill_due_insert AFTER INSERT ON payables_bill BEGIN INSERT INTO fb_seal_due VALUES('bill_revision',NEW.id||':'||NEW.revision); END;
CREATE TRIGGER fb_bill_due_update AFTER UPDATE ON payables_bill BEGIN INSERT INTO fb_seal_due VALUES('bill_revision',NEW.id||':'||NEW.revision); END;
CREATE TRIGGER fb_bill_revision_insert BEFORE INSERT ON payables_billrevision BEGIN
 SELECT RAISE(ABORT,'bill_history_immutable: Bill revisions are immutable') WHERE EXISTS(SELECT 1 FROM payables_billrevision WHERE id=NEW.id OR (bill_id=NEW.bill_id AND number=NEW.number));
 SELECT RAISE(ABORT,'bill_revision_scope: Invalid revision ownership') WHERE NOT EXISTS(SELECT 1 FROM payables_bill b JOIN directory_vendor v ON v.company_id=b.company_id WHERE b.id=NEW.bill_id AND b.revision=NEW.number AND v.id=NEW.vendor_id) OR EXISTS(SELECT 1 FROM fb_seal WHERE kind='bill_revision' AND doc_id=NEW.bill_id||':'||NEW.number);
 SELECT RAISE(ABORT,'bill_transition: Invalid bill state transition') WHERE (NEW.number=1 AND (NEW.state<>'DRAFT' OR NEW.action<>'created')) OR (NEW.number>1 AND NOT EXISTS(SELECT 1 FROM payables_billrevision WHERE bill_id=NEW.bill_id AND number=NEW.number-1 AND ((state='DRAFT' AND NEW.state IN('DRAFT','OPEN')) OR (state='OPEN' AND NEW.state='VOID'))));
 SELECT RAISE(ABORT,'bill_reference_current: Bill reference index does not match revision') WHERE NOT EXISTS(SELECT 1 FROM payables_bill WHERE id=NEW.bill_id AND vendor_id=NEW.vendor_id AND state=NEW.state AND reference_key=fb_lower(trim(NEW.reference,char(32,9,10,13,12,11))));
 SELECT RAISE(ABORT,'bill_reentry_link: Reference reuse requires latest voided bill and reason') WHERE NEW.state='DRAFT' AND (
 (EXISTS(SELECT 1 FROM payables_bill b JOIN payables_bill own ON own.id=NEW.bill_id WHERE b.company_id=own.company_id AND b.vendor_id=NEW.vendor_id AND b.id<>NEW.bill_id AND b.reference_key=own.reference_key AND b.reference_key<>'') AND
 (length(trim(NEW.reentry_reason))=0 OR NEW.supersedes_id IS NOT (SELECT b.id FROM payables_bill b JOIN payables_bill own ON own.id=NEW.bill_id WHERE b.company_id=own.company_id AND b.vendor_id=NEW.vendor_id AND b.id<>NEW.bill_id AND b.reference_key=own.reference_key AND b.reference_key<>'' ORDER BY b.id DESC LIMIT 1) OR (SELECT state FROM payables_bill WHERE id=NEW.supersedes_id) IS NOT 'VOID')) OR
 (NOT EXISTS(SELECT 1 FROM payables_bill b JOIN payables_bill own ON own.id=NEW.bill_id WHERE b.company_id=own.company_id AND b.vendor_id=NEW.vendor_id AND b.id<>NEW.bill_id AND b.reference_key=own.reference_key AND b.reference_key<>'') AND (NEW.supersedes_id IS NOT NULL OR NEW.reentry_reason<>'')));
 SELECT RAISE(ABORT,'bill_live_payments: Reverse live payments before voiding a bill') WHERE NEW.state='VOID' AND EXISTS(SELECT 1 FROM fb_payables_live_allocations WHERE bill_id=NEW.bill_id);
 SELECT RAISE(ABORT,'bill_posting_link: Invalid bill posting linkage') WHERE
 (NEW.state='DRAFT' AND (NEW.journal_id IS NOT NULL OR NEW.reversal_id IS NOT NULL OR NEW.review_digest<>'')) OR
 (NEW.state IN('OPEN','VOID') AND (NEW.total<=0 OR NOT EXISTS(SELECT 1 FROM ledger_journal j JOIN payables_bill b ON b.identity_id=j.source_id WHERE b.id=NEW.bill_id AND j.id=NEW.journal_id AND j.company_id=b.company_id AND j.status='POSTED' AND j.reversal_of_id IS NULL AND j.date=NEW.date))) OR
 (NEW.state='OPEN' AND (NEW.reversal_id IS NOT NULL OR length(NEW.review_digest)<>64)) OR
 (NEW.state='VOID' AND (length(trim(NEW.reason))=0 OR NOT EXISTS(SELECT 1 FROM payables_billrevision p JOIN ledger_journal j ON j.reversal_of_id=p.journal_id WHERE p.bill_id=NEW.bill_id AND p.number=NEW.number-1 AND p.journal_id=NEW.journal_id AND p.review_digest=NEW.review_digest AND j.id=NEW.reversal_id AND j.status='POSTED')));
 SELECT RAISE(ABORT,'bill_financial_immutable: Posting and void preserve financial evidence') WHERE NEW.state IN('OPEN','VOID') AND NOT EXISTS(SELECT 1 FROM payables_billrevision p WHERE p.bill_id=NEW.bill_id AND p.number=NEW.number-1 AND p.vendor_id=NEW.vendor_id AND p.vendor_snapshot=NEW.vendor_snapshot AND p.company_snapshot=NEW.company_snapshot AND p.date=NEW.date AND p.due_date=NEW.due_date AND p.memo=NEW.memo AND p.reference=NEW.reference AND p.supersedes_id IS NEW.supersedes_id AND p.reentry_reason=NEW.reentry_reason AND p.total=NEW.total);
END;
CREATE TRIGGER fb_bill_line_insert BEFORE INSERT ON payables_billline BEGIN
 SELECT RAISE(ABORT,'bill_history_immutable: Bill lines are immutable') WHERE EXISTS(SELECT 1 FROM payables_billline WHERE id=NEW.id OR (revision_id=NEW.revision_id AND position=NEW.position));
 SELECT RAISE(ABORT,'bill_line_scope: Invalid bill line scope or committed history') WHERE NOT EXISTS(SELECT 1 FROM payables_billrevision r JOIN payables_bill b ON b.id=r.bill_id WHERE r.id=NEW.revision_id AND r.number=b.revision AND NOT EXISTS(SELECT 1 FROM fb_seal WHERE kind='bill_revision' AND doc_id=b.id||':'||b.revision) AND (NEW.item_id IS NULL OR EXISTS(SELECT 1 FROM directory_item WHERE id=NEW.item_id AND company_id=b.company_id)) AND (NEW.income_account_id IS NULL OR EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.income_account_id AND company_id=b.company_id)) AND (NEW.expense_account_id IS NULL OR EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.expense_account_id AND company_id=b.company_id)));
END;
CREATE TRIGGER fb_bill_seal BEFORE INSERT ON fb_seal WHEN NEW.kind='bill_revision' BEGIN
 SELECT RAISE(ABORT,'bill_complete: Incomplete or inconsistent bill totals') WHERE NOT EXISTS(SELECT 1 FROM payables_billrevision r JOIN payables_bill b ON b.id=r.bill_id WHERE NEW.doc_id=b.id||':'||b.revision AND r.number=b.revision AND b.vendor_id=r.vendor_id AND b.state=r.state AND b.reference_key=fb_lower(trim(r.reference,char(32,9,10,13,12,11))) AND (SELECT count(*) FROM payables_billline WHERE revision_id=r.id) BETWEEN 1 AND 50 AND (SELECT min(position) FROM payables_billline WHERE revision_id=r.id)=1 AND (SELECT max(position) FROM payables_billline WHERE revision_id=r.id)=(SELECT count(*) FROM payables_billline WHERE revision_id=r.id) AND r.total=(SELECT sum(amount) FROM payables_billline WHERE revision_id=r.id));
 SELECT RAISE(ABORT,'bill_line_account: Invalid purchase posting account or inactive parent') WHERE EXISTS(SELECT 1 FROM payables_billrevision r JOIN payables_bill b ON b.id=r.bill_id WHERE NEW.doc_id=b.id||':'||r.number AND r.state='OPEN' AND (
 NOT EXISTS(SELECT 1 FROM directory_vendor WHERE id=r.vendor_id AND company_id=b.company_id AND active=1) OR
 EXISTS(SELECT 1 FROM payables_billline l WHERE l.revision_id=r.id AND ((l.item_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM directory_item WHERE id=l.item_id AND company_id=b.company_id AND active=1)) OR NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=l.expense_account_id AND a.company_id=b.company_id AND a.active=1 AND a.postable=1 AND a.detail_type IN('BANK','OTHER_CURRENT_ASSET','FIXED_ASSET','OTHER_ASSET','CREDIT_CARD','OTHER_CURRENT_LIABILITY','LONG_TERM_LIABILITY','EQUITY','COST_OF_GOODS_SOLD','EXPENSE','OTHER_EXPENSE') AND NOT EXISTS(SELECT 1 FROM fb_control_accounts WHERE company_id=b.company_id AND account_id=a.id))))));
 SELECT RAISE(ABORT,'bill_financial_immutable: Posting and void must copy exact lines') WHERE EXISTS(SELECT 1 FROM payables_billrevision r WHERE NEW.doc_id=r.bill_id||':'||r.number AND r.state IN('OPEN','VOID') AND (
 EXISTS(SELECT position,item_id,item_snapshot,description,quantity,rate,amount,income_account_id,expense_account_id,account_snapshot FROM payables_billline WHERE revision_id=r.id EXCEPT SELECT position,item_id,item_snapshot,description,quantity,rate,amount,income_account_id,expense_account_id,account_snapshot FROM payables_billline WHERE revision_id=(SELECT id FROM payables_billrevision WHERE bill_id=r.bill_id AND number=r.number-1)) OR
 EXISTS(SELECT position,item_id,item_snapshot,description,quantity,rate,amount,income_account_id,expense_account_id,account_snapshot FROM payables_billline WHERE revision_id=(SELECT id FROM payables_billrevision WHERE bill_id=r.bill_id AND number=r.number-1) EXCEPT SELECT position,item_id,item_snapshot,description,quantity,rate,amount,income_account_id,expense_account_id,account_snapshot FROM payables_billline WHERE revision_id=r.id)));
 SELECT RAISE(ABORT,'bill_journal_split: Bill must match exact expense/payable posting and frozen accounts') WHERE EXISTS(SELECT 1 FROM payables_billrevision r JOIN payables_bill b ON b.id=r.bill_id WHERE NEW.doc_id=b.id||':'||r.number AND r.state IN('OPEN','VOID') AND (
 NOT EXISTS(SELECT 1 FROM documents_documentaccount WHERE source_id=b.identity_id AND role='payables' AND account_kind='LIABILITY') OR
 EXISTS(SELECT 1 FROM payables_billline l WHERE l.revision_id=r.id AND NOT EXISTS(SELECT 1 FROM documents_documentaccount d WHERE d.source_id=b.identity_id AND d.role='expense:'||l.position AND d.account_id=l.expense_account_id)) OR
 (SELECT count(*) FROM ledger_journalline WHERE journal_id=r.journal_id)<>(SELECT count(DISTINCT expense_account_id) FROM payables_billline WHERE revision_id=r.id AND amount>0)+1 OR
 EXISTS(SELECT account_id,debit,credit FROM ledger_journalline WHERE journal_id=r.journal_id EXCEPT SELECT expense_account_id,sum(amount),0 FROM payables_billline WHERE revision_id=r.id GROUP BY expense_account_id HAVING sum(amount)>0 EXCEPT SELECT account_id,0,r.total FROM documents_documentaccount WHERE source_id=b.identity_id AND role='payables') OR
 EXISTS(SELECT expense_account_id,sum(amount),0 FROM payables_billline WHERE revision_id=r.id GROUP BY expense_account_id HAVING sum(amount)>0 EXCEPT SELECT account_id,debit,credit FROM ledger_journalline WHERE journal_id=r.journal_id) OR
 NOT EXISTS(SELECT 1 FROM ledger_journalline l JOIN documents_documentaccount d ON d.account_id=l.account_id WHERE l.journal_id=r.journal_id AND d.source_id=b.identity_id AND d.role='payables' AND l.debit=0 AND l.credit=r.total)));
END;
CREATE TRIGGER fb_payment_insert BEFORE INSERT ON payables_billpayment BEGIN
 SELECT RAISE(ABORT,'payment_immutable: Payment history is immutable') WHERE EXISTS(SELECT 1 FROM payables_billpayment WHERE id=NEW.id OR identity_id=NEW.identity_id OR journal_id=NEW.journal_id);
 SELECT RAISE(ABORT,'payment_scope: Invalid payment ownership/posting') WHERE NOT EXISTS(SELECT 1 FROM documents_documentidentity WHERE id=NEW.identity_id AND company_id=NEW.company_id AND kind='BPAY') OR NOT EXISTS(SELECT 1 FROM directory_vendor WHERE id=NEW.vendor_id AND company_id=NEW.company_id AND active=1) OR NOT EXISTS(SELECT 1 FROM ledger_account WHERE id=NEW.bank_id AND company_id=NEW.company_id AND kind='ASSET' AND active=1 AND postable=1) OR NOT EXISTS(SELECT 1 FROM documents_designatedbank d JOIN documents_accountingsetup s ON s.id=d.setup_id WHERE s.company_id=NEW.company_id AND d.account_id=NEW.bank_id) OR NOT EXISTS(SELECT 1 FROM ledger_journal WHERE id=NEW.journal_id AND company_id=NEW.company_id AND source_id=NEW.identity_id AND status='POSTED' AND reversal_of_id IS NULL AND date=NEW.date);
 SELECT RAISE(ABORT,'bill_payment_check_number: Check number already used on this bank') WHERE NEW.method='CHECK' AND length(NEW.reference) BETWEEN 1 AND 20 AND NEW.reference NOT GLOB '*[^0-9]*' AND EXISTS(SELECT 1 FROM fb_payables_live_checks WHERE company_id=NEW.company_id AND bank_id=NEW.bank_id AND number=NEW.reference);
END;
CREATE TRIGGER fb_payment_due AFTER INSERT ON payables_billpayment BEGIN INSERT INTO fb_seal_due VALUES('bill_payment',NEW.id); END;
CREATE TRIGGER fb_allocation_insert BEFORE INSERT ON payables_paymentallocation BEGIN
 SELECT RAISE(ABORT,'payment_immutable: Allocation history is immutable') WHERE EXISTS(SELECT 1 FROM payables_paymentallocation WHERE id=NEW.id OR (payment_id=NEW.payment_id AND bill_id=NEW.bill_id));
 SELECT RAISE(ABORT,'payment_late_allocation: Cannot append to committed allocations') WHERE NOT EXISTS(SELECT 1 FROM payables_billpayment WHERE id=NEW.payment_id) OR EXISTS(SELECT 1 FROM fb_seal WHERE kind='bill_payment' AND doc_id=NEW.payment_id);
 SELECT RAISE(ABORT,'payment_allocation_scope: Invalid allocation target or original payable') WHERE NOT EXISTS(SELECT 1 FROM payables_billpayment p JOIN payables_bill b ON b.company_id=p.company_id JOIN payables_billrevision r ON r.bill_id=b.id AND r.number=b.revision JOIN documents_documentaccount d ON d.source_id=b.identity_id AND d.role='payables' JOIN ledger_account a ON a.id=d.account_id WHERE p.id=NEW.payment_id AND b.id=NEW.bill_id AND r.id=NEW.bill_revision_id AND r.vendor_id=p.vendor_id AND r.state='OPEN' AND r.date<=p.date AND a.id=NEW.payables_id AND a.company_id=p.company_id AND a.kind='LIABILITY' AND a.active=1 AND a.postable=1 AND a.id<>p.bank_id);
 SELECT RAISE(ABORT,'payment_outstanding: Live allocations cannot overpay or target void bills') WHERE NEW.amount+coalesce((SELECT sum(amount) FROM fb_payables_live_allocations WHERE bill_id=NEW.bill_id),0)>(SELECT total FROM payables_billrevision WHERE id=NEW.bill_revision_id);
END;
CREATE TRIGGER fb_payment_seal BEFORE INSERT ON fb_seal WHEN NEW.kind='bill_payment' BEGIN
 SELECT RAISE(ABORT,'payment_complete: Payment allocations must equal amount') WHERE NOT EXISTS(SELECT 1 FROM payables_billpayment p WHERE p.id=NEW.doc_id AND (SELECT count(*) FROM payables_paymentallocation WHERE payment_id=p.id) BETWEEN 1 AND 50 AND p.amount=(SELECT sum(amount) FROM payables_paymentallocation WHERE payment_id=p.id));
 SELECT RAISE(ABORT,'payment_journal_split: Payment journal must exactly clear original payables') WHERE EXISTS(SELECT 1 FROM payables_billpayment p WHERE p.id=NEW.doc_id AND (
 (SELECT count(*) FROM ledger_journalline WHERE journal_id=p.journal_id)<>(SELECT count(DISTINCT payables_id) FROM payables_paymentallocation WHERE payment_id=p.id)+1 OR
 NOT EXISTS(SELECT 1 FROM ledger_journalline WHERE journal_id=p.journal_id AND account_id=p.bank_id AND debit=0 AND credit=p.amount) OR
 EXISTS(SELECT payables_id,sum(amount),0 FROM payables_paymentallocation WHERE payment_id=p.id GROUP BY payables_id EXCEPT SELECT account_id,debit,credit FROM ledger_journalline WHERE journal_id=p.journal_id)));
 SELECT RAISE(ABORT,'payment_account_evidence: Payment account evidence must match original split') WHERE EXISTS(SELECT 1 FROM payables_billpayment p WHERE p.id=NEW.doc_id AND (
 (SELECT count(*) FROM documents_documentaccount WHERE source_id=p.identity_id)<>(SELECT count(DISTINCT payables_id) FROM payables_paymentallocation WHERE payment_id=p.id)+1 OR
 NOT EXISTS(SELECT 1 FROM documents_documentaccount WHERE source_id=p.identity_id AND role='bank' AND account_id=p.bank_id) OR
 EXISTS(SELECT 1 FROM payables_paymentallocation a WHERE a.payment_id=p.id AND NOT EXISTS(SELECT 1 FROM documents_documentaccount WHERE source_id=p.identity_id AND role='payables:'||a.payables_id AND account_id=a.payables_id))));
END;
CREATE TRIGGER fb_payment_reversal_insert BEFORE INSERT ON payables_paymentreversal BEGIN
 SELECT RAISE(ABORT,'payment_immutable: Reversal history is immutable') WHERE EXISTS(SELECT 1 FROM payables_paymentreversal WHERE id=NEW.id OR payment_id=NEW.payment_id OR journal_id=NEW.journal_id);
 SELECT RAISE(ABORT,'payment_reversal_link: Invalid payment reversal') WHERE NOT EXISTS(SELECT 1 FROM payables_billpayment p JOIN ledger_journal j ON j.source_id=p.identity_id AND j.company_id=p.company_id AND j.reversal_of_id=p.journal_id WHERE p.id=NEW.payment_id AND NEW.date>=p.date AND j.id=NEW.journal_id AND j.date=NEW.date AND j.status='POSTED' AND EXISTS(SELECT 1 FROM fb_seal WHERE kind='bill_payment' AND doc_id=p.id));
END;
CREATE TRIGGER fb_payables_journal_due AFTER INSERT ON ledger_journal WHEN EXISTS(SELECT 1 FROM documents_documentidentity WHERE id=NEW.source_id AND kind IN('BILL','BPAY')) BEGIN INSERT INTO fb_seal_due VALUES('payables_journal',NEW.id); END;
CREATE TRIGGER fb_payables_journal_seal BEFORE INSERT ON fb_seal WHEN NEW.kind='payables_journal' BEGIN
 SELECT RAISE(ABORT,'payables_journal_owner: Journal requires complete owning lifecycle') WHERE NOT EXISTS(
 SELECT 1 FROM ledger_journal j WHERE j.id=NEW.doc_id AND j.status='POSTED' AND (
 EXISTS(SELECT 1 FROM payables_bill b JOIN payables_billrevision r ON r.bill_id=b.id AND r.number=b.revision JOIN fb_seal s ON s.kind='bill_revision' AND s.doc_id=b.id||':'||b.revision WHERE b.identity_id=j.source_id AND ((j.reversal_of_id IS NULL AND r.journal_id=j.id) OR (r.state='VOID' AND r.journal_id=j.reversal_of_id AND r.reversal_id=j.id))) OR
 EXISTS(SELECT 1 FROM payables_billpayment p JOIN fb_seal s ON s.kind='bill_payment' AND s.doc_id=p.id WHERE p.identity_id=j.source_id AND ((j.reversal_of_id IS NULL AND p.journal_id=j.id) OR EXISTS(SELECT 1 FROM payables_paymentreversal z WHERE z.payment_id=p.id AND z.journal_id=j.id AND p.journal_id=j.reversal_of_id)))));
END;
CREATE TRIGGER fb_payables_late_reference BEFORE INSERT ON documents_documentaccount WHEN EXISTS(SELECT 1 FROM ledger_journal j JOIN fb_seal s ON s.kind='payables_journal' AND s.doc_id=j.id WHERE j.source_id=NEW.source_id) BEGIN SELECT RAISE(ABORT,'payment_late_reference: Cannot append posted account evidence'); END;
