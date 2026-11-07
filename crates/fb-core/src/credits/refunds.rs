use super::*;
fn number_text(v: &Value) -> FbResult<String> {
    let raw = v.as_str().ok_or_else(|| FbError::validation("Enter a check number up to 20 characters."))?;
    fb_db::check_numbers::normalize(raw).map(str::to_owned).map_err(FbError::validation)
}

fn normalize(v: &Value) -> FbResult<Value> {
    let amount = cents(&v["amount"])?;
    if amount <= 0 {
        return Err(FbError::validation("Enter a positive refund amount."));
    }
    let method = v["method"]
        .as_str()
        .filter(|s| ["CHECK", "CASH", "OTHER"].contains(s))
        .ok_or_else(|| FbError::validation("Choose a refund method (record only)."))?;
    let number = number_text(v.get("check_number").unwrap_or(&json!("")))?;
    if method != "CHECK" && !number.is_empty() {
        return Err(FbError::validation(
            "Only a refund by check carries a check number.",
        ));
    }
    Ok(
        json!({"request_key":key(&v["request_key"])?,"date":date(&v["date"])?,"amount":cash(amount),"bank_id":id(&v["bank_id"],false)?,"method":method,"check_number":number,"memo":crate::card_numbers::card_free(field_text(v,"memo",2000,false)?,"Memo")?}),
    )
}
/// Cross-module number holders are read under the owning book's IMMEDIATE writer.
/// The matching SQL triggers protect callers of the payables and future checks APIs too.
pub fn refuse_number_conflict(tx: &Writer<'_>, c: CompanyId, bank: &str, n: &str) -> FbResult<()> {
    crate::check_numbers::refuse_conflict(tx, c, bank, n, None)
}
fn prepare(
    tx: &Writer<'_>,
    actor: UserId,
    c: CompanyId,
    m: &Value,
    data: &Value,
) -> FbResult<Value> {
    let memo = stored_str(m, "id")?;
    if undo(tx, "credits_creditmemovoid", "credit_memo_id", memo)?.is_some() {
        return Err(FbError::validation(
            "A voided credit memo has no credit to refund.",
        ));
    }
    if data["date"].as_str() < m["date"].as_str() {
        return Err(FbError::validation(
            "The refund date cannot precede the credit memo date.",
        ));
    }
    let available = available_credit(tx, m)?;
    if cents(&data["amount"])? > available {
        return Err(FbError::validation(format!(
            "Refund {} exceeds the available credit {}.",
            stored_str(data, "amount")?,
            cash(available).as_str().unwrap()
        )));
    }
    let bank = stored_str(data, "bank_id")?;
    refuse_number_conflict(tx, c, bank, stored_str(data, "check_number")?)?;
    let customer = directory::get(tx, c, Model::Customer, stored_str(m, "customer_id")?)
        .map_err(|_| FbError::validation("Historical customer must belong to this company."))?;
    let invalid_bank = || {
        FbError::validation(
            "Choose a designated bank account in this company, separate from receivables.",
        )
    };
    let ar = account(tx, c, stored_str(m, "receivables_id")?).map_err(|_| invalid_bank())?;
    let b = account(tx, c, bank).map_err(|_| invalid_bank())?;
    if ar.kind.as_str() != "ASSET"
        || b.kind.as_str() != "ASSET"
        || !ar.active
        || !ar.postable
        || !b.active
        || !b.postable
    {
        return Err(FbError::validation(
            "The bank and the credit memo’s original receivables account must be active postable ASSET accounts.",
        ));
    }
    if documents::control_role(tx, c, b.id)?.is_some() || b.id == ar.id {
        return Err(FbError::validation(
            "A refund cannot be paid from a receivables, undeposited-funds or other control account. Choose a designated bank.",
        ));
    }
    documents::lock_configuration(tx, actor, c, &[], Some(b.id), &[ar.id])?;
    let mapping = json!({"bank":bank,"receivables":ar.id.to_db()});
    let display = mapping_display(tx, c, &mapping)?;
    let entries = json!([{"account":ar.id.to_db(),"debit":data["amount"],"credit":"0.00"},{"account":bank,"debit":"0.00","credit":data["amount"]}]);
    let party = party(&customer);
    let co = company_snapshot(tx, c)?;
    let hash = digest(
        &json!({"company":c.to_db(),"credit_memo":memo,"intent":data,"available":cash(available),"mapping":display,"party":party,"company_snapshot":co}),
    );
    Ok(
        json!({"mapping":mapping,"display":display,"entries":entries,"customer_snapshot":party,"company_snapshot":co,"digest":hash,"available":cash(available),"after":cash(available-cents(&data["amount"])?) }),
    )
}
pub fn review_refund(
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
        let p = prepare(tx, actor, c, &m, &data)?;
        Ok(Review {
            domain: "refund",
            digest: p["digest"].as_str().unwrap().into(),
            display: json!({"mapping":p["display"],"entries":p["entries"],"data":data,"customer_snapshot":p["customer_snapshot"],"available":p["available"],"after":p["after"],"method_label":match stored_str(&data, "method")?{"CHECK"=>"Check","CASH"=>"Cash",_=>"Other (record only)"}}),
        })
    })
}
pub fn record_refund(
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
            "refund",
            "Invalid review. Open a fresh refund review.",
        )?;
        let existing:Option<String>=tx.query_row("SELECT r.id FROM credits_creditrefund r JOIN documents_documentidentity d ON d.id=r.identity_id WHERE r.company_id=?1 AND d.request_key=?2",params![c.to_db(),data["request_key"].as_str()],|r|r.get(0)).optional()?;
        if let Some(key) = existing {
            let r = scoped(tx, c, "credits_creditrefund", &key)?;
            if r["credit_memo_id"] != memo
                || r["creation_digest"] != hash
                || r["review_digest"] != reviewed
            {
                return Err(FbError::validation(
                    "This request key was recorded with a different payload or review.",
                ));
            }
            return Ok(r);
        }
        let identity =
            documents::create_identity(tx, actor, c, "RFD", stored_str(&data, "request_key")?)?;
        let m = scoped(tx, c, "credits_creditmemo", memo)?;
        let p = prepare(tx, actor, c, &m, &data)?;
        if p["digest"] != reviewed {
            return Err(FbError::validation(
                "Available credit, check number or posting mapping changed. Open a fresh review before recording.",
            ));
        }
        let source = stored_str(&identity, "id")?;
        documents::snapshot_source_accounts(tx, actor, c, source, &p["mapping"])?;
        let cm = documents::identity(tx, c, stored_str(&m, "identity_id")?)?;
        let extra = if data["memo"] != "" {
            format!(" · {}", stored_str(&data, "memo")?)
        } else {
            String::new()
        };
        let summary = py::py_truncate(
            &format!(
                "{}: refund of {} to {}{}",
                stored_str(&identity, "number")?,
                stored_str(&cm, "number")?,
                stored_str(&p["customer_snapshot"], "display_name")?,
                extra
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
        let mut r = data.clone();
        r.as_object_mut().unwrap().remove("request_key");
        r["id"] = json!(new_id());
        r["company_id"] = json!(c.to_db());
        r["identity_id"] = json!(source);
        r["credit_memo_id"] = json!(memo);
        r["creation_digest"] = json!(hash);
        r["review_digest"] = json!(reviewed);
        r["customer_id"] = m["customer_id"].clone();
        r["customer_snapshot"] = p["customer_snapshot"].clone();
        r["company_snapshot"] = p["company_snapshot"].clone();
        r["receivables_id"] = m["receivables_id"].clone();
        r["journal_id"] = json!(j.id.to_db());
        r["actor_id"] = json!(actor.to_db());
        r["actor_name"] = json!(user.username);
        tx.execute("INSERT INTO credits_creditrefund(id,company_id,identity_id,credit_memo_id,customer_id,date,amount,bank_id,receivables_id,method,check_number,journal_id,actor_id,data) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",params![r["id"].as_str(),c.to_db(),source,memo,r["customer_id"].as_str(),r["date"].as_str(),cents(&r["amount"])?,r["bank_id"].as_str(),r["receivables_id"].as_str(),r["method"].as_str(),r["check_number"].as_str(),j.id.to_db(),actor.to_db(),r.to_string()])?;
        seal(tx, "refund", stored_str(&r, "id")?)?;
        seal(tx, "journal", &j.id.to_db())?;
        services::audit(
            tx,
            &user,
            c,
            "credit_refund.recorded",
            format!("creditrefund:{}", stored_str(&r, "id")?),
            json!({"number":identity["number"],"credit_memo":cm["number"],"amount":r["amount"],"bank_id":r["bank_id"],"method":r["method"],"check_number":r["check_number"],"memo":r["memo"]}),
        )?;
        Ok(r)
    })
}
pub fn void_refund(
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
        let r = scoped(tx, c, "credits_creditrefund", key)?;
        let d = date(when)?;
        let reason = text(reason, 500, true)?;
        if let Some(v) = undo(tx, "credits_creditrefundvoid", "refund_id", key)? {
            if expected == Some(1) && v["date"] == d && v["reason"] == reason {
                return Ok(v);
            }
            return Err(FbError::validation(
                "Refund already voided with a different or stale command.",
            ));
        }
        documents::check_revision(1, expected)?;
        if d.as_str() < stored_str(&r, "date")? {
            return Err(FbError::validation(
                "Void date cannot precede the refund date.",
            ));
        }
        let j =
            documents::reverse_source(tx, actor, c, stored_str(&r, "identity_id")?, &d, &reason)?;
        let v = json!({"id":new_id(),"refund_id":key,"journal_id":j.id.to_db(),"date":d,"reason":reason,"actor_id":actor.to_db(),"actor_name":user.username});
        tx.execute("INSERT INTO credits_creditrefundvoid(id,refund_id,journal_id,date,actor_id,data) VALUES(?1,?2,?3,?4,?5,?6)",params![v["id"].as_str(),key,j.id.to_db(),d,actor.to_db(),v.to_string()])?;
        seal(tx, "refund_void", v["id"].as_str().unwrap())?;
        seal(tx, "journal", &j.id.to_db())?;
        services::audit(
            tx,
            &user,
            c,
            "credit_refund.voided",
            format!("creditrefund:{key}"),
            json!({"reason":reason,"date":d,"journal_id":j.id.to_db()}),
        )?;
        Ok(v)
    })
}
