use super::*;
fn normalize(p: &Value) -> FbResult<Value> {
    Ok(
        json!({"request_key":key(&p["request_key"])? ,"date":date(&p["date"])? ,"invoice_id":id(&p["invoice_id"],false)? ,"amount":money(&p["amount"],"The amount to apply")?}),
    )
}
pub fn invoice_open_balance(
    q: &(impl Query + ?Sized),
    c: CompanyId,
    invoice: &str,
) -> FbResult<i64> {
    let inv = sales::invoice(q, c, invoice)?;
    let used = sum(
        q,
        "SELECT amount FROM prepayments_live_settlements WHERE invoice_id=?",
        invoice,
    )?;
    cents(&inv["current"]["total"])?
        .checked_sub(used)
        .ok_or_else(|| err("Invoice settlements exceed supported limits."))
}
fn prepare(q: &Writer<'_>, c: CompanyId, r: &Value, d: &Value) -> FbResult<Value> {
    if load(q, "sales_receiptreversal", "receipt_id", s(r, "id"))?.is_some() {
        return Err(err("A voided customer deposit has nothing to apply."));
    }
    if s(d, "date") < s(r, "date") {
        return Err(err("The application date cannot precede the deposit date."));
    }
    let inv = sales::invoice(q, c, s(d, "invoice_id"))
        .map_err(|_| err("Choose invoices belonging to this company."))?;
    let identity = documents::identity(q, c, s(&inv, "identity_id"))?;
    let number = s(&identity, "number");
    let h = &inv["current"];
    if h["customer_id"] != r["customer_id"] || h["state"] != "OPEN" {
        return Err(err(format!(
            "Apply a deposit only to this customer’s posted, unvoided invoices ({number} is not one)."
        )));
    }
    if s(h, "date") > s(d, "date") {
        return Err(err(format!(
            "The application date cannot precede invoice {number}’s date."
        )));
    }
    let ar:Option<String>=q.query_row("SELECT account_id FROM documents_documentaccount WHERE source_id=? AND role='receivables'",[s(&inv,"identity_id")],|r|r.get(0)).optional()?;
    let ar = ar.ok_or_else(|| err("Invoice has no original receivables posting reference."))?;
    let outstanding = invoice_open_balance(q, c, s(&inv, "id"))?;
    let amount = cents(&d["amount"])?;
    if amount > outstanding {
        return Err(err(format!(
            "The amount to apply ({}) exceeds invoice {number}’s open balance {}.",
            s(d, "amount"),
            Money::from_db(outstanding)
        )));
    }
    let left = remaining(q, r)?;
    if amount > left {
        return Err(err(format!(
            "The amount to apply ({}) exceeds the deposit’s remaining balance {}.",
            s(d, "amount"),
            Money::from_db(left)
        )));
    }
    directory::get(q, c, Model::Customer, s(r, "customer_id"))
        .map_err(|_| err("Historical customer must belong to this company."))?;
    let liability = s(&r["prepayment"], "liability_id");
    let l = account(q, c, liability)?;
    let a = account(q, c, &ar)?;
    if ar == liability {
        return Err(err("Posting accounts must belong to this company."));
    }
    if !a.active || !a.postable || !l.active || !l.postable {
        return Err(err(
            "The deposit’s customer deposits account and the invoice’s receivables account must both be active and postable. Reactivate them before applying.",
        ));
    }
    let mapping = json!({"customer_deposits":liability,"receivables":ar});
    let display = display(q, c, &mapping)?;
    let row = json!({"invoice_id":inv["id"],"number":number,"revision":inv["revision"],"receivables_id":ar,"outstanding":cash(outstanding),"after":cash(outstanding.checked_sub(amount).ok_or_else(||err("Invalid invoice balance."))?)});
    let digest = digest(
        &json!({"company":c.to_db(),"deposit":r["id"],"intent":d,"row":row,"remaining":cash(left),"mapping":display}),
    );
    Ok(
        json!({"invoice":inv,"mapping":mapping,"display":display,"entries":entries(liability,&ar,&d["amount"]),"row":row,"remaining":cash(left),"after":cash(left.checked_sub(amount).ok_or_else(||err("Invalid deposit balance."))?),"digest":digest}),
    )
}
pub fn review_application(
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
        let p = prepare(tx, c, &r, &d)?;
        Ok(Review {
            domain: "application",
            digest: s(&p, "digest").into(),
            display: json!({"data":d,"row":p["row"],"remaining":p["remaining"],"after":p["after"],"mapping":p["display"],"entries":p["entries"]}),
        })
    })
}
pub fn apply_deposit(
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
        let reviewed = token(review, "application", "deposit application")?;
        let existing:Option<String>=tx.query_row("SELECT a.data FROM prepayments_depositapplication a JOIN documents_documentidentity i ON i.id=a.identity_id WHERE a.company_id=? AND i.request_key=?",params![c.to_db(),s(&d,"request_key")],|r|r.get(0)).optional()?;
        if let Some(raw) = existing {
            let v: Value =
                fb_db::parse_unique_json_as(&raw).map_err(|_| err("Invalid stored application."))?;
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
        let identity = documents::create_identity(tx, actor, c, "CDA", s(&d, "request_key"))?;
        let r = deposit(tx, c, deposit_id)?;
        let p = prepare(tx, c, &r, &d)?;
        if reviewed != s(&p, "digest") {
            return Err(err(
                "The invoice balance or the deposit’s remaining balance changed. Open a fresh review before applying.",
            ));
        }
        documents::snapshot_source_accounts(tx, actor, c, s(&identity, "id"), &p["mapping"])?;
        let dep = documents::identity(tx, c, s(&r, "identity_id"))?;
        let memo = summary(
            s(&identity, "number"),
            &format!(
                "{} applied to {} ({})",
                s(&dep, "number"),
                s(&p["row"], "number"),
                s(&r["customer_snapshot"], "display_name")
            ),
        );
        let j = documents::post_source(
            tx,
            actor,
            c,
            s(&identity, "id"),
            s(&d, "date"),
            &memo,
            p["entries"]
                .as_array()
                .cloned()
                .ok_or_else(|| err("Invalid posting plan."))?,
        )?;
        let a = json!({"id":new_id(),"company_id":c.to_db(),"identity_id":identity["id"],"deposit_id":deposit_id,"creation_digest":creation,"review_digest":reviewed,"invoice_id":p["invoice"]["id"],"invoice_revision_id":p["invoice"]["current"]["id"],"receivables_id":p["mapping"]["receivables"],"liability_id":p["mapping"]["customer_deposits"],"date":d["date"],"amount":d["amount"],"journal_id":j.id.to_db(),"actor_id":actor.to_db(),"actor_name":u.username});
        persist(
            tx,
            "prepayments_depositapplication",
            &a,
            &[
                "id",
                "company_id",
                "identity_id",
                "deposit_id",
                "invoice_id",
                "invoice_revision_id",
                "receivables_id",
                "liability_id",
                "date",
                "amount",
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
            "customer_deposit.applied",
            format!("receipt:{deposit_id}"),
            json!({"number":dep["number"],"application":identity["number"],"application_id":a["id"],"invoice":p["row"]["number"],"invoice_id":a["invoice_id"],"date":d["date"],"amount":d["amount"],"journal_id":j.id.to_db()}),
        )?;
        Ok(a)
    })
}
pub fn unapply_deposit(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    application_id: &str,
    expected: Option<i64>,
    dt: &Value,
    reason: &Value,
) -> FbResult<Value> {
    command(book, |tx| {
        let u = services::access(tx, actor, c)?;
        let a = load(tx, "prepayments_depositapplication", "id", application_id)?
            .filter(|a| a["company_id"] == c.to_db())
            .ok_or_else(|| FbError::DoesNotExist(String::new()))?;
        let r = deposit(tx, c, s(&a, "deposit_id"))?;
        let dt = date(dt)?;
        let reason = text(reason, 500, true)?;
        if let Some(v) = load(
            tx,
            "prepayments_depositunapplication",
            "application_id",
            application_id,
        )? {
            if expected == Some(1) && v["date"] == dt && v["reason"] == reason {
                return Ok(v);
            }
            return Err(err(
                "Deposit already unapplied with a different or stale command.",
            ));
        }
        documents::check_revision(1, expected)?;
        if dt.as_str() < s(&a, "date") {
            return Err(err("The unapply date cannot precede the application date."));
        }
        let j = documents::reverse_source(tx, actor, c, s(&a, "identity_id"), &dt, &reason)?;
        let v = json!({"id":new_id(),"application_id":application_id,"journal_id":j.id.to_db(),"date":dt,"reason":reason,"actor_id":actor.to_db(),"actor_name":u.username});
        persist(
            tx,
            "prepayments_depositunapplication",
            &v,
            &[
                "id",
                "application_id",
                "journal_id",
                "date",
                "actor_id",
                "data",
            ],
        )?;
        seal(tx, &j.id.to_db())?;
        let dep = documents::identity(tx, c, s(&r, "identity_id"))?;
        let app = documents::identity(tx, c, s(&a, "identity_id"))?;
        services::audit(
            tx,
            &u,
            c,
            "customer_deposit.unapplied",
            format!("receipt:{}", s(&r, "id")),
            json!({"number":dep["number"],"application":app["number"],"application_id":application_id,"date":dt,"reason":reason,"journal_id":j.id.to_db()}),
        )?;
        Ok(v)
    })
}
