use super::*;
/// Current deposit membership; historical memberships remain in their owning deposits.
pub fn deposit_status(
    q: &(impl Query + ?Sized),
    c: CompanyId,
    receipt_ids: &[String],
) -> FbResult<BTreeMap<String, Value>> {
    let mut out = BTreeMap::new();
    for key in receipt_ids {
        let found:Option<String>=q.query_row("SELECT d.id FROM banking_depositline l JOIN banking_deposit d ON d.id=l.deposit_id WHERE l.receipt_id=?1 AND d.company_id=?2 AND NOT EXISTS(SELECT 1 FROM banking_depositreversal z WHERE z.deposit_id=d.id)",params![key,c.to_db()],|r|r.get(0)).optional()?;
        if let Some(d) = found {
            out.insert(key.clone(), deposit(q, c, &d)?);
        }
    }
    Ok(out)
}
/// Latest release per receipt, ordered by reversal date and then deposit creation identity.
pub fn release_dates(
    q: &(impl Query + ?Sized),
    c: CompanyId,
    receipt_ids: &[String],
) -> FbResult<BTreeMap<String, (String, String)>> {
    let mut out = BTreeMap::new();
    for key in receipt_ids {
        let found:Option<(String,String)>=q.query_row("SELECT i.number,z.date FROM banking_depositline l JOIN banking_deposit d ON d.id=l.deposit_id JOIN banking_depositreversal z ON z.deposit_id=d.id JOIN documents_documentidentity i ON i.id=d.identity_id WHERE l.receipt_id=?1 AND d.company_id=?2 ORDER BY z.date DESC,d.id DESC LIMIT 1",params![key,c.to_db()],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        if let Some(row) = found {
            out.insert(key.clone(), row);
        }
    }
    Ok(out)
}
/// Sales' reversal preflight, also repeated by SQL. The existing sales hook remains independently safe.
pub fn refuse_live_deposit(
    q: &(impl Query + ?Sized),
    c: CompanyId,
    key: &str,
    d: &str,
) -> FbResult<()> {
    let r = sales::receipt(q, c, key)?;
    let live:Option<String>=q.query_row("SELECT i.number FROM banking_depositline l JOIN banking_deposit x ON x.id=l.deposit_id JOIN documents_documentidentity i ON i.id=x.identity_id WHERE l.receipt_id=?1 AND NOT EXISTS(SELECT 1 FROM banking_depositreversal z WHERE z.deposit_id=l.deposit_id) LIMIT 1",[key],|r|r.get(0)).optional()?;
    if let Some(number) = live {
        return Err(FbError::validation(format!(
            "Reverse deposit {number} before reversing this receipt."
        )));
    }
    let release:Option<(String,String)>=q.query_row("SELECT i.number,z.date FROM banking_depositline l JOIN banking_depositreversal z ON z.deposit_id=l.deposit_id JOIN banking_deposit x ON x.id=l.deposit_id JOIN documents_documentidentity i ON i.id=x.identity_id WHERE l.receipt_id=?1 ORDER BY z.date DESC,x.id DESC LIMIT 1",[key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((number, rd)) = release
        && d < rd.as_str()
    {
        let receipt_number: String = q.query_row(
            "SELECT number FROM documents_documentidentity WHERE id=?1",
            [s(&r, "identity_id")?],
            |r| r.get(0),
        )?;
        return Err(FbError::validation(format!(
            "{receipt_number} was released from deposit {number} on {rd}; the receipt reversal cannot be dated earlier."
        )));
    }
    Ok(())
}
fn normalize(v: &Value) -> FbResult<Value> {
    let rows = v["receipt_ids"]
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= 50)
        .ok_or_else(|| FbError::validation("Select 1–50 whole undeposited receipts."))?;
    let mut ids = rows.iter().map(id).collect::<FbResult<Vec<_>>>()?;
    ids.sort();
    if ids.windows(2).any(|w| w[0] == w[1]) {
        return Err(FbError::validation("Select each receipt only once."));
    }
    Ok(
        json!({"request_key":uuid(&v["request_key"])?,"bank_id":id(&v["bank_id"])?,"date":date(&v["date"])?,"reference":crate::card_numbers::card_free(field(v,"reference",120)?,"Reference")?,"memo":crate::card_numbers::card_free(field(v,"memo",2000)?,"Memo")?,"receipt_ids":ids}),
    )
}
fn prepare(tx: &Writer<'_>, actor: UserId, c: CompanyId, v: &Value) -> FbResult<Value> {
    let mut credits: BTreeMap<String, i64> = BTreeMap::new();
    let mut rows = vec![];
    let d = s(v, "date")?;
    for key in v["receipt_ids"]
        .as_array()
        .ok_or_else(|| FbError::validation("Invalid receipt selection."))?
    {
        let key = key
            .as_str()
            .ok_or_else(|| FbError::validation("Invalid receipt."))?;
        let r = sales::receipt(tx, c, key)
            .map_err(|_| FbError::validation("Choose receipts belonging to this company."))?;
        let num: String = tx.query_row(
            "SELECT number FROM documents_documentidentity WHERE id=?1",
            [s(&r, "identity_id")?],
            |r| r.get(0),
        )?;
        if r["destination"] != "UNDEPOSITED" {
            return Err(FbError::validation(format!(
                "{num} was recorded directly to a bank and cannot be deposited again."
            )));
        }
        let reversed: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sales_receiptreversal WHERE receipt_id=?1)",
            [key],
            |r| r.get(0),
        )?;
        if reversed {
            return Err(FbError::validation(format!(
                "{num} is reversed and cannot be deposited."
            )));
        }
        let live:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM banking_depositline l WHERE l.receipt_id=?1 AND NOT EXISTS(SELECT 1 FROM banking_depositreversal z WHERE z.deposit_id=l.deposit_id))",[key],|r|r.get(0))?;
        if live {
            return Err(FbError::validation(format!(
                "{num} already belongs to a live deposit."
            )));
        }
        if s(&r, "date")? > d {
            return Err(FbError::validation(
                "Deposit date cannot precede any selected receipt date.",
            ));
        }
        let release:Option<(String,String)>=tx.query_row("SELECT i.number,z.date FROM banking_depositline l JOIN banking_depositreversal z ON z.deposit_id=l.deposit_id JOIN banking_deposit x ON x.id=l.deposit_id JOIN documents_documentidentity i ON i.id=x.identity_id WHERE l.receipt_id=?1 ORDER BY z.date DESC,x.id DESC LIMIT 1",[key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        if let Some((old, rd)) = release
            && d < rd.as_str()
        {
            return Err(FbError::validation(format!(
                "{num} was released from deposit {old} on {rd}; the new deposit cannot be dated earlier."
            )));
        }
        let dest = s(&r, "destination_account_id")?.to_owned();
        let amount = cents(&r["amount"])?;
        let n = add(*credits.get(&dest).unwrap_or(&0), amount)?;
        credits.insert(dest.clone(), n);
        let method = match s(&r, "method")? {
            "CHECK" => "Check",
            "CASH" => "Cash",
            "CARD" => "Card (record only)",
            "TRANSFER" => "Transfer (record only)",
            _ => "Other",
        };
        rows.push(json!({"receipt_id":key,"number":num,"date":r["date"],"kind":if r["kind"]=="SR"{"Sales receipt"}else{"Customer receipt"},"customer":r["customer_snapshot"]["display_name"],"method":method,"reference":r["reference"],"undeposited_id":dest,"amount":r["amount"]}));
    }
    let bank = s(v, "bank_id")?;
    if credits.contains_key(bank) {
        return Err(FbError::validation(
            "Bank must differ from the receipts’ original undeposited funds accounts.",
        ));
    }
    let mut mapping = json!({"bank":bank});
    for key in credits.keys() {
        mapping[format!("undeposited:{key}")] = json!(key);
    }
    let mut display = vec![];
    for (role, key) in mapping
        .as_object()
        .ok_or_else(|| FbError::validation("Invalid mapping."))?
    {
        let a = account(
            tx,
            c,
            key.as_str()
                .ok_or_else(|| FbError::validation("Invalid mapping."))?,
        )?;
        if a.kind.as_str() != "ASSET" || !a.active || !a.postable {
            return Err(FbError::validation(
                "Bank and original undeposited funds accounts must be active postable ASSET accounts.",
            ));
        }
        display.push(json!({"role":role,"id":a.id.to_db(),"code":a.code,"name":a.name,"kind":a.kind.as_str()}));
    }
    documents::lock_configuration(
        tx,
        actor,
        c,
        &[],
        AccountId::parse(bank),
        &credits
            .keys()
            .filter_map(|k| AccountId::parse(k))
            .collect::<Vec<_>>(),
    )?;
    let amount = credits.values().try_fold(0, |a, b| add(a, *b))?;
    let mut entries = vec![json!({"account":bank,"debit":cash(amount),"credit":"0.00"})];
    entries.extend(
        credits
            .iter()
            .map(|(k, n)| json!({"account":k,"debit":"0.00","credit":cash(*n)})),
    );
    let co = store::get_company(tx, c)?;
    let co = json!({"id":c.to_db(),"name":co.name,"currency":co.currency});
    let hash = digest(
        &json!({"company":c.to_db(),"intent":v,"receipts":rows,"mapping":display,"company_snapshot":co}),
    );
    Ok(
        json!({"mapping":mapping,"display":display,"entries":entries,"receipts":rows,"amount":cash(amount),"company_snapshot":co,"digest":hash}),
    )
}
pub fn review_deposit(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    input: &Value,
) -> FbResult<Review> {
    command(book, |tx| review_deposit_tx(tx, actor, c, input))
}
/// [`review_deposit`] inside a caller's writer (bank-feed acceptance composes review, record and decision).
pub fn review_deposit_tx(tx: &Writer<'_>, actor: UserId, c: CompanyId, input: &Value) -> FbResult<Review> {
    tx.atomic(|| {
        services::access(tx, actor, c)?;
        let v = normalize(input)?;
        let p = prepare(tx, actor, c, &v)?;
        Ok(Review {
            digest: s(&p, "digest")?.into(),
            display: json!({"mapping":p["display"],"entries":p["entries"],"receipts":p["receipts"],"amount":p["amount"],"company_snapshot":p["company_snapshot"],"data":v}),
        })
    })
}
pub fn deposit(q: &(impl Query + ?Sized), c: CompanyId, key: &str) -> FbResult<Value> {
    values(
        q,
        "deposit",
        "SELECT data FROM banking_deposit WHERE id=?1",
        key,
    )?
    .into_iter()
    .find(|v| v["company_id"] == c.to_db())
    .ok_or_else(|| FbError::missing("Deposit"))
}
pub fn record_deposit(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    input: &Value,
    review: &Review,
) -> FbResult<Value> {
    command(book, |tx| record_deposit_tx(tx, actor, c, input, review))
}
/// [`record_deposit`] inside a caller's writer; rolls back with it.
pub fn record_deposit_tx(
    tx: &Writer<'_>,
    actor: UserId,
    c: CompanyId,
    input: &Value,
    review: &Review,
) -> FbResult<Value> {
    tx.atomic(|| {
        let user = services::access(tx, actor, c)?;
        let v = normalize(input)?;
        let hash = digest(&v);
        let key = s(&v, "request_key")?;
        let existing:Option<String>=tx.query_row("SELECT d.id FROM banking_deposit d JOIN documents_documentidentity i ON i.id=d.identity_id WHERE d.company_id=?1 AND i.request_key=?2",params![c.to_db(),key],|r|r.get(0)).optional()?;
        if let Some(key) = existing {
            let old = deposit(tx, c, &key)?;
            if old["creation_digest"] != hash || old["review_digest"] != review.digest {
                return Err(FbError::validation(
                    "This request key was recorded with a different payload or review.",
                ));
            }
            return Ok(old);
        }
        let source = documents::create_identity(tx, actor, c, "DEP", key)?;
        let p = prepare(tx, actor, c, &v)?;
        if p["digest"] != review.digest {
            return Err(FbError::validation(
                "Deposit selection or posting mapping changed. Open a fresh review before recording.",
            ));
        }
        let sid = s(&source, "id")?;
        documents::snapshot_source_accounts(tx, actor, c, sid, &p["mapping"])?;
        let memo = py::py_truncate(
            &format!("{}: {}", s(&source, "number")?, s(&v, "memo")?),
            240,
        );
        let j = documents::post_source(
            tx,
            actor,
            c,
            sid,
            s(&v, "date")?,
            &memo,
            p["entries"]
                .as_array()
                .ok_or_else(|| FbError::validation("Invalid entries."))?
                .clone(),
        )?;
        let key = new_id();
        let out = json!({"id":key,"company_id":c.to_db(),"identity_id":sid,"creation_digest":hash,"review_digest":review.digest,"company_snapshot":p["company_snapshot"],"date":v["date"],"amount":p["amount"],"reference":v["reference"],"memo":v["memo"],"bank_id":v["bank_id"],"journal_id":j.id.to_db(),"actor_name":user.username});
        tx.execute("INSERT INTO banking_deposit(id,company_id,identity_id,bank_id,journal_id,date,amount,data,actor_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![key,c.to_db(),sid,s(&v,"bank_id")?,j.id.to_db(),s(&v,"date")?,cents(&p["amount"])?,out.to_string(),actor.to_db()])?;
        for row in p["receipts"]
            .as_array()
            .ok_or_else(|| FbError::validation("Invalid receipts."))?
        {
            tx.execute("INSERT INTO banking_depositline(id,deposit_id,receipt_id,undeposited_id,amount) VALUES(?1,?2,?3,?4,?5)",params![new_id(),key,s(row,"receipt_id")?,s(row,"undeposited_id")?,cents(&row["amount"]) ?])?;
        }
        seal(tx, "deposit", &key)?;
        seal(tx, "journal", &j.id.to_db())?;
        services::audit(
            tx,
            &user,
            c,
            "deposit.recorded",
            format!("deposit:{key}"),
            json!({"number":source["number"],"amount":p["amount"],"memo":v["memo"],"bank_id":v["bank_id"],"receipts":p["receipts"].as_array().ok_or_else(||FbError::validation("Invalid receipts."))?.iter().map(|r|json!({"receipt_id":r["receipt_id"],"amount":r["amount"]})).collect::<Vec<_>>()}),
        )?;
        Ok(out)
    })
}
pub fn reverse_deposit(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    key: &str,
    expected: Option<i64>,
    d: &Value,
    reason: &Value,
) -> FbResult<Value> {
    command(book, |tx| {
        let user = services::access(tx, actor, c)?;
        let old = deposit(tx, c, key)?;
        let d = date(d)?;
        let reason = text(reason, 500, true)?;
        if let Some(existing) = values(
            tx,
            "reversal",
            "SELECT data FROM banking_depositreversal WHERE deposit_id=?1",
            key,
        )?
        .into_iter()
        .next()
        {
            if expected == Some(1) && existing["date"] == d && existing["reason"] == reason {
                return Ok(existing);
            }
            return Err(FbError::validation(
                "Deposit already reversed with a different or stale command.",
            ));
        }
        documents::check_revision(1, expected)?;
        if d.as_str() < s(&old, "date")? {
            return Err(FbError::validation(
                "Reversal date cannot precede the deposit date.",
            ));
        }
        refuse_cleared_reversal(tx, c, s(&old, "journal_id")?)?;
        let j = documents::reverse_source(tx, actor, c, s(&old, "identity_id")?, &d, &reason)?;
        let rid = new_id();
        let out = json!({"id":rid,"deposit_id":key,"journal_id":j.id.to_db(),"date":d,"reason":reason,"actor_name":user.username});
        tx.execute("INSERT INTO banking_depositreversal(id,deposit_id,journal_id,date,data,actor_id) VALUES(?1,?2,?3,?4,?5,?6)",params![rid,key,j.id.to_db(),d,out.to_string(),actor.to_db()])?;
        seal(tx, "journal", &j.id.to_db())?;
        services::audit(
            tx,
            &user,
            c,
            "deposit.reversed",
            format!("deposit:{key}"),
            json!({"reason":reason,"date":d,"journal_id":j.id.to_db()}),
        )?;
        Ok(out)
    })
}
