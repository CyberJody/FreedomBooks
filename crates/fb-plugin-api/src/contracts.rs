//! Additive rich results. Legacy slots remain source-compatible, not feature-complete.
use crate::*;

/// This is submission state, not delivery state. No variant authorizes automatic retry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SendOutcome {
    NotSubmitted {
        error: PluginError,
    },
    Accepted {
        receipt: SendReceipt,
        recording_error: Option<PluginError>,
    },
    Uncertain {
        error: PluginError,
    },
}

/// Metadata extension rather than a new required field on every legacy AuditEvent.
/// Construct only with provider-validated opaque identifiers, never response text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentSendAudit {
    pub event: AuditEvent,
    pub document: DocumentRef,
    pub receipt: SendReceipt,
}
impl DocumentSendAudit {
    pub fn validate(&self) -> PluginResult<()> {
        if self.event.action != AuditAction::Send
            || self.event.purpose != AuditPurpose::DocumentDelivery
            || self.event.outcome != AuditOutcome::Success
            || self.document.id.is_nil()
            || self.receipt.provider_message_id.as_ref().is_some_and(|id| {
                id.is_empty() || id.len() > 512 || !id.bytes().all(|b| b.is_ascii_graphic())
            })
        {
            return Err(PluginError::InvalidConfiguration);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeedCompleteness {
    Complete,
    Partial,
    /// A legacy Vec result says nothing about provider completeness or inventory.
    Unknown,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeedWarningCode {
    AuthenticationRequired,
    SubscriptionRequired,
    ProviderUnavailable,
    InvalidAccount,
    InvalidTransaction,
    MissingIdentifier,
    UnsupportedCurrency,
    InvalidAmount,
    InvalidDate,
    Other,
}
/// Closed warning codes deliberately exclude arbitrary provider error messages.
/// Identifiers are provider-normalized data, not log strings or credentials.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeedWarning {
    pub code: FeedWarningCode,
    pub account_remote_id: Option<String>,
    pub transaction_remote_id: Option<String>,
    pub connection_remote_id: Option<String>,
    pub source_index: Option<usize>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeedAccountMetadata {
    pub account_remote_id: String,
    pub organization_remote_id: Option<String>,
    pub organization_name: Option<String>,
    pub connection_remote_id: Option<String>,
    pub available_balance: Option<Amount>,
    pub balance_at: Option<Timestamp>,
}
pub struct FeedResult<T> {
    pub items: Vec<T>,
    pub completeness: FeedCompleteness,
    /// None means the provider/legacy adapter did not supply account inventory.
    /// Some(empty) is a known empty inventory, never interchangeable with None.
    pub present_account_ids: Option<Vec<String>>,
    pub account_metadata: Vec<FeedAccountMetadata>,
    pub warnings: Vec<FeedWarning>,
}
impl<T> FeedResult<T> {
    pub fn legacy(items: Vec<T>) -> Self {
        Self {
            items,
            completeness: FeedCompleteness::Unknown,
            present_account_ids: None,
            account_metadata: vec![],
            warnings: vec![],
        }
    }
    /// Conservative guard for host pending-withdrawal reconciliation. The host
    /// must also confirm that this pull covers its intended account/date scope.
    pub fn has_complete_inventory(&self) -> bool {
        self.completeness == FeedCompleteness::Complete
            && self.warnings.is_empty()
            && self.present_account_ids.is_some()
    }
}
