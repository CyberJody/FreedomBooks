//! fb-db: the encrypted book file. The only crate that knows the book is SQLCipher.
//!
//! * `Book::create` / `Book::open` key the file with a raw 32-byte key (`PRAGMA key = "x'<hex>'"`,
//!   no PBKDF2 because the key is random), set the connection rules every connection needs
//!   (`foreign_keys`, `recursive_triggers`, WAL, busy timeout) and verify they took, run migrations,
//!   then run the open-time checks (`quick_check`, `foreign_key_check`, versioned schema-definition hashes and
//!   the ledger violation queries). A book that fails a check is refused.
//! * `Book::write` runs a closure inside `BEGIN IMMEDIATE`: the write lock is taken before the first
//!   read, which is what the Python app's company `FOR NO KEY UPDATE` + row locks gave
//!   (SAFETY-RULES-PORT section 4). Never `BEGIN DEFERRED` for a write.
//! * Guard errors are `RAISE(ABORT, '<name>: <message>')`; `constraint_of` recovers the name.

mod backup;
pub mod assistant;
pub mod collation;
pub mod dates;
mod sales;
mod legacy;
#[cfg(test)]
mod release_integration_tests;
#[cfg(feature = "test-support")]
pub mod test_support;
mod credits;
pub use credits::check_number_char_allowed as credit_check_number_char_allowed;
pub use credits::fields as credit_fields_valid;

use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use std::fmt;
use std::sync::{Arc, OnceLock, atomic::{AtomicU8, Ordering}};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use std::collections::BTreeMap;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;

pub use rusqlite;
mod payables_functions;
pub mod validators;
pub mod check_numbers;
pub mod checks_snapshots;
pub mod prepayments;
mod banking_functions;
pub mod banking_evidence;
pub mod json_evidence;
pub mod company_document_data;
pub mod bankfeeds_text;
pub mod credential_mask;
pub mod plugin_credentials;
pub use json_evidence::{parse_unique_json, parse_unique_json_as};

#[cfg(test)]
mod core_int_tests;
#[cfg(test)]
mod json_column_tests;
#[cfg(test)]
mod value_guard_tests;
#[cfg(test)]
mod reviewed_inventory_tests;
#[cfg(test)]
mod desktop_upgrade_tests;
#[cfg(test)]
mod open_tests;

/// Numbered migrations; `PRAGMA user_version` records how many are applied.
pub const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_ledger", include_str!("../sql/0001_ledger.sql")),
    ("0002_review_hardening", include_str!("../sql/0002_review_hardening.sql")),
    ("0003_callback_and_collision_hardening", include_str!("../sql/0003_callback_and_collision_hardening.sql")),
    ("0004_directory_documents_tax", include_str!("../sql/0004_directory_documents_tax.sql")),
    ("0005_source_journal_hooks", include_str!("../sql/0005_source_journal_hooks.sql")),
    ("0006_l2a_hardening", include_str!("../sql/0006_l2a_hardening.sql")),
    ("0007_permanent_identities", include_str!("../sql/0007_permanent_identities.sql")),
    ("0008_l2a_permanent_identities", include_str!("../sql/0008_l2a_permanent_identities.sql")),
    ("0009_l2a_review_fixes", include_str!("../sql/0009_l2a_review_fixes.sql")),
    ("0010_sales", include_str!("../sql/0010_sales.sql")),
    ("0011_sales_references", include_str!("../sql/0011_sales_references.sql")),
    ("0012_sales_undeposited", include_str!("../sql/0012_sales_undeposited.sql")),
    ("0013_sales_owned_seals", include_str!("../sql/0013_sales_owned_seals.sql")),
    ("0014_sales_hardening", include_str!("../sql/0014_sales_hardening.sql")),
    ("0015_sales_core_adoption", include_str!("../sql/0015_sales_core_adoption.sql")),
    ("0016_sales_review_fixes", include_str!("../sql/0016_sales_review_fixes.sql")),
    ("0020_payables", include_str!("../sql/0020_payables.sql")),
    ("0021_payables_hardening", include_str!("../sql/0021_payables_hardening.sql")),
    ("0022_payables_source_hardening", include_str!("../sql/0022_payables_source_hardening.sql")),
    ("0023_payables_parent_unique_keys", include_str!("../sql/0023_payables_parent_unique_keys.sql")),
    ("0024_payables_line_calculation", include_str!("../sql/0024_payables_line_calculation.sql")),
    ("0025_payables_core_hardening", include_str!("../sql/0025_payables_core_hardening.sql")),
    ("0026_payables_core_adoption", include_str!("../sql/0026_payables_core_adoption.sql")),
    ("0027_payables_snapshots_and_origins", include_str!("../sql/0027_payables_snapshots_and_origins.sql")),
    ("0030_checks_transfers", include_str!("../sql/0030_checks_transfers.sql")),
    ("0031_checks_parent_eligibility", include_str!("../sql/0031_checks_parent_eligibility.sql")),
    ("0032_checks_root_parent_ordering", include_str!("../sql/0032_checks_root_parent_ordering.sql")),
    ("0033_checks_review_fixes", include_str!("../sql/0033_checks_review_fixes.sql")),
    ("0050_banking", include_str!("../sql/0050_banking.sql")),
    ("0051_banking_integrity", include_str!("../sql/0051_banking_integrity.sql")),
    ("0052_banking_completion", include_str!("../sql/0052_banking_completion.sql")),
    ("0053_banking_actors", include_str!("../sql/0053_banking_actors.sql")),
    ("0054_banking_review", include_str!("../sql/0054_banking_review.sql")),
    ("0055_banking_unique_json", include_str!("../sql/0055_banking_unique_json.sql")),
    ("0060_credits", include_str!("../sql/0060_credits.sql")),
    ("0061_credits_review_fixes", include_str!("../sql/0061_credits_review_fixes.sql")),
    ("0070_prepayments", include_str!("../sql/0070_prepayments.sql")),
    ("0071_prepayment_number_eligibility", include_str!("../sql/0071_prepayment_number_eligibility.sql")),
    ("0072_prepayment_review_fixes", include_str!("../sql/0072_prepayment_review_fixes.sql")),
    ("0073_shared_check_numbers", include_str!("../sql/0073_shared_check_numbers.sql")),
    ("0074_shared_settlements", include_str!("../sql/0074_shared_settlements.sql")),
    ("0075_restore_sales_receipt_destination", include_str!("../sql/0075_restore_sales_receipt_destination.sql")),
    ("0076_credit_check_numbers", include_str!("../sql/0076_credit_check_numbers.sql")),
    ("0077_credit_settlements", include_str!("../sql/0077_credit_settlements.sql")),
    ("0078_shared_unique_json", include_str!("../sql/0078_shared_unique_json.sql")),
    ("0079_restore_sales_receipt_snapshot", include_str!("../sql/0079_restore_sales_receipt_snapshot.sql")),
    ("0100_assistant", include_str!("../sql/0100_assistant.sql")),
    ("0101_assistant_unique_json", include_str!("../sql/0101_assistant_unique_json.sql")),
    ("0102_assistant_model_selection", include_str!("../sql/0102_assistant_model_selection.sql")),
    ("0103_assistant_completion", include_str!("../sql/0103_assistant_completion.sql")),
    ("0110_attachments", include_str!("../sql/0110_attachments.sql")),
    ("0111_attachment_text", include_str!("../sql/0111_attachment_text.sql")),
    ("0120_owner", include_str!("../sql/0120_owner.sql")),
    ("0121_owner_eligibility", include_str!("../sql/0121_owner_eligibility.sql")),
    ("0130_shared_validators", include_str!("../sql/0130_shared_validators.sql")),
    ("0131_estimates", include_str!("../sql/0131_estimates.sql")),
    ("0132_estimate_validators", include_str!("../sql/0132_estimate_validators.sql")),
    ("0133_bankfeeds", include_str!("../sql/0133_bankfeeds.sql")),
    ("0134_bankfeeds_hardening", include_str!("../sql/0134_bankfeeds_hardening.sql")),
    ("0135_bankfeeds_activation_eligibility", include_str!("../sql/0135_bankfeeds_activation_eligibility.sql")),
    ("0136_bankfeeds_identity", include_str!("../sql/0136_bankfeeds_identity.sql")),
    ("0137_bankfeeds_reconnect", include_str!("../sql/0137_bankfeeds_reconnect.sql")),
    ("0140_deposit_estimates", include_str!("../sql/0140_deposit_estimates.sql")),
    ("0160_plugin_credentials", include_str!("../sql/0160_plugin_credentials.sql")),
    ("0161_credential_validators", include_str!("../sql/0161_credential_validators.sql")),
    ("0162_numbering", include_str!("../sql/0162_numbering.sql")),
    ("0180_company_documents", include_str!("../sql/0180_company_documents.sql")),
    ("0181_company_document_guards", include_str!("../sql/0181_company_document_guards.sql")),
    ("0182_company_document_data", include_str!("../sql/0182_company_document_data.sql")),
    ("0183_company_document_json_ownership", include_str!("../sql/0183_company_document_json_ownership.sql")),
    ("0184_release_guard_union", include_str!("../sql/0184_release_guard_union.sql")),
    ("0185_desktop_users", include_str!("../sql/0185_desktop_users.sql")),
    ("0186_desktop_user_value_guards", include_str!("../sql/0186_desktop_user_value_guards.sql")),
    ("0187_desktop_mfa", include_str!("../sql/0187_desktop_mfa.sql")),
];

