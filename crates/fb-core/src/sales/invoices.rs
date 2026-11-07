use super::*;
use chrono::Days;

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
    let due = if input["due_date"].is_null() {
        py::model_date(&d)
            .map_err(|_| invalid_stored())?
            .checked_add_days(Days::new(30))
            .ok_or_else(|| FbError::validation("Enter a valid calendar date."))?
            .to_string()
    } else {
        date(&input["due_date"])?
    };
    if due < d {
        return Err(FbError::validation(
            "Due date cannot precede the invoice date.",
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
        "income_account_id",
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
        lines.push(json!({"item_id":id(&row["item_id"],true)?,"income_account_id":id(&row["income_account_id"],true)?,"origin_position":origin,"description":py::input_text(&row["description"],1000,true,py::retained_line_text(old.and_then(|o| o["lines"].as_array()), &origin, "description"))?,"quantity":calculations::quantity(&row["quantity"])?.to_string(),"rate":calculations::unit_rate(&row["rate"])?.to_string(),"taxable":boolean(&row["taxable"])?,"refresh_item":boolean(row.get("refresh_item").unwrap_or(&json!(false)))?,"keep_archived":boolean(row.get("keep_archived").unwrap_or(&json!(false)))?}));
    }
    let requested = ["ITEM", "KEEP"].contains(&input["tax_mode"].as_str().unwrap_or(""));
    let preliminary = if requested {
        calculate(&lines, &json!({"tax_mode":"NONE"}), false)?
    } else {
        calculate(&lines, input, false)?
    };
    let mut intent = json!({"customer_id":customer,"date":d,"due_date":due,"memo":py::input_text(input.get("memo").unwrap_or(&json!("")),2000,false,previous.and_then(|p| p["memo"].as_str()))?,"lines":lines,"tax_mode":preliminary["tax_mode"],"tax_rate":preliminary["tax_rate"],"tax_label":preliminary["tax_label"],"refresh_customer":boolean(input.get("refresh_customer").unwrap_or(&json!(false)))?,"keep_archived":boolean(input.get("keep_archived").unwrap_or(&json!(false)))?});
    if requested {
        intent["tax_mode"] = input["tax_mode"].clone();
        intent["tax_rate"] = Value::Null;
        intent["tax_label"] = json!("");
        intent["tax_item_id"] = input["tax_item_id"].clone()
    }
    Ok(intent)
}

pub fn save_invoice(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    input: &Value,
) -> FbResult<Value> {
    command(book, |tx| {
        let user = services::access(tx, actor, c)?;
        let editing_id = id(&input["invoice_id"], true)?;
        let editing = editing_id.as_deref();
        let old = editing.map(|key| invoice(tx, c, key)).transpose();
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
                    "Only Draft invoices can be edited. Posted or voided financial history is immutable.",
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
                "INV",
                input["request_key"].as_str().unwrap_or(""),
            )?;
            let source = identity["id"]
                .as_str()
                .ok_or_else(invalid_stored)?
                .to_string();
            let existing: Option<(String, String)> = tx
                .query_row(
                    "SELECT id,creation_digest FROM sales_invoice WHERE identity_id=?1",
                    [&source],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((key, prior)) = existing {
                if hash != prior {
                    return Err(FbError::validation(
                        "This retry key belongs to a different invoice payload. Start a new invoice instead.",
                    ));
                }
                return invoice(tx, c, &key);
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
                ("income_account", row["income_account_id"].clone()),
                ("expense_account", expense.clone()),
            ] {
                if let Some(key) = key.as_str() {
                    let a = account(tx, c, key).map_err(|_| {
                        FbError::validation("Item account defaults must belong to this company.")
                    })?;
                    if role == "income_account"
                        && (!a.active || !a.postable || a.kind.as_str() != "INCOME")
                        && prior.is_none_or(|p| p["income_account_id"] != key)
                    {
                        return Err(FbError::validation(
                            "Choose an active postable income account, or clear it for later correction.",
                        ));
                    }
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
            lines.push(json!({"position":i+1,"origin_position":origin,"origin_revision_id":if origin.is_some(){old.as_ref().map(|o|o["current"]["id"].clone()).unwrap_or(Value::Null)}else{Value::Null},"item_id":row["item_id"],"item_snapshot":retained.map(|p|p["item_snapshot"].clone()).unwrap_or_else(||item.as_ref().map(item_snapshot).unwrap_or(json!({}))),"description":row["description"],"quantity":row["quantity"],"rate":row["rate"],"taxable":row["taxable"],"amount":calc["lines"][i],"income_account_id":row["income_account_id"],"expense_account_id":expense,"account_snapshot":snapshots}));
        }
        let mut current = json!({"customer_id":customer_id,"customer_snapshot":if let Some(o)=&old{if o["current"]["customer_id"]==customer_id&&normalized["refresh_customer"]!=true{o["current"]["customer_snapshot"].clone()}else{party(&customer)}}else{party(&customer)},"company_snapshot":old.as_ref().map(|o|o["current"]["company_snapshot"].clone()).unwrap_or(company_snapshot(tx,c)?),"date":normalized["date"],"due_date":normalized["due_date"],"memo":normalized["memo"],"state":"DRAFT","action":if old.is_some(){"revised"}else{"created"},"reason":if normalized["refresh_customer"]==true||normalized["lines"].as_array().ok_or_else(invalid_stored)?.iter().any(|l|l["refresh_item"]==true){"Explicit snapshot refresh"}else{""},"tax_item_id":tax["tax_item_id"],"journal_id":null,"reversal_id":null,"review_digest":""});
        copy_calc(&mut current, &calc);
        if old.is_none() {
            tx.execute("INSERT INTO sales_invoice(id,company_id,identity_id,creation_digest,revision) VALUES(?1,?2,?3,?4,1)",params![key,c.to_db(),source,hash])?;
        } else {
            tx.execute(
                "UPDATE sales_invoice SET revision=?2 WHERE id=?1",
                params![key, revision],
            )?;
        }
        append(tx, &user, c, &key, revision, &mut current, &lines)?;
        invoice(tx, c, &key)
    })
}
pub(super) fn append(
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
    tx.execute("INSERT INTO sales_invoicerevision(id,invoice_id,number,customer_id,date,due_date,state,total,journal_id,reversal_id,data,actor_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",params![rid,key,revision,header["customer_id"].as_str(),header["date"].as_str(),header["due_date"].as_str(),header["state"].as_str(),cents(&header["total"])?,header["journal_id"].as_str(),header["reversal_id"].as_str(),header.to_string(),user.id.to_db()])?;
    for l in lines {
        tx.execute("INSERT INTO sales_invoiceline(id,revision_id,position,item_id,income_account_id,expense_account_id,amount,taxable,data) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![new_id(),rid,l["position"].as_i64(),l["item_id"].as_str(),l["income_account_id"].as_str(),l["expense_account_id"].as_str(),cents(&l["amount"])?,l["taxable"].as_bool(),l.to_string()])?;
    }
    seal(tx, "sales_invoice_revision", &rid)?;
    let source: String = tx.query_row(
        "SELECT identity_id FROM sales_invoice WHERE id=?1",
        [key],
        |r| r.get(0),
    )?;
    let identity = documents::identity(tx, c, &source)?;
    services::audit(
        tx,
        user,
        c,
        &format!(
            "invoice.{}",
            header["action"].as_str().ok_or_else(invalid_stored)?
        ),
        format!("invoice:{key}"),
        json!({"revision":revision,"previous_revision":if revision==1{None}else{Some(revision-1)},"state":header["state"],"reason":header["reason"],"number":identity["number"],"total":header["total"]}),
    )?;
    Ok(())
}
fn posting(
    tx: &Writer<'_>,
    actor: UserId,
    c: CompanyId,
    inv: &Value,
) -> FbResult<(Value, Vec<Value>, String)> {
    let h = &inv["current"];
    if h["state"] != "DRAFT" {
        return Err(FbError::validation(
            "Only a draft invoice can be reviewed or posted. Voided invoices cannot be reposted.",
        ));
    }
    let lines = inv["lines"].as_array().ok_or_else(invalid_stored)?;
    let calc = calculate(lines, h, true)?;
    if ["subtotal", "taxable", "tax", "total", "tax_snapshot"]
        .iter()
        .any(|k| calc[*k] != h[*k])
    {
        return Err(FbError::validation(
            "Stored invoice arithmetic does not match its lines.",
        ));
    }
    let customer = directory::get(
        tx,
        c,
        Model::Customer,
        h["customer_id"].as_str().ok_or_else(invalid_stored)?,
    )?;
    let mut active = customer["active"] == true;
    let mut ids = Vec::new();
    for l in lines {
        if let Some(key) = l["item_id"].as_str() {
            active &= directory::get(tx, c, Model::Item, key)?["active"] == true
        }
        for f in ["income_account_id", "expense_account_id"] {
            if let Some(key) = l[f].as_str() {
                account(tx, c, key)?;
            }
        }
        ids.push(l["income_account_id"].as_str().and_then(AccountId::parse));
    }
    if !active {
        return Err(FbError::validation(
            "Customer and every selected item must be active before posting.",
        ));
    }
    if ids.iter().any(Option::is_none) {
        return Err(FbError::validation(
            "Choose an explicit income account on every line before posting.",
        ));
    }
    let ids = ids.into_iter().flatten().collect::<Vec<_>>();
    let roles = if cents(&calc["tax"])? > 0 {
        vec!["receivables", "collected_tax"]
    } else {
        vec!["receivables"]
    };
    let mut mapping = documents::lock_configuration(tx, actor, c, &roles, None, &ids)?;
    let mut credits = BTreeMap::new();
    for l in lines {
        let key = l["income_account_id"].as_str().ok_or_else(invalid_stored)?;
        let a = account(tx, c, key)?;
        if a.kind.as_str() != "INCOME" {
            return Err(FbError::validation(
                "Every line requires an active postable INCOME account.",
            ));
        }
        mapping[format!("income:{}", l["position"])] = json!(key);
        *credits.entry(key.to_string()).or_insert(0) += cents(&l["amount"])?;
    }
    if roles.len() > 1 {
        *credits
            .entry(
                mapping["collected_tax"]
                    .as_str()
                    .ok_or_else(invalid_stored)?
                    .into(),
            )
            .or_insert(0) += cents(&calc["tax"])?;
    }
    let es = entries(
        mapping["receivables"].as_str().ok_or_else(invalid_stored)?,
        cents(&calc["total"])?,
        credits,
    );
    let display = mapping_display(tx, c, &mapping)?;
    let hash = digest(
        &json!({"invoice":inv["id"],"company":c.to_db(),"revision":inv["revision"],"mapping":display,"entries":es,"total":calc["total"]}),
    );
    Ok((mapping, es, hash))
}
pub fn review_invoice(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    key: &str,
    expected: Option<i64>,
) -> FbResult<Review> {
    command(book, |tx| {
        services::access(tx, actor, c)?;
        let inv = invoice(tx, c, key)?;
        documents::check_revision(
            inv["revision"].as_i64().ok_or_else(invalid_stored)?,
            expected,
        )?;
        let (m, es, hash) = posting(tx, actor, c, &inv)?;
        Ok(Review {
            domain: "invoice",
            digest: hash,
            display: json!({"mapping":mapping_display(tx,c,&m)?,"entries":es}),
        })
    })
}
pub fn post_invoice(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    key: &str,
    expected: Option<i64>,
    review: Option<&Review>,
) -> FbResult<Value> {
    command(book, |tx| {
        let user = services::access(tx, actor, c)?;
        let inv = invoice(tx, c, key)?;
        let reviewed = token(
            review,
            "invoice",
            "Invalid review. Open a new posting review.",
        )?;
        let rev = inv["revision"].as_i64().ok_or_else(invalid_stored)?;
        let h = &inv["current"];
        if h["state"] == "OPEN" && expected == Some(rev - 1) && h["review_digest"] == reviewed {
            return Ok(inv);
        }
        documents::check_revision(rev, expected)?;
        let next_revision = documents::next_revision(rev)?;
        let (mapping, es, hash) = posting(tx, actor, c, &inv)?;
        if reviewed != hash {
            return Err(FbError::validation(
                "Posting mapping changed. Open a new review before posting.",
            ));
        }
        let source = inv["identity_id"].as_str().ok_or_else(invalid_stored)?;
        documents::snapshot_source_accounts(tx, actor, c, source, &mapping)?;
        let identity = documents::identity(tx, c, source)?;
        let summary = py::py_truncate(
            &format!(
                "{}: {}",
                identity["number"].as_str().ok_or_else(invalid_stored)?,
                h["memo"].as_str().ok_or_else(invalid_stored)?
            ),
            240,
        );
        let j = documents::post_source(
            tx,
            actor,
            c,
            source,
            h["date"].as_str().ok_or_else(invalid_stored)?,
            &summary,
            es,
        )?;
        let mut next = h.clone();
        next["state"] = json!("OPEN");
        next["action"] = json!("posted");
        next["reason"] = json!("");
        next["journal_id"] = json!(j.id.to_db());
        next["review_digest"] = json!(hash);
        tx.execute(
            "UPDATE sales_invoice SET revision=?1 WHERE id=?2",
            params![next_revision, key],
        )?;
        append(
            tx,
            &user,
            c,
            key,
            next_revision,
            &mut next,
            inv["lines"].as_array().ok_or_else(invalid_stored)?,
        )?;
        finish_journal(tx, &j.id.to_db())?;
        invoice(tx, c, key)
    })
}
pub fn void_invoice(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    key: &str,
    expected: Option<i64>,
    when: &Value,
    reason: &Value,
) -> FbResult<Value> {
    command(book, |tx| {
        let user = services::access(tx, actor, c)?;
        let inv = invoice(tx, c, key)?;
        let d = date(when)?;
        let reason = text(reason, 500, true)?;
        let rev = inv["revision"].as_i64().ok_or_else(invalid_stored)?;
        let h = &inv["current"];
        if h["state"] == "VOID" && expected == Some(rev - 1) && h["reason"] == reason {
            let jd: String = tx.query_row(
                "SELECT date FROM ledger_journal WHERE id=?1",
                [h["reversal_id"].as_str().ok_or_else(invalid_stored)?],
                |r| r.get(0),
            )?;
            if d == jd {
                return Ok(inv);
            }
        }
        documents::check_revision(rev, expected)?;
        let next_revision = documents::next_revision(rev)?;
        if h["state"] != "OPEN" {
            return Err(FbError::validation(
                "Only an open posted invoice can be voided.",
            ));
        }
        if d.as_str() < h["date"].as_str().ok_or_else(invalid_stored)? {
            return Err(FbError::validation(
                "Reversal date cannot precede the invoice date.",
            ));
        }
        if queries::allocated(tx, key)? > 0 {
            return Err(FbError::validation(
                "Reverse every live receipt allocated to this invoice before voiding it.",
            ));
        }
        queries::refuse_external_invoice_dependencies(tx, key, &d)?;
        let source = inv["identity_id"].as_str().ok_or_else(invalid_stored)?;
        let j = documents::reverse_source(tx, actor, c, source, &d, &reason)?;
        let mut next = h.clone();
        next["state"] = json!("VOID");
        next["action"] = json!("voided");
        next["reason"] = json!(reason);
        next["reversal_id"] = json!(j.id.to_db());
        tx.execute(
            "UPDATE sales_invoice SET revision=?1 WHERE id=?2",
            params![next_revision, key],
        )?;
        append(
            tx,
            &user,
            c,
            key,
            next_revision,
            &mut next,
            inv["lines"].as_array().ok_or_else(invalid_stored)?,
        )?;
        finish_journal(tx, &j.id.to_db())?;
        invoice(tx, c, key)
    })
}
