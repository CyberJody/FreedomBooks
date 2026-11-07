//! D4: frozen sales views. The renderer owns presentation; typed commands own writes.
use super::d2::{
    clean_date, clean_integer, clean_text, decorate, draw, error, nonfield, put, token,
};
use super::*;
use crate::Command;
use fb_db::{
    Query,
    rusqlite::{OptionalExtension, params},
};
type Errors = HashMap<String, Vec<String>>;
mod choices;
pub(super) mod doc_forms;
mod estimates;
mod invoices;
pub(super) mod payments;
pub(super) use choices::customers as customer_choices;

pub(super) fn is_route(name: &str) -> bool {
    name.starts_with("receipts:")
        || name.starts_with("sales_receipts:")
        || name.starts_with("sales:")
        || (name.starts_with("invoices:") && name != "invoices:list")
}
pub(crate) fn is_d4_command(c: &Command) -> bool {
    matches!(
        c,
        Command::ReviewCustomerReceipt { .. }
            | Command::RecordCustomerReceipt { .. }
            | Command::ReverseCustomerReceipt { .. }
            | Command::SaveEstimate { .. }
            | Command::TransitionEstimate { .. }
            | Command::ConvertEstimate { .. }
            | Command::SaveInvoiceDraft { .. }
            | Command::PostReviewedInvoice { .. }
            | Command::VoidInvoice { .. }
    )
}
pub(crate) fn run_d4_command(app: &mut App, cmd: Command) -> AppResult<Outcome> {
    let review = if let Command::PostReviewedInvoice { review_token, .. } = &cmd {
        app.sales_reviews.remove(review_token)
    } else {
        None
    };
    if let Command::ReviewCustomerReceipt {
        company_id,
        sales_receipt,
        values,
    } = &cmd
    {
        let c = crate::session::company(company_id)?;
        let input = receipt_input(values, *sales_receipt);
        let o = app.open_mut()?;
        let (cap, vm) =
            fb_core::sales::review_receipt_form(&mut o.book, o.owner, c, &input, *sales_receipt)?;
        let key = invoices::remember(app, cap);
        return Ok(Outcome::Done {
            data: json!({"review":vm,"token":key}),
        });
    }
    let receipt_review = if let Command::RecordCustomerReceipt { review_token, .. } = &cmd {
        app.sales_reviews.remove(review_token)
    } else {
        None
    };
    let o = app.open_mut()?;
    let (book, actor) = (&mut o.book, o.owner);
    let data = match cmd {
        Command::RecordCustomerReceipt {
            company_id,
            sales_receipt,
            values,
            ..
        } => {
            let f = if sales_receipt {
                fb_core::sales::record_sales_receipt
            } else {
                fb_core::sales::record_receipt
            };
            f(
                book,
                actor,
                crate::session::company(&company_id)?,
                &receipt_input(&values, sales_receipt),
                receipt_review.as_ref(),
            )?
        }
        Command::ReverseCustomerReceipt {
            company_id,
            receipt_id,
            sales_receipt,
            expected_revision,
            date,
            reason,
        } => {
            let f = if sales_receipt {
                fb_core::sales::void_sales_receipt
            } else {
                fb_core::sales::reverse_receipt
            };
            f(
                book,
                actor,
                crate::session::company(&company_id)?,
                &receipt_id,
                Some(expected_revision),
                &json!(date),
                &json!(reason),
            )?
        }
        Command::SaveEstimate {
            company_id,
            estimate_id,
            expected_revision,
            expiry,
            values,
        } => {
            let mut input = serde_json::to_value(values).expect("typed estimate");
            for k in ["tax_rate", "tax_label", "tax_item_id"] {
                if input[k].is_null() {
                    input.as_object_mut().unwrap().remove(k);
                }
            }
            input.as_object_mut().unwrap().remove("due_date");
            input["expiry"] = json!(expiry);
            input["estimate_id"] = json!(estimate_id);
            input["expected_revision"] = json!(expected_revision);
            for line in input["lines"].as_array_mut().unwrap() {
                line.as_object_mut().unwrap().remove("income_account_id");
            }
            fb_core::sales::save_estimate(
                book,
                actor,
                crate::session::company(&company_id)?,
                &input,
            )?
        }
        Command::TransitionEstimate {
            company_id,
            estimate_id,
            expected_revision,
            state,
            reason,
        } => fb_core::sales::transition_estimate(
            book,
            actor,
            crate::session::company(&company_id)?,
            &estimate_id,
            &state,
            Some(expected_revision),
            &reason,
        )?,
        Command::ConvertEstimate {
            company_id,
            estimate_id,
            expected_revision,
            date,
            due_date,
        } => fb_core::sales::convert_estimate(
            book,
            actor,
            crate::session::company(&company_id)?,
            &estimate_id,
            Some(expected_revision),
            &date,
            &due_date,
        )?,
        Command::SaveInvoiceDraft {
            company_id,
            invoice_id,
            expected_revision,
            values,
        } => save_invoice(
            book,
            actor,
            crate::session::company(&company_id)?,
            &values,
            invoice_id.as_deref(),
            expected_revision,
        )?,
        Command::PostReviewedInvoice {
            company_id,
            invoice_id,
            expected_revision,
            ..
        } => fb_core::sales::post_invoice(
            book,
            actor,
            crate::session::company(&company_id)?,
            &invoice_id,
            Some(expected_revision),
            review.as_ref(),
        )?,
        Command::VoidInvoice {
            company_id,
            invoice_id,
            expected_revision,
            date,
            reason,
        } => fb_core::sales::void_invoice(
            book,
            actor,
            crate::session::company(&company_id)?,
            &invoice_id,
            Some(expected_revision),
            &json!(date),
            &json!(reason),
        )?,
        _ => unreachable!("D4 command dispatch"),
    };
    Ok(Outcome::Done { data })
}
fn context(app: &mut App, r: &Route) -> AppResult<Value> {
    let mut vm = serde_json::to_value(shell(
        app,
        Some(arg_company(r)?),
        &Current {
            route: &r.name,
            kind: None,
        },
    )?)
    .expect("shell");
    vm["files"] = json!([]);
    vm["removed"] = json!([]);
    vm["notes"] = json!([]);
    vm["allowed"] = json!("PDF, PNG, JPEG or WebP");
    vm["max_mb"] = json!(10);
    vm["max_count"] = json!(20);
    Ok(vm)
}
/// The inlined panel's flat names (D4's port of `attachments/panel.html`) from the real D9 panel.
fn attachments(app: &mut App, c: CompanyId, identity: &str, vm: &mut Value) -> AppResult<()> {
    super::d9::attachments::panel(app, c, identity, vm)?;
    let panel = vm["attachments"].take();
    for (flat, key) in [("attachment_add_url", "add_url"), ("attachment_csrf", "csrf_token"), ("attachment_request_key", "request_key"),
                        ("files", "files"), ("removed", "removed"), ("notes", "notes"), ("full", "full")] {
        vm[flat] = panel[key].clone();
    }
    Ok(())
}
pub(super) fn render(
    app: &mut App,
    r: &Route,
    p: Posted<'_>,
    path: &str,
) -> AppResult<(String, u16)> {
    if r.name.starts_with("receipts:") || r.name.starts_with("sales_receipts:") {
        payments::render(app, r, p, path)
    } else if r.name.starts_with("sales:") {
        estimates::render(app, r, p, path)
    } else {
        invoices::render(app, r, p, path)
    }
}
/// The D4 routes whose Django view accepts POST. Every other D4 route (lists, details, revisions,
/// prints, invoice review) answers a POST with 405, after the company membership check, as Django's
/// views do (`HttpResponseNotAllowed`). Without this gate a POST to `sales:list` reached
/// `estimates::submit` and panicked on the missing `estimate_id` (fuzz audit F-02).
const POST_ROUTES: &[&str] = &[
    "sales:new",
    "sales:edit",
    "sales:state",
    "invoices:new",
    "invoices:edit",
    "invoices:post",
    "invoices:void",
    "invoices:convert",
    "receipts:new",
    "receipts:reverse",
    "sales_receipts:new",
    "sales_receipts:void",
];
pub(super) fn submit(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    if !POST_ROUTES.contains(&r.name.as_str()) {
        return super::d8::method_not_allowed(app, r);
    }
    if r.name.starts_with("receipts:") || r.name.starts_with("sales_receipts:") {
        payments::submit(app, r, path, p)
    } else if r.name.starts_with("sales:") {
        estimates::submit(app, r, path, p)
    } else {
        invoices::submit(app, r, path, p)
    }
}
fn identity(tx: &(impl Query + ?Sized), id: &str) -> Result<Value, FbError> {
    Ok(tx.query_row(
        "SELECT id,number FROM documents_documentidentity WHERE id=?1",
        [id],
        |r| Ok(json!({"pk":r.get::<_,String>(0)?,"number":r.get::<_,String>(1)?})),
    )?)
}
fn journal_label(tx: &(impl Query + ?Sized), id: &Value) -> Result<String, FbError> {
    let Some(id) = id.as_str() else {
        return Ok("Journal unavailable".into());
    };
    let j = fb_core::store::journal(
        tx,
        fb_core::JournalId::parse(id).ok_or_else(|| FbError::missing("Journal"))?,
    )?;
    Ok(j.map(|j| format!("Journal J{:06}", j.number))
        .unwrap_or_else(|| "Journal unavailable".into()))
}
fn display_revision(h: &mut Value) {
    h["get_state_display"] = json!(match h["state"].as_str().unwrap_or("") {
        "DRAFT" => "Draft",
        "OPEN" => "Open",
        "VOID" => "Voided",
        "ACCEPTED" => "Accepted",
        "DECLINED" => "Declined",
        "CANCELLED" => "Cancelled",
        s => s,
    });
    h["get_tax_mode_display"] = json!(match h["tax_mode"].as_str().unwrap_or("") {
        "NONE" => "No tax",
        "MANUAL" => "Manual rate",
        "RATE" => "Tax rate",
        "GROUP" => "Tax group",
        "ITEM" => "Sales tax item",
        s => s,
    });
}
fn references(tx: &(impl Query + ?Sized), source: &str) -> Result<Vec<Value>, FbError> {
    let mut st=tx.prepare("SELECT role,account_code,account_name,account_kind FROM documents_documentaccount WHERE source_id=?1 ORDER BY role")?;
    Ok(st.query_map([source],|r|Ok(json!({"role":r.get::<_,String>(0)?,"account_code":r.get::<_,String>(1)?,"account_name":r.get::<_,String>(2)?,"account_kind":r.get::<_,String>(3)?})))?.collect::<Result<_,_>>()?)
}
fn reversal_fields(
    p: Option<Posted<'_>>,
    revision: i64,
    today: chrono::NaiveDate,
    _sales: bool,
    e: &Errors,
) -> Vec<Field> {
    let v = |k: &str, initial: String| super::d2::data_value(p, k, Some(initial));
    let mut fs = vec![
        Field::new("expected_revision", "Expected revision", Widget::Hidden)
            .required()
            .value_opt(v("expected_revision", revision.to_string())),
        Field::new("date", "Reversal date", Widget::Date)
            .required()
            .value_opt(v("date", today.to_string())),
        Field::new("reason", "Reason", Widget::Textarea { cols: 40, rows: 2 })
            .required()
            .attr("maxlength", "500")
            .value_opt(v("reason", String::new())),
    ];
    if _sales {
        fs.last_mut().unwrap().attrs.clear();
    }
    decorate(fs, e)
}
fn clean_revision(p: Posted<'_>, min: i64, e: &mut Errors) -> i64 {
    let s = get(p, "expected_revision").unwrap_or("");
    match clean_integer(s) {
        Some(n) if n >= min => n,
        Some(n) => {
            error(
                e,
                "expected_revision",
                format!("Ensure this value is greater than or equal to {min}."),
            );
            n
        }
        None => {
            error(
                e,
                "expected_revision",
                if s.is_empty() {
                    "This field is required."
                } else {
                    "Enter a whole number."
                },
            );
            0
        }
    }
}

fn receipt_input(v: &crate::commands::CustomerReceiptValues, sale: bool) -> Value {
    let mut input = serde_json::to_value(v).expect("typed receipt");
    if sale {
        input.as_object_mut().unwrap().remove("allocations");
        input.as_object_mut().unwrap().remove("amount");
        for l in input["lines"].as_array_mut().unwrap() {
            for k in ["origin_position", "refresh_item", "keep_archived"] {
                l.as_object_mut().unwrap().remove(k);
            }
        }
    } else {
        for k in ["tax_mode", "tax_item_id", "lines"] {
            input.as_object_mut().unwrap().remove(k);
        }
    }
    input
}
pub(super) fn fixed_document(h: &mut Value, lines: &mut [Value]) {
    fn fixed(v: &mut Value, n: usize) {
        if let Some(s) = v.as_str() {
            let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
            let mut frac = frac.to_string();
            while frac.len() < n {
                frac.push('0');
            }
            *v = json!(format!("{whole}.{frac}"));
        }
    }
    fixed(&mut h["tax_rate"], 4);
    for l in lines {
        fixed(&mut l["quantity"], 4);
        fixed(&mut l["rate"], 2);
    }
}
