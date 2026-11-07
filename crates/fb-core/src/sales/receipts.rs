use super::*;

fn request(input: &Value, sale: bool) -> FbResult<Value> {
    let key = input["request_key"]
        .as_str()
        .and_then(py::py_uuid)
        .ok_or_else(|| FbError::validation("Supply a stable UUID request key."))?
        .0;
    let method = input["method"]
        .as_str()
        .filter(|m| ["CHECK", "CASH", "CARD", "TRANSFER", "OTHER"].contains(m))
        .ok_or_else(|| FbError::validation("Choose a payment method (record only)."))?;
    let dest = input["destination"]
        .as_str()
        .filter(|m| ["BANK", "UNDEPOSITED"].contains(m))
        .ok_or_else(|| FbError::validation("Choose a receipt destination."))?;
    let bank = id(&input["bank_id"], dest == "UNDEPOSITED")?;
    if dest == "UNDEPOSITED" && bank.is_some() {
        return Err(FbError::validation(
            "Do not select a bank for undeposited funds.",
        ));
    }
    let mut req = json!({"request_key":key,"customer_id":id(&input["customer_id"],false)?,"date":date(&input["date"])?,"method":method,"reference":crate::card_numbers::card_free(field_text(input,"reference",120,false)?,"Reference")?,"memo":crate::card_numbers::card_free(field_text(input,"memo",2000,false)?,"Memo")?,"destination":dest,"bank_id":bank});
    if sale {
        let rows = input["lines"]
            .as_array()
            .filter(|a| (1..=50).contains(&a.len()))
            .ok_or_else(|| FbError::validation("Enter 1–50 meaningful lines."))?;
        let allowed = [
            "item_id",
            "description",
            "quantity",
            "rate",
            "taxable",
            "income_account_id",
        ];
        let mut ls = Vec::new();
        for row in rows {
            if row.as_object().is_none_or(|r| {
                r.len() != allowed.len() || r.keys().any(|k| !allowed.contains(&k.as_str()))
            }) {
                return Err(FbError::validation("Unexpected line fields."));
            }
            ls.push(json!({"item_id":id(&row["item_id"],true)?,"description":field_text(row,"description",1000,true)?,"quantity":calculations::quantity(&row["quantity"])?.to_string(),"rate":calculations::unit_rate(&row["rate"])?.to_string(),"taxable":boolean(&row["taxable"])?,"income_account_id":id(&row["income_account_id"],false)?}));
        }
        req["lines"] = json!(ls);
        for k in ["tax_mode", "tax_rate", "tax_label", "tax_item_id"] {
            req[k] = input.get(k).cloned().unwrap_or(if k == "tax_label" {
                json!("")
            } else {
                Value::Null
            })
        }
    } else {
        let rows = input["allocations"]
            .as_array()
            .filter(|a| (1..=50).contains(&a.len()))
            .ok_or_else(|| FbError::validation("Enter 1–50 explicit allocations."))?;
        let mut ls = Vec::new();
        let mut seen = BTreeSet::new();
        let mut sum = 0i64;
        for row in rows {
            if row.as_object().is_none_or(|r| {
                r.len() != 2 || !r.contains_key("invoice_id") || !r.contains_key("amount")
            }) {
                return Err(FbError::validation("Unexpected allocation fields."));
            }
            let key = id(&row["invoice_id"], false)?.ok_or_else(invalid_stored)?;
            let n = positive(&row["amount"])?;
            if !seen.insert(key.clone()) {
                return Err(FbError::validation("Allocate to each invoice only once."));
            }
            sum=sum.checked_add(n).ok_or_else(||FbError::validation("Explicit allocations must equal the receipt amount exactly; no on-account credit."))?;
            ls.push(json!({"invoice_id":key,"amount":cash(n)}));
        }
        ls.sort_by(|a, b| a["invoice_id"].as_str().cmp(&b["invoice_id"].as_str()));
        let amount = positive(&input["amount"])?;
        if sum != amount {
            return Err(FbError::validation(
                "Explicit allocations must equal the receipt amount exactly; no on-account credit.",
            ));
        }
        req["amount"] = cash(amount);
        req["allocations"] = json!(ls);
    }
    Ok(req)
}
fn positive(v: &Value) -> FbResult<i64> {
    let n = cents(v)?;
    if n <= 0 {
        return Err(FbError::validation(
            "Receipt and allocation amounts must be positive.",
        ));
    }
    Ok(n)
}
fn expand(tx: &Writer<'_>, c: CompanyId, req: &Value) -> FbResult<Value> {
    let tax = crate::salestax::document_tax(tx, c, req, None, false)?;
    let calc = calculate(
        req["lines"].as_array().ok_or_else(invalid_stored)?,
        &tax,
        true,
    )?;
    let mut out = req.clone();
    copy_calc(&mut out, &calc);
    out["tax_item_id"] = tax["tax_item_id"].clone();
    out["line_amounts"] = calc["lines"].clone();
    Ok(out)
}
fn prepare(
    tx: &Writer<'_>,
    actor: UserId,
    c: CompanyId,
    req: &Value,
    sale: bool,
) -> FbResult<Value> {
    let data = if sale {
        expand(tx, c, req)?
    } else {
        req.clone()
    };
    let mut credits = BTreeMap::new();
    let mut allocations = Vec::new();
    let mut prepared = Vec::new();
    if !sale {
        for row in data["allocations"].as_array().ok_or_else(invalid_stored)? {
            let key = row["invoice_id"].as_str().ok_or_else(invalid_stored)?;
            let inv = invoice(tx, c, key)
                .map_err(|_| FbError::validation("Choose invoices belonging to this company."))?;
            let h = &inv["current"];
            if h["customer_id"] != data["customer_id"] || h["state"] != "OPEN" {
                return Err(FbError::validation(
                    "Allocate only to this customer’s posted, unreversed invoices.",
                ));
            }
            if h["date"].as_str().ok_or_else(invalid_stored)?
                > data["date"].as_str().ok_or_else(invalid_stored)?
            {
                return Err(FbError::validation(
                    "Payment date cannot precede any allocated invoice date.",
                ));
            }
            if cents(&row["amount"])? > cents(&h["total"])? - queries::settled(tx, key)? {
                return Err(FbError::validation(
                    "Allocation exceeds the actual outstanding invoice amount. Review again.",
                ));
            }
            let ar:Option<String>=tx.query_row("SELECT account_id FROM documents_documentaccount WHERE source_id=?1 AND role='receivables'",[inv["identity_id"].as_str().ok_or_else(invalid_stored)?],|r|r.get(0)).optional()?;
            let ar = ar.ok_or_else(|| {
                FbError::validation("Invoice has no original receivables posting reference.")
            })?;
            *credits.entry(ar.clone()).or_insert(0) += cents(&row["amount"])?;
            allocations.push(json!({"invoice_id":key,"invoice_revision_id":h["id"],"receivables_id":ar,"amount":row["amount"]}));
        }
    }
    let customer = directory::get(
        tx,
        c,
        Model::Customer,
        data["customer_id"].as_str().ok_or_else(invalid_stored)?,
    )
    .map_err(|_| FbError::validation("Choose an active customer in this company."))?;
    if customer["active"] != true {
        return Err(FbError::validation(
            "Choose an active customer in this company.",
        ));
    }
    if sale {
        for (i, row) in data["lines"]
            .as_array()
            .ok_or_else(invalid_stored)?
            .iter()
            .enumerate()
        {
            let item = row["item_id"]
                .as_str()
                .map(|key| {
                    directory::get(tx, c, Model::Item, key)
                        .map_err(|_| FbError::validation("Choose items from this company."))
                })
                .transpose()?;
            if item.as_ref().is_some_and(|v| v["active"] != true) {
                return Err(FbError::validation(
                    "Every selected item must be active before recording.",
                ));
            }
            let key = row["income_account_id"]
                .as_str()
                .ok_or_else(invalid_stored)?;
            *credits.entry(key.to_string()).or_insert(0) += cents(&data["line_amounts"][i])?;
            let a = account(tx, c, key).map_err(|_| {
                FbError::validation("Posting accounts must belong to this company.")
            })?;
            let mut line = row.clone();
            line["position"] = json!(i + 1);
            line["amount"] = data["line_amounts"][i].clone();
            line["item_snapshot"] = item.as_ref().map(item_snapshot).unwrap_or(json!({}));
            line["account_snapshot"] = json!({"income_account":account_snapshot(&a)});
            prepared.push(line);
        }
        crate::salestax::check_document_tax(tx, c, data["tax_item_id"].as_str())?;
    }
    let setup = documents::setup_snapshot(tx, c)?;
    let dest = if data["destination"] == "BANK" {
        data["bank_id"].as_str()
    } else {
        setup["undeposited"].as_str()
    }
    .ok_or_else(|| {
        FbError::validation("Configure an explicit undeposited funds destination first.")
    })?;
    let tax_id = if sale && cents(&data["tax"])? > 0 {
        Some(setup["collected_tax"].as_str().ok_or_else(|| {
            FbError::validation(
                "Configure collected tax in Accounting setup before recording a taxed sale.",
            )
        })?)
    } else {
        None
    };
    let dest_account = account(tx, c, dest)
        .map_err(|_| FbError::validation("Posting accounts must belong to this company."))?;
    let mut actual = credits.keys().cloned().collect::<BTreeSet<_>>();
    actual.insert(dest.into());
    if let Some(t) = tax_id {
        actual.insert(t.into());
    }
    let accounts = actual
        .iter()
        .map(|key| {
            Ok((
                key.clone(),
                account(tx, c, key).map_err(|_| {
                    FbError::validation("Posting accounts must belong to this company.")
                })?,
            ))
        })
        .collect::<FbResult<BTreeMap<_, _>>>()?;
    if !sale {
        if accounts
            .values()
            .any(|a| a.kind.as_str() != "ASSET" || !a.active || !a.postable)
        {
            return Err(FbError::validation(
                "Actual receipt accounts, including original receivables, must be active postable ASSET accounts.",
            ));
        }
        if credits.contains_key(dest) {
            return Err(FbError::validation(
                "Destination must differ from the invoices’ original receivables accounts.",
            ));
        }
    } else {
        if accounts.values().any(|a| !a.active || !a.postable) {
            return Err(FbError::validation(
                "Every posting account must be active and postable.",
            ));
        }
        if credits
            .keys()
            .any(|key| accounts[key].kind.as_str() != "INCOME")
        {
            return Err(FbError::validation(
                "Every line requires an active postable INCOME account.",
            ));
        }
        if dest_account.kind.as_str() != "ASSET" {
            return Err(FbError::validation(
                "The destination must be an active postable ASSET account.",
            ));
        }
        if tax_id.is_some_and(|t| accounts[t].kind.as_str() != "LIABILITY") {
            return Err(FbError::validation(
                "Collected tax must be an active postable LIABILITY account.",
            ));
        }
    }
    let mut roles = Vec::new();
    if data["destination"] == "UNDEPOSITED" {
        roles.push("undeposited")
    }
    if tax_id.is_some() {
        roles.push("collected_tax")
    }
    documents::lock_configuration(
        tx,
        actor,
        c,
        &roles,
        data["bank_id"].as_str().and_then(AccountId::parse),
        &credits
            .keys()
            .filter_map(|k| AccountId::parse(k))
            .collect::<Vec<_>>(),
    )?;
    let mut mapping = json!({"destination":dest});
    for key in credits.keys() {
        mapping[format!("{}:{key}", if sale { "income" } else { "receivables" })] = json!(key)
    }
    if let Some(t) = tax_id {
        mapping["collected_tax"] = json!(t);
        *credits.entry(t.into()).or_insert(0) += cents(&data["tax"])?;
    }
    let total = cents(&data[if sale { "total" } else { "amount" }])?;
    let es = entries(dest, total, credits);
    let display = mapping_display(tx, c, &mapping)?;
    let party = party(&customer);
    let co = company_snapshot(tx, c)?;
    // Python binds the displayed sale lines, not unshown live catalog metadata.
    let displayed=prepared.iter().map(|l|json!({"position":l["position"],"item":l["item_snapshot"].get("code").cloned().unwrap_or(json!("")),"description":l["description"],"quantity":l["quantity"],"rate":l["rate"],"taxable":l["taxable"],"amount":l["amount"],"income_account_id":l["income_account_id"]})).collect::<Vec<_>>();
    let hash = digest(
        &json!({"company":c.to_db(),"intent":data,"lines":displayed,"allocations":allocations,"mapping":display,"party":party,"company_snapshot":co}),
    );
    Ok(
        json!({"data":data,"mapping":mapping,"display":display,"entries":es,"prepared":prepared,"allocations":allocations,"customer_snapshot":party,"company_snapshot":co,"digest":hash,"destination_account_id":dest,"collected_tax_id":tax_id}),
    )
}
fn review(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    input: &Value,
    sale: bool,
) -> FbResult<Review> {
    command(book, |tx| {
        services::access(tx, actor, c)?;
        let req = request(input, sale)?;
        let p = prepare(tx, actor, c, &req, sale)?;
        Ok(Review {
            domain: if sale { "sales_receipt" } else { "receipt" },
            digest: p["digest"].as_str().ok_or_else(invalid_stored)?.into(),
            display: json!({"mapping":p["display"],"entries":p["entries"]}),
        })
    })
}
pub fn review_receipt(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    input: &Value,
) -> FbResult<Review> {
    review(book, actor, c, input, false)
}
pub fn review_sales_receipt(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    input: &Value,
) -> FbResult<Review> {
    review(book, actor, c, input, true)
}