/// Every trigger the schema must carry. A book missing one was altered outside the core and is refused.
pub const REQUIRED_TRIGGERS: &[&str] = &[
    "company_documents_data_insert",
    "company_documents_data_update",
    "fb_json_company_document_details_insert",
    "fb_json_company_document_details_update",
    "company_documents_identity_insert",
    "company_documents_identity_update",
    "company_documents_identity_delete",
    "fb_numbering_insert", "fb_numbering_update", "fb_numbering_delete",
    "fb_numbering_bank_identity", "fb_numbering_bank_delete",
    "pp_customerdeposit_estimate_scope",
    "fb_estimate_due_insert",
    "fb_json_sales_estimateline_insert",
    "fb_json_sales_estimateline_update",
    "fb_json_sales_estimaterevision_insert",
    "fb_json_sales_estimaterevision_update",
    "fb_json_estimate_seal",
    "fb_estimate_key_sales_estimate",
    "fb_estimate_key_sales_estimaterevision",
    "fb_estimate_key_sales_estimateline",
    "fb_estimate_key_sales_estimateconversion",
    "fb_estimate_root_insert",
    "fb_estimate_root_update",
    "fb_estimate_root_due_insert",
    "fb_estimate_root_due_update",
    "fb_estimate_revision_insert",
    "fb_estimate_revision_due",
    "fb_estimate_line_insert",
    "fb_estimate_seal",
    "fb_estimate_conversion_insert",
    "fb_sales_estimate_delete",
    "fb_sales_estimaterevision_update",
    "fb_sales_estimaterevision_delete",
    "fb_sales_estimateline_update",
    "fb_sales_estimateline_delete",
    "fb_sales_estimateconversion_update",
    "fb_sales_estimateconversion_delete",
    "fb_estimate_seal_update",
    "fb_estimate_seal_delete",
    "fb_estimate_seal_due_update",
    "fb_estimate_seal_due_delete",
    "fb_estimate_parent_directory_customer_delete",
    "fb_estimate_parent_directory_customer_update",
    "fb_estimate_parent_directory_item_delete",
    "fb_estimate_parent_directory_item_update",
    "fb_estimate_parent_ledger_account_delete",
    "fb_estimate_parent_ledger_account_update",
    "fb_estimate_parent_fb_user_delete",
    "fb_estimate_parent_fb_user_update",
    "fb_sales_receipt_snapshot",
    "desktop_user_identity", "desktop_user_kept", "desktop_user_password_kept",
    "desktop_user_last_admin", "desktop_user_last_active", "desktop_user_last_member",
    "fb_owner_insert", "fb_owner_update", "fb_owner_delete",
    "fb_owner_user_insert", "fb_owner_user_update", "fb_owner_user_delete",
    "attachment_text_insert",
    "attachment_text_update",
    "attachment_guard_insert",
    "attachment_primary_key_permanent",
    "attachment_guard_update",
    "attachment_guard_delete",
    "attachment_added_due",
    "attachment_removed_due",
    "attachment_audit_seal",
    "attachment_snapshot_insert",
    "attachment_snapshot_remove",
    "attachment_document_id_kept",
    "attachment_journal_id_kept",
    "attachment_company_id_kept",
    "attachment_operator_kept",

    "fb_assistant_reading_complete_pending",
    "fb_assistant_model_insert", "fb_assistant_model_update",
    "fb_json_assistant_connection_insert", "fb_json_assistant_connection_update", "fb_json_assistant_reading_insert", "fb_json_assistant_reading_update",
    "fb_assistant_connection_insert", "fb_assistant_connection_update", "fb_assistant_connection_delete",
    "fb_assistant_reading_insert", "fb_assistant_reading_update", "fb_assistant_reading_delete",
    "fb_assistant_user_delete", "fb_assistant_company_delete", "fb_assistant_bill_delete",
    "fb_bankfeeds_connection_insert",
    "fb_bankfeeds_connection_update",
    "fb_bankfeeds_connection_id",
    "fb_bankfeeds_connection_delete",
    "fb_bankfeeds_account_insert",
    "fb_bankfeeds_account_update",
    "fb_bankfeeds_account_id",
    "fb_bankfeeds_account_delete",
    "fb_bankfeeds_transaction_insert",
    "fb_bankfeeds_transaction_count",
    "fb_bankfeeds_counter_update",
    "fb_bankfeeds_counter_insert",
    "fb_bankfeeds_counter_delete",
    "fb_bankfeeds_transaction_update",
    "fb_bankfeeds_transaction_id",
    "fb_bankfeeds_transaction_delete",
    "fb_bankfeeds_rule_insert",
    "fb_bankfeeds_rule_update",
    "fb_bankfeeds_rule_id",
    "fb_bankfeeds_rule_delete",
    "fb_bankfeeds_account_kept",
    "fb_bankfeeds_account_moved",
    "fb_bankfeeds_user_kept",
    "fb_bankfeeds_company_kept",
    "fb_bankfeeds_journal_kept",
    "fb_bankfeeds_line_kept",
    "fb_bankfeeds_connection_text_insert",
    "fb_bankfeeds_connection_text_update",
    "fb_bankfeeds_account_text_insert",
    "fb_bankfeeds_account_text_update",
    "fb_bankfeeds_transaction_text_insert",
    "fb_bankfeeds_transaction_text_update",
    "fb_bankfeeds_rule_text_insert",
    "fb_bankfeeds_rule_text_update",
    "fb_bankfeeds_retired_insert",
    "fb_bankfeeds_retired_update",
    "fb_bankfeeds_retired_delete",
    "fb_bankfeeds_transaction_withdrawn",
    "fb_bankfeeds_transaction_identity",
    "fb_bankfeeds_transaction_eligible",
    "fb_bankfeeds_account_activation",
    "fb_bankfeeds_identity_secret_insert",
    "fb_bankfeeds_identity_secret_update",
    "fb_bankfeeds_identity_secret_delete",
    "fb_json_banking_deposit_insert",
    "fb_json_banking_deposit_update",
    "fb_json_banking_depositreversal_insert",
    "fb_json_banking_depositreversal_update",
    "fb_json_banking_reconciliation_insert",
    "fb_json_banking_reconciliation_update",
    "fb_json_checks_checkline_insert",
    "fb_json_checks_checkline_update",
    "fb_json_checks_checkrevision_insert",
    "fb_json_checks_checkrevision_update",
    "fb_json_credits_creditapplication_insert",
    "fb_json_credits_creditapplication_update",
    "fb_json_credits_creditapplicationline_insert",
    "fb_json_credits_creditapplicationline_update",
    "fb_json_credits_creditmemo_insert",
    "fb_json_credits_creditmemo_update",
    "fb_json_credits_creditmemoline_insert",
    "fb_json_credits_creditmemoline_update",
    "fb_json_credits_creditmemovoid_insert",
    "fb_json_credits_creditmemovoid_update",
    "fb_json_credits_creditrefund_insert",
    "fb_json_credits_creditrefund_update",
    "fb_json_credits_creditrefundvoid_insert",
    "fb_json_credits_creditrefundvoid_update",
    "fb_json_credits_creditunapplication_insert",
    "fb_json_credits_creditunapplication_update",
    "fb_json_ledger_auditevent_insert",
    "fb_json_ledger_auditevent_update",
    "fb_json_payables_billline_insert",
    "fb_json_payables_billline_update",
    "fb_json_payables_billpayment_insert",
    "fb_json_payables_billpayment_update",
    "fb_json_payables_billrevision_insert",
    "fb_json_payables_billrevision_update",
    "fb_json_prepayments_depositapplication_insert",
    "fb_json_prepayments_depositapplication_update",
    "fb_json_prepayments_depositrefund_insert",
    "fb_json_prepayments_depositrefund_update",
    "fb_json_prepayments_depositrefundvoid_insert",
    "fb_json_prepayments_depositrefundvoid_update",
    "fb_json_prepayments_depositunapplication_insert",
    "fb_json_prepayments_depositunapplication_update",
    "fb_json_sales_invoiceline_insert",
    "fb_json_sales_invoiceline_update",
    "fb_json_sales_invoicerevision_insert",
    "fb_json_sales_invoicerevision_update",
    "fb_json_sales_receipt_insert",
    "fb_json_sales_receipt_update",
    "fb_json_sales_receiptreversal_insert",
    "fb_json_sales_receiptreversal_update",
    "fb_json_sales_salesreceipt_insert",
    "fb_json_sales_salesreceipt_update",
    "fb_json_sales_salesreceiptline_insert",
    "fb_json_sales_salesreceiptline_update",
    "fb_json_transfers_transfer_insert",
    "fb_json_transfers_transfer_update",
    "fb_json_fb_seal_seal",
    "fb_json_sales_seal_seal",
    "fb_json_credits_seal_seal",
    "fb_json_prepayments_seal_seal",
    "fb_json_banking_seal_seal",

    "fb_number_credits_refund_insert",
    "fb_sales_receipt_destination",
    "fb_invoice_shared_settlements",
    "fb_number_checks_check_insert",
    "fb_number_checks_check_update",
    "fb_number_payables_billpayment_insert",

    "fb_checks_line_origin_insert",
    "fb_checks_revision_snapshot_insert",
    "fb_checks_line_snapshot_insert",
    "fb_checks_transfer_snapshot_insert",
    "fb_checks_number_insert",
    "fb_checks_number_update",
    "fb_checks_evidence_seal",
    "fb_checks_transfer_evidence_seal",

    "fb_checks_check_existing_parent",
    "fb_checks_check_update_existing_parent",
    "fb_checks_checkrevision_existing_parent",
    "fb_transfers_transfer_existing_parent",
    "fb_transfers_transfervoid_existing_parent",

    "fb_check_insert",
    "fb_check_update",
    "fb_check_due_insert",
    "fb_check_due_update",
    "fb_check_revision_insert",
    "fb_check_line_insert",
    "fb_check_seal",
    "fb_check_payment_insert",
    "fb_check_payment_update",
    "fb_checks_payment_collision",
    "fb_transfer_insert",
    "fb_transfer_void_insert",
    "fb_checks_journal_due",
    "fb_transfer_journal_due",
    "fb_checks_journal_seal",
    "fb_transfer_journal_seal",
    "fb_checks_late_reference",
    "fb_checks_check_primary_key",
    "fb_checks_check_delete",
    "fb_checks_checkrevision_primary_key",
    "fb_checks_checkrevision_update",
    "fb_checks_checkrevision_delete",
    "fb_checks_checkline_primary_key",
    "fb_checks_checkline_update",
    "fb_checks_checkline_delete",
    "fb_transfers_transfer_primary_key",
    "fb_transfers_transfer_update",
    "fb_transfers_transfer_delete",
    "fb_transfers_transfervoid_primary_key",
    "fb_transfers_transfervoid_update",
    "fb_transfers_transfervoid_delete",
    "fb_checks_ledger_account_kept",
    "fb_checks_ledger_account_insert",
    "fb_checks_ledger_account_update",
    "fb_checks_directory_vendor_kept",
    "fb_checks_directory_vendor_insert",
    "fb_checks_directory_vendor_update",
    "fb_checks_directory_item_kept",
    "fb_checks_directory_item_insert",
    "fb_checks_directory_item_update",
    "fb_checks_documents_documentidentity_kept",
    "fb_checks_documents_documentidentity_insert",
    "fb_checks_documents_documentidentity_update",
    "fb_checks_ledger_journal_kept",
    "fb_checks_ledger_journal_insert",
    "fb_checks_ledger_journal_update",
    "fb_checks_fb_user_kept",
    "fb_checks_fb_user_insert",
    "fb_checks_fb_user_update",
    "pp_customerdeposit_collision",
    "pp_customerdeposit_update",
    "pp_customerdeposit_delete",
    "pp_depositapplication_collision",
    "pp_depositapplication_update",
    "pp_depositapplication_delete",
    "pp_depositrefund_collision",
    "pp_depositrefund_update",
    "pp_depositrefund_delete",
    "pp_depositunapplication_collision",
    "pp_depositunapplication_update",
    "pp_depositunapplication_delete",
    "pp_depositrefundvoid_collision",
    "pp_depositrefundvoid_update",
    "pp_depositrefundvoid_delete",
    "pp_prepayments_seal_collision",
    "pp_prepayments_seal_update",
    "pp_prepayments_seal_delete",
    "pp_prepayments_due_collision",
    "pp_prepayments_due_update",
    "pp_prepayments_due_delete",
    "pp_detail",
    "pp_receipt_complete",
    "pp_application_scope",
    "pp_refund_scope",
    "pp_payment_number",
    "pp_allocation_bound",
    "pp_deposit_reversal",
    "pp_invoice_void",
    "pp_depositunapplication_link",
    "pp_depositrefundvoid_link",
    "pp_depositapplication_fields",
    "pp_depositrefund_fields",
    "pp_journal_due",
    "pp_journal_seal",
    "pp_late_reference",
    "fb_credits_memo_designations",
    "fb_credits_snapshot_seal",
    "fb_credits_seal_insert",
    "fb_credits_seal_due_insert",
    "fb_credits_creditmemo_update",
    "fb_credits_creditmemo_delete",
    "fb_credits_creditmemoline_update",
    "fb_credits_creditmemoline_delete",
    "fb_credits_creditmemovoid_update",
    "fb_credits_creditmemovoid_delete",
    "fb_credits_creditapplication_update",
    "fb_credits_creditapplication_delete",
    "fb_credits_creditapplicationline_update",
    "fb_credits_creditapplicationline_delete",
    "fb_credits_creditunapplication_update",
    "fb_credits_creditunapplication_delete",
    "fb_credits_creditrefund_update",
    "fb_credits_creditrefund_delete",
    "fb_credits_creditrefundvoid_update",
    "fb_credits_creditrefundvoid_delete",
    "fb_credits_seal_update",
    "fb_credits_seal_delete",
    "fb_credits_seal_due_update",
    "fb_credits_seal_due_delete",
    "fb_credits_creditmemo_insert",
    "fb_credits_creditmemoline_insert",
    "fb_credits_creditmemovoid_insert",
    "fb_credits_creditapplication_insert",
    "fb_credits_creditapplicationline_insert",
    "fb_credits_creditunapplication_insert",
    "fb_credits_creditrefund_insert",
    "fb_credits_creditrefundvoid_insert",
    "fb_credits_creditmemo_due",
    "fb_credits_creditmemovoid_due",
    "fb_credits_creditapplication_due",
    "fb_credits_creditunapplication_due",
    "fb_credits_creditrefund_due",
    "fb_credits_creditrefundvoid_due",
    "fb_credits_memo_scope",
    "fb_credits_line_scope",
    "fb_credits_application_scope",
    "fb_credits_application_target",
    "fb_credits_application_bound",
    "fb_credits_receipt_bound",
    "fb_credits_use_application",
    "fb_credits_use_refund",
    "fb_credits_unapplication_scope",
    "fb_credits_refund_scope",
    "fb_credits_payment_number",
    "fb_credits_memo_void_scope",
    "fb_credits_refund_void_scope",
    "fb_credits_invoice_void",
    "fb_credits_journal_due",
    "fb_credits_late_reference",
    "fb_credits_seal_memo",
    "fb_credits_seal_memo_void",
    "fb_credits_seal_application",
    "fb_credits_seal_unapplication",
    "fb_credits_seal_refund",
    "fb_credits_seal_refund_void",
    "fb_credits_seal_journal",
    "fb_credits_tax_scope",
    "fb_banking_actor_insert",
    "fb_banking_actor_primary_key_permanent",
    "fb_banking_actor_update",
    "fb_banking_actor_delete",
    "fb_banking_actor_user_kept",
    "fb_banking_actor_seal",

    "fb_banking_all_cleared_recorded", "fb_banking_rec_request_key", "fb_banking_rec_transition_fields",
    "fb_banking_charges_fixed", "fb_banking_charges_seal", "fb_banking_account_kept", "fb_banking_user_kept",
    "fb_banking_deposit_scope",
    "fb_banking_deposit_due",
    "fb_banking_line_scope",
    "fb_banking_reversal_scope",
    "fb_banking_receipt_live_deposit",
    "fb_banking_reversal_cleared",
    "fb_banking_journal_due",
    "fb_banking_late_reference",
    "fb_banking_rec_insert",
    "fb_banking_rec_fields_insert",
    "fb_banking_rec_update",
    "fb_banking_rec_due_insert",
    "fb_banking_rec_due_update",
    "fb_banking_rec_line_scope",
    "fb_banking_rec_line_due",
    "fb_banking_status_insert",
    "fb_banking_status_update",
    "fb_banking_seal_check",
    "fb_banking_deposit_collision_insert",
    "fb_banking_deposit_primary_key_permanent",
    "fb_banking_deposit_immutable_update",
    "fb_banking_deposit_kept",
    "fb_banking_depositline_collision_insert",
    "fb_banking_depositline_primary_key_permanent",
    "fb_banking_depositline_immutable_update",
    "fb_banking_depositline_kept",
    "fb_banking_depositreversal_collision_insert",
    "fb_banking_depositreversal_primary_key_permanent",
    "fb_banking_depositreversal_immutable_update",
    "fb_banking_depositreversal_kept",
    "fb_banking_reconciliation_collision_insert",
    "fb_banking_reconciliation_primary_key_permanent",
    "fb_banking_reconciliation_collision_update",
    "fb_banking_reconciliation_kept",
    "fb_banking_banklinestatus_collision_insert",
    "fb_banking_banklinestatus_primary_key_permanent",
    "fb_banking_banklinestatus_collision_update",
    "fb_banking_banklinestatus_kept",
    "fb_banking_reconciliationline_collision_insert",
    "fb_banking_reconciliationline_primary_key_permanent",
    "fb_banking_reconciliationline_immutable_update",
    "fb_banking_reconciliationline_kept",
    "fb_banking_seal_collision_insert",
    "fb_banking_seal_primary_key_permanent",
    "fb_banking_seal_immutable_update",
    "fb_banking_seal_kept",
    "fb_banking_seal_due_collision_insert",
    "fb_banking_seal_due_primary_key_permanent",
    "fb_banking_seal_due_immutable_update",
    "fb_banking_seal_due_kept",
    "fb_banking_one_open",

    "fb_sales_draft_customer_eligible",
    "fb_payables_line_origin_insert", "fb_payables_billline_snapshot_shape", "fb_payables_billrevision_snapshot_shape", "fb_payables_billpayment_snapshot_shape",
    "fb_payables_draft_vendor_eligible",
    "fb_payables_draft_item_eligible",
    "fb_l2_revision_documents_accountingsetup_insert",
    "fb_l2_revision_documents_accountingsetup_update",
    "fb_l2_revision_salestax_taxitem_insert",
    "fb_l2_revision_salestax_taxitem_update",
    "fb_l2_revision_salestax_salestaxsettings_insert",
    "fb_l2_revision_salestax_salestaxsettings_update",
    "fb_l2_directory_customer_primary_key_permanent",
    "fb_l2_directory_vendor_primary_key_permanent",
    "fb_l2_directory_item_primary_key_permanent",
    "fb_l2_salestax_taxitem_primary_key_permanent",
    "fb_l2_salestax_taxgroupmember_primary_key_permanent",
    "fb_l2_salestax_salestaxsettings_primary_key_permanent",
    "fb_l2_documents_documentsequence_primary_key_permanent",
    "fb_l2_documents_documentidentity_primary_key_permanent",
    "fb_l2_documents_documentaccount_primary_key_permanent",
    "fb_l2_documents_accountingsetup_primary_key_permanent",
    "fb_l2_documents_designatedbank_primary_key_permanent",
    "fb_l2_directory_customer_ownership_permanent",
    "fb_l2_directory_vendor_ownership_permanent",
    "fb_l2_directory_item_ownership_permanent",
    "fb_l2_salestax_taxitem_ownership_permanent",
    "fb_l2_salestax_taxgroupmember_ownership_permanent",
    "fb_l2_salestax_salestaxsettings_ownership_permanent",
    "fb_l2_documents_documentsequence_ownership_permanent",
    "fb_l2_documents_documentidentity_ownership_permanent",
    "fb_l2_documents_documentaccount_ownership_permanent",
    "fb_l2_documents_accountingsetup_ownership_permanent",
    "fb_l2_documents_designatedbank_ownership_permanent",
    "fb_l2_ledger_journal_ownership_permanent",
    "fb_user_primary_key_permanent",
    "ledger_company_primary_key_permanent",
    "ledger_account_primary_key_permanent",
    "ledger_journal_primary_key_permanent",
    "ledger_journalline_primary_key_permanent",
    "ledger_auditevent_primary_key_permanent",
    "ledger_company_members_primary_key_permanent",
    "fb_seal_primary_key_permanent",
    "fb_seal_due_primary_key_permanent",
    "fb_account_company_permanent",
    "fb_line_journal_permanent",
    "fb_journal_origin_permanent",
    "fb_payables_bill_permanent_id", "fb_payables_billrevision_permanent_id", "fb_payables_billline_permanent_id", "fb_payables_billpayment_permanent_id", "fb_payables_paymentallocation_permanent_id", "fb_payables_paymentreversal_permanent_id", "fb_payables_draft_line_eligible",
    "fb_allocation_insert", "fb_bill_delete", "fb_bill_due_insert", "fb_bill_due_update", "fb_bill_insert", "fb_bill_line_calculation", "fb_bill_line_insert", "fb_bill_revision_insert", "fb_bill_seal", "fb_bill_update", "fb_bill_update_collision", "fb_payables_billline_delete", "fb_payables_billline_update", "fb_payables_billpayment_delete", "fb_payables_billpayment_update", "fb_payables_billrevision_delete", "fb_payables_billrevision_update", "fb_payables_directory_item_kept", "fb_payables_directory_item_replace", "fb_payables_directory_item_scope", "fb_payables_directory_vendor_kept", "fb_payables_directory_vendor_replace", "fb_payables_directory_vendor_scope", "fb_payables_documentaccount_replace", "fb_payables_documents_documentidentity_kept", "fb_payables_documents_documentidentity_replace", "fb_payables_fb_seal_due_replace", "fb_payables_fb_seal_replace", "fb_payables_journal_due", "fb_payables_journal_line_replace", "fb_payables_journal_seal", "fb_payables_late_reference", "fb_payables_ledger_account_kept", "fb_payables_ledger_account_replace", "fb_payables_ledger_account_scope", "fb_payables_ledger_journal_kept", "fb_payables_ledger_journal_replace", "fb_payables_line_parent", "fb_payables_paymentallocation_delete", "fb_payables_paymentallocation_update", "fb_payables_paymentreversal_delete", "fb_payables_paymentreversal_update", "fb_payables_post_eligibility", "fb_payment_due", "fb_payment_insert", "fb_payment_reversal_insert", "fb_payment_seal",
    "fb_company_guard_delete", "fb_account_guard_delete", "fb_audit_guard_insert",
    "fb_user_guard_update", "fb_members_guard_update",
    "fb_user_guard_insert", "fb_user_guard_delete", "fb_seal_guard_insert", "fb_seal_due_guard_insert",
    "fb_source_journal_insert", "fb_source_journal_update", "fb_journal_control_account", "fb_account_configured_company",
    "fb_salestax_item_delete",
    "fb_salestax_item_identity",
    "fb_salestax_item_agency_insert",
    "fb_salestax_member_insert",
    "fb_salestax_settings_default_insert",
    "fb_salestax_item_agency_update",
    "fb_salestax_member_update",
    "fb_salestax_settings_default_update",
    "fb_salestax_settings_delete",
    "fb_salestax_settings_company",
    "fb_document_sequence_delete",
    "fb_document_sequence_update",
    "fb_document_identity_update",
    "fb_document_identity_delete",
    "fb_document_account_update",
    "fb_document_account_delete",
    "fb_document_identity_insert",
    "fb_document_account_insert",
    "fb_account_document_reference",
    "fb_company_document_currency",

    "fb_seal_frozen",
    "fb_seal_kept",
    "fb_seal_due_frozen",
    "fb_seal_due_kept",
    "fb_company_guard_insert",
    "fb_company_guard_update",
    "fb_account_guard_insert",
    "fb_account_guard_update",
    "fb_account_fill_type_insert",
    "fb_account_fill_type_update",
    "fb_audit_no_update",
    "fb_audit_no_delete",
    "fb_journal_guard_insert",
    "fb_journal_guard_update",
    "fb_journal_guard_delete",
    "fb_line_guard_insert",
    "fb_line_guard_update",
    "fb_line_guard_delete",
];

