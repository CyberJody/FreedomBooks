use crate::SmtpConfig;
use fb_plugin_api::{
    ExposeSecret, OutgoingDocument, PluginError, PluginResult, SecretBytes, Timestamp,
};
use lettre::{
    address::Envelope,
    message::{header::ContentType, Attachment, Mailbox, MultiPart, SinglePart},
    Message,
};
use std::time::{Duration, SystemTime};

pub(super) struct PreparedMessage {
    pub envelope: Envelope,
    pub bytes: SecretBytes,
}

fn header(value: &str) -> PluginResult<()> {
    if value.chars().any(char::is_control) {
        Err(PluginError::InvalidConfiguration)
    } else {
        Ok(())
    }
}
pub(super) fn mailbox(address: &str, name: Option<String>) -> PluginResult<Mailbox> {
    header(address)?;
    if let Some(name) = &name {
        header(name)?;
    }
    Ok(Mailbox::new(
        name,
        address
            .parse()
            .map_err(|_| PluginError::InvalidConfiguration)?,
    ))
}

pub(super) fn prepare(
    config: &SmtpConfig,
    document: OutgoingDocument,
    now: Timestamp,
) -> PluginResult<PreparedMessage> {
    // lettre's date formatter panics outside its supported 1970..9999 range.
    if now.0 >= 253_402_300_800
        || document.document.id.is_nil()
        || document.to.is_empty()
        || document.to.len() > 50
        || document.attachments.len() > 10
        || !document.pdf.expose().starts_with(b"%PDF-")
    {
        return Err(PluginError::InvalidConfiguration);
    }
    header(&document.subject)?;
    // Bound the input before MIME encoding can allocate several copies. Also cap the
    // actual encoded message afterwards: base64 overhead counts against the limit.
    let mut total = 0usize;
    let mut count = |size: usize| -> PluginResult<()> {
        total = total
            .checked_add(size)
            .ok_or(PluginError::OutboundTooLarge)?;
        if total > config.max_message_bytes {
            Err(PluginError::OutboundTooLarge)
        } else {
            Ok(())
        }
    };
    for size in [
        document.subject.len(),
        document.body_text.expose_secret().len(),
        document.pdf.expose().len(),
        config.from.len(),
    ] {
        count(size)?;
    }
    for recipient in &document.to {
        count(recipient.address.len())?;
        count(recipient.display_name.as_deref().unwrap_or("").len())?;
    }
    for attachment in &document.attachments {
        count(attachment.bytes.expose().len())?;
        count(attachment.filename.len())?;
        count(attachment.media_type.len())?;
        header(&attachment.filename)?;
        header(&attachment.media_type)?;
        if attachment.filename.is_empty() || attachment.filename.contains(['/', '\\', '"']) {
            return Err(PluginError::InvalidConfiguration);
        }
    }
    let date = SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(now.0))
        .ok_or(PluginError::InvalidConfiguration)?;
    let mut builder = Message::builder()
        .from(mailbox(&config.from, None)?)
        .subject(document.subject)
        .date(date);
    for recipient in document.to {
        builder = builder.to(mailbox(&recipient.address, recipient.display_name)?);
    }
    let mut parts = MultiPart::mixed()
        .singlepart(SinglePart::plain(
            document.body_text.expose_secret().to_owned(),
        ))
        .singlepart(Attachment::new("document.pdf".into()).body(
            document.pdf.expose().to_vec(),
            ContentType::parse("application/pdf").map_err(|_| PluginError::InvalidConfiguration)?,
        ));
    for attachment in document.attachments {
        let content_type = ContentType::parse(&attachment.media_type)
            .map_err(|_| PluginError::InvalidConfiguration)?;
        parts = parts.singlepart(
            Attachment::new(attachment.filename)
                .body(attachment.bytes.expose().to_vec(), content_type),
        );
    }
    let message = builder
        .multipart(parts)
        .map_err(|_| PluginError::InvalidConfiguration)?;
    let bytes = SecretBytes::new(message.formatted());
    if bytes.expose().len() > config.max_message_bytes {
        return Err(PluginError::OutboundTooLarge);
    }
    Ok(PreparedMessage {
        envelope: message.envelope().clone(),
        bytes,
    })
}
