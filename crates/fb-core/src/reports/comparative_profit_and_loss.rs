//! Fiscal calendar comparisons on one posted-books snapshot.

use super::financial::{self, Basis, Books, ProfitAndLoss, pl_from, sorted_accounts};
use super::{check_range, check_rows};
use crate::error::{FbError, FbResult};
use crate::model::Account;
use crate::{periods, services::access};
use chrono::{Datelike, Months, NaiveDate};
use fb_db::Book;
use fb_ids::{CompanyId, UserId};
use fb_money::Money;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Periods {
    Monthly,
    Quarterly,
    FiscalYears,
}

impl Periods {
    pub const CHOICES: [(&str, &str); 3] = [
        ("monthly", "Monthly"),
        ("quarterly", "Quarterly"),
        ("fiscal_years", "Current FY and prior FY"),
    ];

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "" | "monthly" => Some(Self::Monthly),
            "quarterly" => Some(Self::Quarterly),
            "fiscal_years" => Some(Self::FiscalYears),
            _ => None,
        }
    }

    pub fn key(self) -> &'static str {
        Self::CHOICES[self as usize].0
    }
}

#[derive(Debug)]
pub struct Column {
    pub label: String,
    pub report: ProfitAndLoss,
}

#[derive(Debug)]
pub struct ComparativeRow {
    pub label: String,
    pub kind: &'static str,
    pub account: Option<Account>,
    pub amounts: Vec<Money>,
}

#[derive(Debug)]
pub struct ComparativeProfitAndLoss {
    pub columns: Vec<Column>,
    pub rows: Vec<ComparativeRow>,
    pub unconverted: i64,
}

fn period_ranges(
    start: NaiveDate,
    end: NaiveDate,
    period: Periods,
    fiscal_month: u32,
) -> FbResult<Vec<(String, NaiveDate, NaiveDate)>> {
    let invalid =
        || FbError::validation("This comparison falls outside the supported report dates.");
    let add = |date: NaiveDate, months| {
        date.checked_add_months(Months::new(months))
            .ok_or_else(invalid)
    };
    let prior_day = |date: NaiveDate| date.pred_opt().ok_or_else(invalid);
    if period == Periods::FiscalYears {
        let current = financial::fiscal_year_start(end, fiscal_month)?;
        let prior = current
            .checked_sub_months(Months::new(12))
            .ok_or_else(invalid)?;
        let current_end = prior_day(add(current, 12)?)?;
        check_range(prior, current_end)?;
        return Ok(vec![
            ("Current FY".into(), current, current_end),
            ("Prior FY".into(), prior, prior_day(current)?),
        ]);
    }
    let mut ranges = Vec::new();
    let mut from = start;
    while from <= end {
        if ranges.len() == 12 {
            return Err(FbError::validation(
                "Choose a date range with no more than 12 periods.",
            ));
        }
        let (label, next) = if period == Periods::Monthly {
            let first = from.with_day(1).ok_or_else(invalid)?;
            (first.format("%b %Y").to_string(), add(first, 1)?)
        } else {
            let fiscal = financial::fiscal_year_start(from, fiscal_month)?;
            let quarter = ((from.month() + 12 - fiscal_month) % 12) / 3;
            (
                format!("Q{} · FY {}", quarter + 1, fiscal.format("%b %Y")),
                add(fiscal, (quarter + 1) * 3)?,
            )
        };
        ranges.push((label, from, prior_day(next)?.min(end)));
        from = next;
    }
    ranges.push(("Period total".into(), start, end));
    Ok(ranges)
}

