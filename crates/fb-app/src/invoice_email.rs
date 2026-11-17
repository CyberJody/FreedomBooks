//! Invoice email orchestration. Only the native main process supplies rendered PDF bytes.
use crate::{App, AppError, AppResult};
use crate::{Outcome, identity::Permission};
use base64::{Engine, engine::general_purpose::STANDARD};
use fb_core::CompanyId;
use fb_core::{UserId, invoice_email as core};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::Instant;

#[derive(Default)]
pub(crate) struct State {
    pub signins: HashMap<CompanyId, (u64, i64)>,
    pending: HashMap<String, Pending>,
}
struct Pending {
    company: CompanyId,
    actor: UserId,
    generation: u64,
    created: Instant,
    attempt: core::Attempt,
    snapshot: Snapshot,
}
pub(crate) struct Snapshot {
    pub payload: Value,
    print: String,
    hash: String,
    revision: String,
}
pub(crate) fn require(app: &App) -> AppResult<()> {
    if !app.invoice_delivery {
        return Err(AppError::Invalid(
            "Invoice email is not available in this build.".into(),
        ));
    }
    if !app.os_store_usable || app.vault_key.is_none() {
        return Err(AppError::Invalid("Google connections need this computer’s keyring. Unlock or enable it, then restart FreedomBooks.".into()));
    }
    Ok(())
}

