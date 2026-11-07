//! Port of `ledger/test_reports_units.py` (cent rule, presets, display) plus the data logic of the report
//! controls and paging. Book-level report tests live in fb-harness (`financial_reports_tests.rs`), which builds
//! the Python `ReportScenario` through the real document services.

use super::financial::{self, Basis, share};
use super::params::{self, RangeInput};
use chrono::NaiveDate;
use fb_money::Money;
use std::collections::BTreeMap;

fn d(text: &str) -> Money {
    let negative = text.starts_with('-');
    let digits = text.trim_start_matches('-');
    let (whole, frac) = digits.split_once('.').unwrap_or((digits, "00"));
    let cents = whole.parse::<i128>().unwrap() * 100 + format!("{frac:0<2}")[..2].parse::<i128>().unwrap();
    Money::from_cents(if negative { -cents } else { cents })
}

fn day(text: &str) -> NaiveDate {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
}

fn map(pairs: &[(u32, &str)]) -> BTreeMap<u32, Money> {
    pairs.iter().map(|(k, v)| (*k, d(v))).collect()
}

#[test]
fn partial_share_floors_and_gives_leftover_cents_to_the_largest_remainders() {
    // Invoice 30.80: income 10.00 / 20.00, tax 0.80 (credits). 10.00 settled -> 324.68 / 649.35 / 25.97 cents exact.
    let nets = [(1, d("-10.00")), (2, d("-20.00")), (3, d("-0.80"))];
    assert_eq!(share(&nets, d("30.80"), d("10.00")).unwrap(), map(&[(1, "-3.25"), (2, "-6.49"), (3, "-0.26")]));
    assert_eq!(share(&nets, d("30.80"), d("5.40")).unwrap(), map(&[(1, "-1.75"), (2, "-3.51"), (3, "-0.14")]));
    assert_eq!(share(&nets, d("30.80"), d("15.40")).unwrap(), map(&[(1, "-5.00"), (2, "-10.00"), (3, "-0.40")]));
}

#[test]
fn full_and_zero_settlement_are_exact() {
    let nets = [(1, d("-10.00")), (2, d("-20.00")), (3, d("-0.80"))];
    assert_eq!(share(&nets, d("30.80"), d("30.80")).unwrap(), nets.iter().copied().collect());
    assert!(share(&nets, d("30.80"), d("0.00")).unwrap().is_empty());
    assert!(share(&nets, Money::ZERO, d("1.00")).unwrap().is_empty());
}

#[test]
fn three_equal_thirds_tie_break_by_account() {
    let mut nets = vec![(7, d("1.00")), (5, d("1.00")), (9, d("1.00"))];
    nets.sort();
    let got = share(&nets, d("3.00"), d("1.00")).unwrap();
    assert_eq!(got.values().copied().sum::<Money>(), d("1.00"));
    assert_eq!(got, map(&[(5, "0.34"), (7, "0.33"), (9, "0.33")]));
}

#[test]
fn mixed_sign_lines_still_sum_to_the_settled_amount() {
    // A bill with a 5.00 credit line: expense 25.00 + 0.01, discount -5.00 -> total 20.01.
    let nets = [(1, d("25.00")), (2, d("-5.00")), (3, d("0.01"))];
    for settled in ["0.01", "7.00", "13.33", "20.00"] {
        assert_eq!(share(&nets, d("20.01"), d(settled)).unwrap().values().copied().sum::<Money>(), d(settled), "{settled}");
    }
}

#[test]
fn cumulative_differences_add_back_to_the_document() {
    let nets = [(1, d("-33.33")), (2, d("-66.67")), (3, d("-8.00"))];
    let total = d("108.00");
    let mut settled = Money::ZERO;
    let mut seen: BTreeMap<u32, Money> = BTreeMap::new();
    for step in ["0.01", "10.00", "33.33", "0.66", "64.00"] {
        let before = share(&nets, total, settled).unwrap();
        settled += d(step);
        let after = share(&nets, total, settled).unwrap();
        for account in after.keys().chain(before.keys()).copied().collect::<std::collections::BTreeSet<_>>() {
            *seen.entry(account).or_default() += after.get(&account).copied().unwrap_or_default() - before.get(&account).copied().unwrap_or_default();
        }
    }
    assert_eq!(settled, total);
    assert_eq!(seen, nets.iter().copied().collect());
}

#[test]
fn negative_cumulative_settlement_follows_python_floor_semantics() {
    // A release dated before its settlement leaves a negative cumulative amount. Python: floors towards -inf
    // (-325 / -650 / -26 cents), leftover -1000 - (-1001) = 1 cent to the largest remainder (account 2).
    let nets = [(1, d("-10.00")), (2, d("-20.00")), (3, d("-0.80"))];
    assert_eq!(share(&nets, d("30.80"), d("-10.00")).unwrap(), map(&[(1, "3.25"), (2, "6.49"), (3, "0.26")]));
}

