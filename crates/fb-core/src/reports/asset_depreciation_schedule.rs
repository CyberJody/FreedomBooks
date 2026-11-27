//! Posted fixed-asset rollforward and an optional, run-time straight-line worksheet.

use super::financial::sorted_accounts;
use super::{check_range, check_rows};
use crate::error::{FbError, FbResult};
use crate::model::{Account, DetailType};
use crate::services::access;
use chrono::{Months, NaiveDate};
use fb_db::{Book, Query, rusqlite::params};
use fb_ids::{AccountId, CompanyId, JournalId, JournalLineId, UserId};
use fb_money::Money;
use std::collections::{BTreeSet, HashMap};
use std::ops::AddAssign;

pub const MAX_ASSETS: usize = 100;
pub const MAX_LABEL_LENGTH: usize = 80;
pub const MAX_LIFE_MONTHS: u32 = 1_200;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cost {
    pub opening: Money,
    pub additions: Money,
    pub disposals: Money,
    pub reversals: Money,
}

impl Cost {
    pub fn closing(self) -> Money {
        self.opening + self.additions - self.disposals + self.reversals
    }
}

impl AddAssign for Cost {
    fn add_assign(&mut self, other: Self) {
        self.opening += other.opening;
        self.additions += other.additions;
        self.disposals += other.disposals;
        self.reversals += other.reversals;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Accumulated {
    pub opening: Money,
    pub booked: Money,
    pub other_credits: Money,
    pub other_debits: Money,
    pub unresolved_credits: Money,
    pub unresolved_debits: Money,
}

impl Accumulated {
    pub fn period_net(self) -> Money {
        self.booked + self.other_credits - self.other_debits + self.unresolved_credits
            - self.unresolved_debits
    }

    pub fn closing(self) -> Money {
        self.opening + self.period_net()
    }
}

impl AddAssign for Accumulated {
    fn add_assign(&mut self, other: Self) {
        self.opening += other.opening;
        self.booked += other.booked;
        self.other_credits += other.other_credits;
        self.other_debits += other.other_debits;
        self.unresolved_credits += other.unresolved_credits;
        self.unresolved_debits += other.unresolved_debits;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    StraightLine,
}

impl Method {
    pub fn label(self) -> &'static str {
        match self {
            Self::StraightLine => "Straight-line",
        }
    }
}

#[derive(Clone, Debug)]
pub struct AssetInput {
    pub label: String,
    pub account: Option<AccountId>,
    pub journal: Option<JournalId>,
    pub line: Option<JournalLineId>,
    pub in_service: Option<NaiveDate>,
    pub disposal_date: Option<NaiveDate>,
    pub cost_cents: Option<i64>,
    pub salvage_cents: Option<i64>,
    pub method: Option<Method>,
    pub life_months: Option<u32>,
    pub prior_cents: Option<i64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Values {
    pub cost: Money,
    pub period_depreciation: Money,
    pub accumulated: Money,
    pub net_book_value: Money,
}

impl Values {
    pub fn difference(self, ledger: Self) -> Self {
        Self {
            cost: self.cost - ledger.cost,
            period_depreciation: self.period_depreciation - ledger.period_depreciation,
            accumulated: self.accumulated - ledger.accumulated,
            net_book_value: self.net_book_value - ledger.net_book_value,
        }
    }
}

#[derive(Debug)]
pub struct Asset {
    pub input: AssetInput,
    pub cost: Option<Money>,
    pub values: Option<Values>,
    pub disposed: bool,
    pub missing: Vec<&'static str>,
}

#[derive(Debug)]
pub struct DepreciationSchedule {
    pub cost_accounts: Vec<(Account, Cost)>,
    pub contra_accounts: Vec<(Account, Accumulated)>,
    pub expense_accounts: Vec<(Account, Money)>,
    pub cost: Cost,
    pub accumulated: Accumulated,
    pub ledger: Values,
    pub assets: Vec<Asset>,
    pub known_assets: Values,
    pub incomplete: usize,
}

pub fn asset_depreciation_schedule(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    start: NaiveDate,
    end: NaiveDate,
    inputs: &[AssetInput],
) -> FbResult<DepreciationSchedule> {
    book.read(|tx| {
        access(tx, actor, company)?;
        check_range(start, end)?;
        if inputs.len() > MAX_ASSETS {
            return Err(FbError::validation("Use at most 100 asset entries."));
        }
        let accounts = sorted_accounts(tx, company)?;
        let mut costs = HashMap::new();
        let mut contra = HashMap::new();
        for account in accounts
            .iter()
            .filter(|a| a.detail_type == DetailType::FixedAsset)
        {
            if account.is_contra {
                contra.insert(account.id.to_db(), Accumulated::default());
            } else {
                costs.insert(account.id.to_db(), Cost::default());
            }
        }
        check_rows(costs.len() + contra.len() + inputs.len() + 14)?;
        opening(tx, company, start, &mut costs, &mut contra)?;
        let mut expenses = HashMap::new();
        period(
            tx,
            company,
            start,
            end,
            &mut costs,
            &mut contra,
            &mut expenses,
        )?;
        check_rows(costs.len() + contra.len() + expenses.len() + inputs.len() + 14)?;
        let mut cost_total = Cost::default();
        let mut accum_total = Accumulated::default();
        let mut cost_accounts = Vec::new();
        let mut contra_accounts = Vec::new();
        let mut expense_accounts = Vec::new();
        for account in accounts {
            let id = account.id.to_db();
            if let Some(cost) = costs.get(&id).copied() {
                cost_total += cost;
                cost_accounts.push((account, cost));
            } else if let Some(accum) = contra.remove(&id) {
                accum_total += accum;
                contra_accounts.push((account, accum));
            } else if let Some(expense) = expenses.remove(&id) {
                expense_accounts.push((account, expense));
            }
        }
        let ledger = Values {
            cost: cost_total.closing(),
            period_depreciation: accum_total.booked,
            accumulated: accum_total.closing(),
            net_book_value: cost_total.closing() - accum_total.closing(),
        };

        let mut labels = BTreeSet::new();
        for input in inputs {
            let label = input.label.trim();
            if label.is_empty()
                || label != input.label
                || label.chars().count() > MAX_LABEL_LENGTH
                || label.chars().any(char::is_control)
                || !labels.insert(label)
            {
                return Err(FbError::validation(
                    "Each asset needs a unique label of 1 to 80 characters.",
                ));
            }
            if input
                .account
                .is_some_and(|id| !costs.contains_key(&id.to_db()))
            {
                return Err(FbError::validation(
                    "Choose a fixed-asset cost account in this company.",
                ));
            }
        }
        // Claim exact lines first, then account-restricted journals, then entire
        // journals. A broad link can only use the remaining eligible posted cents.
        let mut order: Vec<_> = (0..inputs.len()).collect();
        order.sort_by_key(|&i| (inputs[i].line.is_none(), inputs[i].account.is_none()));
        let mut claims = HashMap::new();
        let mut linked = vec![None; inputs.len()];
        for i in order {
            linked[i] = linked_cost(tx, company, end, &inputs[i], &mut claims)?;
        }
        let mut assets = Vec::new();
        let mut known_assets = Values::default();
        let mut incomplete = 0;
        for (input, linked) in inputs.iter().zip(linked) {
            let asset = calculate(input, linked, start, end)?;
            if !asset.disposed {
                known_assets.cost += asset.cost.unwrap_or_default();
            }
            if let Some(values) = asset.values {
                known_assets.period_depreciation += values.period_depreciation;
                known_assets.accumulated += values.accumulated;
                known_assets.net_book_value += values.net_book_value;
            } else {
                incomplete += 1;
            }
            assets.push(asset);
        }
        Ok(DepreciationSchedule {
            cost_accounts,
            contra_accounts,
            expense_accounts,
            cost: cost_total,
            accumulated: accum_total,
            ledger,
            assets,
            known_assets,
            incomplete,
        })
    })
}

fn opening(
    tx: &impl Query,
    company: CompanyId,
    start: NaiveDate,
    costs: &mut HashMap<String, Cost>,
    contra: &mut HashMap<String, Accumulated>,
) -> FbResult<()> {
    let mut stmt = tx.prepare(
        "SELECT l.account_id, l.debit, l.credit
         FROM ledger_journalline l JOIN ledger_journal j ON j.id=l.journal_id
         JOIN ledger_account a ON a.id=l.account_id
         WHERE j.company_id=?1 AND a.company_id=?1 AND j.status='POSTED'
           AND j.date<?2 AND a.detail_type='FIXED_ASSET'",
    )?;
    let mut rows = stmt.query(params![company.to_db(), start.to_string()])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let net = Money::from_db(row.get(1)?) - Money::from_db(row.get(2)?);
        if let Some(cost) = costs.get_mut(&id) {
            cost.opening += net;
        } else if let Some(accum) = contra.get_mut(&id) {
            accum.opening -= net;
        }
    }
    Ok(())
}

#[derive(Default)]
struct PeriodJournal {
    contra: HashMap<String, (Money, Money)>,
    expenses: HashMap<String, Money>,
    other_accounts: bool,
    expense_debits: bool,
    expense_credits: bool,
}

impl PeriodJournal {
    fn apply(
        self,
        contra: &mut HashMap<String, Accumulated>,
        expenses: &mut HashMap<String, Money>,
    ) -> FbResult<()> {
        let credits: Money = self.contra.values().map(|(credit, _)| *credit).sum();
        let debits: Money = self.contra.values().map(|(_, debit)| *debit).sum();
        let net = credits - debits;

        // Attribute only direct contra/expense entries and their reversals.
        let opposing_expense = (!credits.is_zero() && self.expense_debits)
            || (!debits.is_zero() && self.expense_credits);
        let booked = !self.other_accounts
            && (credits.is_zero() != debits.is_zero())
            && (self.expense_debits != self.expense_credits)
            && opposing_expense
            && self.expenses.values().sum::<Money>() == net;
        for (account, (credit, debit)) in self.contra {
            let values = contra.get_mut(&account).expect("loaded contra account");
            if booked {
                values.booked += credit - debit;
            } else if opposing_expense {
                values.unresolved_credits += credit;
                values.unresolved_debits += debit;
            } else {
                values.other_credits += credit;
                values.other_debits += debit;
            }
        }
        if booked {
            for (account, amount) in self.expenses {
                *expenses.entry(account).or_default() += amount;
            }
            check_rows(expenses.len())?;
        }
        Ok(())
    }
}

fn period(
    tx: &impl Query,
    company: CompanyId,
    start: NaiveDate,
    end: NaiveDate,
    costs: &mut HashMap<String, Cost>,
    contra: &mut HashMap<String, Accumulated>,
    expenses: &mut HashMap<String, Money>,
) -> FbResult<()> {
    let mut stmt = tx.prepare(
        "SELECT j.id, l.account_id, a.detail_type, l.debit, l.credit, j.reversal_of_id IS NOT NULL
         FROM ledger_journal j JOIN ledger_journalline l ON l.journal_id=j.id
         JOIN ledger_account a ON a.id=l.account_id
         WHERE j.company_id=?1 AND a.company_id=?1 AND j.status='POSTED' AND j.date BETWEEN ?2 AND ?3
           AND EXISTS (
             SELECT 1 FROM ledger_journalline f JOIN ledger_account fa ON fa.id=f.account_id
             WHERE f.journal_id=j.id AND fa.company_id=?1 AND fa.detail_type='FIXED_ASSET'
           )
         ORDER BY j.id, l.position, l.id",
    )?;
    let mut rows = stmt.query(params![company.to_db(), start.to_string(), end.to_string()])?;
    let mut journal = PeriodJournal::default();
    let mut previous = String::new();
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        if id != previous {
            std::mem::take(&mut journal).apply(contra, expenses)?;
            previous = id;
        }
        let account: String = row.get(1)?;
        let detail: String = row.get(2)?;
        let net = Money::from_db(row.get(3)?) - Money::from_db(row.get(4)?);
        if net.is_zero() {
            continue;
        }
        if let Some(cost) = costs.get_mut(&account) {
            if row.get::<_, bool>(5)? {
                cost.reversals += net;
            } else {
                cost.additions += net.max(Money::ZERO);
                cost.disposals += (-net).max(Money::ZERO);
            }
        }
        if contra.contains_key(&account) {
            let values = journal.contra.entry(account).or_default();
            values.0 += (-net).max(Money::ZERO);
            values.1 += net.max(Money::ZERO);
        } else if detail == DetailType::Expense.as_str()
            || detail == DetailType::OtherExpense.as_str()
            || detail == DetailType::CostOfGoodsSold.as_str()
        {
            journal.expense_debits |= net > Money::ZERO;
            journal.expense_credits |= net < Money::ZERO;
            *journal.expenses.entry(account).or_default() += net;
        } else {
            journal.other_accounts = true;
        }
    }
    journal.apply(contra, expenses)
}

fn linked_cost(
    tx: &impl Query,
    company: CompanyId,
    end: NaiveDate,
    input: &AssetInput,
    claims: &mut HashMap<String, Money>,
) -> FbResult<Option<Money>> {
    if input.journal.is_none() && input.line.is_none() {
        return Ok(None);
    }
    let mut stmt = tx.prepare(
        "SELECT l.id, l.debit, l.credit
         FROM ledger_journalline l JOIN ledger_journal j ON j.id=l.journal_id
         JOIN ledger_account a ON a.id=l.account_id
         WHERE j.company_id=?1 AND a.company_id=?1 AND j.status='POSTED' AND j.date<=?2
           AND j.reversal_of_id IS NULL AND NOT EXISTS (
               SELECT 1 FROM ledger_journal z WHERE z.reversal_of_id=j.id AND z.company_id=j.company_id
                 AND z.status='POSTED' AND z.date<=?2)
           AND a.detail_type='FIXED_ASSET' AND a.is_contra=0 AND l.debit>l.credit
           AND (?3 IS NULL OR j.id=?3) AND (?4 IS NULL OR l.id=?4) AND (?5 IS NULL OR a.id=?5)
         ORDER BY l.id",
    )?;
    let mut rows = stmt.query(params![
        company.to_db(),
        end.to_string(),
        input.journal.map(|id| id.to_db()),
        input.line.map(|id| id.to_db()),
        input.account.map(|id| id.to_db()),
    ])?;
    let mut lines = Vec::new();
    while let Some(row) = rows.next()? {
        let posted = Money::from_db(row.get(1)?) - Money::from_db(row.get(2)?);
        lines.push((row.get::<_, String>(0)?, posted));
        check_rows(lines.len())?;
    }
    if lines.is_empty() {
        return Err(FbError::validation(
            "Link to a posted fixed-asset cost debit in this company through the To date.",
        ));
    }
    if input.cost_cents.is_none() && lines.len() != 1 {
        return Err(FbError::validation(
            "A journal with multiple eligible cost debits needs a specific posted line ID to derive cost.",
        ));
    }

    let cost = input.cost_cents.map_or(lines[0].1, Money::from_db);
    let mut remaining = cost;
    for (id, posted) in lines {
        let claimed = claims.entry(id).or_default();
        let take = remaining.max(Money::ZERO).min(posted - *claimed);
        *claimed += take;
        remaining -= take;
    }
    if remaining > Money::ZERO {
        return Err(FbError::validation(
            "Linked asset cost exceeds the unclaimed amount on the posted cost line or journal.",
        ));
    }
    Ok(Some(cost))
}

fn calculate(
    input: &AssetInput,
    linked: Option<Money>,
    start: NaiveDate,
    end: NaiveDate,
) -> FbResult<Asset> {
    let cost = input.cost_cents.map(Money::from_db).or(linked);
    let salvage = input.salvage_cents.map(Money::from_db);
    let prior = input.prior_cents.map(Money::from_db);
    let valid = [cost, salvage, prior]
        .into_iter()
        .flatten()
        .all(|amount| amount >= Money::ZERO)
        && cost.is_none_or(|cost| salvage.unwrap_or_default() + prior.unwrap_or_default() <= cost);
    if !valid {
        return Err(FbError::validation(
            "Use nonnegative asset amounts and keep salvage plus prior depreciation within cost.",
        ));
    }
    if input
        .life_months
        .is_some_and(|life| !(1..=MAX_LIFE_MONTHS).contains(&life))
    {
        return Err(FbError::validation(
            "Use a useful life of 1 to 1,200 months.",
        ));
    }
    if let Some(in_service) = input.in_service {
        check_range(in_service, in_service)?;
        if in_service >= start && prior.is_some_and(|amount| !amount.is_zero()) {
            return Err(FbError::validation(
                "Prior depreciation must be zero for assets placed in service during or after the period.",
            ));
        }
    }
    if let Some(disposal) = input.disposal_date {
        check_range(disposal, disposal)?;
        if input.in_service.is_some_and(|service| disposal < service) {
            return Err(FbError::validation(
                "Disposal date must be on or after the in-service date.",
            ));
        }
    }
    let disposed = input.disposal_date.is_some_and(|disposal| disposal <= end);
    let missing = [
        ("cost", cost.is_none()),
        ("salvage", salvage.is_none()),
        ("prior depreciation", prior.is_none()),
        ("in-service date", input.in_service.is_none()),
        ("method", input.method.is_none()),
        ("useful life", input.life_months.is_none()),
    ]
    .into_iter()
    .filter_map(|(label, missing)| missing.then_some(label))
    .collect();
    let values = match (
        cost,
        salvage,
        prior,
        input.in_service,
        input.method,
        input.life_months,
    ) {
        (
            Some(cost),
            Some(salvage),
            Some(prior),
            Some(service),
            Some(Method::StraightLine),
            Some(months),
        ) => {
            let base = cost - salvage;
            let last = input
                .disposal_date
                .map_or(end, |disposal| disposal.min(end));
            let period_depreciation = if last < start {
                Money::ZERO
            } else {
                straight_line(base, service, months, start, last)?.min(base - prior)
            };
            let accumulated = prior + period_depreciation;
            Some(if disposed {
                Values {
                    period_depreciation,
                    ..Values::default()
                }
            } else {
                Values {
                    cost,
                    period_depreciation,
                    accumulated,
                    net_book_value: cost - accumulated,
                }
            })
        }
        _ => None,
    };
    Ok(Asset {
        input: input.clone(),
        cost,
        values,
        disposed,
        missing,
    })
}

fn straight_line(
    base: Money,
    service: NaiveDate,
    months: u32,
    start: NaiveDate,
    end: NaiveDate,
) -> FbResult<Money> {
    // Use actual civil days from the in-service day (included) through the monthly
    // anniversary at the end of useful life (excluded, clamped for shorter months).
    // The report endpoints are inclusive. Difference cumulative half-up cent amounts
    // at the edges so adjacent date ranges telescope. Supplied prior depreciation
    // limits remaining depreciation, with no catch-up for earlier periods.
    let outside = || FbError::validation("Asset useful life falls outside the supported calendar.");
    let expiration = service
        .checked_add_months(Months::new(months))
        .ok_or_else(outside)?;
    let life_days = i128::from((expiration - service).num_days());
    let cumulative = |date: NaiveDate| {
        let days = i128::from((date.clamp(service, expiration) - service).num_days());
        Money::from_cents((base.cents() * days + life_days / 2) / life_days)
    };
    Ok(cumulative(end.succ_opt().ok_or_else(outside)?) - cumulative(start))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_line_includes_service_day_and_adjacent_ranges_telescope_through_leap_day() {
        let day = |s: &str| s.parse().unwrap();
        let service = day("2028-01-31");
        let base = Money::from_db(1001);
        let run = |start, end| straight_line(base, service, 1, day(start), day(end)).unwrap();
        assert_eq!(run("2028-01-01", "2028-01-30"), Money::ZERO);
        assert_eq!(run("2028-01-31", "2028-01-31"), Money::from_db(35));
        assert_eq!(run("2028-02-29", "2028-03-01"), Money::ZERO);
        assert_eq!(
            run("2028-01-01", "2028-02-14") + run("2028-02-15", "2028-03-01"),
            base
        );
    }

    #[test]
    fn worksheet_depreciation_is_capped_at_cost_less_salvage_and_prior() {
        let day = |s: &str| s.parse().unwrap();
        let asset = AssetInput {
            label: "Synthetic equipment".into(),
            account: None,
            journal: None,
            line: None,
            in_service: Some(day("2024-01-01")),
            disposal_date: None,
            cost_cents: Some(2000),
            salvage_cents: Some(500),
            method: Some(Method::StraightLine),
            life_months: Some(24),
            prior_cents: Some(1499),
        };
        let values = calculate(&asset, None, day("2025-01-01"), day("2025-12-31"))
            .unwrap()
            .values
            .unwrap();
        assert_eq!(values.period_depreciation, Money::from_db(1));
        assert_eq!(values.net_book_value, Money::from_db(500));
    }
}
