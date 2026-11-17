use super::*;
use crate::identity::Permission;
use fb_core::invoice_email::{self as core, ConnectionState};

fn gmail_scope(c: CompanyId) -> AppResult<CredentialScope> {
    scope("gmail", c, &c.to_db())
}
pub(crate) fn begin(
    app: &mut App,
    c: CompanyId,
    revision: i64,
    client: &str,
    secret: &str,
    from: &str,
) -> AppResult<String> {
    app.authorize(Some(c), Permission::Administer)?;
    crate::invoice_email::require(app)?;
    if secret.len() > 10_000 || secret.chars().any(char::is_control) {
        return Err(AppError::Invalid(
            "Enter a valid Google desktop client secret.".into(),
        ));
    }
    if super::signin_held_by_other(app, "gmail", c) {
        return Err(AppError::Invalid(SIGNIN_JOB_TAKEN.into()));
    }
    let state = {
        let open = app.open_mut()?;
        core::begin_connection(&mut open.book, open.owner, c, revision, client, from)?
    };
    let _ = net_call(app, &json!({"op":"gmail_cancel","company":c.to_db()}));
    app.email.signins.remove(&c);
    let mut request = json!({"op":"gmail_begin","company":c.to_db(),"gmail":{"client_id":state.client_id,"client_secret":if secret.is_empty(){None}else{Some(secret)},"from_address":state.from_address}});
    let result = net_call(app, &request);
    request.zeroize_strings();
    match result {
        Ok(mut reply) if reply["ok"] == true && reply["authorization_url"].is_string() => {
            let url = reply["authorization_url"].as_str().unwrap().to_owned();
            reply.zeroize_strings();
            app.email
                .signins
                .insert(c, (app.session_generation(), state.revision));
            Ok(url)
        }
        _ => {
            let open = app.open_mut()?;
            core::set_connection_state(
                &mut open.book,
                open.owner,
                c,
                state.revision,
                ConnectionState::NeedsSignIn,
            )?;
            Err(AppError::Invalid(
                "Google sign-in could not start. Check the desktop client settings and try again."
                    .into(),
            ))
        }
    }
}
pub(crate) fn poll(app: &mut App, c: CompanyId) -> AppResult<bool> {
    app.authorize(Some(c), Permission::Administer)?;
    crate::invoice_email::require(app)?;
    let Some((generation, revision)) = app.email.signins.get(&c).copied() else {
        return Ok(false);
    };
    if generation != app.session_generation() {
        return Err(AppError::Invalid(
            "The active user changed. Connect Google again.".into(),
        ));
    }
    let mut reply = match net_call(app, &json!({"op":"gmail_poll","company":c.to_db()})) {
        Ok(reply) => reply,
        Err(_) => json!({"ok":false}),
    };
    if reply["pending"] == true {
        return Ok(false);
    }
    app.email.signins.remove(&c);
    let record = reply["record"]
        .as_str()
        .filter(|r| !r.is_empty())
        .map(|r| SecretBytes::new(r.as_bytes().to_vec()));
    let ok = reply["ok"] == true;
    reply.zeroize_strings();
    let current = {
        let open = app.open_mut()?;
        open.book.read(|q| core::connection(q, c))?
    };
    if !current.is_some_and(|v| v.revision == revision && v.status == "AUTHORIZING") {
        return Err(AppError::Invalid(
            "Google settings changed. Connect again.".into(),
        ));
    }
    if !ok || record.is_none() {
        let open = app.open_mut()?;
        core::set_connection_state(
            &mut open.book,
            open.owner,
            c,
            revision,
            ConnectionState::NeedsSignIn,
        )?;
        return Err(AppError::Invalid(
            "Google sign-in was not completed. Connect again.".into(),
        ));
    }
    let scope = gmail_scope(c)?;
    with_vault(app, |v| v.save(&scope, record.unwrap()))?.map_err(|_| {
        AppError::Invalid("Google sign-in could not be saved in these books.".into())
    })?;
    let connected = {
        let open = app.open_mut()?;
        core::set_connection_state(
            &mut open.book,
            open.owner,
            c,
            revision,
            ConnectionState::Connected,
        )
    };
    if let Err(e) = connected {
        let _ = with_vault(app, |v| v.forget(&scope));
        return Err(AppError::Core(e));
    }
    Ok(true)
}
pub(crate) fn disconnect(app: &mut App, c: CompanyId, revision: i64) -> AppResult<bool> {
    app.authorize(Some(c), Permission::Administer)?;
    crate::invoice_email::require(app)?;
    // Local disconnect wins before any network call or credential access.
    {
        let open = app.open_mut()?;
        core::set_connection_state(
            &mut open.book,
            open.owner,
            c,
            revision,
            ConnectionState::Disconnected,
        )?;
    }
    app.email.signins.remove(&c);
    let _ = net_call(app, &json!({"op":"gmail_cancel","company":c.to_db()}));
    let scope = gmail_scope(c)?;
    let old = load(app, &scope)?.ok().flatten();
    with_vault(app, |v| v.forget(&scope))?.map_err(|_| {
        AppError::Invalid(
            "The connection was disabled, but its saved access could not be removed.".into(),
        )
    })?;
    let Some(old) = old else { return Ok(false) };
    let mut request =
        json!({"op":"gmail_disconnect","record":std::str::from_utf8(old.expose()).unwrap_or("")});
    let result = net_call(app, &request);
    request.zeroize_strings();
    Ok(result.is_ok_and(|v| v["ok"] == true && v["revoked"] == true))
}

