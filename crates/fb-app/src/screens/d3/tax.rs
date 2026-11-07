use super::*;
fn at(app: &mut App, route: &Route) -> AppResult<Option<Value>> {
    let Some(id) = route.args.get("item_id") else {
        return Ok(None);
    };
    let c = arg_company(route)?;
    let v = app.open_mut()?.book.read(|tx| salestax::item(tx, c, id))?;
    if (route.name.contains("rate_") && v["kind"] != "RATE")
        || (route.name.contains("group_") && v["kind"] != "GROUP")
    {
        return Err(AppError::NotFound("tax item".into()));
    }
    Ok(Some(v))
}
fn item_vm(tx: &(impl Query + ?Sized), c: CompanyId, mut item: Value) -> Result<Value, FbError> {
    item["pk"] = item["id"].clone();
    item["get_kind_display"] = json!(if item["kind"] == "RATE" {
        "Tax rate"
    } else {
        "Tax group"
    });
    item["agency"] = item["agency_id"]
        .as_str()
        .map(|id| directory::get(tx, c, Model::Vendor, id))
        .transpose()?
        .unwrap_or(Value::Null);
    Ok(item)
}
fn context(app: &mut App, route: &Route) -> AppResult<Value> {
    let mut vm = ctx(app, route)?;
    put(
        &mut vm,
        "tab",
        if route.name == "salestax:settings" || route.name == "salestax:bulk_step" {
            "settings"
        } else {
            "rates"
        },
    );
    put(&mut vm, "messages", std::mem::take(&mut app.messages));
    Ok(vm)
}
fn settings_fields(app: &mut App, c: CompanyId, s: &Value) -> AppResult<Vec<Field>> {
    let opts = app.open_mut()?.book.read(|tx| -> Result<Vec<_>, FbError> {
        let mut out = vec![(
            String::new(),
            "None — each form starts with \"Choose sales tax\"".into(),
        )];
        for r in salestax::list(tx, c)? {
            if r["active"] == true || r["id"] == s["default_item_id"] {
                out.push((
                    r["id"].as_str().unwrap().into(),
                    salestax::item_label(tx, c, &r)?,
                ));
            }
        }
        Ok(out)
    })?;
    Ok(vec![Field::new("expected_revision","Expected revision",Widget::Hidden).required().value(s["revision"].to_string()),
    Field::new("charge_tax","Do you charge sales tax?",Widget::Radio{choices:choices(&[("yes","Yes"),("no","No")])}).required().value(if s["charge_tax"]==true{"yes"}else{"no"}),
    Field::new("default_item","Default tax for new sales forms",Widget::Select{choices:opts}).value(s["default_item_id"].as_str().unwrap_or("")).help("Pre-selected on new estimates, invoices, sales receipts and credit memos. Each form can still change it."),
    Field::new("owed_basis","Sales tax is owed",Widget::Radio{choices:choices(&[("ACCRUAL","As of the invoice date (accrual basis)"),("CASH","When the customer pays (cash basis)")])}).required().value(s["owed_basis"].as_str().unwrap()),
    Field::new("filing_frequency","You file sales tax",Widget::Radio{choices:choices(&[("MONTHLY","Monthly"),("QUARTERLY","Quarterly"),("ANNUALLY","Annually")])}).required().value(s["filing_frequency"].as_str().unwrap())])
}
fn settings(
    app: &mut App,
    route: &Route,
    path: &str,
    posted: Option<Posted<'_>>,
    errors: &Errors,
) -> AppResult<String> {
    let c = arg_company(route)?;
    let mut vm = context(app, route)?;
    let settings = app
        .open_mut()?
        .book
        .read(|tx| salestax::settings_for(tx, c))?;
    let mut fs = settings_fields(app, c, &settings)?;
    for f in &mut fs {
        f.value = data_value(posted, &f.name, f.value.clone());
    }
    let items = app.open_mut()?.book.read(|tx| salestax::list(tx, c))?;
    put(&mut vm, "settings", settings);
    put(
        &mut vm,
        "counts",
        json!({"rates":items.iter().filter(|v|v["active"]==true&&v["kind"]=="RATE").count(),"groups":items.iter().filter(|v|v["active"]==true&&v["kind"]=="GROUP").count()}),
    );
    put(
        &mut vm,
        "form",
        forms::bind(&decorate(fs, errors), &nonfield(errors, "__all__")),
    );
    put(&mut vm, "csrf_token", token(app, path));
    draw(app, "salestax/settings.html", &vm)
}
fn rate_members(app: &mut App, c: CompanyId, item: Option<&Value>) -> AppResult<Vec<Value>> {
    let ids = item
        .and_then(|r| r["member_ids"].as_array())
        .cloned()
        .unwrap_or_default();
    Ok(app.open_mut()?.book.read(|tx| -> Result<Vec<_>, FbError> {
        salestax::list(tx, c)?
            .into_iter()
            .filter(|r| r["kind"] == "RATE" && (r["active"] == true || ids.contains(&r["id"])))
            .map(|r| item_vm(tx, c, r))
            .collect()
    })?)
}
fn item_fields(
    app: &mut App,
    c: CompanyId,
    item: Option<&Value>,
    group: bool,
) -> AppResult<Vec<Field>> {
    let mut fs = vec![
        Field::new("expected_revision", "Expected revision", Widget::Hidden),
        Field::new("name", "Name", Widget::Text)
            .required()
            .attr("maxlength", "60"),
        Field::new(
            "description",
            "Description (printed on forms)",
            Widget::Text,
        )
        .required()
        .attr("maxlength", "120")
        .value("Sales tax"),
    ];
    if group {
        fs[2]=fs[2].clone().help("Customers see one tax line with this description; the books keep each rate and agency separately.");
        let rates = rate_members(app, c, item)?;
        let labels = app
            .open_mut()?
            .book
            .read(|tx| labels(tx, c, Model::Vendor))?;
        let opts = rates
            .iter()
            .map(|r| {
                let raw = r["agency_id"].as_str().unwrap_or("");
                let label = labels[raw]
                    .as_str()
                    .or(r["agency"]["display_name"].as_str())
                    .unwrap_or("");
                (
                    r["id"].as_str().unwrap().into(),
                    format!(
                        "{} · {}% · {}{}",
                        r["name"].as_str().unwrap(),
                        pct(&r["rate"]),
                        label,
                        if r["active"] == true {
                            ""
                        } else {
                            " (inactive)"
                        }
                    ),
                )
            })
            .collect();
        fs.push(
            Field::new(
                "members",
                "Tax rates in this group",
                Widget::Checkboxes {
                    choices: opts,
                    selected: vec![],
                },
            )
            .required()
            .help("Choose 2 to 20 rates. The group rate is their sum."),
        );
    } else {
        fs[1] = fs[1]
            .clone()
            .help("Shown in the sales tax picker, e.g. \"Kettle County\".");
        fs.push(Field::new("rate","Rate (%)",Widget::Text).required().attr("inputmode","decimal").attr("maxlength","20").help("0 to 100, up to 4 decimal places. Saved forms keep the rate they were saved with."));
        let opts = app.open_mut()?.book.read(|tx| -> Result<Vec<_>, FbError> {
            let map = labels(tx, c, Model::Vendor)?;
            let mut opts = vec![(String::new(), "Choose a vendor".into())];
            for v in directory::list(tx, c, Model::Vendor)? {
                if v["active"] == true || item.is_some_and(|i| i["agency_id"] == v["id"]) {
                    let id = v["id"].as_str().unwrap();
                    opts.push((
                        id.into(),
                        format!(
                            "{}{}",
                            map[id].as_str().unwrap(),
                            if v["active"] == true {
                                ""
                            } else {
                                " (inactive)"
                            }
                        ),
                    ));
                }
            }
            Ok(opts)
        })?;
        fs.push(Field::new(
            "agency",
            "Tax agency (vendor you pay)",
            Widget::Select { choices: opts },
        ));
        fs.push(
            Field::new(
                "new_agency_name",
                "…or add a new agency by name",
                Widget::Text,
            )
            .attr("maxlength", "120")
            .help("Creates a vendor with just this name."),
        );
    }
    Ok(fs)
}
fn pct(v: &Value) -> String {
    let s = v.as_str().unwrap_or("0");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').into()
    } else {
        s.into()
    }
}
fn item_form(
    app: &mut App,
    route: &Route,
    path: &str,
    posted: Option<Posted<'_>>,
    errors: &Errors,
) -> AppResult<String> {
    let c = arg_company(route)?;
    let item = at(app, route)?;
    let group = route.name.contains("group_");
    let mut vm = context(app, route)?;
    let mut fs = item_fields(app, c, item.as_ref(), group)?;
    for f in &mut fs {
        let initial = item
            .as_ref()
            .and_then(|r| match f.name.as_str() {
                "expected_revision" => Some(r["revision"].to_string()),
                "agency" => string(&r["agency_id"]),
                "rate" => Some(pct(&r["rate"])),
                n => string(&r[n]),
            })
            .or(f.value.clone());
        f.value = data_value(posted, &f.name, initial);
        if let Widget::Checkboxes { selected, .. } = &mut f.widget {
            *selected = posted
                .map(|p| {
                    p.iter()
                        .filter(|(k, _)| k == "members")
                        .map(|(_, v)| v.clone())
                        .collect()
                })
                .unwrap_or_else(|| {
                    item.as_ref()
                        .and_then(|r| r["member_ids"].as_array())
                        .map(|a| a.iter().filter_map(string).collect())
                        .unwrap_or_default()
                });
        }
    }
    let all = app.open_mut()?.book.read(|tx| salestax::list(tx, c))?;
    let groups = all
        .into_iter()
        .filter(|r| {
            item.as_ref().is_some_and(|i| {
                r["member_ids"]
                    .as_array()
                    .is_some_and(|ids| ids.contains(&i["id"]))
            })
        })
        .collect::<Vec<_>>();
    let rates = rate_members(app, c, item.as_ref())?;
    let rate_map: serde_json::Map<String, Value> = rates
        .iter()
        .map(|r| (r["id"].as_str().unwrap().into(), json!(pct(&r["rate"]))))
        .collect();
    let group_rate = if let Some(item) = &item {
        if group {
            let dec = app
                .open_mut()?
                .book
                .read(|tx| salestax::group_rate(tx, c, item))?;
            salestax::percent(dec)
        } else {
            "0".into()
        }
    } else {
        "0".into()
    };
    put(&mut vm, "kind", if group { "group" } else { "rate" });
    put(&mut vm, "item", item);
    put(&mut vm, "groups", groups);
    put(&mut vm, "rates", rate_map);
    put(&mut vm, "group_rate", group_rate);
    put(
        &mut vm,
        "form",
        forms::bind(&decorate(fs, errors), &nonfield(errors, "__all__")),
    );
    put(&mut vm, "csrf_token", token(app, path));
    draw(app, "salestax/item_form.html", &vm)
}
fn list(app: &mut App, route: &Route, p: Posted<'_>) -> AppResult<(String, u16)> {
    let c = arg_company(route)?;
    let mut vm = context(app, route)?;
    let mut errors = Errors::new();
    let status = get(p, "status").unwrap_or("active");
    let opts = choices(&[
        ("active", "Active only"),
        ("all", "All (including inactive)"),
        ("inactive", "Inactive only"),
    ]);
    if !status.is_empty() && !opts.iter().any(|(v, _)| v == status) {
        error(
            &mut errors,
            "status",
            format!("Select a valid choice. {status} is not one of the available choices."),
        );
    }
    let selected = if errors.is_empty() { status } else { "active" };
    let rows = app.open_mut()?.book.read(|tx| -> Result<Vec<_>, FbError> {
        let mut rows = vec![];
        for item in salestax::list(tx, c)?
            .into_iter()
            .filter(|r| selected == "all" || r["active"] == (selected == "active"))
            .take(500)
        {
            let rate = if item["kind"] == "GROUP" {
                json!(salestax::group_rate(tx, c, &item)?.to_string())
            } else {
                item["rate"].clone()
            };
            let mut members = vec![];
            if let Some(ids) = item["member_ids"].as_array() {
                for id in ids {
                    members.push(item_vm(
                        tx,
                        c,
                        salestax::item(tx, c, id.as_str().unwrap())?,
                    )?);
                }
            }
            rows.push(json!({"item":item_vm(tx,c,item)?,"members":members,"rate":rate}));
        }
        Ok(rows)
    })?;
    let mut tokens = json!({});
    for r in &rows {
        tokens[r["item"]["pk"].as_str().unwrap()] = json!(token(
            app,
            &url(
                "salestax:status",
                &[&c.to_db(), r["item"]["pk"].as_str().unwrap()]
            )
        ));
    }
    put(&mut vm, "status_tokens", tokens);
    put(&mut vm, "rows", rows);
    put(
        &mut vm,
        "settings",
        app.open_mut()?
            .book
            .read(|tx| salestax::settings_for(tx, c))?,
    );
    put(
        &mut vm,
        "filter_form",
        forms::bind(
            &decorate(
                vec![Field::new("status", "Show", Widget::Select { choices: opts }).value(status)],
                &errors,
            ),
            &[],
        ),
    );
    Ok((
        draw(app, "salestax/list.html", &vm)?,
        if errors.is_empty() { 200 } else { 400 },
    ))
}
fn bulk(
    app: &mut App,
    route: &Route,
    path: &str,
    posted: Option<Posted<'_>>,
    errors: &Errors,
) -> AppResult<(String, u16)> {
    let c = arg_company(route)?;
    let settings = app
        .open_mut()?
        .book
        .read(|tx| salestax::settings_for(tx, c))?;
    if settings["charge_tax"] != true || settings["bulk_step"] != "" {
        app.messages
            .push("The one-time update is offered once, when sales tax is first turned on.".into());
        return Ok((url("salestax:settings", &[&c.to_db()]), 302));
    }
    let mut vm = context(app, route)?;
    let preview = app.open_mut()?.book.read(|tx| -> Result<Value, FbError> {
        let mut v = salestax::bulk_preview(tx, c)?;
        for (key, m) in [("customers", Model::Customer), ("items", Model::Item)] {
            let ids = v[key].as_array().unwrap();
            let rows = ids
                .iter()
                .map(|id| directory::get(tx, c, m, id.as_str().unwrap()))
                .collect::<Result<Vec<_>, _>>()?;
            v[key] = json!(rows);
        }
        Ok(v)
    })?;
    let fs = vec![
        Field::new("digest", "Digest", Widget::Hidden)
            .required()
            .value(data_value(posted, "digest", string(&preview["digest"])).unwrap_or_default()),
        Field::new(
            "customers",
            "Make all existing customers taxable",
            Widget::Checkbox {
                checked: posted.map(|p| checked(p, "customers")).unwrap_or(true),
            },
        ),
        Field::new(
            "items",
            "Make all existing non-stock (goods) items taxable",
            Widget::Checkbox {
                checked: posted.map(|p| checked(p, "items")).unwrap_or(true),
            },
        ),
    ];
    put(
        &mut vm,
        "customer_rows",
        preview["customers"]
            .as_array()
            .unwrap()
            .iter()
            .take(50)
            .cloned()
            .collect::<Vec<_>>(),
    );
    put(
        &mut vm,
        "item_rows",
        preview["items"]
            .as_array()
            .unwrap()
            .iter()
            .take(50)
            .cloned()
            .collect::<Vec<_>>(),
    );
    put(&mut vm, "preview", preview);
    put(
        &mut vm,
        "form",
        forms::bind(&decorate(fs, errors), &nonfield(errors, "__all__")),
    );
    put(&mut vm, "csrf_token", token(app, path));
    Ok((draw(app, "salestax/bulk_step.html", &vm)?, 200))
}
pub(super) fn render(
    app: &mut App,
    route: &Route,
    p: Posted<'_>,
    path: &str,
) -> AppResult<(String, u16)> {
    match route.name.as_str() {
        "salestax:settings" => Ok((settings(app, route, path, None, &Errors::new())?, 200)),
        "salestax:list" => list(app, route, p),
        "salestax:rate_new"
        | "salestax:rate_edit"
        | "salestax:group_new"
        | "salestax:group_edit" => Ok((item_form(app, route, path, None, &Errors::new())?, 200)),
        "salestax:bulk_step" => bulk(app, route, path, None, &Errors::new()),
        _ => Err(AppError::NotFound(path.into())),
    }
}
fn service_errors(errors: &mut Errors, messages: Vec<String>) {
    // _errors in the Python view attaches the service's message_dict to form fields.
    // The core's transport error flattens messages, so restore only these documented associations.
    for message in messages {
        let field = if message
            == "A tax rate or group with this name already exists in this company (including inactive ones)."
        {
            "name"
        } else if message == "Enter a rate from 0 to 100 percent with at most 4 decimal places." {
            "rate"
        } else if message.contains("agency")
            || message == "Choose an active vendor as the tax agency."
        {
            "agency"
        } else if message.starts_with("A tax group")
            || message.starts_with("Choose each tax rate")
            || message.starts_with("Choose tax rates")
            || message == "Choose active tax rates."
            || message.starts_with("An active group needs")
        {
            "members"
        } else if [
            "Choose a default tax from this company.",
            "Choose an active tax rate or group as the default.",
            "Every rate in that group must be active.",
        ]
        .contains(&message.as_str())
        {
            "default_item"
        } else {
            "__all__"
        };
        error(errors, field, message);
    }
}
pub(super) fn submit(
    app: &mut App,
    route: &Route,
    path: &str,
    p: Posted<'_>,
) -> AppResult<Outcome> {
    let c = arg_company(route)?;
    let mut errors = Errors::new();
    if route.name == "salestax:status" {
        let item = at(app, route)?.unwrap();
        let revision = integer_clean(p, "expected_revision", 1, None, true, &mut errors);
        let active = get(p, "active");
        let mut messages = vec!["Choose deactivate or reactivate.".into()];
        if errors.is_empty() && [Some("0"), Some("1")].contains(&active) {
            match app.run(Command::SetTaxItemActive {
                company_id: c.to_db(),
                item_id: route.args["item_id"].clone(),
                active: active == Some("1"),
                expected_revision: revision.unwrap(),
            }) {
                Ok(_) => {
                    app.messages.push(format!(
                        "\"{}\" {}.",
                        item["name"].as_str().unwrap(),
                        if active == Some("1") {
                            "reactivated"
                        } else {
                            "deactivated"
                        }
                    ));
                    return Ok(Outcome::Redirect {
                        location: url("salestax:list", &[&c.to_db()]),
                    });
                }
                Err(AppError::Core(FbError::Validation(e))) => messages = e,
                Err(e) => return Err(e),
            }
        }
        let mut vm = context(app, route)?;
        put(&mut vm, "item", item);
        put(&mut vm, "errors", messages);
        return Ok(Outcome::Invalid {
            html: draw(app, "salestax/status_error.html", &vm)?,
        });
    }
    if route.name == "salestax:bulk_step" {
        let (_, status) = bulk(app, route, path, None, &Errors::new())?;
        if status == 302 {
            return Ok(Outcome::Redirect {
                location: url("salestax:settings", &[&c.to_db()]),
            });
        }
        let digest = clean_text(p, "digest", 64, true, true, &mut errors);
        if digest.chars().count() < 64 && !digest.is_empty() {
            error(
                &mut errors,
                "digest",
                format!(
                    "Ensure this value has at least 64 characters (it has {}).",
                    digest.chars().count()
                ),
            );
        }
        let decision = choice_clean(
            p,
            "decision",
            &choices(&[("apply", "Apply"), ("skip", "Skip")]),
            true,
            &mut errors,
        );
        if errors.is_empty() {
            match app.run(Command::FinishTaxBulkStep {
                company_id: c.to_db(),
                customers: decision != "skip" && checked(p, "customers"),
                items: decision != "skip" && checked(p, "items"),
                digest,
            }) {
                Ok(Outcome::Done { data }) => {
                    app.messages.push(format!(
                        "Sales tax is on. Marked {} customer(s) and {} item(s) taxable.",
                        data["customers"].as_array().unwrap().len(),
                        data["items"].as_array().unwrap().len()
                    ));
                    return Ok(Outcome::Redirect {
                        location: url("salestax:settings", &[&c.to_db()]),
                    });
                }
                Err(AppError::Core(FbError::Validation(e))) => service_errors(&mut errors, e),
                Err(e) => return Err(e),
                _ => unreachable!(),
            }
        }
        return Ok(Outcome::Invalid {
            html: bulk(app, route, path, Some(p), &errors)?.0,
        });
    }
    let settings_page = route.name == "salestax:settings";
    let group = route.name.contains("group_");
    let item = if settings_page { None } else { at(app, route)? };
    let fs = if settings_page {
        let settings = app
            .open_mut()?
            .book
            .read(|tx| salestax::settings_for(tx, c))?;
        settings_fields(app, c, &settings)?
    } else {
        item_fields(app, c, item.as_ref(), group)?
    };
    let mut values = json!({});
    let rev = integer_clean(p, "expected_revision", 0, None, settings_page, &mut errors);
    let mut members = vec![];
    for f in fs.iter().filter(|f| f.name != "expected_revision") {
        let v = match &f.widget {
            Widget::Radio { choices } | Widget::Select { choices } => {
                let raw = get(p, &f.name).unwrap_or("");
                if f.name == "agency" && !raw.is_empty() && !choices.iter().any(|(id, _)| id == raw)
                {
                    error(&mut errors, &f.name, MODEL_CHOICE);
                    raw.into()
                } else {
                    choice_clean(p, &f.name, choices, f.required, &mut errors)
                }
            }
            Widget::Checkboxes { choices, .. } => {
                members = p
                    .iter()
                    .filter(|(k, _)| k == "members")
                    .map(|(_, v)| v.clone())
                    .collect();
                if members.is_empty() {
                    error(&mut errors, "members", REQUIRED);
                } else if let Some(invalid) = members
                    .iter()
                    .find(|id| !choices.iter().any(|(v, _)| v == *id))
                {
                    error(
                        &mut errors,
                        "members",
                        format!(
                            "Select a valid choice. {invalid} is not one of the available choices."
                        ),
                    );
                }
                String::new()
            }
            _ => clean_text(
                p,
                &f.name,
                f.attrs
                    .iter()
                    .find(|(k, _)| k == "maxlength")
                    .and_then(|(_, v)| v.parse().ok())
                    .unwrap_or(usize::MAX),
                f.required,
                true,
                &mut errors,
            ),
        };
        values[&f.name] = json!(v);
    }
    if errors.is_empty() {
        let s = |k: &str| values[k].as_str().unwrap_or("").to_owned();
        let command = if settings_page {
            Command::SaveTaxSettings {
                company_id: c.to_db(),
                expected_revision: rev.unwrap(),
                charge_tax: s("charge_tax") == "yes",
                default_item_id: (!s("default_item").is_empty()).then(|| s("default_item")),
                owed_basis: s("owed_basis"),
                filing_frequency: s("filing_frequency"),
            }
        } else if group {
            Command::SaveTaxGroup {
                company_id: c.to_db(),
                item_id: route.args.get("item_id").cloned(),
                expected_revision: rev,
                name: s("name"),
                description: s("description"),
                member_ids: members,
            }
        } else {
            Command::SaveTaxRate {
                company_id: c.to_db(),
                item_id: route.args.get("item_id").cloned(),
                expected_revision: rev,
                name: s("name"),
                description: s("description"),
                rate: s("rate"),
                agency_id: (!s("agency").is_empty()).then(|| s("agency")),
                new_agency_name: s("new_agency_name"),
            }
        };
        match app.run(command) {
            Ok(Outcome::Done { data }) => {
                let dest = if settings_page && data["offer"] == true {
                    "salestax:bulk_step"
                } else if settings_page {
                    app.messages.push("Sales tax settings saved.".into());
                    "salestax:settings"
                } else {
                    app.messages.push(if group{"Tax group saved. Documents saved earlier keep the rates they were saved with."}else{"Tax rate saved. Documents saved earlier keep the rate they were saved with."}.into());
                    "salestax:list"
                };
                return Ok(Outcome::Redirect {
                    location: url(dest, &[&c.to_db()]),
                });
            }
            Err(AppError::Core(FbError::Validation(e))) => service_errors(&mut errors, e),
            Err(e) => return Err(e),
            _ => unreachable!(),
        }
    }
    Ok(Outcome::Invalid {
        html: if settings_page {
            settings(app, route, path, Some(p), &errors)?
        } else {
            item_form(app, route, path, Some(p), &errors)?
        },
    })
}
