use super::*;
fn positive(v: &Value) -> FbResult<i64> {
    let n = cents(v)?;
    if n <= 0 {
        return Err(err("Payment and allocation amounts must be positive."));
    }
    Ok(n)
}
// All unwraps on d/allocations below consume values constructed by normalize,
// never stored JSON. Stored bill rows use the checked readers in mod.rs.
fn normalize(p: &Value) -> FbResult<Value> {
    let key = py::py_uuid(s(p, "request_key"))
        .ok_or_else(|| err("Supply a stable UUID request key."))?
        .0;
    if !["CHECK", "CASH", "CARD", "TRANSFER", "OTHER"].contains(&s(p, "method")) {
        return Err(err("Choose a payment method (record only)."));
    }
    let bank = id(&p["bank_id"], false)?.unwrap();
    let input = p["allocations"]
        .as_array()
        .filter(|a| (1..=50).contains(&a.len()))
        .ok_or_else(|| err("Enter 1–50 explicit allocations."))?;
    let mut allocations = vec![];
    for a in input {
        if a.as_object()
            .is_none_or(|m| m.len() != 2 || !m.contains_key("bill_id") || !m.contains_key("amount"))
        {
            return Err(err("Unexpected allocation fields."));
        }
        allocations.push(
            json!({"bill_id":id(&a["bill_id"],false)?.unwrap(),"amount":positive(&a["amount"])?}),
        );
    }
    allocations.sort_by(|a, b| s(a, "bill_id").cmp(s(b, "bill_id")));
    if allocations
        .windows(2)
        .any(|w| w[0]["bill_id"] == w[1]["bill_id"])
    {
        return Err(err("Allocate to each bill only once."));
    }
    let amount = positive(&p["amount"])?;
    if allocations
        .iter()
        .map(|v| v["amount"].as_i64().unwrap() as i128)
        .sum::<i128>()
        != amount as i128
    {
        return Err(err(
            "Explicit allocations must equal the payment amount exactly; no on-account credit.",
        ));
    }
    Ok(
        json!({"request_key":key,"vendor_id":id(&p["vendor_id"],false)?.unwrap(),"date":date(&p["date"])?,"amount":amount,"method":p["method"],"reference":crate::card_numbers::card_free(field_text(p,"reference",120)?,"Reference")?,"memo":crate::card_numbers::card_free(field_text(p,"memo",2000)?,"Memo")?,"bank_id":bank,"allocations":allocations}),
    )
}
struct Plan {
    mapping: Value,
    entries: Vec<Value>,
    allocations: Vec<Value>,
    vendor: Value,
    company: Value,
    review: Review,
}
fn prepare(tx: &Writer<'_>, actor: UserId, co: CompanyId, d: &Value) -> FbResult<Plan> {
    crate::check_numbers::refuse_conflict(tx, co, s(d,"bank_id"), crate::check_numbers::payment_number(s(d,"method"),s(d,"reference")), None)?;
    let mut debits = BTreeMap::<String, i64>::new();
    let mut prepared = vec![];
    let mut allocation_display = vec![];
    for a in d["allocations"].as_array().unwrap() {
        let b = bill(tx, co, s(a, "bill_id"))
            .map_err(|_| err("Every bill target must belong to this company."))?;
        let r = &b["current"];
        if r["vendor_id"] != d["vendor_id"] || r["state"] != "OPEN" {
            return Err(err(
                "Allocate only to this vendor’s posted, unreversed bills.",
            ));
        }
        if s(r, "date") > s(d, "date") {
            return Err(err("Payment date cannot precede any allocated bill date."));
        }
        if a["amount"].as_i64().unwrap() > stored_i64(r, "total")? - allocated(tx, s(&b, "id"))? {
            return Err(err(
                "Allocation exceeds the actual outstanding bill amount. Review again.",
            ));
        }
        let ap:Option<String>=tx.query_row("SELECT account_id FROM documents_documentaccount WHERE source_id=? AND role='payables'",[s(&b,"identity_id")],|r|r.get(0)).optional()?;
        let ap = ap.ok_or_else(|| err("Bill has no original payables posting reference."))?;
        *debits.entry(ap.clone()).or_default() += a["amount"].as_i64().unwrap();
        prepared.push(json!({"bill_id":b["id"],"bill_revision_id":r["id"],"payables_id":ap,"amount":a["amount"]}));
        allocation_display.push(json!({"bill_id":b["id"],"number":b["number"],"revision":r["number"],"payables_id":ap,"amount":a["amount"]}));
    }
    let vendor = directory::get(tx, co, Model::Vendor, s(d, "vendor_id"))
        .map_err(|_| err("Choose an active vendor in this company."))?;
    if vendor["active"] != true {
        return Err(err("Choose an active vendor in this company."));
    }
    let bank = account(tx, co, s(d, "bank_id"))
        .map_err(|_| err("Posting accounts must belong to this company."))?;
    for aid in debits
        .keys()
        .chain(std::iter::once(&s(d, "bank_id").to_string()))
    {
        let a = account(tx, co, aid)
            .map_err(|_| err("Posting accounts must belong to this company."))?;
        let kind = if aid == s(d, "bank_id") {
            "ASSET"
        } else {
            "LIABILITY"
        };
        if a.kind.as_str() != kind || !a.active || !a.postable {
            return Err(err(
                "Bank must be active postable ASSET; original payables must be active postable LIABILITY accounts.",
            ));
        }
    }
    if debits.contains_key(s(d, "bank_id")) {
        return Err(err("Bank must differ from original payables accounts."));
    }
    let ids = debits
        .keys()
        .map(|v| AccountId::parse(v).ok_or_else(|| err("Invalid stored payables account ID.")))
        .collect::<FbResult<Vec<_>>>()?;
    documents::lock_configuration(tx, actor, co, &[], Some(bank.id), &ids)?;
    let mut mapping = json!({"bank":bank.id.to_db()});
    let mut entries = vec![
        json!({"account":bank.id.to_db(),"debit":"0.00","credit":Money::from_db(d["amount"].as_i64().unwrap()).to_string()}),
    ];
    for (a, amount) in debits {
        mapping[format!("payables:{a}")] = json!(a);
        entries
            .push(json!({"account":a,"debit":Money::from_db(amount).to_string(),"credit":"0.00"}));
    }
    let display = display_mapping(tx, co, &mapping)?;
    let vendor = party(&vendor);
    let company = company_snap(tx, co)?;
    let digest = digest(
        &json!({"company":co.to_db(),"intent":d,"allocations":allocation_display,"mapping":display,"party":vendor,"company_snapshot":company}),
    );
    let review = Review {
        digest,
        kind: "payment",
        display: json!({"mapping":display,"entries":entries,"allocations":allocation_display,"vendor_snapshot":vendor,"company_snapshot":company}),
    };
    Ok(Plan {
        mapping,
        entries,
        allocations: prepared,
        vendor,
        company,
        review,
    })
}
pub fn review_payment(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    p: &Value,
) -> FbResult<Review> {
    write(book, |tx| {
        services::access(tx, actor, co)?;
        let d = normalize(p)?;
        Ok(prepare(tx, actor, co, &d)?.review)
    })
}
pub fn record_payment(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    p: &Value,
    review: &Review,
) -> FbResult<Value> {
    write(book, |tx| {
        let u = services::access(tx, actor, co)?;
        let d = normalize(p)?;
        let creation = digest(&d);
        if review.kind != "payment" {
            return Err(err("Invalid review. Open a fresh payment review."));
        }
        let existing = rows(
            tx,
            "SELECT p.* FROM payables_billpayment p JOIN documents_documentidentity i ON i.id=p.identity_id WHERE p.company_id=? AND i.request_key=?",
            &[&co.to_db(), &s(&d, "request_key")],
        )?;
        if let Some(v) = existing.first() {
            if v["creation_digest"] != creation || v["review_digest"] != review.digest {
                return Err(err(
                    "This request key was recorded with a different payload or review.",
                ));
            }
            return Ok(v.clone());
        }
        let identity = documents::create_identity(tx, actor, co, "BPAY", s(&d, "request_key"))?;
        let plan = prepare(tx, actor, co, &d)?;
        if plan.review.digest != review.digest {
            return Err(err(
                "Payment or posting mapping changed. Open a fresh review before recording.",
            ));
        }
        documents::snapshot_source_accounts(tx, actor, co, s(&identity, "id"), &plan.mapping)?;
        let j = documents::post_source(
            tx,
            actor,
            co,
            s(&identity, "id"),
            s(&d, "date"),
            &py::py_truncate(
                &format!("{}: {}", s(&identity, "number"), s(&d, "memo")),
                240,
            ),
            plan.entries,
        )?;
        let mut r = json!({"id":fb_ids::new_v7().to_string(),"company_id":co.to_db(),"identity_id":identity["id"],"creation_digest":creation,"review_digest":review.digest,"vendor_snapshot":plan.vendor,"company_snapshot":plan.company,"journal_id":j.id.to_db(),"actor_id":actor.to_db(),"actor_name":u.username,"created_at":py::now_utc()});
        for k in [
            "vendor_id",
            "date",
            "amount",
            "method",
            "reference",
            "memo",
            "bank_id",
        ] {
            r[k] = d[k].clone();
        }
        insert(tx, "payables_billpayment", &r)?;
        for mut a in plan.allocations {
            a["id"] = json!(fb_ids::new_v7().to_string());
            a["payment_id"] = r["id"].clone();
            insert(tx, "payables_paymentallocation", &a)?;
        }
        tx.execute(
            "INSERT INTO fb_seal VALUES('bill_payment',?)",
            [s(&r, "id")],
        )?;
        seal_journal(tx, &j.id.to_db())?;
        services::audit(
            tx,
            &u,
            co,
            "payment.recorded",
            format!("billpayment:{}", s(&r, "id")),
            json!({"number":identity["number"],"amount":Money::from_db(d["amount"].as_i64().unwrap()).to_string(),"memo":d["memo"],"allocations":d["allocations"].as_array().unwrap().iter().map(|a|json!({"bill_id":a["bill_id"],"amount":Money::from_db(a["amount"].as_i64().unwrap()).to_string()})).collect::<Vec<_>>()}),
        )?;
        Ok(r)
    })
}
pub fn reverse_payment(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    pid: &str,
    expected: Option<i64>,
    dt: &Value,
    reason: &Value,
) -> FbResult<Value> {
    write(book, |tx| {
        let u = services::access(tx, actor, co)?;
        let p = rows(
            tx,
            "SELECT * FROM payables_billpayment WHERE company_id=? AND id=?",
            &[&co.to_db(), &pid],
        )?
        .pop()
        .ok_or_else(|| FbError::missing("BillPayment"))?;
        let dt = date(dt)?;
        let reason = text(reason, 500, true)?;
        if let Some(r) = rows(
            tx,
            "SELECT * FROM payables_paymentreversal WHERE payment_id=?",
            &[&pid],
        )?
        .pop()
        {
            if expected == Some(1) && r["date"] == dt && r["reason"] == reason {
                return Ok(r);
            }
            return Err(err(
                "Payment already reversed with a different or stale command.",
            ));
        }
        documents::check_revision(1, expected)?;
        if dt.as_str() < s(&p, "date") {
            return Err(err("Reversal date cannot precede payment date."));
        }
        directory::get(tx, co, Model::Vendor, s(&p, "vendor_id"))
            .map_err(|_| err("Historical vendor must belong to this company."))?;
        let j = documents::reverse_source(tx, actor, co, s(&p, "identity_id"), &dt, &reason)?;
        let r = json!({"id":fb_ids::new_v7().to_string(),"payment_id":pid,"journal_id":j.id.to_db(),"date":dt,"reason":reason,"actor_id":actor.to_db(),"actor_name":u.username,"created_at":py::now_utc()});
        insert(tx, "payables_paymentreversal", &r)?;
        seal_journal(tx, &j.id.to_db())?;
        services::audit(
            tx,
            &u,
            co,
            "payment.reversed",
            format!("billpayment:{pid}"),
            json!({"reason":reason,"date":dt,"journal_id":j.id.to_db()}),
        )?;
        Ok(r)
    })
}
