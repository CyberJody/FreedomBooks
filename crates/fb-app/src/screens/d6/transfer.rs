use super::*;
fn record(app: &mut App, c: CompanyId, id: &str) -> AppResult<Value> {
    let mut v=scoped_rows(app,c,"SELECT t.*,i.number FROM transfers_transfer t JOIN documents_documentidentity i ON i.id=t.identity_id WHERE t.company_id=? AND t.id=?",&[&c.to_db(),&id])?.pop().ok_or_else(||AppError::NotFound("transfer".into()))?;
    json_columns(&mut v);
    v["pk"] = v["id"].clone();
    v["identity"] = json!({"number":v["number"]});
    v["amount"] = json!(cash(v["amount"].as_i64().unwrap() as i128));
    let void = scoped_rows(
        app,
        c,
        "SELECT * FROM transfers_transfervoid WHERE transfer_id=?",
        &[&id],
    )?
    .pop();
    v["voided"] = json!(void.is_some());
    v["state_label"] = json!(if void.is_some() { "Voided" } else { "Recorded" });
    v["void"] = json!(void);
    Ok(v)
}
fn fields(app: &mut App, c: CompanyId, prior: Option<&Value>) -> AppResult<Vec<Field>> {
    let extra = prior
        .map(|p| vec![s(p, "from_account_id"), s(p, "to_account_id")])
        .unwrap_or_default();
    let opts = banks(app, c, &extra)?
        .iter()
        .map(|a| {
            (
                a.id.to_db(),
                format!(
                    "{} · {}{}",
                    a.code,
                    a.name,
                    if a.active && a.postable {
                        ""
                    } else {
                        " (inactive)"
                    }
                ),
            )
        })
        .collect::<Vec<_>>();
    Ok(vec![
        Field::new("request_key", "Request key", Widget::Hidden).required(),
        Field::new("review_token", "Review token", Widget::Hidden),
        Field::new("date", "Date", Widget::Date).required(),
        select(
            "from_account",
            "Transfer funds from",
            "Choose…",
            opts.clone(),
            true,
        ),
        select("to_account", "Transfer funds to", "Choose…", opts, true),
        Field::new("amount", "Transfer amount", Widget::Text)
            .attr("inputmode", "decimal")
            .attr("placeholder", "0.00")
            .attr("maxlength", "32")
            .required(),
        Field::new("memo", "Memo", Widget::Textarea { cols: 40, rows: 2 }),
    ])
}
fn prior(app: &mut App, c: CompanyId, p: Option<Posted<'_>>) -> AppResult<Option<Value>> {
    let key = p
        .and_then(|p| get(p, "request_key"))
        .and_then(py::py_uuid)
        .map(|v| v.0)
        .unwrap_or_default();
    Ok(scoped_rows(app,c,"SELECT t.* FROM transfers_transfer t JOIN documents_documentidentity i ON i.id=t.identity_id WHERE t.company_id=? AND i.request_key=?",&[&c.to_db(),&key])?.pop())
}
fn form(
    app: &mut App,
    r: &Route,
    path: &str,
    p: Option<Posted<'_>>,
    e: &Errors,
    review: Option<Value>,
) -> AppResult<String> {
    let c = arg_company(r)?;
    let mut vm = ctx(app, r)?;
    let prior = prior(app, c, p)?;
    let fs = fields(app, c, prior.as_ref())?;
    let banks = banks(app, c, &[])?;
    let mut initial = json!({"request_key":uuid::Uuid::new_v4().to_string(),"date":(app.today)().to_string(),"memo":"Funds transfer"});
    if banks.len() >= 2 {
        initial["from_account"] = json!(banks[0].id.to_db());
        initial["to_account"] = json!(banks[1].id.to_db());
    }
    put(&mut vm, "form", bind_fields(fs, p, &initial, e, true));
    put(&mut vm, "csrf_token", token(app, path));
    put(&mut vm, "review", review);
    put(&mut vm, "too_few_banks", banks.len() < 2);
    draw(app, "transfers/form.html", &vm)
}
fn filter_fields(app: &mut App, c: CompanyId) -> AppResult<Vec<Field>> {
    // SIM-FIX item 3 (deliberate divergence): v1 offered every asset account here. Offer the designated
    // banks, as the entry form does, plus any account a recorded transfer used (a bank that has since
    // been undesignated), so every existing transfer stays findable.
    let used = scoped_rows(
        app,
        c,
        "SELECT from_account_id AS id FROM transfers_transfer WHERE company_id=? UNION SELECT to_account_id FROM transfers_transfer WHERE company_id=?",
        &[&c.to_db(), &c.to_db()],
    )?;
    let used = used.iter().map(|v| s(v, "id")).collect::<Vec<_>>();
    let a = banks(app, c, &used)?;
    Ok(vec![
        Field::new("q", "Number or memo", Widget::Text).attr("maxlength", "120"),
        select(
            "account",
            "Bank (from or to)",
            "All banks",
            a.iter().map(|a| (a.id.to_db(), a.label())).collect(),
            false,
        ),
        select(
            "state",
            "State",
            "All transfers",
            vec![
                ("LIVE".into(), "Recorded".into()),
                ("VOID".into(), "Voided".into()),
            ],
            false,
        ),
        Field::new("start", "From", Widget::Date),
        Field::new("end", "Through", Widget::Date),
    ])
}
fn listing(app: &mut App, r: &Route, p: Posted<'_>) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let mut vm = ctx(app, r)?;
    let fs = filter_fields(app, c)?;
    let mut e = Errors::new();
    let q = clean_text(p, "q", 120, false, true, &mut e);
    let account = choice(p, &fs[1], &mut e, true);
    let state = choice(p, &fs[2], &mut e, false);
    let start = clean_date(p, "start", false, &mut e);
    let end = clean_date(p, "end", false, &mut e);
    if start.as_ref().zip(end.as_ref()).is_some_and(|(a, b)| a > b) {
        error(
            &mut e,
            "__all__",
            "The start date must not be after the end date.",
        );
    }
    let mut records = vec![];
    if e.is_empty() {
        let ids = scoped_rows(
            app,
            c,
            "SELECT id FROM transfers_transfer WHERE company_id=? ORDER BY date DESC,id DESC",
            &[&c.to_db()],
        )?;
        for id in ids {
            let v = record(app, c, s(&id, "id"))?;
            let upper = fb_db::collation::upper(&q);
            if (!q.is_empty()
                && !fb_db::collation::upper(s(&v, "number")).contains(&upper)
                && !fb_db::collation::upper(s(&v, "memo")).contains(&upper))
                || (!account.is_empty()
                    && v["from_account_id"] != account
                    && v["to_account_id"] != account)
                || (!state.is_empty() && (v["voided"] == true) != (state == "VOID"))
                || start.as_deref().is_some_and(|d| s(&v, "date") < d)
                || end.as_deref().is_some_and(|d| s(&v, "date") > d)
            {
                continue;
            }
            records.push(v);
        }
    }
    put(
        &mut vm,
        "filter_form",
        bind_fields(fs, Some(p), &Value::Null, &e, true),
    );
    put(&mut vm, "page", Page::of(&records, 25, get(p, "page")));
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
    Ok((
        draw(app, "transfers/list.html", &vm)?,
        if e.is_empty() { 200 } else { 400 },
    ))
}
pub(super) fn render(
    app: &mut App,
    r: &Route,
    p: Posted<'_>,
    path: &str,
) -> AppResult<(String, u16)> {
    match r.name.as_str() {
        "transfers:new" => Ok((form(app, r, path, None, &Errors::new(), None)?, 200)),
        "transfers:list" => listing(app, r, p),
        "transfers:detail" => {
            let c = arg_company(r)?;
            let v = record(app, c, &r.args["transfer_id"])?;
            let mut vm = ctx(app, r)?;
            put(&mut vm, "transfer", &v);
            put(&mut vm, "void", &v["void"]);
            put(
                &mut vm,
                "references",
                references(app, c, s(&v, "identity_id"))?,
            );
            let mut lines = scoped_rows(
                app,
                c,
                "SELECT l.debit,l.credit,a.code,a.name FROM ledger_journalline l JOIN ledger_account a ON a.id=l.account_id WHERE l.journal_id=? ORDER BY l.position",
                &[&s(&v, "journal_id")],
            )?;
            for l in &mut lines {
                l["account"] = json!(format!("{} · {}", s(l, "code"), s(l, "name")));
                for k in ["debit", "credit"] {
                    let n = l[k].as_i64().unwrap();
                    l[k] = if n == 0 {
                        Value::Null
                    } else {
                        json!(cash(n as i128))
                    };
                }
            }
            put(&mut vm, "lines", lines);
            evidence(app, c, &mut vm)?;
            attachments(app, c, s(&v, "identity_id"), &mut vm)?;
            Ok((draw(app, "transfers/detail.html", &vm)?, 200))
        }
        "transfers:void" => Ok((void_form(app, r, path, None, &Errors::new())?, 200)),
        _ => Err(AppError::NotFound(r.name.clone())),
    }
}
fn void_form(
    app: &mut App,
    r: &Route,
    path: &str,
    p: Option<Posted<'_>>,
    e: &Errors,
) -> AppResult<String> {
    let c = arg_company(r)?;
    let v = record(app, c, &r.args["transfer_id"])?;
    let mut vm = ctx(app, r)?;
    put(&mut vm, "transfer", v);
    put(
        &mut vm,
        "form",
        bind_fields(
            reversal_fields(),
            p,
            &json!({"expected_revision":1,"date":(app.today)().to_string()}),
            e,
            true,
        ),
    );
    put(&mut vm, "csrf_token", token(app, path));
    draw(app, "transfers/void.html", &vm)
}
pub(super) fn submit(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(r)?;
    let mut e = Errors::new();
    if r.name == "transfers:void" {
        let _ = record(app, c, &r.args["transfer_id"])?;
        let (rev, date, reason) = reverse_values(p, &mut e);
        if e.is_empty()
            && result_error(
                app.run(Command::VoidTransfer {
                    company_id: c.to_db(),
                    transfer_id: r.args["transfer_id"].clone(),
                    expected_revision: rev,
                    date,
                    reason,
                }),
                &mut e,
            )?
            .is_some()
        {
            return Ok(Outcome::Redirect {
                location: url("transfers:detail", &[&c.to_db(), &r.args["transfer_id"]]),
            });
        }
        return Ok(Outcome::Invalid {
            html: void_form(app, r, path, Some(p), &e)?,
        });
    }
    if r.name != "transfers:new" {
        return Err(AppError::NotFound(path.into()));
    }
    let prior = prior(app, c, Some(p))?;
    let fs = fields(app, c, prior.as_ref())?;
    let key = key(p, true, &mut e);
    let rt = clean_text(p, "review_token", 1000, false, true, &mut e);
    let date = clean_date(p, "date", true, &mut e).unwrap_or_default();
    let from = choice(p, &fs[3], &mut e, true);
    let to = choice(p, &fs[4], &mut e, true);
    let amount = clean_text(p, "amount", 32, true, true, &mut e);
    let memo = clean_text(p, "memo", 2000, false, true, &mut e);
    super::super::d2::refuse_card_number(p, "memo", &mut e);
    if !e.contains_key("from_account")
        && !e.contains_key("to_account")
        && !from.is_empty()
        && from == to
    {
        error(
            &mut e,
            "to_account",
            "Choose a different bank account than the one you transfer from.",
        );
    }
    let mut review = None;
    if e.is_empty() {
        let values = TransferValues {
            request_key: key,
            date,
            from_account_id: from,
            to_account_id: to,
            amount,
            memo,
        };
        let action = get(p, "action").unwrap_or("review");
        if action == "record" {
            if let Some(d) = result_error(
                app.run(Command::RecordTransfer {
                    company_id: c.to_db(),
                    values,
                    review_token: rt,
                }),
                &mut e,
            )? {
                return Ok(Outcome::Redirect {
                    location: url("transfers:detail", &[&c.to_db(), s(&d, "id")]),
                });
            }
        } else if action == "review" {
            review = result_error(
                app.run(Command::ReviewTransfer {
                    company_id: c.to_db(),
                    values,
                }),
                &mut e,
            )?;
        } else {
            error(&mut e, "__all__", "Choose Review or explicitly Record.");
        }
    }
    let mut fields = p.to_vec();
    if let Some(d) = &review {
        fields.retain(|(k, _)| k != "review_token");
        fields.push(("review_token".into(), s(d, "token").into()));
    }
    let status = if review.is_some() { 200 } else { 400 };
    let html = form(app, r, path, Some(&fields), &e, review)?;
    Ok(Outcome::Page { html, status })
}
