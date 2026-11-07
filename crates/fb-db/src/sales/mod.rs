//! Sales-owned exact guard callbacks and open-time row verification.
use fb_money::Money;
use rusqlite::{self, Connection, functions::FunctionFlags};
use serde_json::{Value, json};
mod calculations;
mod snapshots;
fn text(v: &Value, max: usize, required: bool) -> FbResult<String> {
    v.as_str()
        .map(py::py_strip)
        .filter(|s| py::py_len(s) <= max && (!required || !s.is_empty()))
        .map(str::to_owned)
        .ok_or(Invalid)
}
fn field_text(v: &Value, k: &str, max: usize, required: bool) -> FbResult<String> {
    text(v.get(k).unwrap_or(&json!("")), max, required)
}
fn cents(v: &Value) -> FbResult<i64> {
    Money::from_decimal_exact(calculations::unit_rate(v)?)
        .map(|m| m.cents() as i64)
        .ok_or(Invalid)
}
fn calculate(lines: &[Value], tax: &Value, posting: bool) -> FbResult<Value> {
    calculations::totals(
        lines,
        tax["tax_mode"].as_str().unwrap_or(""),
        tax.get("tax_rate").filter(|v| !v.is_null()),
        tax.get("tax_label").unwrap_or(&json!("")),
        tax.get("tax_snapshot").filter(|v| !v.is_null()),
        posting,
    )
}
mod py {
    pub fn py_strip(s: &str) -> &str {
        s.trim_matches(fb_money::py_isspace)
    }
    pub fn py_len(s: &str) -> usize {
        s.chars().count()
    }
}
struct Invalid;
type FbResult<T> = Result<T, Invalid>;
struct FbError;
impl FbError {
    fn validation(_message: impl Into<String>) -> Invalid {
        Invalid
    }
}
fn document_ok(header: &Value, ls: &[Value], total: i64) -> bool {
    let Ok(calc) = calculate(ls, header, header["state"] != "DRAFT") else {
        return false;
    };
    (1..=50).contains(&ls.len())
        && ls.iter().all(snapshots::line)
        && snapshots::tax(header)
        && [
            "subtotal",
            "taxable",
            "tax",
            "total",
            "tax_mode",
            "tax_rate",
            "tax_label",
            "tax_snapshot",
        ]
        .iter()
        .all(|k| calc[*k] == header[*k])
        && ls.iter().enumerate().all(|(i, l)| {
            l["position"].as_u64() == Some(i as u64 + 1) && l["amount"] == calc["lines"][i]
        })
        && cents(&header["total"]).ok() == Some(total)
        && match header["tax_mode"].as_str() {
            Some("RATE" | "GROUP") => {
                header["tax_item_id"].is_string()
                    && header["tax_item_id"] == header["tax_snapshot"]["id"]
            }
            Some("NONE" | "MANUAL") => header["tax_item_id"].is_null(),
            _ => false,
        }
}
pub(super) fn register(conn: &Connection) -> rusqlite::Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8
        | FunctionFlags::SQLITE_DETERMINISTIC
        | FunctionFlags::SQLITE_INNOCUOUS;
    conn.create_scalar_function("fb_estimate_transition_ok", 4, flags, |ctx| {
        let old: String = ctx.get(0)?;
        let state: String = ctx.get(1)?;
        let action: String = ctx.get(2)?;
        let reason: String = ctx.get(3)?;
        let transition = matches!(
            (old.as_str(), state.as_str(), action.as_str()),
            ("", "DRAFT", "created")
                | ("DRAFT", "DRAFT", "revised")
                | ("DRAFT", "ACCEPTED", "accepted")
                | ("DRAFT", "DECLINED", "declined")
                | ("DRAFT" | "ACCEPTED" | "DECLINED", "CANCELLED", "cancelled")
                | ("ACCEPTED" | "DECLINED", "DRAFT", "reopened")
        );
        Ok(transition && text(&json!(reason), 500, state == "CANCELLED").is_ok())
    })?;
    conn.create_scalar_function("fb_sales_money", 1, flags, |ctx| {
        let s: String = ctx.get(0)?;
        Ok(crate::json_evidence::parse_unique_json_as::<Value>(&s)
            .ok()
            .and_then(|v| cents(&v).ok()))
    })?;
    conn.create_scalar_function("fb_sales_header_ok", 2, flags, |ctx| {
        let kind: String = ctx.get(0)?;
        let raw: String = ctx.get(1)?;
        let ok = (|| {
            let v: Value = crate::json_evidence::parse_unique_json_as(&raw).ok()?;
            let valid = field_text(&v, "actor_name", 150, true).is_ok()
                && field_text(&v, "memo", 2000, false).is_ok();
            Some(
                valid
                    && snapshots::header(&v)
                    && if kind == "invoice" {
                        snapshots::tax(&v)
                            && field_text(&v, "reason", 500, false).is_ok()
                            && field_text(&v, "action", 20, true).is_ok()
                    } else {
                        v["method"].as_str().is_some_and(|s| {
                            ["CASH", "CHECK", "CARD", "TRANSFER", "OTHER"].contains(&s)
                        }) && field_text(&v, "reference", 120, false).is_ok()
                            && v["creation_digest"].as_str().is_some_and(|s| s.len() == 64)
                            && v["review_digest"].as_str().is_some_and(|s| s.len() == 64)
                    },
            )
        })();
        Ok(ok.unwrap_or(false))
    })?;
    conn.create_scalar_function("fb_sales_line_ok", 1, flags, |ctx| {
        let s: String = ctx.get(0)?;
        let out = (|| {
            let l: Value = crate::json_evidence::parse_unique_json_as(&s).ok()?;
            let amount = calculations::line_amount(&l["quantity"], &l["rate"]).ok()?;
            Some(
                snapshots::line(&l)
                    && l["quantity"].is_string()
                    && l["rate"].is_string()
                    && l["amount"].is_string()
                    && text(&l["description"], 1000, true).is_ok()
                    && l["taxable"].is_boolean()
                    && cents(&l["amount"]).ok() == amount.to_db(),
            )
        })();
        Ok(out.unwrap_or(false))
    })?;
    conn.create_scalar_function("fb_sales_snapshot_ok", 2, flags, |ctx| {
        let kind: String = ctx.get(0)?;
        let raw: String = ctx.get(1)?;
        Ok(crate::json_evidence::parse_unique_json_as::<Value>(&raw)
            .ok()
            .is_some_and(|v| match kind.as_str() {
                "line" => snapshots::line(&v),
                "detail" => snapshots::tax(&v),
                _ => false,
            }))
    })?;
    conn.create_scalar_function("fb_sales_reversal_ok", 1, flags, |ctx| {
        let raw: String = ctx.get(0)?;
        Ok(crate::json_evidence::parse_unique_json_as::<Value>(&raw)
            .ok()
            .is_some_and(|v| snapshots::reversal(&v)))
    })?;
    conn.create_scalar_function("fb_sales_document_ok", 3, flags, |ctx| {
        let h: String = ctx.get(0)?;
        let ls: String = ctx.get(1)?;
        let n: i64 = ctx.get(2)?;
        Ok(
            match (
                crate::json_evidence::parse_unique_json_as::<Value>(&h),
                crate::json_evidence::parse_unique_json_as::<Vec<Value>>(&ls),
            ) {
                (Ok(h), Ok(ls)) => document_ok(&h, &ls, n),
                _ => false,
            },
        )
    })?;
    Ok(())
}
pub(super) fn check(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut problems = Vec::new();
    let unsealed:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sales_invoicerevision r WHERE NOT EXISTS(SELECT 1 FROM sales_seal s WHERE s.kind='sales_invoice_revision' AND s.doc_id=r.id)) OR EXISTS(SELECT 1 FROM sales_receipt r WHERE NOT EXISTS(SELECT 1 FROM sales_seal s WHERE s.kind='sales_receipt' AND s.doc_id=r.id)) OR EXISTS(SELECT 1 FROM sales_receiptreversal r WHERE NOT EXISTS(SELECT 1 FROM sales_seal s WHERE s.kind='sales_receipt_reversal' AND s.doc_id=r.id))",[],|r|r.get(0))?;
    if unsealed {
        problems.push("unsealed sales evidence".into())
    }
    for (table, parent, child, fk) in [
        (
            "sales_invoicerevision",
            "invoice_id",
            "sales_invoiceline",
            "revision_id",
        ),
        (
            "sales_salesreceipt",
            "receipt_id",
            "sales_salesreceiptline",
            "sale_id",
        ),
    ] {
        let mut stmt=conn.prepare(&format!("SELECT r.id,r.data,r.total,coalesce((SELECT json_group_array(json(l.data)) FROM (SELECT data FROM {child} WHERE {fk}=r.id ORDER BY position) l),'[]') FROM {table} r ORDER BY r.{parent}"))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for (id, h, total, ls) in rows {
            if !document_ok(
                &crate::json_evidence::parse_unique_json_as::<Value>(&h).unwrap_or(Value::Null),
                &crate::json_evidence::parse_unique_json_as::<Vec<Value>>(&ls).unwrap_or_default(),
                total,
            ) {
                problems.push(format!("inconsistent sales totals {id}"))
            }
        }
    }
    let mismatched:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sales_journal_mismatch) OR EXISTS(SELECT 1 FROM sales_invoice i WHERE NOT EXISTS(SELECT 1 FROM sales_invoicerevision r WHERE r.invoice_id=i.id AND r.number=i.revision)) OR EXISTS(SELECT 1 FROM sales_receiptallocation a JOIN sales_invoicerevision v ON v.id=a.invoice_revision_id WHERE a.amount>v.total)",[],|r|r.get(0))?;
    if mismatched {
        problems.push("inconsistent sales ownership/posting".into())
    }
    let mut stmt = conn.prepare("SELECT problem FROM sales_snapshot_violations UNION ALL SELECT 'invalid sales origin: '||id FROM sales_origin_violations")?;
    problems.extend(
        stmt.query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?,
    );
    let mut stmt = conn.prepare(include_str!("integrity.sql"))?;
    problems.extend(
        stmt.query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?,
    );
    let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
    for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
        let table = row?;
        if table.starts_with("sales_") {
            problems.push(format!("missing sales parent in {table}"));
        }
    }
    Ok(problems)
}
