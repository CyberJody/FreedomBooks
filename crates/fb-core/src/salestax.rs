//! FX11 tax settings, rates/groups, resolver and one-time bulk step, translated from salestax.services.
use crate::{Book, CompanyId, FbError, FbResult, UserId, calculations, directory, documents, py, services};
use fb_db::Writer;
use fb_db::rusqlite::{OptionalExtension, params};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub fn percent(v: Decimal) -> String {
    v.normalize().to_string()
}
fn text(v: &Value, max: usize, label: &str, required: bool) -> FbResult<String> {
    retained_text(v, max, label, required, None)
}
fn retained_text(v: &Value, max: usize, label: &str, required: bool, previous: Option<&str>) -> FbResult<String> {
    if let Some(raw)=v.as_str() { py::clean_changed_free_text(raw, previous)?; }
    v.as_str()
        .map(py::py_strip)
        .filter(|s| py::py_len(s) <= max && (!required || !s.is_empty()))
        .map(str::to_string)
        .ok_or_else(|| FbError::validation(format!("Enter {label} (up to {max} characters).")))
}
pub fn settings_for(conn: &(impl fb_db::Query + ?Sized), company: CompanyId) -> FbResult<Value> {
    Ok(conn.query_row("SELECT id,charge_tax,default_item_id,owed_basis,filing_frequency,bulk_step,revision FROM salestax_salestaxsettings WHERE company_id=?1",[company.to_db()],|r|Ok(json!({"id":r.get::<_,String>(0)?,"charge_tax":r.get::<_,bool>(1)?,"default_item_id":r.get::<_,Option<String>>(2)?,"owed_basis":r.get::<_,String>(3)?,"filing_frequency":r.get::<_,String>(4)?,"bulk_step":r.get::<_,String>(5)?,"revision":r.get::<_,i64>(6)?}))).optional()?.unwrap_or_else(||json!({"charge_tax":false,"default_item_id":null,"owed_basis":"ACCRUAL","filing_frequency":"MONTHLY","bulk_step":"","revision":0})))
}
pub fn item(conn: &(impl fb_db::Query + ?Sized), company: CompanyId, id: &str) -> FbResult<Value> {
    let mut v=conn.query_row("SELECT id,kind,name,description,active,revision,rate,agency_id FROM salestax_taxitem WHERE company_id=?1 AND id=?2",params![company.to_db(),id],|r|Ok(json!({"id":r.get::<_,String>(0)?,"kind":r.get::<_,String>(1)?,"name":r.get::<_,String>(2)?,"description":r.get::<_,String>(3)?,"active":r.get::<_,bool>(4)?,"revision":r.get::<_,i64>(5)?,"rate":r.get::<_,Option<i64>>(6)?.map(|n|Decimal::from_i128_with_scale(n.into(),4).to_string()),"agency_id":r.get::<_,Option<String>>(7)?}))).optional()?.ok_or_else(||FbError::validation("Choose a sales tax rate or group from this company."))?;
    if v["kind"] == "GROUP" {
        let mut stmt = conn.prepare("SELECT member_id FROM salestax_taxgroupmember WHERE group_id=?1 ORDER BY position")?;
        v["member_ids"] = json!(stmt.query_map([id], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?);
    }
    Ok(v)
}
pub fn list(conn: &(impl fb_db::Query + ?Sized), company: CompanyId) -> FbResult<Vec<Value>> {
    let mut stmt = conn.prepare("SELECT id FROM salestax_taxitem WHERE company_id=?1 ORDER BY name COLLATE fb_text,id")?;
    let ids = stmt
        .query_map([company.to_db()], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    ids.iter().map(|id| item(conn, company, id)).collect()
}
fn members(conn: &(impl fb_db::Query + ?Sized), company: CompanyId, v: &Value) -> FbResult<Vec<Value>> {
    match v.get("member_ids") {
        None if v["kind"] == "RATE" => Ok(Vec::new()),
        Some(Value::Array(ids)) => ids.iter().map(|id| {
            let id = id.as_str().ok_or_else(|| FbError::validation("Choose tax rates from this company."))?;
            let row = item(conn, company, id)?;
            if row["kind"] != "RATE" { return Err(FbError::validation("Choose tax rates (not groups) from this company.")); }
            Ok(row)
        }).collect(),
        _ => Err(FbError::validation("Choose tax rates from this company.")),
    }
}
pub fn group_rate(conn: &(impl fb_db::Query + ?Sized), company: CompanyId, v: &Value) -> FbResult<Decimal> {
    members(conn, company, v)?.iter().try_fold(Decimal::from_i128_with_scale(0, 4), |a, m| {
        a.checked_add(value_rate(m)?).ok_or_else(|| FbError::validation("A tax group cannot add up to more than 100%."))
    })
}
fn value_rate(v: &Value) -> FbResult<Decimal> {
    calculations::decimal(&v["rate"], 4, Decimal::ONE_HUNDRED, false)
}
fn optional_id<'a>(input: &'a Value, field: &str, message: &str) -> FbResult<Option<&'a str>> {
    match input.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(id)) => Ok(Some(id)),
        _ => Err(FbError::validation(message)),
    }
}
pub fn item_label(conn: &(impl fb_db::Query + ?Sized), company: CompanyId, v: &Value) -> FbResult<String> {
    let r = if v["kind"] == "GROUP" {
        group_rate(conn, company, v)?
    } else {
        value_rate(v)?
    };
    Ok(format!(
        "{} · {}%{}{}",
        v["name"].as_str().ok_or_else(|| FbError::validation("Enter a name."))?,
        percent(r),
        if v["kind"] == "GROUP" { " (group)" } else { "" },
        if v["active"] == true { "" } else { " (inactive)" }
    ))
}
pub fn tax_snapshot(conn: &(impl fb_db::Query + ?Sized), company: CompanyId, v: &Value) -> FbResult<Value> {
    let rows = if v["kind"] == "RATE" {
        vec![v.clone()]
    } else {
        members(conn, company, v)?
    };
    let rows=rows.iter().map(|r|{
        let agency=r["agency_id"].as_str().map(|id|directory::get(conn,company,directory::Model::Vendor,id)).transpose()?;
        Ok(json!({"id":r["id"],"name":r["name"],"description":r["description"],"rate":r["rate"],"agency_id":r["agency_id"],"agency_name":agency.map(|a|a["display_name"].clone()).unwrap_or(json!(""))}))
    }).collect::<FbResult<Vec<_>>>()?;
    Ok(json!({"kind":v["kind"],"id":v["id"],"name":v["name"],"description":v["description"],"members":rows}))
}
pub fn same_snapshot(conn: &(impl fb_db::Query + ?Sized), company: CompanyId, stored: &Value, v: &Value) -> FbResult<bool> {
    let fresh = tax_snapshot(conn, company, v)?;
    fn strip(v: &Value) -> Option<Value> {
        let mut result = json!({});
        for k in ["kind", "id", "name", "description"] {
            result[k] = v[k].clone();
        }
        let rows = v
            .get("members")?
            .as_array()?
            .iter()
            .map(|m| {
                let mut row = json!({});
                for k in ["id", "name", "description", "rate", "agency_id", "agency_name"] {
                    row[k] = m[k].clone();
                }
                let r = Decimal::from_str_exact(m["rate"].as_str()?).ok()?;
                row["rate"] = json!(r.to_string());
                Some(row)
            })
            .collect::<Option<Vec<_>>>()?;
        result["members"] = json!(rows);
        Some(result)
    }
    Ok(strip(stored).is_some() && strip(stored) == strip(&fresh))
}
fn eligible(conn: &(impl fb_db::Query + ?Sized), company: CompanyId, v: &Value) -> FbResult<()> {
    if settings_for(conn, company)?["charge_tax"] != true {
        return Err(FbError::validation(
            "Sales tax is turned off for this company. Turn it on in Company → Sales tax, or choose No tax.",
        ));
    }
    if v["active"] != true || members(conn, company, v)?.iter().any(|m| m["active"] != true) {
        return Err(FbError::validation(format!(
            "Sales tax \"{}\" is inactive. Choose an active tax rate or group.",
            v["name"].as_str().unwrap()
        )));
    }
    Ok(())
}
pub fn document_tax(
    conn: &(impl fb_db::Query + ?Sized),
    company: CompanyId,
    request: &Value,
    previous: Option<&Value>,
    check_eligible: bool,
) -> FbResult<Value> {
    let mode = request["tax_mode"].as_str().unwrap_or("");
    match mode {
        "NONE" | "MANUAL" => {
            if let Some(raw) = request["tax_label"].as_str() {
                py::clean_changed_free_text(raw, previous.and_then(|p| p["tax_label"].as_str()))?;
            }
            if !request["tax_item_id"].is_null() {
                return Err(FbError::validation("Do not send a tax rate with No tax."));
            }
            Ok(
                json!({"tax_mode":mode,"tax_rate":request["tax_rate"],"tax_label":request.get("tax_label").cloned().unwrap_or(json!("")),"tax_snapshot":null,"tax_item_id":null}),
            )
        }
        "KEEP" => {
            let old = previous.ok_or_else(|| FbError::validation("There is no saved sales tax to keep. Choose a tax rate or No tax."))?;
            let mut result = json!({});
            for k in ["tax_mode", "tax_rate", "tax_label", "tax_snapshot", "tax_item_id"] {
                result[k] = old[k].clone();
            }
            if !py_truthy(&result["tax_snapshot"]) {
                result["tax_snapshot"] = Value::Null;
            }
            Ok(result)
        }
        "ITEM" => {
            let id = request["tax_item_id"]
                .as_str()
                .ok_or_else(|| FbError::validation("Choose a sales tax rate or group from this company."))?;
            let v = item(conn, company, id)?;
            if check_eligible {
                eligible(conn, company, &v)?;
            }
            Ok(json!({"tax_mode":v["kind"],"tax_rate":null,"tax_label":"","tax_snapshot":tax_snapshot(conn,company,&v)?,"tax_item_id":id}))
        }
        _ => Err(FbError::validation("Explicitly choose No tax or a sales tax rate.")),
    }
}
fn unique_name(conn: &(impl fb_db::Query + ?Sized), company: CompanyId, name: &str, exclude: &str) -> FbResult<()> {
    if conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM salestax_taxitem WHERE company_id=?1 AND (fb_upper(name)=fb_upper(?2) OR fb_lower(name)=fb_lower(?2)) AND id<>?3)",
        params![company.to_db(), name, exclude],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(FbError::validation(
            "A tax rate or group with this name already exists in this company (including inactive ones).",
        ));
    }
    Ok(())
}
fn locked_item(conn: &(impl fb_db::Query + ?Sized), company: CompanyId, id: Option<&str>, kind: &str, revision: Option<i64>) -> FbResult<Option<Value>> {
    if let Some(id) = id {
        let v = item(conn, company, id)?;
        if v["kind"] != kind {
            return Err(FbError::validation("Choose a sales tax rate or group from this company."));
        }
        documents::check_revision(v["revision"].as_i64().unwrap(), revision)?;
        Ok(Some(v))
    } else {
        Ok(None)
    }
}
fn persist_item(conn: &Writer<'_>, company: CompanyId, v: &Value) -> FbResult<()> {
    let rate = v["rate"].as_str().map(|r| Decimal::from_str_exact(r).unwrap().mantissa() as i64);
    let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM salestax_taxitem WHERE id=?1)", [v["id"].as_str()], |r| r.get(0))?;
    let sql = if exists {
        "UPDATE salestax_taxitem SET name=?4,description=?5,rate=?6,agency_id=?7,active=?8,revision=?9 WHERE id=?1 AND company_id=?2 AND kind=?3"
    } else {
        "INSERT INTO salestax_taxitem(id,company_id,kind,name,description,rate,agency_id,active,revision) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)"
    };
    conn.execute(sql,params![v["id"].as_str(),company.to_db(),v["kind"].as_str(),v["name"].as_str(),v["description"].as_str(),rate,v["agency_id"].as_str(),v["active"].as_bool(),v["revision"].as_i64()]).map_err(|error|{
        let error=FbError::from(error);
        if error.constraint()==Some("unique:index 'salestax_item_name_ci_per_company'") {
            FbError::validation("A tax rate or group with this name already exists in this company (including inactive ones).")
        }else{error}
    })?;
    Ok(())
}
/// `input`: name, description, rate, agency_id/new_agency_name; optional item_id/expected_revision.
pub fn save_rate(book: &mut Book, actor: UserId, company: CompanyId, input: &Value) -> FbResult<Value> {
    book.write(|tx|{
        let user=services::access(tx,actor,company)?;
        // locked_item below retains its original place in the validation order.
        let retained_result=input["item_id"].as_str().map(|id|item(tx,company,id)).transpose();
        let retained=retained_result.as_ref().ok().and_then(|v|v.as_ref());
        let name=retained_text(&input["name"],60,"a name",true,retained.as_ref().and_then(|r|r["name"].as_str()))?;
        let description=retained_text(&input["description"],120,"a description to print on forms",true,retained.as_ref().and_then(|r|r["description"].as_str()))?;
        let rate=calculations::decimal(&input["rate"],4,Decimal::ONE_HUNDRED,false).map_err(|_|FbError::validation("Enter a rate from 0 to 100 percent with at most 4 decimal places."))?;
        let new_agency=text(input.get("new_agency_name").filter(|v|py_truthy(v)).unwrap_or(&json!("")),120,"an agency name",false)?;
        if !input["agency_id"].is_null()&&!new_agency.is_empty(){return Err(FbError::validation("Choose an existing agency or type a new agency name, not both."));}
        let before=locked_item(tx,company,optional_id(input,"item_id","Choose a sales tax rate or group from this company.")?,"RATE",input["expected_revision"].as_i64())?;
        let id=before.as_ref().and_then(|v|v["id"].as_str()).map(str::to_string).unwrap_or_else(||fb_ids::new_v7().to_string());
        unique_name(tx,company,&name,&id)?;
        let agency=if !new_agency.is_empty(){directory::save_record_tx(tx,actor,company,directory::Model::Vendor,&json!({"display_name":new_agency}),None)?}
        else if let Some(id)=optional_id(input,"agency_id","Choose a tax agency (vendor) from this company.")?{
            let agency=directory::get(tx,company,directory::Model::Vendor,id).map_err(|_|FbError::validation("Choose a tax agency (vendor) from this company."))?;
            if agency["active"]!=true&&before.as_ref().is_none_or(|b|b["agency_id"]!=id){return Err(FbError::validation("Choose an active vendor as the tax agency."));}agency
        }else{return Err(FbError::validation("Choose the tax agency (the vendor you pay this tax to), or type a new agency name."));};
        let v=json!({"id":id,"kind":"RATE","name":name,"description":description,"rate":rate.to_string(),"agency_id":agency["id"],"active":before.as_ref().map(|v|v["active"].clone()).unwrap_or(json!(true)),"revision":documents::next_revision(before.as_ref().map(|v|v["revision"].as_i64().unwrap()).unwrap_or(0))?});
        persist_item(tx,company,&v)?;
        services::audit(tx,&user,company,if before.is_some(){"salestax.rate.updated"}else{"salestax.rate.created"},format!("taxitem:{id}"),json!({"before":before.as_ref().map(item_snapshot),"after":item_snapshot(&v)}))?;Ok(v)
    })
}
pub fn save_group(book: &mut Book, actor: UserId, company: CompanyId, input: &Value) -> FbResult<Value> {
    book.write(|tx|{
        let user=services::access(tx,actor,company)?;
        // locked_item below retains its original place in the validation order.
        let retained_result=input["item_id"].as_str().map(|id|item(tx,company,id)).transpose();
        let retained=retained_result.as_ref().ok().and_then(|v|v.as_ref());
        let name=retained_text(&input["name"],60,"a name",true,retained.as_ref().and_then(|r|r["name"].as_str()))?;
        let description=retained_text(&input["description"],120,"a description to print on forms",true,retained.as_ref().and_then(|r|r["description"].as_str()))?;
        let ids=input["member_ids"].as_array().ok_or_else(||FbError::validation("Choose tax rates from this company."))?;
        let ids=ids.iter().map(Value::as_str).collect::<Option<Vec<_>>>().ok_or_else(||FbError::validation("Choose tax rates from this company."))?;
        if ids.iter().collect::<std::collections::BTreeSet<_>>().len()!=ids.len(){return Err(FbError::validation("Choose each tax rate once."));}
        if !(2..=20).contains(&ids.len()){return Err(FbError::validation("A tax group combines 2 to 20 tax rates."));}
        let before=locked_item(tx,company,optional_id(input,"item_id","Choose a sales tax rate or group from this company.")?,"GROUP",input["expected_revision"].as_i64())?;
        let id=before.as_ref().and_then(|v|v["id"].as_str()).map(str::to_string).unwrap_or_else(||fb_ids::new_v7().to_string());
        unique_name(tx,company,&name,&id)?;
        let members=ids.iter().map(|id|item(tx,company,id)).collect::<FbResult<Vec<_>>>().map_err(|_|FbError::validation("Choose tax rates (not groups) from this company."))?;
        if members.iter().any(|m|m["kind"]!="RATE"){return Err(FbError::validation("Choose tax rates (not groups) from this company."));}
        let old=before.as_ref().and_then(|b|b["member_ids"].as_array()).cloned().unwrap_or_default();
        if members.iter().any(|m|m["active"]!=true&&!old.contains(&m["id"])){return Err(FbError::validation("Choose active tax rates."));}
        let active=before.as_ref().is_none_or(|b|b["active"]==true);
        if active&&members.iter().any(|m|m["active"]!=true){return Err(FbError::validation("An active group needs active tax rates. Replace the inactive rate or deactivate the group."));}
        if members.iter().map(|m|Decimal::from_str_exact(m["rate"].as_str().unwrap()).unwrap()).sum::<Decimal>()>Decimal::ONE_HUNDRED{return Err(FbError::validation("A tax group cannot add up to more than 100%."));}
        let v=json!({"id":id,"kind":"GROUP","name":name,"description":description,"rate":null,"agency_id":null,"active":active,"revision":documents::next_revision(before.as_ref().map(|v|v["revision"].as_i64().unwrap()).unwrap_or(0))?,"member_ids":ids});
        persist_item(tx,company,&v)?;tx.execute("DELETE FROM salestax_taxgroupmember WHERE group_id=?1",[&id])?;
        for (i,member)in ids.iter().enumerate(){tx.execute("INSERT INTO salestax_taxgroupmember VALUES(?1,?2,?3,?4)",params![fb_ids::new_v7().to_string(),id,member,(i+1)as i64])?;}
        services::audit(tx,&user,company,if before.is_some(){"salestax.group.updated"}else{"salestax.group.created"},format!("taxitem:{id}"),json!({"before":before.as_ref().map(item_snapshot),"after":item_snapshot(&v)}))?;Ok(v)
    })
}
fn persist_settings(conn: &Writer<'_>, company: CompanyId, v: &Value) -> FbResult<()> {
    let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM salestax_salestaxsettings WHERE company_id=?1)", [company.to_db()], |r| r.get(0))?;
    let sql = if exists {
        "UPDATE salestax_salestaxsettings SET charge_tax=?3,default_item_id=?4,owed_basis=?5,filing_frequency=?6,bulk_step=?7,revision=?8 WHERE id=?1 AND company_id=?2"
    } else {
        "INSERT INTO salestax_salestaxsettings VALUES(?1,?2,?3,?4,?5,?6,?7,?8)"
    };
    conn.execute(sql,params![v["id"].as_str(),company.to_db(),v["charge_tax"].as_bool(),v["default_item_id"].as_str(),v["owed_basis"].as_str(),v["filing_frequency"].as_str(),v["bulk_step"].as_str(),v["revision"].as_i64()])?;
    Ok(())
}
pub fn save_settings(book: &mut Book, actor: UserId, company: CompanyId, input: &Value) -> FbResult<(Value, bool)> {
    book.write(|tx| {
        let user = services::access(tx, actor, company)?;
        if !input["charge_tax"].is_boolean() {
            return Err(FbError::validation("Choose whether this company charges sales tax."));
        }
        if !["ACCRUAL", "CASH"].iter().any(|s| input["owed_basis"] == *s) {
            return Err(FbError::validation("Choose when sales tax is owed."));
        }
        if !["MONTHLY", "QUARTERLY", "ANNUALLY"].iter().any(|s| input["filing_frequency"] == *s) {
            return Err(FbError::validation("Choose a filing frequency."));
        }
        let before = settings_for(tx, company)?;
        documents::check_revision(before["revision"].as_i64().unwrap(), input["expected_revision"].as_i64())?;
        if !input["default_item_id"].is_null() {
            let id = input["default_item_id"]
                .as_str()
                .ok_or_else(|| FbError::validation("Choose a default tax from this company."))?;
            let v = item(tx, company, id).map_err(|_| FbError::validation("Choose a default tax from this company."))?;
            if v["active"] != true && before["default_item_id"] != id {
                return Err(FbError::validation("Choose an active tax rate or group as the default."));
            }
            if v["active"] == true && members(tx, company, &v)?.iter().any(|m| m["active"] != true) {
                return Err(FbError::validation("Every rate in that group must be active."));
            }
        }
        let offer = input["charge_tax"] == true && before["charge_tax"] != true && before["bulk_step"] == "";
        let mut after = before.clone();
        for k in ["charge_tax", "default_item_id", "owed_basis", "filing_frequency"] {
            after[k] = input[k].clone();
        }
        after["id"] = before.get("id").cloned().unwrap_or_else(|| json!(fb_ids::new_v7().to_string()));
        after["revision"] = json!(documents::next_revision(before["revision"].as_i64().unwrap())?);
        persist_settings(tx, company, &after)?;
        services::audit(
            tx,
            &user,
            company,
            "salestax.settings.updated",
            format!("salestaxsettings:{}", after["id"].as_str().unwrap()),
            json!({"before":settings_snapshot(&before),"after":settings_snapshot(&after)}),
        )?;
        Ok((after, offer))
    })
}
pub fn set_item_active(
    book: &mut Book,
    actor: UserId,
    company: CompanyId,
    id: &str,
    active: bool,
    expected: Option<i64>,
) -> FbResult<Value> {
    book.write(|tx|{
        let user=services::access(tx,actor,company)?;let mut v=item(tx,company,id)?;documents::check_revision(v["revision"].as_i64().unwrap(),expected)?;
        if v["active"]==active{return Ok(v);}
        if !active{
            if settings_for(tx,company)?["default_item_id"]==id{return Err(FbError::validation(format!("\"{}\" is the company default sales tax. Choose another default in Sales tax settings first.",v["name"].as_str().unwrap())));}
            if v["kind"]=="RATE"{
                let mut stmt=tx.prepare("SELECT g.name FROM salestax_taxitem g JOIN salestax_taxgroupmember m ON m.group_id=g.id WHERE g.company_id=?1 AND g.active AND m.member_id=?2 ORDER BY g.name COLLATE fb_text")?;
                let groups=stmt.query_map(params![company.to_db(),id],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
                if !groups.is_empty(){return Err(FbError::validation(format!("\"{}\" is part of the active tax group(s) {}. Remove it from those groups or deactivate them first.",v["name"].as_str().unwrap(),groups.join(", "))));}
            }
        }else if v["kind"]=="GROUP"&&members(tx,company,&v)?.iter().any(|m|m["active"]!=true){return Err(FbError::validation("Reactivate every tax rate in this group first."));}
        let before=v.clone();v["active"]=json!(active);v["revision"]=json!(documents::next_revision(v["revision"].as_i64().unwrap())?);persist_item(tx,company,&v)?;
        services::audit(tx,&user,company,&format!("salestax.{}.{}",v["kind"].as_str().unwrap().to_lowercase(),if active{"reactivated"}else{"deactivated"}),format!("taxitem:{id}"),json!({"before":item_snapshot(&before),"after":item_snapshot(&v)}))?;Ok(v)
    })
}
pub fn bulk_preview(conn: &(impl fb_db::Query + ?Sized), company: CompanyId) -> FbResult<Value> {
    let ids = |sql: &str| -> FbResult<Vec<String>> {
        let mut stmt = conn.prepare(sql)?;
        Ok(stmt.query_map([company.to_db()], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?)
    };
    let c = ids("SELECT id FROM directory_customer WHERE company_id=?1 AND tax_status='EXEMPT' ORDER BY id")?;
    let i = ids("SELECT id FROM directory_item WHERE company_id=?1 AND NOT taxable AND kind<>'SERVICE' ORDER BY id")?;
    let v = json!({"customers":c,"items":i});
    let digest = Sha256::digest(py::py_json_dumps_sorted(&v).as_bytes());
    Ok(json!({"customers":c,"items":i,"digest":format!("{digest:x}")}))
}
pub fn finish_bulk_step(book: &mut Book, actor: UserId, company: CompanyId, customers: bool, items: bool, digest: &str) -> FbResult<Value> {
    book.write(|tx| {
        let user = services::access(tx, actor, company)?;
        let mut row = settings_for(tx, company)?;
        if row["charge_tax"] != true || row["bulk_step"] != "" {
            return Err(FbError::validation(
                "The one-time sales tax update is not available. It is offered once, when sales tax is first turned on.",
            ));
        }
        let preview = bulk_preview(tx, company)?;
        if preview["digest"] != digest {
            return Err(FbError::validation(
                "Customers or items changed since this preview. Review the updated list and confirm again.",
            ));
        }
        let mut changed = json!({"customers":[],"items":[]});
        for (flag, key, model, field, value) in [
            (customers, "customers", directory::Model::Customer, "tax_status", json!("TAXABLE")),
            (items, "items", directory::Model::Item, "taxable", json!(true)),
        ] {
            if flag {
                for id in preview[key].as_array().unwrap() {
                    // Bulk writes intentionally bypass stale account validation just as Python obj.save does.
                    let id = id.as_str().unwrap();
                    let before = directory::get(tx, company, model, id)?;
                    tx.execute(
                        &format!("UPDATE {} SET {field}=?1 WHERE id=?2", model.table()),
                        params![
                            if let Some(b) = value.as_bool() {
                                fb_db::rusqlite::types::Value::Integer(i64::from(b))
                            } else {
                                fb_db::rusqlite::types::Value::Text(value.as_str().unwrap().into())
                            },
                            id
                        ],
                    )?;
                    let after = directory::get(tx, company, model, id)?;
                    services::audit(
                        tx,
                        &user,
                        company,
                        &format!("{}.updated", model.name()),
                        format!("{}:{id}", model.name()),
                        json!({"before":before,"after":after,"reason":"Sales tax turned on: one-time update"}),
                    )?;
                }
                changed[key] = preview[key].clone();
            }
        }
        let before = row.clone();
        row["bulk_step"] = json!(if customers || items { "APPLIED" } else { "SKIPPED" });
        row["revision"] = json!(documents::next_revision(row["revision"].as_i64().unwrap())?);
        persist_settings(tx, company, &row)?;
        services::audit(
            tx,
            &user,
            company,
            "salestax.bulk_step",
            format!("salestaxsettings:{}", row["id"].as_str().unwrap()),
            json!({"before":settings_snapshot(&before),"after":settings_snapshot(&row),"customers_marked_taxable":changed["customers"],"items_marked_taxable":changed["items"]}),
        )?;
        Ok(changed)
    })
}

fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}
pub fn item_snapshot(v: &Value) -> Value {
    let mut result = v.clone();
    if v["kind"] == "GROUP" {
        result.as_object_mut().unwrap().remove("rate");
        result.as_object_mut().unwrap().remove("agency_id");
    }
    result
}
fn settings_snapshot(v: &Value) -> Value {
    let mut result = v.clone();
    result.as_object_mut().unwrap().remove("id");
    result
}
pub fn check_document_tax(conn: &(impl fb_db::Query + ?Sized), company: CompanyId, id: Option<&str>) -> FbResult<()> {
    if let Some(id) = id {
        eligible(conn, company, &item(conn, company, id)?)?;
    }
    Ok(())
}
/// Exact retries of record-once sales documents hash the normalized request for ITEM,
/// never the live tax rate/group. Other modes retain the pre-FX11 expanded-data digest.
pub fn creation_digest(request: &Value, expand: impl FnOnce() -> FbResult<Value>) -> FbResult<String> {
    let value = if request["tax_mode"] == "ITEM" {
        if request["tax_item_id"].as_str().and_then(py::record_uuid).is_none() {
            return Err(FbError::validation("Choose a sales tax rate or group from this company."));
        }
        let mut v = request.clone();
        v.as_object_mut()
            .ok_or_else(|| FbError::validation("Choose a sales tax rate or group from this company."))?
            .retain(|k, _| k != "tax_rate" && k != "tax_label");
        v
    } else {
        expand()?
    };
    Ok(format!("{:x}", Sha256::digest(py::py_json_dumps_sorted(&value).as_bytes())))
}