pub const REQUIRED_INDEXES: &[&str] = &[
    "fb_checks_line_origin","journal_original_per_source","item_code_ci_per_company", "salestax_item_name_ci_per_company", "journal_entry_number_per_company", "journal_request_key_per_company"];

/// The shared value guards of 0130 (one BEFORE INSERT and one BEFORE UPDATE per table with TEXT columns),
/// required like [`REQUIRED_TRIGGERS`]; named from the migration itself so the two cannot drift.
pub fn value_guard_triggers() -> impl Iterator<Item = &'static str> {
    [include_str!("../sql/0130_shared_validators.sql"), include_str!("../sql/0132_estimate_validators.sql"), include_str!("../sql/0137_bankfeeds_reconnect.sql"), include_str!("../sql/0161_credential_validators.sql"), include_str!("../sql/0162_numbering.sql"), include_str!("../sql/0181_company_document_guards.sql"), include_str!("../sql/0186_desktop_user_value_guards.sql"), include_str!("../sql/0187_desktop_mfa.sql")]
        .into_iter().flat_map(str::lines)
        .filter_map(|line| line.strip_prefix("CREATE TRIGGER "))
        .filter_map(|rest| rest.split(' ').next())
        .filter(|name| name.starts_with("fb_valid_"))
}

/// Violation queries run on every open: each returns rows only when the books break a ledger rule
/// that a connection without the core's pragmas (or with triggers dropped) could have slipped in.
pub const VIOLATION_QUERIES: &[(&str, &str)] = &[
    ("invalid estimate conversion copy", include_str!("estimates_conversion_integrity.sql")),
    ("invalid estimate states", "SELECT r.id FROM sales_estimaterevision r LEFT JOIN sales_estimaterevision p ON p.estimate_id=r.estimate_id AND p.number=r.number-1 JOIN sales_estimate e ON e.id=r.estimate_id WHERE NOT fb_estimate_transition_ok(coalesce(p.state,''),r.state,json_extract(r.data,'$.action'),json_extract(r.data,'$.reason')) OR json_extract(r.data,'$.company_snapshot.id') IS NOT e.company_id OR json_extract(r.data,'$.id') IS NOT r.id OR json_extract(r.data,'$.number') IS NOT r.number OR json_extract(r.data,'$.customer_id') IS NOT r.customer_id OR json_extract(r.data,'$.date') IS NOT r.date OR json_extract(r.data,'$.expiry') IS NOT r.expiry OR json_extract(r.data,'$.state') IS NOT r.state"),
    ("invalid estimate evidence", "SELECT e.id FROM sales_estimate e WHERE NOT EXISTS(SELECT 1 FROM sales_estimaterevision r JOIN estimate_seal s ON s.kind='revision' AND s.doc_id=r.id WHERE r.estimate_id=e.id AND r.number=e.revision) UNION ALL SELECT r.id FROM sales_estimaterevision r WHERE NOT fb_sales_header_ok('invoice',r.data) OR NOT fb_sales_document_ok(json_set(r.data,'$.state','DRAFT'),(SELECT json_group_array(json(l.data)) FROM (SELECT data FROM sales_estimateline WHERE revision_id=r.id ORDER BY position) l),r.total)"),
    ("desktop user administration", "SELECT problem FROM desktop_user_invalid"),
    ("owner", "SELECT 'invalid owner account' FROM accounts_owner o WHERE NOT EXISTS(SELECT 1 FROM fb_user u WHERE u.id=o.user_id)"),
    ("invalid stored text, identifier, date or timestamp", "SELECT problem FROM fb_value_invalid"),
    ("invalid numbering settings", "SELECT company_id FROM fb_numbering_invalid"),
    ("invalid attachment evidence", "SELECT id FROM attachment_invalid UNION ALL SELECT id FROM attachment_audit_invalid UNION ALL SELECT id FROM attachment_text_invalid"),
    ("invalid assistant evidence", "SELECT id FROM assistant_invalid"),
    ("invalid assistant model selection", "SELECT id FROM assistant_selection_invalid"),
    ("ambiguous stored JSON", "SELECT problem FROM fb_json_invalid"),
    ("duplicate live shared check number", "SELECT company_id||':'||bank_id||':'||number FROM fb_check_number_live GROUP BY company_id,bank_id,number HAVING count(*)>1"),
    ("invalid checks/transfers evidence", include_str!("checks_violations.sql")),
    ("prepayment invariants", include_str!("prepayments_integrity.sql")),
    ("ambiguous banking JSON", "SELECT id FROM banking_json_invalid"),
    ("invalid banking payloads, dates or keys", "SELECT id FROM banking_evidence_invalid"),
    ("invalid bank-feed decisions, mappings or parents", "SELECT id FROM bankfeeds_invalid"),
    ("invalid bank-feed text, keys, dates or timestamps", "SELECT id FROM bankfeeds_text_invalid"),
    ("invalid banking operators", "SELECT id FROM banking_actor_invalid"),
    ("invalid banking relationships", "SELECT id FROM banking_relationship_invalid"),
    ("invalid banking charges", "SELECT id FROM banking_charges_invalid"),
    ("invalid banking deposits", "SELECT id FROM banking_deposit_invalid"),
    ("invalid banking reconciliation", "SELECT id FROM banking_rec_invalid"),
    ("invalid payables evidence", include_str!("payables_violations.sql")),
    ("vendor default account outside its company", "SELECT v.id FROM directory_vendor v WHERE v.default_expense_account_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=v.default_expense_account_id AND a.company_id=v.company_id)"),
    ("item default account outside its company or kind", "SELECT i.id FROM directory_item i WHERE (i.income_account_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=i.income_account_id AND a.company_id=i.company_id AND a.kind='INCOME')) OR (i.expense_account_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=i.expense_account_id AND a.company_id=i.company_id AND a.kind='EXPENSE'))"),
    ("setup account outside its company or kind", "SELECT s.id FROM documents_accountingsetup s WHERE (s.receivables_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=s.receivables_id AND a.company_id=s.company_id AND a.kind='ASSET')) OR (s.payables_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=s.payables_id AND a.company_id=s.company_id AND a.kind='LIABILITY')) OR (s.undeposited_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=s.undeposited_id AND a.company_id=s.company_id AND a.kind='ASSET')) OR (s.collected_tax_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=s.collected_tax_id AND a.company_id=s.company_id AND a.kind='LIABILITY')) OR (s.customer_deposits_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=s.customer_deposits_id AND a.company_id=s.company_id AND a.kind='LIABILITY'))"),
    ("designated bank outside its company", "SELECT b.id FROM documents_designatedbank b JOIN documents_accountingsetup s ON s.id=b.setup_id WHERE NOT EXISTS(SELECT 1 FROM ledger_account a WHERE a.id=b.account_id AND a.company_id=s.company_id)"),
    ("inactive member in active tax group", "SELECT m.id FROM salestax_taxgroupmember m JOIN salestax_taxitem g ON g.id=m.group_id LEFT JOIN salestax_taxitem r ON r.id=m.member_id WHERE g.active=1 AND (r.id IS NULL OR r.active<>1)"),
    ("inactive default tax item", "SELECT s.id FROM salestax_salestaxsettings s LEFT JOIN salestax_taxitem t ON t.id=s.default_item_id WHERE s.default_item_id IS NOT NULL AND (t.id IS NULL OR t.active<>1)"),
    ("document identity currency or number mismatch", "SELECT d.id FROM documents_documentidentity d JOIN ledger_company c ON c.id=d.company_id WHERE d.currency<>c.currency OR d.number<>d.kind||'-'||printf('%06d',d.ordinal)"),
    ("document account outside its company or frozen kind", "SELECT d.id FROM documents_documentaccount d JOIN documents_documentidentity s ON s.id=d.source_id JOIN ledger_account a ON a.id=d.account_id WHERE s.company_id<>a.company_id OR d.account_kind<>a.kind"),
    ("sales tax agency outside its company", "SELECT t.id FROM salestax_taxitem t JOIN directory_vendor v ON v.id=t.agency_id WHERE t.company_id<>v.company_id"),
    ("sales tax group member outside its company or kind", "SELECT m.id FROM salestax_taxgroupmember m JOIN salestax_taxitem g ON g.id=m.group_id JOIN salestax_taxitem t ON t.id=m.member_id WHERE g.company_id<>t.company_id OR g.kind<>'GROUP' OR t.kind<>'RATE'"),
    ("sales tax default outside its company", "SELECT s.id FROM salestax_salestaxsettings s JOIN salestax_taxitem t ON t.id=s.default_item_id WHERE s.company_id<>t.company_id"),
    ("journal source outside its company or kind", "SELECT j.id FROM ledger_journal j JOIN documents_documentidentity d ON d.id=j.source_id WHERE j.company_id<>d.company_id OR d.kind='EST'"),
    ("reversal source ownership mismatch", "SELECT j.id FROM ledger_journal j JOIN ledger_journal o ON o.id=j.reversal_of_id WHERE j.source_id IS NOT o.source_id"),

    (
        "posted journal not balanced (>= 2 lines, equal nonzero debits and credits)",
        "SELECT j.id FROM ledger_journal j WHERE j.status = 'POSTED' AND (
            (SELECT count(*) FROM ledger_journalline WHERE journal_id = j.id) < 2
         OR coalesce((SELECT sum(debit) FROM ledger_journalline WHERE journal_id = j.id), 0) <= 0
         OR coalesce((SELECT sum(debit) FROM ledger_journalline WHERE journal_id = j.id), 0)
            <> coalesce((SELECT sum(credit) FROM ledger_journalline WHERE journal_id = j.id), 0)
         OR j.posted_by_id IS NULL OR j.posted_at IS NULL)",
    ),
    (
        "journal line on another company's account",
        "SELECT l.id FROM ledger_journalline l JOIN ledger_journal j ON j.id = l.journal_id
         JOIN ledger_account a ON a.id = l.account_id WHERE a.company_id <> j.company_id",
    ),
    (
        "journal state invalid",
        "SELECT id FROM ledger_journal WHERE status NOT IN ('DRAFT', 'POSTED')
         OR (status = 'DRAFT' AND (posted_at IS NOT NULL OR posted_by_id IS NOT NULL))",
    ),
    (
        "reversal does not exactly compensate its original",
        "SELECT r.id FROM ledger_journal r JOIN ledger_journal o ON o.id = r.reversal_of_id
         WHERE r.status = 'POSTED' AND (o.status <> 'POSTED' OR o.reversal_of_id IS NOT NULL OR r.date < o.date
            OR o.company_id <> r.company_id OR r.adjusting <> o.adjusting
            OR EXISTS (SELECT position, account_id, debit, credit FROM ledger_journalline WHERE journal_id = o.id
                       EXCEPT SELECT position, account_id, credit, debit FROM ledger_journalline WHERE journal_id = r.id)
            OR EXISTS (SELECT position, account_id, credit, debit FROM ledger_journalline WHERE journal_id = r.id
                       EXCEPT SELECT position, account_id, debit, credit FROM ledger_journalline WHERE journal_id = o.id))",
    ),
    (
        "account with a blank type (the fill trigger did not run)",
        "SELECT id FROM ledger_account WHERE detail_type = ''",
    ),
    (
        "manual posted entry without a number or over its bounds",
        "SELECT j.id FROM ledger_journal j WHERE j.status = 'POSTED' AND j.source_id IS NULL AND j.reversal_of_id IS NULL AND (j.entry_number = ''
           OR (SELECT count(*) FROM ledger_journalline WHERE journal_id = j.id) > 50
           OR coalesce((SELECT sum(debit) FROM ledger_journalline WHERE journal_id=j.id),0) > 9999999999999999)",
    ),
    (
        "invalid journal retry evidence or revision",
        "SELECT id FROM ledger_journal WHERE revision NOT BETWEEN 1 AND 9223372036854775806
          OR (request_key IS NOT NULL AND length(creation_digest) <> 64)
          OR (reversal_of_id IS NOT NULL AND (entry_number <> '' OR request_key IS NOT NULL))",
    ),
];

