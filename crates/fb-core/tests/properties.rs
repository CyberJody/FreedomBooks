//! Property tests (ARCHITECTURE section 9.5): random balanced journals post; random unbalanced ones are
//! refused; a reversal zeroes its original's account effects; the trial balance always balances.
mod common;

use common::*;
use fb_core::services::{self, DraftInput, line};
use fb_core::{AccountId, CompanyId, reports};
use proptest::prelude::*;

fn setup() -> (Fx, CompanyId, Vec<AccountId>) {
    let mut fx = Fx::new("prop");
    let company = fx.company("Property books");
    let accounts = ["ASSET", "LIABILITY", "EQUITY", "INCOME", "EXPENSE", "ASSET"]
        .iter()
        .enumerate()
        .map(|(i, kind)| fx.account(company, &format!("{}", 1000 + i), &format!("Account {i}"), kind))
        .collect();
    (fx, company, accounts)
}

fn amount(cents: u64) -> String {
    format!("{}.{:02}", cents / 100, cents % 100)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, .. ProptestConfig::default() })]

    #[test]
    fn balanced_posts_unbalanced_refused_reversal_zeroes_and_tb_balances(
        debits in prop::collection::vec((0usize..6, 1u64..10_000_000), 1..6),
        credit_accounts in prop::collection::vec(0usize..6, 1..4),
        skew in 0i64..3,
    ) {
        let (mut fx, company, accounts) = setup();
        let total: u64 = debits.iter().map(|(_, c)| c).sum();
        let mut lines: Vec<serde_json::Value> = debits.iter().map(|(a, c)| line(accounts[*a]).debit(&amount(*c)).build()).collect();
        let parts = credit_accounts.len() as u64;
        for (i, a) in credit_accounts.iter().enumerate() {
            let mut part = total / parts;
            if i == 0 { part += total % parts; }
            if part == 0 { continue; }
            lines.push(line(accounts[*a]).credit(&amount(part)).build());
        }
        let lines_count = lines.len();
        if skew > 0 {
            // Unbalance by one or two cents on an extra line: posting must be refused, nothing changes.
            lines.push(line(accounts[0]).credit(&amount(skew as u64)).build());
        }
        let draft = services::save_draft(&mut fx.book, fx.user, company, DraftInput::new("2026-03-01", "Prop", lines)).unwrap();
        let posted = services::post_journal(&mut fx.book, fx.user, company, draft.id, None);
        if skew > 0 || lines_count < 2 {
            prop_assert!(posted.is_err());
            let tb = reports::trial_balance(&mut fx.book, fx.user, company, "2026-12-31").unwrap();
            prop_assert_eq!(tb.debit.cents(), 0);
            return Ok(());
        }
        prop_assert!(posted.is_ok(), "{:?}", posted.err());
        let tb = reports::trial_balance(&mut fx.book, fx.user, company, "2026-12-31").unwrap();
        prop_assert_eq!(tb.debit, tb.credit);
        services::reverse_journal(&mut fx.book, fx.user, company, draft.id, "2026-03-02", "Prop reversal").unwrap();
        let after = reports::trial_balance(&mut fx.book, fx.user, company, "2026-12-31").unwrap();
        prop_assert_eq!(after.debit.cents(), 0);
        prop_assert_eq!(after.credit.cents(), 0);
        for account in &accounts {
            let register = reports::register(&mut fx.book, fx.user, company, *account, None, None).unwrap();
            prop_assert_eq!(register.closing.cents(), 0);
        }
        prop_assert!(fx.book.check().unwrap().is_empty());
    }

    #[test]
    fn money_text_round_trips_through_cents(cents in 0i128..fb_money::AMOUNT_LIMIT_CENTS) {
        let text = fb_money::Money::from_cents(cents).to_string();
        let parsed = fb_money::money(&fb_money::AmountInput::Text(text.clone())).unwrap();
        prop_assert_eq!(parsed.cents(), cents);
        prop_assert_eq!(parsed.to_decimal().to_string(), text);
    }
}

proptest! {
    #[test]
    fn review_extreme_exponents_never_panic_and_match_python_limits(
        coefficient in prop_oneof![Just("1"),Just("125"),Just("0"),Just("-0")],
        exponent in prop_oneof![Just("-18446744073709551618"),Just("-18446744073709551619"),Just("9999999999999999999"),Just("-9999999999999999999"),Just("1000000000000000000")]
    ) {
        let value=format!("{coefficient}e{exponent}");
        prop_assert!(fb_money::money(&fb_money::AmountInput::Text(value)).is_err());
    }
}

proptest! {
    #[test]
    fn arbitrary_monetary_text_never_panics(value in any::<String>()) {
        let _=fb_money::money(&fb_money::AmountInput::Text(value));
    }
    #[test]
    fn arbitrary_exponent_narrowing_never_panics(exponent in any::<i128>(), coefficient in "[0-9]{1,40}") {
        let _=fb_money::money(&fb_money::AmountInput::Text(format!("{coefficient}e{exponent}")));
    }
}
