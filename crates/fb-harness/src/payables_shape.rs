//! Exact payables dump contract, validated before comparison or normalization.
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
const ITEM: Shape = Object(&[
    ("id", Optional(&String)),
    ("code", Optional(&String)),
    ("description", Optional(&String)),
    ("kind", Optional(&String)),
    ("purchase_cost", Optional(&Nullable(&String))),
]);
const ACCOUNTS: Shape = Object(&[
    ("income_account", Optional(&ACCOUNT)),
    ("expense_account", Optional(&ACCOUNT)),
]);
const BILL: Shape = Object(&[
    ("id", String),
    ("company_id", String),
    ("identity_id", String),
    ("revision", Int),
    ("vendor_id", String),
    ("reference_key", String),
    ("state", String),
]);
const REVISION: Shape = Object(&[
    ("id", String),
    ("bill_id", String),
    ("number", Int),
    ("state", String),
    ("action", String),
    ("reason", String),
    ("actor_id", String),
    ("actor_name", String),
    ("vendor_id", String),
    ("vendor_snapshot", PARTY),
    ("company_snapshot", COMPANY),
    ("date", String),
    ("due_date", String),
    ("reference", String),
    ("supersedes_id", Nullable(&String)),
    ("reentry_reason", String),
    ("journal_id", Nullable(&String)),
    ("reversal_id", Nullable(&String)),
    ("memo", String),
    ("total", Int),
]);
const LINE: Shape = Object(&[
    ("id", String),
    ("revision_id", String),
    ("position", Int),
    ("item_id", Nullable(&String)),
    ("item_snapshot", ITEM),
    ("description", String),
    ("quantity", Int),
    ("rate", Int),
    ("amount", Int),
    ("income_account_id", Nullable(&String)),
    ("expense_account_id", Nullable(&String)),
    ("account_snapshot", ACCOUNTS),
]);
const PAYMENT: Shape = Object(&[
    ("id", String),
    ("company_id", String),
    ("identity_id", String),
    ("vendor_id", String),
    ("vendor_snapshot", PARTY),
    ("company_snapshot", COMPANY),
    ("date", String),
    ("amount", Int),
    ("method", String),
    ("reference", String),
    ("memo", String),
    ("bank_id", String),
    ("journal_id", String),
    ("actor_id", String),
    ("actor_name", String),
]);
const ALLOCATION: Shape = Object(&[
    ("id", String),
    ("payment_id", String),
    ("bill_id", String),
    ("bill_revision_id", String),
    ("payables_id", String),
    ("amount", Int),
]);
const REVERSAL: Shape = Object(&[
    ("id", String),
    ("payment_id", String),
    ("journal_id", String),
    ("date", String),
    ("reason", String),
    ("actor_id", String),
    ("actor_name", String),
]);
const BALANCE: Shape = Object(&[
    ("company_id", String),
    ("bill_id", String),
    ("payment_state", String),
    ("allocated", String),
    ("outstanding", Nullable(&String)),
    ("paid_on", Nullable(&String)),
]);
const TOTALS: Shape = Object(&[
    ("open_money", String),
    ("overdue_money", String),
    ("paid_recent_money", String),
    ("draft_count", Int),
    ("currency", String),
    ("today", String),
]);
const FILTERED: Shape = Object(&[
    ("name", String),
    ("bill_ids", Array(&String)),
    ("totals", TOTALS),
]);
const TRACKER: Shape = Object(&[
    ("company_id", String),
    ("totals", TOTALS),
    ("filtered", Array(&FILTERED)),
]);
const ENTRY: Shape = Object(&[
    ("bill_id", String),
    ("number", String),
    ("vendor_id", String),
    ("vendor_name", String),
    ("reference", String),
    ("due_date", String),
    ("total", String),
    ("remaining", String),
    ("payment_state", String),
    ("status", String),
]);
const DAY: Shape = Object(&[
    ("date", String),
    ("entries", Array(&ENTRY)),
    ("total", String),
]);
const CALENDAR: Shape = Object(&[
    ("company_id", String),
    (
        "data",
        Object(&[
            ("entries", Array(&ENTRY)),
            ("days", Array(&DAY)),
            (
                "weeks",
                Array(&Tuple(&[
                    Nullable(&DAY),
                    Nullable(&DAY),
                    Nullable(&DAY),
                    Nullable(&DAY),
                    Nullable(&DAY),
                    Nullable(&DAY),
                    Nullable(&DAY),
                ])),
            ),
            ("agenda", Array(&DAY)),
            ("day_totals", Map(&String)),
            ("month_total", String),
            ("month_label", String),
            (
                "weekdays",
                Tuple(&[String, String, String, String, String, String, String]),
            ),
        ]),
    ),
]);