#[test]
fn share_refuses_overflow_instead_of_panicking() {
    let huge = Money::from_cents(i128::MAX / 2);
    assert!(share(&[(1, huge)], huge, Money::from_cents(3)).is_err());
}

#[test]
fn presets_on_ordinary_and_boundary_days() {
    type Expected = &'static [(&'static str, &'static str, &'static str)];
    let cases: &[(&str, Expected)] = &[
        (
            "2026-10-29",
            &[
                ("this_month", "2026-10-01", "2026-10-31"),
                ("last_month", "2026-09-01", "2026-09-30"),
                ("this_quarter", "2026-10-01", "2026-12-31"),
                ("this_year", "2026-01-01", "2026-12-31"),
                ("year_to_date", "2026-01-01", "2026-10-29"),
            ],
        ),
        ("2028-02-29", &[("this_month", "2028-02-01", "2028-02-29"), ("last_month", "2028-01-01", "2028-01-31"), ("this_quarter", "2028-01-01", "2028-03-31")]),
        ("2027-01-01", &[("last_month", "2026-12-01", "2026-12-31"), ("year_to_date", "2027-01-01", "2027-01-01")]),
        ("2026-12-31", &[("this_month", "2026-12-01", "2026-12-31"), ("this_quarter", "2026-10-01", "2026-12-31")]),
    ];
    for (today, expected) in cases {
        for (key, start, end) in *expected {
            assert_eq!(params::preset_range(key, day(today)).unwrap(), (day(start), day(end)), "{today} {key}");
        }
    }
    assert!(params::preset_range("custom", day("2026-10-29")).is_err());
    // Python raises OverflowError past 9999-12-31 instead of inventing a date.
    assert!(params::preset_range("this_month", day("9999-12-15")).is_err());
    assert!(params::preset_range("this_year", day("9999-06-01")).is_err());
    assert_eq!(params::preset_range("year_to_date", day("9999-06-01")).unwrap(), (day("9999-01-01"), day("9999-06-01")));
}

#[test]
fn fiscal_year_defaults_to_the_calendar_year() {
    assert_eq!(financial::fiscal_year_start(day("2026-10-29")), day("2026-01-01"));
}

#[test]
fn zero_is_never_negative_and_text_formulas_are_neutralised() {
    assert_eq!(params::csv_amount(-Money::ZERO), "0.00");
    assert_eq!(params::csv_amount(d("-12.00")), "-12.00");
    assert_eq!(params::csv_text("-SUM(A1)"), "'-SUM(A1)");
    assert_eq!(params::csv_text("@cmd"), "'@cmd");
    assert_eq!(params::csv_text("=HYPERLINK(\"x\")"), "'=HYPERLINK(\"x\")");
    assert_eq!(params::csv_text("Plain"), "Plain");
}

#[test]
fn range_form_defaults_presets_and_custom_rules() {
    let today = day("2026-10-29");
    let r = params::range(&RangeInput::default(), today).unwrap();
    assert_eq!((r.basis, r.preset.as_str(), r.start, r.end), (Basis::Accrual, "this_month", Some(day("2026-10-01")), day("2026-10-31")));
    let typed = params::range(&RangeInput { end: Some("2026-10-31"), ..Default::default() }, today).unwrap();
    assert_eq!((typed.preset.as_str(), typed.start), ("custom", None));
    let overridden = params::range(&RangeInput { preset: Some("this_month"), start: Some("2020-01-01"), end: Some("2020-01-31"), ..Default::default() }, today).unwrap();
    assert_eq!(overridden.start, Some(day("2026-10-01")));
    let refuse = |input: RangeInput<'_>| params::range(&input, today).unwrap_err().messages();
    assert_eq!(refuse(RangeInput { preset: Some("custom"), ..Default::default() }), vec!["Enter a \"To\" date for a custom range."]);
    assert_eq!(refuse(RangeInput { start: Some("2026-12-01"), end: Some("2026-01-01"), ..Default::default() }), vec!["The \"From\" date must not be after the \"To\" date."]);
    assert_eq!(refuse(RangeInput { end: Some("0001-01-01"), ..Default::default() }), vec![params::BOUNDS_MESSAGE]);
    assert_eq!(refuse(RangeInput { end: Some("9999-12-31"), ..Default::default() }), vec![params::BOUNDS_MESSAGE]);
    assert_eq!(
        refuse(RangeInput { basis: Some("X"), preset: Some("Y"), start: Some("bad"), end: Some("bad") }),
        vec![
            "Select a valid choice. X is not one of the available choices.",
            "Select a valid choice. Y is not one of the available choices.",
            "Enter a valid date.",
            "Enter a valid date."
        ]
    );
    assert!(params::range(&RangeInput { start: Some("1900-01-02"), end: Some("9999-12-30"), ..Default::default() }, today).is_ok());
}

