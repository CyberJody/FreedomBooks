//! Scoped form choices, including only a retry's original archived references.
use super::*;
pub(in crate::screens) fn customers(
    book: &mut Book,
    c: CompanyId,
    all: bool,
    retained: Option<&str>,
    confirm: bool,
) -> AppResult<Vec<(String, String)>> {
    book.read(|tx| -> Result<_, FbError> {
        let rows = fb_core::directory::list(tx, c, fb_core::directory::Model::Customer)?;
        let rows = rows
            .into_iter()
            .filter(|r| all || r["active"] == true || r["id"].as_str() == retained)
            .collect::<Vec<_>>();
        let duplicates = fb_core::party_display::duplicate_active_display_names(
            tx,
            c,
            fb_core::directory::Model::Customer,
        )?;
        let ordinals = super::super::d2::ordinals(tx, c, "directory_customer")?;
        Ok(rows
            .iter()
            .map(|r| {
                let id = r["id"].as_str().unwrap();
                let peers = rows
                    .iter()
                    .filter(|p| p["display_name"] == r["display_name"])
                    .cloned()
                    .collect::<Vec<_>>();
                let text = fb_core::party_display::contact_list_label(
                    r,
                    &duplicates,
                    Some(&peers),
                    if confirm {
                        " (archived — confirm retention or replace)"
                    } else {
                        " (archived)"
                    },
                );
                (
                    id.into(),
                    super::super::short_contact_label(text, id, ordinals[id]),
                )
            })
            .collect())
    })
    .map_err(AppError::from)
}
pub(super) fn retry_document(
    app: &mut App,
    c: CompanyId,
    estimate: bool,
    p: Option<Posted<'_>>,
) -> AppResult<Option<Value>> {
    let Some(key) = p
        .and_then(|p| get(p, "request_key"))
        .and_then(fb_core::py::py_uuid)
        .map(|v| v.0)
    else {
        return Ok(None);
    };
    app.open_mut()?.book.read(|tx|->Result<_,FbError>{
  let (root,revisions,lines,parent)=if estimate{("sales_estimate","sales_estimaterevision","sales_estimateline","estimate_id")}else{("sales_invoice","sales_invoicerevision","sales_invoiceline","invoice_id")};
  let raw:Option<(String,String)>=tx.query_row(&format!("SELECT r.id,r.data FROM {root} e JOIN {revisions} r ON r.{parent}=e.id AND r.number=1 JOIN documents_documentidentity d ON d.id=e.identity_id WHERE e.company_id=?1 AND d.request_key=?2"),params![c.to_db(),key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
  let Some((id,raw))=raw else{return Ok(None)};let current:Value=fb_db::parse_unique_json_as(&raw).map_err(|_|FbError::validation("Invalid stored sales revision."))?;let mut st=tx.prepare(&format!("SELECT data FROM {lines} WHERE revision_id=?1 ORDER BY position"))?;
  let lines=st.query_map([id],|r|r.get::<_,String>(0))?.map(|s|fb_db::parse_unique_json_as::<Value>(&s?).map_err(|_|FbError::validation("Invalid stored sales line."))).collect::<Result<Vec<_>,FbError>>()?;
  Ok(Some(json!({"current":current,"lines":lines})))
 }).map_err(AppError::from)
}
pub(super) fn prior_receipt(
    app: &mut App,
    c: CompanyId,
    sale: bool,
    p: Option<Posted<'_>>,
) -> AppResult<Option<Value>> {
    let Some(key) = p
        .and_then(|p| get(p, "request_key"))
        .and_then(fb_core::py::py_uuid)
        .map(|v| v.0)
    else {
        return Ok(None);
    };
    app.open_mut()?.book.read(|tx|->Result<_,FbError>{
  let raw:Option<String>=tx.query_row("SELECT r.data FROM sales_receipt r JOIN documents_documentidentity d ON d.id=r.identity_id WHERE r.company_id=?1 AND r.kind=?2 AND d.request_key=?3",params![c.to_db(),if sale{"SR"}else{"RCP"},key],|r|r.get(0)).optional()?;
  let Some(raw)=raw else{return Ok(None)};let mut v:Value=fb_db::parse_unique_json_as(&raw).map_err(|_|FbError::validation("Invalid stored receipt."))?;let id=v["id"].as_str().unwrap();
  let mut st=tx.prepare("SELECT invoice_id FROM sales_receiptallocation WHERE receipt_id=?1 ORDER BY id")?;let invoices=st.query_map([id],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
  let mut lines=vec![];let mut detail=Value::Null;
  if sale{let (sid,raw):(String,String)=tx.query_row("SELECT id,data FROM sales_salesreceipt WHERE receipt_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?)))?;detail=fb_db::parse_unique_json_as(&raw).map_err(|_|FbError::validation("Invalid stored sale."))?;let mut st=tx.prepare("SELECT data FROM sales_salesreceiptline WHERE sale_id=?1 ORDER BY position")?;for row in st.query_map([sid],|r|r.get::<_,String>(0))?{lines.push(fb_db::parse_unique_json_as::<Value>(&row?).map_err(|_|FbError::validation("Invalid stored sale line."))?);}}
  v["lines"]=json!(lines);v["sale"]=detail;v["invoice_ids"]=json!(invoices);Ok(Some(v))
 }).map_err(AppError::from)
}