pub fn comparative_profit_and_loss(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    basis: Basis,
    periods: Periods,
    start: NaiveDate,
    end: NaiveDate,
) -> FbResult<ComparativeProfitAndLoss> {
    book.read(|tx| {
        access(tx, actor, company)?;
        check_range(start, end)?;
        let month = periods::policy(tx, company)?.values.fiscal_year_start_month;
        let ranges = period_ranges(start, end, periods, month)?;
        let latest = ranges.iter().map(|(_, _, end)| *end).max().unwrap_or(end);
        let books = Books::load(tx, company, basis, latest)?;
        let accounts = sorted_accounts(tx, company)?;
        let columns = ranges
            .into_iter()
            .map(|(label, start, end)| {
                let mut report = pl_from(&books, &accounts, Some(start), end)?;
                // The comparison reports a single warning for the latest snapshot.
                // Individual columns retain the standalone through-date warning.
                if books.unconverted > 0 && end != latest {
                    report.unconverted = Books::count_unconverted(tx, company, end)?;
                }
                Ok(Column { label, report })
            })
            .collect::<FbResult<Vec<_>>>()?;
        let rows = aligned_rows(&columns, &accounts)?;
        Ok(ComparativeProfitAndLoss {
            columns,
            rows,
            unconverted: books.unconverted,
        })
    })
}

fn aligned_rows(columns: &[Column], accounts: &[Account]) -> FbResult<Vec<ComparativeRow>> {
    let mut rows = Vec::new();
    let computed = |label: &str, extract: fn(&ProfitAndLoss) -> Money| ComparativeRow {
        label: label.into(),
        kind: "total",
        account: None,
        amounts: columns
            .iter()
            .map(|column| extract(&column.report))
            .collect(),
    };
    for (index, (key, heading, total, _)) in financial::PL_SECTIONS.into_iter().enumerate() {
        let mut values = BTreeMap::new();
        for (col, column) in columns.iter().enumerate() {
            for row in &column.report.sections[index].rows {
                values
                    .entry(row.account.id)
                    .or_insert_with(|| vec![Money::ZERO; columns.len()])[col] = row.amount;
            }
        }
        rows.push(ComparativeRow {
            label: heading.into(),
            kind: "heading",
            account: None,
            amounts: vec![],
        });
        for account in accounts {
            if let Some(amounts) = values.remove(&account.id) {
                rows.push(ComparativeRow {
                    label: account.label(),
                    kind: "account",
                    account: Some(account.clone()),
                    amounts,
                });
                check_rows(rows.len())?;
            }
        }
        rows.push(ComparativeRow {
            label: total.into(),
            kind: "total",
            account: None,
            amounts: columns
                .iter()
                .map(|column| column.report.section(key).expect("P&L section").total)
                .collect(),
        });
        match key {
            "cogs" => rows.push(computed("Gross profit", |report| report.gross_profit)),
            "expense" => rows.push(computed("Net ordinary income", |report| {
                report.net_ordinary_income
            })),
            "other_expense" => rows.push(computed("Net other income", |report| {
                report.net_other_income
            })),
            _ => {}
        }
    }
    rows.push(computed("Net income", |report| report.net_income));
    check_rows(rows.len())?;
    Ok(rows)
}

/// Variance percentage in hundredths of a percent. Use the magnitude of the
/// prior result as the denominator so an improvement from a loss reads positive.
pub fn variance_percent(current: Money, prior: Money) -> Option<String> {
    if prior.is_zero() {
        return None;
    }
    let denominator = prior.cents().abs();
    let delta = (current - prior).cents();
    let pct = (delta.abs() * 10_000 + denominator / 2) / denominator;
    Some(format!(
        "{}{}.{:02}%",
        if delta < 0 && pct != 0 { "-" } else { "" },
        pct / 100,
        pct % 100
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarters_follow_the_fiscal_year_and_clip_partial_endpoints() {
        let date = |s: &str| s.parse().unwrap();
        let columns = period_ranges(
            date("2027-12-20"),
            date("2028-02-29"),
            Periods::Quarterly,
            11,
        )
        .unwrap();
        assert_eq!(
            columns,
            vec![
                (
                    "Q1 · FY Nov 2027".into(),
                    date("2027-12-20"),
                    date("2028-01-31")
                ),
                (
                    "Q2 · FY Nov 2027".into(),
                    date("2028-02-01"),
                    date("2028-02-29")
                ),
                (
                    "Period total".into(),
                    date("2027-12-20"),
                    date("2028-02-29")
                ),
            ]
        );
    }
}