fn preview_path(c: CompanyId, invoice: &str) -> String {
    format!("/c/{c}/invoices/{invoice}/email-preview/")
}
fn snapshot(app: &mut App, c: CompanyId, invoice: &str) -> AppResult<Snapshot> {
    let route = crate::screens::resolve(&preview_path(c, invoice))
        .ok_or_else(|| AppError::Invalid("Invoice preview is unavailable.".into()))?;
    let mut vm = crate::screens::printing::view_model(app, &route)?;
    if vm["current"]["state"] != "OPEN" || vm["doc"]["superseded"] == true {
        return Err(AppError::Invalid(
            "Only the current posted invoice can be emailed.".into(),
        ));
    }
    if vm["doc"]["can_pay"]==true && vm["doc"]["preferences"]["stripe_enabled"]==true && (vm["doc"]["preferences"]["stripe_mode"]!="LIVE" || !vm["doc"]["payment_url"].is_string()) {return Err(AppError::Invalid("Prepare a live Stripe payment link before emailing this invoice, or turn off Stripe in invoice settings.".into()));}
    let logo = app
        .open_mut()?
        .book
        .read(|q| fb_core::company_documents::logo(q, c))?;
    let mut digest_doc = vm["doc"].clone();
    digest_doc.as_object_mut().unwrap().remove("printed_at");
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(&digest_doc).expect("invoice view"));
    if let Some((mime, bytes)) = &logo {
        digest.update(mime);
        digest.update(bytes);
        vm["doc"]["company_logo_url"] =
            json!(format!("data:{mime};base64,{}", STANDARD.encode(bytes)));
    }
    vm["stamp"] = json!("");
    vm["note"] = json!("");
    let print = app.screens.render("sales/print/document.html", &vm)?;
    let revision = vm["current"]["id"]
        .as_str()
        .ok_or_else(|| AppError::Invalid("Invalid invoice revision.".into()))?
        .to_owned();
    if logo.is_some() {
        vm["doc"]["company_logo_url"] = json!("cid:freedombooks-logo");
    }
    let mut brand_images=Vec::new();
    if vm["doc"]["can_pay"]==true {
        if vm["doc"]["preferences"]["zelle_enabled"]==true {brand_images.push("zelle");}
        if vm["doc"]["preferences"]["stripe_enabled"]==true || vm["doc"]["payment_url"].is_string() {brand_images.extend(["stripe","apple","visa","master","amex","discover","bank"]);}
    }
    for key in &brand_images {vm["doc"]["payment_images"][*key]=json!(format!("cid:freedombooks-pay-{key}"));}
    let html = app.screens.render("sales/email/card.html", &vm)?;
    let doc = &vm["doc"];
    let prefs = &doc["preferences"];
    let s = |v: &Value| v.as_str().unwrap_or("").to_owned();
    let mut text = format!(
        "{}\n\nInvoice {}\nBalance due: {} {}\nDue: {}\nYour invoice is attached as a PDF.\n",
        s(&prefs["email_greeting"]),
        s(&doc["number"]),
        s(&doc["balance"]["due"]),
        s(&doc["currency"]),
        s(&doc["second_date"])
    );
    if doc["can_pay"] == true {
        if prefs["zelle_enabled"] == true {
            text.push_str(&format!(
                "\nZelle: {} ({})\nInclude Invoice {} in the memo.\n{}\n",
                s(&prefs["zelle_recipient"]),
                s(&prefs["zelle_name"]),
                s(&doc["number"]),
                s(&prefs["zelle_instructions"])
            ));
        }
        if let Some(url) = doc["payment_url"].as_str() {
            text.push_str(&format!("\nPay online: {url}\n"));
        }
        text.push_str(&s(&doc["business"]["payment_instructions"]));
    }
    text.push_str(&format!(
        "\n\n{}\n{}\n{}",
        s(&doc["memo"]),
        s(&prefs["email_signature"]),
        s(&prefs["footer"])
    ));
    let reply_to = s(&prefs["reply_to"]);
    Ok(Snapshot {
        print,
        hash: format!("{:x}", digest.finalize()),
        revision,
        payload: json!({"invoice":invoice,"to":doc["party"]["email"],"subject":prefs["email_subject"],"text":text,"html":html,
        "reply_to":if reply_to.is_empty(){None}else{Some(reply_to)},"brand_images":brand_images,"logo":logo.map(|(mime,bytes)|json!({"mime":mime,"data":STANDARD.encode(bytes)}))}),
    })
}
/// Whether this company can send invoice email now: delivery is on and Gmail is connected.
pub(crate) fn gmail_ready(app: &mut App, c: CompanyId) -> AppResult<bool> {
    if !app.invoice_delivery || !app.os_store_usable || app.vault_key.is_none() {
        return Ok(false);
    }
    let connection = app.open_mut()?.book.read(|q| core::connection(q, c))?;
    Ok(connection.is_some_and(|v| v.status == "CONNECTED"))
}
pub(crate) fn preview(app: &mut App, c: CompanyId, invoice: &str, vm: &mut Value) -> AppResult<()> {
    if !app.invoice_delivery {
        return Ok(());
    }
    let open = app.open_mut()?;
    let actor = open.owner;
    let connection = open.book.read(|q| core::connection(q, c))?;
    let history = open.book.read(|q| core::history(q, actor, c, invoice))?;
    vm["email_unknown"] = json!(
        history
            .first()
            .filter(|a| a.state == "UNKNOWN")
            .map(|a| a.id.clone())
    );
    vm["email_history"] = serde_json::to_value(history).expect("history");
    vm["email_delivery_enabled"] = json!(true);
    vm["gmail_connected"] = json!(
        connection.as_ref().is_some_and(|v| v.status == "CONNECTED") && app.os_store_usable && app.vault_key.is_some()
    );
    vm["email_ready"] = json!(
        connection.as_ref().is_some_and(|v| v.status == "CONNECTED")
            && app.os_store_usable
            && app.vault_key.is_some()
            && vm["current"]["state"] == "OPEN"
    );
    vm["email_from"] = json!(connection.map(|v| v.from_address));
    vm["email_messages"] = json!(std::mem::take(&mut app.messages));
    Ok(())
}
pub(crate) fn submit(
    app: &mut App,
    c: CompanyId,
    invoice: &str,
    path: &str,
    fields: &[(String, String)],
) -> AppResult<Outcome> {
    let session = app.authorize(Some(c), Permission::Write)?;
    require(app)?;
    let field = |key: &str| {
        fields
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .unwrap_or("")
    };
    let mode = match field("action") {
        "email_draft" => core::Mode::Draft,
        "email_send" => core::Mode::Send,
        "email_resolve" => {
            // Explicitly close a preparation or a possibly submitted attempt; never resend it.
            let id = field("attempt");
            if app.email.pending.values().any(|p| p.attempt.id == id) {
                return Err(AppError::Invalid(
                    "This email is still being prepared.".into(),
                ));
            }
            let open = app.open_mut()?;
            let row = open.book.read(|q| core::attempt(q, session.actor, c, id))?;
            if row.invoice_id != invoice {
                return Err(AppError::Invalid("Wrong invoice.".into()));
            }
            core::resolve_interrupted(&mut open.book, session.actor, c, id)?;
            app.messages.push("The interrupted attempt is retained in history. Check Gmail before starting another email.".into());
            return Ok(Outcome::Redirect {
                location: path.into(),
            });
        }
        _ => {
            return Err(AppError::Invalid(
                "Choose Save to Drafts or Send email.".into(),
            ));
        }
    };
    let snap = snapshot(app, c, invoice)?;
    let open = app.open_mut()?;
    let connection = open
        .book
        .read(|q| core::connection(q, c))?
        .ok_or_else(|| AppError::Invalid("Connect Google in company settings first.".into()))?;
    let attempt = core::prepare(
        &mut open.book,
        session.actor,
        c,
        core::Preparation {
            invoice,
            revision: &snap.revision,
            connection_revision: connection.revision,
            mode,
            to: snap.payload["to"].as_str().unwrap_or(""),
            subject: snap.payload["subject"].as_str().unwrap_or(""),
            document_hash: &snap.hash,
            checked_unknown: if field("email_checked") == "on" {
                Some(field("unknown_attempt"))
            } else {
                None
            },
        },
    )?;
    let ticket = uuid::Uuid::new_v4().to_string();
    app.email.pending.insert(
        ticket.clone(),
        Pending {
            company: c,
            actor: session.actor,
            generation: session.generation,
            created: Instant::now(),
            attempt,
            snapshot: snap,
        },
    );
    Ok(Outcome::Done {
        data: json!({"invoice_email_pdf":ticket}),
    })
}
fn valid(app: &App, p: &Pending) -> bool {
    p.generation == app.session_generation() && p.created.elapsed().as_secs() < 300
}
/// Main-process-only access: never exposed by the renderer command vocabulary.
pub fn print_document(app: &mut App, ticket: &str) -> AppResult<String> {
    let p = app
        .email
        .pending
        .get(ticket)
        .ok_or_else(|| AppError::Invalid("Email preparation expired.".into()))?;
    if !valid(app, p) {
        return Err(AppError::Invalid("Email preparation expired.".into()));
    }
    let c = p.company;
    app.authorize(Some(c), Permission::Write)?;
    Ok(app.email.pending[ticket].snapshot.print.clone())
}
/// A main-process print failure supplies no bytes. The capability is single-use.
pub fn finish_pdf(app: &mut App, ticket: &str, pdf: &[u8]) -> AppResult<Outcome> {
    let pending = app
        .email
        .pending
        .remove(ticket)
        .ok_or_else(|| AppError::Invalid("Email preparation expired.".into()))?;
    let c = pending.company;
    let invoice = pending.attempt.invoice_id.clone();
    let session = app.authorize(Some(c), Permission::Write)?;
    require(app)?;
    if !valid(app, &pending) || pending.actor != session.actor {
        return Err(AppError::Invalid(
            "The active user changed or email preparation expired.".into(),
        ));
    }
    let refreshed = snapshot(app, c, &invoice);
    let ready = refreshed
        .as_ref()
        .is_ok_and(|s| s.hash == pending.snapshot.hash);
    if pdf.is_empty() || !ready {
        let open = app.open_mut()?;
        core::finish(
            &mut open.book,
            session.actor,
            c,
            &pending.attempt.id,
            core::ResultRecord::Refused(if ready {
                core::Failure::Preparation
            } else {
                core::Failure::Stale
            }),
        )?;
        app.messages.push(
            "The invoice changed or its PDF could not be prepared. Nothing was submitted to Gmail."
                .into(),
        );
    } else {
        {
            let open = app.open_mut()?;
            core::mark_submitting(
                &mut open.book,
                session.actor,
                c,
                &pending.attempt.id,
                &pending.snapshot.hash,
                pdf,
            )?;
        }
        crate::net::gmail::deliver(app, c, pending.attempt, pending.snapshot.payload, pdf)?;
    }
    Ok(Outcome::Redirect {
        location: preview_path(c, &invoice),
    })
}
