//! Core half of receipt intake: extracted fields become an editable bill payload.
//! No provider call, file I/O, automatic vendor creation, posting, or implicit consent.
//! The host/CoreBridge resolves an explicit local vendor and supplies a stable UUID key.
use super::*;
fn input(
    c: &(impl Query + ?Sized),
    co: CompanyId,
    vendor_id: &str,
    fields: &Value,
    key: &str,
) -> FbResult<Value> {
    let vendor = directory::get(c, co, Model::Vendor, vendor_id)?;
    if vendor["active"] != true {
        return Err(err("Choose an active vendor in this company."));
    }
    let dt = date(&fields["document_date"])?;
    let defaults = directory::prefill_catalog(c, co)?;
    let hint = &defaults[vendor_id];
    let due = if fields["due_date"].is_null() || fields["due_date"] == "" {
        let day = chrono::NaiveDate::parse_from_str(&dt, "%Y-%m-%d")
            .map_err(|_| err("Enter a valid calendar date."))?;
        let terms = s(hint, "terms").parse::<i64>().unwrap_or(0);
        day.checked_add_signed(chrono::Duration::days(terms))
            .ok_or_else(|| err("Enter a valid calendar date."))?
            .to_string()
    } else {
        date(&fields["due_date"])?
    };
    let rows = fields["lines"]
        .as_array()
        .ok_or_else(|| err("Enter between 1 and 50 meaningful lines."))?;
    let mut lines = vec![];
    for l in rows {
        lines.push(json!({"description":l["description"],"quantity":l["quantity"],"rate":l["rate"],"expense_account_id":if s(hint,"expense_account").is_empty(){Value::Null}else{hint["expense_account"].clone()}}));
    }
    Ok(
        json!({"vendor_id":vendor_id,"date":dt,"due_date":due,"reference":fields.get("document_number").unwrap_or(&json!("")),"memo":"","request_key":key,"lines":lines}),
    )
}
fn check_lines(fields: &Value) -> FbResult<()> {
    for l in fields["lines"]
        .as_array()
        .ok_or_else(|| err("Enter between 1 and 50 meaningful lines."))?
    {
        let amount = calculations::line_amount(&l["quantity"], &l["rate"])?;
        if let Some(printed) = l.get("amount").filter(|v| !v.is_null() && **v != "")
            && cents(printed)? as i128 != amount.cents()
        {
            return Err(err(
                "The receipt line does not add up. Correct the quantity or rate before saving.",
            ));
        }
    }
    Ok(())
}
/// Read-only suggestions from vendor defaults. Extracted receipt lines already include supplier tax
/// as a separate line, as produced by the frozen receipt parser; never tax the bill again.
pub fn receipt_draft_input(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    vendor_id: &str,
    fields: &Value,
    key: &str,
) -> FbResult<Value> {
    book.read(|tx| {
        services::access(tx, actor, co)?;
        input(tx, co, vendor_id, fields, key)
    })
}
/// Explicit user Save only. A successful call creates DRAFT history and no journal.
pub fn save_receipt_draft(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    vendor_id: &str,
    fields: &Value,
    key: &str,
) -> FbResult<Value> {
    write(book, |tx| {
        services::access(tx, actor, co)?;
        check_lines(fields)?;
        let p = input(tx, co, vendor_id, fields, key)?;
        save_bill_tx(tx, actor, co, &p)
    })
}
