//! The data logic of `ledger/report_views.py` and the date helpers of `ledger/financial.py`: presets,
//! fiscal-year start, form validation of the report controls, display rows (shared by screen, print and CSV),
//! CSV cells and general-ledger paging. HTTP, templates and URLs stay with the presentation layer; drill-downs
//! are returned as typed targets instead of URLs.

use super::financial::{self, BalanceSheet, Basis, LedgerSection, ProfitAndLoss};
use crate::error::{FbError, FbResult};
use crate::py;
use chrono::{Datelike, Duration, NaiveDate};
use fb_ids::AccountId;
use fb_money::Money;

/// `PRESETS`: (value, label) in Python's order.
pub const PRESETS: [(&str, &str); 6] = [
    ("this_month", "This month"),
    ("last_month", "Last month"),
    ("this_quarter", "This quarter"),
    ("this_year", "This year"),
    ("year_to_date", "Year to date"),
    ("custom", "Custom"),
];
pub const DEFAULT_PRESET: &str = "this_month";
pub const GL_PAGE_ROWS: usize = 200;
/// `MIN_DATE`, `MAX_DATE`.
pub fn min_date() -> NaiveDate {
    NaiveDate::from_ymd_opt(1900, 1, 2).unwrap_or(NaiveDate::MIN)
}
pub fn max_date() -> NaiveDate {
    NaiveDate::from_ymd_opt(9999, 12, 30).unwrap_or(NaiveDate::MAX)
}
pub const BOUNDS_MESSAGE: &str = "Use a date between 1900-01-02 and 9999-12-30.";

fn out_of_range() -> FbError {
    // Python raises OverflowError ("date value out of range") past year 9999.
    FbError::validation("date value out of range")
}

fn first_of_month(day: NaiveDate) -> NaiveDate {
    day.with_day(1).unwrap_or(day)
}

/// `preset_range(key, today)`; an unknown key is Python's ValueError.
pub fn preset_range(key: &str, today: NaiveDate) -> FbResult<(NaiveDate, NaiveDate)> {
    let ymd = |y: i32, m: u32, d: u32| NaiveDate::from_ymd_opt(y, m, d).ok_or_else(out_of_range);
    let back = |d: NaiveDate| d.checked_sub_signed(Duration::days(1)).ok_or_else(out_of_range);
    match key {
        "this_month" => {
            let start = first_of_month(today);
            let next = start.checked_add_signed(Duration::days(32)).ok_or_else(out_of_range)?;
            if next.year() > 9999 {
                return Err(out_of_range());
            }
            Ok((start, back(first_of_month(next))?))
        }
        "last_month" => {
            let end = back(first_of_month(today))?;
            Ok((first_of_month(end), end))
        }
        "this_quarter" => {
            let month = 3 * ((today.month() - 1) / 3) + 1;
            let start = ymd(today.year(), month, 1)?;
            let end = if month == 10 { ymd(today.year() + 1, 1, 1) } else { ymd(today.year(), month + 3, 1) };
            if today.year() >= 9999 && month == 10 {
                return Err(out_of_range());
            }
            Ok((start, back(end?)?))
        }
        "this_year" => {
            let start = financial::fiscal_year_start(today);
            if start.year() >= 9999 {
                return Err(out_of_range());
            }
            Ok((start, back(ymd(start.year() + 1, start.month(), start.day())?)?))
        }
        "year_to_date" => Ok((financial::fiscal_year_start(today), today)),
        other => Err(FbError::validation(other.to_string())),
    }
}

fn choice_error(value: &str) -> String {
    format!("Select a valid choice. {value} is not one of the available choices.")
}

/// `BoundedDateField` (required=False): blank is None; otherwise Django's date parsing, then the bounds.
fn bounded_date(raw: Option<&str>) -> Result<Option<NaiveDate>, String> {
    // Only the exact empty string is blank: Django strips after the empty-value check, so "  " is invalid.
    let Some(raw) = raw.filter(|r| !r.is_empty()) else { return Ok(None) };
    if py::py_strip(raw).is_empty() {
        return Err("Enter a valid date.".into());
    }
    let value = py::form_date(raw)?;
    if !(min_date()..=max_date()).contains(&value) {
        return Err(BOUNDS_MESSAGE.into());
    }
    Ok(Some(value))
}

