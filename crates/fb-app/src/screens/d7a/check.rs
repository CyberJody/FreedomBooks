use super::*;
fn all(app: &mut App, c: CompanyId) -> AppResult<Vec<Value>> {
    let actor = app.open_mut()?.owner;
    Ok(checks::current_checks(&mut app.open_mut()?.book, actor, c)?)
}
fn load(app: &mut App, c: CompanyId, id: &str) -> AppResult<Value> {
    let actor = app.open_mut()?.owner;
    let mut v = checks::get_check(&mut app.open_mut()?.book, actor, c, id)?;
    v["pk"] = v["id"].clone();
    v["identity"] = json!({"number":v["number"]});
    Ok(v)
}
fn rev_vm(mut v: Value) -> Value {
    json_columns(&mut v);
    v["get_state_display"] = json!(match s(&v, "state") {
        "DRAFT" => "Draft",
        "RECORDED" => "Recorded",
        "VOID" => "Voided",
        x => x,
    });
    v
}
fn historical(app: &mut App, c: CompanyId, id: &str, n: i64) -> AppResult<Value> {
    let _ = load(app, c, id)?;
    let mut v = scoped_rows(
        app,
        c,
        "SELECT * FROM checks_checkrevision WHERE document_id=? AND number=?",
        &[&id, &n],
    )?
    .pop()
    .ok_or_else(|| AppError::NotFound("revision".into()))?;
    let mut lines = scoped_rows(
        app,
        c,
        "SELECT * FROM checks_checkline WHERE revision_id=? ORDER BY position",
        &[&s(&v, "id")],
    )?;
    let mut totals = json!({"EXPENSE":0i64,"ITEM":0i64});
    for l in &mut lines {
        json_columns(l);
        let tab = s(l, "tab").to_owned();
        totals[&tab] = json!(
            totals[&tab]
                .as_i64()
                .unwrap()
                .checked_add(l["amount"].as_i64().unwrap())
                .ok_or_else(|| AppError::Invalid("Check total overflow.".into()))?
        );
        for k in ["rate", "amount"] {
            l[k] = json!(cash(i128::from(l[k].as_i64().unwrap())));
        }
        l["quantity"] = json!(format!(
            "{}.{:04}",
            l["quantity"].as_i64().unwrap() / 10000,
            l["quantity"].as_i64().unwrap() % 10000
        ));
    }
    for k in ["EXPENSE", "ITEM"] {
        totals[k] = json!(cash(i128::from(totals[k].as_i64().unwrap())));
    }
    v["lines"] = json!(lines);
    v["tab_totals"] = totals;
    v["total"] = json!(cash(i128::from(v["total"].as_i64().unwrap())));
    v["print_later"] = json!(v["print_later"] == 1);
    v["number_display"] = json!(if !s(&v, "check_number").is_empty() {
        s(&v, "check_number")
    } else if v["print_later"] == true {
        "To print"
    } else {
        "No number"
    });
    Ok(rev_vm(v))
}
fn guidance(app: &mut App, c: CompanyId, vendor: &str) -> AppResult<Value> {
    if vendor.is_empty() {
        return Ok(Value::Null);
    }
    let actor = app.open_mut()?.owner;
    let count = fb_core::payables::current_bills(&mut app.open_mut()?.book, actor, c)?
        .iter()
        .filter(|b| {
            b["current"]["vendor_id"] == vendor
                && ["OPEN", "PARTIAL"].contains(&s(b, "payment_state"))
        })
        .count();
    Ok(if count > 0 {
        json!({"count":count,"vendor_id":vendor})
    } else {
        Value::Null
    })
}
fn context(app: &mut App, r: &Route) -> AppResult<Value> {
    let c = arg_company(r)?;
    let b = load(app, c, &r.args["check_id"])?;
    let mut vm = ctx(app, r)?;
    let v = rev_vm(b["current"].clone());
    put(&mut vm, "guidance", guidance(app, c, s(&v, "vendor_id"))?);
    put(&mut vm, "lines", &v["lines"]);
    put(&mut vm, "tab_totals", &v["tab_totals"]);
    put(&mut vm, "current", v);
    put(&mut vm, "check", b);
    Ok(vm)
}
fn bank_choices(
    app: &mut App,
    c: CompanyId,
    extra: Option<&str>,
) -> AppResult<Vec<(String, String)>> {
    let banks = banks(app, c, &extra.into_iter().collect::<Vec<_>>())?;
    let ord = app
        .open_mut()?
        .book
        .read(|tx| super::super::d2::ordinals(tx, c, "ledger_account"))?;
    Ok(banks
        .iter()
        .map(|a| {
            let id = a.id.to_db();
            (
                id.clone(),
                format!(
                    "{} · {} · #{}{}",
                    a.code,
                    a.name,
                    ord[&id],
                    if a.active && a.postable && a.kind == Kind::Asset {
                        ""
                    } else {
                        " (unavailable — replace)"
                    }
                ),
            )
        })
        .collect())
}
fn header_fields(app: &mut App, c: CompanyId, current: Option<&Value>) -> AppResult<Vec<Field>> {
    Ok(vec![
        Field::new("request_key","Request key",Widget::Hidden),
        Field::new("expected_revision","Expected revision",Widget::Hidden).required(),
        select("bank","Bank account","Choose…",bank_choices(app,c,current.map(|r|s(r,"bank_id")))?,true),
        Field::new("date","Date",Widget::Date).required(),
        Field::new("check_number","Check number (No.)",Widget::Text).help("Next number on this bank. Write checks, bill payments by check, and refund checks share one number sequence. A voided number can be used again. Print later leaves this blank until you assign the number used."),
        Field::new("print_later","Print later (leave the number blank until set)",Widget::Checkbox{checked:false}),
        Field::new("payee_kind","Pay to the order of",Widget::Radio{choices:vec![("VENDOR".into(),"Vendor".into()),("OTHER".into(),"Other payee (name only)".into())]}).required(),
        select("vendor","Vendor payee","Choose…",contacts(app,c,true,current.map(|r|s(r,"vendor_id")),"confirm")?,false),
        Field::new("payee_name","Other payee name (when not a vendor)",Widget::Text),
        Field::new("address","Address (printed on the check)",Widget::Textarea{cols:40,rows:3}),
        Field::new("memo","Memo",Widget::Textarea{cols:40,rows:2}),
        Field::new("refresh_payee","Update vendor name and address from the vendor list",Widget::Checkbox{checked:false}),
        Field::new("keep_archived","Keep archived vendor details on this check",Widget::Checkbox{checked:false}),
    ])
}
struct Choices {
    items: Vec<(String, String)>,
    accounts: Vec<Account>,
    eligible: Vec<String>,
    catalog: Value,
}
fn choices(app: &mut App, c: CompanyId, current: Option<&Value>) -> AppResult<Choices> {
    Ok(app.open_mut()?.book.read(|tx|->Result<_,FbError>{
        let ord=super::super::d2::ordinals(tx,c,"directory_item")?;let mut items=vec![];let mut catalog=json!({});
        for i in directory::list(tx,c,Model::Item)? {
            let id=s(&i,"id");let retained=current.and_then(|r|r["lines"].as_array()).is_some_and(|ls|ls.iter().any(|l|l["item_id"]==id));
            if i["active"]==true||retained{items.push((id.into(),format!("{} · #{}{}",s(&i,"code"),ord[id],if i["active"]==true{""}else{" (archived — confirm retention or replace)"})));}
            if i["active"]==true{catalog[id]=json!({"description":i["description"],"rate":i["purchase_cost"].as_str().unwrap_or(""),"account":i["expense_account"].as_str().unwrap_or("")});}
        }
        Ok(Choices{items,accounts:fb_core::store::accounts(tx,c)?,eligible:documents::eligible_line_accounts(tx,c)?.iter().map(|a|a.id.to_db()).collect(),catalog})
    })?)
}
fn line_fields(ch: &Choices, prefix: &str, keep: Option<&str>, tab: &str) -> Vec<Field> {
    let keep = keep.map(py::py_strip);
    let mut accounts = ch.accounts.iter().collect::<Vec<_>>();
    accounts.sort_by(|a, b| {
        a.detail_type
            .order()
            .cmp(&b.detail_type.order())
            .then(a.code.cmp(&b.code))
            .then(a.id.to_db().cmp(&b.id.to_db()))
    });
    let mut groups = vec![(String::new(), vec![(String::new(), "Choose…".into())])];
    let mut unavailable = vec![];
    for a in accounts {
        let id = a.id.to_db();
        if ch.eligible.contains(&id) {
            let group = a.detail_type.label();
            if groups.last().is_none_or(|(g, _)| g != group) {
                groups.push((group.into(), vec![]));
            }
            groups.last_mut().unwrap().1.push((id, a.label()));
        } else if Some(id.as_str()) == keep {
            unavailable.push((id, a.label()));
        }
    }
    if !unavailable.is_empty() {
        groups.push(("Unavailable — replace before posting".into(), unavailable));
    }
    vec![
        Field::new("tab", "Tab", Widget::Hidden),
        Field::new("origin_position", "Origin position", Widget::Hidden),
        Field::new(
            "account",
            "Account (required to record)",
            Widget::GroupedSelect { groups },
        ),
        select("item", "Item", "Choose…", ch.items.clone(), false),
        Field::new(
            "description",
            "Memo / description",
            Widget::Textarea { cols: 40, rows: 1 },
        ),
        Field::new("quantity", "Quantity", Widget::Text).value("1"),
        Field::new(
            "rate",
            if tab == "ITEM" {
                "Cost per unit"
            } else {
                "Amount"
            },
            Widget::Text,
        ),
        Field::new(
            "refresh_item",
            "Update item details from the item list",
            Widget::Checkbox { checked: false },
        ),
        Field::new(
            "keep_archived",
            "Keep archived item details on this line",
            Widget::Checkbox { checked: false },
        ),
        Field::new("DELETE", "Delete", Widget::Checkbox { checked: false }),
    ]
    .into_iter()
    .map(|f| f.prefix(prefix))
    .collect()
}
fn prior(
    app: &mut App,
    c: CompanyId,
    b: Option<&Value>,
    p: Option<Posted<'_>>,
) -> AppResult<Option<Value>> {
    if let Some(b) = b {
        return Ok(Some(b["current"].clone()));
    }
    if let Some(k) = p.and_then(|p| get(p, "request_key")).and_then(py::py_uuid)
        && let Some(b)=scoped_rows(app,c,"SELECT b.id FROM checks_check b JOIN documents_documentidentity i ON i.id=b.identity_id WHERE b.company_id=? AND i.request_key=?",&[&c.to_db(),&k.0])?.pop(){
        return Ok(Some(historical(app,c,s(&b,"id"),1)?));
    }
    Ok(None)
}
fn form(
    app: &mut App,
    r: &Route,
    path: &str,
    p: Option<Posted<'_>>,
    e: &Errors,
    nf: &[String],
) -> AppResult<String> {
    let c = arg_company(r)?;
    let b = r
        .args
        .get("check_id")
        .map(|id| load(app, c, id))
        .transpose()?;
    let current = prior(app, c, b.as_ref(), p)?;
    let mut fs = header_fields(app, c, current.as_ref())?;
    fs[0].required = b.is_none();
    let bs = banks(
        app,
        c,
        &b.as_ref()
            .map(|b| s(&b["current"], "bank_id"))
            .into_iter()
            .collect::<Vec<_>>(),
    )?;
    let mut init = json!({"request_key":uuid::Uuid::new_v4().to_string(),"expected_revision":0,"date":(app.today)().to_string(),"payee_kind":"VENDOR"});
    let mut initials = vec![];
    if let Some(b) = &b {
        let v = &b["current"];
        for k in [
            "date",
            "check_number",
            "print_later",
            "payee_kind",
            "payee_name",
            "address",
            "memo",
        ] {
            init[k] = v[k].clone();
        }
        init["bank"] = v["bank_id"].clone();
        init["vendor"] = v["vendor_id"].clone();
        init["expected_revision"] = b["revision"].clone();
        for l in v["lines"].as_array().unwrap() {
            let mut i = l.clone();
            i["origin_position"] = l["position"].clone();
            i["account"] = l["account_id"].clone();
            i["item"] = l["item_id"].clone();
            initials.push(i);
        }
    } else if let Some(bank) = bs.first() {
        let actor = app.open_mut()?.owner;
        init["bank"] = json!(bank.id.to_db());
        init["check_number"] = json!(checks::next_number(
            &mut app.open_mut()?.book,
            actor,
            c,
            bank.id
        )?);
    }
    let initial_count = initials.len();
    initials.extend([json!({"tab":"EXPENSE"}), json!({"tab":"ITEM"})]);
    let ch = choices(app, c, current.as_ref())?;
    let mut vm = ctx(app, r)?;
    if let Some(b) = b {
        put(&mut vm, "current", rev_vm(b["current"].clone()));
        put(&mut vm, "check", b);
    }
    let mut forms = vec![];
    let mut expense = vec![];
    let mut item = vec![];
    let count = p
        .map(|p| {
            get(p, "lines-TOTAL_FORMS")
                .and_then(super::super::d2::clean_integer)
                .unwrap_or(0)
                .clamp(0, 50) as usize
        })
        .unwrap_or(initials.len().min(50));
    for i in 0..count {
        let pref = format!("lines-{i}");
        let init = initials.get(i).unwrap_or(&Value::Null);
        let tab = p
            .map(|p| get(p, &format!("{pref}-tab")).unwrap_or(""))
            .unwrap_or(s(init, "tab"));
        let keep = p
            .and_then(|p| get(p, &format!("{pref}-account")))
            .or_else(|| init["account"].as_str());
        let row = bind_fields(line_fields(&ch, &pref, keep, tab), p, init, e, true);
        if tab == "ITEM" {
            item.push(row.clone());
        } else {
            expense.push(row.clone());
        }
        forms.push(row);
    }
    let bound = bind_fields(fs, p, &init, e, true);
    let selected = bound
        .f
        .get("vendor")
        .and_then(|f| f.value.as_deref())
        .unwrap_or("");
    put(
        &mut vm,
        "guidance",
        guidance(app, c, py::py_strip(selected))?,
    );
    put(&mut vm, "form", bound);
    put(&mut vm, "expense_lines", expense);
    put(&mut vm, "item_lines", item);
    put(
        &mut vm,
        "formset",
        FormsetVm {
            management_form: management(p, "lines", count, initial_count),
            non_form_errors: formset_errors(nf),
            total_error_count: super::super::d2::formset_error_count(e, p, "lines") + nf.len(),
            forms,
            empty_form: bind_fields(
                line_fields(&ch, "lines-__prefix__", None, ""),
                None,
                &Value::Null,
                &Errors::new(),
                true,
            ),
        },
    );
    put(&mut vm, "catalog", ch.catalog);
    put(&mut vm, "no_banks", bs.is_empty());
    put(&mut vm, "csrf_token", token(app, path));
    let offered = contacts(
        app,
        c,
        true,
        current.as_ref().map(|r| s(r, "vendor_id")),
        "confirm",
    )?;
    let vendors=app.open_mut()?.book.read(|tx|->Result<_,FbError>{let hints=directory::prefill_catalog(tx,c)?;let mut v=json!({});for row in directory::list(tx,c,Model::Vendor)?{let id=s(&row,"id");if offered.iter().any(|(k,_)|k==id){v[id]=json!({"name":row["display_name"],"address":row["billing_address"],"expense_account":hints[id]["expense_account"]});}}Ok(v)})?;
    put(&mut vm, "vendors", vendors);
    let mut numbers = json!({});
    for b in bs {
        let actor = app.open_mut()?.owner;
        numbers[b.id.to_db()] = json!(checks::next_number(
            &mut app.open_mut()?.book,
            actor,
            c,
            b.id
        )?);
    }
    put(&mut vm, "next_numbers", numbers);
    draw(app, "checks/form.html", &vm)
}
fn filter_fields(app: &mut App, c: CompanyId) -> AppResult<Vec<Field>> {
    Ok(vec![
        Field::new("q", "Search number / payee / memo", Widget::Text),
        select(
            "bank",
            "Bank account",
            "Choose…",
            bank_choices(app, c, None)?,
            false,
        ),
        select(
            "vendor",
            "Vendor payee",
            "Choose…",
            contacts(app, c, false, None, "short")?,
            false,
        ),
        Field::new("check_number", "Check number", Widget::Text),
        select(
            "state",
            "State",
            "All states",
            vec![
                ("DRAFT".into(), "Draft".into()),
                ("RECORDED".into(), "Recorded".into()),
                ("VOID".into(), "Voided".into()),
            ],
            false,
        ),
        select(
            "print_later",
            "Print later",
            "Numbered and to print",
            vec![
                ("1".into(), "To print only".into()),
                ("0".into(), "Numbered only".into()),
            ],
            false,
        ),
        Field::new("start", "Dated on or after", Widget::Date),
        Field::new("end", "Dated on or before", Widget::Date),
    ])
}
fn listing(app: &mut App, r: &Route, p: Posted<'_>) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let mut vm = ctx(app, r)?;
    let fs = filter_fields(app, c)?;
    let mut e = Errors::new();
    let q = clean_text(p, "q", 120, false, true, &mut e);
    let bank = choice(p, &fs[1], &mut e, true);
    let vendor = choice(p, &fs[2], &mut e, true);
    let number = clean_text(p, "check_number", 20, false, true, &mut e);
    let state = choice(p, &fs[4], &mut e, false);
    let print = choice(p, &fs[5], &mut e, false);
    let start = clean_date(p, "start", false, &mut e);
    let end = clean_date(p, "end", false, &mut e);
    let mut records = vec![];
    if e.is_empty() {
        for b in all(app, c)? {
            let mut v = rev_vm(b["current"].clone());
            let upper = fb_db::collation::upper(&q);
            if (!q.is_empty()
                && ![
                    s(&b, "number"),
                    s(&v, "check_number"),
                    s(&v, "payee_name"),
                    s(&v, "memo"),
                ]
                .iter()
                .any(|s| fb_db::collation::upper(s).contains(&upper)))
                || (!bank.is_empty() && v["bank_id"] != bank)
                || (!vendor.is_empty() && v["vendor_id"] != vendor)
                || (!number.is_empty() && v["check_number"] != number)
                || (!state.is_empty() && v["state"] != state)
                || (!print.is_empty() && (v["print_later"] == true) != (print == "1"))
                || start.as_deref().is_some_and(|d| s(&v, "date") < d)
                || end.as_deref().is_some_and(|d| s(&v, "date") > d)
            {
                continue;
            }
            v["document"] = json!({"identity":{"number":b["number"]}});
            records.push(v);
        }
        ordered(&mut records, "date", "id");
    }
    put(
        &mut vm,
        "filter_form",
        bind_fields(fs, Some(p), &Value::Null, &e, true),
    );
    put(&mut vm, "page", Page::of(&records, 25, get(p, "page")));
    put(&mut vm, "query_string", query_string(p));
    put(&mut vm, "vendor_labels", super::super::d6::labels(app, c)?);
    Ok((
        draw(app, "checks/list.html", &vm)?,
        if e.is_empty() { 200 } else { 400 },
    ))
}
fn detail(app: &mut App, r: &Route, p: Posted<'_>) -> AppResult<String> {
    let c = arg_company(r)?;
    let mut vm = context(app, r)?;
    let b = vm["check"].clone();
    let mut history = vec![];
    for n in (1..=b["revision"].as_i64().unwrap()).rev() {
        history.push(historical(app, c, s(&b, "id"), n)?);
    }
    put(
        &mut vm,
        "history_page",
        Page::of(&history, 20, get(p, "page")),
    );
    put(
        &mut vm,
        "references",
        references(app, c, s(&b, "identity_id"))?,
    );
    evidence(app, c, &mut vm)?;
    attachments(app, c, s(&b, "identity_id"), &mut vm)?;
    draw(app, "checks/detail.html", &vm)
}
fn action_fields(number: bool) -> Vec<Field> {
    if number {
        vec![Field::new("expected_revision","Expected revision",Widget::Hidden).required(),Field::new("check_number","Check number used",Widget::Text).required().help("This number shares this bank’s check numbers with Write checks, bill payments by check, and refund checks. A voided number can be used again.")]
    } else {
        let mut f = reversal_fields();
        f[1].label = "Void date".into();
        f
    }
}
fn action_form(
    app: &mut App,
    r: &Route,
    path: &str,
    p: Option<Posted<'_>>,
    e: &Errors,
) -> AppResult<String> {
    let c = arg_company(r)?;
    let mut vm = context(app, r)?;
    let number = r.name == "checks:number";
    let mut init =
        json!({"expected_revision":vm["check"]["revision"],"date":(app.today)().to_string()});
    if number {
        let actor = app.open_mut()?.owner;
        let bank = AccountId::parse(s(&vm["current"], "bank_id")).unwrap();
        init["check_number"] = json!(checks::next_number(
            &mut app.open_mut()?.book,
            actor,
            c,
            bank
        )?);
    }
    put(
        &mut vm,
        "form",
        bind_fields(action_fields(number), p, &init, e, true),
    );
    put(&mut vm, "csrf_token", token(app, path));
    draw(
        app,
        if number {
            "checks/number.html"
        } else {
            "checks/void.html"
        },
        &vm,
    )
}
fn review(
    app: &mut App,
    r: &Route,
    p: Posted<'_>,
    path: &str,
    post: bool,
    e: &Errors,
) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let mut vm = context(app, r)?;
    let mut e = e.clone();
    let mut initial = Value::Null;
    if post {
        put(&mut vm, "guidance", Value::Null);
    }
    if !post {
        let raw = get(p, "revision").unwrap_or("");
        if raw.is_empty() || !raw.chars().all(|c| fb_money::unicode_decimal(c).is_some()) {
            error(
                &mut e,
                "review",
                "Open the review from the current check revision.",
            );
        } else if let Some(n) = forms::python_integer(raw) {
            match app.run(Command::ReviewCheck {
                company_id: c.to_db(),
                check_id: r.args["check_id"].clone(),
                expected_revision: n,
            }) {
                Ok(Outcome::Done { data }) => {
                    put(&mut vm, "mapping", &data["mapping"]);
                    put(&mut vm, "entries", &data["entries"]);
                    initial = json!({"expected_revision":n,"review_token":data["token"]});
                }
                Err(AppError::Core(FbError::Validation(msg))) => {
                    e.insert("review".into(), msg);
                }
                Err(e) => return Err(e),
                _ => unreachable!(),
            }
        } else {
            error(&mut e, "review", "Enter a whole number.");
        }
    }
    if post || e.is_empty() {
        put(
            &mut vm,
            "form",
            bind_fields(
                vec![
                    Field::new("expected_revision", "Expected revision", Widget::Hidden).required(),
                    Field::new("review_token", "Review token", Widget::Hidden).required(),
                ],
                if post { Some(p) } else { None },
                &initial,
                &e,
                true,
            ),
        );
    }
    put(&mut vm, "errors", nonfield(&e, "review"));
    let postpath = if post {
        path.into()
    } else {
        url("checks:record", &[&c.to_db(), &r.args["check_id"]])
    };
    put(&mut vm, "csrf_token", token(app, &postpath));
    Ok((
        draw(app, "checks/review.html", &vm)?,
        if post || !e.is_empty() { 400 } else { 200 },
    ))
}
pub(super) fn render(
    app: &mut App,
    r: &Route,
    p: Posted<'_>,
    path: &str,
) -> AppResult<(String, u16)> {
    match r.name.as_str() {
        "checks:list" => listing(app, r, p),
        "checks:new" | "checks:edit" => Ok((form(app, r, path, None, &Errors::new(), &[])?, 200)),
        "checks:detail" => Ok((detail(app, r, p)?, 200)),
        "checks:number" | "checks:void" => {
            Ok((action_form(app, r, path, None, &Errors::new())?, 200))
        }
        "checks:review" => review(app, r, p, path, false, &Errors::new()),
        "checks:revision" => {
            let c = arg_company(r)?;
            let mut vm = context(app, r)?;
            let v = historical(
                app,
                c,
                &r.args["check_id"],
                r.args["number"]
                    .parse()
                    .map_err(|_| AppError::NotFound("revision".into()))?,
            )?;
            put(&mut vm, "lines", &v["lines"]);
            put(&mut vm, "tab_totals", &v["tab_totals"]);
            put(&mut vm, "current", v);
            Ok((draw(app, "checks/revision.html", &vm)?, 200))
        }
        _ => Err(AppError::NotFound(r.name.clone())),
    }
}
pub(super) fn submit(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(r)?;
    let mut e = Errors::new();
    if ["checks:void", "checks:number", "checks:record"].contains(&r.name.as_str()) {
        let _ = load(app, c, &r.args["check_id"])?;
        let rev = integer(p, "expected_revision", 1, None, true, &mut e).unwrap_or(0);
        let cmd = match r.name.as_str() {
            "checks:void" => {
                let date = clean_date(p, "date", true, &mut e).unwrap_or_default();
                let reason = clean_text(p, "reason", 500, true, true, &mut e);
                Command::VoidCheck {
                    company_id: c.to_db(),
                    check_id: r.args["check_id"].clone(),
                    expected_revision: rev,
                    date,
                    reason,
                }
            }
            "checks:number" => {
                let check_number = clean_text(p, "check_number", 20, true, true, &mut e);
                Command::AssignCheckNumber {
                    company_id: c.to_db(),
                    check_id: r.args["check_id"].clone(),
                    expected_revision: rev,
                    check_number,
                }
            }
            _ => {
                let review_token = clean_text(p, "review_token", 1000, true, true, &mut e);
                Command::RecordCheck {
                    company_id: c.to_db(),
                    check_id: r.args["check_id"].clone(),
                    expected_revision: rev,
                    review_token,
                }
            }
        };
        if e.is_empty() && result_error(app.run(cmd), &mut e)?.is_some() {
            return Ok(Outcome::Redirect {
                location: url("checks:detail", &[&c.to_db(), &r.args["check_id"]]),
            });
        }
        return Ok(Outcome::Invalid {
            html: if r.name == "checks:record" {
                review(app, r, p, path, true, &e)?.0
            } else {
                action_form(app, r, path, Some(p), &e)?
            },
        });
    }
    if !["checks:new", "checks:edit"].contains(&r.name.as_str()) {
        return Err(AppError::NotFound(path.into()));
    }
    let b = r
        .args
        .get("check_id")
        .map(|id| load(app, c, id))
        .transpose()?;
    let current = prior(app, c, b.as_ref(), Some(p))?;
    let fs = header_fields(app, c, current.as_ref())?;
    let key = key(p, b.is_none(), &mut e);
    let rev = integer(p, "expected_revision", 0, None, true, &mut e).unwrap_or(0);
    let bank = choice(p, &fs[2], &mut e, true);
    let date = clean_date(p, "date", true, &mut e).unwrap_or_default();
    let number = clean_text(p, "check_number", 20, false, true, &mut e);
    let print = checked(p, "print_later");
    let kind = get(p, "payee_kind").unwrap_or("");
    if kind.is_empty() {
        error(&mut e, "payee_kind", "This field is required.");
    } else if !["VENDOR", "OTHER"].contains(&kind) {
        error(
            &mut e,
            "payee_kind",
            format!("Select a valid choice. {kind} is not one of the available choices."),
        );
    }
    let vendor = choice(p, &fs[7], &mut e, true);
    let name = clean_text(p, "payee_name", 120, false, true, &mut e);
    let address = clean_text(p, "address", 1000, false, true, &mut e);
    let memo = clean_text(p, "memo", 2000, false, true, &mut e);
    super::super::d2::refuse_card_number(p, "memo", &mut e);
    // Form.clean() runs after all field cleans and uses only fields left in cleaned_data.
    if kind == "VENDOR" && (vendor.is_empty() || e.contains_key("vendor")) {
        error(
            &mut e,
            "vendor",
            "Choose a vendor, or switch to an other payee.",
        );
    }
    if kind == "OTHER" && (name.is_empty() || e.contains_key("payee_name")) {
        error(&mut e, "payee_name", "Enter the payee name.");
    }
    if print && !number.is_empty() && !e.contains_key("check_number") {
        error(
            &mut e,
            "check_number",
            "Clear the number when the check is marked print later.",
        );
    }
    if !print && (number.is_empty() || e.contains_key("check_number")) {
        error(
            &mut e,
            "check_number",
            "Enter a check number, or mark the check print later.",
        );
    }
    let ch = choices(app, c, current.as_ref())?;
    let count = get(p, "lines-TOTAL_FORMS")
        .and_then(super::super::d2::clean_integer)
        .unwrap_or(0)
        .clamp(0, 50) as usize;
    let initial = get(p, "lines-INITIAL_FORMS")
        .and_then(super::super::d2::clean_integer)
        .unwrap_or(0)
        .max(0) as usize;
    let stored = b
        .as_ref()
        .and_then(|b| b["current"]["lines"].as_array())
        .map_or(0, Vec::len);
    let mut meaningful = 0;
    let mut lines = vec![];
    for i in 0..count {
        let prefix = format!("lines-{i}");
        let row = p
            .iter()
            .filter_map(|(k, v)| {
                k.strip_prefix(&(prefix.clone() + "-"))
                    .map(|k| (k.into(), v.clone()))
            })
            .collect::<Vec<_>>();
        let initial_tab = if i == stored {
            "EXPENSE"
        } else if i == stored + 1 {
            "ITEM"
        } else {
            ""
        };
        let changed = i < stored
            || get(&row, "tab").unwrap_or("") != initial_tab
            || super::super::d2::extra_row_changed(
                &row,
                &[("description", ""), ("quantity", "1"), ("rate", "")],
                &["account", "item", "origin_position"],
                &["refresh_item", "keep_archived", "DELETE"],
            );
        let deleted = checked(&row, "DELETE");
        if !changed && i >= initial {
            continue;
        }
        meaningful += usize::from(!deleted);
        let mut le = Errors::new();
        let tab = get(&row, "tab").unwrap_or("");
        if tab.is_empty() {
            error(&mut le, "tab", "This field is required.");
        } else if !["EXPENSE", "ITEM"].contains(&tab) {
            error(
                &mut le,
                "tab",
                format!("Select a valid choice. {tab} is not one of the available choices."),
            );
        }
        let pos = integer(&row, "origin_position", 1, Some(50), false, &mut le);
        let raw_account = get(&row, "account").unwrap_or("");
        let account = py::py_strip(raw_account);
        if !raw_account.is_empty() && !ch.accounts.iter().any(|a| a.id.to_db() == account) {
            error(&mut le, "account", CHOICE);
        }
        let lf = line_fields(&ch, "", Some(account), tab);
        let item = choice(&row, &lf[3], &mut le, true);
        let description = clean_text(&row, "description", 1000, false, true, &mut le);
        let quantity = clean_text(&row, "quantity", 40, false, true, &mut le);
        let rate = clean_text(&row, "rate", 40, true, true, &mut le);
        for (k, v) in [("quantity", &quantity), ("rate", &rate)] {
            if !le.contains_key(k) && (!v.is_empty() || k == "rate") {
                let result = if k == "quantity" {
                    fb_core::calculations::quantity(&json!(v)).map(|_| ())
                } else {
                    fb_core::calculations::unit_rate(&json!(v)).map(|_| ())
                };
                if let Err(FbError::Validation(msg)) = result {
                    le.insert(k.into(), msg);
                }
            }
        }
        if tab == "ITEM" {
            if item.is_empty() || le.contains_key("item") {
                error(&mut le, "item", "Choose an item for an Items-tab line.");
            }
            if quantity.is_empty() && !le.contains_key("quantity") {
                error(&mut le, "quantity", "Enter a positive quantity.");
            }
        }
        for (k, m) in le {
            e.insert(format!("{prefix}-{k}"), m);
        }
        if deleted { continue; }
        lines.push(CheckLineValues {
            tab: tab.into(),
            origin_position: pos,
            account_id: (!account.is_empty()).then(|| account.into()),
            item_id: (tab == "ITEM" && !item.is_empty()).then_some(item),
            description,
            quantity: if tab == "EXPENSE" {
                "1".into()
            } else {
                quantity
            },
            rate,
            refresh_item: checked(&row, "refresh_item"),
            keep_archived: checked(&row, "keep_archived"),
        });
    }
    let nf = set_errors(p, "lines", meaningful, false);
    if super::super::d2::active_form_errors(&e, p) == 0 && nf.is_empty() {
        let values = CheckValues {
            check_id: r.args.get("check_id").cloned(),
            expected_revision: rev,
            request_key: key,
            bank_id: bank,
            date,
            check_number: number,
            print_later: print,
            payee_kind: kind.into(),
            vendor_id: (kind == "VENDOR" && !vendor.is_empty()).then_some(vendor),
            payee_name: if kind == "VENDOR" {
                String::new()
            } else {
                name
            },
            address,
            memo,
            refresh_payee: checked(p, "refresh_payee"),
            keep_archived: checked(p, "keep_archived"),
            lines,
        };
        if let Some(d) = result_error(
            app.run(Command::SaveCheck {
                company_id: c.to_db(),
                values,
            }),
            &mut e,
        )? {
            return Ok(Outcome::Redirect {
                location: url("checks:detail", &[&c.to_db(), s(&d, "id")]),
            });
        }
    }
    Ok(Outcome::Invalid {
        html: form(app, r, path, Some(p), &e, &nf)?,
    })
}
