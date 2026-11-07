//! Required sales fields and types are checked before comparison or normalization.
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
const LINE: &[(&str, Shape)] = &[
    ("position", Int),
    ("item_id", Nullable(&String)),
    ("item_snapshot", ITEM),
    ("description", String),
    ("quantity", String),
    ("rate", String),
    ("taxable", Bool),
    ("amount", String),
    ("income_account_id", Nullable(&String)),
    ("account_snapshot", Map(&ACCOUNT)),
    ("expense_account_id", Optional(&Nullable(&String))),
];
const HEADER: Shape = Object(&[
    ("number", Int),
    ("state", String),
    ("action", String),
    ("reason", String),
    ("actor_name", String),
    ("customer_id", String),
    ("customer_snapshot", PARTY),
    ("company_snapshot", COMPANY),
    ("date", String),
    ("due_date", String),
    ("memo", String),
    ("tax_mode", String),
    ("tax_item_id", Nullable(&String)),
    ("tax_snapshot", Snapshot),
    ("tax_label", String),
    ("tax_rate", String),
    ("subtotal", String),
    ("taxable", String),
    ("tax", String),
    ("total", String),
    ("journal_id", Nullable(&String)),
    ("reversal_id", Nullable(&String)),
    ("lines", Array(&Object(LINE))),
]);
const DETAIL: Shape = Object(&[
    ("collected_tax_id", Nullable(&String)),
    ("tax_item_id", Nullable(&String)),
    ("tax_mode", String),
    ("tax_rate", String),
    ("tax_label", String),
    ("tax_snapshot", Snapshot),
    ("subtotal", String),
    ("taxable", String),
    ("tax", String),
    ("total", String),
]);
const RECEIPT: Shape = Object(&[
    ("ref", String),
    ("kind", String),
    ("actor_name", String),
    ("customer_id", String),
    ("customer_snapshot", PARTY),
    ("company_snapshot", COMPANY),
    ("date", String),
    ("amount", String),
    ("method", String),
    ("reference", String),
    ("memo", String),
    ("destination", String),
    ("destination_account_id", String),
    ("journal_id", String),
    ("sale", Nullable(&DETAIL)),
    ("lines", Array(&Object(LINE))),
    (
        "allocations",
        Array(&Object(&[
            ("invoice_id", String),
            ("revision", Int),
            ("receivables_id", String),
            ("amount", String),
        ])),
    ),
    (
        "reversal",
        Nullable(&Object(&[
            ("date", String),
            ("reason", String),
            ("actor_name", String),
            ("journal_id", String),
        ])),
    ),
]);
const ESTIMATE_HEADER: Shape = Object(&[
    ("number", Int),
    ("state", String),
    ("action", String),
    ("reason", String),
    ("actor_name", String),
    ("customer_id", String),
    ("customer_snapshot", PARTY),
    ("company_snapshot", COMPANY),
    ("date", String),
    ("expiry", Nullable(&String)),
    ("memo", String),
    ("tax_mode", String),
    ("tax_item_id", Nullable(&String)),
    ("tax_snapshot", Snapshot),
    ("tax_label", String),
    ("tax_rate", String),
    ("subtotal", String),
    ("taxable", String),
    ("tax", String),
    ("total", String),
    ("lines", Array(&Object(LINE))),
]);
pub(crate) const SALES: Shape = Object(&[
    (
        "estimates",
        Array(&Object(&[
            ("ref", String),
            ("revision", Int),
            ("history", Array(&ESTIMATE_HEADER)),
            (
                "conversion",
                Nullable(&Object(&[("revision", Int), ("invoice", String)])),
            ),
        ])),
    ),
    (
        "invoices",
        Array(&Object(&[
            ("ref", String),
            ("revision", Int),
            ("history", Array(&HEADER)),
        ])),
    ),
    ("receipts", Array(&RECEIPT)),
    (
        "balances",
        Array(&Object(&[
            ("invoice_id", String),
            ("customer_id", String),
            ("date", String),
            ("due_date", String),
            ("state", String),
            ("total", String),
            ("allocated", String),
            ("credited", String),
            ("deposited", String),
            ("settled", String),
            ("outstanding", Nullable(&String)),
            ("payment_state", String),
        ])),
    ),
]);
pub(crate) fn unique(v: &Value) -> Result<(), std::string::String> {
    fn keys(rows: &Value, field: &str, path: &str) -> Result<(), std::string::String> {
        let mut seen = BTreeSet::new();
        for row in array(rows)? {
            if !seen.insert(row[field].to_string()) {
                return Err(format!("{path}: duplicate {field}"));
            }
        }
        Ok(())
    }
    for (name, key) in [
        ("invoices", "ref"),
        ("receipts", "ref"),
        ("balances", "invoice_id"),
    ] {
        keys(&v["sales"][name], key, &format!("/sales/{name}"))?;
    }
    let invoice_keys = key_set(&v["sales"]["invoices"], "ref")?;
    let balance_keys = key_set(&v["sales"]["balances"], "invoice_id")?;
    if invoice_keys != balance_keys {
        return Err("/sales/balances: keys must equal invoice refs".into());
    }
    for b in array(&v["sales"]["balances"])? {
        amounts(
            b,
            &["total", "allocated", "credited", "deposited", "settled"],
        )?;
        if !b["outstanding"].is_null() {
            money(&b["outstanding"])?;
        }
    }
    for inv in array(&v["sales"]["invoices"])? {
        keys(&inv["history"], "number", "sales history")?;
        for h in array(&inv["history"])? {
            totals(h)?;
            keys(&h["lines"], "position", "sales invoice lines")?;
            if let Some(rows) = h["tax_snapshot"].get("members") {
                keys(rows, "id", "invoice tax members")?;
            }
            for l in array(&h["lines"])? {
                line_money(l)?;
                if l.get("expense_account_id").is_none() {
                    return Err("invoice line requires expense_account_id".into());
                }
            }
        }
    }
    for r in array(&v["sales"]["receipts"])? {
        money(&r["amount"])?;
        for l in array(&r["lines"])? {
            line_money(l)?;
        }
        for a in array(&r["allocations"])? {
            money(&a["amount"])?;
        }
        if !r["sale"].is_null() {
            totals(&r["sale"])?;
        }
        keys(&r["lines"], "position", "sales receipt lines")?;
        keys(&r["allocations"], "invoice_id", "sales allocations")?;
        if let Some(rows) = r["sale"]["tax_snapshot"].get("members") {
            keys(rows, "id", "receipt tax members")?;
        }
    }
    Ok(())
}

