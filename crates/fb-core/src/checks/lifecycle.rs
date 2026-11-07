use super::*;

fn references(c: &(impl Query + ?Sized), co: CompanyId, r: &Value, eligible: bool) -> FbResult<()> {
    let mut active = true;
    if let Some(id) = r["vendor_id"].as_str() {
        active = directory::get(c, co, Model::Vendor, id)
            .map_err(|_| err("The historical vendor is no longer available in this company."))?["active"]
            == true;
    }
    for l in r["lines"].as_array().unwrap() {
        if let Some(id) = l["item_id"].as_str() {
            active &= directory::get(c, co, Model::Item, id)
                .map_err(|_| err("Historical items must belong to this company."))?["active"]
                == true;
        }
        if let Some(id) = l["account_id"].as_str() {
            account(c, co, id)
                .map_err(|_| err("Historical accounts must belong to this company."))?;
        }
    }
    account(c, co, s(r, "bank_id"))
        .map_err(|_| err("Historical accounts must belong to this company."))?;
    if eligible && !active {
        return Err(err(
            "The vendor and every selected item must be active before recording.",
        ));
    }
    Ok(())
}
pub(super) fn posting(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    b: &Value,
    recording: bool,
) -> FbResult<(Value, Vec<Value>, Review)> {
    let r = &b["current"];
    if r["state"] != "DRAFT" {
        return Err(err(
            "Only a draft check can be reviewed or recorded. Voided checks cannot be re-recorded.",
        ));
    }
    let lines = r["lines"].as_array().unwrap();
    if !(1..=50).contains(&lines.len()) {
        return Err(err("A check requires 1–50 lines."));
    }
    let mut total = 0i64;
    for l in lines {
        let qty =
            rust_decimal::Decimal::from_i128_with_scale(l["quantity"].as_i64().unwrap() as i128, 4)
                .to_string();
        let rate = Money::from_db(l["rate"].as_i64().unwrap()).to_string();
        let amount = l["amount"].as_i64().unwrap();
        if calculations::line_amount(&json!(qty), &json!(rate))?.cents() != amount as i128
            || amount <= 0
        {
            return Err(err("Stored check arithmetic does not match its lines."));
        }
        total = total
            .checked_add(amount)
            .ok_or_else(|| err("Stored check arithmetic does not match its lines."))?;
    }
    if total <= 0 || r["total"] != total {
        return Err(err("Stored check arithmetic does not match its lines."));
    }
    if (r["print_later"] == 1) == !s(r, "check_number").is_empty() {
        return Err(err(
            "A check needs a number, or the print-later flag with no number.",
        ));
    }
    if recording {
        refuse_conflict(
            tx,
            co,
            s(r, "bank_id"),
            s(r, "check_number"),
            Some(s(b, "id")),
        )?;
    }
    references(tx, co, r, true)?;
    let mut ids = vec![];
    for l in lines {
        let aid = l["account_id"]
            .as_str()
            .and_then(AccountId::parse)
            .ok_or_else(|| err("Choose an account on every line before recording."))?;
        if aid.to_db() == s(r, "bank_id") || documents::control_role(tx, co, aid)?.is_some() {
            return Err(err(
                "Line accounts must differ from the paying bank and from the receivables, payables and undeposited-funds control accounts (current setup or frozen by any posted document). A check is not a bill payment: use Pay bills to settle vendor bills.",
            ));
        }
        ids.push(aid);
    }
    let mut mapping =
        documents::lock_configuration(tx, actor, co, &[], AccountId::parse(s(r, "bank_id")), &ids)?;
    let mut debits = BTreeMap::<String, i64>::new();
    for l in lines {
        let a = account(tx, co, s(l, "account_id"))?;
        if let Some(e) = documents::line_account_error(
            tx,
            co,
            Some(&a),
            AccountId::parse(s(r, "bank_id")),
            false,
        )? {
            return Err(err(&e));
        }
        mapping[format!("debit:{}", l["position"])] = json!(a.id.to_db());
        let value = debits.entry(a.id.to_db()).or_default();
        *value = value
            .checked_add(l["amount"].as_i64().unwrap())
            .ok_or_else(|| err("Check total exceeds 99999999999999.99."))?;
    }
    let mut entries = vec![
        json!({"account":mapping["bank"],"debit":"0.00","credit":Money::from_db(total).to_string()}),
    ];
    for (a, n) in debits {
        entries.push(json!({"account":a,"debit":Money::from_db(n).to_string(),"credit":"0.00"}));
    }
    let display = display_mapping(tx, co, &mapping)?;
    let digest = digest(
        &json!({"check":b["id"],"company":co.to_db(),"revision":b["revision"],"mapping":display,"entries":entries,"total":Money::from_db(total).to_string(),"check_number":r["check_number"],"print_later":r["print_later"]==1,"payee":r["payee_name"],"date":r["date"]}),
    );
    Ok((
        mapping,
        entries.clone(),
        Review {
            digest,
            display: json!({"mapping":display,"entries":entries}),
        },
    ))
}
pub fn review_check(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    cid: &str,
    expected: Option<i64>,
) -> FbResult<Review> {
    write(book, |tx| review_check_tx(tx, actor, co, cid, expected))
}
/// [`review_check`] inside a caller's writer (bank-feed acceptance composes review, record and decision).
pub fn review_check_tx(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    cid: &str,
    expected: Option<i64>,
) -> FbResult<Review> {
    tx.atomic(|| {
        services::access(tx, actor, co)?;
        let b = check(tx, co, cid)?;
        documents::check_revision(b["revision"].as_i64().unwrap(), expected)?;
        Ok(posting(tx, actor, co, &b, false)?.2)
    })
}
pub fn record_check(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    cid: &str,
    expected: Option<i64>,
    review: &Review,
) -> FbResult<Value> {
    write(book, |tx| record_check_tx(tx, actor, co, cid, expected, review))
}
/// [`record_check`] inside a caller's writer; rolls back with it.
pub fn record_check_tx(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    cid: &str,
    expected: Option<i64>,
    review: &Review,
) -> FbResult<Value> {
    tx.atomic(|| {
        let u = services::access(tx, actor, co)?;
        let mut b = check(tx, co, cid)?;
        let mut r = b["current"].clone();
        let rev = b["revision"].as_i64().unwrap();
        if r["state"] == "RECORDED"
            && r["action"] == "recorded"
            && expected == Some(rev - 1)
            && r["review_digest"] == review.digest
        {
            return Ok(money_display(b));
        }
        documents::check_revision(rev, expected)?;
        let next = documents::next_revision(rev)?;
        let (mapping, entries, current) = posting(tx, actor, co, &b, true)?;
        if current.digest != review.digest {
            return Err(err(
                "Check or posting mapping changed. Open a new review before recording.",
            ));
        }
        documents::snapshot_source_accounts(tx, actor, co, s(&b, "identity_id"), &mapping)?;
        let j = documents::post_source(
            tx,
            actor,
            co,
            s(&b, "identity_id"),
            s(&r, "date"),
            &py::py_truncate(
                &format!(
                    "{}: {} · {}",
                    s(&b, "number"),
                    s(&r, "payee_name"),
                    s(&r, "memo")
                ),
                240,
            ),
            entries,
        )?;
        services::audit(
            tx,
            &u,
            co,
            "check.journal_evidence",
            format!("check:{cid}"),
            json!({"journal_id":j.id.to_db(),"memo":r["memo"],"payee":r["payee_name"],"check_number":r["check_number"],"print_later":r["print_later"]==1,"total":Money::from_db(r["total"].as_i64().unwrap()).to_string()}),
        )?;
        r["state"] = json!("RECORDED");
        r["action"] = json!("recorded");
        r["reason"] = json!("");
        r["journal_id"] = json!(j.id.to_db());
        r["review_digest"] = json!(review.digest);
        advance(tx, actor, co, &mut b, r, next)?;
        seal_journal(tx, &j.id.to_db())?;
        Ok(money_display(check(tx, co, cid)?))
    })
}
fn advance(
    tx: &Writer<'_>,
    actor: UserId,
    co: CompanyId,
    b: &mut Value,
    r: Value,
    next: i64,
) -> FbResult<()> {
    b["revision"] = json!(next);
    tx.execute(
        "UPDATE checks_check SET revision=?,state=?,number_key=? WHERE id=?",
        params![next, s(&r, "state"), s(&r, "check_number"), s(b, "id")],
    )?;
    let lines = r["lines"].as_array().unwrap().clone();
    append(tx, actor, co, b, r, lines)
}
pub fn void_check(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    cid: &str,
    expected: Option<i64>,
    dt: &Value,
    reason: &Value,
) -> FbResult<Value> {
    write(book, |tx| {
        services::access(tx, actor, co)?;
        let mut b = check(tx, co, cid)?;
        let mut r = b["current"].clone();
        let rev = b["revision"].as_i64().unwrap();
        let dt = date(dt)?;
        let reason = text(reason, 500, true)?;
        if r["state"] == "VOID" && expected == Some(rev - 1) && r["reason"] == reason {
            let old: String = tx.query_row(
                "SELECT date FROM ledger_journal WHERE id=?",
                [s(&r, "reversal_id")],
                |r| r.get(0),
            )?;
            if old == dt {
                return Ok(money_display(b));
            }
        }
        documents::check_revision(rev, expected)?;
        let next = documents::next_revision(rev)?;
        if r["state"] != "RECORDED" {
            return Err(err("Only a recorded check can be voided."));
        }
        if dt.as_str() < s(&r, "date") {
            return Err(err("Void date cannot precede the check date."));
        }
        references(tx, co, &r, false)?;
        let j = documents::reverse_source(tx, actor, co, s(&b, "identity_id"), &dt, &reason)?;
        r["state"] = json!("VOID");
        r["action"] = json!("voided");
        r["reason"] = json!(reason);
        r["reversal_id"] = json!(j.id.to_db());
        advance(tx, actor, co, &mut b, r, next)?;
        seal_journal(tx, &j.id.to_db())?;
        Ok(money_display(check(tx, co, cid)?))
    })
}
pub fn assign_number(
    book: &mut Book,
    actor: UserId,
    co: CompanyId,
    cid: &str,
    expected: Option<i64>,
    number: &Value,
) -> FbResult<Value> {
    write(book, |tx| {
        services::access(tx, actor, co)?;
        let mut b = check(tx, co, cid)?;
        let mut r = b["current"].clone();
        let rev = b["revision"].as_i64().unwrap();
        let number = number_text(number)?;
        if number.is_empty() {
            return Err(err("Enter the check number that was used."));
        }
        if r["state"] == "RECORDED"
            && r["action"] == "numbered"
            && expected == Some(rev - 1)
            && r["check_number"] == number
        {
            return Ok(money_display(b));
        }
        documents::check_revision(rev, expected)?;
        let next = documents::next_revision(rev)?;
        if r["state"] != "RECORDED" || r["print_later"] != 1 {
            return Err(err(
                "Only a recorded check marked print later can be given a number here; edit a draft instead.",
            ));
        }
        numbering::validate_number(tx, co, Some(cid), s(&r, "bank_id"), &number)?;
        references(tx, co, &r, false)?;
        r["action"] = json!("numbered");
        r["reason"] = json!("");
        r["check_number"] = json!(number);
        r["print_later"] = json!(0);
        advance(tx, actor, co, &mut b, r, next)?;
        Ok(money_display(check(tx, co, cid)?))
    })
}
