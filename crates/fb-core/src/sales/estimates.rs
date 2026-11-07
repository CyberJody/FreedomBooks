use super::*;

pub fn estimate(tx: &(impl Query + ?Sized), c: CompanyId, key: &str) -> FbResult<Value> {
    let (source,revision,hash):(String,i64,String)=tx.query_row("SELECT identity_id,revision,creation_digest FROM sales_estimate WHERE company_id=?1 AND id=?2",params![c.to_db(),key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?.ok_or_else(||FbError::missing("Estimate"))?;
    let raw: String = tx.query_row(
        "SELECT data FROM sales_estimaterevision WHERE estimate_id=?1 AND number=?2",
        params![key, revision],
        |r| r.get(0),
    )?;
    let current: Value = fb_db::parse_unique_json_as(&raw).map_err(|_| invalid_stored())?;
    let valid: bool = tx.query_row(
        "SELECT fb_sales_header_ok('invoice',?1) AND json_extract(?1,'$.company_snapshot.id')=?2",
        params![raw, c.to_db()],
        |r| r.get(0),
    )?;
    if !valid {
        return Err(invalid_stored());
    }
    let mut st =
        tx.prepare("SELECT data FROM sales_estimateline WHERE revision_id=?1 ORDER BY position")?;
    let lines = st
        .query_map([current["id"].as_str().ok_or_else(invalid_stored)?], |r| {
            r.get::<_, String>(0)
        })?
        .map(|raw| fb_db::parse_unique_json_as::<Value>(&raw?).map_err(|_| invalid_stored()))
        .collect::<FbResult<Vec<_>>>()?;
    for line in &lines {
        let valid: bool = tx.query_row("SELECT fb_sales_line_ok(?1)", [line.to_string()], |r| {
            r.get(0)
        })?;
        if !valid {
            return Err(invalid_stored());
        }
    }
    Ok(
        json!({"id":key,"identity_id":source,"revision":revision,"creation_digest":hash,"current":current,"lines":lines}),
    )
}

fn append(
    tx: &Writer<'_>,
    user: &crate::store::User,
    c: CompanyId,
    key: &str,
    revision: i64,
    header: &mut Value,
    lines: &[Value],
) -> FbResult<()> {
    let rid = new_id();
    header["id"] = json!(rid);
    header["number"] = json!(revision);
    header["actor_name"] = json!(user.username);
    tx.execute("INSERT INTO sales_estimaterevision(id,estimate_id,number,customer_id,date,expiry,state,total,data,actor_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![rid,key,revision,header["customer_id"].as_str(),header["date"].as_str(),header["expiry"].as_str(),header["state"].as_str(),cents(&header["total"])?,header.to_string(),user.id.to_db()])?;
    for l in lines {
        tx.execute("INSERT INTO sales_estimateline(id,revision_id,position,item_id,income_account_id,expense_account_id,amount,taxable,data) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![new_id(),rid,l["position"].as_i64(),l["item_id"].as_str(),l["income_account_id"].as_str(),l["expense_account_id"].as_str(),cents(&l["amount"])?,l["taxable"].as_bool(),l.to_string()])?;
    }
    tx.execute("INSERT INTO estimate_seal VALUES('revision',?1)", [&rid])?;
    tx.execute(
        "INSERT INTO estimate_seal VALUES('head',?1)",
        [format!("{key}:{revision}")],
    )?;
    let number:String=tx.query_row("SELECT i.number FROM documents_documentidentity i JOIN sales_estimate e ON e.identity_id=i.id WHERE e.id=?1",[key],|r|r.get(0))?;
    services::audit(
        tx,
        user,
        c,
        &format!(
            "estimate.{}",
            header["action"].as_str().ok_or_else(invalid_stored)?
        ),
        format!("estimate:{key}"),
        json!({"revision":revision,"previous_revision":if revision==1{None}else{Some(revision-1)},"state":header["state"],"reason":header["reason"],"number":number,"total":header["total"]}),
    )?;
    Ok(())
}

pub fn transition_estimate(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    key: &str,
    state: &str,
    expected: Option<i64>,
    reason: &str,
) -> FbResult<Value> {
    command(book, |tx| {
        let user = services::access(tx, actor, c)?;
        let old = estimate(tx, c, key)?;
        let revision = old["revision"].as_i64().ok_or_else(invalid_stored)?;
        documents::check_revision(revision, expected)?;
        let allowed = match old["current"]["state"]
            .as_str()
            .ok_or_else(invalid_stored)?
        {
            "DRAFT" => ["ACCEPTED", "DECLINED", "CANCELLED"].contains(&state),
            "ACCEPTED" | "DECLINED" => ["DRAFT", "CANCELLED"].contains(&state),
            _ => false,
        };
        if !allowed {
            return Err(FbError::validation(
                "This transition is not available. Reload and review the current state.",
            ));
        }
        let reason = text(&json!(reason), 500, state == "CANCELLED")?;
        let mut h = old["current"].clone();
        h["state"] = json!(state);
        h["action"] = json!(if state == "DRAFT" {
            "reopened".into()
        } else {
            state.to_lowercase()
        });
        h["reason"] = json!(reason);
        let next = documents::next_revision(revision)?;
        tx.execute(
            "UPDATE sales_estimate SET revision=?2 WHERE id=?1",
            params![key, next],
        )?;
        append(
            tx,
            &user,
            c,
            key,
            next,
            &mut h,
            old["lines"].as_array().ok_or_else(invalid_stored)?,
        )?;
        estimate(tx, c, key)
    })
}

pub fn convert_estimate(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    key: &str,
    expected: Option<i64>,
    when: &str,
    due: &str,
) -> FbResult<Value> {
    command(book, |tx| {
        let user = services::access(tx, actor, c)?;
        let when = date(&json!(when))?;
        let due = date(&json!(due))?;
        if due < when {
            return Err(FbError::validation("Due date cannot precede invoice date."));
        }
        let old = estimate(tx, c, key)?;
        let hash = digest(&json!({"estimate":key,"revision":expected,"date":when,"due_date":due}));
        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT invoice_id,digest FROM sales_estimateconversion WHERE estimate_id=?1",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((id, prior)) = existing {
            if prior != hash {
                return Err(FbError::validation(
                    "This estimate already has an invoice conversion. Open its linked invoice.",
                ));
            }
            return invoice(tx, c, &id);
        }
        documents::check_revision(
            old["revision"].as_i64().ok_or_else(invalid_stored)?,
            expected,
        )?;
        if old["current"]["state"] != "ACCEPTED" {
            return Err(FbError::validation(
                "Convert the explicitly selected current accepted revision only.",
            ));
        }
        let identity = documents::create_identity(tx, actor, c, "INV", &new_id())?;
        let id = new_id();
        tx.execute("INSERT INTO sales_invoice(id,company_id,identity_id,creation_digest,revision) VALUES(?1,?2,?3,?4,1)",params![id,c.to_db(),identity["id"].as_str(),hash])?;
        let mut h = old["current"].clone();
        h.as_object_mut()
            .ok_or_else(invalid_stored)?
            .remove("expiry");
        h["date"] = json!(when);
        h["due_date"] = json!(due);
        h["state"] = json!("DRAFT");
        h["action"] = json!("created");
        h["reason"] = json!(format!(
            "Converted accepted estimate revision {}",
            old["revision"]
        ));
        h["journal_id"] = Value::Null;
        h["reversal_id"] = Value::Null;
        h["review_digest"] = json!("");
        let mut lines = old["lines"].as_array().ok_or_else(invalid_stored)?.clone();
        for l in &mut lines {
            l["origin_position"] = Value::Null;
            l["origin_revision_id"] = Value::Null;
        }
        super::invoices::append(tx, &user, c, &id, 1, &mut h, &lines)?;
        let link = new_id();
        tx.execute(
            "INSERT INTO sales_estimateconversion VALUES(?1,?2,?3,?4,?5)",
            params![link, key, old["current"]["id"].as_str(), id, hash],
        )?;
        services::audit(
            tx,
            &user,
            c,
            "invoice.converted",
            format!("invoice:{id}"),
            json!({"estimate_id":key,"estimate_revision":old["revision"],"conversion_id":link}),
        )?;
        invoice(tx, c, &id)
    })
}
fn selection(v: &Value, previous: Option<&str>, keep: bool, refresh: bool) -> FbResult<()> {
    if v["active"] != true && (v["id"].as_str() != previous || !keep || refresh) {
        return Err(FbError::validation(
            "Archived selection: replace it, or explicitly retain its historical snapshot. Reactivate in the directory before refreshing.",
        ));
    }
    Ok(())
}
fn normalize(input: &Value, editing: bool, old: Option<&Value>) -> FbResult<Value> {
    let previous = old.map(|o| &o["current"]);
    if let Some(raw) = input["tax_label"].as_str() {
        py::clean_changed_free_text(raw, previous.and_then(|p| p["tax_label"].as_str()))?;
    }
    let customer = id(&input["customer_id"], false)?.ok_or_else(invalid_stored)?;
    let d = date(&input["date"])?;
    let expiry = if input["expiry"].is_null() {
        None
    } else {
        Some(date(&input["expiry"])?)
    };
    if expiry.as_ref().is_some_and(|expiry| expiry < &d) {
        return Err(FbError::validation(
            "Expiry cannot precede the estimate date.",
        ));
    }
    let rows = input["lines"]
        .as_array()
        .filter(|a| (1..=50).contains(&a.len()))
        .ok_or_else(|| FbError::validation("Enter between 1 and 50 meaningful lines."))?;
    let allowed = [
        "item_id",
        "description",
        "quantity",
        "rate",
        "taxable",
        "refresh_item",
        "keep_archived",
        "origin_position",
    ];
    let mut lines = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        if row
            .as_object()
            .is_none_or(|r| r.keys().any(|k| !allowed.contains(&k.as_str())))
        {
            return Err(FbError::validation("Unexpected line fields."));
        }
        let origin = row.get("origin_position").cloned().unwrap_or(if editing {
            json!(i + 1)
        } else {
            Value::Null
        });
        if !origin.is_null() && origin.as_i64().is_none_or(|n| n <= 0) {
            return Err(FbError::validation("Choose a record from this company."));
        }
        lines.push(json!({"item_id":id(&row["item_id"],true)?,"origin_position":origin,"description":py::input_text(&row["description"],1000,true,py::retained_line_text(old.and_then(|o| o["lines"].as_array()), &origin, "description"))?,"quantity":calculations::quantity(&row["quantity"])?.to_string(),"rate":calculations::unit_rate(&row["rate"])?.to_string(),"taxable":boolean(&row["taxable"])?,"refresh_item":boolean(row.get("refresh_item").unwrap_or(&json!(false)))?,"keep_archived":boolean(row.get("keep_archived").unwrap_or(&json!(false)))?}));
    }
    let requested = ["ITEM", "KEEP"].contains(&input["tax_mode"].as_str().unwrap_or(""));
    let preliminary = if requested {
        calculate(&lines, &json!({"tax_mode":"NONE"}), false)?
    } else {
        calculate(&lines, input, false)?
    };
    let mut intent = json!({"customer_id":customer,"date":d,"expiry":expiry,"memo":py::input_text(input.get("memo").unwrap_or(&json!("")),2000,false,previous.and_then(|p| p["memo"].as_str()))?,"lines":lines,"tax_mode":preliminary["tax_mode"],"tax_rate":preliminary["tax_rate"],"tax_label":preliminary["tax_label"],"refresh_customer":boolean(input.get("refresh_customer").unwrap_or(&json!(false)))?,"keep_archived":boolean(input.get("keep_archived").unwrap_or(&json!(false)))?});
    if requested {
        intent["tax_mode"] = input["tax_mode"].clone();
        intent["tax_rate"] = Value::Null;
        intent["tax_label"] = json!("");
        intent["tax_item_id"] = input["tax_item_id"].clone()
    }
    Ok(intent)
}

pub fn save_estimate(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    input: &Value,
) -> FbResult<Value> {
    command(book, |tx| {
        let user = services::access(tx, actor, c)?;
        let editing_id = id(&input["estimate_id"], true)?;
        let editing = editing_id.as_deref();
        let old = editing.map(|key| estimate(tx, c, key)).transpose();
        let normalized = normalize(input, editing.is_some(), old.as_ref().ok().and_then(|v| v.as_ref()))?;
        let hash = digest(&normalized);
        let old = old?;
        let (key, source, revision) = if let Some(old) = &old {
            documents::check_revision(
                old["revision"].as_i64().ok_or_else(invalid_stored)?,
                input["expected_revision"].as_i64(),
            )?;
            if old["current"]["state"] != "DRAFT" {
                return Err(FbError::validation(
                    "Reopen the estimate before revising. Cancelled estimates cannot be revised.",
                ));
            }
            (
                old["id"].as_str().ok_or_else(invalid_stored)?.to_string(),
                old["identity_id"]
                    .as_str()
                    .ok_or_else(invalid_stored)?
                    .to_string(),
                documents::next_revision(old["revision"].as_i64().ok_or_else(invalid_stored)?)?,
            )
        } else {
            documents::check_revision(
                0,
                input.get("expected_revision").unwrap_or(&json!(0)).as_i64(),
            )?;
            let identity = documents::create_identity(
                tx,
                actor,
                c,
                "EST",
                input["request_key"].as_str().unwrap_or(""),
            )?;
            let source = identity["id"]
                .as_str()
                .ok_or_else(invalid_stored)?
                .to_string();
            let existing: Option<(String, String)> = tx
                .query_row(
                    "SELECT id,creation_digest FROM sales_estimate WHERE identity_id=?1",
                    [&source],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((key, prior)) = existing {
                if hash != prior {
                    return Err(FbError::validation(
                        "This retry key belongs to a different estimate payload. Start a new estimate instead.",
                    ));
                }
                return estimate(tx, c, &key);
            }
            (new_id(), source, 1)
        };
        let customer_id = normalized["customer_id"]
            .as_str()
            .ok_or_else(invalid_stored)?;
        let customer = directory::get(tx, c, Model::Customer, customer_id)
            .map_err(|_| FbError::validation("Choose a record from this company."))?;
        selection(
            &customer,
            old.as_ref()
                .and_then(|o| o["current"]["customer_id"].as_str()),
            normalized["keep_archived"] == true,
            normalized["refresh_customer"] == true,
        )?;
        let previous = old.as_ref().map(|o| &o["current"]);
        let tax = crate::salestax::document_tax(tx, c, input, previous, true)?;
        let calc = calculate(
            normalized["lines"].as_array().ok_or_else(invalid_stored)?,
            &tax,
            false,
        )?;
        let old_lines = old.as_ref().and_then(|o| o["lines"].as_array());
        let mut seen = BTreeSet::new();
        let mut lines = Vec::new();
        for (i, row) in normalized["lines"]
            .as_array()
            .ok_or_else(invalid_stored)?
            .iter()
            .enumerate()
        {
            let origin = row["origin_position"].as_u64();
            let prior = origin.and_then(|p| {
                old_lines.and_then(|ls| ls.iter().find(|l| l["position"].as_u64() == Some(p)))
            });
            if origin.is_some() && (!seen.insert(origin) || prior.is_none()) {
                return Err(FbError::validation(
                    "Invalid or duplicated retained line identity. Reload and review.",
                ));
            }
            let item = row["item_id"]
                .as_str()
                .map(|key| {
                    directory::get(tx, c, Model::Item, key)
                        .map_err(|_| FbError::validation("Choose items from this company."))
                })
                .transpose()?;
            if let Some(item) = &item {
                selection(
                    item,
                    prior.and_then(|p| p["item_id"].as_str()),
                    row["keep_archived"] == true,
                    row["refresh_item"] == true,
                )?
            }
            let retained =
                prior.filter(|p| p["item_id"] == row["item_id"] && row["refresh_item"] != true);
            let income = if let Some(p) = retained {
                p["income_account_id"].clone()
            } else {
                item.as_ref()
                    .map(|v| v["income_account"].clone())
                    .unwrap_or(Value::Null)
            };
            let expense = if let Some(p) = retained {
                p["expense_account_id"].clone()
            } else {
                item.as_ref()
                    .map(|v| v["expense_account"].clone())
                    .unwrap_or(Value::Null)
            };
            let mut snapshots = retained
                .map(|p| p["account_snapshot"].clone())
                .unwrap_or(json!({}));
            for (role, key) in [
                ("income_account", income.clone()),
                ("expense_account", expense.clone()),
            ] {
                if let Some(key) = key.as_str() {
                    let a = account(tx, c, key).map_err(|_| {
                        FbError::validation("Item account defaults must belong to this company.")
                    })?;
                    if retained.is_none()
                        || (role == "income_account"
                            && prior.is_none_or(|p| p["income_account_id"] != key))
                    {
                        snapshots[role] = account_snapshot(&a)
                    }
                } else if role == "income_account" {
                    snapshots
                        .as_object_mut()
                        .ok_or_else(invalid_stored)?
                        .remove(role);
                }
            }
            lines.push(json!({"position":i+1,"origin_position":origin,"origin_revision_id":if origin.is_some(){old.as_ref().map(|o|o["current"]["id"].clone()).unwrap_or(Value::Null)}else{Value::Null},"item_id":row["item_id"],"item_snapshot":retained.map(|p|p["item_snapshot"].clone()).unwrap_or_else(||item.as_ref().map(item_snapshot).unwrap_or(json!({}))),"description":row["description"],"quantity":row["quantity"],"rate":row["rate"],"taxable":row["taxable"],"amount":calc["lines"][i],"income_account_id":income,"expense_account_id":expense,"account_snapshot":snapshots}));
        }
        let mut current = json!({"customer_id":customer_id,"customer_snapshot":if let Some(o)=&old{if o["current"]["customer_id"]==customer_id&&normalized["refresh_customer"]!=true{o["current"]["customer_snapshot"].clone()}else{party(&customer)}}else{party(&customer)},"company_snapshot":old.as_ref().map(|o|o["current"]["company_snapshot"].clone()).unwrap_or(company_snapshot(tx,c)?),"date":normalized["date"],"expiry":normalized["expiry"],"memo":normalized["memo"],"state":"DRAFT","action":if old.is_some(){"revised"}else{"created"},"reason":if normalized["refresh_customer"]==true||normalized["lines"].as_array().ok_or_else(invalid_stored)?.iter().any(|l|l["refresh_item"]==true){"Explicit snapshot refresh"}else{""},"tax_item_id":tax["tax_item_id"]});
        copy_calc(&mut current, &calc);
        if old.is_none() {
            tx.execute("INSERT INTO sales_estimate(id,company_id,identity_id,creation_digest,revision) VALUES(?1,?2,?3,?4,1)",params![key,c.to_db(),source,hash])?;
        } else {
            tx.execute(
                "UPDATE sales_estimate SET revision=?2 WHERE id=?1",
                params![key, revision],
            )?;
        }
        append(tx, &user, c, &key, revision, &mut current, &lines)?;
        estimate(tx, c, &key)
    })
}
