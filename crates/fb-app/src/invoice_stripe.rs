//! Company-scoped Stripe screens and opaque payment reviews. Refresh never posts.
use crate::net::stripe as net;
use crate::{App, AppError, AppResult, Outcome, identity::Permission};
use fb_core::{CompanyId, invoice_stripe as core};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
#[derive(Default)]
pub(crate) struct State {
    reviews: HashMap<String, Pending>,
    refreshed: HashMap<(CompanyId, String), Instant>,
    pub status: HashMap<CompanyId, String>,
}
struct Pending {
    company: CompanyId,
    payment: String,
    generation: u64,
    at: Instant,
    review: core::PaymentReview,
}
fn invalid(s: &str) -> AppError {
    AppError::Invalid(s.into())
}
fn field<'a>(fields: &'a [(String, String)], key: &str) -> &'a str {
    fields
        .iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .unwrap_or("")
}
pub(crate) fn page(c: CompanyId) -> String {
    format!("/c/{c}/stripe/")
}
pub(crate) fn settings(app: &mut App, c: CompanyId, vm: &mut Value) -> AppResult<()> {
    if !app.invoice_delivery {
        return Ok(());
    }
    let o = app.open_mut()?;
    let mut rows = vec![];
    for mode in ["TEST", "LIVE"] {
        let conn = o.book.read(|q| core::current_connection(q, c, mode))?;
        rows.push(json!({"mode":mode,"revision":conn.as_ref().and_then(|v|v["revision"].as_i64()).unwrap_or(0),"connection":conn}));
    }
    vm["stripe_connections"] = json!(rows);
    vm["stripe_enabled"] = json!(true);
    vm["stripe_keyring"] = json!(app.os_store_usable && app.vault_key.is_some());
    vm["stripe_page"] = json!(page(c));
    Ok(())
}
pub(crate) fn view(app: &mut App, c: CompanyId, ticket: Option<&str>) -> AppResult<Value> {
    app.authorize(Some(c), Permission::Read)?;
    if !app.invoice_delivery {
        return Err(invalid("Stripe invoicing is not available in this build."));
    }
    let o = app.open_mut()?;
    let links = o.book.read(|q| core::links(q, o.owner, c))?;
    let mut payments = o.book.read(|q| core::payments(q, o.owner, c))?;
    for p in &mut payments {
        let disputed=p["disputed"].as_bool().unwrap_or(p["disputed"].as_i64().unwrap_or(0)!=0);
        let refunded=p["refunded"].as_i64().unwrap_or(0)>0;
        let posted=p["receipt_id"].as_str().is_some_and(|id| !id.is_empty());
        let ready=p["mode"]=="LIVE" && p["provider_state"]=="READY" && !posted && !disputed && !refunded;
        p["review_ready"]=json!(ready);
        p["display_status"]=json!(if disputed {"Disputed"} else if refunded {"Refunded"} else if posted {"Posted"} else if ready {"Awaiting review"} else {"Downloaded"});
        p["attention"]=json!(disputed || refunded || ready);
        p["amount_display"] =
            json!(fb_core::Money::from_db(p["amount"].as_i64().unwrap()).to_string());
        p["refunded_display"] =
            json!(fb_core::Money::from_db(p["refunded"].as_i64().unwrap()).to_string());
        if let Some(l) = links.iter().find(|l| l["id"] == p["link_id"]) {
            p["invoice_url"] = json!(format!(
                "/c/{c}/invoices/{}/",
                l["invoice_id"].as_str().unwrap()
            ));
        }
    }
    let mut vm = json!({"links":links,"payments":payments,"today":(app.today)().to_string(),"refresh_status":app.stripe.status.get(&c),"invoice_delivery":true});
    if let Some(ticket) = ticket {
        let p = app
            .stripe
            .reviews
            .get(ticket)
            .filter(|p| {
                p.company == c
                    && p.generation == app.session_generation()
                    && p.at.elapsed() < Duration::from_secs(300)
            })
            .ok_or_else(|| invalid("Open a fresh Stripe payment review."))?;
        vm["review"] = p.review.display.clone();
        vm["review_token"] = json!(ticket);
    }
    vm["invoice_notices"] = json!(std::mem::take(&mut app.messages));
    Ok(vm)
}
pub(crate) fn submit(
    app: &mut App,
    c: CompanyId,
    path: &str,
    fields: &[(String, String)],
) -> AppResult<Outcome> {
    app.authorize(Some(c), Permission::Write)?;
    net::require(app)?;
    let action = field(fields, "action");
    let id = field(fields, "id");
    let revision = || {
        field(fields, "stripe_revision")
            .parse::<i64>()
            .map_err(|_| invalid("Reopen settings before changing Stripe."))
    };
    match action {
        "stripe_connect" => {
            net::connect(
                app,
                c,
                field(fields, "stripe_mode"),
                revision()?,
                field(fields, "stripe_key"),
            )?;
            app.messages
                .push("Stripe is connected for this company and environment.".into());
        }
        "stripe_disconnect" => {
            net::disconnect(app, c, field(fields, "stripe_mode"), revision()?)?;
            app.messages
                .push("Stripe is disconnected. Its history is retained.".into());
        }
        "stripe_create" => {
            net::create(app, c, id)?;
        }
        "stripe_recover" => {
            let found = net::recover(app, c, id)?;
            app.messages.push(if found{"Stripe link recovered."}else{"No matching Stripe link was found. The request stays unresolved to prevent duplicate payment requests."}.into());
        }
        "stripe_resolve_missing" => {
            if field(fields, "checked_stripe") != "on" {
                return Err(invalid(
                    "Check the Stripe dashboard before resolving this request.",
                ));
            }
            if net::recover(app, c, id)? {
                return Err(invalid(
                    "Stripe has this payment link. Close it before changing the invoice.",
                ));
            }
            let o = app.open_mut()?;
            core::resolve_uncreated(&mut o.book, o.owner, c, id)?;
            app.messages.push(
                "The checked request is closed as not created. You can prepare a new link.".into(),
            );
        }
        "stripe_deactivate" => {
            net::deactivate(app, c, id)?;
            app.messages.push("Payment link and open checkouts are closed. Downloaded payments still require review.".into());
        }
        "stripe_refresh" => {
            net::refresh_all(app, c, id)?;
            app.messages
                .push("Stripe status refreshed. No payment has been posted.".into());
        }
        "stripe_refresh_company" => {
            let links = {
                let o = app.open_mut()?;
                o.book.read(|q| core::links(q, o.owner, c))?
            };
            let mut count = 0;
            for l in links.into_iter().filter(|l| {
                matches!(
                    l["state"].as_str(),
                    Some("READY" | "INACTIVE" | "REACTIVATED")
                )
            }) {
                net::refresh_all(app, c, l["id"].as_str().unwrap())?;
                count += 1;
            }
            app.messages.push(format!(
                "Refreshed {count} payment links. Payments still require review."
            ));
        }
        "stripe_review" => {
            let p = {
                let o = app.open_mut()?;
                o.book.read(|q| core::payment(q, c, id))?
            };
            if p["mode"] != "LIVE" || p["provider_state"] != "READY" || p["receipt_id"].is_string()
            {
                return Err(invalid(
                    "Only an unposted successful live payment can be reviewed.",
                ));
            }
            net::deactivate(app, c, p["link_id"].as_str().unwrap())?;
            let review = {
                let o = app.open_mut()?;
                core::review_payment(&mut o.book, o.owner, c, id, field(fields, "date"))?
            };
            let ticket = fb_ids::new_v7().to_string();
            app.stripe
                .reviews
                .retain(|_, p| p.at.elapsed() < Duration::from_secs(300));
            if app.stripe.reviews.len() >= 10 {
                app.stripe.reviews.clear();
            }
            app.stripe.reviews.insert(
                ticket.clone(),
                Pending {
                    company: c,
                    payment: id.into(),
                    generation: app.session_generation(),
                    at: Instant::now(),
                    review,
                },
            );
            return Ok(Outcome::Redirect {
                location: format!("{}?review={ticket}", page(c)),
            });
        }
        "stripe_post" => {
            let pending = app
                .stripe
                .reviews
                .remove(field(fields, "review_token"))
                .filter(|p| {
                    p.company == c
                        && p.generation == app.session_generation()
                        && p.at.elapsed() < Duration::from_secs(300)
                })
                .ok_or_else(|| invalid("Open a fresh Stripe payment review."))?;
            let p = {
                let o = app.open_mut()?;
                o.book.read(|q| core::payment(q, c, &pending.payment))?
            };
            net::refresh_all(app, c, p["link_id"].as_str().unwrap())?;
            let o = app.open_mut()?;
            core::post_payment(&mut o.book, o.owner, c, &pending.review)?;
            app.messages.push("Stripe payment recorded in Undeposited Funds and applied to the invoice. Fees and payouts are recorded separately.".into());
        }
        _ => return Err(invalid("Unknown Stripe action.")),
    }
    Ok(Outcome::Redirect {
        location: if action == "stripe_post" {
            page(c)
        } else {
            path.into()
        },
    })
}
/// Trusted desktop timer, absent from renderer IPC. One bounded read-only provider
/// page per tick; saved cursors continue catch-up after interruption or restart.
pub fn desktop_tick(app: &mut App, company: &str) -> AppResult<()> {
    let Some(c) = CompanyId::parse(company) else {
        return Ok(());
    };
    if !app.invoice_delivery
        || app.backups.viewing_restored().is_some()
        || app.authorize(Some(c), Permission::Write).is_err()
        || net::require(app).is_err()
    {
        return Ok(());
    }
    let links = {
        let o = app.open_mut()?;
        o.book.read(|q| core::links(q, o.owner, c))?
    };
    let next = next_link(links, &app.stripe.refreshed, c);
    if let Some(l) = next {
        let id = l["id"].as_str().unwrap();
        match net::refresh_page(app, c, id, false) {
            Ok(done) => {
                if done {
                    app.stripe.refreshed.insert((c, id.into()), Instant::now());
                }
                app.stripe.status.insert(
                    c,
                    "Stripe status checked. Open payments below to review before posting.".into(),
                );
            }
            Err(_) => {
                app.stripe.refreshed.insert((c, id.into()), Instant::now());
                app.stripe.status.insert(
                    c,
                    "Stripe could not refresh. Use Refresh or reconnect in company settings."
                        .into(),
                );
            }
        }
    }
    Ok(())
}

