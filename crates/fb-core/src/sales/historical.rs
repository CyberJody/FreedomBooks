//! Faithful, bounded CyberBrain invoice import through the same Writer services as normal sales.
//! Source evidence is immutable; source timestamps never become actor audit timestamps.
use super::*;

fn fail() -> FbError {
    FbError::validation(
        "The source invoice cannot be represented faithfully. Review its source data and mappings.",
    )
}
fn decimal(v: &Value) -> FbResult<Value> {
    match v {
        Value::String(s) => Ok(json!(s)),
        Value::Number(n) => Ok(json!(n.to_string())),
        _ => Err(fail()),
    }
}
fn source_money(v: &Value) -> FbResult<i64> {
    let s = decimal(v)?;
    let s = s.as_str().ok_or_else(fail)?;
    let (sign, abs) = if let Some(s) = s.strip_prefix('-') {
        (-1, s)
    } else {
        (1, s)
    };
    Ok(sign * cents(&json!(abs))?)
}
fn bounded(v: &Value, depth: usize, count: &mut usize) -> FbResult<()> {
    *count += 1;
    if depth > 12 || *count > 4096 {
        return Err(fail());
    }
    match v {
        Value::String(s) if s.len() > 16_384 => return Err(fail()),
        Value::Array(a) => {
            if a.len() > 100 {
                return Err(fail());
            }
            for v in a {
                bounded(v, depth + 1, count)?;
            }
        }
        Value::Object(o) => {
            if o.len() > 100 {
                return Err(fail());
            }
            for (k, v) in o {
                if k.len() > 100 {
                    return Err(fail());
                }
                bounded(v, depth + 1, count)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn uuid(v: &Value) -> FbResult<String> {
    py::py_uuid(v.as_str().ok_or_else(fail)?)
        .map(|v| v.0)
        .ok_or_else(fail)
}
fn timestamp(v: &Value, required: bool) -> FbResult<Option<String>> {
    if v.is_null() || v == "" {
        return if required { Err(fail()) } else { Ok(None) };
    }
    let s = v.as_str().ok_or_else(fail)?;
    chrono::DateTime::parse_from_rfc3339(s).map_err(|_| fail())?;
    Ok(Some(s.into()))
}
fn source_number(s: &Value) -> FbResult<String> {
    let n = s
        .get("display_number")
        .filter(|v| v.as_str().is_some_and(|v| !v.is_empty()))
        .cloned()
        .unwrap_or(decimal(&s["number"])?);
    text(&n, 80, true)
}
fn validate(raw: &Value) -> FbResult<Value> {
    if raw.to_string().len() > 128 * 1024 {
        return Err(fail());
    }
    bounded(raw, 0, &mut 0)?;
    let source = &raw["source"];
    let mut value = raw["invoice"].clone();
    let source_id = uuid(&source["id"])?;
    if source["id"] != source_id {
        return Err(fail());
    }
    let number = source_number(source)?;
    let state = source["status"]
        .as_str()
        .filter(|s| ["draft", "sent", "partial", "paid", "void"].contains(s))
        .ok_or_else(fail)?;
    for k in ["created_at", "updated_at"] {
        timestamp(&source[k], true)?;
    }
    for k in ["sent_at", "paid_at", "voided_at"] {
        timestamp(&source[k], false)?;
    }
    if date(&source["issue_date"])? != date(&value["date"])?
        || date(&source["due_date"])? != date(&value["due_date"])?
    {
        return Err(fail());
    }
    if !value["invoice_id"].is_null()
        || value["refresh_customer"] == true
        || value["keep_archived"] == true
    {
        return Err(fail());
    }
    value["request_key"] = json!(source_id);
    value["expected_revision"] = json!(0);
    let lines = value["lines"]
        .as_array()
        .filter(|a| (1..=50).contains(&a.len()))
        .ok_or_else(fail)?;
    let original = source["lines"]
        .as_array()
        .filter(|a| a.len() == lines.len())
        .ok_or_else(fail)?;
    for (a, b) in lines.iter().zip(original) {
        if !a["origin_position"].is_null()
            || a["refresh_item"] == true
            || a["keep_archived"] == true
        {
            return Err(fail());
        }
        let kind = calculations::invoice_kind(a)?;
        let signed = source_money(&b["amount"])?;
        let rate = source_money(&b["unit_price"])?;
        if a["description"].as_str().map(py::py_strip)
            != b["description"].as_str().map(py::py_strip)
            || a["taxable"].as_bool()
                != b["taxable"].as_bool().or_else(|| {
                    b["taxable"]
                        .as_i64()
                        .filter(|v| *v == 0 || *v == 1)
                        .map(|v| v == 1)
                })
        {
            return Err(fail());
        }
        if calculations::invoice_line_amount(a)?.cents() != i128::from(signed.abs()) {
            return Err(fail());
        }
        let expected = if signed < 0 || rate < 0 {
            "DISCOUNT"
        } else if calculations::decimal(
            &decimal(&b["quantity"])?,
            4,
            rust_decimal::Decimal::MAX,
            false,
        )?
        .is_zero()
            && rate == 0
        {
            "NOTE"
        } else {
            "CHARGE"
        };
        if kind != expected || cents(&a["rate"])? != rate.abs() {
            return Err(fail());
        }
        if kind != "NOTE"
            && calculations::quantity(&a["quantity"])?
                != calculations::quantity(&decimal(&b["quantity"])?)?
        {
            return Err(fail());
        }
    }
    let calc = calculations::invoice_totals(lines, &value, false)?;
    let total = source_money(&source["total"])?;
    if cents(&calc["total"])? != total
        || cents(&calc["tax"])? != source_money(&source["tax_amount"])?
        || cents(&calc["subtotal"])?
            != source_money(&source["subtotal"])? + source_money(&source["gc_discount"])?
    {
        return Err(fail());
    }
    let payments = source["payments"]
        .as_array()
        .filter(|p| p.len() <= 50)
        .ok_or_else(fail)?;
    let mut paid = 0i64;
    let mut ids = BTreeSet::new();
    let mut refs = BTreeSet::new();
    for p in payments {
        if !ids.insert(uuid(&p["id"])?) || uuid(&p["invoice_id"])? != source_id {
            return Err(fail());
        }
        timestamp(&p["created_at"], true)?;
        if date(&p["date"])? < date(&value["date"])? {
            return Err(fail());
        }
        let amount = source_money(&p["amount"])?;
        if amount <= 0 {
            return Err(fail());
        }
        paid = paid.checked_add(amount).ok_or_else(fail)?;
        let reference = field_text(p, "external_id", 120, false)?;
        if !reference.is_empty() && !refs.insert(reference) {
            return Err(fail());
        }
    }
    if paid != source_money(&source["amount_paid"])?
        || paid > total
        || (state == "paid" && paid != total)
        || (["draft", "void"].contains(&state) && paid != 0)
    {
        return Err(fail());
    }
    if (state == "partial" && (paid <= 0 || paid >= total)) || (state == "sent" && paid != 0) {
        return Err(fail());
    }
    if py::py_strip(value["memo"].as_str().unwrap_or(""))
        != py::py_strip(source["notes"].as_str().ok_or_else(fail)?)
    {
        return Err(fail());
    }
    if calculations::decimal(
        &decimal(&source["tax_rate"])?,
        4,
        rust_decimal::Decimal::ONE_HUNDRED,
        false,
    )? != calculations::decimal(
        &calc["tax_rate"],
        4,
        rust_decimal::Decimal::ONE_HUNDRED,
        false,
    )? {
        return Err(fail());
    }
    if state == "void" {
        timestamp(&source["voided_at"], true)?;
    }
    if state == "draft" && timestamp(&source["sent_at"], false)?.is_some() {
        return Err(fail());
    }
    Ok(
        json!({"source":source,"invoice":value,"source_id":source_id,"number":number,"digest":fb_db::invoice_import_digest(&json!({"source":source,"invoice":value})).ok_or_else(fail)?}),
    )
}
fn payment_input(source: &Value, p: &Value, invoice: &Value) -> FbResult<Value> {
    let method = match p["method"].as_str().ok_or_else(fail)? {
        "card" => "CARD",
        "cash" => "CASH",
        "check" => "CHECK",
        "transfer" | "zelle" => "TRANSFER",
        "other" => "OTHER",
        _ => return Err(fail()),
    };
    Ok(
        json!({"request_key":uuid(&p["id"])?,"customer_id":invoice["current"]["customer_id"],"date":date(&p["date"])?,"method":method,
        "reference":field_text(p,"reference",120,false)?,"memo":format!("Imported CyberBrain payment for {}",source_number(source)?),
        "destination":"UNDEPOSITED","amount":cash(source_money(&p["amount"])?),"allocations":[{"invoice_id":invoice["id"],"amount":cash(source_money(&p["amount"])?) }]}),
    )
}
fn existing(tx: &Writer<'_>, c: CompanyId, v: &Value) -> FbResult<Option<Value>> {
    let prior:Option<(String,String)>=tx.query_row("SELECT invoice_id,digest FROM sales_importinvoice WHERE company_id=?1 AND source='CyberBrain' AND source_id=?2",params![c.to_db(),v["source_id"].as_str()],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((key, hash)) = prior {
        if v["digest"] != hash {
            return Err(FbError::validation(
                "This source invoice was already imported with different data or mappings. Review its existing history; do not import it twice.",
            ));
        }
        return Ok(Some(
            json!({"invoice_id":key,"number":v["number"],"already_imported":true}),
        ));
    }
    Ok(None)
}
fn context(tx: &Writer<'_>, c: CompanyId, rows: &[Value]) -> FbResult<Value> {
    let mut selected = Vec::new();
    for row in rows {
        let v = &row["invoice"];
        selected.push(directory::get(
            tx,
            c,
            Model::Customer,
            v["customer_id"].as_str().ok_or_else(fail)?,
        )?);
        for l in v["lines"].as_array().ok_or_else(fail)? {
            if let Some(id) = l["item_id"].as_str() {
                selected.push(directory::get(tx, c, Model::Item, id)?);
            }
            if let Some(id) = l["income_account_id"].as_str() {
                let a = account(tx, c, id)?;
                selected.push(
                    json!({"account":account_snapshot(&a),"active":a.active,"postable":a.postable}),
                );
            }
        }
    }
    let setup = documents::setup_snapshot(tx, c)?;
    for role in ["receivables", "undeposited", "collected_tax"] {
        if let Some(id) = setup[role].as_str() {
            let a = account(tx, c, id)?;
            selected.push(json!({"role":role,"account":account_snapshot(&a),"active":a.active,"postable":a.postable}));
        }
    }
    Ok(json!({"company":company_snapshot(tx,c)?,"setup":setup,"selection":selected,"imports":rows}))
}
fn apply_one(tx: &Writer<'_>, actor: UserId, c: CompanyId, v: &Value) -> FbResult<Value> {
    if let Some(old) = existing(tx, c, v)? {
        return Ok(old);
    }
    let source = &v["source"];
    let input = &v["invoice"];
    let duplicate:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sales_invoice i JOIN sales_invoicerevision r ON r.invoice_id=i.id AND r.number=i.revision JOIN documents_documentidentity d ON d.id=i.identity_id WHERE i.company_id=?1 AND ((r.customer_id=?2 AND r.date=?3 AND r.total=?4) OR d.number=?5)) OR EXISTS(SELECT 1 FROM documents_documentidentity WHERE company_id=?1 AND request_key=?6)",params![c.to_db(),input["customer_id"].as_str(),input["date"].as_str(),source_money(&source["total"])?,v["number"].as_str(),v["source_id"].as_str()],|r|r.get(0))?;
    if duplicate {
        return Err(FbError::validation(
            "A possible existing invoice matches this source number, customer/date/amount, or source key. Resolve the duplicate before importing.",
        ));
    }
    let mut inv = invoices::save_invoice_tx(tx, actor, c, input)?;
    let key = inv["id"].as_str().ok_or_else(fail)?.to_owned();
    let posted = source["status"] != "draft"
        && (source["status"] != "void" || timestamp(&source["sent_at"], false)?.is_some());
    let mut plans = Vec::new();
    if posted {
        let r = invoices::review_invoice_tx(tx, actor, c, &key, Some(1))?;
        plans.push(r.display.clone());
        inv = invoices::post_invoice_tx(tx, actor, c, &key, Some(1), Some(&r))?;
    }
    let mut payments = Vec::new();
    for p in source["payments"].as_array().ok_or_else(fail)? {
        let reference = field_text(p, "reference", 120, false)?;
        let external = field_text(p, "external_id", 120, false)?;
        let duplicate:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sales_receipt WHERE company_id=?1 AND ((?2<>'' AND json_extract(data,'$.reference')=?2) OR (?3<>'' AND json_extract(data,'$.reference')=?3))) OR EXISTS(SELECT 1 FROM invoice_stripe_payment WHERE company_id=?1 AND ?3<>'' AND intent_key=?3) OR EXISTS(SELECT 1 FROM documents_documentidentity WHERE company_id=?1 AND request_key=?4)",params![c.to_db(),reference,external,uuid(&p["id"])?],|r|r.get(0))?;
        if duplicate {
            return Err(FbError::validation(
                "A payment reference or source key already exists. Review it before importing another receipt.",
            ));
        }
        let data = payment_input(source, p, &inv)?;
        let review = receipts::review_tx(tx, actor, c, &data, false)?;
        plans.push(review.display.clone());
        let receipt = receipts::record_receipt_tx(tx, actor, c, &data, Some(&review), false)?;
        payments.push(
            json!({"source_id":uuid(&p["id"])?,"external_id":external,"receipt_id":receipt["id"]}),
        );
    }
    if source["status"] == "void" {
        let at = timestamp(&source["voided_at"], true)?.ok_or_else(fail)?;
        let when = chrono::DateTime::parse_from_rfc3339(&at)
            .map_err(|_| fail())?
            .date_naive()
            .to_string();
        inv = invoices::void_invoice_tx(
            tx,
            actor,
            c,
            &key,
            inv["revision"].as_i64(),
            &json!(when),
            &json!("Imported CyberBrain void history"),
        )?;
    }
    let user = services::access(tx, actor, c)?;
    services::audit(
        tx,
        &user,
        c,
        "invoice.imported",
        format!("invoice:{key}"),
        json!({"source":"CyberBrain","source_id":v["source_id"],"source_number":v["number"],"digest":v["digest"],"total":inv["current"]["total"],"payments":payments.len()}),
    )?;
    let audit:String=tx.query_row("SELECT id FROM ledger_auditevent WHERE company_id=?1 AND action='invoice.imported' AND target=?2 AND json_extract(detail,'$.digest')=?3",params![c.to_db(),format!("invoice:{key}"),v["digest"].as_str()],|r|r.get(0))?;
    tx.execute("INSERT INTO sales_importinvoice(company_id,source,source_id,source_number,invoice_id,revision_id,digest,data,actor_id,audit_id) VALUES(?1,'CyberBrain',?2,?3,?4,?5,?6,?7,?8,?9)",params![c.to_db(),v["source_id"].as_str(),v["number"].as_str(),key,inv["current"]["id"].as_str(),v["digest"].as_str(),json!({"source":source,"invoice":input,"payments":payments}).to_string(),actor.to_db(),audit])?;
    for p in &payments {
        tx.execute("INSERT INTO sales_importpayment(company_id,source,source_id,invoice_id,receipt_id,external_key) VALUES(?1,'CyberBrain',?2,?3,?4,?5)",params![c.to_db(),p["source_id"].as_str(),key,p["receipt_id"].as_str(),p["external_id"].as_str()])?;
    }
    tx.execute(
        "INSERT INTO sales_importseal(invoice_id) VALUES(?1)",
        [&key],
    )?;
    Ok(
        json!({"invoice_id":key,"number":v["number"],"source_status":source["status"],"total":inv["current"]["total"],"tax":inv["current"]["tax"],"source_subtotal":source["subtotal"],"source_discount":source["gc_discount"],"already_imported":false,"payments":payments,"posting_plans":plans}),
    )
}
#[derive(Debug)]
enum RunError {
    Core(FbError),
    Preview(Value),
}
impl From<fb_db::rusqlite::Error> for RunError {
    fn from(e: fb_db::rusqlite::Error) -> Self {
        Self::Core(e.into())
    }
}
impl From<FbError> for RunError {
    fn from(e: FbError) -> Self {
        Self::Core(e)
    }
}
/// Preview and apply execute identical services. Only the outer Writer decides whether to commit.
pub fn run(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    raw: &[Value],
    preview: bool,
    expected: Option<&str>,
) -> FbResult<Value> {
    if raw.is_empty() || raw.len() > 25 {
        return Err(FbError::validation(
            "Import between one and 25 invoices at a time.",
        ));
    }
    let rows = raw.iter().map(validate).collect::<FbResult<Vec<_>>>()?;
    let mut ids = BTreeSet::new();
    for v in &rows {
        if !ids.insert(v["source_id"].clone().to_string()) {
            return Err(fail());
        }
    }
    let result=book.write(|tx| -> Result<Value,RunError> {
        services::access(tx,actor,c)?;
        let prior=rows.iter().map(|v|existing(tx,c,v)).collect::<FbResult<Vec<_>>>()?;
        if prior.iter().all(Option::is_some) {return Ok(json!({"invoices":prior,"already_imported":true,"preview":preview}));}
        let hash=digest(&context(tx,c,&rows)?);
        if !preview&&expected!=Some(hash.as_str()) {return Err(FbError::validation("Import data, customer/item details, or accounting setup changed. Preview again before importing.").into());}
        let output=rows.iter().map(|v|apply_one(tx,actor,c,v)).collect::<FbResult<Vec<_>>>()?;
        let result=json!({"invoices":output,"review_digest":hash,"preview":preview});
        if preview {Err(RunError::Preview(result))} else {Ok(result)}
    });
    match result {
        Ok(v) | Err(RunError::Preview(v)) => Ok(v),
        Err(RunError::Core(e)) => Err(e),
    }
}

/// Read-only, escaped presentation evidence; none of these provider identifiers initiate provider actions.
pub fn evidence(q: &(impl Query + ?Sized), c: CompanyId, key: &str) -> FbResult<Value> {
    let raw:Option<(String,String)>=q.query_row("SELECT h.data,a.occurred_at FROM sales_importinvoice h JOIN ledger_auditevent a ON a.id=h.audit_id WHERE h.company_id=?1 AND h.invoice_id=?2",params![c.to_db(),key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((raw, at)) = raw else {
        return Ok(Value::Null);
    };
    let data: Value = fb_db::parse_unique_json_as(&raw).map_err(|_| invalid_stored())?;
    let source = &data["source"];
    let payments = source["payments"]
        .as_array()
        .ok_or_else(invalid_stored)?
        .iter()
        .map(|p| {
            let mut p = p.clone();
            p["amount"] = json!(cash(source_money(&p["amount"])?));
            Ok(p)
        })
        .collect::<FbResult<Vec<_>>>()?;
    Ok(
        json!({"source":source,"payments":payments,"imported_at":at,"subtotal":cash(source_money(&source["subtotal"])?),"discount":cash(source_money(&source["gc_discount"])?),"has_discount":source_money(&source["gc_discount"])?!=0,"tax":cash(source_money(&source["tax_amount"])?),"total":cash(source_money(&source["total"])?)}),
    )
}

/// Use source display details only while this revision still has the imported financial content.
/// Editing a draft must never pair new totals with an old source subtotal or customer address.
pub fn presentation(
    q: &(impl Query + ?Sized),
    c: CompanyId,
    key: &str,
    header: &Value,
    lines: &[Value],
) -> FbResult<Value> {
    let mut out = evidence(q, c, key)?;
    if out.is_null() {
        return Ok(out);
    }
    let raw:String=q.query_row("SELECT r.data FROM sales_importinvoice h JOIN sales_invoicerevision r ON r.id=h.revision_id WHERE h.company_id=?1 AND h.invoice_id=?2",params![c.to_db(),key],|r|r.get(0))?;
    let original: Value = fb_db::parse_unique_json_as(&raw).map_err(|_| invalid_stored())?;
    let mut statement =
        q.prepare("SELECT data FROM sales_invoiceline WHERE revision_id=?1 ORDER BY position")?;
    let original_lines = statement
        .query_map([original["id"].as_str()], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|s| fb_db::parse_unique_json_as::<Value>(&s).map_err(|_| invalid_stored()))
        .collect::<FbResult<Vec<_>>>()?;
    let headers = [
        "customer_id",
        "customer_snapshot",
        "date",
        "due_date",
        "memo",
        "tax_mode",
        "tax_rate",
        "tax_label",
        "tax_snapshot",
        "subtotal",
        "taxable",
        "tax",
        "total",
    ];
    let fields = [
        "item_id",
        "item_snapshot",
        "income_account_id",
        "description",
        "quantity",
        "rate",
        "taxable",
        "amount",
        "line_kind",
    ];
    let same = headers.iter().all(|k| header[k] == original[k])
        && lines.len() == original_lines.len()
        && lines
            .iter()
            .zip(&original_lines)
            .all(|(a, b)| fields.iter().all(|k| a[k] == b[k]));
    out["matches_revision"] = json!(same);
    Ok(out)
}
