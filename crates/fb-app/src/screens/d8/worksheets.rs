//! Structured, run-time worksheet inputs and the shared screen/print/CSV rows.

use super::statements::{Cell, Table};
use super::*;
use fb_core::reports::{
    asset_depreciation_schedule::{self as asset, Accumulated, AssetInput, Cost, Method, Values},
    schedule_c_activity::{self as sc, Activity, Category, Mapping, Share, Split, Target, Totals},
};
use fb_db::Book;
use fb_ids::{JournalId, JournalLineId};
use std::collections::BTreeMap;

// Leave room in the 4,096-byte connected-host target for the output and page marker.
pub(super) const MAX_TARGET: usize = 3800;
pub(super) const TARGET_ERROR: &str =
    "Worksheet inputs are too long. Use fewer rows or shorter labels.";

pub(super) fn editor(vm: &mut Value, key: &str, raw: &str) {
    vm["worksheet"] = json!(key);
    vm["worksheet_raw"] = json!(if raw.len() <= MAX_TARGET { raw } else { "" });
    vm["worksheet_limit"] = json!(MAX_TARGET);
    vm["worksheet_limit_error"] = json!(TARGET_ERROR);
    if key == "mappings" {
        vm["category_choices"] = json!(
            Category::CHOICES
                .iter()
                .map(|(_, key, label)| (*key, *label))
                .collect::<Vec<_>>()
        );
    }
}

fn fields<const N: usize>(
    raw: &str,
    optional_last: bool,
) -> impl Iterator<Item = Result<(usize, [&str; N]), FbError>> {
    let required = N - usize::from(optional_last);
    raw.lines()
        .enumerate()
        .filter(|(_, row)| !row.trim().is_empty())
        .map(move |(index, row)| {
            let line = index + 1;
            let mut parts = row.split('|').map(str::trim);
            let columns = std::array::from_fn(|_| parts.next());
            if columns[..required].contains(&None) || parts.next().is_some() {
                return Err(FbError::validation(format!(
                    "Worksheet row {line} needs {N} fields separated by vertical bars."
                )));
            }
            Ok((line, columns.map(|part| part.unwrap_or_default())))
        })
}

pub(super) fn schedule_c(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    basis: f::Basis,
    start: NaiveDate,
    end: NaiveDate,
    raw: &str,
) -> Result<sc::ScheduleCActivity, FbError> {
    let mut targets = BTreeMap::<Target, Vec<Split>>::new();
    for row in fields(raw, false) {
        let (line, [scope, id, activity, category, share]) = row?;
        let invalid =
            |field| FbError::validation(format!("Enter a valid {field} on mapping row {line}."));
        let target = match scope {
            "account" => AccountId::parse(id).map(Target::Account),
            "journal" => JournalId::parse(id).map(Target::Journal),
            "line" => JournalLineId::parse(id).map(Target::Line),
            _ => return Err(invalid("target type")),
        }
        .ok_or_else(|| invalid("target ID"))?;
        let activity = if activity.eq_ignore_ascii_case(Activity::Excluded.label()) {
            Activity::Excluded
        } else if activity.eq_ignore_ascii_case(Activity::Unassigned.label()) {
            Activity::Unassigned
        } else {
            Activity::Business(activity.into())
        };
        let category = match (category, &activity) {
            ("", Activity::Excluded) => Some(Category::ExcludedPersonal),
            ("", Activity::Unassigned) => None,
            (category, _) => {
                Some(Category::parse(category).ok_or_else(|| invalid("category key"))?)
            }
        };
        let share = if let Some(weight) = share.strip_suffix('w') {
            Share::Weight(weight.parse().map_err(|_| invalid("whole-number weight"))?)
        } else if let Some(cents) = share.strip_suffix('c') {
            Share::Cents(
                cents
                    .parse()
                    .map_err(|_| invalid("signed whole-cent share"))?,
            )
        } else {
            return Err(invalid("share ending in w for weight or c for cents"));
        };
        targets.entry(target).or_default().push(Split {
            activity,
            category,
            share,
        });
    }
    let mappings = targets
        .into_iter()
        .map(|(target, splits)| Mapping { target, splits })
        .collect::<Vec<_>>();
    sc::schedule_c_activity(book, actor, company, basis, start, end, &mappings)
}

