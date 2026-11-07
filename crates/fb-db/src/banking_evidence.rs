//! Pure predicates for frozen banking evidence, shared by SQL guards and typed reads.
//! These functions hold no connection or mutable state.
use serde_json::Value;

pub fn canonical_uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
            }
        })
}
pub fn calendar_date(s: &str) -> bool {
    s.len() == 10
        && s.bytes().enumerate().all(|(i, b)| {
            if [4, 7].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
        && ("0001-01-01"..="9999-12-31").contains(&s)
        && chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}
fn strings(v: &Value, fields: &[&str]) -> bool {
    v.is_object()
        && fields
            .iter()
            .all(|k| v.get(*k).is_some_and(Value::is_string))
}
fn ids(v: &Value, fields: &[&str]) -> bool {
    fields
        .iter()
        .all(|k| v[*k].as_str().is_some_and(canonical_uuid))
}
fn text(v: &Value, max: usize, required: bool) -> bool {
    v.as_str()
        .is_some_and(|s| s.chars().count() <= max && (!required || !s.trim().is_empty()))
}
fn hash(v: &Value) -> bool {
    v.as_str().is_some_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
/// Canonical fixed-point stored money, with independent document/aggregate bounds.
pub fn money(v: &Value, document: bool) -> Option<i64> {
    let s = v.as_str()?;
    let raw = s.strip_prefix('-').unwrap_or(s);
    let (whole, fraction) = raw.split_once('.')?;
    if whole.is_empty()
        || (whole.len() > 1 && whole.starts_with('0'))
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() != 2
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let n = whole
        .parse::<i128>()
        .ok()?
        .checked_mul(100)?
        .checked_add(fraction.parse::<i128>().ok()?)?;
    let n = if s.starts_with('-') { -n } else { n };
    if document && n.unsigned_abs() > 9_999_999_999_999_999 {
        return None;
    }
    i64::try_from(n).ok()
}
pub fn payload(kind: &str, v: &Value) -> bool {
    match kind {
        "deposit" => {
            strings(
                v,
                &[
                    "id",
                    "company_id",
                    "identity_id",
                    "bank_id",
                    "journal_id",
                    "date",
                    "amount",
                    "creation_digest",
                    "review_digest",
                    "memo",
                    "reference",
                    "actor_name",
                ],
            ) && ids(
                v,
                &["id", "company_id", "identity_id", "bank_id", "journal_id"],
            ) && v["date"].as_str().is_some_and(calendar_date)
                && money(&v["amount"], true).is_some_and(|n| n > 0)
                && hash(&v["creation_digest"])
                && hash(&v["review_digest"])
                && text(&v["memo"], 2000, false)
                && text(&v["reference"], 120, false)
                && text(&v["actor_name"], 150, true)
                && strings(&v["company_snapshot"], &["id", "name", "currency"])
                && v["company_snapshot"]["id"] == v["company_id"]
        }
        "reversal" => {
            strings(
                v,
                &[
                    "id",
                    "deposit_id",
                    "journal_id",
                    "date",
                    "reason",
                    "actor_name",
                ],
            ) && ids(v, &["id", "deposit_id", "journal_id"])
                && v["date"].as_str().is_some_and(calendar_date)
                && text(&v["reason"], 500, true)
                && text(&v["actor_name"], 150, true)
        }
        "reconciliation" => reconciliation(v),
        _ => false,
    }
}
fn reconciliation(v: &Value) -> bool {
    if !strings(
        v,
        &[
            "id",
            "company_id",
            "account_id",
            "status",
            "statement_date",
            "ending_balance",
            "request_key",
            "creation_digest",
            "started_by",
            "undo_reason",
        ],
    ) || !ids(v, &["id", "company_id", "account_id", "request_key"])
        || !hash(&v["creation_digest"])
        || !v["statement_date"]
            .as_str()
            .is_some_and(|d| calendar_date(d) && ("1900-01-02"..="9999-12-30").contains(&d))
        || money(&v["ending_balance"], true).is_none()
        || !v["hide_later"].is_boolean()
        || v["revision"].as_i64().is_none_or(|n| n < 1)
        || !text(&v["started_by"], 150, true)
    {
        return false;
    }
    if [
        "completion_key",
        "undo_key",
        "beginning_balance",
        "cleared_debit",
        "cleared_credit",
        "cleared_count",
        "service_charge",
        "service_account_id",
        "service_journal_id",
        "interest",
        "interest_account_id",
        "interest_journal_id",
    ]
    .iter()
    .any(|k| v.get(*k).is_none())
    {
        return false;
    }
    let done = matches!(v["status"].as_str(), Some("DONE" | "UNDONE"));
    let undone = v["status"] == "UNDONE";
    if !done && v["status"] != "OPEN" {
        return false;
    }
    for key in ["beginning_balance", "cleared_debit", "cleared_credit"] {
        if done {
            if money(&v[key], true).is_none_or(|n| key != "beginning_balance" && n < 0) {
                return false;
            }
        } else if !v[key].is_null() {
            return false;
        }
    }
    for (present, key, name) in [
        (done, "completion_key", "completed_by"),
        (undone, "undo_key", "undone_by"),
    ] {
        // Older OPEN rows omit the two unused actor display labels. Financial
        // fields and retry keys above must still be explicitly present/null.
        if present {
            if !v[key].as_str().is_some_and(canonical_uuid) || !text(&v[name], 150, true) {
                return false;
            }
        } else if !v[key].is_null() || !v[name].is_null() {
            return false;
        }
    }
    if (done && v["cleared_count"].as_i64().is_none_or(|n| n < 0))
        || (!done && !v["cleared_count"].is_null())
        || !text(&v["undo_reason"], 500, undone)
        || (!undone && v["undo_reason"] != "")
    {
        return false;
    }
    for (amount, account, journal) in [
        ("service_charge", "service_account_id", "service_journal_id"),
        ("interest", "interest_account_id", "interest_journal_id"),
    ] {
        if v[amount].is_null() {
            if !v[account].is_null() || !v[journal].is_null() {
                return false;
            }
        } else if money(&v[amount], true).is_none_or(|n| n <= 0) || !ids(v, &[account, journal]) {
            return false;
        }
    }
    true
}