pub(super) const SHAPE: Shape = Object(&[
    ("bill", Array(&BILL)),
    ("billrevision", Array(&REVISION)),
    ("billline", Array(&LINE)),
    ("billpayment", Array(&PAYMENT)),
    ("paymentallocation", Array(&ALLOCATION)),
    ("paymentreversal", Array(&REVERSAL)),
    ("balances", Array(&BALANCE)),
    ("tracker", Array(&TRACKER)),
    ("calendar", Array(&CALENDAR)),
]);

pub(super) fn validate(dump: &Value) -> Result<(), std::string::String> {
    let p = &dump["payables"];
    validate_amounts(p)?;
    for row in p["billline"].as_array().unwrap() {
        let snapshot = row["item_snapshot"].as_object().unwrap();
        if !snapshot.is_empty() && snapshot.len() != 5 {
            return Err("/payables/billline/item_snapshot: partial snapshot".into());
        }
    }
    for table in [
        "bill",
        "billrevision",
        "billline",
        "billpayment",
        "paymentallocation",
        "paymentreversal",
        "balances",
        "tracker",
        "calendar",
    ] {
        let key = match table {
            "balances" => "bill_id",
            "tracker" | "calendar" => "company_id",
            _ => "id",
        };
        let mut seen = BTreeSet::new();
        for row in p[table].as_array().unwrap() {
            if !seen.insert(row[key].as_str().unwrap()) {
                return Err(format!("/payables/{table}: duplicate {key}"));
            }
        }
    }
    let keys = |table: &str, key: &str| -> BTreeSet<std::string::String> {
        p[table]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r[key].as_str().unwrap().to_owned())
            .collect()
    };
    let companies: BTreeSet<_> = dump["companies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["ref"].as_str().unwrap().to_owned())
        .collect();
    for tracker in p["tracker"].as_array().unwrap() {
        let mut names = BTreeSet::new();
        for result in tracker["filtered"].as_array().unwrap() {
            let ids = result["bill_ids"].as_array().unwrap();
            let unique: BTreeSet<_> = ids.iter().map(|v| v.as_str().unwrap()).collect();
            if !names.insert(result["name"].as_str().unwrap())
                || unique.len() != ids.len()
                || ids
                    .windows(2)
                    .any(|pair| pair[0].as_str() > pair[1].as_str())
                || unique.iter().any(|id| {
                    !p["bill"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|b| b["id"] == *id && b["company_id"] == tracker["company_id"])
                })
            {
                return Err(
                    "/payables/tracker/filtered: duplicate or invalid result identity".into(),
                );
            }
        }
    }
    let reported = keys("tracker", "company_id");
    if reported != keys("calendar", "company_id")
        || reported != companies
        || !keys("bill", "company_id").is_subset(&reported)
    {
        return Err("/payables: tracker/calendar company membership differs".into());
    }
    if keys("bill", "id") != keys("balances", "bill_id") {
        return Err("/payables/balances: keys must equal bill ids".into());
    }
    for table in [
        "billrevision",
        "billline",
        "billpayment",
        "paymentallocation",
        "paymentreversal",
    ] {
        let fields: &[(&str, &str)] = match table {
            "billrevision" => &[("bill_id", "bill")],
            "billline" => &[("revision_id", "billrevision")],
            "paymentallocation" => &[
                ("payment_id", "billpayment"),
                ("bill_id", "bill"),
                ("bill_revision_id", "billrevision"),
            ],
            "paymentreversal" => &[("payment_id", "billpayment")],
            _ => &[],
        };
        for (key, parent) in fields {
            if !keys(table, key).is_subset(&keys(parent, "id")) {
                return Err(format!("/payables/{table}: missing {parent} parent"));
            }
        }
    }
    Ok(())
}

// Frozen Python documents.calculations and payables models store money in
// numeric(16,2), quantities in numeric(18,4). Payments/allocations are positive;
// bill totals/rates/amounts can be zero. Live allocations cannot overpay a bill.
const MAX_CENTS: i128 = 9_999_999_999_999_999;

fn bounded_integer(
    v: &Value,
    minimum: i128,
    maximum: i128,
    path: &str,
) -> Result<(), std::string::String> {
    if v.as_i64()
        .is_some_and(|n| (minimum..=maximum).contains(&i128::from(n)))
    {
        Ok(())
    } else {
        Err(format!("{path}: invalid bounded stored amount {v}"))
    }
}

// Validate the dump representation, never the permissive/rounding input parser.
fn money(v: &Value, maximum: i128, path: &str) -> Result<(), std::string::String> {
    let cents = v.as_str().and_then(|s| {
        let (whole, fraction) = s.split_once('.')?;
        if whole.is_empty()
            || (whole.len() > 1 && whole.starts_with('0'))
            || !whole.bytes().all(|b| b.is_ascii_digit())
            || fraction.len() != 2
            || !fraction.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        whole
            .parse::<i128>()
            .ok()?
            .checked_mul(100)?
            .checked_add(fraction.parse::<i128>().ok()?)
    });
    if cents.is_some_and(|n| (0..=maximum).contains(&n)) {
        Ok(())
    } else {
        Err(format!("{path}: invalid bounded money string {v}"))
    }
}