fn array(v: &Value) -> Result<&[Value], std::string::String> {
    v.as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| "/sales: expected array".into())
}
fn key_set(v: &Value, key: &str) -> Result<BTreeSet<std::string::String>, std::string::String> {
    array(v)?
        .iter()
        .map(|r| {
            r[key]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("/sales: invalid {key}"))
        })
        .collect()
}

// Dumps contain canonical, nonnegative two-place Decimal strings, bounded by
// MAX_TOTAL_CENTS. Do not use the permissive, rounding input-money parser here.
fn money(v: &Value) -> Result<(), std::string::String> {
    let valid = v
        .as_str()
        .and_then(|s| s.split_once('.'))
        .is_some_and(|(whole, fraction)| {
            !whole.is_empty()
                && whole.len() <= 14
                && (whole.len() == 1 || !whole.starts_with('0'))
                && whole.bytes().all(|b| b.is_ascii_digit())
                && fraction.len() == 2
                && fraction.bytes().all(|b| b.is_ascii_digit())
        });
    if valid {
        Ok(())
    } else {
        Err(format!("/sales: invalid bounded money string {v}"))
    }
}
fn amounts(v: &Value, fields: &[&str]) -> Result<(), std::string::String> {
    for field in fields {
        money(&v[*field])?;
    }
    Ok(())
}
fn totals(v: &Value) -> Result<(), std::string::String> {
    amounts(v, &["subtotal", "taxable", "tax", "total"])?;
    if let Some(members) = v["tax_snapshot"]["members"].as_array() {
        amounts(&v["tax_snapshot"], &["taxable", "tax"])?;
        for member in members {
            money(&member["tax"])?;
        }
    }
    Ok(())
}
fn line_money(v: &Value) -> Result<(), std::string::String> {
    amounts(v, &["rate", "amount"])?;
    if let Some(rate) = v["item_snapshot"].get("sale_rate").filter(|r| !r.is_null()) {
        money(rate)?;
    }
    Ok(())
}
