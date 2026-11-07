//! TLS-only SMTP document delivery. The host renders and records documents via CoreBridge.
#![forbid(unsafe_code)]

mod message;
mod transport;

use fb_plugin_api::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 10 * 1024 * 1024;
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

pub static MANIFEST: PluginManifest = PluginManifest {
    id: "smtp",
    name: "SMTP email",
    version: env!("CARGO_PKG_VERSION"),
    slots: &[Slot::DocumentSender],
    auth: AuthKind::Password,
    outside_account: "Your mail provider's SMTP account (username and app password)",
    scopes: &["smtp.send"],
    data_reads: &[DataKind::DocumentContents, DataKind::RecipientAddresses],
    data_sends: &[DataKind::DocumentContents, DataKind::RecipientAddresses],
    settings_fields: &[
        SettingsField {
            key: "host",
            label: "SMTP server (explicit consent required)",
            kind: FieldKind::Text,
            required: true,
        },
        SettingsField {
            key: "port",
            label: "SMTP port",
            kind: FieldKind::Port,
            required: true,
        },
        SettingsField {
            key: "security",
            label: "Encryption",
            kind: FieldKind::Choice(&["starttls", "implicit-tls"]),
            required: true,
        },
        SettingsField {
            key: "from",
            label: "Sender email address",
            kind: FieldKind::Text,
            required: true,
        },
        SettingsField {
            key: "username",
            label: "SMTP username",
            kind: FieldKind::Secret,
            required: true,
        },
        SettingsField {
            key: "app_password",
            label: "App password (provider must permit SMTP authentication)",
            kind: FieldKind::Secret,
            required: true,
        },
    ],
    // No wildcard: the host must add the explicitly consented connection hostname
    // to cx.http().policy(). The SMTP socket uses that policy too.
    network_hosts: &[],
};

/// There is intentionally no plaintext or opportunistic-TLS mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Security {
    StartTls,
    ImplicitTls,
}

