use super::*;
pub(super) fn raw(
    app: &mut App,
    c: CompanyId,
    table: &str,
    field: &str,
    id: &str,
) -> AppResult<Option<Value>> {
    let v = scoped_rows(
        app,
        c,
        &format!("SELECT data FROM {table} WHERE {field}=?"),
        &[&id],
    )?
    .pop();
    v.map(|v| {
        fb_db::parse_unique_json_as(s(&v, "data")).map_err(|e| AppError::Invalid(e.to_string()))
    })
    .transpose()
}
fn account(app: &mut App, c: CompanyId, id: &str) -> AppResult<Value> {
    let a = app
        .open_mut()?
        .book
        .read(|tx| {
            fb_core::store::account(
                tx,
                AccountId::parse(id).ok_or_else(|| FbError::missing("Account"))?,
            )
        })?
        .filter(|a| a.company_id == c)
        .ok_or_else(|| AppError::NotFound("Account".into()))?;
    Ok(json!({"code":a.code,"name":a.name}))
}
pub(in crate::screens) fn identity(app: &mut App, c: CompanyId, id: &str) -> AppResult<Value> {
    Ok(app
        .open_mut()?
        .book
        .read(|tx| fb_core::documents::identity(tx, c, id))?)
}
pub(super) fn invoice(app: &mut App, c: CompanyId, id: &str) -> AppResult<Value> {
    let o = app.open_mut()?;
    let mut v = fb_core::sales::invoice_balances(&mut o.book, o.owner, c)?
        .into_iter()
        .find(|v| v["invoice_id"] == id)
        .ok_or_else(|| AppError::NotFound("Invoice".into()))?;
    let d = app
        .open_mut()?
        .book
        .read(|tx| fb_core::sales::invoice(tx, c, id))?;
    v["invoice"] = json!({"identity":identity(app,c,s(&d,"identity_id"))?});
    v["customer_snapshot"] = d["current"]["customer_snapshot"].clone();
    v["identity_id"] = d["identity_id"].clone();
    Ok(v)
}
pub(in crate::screens) fn source(app: &mut App, c: CompanyId, id: &str, dep: bool) -> AppResult<Value> {
    let o = app.open_mut()?;
    let actor = o.owner;
    let mut v = if dep {
        prepayments::deposit_balances(&mut o.book, actor, c)?
            .into_iter()
            .find(|v| v["id"] == id)
            .ok_or_else(|| AppError::NotFound("Customer deposit".into()))?
    } else {
        let mut v = credits::get_credit_memo(&mut o.book, actor, c, id)?;
        let b = credits::memo_balances(&mut o.book, actor, c)?
            .into_iter()
            .find(|v| v["id"] == id)
            .ok_or_else(|| AppError::NotFound("Credit memo".into()))?;
        for (k, x) in b.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    };
    v["pk"] = v["id"].clone();
    v["identity"] = identity(app, c, s(&v, "identity_id"))?;
    v["has_live_usage"] = json!(money(&v["applied"]) != 0 || money(&v["refunded"]) != 0);
    v["available_positive"] = json!(money(&v["available"]) > 0);
    v["remaining_positive"] = json!(money(&v["remaining"]) > 0);
    let state = if dep { "deposit_state" } else { "credit_state" };
    v["is_void"] = json!(v[state] == "VOID");
    v["state_label"] = json!(match s(&v, state) {
        "AVAILABLE" =>
            if dep {
                "Unapplied"
            } else {
                "Credit available"
            },
        "PARTIAL" => "Partly used",
        "USED" => "Fully used",
        _ => "Voided",
    });
    let undo = raw(
        app,
        c,
        if dep {
            "sales_receiptreversal"
        } else {
            "credits_creditmemovoid"
        },
        if dep { "receipt_id" } else { "credit_memo_id" },
        id,
    )?
    .unwrap_or(Value::Null);
    v[if dep { "reversal" } else { "void" }] = undo;
    if dep {
        v["prepayment"]["liability"] = account(app, c, s(&v["prepayment"], "liability_id"))?;
        if let Some(id) = v["prepayment"]["estimate_id"].as_str() {
            let id = id.to_owned();
            let e = scoped_rows(
                app,
                c,
                "SELECT identity_id FROM sales_estimate WHERE company_id=? AND id=?",
                &[&c.to_db(), &id],
            )?
            .pop()
            .ok_or_else(|| AppError::NotFound("Estimate".into()))?;
            v["prepayment"]["estimate"] = json!({"identity":identity(app,c,s(&e,"identity_id"))?});
        }
        v["destination_account"] = account(app, c, s(&v, "destination_account_id"))?;
        v["get_method_display"] = json!(method(s(&v, "method"), false));
        v["get_destination_display"] = json!(if v["destination"] == "BANK" {
            "Direct to designated bank"
        } else {
            "Undeposited funds"
        });
    } else {
        v["receivables"] = account(app, c, s(&v, "receivables_id"))?;
        journal(app, c, &mut v)?;
        if !v["void"].is_null() {
            journal(app, c, &mut v["void"])?;
        }
        let mut lines = v["lines"].as_array().cloned().unwrap_or_default();
        super::super::d4::fixed_document(&mut v, &mut lines);
        v["lines"] = json!(lines);
    }
    Ok(v)
}
fn journal(app: &mut App, c: CompanyId, v: &mut Value) -> AppResult<()> {
    let j = scoped_rows(
        app,
        c,
        "SELECT number FROM ledger_journal WHERE company_id=? AND id=?",
        &[&c.to_db(), &s(v, "journal_id")],
    )?
    .pop()
    .ok_or_else(|| AppError::NotFound("Journal".into()))?;
    v["journal"] = json!({"label":format!("Journal J{:06}",j["number"].as_i64().unwrap())});
    Ok(())
}
pub(in crate::screens) fn method(s: &str, refund: bool) -> &str {
    match s {
        "CHECK" => "Check",
        "CASH" => "Cash",
        "CARD" => "Card (record only)",
        "TRANSFER" => "Transfer (record only)",
        _ => {
            if refund {
                "Other (record only)"
            } else {
                "Other"
            }
        }
    }
}
pub(super) fn application(app: &mut App, c: CompanyId, id: &str, dep: bool) -> AppResult<Value> {
    let prefix = if dep {
        "prepayments_deposit"
    } else {
        "credits_credit"
    };
    let mut v = raw(app, c, &format!("{prefix}application"), "id", id)?
        .filter(|v| v["company_id"] == c.to_db())
        .ok_or_else(|| AppError::NotFound("Application".into()))?;
    v["pk"] = v["id"].clone();
    v["unapplication"] = raw(
        app,
        c,
        &format!("{prefix}unapplication"),
        "application_id",
        id,
    )?
    .unwrap_or(Value::Null);
    v["live"] = json!(v["unapplication"].is_null());
    let source_field = if dep { "deposit_id" } else { "credit_memo_id" };
    let src = source(app, c, s(&v, source_field), dep)?;
    v[if dep { "deposit" } else { "credit_memo" }] = json!({"identity":src["identity"]});
    if dep {
        v["identity"] = identity(app, c, s(&v, "identity_id"))?;
        v["invoice"] = invoice(app, c, s(&v, "invoice_id"))?["invoice"].clone();
    } else {
        let mut lines = scoped_rows(
            app,
            c,
            "SELECT data FROM credits_creditapplicationline WHERE application_id=? ORDER BY id",
            &[&id],
        )?;
        for l in &mut lines {
            *l = fb_db::parse_unique_json_as(s(l, "data"))
                .map_err(|e| AppError::Invalid(e.to_string()))?;
            l["invoice"] = invoice(app, c, s(l, "invoice_id"))?["invoice"].clone();
        }
        v["lines"] = json!(lines);
    }
    Ok(v)
}
pub(super) fn refund(app: &mut App, c: CompanyId, id: &str, dep: bool) -> AppResult<Value> {
    let table = if dep {
        "prepayments_depositrefund"
    } else {
        "credits_creditrefund"
    };
    let mut v = raw(app, c, table, "id", id)?
        .filter(|v| v["company_id"] == c.to_db())
        .ok_or_else(|| AppError::NotFound("Refund".into()))?;
    v["pk"] = v["id"].clone();
    v["identity"] = identity(app, c, s(&v, "identity_id"))?;
    v["bank"] = account(app, c, s(&v, "bank_id"))?;
    v["void"] = raw(app, c, &format!("{table}void"), "refund_id", id)?.unwrap_or(Value::Null);
    v["voided"] = json!(!v["void"].is_null());
    v["get_method_display"] = json!(method(s(&v, "method"), true));
    v["number_display"] = json!(if v["method"] != "CHECK" {
        "—"
    } else if s(&v, "check_number").is_empty() {
        "No number"
    } else {
        s(&v, "check_number")
    });
    let src = source(
        app,
        c,
        s(&v, if dep { "deposit_id" } else { "credit_memo_id" }),
        dep,
    )?;
    v[if dep { "deposit" } else { "credit_memo" }] = json!({"identity":src["identity"]});
    Ok(v)
}
pub(super) fn listing(app: &mut App, r: &Route, p: Posted<'_>) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let dep = deposit(r);
    let mut vm = ctx(app, r)?;
    let fs = vec![
        Field::new(
            "q",
            if dep {
                "Number, reference or customer"
            } else {
                "Number, memo or customer"
            },
            Widget::Text,
        )
        .attr("maxlength", "120"),
        select(
            "customer",
            "Customer",
            "Choose…",
            super::super::d4::customer_choices(&mut app.open_mut()?.book, c, true, None, false)?,
            false,
        ),
        select(
            "state",
            "State",
            if dep {
                "All deposits"
            } else {
                "All credit memos"
            },
            vec![
                (
                    "AVAILABLE".into(),
                    if dep { "Unapplied" } else { "Credit available" }.into(),
                ),
                ("PARTIAL".into(), "Partly used".into()),
                ("USED".into(), "Fully used".into()),
                ("VOID".into(), "Voided".into()),
            ],
            false,
        ),
    ];
    let mut e = Errors::new();
    let q = clean_text(p, "q", 120, false, true, &mut e);
    let customer = choice(p, &fs[1], &mut e, true);
    let state = choice(p, &fs[2], &mut e, false);
    let table = if dep {
        "sales_receipt"
    } else {
        "credits_creditmemo"
    };
    let ids = scoped_rows(
        app,
        c,
        &format!(
            "SELECT id FROM {table} WHERE company_id=? {} ORDER BY date DESC,id DESC",
            if dep { "AND kind='CDP'" } else { "" }
        ),
        &[&c.to_db()],
    )?;
    let mut rows = vec![];
    let mut total = 0i128;
    if e.is_empty() {
        for id in ids {
            let v = source(app, c, s(&id, "id"), dep)?;
            let upper = fb_db::collation::upper(&q);
            if (!customer.is_empty() && v["customer_id"] != customer)
                || (!state.is_empty()
                    && v[if dep { "deposit_state" } else { "credit_state" }] != state)
                || (!q.is_empty()
                    && ![
                        s(&v["identity"], "number"),
                        if dep {
                            s(&v["prepayment"], "order_reference")
                        } else {
                            s(&v, "memo")
                        },
                        s(&v["customer_snapshot"], "display_name"),
                        if dep { s(&v, "reference") } else { "" },
                    ]
                    .iter()
                    .any(|s| fb_db::collation::upper(s).contains(&upper)))
            {
                continue;
            }
            total = total
                .checked_add(money(&v["remaining"]))
                .ok_or_else(|| AppError::Invalid("Deposit total overflow.".into()))?;
            rows.push(v);
        }
    }
    put(
        &mut vm,
        "filter_form",
        bind_fields(fs, Some(p), &Value::Null, &e, true),
    );
    put(&mut vm, "page", Page::of(&rows, 25, get(p, "page")));
    put(
        &mut vm,
        "query_string",
        urlencode(
            &p.iter()
                .filter(|(k, _)| k != "page")
                .cloned()
                .collect::<Vec<_>>(),
        ),
    );
    put(&mut vm, "unapplied", cash(total));
    let setup = app
        .open_mut()?
        .book
        .read(|tx| fb_core::documents::setup_snapshot(tx, c))?;
    put(
        &mut vm,
        "setup_ready",
        !setup["customer_deposits"].is_null(),
    );
    Ok((
        draw(app, &format!("{}/list.html", ns(r)), &vm)?,
        if e.is_empty() { 200 } else { 400 },
    ))
}
pub(super) fn detail(app: &mut App, r: &Route, p: Posted<'_>) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let dep = deposit(r);
    let id = &r.args[source_arg(r)];
    let src = source(app, c, id, dep)?;
    let mut vm = ctx(app, r)?;
    put(&mut vm, "state_label", &src["state_label"]);
    put(&mut vm, "lines", &src["lines"]);
    put(&mut vm, if dep { "deposit" } else { "memo" }, &src);
    let prefix = if dep {
        "prepayments_deposit"
    } else {
        "credits_credit"
    };
    let field = if dep { "deposit_id" } else { "credit_memo_id" };
    let ids = scoped_rows(
        app,
        c,
        &format!(
            "SELECT id FROM {prefix}application WHERE company_id=? AND {field}=? ORDER BY id DESC"
        ),
        &[&c.to_db(), &id],
    )?;
    let mut apps = vec![];
    for v in ids {
        apps.push(application(app, c, s(&v, "id"), dep)?);
    }
    let ids = scoped_rows(
        app,
        c,
        &format!("SELECT id FROM {prefix}refund WHERE company_id=? AND {field}=? ORDER BY id DESC"),
        &[&c.to_db(), &id],
    )?;
    let mut refunds = vec![];
    for v in ids {
        refunds.push(refund(app, c, s(&v, "id"), dep)?);
    }
    put(
        &mut vm,
        "applications",
        Page::of(&apps, 25, get(p, "applications_page")),
    );
    put(
        &mut vm,
        "refunds",
        Page::of(&refunds, 25, get(p, "refunds_page")),
    );
    put(
        &mut vm,
        "references",
        references(app, c, s(&src, "identity_id"))?,
    );
    put(&mut vm, "events", events(app, c, id, dep)?);
    if dep {
        let bank=scoped_rows(app,c,"SELECT d.id,d.date,d.identity_id FROM banking_deposit d JOIN banking_depositline l ON l.deposit_id=d.id WHERE d.company_id=? AND l.receipt_id=? AND NOT EXISTS(SELECT 1 FROM banking_depositreversal v WHERE v.deposit_id=d.id)",&[&c.to_db(),&id])?.pop();
        let bank = if let Some(mut b) = bank {
            b["pk"] = b["id"].clone();
            b["identity"] = identity(app, c, s(&b, "identity_id"))?;
            b
        } else {
            Value::Null
        };
        put(&mut vm, "bank_deposit", bank);
    }
    evidence(app, c, &mut vm)?;
    attachments(app, c, s(&src, "identity_id"), &mut vm)?;
    Ok((draw(app, &format!("{}/detail.html", ns(r)), &vm)?, 200))
}
fn events(app: &mut App, c: CompanyId, id: &str, dep: bool) -> AppResult<Vec<Value>> {
    let mut rows = scoped_rows(
        app,
        c,
        "SELECT occurred_at,action,actor_name,detail FROM ledger_auditevent WHERE company_id=? AND target=? ORDER BY occurred_at,id LIMIT 100",
        &[
            &c.to_db(),
            &format!("{}:{id}", if dep { "receipt" } else { "creditmemo" }),
        ],
    )?;
    for row in &mut rows {
        let d: Value = fb_db::parse_unique_json_as(s(row, "detail"))
            .map_err(|e| AppError::Invalid(e.to_string()))?;
        let detail = match s(row, "action") {
            "credit_memo.recorded" => format!(
                "{} · total {} · {} line(s)",
                s(&d, "number"),
                s(&d, "total"),
                d["lines"].as_array().map_or(0, Vec::len)
            ),
            "credit_memo.applied" => format!(
                "applied {} on {} to {}",
                s(&d, "amount"),
                s(&d, "date"),
                d["invoices"]
                    .as_array()
                    .map(|v| v
                        .iter()
                        .map(|i| format!("{} {}", s(i, "number"), s(i, "amount")))
                        .collect::<Vec<_>>()
                        .join(", "))
                    .unwrap_or_default()
            ),
            "credit_memo.unapplied" => format!(
                "application #{} unapplied on {} · reason: {}",
                ordinal(app, c, "credits_creditapplication", s(&d, "application_id"))?,
                s(&d, "date"),
                s(&d, "reason")
            ),
            "credit_memo.voided" => format!(
                "void dated {} · journal #{} · reason: {}",
                s(&d, "date"),
                ordinal(app, c, "ledger_journal", s(&d, "journal_id"))?,
                s(&d, "reason")
            ),
            "customer_deposit.recorded" => format!(
                "{} · amount {}{}",
                s(&d, "number"),
                s(&d, "amount"),
                if s(&d, "estimate").is_empty() {
                    String::new()
                } else {
                    format!(" · estimate {}", s(&d, "estimate"))
                }
            ),
            "customer_deposit.applied" => format!(
                "{} applied {} on {} to {}",
                s(&d, "application"),
                s(&d, "amount"),
                s(&d, "date"),
                s(&d, "invoice")
            ),
            "customer_deposit.unapplied" => format!(
                "{} unapplied on {} · reason: {}",
                s(&d, "application"),
                s(&d, "date"),
                s(&d, "reason")
            ),
            "customer_deposit.voided" => {
                let mut v = json!({"journal_id":d["journal_id"]});
                journal(app, c, &mut v)?;
                format!(
                    "void dated {} · {} · reason: {}",
                    s(&d, "date"),
                    s(&v["journal"], "label"),
                    s(&d, "reason")
                )
            }
            _ => String::new(),
        };
        row["detail"] = json!(detail);
    }
    Ok(rows)
}
fn ordinal(app: &mut App, c: CompanyId, table: &str, id: &str) -> AppResult<usize> {
    let v = scoped_rows(
        app,
        c,
        &format!("SELECT count(*) AS n FROM {table} WHERE company_id=? AND id<=?"),
        &[&c.to_db(), &id],
    )?;
    Ok(v[0]["n"].as_u64().unwrap_or(0) as usize)
}
pub(super) fn refund_detail(app: &mut App, r: &Route) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let v = refund(app, c, &r.args[refund_arg(r)], deposit(r))?;
    let mut vm = ctx(app, r)?;
    put(&mut vm, "refund", &v);
    put(
        &mut vm,
        "references",
        references(app, c, s(&v, "identity_id"))?,
    );
    evidence(app, c, &mut vm)?;
    attachments(app, c, s(&v, "identity_id"), &mut vm)?;
    Ok((
        draw(app, &format!("{}/refund_detail.html", ns(r)), &vm)?,
        200,
    ))
}
