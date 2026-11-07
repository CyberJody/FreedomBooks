use super::*;
fn load(app: &mut App, c: CompanyId, id: &str) -> AppResult<Value> {
    let mut v=scoped_rows(app,c,"SELECT d.*,i.number FROM banking_deposit d JOIN documents_documentidentity i ON i.id=d.identity_id WHERE d.company_id=? AND d.id=?",&[&c.to_db(),&id])?.pop().ok_or_else(||AppError::NotFound("deposit".into()))?;
    let data: Value = serde_json::from_str(s(&v, "data")).expect("deposit evidence");
    for (k, val) in data.as_object().unwrap() {
        v[k] = val.clone();
    }
    v["pk"] = v["id"].clone();
    v["identity"] = json!({"number":v["number"]});
    let mut reversal = scoped_rows(
        app,
        c,
        "SELECT * FROM banking_depositreversal WHERE deposit_id=?",
        &[&id],
    )?
    .pop()
    .unwrap_or(Value::Null);
    if !reversal.is_null() {
        let data: Value =
            serde_json::from_str(s(&reversal, "data")).expect("deposit reversal evidence");
        for (k, val) in data.as_object().unwrap() {
            reversal[k] = val.clone();
        }
    }
    v["reversed"] = json!(!reversal.is_null());
    v["reversal"] = reversal;
    let bank = scoped_rows(
        app,
        c,
        "SELECT code,name FROM ledger_account WHERE company_id=? AND id=?",
        &[&c.to_db(), &s(&v, "bank_id")],
    )?
    .pop()
    .ok_or_else(|| AppError::NotFound("bank".into()))?;
    v["bank"] = bank;
    for (key, action) in [
        ("created_at", "deposit.recorded"),
        ("reversal", "deposit.reversed"),
    ] {
        let stamp=scoped_rows(app,c,"SELECT occurred_at FROM ledger_auditevent WHERE company_id=? AND target='deposit:'||? AND action=? ORDER BY id LIMIT 1",&[&c.to_db(),&id,&action])?.pop().unwrap_or(Value::Null);
        if key == "created_at" {
            v[key] = stamp["occurred_at"].clone();
        } else if !v[key].is_null() {
            v[key]["created_at"] = stamp["occurred_at"].clone();
        }
    }
    Ok(v)
}
fn prior(app: &mut App, c: CompanyId, p: Option<Posted<'_>>) -> AppResult<Option<Value>> {
    let key = p
        .and_then(|p| get(p, "request_key"))
        .and_then(py::py_uuid)
        .map(|v| v.0)
        .unwrap_or_default();
    let row=scoped_rows(app,c,"SELECT d.id FROM banking_deposit d JOIN documents_documentidentity i ON i.id=d.identity_id WHERE d.company_id=? AND i.request_key=?",&[&c.to_db(),&key])?.pop();
    row.map(|r| load(app, c, s(&r, "id"))).transpose()
}
fn eligible(app: &mut App, c: CompanyId, prior: Option<&Value>) -> AppResult<Vec<Value>> {
    let id = prior.map(|v| s(v, "id")).unwrap_or("");
    let raw = scoped_rows(
        app,
        c,
        "SELECT r.data,a.code AS account_code,a.name AS account_name,i.number FROM sales_receipt r JOIN documents_documentidentity i ON i.id=r.identity_id JOIN ledger_account a ON a.id=r.destination_account_id WHERE r.company_id=? AND r.destination='UNDEPOSITED' AND ((NOT EXISTS(SELECT 1 FROM sales_receiptreversal z WHERE z.receipt_id=r.id) AND NOT EXISTS(SELECT 1 FROM banking_depositline l WHERE l.receipt_id=r.id AND NOT EXISTS(SELECT 1 FROM banking_depositreversal z WHERE z.deposit_id=l.deposit_id))) OR EXISTS(SELECT 1 FROM banking_depositline l WHERE l.receipt_id=r.id AND l.deposit_id=?)) ORDER BY r.date,r.id",
        &[&c.to_db(), &id],
    )?;
    Ok(raw
        .into_iter()
        .map(|row| {
            let mut v: Value = serde_json::from_str(s(&row, "data")).expect("receipt evidence");
            v["pk"] = v["id"].clone();
            v["identity"] = json!({"number":row["number"]});
            v["destination_account"] =
                json!({"code":row["account_code"],"name":row["account_name"]});
            v["get_kind_display"] = json!(if v["kind"] == "SR" {
                "Sales receipt"
            } else {
                "Customer receipt"
            });
            v["get_method_display"] = json!(match s(&v, "method") {
                "CASH" => "Cash",
                "CHECK" => "Check",
                "CARD" => "Card (record only)",
                "TRANSFER" => "Transfer (record only)",
                _ => "Other",
            });
            v
        })
        .collect())
}
fn fields(app: &mut App, c: CompanyId, prior: Option<&Value>) -> AppResult<Vec<Field>> {
    let opts = banks(
        app,
        c,
        &prior
            .map(|v| s(v, "bank_id"))
            .into_iter()
            .collect::<Vec<_>>(),
    )?
    .iter()
    .map(|a| (a.id.to_db(), a.label()))
    .collect();
    Ok(vec![
        Field::new("request_key", "Request key", Widget::Hidden).required(),
        Field::new("review_token", "Review token", Widget::Hidden),
        select(
            "bank",
            "Deposit to designated bank",
            "Choose a bank",
            opts,
            true,
        ),
        Field::new("date", "Deposit date", Widget::Date).required(),
        Field::new("reference", "Reference (slip number or note)", Widget::Text)
            .help("Reference only. Never enter account numbers, credentials or secrets."),
        Field::new("memo", "Memo", Widget::Textarea { cols: 40, rows: 3 }),
        Field::new(
            "receipts",
            "Whole receipts to deposit",
            Widget::Checkboxes {
                choices: vec![],
                selected: vec![],
            },
        )
        .required(),
    ])
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
    let receipts = eligible(app, c, prior.as_ref())?;
    put(
        &mut vm,
        "form",
        bind_fields(
            fs,
            p,
            &json!({"request_key":uuid::Uuid::new_v4().to_string(),"date":(app.today)().to_string()}),
            e,
            true,
        ),
    );
    put(&mut vm, "receipt_rows", receipts);
    put(
        &mut vm,
        "selected",
        p.map(|p| {
            p.iter()
                .filter(|(k, _)| k == "receipts")
                .map(|(_, v)| v.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default(),
    );
    put(&mut vm, "review", review);
    put(&mut vm, "csrf_token", token(app, path));
    draw(app, "banking/deposits/form.html", &vm)
}
fn listing(app: &mut App, r: &Route, p: Posted<'_>) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let mut vm = ctx(app, r)?;
    let banks = scoped_rows(
        app,
        c,
        "SELECT a.id,a.code,a.name FROM ledger_account a WHERE a.company_id=? AND a.id IN(SELECT bank_id FROM banking_deposit WHERE company_id=?) ORDER BY a.code,a.id",
        &[&c.to_db(), &c.to_db()],
    )?;
    let fs = vec![
        Field::new("q", "Number, reference or memo", Widget::Text).attr("maxlength", "120"),
        select(
            "bank",
            "Bank",
            "All banks",
            banks
                .iter()
                .map(|a| {
                    (
                        s(a, "id").into(),
                        format!("{} · {}", s(a, "code"), s(a, "name")),
                    )
                })
                .collect(),
            false,
        ),
        select(
            "state",
            "State",
            "All deposits",
            vec![
                ("LIVE".into(), "Recorded".into()),
                ("REVERSED".into(), "Reversed".into()),
            ],
            false,
        ),
    ];
    let mut e = Errors::new();
    let q = clean_text(p, "q", 120, false, true, &mut e);
    let bank = choice(p, &fs[1], &mut e, true);
    let state = choice(p, &fs[2], &mut e, false);
    let mut records = vec![];
    if e.is_empty() {
        for id in scoped_rows(
            app,
            c,
            "SELECT id FROM banking_deposit WHERE company_id=? ORDER BY date DESC,id DESC",
            &[&c.to_db()],
        )? {
            let v = load(app, c, s(&id, "id"))?;
            let upper = fb_db::collation::upper(&q);
            if (!q.is_empty()
                && ![s(&v, "number"), s(&v, "reference"), s(&v, "memo")]
                    .iter()
                    .any(|s| fb_db::collation::upper(s).contains(&upper)))
                || (!bank.is_empty() && v["bank_id"] != bank)
                || (!state.is_empty() && (v["reversed"] == true) != (state == "REVERSED"))
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
    put(&mut vm, "query_string", query_string(p));
    Ok((
        draw(app, "banking/deposits/list.html", &vm)?,
        if e.is_empty() { 200 } else { 400 },
    ))
}
fn detail(app: &mut App, r: &Route) -> AppResult<String> {
    let c = arg_company(r)?;
    let v = load(app, c, &r.args["deposit_id"])?;
    let mut vm = ctx(app, r)?;
    let raw = scoped_rows(
        app,
        c,
        "SELECT l.*,r.data AS receipt_data,i.number FROM banking_depositline l JOIN sales_receipt r ON r.id=l.receipt_id JOIN documents_documentidentity i ON i.id=r.identity_id WHERE l.deposit_id=? ORDER BY l.id",
        &[&s(&v, "id")],
    )?;
    let mut lines = vec![];
    for mut l in raw {
        let mut receipt: Value =
            serde_json::from_str(s(&l, "receipt_data")).expect("receipt evidence");
        receipt["identity"] = json!({"number":l["number"]});
        receipt["get_kind_display"] = json!(if receipt["kind"] == "SR" {
            "Sales receipt"
        } else {
            "Customer receipt"
        });
        l["receipt"] = receipt;
        l["amount"] = json!(cash(i128::from(l["amount"].as_i64().unwrap())));
        lines.push(l);
    }
    let accounts = scoped_rows(
        app,
        c,
        "SELECT id,code,name FROM ledger_account WHERE company_id=? ORDER BY code,id",
        &[&c.to_db()],
    )?;
    put(&mut vm, "account_labels", accounts);
    put(&mut vm, "lines", lines);
    put(
        &mut vm,
        "references",
        references(app, c, s(&v, "identity_id"))?,
    );
    evidence(app, c, &mut vm)?;
    attachments(app, c, s(&v, "identity_id"), &mut vm)?;
    put(&mut vm, "deposit", v);
    draw(app, "banking/deposits/detail.html", &vm)
}
fn reverse_form(
    app: &mut App,
    r: &Route,
    path: &str,
    p: Option<Posted<'_>>,
    e: &Errors,
) -> AppResult<String> {
    let c = arg_company(r)?;
    let v = load(app, c, &r.args["deposit_id"])?;
    let mut vm = ctx(app, r)?;
    put(&mut vm, "deposit", v);
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
    draw(app, "banking/deposits/reverse.html", &vm)
}
pub(super) fn render(
    app: &mut App,
    r: &Route,
    p: Posted<'_>,
    path: &str,
) -> AppResult<(String, u16)> {
    match r.name.as_str() {
        "deposits:list" => listing(app, r, p),
        "deposits:new" => Ok((form(app, r, path, None, &Errors::new(), None)?, 200)),
        "deposits:detail" => Ok((detail(app, r)?, 200)),
        "deposits:reverse" => Ok((reverse_form(app, r, path, None, &Errors::new())?, 200)),
        _ => Err(AppError::NotFound(r.name.clone())),
    }
}
pub(super) fn submit(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(r)?;
    let mut e = Errors::new();
    if r.name == "deposits:reverse" {
        let _ = load(app, c, &r.args["deposit_id"])?;
        let (rev, date, reason) = reverse_values(p, &mut e);
        if e.is_empty()
            && result_error(
                app.run(Command::ReverseDeposit {
                    company_id: c.to_db(),
                    deposit_id: r.args["deposit_id"].clone(),
                    expected_revision: rev,
                    date,
                    reason,
                }),
                &mut e,
            )?
            .is_some()
        {
            return Ok(Outcome::Redirect {
                location: url("deposits:detail", &[&c.to_db(), &r.args["deposit_id"]]),
            });
        }
        return Ok(Outcome::Invalid {
            html: reverse_form(app, r, path, Some(p), &e)?,
        });
    }
    if r.name != "deposits:new" {
        return Err(AppError::NotFound(path.into()));
    }
    let prior = prior(app, c, Some(p))?;
    let fs = fields(app, c, prior.as_ref())?;
    let offered = eligible(app, c, prior.as_ref())?;
    let key = key(p, true, &mut e);
    let rt = clean_text(p, "review_token", 1000, false, true, &mut e);
    let bank = choice(p, &fs[2], &mut e, true);
    let date = clean_date(p, "date", true, &mut e).unwrap_or_default();
    let reference = clean_text(p, "reference", 120, false, true, &mut e);
    super::super::d2::refuse_card_number(p, "reference", &mut e);
    let memo = clean_text(p, "memo", 2000, false, true, &mut e);
    super::super::d2::refuse_card_number(p, "memo", &mut e);
    let ids = p
        .iter()
        .filter(|(k, _)| k == "receipts")
        .map(|(_, v)| v.clone())
        .collect::<Vec<_>>();
    if ids.is_empty() {
        error(
            &mut e,
            "receipts",
            "Select 1–50 whole undeposited receipts.",
        );
    } else if ids.iter().any(|id| !offered.iter().any(|r| r["id"] == *id)) {
        error(
            &mut e,
            "receipts",
            "Choose undeposited receipts belonging to this company.",
        );
    } else if ids.len() > 50 {
        error(
            &mut e,
            "receipts",
            "Select 1–50 whole undeposited receipts.",
        );
    }
    let mut review = None;
    if e.is_empty() {
        let values = DepositValues {
            request_key: key,
            bank_id: bank,
            date,
            reference,
            memo,
            receipt_ids: ids,
        };
        match get(p, "action").unwrap_or("review") {
            "record" => {
                if let Some(d) = result_error(
                    app.run(Command::RecordDeposit {
                        company_id: c.to_db(),
                        values,
                        review_token: rt,
                    }),
                    &mut e,
                )? {
                    return Ok(Outcome::Redirect {
                        location: url("deposits:detail", &[&c.to_db(), s(&d, "id")]),
                    });
                }
            }
            "review" => {
                review = result_error(
                    app.run(Command::ReviewDeposit {
                        company_id: c.to_db(),
                        values,
                    }),
                    &mut e,
                )?;
            }
            _ => error(&mut e, "__all__", "Choose Review or explicitly Record."),
        }
    }
    let mut p = p.to_vec();
    if let Some(r) = &review {
        p.retain(|(k, _)| k != "review_token");
        p.push(("review_token".into(), s(r, "token").into()));
    }
    let status = if review.is_some() { 200 } else { 400 };
    Ok(Outcome::Page {
        html: form(app, r, path, Some(&p), &e, review)?,
        status,
    })
}
