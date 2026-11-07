//! Read-only SimpleFIN BankFeed provider. Never matches, accepts, or posts a transaction.
#![forbid(unsafe_code)]
pub mod client;
pub mod parse;

use chrono_tz::Tz;
use fb_plugin_api::*;
use std::time::Instant;

pub static MANIFEST: PluginManifest = PluginManifest {
    id: "simplefin",
    name: "SimpleFIN bank feeds",
    version: env!("CARGO_PKG_VERSION"),
    slots: &[Slot::BankFeed],
    auth: AuthKind::SetupToken,
    outside_account: "Your own SimpleFIN Bridge connection; paste a fresh setup token once. If a token was used without your knowledge, disable it at SimpleFIN and create a new one. Forgetting here does not revoke the connection at SimpleFIN.",
    scopes: &["Read account balances and transactions (no payment initiation; no OAuth scopes)"],
    data_reads: &[DataKind::BankData],
    data_sends: &[DataKind::BankData],
    settings_fields: &[SettingsField { key: "setup_token", label: "One-time setup token", kind: FieldKind::Secret, required: true }],
    // The public Bridge setup flow currently issues claims on beta-bridge as well.
    network_hosts: &["bridge.simplefin.org", "beta-bridge.simplefin.org"],
};

/// Construct with the book's display timezone, NOT the machine timezone. This preserves
/// Python's epoch-to-local-accounting-date conversion, including daylight-saving changes.
pub struct SimpleFin {
    zone: Tz,
}
impl SimpleFin {
    pub fn new(display_time_zone: &str) -> PluginResult<Self> {
        Ok(Self {
            zone: display_time_zone
                .parse()
                .map_err(|_| PluginError::InvalidConfiguration)?,
        })
    }

    /// The book display zone this instance converts epochs in.
    pub fn zone(&self) -> Tz {
        self.zone
    }

    /// Protocol-specific API retains full epochs and sanitized diagnostics beyond the
    /// neutral BankFeed envelope. No staging occurs; this is not a separate sync path.
    pub fn fetch_account_set(
        &self,
        cx: &PluginContext,
        connection: &FeedConnection,
        options: client::FetchOptions,
    ) -> PluginResult<parse::AccountSet> {
        audited(cx, AuditAction::Fetch, AuditPurpose::BankSync, |received| {
            check_connection(cx, connection)?;
            client::fetch(cx, &options, received)
        })
    }

