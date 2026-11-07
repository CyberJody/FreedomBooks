-- Extend shared value validation for opaque credential envelope scope metadata.
CREATE TRIGGER fb_valid_plugin_credential_insert BEFORE INSERT ON plugin_credential BEGIN
 SELECT RAISE(ABORT,'fb_valid: Invalid stored text, identifier, date or timestamp') WHERE
 NOT fb_valid_uuid(NEW.company_id) OR
 NOT fb_valid_text(NEW.plugin_id) OR
 NOT fb_valid_uuid(NEW.connection_id);
END;
CREATE TRIGGER fb_valid_plugin_credential_update BEFORE UPDATE ON plugin_credential BEGIN
 SELECT RAISE(ABORT,'fb_valid: Invalid stored text, identifier, date or timestamp') WHERE
 NOT fb_valid_uuid(NEW.company_id) OR
 NOT fb_valid_text(NEW.plugin_id) OR
 NOT fb_valid_uuid(NEW.connection_id);
END;
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
 NOT fb_valid_uuid(transfer_id);
