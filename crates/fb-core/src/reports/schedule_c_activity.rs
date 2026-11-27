//! Explicit tax-preparation allocations on the same snapshot as the book P&L.

use super::financial::{Basis, Books, PL_SECTIONS, pl_from, pl_section, sorted_accounts};
use super::{check_range, check_rows};
use crate::error::{FbError, FbResult};
use crate::model::Account;
use crate::services::access;
use chrono::NaiveDate;
use fb_db::Book;
use fb_ids::{AccountId, CompanyId, JournalId, JournalLineId, UserId};
use fb_money::Money;
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub const MAX_MAPPINGS: usize = 100;
pub const MAX_SPLITS: usize = 16;
pub const MAX_ACTIVITY_LENGTH: usize = 80;

macro_rules! categories {
    ($($variant:ident => $key:literal, $label:literal;)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
        pub enum Category { $($variant,)* }

        impl Category {
            pub const CHOICES: &[(Self, &str, &str)] = &[$((Self::$variant, $key, $label),)*];
            pub fn parse(key: &str) -> Option<Self> {
                Self::CHOICES.iter().find(|(_, name, _)| *name == key).map(|(value, _, _)| *value)
            }
            pub fn label(self) -> &'static str {
                Self::CHOICES[self as usize].2
            }
        }
    };
}