/// `ReportForm.clean_basis`: blank means accrual.
fn basis_of(raw: Option<&str>) -> Result<Basis, String> {
    match raw.unwrap_or("") {
        "" => Ok(Basis::Accrual),
        other => Basis::parse(other).ok_or_else(|| choice_error(other)),
    }
}

/// Raw report controls, exactly as typed (None = absent from the query).
#[derive(Clone, Debug, Default)]
pub struct RangeInput<'a> {
    pub basis: Option<&'a str>,
    pub preset: Option<&'a str>,
    pub start: Option<&'a str>,
    pub end: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Range {
    pub basis: Basis,
    pub preset: String,
    pub start: Option<NaiveDate>,
    pub end: NaiveDate,
}

/// The cleaned `RangeForm` fields (preset None = blank).
struct RangeFields {
    basis: Basis,
    preset: Option<String>,
    start: Option<NaiveDate>,
    end: Option<NaiveDate>,
}

/// Django `_clean_fields` for the range controls, in field order; errors are appended.
fn range_fields(input: &RangeInput<'_>, errors: &mut Vec<String>) -> Option<RangeFields> {
    let basis = basis_of(input.basis).map_err(|e| errors.push(e)).ok();
    let preset = match input.preset.unwrap_or("") {
        "" => Some(None),
        p if PRESETS.iter().any(|(k, _)| *k == p) => Some(Some(p.to_string())),
        p => {
            errors.push(choice_error(p));
            None
        }
    };
    let start = bounded_date(input.start).map_err(|e| errors.push(e)).ok();
    let end = bounded_date(input.end).map_err(|e| errors.push(e)).ok();
    Some(RangeFields { basis: basis?, preset: preset?, start: start?, end: end? })
}

/// `RangeForm.clean`, run only when every field (of this form or a subclass) is valid. A non-custom preset
/// replaces any typed dates; with no preset, typed dates mean "custom" and otherwise this month.
fn range_clean(input: &RangeInput<'_>, fields: RangeFields, today: NaiveDate) -> FbResult<Range> {
    let RangeFields { basis, preset, mut start, mut end } = fields;
    let typed = input.start.is_some_and(|s| !s.is_empty()) || input.end.is_some_and(|s| !s.is_empty());
    let preset = preset.unwrap_or_else(|| if typed { "custom".into() } else { DEFAULT_PRESET.into() });
    if preset != "custom" {
        let (s, e) = preset_range(&preset, today)?;
        start = Some(s);
        end = Some(e);
    } else if end.is_none() {
        return Err(FbError::validation("Enter a \"To\" date for a custom range."));
    } else if let (Some(s), Some(e)) = (start, end)
        && s > e
    {
        return Err(FbError::validation("The \"From\" date must not be after the \"To\" date."));
    }
    let end = end.ok_or_else(|| FbError::validation("Enter a \"To\" date for a custom range."))?;
    Ok(Range { basis, preset, start, end })
}

