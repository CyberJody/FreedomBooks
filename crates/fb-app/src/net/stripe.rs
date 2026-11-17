//! Stripe credentials and fixed provider operations. Never posts accounting entries.
use super::*;
use crate::identity::Permission;
use fb_core::invoice_stripe as core;
fn invalid(s: &str) -> AppError {
    AppError::Invalid(s.into())
}
pub(crate) fn require(app: &App) -> AppResult<()> {
    if app.backups.viewing_restored().is_some() {
        return Err(invalid(
            "Stripe is disabled while viewing a restored backup. Return to your current books to use Stripe.",
        ));
    }
    if !app.invoice_delivery {
        return Err(invalid("Stripe invoicing is not available in this build."));
    }
    if !app.os_store_usable || app.vault_key.is_none() {
        return Err(invalid(
            "Stripe connections need this computer’s keyring. Unlock it and restart FreedomBooks.",
        ));
    }
    Ok(())
}
fn stripe_scope(c: CompanyId, mode: &str) -> AppResult<CredentialScope> {
    if !["TEST", "LIVE"].contains(&mode) {
        return Err(invalid("Choose Test or Live."));
    }
    scope(
        if mode == "LIVE" {
            "stripe-live"
        } else {
            "stripe-test"
        },
        c,
        &c.to_db(),
    )
}
pub(crate) fn connect(
    app: &mut App,
    c: CompanyId,
    mode: &str,
    revision: i64,
    key: &str,
) -> AppResult<()> {
    app.authorize(Some(c), Permission::Administer)?;
    require(app)?;
    let scope = stripe_scope(c, mode)?;
    let prefix = if mode == "LIVE" {
        ["rk_live_", "sk_live_"]
    } else {
        ["rk_test_", "sk_test_"]
    };
    if !prefix.iter().any(|p| key.starts_with(p))
        || !(16..=512).contains(&key.len())
        || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(invalid(
            "Enter a Stripe secret or restricted key for the selected environment.",
        ));
    }
    {
        let o = app.open_mut()?;
        o.book
            .read(|q| core::check_connection_revision(q, c, mode, revision))?;
    }
    let mut record = json!({"version":1,"key":key,"mode":mode,"account":""});
    let mut request = json!({"op":"stripe_probe","record":record.to_string(),"stripe":{}});
    let result = net_call(app, &request);
    request.zeroize_strings();
    let reply = match result {
        Ok(v) if v["ok"] == true && v["mode"] == mode => v,
        _ => {
            record.zeroize_strings();
            return Err(invalid(
                "Stripe could not verify this key. Check its environment and account permissions.",
            ));
        }
    };
    let account = reply["account"]
        .as_str()
        .ok_or_else(|| invalid("Stripe did not return an account."))?;
    let old_conn = {
        let o = app.open_mut()?;
        o.book.read(|q| core::current_connection(q, c, mode))?
    };
    if old_conn
        .as_ref()
        .is_some_and(|v| v["status"] == "CONNECTED" && v["account_key"] != account)
    {
        record.zeroize_strings();
        return Err(invalid(
            "Disconnect the current Stripe account before changing accounts.",
        ));
    }
    record["account"] = json!(account);
    let encoded = Zeroizing::new(record.to_string());
    record.zeroize_strings();
    let old = load(app, &scope)?.ok().flatten();
    with_vault(app, |v| {
        v.save(&scope, SecretBytes::new(encoded.as_bytes().to_vec()))
    })?
    .map_err(|_| invalid("Stripe access could not be saved in these books."))?;
    let outcome = {
        let o = app.open_mut()?;
        core::connect(
            &mut o.book,
            o.owner,
            c,
            mode,
            revision,
            account,
            &key[key.len() - 4..],
        )
    };
    if let Err(e) = outcome {
        let _ = with_vault(app, |v| match old {
            Some(old) => v.save(&scope, old),
            None => v.forget(&scope),
        });
        return Err(e.into());
    }
    Ok(())
}
pub(crate) fn disconnect(app: &mut App, c: CompanyId, mode: &str, revision: i64) -> AppResult<()> {
    app.authorize(Some(c), Permission::Administer)?;
    require(app)?;
    {
        let o = app.open_mut()?;
        let conn = o
            .book
            .read(|q| core::current_connection(q, c, mode))?
            .ok_or_else(|| invalid("Stripe is not connected."))?;
        core::disconnect(
            &mut o.book,
            o.owner,
            c,
            conn["id"].as_str().unwrap(),
            revision,
        )?;
    }
    with_vault(app, |v| {
        v.forget(&stripe_scope(c, mode).expect("validated mode"))
    })?
    .map_err(|_| invalid("Stripe is disconnected, but its saved key could not be removed."))?;
    Ok(())
}
fn access_record(app: &mut App, c: CompanyId, conn: &Value) -> AppResult<SecretBytes> {
    app.authorize(Some(c), Permission::Write)?;
    require(app)?;
    if app.net.is_none() {
        return Err(invalid("The Stripe network helper is unavailable."));
    }
    if conn["status"] != "CONNECTED" {
        return Err(invalid("Reconnect Stripe in company settings."));
    }
    let scope = stripe_scope(c, conn["mode"].as_str().unwrap_or(""))?;
    let record = load(app, &scope)?
        .map_err(|_| invalid("Reconnect Stripe on this computer."))?
        .ok_or_else(|| invalid("Connect Stripe in company settings."))?;
    let mut decoded: Value = serde_json::from_slice(record.expose())
        .map_err(|_| invalid("Reconnect Stripe on this computer."))?;
    let valid = decoded["account"] == conn["account_key"]
        && decoded["mode"] == conn["mode"]
        && decoded["key"]
            .as_str()
            .is_some_and(|k| k.ends_with(conn["key_last4"].as_str().unwrap_or("!")));
    decoded.zeroize_strings();
    if !valid {
        return Err(invalid(
            "Stripe settings and saved access differ. Reconnect Stripe.",
        ));
    }
    Ok(record)
}
fn call(
    app: &mut App,
    c: CompanyId,
    conn: &Value,
    op: &str,
    link: Option<&Value>,
    cursor: &str,
) -> AppResult<Value> {
    let record = access_record(app, c, conn)?;
    let mut request = json!({"op":op,"record":std::str::from_utf8(record.expose()).unwrap_or(""),"stripe":{"link":link.map(core::request).transpose()?,"cursor":cursor}});
    if op == "stripe_conflicts" {
        let o = app.open_mut()?;
        request["stripe"]["known_links"] = json!(
            o.book
                .read(|q| core::links(q, o.owner, c))?
                .iter()
                .filter(|v| v["state"] == "INACTIVE")
                .map(|v| json!({"request":v["id"],"provider":v["provider_key"]}))
                .collect::<Vec<_>>()
        );
    }
    let result = net_call(app, &request);
    request.zeroize_strings();
    match result {
        Ok(v) if v["ok"] == true => Ok(v),
        _ => Err(invalid(
            "Stripe refresh was not confirmed. Check the connection and try again; nothing was posted.",
        )),
    }
}
fn connection(app: &mut App, c: CompanyId, link: &Value) -> AppResult<Value> {
    let o = app.open_mut()?;
    Ok(o.book
        .read(|q| core::connection(q, c, link["connection_id"].as_str().unwrap_or("")))?)
}
fn get_link(app: &mut App, c: CompanyId, id: &str) -> AppResult<Value> {
    let o = app.open_mut()?;
    Ok(o.book.read(|q| core::link(q, c, id))?)
}
pub(crate) fn create(app: &mut App, c: CompanyId, invoice: &str) -> AppResult<()> {
    app.authorize(Some(c), Permission::Write)?;
    require(app)?;
    let conn = {
        let o = app.open_mut()?;
        let mode = o
            .book
            .read(|q| fb_core::invoice_preferences::get(q, c))?
            .settings
            .stripe_mode;
        o.book
            .read(|q| core::current_connection(q, c, &mode))?
            .ok_or_else(|| invalid("Connect Stripe in company settings."))?
    };
    let _preflight = access_record(app, c, &conn)?;
    // Existing durable requests are recovered by metadata, never re-created blindly.
    let previous = {
        let o = app.open_mut()?;
        o.book
            .read(|q| core::links(q, o.owner, c))?
            .into_iter()
            .map(|v| v["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let link = {
        let o = app.open_mut()?;
        core::prepare_link(&mut o.book, o.owner, c, invoice)?
    };
    let id = link["id"].as_str().unwrap();
    if link["state"] == "READY" {
        return Ok(());
    }
    if previous.iter().any(|v| v == id) {
        recover(app, c, id)?;
        return Ok(());
    }
    let conn = connection(app, c, &link)?;
    // A restored/copy book can lack a request created since its snapshot. Reconcile
    // remote invoice metadata before a new POST, even after the restored flag is gone.
    if let Err(error) = check_known_requests(app, c, &conn, &link) {
        let o = app.open_mut()?;
        core::finish_link(
            &mut o.book,
            o.owner,
            c,
            id,
            link["revision"].as_i64().unwrap(),
            core::LinkResult::Refused,
        )?;
        return Err(error);
    }
    let result = match call(app, c, &conn, "stripe_create", Some(&link), "") {
        Ok(v) if v["outcome"] == "created" => serde_json::from_value(v["link"].clone())
            .map(core::LinkResult::Created)
            .unwrap_or(core::LinkResult::Unknown),
        Ok(v) if v["outcome"] == "refused" => core::LinkResult::Refused,
        _ => core::LinkResult::Unknown,
    };
    let o = app.open_mut()?;
    let stored = core::finish_link(
        &mut o.book,
        o.owner,
        c,
        id,
        link["revision"].as_i64().unwrap(),
        result,
    )?;
    if stored["state"] != "READY" {
        app.messages.push("The payment link was not confirmed. Recover its status before creating another request.".into())
    }
    Ok(())
}
/// Full metadata scan. A missing result never implies that an ambiguous POST was safe to repeat.
pub(crate) fn recover(app: &mut App, c: CompanyId, id: &str) -> AppResult<bool> {
    let link = get_link(app, c, id)?;
    if !matches!(link["state"].as_str(), Some("CREATING" | "UNKNOWN")) {
        return Ok(true);
    }
    let conn = connection(app, c, &link)?;
    let mut cursor = String::new();
    for _ in 0..20 {
        let reply = call(app, c, &conn, "stripe_recover", Some(&link), &cursor)?;
        if reply["found"].is_object() {
            let remote = serde_json::from_value(reply["found"].clone())
                .map_err(|_| invalid("Stripe returned invalid link evidence."))?;
            let o = app.open_mut()?;
            core::finish_link(
                &mut o.book,
                o.owner,
                c,
                id,
                link["revision"].as_i64().unwrap(),
                core::LinkResult::Created(remote),
            )?;
            return Ok(true);
        }
        let next = reply["next"]
            .as_str()
            .ok_or_else(|| invalid("Invalid Stripe page."))?;
        if next.is_empty() {
            return Ok(false);
        }
        if next == cursor {
            return Err(invalid("Stripe repeated a page."));
        }
        cursor = next.into();
    }
    Err(invalid(
        "Stripe recovery needs more than 2,000 links. The request remains unresolved; no duplicate was created.",
    ))
}
/// Returns true only at the end of a complete session scan. A saved cursor catches up after interruption.
pub(crate) fn refresh_page(
    app: &mut App,
    c: CompanyId,
    id: &str,
    restart: bool,
) -> AppResult<bool> {
    fetch_page(app, c, id, restart, false)
}
fn fetch_page(
    app: &mut App,
    c: CompanyId,
    id: &str,
    restart: bool,
    closing: bool,
) -> AppResult<bool> {
    let link = get_link(app, c, id)?;
    let conn = connection(app, c, &link)?;
    let cursor = if restart {
        ""
    } else {
        link["cursor"].as_str().unwrap_or("")
    };
    let reply = call(app, c, &conn, "stripe_refresh", Some(&link), cursor)?;
    if (link["state"] == "INACTIVE" || closing) && reply["link"]["active"] != false {
        let o = app.open_mut()?;
        core::mark_reactivated(
            &mut o.book,
            o.owner,
            c,
            id,
            link["revision"].as_i64().unwrap(),
        )?;
        return Err(invalid(
            "This link was reactivated in Stripe. Close it again before changing the invoice or reviewing payments.",
        ));
    }
    let observations: Vec<core::Observation> =
        serde_json::from_value(reply["observations"].clone())
            .map_err(|_| invalid("Stripe returned invalid payment evidence."))?;
    let next = reply["next"]
        .as_str()
        .ok_or_else(|| invalid("Stripe returned an invalid page."))?;
    let o = app.open_mut()?;
    core::stage_page(
        &mut o.book,
        o.owner,
        c,
        id,
        link["revision"].as_i64().unwrap(),
        &observations,
        next,
    )?;
    Ok(next.is_empty())
}
pub(crate) fn refresh_all(app: &mut App, c: CompanyId, id: &str) -> AppResult<()> {
    scan(app, c, id, false)
}
fn scan(app: &mut App, c: CompanyId, id: &str, closing: bool) -> AppResult<()> {
    for n in 0..20 {
        if fetch_page(app, c, id, n == 0, closing)? {
            return Ok(());
        }
    }
    Err(invalid(
        "This link has more than 2,000 checkout sessions. No payment was posted; reconcile it separately before changing the invoice.",
    ))
}
pub(crate) fn deactivate(app: &mut App, c: CompanyId, id: &str) -> AppResult<()> {
    let link = get_link(app, c, id)?;
    let conn = connection(app, c, &link)?;
    let mut closed = false;
    for _ in 0..4 {
        if call(app, c, &conn, "stripe_deactivate", Some(&link), "")?["closed"] == true {
            closed = true;
            break;
        }
    }
    if !closed {
        return Err(invalid(
            "More open checkouts remain. Deactivate again to finish closing the link.",
        ));
    }
    // Stage completed and delayed payments BEFORE releasing local balance guards.
    scan(app, c, id, true)?;
    let current = get_link(app, c, id)?;
    let o = app.open_mut()?;
    core::mark_deactivated(
        &mut o.book,
        o.owner,
        c,
        id,
        current["revision"].as_i64().unwrap(),
    )?;
    Ok(())
}

fn check_known_requests(app: &mut App, c: CompanyId, conn: &Value, link: &Value) -> AppResult<()> {
    let mut cursor = String::new();
    for _ in 0..20 {
        let v = call(app, c, conn, "stripe_conflicts", Some(link), &cursor)?;
        if let Some(blocking) = v["blocking"].as_array() {
            for row in blocking {
                let id = row["request"]
                    .as_str()
                    .ok_or_else(|| invalid("Invalid Stripe conflict evidence."))?;
                let old = get_link(app, c, id)?;
                if old["provider_key"] != row["provider"] || old["invoice_id"] != link["invoice_id"]
                {
                    return Err(invalid("Invalid Stripe conflict scope."));
                }
                let o = app.open_mut()?;
                core::mark_reactivated(
                    &mut o.book,
                    o.owner,
                    c,
                    id,
                    old["revision"].as_i64().unwrap(),
                )?;
            }
        }
        if v["conflict"] != false {
            return Err(invalid(
                "Stripe has a payment request for this invoice that is missing from these books. Reconcile it using your current books before creating another link.",
            ));
        }
        let next = v["next"]
            .as_str()
            .ok_or_else(|| invalid("Stripe returned an invalid page."))?;
        if next.is_empty() {
            return Ok(());
        }
        if next == cursor {
            return Err(invalid("Stripe repeated a page."));
        }
        cursor = next.into();
    }
    Err(invalid(
        "The Stripe account has more than 2,000 payment links. Creation is paused until its invoice requests can be fully reconciled.",
    ))
}
