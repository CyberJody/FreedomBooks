use super::doc_forms::KindForm;
use super::*;
const STATES: &[(&str, &str)] = &[
    ("DRAFT", "Draft"),
    ("ACCEPTED", "Accepted"),
    ("DECLINED", "Declined"),
    ("CANCELLED", "Cancelled"),
];
pub(super) fn load(app: &mut App, r: &Route) -> AppResult<Value> {
    let c = arg_company(r)?;
    let key = &r.args["estimate_id"];
    let o = app.open_mut()?;
    let actor = o.owner;
    o.book.read(|tx|->Result<Value,FbError>{fb_core::services::access(tx,actor,c)?;let mut root=fb_core::sales::estimate(tx,c,key)?;root["pk"]=root["id"].clone();root["identity"]=identity(tx,root["identity_id"].as_str().unwrap())?;
 let mut st=tx.prepare("SELECT r.data,(SELECT a.occurred_at FROM ledger_auditevent a WHERE a.target='estimate:'||e.id AND json_extract(a.detail,'$.revision')=r.number ORDER BY a.id LIMIT 1) FROM sales_estimaterevision r JOIN sales_estimate e ON e.id=r.estimate_id WHERE e.id=?1 ORDER BY r.number DESC")?;
 let mut history=vec![];for row in st.query_map([key],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?)))?{let (raw,stamp)=row?;let mut h:Value=fb_db::parse_unique_json_as(&raw).map_err(|_|FbError::validation("Invalid stored estimate revision."))?;fixed_document(&mut h,&mut []);display_revision(&mut h);h["created_at"]=json!(stamp);history.push(h);}
 root["history"]=json!(history);root["current"]=root["history"][0].clone();let mut h=root["current"].clone();let mut lines=root["lines"].as_array().unwrap().clone();fixed_document(&mut h,&mut lines);root["lines"]=json!(lines);
 let link:Option<(String,i64,String)>=tx.query_row("SELECT x.invoice_id,r.number,i.identity_id FROM sales_estimateconversion x JOIN sales_estimaterevision r ON r.id=x.revision_id JOIN sales_invoice i ON i.id=x.invoice_id WHERE x.estimate_id=?1",[key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
 root["conversion"]=if let Some((id,n,source))=link{json!({"invoice_id":id,"invoice":{"identity":identity(tx,&source)?},"revision":{"number":n}})}else{Value::Null};Ok(root)
 }).map_err(AppError::from)
}
fn state_fields(p: Option<Posted<'_>>, rev: i64, e: &Errors) -> Vec<Field> {
    decorate(
        vec![
            Field::new("expected_revision", "Expected revision", Widget::Hidden)
                .required()
                .value_opt(super::super::d2::data_value(
                    p,
                    "expected_revision",
                    Some(rev.to_string()),
                )),
            Field::new(
                "state",
                "State",
                Widget::Select {
                    choices: STATES
                        .iter()
                        .map(|(a, b)| (a.to_string(), b.to_string()))
                        .collect(),
                },
            )
            .required()
            .value_opt(super::super::d2::data_value(p, "state", None)),
            Field::new("reason", "Reason", Widget::Textarea { cols: 40, rows: 2 })
                .attr("maxlength", "500")
                .value_opt(super::super::d2::data_value(p, "reason", None)),
        ],
        e,
    )
}
fn base(app: &mut App, r: &Route) -> AppResult<Value> {
    let mut vm = context(app, r)?;
    let root = load(app, r)?;
    put(&mut vm, "current", &root["current"]);
    put(&mut vm, "lines", &root["lines"]);
    super::attachments(app, arg_company(r)?, root["identity_id"].as_str().unwrap(), &mut vm)?;
    put(&mut vm, "estimate", root);
    Ok(vm)
}
fn detail(
    app: &mut App,
    r: &Route,
    p: Posted<'_>,
    posted: Option<Posted<'_>>,
    e: &Errors,
) -> AppResult<(String, u16)> {
    let mut vm = base(app, r)?;
    let c = arg_company(r)?;
    let state = vm["current"]["state"].as_str().unwrap();
    let choices = match state {
        "DRAFT" => vec!["ACCEPTED", "CANCELLED", "DECLINED"],
        "ACCEPTED" | "DECLINED" => vec!["CANCELLED", "DRAFT"],
        _ => vec![],
    };
    let attempted = posted.and_then(|p| get(p, "state"));
    let unavailable =
        attempted.filter(|s| STATES.iter().any(|(v, _)| v == s) && !choices.contains(s));
    put(&mut vm, "unavailable_state", unavailable);
    put(
        &mut vm,
        "transitions",
        choices
            .iter()
            .map(|s| STATES.iter().find(|(v, _)| v == s).unwrap())
            .collect::<Vec<_>>(),
    );
    let rev = vm["estimate"]["revision"].as_i64().unwrap();
    put(
        &mut vm,
        "state_form",
        forms::bind(&state_fields(posted, rev, e), &nonfield(e, "")),
    );
    let expired = vm["current"]["expiry"]
        .as_str()
        .is_some_and(|s| s < (app.today)().to_string().as_str());
    put(&mut vm, "expired", expired);
    let history = Page::of(
        vm["estimate"]["history"].as_array().unwrap(),
        20,
        get(p, "page"),
    );
    put(&mut vm, "history_page", history);
    put(
        &mut vm,
        "csrf_token",
        token(
            app,
            &url("sales:state", &[&c.to_db(), &r.args["estimate_id"]]),
        ),
    );
    Ok((
        draw(app, "sales/detail.html", &vm)?,
        if posted.is_some() { 400 } else { 200 },
    ))
}
pub(super) fn render(
    app: &mut App,
    r: &Route,
    p: Posted<'_>,
    path: &str,
) -> AppResult<(String, u16)> {
    match r.name.as_str() {
        "sales:new" => doc_forms::page(
            app,
            r,
            path,
            KindForm::Estimate,
            None,
            None,
            &Errors::new(),
            &[],
        ),
        "sales:edit" => {
            let root = load(app, r)?;
            doc_forms::page(
                app,
                r,
                path,
                KindForm::Estimate,
                Some(&root),
                None,
                &Errors::new(),
                &[],
            )
        }
        "sales:detail" => detail(app, r, p, None, &Errors::new()),
        "sales:revision" => {
            let mut vm = base(app, r)?;
            let n = clean_integer(&r.args["number"]).unwrap_or(0);
            let mut h = vm["estimate"]["history"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["number"] == n)
                .cloned()
                .ok_or_else(|| AppError::NotFound(path.into()))?;
            let id = h["id"].as_str().unwrap().to_owned();
            let mut lines = app.open_mut()?.book.read(|tx| {
                let mut st = tx.prepare(
                    "SELECT data FROM sales_estimateline WHERE revision_id=?1 ORDER BY position",
                )?;
                st.query_map([id], |r| r.get::<_, String>(0))?
                    .map(|r| r.map(|s| fb_db::parse_unique_json_as::<Value>(&s).unwrap()))
                    .collect::<Result<Vec<_>, _>>()
            })?;
            fixed_document(&mut h, &mut lines);
            put(&mut vm, "current", h);
            put(&mut vm, "lines", lines);
            Ok((draw(app, "sales/revision.html", &vm)?, 200))
        }
        "sales:list" => listing(app, r, p),
        _ => Err(AppError::NotFound(path.into())),
    }
}
fn listing(app: &mut App, r: &Route, p: Posted<'_>) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let mut vm = context(app, r)?;
    let ch = choices::customers(&mut app.open_mut()?.book, c, true, None, false)?;
    let mut e = Errors::new();
    let q = clean_text(p, "q", 120, false, true, &mut e);
    let customer = get(p, "customer").unwrap_or("");
    if !customer.is_empty() && !ch.iter().any(|(id, _)| id == customer) {
        error(
            &mut e,
            "customer",
            "Select a valid choice. That choice is not one of the available choices.",
        );
    }
    let state = get(p, "state").unwrap_or("");
    if !state.is_empty() && !STATES.iter().any(|(v, _)| *v == state) {
        error(
            &mut e,
            "state",
            format!("Select a valid choice. {state} is not one of the available choices."),
        );
    }
    let start = clean_date(p, "start", false, &mut e);
    let end = clean_date(p, "end", false, &mut e);
    let mut customer_choices = vec![("".into(), "Choose…".into())];
    customer_choices.extend(ch);
    let mut states = vec![("".into(), "All states".into())];
    states.extend(STATES.iter().map(|(a, b)| (a.to_string(), b.to_string())));
    let fields = decorate(
        vec![
            Field::new("q", "Search number / customer / memo", Widget::Text)
                .attr("maxlength", "120")
                .value(get(p, "q").unwrap_or("")),
            Field::new("state", "State", Widget::Select { choices: states }).value(state),
            Field::new(
                "customer",
                "Customer",
                Widget::Select {
                    choices: customer_choices,
                },
            )
            .value(customer),
            Field::new("start", "Start", Widget::Date)
                .value_opt(get(p, "start").filter(|s| !s.is_empty()).map(str::to_owned)),
            Field::new("end", "End", Widget::Date)
                .value_opt(get(p, "end").filter(|s| !s.is_empty()).map(str::to_owned)),
        ],
        &e,
    );
    let mut rows=app.open_mut()?.book.read(|tx|->Result<Vec<Value>,FbError>{let mut st=tx.prepare("SELECT r.data,e.id,e.identity_id FROM sales_estimaterevision r JOIN sales_estimate e ON e.id=r.estimate_id AND e.revision=r.number WHERE e.company_id=?1 ORDER BY r.date DESC,r.id DESC")?;let raw=st.query_map([c.to_db()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?.collect::<Result<Vec<_>,_>>()?;raw.into_iter().map(|(raw,id,source)|{let mut v:Value=fb_db::parse_unique_json_as(&raw).map_err(|_|FbError::validation("Invalid stored estimate."))?;display_revision(&mut v);v["estimate_id"]=json!(id);v["estimate"]=json!({"identity":identity(tx,&source)?});Ok(v)}).collect()})?;
    rows.retain(|v| {
        e.is_empty()
            && (customer.is_empty() || v["customer_id"] == customer)
            && (state.is_empty() || v["state"] == state)
            && start
                .as_ref()
                .is_none_or(|s| v["date"].as_str().unwrap() >= s.as_str())
            && end
                .as_ref()
                .is_none_or(|s| v["date"].as_str().unwrap() <= s.as_str())
            && (q.is_empty()
                || [
                    v["estimate"]["identity"]["number"].as_str().unwrap(),
                    v["customer_snapshot"]["display_name"].as_str().unwrap(),
                    v["memo"].as_str().unwrap(),
                ]
                .iter()
                .any(|s| s.to_lowercase().contains(&q.to_lowercase())))
    });
    put(&mut vm, "page", Page::of(&rows, 25, get(p, "page")));
    put(&mut vm, "filter_form", forms::bind(&fields, &[]));
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
        draw(app, "sales/list.html", &vm)?,
        if e.is_empty() { 200 } else { 400 },
    ))
}
pub(super) fn submit(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    if r.name == "sales:new" || r.name == "sales:edit" {
        let root = if r.name == "sales:edit" {
            Some(load(app, r)?)
        } else {
            None
        };
        return doc_forms::submit(app, r, path, KindForm::Estimate, root.as_ref(), p);
    }
    let mut e = Errors::new();
    let rev = clean_revision(p, 1, &mut e);
    let state = get(p, "state").unwrap_or("");
    if state.is_empty() {
        error(&mut e, "state", "This field is required.");
    } else if !STATES.iter().any(|(v, _)| *v == state) {
        error(
            &mut e,
            "state",
            format!("Select a valid choice. {state} is not one of the available choices."),
        );
    }
    let reason = clean_text(p, "reason", 500, false, true, &mut e);
    if e.is_empty() {
        match app.run(Command::TransitionEstimate {
            company_id: arg_company(r)?.to_db(),
            estimate_id: r.args["estimate_id"].clone(),
            expected_revision: rev,
            state: state.into(),
            reason,
        }) {
            Ok(_) => {
                return Ok(Outcome::Redirect {
                    location: url(
                        "sales:detail",
                        &[&arg_company(r)?.to_db(), &r.args["estimate_id"]],
                    ),
                });
            }
            Err(AppError::Core(FbError::Validation(messages))) => {
                error(&mut e, "", messages.join("; "))
            }
            Err(e) => return Err(e),
        }
    }
    Ok(Outcome::Invalid {
        html: detail(app, r, &[], Some(p), &e)?.0,
    })
}
