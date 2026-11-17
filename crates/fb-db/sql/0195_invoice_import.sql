-- Immutable source evidence supplements native invoices; native identities and ledger services remain authoritative.
CREATE TABLE sales_importinvoice (
 company_id TEXT NOT NULL REFERENCES ledger_company(id),
 source TEXT NOT NULL CHECK(source='CyberBrain'),
 source_id TEXT NOT NULL CHECK(length(source_id)=36),
 source_number TEXT NOT NULL CHECK(length(source_number) BETWEEN 1 AND 80),
 invoice_id TEXT NOT NULL UNIQUE REFERENCES sales_invoice(id) REFERENCES sales_importseal(invoice_id) DEFERRABLE INITIALLY DEFERRED,
 revision_id TEXT NOT NULL UNIQUE REFERENCES sales_invoicerevision(id),
 digest TEXT NOT NULL CHECK(length(digest)=64 AND digest NOT GLOB '*[^0-9a-f]*'),
 data TEXT NOT NULL CHECK(json_valid(data) AND json_type(data)='object' AND length(data)<=262144),
 actor_id TEXT NOT NULL REFERENCES fb_user(id),
 audit_id TEXT NOT NULL UNIQUE REFERENCES ledger_auditevent(id),
 PRIMARY KEY(company_id,source,source_id),
 UNIQUE(company_id,source,source_number)
) STRICT;
CREATE TABLE sales_importpayment (
 company_id TEXT NOT NULL REFERENCES ledger_company(id),
 source TEXT NOT NULL CHECK(source='CyberBrain'),
 source_id TEXT NOT NULL CHECK(length(source_id)=36),
 invoice_id TEXT NOT NULL REFERENCES sales_importinvoice(invoice_id),
 receipt_id TEXT NOT NULL UNIQUE REFERENCES sales_receipt(id),
 external_key TEXT NOT NULL CHECK(length(external_key)<=120),
 PRIMARY KEY(company_id,source,source_id)
) STRICT;
CREATE UNIQUE INDEX sales_importpayment_external ON sales_importpayment(company_id,source,external_key) WHERE external_key<>'';
CREATE VIEW sales_import_violations AS
 SELECT h.invoice_id problem FROM sales_importinvoice h
 LEFT JOIN sales_invoice i ON i.id=h.invoice_id
 LEFT JOIN sales_invoicerevision r ON r.id=h.revision_id
 LEFT JOIN ledger_auditevent a ON a.id=h.audit_id
 WHERE NOT fb_valid_uuid(h.company_id) OR NOT fb_valid_uuid(h.source_id) OR NOT fb_valid_uuid(h.invoice_id) OR NOT fb_valid_uuid(h.revision_id) OR NOT fb_valid_uuid(h.actor_id) OR NOT fb_valid_uuid(h.audit_id) OR NOT fb_valid_text(h.source_number)
 OR fb_invoice_import_digest(h.data) IS NOT h.digest
 OR NOT fb_invoice_import_matches(h.data,r.data,(SELECT json_group_array(json(data)) FROM (SELECT data FROM sales_invoiceline WHERE revision_id=h.revision_id ORDER BY position)))
 OR NOT fb_json_unique_keys(h.data) OR i.company_id IS NOT h.company_id OR r.invoice_id IS NOT h.invoice_id
 OR a.company_id IS NOT h.company_id OR a.actor_id IS NOT h.actor_id OR a.action IS NOT 'invoice.imported' OR a.target IS NOT 'invoice:'||h.invoice_id
 OR json_extract(a.detail,'$.source') IS NOT h.source OR json_extract(a.detail,'$.source_id') IS NOT h.source_id OR json_extract(a.detail,'$.digest') IS NOT h.digest
 OR json_extract(h.data,'$.source.id') IS NOT h.source_id
 OR coalesce(nullif(json_extract(h.data,'$.source.display_number'),''),cast(json_extract(h.data,'$.source.number') AS TEXT)) IS NOT h.source_number
 OR json_extract(h.data,'$.invoice.customer_id') IS NOT r.customer_id OR json_extract(h.data,'$.invoice.date') IS NOT r.date OR json_extract(h.data,'$.invoice.due_date') IS NOT r.due_date
 OR fb_source_money(json_quote(json_extract(h.data,'$.source.total'))) IS NOT r.total
 OR fb_source_money(json_quote(json_extract(h.data,'$.source.tax_amount'))) IS NOT fb_sales_money(json_quote(json_extract(r.data,'$.tax')))
 OR fb_source_money(json_quote(json_extract(h.data,'$.source.subtotal')))+fb_source_money(json_quote(json_extract(h.data,'$.source.gc_discount'))) IS NOT fb_sales_money(json_quote(json_extract(r.data,'$.subtotal')))
 OR (SELECT count(*) FROM sales_invoiceline WHERE revision_id=r.id) IS NOT json_array_length(h.data,'$.source.lines')
 OR EXISTS(SELECT 1 FROM sales_invoiceline l JOIN json_each(h.data,'$.source.lines') s ON cast(s.key AS INTEGER)+1=l.position WHERE l.revision_id=r.id AND CASE WHEN json_extract(l.data,'$.line_kind')='DISCOUNT' THEN -l.amount ELSE l.amount END IS NOT fb_source_money(json_quote(json_extract(s.value,'$.amount'))))
 OR (json_extract(h.data,'$.source.status')='draft' AND r.state<>'DRAFT')
 OR (json_extract(h.data,'$.source.status')='void' AND r.state<>'VOID')
 OR (json_extract(h.data,'$.source.status') IN('paid','partial','sent') AND r.state<>'OPEN')
 UNION ALL
 SELECT p.receipt_id FROM sales_importpayment p
 LEFT JOIN sales_importinvoice i ON i.invoice_id=p.invoice_id
 LEFT JOIN sales_receipt r ON r.id=p.receipt_id
 WHERE NOT fb_valid_uuid(p.company_id) OR NOT fb_valid_uuid(p.source_id) OR NOT fb_valid_uuid(p.invoice_id) OR NOT fb_valid_uuid(p.receipt_id) OR NOT fb_valid_text(p.external_key) OR i.company_id IS NOT p.company_id OR i.source IS NOT p.source OR r.company_id IS NOT p.company_id OR r.kind IS NOT 'RCP'
 OR NOT EXISTS(SELECT 1 FROM sales_receiptallocation a WHERE a.receipt_id=p.receipt_id AND a.invoice_id=p.invoice_id AND a.amount=r.amount)
 OR NOT EXISTS(SELECT 1 FROM json_each(i.data,'$.source.payments') s WHERE json_extract(s.value,'$.id')=p.source_id AND json_extract(s.value,'$.invoice_id')=i.source_id AND coalesce(json_extract(s.value,'$.external_id'),'')=p.external_key AND fb_source_money(json_quote(json_extract(s.value,'$.amount')))=r.amount AND json_extract(s.value,'$.date')=r.date AND coalesce(json_extract(s.value,'$.reference'),'')=json_extract(r.data,'$.reference') AND CASE json_extract(s.value,'$.method') WHEN 'card' THEN 'CARD' WHEN 'cash' THEN 'CASH' WHEN 'check' THEN 'CHECK' WHEN 'transfer' THEN 'TRANSFER' WHEN 'zelle' THEN 'TRANSFER' WHEN 'other' THEN 'OTHER' END=json_extract(r.data,'$.method'));