#[derive(Debug)]
pub enum DbError {
    /// The key does not open the file (or the file is not a book).
    WrongKeyOrNotABook,
    /// The file fails a format check: empty, cut short, or not a FreedomBooks book.
    Damaged(&'static str),
    /// A connection rule did not take (e.g. foreign_keys reads back 0).
    Pragma(String),
    /// The book failed an open-time check; each entry names one problem.
    Integrity(Vec<String>),
    /// The file was written by a newer version.
    TooNew { found: i64, known: i64 },
    /// The file could not be reached (missing, no permission).
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbError::WrongKeyOrNotABook => f.write_str("The key does not open this book, or the file is not a book."),
            DbError::Damaged(what) => write!(f, "The file is not a readable book: {what}."),
            DbError::Pragma(p) => write!(f, "Connection rule not in effect: {p}"),
            DbError::Integrity(problems) => write!(f, "The book failed its integrity checks: {}", problems.join("; ")),
            DbError::TooNew { found, known } => write!(f, "Book schema {found} is newer than this app ({known})."),
            DbError::Io(e) => write!(f, "{e}"),
            DbError::Sqlite(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DbError {}

impl DbError {
    /// The file itself is bad (as opposed to a wrong key, a newer book or an unavailable disk).
    pub fn is_damage(&self) -> bool {
        match self {
            DbError::Damaged(_) | DbError::Integrity(_) => true,
            DbError::Sqlite(e) => matches!(e.sqlite_error_code(), Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase)),
            _ => false,
        }
    }
}

/// Every valid SQLite page size is a multiple of this.
const MIN_PAGE: u64 = 512;

/// Checks that need no key, run before any key or password is tried, so a damaged file is named
/// the same way whatever secret is supplied. A book is a non-empty regular file of whole pages
/// whose first bytes are ciphertext (an unencrypted SQLite file is not a book).
pub fn check_file(path: &Path) -> Result<(), DbError> {
    use std::io::Read;
    let meta = std::fs::metadata(path).map_err(DbError::Io)?;
    if !meta.is_file() { return Err(DbError::Damaged("not a file")); }
    if meta.len() == 0 { return Err(DbError::Damaged("empty")); }
    if meta.len() % MIN_PAGE != 0 { return Err(DbError::Damaged("cut short")); }
    let mut head = [0u8; 16];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).map_err(|_| DbError::Damaged("unreadable"))?;
    if &head == b"SQLite format 3\0" { return Err(DbError::Damaged("not encrypted")); }
    Ok(())
}

/// A book opened before its pending migrations ran. It can only be copied (for the pre-upgrade
/// backup) or upgraded; nothing else reads or writes it in this state.
pub struct Unmigrated {
    book: Book,
    from: usize,
    installed: Vec<(&'static str, &'static str)>,
}

/// The result of [`Book::open_staged`].
pub enum Opened {
    Current(Book),
    Behind(Unmigrated),
}

/// A source for a backup snapshot: an open book, or one waiting for its upgrade.
pub trait SnapshotSource {
    fn snapshot_size(&self) -> Result<u64, DbError>;
    fn snapshot(&self, path: &Path, key: &Key) -> Result<(), DbError>;
    /// The exact known migration history carried by the snapshot, including historical ordinals.
    fn migrations(&self) -> &[(&'static str, &'static str)];
}

impl SnapshotSource for Book {
    fn snapshot_size(&self) -> Result<u64, DbError> { Book::snapshot_size(self) }
    fn snapshot(&self, path: &Path, key: &Key) -> Result<(), DbError> { Book::snapshot(self, path, key) }
    fn migrations(&self) -> &[(&'static str, &'static str)] { MIGRATIONS }
}

impl Unmigrated {
    /// The book's schema version (applied migrations).
    pub fn from(&self) -> usize { self.from }
    /// This app's schema version.
    pub fn to(&self) -> usize { MIGRATIONS.len() }
    /// Validate an older schema before choosing the backup or migration path.
    fn validate(&self) -> Result<(), DbError> {
        let conn = &self.book.conn;
        let installed = authenticated_migrations(conn, self.from)?;
        if installed != self.installed { return Err(DbError::Integrity(vec!["pending migration history changed".into()])); }
        let mut problems = Vec::new();
        if !problems.is_empty() { return Err(DbError::Integrity(problems)); }
        let quick: Vec<String> = collect(conn, "PRAGMA quick_check", |r| r.get(0))?;
        if quick != ["ok"] { problems.extend(quick.into_iter().map(|q| format!("quick_check: {q}"))); }
        let foreign: i64 = conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r.get(0))?;
        if foreign != 0 { problems.push("foreign_key_check failed".into()); }
        if !problems.is_empty() { return Err(DbError::Integrity(problems)); }
        Ok(())
    }
    /// Run the pending migrations and the open-time checks.
    pub fn upgrade(self) -> Result<Book, DbError> {
        let mut book = self.book;
        Book::enable_wal(&book.conn)?;
        book.migrate()?;
        book.finish_open()?;
        Ok(book)
    }
}