/// `RangeForm`: field errors in field order; the cross-field rule only once every field is valid.
pub fn range(input: &RangeInput<'_>, today: NaiveDate) -> FbResult<Range> {
    let mut errors = Vec::new();
    match range_fields(input, &mut errors) {
        Some(fields) => range_clean(input, fields, today),
        None => Err(FbError::Validation(errors)),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerRange {
    pub range: Range,
    pub account: Option<AccountId>,
    pub group: String,
}

/// `LedgerForm`: the range plus an account and an account group. Rust account ids are UUIDs, so the account
/// control accepts an id instead of Python's decimal pk; anything else is "Choose an account from the list.".
pub fn ledger_range(input: &RangeInput<'_>, account: Option<&str>, group: Option<&str>, today: NaiveDate) -> FbResult<LedgerRange> {
    // Django: every field (range controls, then account, then group) before `RangeForm.clean`.
    let mut errors = Vec::new();
    let fields = range_fields(input, &mut errors);
    let account = match account.map(py::py_strip).unwrap_or("") {
        "" => Some(None),
        text => match AccountId::parse(text) {
            Some(id) => Some(Some(id)),
            None => {
                errors.push("Choose an account from the list.".into());
                None
            }
        },
    };
    let group_text = group.unwrap_or("");
    if financial::group_types(group_text).is_none() {
        errors.push(choice_error(group_text));
    }
    let (Some(fields), Some(account), true) = (fields, account, errors.is_empty()) else {
        return Err(FbError::Validation(errors));
    };
    Ok(LedgerRange { range: range_clean(input, fields, today)?, account, group: group_text.to_string() })
}

/// `AsOfForm`: blank means today.
pub fn as_of(basis: Option<&str>, as_of: Option<&str>, today: NaiveDate) -> FbResult<(Basis, NaiveDate)> {
    let mut errors = Vec::new();
    let basis = basis_of(basis).map_err(|e| errors.push(e)).ok();
    let date = bounded_date(as_of).map_err(|e| errors.push(e)).ok();
    match (basis, date) {
        (Some(b), Some(d)) => Ok((b, d.unwrap_or(today))),
        _ => Err(FbError::Validation(errors)),
    }
}

/// Where a report total or row drills to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Drill {
    /// The general ledger with preset=custom, one account or one group.
    Ledger { basis: Basis, start: Option<NaiveDate>, end: NaiveDate, account: Option<AccountId>, group: Option<&'static str> },
    /// The profit and loss with preset=custom.
    ProfitLoss { basis: Basis, start: Option<NaiveDate>, end: NaiveDate },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Heading,
    Account,
    Total,
    Grand,
    Computed,
    Spacer,
}

impl RowKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RowKind::Heading => "heading",
            RowKind::Account => "account",
            RowKind::Total => "total",
            RowKind::Grand => "grand",
            RowKind::Computed => "computed",
            RowKind::Spacer => "spacer",
        }
    }
}

/// One display row (screen, print and CSV share them).
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayRow {
    pub kind: RowKind,
    pub section: String,
    pub group: String,
    pub label: String,
    pub account: Option<crate::model::Account>,
    pub amount: Option<Money>,
    pub depth: u8,
    pub drill: Option<Drill>,
    pub note: Option<String>,
    pub final_row: bool,
}

impl DisplayRow {
    fn new(kind: RowKind, section: &str, group: &str, label: &str, depth: u8) -> DisplayRow {
        DisplayRow {
            kind,
            section: section.into(),
            group: group.into(),
            label: label.into(),
            account: None,
            amount: None,
            depth,
            drill: None,
            note: None,
            final_row: false,
        }
    }
}

/// FX2: P&L section -> the ledger group behind its total.
fn pl_drill(key: &str) -> &'static str {
    match key {
        "income" => "operating_income",
        "cogs" => "cogs",
        "expense" => "operating_expense",
        "other_income" => "other_income",
        _ => "other_expense",
    }
}

/// `pl_rows`.
pub fn pl_rows(report: &ProfitAndLoss) -> Vec<DisplayRow> {
    let (basis, start, end) = (report.basis, report.start, report.end);
    let ledger = |account: Option<AccountId>, group: Option<&'static str>| Drill::Ledger { basis, start, end, account, group };
    let mut rows = Vec::new();
    let section = |rows: &mut Vec<DisplayRow>, key: &str| {
        let Some(part) = report.section(key) else { return };
        let heading = part.heading;
        rows.push(DisplayRow::new(RowKind::Heading, heading, heading, heading, 0));
        for row in &part.rows {
            let mut r = DisplayRow::new(RowKind::Account, heading, heading, &row.account.label(), 1);
            r.account = Some(row.account.clone());
            r.amount = Some(row.amount);
            r.drill = Some(ledger(Some(row.account.id), None));
            rows.push(r);
        }
        let mut total = DisplayRow::new(RowKind::Total, heading, heading, part.total_label, 0);
        total.amount = Some(part.total);
        total.drill = Some(ledger(None, Some(pl_drill(key))));
        rows.push(total);
    };
    let grand = |label: &str, amount: Money, final_row: bool| {
        let mut r = DisplayRow::new(RowKind::Grand, "", "", label, 0);
        r.group = String::new();
        r.amount = Some(amount);
        r.drill = Some(ledger(None, Some("pl")));
        r.final_row = final_row;
        r
    };
    section(&mut rows, "income");
    if report.show_cogs {
        section(&mut rows, "cogs");
    }
    rows.push(grand("Gross profit", report.gross_profit, false));
    section(&mut rows, "expense");
    rows.push(grand("Net ordinary income", report.net_ordinary_income, false));
    if report.show_other {
        for key in ["other_income", "other_expense"] {
            if report.section(key).is_some_and(|s| !s.rows.is_empty()) {
                section(&mut rows, key);
            }
        }
        rows.push(grand("Net other income", report.net_other_income, false));
    }
    rows.push(grand("Net income", report.net_income, true));
    rows
}

