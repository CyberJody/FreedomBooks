//! Private-pipe Gmail operations. No database, filesystem attachment paths, arbitrary
//! hosts or renderer capabilities enter this process. The book owns delivery history.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use fb_plugin_gmail::{DeliveryContent, DeliveryMode, DeliveryOutcome, GmailSender};

fn secret<'de, D: serde::Deserializer<'de>>(de: D) -> Result<SecretString, D::Error> {
    String::deserialize(de).map(SecretString::from)
}
fn optional_secret<'de, D: serde::Deserializer<'de>>(
    de: D,
) -> Result<Option<SecretString>, D::Error> {
    Option::<String>::deserialize(de).map(|s| s.map(SecretString::from))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Input {
    #[serde(default)]
    client_id: String,
    #[serde(default, deserialize_with = "optional_secret")]
    client_secret: Option<SecretString>,
    #[serde(default)]
    from_address: String,
    #[serde(default)]
    delivery: Option<Mail>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mail {
    invoice: Uuid,
    mode: Mode,
    to: String,
    subject: String,
    #[serde(deserialize_with = "secret")]
    text: SecretString,
    #[serde(deserialize_with = "secret")]
    html: SecretString,
    #[serde(deserialize_with = "secret")]
    pdf: SecretString,
    #[serde(default)]
    reply_to: Option<String>,
    #[serde(default)]
    logo: Option<Logo>,
    #[serde(default)]
    brand_images: Vec<String>,
    message_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum Mode {
    Draft,
    Send,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Logo {
    mime: String,
    #[serde(deserialize_with = "secret")]
    data: SecretString,
}

// This envelope is OS-sealed by the book-side credential store. It keeps the
// registration and token together so refresh never needs a secret in the UI.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u8,
    client_id: String,
    #[serde(deserialize_with = "optional_secret")]
    client_secret: Option<SecretString>,
    #[serde(deserialize_with = "secret")]
    tokens: SecretString,
}
impl Stored {
    fn read(request: &Request) -> PluginResult<Self> {
        let record: Self =
            serde_json::from_str(request.record.as_deref().ok_or(PluginError::NeedsSignIn)?)
                .map_err(|_| PluginError::NeedsSignIn)?;
        if record.version != 1 {
            return Err(PluginError::NeedsSignIn);
        }
        Ok(record)
    }
}
fn encode(
    client_id: &str,
    client_secret: Option<&SecretString>,
    tokens: &SecretBytes,
) -> PluginResult<SecretBytes> {
    #[derive(Serialize)]
    struct Envelope<'a> {
        version: u8,
        client_id: &'a str,
        client_secret: Option<&'a str>,
        tokens: &'a str,
    }
    serde_json::to_vec(&Envelope {
        version: 1,
        client_id,
        client_secret: client_secret.map(SecretString::expose_secret),
        tokens: exposed(tokens)?,
    })
    .map(SecretBytes::new)
    .map_err(|_| PluginError::CredentialStore)
}
fn plugin(
    client_id: &str,
    secret: Option<&SecretString>,
) -> PluginResult<(GmailSender, PluginHttp)> {
    if secret.is_some_and(|s| s.expose_secret().len() > 16 * 1024) {
        return Err(PluginError::InvalidConfiguration);
    }
    let plugin = GmailSender::new(
        client_id.to_owned(),
        secret.map(|s| SecretString::from(s.expose_secret())),
    )?
    .with_drafts();
    #[cfg(not(feature = "fake-network"))]
    {
        Ok((
            plugin,
            PluginHttp::new(HttpPolicy::new(
                fb_plugin_gmail::COMPOSE_MANIFEST.network_hosts,
            )?),
        ))
    }
    #[cfg(feature = "fake-network")]
    {
        let base =
            std::env::var("FB_NET_FAKE_GMAIL").map_err(|_| PluginError::InvalidConfiguration)?;
        let port = base
            .rsplit(':')
            .next()
            .and_then(|p| p.trim_end_matches('/').parse().ok())
            .ok_or(PluginError::InvalidConfiguration)?;
        Ok((
            plugin.with_loopback_endpoints(&base)?,
            PluginHttp::new(HttpPolicy::loopback_for_tests(port)?),
        ))
    }
}
pub(super) fn begin(request: &Request) -> Result<(), (Option<&'static str>, PluginError)> {
    let result = (|| -> PluginResult<_> {
        let input = request
            .gmail
            .as_ref()
            .ok_or(PluginError::InvalidConfiguration)?;
        if input.delivery.is_some() || request.record.is_some() {
            return Err(PluginError::InvalidConfiguration);
        }
        let (plugin, http) = plugin(&input.client_id, input.client_secret.as_ref())?;
        let (cx, store) = context(fb_plugin_gmail::MANIFEST.id, http, None)?;
        let pending = plugin.begin_authorization(&cx, input.from_address.clone())?;
        Ok((input, plugin, cx, store, pending))
    })()
    .map_err(|e| (None, e))?;
    let (input, plugin, cx, store, pending) = result;
    #[derive(Serialize)]
    struct Authorize<'a> {
        ok: bool,
        stage: &'static str,
        authorization_url: &'a str,
    }
    reply(&Authorize {
        ok: true,
        stage: "authorize",
        authorization_url: pending.authorization_url().expose_secret(),
    });
    let done = Some("done");
    plugin
        .complete_authorization(&cx, pending)
        .map_err(|e| (done, e))?;
    let tokens = store
        .take()
        .map_err(|e| (done, e))?
        .ok_or((done, PluginError::CredentialStore))?;
    let record =
        encode(&input.client_id, input.client_secret.as_ref(), &tokens).map_err(|e| (done, e))?;
    #[derive(Serialize)]
    struct Connected<'a> {
        ok: bool,
        stage: &'static str,
        record: &'a str,
        from_address: &'a str,
    }
    reply(&Connected {
        ok: true,
        stage: "done",
        record: exposed(&record).map_err(|e| (done, e))?,
        from_address: &input.from_address,
    });
    Ok(())
}
pub(super) fn disconnect(request: &Request) -> PluginResult<()> {
    let stored = Stored::read(request)?;
    let (plugin, http) = plugin(&stored.client_id, stored.client_secret.as_ref())?;
    let (cx, _) = context(
        fb_plugin_gmail::MANIFEST.id,
        http,
        Some(stored.tokens.expose_secret()),
    )?;
    let result = plugin.disconnect(&cx)?;
    #[derive(Serialize)]
    struct Disconnected {
        ok: bool,
        revoked: bool,
    }
    reply(&Disconnected {
        ok: true,
        revoked: result.revocation_confirmed,
    });
    Ok(())
}
fn decode(value: &SecretString, max: usize) -> PluginResult<SecretBytes> {
    if value.expose_secret().len() > max.div_ceil(3) * 4 {
        return Err(PluginError::TooLarge);
    }
    let bytes = SecretBytes::new(
        STANDARD
            .decode(value.expose_secret())
            .map_err(|_| PluginError::InvalidConfiguration)?,
    );
    if bytes.expose().is_empty() || bytes.expose().len() > max {
        return Err(PluginError::TooLarge);
    }
    Ok(bytes)
}
pub(super) fn deliver(request: &Request) -> PluginResult<()> {
    let stored = Stored::read(request)?;
    let input = request
        .gmail
        .as_ref()
        .ok_or(PluginError::InvalidConfiguration)?;
    if !input.client_id.is_empty()
        || input.client_secret.is_some()
        || !input.from_address.is_empty()
    {
        return Err(PluginError::InvalidConfiguration);
    }
    let mail = input
        .delivery
        .as_ref()
        .ok_or(PluginError::InvalidConfiguration)?;
    let pdf = decode(&mail.pdf, 8 * 1024 * 1024)?;
    if !pdf.expose().starts_with(b"%PDF-") || mail.invoice.is_nil() {
        return Err(PluginError::InvalidConfiguration);
    }
    let logo = mail
        .logo
        .as_ref()
        .map(|logo| {
            let extension = match logo.mime.as_str() {
                "image/png" => "png",
                "image/jpeg" => "jpg",
                _ => return Err(PluginError::InvalidConfiguration),
            };
            Ok(Attachment {
                filename: format!("logo.{extension}"),
                media_type: logo.mime.clone(),
                bytes: decode(&logo.data, 128 * 1024)?,
            })
        })
        .transpose()?;
    if mail.brand_images.len()>8 {return Err(PluginError::TooLarge);}
    let mut seen=std::collections::HashSet::new();
    let mut inline_images=Vec::new();
    for id in &mail.brand_images {
        if !seen.insert(id) {return Err(PluginError::InvalidConfiguration);}
        let bytes:&[u8]=match id.as_str() {
            "zelle"=>include_bytes!("../../../electron/renderer/static/sales/pay/zelle-badge-purple.png"),
            "stripe"=>include_bytes!("../../../electron/renderer/static/sales/pay/powered-by-stripe-blurple.png"),
            "visa"=>include_bytes!("../../../electron/renderer/static/sales/pay/visa.png"),
            "master"=>include_bytes!("../../../electron/renderer/static/sales/pay/master.png"),
            "amex"=>include_bytes!("../../../electron/renderer/static/sales/pay/american_express.png"),
            "discover"=>include_bytes!("../../../electron/renderer/static/sales/pay/discover.png"),
            "bank"=>include_bytes!("../../../electron/renderer/static/sales/pay/genericbank.png"),
            "apple"=>include_bytes!("../../../electron/renderer/static/sales/pay/apple_pay.png"),
            _=>return Err(PluginError::InvalidConfiguration),
        };
        inline_images.push(fb_plugin_gmail::InlineImage {content_id:format!("freedombooks-pay-{id}"),file:Attachment {filename:format!("{id}.png"),media_type:"image/png".into(),bytes:SecretBytes::new(bytes.to_vec())}});
    }
    let (plugin, http) = plugin(&stored.client_id, stored.client_secret.as_ref())?;
    let (cx, store) = context(
        fb_plugin_gmail::MANIFEST.id,
        http,
        Some(stored.tokens.expose_secret()),
    )?;
    let document = OutgoingDocument {
        document: DocumentRef {
            kind: DocumentKind::Invoice,
            id: mail.invoice,
        },
        to: vec![Mailbox {
            address: mail.to.clone(),
            display_name: None,
        }],
        subject: mail.subject.clone(),
        body_text: SecretString::from(mail.text.expose_secret()),
        pdf,
        attachments: vec![],
    };
    let content = DeliveryContent {
        html: Some(SecretString::from(mail.html.expose_secret())),
        reply_to: mail.reply_to.as_ref().map(|address| Mailbox {
            address: address.clone(),
            display_name: None,
        }),
        inline_logo: logo,
        inline_images,
        message_id: Some(mail.message_id.clone()),
    };
    let result = plugin.deliver_outcome(
        &cx,
        document,
        content,
        match mail.mode {
            Mode::Draft => DeliveryMode::Draft,
            Mode::Send => DeliveryMode::Send,
        },
    );
    // Even a refused operation may have refreshed or invalidated a token. Return
    // that state without turning provider acceptance into a retryable error.
    let record = store
        .take()?
        .map(|tokens| encode(&stored.client_id, stored.client_secret.as_ref(), &tokens))
        .transpose()?;
    #[derive(Serialize)]
    struct Delivered<'a> {
        ok: bool,
        outcome: &'static str,
        record: Option<&'a str>,
        message_id: Option<&'a str>,
        draft_id: Option<&'a str>,
        code: Option<&'static str>,
    }
    let record = record.as_ref().map(exposed).transpose()?;
    match result {
        DeliveryOutcome::Accepted { receipt, .. } => reply(&Delivered {
            ok: true,
            outcome: "accepted",
            record,
            message_id: Some(&receipt.message_id),
            draft_id: receipt.draft_id.as_deref(),
            code: None,
        }),
        DeliveryOutcome::NotSubmitted { error } | DeliveryOutcome::Uncertain { error } => {
            let outcome = if matches!(result, DeliveryOutcome::Uncertain { .. }) {
                "unknown"
            } else {
                "refused"
            };
            let code = if error == PluginError::NeedsSignIn {
                "needs_signin"
            } else {
                "provider_failed"
            };
            reply(&Delivered {
                ok: true,
                outcome,
                record,
                message_id: None,
                draft_id: None,
                code: Some(code),
            });
        }
    }
    Ok(())
}