impl SnapshotSource for Unmigrated {
    fn snapshot_size(&self) -> Result<u64, DbError> { self.book.snapshot_size() }
    /// The copy is checked against the book's own (older) schema, like a staged backup snapshot.
    fn snapshot(&self, path: &Path, key: &Key) -> Result<(), DbError> {
        self.validate()?;
        self.book.write_snapshot(path, key)
    }
    fn migrations(&self) -> &[(&'static str, &'static str)] { &self.installed }
}

impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        DbError::Sqlite(e)
    }
}

/// One open, keyed book. Holds a single connection; open a second `Book` on the same file for a
/// second thread (SQLite serializes writers; `busy_timeout` makes the second one wait).
/// Those connections share a local-process guard, retained until the last connection closes.
pub struct Book {
    conn: Connection,
    callback_mode: Arc<AtomicU8>,
    owner_empty_command: Arc<AtomicU8>,
    pending_credential_masks: std::collections::BTreeSet<String>,
    retained_text_guards: BTreeMap<String, TextGuard>,
    retained_text_state: RetainedState,
}

// Last Book field: the live guard outlives conn and every retained text policy. Private snapshot
// inspection uses Default without a guard and must never inspect an active book (backup.rs).
#[derive(Default)]
struct RetainedState {
    text: BTreeMap<String, Box<dyn std::any::Any + Send + Sync>>,
    _process_lock: Option<Arc<ProcessBookGuard>>,
}

pub type Key = [u8; 32];

fn hex(key: &Key) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

// Both guards outlive SQLite: the compatible pathname sidecar excludes old builds, while
// the book descriptor's OS lock excludes other processes opening hardlink aliases.
struct ProcessBookGuard {
    _sidecar: std::fs::File,
    book: std::fs::File,
    path: std::path::PathBuf,
    admission: std::sync::Mutex<()>,
    maintenance: std::sync::atomic::AtomicBool,
}
impl ProcessBookGuard {
    fn ensure_single_link(&self) -> Result<(), DbError> {
        #[cfg(unix)] {
            use std::os::unix::fs::MetadataExt;
            if self.book.metadata().map_err(DbError::Io)?.nlink() != 1 {
                return Err(DbError::Pragma("This book has hard links. Restore a backup as a separate book before opening or changing protection.".into()));
            }
        }
        Ok(())
    }
    fn verify(&self) -> Result<(), DbError> {
        let actual = std::fs::metadata(&self.path).map_err(DbError::Io)?;
        let held = self.book.metadata().map_err(DbError::Io)?;
        #[cfg(unix)] {
            use std::os::unix::fs::MetadataExt;
            if (actual.dev(), actual.ino()) != (held.dev(), held.ino()) {
                return Err(DbError::Pragma("The book file changed while opening. Try again.".into()));
            }
        }
        #[cfg(not(unix))] { let _ = (actual, held); }
        Ok(())
    }
}
fn lock_book_file(file: &std::fs::File) -> Result<(), DbError> {
    match file.try_lock() {
        Ok(()) => Ok(()),
        Err(std::fs::TryLockError::WouldBlock) => Err(DbError::Pragma("This book is already open.".into())),
        Err(std::fs::TryLockError::Error(e)) => Err(DbError::Io(e)),
    }
}

// Absence is authoritative only while the sidecar/inode locks (or a shared admission lock)
// are held. An I/O error is never evidence that recovery is unnecessary.
fn check_recovery_marker(path: &Path) -> Result<(), DbError> {
    let mut marker = path.as_os_str().to_owned();
    marker.push(".rekey.json");
    match std::fs::symlink_metadata(Path::new(&marker)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(DbError::Io(e)),
        Ok(_) => Err(DbError::Pragma("This book is already open or has an interrupted protection change. Recover it before opening.".into())),
    }
}

/// A reservation on an already-open book; no new same-process connections may share its guard.
pub struct ProtectionLease { guard: Arc<ProcessBookGuard> }
impl Drop for ProtectionLease {
    fn drop(&mut self) { self.guard.maintenance.store(false, Ordering::Release); }
}

/// Exclusive filesystem maintenance. Unlike normal connections it never shares an open Book's
/// guard, even in this process. Keep it alive through all rollback and key-record publication.
pub struct MaintenanceGuard { _guard: Arc<ProcessBookGuard> }
impl MaintenanceGuard {
    pub fn acquire(path: &Path) -> Result<Self, DbError> {
        Ok(Self { _guard: process_book_lock(path, false, true)? })
    }
    /// First-run publication may still have its own staged hard link; remove that known
    /// stage under this guard, then require a unique pathname before recovery changes bytes.
    pub fn ensure_single_link(&self) -> Result<(), DbError> {
        self._guard.verify()?;
        self._guard.ensure_single_link()
    }
    /// Lock the replacement inode before publication, until maintenance finishes.
    pub fn lock_replacement(&self, path: &Path) -> Result<std::fs::File, DbError> {
        let file = std::fs::OpenOptions::new().read(true).write(true).open(path).map_err(DbError::Io)?;
        lock_book_file(&file)?;
        Ok(file)
    }
}

// The local-process subset of book-lock-fix: the same canonical <book>.lock sidecar and
// std OS lock, acquired before SQLite can read/migrate. Never unlink a lock sidecar.
// Existing same-process connections share the guard; the last connection releases it.
fn process_book_lock(path: &Path, create: bool, maintenance: bool) -> Result<Arc<ProcessBookGuard>, DbError> {
    use std::fs::OpenOptions;
    use std::sync::{Mutex, Weak};
    static HELD: OnceLock<Mutex<BTreeMap<std::path::PathBuf, Weak<ProcessBookGuard>>>> = OnceLock::new();
    let fail = |error: std::io::Error| DbError::Pragma(format!("Book lock unavailable: {error}"));
    let canonical = match path.canonicalize() {
        Ok(path) => path,
        Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
            parent.canonicalize().map_err(fail)?.join(path.file_name().ok_or_else(|| fail(std::io::Error::other("Missing book name")))?)
        }
        Err(error) => return Err(fail(error)),
    };
    if !create && !std::fs::metadata(&canonical).map_err(DbError::Io)?.is_file() {
        return Err(DbError::Damaged("not a file"));
    }
    #[cfg(feature = "key-test-hooks")]
    if let Some(barrier) = std::env::var_os("FB_TEST_OPEN_BARRIER") {
        let barrier = std::path::PathBuf::from(barrier);
        std::fs::write(barrier.with_extension("ready"), b"ready").map_err(DbError::Io)?;
        while !barrier.with_extension("resume").exists() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    let mut held = HELD.get_or_init(Default::default).lock()
        .map_err(|_| fail(std::io::Error::other("Book lock state unavailable")))?;
    held.retain(|_, guard| guard.strong_count() != 0);
    if let Some(guard) = held.get(&canonical).and_then(Weak::upgrade) {
        let admission = guard.admission.lock().map_err(|_| DbError::Pragma("Book admission unavailable".into()))?;
        guard.verify()?;
        if maintenance || guard.maintenance.load(Ordering::Acquire) { return Err(DbError::Pragma("This book is already open.".into())); }
        guard.ensure_single_link()?;
        check_recovery_marker(&canonical)?;
        drop(admission);
        return Ok(guard);
    }
    let mut lock_name = canonical.as_os_str().to_owned();
    lock_name.push(".lock");
    let lock_path = Path::new(&lock_name);
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = match options.open(lock_path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // Refuse a redirected or non-file guard. The enclosing book folder must remain trusted,
            // as for book-lock-fix; never delete/replace a guard while another opener may hold it.
            if !std::fs::symlink_metadata(lock_path).map_err(fail)?.file_type().is_file() {
                return Err(fail(std::io::Error::other("Book lock is not a regular file")));
            }
            OpenOptions::new().read(true).write(true).open(lock_path).map_err(fail)?
        }
        Err(error) => return Err(fail(error)),
    };
    match file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Err(DbError::Pragma("This book is already open.".into())),
        Err(std::fs::TryLockError::Error(error)) => return Err(fail(error)),
    }
    // Do not erase or reinterpret book-lock-fix's foreign/stale record or implement takeover here.
    if !file.metadata().map_err(fail)?.is_file() || file.metadata().map_err(fail)?.len() != 0 {
        return Err(fail(std::io::Error::other("Existing book-lock record requires the matching book-lock version")));
    }
    let book = if create {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        options.open(&canonical).map_err(fail)?
    } else { OpenOptions::new().read(true).write(true).open(&canonical).map_err(fail)? };
    lock_book_file(&book)?;
    let guard = Arc::new(ProcessBookGuard { _sidecar: file, book, path: canonical.clone(), admission: Default::default(), maintenance: std::sync::atomic::AtomicBool::new(maintenance) });
    guard.verify()?;
    if !maintenance {
        guard.ensure_single_link()?;
        check_recovery_marker(&canonical)?;
    }
    held.insert(canonical, Arc::downgrade(&guard));
    Ok(guard)
}

impl Book {
    /// Reserve this sole connection for a protection operation. New same-process connections
    /// are refused until the lease drops; the existing connection retains both OS locks.
    pub fn protection_lease(&self) -> Result<ProtectionLease, DbError> {
        let guard = self.retained_text_state._process_lock.as_ref().ok_or_else(|| DbError::Pragma("Book guard unavailable".into()))?;
        let _admission = guard.admission.lock().map_err(|_| DbError::Pragma("Book admission unavailable".into()))?;
        guard.verify()?;
        guard.ensure_single_link()?;
        if Arc::strong_count(guard) != 1 || guard.maintenance.load(Ordering::Acquire) {
            return Err(DbError::Pragma("This book is already open in another connection.".into()));
        }
        guard.maintenance.store(true, Ordering::Release);
        Ok(ProtectionLease { guard: guard.clone() })
    }
    /// Create a new book file (refuses an existing path) and migrate it.
    pub fn create(path: &Path, key: &Key) -> Result<Book, DbError> {
        if path.exists() {
            return Err(DbError::Pragma(format!("{} already exists", path.display())));
        }
        let guard = process_book_lock(path, true, false)?;
        Self::open_inner(path, key, true, guard)
    }

    /// Open an existing book, migrate it forward and run the open-time checks.
    pub fn open(path: &Path, key: &Key) -> Result<Book, DbError> {
        Self::open_inner(path, key, false, process_book_lock(path, false, false)?)
    }

    /// Open an existing book without migrating it. A current book is checked and returned; an older
    /// one comes back [`Opened::Behind`] so the caller can copy it before [`Unmigrated::upgrade`].
    pub fn open_staged(path: &Path, key: &Key) -> Result<Opened, DbError> {
        let guard = process_book_lock(path, false, false)?;
        check_file(path)?;
        let book = Self::connect(path, key, false, guard)?;
        let applied = Self::schema_version(&book.conn)?;
        if applied < MIGRATIONS.len() as i64 && applied > 0 {
            let installed = authenticated_migrations(&book.conn, applied as usize)?;
            let old = Unmigrated { book, from: applied as usize, installed };
            old.validate()?;
            return Ok(Opened::Behind(old));
        }
        let mut book = book;
        book.migrate()?;
        book.finish_open()?;
        Self::enable_wal(&book.conn)?;
        Ok(Opened::Current(book))
    }

    fn schema_version(conn: &Connection) -> Result<i64, DbError> {
        let applied: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let known = MIGRATIONS.len() as i64;
        if applied > known {
            return Err(DbError::TooNew { found: applied, known });
        }
        if applied < 0 { return Err(DbError::Integrity(vec!["negative schema version".into()])); }
        Ok(applied)
    }

    fn open_inner(path: &Path, key: &Key, create: bool, guard: Arc<ProcessBookGuard>) -> Result<Book, DbError> {
        if !create { check_file(path)?; }
        let mut book = Self::connect(path, key, create, guard)?;
        if !create { Self::enable_wal(&book.conn)?; }
        book.migrate()?;
        book.finish_open()?;
        Ok(book)
    }

