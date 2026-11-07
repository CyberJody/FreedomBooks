use crate::dump_shape::Shape::{self, *};
use serde_json::Value;
use std::collections::BTreeSet;
const ACCOUNT: Shape = Object(&[
    ("id", String),
    ("code", String),
    ("name", String),
    ("kind", String),
]);
const MAY_ACCOUNT: Shape = Object(&[
    ("id", Optional(&String)),
    ("code", Optional(&String)),
    ("name", Optional(&String)),
    ("kind", Optional(&String)),
]);
const PARTY: Shape = Object(&[
    ("id", Optional(&String)),
    ("display_name", String),
    ("business_name", Optional(&String)),
    ("contact_name", Optional(&String)),
    ("billing_address", Optional(&String)),
    ("email", Optional(&String)),
    ("phone", Optional(&String)),
]);
const COMPANY: Shape = Object(&[("id", String), ("name", String), ("currency", String)]);
const ITEM: Shape = Object(&[
    ("id", Optional(&String)),
    ("code", Optional(&String)),
    ("description", Optional(&String)),
    ("kind", Optional(&String)),
    ("purchase_cost", Optional(&Nullable(&String))),
    ("expense_account_id", Optional(&Nullable(&String))),
]);
const CHECK: Shape = Object(&[
    ("id", String),
    ("company_id", String),
    ("identity_id", String),
    ("revision", Int),
    ("bank_id", String),
    ("vendor_id", Nullable(&String)),
    ("number_key", String),
    ("state", String),
]);
const REVISION: Shape = Object(&[
    ("id", String),
    ("document_id", String),
    ("number", Int),
    ("state", String),
    ("action", String),
    ("reason", String),
    ("actor_id", String),
    ("actor_name", String),
    ("bank_id", String),
    ("bank_snapshot", ACCOUNT),
    ("payee_kind", String),
    ("vendor_id", Nullable(&String)),
    ("payee_name", String),
    ("payee_snapshot", PARTY),
    ("address", String),
    ("company_snapshot", COMPANY),
    ("date", String),
    ("check_number", String),
    ("print_later", Int),
    ("journal_id", Nullable(&String)),
    ("reversal_id", Nullable(&String)),
    ("memo", String),
    ("total", Int),
]);
const LINE: Shape = Object(&[
    ("id", String),
    ("revision_id", String),
    ("position", Int),
    ("tab", String),
    ("account_id", Nullable(&String)),
    ("account_snapshot", MAY_ACCOUNT),
    ("item_id", Nullable(&String)),
    ("item_snapshot", ITEM),
    ("description", String),
    ("quantity", Int),
    ("rate", Int),
    ("amount", Int),
]);
const TRANSFER: Shape = Object(&[
    ("id", String),
    ("company_id", String),
    ("identity_id", String),
    ("from_account_id", String),
    ("to_account_id", String),
    ("from_snapshot", ACCOUNT),
    ("to_snapshot", ACCOUNT),
    ("company_snapshot", COMPANY),
    ("date", String),
    ("amount", Int),
    ("memo", String),
    ("journal_id", String),
    ("actor_id", String),
    ("actor_name", String),
]);
const VOID: Shape = Object(&[
    ("id", String),
    ("transfer_id", String),
    ("journal_id", String),
    ("date", String),
    ("reason", String),
    ("actor_id", String),
    ("actor_name", String),
]);
pub(super) const SHAPE: Shape = Object(&[
    ("numbers", Map(&String)),
    ("check", Array(&CHECK)),
    ("checkrevision", Array(&REVISION)),
    ("checkline", Array(&LINE)),
    ("transfer", Array(&TRANSFER)),
    ("transfervoid", Array(&VOID)),
]);

