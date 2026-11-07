use super::*;

// These are reconcile's _uuid/_revision, rather than the generic banking rules.
fn request_key(value: &Value, what: &str) -> FbResult<String> {
    value.as_str().and_then(py::py_uuid).map(|u| u.0)
        .ok_or_else(|| FbError::validation(format!("Reload the page and try again (missing {what} key).")))
}
fn integer_value(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_bool().map(i64::from))
        .or_else(|| match value.as_str().and_then(py::py_int) {
            Some(py::PyInt::Small(n)) => Some(n),
            Some(py::PyInt::Large { negative }) => Some(if negative { i64::MIN } else { i64::MAX }),
            _ => None,
        })
        // Python int() truncates floats. Values beyond i64 saturate here: they
        // cannot equal a stored revision, but a no-op tick save ignores them.
        .or_else(|| value.as_f64().filter(|n| n.is_finite()).map(|n| n.trunc() as i64))
}
fn required_revision(value: &Value) -> FbResult<i64> {
    integer_value(value).filter(|n| !value.is_boolean() && (0..=1_000_000_000).contains(n))
        .ok_or_else(|| FbError::validation("Reload the page and try again."))
}
fn optional_revision(value: &Value) -> FbResult<Option<i64>> {
    if value.is_null() || value.as_str() == Some("") { Ok(None) }
    else { required_revision(value).map(Some) }
}
fn parse_amount(v: &Value, label: &str, negative: bool, required: bool) -> FbResult<Option<i64>> {
    if v.is_null() || v.as_str().is_some_and(|s| py::py_strip(s).is_empty()) {
        return if required {
            Err(FbError::validation(format!("Enter the {label}.")))
        } else {
            Ok(None)
        };
    }
    let raw = if let Some(s) = v.as_str() {
        s.to_owned()
    } else if v.is_i64() || v.is_u64() {
        v.to_string()
    } else {
        return Err(FbError::validation(format!(
            "Enter the {label} as an amount such as 1250.00."
        )));
    };
    let raw = py::py_strip(&raw).replace(',', "");
    let minus = raw.starts_with('-');
    let raw = raw.trim_start_matches('-').trim_start_matches('$');
    if fb_money::parse_python_decimal(raw).is_none() {
        return Err(FbError::validation(format!("Enter the {label} as an amount such as 1250.00.")));
    }
    let n = cents(&json!(raw)).map_err(|_| {
        FbError::validation(format!(
            "Enter the {label} with at most two decimal places (up to 99999999999999.99)."
        ))
    })?;
    let n = if minus { -n } else { n };
    if !negative && n <= 0 {
        return Err(FbError::validation(format!(
            "The {label} must be more than 0.00 (leave it blank for none)."
        )));
    }
    Ok(Some(n))
}
fn hydrate(q: &(impl Query + ?Sized), mut v: Value) -> FbResult<Value> {
    let key = s(&v, "id")?;
    let started:String=q.query_row("SELECT u.username FROM banking_reconciliation r JOIN fb_user u ON u.id=r.started_by_id WHERE r.id=?1",[key],|r|r.get(0))?;
    let mut st=q.prepare("SELECT a.phase,u.username FROM banking_reconciliationactor a JOIN fb_user u ON u.id=a.user_id WHERE a.reconciliation_id=?1")?;
    let names = st
        .query_map([key], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    v["started_by"] = json!(started);
    for (phase, name) in names {
        v[if phase == "COMPLETE" {
            "completed_by"
        } else {
            "undone_by"
        }] = json!(name);
    }
    Ok(v)
}
pub fn reconciliation(
    q: &(impl Query + ?Sized),
    c: CompanyId,
    key: &str,
    aid: &str,
) -> FbResult<Value> {
    let v = values(
        q,
        "reconciliation",
        "SELECT data FROM banking_reconciliation WHERE id=?1",
        key,
    )?
    .into_iter()
    .find(|v| v["company_id"] == c.to_db() && v["account_id"] == aid)
    .ok_or_else(|| FbError::missing("Reconciliation"))?;
    hydrate(q, v)
}
fn last(q: &(impl Query + ?Sized), aid: &str) -> FbResult<Option<Value>> {
    values(q,"reconciliation","SELECT data FROM banking_reconciliation WHERE account_id=?1 AND status='DONE' ORDER BY id DESC LIMIT 1",aid)?.into_iter().next().map(|v|hydrate(q,v)).transpose()
}
fn beginning(q: &(impl Query + ?Sized), aid: &str) -> FbResult<i64> {
    last(q, aid)?
        .map(|v| cents(&v["ending_balance"]))
        .unwrap_or(Ok(0))
}
fn persist(tx: &Writer<'_>, v: &Value, insert: bool, actor: UserId) -> FbResult<()> {
    if insert {
        tx.execute("INSERT INTO banking_reconciliation(id,company_id,account_id,status,statement_date,ending_balance,hide_later,request_key,creation_digest,revision,data,started_by_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",params![s(v,"id")?,s(v,"company_id")?,s(v,"account_id")?,s(v,"status")?,s(v,"statement_date")?,cents(&v["ending_balance"])?,boolean(v,"hide_later")?,s(v,"request_key")?,s(v,"creation_digest")?,v["revision"].as_i64(),v.to_string(),actor.to_db()])?;
    } else {
        tx.execute("UPDATE banking_reconciliation SET status=?1,statement_date=?2,ending_balance=?3,hide_later=?4,revision=?5,data=?6 WHERE id=?7",params![s(v,"status")?,s(v,"statement_date")?,cents(&v["ending_balance"])?,boolean(v,"hide_later")?,v["revision"].as_i64(),v.to_string(),s(v,"id")?])?;
    }
    seal(
        tx,
        "reconciliation",
        &format!("{}:{}", s(v, "id")?, v["revision"]),
    )
}
fn charge_account(tx: &Writer<'_>, c: CompanyId, a: &crate::model::Account, other: &Value, interest: bool) -> FbResult<crate::model::Account> {
    let label = if interest {
        "Interest earned"
    } else if a.section_balance() == crate::model::Side::Credit {
        "Finance charge"
    } else {
        "Service charge"
    };
    let kind = if interest { "INCOME" } else { "EXPENSE" };
    let other = other.as_str().filter(|s| fb_ids::parse_uuid(s).is_some() || py::py_int(s).is_some())
        .ok_or_else(|| FbError::validation(format!("Choose the account for the {}.", label.to_lowercase())))?;
    let b = account(tx, c, other)
        .ok()
        .filter(|b| b.kind.as_str() == kind && b.active && b.postable && b.id != a.id)
        .ok_or_else(|| {
            FbError::validation(format!(
                "Choose an active {} account of this company for the {}.",
                if interest { "income" } else { "expense" },
                label.to_lowercase()
            ))
        })?;
    Ok(b)
}
#[allow(clippy::too_many_arguments)] // Preserve the frozen service argument contract.
fn charge(
    tx: &Writer<'_>,
    actor: UserId,
    c: CompanyId,
    a: &crate::model::Account,
    d: &str,
    n: i64,
    other: &Value,
    key: &str,
    interest: bool,
) -> FbResult<String> {
    let label = if interest {
        "Interest earned"
    } else if a.section_balance() == crate::model::Side::Credit {
        "Finance charge"
    } else {
        "Service charge"
    };
    let b = charge_account(tx, c, a, other, interest)?;
    let namespace = uuid::Uuid::from_u128(0x6f1d2a523b8e4c559a430e6e0f2b18a1);
    let derived = uuid::Uuid::new_v5(
        &namespace,
        format!("{key}:{}", if interest { "interest" } else { "service" }).as_bytes(),
    )
    .to_string();
    let lines = if interest {
        vec![
            json!({"account":a.id.to_db(),"debit":cash(n),"description":label}),
            json!({"account":b.id.to_db(),"credit":cash(n),"description":label}),
        ]
    } else {
        vec![
            json!({"account":b.id.to_db(),"debit":cash(n),"description":label}),
            json!({"account":a.id.to_db(),"credit":cash(n),"description":label}),
        ]
    };
    let mut input = services::DraftInput::new(
        d,
        &py::py_truncate(
            &format!("{label} · statement {d} · {} · {}", a.code, a.name),
            240,
        ),
        lines,
    );
    input.request_key = Some(derived);
    let j = services::save_draft_tx(tx, actor, c, &input, None)?;
    let j = services::post_journal_tx(tx, actor, c, j.id, None)?;
    let lid: String = tx.query_row(
        "SELECT id FROM ledger_journalline WHERE journal_id=?1 AND account_id=?2",
        params![j.id.to_db(), a.id.to_db()],
        |r| r.get(0),
    )?;
    set_cleared_tx(tx, actor, c, &lid, "C", Some(0), Some(&a.id.to_db()))?;
    Ok(j.id.to_db())
}
pub fn begin(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    aid: &str,
    input: &Value,
) -> FbResult<Value> {
    command(book, |tx| {
        let user = services::access(tx, actor, c)?;
        let key = request_key(&input["request_key"], "request")?;
        let d = statement_date(&input["statement_date"])?;
        let ending = parse_amount(
            &input["ending_balance"],
            "statement ending balance",
            true,
            true,
        )?
        .ok_or_else(|| FbError::validation("Enter the statement ending balance."))?;
        let ch = parse_amount(&input["service_charge"], "service charge", false, false)?;
        let earned = parse_amount(&input["interest"], "interest earned", false, false)?;
        let rev = optional_revision(&input["expected_revision"])?;
        let hash = digest(
            &json!({"account":aid,"statement_date":d,"ending":cash(ending),"charge":ch.map(cash),"charge_account":if ch.is_some(){input["service_account_id"].clone()}else{Value::Null},"interest":earned.map(cash),"interest_account":if earned.is_some(){input["interest_account_id"].clone()}else{Value::Null},"revision":rev}),
        );
        let old: Option<String> = tx
            .query_row(
                "SELECT data FROM banking_reconciliation WHERE company_id=?1 AND request_key=?2",
                params![c.to_db(), key],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(raw) = old {
            let old = stored("reconciliation", &raw)?;
            if old["creation_digest"] != hash || old["account_id"] != aid {
                return Err(FbError::validation(
                    "This retry key belongs to a different reconciliation. Reload Banking → Reconcile.",
                ));
            }
            return hydrate(tx, old);
        }
        let a = account(tx, c, aid)?;
        if !is_register_account(tx, c, aid)? {
            return Err(FbError::validation(
                "Only bank and credit card accounts are reconciled.",
            ));
        }
        if !a.active {
            return Err(FbError::validation(format!(
                "{} · {} is inactive. Make it active before reconciling it.",
                a.code, a.name
            )));
        }
        if ch.is_some() { charge_account(tx, c, &a, &input["service_account_id"], false)?; }
        if earned.is_some() && a.section_balance() == crate::model::Side::Credit {
            return Err(FbError::validation(
                "Interest earned is for bank accounts. Record card credits as payments.",
            ));
        }
        if earned.is_some() { charge_account(tx, c, &a, &input["interest_account_id"], true)?; }
        if last(tx, aid)?.is_some_and(|r| s(&r, "statement_date").is_ok_and(|rd| rd > d.as_str())) {
            return Err(FbError::validation(format!(
                "The statement date must be on or after {}, the last reconciled statement of {} · {}.",
                s(
                    &last(tx, aid)?.ok_or_else(|| FbError::validation("Missing history."))?,
                    "statement_date"
                )?,
                a.code,
                a.name
            )));
        }
        let current = values(
            tx,
            "reconciliation",
            "SELECT data FROM banking_reconciliation WHERE account_id=?1 AND status='OPEN'",
            aid,
        )?
        .into_iter()
        .next();
        let current = current.map(|v| hydrate(tx, v)).transpose()?;
        let insert = current.is_none();
        let mut out=current.unwrap_or(json!({"id":new_id(),"company_id":c.to_db(),"account_id":aid,"status":"OPEN","request_key":key,"creation_digest":hash,"revision":1,"hide_later":true,"started_by":user.username,"completion_key":null,"undo_key":null,"undo_reason":"","beginning_balance":null,"cleared_debit":null,"cleared_credit":null,"cleared_count":null,"service_charge":null,"service_account_id":null,"service_journal_id":null,"interest":null,"interest_account_id":null,"interest_journal_id":null}));
        let before = if insert {
            if rev.is_some() {
                return Err(FbError::validation(
                    "This reconciliation was completed or changed meanwhile. Reload Banking → Reconcile.",
                ));
            }
            Value::Null
        } else {
            if rev.is_none() {
                return Err(FbError::validation(format!(
                    "A reconciliation of {} · {} is already in progress (statement {}, started by {}). Open it from Banking → Reconcile to continue or change it.",
                    a.code,
                    a.name,
                    s(&out, "statement_date")?,
                    s(&out, "started_by")?
                )));
            }
            let retry:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM ledger_auditevent WHERE company_id=?1 AND action='reconciliation.changed' AND target=?2 AND json_extract(detail,'$.request_key')=?3)",params![c.to_db(),format!("reconciliation:{}",s(&out,"id")?),key],|r|r.get(0))?;
            if retry {
                return Ok(out);
            }
            if out["revision"].as_i64() != rev {
                return Err(FbError::validation(
                    "This reconciliation changed after you opened it (another window or user). Reload it and try again.",
                ));
            }
            if ch.is_some() && !out["service_journal_id"].is_null() {
                return Err(FbError::validation(format!(
                    "A {} of {} is already recorded for this statement. Record another one as a journal entry.",
                    if a.section_balance() == crate::model::Side::Credit {
                        "finance charge"
                    } else {
                        "service charge"
                    },
                    s(&out, "service_charge")?
                )));
            }
            if earned.is_some() && !out["interest_journal_id"].is_null() {
                return Err(FbError::validation(format!(
                    "Interest earned of {} is already recorded for this statement. Record more as a journal entry.",
                    s(&out, "interest")?
                )));
            }
            let before = json!({"statement_date":out["statement_date"],"ending_balance":out["ending_balance"]});
            out["revision"] = json!(documents::next_revision(
                out["revision"]
                    .as_i64()
                    .ok_or_else(|| FbError::validation("Invalid revision."))?
            )?);
            before
        };
        out["statement_date"] = json!(d);
        out["ending_balance"] = cash(ending);
        for (n, interest) in [(ch, false), (earned, true)] {
            if let Some(n) = n {
                let prefix = if interest { "interest" } else { "service" };
                let field = if interest {
                    "interest"
                } else {
                    "service_charge"
                };
                let other = &input[format!("{prefix}_account_id")];
                let j = charge(tx, actor, c, &a, &d, n, other, &key, interest)?;
                out[field] = cash(n);
                out[format!("{prefix}_account_id")] = other.clone();
                out[format!("{prefix}_journal_id")] = json!(j);
            }
        }
        persist(tx, &out, insert, actor)?;
        services::audit(
            tx,
            &user,
            c,
            if insert {
                "reconciliation.started"
            } else {
                "reconciliation.changed"
            },
            format!("reconciliation:{}", s(&out, "id")?),
            json!({"account_id":aid,"account":format!("{} · {}",a.code,a.name),"statement_date":d,"ending_balance":cash(ending),"before":before,"service_charge":ch.map(cash),"service_journal_id":if ch.is_some(){out["service_journal_id"].clone()}else{Value::Null},"interest":earned.map(cash),"interest_journal_id":if earned.is_some(){out["interest_journal_id"].clone()}else{Value::Null},"revision":out["revision"],"request_key":key}),
        )?;
        Ok(out)
    })
}
fn open_lines(q: &(impl Query + ?Sized), c: CompanyId, rec: &Value) -> FbResult<Vec<Value>> {
    let mut st=q.prepare("SELECT l.id FROM ledger_journalline l JOIN ledger_journal j ON j.id=l.journal_id LEFT JOIN banking_banklinestatus s ON s.journal_line_id=l.id WHERE j.company_id=?1 AND l.account_id=?2 AND j.status='POSTED' AND coalesce(s.state,'U')<>'R' AND (?3=0 OR j.date<=?4) ORDER BY j.date,j.id,l.position,l.id")?;
    let keys = st
        .query_map(
            params![
                c.to_db(),
                s(rec, "account_id")?,
                boolean(rec, "hide_later")?,
                s(rec, "statement_date")?
            ],
            |r| r.get::<_, String>(0),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    keys.iter().map(|k| cleared::line(q, c, k)).collect()
}
pub fn figures(q: &(impl Query + ?Sized), c: CompanyId, rec: &Value) -> FbResult<Value> {
    if !fb_db::banking_evidence::payload("reconciliation", rec) {
        return Err(FbError::validation("Invalid reconciliation evidence."));
    }

    let aid = s(rec, "account_id")?;
    let a = account(q, c, aid)?;
    let beginning = beginning(q, aid)?;
    let rows = open_lines(q, c, rec)?;
    let (mut debit, mut credit, mut nd, mut nc) = (0, 0, 0, 0);
    for row in rows.iter().filter(|v| v["state"] == "C") {
        let d = cents(&row["debit"])?;
        let cr = cents(&row["credit"])?;
        debit = add(debit, d)?;
        credit = add(credit, cr)?;
        nd += usize::from(d > 0);
        nc += usize::from(cr > 0);
    }
    let net = add(debit, -credit)?;
    let cleared = add(
        beginning,
        if a.section_balance() == crate::model::Side::Debit {
            net
        } else {
            -net
        },
    )?;
    Ok(
        json!({"beginning":cash(beginning),"cleared_debit":cash(debit),"cleared_credit":cash(credit),"n_debit":nd,"n_credit":nc,"cleared":cash(cleared),"ending":rec["ending_balance"],"difference":cash(add(cents(&rec["ending_balance"])?,signed(cleared,-1)?)?)}),
    )
}
pub fn save_ticks(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    key: &str,
    aid: &str,
    input: &Value,
) -> FbResult<Value> {
    command(book, |tx| {
        services::access(tx, actor, c)?;
        let rev = Some(required_revision(&input["expected_revision"])?);
        let shown = input["shown"]
            .as_object()
            .filter(|m| m.len() <= 900)
            .ok_or_else(|| FbError::validation("Reload the reconciliation and try again."))?;
        let ticked = input["ticked"]
            .as_array()
            .ok_or_else(|| FbError::validation("Reload the reconciliation and try again."))?
            .iter()
            .map(|v| id(v).map_err(|_| FbError::validation("Reload the reconciliation and try again.")))
            .collect::<FbResult<BTreeSet<_>>>()?;
        if ticked.iter().any(|k| !shown.contains_key(k)) {
            return Err(FbError::validation(
                "Reload the reconciliation and try again.",
            ));
        }
        let shown = shown.iter().map(|(key, value)| {
            if fb_ids::parse_uuid(key).is_none() && py::py_int(key).is_none() {
                return Err(FbError::validation("Reload the reconciliation and try again."));
            }
            integer_value(value).map(|n| (key.clone(), n))
                .ok_or_else(|| FbError::validation("Reload the reconciliation and try again."))
        }).collect::<FbResult<BTreeMap<_, _>>>()?;
        let mut rec = reconciliation(tx, c, key, aid)?;
        if rec["status"] != "OPEN" {
            return Err(FbError::validation("The reconciliation is no longer in progress."));
        }
        let hide = input
            .get("hide_later")
            .filter(|v| !v.is_null())
            .map(|v| {
                v.as_bool()
                    .ok_or_else(|| FbError::validation("Invalid checkbox value."))
            })
            .transpose()?;
        let mut changes = vec![];
        for (k, expected) in &shown {
            let expected = *expected;
            let row=cleared::line(tx,c,k).map_err(|_|FbError::validation("Some lines on this screen are no longer in this register. Reload the reconciliation and try again."))?;
            if row["account_id"] != aid {
                return Err(FbError::validation(
                    "Some lines on this screen are no longer in this register. Reload the reconciliation and try again.",
                ));
            }
            let wanted = if ticked.contains(k) { "C" } else { "U" };
            if row["state"] != wanted {
                changes.push((row, wanted, expected));
            }
        }
        let option = hide.is_some_and(|h| rec["hide_later"] != h);
        if changes.is_empty() && !option {
            return Ok(rec);
        }
        if rec["revision"].as_i64() != rev {
            return Err(FbError::validation(
                "This reconciliation changed after you opened it (another window or user). Reload it and tick again.",
            ));
        }
        for (row, _, _) in &changes {
            s(row, "id")?;
        }
        changes.sort_by(|(a, _, _), (b, _, _)| a["id"].as_str().cmp(&b["id"].as_str()));
        for (row, wanted, expected) in changes {
            if row["state"] == "R" {
                return Err(FbError::validation(
                    "A line on this screen is reconciled. Reload the reconciliation.",
                ));
            }
            if row["revision"].as_i64() != Some(expected) {
                return Err(FbError::validation(
                    "A line on this screen changed in the register after you opened it. Reload the reconciliation and tick again.",
                ));
            }
            cleared::write_state(
                tx,
                actor,
                c,
                &row,
                wanted,
                None,
                json!({"via":"reconcile","reconciliation_id":key}),
                None,
            )?;
        }
        if let Some(h) = hide {
            rec["hide_later"] = json!(h);
        }
        rec["revision"] = json!(documents::next_revision(
            rec["revision"]
                .as_i64()
                .ok_or_else(|| FbError::validation("Invalid revision."))?
        )?);
        persist(tx, &rec, false, actor)?;
        Ok(rec)
    })
}
pub fn reconcile_now(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    key: &str,
    aid: &str,
    expected: Option<i64>,
    completion: &Value,
) -> FbResult<Value> {
    command(book, |tx| {
        let user = services::access(tx, actor, c)?;
        let ck = request_key(completion, "completion")?;
        let expected = Some(required_revision(&json!(expected))?);
        let mut rec = reconciliation(tx, c, key, aid)?;
        if rec["status"] != "OPEN" && rec["completion_key"] == ck {
            return Ok(rec);
        }
        if rec["status"] != "OPEN" {
            return Err(FbError::validation(format!(
                "This statement was already reconciled by {}. Open the reconciliation report or start the next statement.",
                s(&rec, "completed_by")?
            )));
        }
        if rec["revision"].as_i64() != expected {
            return Err(FbError::validation(
                "This reconciliation changed after you opened it (another window or user). Reload it and check the difference again.",
            ));
        }
        let totals = figures(tx, c, &rec)?;
        if cents(&totals["difference"])? != 0 {
            return Err(FbError::validation(format!(
                "The difference is {}, not 0.00. Tick exactly the lines on the statement, or correct the statement ending balance with Modify, then reconcile.",
                s(&totals, "difference")?
            )));
        }
        let mut rows = open_lines(tx, c, &rec)?
            .into_iter()
            .filter(|v| v["state"] == "C")
            .collect::<Vec<_>>();
        for row in &rows {
            s(row, "id")?;
        }
        rows.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        for row in &rows {
            tx.execute("INSERT INTO banking_reconciliationline(id,reconciliation_id,journal_line_id,debit,credit) VALUES(?1,?2,?3,?4,?5)",params![new_id(),key,s(row,"id")?,cents(&row["debit"])?,cents(&row["credit"]) ?])?;
        }
        rec["status"] = json!("DONE");
        for (a, b) in [
            ("beginning_balance", "beginning"),
            ("cleared_debit", "cleared_debit"),
            ("cleared_credit", "cleared_credit"),
        ] {
            rec[a] = totals[b].clone();
        }
        rec["cleared_count"] = json!(rows.len());
        rec["completion_key"] = json!(ck);
        rec["completed_by"] = json!(user.username);
        rec["revision"] = json!(documents::next_revision(
            rec["revision"]
                .as_i64()
                .ok_or_else(|| FbError::validation("Invalid revision."))?
        )?);
        // The root moves first; its deferred completion seal is emitted only after every mark.
        tx.execute(
            "UPDATE banking_reconciliation SET status='DONE',revision=?1,data=?2 WHERE id=?3",
            params![rec["revision"].as_i64(), rec.to_string(), key],
        )?;
        tx.execute(
            "INSERT INTO banking_reconciliationactor VALUES(?1,'COMPLETE',?2)",
            params![key, actor.to_db()],
        )?;
        let mut status_rows = rows.clone();
        for row in &status_rows {
            s(row, "status_id")?;
        }
        status_rows.sort_by(|a, b| a["status_id"].as_str().cmp(&b["status_id"].as_str()));
        for row in &status_rows {
            cleared::write_state(
                tx,
                actor,
                c,
                row,
                "R",
                Some(key),
                json!({"reconciliation_id":key}),
                Some("bank_line.reconciled"),
            )?;
        }
        seal(tx, "reconciliation", &format!("{key}:{}", rec["revision"]))?;
        let a = account(tx, c, aid)?;
        services::audit(
            tx,
            &user,
            c,
            "reconciliation.completed",
            format!("reconciliation:{key}"),
            json!({"account_id":aid,"account":format!("{} · {}",a.code,a.name),"statement_date":rec["statement_date"],"beginning_balance":rec["beginning_balance"],"ending_balance":rec["ending_balance"],"cleared_debit":rec["cleared_debit"],"cleared_credit":rec["cleared_credit"],"cleared_count":rows.len(),"line_ids":rows.iter().map(|r|r["id"].clone()).collect::<Vec<_>>(),"revision":rec["revision"]}),
        )?;
        Ok(rec)
    })
}
#[allow(clippy::too_many_arguments)] // Preserve the frozen service argument contract.
pub fn undo_last(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    key: &str,
    aid: &str,
    expected: Option<i64>,
    reason: &Value,
    undo: &Value,
) -> FbResult<Value> {
    command(book, |tx| {
        let user = services::access(tx, actor, c)?;
        let uk = request_key(undo, "undo")?;
        let expected = Some(required_revision(&json!(expected))?);
        if let Some(raw) = reason.as_str() { py::clean_free_text(raw)?; }
        let reason = reason.as_str().map(py::py_strip).unwrap_or("");
        if reason.is_empty() {
            return Err(FbError::validation("Give a reason for undoing this reconciliation."));
        }
        if py::py_len(reason) > 500 {
            return Err(FbError::validation("Keep the reason to 500 characters."));
        }
        let mut rec = reconciliation(tx, c, key, aid)?;
        if rec["status"] == "UNDONE" && rec["undo_key"] == uk {
            return Ok(rec);
        }
        let latest = last(tx, aid)?;
        if rec["status"] != "DONE" || latest.as_ref().is_none_or(|r| r["id"] != key) {
            return Err(FbError::validation(format!(
                "Only the most recent reconciliation of this account can be undone{}",
                latest
                    .map(|r| -> FbResult<String> {
                        Ok(format!(
                            " (statement {}). Undo that one first.",
                            s(&r, "statement_date")?
                        ))
                    })
                    .transpose()?
                    .unwrap_or(".".into())
            )));
        }
        if rec["revision"].as_i64() != expected {
            return Err(FbError::validation(
                "This reconciliation changed after you opened it. Reload the reconciliation history and try again.",
            ));
        }
        let mut st=tx.prepare("SELECT journal_line_id FROM banking_banklinestatus WHERE reconciliation_id=?1 ORDER BY id")?;
        let keys = st
            .query_map([key], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        rec["status"] = json!("UNDONE");
        rec["undone_by"] = json!(user.username);
        rec["undo_key"] = json!(uk);
        rec["undo_reason"] = json!(reason);
        rec["revision"] = json!(documents::next_revision(
            rec["revision"]
                .as_i64()
                .ok_or_else(|| FbError::validation("Invalid revision."))?
        )?);
        tx.execute(
            "UPDATE banking_reconciliation SET status='UNDONE',revision=?1,data=?2 WHERE id=?3",
            params![rec["revision"].as_i64(), rec.to_string(), key],
        )?;
        tx.execute(
            "INSERT INTO banking_reconciliationactor VALUES(?1,'UNDO',?2)",
            params![key, actor.to_db()],
        )?;
        for k in &keys {
            let row = cleared::line(tx, c, k)?;
            cleared::write_state(
                tx,
                actor,
                c,
                &row,
                "C",
                None,
                json!({"reconciliation_id":key}),
                Some("bank_line.unreconciled"),
            )?;
        }
        seal(tx, "reconciliation", &format!("{key}:{}", rec["revision"]))?;
        let a = account(tx, c, aid)?;
        let kept = [
            rec["service_journal_id"].clone(),
            rec["interest_journal_id"].clone(),
        ]
        .into_iter()
        .filter(|v| !v.is_null())
        .collect::<Vec<_>>();
        services::audit(
            tx,
            &user,
            c,
            "reconciliation.undone",
            format!("reconciliation:{key}"),
            json!({"account_id":aid,"account":format!("{} · {}",a.code,a.name),"statement_date":rec["statement_date"],"reason":reason,"lines":keys.len(),"kept_journal_ids":kept,"note":if kept.is_empty(){""}else{"Service charge and interest entries are not reversed."},"revision":rec["revision"]}),
        )?;
        Ok(rec)
    })
}
pub fn words(a: &crate::model::Account) -> Value {
    if a.section_balance() == crate::model::Side::Credit {
        json!({"card":true,"left":"Charges and cash advances","right":"Payments and credits","left_one":"Charge","right_one":"Payment","left_short":"charges","right_short":"payments","service":"Finance charge","ending":"Ending balance (amount owed)","left_summary":"Cleared charges and cash advances","right_summary":"Cleared payments and credits (−)"})
    } else {
        json!({"card":false,"left":"Checks and payments","right":"Deposits and other credits","left_one":"Payment","right_one":"Deposit","left_short":"payments","right_short":"deposits","service":"Service charge","ending":"Ending balance","left_summary":"Cleared checks and payments (−)","right_summary":"Cleared deposits and other credits"})
    }
}
fn display_rows(mut rows: Vec<Value>) -> Vec<Value> {
    for row in &mut rows {
        row["balance"] = json!("0.00");
        row["other"] = json!("");
        row["splits"] = json!([]);
        row["more_splits"] = json!(0);
    }
    rows
}
pub fn screen(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    key: &str,
    aid: &str,
) -> FbResult<Value> {
    book.read(|q|{
 services::access(q,actor,c)?;let rec=reconciliation(q,c,key,aid)?;let a=account(q,c,aid)?;let card=a.section_balance()==crate::model::Side::Credit;let sign=if card{-1}else{1};let raw=open_lines(q,c,&rec)?;let total=raw.len();let ids=raw.iter().map(|r|s(r,"id").map(str::to_owned)).collect::<FbResult<BTreeSet<_>>>()?;
 let mut totals=figures(q,c,&rec)?;totals["cleared_debit_display"]=cash(signed(aggregate_cents(&totals["cleared_debit"])?,if card{-1}else{1})?);totals["cleared_credit_display"]=cash(signed(aggregate_cents(&totals["cleared_credit"])?,if card{1}else{-1})?);
 let rows=if total>900{vec![]}else{let v=register::build_selected(q,c,aid,&json!({}),1,900,Some(&ids))?;display_rows(v["rows"].as_array().ok_or_else(||FbError::validation("Invalid register rows."))?.clone())};
 let mut hidden_cleared=0;let mut history=0;let mut later=0;let mut st=q.prepare("SELECT s.state,j.date,l.debit,l.credit FROM banking_banklinestatus s JOIN ledger_journalline l ON l.id=s.journal_line_id JOIN ledger_journal j ON j.id=l.journal_id WHERE s.account_id=?1 AND j.status='POSTED'")?;
 for row in st.query_map([aid],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?)))?{let(state,d,debit,credit)=row?;if state=="C"&&d.as_str()>s(&rec,"statement_date")?&&rec["hide_later"]==true{hidden_cleared+=1;}
if state=="R"{history=add(history,add(debit,-credit)?)?;later+=usize::from(d.as_str()>s(&rec,"statement_date")?);}}
 let mut notes=vec![];history=signed(history,sign)?;let beginning=cents(&totals["beginning"])?;if history!=beginning{notes.push(format!("The reconciled lines of this register add up to {}, but the last reconciliation ended at {}. The beginning balance always comes from the last reconciliation; a difference means reconciled history was changed outside FreedomBooks. Undo the last reconciliation and reconcile it again, or ask for help.",Money::from_db(history),s(&totals,"beginning")?));}
 let reg=register::build_query(q,c,aid,&json!({"end":rec["statement_date"]}),1,1)?;if notes.is_empty()&&reg["totals"]["cleared"]!=totals["cleared"]{let mut reasons=vec![];if later>0{reasons.push(format!("{later} reconciled line{} dated after this statement date",if later!=1{"s"}else{""}));}
if rec["hide_later"]==false{reasons.push("cleared lines dated after the statement date are included (they are shown)".into());}notes.push(format!("The register shows a cleared balance of {} through {}{}",s(&reg["totals"],"cleared")?,s(&rec,"statement_date")?,if reasons.is_empty(){".".into()}else{format!(" because of {}.",reasons.join(" and "))}));}
 let mut rows=rows;for r in &mut rows{r["cents"]=json!(cents(if r["left"].is_null(){&r["right"]}else{&r["left"]})?);}let left=rows.iter().filter(|r|!r["left"].is_null()).cloned().collect::<Vec<_>>();let right=rows.iter().filter(|r|!r["right"].is_null()).cloned().collect::<Vec<_>>();
 Ok(json!({"rec":rec,"words":words(&a),"last":last(q,aid)?,"totals":totals,"total":total,"too_many":total>900,"cap":900,"rows":rows,"left":left,"right":right,"hidden_cleared":hidden_cleared,"notes":notes,"sign":sign}))
})
}
pub fn report(
    book: &mut Book,
    actor: UserId,
    c: CompanyId,
    key: &str,
    aid: &str,
) -> FbResult<Value> {
    book.read(|q|{
 services::access(q,actor,c)?;let rec=reconciliation(q,c,key,aid)?;let a=account(q,c,aid)?;let wording=words(&a);let mut st=q.prepare("SELECT journal_line_id FROM banking_reconciliationline WHERE reconciliation_id=?1")?;let keys=st.query_map([key],|r|r.get::<_,String>(0))?.collect::<Result<BTreeSet<_>,_>>()?;
 let v=register::build_selected(q,c,aid,&json!({}),1,keys.len().max(1),Some(&keys))?;let rows=display_rows(v["rows"].as_array().ok_or_else(||FbError::validation("Invalid register rows."))?.clone());let mut sides=vec![];
 for side in["left","right"]{let mut items=rows.iter().filter(|r|!r[side].is_null()).cloned().collect::<Vec<_>>();for row in &items{for key in ["type","date","number","id"]{s(row,key)?;}}items.sort_by(|a,b|["type","date","number","id"].iter().map(|k|a[*k].as_str().cmp(&b[*k].as_str())).find(|v| !v.is_eq()).unwrap_or(std::cmp::Ordering::Equal));let mut groups:Vec<Value>=vec![];let mut total=0;for row in &items{let n=cents(&row[side])?;total=add(total,n)?;if groups.last().is_none_or(|g|g["type"]!=row["type"]){groups.push(json!({"type":row["type"],"rows":[],"total":"0.00"}));}let g=groups.last_mut().ok_or_else(||FbError::validation("Invalid report group."))?;g["total"]=cash(add(aggregate_cents(&g["total"])?,n)?);g["rows"].as_array_mut().ok_or_else(||FbError::validation("Invalid report rows."))?.push(row.clone());}
 let negative=(side=="left")== (a.section_balance()==crate::model::Side::Debit);let label=s(&wording,side)?;sides.push(json!({"key":side,"label":label,"groups":groups,"count":items.len(),"total":cash(total),"signed":cash(signed(total,if negative{-1}else{1})?),"summary_label":format!("Cleared {}{}",label.to_lowercase(),if negative{" (−)"}else{""})}));}
 let mut net=0;let mut od=0;let mut oc=0;let mut st=q.prepare("SELECT l.debit,l.credit,coalesce(s.state,'U') FROM ledger_journalline l JOIN ledger_journal j ON j.id=l.journal_id LEFT JOIN banking_banklinestatus s ON s.journal_line_id=l.id WHERE l.account_id=?1 AND j.company_id=?2 AND j.status='POSTED' AND j.date<=?3")?;
 for row in st.query_map(params![aid,c.to_db(),s(&rec,"statement_date")?],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?)))?{let(d,cr,state)=row?;net=add(net,add(d,-cr)?)?;if state!="R"{od=add(od,d)?;oc=add(oc,cr)?;}}
 Ok(json!({"rec":rec,"words":wording,"rows":rows,"sides":sides,"summary":{"register_balance":cash(signed(net,if a.section_balance()==crate::model::Side::Debit{1}else{-1})?),"open_debit":cash(od),"open_credit":cash(oc)}}))
})
}
