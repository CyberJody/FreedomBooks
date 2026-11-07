//! Host boundary for credentials already held in the plugin's encrypted store. The core does not
//! fetch or persist credentials. Call this for each connection when opening a book, before exposing
//! any command or making a network request. An incompatible credential leaves its connection and
//! downloaded history intact, sets a durable reconnect flag, and never becomes a usable SecretSet.
use super::*;

pub fn restore_connection_credentials<S: AsRef<str>>(
    book: &mut Book, actor: UserId, co: CompanyId, connection_id: &str,
    restore: &fb_db::credential_mask::BankfeedRestore, extra: impl IntoIterator<Item = S>,
) -> FbResult<SecretSet> {
    book.verify_bankfeed_restore(connection_id, restore)?;
    let access_url = restore.access_url();
    let flagged = book.read(|q| {
        services::access(q, actor, co)?;
        Ok::<_, FbError>(connection_row(q, co, connection_id)?["needs_reconnect"] == true)
    })?;
    let extra: Vec<String> = extra.into_iter().map(|v| v.as_ref().to_owned()).collect();
    let result = SecretSet::from_access_url(access_url, &extra);
    // Older long credentials still need masking during later lifecycle writes, even though they
    // cannot be used to fetch. This internal set is never returned to the caller on refusal.
    let installed = book.restore_bankfeed_mask(connection_id, restore, &extra);
    if result.is_ok() { installed?; }
    match result {
        Ok(secrets) if !flagged => {
            write::retain_secrets(book, &secrets)?;
            Ok(secrets)
        }
        Ok(_) => Err(err(UNSAFE_CREDENTIAL)),
        Err(error) => {
            if !flagged { flag_reconnect(book, actor, co, connection_id, &SecretSet::none())?; }
            Err(error)
        }
    }
}

/// The host's other load outcome: the book could not read this connection's stored credential at
/// all (`fb_db::Book::release_unreadable_bankfeed`). The books still open; the connection gets the
/// same durable reconnect flag as a rejected credential, so pulls and staging refuse it until the
/// user reconnects (a new connection) or forgets it. Returns the connection's label for the notice.
pub fn flag_unreadable_credential(
    book: &mut Book, actor: UserId, co: CompanyId, connection_id: &str,
    unreadable: &fb_db::credential_mask::UnreadableBankfeed,
) -> FbResult<String> {
    book.verify_unreadable_bankfeed(connection_id, unreadable)?;
    let row = book.read(|q| {
        services::access(q, actor, co)?;
        connection_row(q, co, connection_id)
    })?;
    if row["needs_reconnect"] != true {
        flag(book, actor, co, connection_id, &SecretSet::none(), "unreadable")?;
    }
    Ok(row["label"].as_str().unwrap_or(DEFAULT_LABEL).to_owned())
}

fn flag_reconnect(book: &mut Book, actor: UserId, co: CompanyId, connection_id: &str, secrets: &SecretSet) -> FbResult<()> {
    flag(book, actor, co, connection_id, secrets, "")
}

fn flag(book: &mut Book, actor: UserId, co: CompanyId, connection_id: &str, secrets: &SecretSet, credential: &str) -> FbResult<()> {
    command(book, secrets, |w| {
        let user = services::access(w.tx, actor, co)?;
        connection_row(w.tx, co, connection_id)?;
        w.execute("UPDATE bankfeeds_connection SET needs_reconnect=1 WHERE id=?1 AND company_id=?2", params![connection_id, co.to_db()])?;
        let detail = if credential.is_empty() { json!({"needs_reconnect":true}) } else { json!({"needs_reconnect":true,"credential":credential}) };
        w.audit(&user, co, "bankfeed.reconnect_required", format!("bankfeedconnection:{connection_id}"), detail)
    })
}