    /// Key the connection and set its rules; no migration and no open-time checks yet.
    fn connect(_path: &Path, key: &Key, create: bool, guard: Arc<ProcessBookGuard>) -> Result<Book, DbError> {
        let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        if create {
            flags |= OpenFlags::SQLITE_OPEN_CREATE;
        }
        guard.verify()?;
        let mut conn = Connection::open_with_flags(&guard.path, flags)?;
        guard.verify()?;
        // The raw-key form skips PBKDF2: the key is 32 random bytes, not a password.
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", hex(key)))?;
        // SQLCipher authenticates page 1 here: with a wrong key (or a damaged first page) it is NOTADB.
        if let Err(e) = conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0)) {
            return Err(match e.sqlite_error_code() {
                Some(rusqlite::ErrorCode::NotADatabase) | None => DbError::WrongKeyOrNotABook,
                _ => DbError::Sqlite(e),
            });
        }
        let cipher: Option<String> = conn.query_row("PRAGMA cipher_version", [], |r| r.get(0)).ok();
        if cipher.as_deref().unwrap_or("").is_empty() {
            return Err(DbError::Pragma("SQLCipher is not active".into()));
        }
        // Refuse unsupported or unversioned existing files before WAL configuration can
        // change their bytes, and before either open path can run any migration.
        if !create && Self::schema_version(&conn)? == 0 {
            return Err(DbError::Damaged("not a FreedomBooks book"));
        }
        if create { Self::configure(&conn)?; } else { Self::configure_connection(&conn)?; }
        let callback_mode = Arc::new(AtomicU8::new(0));
        let mode = callback_mode.clone();
        conn.authorizer(Some(move |ctx: AuthContext<'_>| authorize(ctx, mode.load(Ordering::Relaxed))))?;
        let owner_empty_command = register_owner_command(&conn)?;
        Ok(Book { conn, callback_mode, owner_empty_command, pending_credential_masks: Default::default(), retained_text_guards: BTreeMap::new(), retained_text_state: RetainedState { text: BTreeMap::new(), _process_lock: Some(guard) } })
    }

    fn finish_open(&mut self) -> Result<(), DbError> {
        self.pending_credential_masks = self.conn.prepare("SELECT id FROM bankfeeds_connection WHERE active=1")?
            .query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        let problems = self.check()?;
        if !problems.is_empty() {
            return Err(DbError::Integrity(problems));
        }
        Ok(())
    }

    fn configure(conn: &Connection) -> Result<(), DbError> {
        Self::configure_connection(conn)?;
        Self::enable_wal(conn)
    }

    fn enable_wal(conn: &Connection) -> Result<(), DbError> {
        let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(DbError::Pragma(format!("journal_mode is {mode}")));
        }
        conn.execute_batch("PRAGMA synchronous = FULL;")?;
        Ok(())
    }

    // Connection-only rules: a staged source stays byte-identical until its copy is verified.
    fn configure_connection(conn: &Connection) -> Result<(), DbError> {
        collation::register(conn)?;
        sales::register(conn)?;
        credits::register(conn)?;
        payables_functions::register(conn)?;
        check_numbers::register(conn)?;
        checks_snapshots::register(conn)?;
        prepayments::register(conn)?;
        banking_functions::register(conn)?;
        assistant::register(conn)?;
        json_evidence::register(conn)?;
        validators::register(conn)?;
        bankfeeds_text::register(conn)?;
        conn.busy_timeout(Duration::from_secs(10))?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA recursive_triggers = ON; PRAGMA synchronous = FULL;")?;
        for pragma in ["foreign_keys", "recursive_triggers"] {
            let on: i64 = conn.query_row(&format!("PRAGMA {pragma}"), [], |r| r.get(0))?;
            if on != 1 {
                return Err(DbError::Pragma(format!("{pragma} is off")));
            }
        }
        Ok(())
    }

    fn migrate(&mut self) -> Result<(), DbError> {
        let applied = Self::schema_version(&self.conn)?;
        if applied > 0 {
            let problems = schema_problems(&self.conn, applied as usize)?;
            if !problems.is_empty() {
                if legacy::upgrade_frozen_prefix(&mut self.conn, applied as usize)? { return Ok(()); }
                return Err(DbError::Integrity(problems));
            }
        }
        for (index, (_name, sql)) in MIGRATIONS.iter().enumerate().skip(applied as usize) {
            let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute_batch(sql)?;
            tx.execute_batch(&format!("PRAGMA user_version = {}", index + 1))?;
            tx.commit()?;
        }
        Ok(())
    }

    /// The open-time checks. Returns the list of problems (empty = healthy).
    pub fn check(&self) -> Result<Vec<String>, DbError> {
        let mut problems = Vec::new();
        let quick: Vec<String> = collect(&self.conn, "PRAGMA quick_check", |r| r.get(0))?;
        if quick != ["ok"] {
            problems.extend(quick.into_iter().map(|q| format!("quick_check: {q}")));
        }
        let fk: Vec<String> = collect(&self.conn, "PRAGMA foreign_key_check", |r| {
            Ok(format!("{} row {:?} -> {}", r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, String>(2)?))
        })?;
        problems.extend(fk.into_iter().map(|f| format!("foreign_key_check: {f}")));
        for name in REQUIRED_TRIGGERS.iter().copied().chain(value_guard_triggers()) {
            if !self.has_object("trigger", name)? {
                problems.push(format!("missing guard trigger {name}"));
            }
        }
        for name in REQUIRED_INDEXES {
            if !self.has_object("index", name)? {
                problems.push(format!("missing unique index {name}"));
            }
        }
        problems.extend(schema_problems(&self.conn, MIGRATIONS.len())?);
        for (label, sql) in VIOLATION_QUERIES {
            let rows: Vec<String> = collect(&self.conn, sql, |r| r.get(0))?;
            if !rows.is_empty() {
                problems.push(format!("{label}: {}", rows.join(", ")));
            }
        }
        problems.extend(sales::check(&self.conn)?);
        problems.extend(credits::check(&self.conn)?);
        Ok(problems)
    }

    fn has_object(&self, kind: &str, name: &str) -> Result<bool, DbError> {
        Ok(self.conn.query_row("SELECT count(*) FROM sqlite_master WHERE type = ?1 AND name = ?2", [kind, name], |r| r.get::<_, i64>(0))? == 1)
    }

    /// Retain a final TEXT policy for the lifetime of this open book. Every future Writer inherits
    /// it, including ordinary edits, voids and reversals outside the command that supplied it.
    /// Registration is additive and idempotent by opaque policy identity. A caller cannot remove
    /// a policy while secrets may still occur in stored source fields. Hosts reload their credential
    /// policies on each open, before exposing the book to commands. No policy or secret is persisted.
    /// ```compile_fail
    /// fn bypass(book: &mut fb_db::Book) {
    ///     book.retain_text_guard("bankfeeds".into(), |s| Ok(s.into()));
    /// }
    /// ```
    pub(crate) fn retain_text_guard(&mut self, identity: String, guard: impl Fn(&str) -> rusqlite::Result<String> + Send + Sync + 'static) {
        self.retained_text_guards.entry(identity).or_insert_with(|| Arc::new(guard));
    }

    /// Shared, in-memory state for an additive text policy. A policy can combine newly loaded
    /// secrets in one decoder instead of installing another full decoder per credential. State is
    /// scoped to this Book, never persisted, and an identity cannot be reused with a different type.
    pub(crate) fn retained_text_state<T: Default + Send + Sync + 'static>(&mut self, identity: &str) -> rusqlite::Result<Arc<std::sync::Mutex<T>>> {
        self.retained_text_state.text.entry(identity.to_owned())
            .or_insert_with(|| Box::new(Arc::new(std::sync::Mutex::new(T::default()))))
            .downcast_ref::<Arc<std::sync::Mutex<T>>>().cloned()
            .ok_or_else(|| rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other("A retained text policy has an incompatible state."))))
    }

    /// A credential loader calls this only after installing its retained final-text mask.
    /// Empty/missing policies cannot unlock writes, and every active connection must be loaded.
    /// ```compile_fail
    /// fn bypass(book: &mut fb_db::Book) {
    ///     book.credential_mask_restored("connection").unwrap();
    /// }
    /// ```
    pub(crate) fn credential_mask_restored(&mut self, connection_id: &str) -> rusqlite::Result<()> {
        if !self.retained_text_guards.contains_key("bankfeeds") {
            return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(
                "Restore bank-feed connection credentials before writing to this book."))));
        }
        self.pending_credential_masks.remove(connection_id);
        Ok(())
    }

    /// Only `release_unreadable_bankfeed` calls this, after it has proved that the stored credential
    /// cannot be read with the book's own key, so no mask for it can exist.
    /// ```compile_fail
    /// fn bypass(book: &mut fb_db::Book) {
    ///     book.credential_mask_unavailable("connection");
    /// }
    /// ```
    pub(crate) fn credential_mask_unavailable(&mut self, connection_id: &str) {
        self.pending_credential_masks.remove(connection_id);
    }

    /// Run `f` inside one `BEGIN IMMEDIATE` transaction. Commits on Ok, rolls back on Err. A deferred
    /// constraint (foreign key, seal obligation) that fails at COMMIT turns into an Err and rolls back.
    /// A callback authorizer denies transaction control, PRAGMA writes, DDL and ATTACH/DETACH.
    pub fn write<T, E>(&mut self, f: impl FnOnce(&Writer<'_>) -> Result<T, E>) -> Result<T, E>
    where
        E: From<rusqlite::Error>,
    {
        verify_write_pragmas(&self.conn)?;
        if !self.pending_credential_masks.is_empty() {
            return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(
                "Restore bank-feed connection credentials before writing to this book."))).into());
        }
        let writer = Writer {
            tx: self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?,
            callback_mode: self.callback_mode.clone(),
            text_guards: std::cell::RefCell::new(self.retained_text_guards.values().cloned().collect()),
        };
        let empty: bool = writer.tx.query_row("SELECT NOT EXISTS(SELECT 1 FROM fb_user)", [], |r| r.get(0))?;
        let owner_scope = CallbackScope::enter(&self.owner_empty_command, u8::from(empty));
        let scope = CallbackScope::enter(&self.callback_mode, 1);
        let value = f(&writer);
        drop(owner_scope);
        drop(scope); // Restore library-only transaction control even on callback Err.
        let value = value?;
        verify_write_pragmas(&writer.tx)?;
        writer.tx.commit()?;
        Ok(value)
    }

    /// Run `f` inside one read transaction: every statement sees the same snapshot (WAL), like the
    /// Python reports' REPEATABLE READ READ ONLY. `query_only` is on for its duration, so a write
    /// slipped into a read path fails instead of running under a deferred (non-IMMEDIATE) lock.
    /// A read-only authorizer also prevents disabling that protection from inside the callback.
    pub fn read<T, E>(&mut self, f: impl FnOnce(&Reader<'_>) -> Result<T, E>) -> Result<T, E>
    where
        E: From<rusqlite::Error>,
    {
        self.conn.execute_batch("PRAGMA query_only = ON")?;
        let result = (|| {
            let tx = self.conn.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let reset = QueryOnlyReset(&tx);
            let scope = CallbackScope::enter(&self.callback_mode, 2);
            let value = f(&Reader { conn: &tx });
            drop(scope);
            drop(reset);
            let value = value?;
            tx.finish()?;
            Ok(value)
        })();
        self.conn.execute_batch("PRAGMA query_only = OFF")?;
        result
    }

    /// Raw connection, for tests that attack the triggers directly and for the checks above.
    #[cfg(any(test, feature = "test-support"))]
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

type TextGuard = Arc<dyn Fn(&str) -> rusqlite::Result<String> + Send + Sync>;

/// Proof that the core acquired the IMMEDIATE writer lock. Only `Book::write` constructs it.
/// A raw/autocommit Connection (or a deferred read Transaction) cannot be passed to write helpers.
/// ```compile_fail
/// fn cannot_forge(conn: &fb_db::rusqlite::Connection) -> &fb_db::Writer<'_> { conn }
/// ```
/// ```compile_fail
/// fn cannot_get_raw<'a, 'conn>(writer: &'a fb_db::Writer<'conn>) -> &'a fb_db::rusqlite::Connection { writer }
/// ```
/// ```compile_fail
/// fn cannot_disable_triggers(writer: &fb_db::Writer<'_>) {
///     writer.set_db_config(fb_db::rusqlite::config::DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, false);
/// }
/// ```
/// ```compile_fail
/// fn cannot_remove_hook(writer: &mut fb_db::Writer<'_>) {
///     writer.authorizer(None::<fn(fb_db::rusqlite::hooks::AuthContext<'_>) -> fb_db::rusqlite::hooks::Authorization>);
/// }
/// ```
pub struct Writer<'conn> {
    tx: Transaction<'conn>,
    callback_mode: Arc<AtomicU8>,
    text_guards: std::cell::RefCell<Vec<TextGuard>>,
}

impl Writer<'_> {
    /// Scope a final TEXT-parameter policy over this writer and every composed service using it.
    /// The guard sees the serialized bytes immediately before SQL binding. It cannot be bypassed
    /// with a prepared write, a query returning rows, or a batch: those surfaces refuse writes while
    /// guarded. Nested guards compose, and the previous scope is restored on errors and unwinding.
    pub fn with_text_guard<T, E>(&self, guard: impl Fn(&str) -> rusqlite::Result<String> + Send + Sync + 'static, f: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
        self.text_guards.borrow_mut().push(Arc::new(guard));
        struct Reset<'a>(&'a std::cell::RefCell<Vec<TextGuard>>);
        impl Drop for Reset<'_> {
            fn drop(&mut self) { self.0.borrow_mut().pop(); }
        }
        let _reset = Reset(&self.text_guards);
        f()
    }

    fn guarded_text(&self, text: &str) -> rusqlite::Result<String> {
        self.text_guards.borrow().iter().try_fold(text.to_owned(), |s, guard| guard(&s))
    }

    fn guarded_sql_error() -> rusqlite::Error {
        rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(
            "A guarded write must use Writer::execute with final text parameters.",
        )))
    }

    pub fn prepare(&self, sql: &str) -> rusqlite::Result<rusqlite::Statement<'_>> {
        let statement = self.tx.prepare(sql)?;
        if !self.text_guards.borrow().is_empty() && !statement.readonly() {
            return Err(Self::guarded_sql_error());
        }
        Ok(statement)
    }

    pub fn query_row<T, P: rusqlite::Params, F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>>(
        &self, sql: &str, params: P, f: F,
    ) -> rusqlite::Result<T> {
        self.prepare(sql)?.query_row(params, f)
    }

    pub fn execute<P: rusqlite::Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize> {
        if self.text_guards.borrow().is_empty() { return self.tx.execute(sql, params); }
        // SQL literals are code-owned. If a credential also occurs in that code, refuse rather
        // than changing SQL semantics. All variable text must be bound, never interpolated.
        let plain_sql: String = sql.chars().map(|c| if c <= '\u{1f}' || c == '\u{7f}' { ' ' } else { c }).collect();
        if self.guarded_text(&plain_sql)? != plain_sql { return Err(Self::guarded_sql_error()); }
        let mut statement = self.tx.prepare(sql)?;
        let count = statement.parameter_count();
        if count == 0 { return statement.execute(params); }
        // Let rusqlite bind every supported Params form to a read-only projection. Reading back
        // the typed values avoids SQL expansion/reparsing and preserves NULL/numeric/blob types,
        // numbered holes, repeated bindings and named parameters without custom Params impls.
        let columns = (1..=count).map(|i| statement.parameter_name(i).map(str::to_owned).unwrap_or_else(|| format!("?{i}"))).collect::<Vec<_>>();
        let mut projection = self.tx.prepare(&format!("SELECT {}", columns.join(",")))?;
        let mut values = projection.query_row(params, |row| (0..count).map(|i| row.get::<_, rusqlite::types::Value>(i)).collect::<rusqlite::Result<Vec<_>>>())?;
        for value in &mut values {
            if let rusqlite::types::Value::Text(text) = value { *text = self.guarded_text(text)?; }
        }
        statement.execute(rusqlite::params_from_iter(values))
    }

    pub fn execute_batch(&self, sql: &str) -> rusqlite::Result<()> {
        if !self.text_guards.borrow().is_empty() { return Err(Self::guarded_sql_error()); }
        self.tx.execute_batch(sql)
    }
    pub fn is_autocommit(&self) -> bool { self.tx.is_autocommit() }

    /// Run a composed primitive atomically inside the owning IMMEDIATE transaction.
    /// The library alone controls this savepoint; `f` retains the full callback
    /// authorizer. Errors and panic unwinding roll back only this primitive.
    /// Callback SQL still cannot BEGIN, COMMIT, ROLLBACK or manipulate savepoints.
    pub fn atomic<T, E>(&self, f: impl FnOnce() -> Result<T, E>) -> Result<T, E>
    where
        E: From<rusqlite::Error>,
    {
        self.control("SAVEPOINT fb_library_primitive")?;
        let mut guard = PrimitiveRollback { writer: self, active: true };
        let value = f()?;
        self.control("RELEASE fb_library_primitive")?;
        guard.active = false;
        Ok(value)
    }

    fn control(&self, sql: &str) -> rusqlite::Result<()> {
        // No user SQL or closure executes with library privileges. Restore the
        // preceding policy before returning, including an SQLite error.
        let previous = self.callback_mode.swap(0, Ordering::Relaxed);
        let result = self.tx.execute_batch(sql);
        self.callback_mode.store(previous, Ordering::Relaxed);
        result
    }
}