fn next_link(
    links: Vec<Value>,
    refreshed: &HashMap<(CompanyId, String), Instant>,
    c: CompanyId,
) -> Option<Value> {
    links
        .into_iter()
        .filter(|l| {
            matches!(
                l["state"].as_str(),
                Some("READY" | "INACTIVE" | "REACTIVATED")
            ) && refreshed
                .get(&(c, l["id"].as_str().unwrap().into()))
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(900))
        })
        .min_by_key(|l| {
            refreshed
                .get(&(c, l["id"].as_str().unwrap().into()))
                .copied()
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catch_up_reaches_new_links_before_revisiting_thirty_old_links() {
        let c = CompanyId::parse("00000000-0000-7000-8000-000000000001").unwrap();
        let old = Instant::now() - Duration::from_secs(1800);
        let mut refreshed = HashMap::new();
        let mut links = vec![];
        for n in 0..35 {
            let id = format!("link{n}");
            links.push(json!({"id":id,"state":"READY"}));
            if n < 30 {
                refreshed.insert((c, id), old);
            }
        }
        for n in 30..35 {
            let selected = next_link(links.clone(), &refreshed, c).unwrap();
            assert_eq!(selected["id"], format!("link{n}"));
            refreshed.insert((c, selected["id"].as_str().unwrap().into()), Instant::now());
        }
        assert_eq!(next_link(links, &refreshed, c).unwrap()["id"], "link0");
    }
}