// TOTALS, ENTRY, DAY and CALENDAR repeat monetary fields in several views.
// Walk all of them after SHAPE has required their exact fields and types.
fn report_money(v: &Value, maximum: i128, path: &str) -> Result<(), std::string::String> {
    match v {
        Value::Array(rows) => {
            for (i, row) in rows.iter().enumerate() {
                report_money(row, maximum, &format!("{path}/{i}"))?;
            }
        }
        Value::Object(fields) => {
            for (key, value) in fields {
                let path = format!("{path}/{key}");
                match key.as_str() {
                    "allocated" | "outstanding" if !value.is_null() => {
                        money(value, MAX_CENTS, &path)?
                    }
                    "remaining" => money(value, MAX_CENTS, &path)?,
                    "total" if fields.contains_key("bill_id") => money(value, MAX_CENTS, &path)?,
                    "total" | "month_total" | "open_money" | "overdue_money"
                    | "paid_recent_money" => money(value, maximum, &path)?,
                    "day_totals" => {
                        for (date, total) in value.as_object().unwrap() {
                            money(total, maximum, &format!("{path}/{date}"))?;
                        }
                    }
                    _ => report_money(value, maximum, &path)?,
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_amounts(p: &Value) -> Result<(), std::string::String> {
    for (table, fields, minimum, maximum) in [
        ("billrevision", &["total"][..], 0, MAX_CENTS),
        ("billline", &["rate", "amount"][..], 0, MAX_CENTS),
        ("billline", &["quantity"][..], 1, 999_999_999_999_999_999),
        ("billpayment", &["amount"][..], 1, MAX_CENTS),
        ("paymentallocation", &["amount"][..], 1, MAX_CENTS),
    ] {
        for (i, row) in p[table].as_array().unwrap().iter().enumerate() {
            for field in fields {
                bounded_integer(
                    &row[*field],
                    minimum,
                    maximum,
                    &format!("/payables/{table}/{i}/{field}"),
                )?;
            }
        }
    }
    for (i, row) in p["billline"].as_array().unwrap().iter().enumerate() {
        if let Some(cost) = row["item_snapshot"]
            .get("purchase_cost")
            .filter(|v| !v.is_null())
        {
            money(
                cost,
                MAX_CENTS,
                &format!("/payables/billline/{i}/item_snapshot/purchase_cost"),
            )?;
        }
    }
    report_money(&p["balances"], MAX_CENTS, "/payables/balances")?;
    for table in ["tracker", "calendar"] {
        for (i, row) in p[table].as_array().unwrap().iter().enumerate() {
            // Python SUM/Decimal report totals are not limited to one document.
            // Each current bill contributes at most MAX_CENTS, including filtered
            // totals (state filters affect rows, not the totals' scoped bills).
            let count = p["bill"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|b| b["company_id"] == row["company_id"])
                .count();
            let maximum = (count as i128)
                .checked_mul(MAX_CENTS)
                .ok_or_else(|| "/payables: aggregate bound overflow".to_owned())?;
            report_money(row, maximum, &format!("/payables/{table}/{i}"))?;
            if table == "calendar" {
                calendar_keys(&row["data"])?;
            }
        }
    }
    Ok(())
}

fn calendar_keys(data: &Value) -> Result<(), std::string::String> {
    let dates: BTreeSet<_> = data["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["due_date"].as_str().unwrap())
        .collect();
    let totals: BTreeSet<_> = data["day_totals"]
        .as_object()
        .unwrap()
        .keys()
        .map(|s| s.as_str())
        .collect();
    if dates != totals {
        return Err("/payables/calendar/data/day_totals: keys must equal entry due dates".into());
    }
    let days = data["days"].as_array().unwrap();
    let day_dates: BTreeSet<_> = days
        .iter()
        .map(|day| day["date"].as_str().unwrap())
        .collect();
    let agenda: Vec<_> = days
        .iter()
        .filter(|day| !day["entries"].as_array().unwrap().is_empty())
        .collect();
    let weeks: Vec<_> = data["weeks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|week| week.as_array().unwrap())
        .filter(|day| !day.is_null())
        .collect();
    let entries: Vec<_> = days
        .iter()
        .flat_map(|day| day["entries"].as_array().unwrap())
        .collect();
    if day_dates.len() != days.len()
        || !dates.is_subset(&day_dates)
        || agenda
            != data["agenda"]
                .as_array()
                .unwrap()
                .iter()
                .collect::<Vec<_>>()
        || weeks != days.iter().collect::<Vec<_>>()
        || entries
            != data["entries"]
                .as_array()
                .unwrap()
                .iter()
                .collect::<Vec<_>>()
    {
        return Err("/payables/calendar: days, weeks, agenda and entries must be complete".into());
    }
    Ok(())
}