fn text(v: &Value, max: usize, required: bool) -> bool {
    v.as_str()
        .is_some_and(|s| s.chars().count() <= max && (!required || !s.trim().is_empty()))
}
fn bounded(v: &Value, max: i64) -> bool {
    v.as_i64().is_some_and(|n| (1..=max).contains(&n))
}
fn date(v: &Value) -> bool {
    v.as_str().is_some_and(|s| {
        s.len() == 10
            && s.as_bytes()[4] == b'-'
            && s.as_bytes()[7] == b'-'
            && !s.starts_with("0000")
            && s.bytes()
                .enumerate()
                .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
            && chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
    })
}
fn snapshot(row: &Value, field: &str, kind: &str, id: &Value) -> bool {
    fb_db::checks_snapshots::valid(kind, &row[field], id.as_str())
}
fn unique(rows: &[Value], fields: &[&str]) -> bool {
    rows.iter()
        .map(|r| fields.iter().map(|k| r[k].to_string()).collect::<Vec<_>>())
        .collect::<BTreeSet<_>>()
        .len()
        == rows.len()
}
pub(super) fn validate(dump: &Value) -> Result<(), std::string::String> {
    let p = &dump["checks"];
    let rows = |table: &str| {
        p[table]
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| format!("/checks/{table}: expected rows"))
    };
    let checks = rows("check")?;
    let revisions = rows("checkrevision")?;
    let lines = rows("checkline")?;
    let transfers = rows("transfer")?;
    let voids = rows("transfervoid")?;
    let require = |ok: bool, message: &str| {
        if ok {
            Ok(())
        } else {
            Err(format!("/checks: {message}"))
        }
    };
    const MONEY: i64 = 9_999_999_999_999_999;
    for table in [checks, revisions, lines, transfers, voids] {
        require(
            unique(table, &["id"]) && table.iter().all(|v| text(&v["id"], usize::MAX, true)),
            "invalid or duplicate id",
        )?;
        require(
            table.iter().all(|row| {
                row.as_object().is_some_and(|fields| {
                    fields.iter().all(|(name, value)| {
                        !name.ends_with("_id") || value.is_null() || text(value, usize::MAX, true)
                    })
                })
            }),
            "blank reference identity",
        )?;
    }
    for (table, fields) in [
        (checks, &["identity_id"][..]),
        (revisions, &["document_id", "number"][..]),
        (lines, &["revision_id", "position"][..]),
        (transfers, &["identity_id"][..]),
        (transfers, &["journal_id"][..]),
        (voids, &["transfer_id"][..]),
        (voids, &["journal_id"][..]),
    ] {
        require(unique(table, fields), "duplicate composite evidence")?;
    }
    let live: Vec<Value> = checks
        .iter()
        .filter(|c| c["state"] != "VOID" && c["number_key"] != "")
        .cloned()
        .collect();
    require(
        unique(&live, &["company_id", "bank_id", "number_key"]),
        "duplicate live check number",
    )?;
    for c in checks {
        require(
            bounded(&c["revision"], i64::MAX)
                && matches!(c["state"].as_str(), Some("DRAFT" | "RECORDED" | "VOID"))
                && c["number_key"]
                    .as_str()
                    .is_some_and(fb_db::check_numbers::valid),
            "invalid check root",
        )?;
        let history: Vec<_> = revisions
            .iter()
            .filter(|r| r["document_id"] == c["id"])
            .collect();
        require(
            history.len() as i64 == c["revision"].as_i64().unwrap_or(0),
            "incomplete revision history",
        )?;
        require(
            history.iter().any(|r| {
                r["number"] == c["revision"]
                    && ["state", "bank_id", "vendor_id"]
                        .iter()
                        .all(|k| r[k] == c[k])
                    && r["check_number"] == c["number_key"]
            }),
            "missing or inconsistent current revision",
        )?;
    }
    for r in revisions {
        let c = checks
            .iter()
            .find(|c| c["id"] == r["document_id"])
            .ok_or("/checks: missing revision parent")?;
        require(
            bounded(&r["number"], c["revision"].as_i64().unwrap_or(0))
                && bounded(&r["total"], MONEY)
                && date(&r["date"])
                && text(&r["reason"], 500, r["state"] == "VOID")
                && text(&r["memo"], 2000, false)
                && text(&r["payee_name"], 120, true)
                && text(&r["address"], 1000, false)
                && matches!(r["payee_kind"].as_str(), Some("VENDOR" | "OTHER"))
                && (r["payee_kind"] == "VENDOR") == r["vendor_id"].is_string()
                && matches!(r["print_later"].as_i64(), Some(0 | 1))
                && r["check_number"]
                    .as_str()
                    .is_some_and(fb_db::check_numbers::valid)
                && (r["print_later"] == 1) == (r["check_number"] == ""),
            "invalid revision values",
        )?;
        require(
            snapshot(r, "bank_snapshot", "account", &r["bank_id"])
                && snapshot(r, "payee_snapshot", "party", &r["vendor_id"])
                && snapshot(r, "company_snapshot", "company", &c["company_id"]),
            "invalid header snapshot",
        )?;
        let previous = revisions.iter().find(|old| {
            old["document_id"] == r["document_id"]
                && old["number"].as_i64() == r["number"].as_i64().and_then(|n| n.checked_sub(1))
        });
        require(
            match (r["state"].as_str(), r["action"].as_str(), previous) {
                (Some("DRAFT"), Some("created"), None) => r["number"] == 1,
                (Some("DRAFT"), Some("revised"), Some(old)) => old["state"] == "DRAFT",
                (Some("RECORDED"), Some("recorded"), Some(old)) => old["state"] == "DRAFT",
                (Some("RECORDED"), Some("numbered"), Some(old)) => {
                    old["state"] == "RECORDED" && old["print_later"] == 1 && r["print_later"] == 0
                }
                (Some("VOID"), Some("voided"), Some(old)) => old["state"] == "RECORDED",
                _ => false,
            },
            "invalid revision transition",
        )?;
        require(
            match r["state"].as_str() {
                Some("DRAFT") => r["journal_id"].is_null() && r["reversal_id"].is_null(),
                Some("RECORDED") => r["journal_id"].is_string() && r["reversal_id"].is_null(),
                Some("VOID") => r["journal_id"].is_string() && r["reversal_id"].is_string(),
                _ => false,
            },
            "invalid journal evidence",
        )?;
        let evidence: Vec<_> = lines
            .iter()
            .filter(|l| l["revision_id"] == r["id"])
            .collect();
        require(
            (1..=50).contains(&evidence.len())
                && evidence
                    .iter()
                    .all(|l| bounded(&l["position"], evidence.len() as i64))
                && evidence
                    .iter()
                    .map(|l| i128::from(l["amount"].as_i64().unwrap_or(0)))
                    .sum::<i128>()
                    == i128::from(r["total"].as_i64().unwrap_or(0)),
            "incomplete line evidence or wrong total",
        )?;
    }
    for l in lines {
        let r = revisions
            .iter()
            .find(|r| r["id"] == l["revision_id"])
            .ok_or("/checks: missing line parent")?;
        require(
            bounded(&l["quantity"], 999_999_999_999_999_999)
                && bounded(&l["rate"], MONEY)
                && bounded(&l["amount"], MONEY)
                && text(&l["description"], 1000, false)
                && matches!(l["tab"].as_str(), Some("EXPENSE" | "ITEM"))
                && (l["tab"] == "ITEM") == l["item_id"].is_string()
                && (l["tab"] != "EXPENSE" || l["quantity"] == 10000)
                && (r["state"] == "DRAFT" || l["account_id"].is_string()),
            "invalid line values",
        )?;
        require(
            (i128::from(l["quantity"].as_i64().unwrap()) * i128::from(l["rate"].as_i64().unwrap())
                + 5000)
                / 10000
                == i128::from(l["amount"].as_i64().unwrap()),
            "invalid line calculation",
        )?;
        require(
            snapshot(l, "account_snapshot", "account", &l["account_id"])
                && snapshot(l, "item_snapshot", "item", &l["item_id"]),
            "invalid line snapshot",
        )?;
    }
    for t in transfers {
        require(
            date(&t["date"])
                && bounded(&t["amount"], MONEY)
                && t["from_account_id"] != t["to_account_id"]
                && text(&t["memo"], 2000, false)
                && snapshot(t, "from_snapshot", "account", &t["from_account_id"])
                && snapshot(t, "to_snapshot", "account", &t["to_account_id"])
                && snapshot(t, "company_snapshot", "company", &t["company_id"]),
            "invalid transfer evidence",
        )?;
    }
    for v in voids {
        let t = transfers
            .iter()
            .find(|t| t["id"] == v["transfer_id"])
            .ok_or("/checks: missing void parent")?;
        require(
            date(&v["date"])
                && v["date"].as_str() >= t["date"].as_str()
                && text(&v["reason"], 500, true),
            "invalid transfer void",
        )?;
    }
    require(
        p["numbers"].as_object().is_some_and(|numbers| {
            numbers.values().all(|v| {
                v.as_str().is_some_and(|s| {
                    !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit())
                })
            })
        }),
        "invalid next number evidence",
    )?;
    Ok(())
}
