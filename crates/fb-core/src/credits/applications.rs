use super::*;

fn normalize(input: &Value) -> FbResult<Value> {
    let rows = input["allocations"]
        .as_array()
        .filter(|a| (1..=50).contains(&a.len()))
        .ok_or_else(|| FbError::validation("Apply credit to 1–50 invoices."))?;
    let mut lines = vec![];
    let mut total = 0;
    for row in rows {
        if row.as_object().is_none_or(|m| {
            m.len() != 2 || !m.contains_key("invoice_id") || !m.contains_key("amount")
        }) {
            return Err(FbError::validation("Unexpected allocation fields."));
        }
        let amount = cents(&row["amount"])?;
        if amount <= 0 {
            return Err(FbError::validation("Credit amounts must be positive."));
        }
        total = add(total, amount)?;
        lines.push(json!({"invoice_id":id(&row["invoice_id"],false)?,"amount":cash(amount)}));
    }
    lines.sort_by(|a, b| a["invoice_id"].as_str().cmp(&b["invoice_id"].as_str()));
    if lines
        .windows(2)
        .any(|w| w[0]["invoice_id"] == w[1]["invoice_id"])
    {
        return Err(FbError::validation(
            "Apply credit to each invoice only once.",
        ));
    }
    if total > 9_999_999_999_999_999 {
        return Err(FbError::validation(
            "Document amount exceeds 99999999999999.99.",
        ));
    }
    Ok(
        json!({"request_key":key(&input["request_key"])?,"date":date(&input["date"])?,"allocations":lines,"amount":cash(total)}),
    )
}
fn prepare(tx: &Writer<'_>, c: CompanyId, m: &Value, data: &Value) -> FbResult<Value> {
    let key = stored_str(m, "id")?;
    if undo(tx, "credits_creditmemovoid", "credit_memo_id", key)?.is_some() {
        return Err(FbError::validation(
            "A voided credit memo has no credit to apply.",
        ));
    }
    let d = stored_str(data, "date")?;
    if d < stored_str(m, "date")? {
        return Err(FbError::validation(
            "The application date cannot precede the credit memo date.",
        ));
    }
    let mut rows = vec![];
    let mut prepared = vec![];
    for row in data["allocations"].as_array().unwrap() {
        let inv = crate::sales::invoice(tx, c, row["invoice_id"].as_str().unwrap())
            .map_err(|_| FbError::validation("Choose invoices from this company."))?;
        let h = &inv["current"];
        let source = documents::identity(tx, c, stored_str(&inv, "identity_id")?)?;
        let number = stored_str(&source, "number")?;
        if h["customer_id"] != m["customer_id"] || h["state"] != "OPEN" {
            return Err(FbError::validation(format!(
                "Apply credit only to this customer’s posted, unvoided invoices ({number} is not one)."
            )));
        }
        if stored_str(h, "date")? > d {
            return Err(FbError::validation(format!(
                "The application date cannot precede invoice {number}’s date."
            )));
        }
        let ar:Option<(String,String,String)>=tx.query_row("SELECT account_id,account_code,account_name FROM documents_documentaccount WHERE source_id=?1 AND role='receivables'",[stored_str(&inv, "identity_id")?],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let (ar, code, name) = ar.ok_or_else(|| {
            FbError::validation("Invoice has no original receivables posting reference.")
        })?;
        if m["receivables_id"] != ar {
            return Err(FbError::validation(format!(
                "Invoice {number} was posted to receivables account {code} · {name}, but this credit memo credited a different receivables account. Apply credit only to invoices on the same receivables account."
            )));
        }
        let paid = sum(
            tx,
            "SELECT a.amount FROM sales_receiptallocation a WHERE a.invoice_id=?1 AND NOT EXISTS(SELECT 1 FROM sales_receiptreversal u WHERE u.receipt_id=a.receipt_id)",
            stored_str(&inv, "id")?,
        )?;
        let credit = invoice_credits(tx, stored_str(&inv, "id")?)?;
        let deposited = if has(tx, "prepayments_depositapplication")? {
            sum(
                tx,
                "SELECT a.amount FROM prepayments_depositapplication a WHERE a.invoice_id=?1 AND NOT EXISTS(SELECT 1 FROM prepayments_depositunapplication u WHERE u.application_id=a.id)",
                stored_str(&inv, "id")?,
            )?
        } else {
            0
        };
        let outstanding = cents(&h["total"])? - add(add(paid, credit)?, deposited)?;
        let amount = cents(&row["amount"])?;
        if amount > outstanding {
            return Err(FbError::validation(format!(
                "Credit applied to {number} exceeds its current open balance {}. Review again.",
                cash(outstanding).as_str().unwrap()
            )));
        }
        rows.push(json!({"invoice_id":inv["id"],"number":number,"revision":inv["revision"],"receivables_id":ar,"outstanding":cash(outstanding),"amount":row["amount"],"after":cash(outstanding-amount)}));
        prepared.push(json!({"invoice_id":inv["id"],"invoice_revision_id":h["id"],"receivables_id":ar,"amount":row["amount"]}));
    }
    let available = available_credit(tx, m)?;
    if cents(&data["amount"])? > available {
        return Err(FbError::validation(format!(
            "Credit to apply ({}) exceeds the available credit {}.",
            stored_str(data, "amount")?,
            cash(available).as_str().unwrap()
        )));
    }
    let hash = digest(
        &json!({"company":c.to_db(),"credit_memo":key,"intent":data,"rows":rows,"available":cash(available)}),
    );
    Ok(
        json!({"prepared":prepared,"rows":rows,"available":cash(available),"after":cash(available-cents(&data["amount"])?),"digest":hash}),
    )
}
pub fn review_application(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    memo: &str,
    input: &Value,
) -> FbResult<Review> {
    command(book, |tx| {
        services::access(tx, actor, c)?;
        let data = normalize(input)?;
        let m = scoped(tx, c, "credits_creditmemo", memo)?;
        let p = prepare(tx, c, &m, &data)?;
        Ok(Review {
            domain: "credit application",
            digest: p["digest"].as_str().unwrap().into(),
            display: json!({"rows":p["rows"],"data":data,"available":p["available"],"after":p["after"]}),
        })
    })
}
pub fn apply_credit(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    memo: &str,
    input: &Value,
    review: Option<&Review>,
) -> FbResult<Value> {
    command(book, |tx| {
        let user = services::access(tx, actor, c)?;
        let data = normalize(input)?;
        let hash = digest(&data);
        let reviewed = token(
            review,
            "credit application",
            "Invalid review. Open a fresh credit application review.",
        )?;
        let m = scoped(tx, c, "credits_creditmemo", memo)?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT id FROM credits_creditapplication WHERE company_id=?1 AND request_key=?2",
                params![c.to_db(), data["request_key"].as_str()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(key) = existing {
            let a = scoped(tx, c, "credits_creditapplication", &key)?;
            if a["credit_memo_id"] != memo
                || a["creation_digest"] != hash
                || a["review_digest"] != reviewed
            {
                return Err(FbError::validation(
                    "This request key was recorded with a different payload or review.",
                ));
            }
            return Ok(a);
        }
        let p = prepare(tx, c, &m, &data)?;
        if p["digest"] != reviewed {
            return Err(FbError::validation(
                "Invoice balances or available credit changed. Open a fresh review before applying.",
            ));
        }
        let a = json!({"id":new_id(),"company_id":c.to_db(),"credit_memo_id":memo,"request_key":data["request_key"],"creation_digest":hash,"review_digest":reviewed,"date":data["date"],"amount":data["amount"],"actor_id":actor.to_db(),"actor_name":user.username});
        tx.execute("INSERT INTO credits_creditapplication(id,company_id,credit_memo_id,request_key,date,amount,actor_id,data) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![a["id"].as_str(),c.to_db(),memo,a["request_key"].as_str(),a["date"].as_str(),cents(&a["amount"])?,actor.to_db(),a.to_string()])?;
        for row in p["prepared"].as_array().unwrap() {
            let mut l = row.clone();
            l["id"] = json!(new_id());
            l["application_id"] = a["id"].clone();
            tx.execute("INSERT INTO credits_creditapplicationline(id,application_id,invoice_id,invoice_revision_id,receivables_id,amount,data) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![l["id"].as_str(),a["id"].as_str(),l["invoice_id"].as_str(),l["invoice_revision_id"].as_str(),l["receivables_id"].as_str(),cents(&l["amount"])?,l.to_string()])?;
        }
        seal(tx, "application", stored_str(&a, "id")?)?;
        let identity = documents::identity(tx, c, stored_str(&m, "identity_id")?)?;
        let rows = p["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(
                |r| json!({"invoice_id":r["invoice_id"],"number":r["number"],"amount":r["amount"]}),
            )
            .collect::<Vec<_>>();
        services::audit(
            tx,
            &user,
            c,
            "credit_memo.applied",
            format!("creditmemo:{memo}"),
            json!({"number":identity["number"],"application_id":a["id"],"date":a["date"],"amount":a["amount"],"invoices":rows}),
        )?;
        Ok(a)
    })
}
pub fn unapply_credit(
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
        let a = scoped(tx, c, "credits_creditapplication", key)?;
        let d = date(when)?;
        let reason = text(reason, 500, true)?;
        if let Some(u) = undo(tx, "credits_creditunapplication", "application_id", key)? {
            if expected == Some(1) && u["date"] == d && u["reason"] == reason {
                return Ok(u);
            }
            return Err(FbError::validation(
                "Credit already unapplied with a different or stale command.",
            ));
        }
        documents::check_revision(1, expected)?;
        if d.as_str() < stored_str(&a, "date")? {
            return Err(FbError::validation(
                "The unapply date cannot precede the application date.",
            ));
        }
        let u = json!({"id":new_id(),"application_id":key,"date":d,"reason":reason,"actor_id":actor.to_db(),"actor_name":user.username});
        tx.execute("INSERT INTO credits_creditunapplication(id,application_id,date,actor_id,data) VALUES(?1,?2,?3,?4,?5)",params![u["id"].as_str(),key,d,actor.to_db(),u.to_string()])?;
        seal(tx, "unapplication", u["id"].as_str().unwrap())?;
        let m = scoped(
            tx,
            c,
            "credits_creditmemo",
            stored_str(&a, "credit_memo_id")?,
        )?;
        let identity = documents::identity(tx, c, stored_str(&m, "identity_id")?)?;
        services::audit(
            tx,
            &user,
            c,
            "credit_memo.unapplied",
            format!("creditmemo:{}", stored_str(&a, "credit_memo_id")?),
            json!({"number":identity["number"],"application_id":key,"date":d,"reason":reason}),
        )?;
        Ok(u)
    })
}