/// `bs_rows`: section > group > type (when a group holds several types); only groups and types with balances.
pub fn bs_rows(report: &BalanceSheet) -> Vec<DisplayRow> {
    let (basis, as_of, fy) = (report.basis, report.as_of, report.fiscal_year_start);
    let ledger = |account: Option<AccountId>, group: Option<&'static str>| Drill::Ledger { basis, start: None, end: as_of, account, group };
    let mut rows = Vec::new();
    let account_rows = |rows: &mut Vec<DisplayRow>, heading: &str, group: &str, items: &[financial::AmountRow], depth: u8| {
        for row in items {
            let mut r = DisplayRow::new(RowKind::Account, heading, group, &row.account.label(), depth);
            r.account = Some(row.account.clone());
            r.amount = Some(row.amount);
            r.drill = Some(ledger(Some(row.account.id), None));
            rows.push(r);
        }
    };
    for (key, heading, total_label, group, section) in [
        ("assets", "Assets", "Total assets", "asset", &report.assets),
        ("liabilities", "Liabilities", "Total liabilities", "liability", &report.liabilities),
        ("equity", "Equity", "Total equity", "equity", &report.equity),
    ] {
        rows.push(DisplayRow::new(RowKind::Heading, heading, "", heading, 0));
        for part in &section.groups {
            let Some(part_heading) = part.heading else {
                for kind in &part.types {
                    account_rows(&mut rows, heading, heading, &kind.rows, 1);
                }
                continue;
            };
            rows.push(DisplayRow::new(RowKind::Heading, heading, part_heading, part_heading, 1));
            for kind in &part.types {
                if part.nested {
                    let type_heading = kind.heading.unwrap_or("");
                    rows.push(DisplayRow::new(RowKind::Heading, heading, type_heading, type_heading, 2));
                    account_rows(&mut rows, heading, type_heading, &kind.rows, 3);
                    let mut total = DisplayRow::new(RowKind::Total, heading, type_heading, kind.total_label.unwrap_or(""), 2);
                    total.amount = Some(kind.total);
                    total.drill = Some(ledger(None, kind.drill));
                    rows.push(total);
                } else {
                    account_rows(&mut rows, heading, part_heading, &kind.rows, 2);
                }
            }
            let mut total = DisplayRow::new(RowKind::Total, heading, part_heading, part.total_label.unwrap_or(""), 1);
            total.amount = Some(part.total);
            total.drill = Some(ledger(None, Some(part.drill)));
            rows.push(total);
        }
        let amount = if key == "equity" {
            let before = fy.pred_opt();
            let mut retained = DisplayRow::new(RowKind::Computed, heading, heading, "Retained earnings", 1);
            retained.amount = Some(report.retained_earnings);
            retained.note = Some(format!("Net income before {}, computed, not posted", fy.format("%Y-%m-%d")));
            retained.drill = before.map(|end| Drill::ProfitLoss { basis, start: None, end });
            rows.push(retained);
            let mut income = DisplayRow::new(RowKind::Computed, heading, heading, "Net income", 1);
            income.amount = Some(report.net_income);
            income.note = Some(format!("{} to {}, computed, not posted", fy.format("%Y-%m-%d"), as_of.format("%Y-%m-%d")));
            income.drill = Some(Drill::ProfitLoss { basis, start: Some(fy), end: as_of });
            rows.push(income);
            report.total_equity
        } else {
            section.total
        };
        let mut total = DisplayRow::new(RowKind::Total, heading, "", total_label, 0);
        total.amount = Some(amount);
        total.drill = Some(ledger(None, Some(group)));
        total.final_row = key == "assets";
        rows.push(total);
        if key == "assets" {
            rows.push(DisplayRow::new(RowKind::Spacer, "", "", "", 0));
        }
    }
    let mut grand = DisplayRow::new(RowKind::Grand, "", "", "Total liabilities and equity", 0);
    grand.amount = Some(report.total_liabilities_equity);
    grand.drill = Some(ledger(None, Some("le")));
    grand.final_row = true;
    rows.push(grand);
    rows
}