fn optional<T>(
    raw: &str,
    parse: impl FnOnce(&str) -> Option<T>,
    error: impl FnOnce() -> FbError,
) -> Result<Option<T>, FbError> {
    if raw.is_empty() {
        Ok(None)
    } else {
        parse(raw).map(Some).ok_or_else(error)
    }
}

pub(super) fn assets(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    start: NaiveDate,
    end: NaiveDate,
    raw: &str,
) -> Result<asset::DepreciationSchedule, FbError> {
    let mut inputs = Vec::new();
    for row in fields(raw, true) {
        let (
            line,
            [
                label,
                account,
                journal,
                posted_line,
                service,
                cost,
                salvage,
                method,
                life,
                prior,
                disposal,
            ],
        ) = row?;
        let invalid =
            |field| FbError::validation(format!("Enter a valid {field} on asset row {line}."));
        let cents = |raw, field| optional(raw, |value| value.parse().ok(), || invalid(field));
        let input = AssetInput {
            label: label.into(),
            account: optional(account, AccountId::parse, || invalid("cost account ID"))?,
            journal: optional(journal, JournalId::parse, || invalid("journal ID"))?,
            line: optional(posted_line, JournalLineId::parse, || {
                invalid("posted line ID")
            })?,
            in_service: optional(
                service,
                |s| s.parse().ok(),
                || invalid("in-service date in YYYY-MM-DD format"),
            )?,
            disposal_date: optional(
                disposal,
                |s| {
                    s.parse::<NaiveDate>()
                        .ok()
                        .filter(|date| date.to_string() == s)
                },
                || invalid("disposal date in YYYY-MM-DD format"),
            )?,
            cost_cents: cents(cost, "cost in whole cents")?,
            salvage_cents: cents(salvage, "salvage value in whole cents")?,
            method: optional(
                method,
                |s| (s == "straight_line").then_some(Method::StraightLine),
                || invalid("method (straight_line)"),
            )?,
            life_months: optional(
                life,
                |s| s.parse().ok(),
                || invalid("useful life in whole months"),
            )?,
            prior_cents: cents(prior, "prior depreciation in whole cents")?,
        };
        inputs.push(input);
    }
    asset::asset_depreciation_schedule(book, actor, company, start, end, &inputs)
}

pub(super) fn schedule_c_tables(report: sc::ScheduleCActivity, vm: &mut Value) -> Vec<Table> {
    let mut detail = Table::titled(
        "Activity allocations",
        &[
            "Book account",
            "Activity",
            "Schedule C category",
            "Mapping target",
            "Gross income",
            "Total expenses",
            "Net profit / loss",
        ],
    );
    detail.notes.push("Tax-preparation worksheet. The allocations use the income and expense sides of the selected book P&L.".into());
    if report.unconverted > 0 {
        detail.notes.push(format!("{} receivable or payable journal lines could not be converted to cash basis and remain on their posted dates.", report.unconverted));
    }
    let amounts = |totals: Totals| {
        [
            Cell::money(totals.income),
            Cell::money(totals.expenses),
            Cell::money(totals.net()),
        ]
    };
    let mut picker = BTreeMap::new();
    for (activity, group) in &report.groups {
        detail.push("heading", [Cell::text(activity.label())]);
        for row in &group.rows {
            picker.insert((&row.account.code, row.account.id), row.account.label());
            detail.push(
                "detail",
                [
                    Cell::text(row.account.label()),
                    Cell::text(activity.label()),
                    Cell::text(row.category.map(Category::label).unwrap_or("")),
                    Cell::text(row.target.label()),
                ]
                .into_iter()
                .chain(amounts(row.amounts)),
            );
        }
        detail.push(
            "total",
            [
                Cell::text(format!("{} total", activity.label())),
                Cell::text(""),
                Cell::text(""),
                Cell::text(""),
            ]
            .into_iter()
            .chain(amounts(group.totals)),
        );
    }
    vm["worksheet_accounts"] = json!(
        picker
            .into_iter()
            .map(|((_, id), label)| (id.to_db(), label))
            .collect::<Vec<_>>()
    );
    let mut reconciliation = Table::titled(
        "Reconciliation to Book P&L",
        &[
            "Source",
            "Gross income",
            "Total expenses",
            "Net profit / loss",
        ],
    );
    for (label, totals) in [
        ("Business activity totals", report.assigned),
        (
            "Excluded / Personal",
            report
                .groups
                .get(&Activity::Excluded)
                .map(|g| g.totals)
                .unwrap_or_default(),
        ),
        (
            "Unassigned",
            report
                .groups
                .get(&Activity::Unassigned)
                .map(|g| g.totals)
                .unwrap_or_default(),
        ),
        ("Worksheet total", report.allocated),
        ("Book P&L total", report.book),
        (
            "Difference",
            Totals {
                income: report.allocated.income - report.book.income,
                expenses: report.allocated.expenses - report.book.expenses,
            },
        ),
    ] {
        reconciliation.push(
            "total",
            [Cell::text(label)].into_iter().chain(amounts(totals)),
        );
    }
    vec![detail, reconciliation]
}

