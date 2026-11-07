//! Exact credits dump schema. Validate before comparer indexing.
use crate::dump_shape::Shape::{self, *};
use serde_json::Value;
use std::collections::BTreeSet;
const PARTY: Shape = Object(&[
    ("id", String),
    ("display_name", String),
    ("business_name", String),
    ("contact_name", String),
    ("billing_address", String),
    ("email", String),
    ("phone", String),
]);
const COMPANY: Shape = Object(&[("id", String), ("name", String), ("currency", String)]);
const ACCOUNT: Shape = Object(&[
    ("id", String),
    ("code", String),
    ("name", String),
    ("kind", String),
]);
const ITEM: Shape = EmptyOr(&Object(&[
    ("id", String),
    ("code", String),
    ("description", String),
    ("kind", String),
    ("sale_rate", Nullable(&String)),
]));
const ACCOUNTS: Shape = Object(&[("income_account", ACCOUNT)]);
const CREDITMEMO: Shape = Object(&[
    ("id", String),
    ("company_id", String),
    ("identity_id", String),
    ("customer_id", String),
    ("customer_snapshot", PARTY),
    ("company_snapshot", COMPANY),
    ("date", String),
    ("memo", String),
    ("receivables_id", String),
    ("tax_mode", String),
    ("tax_item_id", Nullable(&String)),
    ("tax_snapshot", Snapshot),
    ("tax_label", String),
    ("tax_rate", String),
    ("subtotal", String),
    ("taxable", String),
    ("tax", String),
    ("total", String),
    ("collected_tax_id", Nullable(&String)),
    ("journal_id", String),
    ("actor_id", String),
    ("actor_name", String),
]);
const CREDITMEMOLINE: Shape = Object(&[
    ("id", String),
    ("credit_memo_id", String),
    ("position", Int),
    ("item_id", Nullable(&String)),
    ("item_snapshot", ITEM),
    ("description", String),
    ("quantity", String),
    ("rate", String),
    ("taxable", Bool),
    ("amount", String),
    ("income_account_id", String),
    ("account_snapshot", ACCOUNTS),
]);
const CREDITMEMOVOID: Shape = Object(&[
    ("id", String),
    ("credit_memo_id", String),
    ("journal_id", String),
    ("date", String),
    ("reason", String),
    ("actor_id", String),
    ("actor_name", String),
]);
const CREDITAPPLICATION: Shape = Object(&[
    ("id", String),
    ("company_id", String),
    ("credit_memo_id", String),
    ("request_key", String),
    ("date", String),
    ("amount", String),
    ("actor_id", String),
    ("actor_name", String),
]);
const CREDITAPPLICATIONLINE: Shape = Object(&[
    ("id", String),
    ("application_id", String),
    ("invoice_id", String),
    ("invoice_revision_id", String),
    ("receivables_id", String),
    ("amount", String),
]);
const CREDITUNAPPLICATION: Shape = Object(&[
    ("id", String),
    ("application_id", String),
    ("date", String),
    ("reason", String),
    ("actor_id", String),
    ("actor_name", String),
]);
const CREDITREFUND: Shape = Object(&[
    ("id", String),
    ("company_id", String),
    ("identity_id", String),
    ("credit_memo_id", String),
    ("customer_id", String),
    ("customer_snapshot", PARTY),
    ("company_snapshot", COMPANY),
    ("date", String),
    ("amount", String),
    ("bank_id", String),
    ("receivables_id", String),
    ("method", String),
    ("check_number", String),
    ("memo", String),
    ("journal_id", String),
    ("actor_id", String),
    ("actor_name", String),
]);
const CREDITREFUNDVOID: Shape = Object(&[
    ("id", String),
    ("refund_id", String),
    ("journal_id", String),
    ("date", String),
    ("reason", String),
    ("actor_id", String),
    ("actor_name", String),
]);
const BALANCES: Shape = Object(&[
    ("id", String),
    ("company_id", String),
    ("customer_id", String),
    ("total", String),
    ("applied", String),
    ("refunded", String),
    ("available", String),
    ("credit_state", String),
]);
pub(super) const SHAPE: Shape = Object(&[
    ("creditmemo", Array(&CREDITMEMO)),
    ("creditmemoline", Array(&CREDITMEMOLINE)),
    ("creditmemovoid", Array(&CREDITMEMOVOID)),
    ("creditapplication", Array(&CREDITAPPLICATION)),
    ("creditapplicationline", Array(&CREDITAPPLICATIONLINE)),
    ("creditunapplication", Array(&CREDITUNAPPLICATION)),
    ("creditrefund", Array(&CREDITREFUND)),
    ("creditrefundvoid", Array(&CREDITREFUNDVOID)),
    ("balances", Array(&BALANCES)),
]);
pub(super) fn validate(d: &Value) -> Result<(), std::string::String> {
    let c = &d["credits"];
    for table in [
        "creditmemo",
        "creditmemoline",
        "creditapplication",
        "creditapplicationline",
        "creditrefund",
        "balances",
    ] {
        for row in c[table].as_array().unwrap() {
            for field in [
                "subtotal",
                "taxable",
                "tax",
                "total",
                "amount",
                "rate",
                "applied",
                "refunded",
                "available",
            ] {
                if let Some(v) = row.get(field).filter(|v| v.is_string())
                    && fb_core::calculations::unit_rate(v).is_err()
                {
                    return Err(format!("/credits/{table}/{field}: invalid money"));
                }
            }
            if table == "creditmemoline"
                && fb_core::calculations::quantity(&row["quantity"]).is_err()
            {
                return Err("/credits/creditmemoline: invalid quantity".into());
            }
            if table == "creditmemo"
                && let Some(members) = row["tax_snapshot"].get("members").and_then(Value::as_array)
            {
                let mut seen = BTreeSet::new();
                for member in members {
                    if !seen.insert(member["id"].as_str().unwrap()) {
                        return Err("/credits/creditmemo/tax_snapshot: duplicate member".into());
                    }
                }
            }
        }
    }
    for table in [
        "creditmemo",
        "creditmemoline",
        "creditmemovoid",
        "creditapplication",
        "creditapplicationline",
        "creditunapplication",
        "creditrefund",
        "creditrefundvoid",
        "balances",
    ] {
        let mut seen = BTreeSet::new();
        for row in c[table].as_array().unwrap() {
            if !seen.insert(row["id"].as_str().unwrap()) {
                return Err(format!("/credits/{table}: duplicate id"));
            }
        }
    }
    let keys = |table: &str| -> BTreeSet<&str> {
        c[table]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect()
    };
    if keys("creditmemo") != keys("balances") {
        return Err("/credits/balances: keys must equal memos".into());
    }
    for (table, field, parent) in [
        ("creditmemoline", "credit_memo_id", "creditmemo"),
        ("creditmemovoid", "credit_memo_id", "creditmemo"),
        ("creditapplication", "credit_memo_id", "creditmemo"),
        (
            "creditapplicationline",
            "application_id",
            "creditapplication",
        ),
        ("creditunapplication", "application_id", "creditapplication"),
        ("creditrefund", "credit_memo_id", "creditmemo"),
        ("creditrefundvoid", "refund_id", "creditrefund"),
    ] {
        let parent = keys(parent);
        for row in c[table].as_array().unwrap() {
            if !parent.contains(row[field].as_str().unwrap()) {
                return Err(format!("/credits/{table}: missing parent"));
            }
        }
    }
    for table in ["creditmemovoid", "creditunapplication", "creditrefundvoid"] {
        let field = match table {
            "creditmemovoid" => "credit_memo_id",
            "creditunapplication" => "application_id",
            _ => "refund_id",
        };
        let mut seen = BTreeSet::new();
        for row in c[table].as_array().unwrap() {
            if !seen.insert(row[field].as_str().unwrap()) {
                return Err(format!("/credits/{table}: duplicate parent"));
            }
        }
    }
    for (table, fields) in [
        ("creditmemo", &["identity_id"][..]),
        ("creditmemo", &["journal_id"][..]),
        ("creditmemoline", &["credit_memo_id", "position"][..]),
        ("creditapplication", &["company_id", "request_key"][..]),
        (
            "creditapplicationline",
            &["application_id", "invoice_id"][..],
        ),
        ("creditrefund", &["identity_id"][..]),
        ("creditrefund", &["journal_id"][..]),
        ("creditmemovoid", &["journal_id"][..]),
        ("creditrefundvoid", &["journal_id"][..]),
    ] {
        let mut seen = BTreeSet::new();
        for row in c[table].as_array().unwrap() {
            let key = fields
                .iter()
                .map(|f| row[*f].to_string())
                .collect::<Vec<_>>();
            if !seen.insert(key) {
                return Err(format!("/credits/{table}: duplicate alternate key"));
            }
        }
    }
    validate_values(d)?;
    Ok(())
}

include!("credits_validation.rs");
