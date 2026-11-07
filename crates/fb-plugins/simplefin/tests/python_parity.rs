use fb_plugin_simplefin::parse::{epoch, money, parse_account_set, MoneyError};
use serde_json::{json, Value};

fn cases() -> Value {
    serde_json::from_str(include_str!("fixtures/python_client_cases.json")).unwrap()
}
#[test]
fn frozen_python_money_oracle() {
    for case in cases()["money"].as_array().unwrap() {
        let got = money(&case["input"]);
        match got {
            Ok(cents) => {
                assert_eq!(case["error"], "");
                assert_eq!(json!(cents), case["cents"], "input={}", case["input"]);
            }
            Err(error) => {
                let reason = match error {
                    MoneyError::Missing => "amount missing",
                    MoneyError::NotNumeric => "amount not numeric",
                    MoneyError::NotFinite => "amount not finite",
                    MoneyError::TooLarge => "amount too large",
                };
                assert_eq!(reason, case["error"], "input={}", case["input"]);
            }
        }
    }
}
#[test]
fn frozen_python_epoch_oracle() {
    for case in cases()["epoch"].as_array().unwrap() {
        assert_eq!(
            json!(epoch(&case["input"])),
            case["output"],
            "input={}",
            case["input"]
        );
    }
}
#[test]
fn frozen_python_account_set_oracle() {
    for case in cases()["account_sets"].as_array().unwrap() {
        let got = parse_account_set(&case["input"]).unwrap();
        let accounts:Vec<_>=got.accounts.iter().map(|a| {
            let transactions:Vec<_>=a.transactions.iter().map(|t| json!({"remote_id":t.remote_id,"posted_epoch":t.posted_epoch,
                "transacted_epoch":t.transacted_epoch,"amount":t.cents,"description":t.description,"pending":t.pending})).collect();
            json!({"conn_key":a.connection_key,"remote_id":a.remote_id,"name":a.name,"org_name":a.organization_name,
                "currency":a.currency,"balance":a.balance_cents,"available":a.available_cents,"balance_epoch":a.balance_epoch,
                "transactions":transactions,"skipped_count":a.skipped.len()})
        }).collect();
        let errors:Vec<_>=got.errors.iter().map(|e|json!({"code":e.code,"message":e.message,"conn_id":e.connection_id,"account_id":e.account_id})).collect();
        assert_eq!(
            json!({"accounts":accounts,"errors":errors,"skipped_count":got.skipped.len()}),
            case["output"]
        );
    }
}
