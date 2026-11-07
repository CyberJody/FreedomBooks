//! SimpleFIN plugin: the `/accounts` JSON body (`parse::parse_account_set_json`), then the feed
//! projections (`feed_accounts`, `feed_transactions` in a few time zones).
//!
//! Asserts: no panic; every accepted amount is within +/- 99,999,999,999,999.99 cents; every account
//! and transaction carries a non-empty remote id; the account inventory lists every parsed account.
#![no_main]
use libfuzzer_sys::fuzz_target;
use fb_plugin_simplefin::parse::parse_account_set_json;

const LIMIT: i64 = 9_999_999_999_999_999;

fuzz_target!(|data: &[u8]| {
    let Ok(set) = parse_account_set_json(data) else { return };
    let accounts = set.feed_accounts();
    for a in &set.accounts {
        assert!(!a.remote_id.is_empty());
        assert!(!a.currency.is_empty());
        assert!(a.balance_cents.abs() <= LIMIT, "balance out of range: {}", a.balance_cents);
        if let Some(v) = a.available_cents {
            assert!(v.abs() <= LIMIT);
        }
        for t in &a.transactions {
            assert!(!t.remote_id.is_empty());
            assert!(t.cents.abs() <= LIMIT, "amount out of range: {}", t.cents);
            assert!(t.description.chars().count() <= fb_plugin_simplefin::parse::DESCRIPTION_LIMIT);
        }
        assert!(a.name.chars().count() <= 200);
    }
    assert_eq!(accounts.len(), set.accounts.len());
    for zone in [chrono_tz::UTC, chrono_tz::America::New_York, chrono_tz::Pacific::Kiritimati, chrono_tz::Pacific::Pago_Pago] {
        let txs = set.feed_transactions(zone);
        assert_eq!(txs.len(), set.accounts.iter().map(|a| a.transactions.len()).sum::<usize>());
    }
});