CREATE TRIGGER sales_importinvoice_insert BEFORE INSERT ON sales_importinvoice BEGIN
 SELECT RAISE(ABORT,'invoice_import_identity: Import identity is already recorded') WHERE EXISTS(SELECT 1 FROM sales_importinvoice WHERE (company_id=NEW.company_id AND source=NEW.source AND (source_id=NEW.source_id OR source_number=NEW.source_number)) OR invoice_id=NEW.invoice_id OR revision_id=NEW.revision_id OR audit_id=NEW.audit_id);
END;
CREATE TRIGGER sales_importinvoice_valid AFTER INSERT ON sales_importinvoice BEGIN
 SELECT RAISE(ABORT,'invoice_import_evidence: Source evidence must match the invoice and audit') WHERE EXISTS(SELECT 1 FROM sales_import_violations WHERE problem=NEW.invoice_id);
END;
CREATE TRIGGER sales_importinvoice_update BEFORE UPDATE ON sales_importinvoice BEGIN SELECT RAISE(ABORT,'invoice_import_immutable: Imported evidence is immutable'); END;
CREATE TRIGGER sales_importinvoice_delete BEFORE DELETE ON sales_importinvoice BEGIN SELECT RAISE(ABORT,'invoice_import_immutable: Imported evidence is retained'); END;
CREATE TRIGGER sales_importpayment_insert BEFORE INSERT ON sales_importpayment BEGIN
 SELECT RAISE(ABORT,'invoice_import_identity: Payment identity is already recorded') WHERE EXISTS(SELECT 1 FROM sales_importpayment WHERE (company_id=NEW.company_id AND source=NEW.source AND (source_id=NEW.source_id OR (NEW.external_key<>'' AND external_key=NEW.external_key))) OR receipt_id=NEW.receipt_id);
END;
CREATE TRIGGER sales_importpayment_valid AFTER INSERT ON sales_importpayment BEGIN
 SELECT RAISE(ABORT,'invoice_import_evidence: Source payment must match its receipt') WHERE EXISTS(SELECT 1 FROM sales_import_violations WHERE problem=NEW.receipt_id);
END;
CREATE TRIGGER sales_importpayment_update BEFORE UPDATE ON sales_importpayment BEGIN SELECT RAISE(ABORT,'invoice_import_immutable: Imported payment evidence is immutable'); END;
CREATE TRIGGER sales_importpayment_delete BEFORE DELETE ON sales_importpayment BEGIN SELECT RAISE(ABORT,'invoice_import_immutable: Imported payment evidence is retained'); END;

CREATE TABLE sales_importseal(invoice_id TEXT PRIMARY KEY REFERENCES sales_importinvoice(invoice_id)) STRICT;
CREATE TRIGGER sales_importseal_insert BEFORE INSERT ON sales_importseal BEGIN
 SELECT RAISE(ABORT,'invoice_import_complete: Imported payments and source totals must agree') WHERE EXISTS(SELECT 1 FROM sales_importseal WHERE invoice_id=NEW.invoice_id) OR NOT EXISTS(
 SELECT 1 FROM sales_importinvoice h JOIN sales_invoicerevision r ON r.id=h.revision_id WHERE h.invoice_id=NEW.invoice_id
 AND json_extract(h.data,'$.source.status') IN('draft','sent','partial','paid','void')
 AND json_array_length(h.data,'$.source.payments')=(SELECT count(*) FROM sales_importpayment WHERE invoice_id=h.invoice_id)
 AND json_array_length(h.data,'$.payments')=(SELECT count(*) FROM sales_importpayment WHERE invoice_id=h.invoice_id)
 AND fb_source_money(json_quote(json_extract(h.data,'$.source.amount_paid')))=(SELECT coalesce(sum(p.amount),0) FROM sales_importpayment i JOIN sales_receipt p ON p.id=i.receipt_id WHERE i.invoice_id=h.invoice_id)
 AND (json_extract(h.data,'$.source.status')<>'paid' OR fb_source_money(json_quote(json_extract(h.data,'$.source.amount_paid')))=r.total)
 AND (json_extract(h.data,'$.source.status') NOT IN('void','draft') OR NOT EXISTS(SELECT 1 FROM sales_importpayment WHERE invoice_id=h.invoice_id))
 AND NOT EXISTS(SELECT 1 FROM sales_import_violations WHERE problem=h.invoice_id OR problem IN(SELECT receipt_id FROM sales_importpayment WHERE invoice_id=h.invoice_id)));
END;
CREATE TRIGGER sales_importseal_update BEFORE UPDATE ON sales_importseal BEGIN SELECT RAISE(ABORT,'invoice_import_immutable: Imported seals are immutable'); END;
CREATE TRIGGER sales_importseal_delete BEFORE DELETE ON sales_importseal BEGIN SELECT RAISE(ABORT,'invoice_import_immutable: Imported seals are retained'); END;
CREATE TRIGGER sales_importpayment_sealed BEFORE INSERT ON sales_importpayment WHEN EXISTS(SELECT 1 FROM sales_importseal WHERE invoice_id=NEW.invoice_id) BEGIN SELECT RAISE(ABORT,'invoice_import_immutable: Sealed payment history is immutable'); END;