#[test]
fn ledger_and_as_of_forms() {
    let today = day("2026-10-29");
    let refuse = |account: Option<&str>, group: Option<&str>| params::ledger_range(&RangeInput::default(), account, group, today).unwrap_err().messages();
    assert_eq!(refuse(Some("abc"), None), vec!["Choose an account from the list."]);
    assert_eq!(refuse(None, Some("nope")), vec!["Select a valid choice. nope is not one of the available choices."]);
    let ok = params::ledger_range(&RangeInput::default(), Some(" "), Some("receivable"), today).unwrap();
    assert_eq!((ok.account, ok.group.as_str()), (None, "receivable"));
    assert_eq!(params::as_of(None, None, today).unwrap(), (Basis::Accrual, today));
    assert_eq!(params::as_of(Some("CASH"), Some("2026-10-31"), today).unwrap(), (Basis::Cash, day("2026-10-31")));
    assert_eq!(params::as_of(None, Some("99999-01-01"), today).unwrap_err().messages(), vec!["Enter a valid date."]);
    assert_eq!(params::as_of(None, Some("0001-01-01"), today).unwrap_err().messages(), vec![params::BOUNDS_MESSAGE]);
}

#[test]
fn group_types_cover_every_account_type_exactly_once_in_the_type_groups() {
    use crate::model::DetailType;
    assert_eq!(financial::group_types("").unwrap().len(), 15);
    assert!(financial::group_types("nope").is_none());
    for (group, _) in financial::GL_GROUPS {
        assert!(financial::group_types(group).is_some(), "{group}");
    }
    // Every type appears in exactly one balance-sheet or P&L section (the MANDATORY account-type boundary).
    for detail in DetailType::ALL {
        let in_bs = financial::BS_LAYOUT.iter().flat_map(|s| s.4.iter()).flat_map(|g| g.4.iter()).filter(|t| t.0 == detail).count();
        let in_pl = usize::from(financial::pl_section(detail).is_some());
        assert_eq!(in_bs + in_pl, 1, "{detail:?}");
    }
}

#[test]
fn ledger_paging_carries_the_balance_forward() {
    use super::financial::{LedgerRow, LedgerSection};
    use crate::model::{Account, DetailType, Kind};
    let account = Account {
        id: fb_ids::AccountId::new(),
        company_id: fb_ids::CompanyId::new(),
        code: "100".into(),
        name: "Bank".into(),
        kind: Kind::Asset,
        detail_type: DetailType::Bank,
        is_contra: false,
        description: String::new(),
        active: true,
        postable: true,
    };
    let row = |balance: i128| LedgerRow {
        date: day("2026-10-01"),
        debit: Money::from_cents(100),
        credit: Money::ZERO,
        journal: None,
        source_id: None,
        reversal: false,
        entry_number: String::new(),
        memo: String::new(),
        name: String::new(),
        synthetic: None,
        balance: Money::from_cents(balance),
        line_id: None,
    };
    let full = LedgerSection { account: account.clone(), opening: d("5.00"), rows: (1..=5).map(|i| row(500 + 100 * i)).collect(), debit: d("5.00"), credit: Money::ZERO, closing: d("10.00") };
    let empty = LedgerSection { account, opening: d("1.00"), rows: vec![], debit: Money::ZERO, credit: Money::ZERO, closing: d("1.00") };
    let sections = [full, empty];
    let (page, pages, groups) = params::ledger_page(&sections, None, 3);
    assert_eq!((page, pages, groups.len()), (1, 2, 1));
    assert!(groups[0].first && !groups[0].last);
    let (page, _, groups) = params::ledger_page(&sections, Some("2"), 3);
    assert_eq!(page, 2);
    assert_eq!((groups[0].first, groups[0].forward, groups[0].rows.clone(), groups[0].last), (false, d("8.00"), vec![3, 4], true));
    assert_eq!((groups[1].index, groups[1].first, groups[1].rows.len(), groups[1].last), (1, true, 0, true));
    assert_eq!(params::ledger_page(&sections, Some("abc"), 3).0, 1);
    assert_eq!(params::ledger_page(&sections, Some("-4"), 3).0, 2);
    assert_eq!(params::ledger_page(&sections, Some("99"), 3).0, 2);
    assert_eq!(params::ledger_page(&[], None, 3).0, 1);
}