pub(super) fn asset_tables(
    report: asset::DepreciationSchedule,
    company: CompanyId,
    start: NaiveDate,
    end: NaiveDate,
    vm: &mut Value,
) -> Vec<Table> {
    vm["landscape"] = json!(true);
    vm["worksheet_accounts"] = json!(
        report
            .cost_accounts
            .iter()
            .map(|(a, _)| (a.id.to_db(), a.label()))
            .collect::<Vec<_>>()
    );
    let account = |a: &fb_core::model::Account| {
        Cell::link(
            a.label(),
            drill_url(
                company,
                &p::Drill::Ledger {
                    basis: f::Basis::Accrual,
                    start: Some(start),
                    end,
                    account: Some(a.id),
                    group: None,
                },
            ),
        )
    };
    let mut costs = Table::titled(
        "Ledger Rollforward · Fixed-asset cost",
        &[
            "Fixed-asset account",
            "Opening cost",
            "Additions",
            "Disposals / adjustments",
            "Closing cost",
            "Book value contribution",
        ],
    );
    let cost = |label, v: Cost| {
        [
            label,
            Cell::money(v.opening),
            Cell::money(v.additions),
            Cell::money(v.disposals),
            Cell::money(v.reversals),
            Cell::money(v.closing()),
            Cell::money(v.closing()),
        ]
    };
    for (a, v) in &report.cost_accounts {
        costs.push("detail", cost(account(a), *v));
    }
    costs.push(
        "total",
        cost(Cell::text("Total fixed-asset cost"), report.cost),
    );

    let mut accumulated = Table::titled(
        "Ledger Rollforward · Accumulated depreciation",
        &[
            "Contra fixed-asset account",
            "Opening accumulated",
            "Booked depreciation, net",
            "Other credits",
            "Other debits",
            "Unresolved credits",
            "Unresolved debits",
            "Period movement, net",
            "Closing accumulated",
            "Book value contribution",
        ],
    );
    let accum = |label, v: Accumulated| {
        [
            label,
            Cell::money(v.opening),
            Cell::money(v.booked),
            Cell::money(v.other_credits),
            Cell::money(v.other_debits),
            Cell::money(v.unresolved_credits),
            Cell::money(v.unresolved_debits),
            Cell::money(v.period_net()),
            Cell::money(v.closing()),
            Cell::money(-v.closing()),
        ]
    };
    for (a, v) in &report.contra_accounts {
        accumulated.push("detail", accum(account(a), *v));
    }
    accumulated.push(
        "total",
        accum(
            Cell::text("Total accumulated depreciation"),
            report.accumulated,
        ),
    );
    accumulated.notes.push("Closing net book value combines the contributions from fixed-asset cost accounts and contra accounts. The ledger keeps these account balances separately.".into());
    accumulated.notes.push("Booked depreciation uses direct postings between contra fixed assets and expense accounts, including cost of goods sold and reversals. Mixed postings with a possible depreciation component appear as unresolved credits or debits.".into());

    let mut expenses = Table::titled(
        "Ledger Rollforward · Booked depreciation expense",
        &["Expense account", "Period expense"],
    );
    for (a, amount) in &report.expense_accounts {
        expenses.push("detail", [account(a), Cell::money(*amount)]);
    }
    expenses.push(
        "total",
        [
            Cell::text("Total booked depreciation expense"),
            Cell::money(report.ledger.period_depreciation),
        ],
    );
    expenses.push(
        "total",
        [
            Cell::text("Closing net book value"),
            Cell::money(report.ledger.net_book_value),
        ],
    );
    let mut tables = vec![costs, accumulated, expenses];

    let mut worksheet = Table::titled(
        "Asset Worksheet · Straight-line",
        &[
            "Asset",
            "In service",
            "Disposal date",
            "Method / life",
            "Original cost",
            "Salvage",
            "Prior accumulated",
            "Period depreciation",
            "Closing cost",
            "Closing accumulated",
            "Closing net book value",
            "Status / evidence",
        ],
    );
    for a in &report.assets {
        let option_money = |amount: Option<fb_core::Money>| {
            amount.map(Cell::money).unwrap_or_else(|| Cell::text(""))
        };
        let input = &a.input;
        let method = input.method.map(Method::label).unwrap_or("");
        let life = input
            .life_months
            .map(|n| format!("{n} months"))
            .unwrap_or_default();
        let mut status = if a.missing.is_empty() {
            "Complete".into()
        } else {
            format!("Incomplete · missing {}", a.missing.join(", "))
        };
        if a.disposed {
            status.push_str(" · Disposed by To date");
        } else if input.disposal_date.is_some() {
            status.push_str(" · Disposal after To date");
        }
        let closing = |amount: Option<fb_core::Money>| {
            option_money(if a.disposed {
                Some(fb_core::Money::ZERO)
            } else {
                amount
            })
        };
        for (kind, id) in [
            ("account", input.account.map(|id| id.to_db())),
            ("journal", input.journal.map(|id| id.to_db())),
            ("line", input.line.map(|id| id.to_db())),
        ] {
            if let Some(id) = id {
                status.push_str(&format!(" · {kind} {id}"));
            }
        }
        worksheet.push(
            "detail",
            [
                Cell::text(&input.label),
                Cell::text(
                    input
                        .in_service
                        .map(|date| date.to_string())
                        .unwrap_or_default(),
                ),
                Cell::text(
                    input
                        .disposal_date
                        .map(|date| date.to_string())
                        .unwrap_or_default(),
                ),
                Cell::text(
                    [method, &life]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" / "),
                ),
                option_money(a.cost),
                option_money(input.salvage_cents.map(fb_core::Money::from_db)),
                option_money(input.prior_cents.map(fb_core::Money::from_db)),
                option_money(a.values.map(|v| v.period_depreciation)),
                closing(a.cost),
                closing(a.values.map(|v| v.accumulated)),
                closing(a.values.map(|v| v.net_book_value)),
                Cell::text(status),
            ],
        );
    }
    if report.assets.is_empty() {
        worksheet.notes.push("Add asset entries to calculate straight-line depreciation and compare them with the ledger.".into());
    } else {
        worksheet.notes.push("Depreciation is prorated by actual days over the stated useful life, beginning on the in-service date and capped at the remaining depreciable cost. Depreciation includes the disposal date. Disposed assets have zero closing balances.".into());
    }
    tables.push(worksheet);

    let mut reconciliation = Table::titled(
        "Ledger Reconciliation",
        &[
            "Source",
            "Closing cost",
            "Period depreciation",
            "Closing accumulated",
            "Closing net book value",
        ],
    );
    let values = |label, v: Values| {
        [
            Cell::text(label),
            Cell::money(v.cost),
            Cell::money(v.period_depreciation),
            Cell::money(v.accumulated),
            Cell::money(v.net_book_value),
        ]
    };
    reconciliation.push("total", values("Ledger rollforward", report.ledger));
    let unresolved = !report.accumulated.unresolved_credits.is_zero()
        || !report.accumulated.unresolved_debits.is_zero();
    if !report.assets.is_empty() {
        reconciliation.push(
            "total",
            values("Known worksheet amounts", report.known_assets),
        );
        let label = if unresolved || report.incomplete > 0 {
            "Variance (worksheet less ledger, period depreciation provisional)"
        } else {
            "Variance (worksheet less ledger)"
        };
        reconciliation.push(
            "total",
            values(label, report.known_assets.difference(report.ledger)),
        );
    }
    if report.incomplete > 0 {
        reconciliation.notes.push(format!(
            "{} incomplete asset entries contribute only known amounts. The period depreciation variance is provisional.",
            report.incomplete
        ));
    }
    if unresolved {
        reconciliation.notes.push("Mixed journals contain unresolved contra movements. Booked depreciation includes only amounts attributable to depreciation, so any period depreciation variance is provisional.".into());
    }
    tables.push(reconciliation);
    tables
}
