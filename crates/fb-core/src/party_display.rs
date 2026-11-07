//! FX9 plain directory/evidence labels, independent of a renderer.
use crate::{
    CompanyId, FbResult,
    directory::{self, Model},
    model::Journal,
    py,
};

use serde_json::{Value, json};
use std::collections::BTreeSet;
pub const UNKNOWN_ACCOUNT_LABEL: &str = "(unknown account)";
pub fn town_from_address(address: &str) -> String {
    let lines = address
        .split([
            '\n', '\r', '\u{b}', '\u{c}', '\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2028}', '\u{2029}',
        ])
        .map(py::py_strip)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    lines
        .get(1)
        .map(|line| py::py_strip(line.split(',').next().unwrap()).to_string())
        .unwrap_or_default()
}
pub fn duplicate_active_display_names(conn: &(impl fb_db::Query + ?Sized), company: CompanyId, model: Model) -> FbResult<BTreeSet<String>> {
    if model == Model::Item {
        return Ok(BTreeSet::new());
    }
    let mut counts = std::collections::BTreeMap::new();
    for row in directory::list(conn, company, model)? {
        if row["active"] == true {
            *counts.entry(row["display_name"].as_str().unwrap().to_string()).or_insert(0) += 1;
        }
    }
    Ok(counts.into_iter().filter(|(_, n)| *n > 1).map(|(s, _)| s).collect())
}
pub fn contact_list_label(record: &Value, duplicates: &BTreeSet<String>, peers: Option<&[Value]>, archived_suffix: &str) -> String {
    let mut name = record["display_name"].as_str().unwrap_or("").to_string();
    if duplicates.contains(&name) {
        let default = vec![record.clone()];
        let peers = peers.unwrap_or(&default);
        let town = town_from_address(record["billing_address"].as_str().unwrap_or(""));
        let disambiguator = if !town.is_empty()
            && peers
                .iter()
                .filter(|p| town_from_address(p["billing_address"].as_str().unwrap_or("")) == town)
                .count()
                == 1
        {
            town
        } else if let Some(email) = record["email"].as_str().filter(|s| !s.is_empty()) {
            email.into()
        } else {
            format!("#{}", record["id"].as_str().unwrap_or(""))
        };
        name = format!("{name} ({disambiguator})");
    }
    if record["active"] == false && !archived_suffix.is_empty() {
        name.push_str(archived_suffix);
    }
    name
}
pub fn contact_label_map(conn: &(impl fb_db::Query + ?Sized), company: CompanyId, model: Model, records: Option<&[Value]>) -> FbResult<Value> {
    let duplicates = duplicate_active_display_names(conn, company, model)?;
    let all = directory::list(conn, company, model)?;
    let rows = records.unwrap_or(&all);
    let mut out = json!({});
    for row in rows {
        let peers = all
            .iter()
            .filter(|p| p["display_name"] == row["display_name"])
            .cloned()
            .collect::<Vec<_>>();
        out[row["id"].as_str().unwrap()] = json!(contact_list_label(row, &duplicates, Some(&peers), ""));
    }
    Ok(out)
}
pub fn journal_evidence_link_label(journal: Option<&Journal>) -> String {
    let Some(j) = journal else {
        return "Journal".into();
    };
    format!(
        "{} {}",
        if j.manual() && !j.entry_number.is_empty() {
            "General journal"
        } else {
            "Journal"
        },
        j.display_number()
    )
}
fn role_label(s: &str) -> Option<&'static str> {
    Some(match s {
        "receivables" => "Accounts receivable",
        "payables" => "Accounts payable",
        "bank" => "Bank account",
        "collected_tax" => "Sales tax collected",
        "destination" => "Deposit to",
        "customer_deposits" => "Customer deposits",
        "from" => "Transfer from",
        "to" => "Transfer to",
        "undeposited" => "Undeposited funds",
        "income" => "Income account",
        "expense" => "Expense account",
        "debit" => "Account charged",
        _ => return None,
    })
}
pub fn posting_role_label(role: Option<&str>, kind: Option<&str>) -> String {
    let text = py::py_strip(role.unwrap_or(""));
    if text.is_empty() {
        return "Account".into();
    }
    let (prefix, rest) = text.split_once(':').map(|(a, b)| (a, Some(b))).unwrap_or((text, None));
    let Some(label) = role_label(prefix) else {
        let prefix = prefix.replace('_', " ");
        let mut chars = prefix.chars();
        return chars
            .next()
            .map(|c| crate::python_digits::titlecase(c) + &chars.as_str().to_lowercase())
            .unwrap_or_else(|| "Account".into());
    };
    if let Some(rest) = rest
        && matches!(
            (kind.unwrap_or(""), prefix),
            ("INV", "income") | ("BILL", "expense") | ("CHK", "debit")
        )
        && !rest.is_empty()
        && rest.chars().all(crate::python_digits::is_digit)
    {
        return format!("{label} (line {rest})");
    }
    label.into()
}
pub fn account_label_from_mapping(mapping: &[Value], account_id: &str) -> String {
    mapping
        .iter()
        .find(|r| r["id"].as_str() == Some(account_id))
        .map(|r| format!("{} · {}", r["code"].as_str().unwrap_or(""), r["name"].as_str().unwrap_or("")))
        .unwrap_or_else(|| UNKNOWN_ACCOUNT_LABEL.into())
}
pub fn account_mapping_rows(conn: &(impl fb_db::Query + ?Sized), company: CompanyId, ids: &[crate::AccountId]) -> FbResult<Vec<Value>> {
    Ok(crate::store::accounts(conn, company)?
        .into_iter()
        .filter(|a| ids.contains(&a.id))
        .map(|a| json!({"id":a.id.to_db(),"code":a.code,"name":a.name}))
        .collect())
}
