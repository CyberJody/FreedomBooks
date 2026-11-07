use super::*;
use crate::commands::BillLineValues;
fn all(app: &mut App, c: CompanyId) -> AppResult<Vec<Value>> {
    let actor = app.open_mut()?.owner;
    Ok(payables::current_bills(
        &mut app.open_mut()?.book,
        actor,
        c,
    )?)
}
fn bill(app: &mut App, c: CompanyId, id: &str) -> AppResult<Value> {
    all(app, c)?
        .into_iter()
        .find(|b| b["id"] == id)
        .ok_or_else(|| AppError::NotFound("bill".into()))
}
fn state(v: &mut Value) {
    v["get_state_display"] = json!(match s(v, "state") {
        "DRAFT" => "Draft",
        "OPEN" => "Open",
        "VOID" => "Voided",
        s => s,
    });
}
fn revision_vm(app: &mut App, c: CompanyId, mut v: Value) -> AppResult<Value> {
    json_columns(&mut v);
    state(&mut v);
    if !s(&v, "supersedes_id").is_empty() {
        let b = bill(app, c, s(&v, "supersedes_id"))?;
        v["superseded_number"] = b["number"].clone();
    }
    v["created_at_display"] = json!(fb_templates::django::default_datetime(s(&v,"created_at")));
    Ok(v)
}
fn context(app: &mut App, r: &Route) -> AppResult<Value> {
    let c = arg_company(r)?;
    let b = bill(app, c, &r.args["bill_id"])?;
    let mut vm = ctx(app, r)?;
    let mut header = b.clone();
    header["pk"] = b["id"].clone();
    header["identity"] = json!({"number":b["number"]});
    let mut current = revision_vm(app, c, b["current"].clone())?;
    for k in ["outstanding", "payment_state", "paid_on"] {
        current[k] = b[k].clone();
    }
    put(&mut vm, "bill", header);
    put(&mut vm, "lines", &current["lines"]);
    put(&mut vm, "current", current);
    Ok(vm)
}
fn filter_fields(app: &mut App, c: CompanyId) -> AppResult<Vec<Field>> {
    Ok(vec![
        Field::new(
            "q",
            "Search number / vendor / reference / memo",
            Widget::Text,
        ),
        select(
            "state",
            "State",
            "All states",
            vec![
                ("OUTSTANDING".into(), "Unpaid balance".into()),
                ("OVERDUE".into(), "Overdue".into()),
                ("PAID_RECENT".into(), "Paid in last 30 days".into()),
                ("DRAFT".into(), "Draft".into()),
                ("OPEN".into(), "Open".into()),
                ("PARTIAL".into(), "Partial".into()),
                ("PAID".into(), "Paid".into()),
                ("VOID".into(), "Voided".into()),
            ],
            false,
        ),
        select(
            "vendor",
            "Vendor",
            "Choose…",
            contacts(app, c, false, None, "short")?,
            false,
        ),
        Field::new("start", "Due on or after", Widget::Date),
        Field::new("end", "Due on or before", Widget::Date),
    ])
}
fn listing(app: &mut App, r: &Route, p: Posted<'_>) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let mut vm = ctx(app, r)?;
    let fs = filter_fields(app, c)?;
    let mut e = Errors::new();
    let q = clean_text(p, "q", 120, false, true, &mut e);
    let state = choice(p, &fs[1], &mut e, false);
    let vendor = choice(p, &fs[2], &mut e, true);
    let start = clean_date(p, "start", false, &mut e);
    let end = clean_date(p, "end", false, &mut e);
    let today = (app.today)().to_string();
    let mut records = vec![];
    if e.is_empty() {
        let actor = app.open_mut()?.owner;
        let filters=json!({"q":q,"state":state,"vendor":vendor,"start":start.unwrap_or_default(),"end":end.unwrap_or_default()});
        let result = payables::bill_tracker(
            &mut app.open_mut()?.book,
            actor,
            c,
            &filters,
            &json!(today),
        )?;
        for b in result["rows"].as_array().unwrap() {
            let mut rev = b["current"].clone();
            rev["bill_id"] = b["id"].clone();
            rev["bill"] = json!({"identity":{"number":b["number"]}});
            for k in ["payment_state", "outstanding"] {
                rev[k] = b[k].clone();
            }
            records.push(rev);
        }
        ordered(&mut records, "date", "id");
        put(&mut vm, "tracker_totals", &result["totals"]);
        let mut links = json!({});
        for (k, state) in [
            ("open", "OUTSTANDING"),
            ("overdue", "OVERDUE"),
            ("paid_recent", "PAID_RECENT"),
            ("drafts", "DRAFT"),
        ] {
            let mut pairs = vec![];
            for key in ["q", "vendor", "start", "end"] {
                if let Some(v) = filters[key].as_str().filter(|s| !s.is_empty()) {
                    pairs.push((key.into(), v.into()));
                }
            }
            pairs.push(("state".into(), state.into()));
            links[k] = json!(urlencode(&pairs));
        }
        put(&mut vm, "tracker_links", links);
    }
    put(&mut vm, "vendor_labels", labels(app, c)?);
    put(&mut vm, "display_today", today);
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
        draw(app, "payables/bills/list.html", &vm)?,
        if e.is_empty() { 200 } else { 400 },
    ))
}
fn header_fields(app: &mut App, c: CompanyId, current: Option<&Value>) -> AppResult<Vec<Field>> {
    let supersedes = all(app, c)?
        .into_iter()
        .filter(|b| b["state"] == "VOID")
        .map(|b| (s(&b, "id").into(), s(&b, "number").into()))
        .collect();
    let mut key = Field::new("request_key", "Request key", Widget::Hidden);
    if current.is_none() {
        key = key.required();
    }
    Ok(vec![
        Field::new(
            "reference",
            "Vendor reference (separate from BILL number)",
            Widget::Text,
        ),
        select(
            "supersedes",
            "Voided bill this replaces",
            "None",
            supersedes,
            false,
        ),
        Field::new(
            "reentry_reason",
            "Reason",
            Widget::Textarea { cols: 40, rows: 2 },
        ),
        select(
            "terms",
            "Due-date convenience (select to fill)",
            "Explicit due date",
            vec![
                ("0".into(), "Due on receipt".into()),
                ("15".into(), "Net 15".into()),
                ("30".into(), "Net 30".into()),
                ("60".into(), "Net 60".into()),
            ],
            false,
        ),
        key,
        Field::new("expected_revision", "Expected revision", Widget::Hidden).required(),
        Field::new("reading", "Reading", Widget::Hidden),
        select(
            "vendor",
            "Vendor",
            "Choose…",
            contacts(app, c, true, current.map(|r| s(r, "vendor_id")), "confirm")?,
            true,
        ),
        Field::new("date", "Date", Widget::Date).required(),
        Field::new("due_date", "Due date", Widget::Date).required(),
        Field::new("memo", "Memo", Widget::Textarea { cols: 40, rows: 2 }),
        Field::new(
            "refresh_vendor",
            "Update vendor name and address from the vendor list",
            Widget::Checkbox { checked: false },
        ),
        Field::new(
            "keep_archived",
            "Keep archived vendor details on this bill",
            Widget::Checkbox { checked: false },
        ),
    ])
}
struct Choices {
    items: Vec<(String, String)>,
    accounts: Vec<Account>,
    eligible: Vec<String>,
    catalog: Value,
    vendors: Value,
}
fn choices(app: &mut App, c: CompanyId, current: Option<&Value>) -> AppResult<Choices> {
    Ok(app.open_mut()?.book.read(|tx|->Result<_,FbError>{let ord=super::super::d2::ordinals(tx,c,"directory_item")?;let mut items=vec![];let mut catalog=json!({});
        for item in directory::list(tx,c,Model::Item)? {let retained=current.and_then(|r|r["lines"].as_array()).is_some_and(|ls|ls.iter().any(|l|l["item_id"]==item["id"]));let id=s(&item,"id");if item["active"]==true||retained {items.push((id.into(),format!("{} · #{}{}",s(&item,"code"),ord[id],if item["active"]==true{""}else{" (archived — confirm retention or replace)"})));}
        if item["active"]==true{catalog[id]=json!({"description":item["description"],"rate":item["purchase_cost"].as_str().unwrap_or(""),"expense_account":item["expense_account"].as_str().unwrap_or("")});}}
        let accounts=fb_core::store::accounts(tx,c)?;let eligible=documents::eligible_line_accounts(tx,c)?.iter().map(|a|a.id.to_db()).collect();let vendors=directory::prefill_catalog(tx,c)?;Ok(Choices{items,accounts,eligible,catalog,vendors})})?)
}
fn line_fields(ch: &Choices, prefix: &str, keep: Option<&str>, required: bool) -> Vec<Field> {
    let mut a = ch.accounts.iter().collect::<Vec<_>>();
    a.sort_by(|a, b| {
        a.detail_type
            .order()
            .cmp(&b.detail_type.order())
            .then(a.code.cmp(&b.code))
            .then(a.id.to_db().cmp(&b.id.to_db()))
    });
    let mut groups = vec![(String::new(), vec![(String::new(), "Choose…".into())])];
    let mut unavailable = vec![];
    for a in a {
        let id = a.id.to_db();
        if ch.eligible.contains(&id) {
            let label = a.detail_type.label();
            if groups.last().is_none_or(|(g, _)| g != label) {
                groups.push((label.into(), vec![]));
            }
            groups.last_mut().unwrap().1.push((id, a.label()));
        } else if Some(id.as_str()) == keep {
            unavailable.push((id, a.label()));
        }
    }
    if !unavailable.is_empty() {
        groups.push(("Unavailable — replace before posting".into(), unavailable));
    }
    let mut f = vec![
        Field::new(
            "expense_account",
            "Account (required to post)",
            Widget::GroupedSelect { groups },
        ),
        Field::new("origin_position", "Origin position", Widget::Hidden),
        Field::new("ai_line", "Ai line", Widget::Hidden),
        select("item", "Item", "Choose…", ch.items.clone(), false),
        Field::new(
            "description",
            "Description",
            Widget::Textarea { cols: 40, rows: 2 },
        ),
        Field::new("quantity", "Quantity", Widget::Text),
        Field::new("rate", "Rate", Widget::Text),
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
        Field::new(
            "keep_as_entered",
            "Keep as entered (the receipt shows a different amount)",
            Widget::Checkbox { checked: false },
        ),
        Field::new("DELETE", "Delete", Widget::Checkbox { checked: false }),
    ];
    for field in &mut f {
        field.prefix = prefix.into();
        if required && ["description", "quantity", "rate"].contains(&field.name.as_str()) {
            field.required = true;
        }
    }
    f
}
fn prior_current(
    app: &mut App,
    c: CompanyId,
    b: Option<&Value>,
    p: Option<Posted<'_>>,
) -> AppResult<Option<Value>> {
    if let Some(b) = b {
        return Ok(Some(b["current"].clone()));
    }
    if let Some(key) = p.and_then(|p| get(p, "request_key")).and_then(py::py_uuid) {
        let prior=scoped_rows(app,c,"SELECT b.id FROM payables_bill b JOIN documents_documentidentity i ON i.id=b.identity_id WHERE b.company_id=? AND i.request_key=?",&[&c.to_db(),&key.0])?.pop();
        if let Some(b) = prior {
            return Ok(Some(historical(app, c, s(&b, "id"), 1)?));
        }
    }
    Ok(None)
}
fn form(
    app: &mut App,
    r: &Route,
    path: &str,
    p: Option<Posted<'_>>,
    e: &Errors,
    non_form: &[String],
) -> AppResult<String> {
    let c = arg_company(r)?;
    let b = r
        .args
        .get("bill_id")
        .map(|id| bill(app, c, id))
        .transpose()?;
    let current = prior_current(app, c, b.as_ref(), p)?;
    let mut fs = header_fields(app, c, current.as_ref())?;
    if let Some(f) = fs.iter_mut().find(|f| f.name == "request_key") {
        f.required = b.is_none();
    }
    let ch = choices(app, c, current.as_ref())?;
    let mut initial = json!({"request_key":uuid::Uuid::new_v4().to_string(),"expected_revision":0,"date":(app.today)().to_string()});
    let mut initials = vec![];
    if let Some(b) = &b {
        let r = &b["current"];
        for k in ["date", "due_date", "memo", "reference", "reentry_reason"] {
            initial[k] = r[k].clone();
        }
        initial["vendor"] = r["vendor_id"].clone();
        initial["supersedes"] = r["supersedes_id"].clone();
        initial["expected_revision"] = b["revision"].clone();
        for l in r["lines"].as_array().unwrap() {
            let mut row = l.clone();
            row["origin_position"] = l["position"].clone();
            row["item"] = l["item_id"].clone();
            row["expense_account"] = l["expense_account_id"].clone();
            initials.push(row);
        }
    }
    let mut vm = ctx(app, r)?;
    if let Some(b) = b {
        let mut header = b.clone();
        header["pk"] = b["id"].clone();
        header["identity"] = json!({"number":b["number"]});
        put(&mut vm, "bill", header);
        put(
            &mut vm,
            "current",
            revision_vm(app, c, b["current"].clone())?,
        );
    }
    put(&mut vm, "form", bind_fields(fs, p, &initial, e, true));
    put(&mut vm, "csrf_token", token(app, path));
    let total = formset_count(p, "lines", initials.len());
    let forms = (0..total)
        .map(|i| {
            let prefix = format!("lines-{i}");
            let init = initials.get(i).unwrap_or(&Value::Null);
            let keep = p
                .and_then(|p| get(p, &format!("{prefix}-expense_account")))
                .or_else(|| init["expense_account"].as_str());
            bind_fields(line_fields(&ch, &prefix, keep, false), p, init, e, true)
        })
        .collect::<Vec<_>>();
    put(
        &mut vm,
        "formset",
        FormsetVm {
            management_form: management(p, "lines", total, initials.len()),
            non_form_errors: formset_errors(non_form),
            total_error_count: super::super::d2::formset_error_count(e, p, "lines") + non_form.len(),
            forms,
            empty_form: bind_fields(
                line_fields(&ch, "lines-__prefix__", None, false),
                None,
                &Value::Null,
                &Errors::new(),
                true,
            ),
        },
    );
    put(&mut vm, "catalog", ch.catalog);
    let offered = contacts(
        app,
        c,
        true,
        current.as_ref().map(|r| s(r, "vendor_id")),
        "confirm",
    )?;
    let mut defaults = ch.vendors;
    defaults
        .as_object_mut()
        .unwrap()
        .retain(|k, _| offered.iter().any(|(id, _)| k == id));
    put(&mut vm, "vendor_defaults", defaults);
    // D9: "Start a bill from a receipt" (receipts.panel) on a new bill only; its notes; the upload key.
    if app.plugins && !r.args.contains_key("bill_id") {
        put(&mut vm, "ai_panel", super::super::d9::assistant::receipt_panel(app, c)?);
        let upload = fb_templates::django::reverse("assistant:read_upload", &[c.to_db()]).map_err(AppError::Render)?;
        put(&mut vm, "ai_upload_token", token(app, &upload));
    }
    put(&mut vm, "ai_notes", Vec::<Value>::new());
    put(&mut vm, "ai_request_key", uuid::Uuid::new_v4().to_string());
    draw(app, "payables/bills/form.html", &vm)
}
fn historical(app: &mut App, c: CompanyId, id: &str, number: i64) -> AppResult<Value> {
    let _ = bill(app, c, id)?;
    let mut rev = scoped_rows(
        app,
        c,
        "SELECT * FROM payables_billrevision WHERE bill_id=? AND number=?",
        &[&id, &number],
    )?
    .pop()
    .ok_or_else(|| AppError::NotFound("revision".into()))?;
    let mut lines = scoped_rows(
        app,
        c,
        "SELECT * FROM payables_billline WHERE revision_id=? ORDER BY position",
        &[&s(&rev, "id")],
    )?;
    for l in &mut lines {
        json_columns(l);
        for k in ["amount", "rate"] {
            l[k] = json!(cash(l[k].as_i64().unwrap() as i128));
        }
        l["quantity"] = json!(format!(
            "{}.{:04}",
            l["quantity"].as_i64().unwrap() / 10000,
            l["quantity"].as_i64().unwrap() % 10000
        ));
    }
    rev["total"] = json!(cash(rev["total"].as_i64().unwrap() as i128));
    rev["lines"] = json!(lines);
    revision_vm(app, c, rev)
}
fn detail(app: &mut App, r: &Route, p: Posted<'_>) -> AppResult<String> {
    let c = arg_company(r)?;
    let mut vm = context(app, r)?;
    let b = vm["bill"].clone();
    let refs = references(app, c, s(&b, "identity_id"))?;
    put(&mut vm, "references", refs);
    let mut allocations = scoped_rows(
        app,
        c,
        "SELECT a.*,p.date,p.identity_id,EXISTS(SELECT 1 FROM payables_paymentreversal r WHERE r.payment_id=p.id) AS reversed,i.number FROM payables_paymentallocation a JOIN payables_billpayment p ON p.id=a.payment_id JOIN documents_documentidentity i ON i.id=p.identity_id WHERE a.bill_id=? ORDER BY p.date DESC,a.id DESC",
        &[&s(&b, "id")],
    )?;
    for a in &mut allocations {
        a["payment"] =
            json!({"date":a["date"],"reversed":a["reversed"]==1,"identity":{"number":a["number"]}});
        a["amount"] = json!(cash(a["amount"].as_i64().unwrap() as i128));
    }
    put(
        &mut vm,
        "allocations_page",
        Page::of(&allocations, 25, get(p, "allocations_page")),
    );
    let mut history = vec![];
    for n in (1..=b["revision"].as_i64().unwrap()).rev() {
        history.push(historical(app, c, s(&b, "id"), n)?);
    }
    put(
        &mut vm,
        "history_page",
        Page::of(&history, 20, get(p, "page")),
    );
    let draft = vm["current"]["state"] == "DRAFT";
    if app.plugins {
        // D9: the real Read with AI section (receipts.bill_actions).
        put(&mut vm, "ai", super::super::d9::assistant::bill_actions(app, c, s(&b, "identity_id"), draft)?);
    }
    evidence(app, c, &mut vm)?;
    attachments(app, c, s(&b, "identity_id"), &mut vm)?;
    draw(app, "payables/bills/detail.html", &vm)
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
    let mut fs = vec![
        Field::new("expected_revision", "Expected revision", Widget::Hidden).required(),
        Field::new("review_token", "Review token", Widget::Hidden).required(),
    ];
    let mut initial = Value::Null;
    if !post {
        let rev = get(p, "revision").unwrap_or("");
        if rev.is_empty() || !rev.chars().all(|c| fb_money::unicode_decimal(c).is_some()) {
            error(
                &mut e,
                "review",
                "Open review from the current bill revision.",
            );
        } else if let Some(n) = clean_integer(rev) {
            match app.run(Command::ReviewBill {
                company_id: c.to_db(),
                bill_id: r.args["bill_id"].clone(),
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
        }
    }
    // PostForm's CodepointTextField has no HTML maxlength.
    for f in &mut fs {
        f.value = data_value(
            if post { Some(p) } else { None },
            &f.name,
            initial[&f.name]
                .as_str()
                .map(str::to_owned)
                .or_else(|| initial[&f.name].as_i64().map(|n| n.to_string())),
        );
    }
    put(
        &mut vm,
        "form",
        forms::bind(&decorate(fs, &e), &nonfield(&e, "__all__")),
    );
    put(&mut vm, "errors", nonfield(&e, "review"));
    let postpath = if post {
        path.into()
    } else {
        url("bills:post", &[&c.to_db(), &r.args["bill_id"]])
    };
    put(&mut vm, "csrf_token", token(app, &postpath));
    Ok((
        draw(app, "payables/bills/review.html", &vm)?,
        if post || !e.is_empty() { 400 } else { 200 },
    ))
}
fn void_form(
    app: &mut App,
    r: &Route,
    path: &str,
    p: Option<Posted<'_>>,
    e: &Errors,
) -> AppResult<String> {
    let mut vm = context(app, r)?;
    let initial =
        json!({"expected_revision":vm["bill"]["revision"],"date":(app.today)().to_string()});
    put(
        &mut vm,
        "form",
        bind_fields(reversal_fields(), p, &initial, e, true),
    );
    put(&mut vm, "csrf_token", token(app, path));
    draw(app, "payables/bills/void.html", &vm)
}
pub(super) fn render(
    app: &mut App,
    r: &Route,
    p: Posted<'_>,
    path: &str,
) -> AppResult<(String, u16)> {
    match r.name.as_str() {
        "bills:list" => listing(app, r, p),
        "bills:new" | "bills:edit" => Ok((form(app, r, path, None, &Errors::new(), &[])?, 200)),
        "bills:detail" => Ok((detail(app, r, p)?, 200)),
        "bills:review" => review(app, r, p, path, false, &Errors::new()),
        "bills:revision" => {
            let c = arg_company(r)?;
            let mut vm = context(app, r)?;
            let rev = historical(
                app,
                c,
                &r.args["bill_id"],
                r.args["number"]
                    .parse()
                    .map_err(|_| AppError::NotFound("revision".into()))?,
            )?;
            put(&mut vm, "lines", &rev["lines"]);
            put(&mut vm, "current", rev);
            Ok((draw(app, "payables/bills/revision.html", &vm)?, 200))
        }
        "bills:void" => Ok((void_form(app, r, path, None, &Errors::new())?, 200)),
        "bills:calendar" => calendar(app, r, p),
        _ => Err(AppError::NotFound(r.name.clone())),
    }
}
pub(super) fn submit(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(r)?;
    let mut e = Errors::new();
    if r.name == "bills:void" {
        let _ = bill(app, c, &r.args["bill_id"])?;
        let (rev, date, reason) = reverse_values(p, &mut e);
        if e.is_empty()
            && result_error(
                app.run(Command::VoidBill {
                    company_id: c.to_db(),
                    bill_id: r.args["bill_id"].clone(),
                    expected_revision: rev,
                    date,
                    reason,
                }),
                &mut e,
            )?
            .is_some()
        {
            return Ok(Outcome::Redirect {
                location: url("bills:detail", &[&c.to_db(), &r.args["bill_id"]]),
            });
        }
        return Ok(Outcome::Invalid {
            html: void_form(app, r, path, Some(p), &e)?,
        });
    }
    if r.name == "bills:post" {
        let _ = bill(app, c, &r.args["bill_id"])?;
        let rev = integer(p, "expected_revision", 1, None, true, &mut e).unwrap_or(0);
        let rt = clean_text(p, "review_token", 1000, true, true, &mut e);
        if e.is_empty()
            && result_error(
                app.run(Command::PostBill {
                    company_id: c.to_db(),
                    bill_id: r.args["bill_id"].clone(),
                    expected_revision: rev,
                    review_token: rt,
                }),
                &mut e,
            )?
            .is_some()
        {
            return Ok(Outcome::Redirect {
                location: url("bills:detail", &[&c.to_db(), &r.args["bill_id"]]),
            });
        }
        return Ok(Outcome::Invalid {
            html: review(app, r, p, path, true, &e)?.0,
        });
    }
    if !["bills:new", "bills:edit"].contains(&r.name.as_str()) {
        return Err(AppError::NotFound(path.into()));
    }
    let b = r
        .args
        .get("bill_id")
        .map(|id| bill(app, c, id))
        .transpose()?;
    let current = prior_current(app, c, b.as_ref(), Some(p))?;
    let fs = header_fields(app, c, current.as_ref())?;
    let reference = clean_text(p, "reference", 120, false, false, &mut e);
    let supersedes = choice(p, &fs[1], &mut e, true);
    let reentry = clean_text(p, "reentry_reason", 500, false, true, &mut e);
    choice(p, &fs[3], &mut e, false);
    let key = key(p, b.is_none(), &mut e);
    let rev = integer(p, "expected_revision", 0, None, true, &mut e).unwrap_or(0);
    integer(p, "reading", 1, None, false, &mut e);
    let vendor = choice(p, &fs[7], &mut e, true);
    let date = clean_date(p, "date", true, &mut e).unwrap_or_default();
    let due = clean_date(p, "due_date", true, &mut e).unwrap_or_default();
    let memo = clean_text(p, "memo", 2000, false, true, &mut e);
    let ch = choices(app, c, current.as_ref())?;
    let mut lines = vec![];
    let count = formset_count(Some(p), "lines", 0);
    let mut any_line_error = false;
    let mut meaningful=0;
    let initial = get(p, "lines-INITIAL_FORMS")
        .and_then(clean_integer)
        .unwrap_or(0);
    for i in 0..count {
        let prefix = format!("lines-{i}");
        let row = p
            .iter()
            .filter_map(|(k, v)| {
                k.strip_prefix(&(prefix.clone() + "-"))
                    .map(|k| (k.to_owned(), v.clone()))
            })
            .collect::<Vec<_>>();
        let stored_initial = b.as_ref().and_then(|b| b["current"]["lines"].as_array()).map_or(0, Vec::len);
        let changed = i < stored_initial || super::super::d2::extra_row_changed(
            &row, &[("description", ""), ("quantity", ""), ("rate", "")],
            &["expense_account", "origin_position", "ai_line", "item"],
            &["refresh_item", "keep_archived", "keep_as_entered", "DELETE"],
        );
        let deleted = checked(&row, "DELETE");
        if !changed && i >= initial as usize && i >= 1 {
            continue;
        }
        if !deleted && (changed || i < initial.max(0) as usize) {meaningful+=1;}
        let mut le = Errors::new();
        let lf = line_fields(&ch, "", get(&row, "expense_account"), true);
        let account = get(&row, "expense_account").unwrap_or("");
        if !account.is_empty() && !ch.accounts.iter().any(|a| a.id.to_db() == account) {
            error(&mut le, "expense_account", CHOICE);
        }
        let pos = integer(&row, "origin_position", 1, Some(50), false, &mut le);
        let item = choice(&row, &lf[3], &mut le, true);
        let description = clean_text(&row, "description", 1000, true, true, &mut le);
        let quantity = clean_text(&row, "quantity", 40, true, true, &mut le);
        let rate = clean_text(&row, "rate", 40, true, true, &mut le);
        for (k, value) in [("quantity", &quantity), ("rate", &rate)] {
            if !le.contains_key(k) {
                let result = if k == "quantity" {
                    fb_core::calculations::quantity(&json!(value)).map(|_| ())
                } else {
                    fb_core::calculations::unit_rate(&json!(value)).map(|_| ())
                };
                if let Err(FbError::Validation(msg)) = result {
                    le.insert(k.into(), msg);
                }
            }
        }
        any_line_error |= !deleted && !le.is_empty();
        for (k, msg) in le {
            e.insert(format!("{prefix}-{k}"), msg);
        }
        if deleted { continue; }
        lines.push(BillLineValues {
            origin_position: pos,
            item_id: (!item.is_empty()).then_some(item),
            expense_account_id: (!account.is_empty()).then(|| account.into()),
            description,
            quantity,
            rate,
            refresh_item: checked(&row, "refresh_item"),
            keep_archived: checked(&row, "keep_archived"),
        });
    }
    let non_form = set_errors(p, "lines", meaningful, any_line_error);
    if super::super::d2::active_form_errors(&e, p) == 0 && non_form.is_empty() {
        let values = BillValues {
            bill_id: r.args.get("bill_id").cloned(),
            expected_revision: rev,
            request_key: key,
            vendor_id: vendor,
            date,
            due_date: due,
            memo,
            reference,
            supersedes_id: (!supersedes.is_empty()).then_some(supersedes),
            reentry_reason: reentry,
            refresh_vendor: checked(p, "refresh_vendor"),
            keep_archived: checked(p, "keep_archived"),
            lines,
        };
        if let Some(d) = result_error(
            app.run(Command::SaveBill {
                company_id: c.to_db(),
                values,
            }),
            &mut e,
        )? {
            return Ok(Outcome::Redirect {
                location: url("bills:detail", &[&c.to_db(), s(&d, "id")]),
            });
        }
    }
    Ok(Outcome::Invalid {
        html: form(app, r, path, Some(p), &e, &non_form)?,
    })
}
fn calendar(app: &mut App, r: &Route, p: Posted<'_>) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let mut vm = ctx(app, r)?;
    let mut e = Errors::new();
    let year = integer(p, "year", 1900, Some(2200), false, &mut e);
    let month = integer(p, "month", 1, Some(12), false, &mut e);
    let vendorfield = select(
        "vendor",
        "Vendor",
        "Choose…",
        contacts(app, c, false, None, "short")?,
        false,
    );
    let vendor = choice(p, &vendorfield, &mut e, true);
    let paid = checked(p, "paid");
    if year.is_some() != month.is_some() {
        error(
            &mut e,
            "__all__",
            "Give both year and month, or neither for the current month.",
        );
    }
    let fs = vec![
        Field::new("year", "Year", Widget::Number)
            .attr("min", "1900")
            .attr("max", "2200"),
        Field::new("month", "Month", Widget::Number)
            .attr("min", "1")
            .attr("max", "12"),
        vendorfield,
        Field::new(
            "paid",
            "Show paid bills",
            Widget::Checkbox { checked: paid },
        ),
    ];
    put(
        &mut vm,
        "form",
        bind_fields(fs, Some(p), &Value::Null, &e, true),
    );
    let today = (app.today)();
    put(&mut vm, "today", today.to_string());
    if !e.is_empty() {
        put(&mut vm, "invalid", true);
        return Ok((draw(app, "payables/bills/calendar.html", &vm)?, 400));
    }
    let (year, month) = (
        year.unwrap_or(today.year() as i64) as i32,
        month.unwrap_or(today.month() as i64) as u32,
    );
    let actor = app.open_mut()?.owner;
    let mut built = payables::bill_calendar(
        &mut app.open_mut()?.book,
        actor,
        c,
        year,
        month,
        &json!(today.to_string()),
        paid,
        (!vendor.is_empty()).then_some(vendor.as_str()),
    )?;
    fn entry(e: &mut Value) {
        let (word, mark) = match s(e, "status") {
            "paid" => ("Paid", "✓"),
            "overdue" => ("Overdue", "!"),
            "today" => ("Due today", "•"),
            _ => ("Upcoming", "›"),
        };
        e["status_word"] = json!(word);
        e["status_mark"] = json!(mark);
        e["payable"] = json!(["OPEN", "PARTIAL"].contains(&s(e, "payment_state")));
        e["label"] = json!(format!(
            "{word}: {}, {}, due {}, {}",
            s(e, "number"),
            s(e, "vendor_name"),
            s(e, "due_date"),
            if s(e, "status") == "paid" {
                "paid in full".into()
            } else {
                format!("{} remaining", s(e, "remaining"))
            }
        ));
    }
    fn day(v: &mut Value) {
        if v.is_null() {
            return;
        }
        v["day_number"] = json!(s(v, "date")[8..].parse::<u32>().unwrap());
        v["anchor"] = json!(format!("day-{}", s(v, "date")));
        for e in v["entries"].as_array_mut().unwrap() {
            entry(e);
        }
        let entries = v["entries"].as_array().unwrap();
        v["hidden_count"] = json!(entries.len().saturating_sub(3));
        v["grid_entries"] = json!(
            v["entries"]
                .as_array()
                .unwrap()
                .iter()
                .take(3)
                .collect::<Vec<_>>()
        );
    }
    for week in built["weeks"].as_array_mut().unwrap() {
        for d in week.as_array_mut().unwrap() {
            day(d);
        }
    }
    for d in built["agenda"].as_array_mut().unwrap() {
        day(d);
    }
    for (k, v) in built.as_object().unwrap() {
        put(&mut vm, k, v);
    }
    let keep = p
        .iter()
        .filter(|(k, _)| k != "year" && k != "month")
        .cloned()
        .collect::<Vec<_>>();
    let extra = if keep.is_empty() {
        String::new()
    } else {
        format!("&{}", urlencode(&keep))
    };
    let previous = if month == 1 {
        (year - 1, 12)
    } else {
        (year, month - 1)
    };
    let next = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    put(
        &mut vm,
        "previous_query",
        (previous.0 >= 1900).then(|| format!("year={}&month={}{}", previous.0, previous.1, extra)),
    );
    put(
        &mut vm,
        "next_query",
        (next.0 <= 2200).then(|| format!("year={}&month={}{}", next.0, next.1, extra)),
    );
    put(&mut vm, "today_query", urlencode(&keep));
    put(&mut vm, "year", year);
    put(&mut vm, "month", month);
    put(&mut vm, "show_paid", paid);
    put(
        &mut vm,
        "is_current",
        (year, month) == (today.year(), today.month()),
    );
    put(&mut vm, "display_time_zone", std::env::var("FB_TIME_ZONE").unwrap_or_else(|_| "America/New_York".into()));
    put(&mut vm, "grid_cap", 3);
    put(&mut vm, "vendor_labels", labels(app, c)?);
    if !vendor.is_empty() {
        let v = app
            .open_mut()?
            .book
            .read(|tx| directory::get(tx, c, Model::Vendor, &vendor))?;
        put(&mut vm, "vendor", v);
    }
    Ok((draw(app, "payables/bills/calendar.html", &vm)?, 200))
}