/// Presentation projection of the same locked review plan used by the receipt services.
/// The opaque capability is still checked against the fresh plan by record_receipt/record_sales_receipt.
pub fn review_receipt_form(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    input: &Value,
    sale: bool,
) -> FbResult<(Review, Value)> {
    command(book, |tx| {
        services::access(tx, actor, c)?;
        let req = request(input, sale)?;
        let p = prepare(tx, actor, c, &req, sale)?;
        let cap = Review {
            domain: if sale { "sales_receipt" } else { "receipt" },
            digest: p["digest"].as_str().ok_or_else(invalid_stored)?.into(),
            display: json!({"mapping":p["display"],"entries":p["entries"]}),
        };
        let lines=p["prepared"].as_array().ok_or_else(invalid_stored)?.iter().map(|l|json!({"position":l["position"],"item":l["item_snapshot"].get("code").cloned().unwrap_or(json!("")),"description":l["description"],"quantity":l["quantity"],"rate":l["rate"],"taxable":l["taxable"],"amount":l["amount"],"income_account_id":l["income_account_id"]})).collect::<Vec<_>>();
        let label = match p["data"]["method"].as_str().ok_or_else(invalid_stored)? {
            "CHECK" => "Check",
            "CASH" => "Cash",
            "CARD" => "Card (record only)",
            "TRANSFER" => "Transfer (record only)",
            _ => "Other",
        };
        let mut allocations = p["allocations"]
            .as_array()
            .ok_or_else(invalid_stored)?
            .clone();
        for row in &mut allocations {
            let number:String=tx.query_row("SELECT d.number FROM sales_invoice i JOIN documents_documentidentity d ON d.id=i.identity_id WHERE i.id=?1",[row["invoice_id"].as_str().ok_or_else(invalid_stored)?],|r|r.get(0))?;
            row["number"] = json!(number);
        }
        let vm = json!({"data":p["data"],"company_snapshot":p["company_snapshot"],"customer_snapshot":p["customer_snapshot"],"mapping":p["display"],"entries":p["entries"],"allocations":allocations,"lines":lines,"method_label":label});
        Ok((cap, vm))
    })
}
fn record(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    input: &Value,
    review: Option<&Review>,
    sale: bool,
) -> FbResult<Value> {
    command(book, |tx| {
        let user = services::access(tx, actor, c)?;
        let req = request(input, sale)?;
        let hash = if sale {
            crate::salestax::creation_digest(&req, || expand(tx, c, &req))?
        } else {
            digest(&req)
        };
        let domain = if sale { "sales_receipt" } else { "receipt" };
        let error = if sale {
            "Invalid review. Open a fresh sales receipt review."
        } else {
            "Invalid review. Open a fresh receipt review."
        };
        let reviewed = token(review, domain, error)?;
        let key = req["request_key"].as_str().ok_or_else(invalid_stored)?;
        let existing:Option<String>=tx.query_row("SELECT r.id FROM sales_receipt r JOIN documents_documentidentity d ON d.id=r.identity_id WHERE r.company_id=?1 AND d.request_key=?2",params![c.to_db(),key],|r|r.get(0)).optional()?;
        if let Some(key) = existing {
            let old = receipt(tx, c, &key)?;
            if old["kind"] != if sale { "SR" } else { "RCP" }
                || old["creation_digest"] != hash
                || old["review_digest"] != reviewed
            {
                return Err(FbError::validation(
                    "This request key was recorded with a different payload or review.",
                ));
            }
            return Ok(old);
        }
        let source =
            documents::create_identity(tx, actor, c, if sale { "SR" } else { "RCP" }, key)?;
        let p = prepare(tx, actor, c, &req, sale)?;
        if p["digest"] != reviewed {
            return Err(FbError::validation(if sale {
                "Sales receipt or posting mapping changed. Open a fresh review before recording."
            } else {
                "Receipt or posting mapping changed. Open a fresh review before recording."
            }));
        }
        let sid = source["id"].as_str().ok_or_else(invalid_stored)?;
        documents::snapshot_source_accounts(tx, actor, c, sid, &p["mapping"])?;
        let summary = py::py_truncate(
            &format!(
                "{}: {}",
                source["number"].as_str().ok_or_else(invalid_stored)?,
                req["memo"].as_str().ok_or_else(invalid_stored)?
            ),
            240,
        );
        let j = documents::post_source(
            tx,
            actor,
            c,
            sid,
            req["date"].as_str().ok_or_else(invalid_stored)?,
            &summary,
            p["entries"].as_array().ok_or_else(invalid_stored)?.clone(),
        )?;
        let key = new_id();
        let mut out = json!({"id":key,"identity_id":sid,"kind":if sale{"SR"}else{"RCP"},"creation_digest":hash,"review_digest":reviewed,"customer_snapshot":p["customer_snapshot"],"company_snapshot":p["company_snapshot"],"destination_account_id":p["destination_account_id"],"journal_id":j.id.to_db(),"actor_name":user.username});
        for k in [
            "customer_id",
            "date",
            "method",
            "reference",
            "memo",
            "destination",
        ] {
            out[k] = req[k].clone()
        }
        out["amount"] = p["data"][if sale { "total" } else { "amount" }].clone();
        tx.execute("INSERT INTO sales_receipt(id,company_id,identity_id,kind,customer_id,date,amount,destination,destination_account_id,journal_id,data,actor_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",params![key,c.to_db(),sid,out["kind"].as_str(),out["customer_id"].as_str(),out["date"].as_str(),cents(&out["amount"])?,out["destination"].as_str(),out["destination_account_id"].as_str(),j.id.to_db(),out.to_string(),actor.to_db()])?;
        if sale {
            let mut detail = json!({"collected_tax_id":p["collected_tax_id"],"tax_item_id":p["data"]["tax_item_id"]});
            copy_calc(&mut detail, &p["data"]);
            let saleid = new_id();
            tx.execute(
                "INSERT INTO sales_salesreceipt(id,receipt_id,total,data) VALUES(?1,?2,?3,?4)",
                params![saleid, key, cents(&detail["total"])?, detail.to_string()],
            )?;
            for line in p["prepared"].as_array().ok_or_else(invalid_stored)? {
                tx.execute("INSERT INTO sales_salesreceiptline(id,sale_id,position,item_id,income_account_id,amount,taxable,data) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![new_id(),saleid,line["position"].as_i64(),line["item_id"].as_str(),line["income_account_id"].as_str(),cents(&line["amount"])?,line["taxable"].as_bool(),line.to_string()])?;
            }
        } else {
            for a in p["allocations"].as_array().ok_or_else(invalid_stored)? {
                tx.execute("INSERT INTO sales_receiptallocation(id,receipt_id,invoice_id,invoice_revision_id,receivables_id,amount) VALUES(?1,?2,?3,?4,?5,?6)",params![new_id(),key,a["invoice_id"].as_str(),a["invoice_revision_id"].as_str(),a["receivables_id"].as_str(),cents(&a["amount"]) ?])?;
            }
        }
        seal(tx, "sales_receipt", &key)?;
        finish_journal(tx, &j.id.to_db())?;
        services::audit(
            tx,
            &user,
            c,
            if sale {
                "sales_receipt.recorded"
            } else {
                "receipt.recorded"
            },
            format!("receipt:{key}"),
            if sale {
                json!({"number":source["number"],"amount":out["amount"],"memo":out["memo"],"destination":out["destination"],"lines":p["prepared"].as_array().ok_or_else(invalid_stored)?.iter().map(|l|json!({"position":l["position"],"income_account_id":l["income_account_id"],"amount":l["amount"]})).collect::<Vec<_>>()})
            } else {
                json!({"number":source["number"],"amount":out["amount"],"memo":out["memo"],"allocations":req["allocations"]})
            },
        )?;
        Ok(out)
    })
}
pub fn record_receipt(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    input: &Value,
    review: Option<&Review>,
) -> FbResult<Value> {
    record(book, actor, c, input, review, false)
}
pub fn record_sales_receipt(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    input: &Value,
    review: Option<&Review>,
) -> FbResult<Value> {
    record(book, actor, c, input, review, true)
}
fn reverse(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    key: &str,
    expected: Option<i64>,
    change: (&Value, &Value),
    sale: bool,
) -> FbResult<Value> {
    command(book, |tx| {
        let user = services::access(tx, actor, c)?;
        let old = receipt(tx, c, key)?;
        if old["kind"] != if sale { "SR" } else { "RCP" } {
            return Err(FbError::missing("Receipt"));
        }
        let d = date(change.0)?;
        let reason = text(change.1, 500, true)?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT data FROM sales_receiptreversal WHERE receipt_id=?1",
                [key],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(existing) = existing {
            let existing: Value = fb_db::parse_unique_json_as(&existing)
                .map_err(|_| FbError::validation("Invalid stored reversal."))?;
            let ok: bool = tx.query_row(
                "SELECT fb_sales_reversal_ok(?1)",
                [existing.to_string()],
                |r| r.get(0),
            )?;
            if !ok {
                return Err(invalid_stored());
            }
            if expected == Some(1) && existing["date"] == d && existing["reason"] == reason {
                return Ok(existing);
            }
            return Err(FbError::validation(if sale {
                "Sales receipt already voided with a different or stale command."
            } else {
                "Receipt already reversed with a different or stale command."
            }));
        }
        documents::check_revision(1, expected)?;
        if d.as_str() < old["date"].as_str().ok_or_else(invalid_stored)? {
            return Err(FbError::validation(if sale {
                "Void date cannot precede the sale date."
            } else {
                "Reversal date cannot precede payment date."
            }));
        }
        queries::refuse_live_deposit(tx, key, &d)?;
        directory::get(
            tx,
            c,
            Model::Customer,
            old["customer_id"].as_str().ok_or_else(invalid_stored)?,
        )?;
        let j = documents::reverse_source(
            tx,
            actor,
            c,
            old["identity_id"].as_str().ok_or_else(invalid_stored)?,
            &d,
            &reason,
        )?;
        let rid = new_id();
        let out = json!({"id":rid,"receipt_id":key,"journal_id":j.id.to_db(),"date":d,"reason":reason,"actor_name":user.username});
        tx.execute("INSERT INTO sales_receiptreversal(id,receipt_id,journal_id,date,data,actor_id) VALUES(?1,?2,?3,?4,?5,?6)",params![rid,key,j.id.to_db(),d,out.to_string(),actor.to_db()])?;
        seal(tx, "sales_receipt_reversal", &rid)?;
        finish_journal(tx, &j.id.to_db())?;
        services::audit(
            tx,
            &user,
            c,
            if sale {
                "sales_receipt.voided"
            } else {
                "receipt.reversed"
            },
            format!("receipt:{key}"),
            json!({"reason":reason,"date":d,"journal_id":j.id.to_db()}),
        )?;
        Ok(out)
    })
}
pub fn reverse_receipt(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    key: &str,
    expected: Option<i64>,
    date: &Value,
    reason: &Value,
) -> FbResult<Value> {
    reverse(book, actor, c, key, expected, (date, reason), false)
}
pub fn void_sales_receipt(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    key: &str,
    expected: Option<i64>,
    date: &Value,
    reason: &Value,
) -> FbResult<Value> {
    reverse(book, actor, c, key, expected, (date, reason), true)
}
