use crate::dump_shape::Shape::{self, *};
use serde_json::Value;
use std::collections::BTreeSet;
const COMPANY: Shape = Object(&[("id", String), ("name", String), ("currency", String)]);
const STATUS: Shape = Object(&[
    ("line_id", String),
    ("account_id", String),
    ("state", String),
    ("revision", Int),
    ("reconciliation_id", Nullable(&String)),
    ("changed_by", String),
]);
const WORDS: Shape = Object(&[
    ("card", Bool),
    ("left", String),
    ("right", String),
    ("left_one", String),
    ("right_one", String),
    ("left_short", String),
    ("right_short", String),
    ("service", String),
    ("ending", String),
    ("left_summary", String),
    ("right_summary", String),
]);
const READROW: Shape = Object(&[
    ("line_id", String),
    ("date", String),
    ("type", String),
    ("number", String),
    ("memo", String),
    ("party", String),
    ("left", Nullable(&String)),
    ("right", Nullable(&String)),
    ("state", String),
    ("revision", Int),
]);
const ROW: Shape = Object(&[
    ("id", String),
    ("journal_id", String),
    ("account_id", String),
    ("date", String),
    ("debit", String),
    ("credit", String),
    ("state", String),
    ("revision", Int),
    ("balance", String),
    ("number", String),
    ("memo", String),
    ("party", String),
    ("other", String),
    ("more_splits", Int),
    (
        "splits",
        Array(&Object(&[
            ("code", String),
            ("name", String),
            ("debit", String),
            ("credit", String),
            ("memo", String),
        ])),
    ),
]);
pub(super) const SHAPE: Shape = Object(&[
    (
        "screens",
        Map(&Object(&[
            ("reconciliation_id", String),
            ("words", WORDS),
            (
                "totals",
                Object(&[
                    ("beginning", String),
                    ("cleared_debit", String),
                    ("cleared_credit", String),
                    ("n_debit", Int),
                    ("n_credit", Int),
                    ("cleared", String),
                    ("ending", String),
                    ("difference", String),
                    ("cleared_debit_display", String),
                    ("cleared_credit_display", String),
                ]),
            ),
            ("total", Int),
            ("too_many", Bool),
            ("cap", Int),
            ("hidden_cleared", Int),
            ("notes", Array(&String)),
            ("sign", Int),
            ("left", Array(&READROW)),
            ("right", Array(&READROW)),
        ])),
    ),
    (
        "reports",
        Map(&Object(&[
            ("reconciliation_id", String),
            ("words", WORDS),
            (
                "summary",
                Object(&[
                    ("register_balance", String),
                    ("open_debit", String),
                    ("open_credit", String),
                ]),
            ),
            (
                "sides",
                Array(&Object(&[
                    ("key", String),
                    ("label", String),
                    ("count", Int),
                    ("total", String),
                    ("signed", String),
                    ("summary_label", String),
                    (
                        "groups",
                        Array(&Object(&[
                            ("type", String),
                            ("total", String),
                            ("rows", Array(&READROW)),
                        ])),
                    ),
                ])),
            ),
        ])),
    ),
    (
        "deposits",
        Array(&Object(&[
            ("id", String),
            ("company_id", String),
            ("identity_id", String),
            ("bank_id", String),
            ("journal_id", String),
            ("date", String),
            ("amount", String),
            ("reference", String),
            ("memo", String),
            ("actor_name", String),
            ("company_snapshot", COMPANY),
        ])),
    ),
    (
        "deposit_lines",
        Array(&Object(&[
            ("deposit_id", String),
            ("receipt_id", String),
            ("undeposited_id", String),
            ("amount", String),
        ])),
    ),
    (
        "reversals",
        Array(&Object(&[
            ("deposit_id", String),
            ("journal_id", String),
            ("date", String),
            ("reason", String),
            ("actor_name", String),
        ])),
    ),
    ("statuses", Array(&STATUS)),
    (
        "reconciliation_lines",
        Array(&Object(&[
            ("reconciliation_id", String),
            ("line_id", String),
            ("debit", String),
            ("credit", String),
        ])),
    ),
    (
        "reconciliations",
        Array(&Object(&[
            ("id", String),
            ("company_id", String),
            ("account_id", String),
            ("status", String),
            ("statement_date", String),
            ("ending_balance", String),
            ("hide_later", Bool),
            ("request_key", String),
            ("revision", Int),
            ("started_by", String),
            ("completed_by", Nullable(&String)),
            ("undone_by", Nullable(&String)),
            ("completion_key", Nullable(&String)),
            ("undo_key", Nullable(&String)),
            ("undo_reason", String),
            ("beginning_balance", Nullable(&String)),
            ("cleared_debit", Nullable(&String)),
            ("cleared_credit", Nullable(&String)),
            ("cleared_count", Nullable(&Int)),
            ("service_charge", Nullable(&String)),
            ("service_account_id", Nullable(&String)),
            ("service_journal_id", Nullable(&String)),
            ("interest", Nullable(&String)),
            ("interest_account_id", Nullable(&String)),
            ("interest_journal_id", Nullable(&String)),
        ])),
    ),
    (
        "registers",
        Map(&Object(&[
            ("account_id", String),
            ("page", Int),
            ("total", Int),
            ("opening", String),
            ("closing", String),
            (
                "totals",
                Object(&[
                    ("cleared", String),
                    ("ending", String),
                    ("left_open", String),
                    ("right_open", String),
                ]),
            ),
            (
                "labels",
                Object(&[
                    ("card", Bool),
                    ("left", String),
                    ("right", String),
                    ("left_open", String),
                    ("right_open", String),
                ]),
            ),
            ("rows", Array(&ROW)),
        ])),
    ),
]);
pub(super) fn validate(dump: &Value) -> Result<(), std::string::String> {
    let v = &dump["banking"];
    validate_values(v)?;
    validate_reports(v)?;
    crate::banking_validation::validate(dump)?;
    for row in v["statuses"].as_array().ok_or("Invalid statuses")? {
        if !["U", "C", "R"].contains(&row["state"].as_str().unwrap_or(""))
            || row["revision"].as_i64().is_none_or(|n| n < 1)
            || (row["state"] == "R") == row["reconciliation_id"].is_null()
        {
            return Err("Invalid banking status evidence".into());
        }
    }
    for row in v["reconciliations"]
        .as_array()
        .ok_or("Invalid reconciliations")?
    {
        for key in [
            "ending_balance",
            "beginning_balance",
            "cleared_debit",
            "cleared_credit",
            "service_charge",
            "interest",
        ] {
            if !row[key].is_null() && fb_db::banking_evidence::money(&row[key], true).is_none() {
                return Err(format!("Invalid stored reconciliation {key}"));
            }
        }
        let done = row["status"] != "OPEN";
        let undone = row["status"] == "UNDONE";
        for (required, keys) in [
            (
                done,
                &[
                    "completion_key",
                    "completed_by",
                    "beginning_balance",
                    "cleared_debit",
                    "cleared_credit",
                    "cleared_count",
                ][..],
            ),
            (undone, &["undo_key", "undone_by"][..]),
        ] {
            if keys.iter().any(|k| row[*k].is_null() == required) {
                return Err("Invalid reconciliation transition evidence".into());
            }
        }
        for (amount, account, journal) in [
            ("service_charge", "service_account_id", "service_journal_id"),
            ("interest", "interest_account_id", "interest_journal_id"),
        ] {
            if row[amount].is_null() != row[account].is_null()
                || row[amount].is_null() != row[journal].is_null()
                || (!row[amount].is_null()
                    && fb_db::banking_evidence::money(&row[amount], true).is_none_or(|n| n <= 0))
            {
                return Err("Invalid reconciliation charge structure".into());
            }
        }
        if !["OPEN", "DONE", "UNDONE"].contains(&row["status"].as_str().unwrap_or(""))
            || row["revision"].as_i64().is_none_or(|n| n < 1)
        {
            return Err("Invalid banking reconciliation evidence".into());
        }
    }
    for (field, keys) in [
        ("deposits", &["id"][..]),
        ("reversals", &["deposit_id"][..]),
        ("reconciliations", &["id"][..]),
        ("statuses", &["line_id"][..]),
        ("deposit_lines", &["deposit_id", "receipt_id"][..]),
        (
            "reconciliation_lines",
            &["reconciliation_id", "line_id"][..],
        ),
    ] {
        let mut seen = BTreeSet::new();
        for row in v[field]
            .as_array()
            .ok_or_else(|| format!("Invalid banking {field}"))?
        {
            let key = keys
                .iter()
                .map(|k| row[k].as_str().unwrap_or(""))
                .collect::<Vec<_>>();
            if !seen.insert(key) {
                return Err(format!("Duplicate banking {field}"));
            }
        }
    }
    for (field, op) in [
        ("registers", "banking_register"),
        ("screens", "banking_screen"),
        ("reports", "banking_report"),
    ] {
        let expected = dump["outcomes"]
            .as_array()
            .ok_or("Missing outcomes")?
            .iter()
            .filter(|v| v["op"] == op && v["ok"] == true)
            .count();
        if v[field]
            .as_object()
            .ok_or("Invalid banking report map")?
            .len()
            != expected
        {
            return Err(format!("Missing or extra banking {field}"));
        }
    }
    fn amounts(v: &Value) -> Result<(), std::string::String> {
        match v {
            Value::Object(m) => {
                for (k, v) in m {
                    if !m.contains_key("card")
                        && [
                            "amount",
                            "ending_balance",
                            "beginning_balance",
                            "cleared_debit",
                            "cleared_credit",
                            "service_charge",
                            "interest",
                            "opening",
                            "closing",
                            "debit",
                            "credit",
                            "balance",
                            "cleared",
                            "ending",
                            "left_open",
                            "right_open",
                            "left",
                            "right",
                            "difference",
                            "beginning",
                            "cleared_debit_display",
                            "cleared_credit_display",
                            "register_balance",
                            "open_debit",
                            "open_credit",
                            "signed",
                            "total",
                        ]
                        .contains(&k.as_str())
                        && !v.is_null()
                        && !v.is_object()
                        && !v.is_array()
                    {
                        if let Some(s) = v.as_str() {
                            let amount = fb_db::banking_evidence::money(
                                &Value::String(s.into()),
                                [
                                    "amount",
                                    "ending_balance",
                                    "beginning_balance",
                                    "service_charge",
                                    "interest",
                                    "debit",
                                    "credit",
                                    "left",
                                    "right",
                                ]
                                .contains(&k.as_str()),
                            );
                            if amount.is_none_or(|n| {
                                (["amount", "service_charge", "interest", "left", "right"]
                                    .contains(&k.as_str())
                                    && n <= 0)
                                    || ([
                                        "debit",
                                        "credit",
                                        "cleared_debit",
                                        "cleared_credit",
                                        "left_open",
                                        "right_open",
                                        "open_debit",
                                        "open_credit",
                                        "total",
                                    ]
                                    .contains(&k.as_str())
                                        && n < 0)
                            }) {
                                return Err(format!("Invalid banking amount {k}"));
                            }
                        } else if !v.is_u64() {
                            return Err(format!("Invalid banking amount {k}"));
                        }
                    }
                    amounts(v)?;
                }
            }
            Value::Array(a) => {
                for v in a {
                    amounts(v)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    amounts(v)?;
    Ok(())
}
// Run after SHAPE, and fail closed on every date/key/count regardless of dump equality.
fn validate_values(v: &Value) -> Result<(), std::string::String> {
    match v {
        Value::Object(fields) => {
            if let Some(state) = fields.get("state")
                && (!state.as_str().is_some_and(|s| ["U", "C", "R"].contains(&s))
                    || (state != "U"
                        && fields
                            .get("revision")
                            .and_then(Value::as_i64)
                            .is_none_or(|n| n < 1)))
            {
                return Err("Invalid banking row state/revision".into());
            }
            if fields.contains_key("company_snapshot")
                && fields["company_snapshot"]["id"] != fields["company_id"]
            {
                return Err("Invalid banking snapshot ownership".into());
            }
            for (key, value) in fields {
                if ["date", "statement_date"].contains(&key.as_str())
                    && !value.as_str().is_some_and(|d| {
                        fb_db::banking_evidence::calendar_date(d)
                            && (key != "statement_date"
                                || ("1900-01-02"..="9999-12-30").contains(&d))
                    })
                {
                    return Err(format!("Invalid banking date {key}"));
                }
                if ["request_key", "completion_key", "undo_key"].contains(&key.as_str())
                    && !value.is_null()
                    && !value
                        .as_str()
                        .is_some_and(fb_db::banking_evidence::canonical_uuid)
                {
                    return Err(format!("Invalid banking UUID {key}"));
                }
                if [
                    "count",
                    "cleared_count",
                    "n_debit",
                    "n_credit",
                    "total",
                    "hidden_cleared",
                    "more_splits",
                    "revision",
                    "page",
                    "cap",
                ]
                .contains(&key.as_str())
                    && value.is_number()
                    && value.as_i64().is_none_or(|n| {
                        n < 0 || (["page", "cap"].contains(&key.as_str()) && n == 0)
                    })
                {
                    return Err(format!("Invalid banking count {key}"));
                }
                validate_values(value)?;
            }
        }
        Value::Array(rows) => {
            for row in rows {
                validate_values(row)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn validate_reports(v: &Value) -> Result<(), std::string::String> {
    let money = |v: &Value| {
        fb_db::banking_evidence::money(v, false).ok_or_else(|| "Invalid report money".to_owned())
    };
    let sum = |a: i64, b: i64| {
        a.checked_add(b)
            .ok_or_else(|| "Report sum overflow".to_owned())
    };
    for report in v["reports"].as_object().ok_or("Invalid reports")?.values() {
        let sides = report["sides"].as_array().ok_or("Invalid report sides")?;
        if sides.len() != 2 || sides[0]["key"] != "left" || sides[1]["key"] != "right" {
            return Err("Report must contain exactly left and right sides".into());
        }
        let mut seen = BTreeSet::new();
        for side in sides {
            let key = side["key"].as_str().ok_or("Invalid report side key")?;
            let mut types = BTreeSet::new();
            let mut count = 0;
            let mut total = 0i64;
            for group in side["groups"].as_array().ok_or("Invalid report groups")? {
                let rows = group["rows"].as_array().ok_or("Invalid report rows")?;
                if rows.is_empty()
                    || !types.insert(group["type"].as_str().ok_or("Invalid group type")?)
                {
                    return Err("Incomplete or duplicate report group".into());
                }
                let mut subtotal = 0i64;
                for row in rows {
                    let other = if key == "left" { "right" } else { "left" };
                    if row[key].is_null()
                        || !row[other].is_null()
                        || !seen.insert(row["line_id"].as_str().ok_or("Invalid report line")?)
                        || row["type"] != group["type"]
                    {
                        return Err("Invalid report side membership".into());
                    }
                    let amount = money(&row[key])?;
                    if amount <= 0 {
                        return Err("Invalid report row amount".into());
                    }
                    subtotal = sum(subtotal, amount)?;
                }
                if money(&group["total"])? != subtotal {
                    return Err("Invalid report group total".into());
                }
                count += rows.len();
                total = sum(total, subtotal)?;
            }
            let negative = (key == "left")
                != report["words"]["card"]
                    .as_bool()
                    .ok_or("Invalid report card")?;
            let signed = if negative {
                total.checked_neg().ok_or("Report sign overflow")?
            } else {
                total
            };
            if side["count"].as_u64() != Some(count as u64)
                || money(&side["total"])? != total
                || money(&side["signed"])? != signed
            {
                return Err("Incomplete report side totals".into());
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn identical_malformed_banking_dumps_never_match() {
        let p = std::env::temp_dir().join(format!("banking-shape-{}.fbbook", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let scenario = json!({"users":[],"steps":[]});
        let mut runner = crate::Runner::new(&scenario, &p);
        let baseline = runner.dump();
        for field in [
            "deposits",
            "deposit_lines",
            "reversals",
            "reconciliations",
            "statuses",
            "reconciliation_lines",
            "registers",
            "screens",
            "reports",
        ] {
            for bad in [Value::Null, json!(true), json!(42), json!("bad")] {
                let mut d = baseline.clone();
                d["banking"][field] = bad;
                assert!(!crate::compare(&d, &d).ok(), "{field}");
            }
        }
    }
    #[test]
    fn real_banking_dump_rejects_identical_duplicate_missing_and_malformed_evidence() {
        let scenario: Value =
            serde_json::from_str(include_str!("../scenarios/banking-reconcile.json")).unwrap();
        let p =
            std::env::temp_dir().join(format!("banking-shape-real-{}.fbbook", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut runner = crate::Runner::new(&scenario, &p);
        runner.run();
        let baseline = runner.dump();
        assert!(crate::compare(&baseline, &baseline).ok());
        for field in ["statuses", "reconciliations", "reconciliation_lines"] {
            let mut bad = baseline.clone();
            let row = bad["banking"][field][0].clone();
            bad["banking"][field].as_array_mut().unwrap().push(row);
            assert!(!crate::compare(&bad, &bad).ok(), "{field}");
        }
        for field in ["reports", "screens", "registers"] {
            let mut bad = baseline.clone();
            bad["banking"][field] = json!({});
            assert!(!crate::compare(&bad, &bad).ok(), "{field}");
        }
        for pointer in [
            "/banking/reconciliations/0/ending_balance",
            "/banking/statuses/0/state",
            "/banking/statuses/0/revision",
        ] {
            let mut bad = baseline.clone();
            *bad.pointer_mut(pointer).unwrap() = json!("bad");
            assert!(!crate::compare(&bad, &bad).ok(), "{pointer}");
        }
        for (pointer, value) in [
            (
                "/banking/reconciliations/0/statement_date".to_owned(),
                json!("2026-99-99"),
            ),
            (
                "/banking/reconciliations/0/completion_key".to_owned(),
                json!("z".repeat(36)),
            ),
            (
                format!(
                    "/banking/reports/{}/sides",
                    baseline["banking"]["reports"]
                        .as_object()
                        .unwrap()
                        .keys()
                        .next()
                        .unwrap()
                ),
                json!([]),
            ),
            (
                format!(
                    "/banking/registers/{}/closing",
                    baseline["banking"]["registers"]
                        .as_object()
                        .unwrap()
                        .keys()
                        .next()
                        .unwrap()
                ),
                json!("99999999999999999.99"),
            ),
            (
                format!(
                    "/banking/registers/{}/rows/0/date",
                    baseline["banking"]["registers"]
                        .as_object()
                        .unwrap()
                        .keys()
                        .next()
                        .unwrap()
                ),
                json!("2026-99-99"),
            ),
        ] {
            let mut bad = baseline.clone();
            *bad.pointer_mut(&pointer).unwrap() = value;
            assert!(!crate::compare(&bad, &bad).ok(), "{pointer}");
        }
        let register = baseline["banking"]["registers"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap();
        let report = baseline["banking"]["reports"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap();
        for (pointer, value) in [
            (
                format!("/banking/registers/{register}/rows/0/debit"),
                json!("-1.00"),
            ),
            (
                format!("/banking/registers/{register}/rows/0/state"),
                json!("invalid"),
            ),
            (
                format!("/banking/reports/{report}/sides/0/count"),
                json!(99),
            ),
        ] {
            let mut bad = baseline.clone();
            *bad.pointer_mut(&pointer).unwrap() = value;
            assert!(!crate::compare(&bad, &bad).ok(), "{pointer}");
        }
        let mut changed = baseline.clone();
        changed["banking"]["reconciliations"][0]["ending_balance"] = json!("5933.43");
        assert!(!crate::compare(&baseline, &changed).ok());
    }

    fn fix2_baseline(name: &str) -> Value {
        let scenario: Value =
            serde_json::from_str(include_str!("../scenarios/banking-reconcile.json")).unwrap();
        let p =
            std::env::temp_dir().join(format!("banking-fix2-{name}-{}.fbbook", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut runner = crate::Runner::new(&scenario, &p);
        runner.run();
        let d = runner.dump();
        assert!(crate::compare(&d, &d).ok());
        d
    }
    fn fix2_reject(d: &Value) {
        assert!(
            !crate::compare(d, d).ok(),
            "Identical corruption must fail independently"
        );
    }
    #[test]
    fn fix2_missing_all_reconciliation_lines() {
        let mut d = fix2_baseline("missing-lines");
        d["banking"]["reconciliation_lines"] = json!([]);
        fix2_reject(&d);
    }
    #[test]
    fn fix2_missing_all_report_rows_with_consistent_totals() {
        let mut d = fix2_baseline("missing-rows");
        for report in d["banking"]["reports"]
            .as_object_mut()
            .unwrap()
            .values_mut()
        {
            for side in report["sides"].as_array_mut().unwrap() {
                side["groups"] = json!([]);
                side["count"] = json!(0);
                for key in ["total", "signed"] {
                    side[key] = json!("0.00");
                }
            }
        }
        fix2_reject(&d);
    }
    #[test]
    fn fix2_nonexistent_report_line() {
        let mut d = fix2_baseline("wrong-line");
        let report = d["banking"]["reports"]
            .as_object_mut()
            .unwrap()
            .values_mut()
            .find(|r| r["sides"][0]["count"].as_u64().unwrap() > 0)
            .unwrap();
        report["sides"][0]["groups"][0]["rows"][0]["line_id"] = json!("nonexistent:999");
        fix2_reject(&d);
    }
    #[test]
    fn fix2_changed_reconciliation_amount() {
        let mut d = fix2_baseline("line-amount");
        d["banking"]["reconciliation_lines"][0]["debit"] = json!("1.00");
        fix2_reject(&d);
    }
    #[test]
    fn fix2_missing_lines_even_if_stored_totals_are_reset() {
        let mut d = fix2_baseline("audit-membership");
        d["banking"]["reconciliation_lines"] = json!([]);
        for rec in d["banking"]["reconciliations"].as_array_mut().unwrap() {
            rec["cleared_count"] = json!(0);
            rec["cleared_debit"] = json!("0.00");
            rec["cleared_credit"] = json!("0.00");
        }
        fix2_reject(&d);
    }
}