    /// Explicit alternative to BankFeed::fetch + host staging. Call ONE path, not both.
    /// The scoped bridge owns deduplication; these remain staged, never posted entries.
    pub fn fetch_and_stage(
        &self,
        cx: &PluginContext,
        connection: &FeedConnection,
        window: DateWindow,
    ) -> PluginResult<StagedFeed> {
        audited(cx, AuditAction::Stage, AuditPurpose::BankSync, |_| {
            let transactions = self.fetch(cx, connection, window)?;
            cx.core().stage_bank_transactions(transactions)
        })
    }
}
impl Plugin for SimpleFin {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }
}
impl BankFeed for SimpleFin {
    fn connect(&self, cx: &PluginContext, input: ConnectInput) -> PluginResult<FeedConnection> {
        audited(
            cx,
            AuditAction::Connect,
            AuditPurpose::Connection,
            |received| {
                let raw = std::str::from_utf8(input.fields.expose())
                    .map_err(|_| PluginError::InvalidConfiguration)?;
                if raw.len() > 16 * 1024 {
                    return Err(PluginError::InvalidConfiguration);
                }
                client::claim(cx, &SecretString::from(raw), received)?;
                Ok(FeedConnection {
                    id: cx.connection_id(),
                })
            },
        )
    }
    fn list_accounts(
        &self,
        cx: &PluginContext,
        connection: &FeedConnection,
    ) -> PluginResult<Vec<FeedAccount>> {
        audited(
            cx,
            AuditAction::ListAccounts,
            AuditPurpose::BankSync,
            |received| {
                check_connection(cx, connection)?;
                let parsed = client::fetch(
                    cx,
                    &client::FetchOptions {
                        balances_only: true,
                        ..Default::default()
                    },
                    received,
                )?;
                parsed.require_complete()?;
                Ok(parsed.feed_accounts())
            },
        )
    }
    fn fetch(
        &self,
        cx: &PluginContext,
        connection: &FeedConnection,
        window: DateWindow,
    ) -> PluginResult<Vec<FeedTransaction>> {
        audited(cx, AuditAction::Fetch, AuditPurpose::BankSync, |received| {
            check_connection(cx, connection)?;
            let (start, end) = parse::window_epochs(window, self.zone)?;
            let parsed = client::fetch(
                cx,
                &client::FetchOptions {
                    start_epoch: Some(start),
                    end_epoch: Some(end),
                    ..Default::default()
                },
                received,
            )?;
            parsed.require_complete()?;
            Ok(parsed.feed_transactions(self.zone))
        })
    }
    fn list_accounts_result(
        &self,
        cx: &PluginContext,
        connection: &FeedConnection,
    ) -> PluginResult<FeedResult<FeedAccount>> {
        audited(
            cx,
            AuditAction::ListAccounts,
            AuditPurpose::BankSync,
            |received| {
                check_connection(cx, connection)?;
                let parsed = client::fetch(
                    cx,
                    &client::FetchOptions {
                        balances_only: true,
                        ..Default::default()
                    },
                    received,
                )?;
                Ok(parsed.feed_result(parsed.feed_accounts()))
            },
        )
    }
    fn fetch_result(
        &self,
        cx: &PluginContext,
        connection: &FeedConnection,
        window: DateWindow,
    ) -> PluginResult<FeedResult<FeedTransaction>> {
        audited(cx, AuditAction::Fetch, AuditPurpose::BankSync, |received| {
            check_connection(cx, connection)?;
            let (start, end) = parse::window_epochs(window, self.zone)?;
            let parsed = client::fetch(
                cx,
                &client::FetchOptions {
                    start_epoch: Some(start),
                    end_epoch: Some(end),
                    ..Default::default()
                },
                received,
            )?;
            Ok(parsed.feed_result(parsed.feed_transactions(self.zone)))
        })
    }
    fn forget(&self, cx: &PluginContext, connection: &FeedConnection) -> PluginResult<()> {
        audited(
            cx,
            AuditAction::Disconnect,
            AuditPurpose::Connection,
            |_| {
                check_connection(cx, connection)?;
                // SimpleFIN has no protocol revocation endpoint. No request, no deletion of staged data.
                cx.credentials().forget()
            },
        )
    }
}
fn check_connection(cx: &PluginContext, connection: &FeedConnection) -> PluginResult<()> {
    if connection.id != cx.connection_id() {
        return Err(PluginError::WrongScope);
    }
    Ok(())
}
fn audited<T>(
    cx: &PluginContext,
    action: AuditAction,
    purpose: AuditPurpose,
    operation: impl FnOnce(&mut u64) -> PluginResult<T>,
) -> PluginResult<T> {
    let start = Instant::now();
    let mut received = 0;
    let result = if cx.scope().plugin_id != MANIFEST.id {
        Err(PluginError::WrongScope)
    } else {
        operation(&mut received)
    };
    let outcome = match &result {
        Ok(_) => AuditOutcome::Success,
        Err(PluginError::NeedsSignIn) => AuditOutcome::NeedsSignIn,
        Err(
            PluginError::WrongScope
            | PluginError::HostNotAllowed
            | PluginError::PrivateAddress
            | PluginError::HttpsRequired
            | PluginError::InvalidConfiguration
            | PluginError::SetupTokenRefused
            | PluginError::ProviderSubscriptionRequired,
        ) => AuditOutcome::Refused,
        Err(_) => AuditOutcome::Failed,
    };
    cx.record_audit(action, purpose, outcome, (0, received), start.elapsed())?;
    result
}
