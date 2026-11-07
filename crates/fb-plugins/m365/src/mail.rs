use base64::{engine::general_purpose::STANDARD, Engine};
use fb_plugin_api::*;
use serde::{Serialize, Serializer};
use zeroize::Zeroizing;

/// Conservative simple-send limits (no upload session or Mail.ReadWrite scope).
/// Aggregate raw PDF/attachment bytes; every individual file is also below 3 MB.
pub const MAX_ATTACHMENT_BYTES: usize = 2_800_000;
pub const MAX_MESSAGE_BYTES: usize = 3_900_000;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Envelope<'a> {
    message: Message<'a>,
    save_to_sent_items: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Message<'a> {
    subject: &'a str,
    body: Body<'a>,
    to_recipients: Vec<Recipient<'a>>,
    attachments: Vec<FileAttachment<'a>>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Body<'a> {
    content_type: &'static str,
    content: &'a str,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Recipient<'a> {
    email_address: Address<'a>,
}
#[derive(Serialize)]
struct Address<'a> {
    address: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileAttachment<'a> {
    #[serde(rename = "@odata.type")]
    kind: &'static str,
    name: &'a str,
    content_type: &'a str,
    #[serde(serialize_with = "base64_bytes")]
    content_bytes: &'a [u8],
}
fn base64_bytes<S: Serializer>(bytes: &&[u8], serializer: S) -> Result<S::Ok, S::Error> {
    let encoded = Zeroizing::new(STANDARD.encode(bytes));
    serializer.serialize_str(&encoded)
}
fn clean(value: &str, max: usize) -> bool {
    value.len() <= max && !value.chars().any(char::is_control)
}
fn valid_mailbox(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && value.len() <= 254
        && !value
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || "<>(),;:\"\\".contains(c))
        && !domain.contains('@')
}

pub(crate) fn encode(doc: &OutgoingDocument) -> PluginResult<SecretBytes> {
    if doc.to.is_empty()
        || doc.to.len() > 100
        || !clean(&doc.subject, 998)
        || doc.pdf.expose().is_empty()
        || doc.attachments.len() > 20
        || doc.to.iter().any(|m| {
            !valid_mailbox(&m.address) || m.display_name.as_ref().is_some_and(|n| !clean(n, 256))
        })
    {
        return Err(PluginError::InvalidConfiguration);
    }
    if doc.body_text.expose_secret().len() > 64 * 1024 {
        return Err(PluginError::TooLarge);
    }
    let mut size = doc.pdf.expose().len();
    for attachment in &doc.attachments {
        if attachment.filename.is_empty()
            || !clean(&attachment.filename, 255)
            || attachment.filename.contains(['/', '\\'])
            || attachment.filename == "."
            || attachment.filename == ".."
            || !clean(&attachment.media_type, 127)
            || !attachment.media_type.is_ascii()
            || !attachment.media_type.contains('/')
        {
            return Err(PluginError::InvalidConfiguration);
        }
        size = size
            .checked_add(attachment.bytes.expose().len())
            .ok_or(PluginError::TooLarge)?;
    }
    if size > MAX_ATTACHMENT_BYTES {
        return Err(PluginError::TooLarge);
    }
    let mut attachments = vec![FileAttachment {
        kind: "#microsoft.graph.fileAttachment",
        name: "document.pdf",
        content_type: "application/pdf",
        content_bytes: doc.pdf.expose(),
    }];
    attachments.extend(doc.attachments.iter().map(|a| FileAttachment {
        kind: "#microsoft.graph.fileAttachment",
        name: &a.filename,
        content_type: &a.media_type,
        content_bytes: a.bytes.expose(),
    }));
    let envelope = Envelope {
        save_to_sent_items: true,
        message: Message {
            subject: &doc.subject,
            body: Body {
                content_type: "Text",
                content: doc.body_text.expose_secret(),
            },
            to_recipients: doc
                .to
                .iter()
                .map(|m| Recipient {
                    email_address: Address {
                        address: &m.address,
                        name: m.display_name.as_deref(),
                    },
                })
                .collect(),
            attachments,
        },
    };
    let mut bytes = Zeroizing::new(Vec::new());
    serde_json::to_writer(&mut *bytes, &envelope).map_err(|_| PluginError::InvalidConfiguration)?;
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(PluginError::TooLarge);
    }
    Ok(SecretBytes::new(std::mem::take(&mut *bytes)))
}