-- Presentation uses original imported invoice numbers while immutable native identity/counters remain unchanged.
CREATE VIEW documents_displayidentity AS
 SELECT d.id,d.company_id,d.kind,d.ordinal,coalesce(h.source_number,d.number) AS number,
 d.request_key,d.currency,d.created_at,d.number AS native_number
 FROM documents_documentidentity d LEFT JOIN sales_invoice i ON i.identity_id=d.id
 LEFT JOIN sales_importinvoice h ON h.invoice_id=i.id AND h.company_id=d.company_id;

CREATE TRIGGER fb_valid_sales_importinvoice_insert BEFORE INSERT ON sales_importinvoice WHEN NOT fb_valid_uuid(NEW.company_id) OR NOT fb_valid_text(NEW.source) OR NOT fb_valid_uuid(NEW.source_id) OR NOT fb_valid_text(NEW.source_number) OR NOT fb_valid_uuid(NEW.invoice_id) OR NOT fb_valid_uuid(NEW.revision_id) OR NOT fb_valid_text(NEW.digest) OR NOT fb_valid_text(NEW.data) OR NOT fb_valid_uuid(NEW.actor_id) OR NOT fb_valid_uuid(NEW.audit_id) BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value'); END;

CREATE TRIGGER fb_valid_sales_importinvoice_update BEFORE UPDATE ON sales_importinvoice WHEN NOT fb_valid_uuid(NEW.company_id) OR NOT fb_valid_text(NEW.source) OR NOT fb_valid_uuid(NEW.source_id) OR NOT fb_valid_text(NEW.source_number) OR NOT fb_valid_uuid(NEW.invoice_id) OR NOT fb_valid_uuid(NEW.revision_id) OR NOT fb_valid_text(NEW.digest) OR NOT fb_valid_text(NEW.data) OR NOT fb_valid_uuid(NEW.actor_id) OR NOT fb_valid_uuid(NEW.audit_id) BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value'); END;

CREATE TRIGGER fb_valid_sales_importpayment_insert BEFORE INSERT ON sales_importpayment WHEN NOT fb_valid_uuid(NEW.company_id) OR NOT fb_valid_text(NEW.source) OR NOT fb_valid_uuid(NEW.source_id) OR NOT fb_valid_uuid(NEW.invoice_id) OR NOT fb_valid_uuid(NEW.receipt_id) OR NOT fb_valid_text(NEW.external_key) BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value'); END;

CREATE TRIGGER fb_valid_sales_importpayment_update BEFORE UPDATE ON sales_importpayment WHEN NOT fb_valid_uuid(NEW.company_id) OR NOT fb_valid_text(NEW.source) OR NOT fb_valid_uuid(NEW.source_id) OR NOT fb_valid_uuid(NEW.invoice_id) OR NOT fb_valid_uuid(NEW.receipt_id) OR NOT fb_valid_text(NEW.external_key) BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value'); END;

CREATE TRIGGER fb_valid_sales_importseal_insert BEFORE INSERT ON sales_importseal WHEN NOT fb_valid_uuid(NEW.invoice_id) BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value'); END;

CREATE TRIGGER fb_valid_sales_importseal_update BEFORE UPDATE ON sales_importseal WHEN NOT fb_valid_uuid(NEW.invoice_id) BEGIN SELECT RAISE(ABORT,'fb_valid: Invalid stored value'); END;

DROP VIEW fb_value_invalid;
CREATE VIEW fb_value_invalid AS
SELECT 'invalid stored value in accounts_owner' AS problem FROM accounts_owner WHERE
 NOT fb_valid_uuid(user_id)