struct PrimitiveRollback<'a, 'conn> {
    writer: &'a Writer<'conn>,
    active: bool,
}
impl Drop for PrimitiveRollback<'_, '_> {
    fn drop(&mut self) {
        if self.active {
            // A callback cannot remove our savepoint. If SQLite has already
            // rolled back the transaction, Book::write's COMMIT also fails.
            let _ = self.writer.control("ROLLBACK TO fb_library_primitive; RELEASE fb_library_primitive");
        }
    }
}
/// The read/query surface shared by callback handles and row readers. It intentionally
/// exposes no connection configuration, hook management, raw handle or transaction control.
/// The Connection implementation permits standalone test fixtures, not extraction from a book.
pub trait Query {
    fn prepare(&self, sql: &str) -> rusqlite::Result<rusqlite::Statement<'_>>;
    fn query_row<T, P: rusqlite::Params, F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>>(
        &self, sql: &str, params: P, f: F,
    ) -> rusqlite::Result<T> {
        self.prepare(sql)?.query_row(params, f)
    }
}
impl Query for Connection {
    fn prepare(&self, sql: &str) -> rusqlite::Result<rusqlite::Statement<'_>> {
        Connection::prepare(self, sql)
    }
}

/// A read callback handle, not a rusqlite Transaction or Connection. SQL writes are denied
/// by both its authorizer and query_only. There is no raw/configuration escape hatch.
/// ```compile_fail
/// fn cannot_change_configuration(reader: &fb_db::Reader<'_>) {
///     reader.set_db_config(fb_db::rusqlite::config::DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, false);
/// }
/// ```
/// ```compile_fail
/// fn cannot_remove_authorizer(reader: &mut fb_db::Reader<'_>) {
///     reader.authorizer(None::<fn(fb_db::rusqlite::hooks::AuthContext<'_>) -> fb_db::rusqlite::hooks::Authorization>);
/// }
/// ```
pub struct Reader<'conn> {
    conn: &'conn Connection,
}

// Forward SQL only; never Deref to Connection. SQL is subject to the installed authorizer,
// including SQL executed from triggers/views and statements prepared through this surface.
macro_rules! sql_surface {
    ($type:ident, $field:ident) => {
        impl $type<'_> {
            pub fn prepare(&self, sql: &str) -> rusqlite::Result<rusqlite::Statement<'_>> {
                self.$field.prepare(sql)
            }
            pub fn query_row<T, P: rusqlite::Params, F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>>(
                &self, sql: &str, params: P, f: F,
            ) -> rusqlite::Result<T> {
                self.$field.query_row(sql, params, f)
            }
            pub fn execute<P: rusqlite::Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize> {
                self.$field.execute(sql, params)
            }
            pub fn execute_batch(&self, sql: &str) -> rusqlite::Result<()> {
                self.$field.execute_batch(sql)
            }
            pub fn is_autocommit(&self) -> bool { self.$field.is_autocommit() }
        }
        impl Query for $type<'_> {
            fn prepare(&self, sql: &str) -> rusqlite::Result<rusqlite::Statement<'_>> {
                self.$field.prepare(sql)
            }
        }
    }
}
impl Query for Writer<'_> {
    fn prepare(&self, sql: &str) -> rusqlite::Result<rusqlite::Statement<'_>> { Writer::prepare(self, sql) }
}
sql_surface!(Reader, conn);

// Zero is library-owned setup/migration/transaction control. Callbacks never receive this
// state or a mutable Connection, so they cannot uninstall the authorizer. The scope resets
// on unwinding as well as Err, before the transaction's rollback executes.
struct QueryOnlyReset<'conn>(&'conn Connection);
impl Drop for QueryOnlyReset<'_> {
    fn drop(&mut self) {
        // The callback authorization scope drops first. Also reset on unwind, before
        // the library rolls back the read transaction; normal return checks OFF below.
        let _ = self.0.execute_batch("PRAGMA query_only = OFF");
    }
}
// This function has no SQL setter. Only Book::write can establish an empty-book
// command, after acquiring the writer lock; Err/unwind clears it before rollback.
fn register_owner_command(conn: &Connection) -> rusqlite::Result<Arc<AtomicU8>> {
    let state = Arc::new(AtomicU8::new(0));
    let captured = state.clone();
    conn.create_scalar_function("fb_owner_empty_command", 0,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8,
        move |_| Ok(captured.load(Ordering::Relaxed) == 1))?;
    Ok(state)
}
struct CallbackScope(Arc<AtomicU8>);
impl CallbackScope {
    fn enter(mode: &Arc<AtomicU8>, value: u8) -> Self {
        mode.store(value, Ordering::Relaxed);
        Self(mode.clone())
    }
}
impl Drop for CallbackScope {
    fn drop(&mut self) { self.0.store(0, Ordering::Relaxed); }
}
fn authorize(ctx: AuthContext<'_>, mode: u8) -> Authorization {
    use AuthAction::*;
    if mode == 0 { return Authorization::Allow; }
    let allowed = match ctx.action {
        Read { .. } | Select | Recursive => true,
        Function { function_name } => !["load_extension", "sqlcipher_export"].iter().any(|name| function_name.eq_ignore_ascii_case(name)),
        Insert { table_name } | Delete { table_name } | Update { table_name, .. } => {
            mode == 1 && !table_name.to_ascii_lowercase().starts_with("sqlite_")
        }
        // No PRAGMA writes are allowed in callbacks. Argument-bearing metadata pragmas
        // are deliberately denied too; the library alone configures the connection.
        Pragma { pragma_name, pragma_value: None } => [
            "foreign_keys", "recursive_triggers", "synchronous", "journal_mode",
            "ignore_check_constraints", "writable_schema", "query_only", "user_version",
            "defer_foreign_keys", "quick_check", "integrity_check", "foreign_key_check",
        ].iter().any(|name| pragma_name.eq_ignore_ascii_case(name)),
        // Fail closed: transaction/savepoint control, all DDL (including TEMP), ATTACH,
        // DETACH, ANALYZE, REINDEX, and future SQLite actions are not callback powers.
        _ => false,
    };
    if allowed { Authorization::Allow } else { Authorization::Deny }
}

fn verify_write_pragmas(conn: &Connection) -> rusqlite::Result<()> {
    for (pragma, expected) in [("foreign_keys", 1), ("recursive_triggers", 1), ("synchronous", 2), ("ignore_check_constraints", 0), ("writable_schema", 0)] {
        let value: i64 = conn.query_row(&format!("PRAGMA {pragma}"), [], |r| r.get(0))?;
        if value != expected {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_MISUSE),
                Some(format!("Required PRAGMA {pragma} is {value}, expected {expected}")),
            ));
        }
    }
    let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_MISUSE),
            Some("Required PRAGMA journal_mode is not WAL".into()),
        ));
    }
    Ok(())
}

