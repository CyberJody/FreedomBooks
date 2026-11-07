//! Commands and queries as plain serializable data (ARCHITECTURE 4.3 and 8).
//!
//! Every call that crosses a transport (the Electron bridge today, perhaps a network host later) is one of
//! these enums, serialized as JSON with a `type` tag. Unknown types and unknown fields are refused by
//! serde (`deny_unknown_fields`) before anything touches the book, and `schema()` publishes the same
//! contract as JSON Schema so a shell can check shapes too. Adding a module = adding variants here and
//! arms in `session.rs`; the bridges don't change.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};


#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DirectoryKind { Customer, Vendor, Item }

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum DirectoryValues {
    Customer {
        display_name: String,
        business_name: String,
        contact_name: String,
        email: String,
        phone: String,
        billing_address: String,
        notes: String,
        tax_status: Option<String>,
        exemption_number: Option<String>,
    },
    Vendor {
        display_name: String,
        business_name: String,
        contact_name: String,
        email: String,
        phone: String,
        billing_address: String,
        notes: String,
        default_expense_account: Option<Id>,
        default_terms: Option<String>,
        expense_shown: bool,
        terms_shown: bool,
    },
    Item {
        code: String,
        description: String,
        item_kind: String,
        sale_rate: String,
        purchase_cost: Option<String>,
        income_account: Option<Id>,
        expense_account: Option<Id>,
        taxable: Option<bool>,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CustomerReceiptValues {
    pub request_key: String,
    pub customer_id: Id,
    pub date: String,
    pub method: String,
    pub reference: String,
    pub memo: String,
    pub destination: String,
    pub bank_id: Option<Id>,
    #[serde(default)]
    pub amount: Option<String>,
    #[serde(default)]
    pub tax_mode: Option<String>,
    #[serde(default)]
    pub tax_item_id: Option<Id>,
    #[serde(default)]
    pub lines: Vec<InvoiceLineValues>,
    #[serde(default)]
    pub allocations: Vec<ReceiptAllocationValues>,
}
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReceiptAllocationValues {
    pub invoice_id: Id,
    pub amount: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BeginReconciliationValues {
    pub statement_date: String, pub ending_balance: String,
    pub service_charge: String, pub service_account_id: Option<Id>,
    pub interest: String, pub interest_account_id: Option<Id>,
    pub request_key: String, pub expected_revision: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CompanyDocumentValues {
    #[serde(default)]
    pub street1: String,
    #[serde(default)]
    pub street2: String,
    #[serde(default)]
    pub city: String,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub postal_code: String,
    #[serde(default)]
    pub country: String,
    #[serde(default)]
    pub phone: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub website: String,
    #[serde(default)]
    pub tax_id: String,
    #[serde(default)]
    pub payment_instructions: String,
    pub logo_upload: Option<String>,
    #[serde(default)]
    pub remove_logo: bool,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StarterChart {
    #[default]
    Empty,
    Standard,
}
impl From<StarterChart> for fb_core::starter_chart::Choice {
    fn from(value: StarterChart) -> Self {
        match value { StarterChart::Empty => Self::Empty, StarterChart::Standard => Self::Standard }
    }
}

/// Ids travel as the core's canonical UUID text.
pub type Id = String;
fn yes() -> bool { true }

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct JournalLineValues { pub account: Id, pub debit: String, pub credit: String, pub description: String, pub name: String }


#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum Command {
    ExportBankRegister { company_id: Id, account_id: Id, filters: Vec<(String,String)> },
    ExportReconciliation { company_id: Id, reconciliation_id: Id, filters: Vec<(String,String)> },
    MarkBankLine { company_id: Id, account_id: Id, line_id: Id, state: String, expected_revision: Option<i64> },
    BeginReconciliation { company_id: Id, account_id: Id, values: BeginReconciliationValues },
    SaveReconciliationTicks { company_id: Id, account_id: Id, reconciliation_id: Id, expected_revision: i64, shown: std::collections::BTreeMap<String, i64>, ticked: Vec<Id>, hide_later: Option<bool> },
    FinishReconciliation { company_id: Id, account_id: Id, reconciliation_id: Id, expected_revision: i64, completion_key: String },
    UndoReconciliation { company_id: Id, account_id: Id, reconciliation_id: Id, expected_revision: Option<i64>, reason: String, undo_key: String },
    SaveCheck { company_id: Id, values: CheckValues },
    ReviewCheck { company_id: Id, check_id: Id, expected_revision: i64 },
    RecordCheck { company_id: Id, check_id: Id, expected_revision: i64, review_token: String },
    AssignCheckNumber { company_id: Id, check_id: Id, expected_revision: i64, check_number: String },
    VoidCheck { company_id: Id, check_id: Id, expected_revision: i64, date: String, reason: String },
    ReviewDeposit { company_id: Id, values: DepositValues },
    RecordDeposit { company_id: Id, values: DepositValues, review_token: String },
    ReverseDeposit { company_id: Id, deposit_id: Id, expected_revision: i64, date: String, reason: String },
    ReviewCreditMemo { company_id: Id, values: CreditMemoValues },
    RecordCreditMemo { company_id: Id, values: CreditMemoValues, review_token: String },
    ReviewCustomerDeposit { company_id: Id, values: CustomerDepositValues },
    RecordCustomerDeposit { company_id: Id, values: CustomerDepositValues, review_token: String },
    ReviewCreditApplication { company_id: Id, source_id: Id, values: CreditApplicationValues },
    RecordCreditApplication { company_id: Id, source_id: Id, values: CreditApplicationValues, review_token: String },
    ReviewDepositApplication { company_id: Id, source_id: Id, values: DepositApplicationValues },
    RecordDepositApplication { company_id: Id, source_id: Id, values: DepositApplicationValues, review_token: String },
    ReviewCustomerRefund { company_id: Id, source_id: Id, deposit: bool, values: CustomerRefundValues },
    RecordCustomerRefund { company_id: Id, source_id: Id, deposit: bool, values: CustomerRefundValues, review_token: String },
    VoidCreditMemo { company_id: Id, record_id: Id, expected_revision: i64, date: String, reason: String },
    VoidCustomerDeposit { company_id: Id, record_id: Id, expected_revision: i64, date: String, reason: String },
    UnapplyCredit { company_id: Id, record_id: Id, expected_revision: i64, date: String, reason: String },
    UnapplyDeposit { company_id: Id, record_id: Id, expected_revision: i64, date: String, reason: String },
    VoidCustomerRefund { company_id: Id, record_id: Id, deposit: bool, expected_revision: i64, date: String, reason: String },
    ReviewCustomerReceipt {
        company_id: Id,
        sales_receipt: bool,
        values: CustomerReceiptValues,
    },
    RecordCustomerReceipt {
        company_id: Id,
        sales_receipt: bool,
        values: CustomerReceiptValues,
        review_token: String,
    },
    ReverseCustomerReceipt {
        company_id: Id,
        receipt_id: Id,
        sales_receipt: bool,
        expected_revision: i64,
        date: String,
        reason: String,
    },
    SaveEstimate {
        company_id: Id,
        estimate_id: Option<Id>,
        expected_revision: i64,
        expiry: Option<String>,
        values: InvoiceValues,
    },
    TransitionEstimate {
        company_id: Id,
        estimate_id: Id,
        expected_revision: i64,
        state: String,
        reason: String,
    },
    ConvertEstimate {
        company_id: Id,
        estimate_id: Id,
        expected_revision: i64,
        date: String,
        due_date: String,
    },
    SaveInvoiceDraft {
        company_id: Id,
        invoice_id: Option<Id>,
        expected_revision: i64,
        values: InvoiceValues,
    },
    PostReviewedInvoice {
        company_id: Id,
        invoice_id: Id,
        expected_revision: i64,
        review_token: String,
    },
    VoidInvoice {
        company_id: Id,
        invoice_id: Id,
        expected_revision: i64,
        date: String,
        reason: String,
    },
    SaveBill { company_id: Id, values: BillValues },
    ReviewBill { company_id: Id, bill_id: Id, expected_revision: i64 },
    PostBill { company_id: Id, bill_id: Id, expected_revision: i64, review_token: String },
    VoidBill { company_id: Id, bill_id: Id, expected_revision: i64, date: String, reason: String },
    ReviewBillPayment { company_id: Id, values: BillPaymentValues },
    RecordBillPayment { company_id: Id, values: BillPaymentValues, review_token: String },
    ReverseBillPayment { company_id: Id, payment_id: Id, expected_revision: i64, date: String, reason: String },
    ReviewTransfer { company_id: Id, values: TransferValues },
    RecordTransfer { company_id: Id, values: TransferValues, review_token: String },
    VoidTransfer { company_id: Id, transfer_id: Id, expected_revision: i64, date: String, reason: String },


    SaveDirectoryRecord {
        company_id: Id,
        record_id: Option<Id>,
        values: DirectoryValues,
    },
    SetDirectoryActive {
        company_id: Id,
        kind: DirectoryKind,
        record_id: Id,
        active: bool,
    },
    SaveTaxRate {
        company_id: Id,
        item_id: Option<Id>,
        expected_revision: Option<i64>,
        name: String,
        description: String,
        rate: String,
        agency_id: Option<Id>,
        new_agency_name: String,
    },
    SaveTaxGroup {
        company_id: Id,
        item_id: Option<Id>,
        expected_revision: Option<i64>,
        name: String,
        description: String,
        member_ids: Vec<Id>,
    },
    SaveTaxSettings {
        company_id: Id,
        expected_revision: i64,
        charge_tax: bool,
        default_item_id: Option<Id>,
        owed_basis: String,
        filing_frequency: String,
    },
    SetTaxItemActive {
        company_id: Id,
        item_id: Id,
        active: bool,
        expected_revision: i64,
    },
    FinishTaxBulkStep {
        company_id: Id,
        customers: bool,
        items: bool,
        digest: String,
    },
    SaveReminderSettings {
        company_id: Id,
        days: i64,
        expected: i64,
    },

    ExportReport {
        company_id: String,
        report: String,
        filters: Vec<(String, String)>,
    },
    ExportHistory {
        company_id: Id,
        filters: Vec<(String, String)>,
    },
    /// DESKTOP-BACKUPS: the Backups page's Download link; the main process asks where to save the copy.
    ExportBackup {
        company_id: Id,
        name: String,
    },
    SaveCompanyDetails {
        company_id: Id,
        name: String,
        currency: String,
        values: CompanyDocumentValues,
    },
    SetNextNumber {
        company_id: Id,
        kind: String,
        bank_id: Option<Id>,
        next: String,
        expected_next: String,
        expected_revision: i64,
    },
    SaveCompany {
        company_id: Option<Id>,
        name: String,
        currency: String,
        #[serde(default)]
        starter_chart: StarterChart,
    },
    SaveJournal {
        company_id: Id,
        journal_id: Option<Id>,
        date: String,
        memo: String,
        entry_number: String,
        adjusting: bool,
        request_key: Option<String>,
        expected_revision: Option<i64>,
        lines: Vec<JournalLineValues>,
    },
    PostJournal {
        company_id: Id,
        journal_id: Id,
        expected_revision: i64,
    },
    ReverseJournal {
        company_id: Id,
        journal_id: Id,
        date: String,
        reason: String,
    },
    FillAccountingSetup {
        company_id: Id,
        digest: String,
        expected_revision: Option<i64>,
    },
    DismissSetup {
        company_id: Id,
    },
    /// Chart of accounts: create an account (`ledger.services.save_account`).
    SaveAccount {
        company_id: Id,
        #[serde(default)]
        account_id: Option<Id>,
        #[serde(default = "yes")]
        active: bool,
        #[serde(default = "yes")]
        postable: bool,
        code: String,
        name: String,
        detail_type: String,
        #[serde(default)]
        is_contra: bool,
        #[serde(default)]
        description: String,
    },
    /// Directory: create a customer (`directory.services.save_record`, model customer).
    SaveCustomer { company_id: Id, display_name: String },
    /// Directory: create an item (`directory.services.save_record`, model item).
    SaveItem {
        company_id: Id,
        code: String,
        description: String,
        kind: String,
        #[serde(default)]
        sale_rate: Option<String>,
        #[serde(default)]
        income_account_id: Option<Id>,
        #[serde(default)]
        expense_account_id: Option<Id>,
    },
    /// Accounting setup: control accounts and designated banks (`documents.services.save_setup`).
    SaveSetup {
        company_id: Id,
        expected_revision: i64,
        receivables_id: Option<Id>,
        undeposited_id: Option<Id>,
        #[serde(default)]
        payables_id: Option<Id>,
        #[serde(default)]
        customer_deposits_id: Option<Id>,
        #[serde(default)]
        collected_tax_id: Option<Id>,
        #[serde(default)]
        bank_ids: Vec<Id>,
    },
    /// Invoices: save a draft (`sales.services.save_invoice`). Lines are the service's own line values.
    SaveInvoice {
        company_id: Id,
        values: InvoiceValues,
    },
    /// Explicit service-level review-and-post command. Desktop forms use PostReviewedInvoice
    /// with the capability from the displayed review.
    PostInvoice {
        company_id: Id,
        invoice_id: Id,
        expected_revision: i64,
    },
    /// A ported HTML form's submission. `path` must be a form route the app renders and `token` the
    /// single-use command token minted when that screen was rendered (it replaces Django's CSRF token).
    SubmitForm { path: String, token: String, fields: Vec<(String, String)> },
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InvoiceValues {
    pub request_key: String,
    pub customer_id: Id,
    pub date: String,
    #[serde(default)]
    pub due_date: Option<String>,
    #[serde(default)]
    pub memo: String,
    pub tax_mode: String,
    #[serde(default)]
    pub tax_rate: Option<String>,
    #[serde(default)]
    pub tax_label: Option<String>,
    #[serde(default)]
    pub tax_item_id: Option<Id>,
    #[serde(default)]
    pub refresh_customer: bool,
    #[serde(default)]
    pub keep_archived: bool,
    pub lines: Vec<InvoiceLineValues>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InvoiceLineValues {
    #[serde(default)]
    pub origin_position: Option<i64>,
    #[serde(default)]
    pub item_id: Option<Id>,
    pub description: String,
    pub quantity: String,
    pub rate: String,
    #[serde(default)]
    pub taxable: bool,
    #[serde(default)]
    pub refresh_item: bool,
    #[serde(default)]
    pub keep_archived: bool,
    #[serde(default)]
    pub income_account_id: Option<Id>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum Query {
    /// The open book's companies (id, name, currency).
    Companies,
    /// Current company logo, served only through the authenticated local image route.
    CompanyLogo { company_id: Id },
    NumberingSettings { company_id: Id },
    /// A company's accounts in chart order.
    Accounts { company_id: Id },
    /// An account register (posted lines, running balance).
    Register { company_id: Id, account_id: Id, #[serde(default)] start: Option<String>, #[serde(default)] end: Option<String> },
    /// Current invoice balances (state, totals, outstanding).
    Invoices { company_id: Id },
    /// The JSON Schema of this contract.
    Schema,
}

/// Outcome of a command: data, or where the window should go next (a ported form's redirect), or a
/// re-rendered form with its errors (Django's 400 response).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    Done { data: serde_json::Value },
    Redirect { location: String },
    Invalid { html: String },
    Page { html: String, status: u16 },
    Export { filename: String, contents: String },
    /// Open `url` (an https address fb-app built, e.g. the ChatGPT sign-in) in the system browser, then
    /// go to `location`. Only the main process acts on it; the renderer never navigates outside fb://.
    External { url: String, location: String },
}

/// The command and query contract as JSON Schema.
pub fn schema() -> serde_json::Value {
    serde_json::json!({
        "command": schemars::schema_for!(Command),
        "query": schemars::schema_for!(Query),
    })
}

/// Largest JSON a caller may send; a ported form of 50 lines is ~40 KB.
pub const MAX_REQUEST_BYTES: usize = 256 * 1024;

pub fn parse_command(json: &str) -> Result<Command, String> {
    if json.len() > MAX_REQUEST_BYTES {
        return Err("Request too large.".into());
    }
    fb_db::parse_unique_json_as::<Command>(json).map_err(|e| format!("Invalid command: {e}"))
}

pub fn parse_query(json: &str) -> Result<Query, String> {
    if json.len() > MAX_REQUEST_BYTES {
        return Err("Request too large.".into());
    }
    fb_db::parse_unique_json_as::<Query>(json).map_err(|e| format!("Invalid query: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_types_and_fields_are_refused() {
        assert!(parse_command(r#"{"type":"SaveCustomer","company_id":"c","display_name":"A"}"#).is_ok());
        assert!(parse_command(r#"{"type":"SaveCustomer","company_id":"c","display_name":"A","admin":true}"#).is_err());
        assert!(parse_command(r#"{"type":"DropTables"}"#).is_err());
        assert!(parse_command(r#"{"type":"SaveCustomer","company_id":"c","display_name":"A","display_name":"B"}"#).is_err(), "duplicate keys");
        assert!(parse_query(r#"{"type":"Invoices","company_id":"c"}"#).is_ok());
        assert!(parse_command(&"x".repeat(MAX_REQUEST_BYTES + 1)).is_err());
    }

    #[test]
    fn schema_names_every_command() {
        let text = schema().to_string();
        for name in ["SaveAccount", "SaveCustomer", "SaveItem", "SaveSetup", "SaveInvoice", "PostInvoice", "SubmitForm", "Register", "SaveCompany", "SaveJournal", "PostJournal", "ReverseJournal", "FillAccountingSetup", "DismissSetup", "ExportHistory"] {
            assert!(text.contains(name), "{name} missing from schema");
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BillValues {
    pub bill_id: Option<Id>, pub expected_revision: i64, pub request_key: String,
    pub vendor_id: Id, pub date: String, pub due_date: String, pub memo: String,
    pub reference: String, pub supersedes_id: Option<Id>, pub reentry_reason: String,
    pub refresh_vendor: bool, pub keep_archived: bool, pub lines: Vec<BillLineValues>,
}
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BillLineValues {
    pub origin_position: Option<i64>, pub item_id: Option<Id>, pub expense_account_id: Option<Id>,
    pub description: String, pub quantity: String, pub rate: String,
    pub refresh_item: bool, pub keep_archived: bool,
}
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BillAllocationValues { pub bill_id: Id, pub amount: String }
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BillPaymentValues {
    pub request_key: String, pub vendor_id: Id, pub date: String, pub amount: String,
    pub method: String, pub reference: String, pub memo: String, pub bank_id: Id,
    pub allocations: Vec<BillAllocationValues>,
}
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TransferValues {
    pub request_key: String, pub date: String, pub from_account_id: Id,
    pub to_account_id: Id, pub amount: String, pub memo: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CheckValues {
    pub check_id: Option<Id>, pub expected_revision: i64, pub request_key: String,
    pub bank_id: Id, pub date: String, pub check_number: String, pub print_later: bool,
    pub payee_kind: String, pub vendor_id: Option<Id>, pub payee_name: String,
    pub address: String, pub memo: String, pub refresh_payee: bool, pub keep_archived: bool,
    pub lines: Vec<CheckLineValues>,
}
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CheckLineValues {
    pub tab: String, pub origin_position: Option<i64>, pub account_id: Option<Id>, pub item_id: Option<Id>,
    pub description: String, pub quantity: String, pub rate: String,
    pub refresh_item: bool, pub keep_archived: bool,
}
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DepositValues {
    pub request_key: String, pub bank_id: Id, pub date: String,
    pub reference: String, pub memo: String, pub receipt_ids: Vec<Id>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CreditMemoValues { pub request_key: String, pub customer_id: Id, pub date: String, pub memo: String, pub tax_mode: String, pub tax_item_id: Option<Id>, pub lines: Vec<InvoiceLineValues> }

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CustomerDepositValues { pub request_key: String, pub customer_id: Id, pub date: String, pub amount: String, pub method: String, pub reference: String, pub destination: String, pub bank_id: Option<Id>, pub estimate_id: Option<Id>, pub order_reference: String, pub memo: String }

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CreditApplicationValues { pub request_key: String, pub date: String, pub allocations: Vec<ReceiptAllocationValues> }

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DepositApplicationValues { pub request_key: String, pub date: String, pub amount: String, pub invoice_id: Id }

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CustomerRefundValues { pub request_key: String, pub date: String, pub amount: String, pub bank_id: Id, pub method: String, pub check_number: String, pub memo: String }
