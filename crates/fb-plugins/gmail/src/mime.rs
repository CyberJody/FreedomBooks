//! MIME construction only: lettre's transport, TLS and socket features are off.
use crate::{DeliveryContent, INLINE_LOGO_CID};
use fb_plugin_api::*;
use lettre::message::{
    Mailbox, Message, MultiPart, SinglePart,
    header::{ContentDisposition, ContentId, ContentTransferEncoding, ContentType},
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
    build_with_content(from, document, &DeliveryContent::default(), now)
}
pub(crate) fn build_with_content(
    from: &str,
    document: &OutgoingDocument,
    content: &DeliveryContent,
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
    if content.inline_images.len()>8 {return Err(PluginError::TooLarge);}
    let mut image_ids=std::collections::HashSet::new();
    let mut image_bytes=0usize;
    for image in &content.inline_images {
        let id=&image.content_id;
        if id.is_empty() || id.len()>64 || !id.bytes().all(|b|b.is_ascii_alphanumeric()||b"-_".contains(&b)) || id==INLINE_LOGO_CID || !image_ids.insert(id) || !["image/png","image/jpeg"].contains(&image.file.media_type.as_str()) || image.file.bytes.expose().is_empty() {return Err(PluginError::InvalidConfiguration);}
        image_bytes=image_bytes.checked_add(image.file.bytes.expose().len()).ok_or(PluginError::TooLarge)?;
    }
    if image_bytes>128*1024 {return Err(PluginError::TooLarge);}
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
    let size = size
        .checked_add(content.html.as_ref().map_or(0, |s| s.expose_secret().len()))
        .and_then(|size| {
            size.checked_add(
                content
                    .inline_logo
                    .as_ref()
                    .map_or(0, |file| file.bytes.expose().len()),
            )
        })
        .ok_or(PluginError::TooLarge)?;
    let size=size.checked_add(image_bytes).ok_or(PluginError::TooLarge)?;
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
    if let Some(id) = &content.message_id {
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-.@_".contains(&b))
            || id.bytes().filter(|b| *b == b'@').count() != 1
        {
            return Err(PluginError::InvalidConfiguration);
        }
        message = message.message_id(Some(format!("<{id}>")));
    }
    if let Some(reply) = &content.reply_to {
        message = message.reply_to(mailbox(&reply.address, reply.display_name.as_deref())?);
    }
    for to in &document.to {
        message = message.to(mailbox(&to.address, to.display_name.as_deref())?);
    }
    let plain = SinglePart::builder()
        .header(ContentType::TEXT_PLAIN)
        .header(ContentTransferEncoding::Base64)
        .body(document.body_text.expose_secret().as_bytes().to_vec());
    let mut parts = if let Some(html) = &content.html {
        let html = SinglePart::builder()
            .header(ContentType::TEXT_HTML)
            .header(ContentTransferEncoding::Base64)
            .body(html.expose_secret().as_bytes().to_vec());
        let alternative = MultiPart::alternative().singlepart(plain);
        let alternative = if content.inline_logo.is_some() || !content.inline_images.is_empty() {
            let mut related=MultiPart::related().singlepart(html);
            if let Some(logo) = &content.inline_logo {
            if !["image/png", "image/jpeg"].contains(&logo.media_type.as_str())
                || logo.bytes.expose().is_empty()
            {
                return Err(PluginError::InvalidConfiguration);
            }
            let inline = SinglePart::builder()
                .header(
                    ContentType::parse(&logo.media_type)
                        .map_err(|_| PluginError::InvalidConfiguration)?,
                )
                .header(ContentDisposition::inline())
                .header(ContentId::from(format!("<{INLINE_LOGO_CID}>")))
                .header(ContentTransferEncoding::Base64)
                .body(logo.bytes.expose().to_vec());
            related=related.singlepart(inline);
            }
            for image in &content.inline_images {
                let inline=SinglePart::builder().header(ContentType::parse(&image.file.media_type).map_err(|_|PluginError::InvalidConfiguration)?).header(ContentDisposition::inline()).header(ContentId::from(format!("<{}>",image.content_id))).header(ContentTransferEncoding::Base64).body(image.file.bytes.expose().to_vec());
                related=related.singlepart(inline);
            }
            alternative.multipart(related)
        } else {
            alternative.singlepart(html)
        };
        MultiPart::mixed().multipart(alternative)
    } else {
        if content.inline_logo.is_some() || !content.inline_images.is_empty() {
            return Err(PluginError::InvalidConfiguration);
        }
        MultiPart::mixed().singlepart(plain)
    };
    parts = parts.singlepart(attachment(
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