pub struct SmtpConfig {
    host: String,
    port: u16,
    security: Security,
    from: String,
    max_message_bytes: usize,
    timeout: Duration,
}
impl SmtpConfig {
    pub fn new(host: String, port: u16, security: Security, from: String) -> PluginResult<Self> {
        // DNS names and IPv4 only; no URL, userinfo, path, bracket or port injection.
        if host.is_empty()
            || host.len() > 253
            || port == 0
            || port == 25
            || host.split('.').any(|label| {
                label.is_empty()
                    || label.len() > 63
                    || label.starts_with('-')
                    || label.ends_with('-')
                    || !label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        {
            return Err(PluginError::InvalidConfiguration);
        }
        message::mailbox(&from, None)?;
        Ok(Self {
            host: host.to_ascii_lowercase(),
            port,
            security,
            from,
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
            timeout: DEFAULT_TIMEOUT,
        })
    }

    /// Limits may be lowered; hard ceilings are 20 MiB and two minutes.
    pub fn with_limits(mut self, bytes: usize, timeout: Duration) -> PluginResult<Self> {
        if bytes == 0
            || bytes > 20 * 1024 * 1024
            || timeout.is_zero()
            || timeout > Duration::from_secs(120)
        {
            return Err(PluginError::InvalidConfiguration);
        }
        self.max_message_bytes = bytes;
        self.timeout = timeout;
        Ok(self)
    }
}

pub struct SmtpSender {
    config: SmtpConfig,
    #[cfg(test)]
    test_endpoint: Option<transport::TestEndpoint>,
}
impl SmtpSender {
    pub fn new(config: SmtpConfig) -> Self {
        Self {
            config,
            #[cfg(test)]
            test_endpoint: None,
        }
    }

    /// Save only through the host's scoped/encrypted store. This does not contact SMTP.
    /// Host connection state/consent is managed separately in wave 4.
    pub fn save_credentials(
        cx: &PluginContext,
        username: SecretString,
        app_password: SecretString,
    ) -> PluginResult<()> {
        check_context(cx)?;
        validate_credential(username.expose_secret())?;
        validate_credential(app_password.expose_secret())?;
        let mut record = zeroize::Zeroizing::new(b"SMTP1\0".to_vec());
        record.extend_from_slice(username.expose_secret().as_bytes());
        record.push(0);
        record.extend_from_slice(app_password.expose_secret().as_bytes());
        cx.credentials()
            .save(SecretBytes::new(std::mem::take(&mut *record)))
    }

    fn credentials(cx: &PluginContext) -> PluginResult<(SecretString, SecretString)> {
        let record = cx.credentials().load()?.ok_or(PluginError::NeedsSignIn)?;
        let text = std::str::from_utf8(record.expose()).map_err(|_| PluginError::NeedsSignIn)?;
        let (user, password) = text
            .strip_prefix("SMTP1\0")
            .and_then(|value| value.split_once('\0'))
            .ok_or(PluginError::NeedsSignIn)?;
        validate_credential(user).map_err(|_| PluginError::NeedsSignIn)?;
        validate_credential(password).map_err(|_| PluginError::NeedsSignIn)?;
        Ok((SecretString::from(user), SecretString::from(password)))
    }
}
fn validate_credential(value: &str) -> PluginResult<()> {
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        Err(PluginError::InvalidConfiguration)
    } else {
        Ok(())
    }
}
fn check_context(cx: &PluginContext) -> PluginResult<()> {
    if cx.scope().plugin_id != MANIFEST.id {
        Err(PluginError::WrongScope)
    } else {
        cx.check_active()
    }
}
impl Plugin for SmtpSender {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }
}
impl DocumentSender for SmtpSender {
    fn send(&self, cx: &PluginContext, document: OutgoingDocument) -> PluginResult<SendReceipt> {
        match self.send_outcome(cx, document) {
            SendOutcome::Accepted {
                receipt,
                recording_error: None,
            } => Ok(receipt),
            SendOutcome::Accepted {
                recording_error: Some(error),
                ..
            }
            | SendOutcome::NotSubmitted { error }
            | SendOutcome::Uncertain { error } => Err(error),
        }
    }
    fn send_outcome(&self, cx: &PluginContext, document: OutgoingDocument) -> SendOutcome {
        // Do not write an SMTP event into another plugin's scoped audit stream.
        if let Err(error) = check_context(cx) {
            return SendOutcome::NotSubmitted { error };
        }
        let start = Instant::now();
        let submitted = AtomicBool::new(false);
        let reference = document.document;
        let mut bytes_sent = 0;
        let result = (|| {
            let message = message::prepare(&self.config, document, cx.clock().now())?;
            let credentials = Self::credentials(cx)?;
            transport::deliver(self, cx, &message, credentials, start, &submitted)?;
            bytes_sent = message.bytes.expose().len() as u64;
            Ok(SendReceipt {
                provider_message_id: None,
                sent_at: cx.clock().now(),
            })
        })();
        match result {
            Ok(receipt) => {
                let recording_error = cx
                    .record_document_send_audit(DocumentSendAudit {
                        event: AuditEvent {
                            scope: cx.scope().clone(),
                            actor: cx.actor(),
                            at: receipt.sent_at,
                            action: AuditAction::Send,
                            purpose: AuditPurpose::DocumentDelivery,
                            outcome: AuditOutcome::Success,
                            bytes_sent,
                            bytes_received: 0,
                            duration: start.elapsed(),
                        },
                        document: reference,
                        receipt: receipt.clone(),
                    })
                    .err();
                SendOutcome::Accepted {
                    receipt,
                    recording_error,
                }
            }
            Err(error) => {
                let outcome = match error {
                    PluginError::NeedsSignIn => AuditOutcome::NeedsSignIn,
                    PluginError::InvalidConfiguration
                    | PluginError::HostNotAllowed
                    | PluginError::PrivateAddress
                    | PluginError::OutboundTooLarge => AuditOutcome::Refused,
                    _ => AuditOutcome::Failed,
                };
                let _ = cx.record_audit(
                    AuditAction::Send,
                    AuditPurpose::DocumentDelivery,
                    outcome,
                    (bytes_sent, 0),
                    start.elapsed(),
                );
                if submitted.load(Ordering::SeqCst) {
                    SendOutcome::Uncertain { error }
                } else {
                    SendOutcome::NotSubmitted { error }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
