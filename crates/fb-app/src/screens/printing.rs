//! Print presentation uses immutable document snapshots and the existing report queries.
use super::d2::token;
use super::d6::{ctx, s, scoped_rows};
use super::*;

pub(super) fn is_document(name: &str) -> bool {
    matches!(
        name,
        "sales:print"
            | "sales:print_revision"
            | "invoices:print"
            | "invoices:print_revision"
            | "sales_receipts:print"
            | "prepayments:print"
    )
}

/// Only generated print chrome gets the platform capability; all document markup stays frozen.
pub(super) fn draw(
    app: &mut App,
    template: &str,
    vm: &mut Value,
    _target: &str,
    audience: &str,
) -> AppResult<(String, u16)> {
    vm["note"] = json!(fb_templates::django::print_toolbar_note(audience, false));
    vm["stamp"] = json!(if audience == "internal" {
        "Alpha: keep your real books in your current software."
    } else {
        ""
    });
    Ok((app.screens.render(template, vm)?, 200))
}

pub(super) fn capability(app: &mut App, html: String, target: &str) -> String {
    let button = "<button type=\"button\" id=\"print-now\">Print / Save as PDF</button>";
    let replacement = format!(
        "<button type=\"button\" id=\"print-now\">Print / Save as PDF{}</button>",
        token(app, target)
    );
    html.replacen(button, &replacement, 1)
}

pub(super) fn submit(app: &mut App, target: &str, fields: Posted<'_>) -> AppResult<Outcome> {
    if fields != [("action".into(), "desktop_print".into())] {
        return Err(AppError::Invalid("Invalid print action.".into()));
    }
    // A capability authorizes exactly the displayed URL, including all report filters and revision.
    let rendered = super::render(app, target)?;
    if rendered.status != 200 || !rendered.html.contains("id=\"print-now\"") {
        return Err(AppError::Invalid("Open a print view first.".into()));
    }
    Ok(Outcome::Done {
        data: json!({"desktop_print":true,"target":target,"print_token":app.tokens.mint(target)}),
    })
}

fn raw(
    app: &mut App,
    c: CompanyId,
    sql: &str,
    args: &[&dyn fb_db::rusqlite::ToSql],
) -> AppResult<Value> {
    let row = scoped_rows(app, c, sql, args)?
        .pop()
        .ok_or_else(|| AppError::NotFound("Document".into()))?;
    let mut parsed: Value = fb_db::parse_unique_json_as(s(&row, "data"))
        .map_err(|e| AppError::Invalid(e.to_string()))?;
    if parsed["id"].is_null() && !row["id"].is_null() {
        parsed["id"] = row["id"].clone();
    }
    Ok(parsed)
}
fn lines(app: &mut App, c: CompanyId, table: &str, field: &str, id: &str) -> AppResult<Vec<Value>> {
    scoped_rows(
        app,
        c,
        &format!("SELECT data FROM {table} WHERE {field}=? ORDER BY position"),
        &[&id],
    )?
    .into_iter()
    .map(|v| {
        fb_db::parse_unique_json_as(s(&v, "data")).map_err(|e| AppError::Invalid(e.to_string()))
    })
    .collect()
}