type SchemaHashes = BTreeMap<String, String>;
fn schema_hashes(conn: &Connection) -> rusqlite::Result<SchemaHashes> {
    let mut stmt = conn.prepare("SELECT type, name, sql FROM sqlite_master WHERE name NOT GLOB 'sqlite_*' AND sql IS NOT NULL ORDER BY type,name")?;
    stmt.query_map([], |r| {
        let kind: String = r.get(0)?;
        let name: String = r.get(1)?;
        let sql: String = r.get(2)?;
        let hash: String = Sha256::digest(sql.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
        Ok((format!("{kind}:{name}"), hash))
    })?
    .collect()
}
fn expected_schema(version: usize) -> Result<&'static SchemaHashes, DbError> {
    static EXPECTED: OnceLock<Result<Vec<SchemaHashes>, String>> = OnceLock::new();
    EXPECTED
        .get_or_init(|| {
            let build = || -> rusqlite::Result<Vec<SchemaHashes>> {
                let conn = Connection::open_in_memory()?;
                payables_functions::register(&conn)?;
                check_numbers::register(&conn)?;
                checks_snapshots::register(&conn)?;
                prepayments::register(&conn)?;
                banking_functions::register(&conn)?;
                assistant::register(&conn)?;
                json_evidence::register(&conn)?;
                validators::register(&conn)?;
                bankfeeds_text::register(&conn)?;
                collation::register(&conn).map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?;
                sales::register(&conn)?;
                credits::register(&conn)?;
                let mut versions = vec![schema_hashes(&conn)?];
                for (_, sql) in MIGRATIONS {
                    conn.execute_batch(sql)?;
                    versions.push(schema_hashes(&conn)?);
                }
                Ok(versions)
            };
            build().map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| DbError::Integrity(vec![format!("Cannot derive versioned schema hashes: {e}")]))?
        .get(version)
        .ok_or_else(|| DbError::Integrity(vec![format!("Unknown schema version {version}")]))
}
/// Authenticate a complete installed schema before using its historical ordinal list.
fn authenticated_migrations(conn: &Connection, version: usize) -> Result<Vec<(&'static str, &'static str)>, DbError> {
    let problems = schema_problems(conn, version)?;
    if problems.is_empty() { return Ok(MIGRATIONS[..version].to_vec()); }
    if let Some(installed) = legacy::recognize(conn, version)? { return Ok(installed); }
    Err(DbError::Integrity(problems))
}

/// Only exact known histories may appear in a backup manifest, including historical ordinals.
pub fn supported_migrations(names: &[String]) -> bool {
    if names.is_empty() { return false; }
    legacy::histories().into_iter().chain(std::iter::once(MIGRATIONS.to_vec()))
        .any(|layout| names.len() <= layout.len() && names.iter().zip(layout).all(|(a,b)| a == b.0))
}

fn schema_problems(conn: &Connection, version: usize) -> Result<Vec<String>, DbError> {
    let expected = expected_schema(version)?;
    let actual = schema_hashes(conn)?;
    let mut problems = Vec::new();
    for (object, hash) in expected {
        if actual.get(object) != Some(hash) {
            problems.push(format!("schema definition mismatch: {object}"));
        }
    }
    for object in actual.keys() {
        if !expected.contains_key(object) {
            problems.push(format!("unexpected schema object: {object}"));
        }
    }
    Ok(problems)
}
fn collect<T>(conn: &Connection, sql: &str, f: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>) -> Result<Vec<T>, DbError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], f)?.collect::<Result<Vec<T>, _>>()?;
    Ok(rows)
}

/// The constraint or guard name of a refused write: the `name:` prefix of a `RAISE(ABORT, ...)`, the
/// name of a CHECK, or `unique:<table>.<cols>` / `foreign_key` for those failures.
pub fn constraint_of(error: &rusqlite::Error) -> Option<String> {
    let message = sqlite_message(error)?;
    if let Some(rest) = message.strip_prefix("CHECK constraint failed: ") {
        return Some(rest.to_string());
    }
    if let Some(rest) = message.strip_prefix("UNIQUE constraint failed: ") {
        return Some(format!("unique:{rest}"));
    }
    if message.starts_with("FOREIGN KEY constraint failed") {
        return Some("foreign_key".into());
    }
    let (name, _) = message.split_once(": ")?;
    if name.chars().all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()) {
        return Some(name.to_string());
    }
    None
}

/// The guard's user message after the `name: ` prefix (or the whole SQLite message).
pub fn guard_message(error: &rusqlite::Error) -> Option<String> {
    let message = sqlite_message(error)?;
    match constraint_of(error) {
        Some(_) if message.contains(": ") && !message.starts_with("CHECK") && !message.starts_with("UNIQUE") => {
            Some(message.split_once(": ").unwrap().1.to_string())
        }
        _ => Some(message),
    }
}

fn sqlite_message(error: &rusqlite::Error) -> Option<String> {
    match error {
        rusqlite::Error::SqliteFailure(_, Some(message)) => Some(message.clone()),
        rusqlite::Error::SqliteFailure(code, None) => Some(code.to_string()),
        _ => None,
    }
}

/// True when the error is a constraint failure (SQLITE_CONSTRAINT family): the IntegrityError of the port.
pub fn is_constraint(error: &rusqlite::Error) -> bool {
    matches!(error, rusqlite::Error::SqliteFailure(e, _) if e.code == rusqlite::ErrorCode::ConstraintViolation)
}

#[cfg(test)]
mod fix2_tests;

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fb-db-test-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("book.fbbook")
    }

    #[test]
    fn create_reopen_wrong_key_and_pragmas() {
        let path = temp("open");
        let key = [7u8; 32];
        {
            let book = Book::create(&path, &key).unwrap();
            let v: i64 = book.connection().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
            assert_eq!(v, MIGRATIONS.len() as i64);
        }
        let book = Book::open(&path, &key).unwrap();
        for pragma in ["foreign_keys", "recursive_triggers"] {
            let on: i64 = book.connection().query_row(&format!("PRAGMA {pragma}"), [], |r| r.get(0)).unwrap();
            assert_eq!(on, 1, "{pragma}");
        }
        drop(book);
        assert!(matches!(Book::open(&path, &[8u8; 32]), Err(DbError::WrongKeyOrNotABook)));
        // The file is encrypted: the SQLite magic header is not on disk.
        let head = std::fs::read(&path).unwrap();
        assert_ne!(&head[..15], b"SQLite format 3");
        assert!(Book::create(&path, &key).is_err());
    }

    #[test]
    fn unsealed_obligation_cannot_commit_and_is_flagged_on_open() {
        let path = temp("seal");
        let key = [9u8; 32];
        let mut book = Book::create(&path, &key).unwrap();
        // Through the core's connection: COMMIT fails on the deferred FK (probe B10a).
        let result: Result<(), rusqlite::Error> = book.write(|tx| {
            tx.execute("INSERT INTO fb_seal_due (kind, doc_id) VALUES ('journal', 'x')", [])?;
            Ok(())
        });
        assert!(result.is_err());
        // Sealing makes it commit.
        book.write(|tx| -> Result<(), rusqlite::Error> {
            tx.execute("INSERT INTO fb_seal_due (kind, doc_id) VALUES ('journal', 'y')", [])?;
            tx.execute("INSERT INTO fb_seal (kind, doc_id) VALUES ('journal', 'y')", [])?;
            Ok(())
        })
        .unwrap();
        let err = book.connection().execute("DELETE FROM fb_seal", []).unwrap_err();
        assert_eq!(constraint_of(&err).as_deref(), Some("seal_immutable"));
        // A connection with foreign_keys OFF can commit an unsealed row (probe B10f) ...
        book.connection().execute_batch("PRAGMA foreign_keys = OFF; INSERT INTO fb_seal_due VALUES ('journal', 'z');").unwrap();
        drop(book);
        // ... and the next open through the core refuses the book.
        match Book::open(&path, &key) {
            Err(DbError::Integrity(problems)) => assert!(problems.iter().any(|p| p.contains("foreign_key_check")), "{problems:?}"),
            other => panic!("expected an integrity refusal, got {:?}", other.map(|_| ())),
        }
    }

    #[test]
    fn dropped_trigger_is_refused_on_open() {
        let path = temp("trigger");
        let key = [3u8; 32];
        let book = Book::create(&path, &key).unwrap();
        book.connection().execute_batch("DROP TRIGGER fb_audit_no_delete;").unwrap();
        drop(book);
        match Book::open(&path, &key) {
            Err(DbError::Integrity(problems)) => assert!(problems.iter().any(|p| p.contains("fb_audit_no_delete"))),
            other => panic!("expected an integrity refusal, got {:?}", other.map(|_| ())),
        }
    }

    #[test]
    fn writes_cannot_run_inside_a_read_transaction() {
        let path = temp("readonly");
        let mut book = Book::create(&path, &[5u8; 32]).unwrap();
        let attempt: Result<(), rusqlite::Error> = book.read(|tx| {
            tx.execute("INSERT INTO fb_seal (kind, doc_id) VALUES ('x', 'y')", [])?;
            Ok(())
        });
        assert!(attempt.is_err());
        // The connection is writable again afterwards, through `write` (BEGIN IMMEDIATE).
        book.write(|tx| -> Result<(), rusqlite::Error> {
            let autocommit = tx.is_autocommit();
            assert!(!autocommit);
            Ok(())
        })
        .unwrap();
        let q: i64 = book.connection().query_row("PRAGMA query_only", [], |r| r.get(0)).unwrap();
        assert_eq!(q, 0);
    }

    #[test]
    fn sqlite_facts_d2_relies_on_still_hold_in_this_sqlcipher_build() {
        let conn = Connection::open_in_memory().unwrap();
        // F1: integer overflow in + silently becomes REAL.
        let t: String = conn.query_row("SELECT typeof(9223372036854775807 + 1)", [], |r| r.get(0)).unwrap();
        assert_eq!(t, "real");
        // F2: sum() overflow raises.
        conn.execute_batch("CREATE TABLE t (v INTEGER); INSERT INTO t VALUES (9223372036854775807), (1);").unwrap();
        assert!(conn.query_row("SELECT sum(v) FROM t", [], |r| r.get::<_, i64>(0)).is_err());
        // F3: round() is binary float.
        let r: f64 = conn.query_row("SELECT round(2.675, 2)", [], |r| r.get(0)).unwrap();
        assert_eq!(r, 2.67);
        // F5: no deferred triggers.
        assert!(conn.execute_batch("CREATE TRIGGER x AFTER INSERT ON t DEFERRABLE INITIALLY DEFERRED BEGIN SELECT 1; END;").is_err());
    }
    #[test]
    fn legacy_schema_migrates_but_tampered_definitions_are_not_repaired_silently() {
        for tamper in [false,true] {
            let path=temp(if tamper {"legacy-tampered"} else {"legacy-clean"}); let key=[12;32];
            let conn=Connection::open(&path).unwrap();
            conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";",hex(&key))).unwrap();
            Book::configure(&conn).unwrap();
            conn.execute_batch(MIGRATIONS[0].1).unwrap();
            conn.execute_batch("PRAGMA user_version=1").unwrap();
            if tamper {conn.execute_batch("DROP TRIGGER fb_journal_guard_insert; CREATE TRIGGER fb_journal_guard_insert BEFORE INSERT ON ledger_journal BEGIN SELECT 1; END;").unwrap();}
            drop(conn);
            let result=Book::open(&path,&key);
            if tamper {assert!(matches!(result,Err(DbError::Integrity(_))));}
            else {let book=result.unwrap();assert!(book.check().unwrap().is_empty());assert_eq!(book.conn.query_row("PRAGMA user_version",[],|r|r.get::<_,i64>(0)).unwrap(),MIGRATIONS.len() as i64);}
        }
    }

    #[test]
    fn changed_trigger_index_check_and_unversioned_view_are_refused_on_open() {
        for (label,sql) in [
            ("noop", "DROP TRIGGER fb_audit_no_delete; CREATE TRIGGER fb_audit_no_delete BEFORE DELETE ON ledger_auditevent BEGIN SELECT 1; END;"),
            ("index", "DROP INDEX journal_entry_number_per_company; CREATE INDEX journal_entry_number_per_company ON ledger_journal(company_id);"),
            ("check", "PRAGMA writable_schema=ON; UPDATE sqlite_master SET sql=replace(sql,'revision >= 0','revision >= -1') WHERE type='table' AND name='ledger_journal'; PRAGMA writable_schema=OFF;"),
            ("view", "CREATE VIEW unexpected_view AS SELECT * FROM ledger_auditevent;"),
        ] {
            let path=temp(label);let key=[13;32];let book=Book::create(&path,&key).unwrap();
            book.conn.execute_batch(sql).unwrap();drop(book);
            assert!(matches!(Book::open(&path,&key),Err(DbError::Integrity(_))),"{label}");
        }
    }

    #[test]
    fn write_verifies_pragmas_before_and_after_the_callback() {
        let path=temp("write-pragmas");let mut book=Book::create(&path,&[14;32]).unwrap();
        for (pragma,disabled,enabled) in [("foreign_keys","OFF","ON"),("recursive_triggers","OFF","ON"),("synchronous","OFF","FULL"),("ignore_check_constraints","ON","OFF"),("writable_schema","ON","OFF")] {
            book.conn.execute_batch(&format!("PRAGMA {pragma}={disabled}")).unwrap();
            let result:rusqlite::Result<()>=book.write(|_|panic!("callback ran with {pragma} disabled"));
            assert!(result.is_err());book.conn.execute_batch(&format!("PRAGMA {pragma}={enabled}")).unwrap();
        }
        let result:rusqlite::Result<()>=book.write(|tx| {tx.execute_batch("INSERT INTO fb_seal VALUES('rollback','x'); PRAGMA recursive_triggers=OFF")?;Ok(())});
        assert!(result.is_err());book.conn.execute_batch("PRAGMA recursive_triggers=ON").unwrap();
        assert_eq!(book.conn.query_row("SELECT count(*) FROM fb_seal",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    }

}

#[cfg(test)]
mod company_documents_upgrade_tests;
