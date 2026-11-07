//! Pure prepayment predicates. They capture no connection or mutable state.
use rusqlite::{Connection, functions::FunctionFlags};
use serde_json::Value;

fn text(v: &Value, key: &str, max: usize, required: bool) -> bool {
    v[key].as_str().is_some_and(|s| {
        s.chars().count() <= max && (!required || !s.trim_matches(fb_money::py_isspace).is_empty())
    })
}
pub use crate::dates::parse_iso_date as parse_prepayment_date;

/// One number predicate for service normalization, SQL evidence and harness.
pub fn validate_prepayment_number(value: &str) -> Result<&str, &'static str> {
    crate::check_numbers::normalize(value)
}
fn number_fields(v: &Value) -> bool {
    if v.get("method").is_none() && v.get("check_number").is_none() {
        return true;
    }
    let (Some(method), Some(number)) = (v["method"].as_str(), v["check_number"].as_str()) else {
        return false;
    };
    ["CHECK", "CASH", "OTHER"].contains(&method)
        && (method == "CHECK" || number.is_empty())
        && validate_prepayment_number(number) == Ok(number)
}
fn strings(v: &Value, fields: &[&str]) -> bool {
    v.is_object() && fields.iter().all(|k| v[*k].is_string())
}
fn ids(v: &Value, fields: &[&str]) -> bool {
    fields
        .iter()
        .all(|k| v[*k].as_str().is_some_and(|s| !s.is_empty()))
}
fn canonical_date(v: &Value) -> bool {
    v.as_str()
        .is_some_and(|s| parse_prepayment_date(s).as_deref() == Some(s))
}
/// Stored amounts have the canonical two-place representation, within DB bounds.
pub fn prepayment_cents(v: &Value) -> Option<i64> {
    let s = v.as_str()?;
    let (whole, fraction) = s.split_once('.')?;
    if whole.is_empty()
        || fraction.len() != 2
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let n = whole
        .parse::<i64>()
        .ok()?
        .checked_mul(100)?
        .checked_add(fraction.parse().ok()?)?;
    (n <= 9_999_999_999_999_999 && fb_money::Money::from_db(n).to_string() == s).then_some(n)
}
fn snapshots(v: &Value) -> bool {
    strings(
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
    ) && v["customer_snapshot"]["id"] == v["customer_id"]
        && strings(&v["company_snapshot"], &["id", "name", "currency"])
        && v["company_snapshot"]["id"] == v["company_id"]
        && v["company_snapshot"]["currency"]
            .as_str()
            .is_some_and(|s| s.len() == 3 && s.bytes().all(|c| c.is_ascii_uppercase()))
}
/// Full, table-specific evidence shape. Relational equality and parent ownership
/// are enforced by the matching SQL insert/seal/open predicates.
pub fn prepayment_snapshot_ok(kind: &str, v: &Value) -> bool {
    if !v.is_object()
        || !ids(v, &["id", "journal_id"])
        || !canonical_date(&v["date"])
        || !text(v, "actor_name", 150, true)
    {
        return false;
    }
    if matches!(kind, "unapplication" | "refundvoid" | "depositvoid") {
        let parent = match kind {
            "unapplication" => "application_id",
            "refundvoid" => "refund_id",
            _ => "receipt_id",
        };
        return ids(v, &[parent])
            && text(v, "reason", 500, true)
            && (kind == "depositvoid" || ids(v, &["actor_id"]));
    }
    if !ids(v, &["company_id", "identity_id", "actor_id"])
        || !prepayment_cents(&v["amount"]).is_some_and(|n| n > 0)
        || !["creation_digest", "review_digest"].iter().all(|k| {
            v[*k]
                .as_str()
                .is_some_and(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()))
        })
    {
        return false;
    }
    match kind {
        "application" => ids(
            v,
            &[
                "deposit_id",
                "invoice_id",
                "invoice_revision_id",
                "receivables_id",
                "liability_id",
            ],
        ),
        "refund" => {
            ids(v, &["deposit_id", "customer_id", "bank_id", "liability_id"])
                && strings(v, &["method", "check_number"])
                && snapshots(v)
                && text(v, "memo", 2000, false)
                && number_fields(v)
        }
        "deposit" => {
            ids(v, &["customer_id", "destination_account_id"])
                && snapshots(v)
                && v["kind"] == "CDP"
                && matches!(v["destination"].as_str(), Some("BANK" | "UNDEPOSITED"))
                && matches!(
                    v["method"].as_str(),
                    Some("CHECK" | "CASH" | "CARD" | "TRANSFER" | "OTHER")
                )
                && text(v, "reference", 120, false)
                && text(v, "memo", 2000, false)
        }
        _ => false,
    }
}
pub(super) fn register(conn: &Connection) -> rusqlite::Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8
        | FunctionFlags::SQLITE_DETERMINISTIC
        | FunctionFlags::SQLITE_INNOCUOUS;
    conn.create_scalar_function("fb_pp_snapshot_ok", 2, flags, |ctx| {
        let kind: String = ctx.get(0)?;
        let raw: String = ctx.get(1)?;
        Ok(crate::json_evidence::parse_unique_json_as::<Value>(&raw).is_ok_and(|v| prepayment_snapshot_ok(&kind, &v)))
    })?;
    conn.create_scalar_function("fb_pp_within", 3, flags, |ctx| {
        let limit: Option<i64> = ctx.get(0)?;
        let extra: i64 = ctx.get(1)?;
        let raw: String = ctx.get(2)?;
        let valid = (|| {
            let limit = limit?;
            let amounts: Vec<i64> = crate::json_evidence::parse_unique_json_as(&raw).ok()?;
            let total = amounts
                .into_iter()
                .try_fold(extra, |n, a| if a < 0 { None } else { n.checked_add(a) })?;
            Some(extra >= 0 && total <= limit)
        })();
        Ok(valid.unwrap_or(false))
    })?;
    conn.create_scalar_function("fb_pp_fields", 1, flags, |ctx| {
        let raw: String = ctx.get(0)?;
        let valid = crate::json_evidence::parse_unique_json_as::<Value>(&raw).is_ok_and(|v| {
            text(&v, "actor_name", 150, true)
                && number_fields(&v)
                && ["creation_digest", "review_digest"].iter().all(|k| {
                    v[*k]
                        .as_str()
                        .is_some_and(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()))
                })
                && (v.get("memo").is_none() || text(&v, "memo", 2000, false))
        });
        Ok(valid)
    })?;
    conn.create_scalar_function("fb_pp_reversal_fields", 5, flags, |ctx| {
        let raw: String = ctx.get(0)?;
        let id: String = ctx.get(1)?;
        let parent: String = ctx.get(2)?;
        let journal: String = ctx.get(3)?;
        let date: String = ctx.get(4)?;
        Ok(crate::json_evidence::parse_unique_json_as::<Value>(&raw).is_ok_and(|v| {
            v["id"] == id
                && (v["application_id"] == parent || v["refund_id"] == parent)
                && v["journal_id"] == journal
                && v["date"] == date
                && text(&v, "reason", 500, true)
                && text(&v, "actor_name", 150, true)
                && number_fields(&v)
        }))
    })?;
    Ok(())
}