/// `_csv_rows`: [row type, section, group, account code, account (or label), account id, amount]; headings and
/// spacers are left out, a grand row is written as "total".
pub fn csv_rows(rows: &[DisplayRow]) -> Vec<[String; 7]> {
    rows.iter()
        .filter(|r| !matches!(r.kind, RowKind::Heading | RowKind::Spacer))
        .map(|r| {
            let kind = if r.kind == RowKind::Grand { "total" } else { r.kind.as_str() };
            let (code, name, id) = match &r.account {
                Some(a) => (a.code.clone(), a.name.clone(), a.id.to_string()),
                None => (String::new(), r.label.clone(), String::new()),
            };
            [kind.into(), r.section.clone(), r.group.clone(), code, name, id, r.amount.map(csv_amount).unwrap_or_default()]
        })
        .collect()
}

/// `_cell` for an amount: never "-0.00" (amounts are not formula-escaped).
pub fn csv_amount(amount: Money) -> String {
    amount.to_string()
}

/// `_cell` for text: a value a spreadsheet would run as a formula (= + - @ tab CR) gets a leading quote.
pub fn csv_text(text: &str) -> String {
    if text.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{text}") } else { text.to_string() }
}

/// Quote one already formula-guarded CSV cell. TAB/CR starts stay explicitly quoted.
pub fn csv_quote(text: &str) -> String {
    if text.contains([',', '"', '\r', '\n']) || text.starts_with("'\t") || text.starts_with("'\r") {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else { text.to_owned() }
}

/// Audit and reports share the same guard and quoting primitives.
pub fn csv_cell(text: &str) -> String { csv_quote(&csv_text(text)) }

/// One page group of `_ledger_page`: a run of one section's rows with its balance-forward header.
#[derive(Clone, Debug, PartialEq)]
pub struct PageGroup {
    pub index: usize,
    pub first: bool,
    pub forward: Money,
    /// Row positions within the section.
    pub rows: Vec<usize>,
    pub last: bool,
}

/// Django `Paginator(...).get_page(number)`: text Python's `int()` refuses is page 1; an integer below 1 or past
/// the end (however large) is the last page.
pub fn page_number(number: Option<&str>, count: usize, per_page: usize) -> usize {
    let per_page = per_page.max(1);
    let pages = if count == 0 { 1 } else { count.div_ceil(per_page) };
    // validate_number: int(number), PageNotAnInteger -> 1; below 1 or past the end (any size) -> the last page.
    match number.and_then(py::py_int) {
        None => 1,
        Some(py::PyInt::Small(n)) if n < 1 => pages,
        Some(py::PyInt::Small(n)) => usize::try_from(n).map(|n| n.min(pages)).unwrap_or(pages),
        Some(py::PyInt::Large { .. }) => pages,
    }
}

/// `_ledger_page`: flatten section rows (an empty section is one slot), page them, regroup with balance-forward
/// headers. Returns (page number, page count, groups).
pub fn ledger_page(sections: &[LedgerSection], number: Option<&str>, per_page: usize) -> (usize, usize, Vec<PageGroup>) {
    let mut flat: Vec<(usize, Option<usize>)> = Vec::new();
    for (index, section) in sections.iter().enumerate() {
        if section.rows.is_empty() {
            flat.push((index, None));
        } else {
            flat.extend((0..section.rows.len()).map(|p| (index, Some(p))));
        }
    }
    let per_page = per_page.max(1);
    let pages = if flat.is_empty() { 1 } else { flat.len().div_ceil(per_page) };
    let page = page_number(number, flat.len(), per_page);
    let mut grouped: Vec<PageGroup> = Vec::new();
    for &(index, position) in flat.iter().skip((page - 1) * per_page).take(per_page) {
        let section = &sections[index];
        if grouped.last().is_none_or(|g| g.index != index) {
            let first = matches!(position, None | Some(0));
            let forward = match position {
                Some(p) if p > 0 => section.rows[p - 1].balance,
                _ => section.opening,
            };
            grouped.push(PageGroup { index, first, forward, rows: Vec::new(), last: false });
        }
        if let Some(group) = grouped.last_mut() {
            if let Some(p) = position {
                group.rows.push(p);
            }
            group.last = position.is_none_or(|p| p + 1 == section.rows.len());
        }
    }
    (page, pages, grouped)
}
