//! Desktop invoice delivery. The host owns durable attempt/history records;
//! provider acceptance is returned even when the separate audit sink fails.
use crate::{COMPOSE_SCOPE, GmailSender, IoStats, mime};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use fb_plugin_api::*;
use std::time::Instant;
use zeroize::Zeroizing;

pub const INLINE_LOGO_CID: &str = "freedombooks-logo";

pub struct InlineImage {
    pub content_id: String,
    pub file: Attachment,
}
#[derive(Default)]
pub struct DeliveryContent {
    pub html: Option<SecretString>,
    pub reply_to: Option<Mailbox>,
    pub inline_logo: Option<Attachment>,
    pub inline_images: Vec<InlineImage>,
    /// A stable, host-generated identifier for this explicit delivery attempt.
    pub message_id: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryMode {
    Draft,
    Send,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliveryReceipt {
    pub message_id: String,
    pub draft_id: Option<String>,
    pub accepted_at: Timestamp,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryOutcome {
    NotSubmitted {
        error: PluginError,
    },
    Accepted {
        receipt: DeliveryReceipt,
        recording_error: Option<PluginError>,
    },
    Uncertain {
        error: PluginError,
    },
}
fn provider_id(id: &str) -> PluginResult<String> {
    if id.is_empty()
        || id.len() > 256
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err(PluginError::InvalidResponse);
    }
    Ok(id.to_owned())
}

impl GmailSender {
    /// One submission, never an automatic retry. Saving a draft is not sending.
    /// The caller must durably mark an attempt before calling and retain an
    /// uncertain result until the user checks Gmail.
    pub fn deliver_outcome(
        &self,
        cx: &PluginContext,
        document: OutgoingDocument,
        content: DeliveryContent,
        mode: DeliveryMode,
    ) -> DeliveryOutcome {
        let start = Instant::now();
        let mut stats = IoStats::default();
        let mut submitted = false;
        let mut accepted = None;
        let action = if mode == DeliveryMode::Draft {
            AuditAction::CreateDraft
        } else {
            AuditAction::Send
        };
        let result = (|| -> PluginResult<()> {
            self.check_scope(cx)?;
            if mode == DeliveryMode::Draft && self.oauth_scope != COMPOSE_SCOPE {
                return Err(PluginError::NeedsSignIn);
            }
            let _lock = self.state.lock().map_err(|_| PluginError::Core)?;
            let old = self.load_tokens(cx)?;
            let mime =
                mime::build_with_content(&old.from_address, &document, &content, cx.clock().now())?;
            let raw = Zeroizing::new(URL_SAFE_NO_PAD.encode(mime.expose()));
            // The serialized MIME is secret-bearing; do not leave a serde Value copy.
            #[derive(serde::Serialize)]
            struct Raw<'a> {
                raw: &'a str,
            }
            #[derive(serde::Serialize)]
            struct Draft<'a> {
                message: Raw<'a>,
            }
            let message = Raw { raw: &raw };
            let body = SecretBytes::new(
                if mode == DeliveryMode::Draft {
                    serde_json::to_vec(&Draft { message })
                } else {
                    serde_json::to_vec(&message)
                }
                .map_err(|_| PluginError::InvalidConfiguration)?,
            );
            let tokens = self.access_token(cx, old, &mut stats)?;
            cx.check_active()?;
            submitted = true;
            let endpoint = if mode == DeliveryMode::Draft {
                &self.endpoints.drafts
            } else {
                &self.endpoints.send
            };
            let response = self.request(
                cx,
                endpoint,
                "application/json",
                body,
                Some(&tokens.access),
                &mut stats,
            )?;
            match response.status {
                200 => (),
                401 => {
                    submitted = false;
                    cx.credentials().forget()?;
                    return Err(PluginError::NeedsSignIn);
                }
                400 | 403 | 404 | 413 | 429 => {
                    submitted = false;
                    return Err(match response.status {
                        403 => PluginError::OAuthDenied,
                        429 => PluginError::Network,
                        _ => PluginError::InvalidResponse,
                    });
                }
                500..=599 => return Err(PluginError::Network),
                _ => return Err(PluginError::InvalidResponse),
            }
            #[derive(serde::Deserialize)]
            struct Message {
                id: String,
            }
            #[derive(serde::Deserialize)]
            struct Delivered {
                id: String,
                message: Option<Message>,
            }
            let wire: Delivered = serde_json::from_slice(response.body.expose())
                .map_err(|_| PluginError::InvalidResponse)?;
            let (message_id, draft_id) = if mode == DeliveryMode::Draft {
                (
                    provider_id(&wire.message.ok_or(PluginError::InvalidResponse)?.id)?,
                    Some(provider_id(&wire.id)?),
                )
            } else {
                (provider_id(&wire.id)?, None)
            };
            accepted = Some(DeliveryReceipt {
                message_id,
                draft_id,
                accepted_at: cx.clock().now(),
            });
            cx.record_audit(
                action,
                AuditPurpose::DocumentDelivery,
                AuditOutcome::Success,
                (stats.sent, stats.received),
                start.elapsed(),
            )
        })();
        if let Some(receipt) = accepted {
            return DeliveryOutcome::Accepted {
                receipt,
                recording_error: result.err(),
            };
        }
        let error = result.expect_err("unaccepted delivery must fail");
        let outcome = match error {
            PluginError::NeedsSignIn => AuditOutcome::NeedsSignIn,
            PluginError::WrongScope
            | PluginError::OAuthDenied
            | PluginError::InvalidConfiguration
            | PluginError::TooLarge => AuditOutcome::Refused,
            _ => AuditOutcome::Failed,
        };
        let _ = cx.record_audit(
            action,
            AuditPurpose::DocumentDelivery,
            outcome,
            (stats.sent, stats.received),
            start.elapsed(),
        );
        if submitted {
            DeliveryOutcome::Uncertain { error }
        } else {
            DeliveryOutcome::NotSubmitted { error }
        }
    }
}
