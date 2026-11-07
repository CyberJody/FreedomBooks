
mod common;
use common::*;
use fb_core::reports::{financial::{self, Basis, share}, params::{self, RangeInput}};
use fb_core::Money;
use chrono::NaiveDate;

fn day(text: &str) -> NaiveDate { NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap() }

#[test]
fn review_share_minimum_total_returns_error_without_panic() {
    let result = std::panic::catch_unwind(|| share(&[(1, Money::from_cents(1))], Money::from_cents(i128::MIN), Money::from_cents(1)));
    assert!(result.is_ok(), "share panicked while normalizing i128::MIN total");
    assert!(result.unwrap().is_err());
}

#[test]
fn review_share_minimum_settlement_returns_error_without_panic() {
    let result = std::panic::catch_unwind(|| share(&[(1, Money::from_cents(1))], Money::from_cents(-1), Money::from_cents(i128::MIN)));
    assert!(result.is_ok(), "share panicked while negating i128::MIN settlement");
    assert!(result.unwrap().is_err());
}

#[test]
fn review_share_remainder_award_returns_error_without_panic() {
    let nets = [(1, Money::from_cents(-i128::MAX)), (2, Money::from_cents(i128::MAX))];
    let result = std::panic::catch_unwind(|| share(&nets, Money::from_cents(-1), Money::from_cents(1)));
    assert!(result.is_ok(), "share overflowed while awarding a leftover cent");
    assert!(result.unwrap().is_err());
}

#[test]
fn review_ledger_field_errors_suppress_range_clean() {
    let input = RangeInput { preset: Some("custom"), ..Default::default() };
    let got = params::ledger_range(&input, None, Some("bogus"), day("2026-10-29")).unwrap_err().messages();
    assert_eq!(got, vec!["Select a valid choice. bogus is not one of the available choices."]);
}

#[test]
fn review_ledger_field_errors_suppress_ordering_error() {
    let input = RangeInput { start: Some("2026-12-01"), end: Some("2026-01-01"), ..Default::default() };
    let got = params::ledger_range(&input, Some("bad"), None, day("2026-10-29")).unwrap_err().messages();
    assert_eq!(got, vec!["Choose an account from the list."]);
}

#[test]
fn review_page_huge_integer_is_last_page() { assert_eq!(params::page_number(Some("999999999999999999999999999999"), 9, 3), 3); }
#[test]
fn review_page_unicode_integer_is_accepted() { assert_eq!(params::page_number(Some("٢"), 9, 3), 2); }
#[test]
fn review_page_python_integer_underscores_are_accepted() { assert_eq!(params::page_number(Some("1_0"), 9, 3), 3); }

#[test]
fn review_report_form_rejects_digits_unknown_to_frozen_python_unicode() {
    let input = RangeInput { end: Some("\u{10d42}\u{10d40}\u{10d42}\u{10d46}-10-31"), ..Default::default() };
    let got = params::range(&input, day("2026-10-29"));
    assert!(got.is_err(), "new Unicode digits were accepted as a year: {got:?}");
    assert_eq!(got.unwrap_err().messages(), vec!["Enter a valid date."]);
}

#[test]
fn review_large_totals_read_only_and_reopen() {
    let path = book_path("report-review-aggregate");
    let mut book = fb_core::Book::create(&path, &KEY).unwrap();
    let user = fb_core::users::create_user(&mut book, "reviewer", false).unwrap();
    let mut fx = Fx { book, user };
    let company = fx.company("Synthetic reports review");
    let bank = fx.account(company, "100", "Bank", "ASSET");
    let income = fx.account(company, "400", "Income", "INCOME");
    for _ in 0..2 {
        let j = fx.draft(company, bank, income, "99999999999999.99", "2026-10-01");
        fx.post(company, j.id);
    }
    let before = fx.count("SELECT count(*) FROM ledger_auditevent");
    let want = Money::from_cents(19_999_999_999_999_998);
    for basis in [Basis::Accrual, Basis::Cash] {
        let pl = financial::profit_and_loss(&mut fx.book, fx.user, company, basis, None, day("2026-10-31")).unwrap();
        assert_eq!(pl.net_income, want);
        let bs = financial::balance_sheet(&mut fx.book, fx.user, company, basis, day("2026-10-31")).unwrap();
        assert_eq!(bs.assets.total, want);
        assert!(bs.balanced);
        let gl = financial::general_ledger(&mut fx.book, fx.user, company, basis, None, day("2026-10-31"), None, "").unwrap();
        assert_eq!(gl.debit, want);
        assert_eq!(gl.credit, want);
    }
    assert_eq!(before, fx.count("SELECT count(*) FROM ledger_auditevent"));
    assert!(fx.book.check().unwrap().is_empty());
    drop(fx.book);
    let reopened = fb_core::Book::open(&path, &KEY).unwrap();
    assert!(reopened.check().unwrap().is_empty());
}