pub(super) fn document(app: &mut App, r: &Route, target: &str) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let mut vm = ctx(app, r)?;
    if r.name == "prepayments:print" {
        vm["deposit"] = super::d5::records::source(app, c, &r.args["customer_deposit_id"], true)?;
        vm["printed_at"] = json!((app.now)().to_rfc3339());
        return draw(app, "prepayments/print.html", &mut vm, target, "customer");
    }
    let (
        kind,
        title,
        number_label,
        root,
        current,
        mut line_rows,
        state,
        mark,
        revision_number,
        revision_total,
    ) = if r.name.starts_with("sales:") || r.name.starts_with("invoices:") {
        let estimate = r.name.starts_with("sales:");
        let (model, key) = if estimate {
            ("estimate", "estimate_id")
        } else {
            ("invoice", "invoice_id")
        };
        let id = &r.args[key];
        let root = {
            let open = app.open_mut()?;
            let actor = open.owner;
            open.book.read(|tx| {
                fb_core::services::access(tx, actor, c)?;
                if estimate {
                    fb_core::sales::estimate(tx, c, id)
                } else {
                    fb_core::sales::invoice(tx, c, id)
                }
            })?
        };
        let n = match r.args.get("number") {
            Some(n) => n
                .parse::<i64>()
                .map_err(|_| AppError::NotFound("Revision".into()))?,
            None => root["revision"].as_i64().unwrap(),
        };
        let current = raw(
            app,
            c,
            &format!("SELECT data FROM sales_{model}revision WHERE {model}_id=? AND number=?"),
            &[&id, &n],
        )?;
        let rows = lines(
            app,
            c,
            &format!("sales_{model}line"),
            "revision_id",
            s(&current, "id"),
        )?;
        let state = match s(&current, "state") {
            "DRAFT" => "Draft",
            "OPEN" => "Open",
            "VOID" => "Voided",
            "ACCEPTED" => "Accepted",
            "DECLINED" => "Declined",
            "CANCELLED" => "Cancelled",
            _ => "",
        };
        let mark = match s(&current, "state") {
            "DRAFT" => "DRAFT",
            "VOID" => "VOID",
            "DECLINED" => "DECLINED",
            "CANCELLED" => "CANCELLED",
            _ => "",
        };
        let total = root["revision"].as_i64().unwrap();
        (
            if estimate { "estimate" } else { "invoice" },
            if estimate { "ESTIMATE" } else { "INVOICE" },
            if estimate { "Estimate #" } else { "Invoice #" },
            root,
            current,
            rows,
            state,
            mark,
            n,
            total,
        )
    } else {
        let id = &r.args["sales_receipt_id"];
        let mut root = raw(
            app,
            c,
            "SELECT data FROM sales_receipt WHERE company_id=? AND id=? AND kind='SR'",
            &[&c.to_db(), &id],
        )?;
        let reversal = !scoped_rows(
            app,
            c,
            "SELECT id FROM sales_receiptreversal WHERE receipt_id=?",
            &[&id],
        )?
        .is_empty();
        root["reversed"] = json!(reversal);
        let current = raw(
            app,
            c,
            "SELECT id,data FROM sales_salesreceipt WHERE receipt_id=?",
            &[&id],
        )?;
        let rows = lines(
            app,
            c,
            "sales_salesreceiptline",
            "sale_id",
            s(&current, "id"),
        )?;
        (
            "sales_receipt",
            "SALES RECEIPT",
            "Sales receipt #",
            root,
            current,
            rows,
            if reversal { "Voided" } else { "Recorded" },
            if reversal { "VOID" } else { "" },
            1,
            1,
        )
    };
    let mut current = current;
    super::d4::fixed_document(&mut current, &mut line_rows);
    let identity = super::d5::records::identity(app, c, s(&root, "identity_id"))?;
    let superseded = revision_number != revision_total;
    let snapshot = if kind == "sales_receipt" {
        &root
    } else {
        &current
    };
    let mut doc = json!({"kind":kind,"title":title,"number":identity["number"],"number_label":number_label,
        "revision_number":revision_number,"revision_total":revision_total,"superseded":superseded,"state_label":state,"mark":mark,
        "company":snapshot["company_snapshot"],"party":snapshot["customer_snapshot"],"currency":snapshot["company_snapshot"]["currency"],
        "memo":snapshot["memo"],"date":snapshot["date"],"lines":line_rows,"printed_at":(app.now)().to_rfc3339(),
        "terms":"","second_date_label":"","second_date":null,"balance":null,"posting_note":""});
    let o = app.open_mut()?;
    doc["business"] = serde_json::to_value(o.book.read(|tx| fb_core::company_documents::get(tx,c))?).expect("details");
    if o.book.read(|tx| fb_core::company_documents::logo(tx,c))?.is_some() {
        doc["company_logo_url"] = json!(format!("/c/{c}/document-logo/"));
    }
    if kind == "estimate" {
        doc["second_date_label"] = json!("Expiry");
        doc["second_date"] = current["expiry"].clone();
        doc["posting_note"] = json!("Estimate — not an invoice, not an amount owed.");
    } else if kind == "invoice" {
        doc["second_date_label"] = json!("Due date");
        doc["second_date"] = current["due_date"].clone();
        if let (Ok(start), Ok(end)) = (
            chrono::NaiveDate::parse_from_str(s(&current, "date"), "%Y-%m-%d"),
            chrono::NaiveDate::parse_from_str(s(&current, "due_date"), "%Y-%m-%d"),
        ) {
            let days = (end - start).num_days();
            doc["terms"] = json!(if days <= 0 {
                "Due on receipt".into()
            } else {
                format!("Net {days}")
            });
        }
        if state == "Open" && !superseded {
            let o = app.open_mut()?;
            let live = fb_core::sales::invoice_balances(&mut o.book, o.owner, c)?
                .into_iter()
                .find(|v| v["invoice_id"] == root["id"])
                .ok_or_else(|| AppError::NotFound("Invoice".into()))?;
            let live_revision = scoped_rows(
                app,
                c,
                "SELECT revision FROM sales_invoice WHERE company_id=? AND id=?",
                &[&c.to_db(), &s(&root, "id")],
            )?
            .pop()
            .ok_or_else(|| AppError::NotFound("Invoice".into()))?["revision"]
                .as_i64()
                .unwrap();
            if live_revision == revision_number && live["state"] == "OPEN" {
                doc["balance"] = json!({"as_of":(app.now)().date_naive().to_string(),"paid":live["settled"],"due":live["outstanding"]});
                // SIM-FIX item 2: posting state plus the live payment state, as on the invoice page.
                if let Some(paid) = live["payment_state"].as_str().and_then(payment_word) {
                    doc["state_label"] = json!(format!("Posted · {paid}"));
                }
            } else {
                doc["superseded"] = json!(true);
                doc["revision_total"] = json!(live_revision.max(revision_number));
            }
        }
        doc["posting_note"] = json!(if state == "Voided" {
            "Voided — the original posting is cancelled by its linked reversal. No amount due."
        } else if state == "Draft" {
            "Draft — not posted. No amount due."
        } else if doc["superseded"] == true {
            "Earlier version — kept for the record. See the current version for any balance."
        } else {
            ""
        });
    } else if kind == "sales_receipt" {
        doc["posting_note"] = json!(if state == "Voided" {
            "Voided — the original posting is cancelled by its linked reversal.".into()
        } else {
            format!(
                "Paid in full on {} by {}. No amount due.",
                s(&root, "date"),
                super::d5::records::method(s(&root, "method"), false).to_lowercase()
            )
        });
    }
    vm["root"] = json!({"pk":root["id"]});
    vm["current"] = current;
    vm["doc"] = doc;
    draw(
        app,
        "sales/print/document.html",
        &mut vm,
        target,
        "customer",
    )
}

/// `invoice_balances` payment state as the invoice page titles it.
fn payment_word(state: &str) -> Option<&'static str> {
    match state {
        "OPEN" => Some("Open"),
        "PARTIAL" => Some("Partial"),
        "PAID" => Some("Paid"),
        _ => None,
    }
}