categories! {
    GrossReceipts => "gross_receipts", "Gross receipts";
    ReturnsAllowances => "returns", "Returns and allowances";
    OtherIncome => "other_income", "Other income";
    CostOfGoodsSold => "cogs", "Cost of goods sold";
    Advertising => "advertising", "Advertising";
    CarTruck => "car_truck", "Car and truck";
    ContractLabor => "contract_labor", "Contract labor";
    Depreciation => "depreciation", "Depreciation";
    Insurance => "insurance", "Insurance";
    LegalProfessional => "legal_professional", "Legal and professional";
    Office => "office", "Office expense";
    RentLease => "rent_lease", "Rent and lease";
    Repairs => "repairs", "Repairs";
    Supplies => "supplies", "Supplies";
    TaxesLicenses => "taxes_licenses", "Taxes and licenses";
    Travel => "travel", "Travel";
    Meals => "meals", "Meals";
    Utilities => "utilities", "Utilities";
    Wages => "wages", "Wages";
    OtherExpenses => "other_expenses", "Other expenses";
    ExcludedPersonal => "excluded", "Excluded / Personal";
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Activity {
    Business(String),
    Excluded,
    Unassigned,
}

impl Activity {
    pub fn label(&self) -> &str {
        match self {
            Self::Business(name) => name,
            Self::Excluded => "Excluded / Personal",
            Self::Unassigned => "Unassigned",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Target {
    Account(AccountId),
    Journal(JournalId),
    Line(JournalLineId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Share {
    Weight(i32),
    Cents(i64),
}

#[derive(Clone, Debug)]
pub struct Split {
    pub activity: Activity,
    pub category: Option<Category>,
    pub share: Share,
}

#[derive(Clone, Debug)]
pub struct Mapping {
    pub target: Target,
    pub splits: Vec<Split>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub income: Money,
    pub expenses: Money,
}

impl Totals {
    pub fn net(self) -> Money {
        self.income - self.expenses
    }

    fn add(&mut self, other: Self) {
        self.income += other.income;
        self.expenses += other.expenses;
    }
}

#[derive(Clone, Copy, Default)]
struct SignedAmounts {
    positive: Money,
    negative: Money,
}

impl SignedAmounts {
    fn add(&mut self, amount: Money) {
        self.positive += amount.max(Money::ZERO);
        self.negative += (-amount).max(Money::ZERO);
    }

    fn net(self) -> Money {
        self.positive - self.negative
    }
}

#[derive(Debug)]
pub struct Row {
    pub account: Account,
    pub target: Target,
    pub category: Option<Category>,
    pub amounts: Totals,
}

#[derive(Debug, Default)]
pub struct ActivityGroup {
    pub rows: Vec<Row>,
    pub totals: Totals,
}

#[derive(Debug)]
pub struct ScheduleCActivity {
    pub groups: BTreeMap<Activity, ActivityGroup>,
    pub assigned: Totals,
    pub allocated: Totals,
    pub book: Totals,
    pub unconverted: i64,
}

impl Mapping {
    fn validate(&self) -> FbResult<()> {
        if self.splits.is_empty() || self.splits.len() > MAX_SPLITS {
            return Err(FbError::validation(
                "Use 1 to 16 shares per mapping target.",
            ));
        }
        let mut destinations = BTreeSet::new();
        for split in &self.splits {
            let valid = match (&split.activity, split.category) {
                (Activity::Business(name), Some(category)) => {
                    !name.is_empty()
                        && name.trim() == name
                        && name.chars().count() <= MAX_ACTIVITY_LENGTH
                        && !name.chars().any(char::is_control)
                        && ![Activity::Excluded.label(), Activity::Unassigned.label()]
                            .iter()
                            .any(|label| label.eq_ignore_ascii_case(name))
                        && category != Category::ExcludedPersonal
                }
                (Activity::Excluded, Some(Category::ExcludedPersonal))
                | (Activity::Unassigned, None) => true,
                _ => false,
            };
            if !valid || !destinations.insert((&split.activity, split.category)) {
                return Err(FbError::validation(
                    "Choose a valid activity and category once per mapping target.",
                ));
            }
        }
        let valid_shares = match self.splits[0].share {
            Share::Cents(_) => self
                .splits
                .iter()
                .all(|s| matches!(s.share, Share::Cents(_))),
            Share::Weight(_) => {
                self.splits
                    .iter()
                    .all(|s| matches!(s.share, Share::Weight(w) if w >= 0))
                    && self
                        .splits
                        .iter()
                        .any(|s| matches!(s.share, Share::Weight(w) if w > 0))
            }
        };
        if !valid_shares {
            return Err(FbError::validation(
                "Use either whole-cent shares or nonnegative weights with a positive total for each target.",
            ));
        }
        Ok(())
    }

    fn allocate(&self, amounts: SignedAmounts) -> FbResult<Vec<Money>> {
        let cents = amounts.net().cents();
        let shares = match self.splits[0].share {
            Share::Cents(_) => {
                let shares: Vec<_> = self
                    .splits
                    .iter()
                    .map(|s| match s.share {
                        Share::Cents(value) => i128::from(value),
                        Share::Weight(_) => unreachable!("validated shares"),
                    })
                    .collect();
                if shares.iter().sum::<i128>() != cents {
                    return Err(FbError::validation(format!(
                        "Cent shares for {} must add up to its exact total of {cents} cents.",
                        self.target.label()
                    )));
                }
                let positive: i128 = shares.iter().filter(|&&share| share > 0).sum();
                let negative: i128 = shares.iter().filter(|&&share| share < 0).sum();
                if positive > amounts.positive.cents() || -negative > amounts.negative.cents() {
                    return Err(FbError::validation(format!(
                        "Positive and negative cent shares for {} must be backed by source amounts of the same sign.",
                        self.target.label()
                    )));
                }
                shares
            }
            Share::Weight(_) => {
                let weights: Vec<_> = self
                    .splits
                    .iter()
                    .map(|s| match s.share {
                        Share::Weight(weight) => i128::from(weight),
                        Share::Cents(_) => unreachable!("validated shares"),
                    })
                    .collect();
                let total: i128 = weights.iter().sum();
                let magnitude = cents.abs();
                let mut amounts = Vec::with_capacity(weights.len());
                let mut remainders = Vec::with_capacity(weights.len());
                for (index, weight) in weights.into_iter().enumerate() {
                    // Splitting the dividend keeps even large account totals exact.
                    let numerator = (magnitude % total) * weight;
                    amounts.push((magnitude / total) * weight + numerator / total);
                    remainders.push((std::cmp::Reverse(numerator % total), index));
                }
                let left = magnitude - amounts.iter().sum::<i128>();
                remainders.sort_unstable();
                for (_, index) in remainders.into_iter().take(left as usize) {
                    amounts[index] += 1;
                }
                amounts
                    .into_iter()
                    .map(|value| value * cents.signum())
                    .collect()
            }
        };
        Ok(shares.into_iter().map(Money::from_cents).collect())
    }
}

impl Target {
    pub fn label(self) -> String {
        match self {
            Self::Account(id) => format!("account {id}"),
            Self::Journal(id) => format!("journal {id}"),
            Self::Line(id) => format!("line {id}"),
        }
    }
}

pub fn schedule_c_activity(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    basis: Basis,
    start: NaiveDate,
    end: NaiveDate,
    mappings: &[Mapping],
) -> FbResult<ScheduleCActivity> {
    book.read(|tx| {
        access(tx, actor, company)?;
        check_range(start, end)?;
        let accounts = sorted_accounts(tx, company)?;
        let pl_accounts = accounts
            .iter()
            .filter_map(|account| {
                let section = pl_section(account.detail_type)?;
                Some((account.id, (account, matches!(section, "income" | "other_income"))))
            })
            .collect::<HashMap<_, _>>();
        check_rows(pl_accounts.len())?;
        if mappings.len() > MAX_MAPPINGS {
            return Err(FbError::validation("Use at most 100 mapping targets."));
        }
        let mut seen = BTreeSet::new();
        let mut account_rules = HashMap::new();
        let mut journal_rules = HashMap::new();
        let mut line_rules = HashMap::new();
        for mapping in mappings {
            mapping.validate()?;
            if !seen.insert(mapping.target) {
                return Err(FbError::validation("Each mapping target must be supplied once."));
            }
            match mapping.target {
                Target::Account(id) if pl_accounts.contains_key(&id) => {
                    account_rules.insert(id, mapping);
                }
                Target::Account(_) => return Err(FbError::validation("Choose a P&L account in this company.")),
                Target::Journal(id) => { journal_rules.insert(id.to_db(), mapping.target); }
                Target::Line(id) => { line_rules.insert(id.to_db(), mapping.target); }
            }
        }

        let books = Books::load(tx, company, basis, end)?;
        let pl = pl_from(&books, &accounts, Some(start), end)?;
        let mut totals = Totals::default();
        let mut amounts = HashMap::new();
        for (section, (_, _, _, sign)) in pl.sections.iter().zip(PL_SECTIONS) {
            if sign > 0 {
                totals.income += section.total;
            } else {
                totals.expenses += section.total;
            }
            amounts.extend(section.rows.iter().map(|row| (row.account.id, row.amount)));
        }

        let mut overrides = BTreeMap::<Target, BTreeMap<AccountId, SignedAmounts>>::new();
        let mut remaining = HashMap::<AccountId, SignedAmounts>::new();
        let mut evidence = HashMap::<AccountId, Money>::new();
        let mut apply = |line: &str, posted_line: Option<&str>, journal: Option<&str>, account, net_debit: Money| {
            if let Some((_, income)) = pl_accounts.get(&account) {
                let amount = if *income { -net_debit } else { net_debit };
                *evidence.entry(account).or_default() += amount;
                let target = line_rules.get(line)
                    .or_else(|| posted_line.and_then(|id| line_rules.get(id)))
                    .or_else(|| journal.and_then(|id| journal_rules.get(id)));
                if let Some(target) = target {
                    overrides.entry(*target).or_default().entry(account).or_default().add(amount);
                } else {
                    remaining.entry(account).or_default().add(amount);
                }
            }
        };
        for (line, journal, account, net_debit) in books.posted_activity(start, end) {
            apply(line, None, Some(journal), account, net_debit);
        }
        if basis == Basis::Cash {
            let keys = pl_accounts.keys().copied().collect();
            books.recognized_activity(tx, company, start, end, &keys, apply)?;
        }
        if pl_accounts.keys().any(|id| evidence.get(id).copied().unwrap_or_default() != amounts.get(id).copied().unwrap_or_default()) {
            return Err(FbError::validation("Worksheet source amounts do not reconcile to the book P&L by account."));
        }

        let mut groups: BTreeMap<Activity, ActivityGroup> = BTreeMap::new();
        let mut row_count = 0;
        let mut add = |id, target, source: SignedAmounts, mapping: Option<&Mapping>| -> FbResult<()> {
            let (account, income) = pl_accounts[&id];
            let mut append = |activity: Activity, category, amount| -> FbResult<()> {
                row_count += 1;
                check_rows(row_count)?;
                let amounts = if income {
                    Totals { income: amount, ..Totals::default() }
                } else {
                    Totals { expenses: amount, ..Totals::default() }
                };
                let group = groups.entry(activity).or_default();
                group.totals.add(amounts);
                group.rows.push(Row { account: account.clone(), target, category, amounts });
                Ok(())
            };
            if let Some(mapping) = mapping {
                for (split, amount) in mapping.splits.iter().zip(mapping.allocate(source)?) {
                    append(split.activity.clone(), split.category, amount)?;
                }
            } else {
                append(Activity::Unassigned, None, source.net())?;
            }
            Ok(())
        };

        for mapping in mappings.iter().filter(|m| !matches!(m.target, Target::Account(_))) {
            let overrides = overrides.remove(&mapping.target).ok_or_else(|| {
                FbError::validation(format!(
                    "No eligible posted P&L activity for {} on these dates and basis. Line mappings take precedence over journals. Cash-basis document recognition uses the original document line, posted line, or settlement journal.",
                    mapping.target.label()
                ))
            })?;
            if overrides.len() > 1 && matches!(mapping.splits[0].share, Share::Cents(_)) {
                return Err(FbError::validation(
                    "A journal covering multiple P&L accounts needs weights or individual line mappings.",
                ));
            }
            for (account, amount) in overrides {
                add(account, mapping.target, amount, Some(mapping))?;
            }
        }
        for account in accounts.iter().filter(|a| pl_accounts.contains_key(&a.id)) {
            let amount = remaining.get(&account.id).copied().unwrap_or_default();
            add(account.id, Target::Account(account.id), amount, account_rules.get(&account.id).copied())?;
        }

        let mut allocated = Totals::default();
        let mut assigned = Totals::default();
        for (activity, group) in &mut groups {
            group.rows.sort_by(|a, b| {
                (&a.account.code, a.account.id, a.target, a.category)
                    .cmp(&(&b.account.code, b.account.id, b.target, b.category))
            });
            allocated.add(group.totals);
            if matches!(activity, Activity::Business(_)) {
                assigned.add(group.totals);
            }
        }
        check_rows(row_count + groups.len() * 2 + 6)?;
        if allocated != totals || allocated.net() != pl.net_income {
            return Err(FbError::validation("The worksheet does not reconcile to the book P&L."));
        }
        Ok(ScheduleCActivity {
            groups, assigned, allocated, book: totals, unconverted: pl.unconverted,
        })
    })
}