UNION ALL
SELECT 'invalid stored value in assistant_connection' AS problem FROM assistant_connection WHERE
 NOT fb_valid_text(account_email) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(created_by_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(key_last4) OR
 NOT fb_valid_text(last_error) OR
 NOT fb_valid_text(mode) OR
 NOT fb_valid_text(model) OR
 NOT fb_valid_text(model_choices) OR
 NOT fb_valid_text(status)
UNION ALL
SELECT 'invalid stored value in assistant_reading' AS problem FROM assistant_reading WHERE
 NOT fb_valid_uuid(bill_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(content_type) OR
 NOT fb_valid_uuid(created_by_id) OR
 NOT fb_valid_text(error) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(mode) OR
 NOT fb_valid_text(model) OR
 NOT fb_valid_uuid(origin_bill_id) OR
 NOT fb_valid_text(original_name) OR
 NOT fb_valid_uuid(request_key) OR
 NOT fb_valid_text(result) OR
 NOT fb_valid_text(sha256) OR
 NOT fb_valid_uuid(source_file_id) OR
 NOT fb_valid_text(status)
UNION ALL
SELECT 'invalid stored value in attachments_attachment' AS problem FROM attachments_attachment WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(content_type) OR
 NOT fb_valid_text(delete_reason) OR
 NOT fb_valid_timestamp(deleted_at) OR
 NOT fb_valid_uuid(deleted_by_id) OR
 NOT fb_valid_text(deleted_by_name) OR
 NOT fb_valid_text(deleted_number) OR
 NOT fb_valid_uuid(document_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(nonce) OR
 NOT fb_valid_text(original_name) OR
 NOT fb_valid_text(original_sha256) OR
 NOT fb_valid_uuid(request_key) OR
 NOT fb_valid_text(sha256) OR
 NOT fb_valid_text(stored_name) OR
 NOT fb_valid_timestamp(uploaded_at) OR
 NOT fb_valid_uuid(uploaded_by_id) OR
 NOT fb_valid_text(uploaded_by_name) OR
 NOT fb_valid_text(uploaded_number) OR
 NOT fb_valid_text(wrap_nonce) OR
 NOT fb_valid_text(wrapped_key)
UNION ALL
SELECT 'invalid stored value in bankfeeds_account' AS problem FROM bankfeeds_account WHERE
 NOT fb_valid_timestamp(balance_at) OR
 NOT fb_valid_text(conn_key) OR
 NOT fb_valid_text(conn_key_identity) OR
 NOT fb_valid_uuid(connection_id) OR
 NOT fb_valid_text(currency) OR
 NOT fb_valid_timestamp(first_seen_at) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_timestamp(last_seen_at) OR
 NOT fb_valid_uuid(ledger_account_id) OR
 NOT fb_valid_timestamp(mapped_at) OR
 NOT fb_valid_text(name) OR
 NOT fb_valid_text(org_name) OR
 NOT fb_valid_text(remote_id) OR
 NOT fb_valid_text(remote_identity)
UNION ALL
SELECT 'invalid stored value in bankfeeds_connection' AS problem FROM bankfeeds_connection WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_uuid(created_by_id) OR
 NOT fb_valid_timestamp(forgotten_at) OR
 NOT fb_valid_text(host) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(label) OR
 NOT fb_valid_timestamp(last_sync_at) OR
 NOT fb_valid_text(last_sync_summary)
UNION ALL
SELECT 'invalid stored value in bankfeeds_counter' AS problem FROM bankfeeds_counter WHERE
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in bankfeeds_identity_secret' AS problem FROM bankfeeds_identity_secret WHERE
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in bankfeeds_retired' AS problem FROM bankfeeds_retired WHERE
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in bankfeeds_rule' AS problem FROM bankfeeds_rule WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(match_text) OR
 NOT fb_valid_text(name) OR
 NOT fb_valid_text(payee_name) OR
 NOT fb_valid_text(sign) OR
 NOT fb_valid_timestamp(updated_at)
UNION ALL
SELECT 'invalid stored value in bankfeeds_transaction' AS problem FROM bankfeeds_transaction WHERE
 NOT fb_valid_text(accepted_as) OR
 NOT fb_valid_timestamp(decided_at) OR
 NOT fb_valid_uuid(decided_by_id) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(feed_account_id) OR
 NOT fb_valid_timestamp(first_seen_at) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(journal_line_id) OR
 NOT fb_valid_text(posted_on) OR
 NOT fb_valid_text(reference) OR
 NOT fb_valid_text(reference_url) OR
 NOT fb_valid_text(remote_id) OR
 NOT fb_valid_text(remote_identity) OR
 NOT fb_valid_text(status) OR
 NOT fb_valid_text(transacted_on) OR
 NOT fb_valid_timestamp(updated_at)
UNION ALL
SELECT 'invalid stored value in banking_banklinestatus' AS problem FROM banking_banklinestatus WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_uuid(changed_by_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_line_id) OR
 NOT fb_valid_uuid(reconciliation_id) OR
 NOT fb_valid_text(state)
UNION ALL
SELECT 'invalid stored value in banking_deposit' AS problem FROM banking_deposit WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id)
UNION ALL
SELECT 'invalid stored value in banking_depositline' AS problem FROM banking_depositline WHERE
 NOT fb_valid_uuid(deposit_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(receipt_id) OR
 NOT fb_valid_uuid(undeposited_id)
UNION ALL
SELECT 'invalid stored value in banking_depositreversal' AS problem FROM banking_depositreversal WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(deposit_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id)
UNION ALL
SELECT 'invalid stored value in banking_reconciliation' AS problem FROM banking_reconciliation WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(request_key) OR
 NOT fb_valid_uuid(started_by_id) OR
 NOT fb_valid_date(statement_date) OR
 NOT fb_valid_text(status)
UNION ALL
SELECT 'invalid stored value in banking_reconciliationactor' AS problem FROM banking_reconciliationactor WHERE
 NOT fb_valid_text(phase) OR
 NOT fb_valid_uuid(reconciliation_id) OR
 NOT fb_valid_uuid(user_id)
UNION ALL
SELECT 'invalid stored value in banking_reconciliationline' AS problem FROM banking_reconciliationline WHERE
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_line_id) OR
 NOT fb_valid_uuid(reconciliation_id)
UNION ALL
SELECT 'invalid stored value in banking_seal' AS problem FROM banking_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in banking_seal_due' AS problem FROM banking_seal_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in checks_check' AS problem FROM checks_check WHERE
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_text(number_key) OR
 NOT fb_valid_text(state) OR
 NOT fb_valid_uuid(vendor_id)
UNION ALL
SELECT 'invalid stored value in checks_checkline' AS problem FROM checks_checkline WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_text(account_snapshot) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(item_id) OR
 NOT fb_valid_text(item_snapshot) OR
 NOT fb_valid_uuid(revision_id) OR
 NOT fb_valid_text(tab)
UNION ALL
SELECT 'invalid stored value in checks_checkrevision' AS problem FROM checks_checkrevision WHERE
 NOT fb_valid_text(action) OR
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_text(address) OR
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_text(bank_snapshot) OR
 NOT fb_valid_text(check_number) OR
 NOT fb_valid_text(company_snapshot) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(document_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(memo) OR
 NOT fb_valid_text(payee_kind) OR
 NOT fb_valid_text(payee_name) OR
 NOT fb_valid_text(payee_snapshot) OR
 NOT fb_valid_text(reason) OR
 NOT fb_valid_uuid(reversal_id) OR
 NOT fb_valid_text(review_digest) OR
 NOT fb_valid_text(state) OR
 NOT fb_valid_uuid(vendor_id)
UNION ALL
SELECT 'invalid stored value in credits_creditapplication' AS problem FROM credits_creditapplication WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(credit_memo_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(request_key)
UNION ALL
SELECT 'invalid stored value in credits_creditapplicationline' AS problem FROM credits_creditapplicationline WHERE
 NOT fb_valid_uuid(application_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(invoice_revision_id) OR
 NOT fb_valid_uuid(receivables_id)
UNION ALL
SELECT 'invalid stored value in credits_creditmemo' AS problem FROM credits_creditmemo WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(collected_tax_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(receivables_id) OR
 NOT fb_valid_uuid(tax_item_id)
UNION ALL
SELECT 'invalid stored value in credits_creditmemoline' AS problem FROM credits_creditmemoline WHERE
 NOT fb_valid_uuid(credit_memo_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_uuid(item_id)
UNION ALL
SELECT 'invalid stored value in credits_creditmemovoid' AS problem FROM credits_creditmemovoid WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(credit_memo_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id)
UNION ALL
SELECT 'invalid stored value in credits_creditrefund' AS problem FROM credits_creditrefund WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_text(check_number) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(credit_memo_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(method) OR
 NOT fb_valid_uuid(receivables_id)
UNION ALL
SELECT 'invalid stored value in credits_creditrefundvoid' AS problem FROM credits_creditrefundvoid WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(refund_id)
UNION ALL
SELECT 'invalid stored value in credits_creditunapplication' AS problem FROM credits_creditunapplication WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(application_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id)
UNION ALL
SELECT 'invalid stored value in credits_seal' AS problem FROM credits_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in credits_seal_due' AS problem FROM credits_seal_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in directory_customer' AS problem FROM directory_customer WHERE
 NOT fb_valid_text(billing_address) OR
 NOT fb_valid_text(business_name) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(contact_name) OR
 NOT fb_valid_text(display_name) OR
 NOT fb_valid_text(email) OR
 NOT fb_valid_text(exemption_number) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(notes) OR
 NOT fb_valid_text(phone) OR
 NOT fb_valid_text(tax_status)
UNION ALL
SELECT 'invalid stored value in directory_item' AS problem FROM directory_item WHERE
 NOT fb_valid_text(code) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(expense_account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in directory_vendor' AS problem FROM directory_vendor WHERE
 NOT fb_valid_text(billing_address) OR
 NOT fb_valid_text(business_name) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(contact_name) OR
 NOT fb_valid_uuid(default_expense_account_id) OR
 NOT fb_valid_text(default_terms) OR
 NOT fb_valid_text(display_name) OR
 NOT fb_valid_text(email) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(notes) OR
 NOT fb_valid_text(phone)
UNION ALL
SELECT 'invalid stored value in documents_accountingsetup' AS problem FROM documents_accountingsetup WHERE
 NOT fb_valid_uuid(collected_tax_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(customer_deposits_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(payables_id) OR
 NOT fb_valid_uuid(receivables_id) OR
 NOT fb_valid_uuid(undeposited_id)
UNION ALL
SELECT 'invalid stored value in documents_designatedbank' AS problem FROM documents_designatedbank WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(setup_id)
UNION ALL
SELECT 'invalid stored value in documents_documentaccount' AS problem FROM documents_documentaccount WHERE
 NOT fb_valid_text(account_code) OR
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_text(account_kind) OR
 NOT fb_valid_text(account_name) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(role) OR
 NOT fb_valid_uuid(source_id)
UNION ALL
SELECT 'invalid stored value in documents_documentidentity' AS problem FROM documents_documentidentity WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_text(currency) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(kind) OR
 NOT fb_valid_text(number) OR
 NOT fb_valid_uuid(request_key)
UNION ALL
SELECT 'invalid stored value in documents_documentsequence' AS problem FROM documents_documentsequence WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in estimate_seal' AS problem FROM estimate_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in estimate_seal_due' AS problem FROM estimate_seal_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in fb_seal' AS problem FROM fb_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in fb_seal_due' AS problem FROM fb_seal_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in fb_user' AS problem FROM fb_user WHERE
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(username)
UNION ALL
SELECT 'invalid stored value in ledger_account' AS problem FROM ledger_account WHERE
 NOT fb_valid_text(code) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_text(detail_type) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(kind) OR
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in ledger_auditevent' AS problem FROM ledger_auditevent WHERE
 NOT fb_valid_text(action) OR
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(detail) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_timestamp(occurred_at) OR
 NOT fb_valid_text(target)
UNION ALL
SELECT 'invalid stored value in ledger_company' AS problem FROM ledger_company WHERE
 NOT fb_valid_text(currency) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in ledger_company_members' AS problem FROM ledger_company_members WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(user_id)
UNION ALL
SELECT 'invalid stored value in ledger_journal' AS problem FROM ledger_journal WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_uuid(created_by_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_text(entry_number) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(memo) OR
 NOT fb_valid_timestamp(posted_at) OR
 NOT fb_valid_uuid(posted_by_id) OR
 NOT fb_valid_uuid(request_key) OR
 NOT fb_valid_uuid(reversal_of_id) OR
 NOT fb_valid_uuid(source_id) OR
 NOT fb_valid_text(status)
UNION ALL
SELECT 'invalid stored value in ledger_journalline' AS problem FROM ledger_journalline WHERE
 NOT fb_valid_uuid(account_id) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in payables_bill' AS problem FROM payables_bill WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_text(reference_key) OR
 NOT fb_valid_text(state) OR
 NOT fb_valid_uuid(vendor_id)
UNION ALL
SELECT 'invalid stored value in payables_billline' AS problem FROM payables_billline WHERE
 NOT fb_valid_text(account_snapshot) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(expense_account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_uuid(item_id) OR
 NOT fb_valid_text(item_snapshot) OR
 NOT fb_valid_uuid(revision_id)
UNION ALL
SELECT 'invalid stored value in payables_billpayment' AS problem FROM payables_billpayment WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(company_snapshot) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(memo) OR
 NOT fb_valid_text(method) OR
 NOT fb_valid_text(reference) OR
 NOT fb_valid_text(review_digest) OR
 NOT fb_valid_uuid(vendor_id) OR
 NOT fb_valid_text(vendor_snapshot)
UNION ALL
SELECT 'invalid stored value in payables_billrevision' AS problem FROM payables_billrevision WHERE
 NOT fb_valid_text(action) OR
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_uuid(bill_id) OR
 NOT fb_valid_text(company_snapshot) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_date(due_date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(memo) OR
 NOT fb_valid_text(reason) OR
 NOT fb_valid_text(reentry_reason) OR
 NOT fb_valid_text(reference) OR
 NOT fb_valid_uuid(reversal_id) OR
 NOT fb_valid_text(review_digest) OR
 NOT fb_valid_text(state) OR
 NOT fb_valid_uuid(supersedes_id) OR
 NOT fb_valid_uuid(vendor_id) OR
 NOT fb_valid_text(vendor_snapshot)
UNION ALL
SELECT 'invalid stored value in payables_paymentallocation' AS problem FROM payables_paymentallocation WHERE
 NOT fb_valid_uuid(bill_id) OR
 NOT fb_valid_uuid(bill_revision_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(payables_id) OR
 NOT fb_valid_uuid(payment_id)
UNION ALL
SELECT 'invalid stored value in payables_paymentreversal' AS problem FROM payables_paymentreversal WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(payment_id) OR
 NOT fb_valid_text(reason)
UNION ALL
SELECT 'invalid stored value in plugin_credential' AS problem FROM plugin_credential WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(plugin_id) OR
 NOT fb_valid_uuid(connection_id)
UNION ALL
SELECT 'invalid stored value in prepayments_customerdeposit' AS problem FROM prepayments_customerdeposit WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(estimate_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(liability_id) OR
 NOT fb_valid_text(order_reference) OR
 NOT fb_valid_uuid(receipt_id)
UNION ALL
SELECT 'invalid stored value in prepayments_depositapplication' AS problem FROM prepayments_depositapplication WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(deposit_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(invoice_revision_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(liability_id) OR
 NOT fb_valid_uuid(receivables_id)
UNION ALL
SELECT 'invalid stored value in prepayments_depositrefund' AS problem FROM prepayments_depositrefund WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(bank_id) OR
 NOT fb_valid_text(check_number) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(deposit_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(liability_id) OR
 NOT fb_valid_text(method)
UNION ALL
SELECT 'invalid stored value in prepayments_depositrefundvoid' AS problem FROM prepayments_depositrefundvoid WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(refund_id)
UNION ALL
SELECT 'invalid stored value in prepayments_depositunapplication' AS problem FROM prepayments_depositunapplication WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(application_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id)
UNION ALL
SELECT 'invalid stored value in prepayments_due' AS problem FROM prepayments_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in prepayments_seal' AS problem FROM prepayments_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in sales_estimate' AS problem FROM sales_estimate WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id)
UNION ALL
SELECT 'invalid stored value in sales_estimateconversion' AS problem FROM sales_estimateconversion WHERE
 NOT fb_valid_text(digest) OR
 NOT fb_valid_uuid(estimate_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(revision_id)
UNION ALL
SELECT 'invalid stored value in sales_estimateline' AS problem FROM sales_estimateline WHERE
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(expense_account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_uuid(item_id) OR
 NOT fb_valid_uuid(revision_id)
UNION ALL
SELECT 'invalid stored value in sales_estimaterevision' AS problem FROM sales_estimaterevision WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(estimate_id) OR
 NOT fb_valid_text(expiry) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(state)
UNION ALL
SELECT 'invalid stored value in sales_invoice' AS problem FROM sales_invoice WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id)
UNION ALL
SELECT 'invalid stored value in sales_invoiceline' AS problem FROM sales_invoiceline WHERE
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(expense_account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_uuid(item_id) OR
 NOT fb_valid_uuid(revision_id)
UNION ALL
SELECT 'invalid stored value in sales_invoicerevision' AS problem FROM sales_invoicerevision WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_date(due_date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(reversal_id) OR
 NOT fb_valid_text(state)
UNION ALL
SELECT 'invalid stored value in sales_receipt' AS problem FROM sales_receipt WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(customer_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_text(destination) OR
 NOT fb_valid_uuid(destination_account_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in sales_receiptallocation' AS problem FROM sales_receiptallocation WHERE
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(invoice_revision_id) OR
 NOT fb_valid_uuid(receipt_id) OR
 NOT fb_valid_uuid(receivables_id)
UNION ALL
SELECT 'invalid stored value in sales_receiptreversal' AS problem FROM sales_receiptreversal WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_uuid(receipt_id)
UNION ALL
SELECT 'invalid stored value in sales_salesreceipt' AS problem FROM sales_salesreceipt WHERE
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(receipt_id)
UNION ALL
SELECT 'invalid stored value in sales_salesreceiptline' AS problem FROM sales_salesreceiptline WHERE
 NOT fb_valid_text(data) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(income_account_id) OR
 NOT fb_valid_uuid(item_id) OR
 NOT fb_valid_uuid(sale_id)
UNION ALL
SELECT 'invalid stored value in sales_seal' AS problem FROM sales_seal WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in sales_seal_due' AS problem FROM sales_seal_due WHERE
 NOT fb_valid_text(doc_id) OR
 NOT fb_valid_text(kind)
UNION ALL
SELECT 'invalid stored value in salestax_salestaxsettings' AS problem FROM salestax_salestaxsettings WHERE
 NOT fb_valid_text(bulk_step) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_uuid(default_item_id) OR
 NOT fb_valid_text(filing_frequency) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(owed_basis)
UNION ALL
SELECT 'invalid stored value in salestax_taxgroupmember' AS problem FROM salestax_taxgroupmember WHERE
 NOT fb_valid_uuid(group_id) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(member_id)
UNION ALL
SELECT 'invalid stored value in salestax_taxitem' AS problem FROM salestax_taxitem WHERE
 NOT fb_valid_uuid(agency_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(description) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_text(kind) OR
 NOT fb_valid_text(name)
UNION ALL
SELECT 'invalid stored value in transfers_transfer' AS problem FROM transfers_transfer WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(company_snapshot) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_text(creation_digest) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(from_account_id) OR
 NOT fb_valid_text(from_snapshot) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(identity_id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(memo) OR
 NOT fb_valid_text(review_digest) OR
 NOT fb_valid_uuid(to_account_id) OR
 NOT fb_valid_text(to_snapshot)
UNION ALL
SELECT 'invalid stored value in transfers_transfervoid' AS problem FROM transfers_transfervoid WHERE
 NOT fb_valid_uuid(actor_id) OR
 NOT fb_valid_text(actor_name) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_date(date) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(journal_id) OR
 NOT fb_valid_text(reason) OR
 NOT fb_valid_uuid(transfer_id)
UNION ALL SELECT 'invalid stored value in company_document_details' AS problem FROM company_document_details WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(data) OR
 NOT fb_valid_text(logo_mime) OR (fb_json_unique_keys(data) AND NOT fb_company_document_valid(data))
UNION ALL
SELECT 'invalid stored value in fb_numbering' AS problem FROM fb_numbering WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(kind) OR
 NOT fb_valid_text(scope) OR
 NOT fb_valid_text(start) OR
 NOT fb_valid_uuid(audit_id)
UNION ALL
SELECT 'invalid stored value in desktop_user' AS problem FROM desktop_user WHERE
 NOT fb_valid_uuid(user_id) OR
 NOT fb_valid_text(role) OR
 NOT fb_valid_text(password_hash)
UNION ALL
SELECT 'invalid stored value in desktop_mfa_totp' FROM desktop_mfa_totp WHERE NOT fb_valid_uuid(user_id)
UNION ALL
SELECT 'invalid stored value in desktop_mfa_recovery' FROM desktop_mfa_recovery WHERE NOT fb_valid_uuid(user_id)
UNION ALL
SELECT 'invalid stored value in invoice_preferences' AS problem FROM invoice_preferences WHERE
 NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(data)
UNION ALL
SELECT 'invalid stored value in invoice_email_connection' AS problem FROM invoice_email_connection WHERE
 NOT fb_valid_text(client_id) OR
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_text(from_address) OR
 NOT fb_valid_text(status) OR
 NOT fb_valid_timestamp(updated_at) OR
 NOT fb_valid_uuid(updated_by_id)
UNION ALL
SELECT 'invalid stored value in invoice_email_delivery' AS problem FROM invoice_email_delivery WHERE
 NOT fb_valid_uuid(company_id) OR
 NOT fb_valid_timestamp(completed_at) OR
 NOT fb_valid_timestamp(created_at) OR
 NOT fb_valid_uuid(created_by_id) OR
 NOT fb_valid_text(document_hash) OR
 NOT fb_valid_text(error_code) OR
 NOT fb_valid_uuid(id) OR
 NOT fb_valid_uuid(invoice_id) OR
 NOT fb_valid_uuid(invoice_revision_id) OR
 NOT fb_valid_text(message_key) OR
 NOT fb_valid_text(mode) OR
 NOT fb_valid_text(pdf_sha256) OR
 NOT fb_valid_text(provider_draft) OR
 NOT fb_valid_text(provider_message) OR
 NOT fb_valid_text(state) OR
 NOT fb_valid_text(subject) OR
 NOT fb_valid_text(to_address)
UNION ALL
SELECT 'invalid stored value in invoice_stripe_connection' AS problem FROM invoice_stripe_connection WHERE NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(account_key) OR NOT fb_valid_text(mode) OR NOT fb_valid_text(key_last4) OR NOT fb_valid_text(status) OR NOT fb_valid_timestamp(created_at) OR NOT fb_valid_timestamp(updated_at) OR NOT fb_valid_uuid(updated_by_id)
UNION ALL
SELECT 'invalid stored value in invoice_stripe_link' AS problem FROM invoice_stripe_link WHERE NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_uuid(connection_id) OR NOT fb_valid_uuid(invoice_id) OR NOT fb_valid_uuid(invoice_revision_id) OR NOT fb_valid_uuid(created_by_id) OR NOT fb_valid_text(description) OR NOT fb_valid_text(currency) OR NOT fb_valid_text(state) OR NOT fb_valid_text(provider_key) OR NOT fb_valid_text(payment_url) OR NOT fb_valid_text(cursor) OR NOT fb_valid_timestamp(created_at) OR NOT fb_valid_timestamp(checked_at)
UNION ALL
SELECT 'invalid stored value in invoice_stripe_payment' AS problem FROM invoice_stripe_payment WHERE NOT fb_valid_uuid(id) OR NOT fb_valid_uuid(company_id) OR NOT fb_valid_uuid(connection_id) OR NOT fb_valid_uuid(link_id) OR NOT fb_valid_text(account_key) OR NOT fb_valid_text(mode) OR NOT fb_valid_text(session_key) OR NOT fb_valid_text(intent_key) OR NOT fb_valid_text(currency) OR NOT fb_valid_text(provider_state) OR NOT fb_valid_timestamp(observed_at) OR NOT fb_valid_uuid(receipt_id)
UNION ALL
SELECT 'invalid stored value in bankfeeds_rule_policy' AS problem FROM bankfeeds_rule_policy WHERE NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(data) OR NOT fb_valid_uuid(rule_id) OR NOT fb_valid_text(source_hash) OR NOT fb_valid_text(source_key)
UNION ALL
SELECT 'invalid stored value in bankfeeds_rule_review' AS problem FROM bankfeeds_rule_review WHERE NOT fb_valid_uuid(item_id) OR NOT fb_valid_text(reason)
UNION ALL
SELECT 'invalid stored value in bankfeeds_auto_queue' AS problem FROM bankfeeds_auto_queue WHERE NOT fb_valid_text(fingerprint) OR NOT fb_valid_uuid(item_id) OR NOT fb_valid_uuid(rule_id)
UNION ALL SELECT 'sales_importinvoice:'||invoice_id FROM sales_importinvoice WHERE NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(source) OR NOT fb_valid_uuid(source_id) OR NOT fb_valid_text(source_number) OR NOT fb_valid_uuid(invoice_id) OR NOT fb_valid_uuid(revision_id) OR NOT fb_valid_text(digest) OR NOT fb_valid_text(data) OR NOT fb_valid_uuid(actor_id) OR NOT fb_valid_uuid(audit_id)
UNION ALL SELECT 'sales_importpayment:'||invoice_id FROM sales_importpayment WHERE NOT fb_valid_uuid(company_id) OR NOT fb_valid_text(source) OR NOT fb_valid_uuid(source_id) OR NOT fb_valid_uuid(invoice_id) OR NOT fb_valid_uuid(receipt_id) OR NOT fb_valid_text(external_key)
UNION ALL SELECT 'sales_importseal:'||invoice_id FROM sales_importseal WHERE NOT fb_valid_uuid(invoice_id);

DROP VIEW fb_json_invalid;
CREATE VIEW fb_json_invalid AS
SELECT 'banking_deposit.data:'||id AS problem FROM banking_deposit WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'banking_depositreversal.data:'||id AS problem FROM banking_depositreversal WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'banking_reconciliation.data:'||id AS problem FROM banking_reconciliation WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'checks_checkline.account_snapshot:'||id AS problem FROM checks_checkline WHERE NOT fb_json_unique_keys(account_snapshot)
UNION ALL SELECT 'checks_checkline.item_snapshot:'||id AS problem FROM checks_checkline WHERE NOT fb_json_unique_keys(item_snapshot)
UNION ALL SELECT 'checks_checkrevision.bank_snapshot:'||id AS problem FROM checks_checkrevision WHERE NOT fb_json_unique_keys(bank_snapshot)
UNION ALL SELECT 'checks_checkrevision.company_snapshot:'||id AS problem FROM checks_checkrevision WHERE NOT fb_json_unique_keys(company_snapshot)
UNION ALL SELECT 'checks_checkrevision.payee_snapshot:'||id AS problem FROM checks_checkrevision WHERE NOT fb_json_unique_keys(payee_snapshot)
UNION ALL SELECT 'credits_creditapplication.data:'||id AS problem FROM credits_creditapplication WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'credits_creditapplicationline.data:'||id AS problem FROM credits_creditapplicationline WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'credits_creditmemo.data:'||id AS problem FROM credits_creditmemo WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'credits_creditmemoline.data:'||id AS problem FROM credits_creditmemoline WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'credits_creditmemovoid.data:'||id AS problem FROM credits_creditmemovoid WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'credits_creditrefund.data:'||id AS problem FROM credits_creditrefund WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'credits_creditrefundvoid.data:'||id AS problem FROM credits_creditrefundvoid WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'credits_creditunapplication.data:'||id AS problem FROM credits_creditunapplication WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'ledger_auditevent.detail:'||id AS problem FROM ledger_auditevent WHERE NOT fb_json_unique_keys(detail)
UNION ALL SELECT 'payables_billline.account_snapshot:'||id AS problem FROM payables_billline WHERE NOT fb_json_unique_keys(account_snapshot)
UNION ALL SELECT 'payables_billline.item_snapshot:'||id AS problem FROM payables_billline WHERE NOT fb_json_unique_keys(item_snapshot)
UNION ALL SELECT 'payables_billpayment.company_snapshot:'||id AS problem FROM payables_billpayment WHERE NOT fb_json_unique_keys(company_snapshot)
UNION ALL SELECT 'payables_billpayment.vendor_snapshot:'||id AS problem FROM payables_billpayment WHERE NOT fb_json_unique_keys(vendor_snapshot)
UNION ALL SELECT 'payables_billrevision.company_snapshot:'||id AS problem FROM payables_billrevision WHERE NOT fb_json_unique_keys(company_snapshot)
UNION ALL SELECT 'payables_billrevision.vendor_snapshot:'||id AS problem FROM payables_billrevision WHERE NOT fb_json_unique_keys(vendor_snapshot)
UNION ALL SELECT 'prepayments_depositapplication.data:'||id AS problem FROM prepayments_depositapplication WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'prepayments_depositrefund.data:'||id AS problem FROM prepayments_depositrefund WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'prepayments_depositrefundvoid.data:'||id AS problem FROM prepayments_depositrefundvoid WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'prepayments_depositunapplication.data:'||id AS problem FROM prepayments_depositunapplication WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'sales_invoiceline.data:'||id AS problem FROM sales_invoiceline WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'sales_invoicerevision.data:'||id AS problem FROM sales_invoicerevision WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'sales_receipt.data:'||id AS problem FROM sales_receipt WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'sales_receiptreversal.data:'||id AS problem FROM sales_receiptreversal WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'sales_salesreceipt.data:'||id AS problem FROM sales_salesreceipt WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'sales_salesreceiptline.data:'||id AS problem FROM sales_salesreceiptline WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'transfers_transfer.company_snapshot:'||id AS problem FROM transfers_transfer WHERE NOT fb_json_unique_keys(company_snapshot)
UNION ALL SELECT 'transfers_transfer.from_snapshot:'||id AS problem FROM transfers_transfer WHERE NOT fb_json_unique_keys(from_snapshot)
UNION ALL SELECT 'transfers_transfer.to_snapshot:'||id AS problem FROM transfers_transfer WHERE NOT fb_json_unique_keys(to_snapshot)
UNION ALL SELECT 'sales_estimateline.data:'||id AS problem FROM sales_estimateline WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'sales_estimaterevision.data:'||id AS problem FROM sales_estimaterevision WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'assistant_connection.model_choices:'||id FROM assistant_connection WHERE NOT fb_json_unique_keys(model_choices)
UNION ALL SELECT 'assistant_reading.result:'||id FROM assistant_reading WHERE NOT fb_json_unique_keys(result)
UNION ALL SELECT 'company_document_details.data:'||company_id FROM company_document_details WHERE NOT fb_json_unique_keys(data) OR NOT fb_company_document_valid(data)
UNION ALL SELECT 'invoice_preferences.data:'||company_id FROM invoice_preferences WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'bankfeeds_rule_policy.data:'||rule_id FROM bankfeeds_rule_policy WHERE NOT fb_json_unique_keys(data)
UNION ALL SELECT 'sales_importinvoice.data:'||invoice_id FROM sales_importinvoice WHERE NOT fb_json_unique_keys(data);

CREATE TRIGGER fb_json_sales_importinvoice_insert BEFORE INSERT ON sales_importinvoice WHEN NOT fb_json_unique_keys(NEW.data) BEGIN SELECT RAISE(ABORT,'duplicate_json: Stored JSON must have unique keys'); END;

CREATE TRIGGER fb_json_sales_importinvoice_update BEFORE UPDATE ON sales_importinvoice WHEN NOT fb_json_unique_keys(NEW.data) BEGIN SELECT RAISE(ABORT,'duplicate_json: Stored JSON must have unique keys'); END;

CREATE TRIGGER fb_document_import_number BEFORE INSERT ON documents_documentidentity
WHEN NEW.kind='INV' AND EXISTS(SELECT 1 FROM sales_importinvoice WHERE company_id=NEW.company_id AND source_number=NEW.number)
BEGIN SELECT RAISE(ABORT,'invoice_import_number: This number belongs to an imported invoice. Choose a different invoice numbering sequence.'); END;
CREATE VIEW sales_import_completion_violations AS
 SELECT h.invoice_id AS problem FROM sales_importinvoice h
 WHERE NOT EXISTS(SELECT 1 FROM sales_importseal s WHERE s.invoice_id=h.invoice_id)
 OR json_array_length(h.data,'$.source.payments') IS NOT (SELECT count(*) FROM sales_importpayment p WHERE p.invoice_id=h.invoice_id)
 OR json_array_length(h.data,'$.payments') IS NOT (SELECT count(*) FROM sales_importpayment p WHERE p.invoice_id=h.invoice_id)
 OR fb_source_money(json_quote(json_extract(h.data,'$.source.amount_paid'))) IS NOT (SELECT coalesce(sum(r.amount),0) FROM sales_importpayment p JOIN sales_receipt r ON r.id=p.receipt_id WHERE p.invoice_id=h.invoice_id);
