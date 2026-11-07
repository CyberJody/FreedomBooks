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
    command(book, secrets, |w| {
        services::access(w.tx, actor, co)?;
        connection_row(w.tx, co, connection_id)?;
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
        if !new_banks.is_empty() {
            // Checks/deposits require designated banks. Keep every existing role and bank,
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
