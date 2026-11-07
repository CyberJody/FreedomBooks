//! Frozen snapshots are evidence, not optional display hints. Never coerce a
//! malformed object to an empty snapshot or consult today's directory labels.
use serde_json::Value;

fn strings(v: &Value, fields: &[&str]) -> bool {
    v.is_object()
        && fields
            .iter()
            .all(|k| v.get(*k).is_some_and(Value::is_string))
}
fn identity(v: &Value) -> bool {
    v.as_str().is_some_and(|s| !s.is_empty())
}
pub(super) fn header(v: &Value) -> bool {
    strings(v, &["id", "customer_id", "date", "actor_name", "memo"])
        && strings(
            &v["customer_snapshot"],
            &[
                "id",
                "display_name",
                "business_name",
                "contact_name",
                "billing_address",
                "email",
                "phone",
            ],
        )
        && identity(&v["customer_id"])
        && v["customer_snapshot"]["id"] == v["customer_id"]
        && strings(&v["company_snapshot"], &["id", "name", "currency"])
        && identity(&v["company_snapshot"]["id"])
}
pub(super) fn line(v: &Value) -> bool {
    let Some(accounts) = v["account_snapshot"].as_object() else {
        return false;
    };
    if accounts
        .keys()
        .any(|k| !["income_account", "expense_account"].contains(&k.as_str()))
    {
        return false;
    }
    for (role, field) in [
        ("income_account", "income_account_id"),
        ("expense_account", "expense_account_id"),
    ] {
        match v.get(field) {
            Some(Value::Null) | None if role == "expense_account" || v.get(field).is_some() => {
                if accounts.contains_key(role) {
                    return false;
                }
            }
            Some(id) if identity(id) => {
                let snapshot = &v["account_snapshot"][role];
                if !strings(snapshot, &["id", "code", "name", "kind"])
                    || snapshot["id"] != *id
                    || !matches!(
                        snapshot["kind"].as_str(),
                        Some("ASSET" | "LIABILITY" | "EQUITY" | "INCOME" | "EXPENSE")
                    )
                {
                    return false;
                }
            }
            _ => return false,
        }
    }
    let item = &v["item_snapshot"];
    match v.get("item_id") {
        Some(Value::Null) => item.as_object().is_some_and(|m| m.is_empty()),
        Some(id) if identity(id) => {
            strings(item, &["id", "code", "description", "kind"])
                && item["id"] == *id
                && item
                    .get("sale_rate")
                    .is_some_and(|r| r.is_null() || (r.is_string() && super::cents(r).is_ok()))
        }
        _ => false,
    }
}
pub(super) fn reversal(v: &Value) -> bool {
    strings(
        v,
        &[
            "id",
            "receipt_id",
            "journal_id",
            "date",
            "reason",
            "actor_name",
        ],
    ) && super::text(&v["reason"], 500, true).is_ok()
        && super::text(&v["actor_name"], 150, true).is_ok()
}

pub(super) fn tax(v: &Value) -> bool {
    let snapshot = &v["tax_snapshot"];
    match v["tax_mode"].as_str() {
        Some("NONE" | "MANUAL") => snapshot.as_object().is_some_and(|s| s.is_empty()),
        Some(mode @ ("RATE" | "GROUP")) => {
            strings(
                snapshot,
                &[
                    "id",
                    "kind",
                    "name",
                    "description",
                    "rate",
                    "taxable",
                    "tax",
                ],
            ) && snapshot["kind"] == mode
                && snapshot["id"] == v["tax_item_id"]
                && identity(&snapshot["id"])
                && super::cents(&snapshot["taxable"]).is_ok()
                && super::cents(&snapshot["tax"]).is_ok()
                && snapshot["members"].as_array().is_some_and(|members| {
                    members.iter().all(|m| {
                        strings(
                            m,
                            &["id", "name", "description", "rate", "agency_name", "tax"],
                        ) && identity(&m["id"])
                            && m.get("agency_id")
                                .is_some_and(|a| a.is_null() || identity(a))
                            && super::cents(&m["tax"]).is_ok()
                    })
                })
        }
        _ => false,
    }
}
