use super::*;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug)]
pub enum NewAccountKind {
    Checking,
    Savings,
    CreditCard,
}

#[derive(Clone, Debug)]
pub enum MappingTarget {
    Existing(Option<AccountId>),
    New { name: String, kind: NewAccountKind },
}

#[derive(Clone, Debug)]
pub struct ImportSettings {
    pub feed_account_id: String,
    pub expected_state: String,
    pub expected_date: Option<String>,
    pub import_from_date: Option<String>,
}

/// Save the mapping form in one Writer transaction, including any newly created accounts.
/// A rejected row rolls back every account, mapping and audit event from this submission.
pub fn save_mappings(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    connection_id: &str,
    mappings: &[(String, MappingTarget)],
    secrets: &SecretSet,
) -> FbResult<()> {
    save_mappings_and_import_settings(book, actor, co, connection_id, mappings, &[], secrets)
}

/// Apply mappings and explicitly submitted import settings in the same Writer.
pub fn save_mappings_and_import_settings(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    connection_id: &str,
    mappings: &[(String, MappingTarget)],
    settings: &[ImportSettings],
    secrets: &SecretSet,
) -> FbResult<()> {
    command(book, secrets, |w| {
        let user = services::access(w.tx, actor, co)?;
        let connection = connection_row(w.tx, co, connection_id)?;
        let mut configured = HashSet::new();
        for change in settings {
            if connection["active"] != true || connection["needs_reconnect"] == true {
                return Err(err("Choose an active bank connection."));
            }
            let f = feed(w.tx, &change.feed_account_id)?
                .filter(|f| f.connection_id == connection_id && f.active)
                .ok_or_else(|| err("Choose an active downloaded account of this connection."))?;
            if !configured.insert(&change.feed_account_id) {
                return Err(err("Choose each downloaded account only once."));
            }
            if f.import_state != change.expected_state || f.import_from_date != change.expected_date {
                return Err(err("The import settings changed. Reload Bank feed accounts and try again."));
            }
            let date = staging::opt_date(&change.import_from_date)?;
            if f.import_state == "ACTIVE" {
                if f.import_from_date != date {
                    return Err(err("The import start is fixed after the first completed transaction import."));
                }
                continue;
            }
            if f.import_state == "READY" && f.import_from_date == date {
                continue;
            }
            w.execute("UPDATE bankfeeds_account SET import_state='READY',import_from_date=?2 WHERE id=?1",
                params![f.id, date])?;
            w.audit(&user, co, "bankfeed.import_start_changed", format!("bankfeedaccount:{}", f.id),
                json!({"before":{"state":f.import_state,"date":f.import_from_date},
                    "after":{"state":"READY","date":date}}))?;
        }
        let mut targets = HashSet::new();
        for (_, target) in mappings {
            if let MappingTarget::Existing(Some(id)) = target && !targets.insert(*id) {
                return Err(err("The same ledger account was selected for two downloaded accounts. Choose a different account for each. No mappings were saved."));
            }
        }
        let mut seen = HashSet::new();
        let mut new_banks = Vec::new();
        for (feed_id, target) in mappings {
            let f = feed(w.tx, feed_id)?
                .filter(|f| f.company_id == co.to_db() && f.connection_id == connection_id)
                .ok_or_else(|| err("Choose a downloaded account of this connection."))?;
            if !seen.insert(feed_id) {
                return Err(err("Choose each downloaded account only once."));
            }
            let wanted = match target {
                MappingTarget::Existing(id) => id.map(|id| id.to_db()),
                MappingTarget::New { name, kind } => {
                    let (start, detail, description) = match kind {
                        NewAccountKind::Checking => (1000, "BANK", "Checking account"),
                        NewAccountKind::Savings => (1100, "BANK", "Savings account"),
                        NewAccountKind::CreditCard => (2100, "CREDIT_CARD", "Credit card account"),
                    };
                    // Pigeonhole bound: N existing codes cannot occupy N+1 candidates.
                    let codes: HashSet<String> = store::accounts(w.tx, co)?
                        .into_iter()
                        .map(|a| a.code)
                        .collect();
                    let code = (start..=start + codes.len())
                        .map(|n| n.to_string())
                        .find(|n| !codes.contains(n))
                        .ok_or_else(|| err("No account number is available."))?;
                    let name = text(&json!(name), 120, true)?;
                    let mut input =
                        services::AccountInput::new(&code, &w.text(&name)).detail(detail);
                    input.description = Some(description.into());
                    let id = services::save_account_tx(w.tx, actor, co, &input)?
                        .id
                        .to_db();
                    if detail == "BANK" {
                        new_banks.push(json!(id));
                    }
                    Some(id)
                }
            };
            if wanted != f.ledger_account_id {
                map_account_tx(w, actor, co, feed_id, &json!(wanted))?;
            }
        }
        // Saving a Bank-type account already made it a bank account when a setup exists.
        let listed = documents::setup_snapshot(w.tx, co)?["banks"].as_array().cloned().unwrap_or_default();
        new_banks.retain(|b| !listed.contains(b));
        if !new_banks.is_empty() {
            // Checks/deposits require bank accounts. Keep every existing role and bank,
            // and audit this addition in the same transaction as creation and mapping.
            let mut setup = documents::setup_snapshot(w.tx, co)?;
            let revision = setup["revision"].as_i64();
            setup["banks"]
                .as_array_mut()
                .ok_or_else(|| err("Invalid bank account setup."))?
                .extend(new_banks);
            documents::save_setup_tx(w.tx, actor, co, &setup, revision)?;
        }
        Ok(())
    })
}
