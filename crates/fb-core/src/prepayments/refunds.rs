use super::*;
fn normalize(p: &Value) -> FbResult<Value> {
    let key = key(&p["request_key"])?;
    let amount = money(&p["amount"], "The refund amount")?;
    if !["CHECK", "CASH", "OTHER"].contains(&s(p, "method")) {
        return Err(err("Choose a refund method (record only)."));
    }
    let number = number_text(p.get("check_number").unwrap_or(&json!("")))?;
    if p["method"] != "CHECK" && !number.is_empty() {
        return Err(err("Only a refund by check carries a check number."));
    }
    Ok(
        json!({"request_key":key,"date":date(&p["date"])? ,"amount":amount,"bank_id":id(&p["bank_id"],false)? ,"method":p["method"],"check_number":number,"memo":crate::card_numbers::card_free(field(p,"memo",2000)?,"Memo")?}),
    )
}
fn refuse_number(
    q: &(impl Query + ?Sized),
    c: CompanyId,
    bank: &str,
    number: &str,
) -> FbResult<()> {
    crate::check_numbers::refuse_conflict(q, c, bank, number, None)
}
fn prepare(tx: &Writer<'_>, actor: UserId, c: CompanyId, r: &Value, d: &Value) -> FbResult<Value> {
    if load(tx, "sales_receiptreversal", "receipt_id", s(r, "id"))?.is_some() {
        return Err(err("A voided customer deposit has nothing to refund."));
    }
    if s(d, "date") < s(r, "date") {
        return Err(err("The refund date cannot precede the deposit date."));
    }
    let left = remaining(tx, r)?;
    let amount = cents(&d["amount"])?;
    if amount > left {
        return Err(err(format!(
            "Refund {} exceeds the deposit’s remaining balance {}.",
            s(d, "amount"),
            Money::from_db(left)
        )));
    }
    refuse_number(tx, c, s(d, "bank_id"), s(d, "check_number"))?;
    let customer = directory::get(tx, c, Model::Customer, s(r, "customer_id"))
        .map_err(|_| err("Historical customer must belong to this company."))?;
    let liability = s(&r["prepayment"], "liability_id");
    let l = account(tx, c, liability)
        .map_err(|_| err("Choose a designated bank account in this company."))?;
    let bank = account(tx, c, s(d, "bank_id"))
        .map_err(|_| err("Choose a designated bank account in this company."))?;
    if bank.kind.as_str() != "ASSET"
        || !bank.active
        || !bank.postable
        || documents::control_role(tx, c, bank.id)?.is_some()
    {
        return Err(err(
            "A deposit refund is paid from a designated bank, never from a receivables, undeposited-funds or other control account.",
        ));
    }
    if !l.active || !l.postable {
        return Err(err(
            "The deposit’s customer deposits account must be active and postable. Reactivate it before refunding.",
        ));
    }
    documents::lock_configuration(tx, actor, c, &[], Some(bank.id), &[l.id])?;
    let mapping = json!({"bank":bank.id.to_db(),"customer_deposits":liability});
    let display = display(tx, c, &mapping)?;
    let party = party(&customer);
    let company = company_snapshot(tx, c)?;
    let digest = digest(
        &json!({"company":c.to_db(),"deposit":r["id"],"intent":d,"remaining":cash(left),"mapping":display,"party":party,"company_snapshot":company}),
    );
    Ok(
        json!({"mapping":mapping,"display":display,"entries":entries(liability,&bank.id.to_db(),&d["amount"]),"customer_snapshot":party,"company_snapshot":company,"digest":digest,"remaining":cash(left),"after":cash(left.checked_sub(amount).ok_or_else(||err("Invalid deposit balance."))?)}),
    )
}
pub fn review_refund(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    deposit_id: &str,
    payload: &Value,
) -> FbResult<Review> {
    command(book, |tx| {
        services::access(tx, actor, c)?;
        let d = normalize(payload)?;
        let r = deposit(tx, c, deposit_id)?;
        let p = prepare(tx, actor, c, &r, &d)?;
        Ok(Review {
            domain: "refund",
            digest: s(&p, "digest").into(),
            display: json!({"method_label":match s(&d,"method"){"CHECK"=>"Check","CASH"=>"Cash",_=>"Other (record only)"},"data":d,"mapping":p["display"],"entries":p["entries"],"customer_snapshot":p["customer_snapshot"],"remaining":p["remaining"],"after":p["after"]}),
        })
    })
}
pub fn record_refund(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    deposit_id: &str,
    review: Option<&Review>,
    payload: &Value,
) -> FbResult<Value> {
    command(book, |tx| {
        let u = services::access(tx, actor, c)?;
        let d = normalize(payload)?;
        let creation = digest(&d);
        let reviewed = token(review, "refund", "deposit refund")?;
        let existing:Option<String>=tx.query_row("SELECT r.data FROM prepayments_depositrefund r JOIN documents_documentidentity i ON i.id=r.identity_id WHERE r.company_id=? AND i.request_key=?",params![c.to_db(),s(&d,"request_key")],|r|r.get(0)).optional()?;
        if let Some(raw) = existing {
            let v: Value = fb_db::parse_unique_json_as(&raw).map_err(|_| err("Invalid stored refund."))?;
            if v["deposit_id"] != deposit_id
                || v["creation_digest"] != creation
                || v["review_digest"] != reviewed
            {
                return Err(err(
                    "This request key was recorded with a different payload or review.",
                ));
            }
            return Ok(v);
        }
        let identity = documents::create_identity(tx, actor, c, "CDR", s(&d, "request_key"))?;
        let r = deposit(tx, c, deposit_id)?;
        let p = prepare(tx, actor, c, &r, &d)?;
        if reviewed != s(&p, "digest") {
            return Err(err(
                "Remaining deposit, check number or posting mapping changed. Open a fresh review before recording.",
            ));
        }
        documents::snapshot_source_accounts(tx, actor, c, s(&identity, "id"), &p["mapping"])?;
        let dep = documents::identity(tx, c, s(&r, "identity_id"))?;
        let mut memo = format!(
            "refund of {} to {}",
            s(&dep, "number"),
            s(&p["customer_snapshot"], "display_name")
        );
        if !s(&d, "memo").is_empty() {
            memo += &format!(" · {}", s(&d, "memo"));
        }
        let j = documents::post_source(
            tx,
            actor,
            c,
            s(&identity, "id"),
            s(&d, "date"),
            &summary(s(&identity, "number"), &memo),
            p["entries"]
                .as_array()
                .cloned()
                .ok_or_else(|| err("Invalid posting plan."))?,
        )?;
        let v = json!({"id":new_id(),"company_id":c.to_db(),"identity_id":identity["id"],"deposit_id":deposit_id,"creation_digest":creation,"review_digest":reviewed,"customer_id":r["customer_id"],"customer_snapshot":p["customer_snapshot"],"company_snapshot":p["company_snapshot"],"date":d["date"],"amount":d["amount"],"bank_id":d["bank_id"],"liability_id":p["mapping"]["customer_deposits"],"method":d["method"],"check_number":d["check_number"],"memo":d["memo"],"journal_id":j.id.to_db(),"actor_id":actor.to_db(),"actor_name":u.username});
        persist(
            tx,
            "prepayments_depositrefund",
            &v,
            &[
                "id",
                "company_id",
                "identity_id",
                "deposit_id",
                "customer_id",
                "bank_id",
                "liability_id",
                "date",
                "amount",
                "method",
                "check_number",
                "journal_id",
                "actor_id",
                "data",
            ],
        )?;
        seal(tx, &j.id.to_db())?;
        services::audit(
            tx,
            &u,
            c,
            "deposit_refund.recorded",
            format!("depositrefund:{}", s(&v, "id")),
            json!({"number":identity["number"],"deposit":dep["number"],"amount":v["amount"],"bank_id":v["bank_id"],"method":v["method"],"check_number":v["check_number"],"memo":v["memo"]}),
        )?;
        Ok(v)
    })
}
pub fn void_refund(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    refund_id: &str,
    expected: Option<i64>,
    dt: &Value,
    reason: &Value,
) -> FbResult<Value> {
    command(book, |tx| {
        let u = services::access(tx, actor, c)?;
        let r = load(tx, "prepayments_depositrefund", "id", refund_id)?
            .filter(|r| r["company_id"] == c.to_db())
            .ok_or_else(|| FbError::DoesNotExist(String::new()))?;
        let dt = date(dt)?;
        let reason = text(reason, 500, true)?;
        if let Some(v) = load(tx, "prepayments_depositrefundvoid", "refund_id", refund_id)? {
            if expected == Some(1) && v["date"] == dt && v["reason"] == reason {
                return Ok(v);
            }
            return Err(err(
                "Refund already voided with a different or stale command.",
            ));
        }
        documents::check_revision(1, expected)?;
        if dt.as_str() < s(&r, "date") {
            return Err(err("Void date cannot precede the refund date."));
        }
        let j = documents::reverse_source(tx, actor, c, s(&r, "identity_id"), &dt, &reason)?;
        let v = json!({"id":new_id(),"refund_id":refund_id,"journal_id":j.id.to_db(),"date":dt,"reason":reason,"actor_id":actor.to_db(),"actor_name":u.username});
        persist(
            tx,
            "prepayments_depositrefundvoid",
            &v,
            &["id", "refund_id", "journal_id", "date", "actor_id", "data"],
        )?;
        seal(tx, &j.id.to_db())?;
        services::audit(
            tx,
            &u,
            c,
            "deposit_refund.voided",
            format!("depositrefund:{refund_id}"),
            json!({"reason":reason,"date":dt,"journal_id":j.id.to_db()}),
        )?;
        Ok(v)
    })
}

fn number_text(v: &Value) -> FbResult<String> {
    let value = v
        .as_str()
        .ok_or_else(|| err("Enter a check number up to 20 characters."))?;
    fb_db::prepayments::validate_prepayment_number(value)
        .map(str::to_owned)
        .map_err(err)
}
