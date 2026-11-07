//! Typed, field-level audit identifier normalization; all non-ID content is exact.
use serde_json::{Value, json};
use std::collections::HashMap;
pub(super) type Ids = HashMap<String, HashMap<String, String>>;
fn id(value: &mut Value, kind: &str, ids: &Ids) {
    if value.is_null() {
        return;
    }
    let raw = value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string());
    *value = json!(
        ids.get(kind)
            .and_then(|m| m.get(&raw))
            .cloned()
            .unwrap_or_else(|| format!("<unknown:{kind}:{raw}>"))
    );
}
fn field(value: &mut Value, key: &str, kind: &str, ids: &Ids) {
    if let Some(v) = value.get_mut(key) {
        id(v, kind, ids);
    }
}
fn array(value: &mut Value, key: &str, kind: &str, ids: &Ids) {
    if let Some(v) = value.get_mut(key).and_then(Value::as_array_mut) {
        for v in v {
            id(v, kind, ids);
        }
    }
}
pub(super) fn normalize(event: &mut Value, ids: &Ids) -> bool {
    let Some((kind, raw)) = event["target"].as_str().and_then(|s| s.split_once(':')) else {
        return false;
    };
    if ![
        "customer",
        "vendor",
        "item",
        "taxitem",
        "documentidentity",
        "accountingsetup",
        "salestaxsettings",
    ]
    .contains(&kind)
    {
        return false;
    }
    let kind = kind.to_string();
    let mapped = ids
        .get(&kind)
        .and_then(|m| m.get(raw))
        .cloned()
        .unwrap_or_else(|| format!("<unknown:{raw}>"));
    event["target"] = json!(format!("{kind}:{mapped}"));
    for key in ["before", "after"] {
        let Some(snapshot) = event["detail"].get_mut(key).filter(|v| v.is_object()) else {
            continue;
        };
        match kind.as_str() {
            "customer" | "vendor" | "item" => {
                field(snapshot, "id", &kind, ids);
                field(snapshot, "company_id", "company", ids);
                for f in [
                    "income_account",
                    "expense_account",
                    "default_expense_account",
                ] {
                    field(snapshot, f, "account", ids);
                }
            }
            "taxitem" => {
                field(snapshot, "id", "taxitem", ids);
                field(snapshot, "agency_id", "vendor", ids);
                array(snapshot, "member_ids", "taxitem", ids);
            }
            "salestaxsettings" => field(snapshot, "default_item_id", "taxitem", ids),
            "accountingsetup" => {
                for role in fb_core::documents::ROLES {
                    field(snapshot, role, "account", ids);
                }
                array(snapshot, "banks", "account", ids);
            }
            _ => {}
        }
    }
    if event["action"] == "setup.filled" {
        for role in ["receivables","payables","undeposited","collected_tax"] {field(&mut event["detail"]["filled"],role,"account",ids);}
        array(&mut event["detail"]["filled"],"banks","account",ids);
    }
    if event["action"] == "salestax.bulk_step" {
        array(
            &mut event["detail"],
            "customers_marked_taxable",
            "customer",
            ids,
        );
        array(&mut event["detail"], "items_marked_taxable", "item", ids);
    }
    if event["action"] == "document.accounts"
        && let Some(accounts) = event["detail"]["accounts"].as_array_mut()
    {
        for account in accounts {
            field(account, "id", "account", ids);
        }
    }
    true
}

/// `document.accounts` lists its accounts in role order, and roles embed generated ids (`income:<account id>`), so
/// the two runtimes order the same accounts differently. After typed normalization, order them canonically by the
/// normalized (role, account); every entry is kept, so duplicates and complete values are still compared.
pub(crate) fn canonical_document_accounts(event: &mut Value) {
    if event["action"] != "document.accounts" {
        return;
    }
    if let Some(accounts) = event.get_mut("detail").and_then(|d| d.get_mut("accounts")).and_then(Value::as_array_mut) {
        accounts.sort_by(|a, b| (a["role"].to_string(), a["id"].to_string()).cmp(&(b["role"].to_string(), b["id"].to_string())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_accounts_order_is_independent_of_generated_ids() {
        let python = json!({"action": "document.accounts", "detail": {"accounts": [
            {"role": "destination", "id": "uf"}, {"role": "income:income", "id": "income"}, {"role": "income:income2", "id": "income2"}]}});
        let mut rust = json!({"action": "document.accounts", "detail": {"accounts": [
            {"role": "income:income2", "id": "income2"}, {"role": "destination", "id": "uf"}, {"role": "income:income", "id": "income"}]}});
        let mut sorted = python.clone();
        canonical_document_accounts(&mut sorted);
        canonical_document_accounts(&mut rust);
        assert_eq!(sorted, rust);
        assert_eq!(sorted, python);
        // Nothing is dropped: a duplicate stays visible to the comparison.
        let mut dup = json!({"action": "document.accounts", "detail": {"accounts": [{"role": "b", "id": "x"}, {"role": "a", "id": "y"}, {"role": "a", "id": "y"}]}});
        canonical_document_accounts(&mut dup);
        assert_eq!(dup["detail"]["accounts"].as_array().unwrap().len(), 3);
        // Other actions are untouched.
        let mut other = json!({"action": "x", "detail": {"accounts": [{"role": "b"}, {"role": "a"}]}});
        canonical_document_accounts(&mut other);
        assert_eq!(other["detail"]["accounts"][0]["role"], "b");
    }
    #[test]
    fn normalization_is_typed_and_never_rewrites_free_text_or_numbers() {
        let ids: Ids = [
            ("customer", [("1", "buyer")]),
            ("vendor", [("1", "agency")]),
            ("taxitem", [("1", "rate")]),
            ("company", [("1", "co")]),
            ("account", [("1", "cash")]),
        ]
        .into_iter()
        .map(|(kind, rows)| {
            (
                kind.into(),
                rows.into_iter()
                    .map(|(a, b)| (a.into(), b.into()))
                    .collect(),
            )
        })
        .collect();
        let mut event = json!({"target":"customer:1","action":"customer.created","detail":{"after":{"id":1,"company_id":1,"notes":"ID 1","display_name":"1","active":true},"business_number":1}});
        assert!(normalize(&mut event, &ids));
        assert_eq!(event["target"], "customer:buyer");
        assert_eq!(event["detail"]["after"]["id"], "buyer");
        assert_eq!(event["detail"]["after"]["notes"], "ID 1");
        assert_eq!(event["detail"]["after"]["display_name"], "1");
        assert_eq!(event["detail"]["business_number"], 1);
        let mut tax = json!({"target":"taxitem:1","action":"salestax.rate.created","detail":{"after":{"id":1,"agency_id":1,"rate":"1.0000","revision":1}}});
        normalize(&mut tax, &ids);
        assert_eq!(tax["detail"]["after"]["id"], "rate");
        assert_eq!(tax["detail"]["after"]["agency_id"], "agency");
        assert_eq!(tax["detail"]["after"]["revision"], 1);
        assert_eq!(tax["detail"]["after"]["rate"], "1.0000");
    }
}
