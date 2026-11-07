-- Shared ambiguity boundary: all modules use the same recursive parser.
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
UNION ALL SELECT 'transfers_transfer.to_snapshot:'||id AS problem FROM transfers_transfer WHERE NOT fb_json_unique_keys(to_snapshot);
CREATE TRIGGER fb_json_banking_deposit_insert BEFORE INSERT ON banking_deposit BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_banking_deposit_update BEFORE UPDATE ON banking_deposit BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_banking_depositreversal_insert BEFORE INSERT ON banking_depositreversal BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_banking_depositreversal_update BEFORE UPDATE ON banking_depositreversal BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_banking_reconciliation_insert BEFORE INSERT ON banking_reconciliation BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_banking_reconciliation_update BEFORE UPDATE ON banking_reconciliation BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_checks_checkline_insert BEFORE INSERT ON checks_checkline BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.account_snapshot) OR NOT fb_json_unique_keys(NEW.item_snapshot); END;
CREATE TRIGGER fb_json_checks_checkline_update BEFORE UPDATE ON checks_checkline BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.account_snapshot) OR NOT fb_json_unique_keys(NEW.item_snapshot); END;
CREATE TRIGGER fb_json_checks_checkrevision_insert BEFORE INSERT ON checks_checkrevision BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.bank_snapshot) OR NOT fb_json_unique_keys(NEW.company_snapshot) OR NOT fb_json_unique_keys(NEW.payee_snapshot); END;
CREATE TRIGGER fb_json_checks_checkrevision_update BEFORE UPDATE ON checks_checkrevision BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.bank_snapshot) OR NOT fb_json_unique_keys(NEW.company_snapshot) OR NOT fb_json_unique_keys(NEW.payee_snapshot); END;
CREATE TRIGGER fb_json_credits_creditapplication_insert BEFORE INSERT ON credits_creditapplication BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_credits_creditapplication_update BEFORE UPDATE ON credits_creditapplication BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_credits_creditapplicationline_insert BEFORE INSERT ON credits_creditapplicationline BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_credits_creditapplicationline_update BEFORE UPDATE ON credits_creditapplicationline BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_credits_creditmemo_insert BEFORE INSERT ON credits_creditmemo BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_credits_creditmemo_update BEFORE UPDATE ON credits_creditmemo BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_credits_creditmemoline_insert BEFORE INSERT ON credits_creditmemoline BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_credits_creditmemoline_update BEFORE UPDATE ON credits_creditmemoline BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_credits_creditmemovoid_insert BEFORE INSERT ON credits_creditmemovoid BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_credits_creditmemovoid_update BEFORE UPDATE ON credits_creditmemovoid BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_credits_creditrefund_insert BEFORE INSERT ON credits_creditrefund BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_credits_creditrefund_update BEFORE UPDATE ON credits_creditrefund BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_credits_creditrefundvoid_insert BEFORE INSERT ON credits_creditrefundvoid BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_credits_creditrefundvoid_update BEFORE UPDATE ON credits_creditrefundvoid BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_credits_creditunapplication_insert BEFORE INSERT ON credits_creditunapplication BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_credits_creditunapplication_update BEFORE UPDATE ON credits_creditunapplication BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_ledger_auditevent_insert BEFORE INSERT ON ledger_auditevent BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.detail); END;
CREATE TRIGGER fb_json_ledger_auditevent_update BEFORE UPDATE ON ledger_auditevent BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.detail); END;
CREATE TRIGGER fb_json_payables_billline_insert BEFORE INSERT ON payables_billline BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.account_snapshot) OR NOT fb_json_unique_keys(NEW.item_snapshot); END;
CREATE TRIGGER fb_json_payables_billline_update BEFORE UPDATE ON payables_billline BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.account_snapshot) OR NOT fb_json_unique_keys(NEW.item_snapshot); END;
CREATE TRIGGER fb_json_payables_billpayment_insert BEFORE INSERT ON payables_billpayment BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.company_snapshot) OR NOT fb_json_unique_keys(NEW.vendor_snapshot); END;
CREATE TRIGGER fb_json_payables_billpayment_update BEFORE UPDATE ON payables_billpayment BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.company_snapshot) OR NOT fb_json_unique_keys(NEW.vendor_snapshot); END;
CREATE TRIGGER fb_json_payables_billrevision_insert BEFORE INSERT ON payables_billrevision BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.company_snapshot) OR NOT fb_json_unique_keys(NEW.vendor_snapshot); END;
CREATE TRIGGER fb_json_payables_billrevision_update BEFORE UPDATE ON payables_billrevision BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.company_snapshot) OR NOT fb_json_unique_keys(NEW.vendor_snapshot); END;
CREATE TRIGGER fb_json_prepayments_depositapplication_insert BEFORE INSERT ON prepayments_depositapplication BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_prepayments_depositapplication_update BEFORE UPDATE ON prepayments_depositapplication BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_prepayments_depositrefund_insert BEFORE INSERT ON prepayments_depositrefund BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_prepayments_depositrefund_update BEFORE UPDATE ON prepayments_depositrefund BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_prepayments_depositrefundvoid_insert BEFORE INSERT ON prepayments_depositrefundvoid BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_prepayments_depositrefundvoid_update BEFORE UPDATE ON prepayments_depositrefundvoid BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_prepayments_depositunapplication_insert BEFORE INSERT ON prepayments_depositunapplication BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_prepayments_depositunapplication_update BEFORE UPDATE ON prepayments_depositunapplication BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_sales_invoiceline_insert BEFORE INSERT ON sales_invoiceline BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_sales_invoiceline_update BEFORE UPDATE ON sales_invoiceline BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_sales_invoicerevision_insert BEFORE INSERT ON sales_invoicerevision BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_sales_invoicerevision_update BEFORE UPDATE ON sales_invoicerevision BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_sales_receipt_insert BEFORE INSERT ON sales_receipt BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_sales_receipt_update BEFORE UPDATE ON sales_receipt BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_sales_receiptreversal_insert BEFORE INSERT ON sales_receiptreversal BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_sales_receiptreversal_update BEFORE UPDATE ON sales_receiptreversal BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_sales_salesreceipt_insert BEFORE INSERT ON sales_salesreceipt BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_sales_salesreceipt_update BEFORE UPDATE ON sales_salesreceipt BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_sales_salesreceiptline_insert BEFORE INSERT ON sales_salesreceiptline BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_sales_salesreceiptline_update BEFORE UPDATE ON sales_salesreceiptline BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.data); END;
CREATE TRIGGER fb_json_transfers_transfer_insert BEFORE INSERT ON transfers_transfer BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.company_snapshot) OR NOT fb_json_unique_keys(NEW.from_snapshot) OR NOT fb_json_unique_keys(NEW.to_snapshot); END;
CREATE TRIGGER fb_json_transfers_transfer_update BEFORE UPDATE ON transfers_transfer BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE NOT fb_json_unique_keys(NEW.company_snapshot) OR NOT fb_json_unique_keys(NEW.from_snapshot) OR NOT fb_json_unique_keys(NEW.to_snapshot); END;
CREATE TRIGGER fb_json_fb_seal_seal BEFORE INSERT ON fb_seal BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE EXISTS(SELECT 1 FROM fb_json_invalid); END;
CREATE TRIGGER fb_json_sales_seal_seal BEFORE INSERT ON sales_seal BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE EXISTS(SELECT 1 FROM fb_json_invalid); END;
CREATE TRIGGER fb_json_credits_seal_seal BEFORE INSERT ON credits_seal BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE EXISTS(SELECT 1 FROM fb_json_invalid); END;
CREATE TRIGGER fb_json_prepayments_seal_seal BEFORE INSERT ON prepayments_seal BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE EXISTS(SELECT 1 FROM fb_json_invalid); END;
CREATE TRIGGER fb_json_banking_seal_seal BEFORE INSERT ON banking_seal BEGIN SELECT RAISE(ABORT,'duplicate_json: Invalid or duplicate JSON keys') WHERE EXISTS(SELECT 1 FROM fb_json_invalid); END;
