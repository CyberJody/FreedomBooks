//! Credits callbacks are pure: no connection capture, hooks, configuration or IO.
use rusqlite::{Connection, functions::FunctionFlags};
use serde_json::Value;
pub use crate::check_numbers::char_allowed as check_number_char_allowed;
fn ascii_whitespace(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '\r' | '\x0c' | '\x0b')
}

fn text(v: &Value, max: usize, required: bool) -> bool {
    v.as_str().is_some_and(|s| {
        s.chars().count() <= max && (!required || !s.trim_matches(fb_money::py_isspace).is_empty())
    })
}
fn snapshot(v: &Value, fields: &[&str]) -> bool {
    v.as_object()
        .is_some_and(|m| m.len() == fields.len() && fields.iter().all(|k| v[*k].is_string()))
}
// Same evidence contract as the sales 0016 and payables 0027 fixes:
// complete nested types, frozen foreign IDs, and no live-label comparisons.
fn identity(v: &Value) -> bool {
    v.as_str().is_some_and(|s| !s.is_empty())
}
fn strings(v: &Value, fields: &[&str]) -> bool {
    v.is_object()
        && fields
            .iter()
            .all(|k| v.get(*k).is_some_and(Value::is_string))
}
fn decimal(v: &Value, scale: u32, max: i64) -> bool {
    v.as_str()
        .and_then(|s| rust_decimal::Decimal::from_str_exact(s).ok())
        .is_some_and(|n| {
            n.scale() <= scale
                && n >= rust_decimal::Decimal::ZERO
                && n <= rust_decimal::Decimal::from(max)
                    + if scale == 2 {
                        rust_decimal::Decimal::new(99, 2)
                    } else {
                        rust_decimal::Decimal::ZERO
                    }
        })
}
fn line_snapshot(v: &Value) -> bool {
    let a = &v["account_snapshot"];
    if !a.as_object().is_some_and(|m| m.len() == 1)
        || !snapshot(&a["income_account"], &["id", "code", "name", "kind"])
        || !identity(&v["income_account_id"])
        || a["income_account"]["id"] != v["income_account_id"]
        || a["income_account"]["kind"] != "INCOME"
    {
        return false;
    }
    let item = &v["item_snapshot"];
    if v["item_id"].is_null() {
        return item.as_object().is_some_and(|m| m.is_empty());
    }
    identity(&v["item_id"])
        && strings(item, &["id", "code", "description", "kind"])
        && item.as_object().is_some_and(|m| m.len() == 5)
        && item["id"] == v["item_id"]
        && matches!(item["kind"].as_str(), Some("SERVICE" | "NONSTOCK"))
        && item
            .get("sale_rate")
            .is_some_and(|r| r.is_null() || decimal(r, 2, 99_999_999_999_999))
}
pub fn fields(table: &str, v: &Value) -> bool {
    if !v.is_object() || !identity(&v["id"]) {
        return false;
    }
    let actor = table.ends_with("line")
        || (text(&v["actor_name"], 150, true) && identity(&v["actor_id"]) && v["date"].is_string());
    let digest = || {
        ["creation_digest", "review_digest"].iter().all(|k| {
            v[*k]
                .as_str()
                .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        })
    };
    let party = || {
        snapshot(
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
        ) && snapshot(&v["company_snapshot"], &["id", "name", "currency"])
            && identity(&v["customer_id"])
            && identity(&v["company_id"])
            && v["customer_snapshot"]["id"] == v["customer_id"]
            && v["company_snapshot"]["id"] == v["company_id"]
    };
    actor
        && match table {
            "creditmemo" => {
                digest()
                    && party()
                    && ["identity_id", "receivables_id", "journal_id"]
                        .iter()
                        .all(|k| identity(&v[*k]))
                    && text(&v["memo"], 2000, false)
                    && text(&v["tax_label"], 120, false)
                    && tax_members(v)
            }
            "creditrefund" => {
                digest()
                    && party()
                    && [
                        "identity_id",
                        "credit_memo_id",
                        "bank_id",
                        "receivables_id",
                        "journal_id",
                    ]
                    .iter()
                    .all(|k| identity(&v[*k]))
                    && matches!(v["method"].as_str(), Some("CHECK" | "CASH" | "OTHER"))
                    && (v["method"] == "CHECK" || v["check_number"] == "")
                    && text(&v["memo"], 2000, false)
                    && text(&v["check_number"], 20, false)
                    && v["check_number"].as_str().is_some_and(|s| {
                        s.trim_matches(ascii_whitespace) == s
                            && s.chars().all(check_number_char_allowed)
                    })
            }
            "creditapplication" => {
                digest()
                    && identity(&v["company_id"])
                    && identity(&v["credit_memo_id"])
                    && v["request_key"].as_str().is_some_and(|s| {
                        s.len() == 36
                            && s.bytes().enumerate().all(|(i, b)| {
                                if [8, 13, 18, 23].contains(&i) {
                                    b == b'-'
                                } else {
                                    b.is_ascii_hexdigit()
                                }
                            })
                    })
            }
            "creditmemovoid" | "creditrefundvoid" | "creditunapplication" => {
                let parent = match table {
                    "creditmemovoid" => "credit_memo_id",
                    "creditrefundvoid" => "refund_id",
                    _ => "application_id",
                };
                text(&v["reason"], 500, true)
                    && identity(&v[parent])
                    && (table == "creditunapplication" || identity(&v["journal_id"]))
            }
            "creditmemoline" => {
                identity(&v["credit_memo_id"])
                    && text(&v["description"], 1000, true)
                    && v["taxable"].is_boolean()
                    && line_snapshot(v)
            }
            "creditapplicationline" => [
                "application_id",
                "invoice_id",
                "invoice_revision_id",
                "receivables_id",
            ]
            .iter()
            .all(|k| identity(&v[*k])),
            _ => false,
        }
}
fn tax_members(v: &Value) -> bool {
    let mode = v["tax_mode"].as_str();
    if !decimal(&v["tax_rate"], 4, 100) {
        return false;
    }
    if matches!(mode, Some("NONE" | "MANUAL")) {
        return v["tax_item_id"].is_null()
            && v["tax_snapshot"].as_object().is_some_and(|m| m.is_empty());
    }
    if !matches!(mode, Some("RATE" | "GROUP")) {
        return false;
    }
    let snap = &v["tax_snapshot"];
    if !strings(
        snap,
        &[
            "id",
            "kind",
            "name",
            "description",
            "rate",
            "taxable",
            "tax",
        ],
    ) || !snap.as_object().is_some_and(|m| m.len() == 8)
        || !identity(&v["tax_item_id"])
        || v["tax_item_id"] != snap["id"]
        || v["tax_mode"] != snap["kind"]
        || !decimal(&snap["rate"], 4, 100)
        || !decimal(&snap["taxable"], 2, 99_999_999_999_999)
        || !decimal(&snap["tax"], 2, 99_999_999_999_999)
    {
        return false;
    }
    let Some(members) = snap["members"].as_array() else {
        return false;
    };
    let mut ids = std::collections::BTreeSet::new();
    let count_ok = if mode == Some("RATE") {
        members.len() == 1
    } else {
        (2..=20).contains(&members.len())
    };
    count_ok
        && members.iter().all(|m| {
            strings(
                m,
                &["id", "name", "description", "rate", "agency_name", "tax"],
            ) && m.as_object().is_some_and(|m| m.len() == 7)
                && identity(&m["id"])
                && ids.insert(m["id"].as_str())
                && m.get("agency_id")
                    .is_some_and(|a| a.is_null() || identity(a))
                && decimal(&m["rate"], 4, 100)
                && decimal(&m["tax"], 2, 99_999_999_999_999)
                && (mode != Some("RATE") || m["id"] == v["tax_item_id"])
        })
}
pub(super) fn register(c: &Connection) -> rusqlite::Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8
        | FunctionFlags::SQLITE_DETERMINISTIC
        | FunctionFlags::SQLITE_INNOCUOUS;
    c.create_scalar_function("fb_credits_sum", 1, flags, |ctx| {
        let raw: String = ctx.get(0)?;
        let result = crate::json_evidence::parse_unique_json_as::<Vec<Value>>(&raw).ok().and_then(|a| {
            a.into_iter()
                .try_fold(0i64, |sum, v| sum.checked_add(v.as_i64()?))
        });
        Ok(result)
    })?;
    c.create_scalar_function("fb_credits_fields", 2, flags, |ctx| {
        let table: String = ctx.get(0)?;
        let raw: String = ctx.get(1)?;
        Ok(crate::json_evidence::parse_unique_json_as(&raw)
            .ok()
            .is_some_and(|v| fields(&table, &v)))
    })?;
    Ok(())
}
pub(super) fn check(c: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = c.prepare(include_str!("credits_integrity.sql"))?;
    stmt.query_map([], |r| r.get::<_, String>(0))?.collect()
}
