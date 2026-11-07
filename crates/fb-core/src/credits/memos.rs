use super::*;

fn request(input: &Value) -> FbResult<Value> {
    let rows = input["lines"]
        .as_array()
        .filter(|a| (1..=50).contains(&a.len()))
        .ok_or_else(|| FbError::validation("Enter 1–50 meaningful lines."))?;
    let mut lines = vec![];
    let fields: BTreeSet<_> = [
        "item_id",
        "description",
        "quantity",
        "rate",
        "taxable",
        "income_account_id",
    ]
    .into_iter()
    .collect();
    for row in rows {
        if row
            .as_object()
            .is_none_or(|r| r.keys().map(String::as_str).collect::<BTreeSet<_>>() != fields)
        {
            return Err(FbError::validation("Unexpected line fields."));
        }
        lines.push(json!({"item_id":id(&row["item_id"],true)?,"description":text(&row["description"],1000,true)?,"quantity":calculations::quantity(&row["quantity"])?.to_string(),"rate":calculations::unit_rate(&row["rate"])?.to_string(),"taxable":boolean(&row["taxable"])?,"income_account_id":id(&row["income_account_id"],false)?}));
    }
    Ok(
        json!({"request_key":key(&input["request_key"])?,"customer_id":id(&input["customer_id"],false)?,"date":date(&input["date"])?,"memo":field_text(input,"memo",2000,false)?,"tax_mode":input["tax_mode"],"tax_rate":input["tax_rate"],"tax_label":input.get("tax_label").unwrap_or(&json!("")),"tax_item_id":input["tax_item_id"],"lines":lines}),
    )
}
fn expand(tx: &Writer<'_>, c: CompanyId, req: &Value) -> FbResult<Value> {
    let tax = crate::salestax::document_tax(tx, c, req, None, false)?;
    let calc = calculate(req["lines"].as_array().unwrap(), &tax, true)?;
    let mut data = req.clone();
    copy_calc(&mut data, &calc);
    data["tax_item_id"] = tax["tax_item_id"].clone();
    data["line_amounts"] = calc["lines"].clone();
    Ok(data)
}
fn creation(tx: &Writer<'_>, c: CompanyId, req: &Value) -> FbResult<String> {
    // Picked-tax retry evidence binds the request, never the later live rate/group.
    if !req["tax_item_id"].is_null() {
        id(&req["tax_item_id"], false)?;
        Ok(digest(req))
    } else {
        Ok(digest(&expand(tx, c, req)?))
    }
}
fn prepare(tx: &Writer<'_>, actor: UserId, c: CompanyId, data: &Value) -> FbResult<Value> {
    let customer = directory::get(tx, c, Model::Customer, stored_str(data, "customer_id")?)
        .map_err(|_| FbError::validation("Choose an active customer in this company."))?;
    if customer["active"] != true {
        return Err(FbError::validation(
            "Choose an active customer in this company.",
        ));
    }
    crate::salestax::check_document_tax(tx, c, data["tax_item_id"].as_str())?;
    let lines = data["lines"].as_array().unwrap();
    let mut debits = BTreeMap::<String, i64>::new();
    let mut prepared = vec![];
    let mut ids = vec![];
    for (i, row) in lines.iter().enumerate() {
        let item = row["item_id"]
            .as_str()
            .map(|key| directory::get(tx, c, Model::Item, key))
            .transpose()
            .map_err(|_| FbError::validation("Choose items from this company."))?;
        if item.as_ref().is_some_and(|v| v["active"] != true) {
            return Err(FbError::validation(
                "Every selected item must be active before recording.",
            ));
        }
        let a = account(tx, c, row["income_account_id"].as_str().unwrap())
            .map_err(|_| FbError::validation("Posting accounts must belong to this company."))?;
        if !a.active || !a.postable {
            return Err(FbError::validation(
                "Every posting account must be active and postable.",
            ));
        }
        if a.kind.as_str() != "INCOME" {
            return Err(FbError::validation(
                "Every line requires an active postable INCOME account.",
            ));
        }
        ids.push(a.id);
        let n = cents(&data["line_amounts"][i])?;
        let value = debits.entry(a.id.to_db()).or_default();
        *value = add(*value, n)?;
        let mut l = row.clone();
        l["position"] = json!(i + 1);
        l["amount"] = cash(n);
        l["item_snapshot"] = item.as_ref().map(item_snapshot).unwrap_or(json!({}));
        l["account_snapshot"] = json!({"income_account":account_snapshot(&a)});
        prepared.push(l);
    }
    let roles = if cents(&data["tax"])? > 0 {
        vec!["receivables", "collected_tax"]
    } else {
        vec!["receivables"]
    };
    let setup = documents::setup_snapshot(tx, c)?;
    if setup["receivables"].is_null() {
        return Err(FbError::validation(
            "Configure a receivables (A/R) account in Accounting setup before recording a credit memo.",
        ));
    }
    if cents(&data["tax"])? > 0 && setup["collected_tax"].is_null() {
        return Err(FbError::validation(
            "Configure collected tax in Accounting setup before recording a taxed credit memo.",
        ));
    }
    let mut mapping = documents::lock_configuration(tx, actor, c, &roles, None, &ids)?;
    for k in debits.keys() {
        mapping[format!("income:{k}")] = json!(k);
    }
    let ar = mapping["receivables"].clone();
    let tax_id = mapping["collected_tax"].clone();
    if let Some(t) = tax_id.as_str() {
        let v = debits.entry(t.into()).or_default();
        *v = add(*v, cents(&data["tax"])?)?;
    }
    let mut entries = debits
        .into_iter()
        .filter(|(_, v)| *v != 0)
        .map(|(a, v)| json!({"account":a,"debit":cash(v),"credit":"0.00"}))
        .collect::<Vec<_>>();
    entries.push(json!({"account":ar,"debit":"0.00","credit":data["total"]}));
    let display = mapping_display(tx, c, &mapping)?;
    let party = party(&customer);
    let co = company_snapshot(tx, c)?;
    // Python reviews display and bind the item code. Catalog description and
    // sale-rate metadata are snapped live on record, but are not review fields.
    let line_display=prepared.iter().map(|l|json!({"position":l["position"],"item":l["item_snapshot"]["code"].as_str().unwrap_or(""),"description":l["description"],"quantity":l["quantity"],"rate":l["rate"],"taxable":l["taxable"],"amount":l["amount"],"income_account_id":l["income_account_id"]})).collect::<Vec<_>>();
    let hash = digest(
        &json!({"company":c.to_db(),"intent":data,"lines":line_display,"mapping":display,"party":party,"company_snapshot":co}),
    );
    Ok(
        json!({"prepared":prepared,"lines":line_display,"mapping":mapping,"entries":entries,"customer_snapshot":party,"company_snapshot":co,"receivables_id":ar,"collected_tax_id":tax_id,"digest":hash,"display":display}),
    )
}
pub fn review_credit_memo(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    input: &Value,
) -> FbResult<Review> {
    command(book, |tx| {
        services::access(tx, actor, c)?;
        let data = expand(tx, c, &request(input)?)?;
        let p = prepare(tx, actor, c, &data)?;
        Ok(Review {
            domain: "credit memo",
            digest: p["digest"].as_str().unwrap().into(),
            display: json!({"mapping":p["display"],"entries":p["entries"],"lines":p["lines"],"customer_snapshot":p["customer_snapshot"],"company_snapshot":p["company_snapshot"],"data":data}),
        })
    })
}
pub fn record_credit_memo(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    input: &Value,
    review: Option<&Review>,
) -> FbResult<Value> {
    command(book, |tx| {
        let user = services::access(tx, actor, c)?;
        let req = request(input)?;
        let hash = creation(tx, c, &req)?;
        let reviewed = token(
            review,
            "credit memo",
            "Invalid review. Open a fresh credit memo review.",
        )?;
        let existing:Option<String>=tx.query_row("SELECT m.id FROM credits_creditmemo m JOIN documents_documentidentity d ON d.id=m.identity_id WHERE m.company_id=?1 AND d.request_key=?2",params![c.to_db(),req["request_key"].as_str()],|r|r.get(0)).optional()?;
        if let Some(key) = existing {
            let m = scoped(tx, c, "credits_creditmemo", &key)?;
            if m["creation_digest"] != hash || m["review_digest"] != reviewed {
                return Err(FbError::validation(
                    "This request key was recorded with a different payload or review.",
                ));
            }
            return Ok(m);
        }
        let data = expand(tx, c, &req)?;
        let identity =
            documents::create_identity(tx, actor, c, "CM", req["request_key"].as_str().unwrap())?;
        let p = prepare(tx, actor, c, &data)?;
        if p["digest"] != reviewed {
            return Err(FbError::validation(
                "Credit memo or posting mapping changed. Open a fresh review before recording.",
            ));
        }
        let source = stored_str(&identity, "id")?;
        documents::snapshot_source_accounts(tx, actor, c, source, &p["mapping"])?;
        let summary = py::py_truncate(
            &format!(
                "{}: {}",
                stored_str(&identity, "number")?,
                stored_str(&data, "memo")?
            ),
            240,
        );
        let j = documents::post_source(
            tx,
            actor,
            c,
            source,
            stored_str(&data, "date")?,
            &summary,
            p["entries"].as_array().unwrap().clone(),
        )?;
        let mut m = data.clone();
        m.as_object_mut().unwrap().remove("lines");
        m.as_object_mut().unwrap().remove("line_amounts");
        m.as_object_mut().unwrap().remove("request_key");
        m["id"] = json!(new_id());
        m["identity_id"] = identity["id"].clone();
        m["company_id"] = json!(c.to_db());
        m["creation_digest"] = json!(hash);
        m["review_digest"] = json!(reviewed);
        m["customer_snapshot"] = p["customer_snapshot"].clone();
        m["company_snapshot"] = p["company_snapshot"].clone();
        m["receivables_id"] = p["receivables_id"].clone();
        m["collected_tax_id"] = p["collected_tax_id"].clone();
        m["journal_id"] = json!(j.id.to_db());
        m["actor_id"] = json!(actor.to_db());
        m["actor_name"] = json!(user.username);
        tx.execute("INSERT INTO credits_creditmemo(id,company_id,identity_id,customer_id,date,receivables_id,collected_tax_id,tax_item_id,total,journal_id,actor_id,data) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",params![m["id"].as_str(),c.to_db(),source,m["customer_id"].as_str(),m["date"].as_str(),m["receivables_id"].as_str(),m["collected_tax_id"].as_str(),m["tax_item_id"].as_str(),cents(&m["total"])?,j.id.to_db(),actor.to_db(),m.to_string()])?;
        for l in p["prepared"].as_array().unwrap() {
            let mut l = l.clone();
            l["id"] = json!(new_id());
            l["credit_memo_id"] = m["id"].clone();
            tx.execute("INSERT INTO credits_creditmemoline(id,credit_memo_id,position,item_id,income_account_id,amount,taxable,data) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![l["id"].as_str(),m["id"].as_str(),l["position"].as_i64(),l["item_id"].as_str(),l["income_account_id"].as_str(),cents(&l["amount"])?,l["taxable"].as_bool(),l.to_string()])?;
        }
        seal(tx, "memo", stored_str(&m, "id")?)?;
        seal(tx, "journal", &j.id.to_db())?;
        let ls=p["prepared"].as_array().unwrap().iter().map(|l|json!({"position":l["position"],"income_account_id":l["income_account_id"],"amount":l["amount"]})).collect::<Vec<_>>();
        services::audit(
            tx,
            &user,
            c,
            "credit_memo.recorded",
            format!("creditmemo:{}", stored_str(&m, "id")?),
            json!({"number":identity["number"],"total":m["total"],"memo":m["memo"],"receivables_id":m["receivables_id"],"lines":ls}),
        )?;
        Ok(m)
    })
}
pub fn void_credit_memo(
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
        let m = scoped(tx, c, "credits_creditmemo", key)?;
        let d = date(when)?;
        let reason = text(reason, 500, true)?;
        if let Some(old) = undo(tx, "credits_creditmemovoid", "credit_memo_id", key)? {
            if expected == Some(1) && old["date"] == d && old["reason"] == reason {
                return Ok(old);
            }
            return Err(FbError::validation(
                "Credit memo already voided with a different or stale command.",
            ));
        }
        documents::check_revision(1, expected)?;
        if d.as_str() < stored_str(&m, "date")? {
            return Err(FbError::validation(
                "Void date cannot precede the credit memo date.",
            ));
        }
        if live_applied(tx, key)? > 0 {
            return Err(FbError::validation(
                "Unapply this credit memo from every invoice before voiding it.",
            ));
        }
        let live:Option<String>=tx.query_row("SELECT d.number FROM credits_creditrefund r JOIN documents_documentidentity d ON d.id=r.identity_id WHERE r.credit_memo_id=?1 AND NOT EXISTS(SELECT 1 FROM credits_creditrefundvoid v WHERE v.refund_id=r.id) ORDER BY r.id LIMIT 1",[key],|r|r.get(0)).optional()?;
        if let Some(n) = live {
            return Err(FbError::validation(format!(
                "Void refund {n} before voiding this credit memo."
            )));
        }
        let released:Option<String>=tx.query_row("SELECT max(date) FROM (SELECT u.date FROM credits_creditunapplication u JOIN credits_creditapplication a ON a.id=u.application_id WHERE a.credit_memo_id=?1 UNION ALL SELECT v.date FROM credits_creditrefundvoid v JOIN credits_creditrefund r ON r.id=v.refund_id WHERE r.credit_memo_id=?1)",[key],|r|r.get(0))?;
        if let Some(released) = released.filter(|s| s > &d) {
            return Err(FbError::validation(format!(
                "Void date cannot precede {released}, when this credit was last released by an unapply or refund void."
            )));
        }
        let j =
            documents::reverse_source(tx, actor, c, stored_str(&m, "identity_id")?, &d, &reason)?;
        let v = json!({"id":new_id(),"credit_memo_id":key,"journal_id":j.id.to_db(),"date":d,"reason":reason,"actor_id":actor.to_db(),"actor_name":user.username});
        tx.execute("INSERT INTO credits_creditmemovoid(id,credit_memo_id,journal_id,date,actor_id,data) VALUES(?1,?2,?3,?4,?5,?6)",params![v["id"].as_str(),key,j.id.to_db(),d,actor.to_db(),v.to_string()])?;
        seal(tx, "memo_void", v["id"].as_str().unwrap())?;
        seal(tx, "journal", &j.id.to_db())?;
        services::audit(
            tx,
            &user,
            c,
            "credit_memo.voided",
            format!("creditmemo:{key}"),
            json!({"reason":reason,"date":d,"journal_id":j.id.to_db()}),
        )?;
        Ok(v)
    })
}
