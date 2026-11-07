//! MIME construction only: lettre's transport, TLS and socket features are off.
use fb_plugin_api::*;
use lettre::message::{
    header::{ContentDisposition, ContentTransferEncoding, ContentType},
    Mailbox, Message, MultiPart, SinglePart,
};
use std::time::{Duration, SystemTime};

/// Conservative application limits leave room for MIME and base64url overhead
/// beneath the framework's 20 MiB default request cap.
pub const MAX_CONTENT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_RAW_BYTES: usize = 16 * 1024 * 1024;
fn header_text(value: &str, max: usize) -> PluginResult<()> {
    if value.len() > max || value.chars().any(char::is_control) {
        return Err(PluginError::InvalidConfiguration);
    }
    Ok(())
}
pub(crate) fn mailbox(address: &str, name: Option<&str>) -> PluginResult<Mailbox> {
    header_text(address, 254)?;
    // No RFC 6532 SMTPUTF8 requirement; unicode display names remain supported.
    if !address.is_ascii() || address.trim() != address {
        return Err(PluginError::InvalidConfiguration);
    }
    if let Some(name) = name {
        header_text(name, 256)?;
    }
    let email = address
        .parse()
        .map_err(|_| PluginError::InvalidConfiguration)?;
    Ok(Mailbox::new(name.map(str::to_owned), email))
}
fn attachment(filename: &str, media_type: &str, bytes: &[u8]) -> PluginResult<SinglePart> {
    header_text(filename, 255)?;
    header_text(media_type, 127)?;
    if filename.is_empty() || filename.contains(['/', '\\']) || filename == "." || filename == ".."
    {
        return Err(PluginError::InvalidConfiguration);
    }
    let content_type =
        ContentType::parse(media_type).map_err(|_| PluginError::InvalidConfiguration)?;
    Ok(SinglePart::builder()
        .header(content_type)
        .header(ContentDisposition::attachment(filename))
        .header(ContentTransferEncoding::Base64)
        .body(bytes.to_vec()))
}
pub(crate) fn build(
    from: &str,
    document: &OutgoingDocument,
    now: Timestamp,
) -> PluginResult<SecretBytes> {
    if document.document.id.is_nil()
        || document.to.is_empty()
        || document.to.len() > 100
        || document.attachments.len() > 20
        || document.pdf.expose().is_empty()
    {
        return Err(PluginError::InvalidConfiguration);
    }
    header_text(&document.subject, 998)?;
    let size = document
        .attachments
        .iter()
        .try_fold(
            document
                .pdf
                .expose()
                .len()
                .saturating_add(document.body_text.expose_secret().len()),
            |size, a| size.checked_add(a.bytes.expose().len()),
        )
        .ok_or(PluginError::TooLarge)?;
    if size > MAX_CONTENT_BYTES {
        return Err(PluginError::TooLarge);
    }
    // httpdate (used by lettre) panics for dates outside its RFC year range.
    if now.0 >= 253_402_300_800 {
        return Err(PluginError::InvalidConfiguration);
    }
    let date = SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(now.0))
        .ok_or(PluginError::InvalidConfiguration)?;
    let mut message = Message::builder()
        .from(mailbox(from, None)?)
        .subject(&document.subject)
        .date(date)
        .message_id(None);
    for to in &document.to {
        message = message.to(mailbox(&to.address, to.display_name.as_deref())?);
    }
    let mut parts = MultiPart::mixed()
        .singlepart(
            SinglePart::builder()
                .header(ContentType::TEXT_PLAIN)
                .header(ContentTransferEncoding::Base64)
                .body(document.body_text.expose_secret().as_bytes().to_vec()),
        )
        .singlepart(attachment(
            "document.pdf",
            "application/pdf",
            document.pdf.expose(),
        )?);
    for file in &document.attachments {
        parts = parts.singlepart(attachment(
            &file.filename,
            &file.media_type,
            file.bytes.expose(),
        )?);
    }
    let message = message
        .multipart(parts)
        .map_err(|_| PluginError::InvalidConfiguration)?;
    let raw = SecretBytes::new(message.formatted());
    if raw.expose().len() > MAX_RAW_BYTES {
        return Err(PluginError::TooLarge);
    }
    Ok(raw)
}
