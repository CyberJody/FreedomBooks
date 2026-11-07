//! Starter accounts are installed only while creating a company, in its creation transaction.
use crate::{CompanyId, FbResult, UserId, services};
use fb_db::Writer;
use serde::Deserialize;
use serde_json::json;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Choice {
    #[default]
    Empty,
    Standard,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StarterAccount {
    pub code: String,
    pub name: String,
    pub detail_type: String,
    pub is_contra: bool,
    pub description: String,
}

pub(crate) fn accounts() -> Vec<StarterAccount> {
    serde_json::from_str(include_str!("../data/standard_chart.json"))
        .expect("the standard chart is validated by tests")
}

pub(crate) fn create_tx(tx: &Writer<'_>, actor: UserId, company: CompanyId) -> FbResult<()> {
    let accounts = accounts();
    let mut ids = Vec::with_capacity(accounts.len());
    for a in accounts {
        let input = services::AccountInput::new(&a.code, &a.name)
            .detail(&a.detail_type)
            .contra(a.is_contra)
            .description(&a.description);
        ids.push(
            services::save_account_tx(tx, actor, company, &input)?
                .id
                .to_db(),
        );
    }
    let user = services::access(tx, actor, company)?;
    services::audit(
        tx,
        &user,
        company,
        "company.starter_chart_created",
        format!("company:{company}"),
        json!({"chart":"standard", "version":1, "accounts":ids}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DetailType, Kind};
    use std::collections::HashSet;

    #[test]
    fn standard_chart_has_unique_valid_accounts_and_schedule_c_notes() {
        let accounts = accounts();
        assert_eq!(accounts.len(), 34);
        let mut names = HashSet::new();
        let mut codes = HashSet::new();
        for a in &accounts {
            assert!(
                names.insert(a.name.to_lowercase()),
                "duplicate name: {}",
                a.name
            );
            assert!(codes.insert(&a.code), "duplicate code: {}", a.code);
            assert!(a.code.bytes().all(|b| b.is_ascii_digit()) && a.code.len() == 4);
            let detail = DetailType::parse(&a.detail_type).expect("valid account type");
            assert!(!a.is_contra || !detail.refuses_contra());
            assert!(!a.name.is_empty() && a.name.chars().count() <= 120);
            assert!(a.description.chars().count() <= 200);
            if matches!(detail.kind(), Kind::Income | Kind::Expense) {
                assert!(
                    a.description.starts_with("Schedule C:"),
                    "missing mapping: {}",
                    a.name
                );
            }
        }
        // Reports compute retained earnings; a ledger account would count earnings twice.
        assert!(!accounts.iter().any(|a| a.name == "Retained Earnings"));
    }
}