pub(crate) fn deliver(
    app: &mut App,
    c: CompanyId,
    attempt: core::Attempt,
    mut payload: Value,
    pdf: &[u8],
) -> AppResult<()> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    app.authorize(Some(c), Permission::Write)?;
    crate::invoice_email::require(app)?;
    let scope = gmail_scope(c)?;
    let record = load(app, &scope)?.ok().flatten();
    let Some(record) = record else {
        let open = app.open_mut()?;
        core::finish(
            &mut open.book,
            open.owner,
            c,
            &attempt.id,
            core::ResultRecord::Refused(core::Failure::NeedsSignIn),
        )?;
        core::set_connection_state(
            &mut open.book,
            open.owner,
            c,
            attempt.connection_revision,
            ConnectionState::NeedsSignIn,
        )?;
        app.messages
            .push("Connect Google again. Nothing was submitted to Gmail.".into());
        return Ok(());
    };
    payload["pdf"] = json!(STANDARD.encode(pdf));
    payload["mode"] = json!(attempt.mode);
    payload["message_id"] = json!(attempt.message_key);
    let mut request = json!({"op":"gmail_deliver","record":std::str::from_utf8(record.expose()).unwrap_or(""),"gmail":{"delivery":payload}});
    let result = net_call(app, &request);
    request.zeroize_strings();
    let mut reply = result.unwrap_or_else(|_| json!({"outcome":"unknown"}));
    let provider_message = reply["message_id"].as_str().unwrap_or("").to_owned();
    let provider_draft = reply["draft_id"].as_str().map(str::to_owned);
    let outcome = reply["outcome"].as_str().unwrap_or("unknown").to_owned();
    let needs_signin = reply["code"] == "needs_signin";
    let updated = reply["record"]
        .as_str()
        .filter(|r| !r.is_empty())
        .map(|r| SecretBytes::new(r.as_bytes().to_vec()));
    let provider_replied = reply["ok"] == true;
    reply.zeroize_strings();
    // Recording acceptance precedes optional token persistence. A local vault
    // failure must never turn an accepted draft/send into a retryable operation.
    let result = match outcome.as_str() {
        "accepted" => core::ResultRecord::Accepted {
            message: &provider_message,
            draft: provider_draft.as_deref(),
        },
        "refused" => core::ResultRecord::Refused(if needs_signin {
            core::Failure::NeedsSignIn
        } else {
            core::Failure::Provider
        }),
        _ => core::ResultRecord::Unknown,
    };
    let stored = {
        let open = app.open_mut()?;
        core::finish(&mut open.book, open.owner, c, &attempt.id, result)?
    };
    let credential_ok = if provider_replied {
        with_vault(app, |v| match updated {
            Some(record) => v.save(&scope, record),
            None => v.forget(&scope),
        })?
        .is_ok()
    } else {
        true
    };
    if needs_signin || !credential_ok {
        let open = app.open_mut()?;
        core::set_connection_state(
            &mut open.book,
            open.owner,
            c,
            attempt.connection_revision,
            ConnectionState::NeedsSignIn,
        )?;
    }
    app.messages.push(match stored.state.as_str(){"DRAFTED"=>"Saved to Gmail Drafts. It has not been sent.","SENT"=>"Gmail accepted the email for sending.","REFUSED"=>"Gmail did not accept this email. Check the connection before trying again.",_=>"Gmail’s result is uncertain. Check Drafts and Sent in Gmail before creating another email."}.into());
    if !credential_ok {
        app.messages.push(
            "The updated Google access could not be saved. Reconnect before the next email.".into(),
        );
    }
    Ok(())
}
