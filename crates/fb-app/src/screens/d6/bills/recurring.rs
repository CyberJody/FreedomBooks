use super::*;
use crate::commands::RecurringBillValues;
use fb_core::payables::recurring as core;
fn load(app: &mut App, r: &Route) -> AppResult<Value> {
    let c = arg_company(r)?;
    let o = app.open_mut()?;
    Ok(core::detail(
        &mut o.book,
        o.owner,
        c,
        &r.args["template_id"],
    )?)
}
fn fields(app: &mut App, c: CompanyId) -> AppResult<Vec<Field>> {
    Ok(vec![
        Field::new("source_date", "Source bill date", Widget::Hidden),
        Field::new("source_bill_id", "Source bill", Widget::Hidden),
        Field::new("request_key", "Request key", Widget::Hidden),
        Field::new("expected_revision", "Revision", Widget::Hidden),
        Field::new("name", "Template name", Widget::Text).required(),
        select(
            "vendor",
            "Vendor",
            "Choose a vendor",
            contacts(app, c, false, None, "short")?,
            true,
        ),
        Field::new("memo", "Memo", Widget::Textarea { cols: 40, rows: 2 }),
        Field::new(
            "terms_days",
            "Due this many days after each bill date",
            Widget::Number,
        )
        .required()
        .attr("min", "0"),
        select(
            "frequency",
            "Repeat",
            "Choose frequency",
            vec![
                ("MONTHLY".into(), "Monthly".into()),
                ("YEARLY".into(), "Yearly".into()),
            ],
            true,
        ),
        Field::new("start_date", "First bill date", Widget::Date).required(),
        Field::new(
            "end_date",
            "Last bill date (optional, inclusive)",
            Widget::Date,
        ),
        Field::new(
            "occurrence_limit",
            "Number of scheduled dates (optional)",
            Widget::Number,
        )
        .attr("min", "1"),
        Field::new("active", "Active", Widget::Checkbox { checked: false }),
        Field::new(
            "auto_post",
            "Post automatically instead of saving drafts for review",
            Widget::Checkbox { checked: false },
        ),
    ])
}
fn form(
    app: &mut App,
    r: &Route,
    p: Option<Posted<'_>>,
    path: &str,
    e: &Errors,
    confirmation: Option<&Value>,
) -> AppResult<String> {
    let c = arg_company(r)?;
    let t = if r.args.contains_key("template_id") {
        Some(load(app, r)?)
    } else {
        None
    };
    let mut initial = json!({"request_key":uuid::Uuid::new_v4().to_string(),"expected_revision":0,"active":true,"auto_post":false,"terms_days":30,"frequency":"MONTHLY","start_date":(app.today)().to_string()});
    let mut lines = vec![];
    if let Some(t) = &t {
        initial = t.clone();
        initial["expected_revision"] = t["revision"].clone();
        initial["vendor"] = t["data"]["vendor_id"].clone();
        initial["memo"] = t["data"]["memo"].clone();
        initial["active"] = json!(t["active"] == 1);
        initial["auto_post"] = json!(t["auto_post"] == 1);
        lines = t["data"]["lines"].as_array().cloned().unwrap_or_default();
    } else if let Some(bid) = p
        .and_then(|p| get(p, "source_bill"))
        .filter(|s| !s.is_empty())
    {
        let today = (app.today)().to_string();
        let o = app.open_mut()?;
        let copied = core::from_bill(&mut o.book, o.owner, c, bid, &today)?;
        initial["source_date"] = copied["source_date"].clone();
        initial["source_bill_id"] = copied["source_bill_id"].clone();
        initial["start_date"] = copied["start_date"].clone();
        initial["vendor"] = copied["vendor_id"].clone();
        initial["memo"] = copied["memo"].clone();
        initial["terms_days"] = copied["terms_days"].clone();
        lines = copied["lines"].as_array().cloned().unwrap_or_default();
    }
    let p = p.filter(|p| !(p.len() == 1 && get(p, "source_bill").is_some()));
    let ch = choices(app, c, None)?;
    let count = p
        .and_then(|p| get(p, "lines-TOTAL_FORMS"))
        .and_then(clean_integer)
        .map(|n| n.clamp(1, 50) as usize)
        .unwrap_or((lines.len() + 1).min(50));
    let forms = (0..count)
        .map(|i| {
            let mut init = lines.get(i).cloned().unwrap_or(json!({"quantity":"1"}));
            init["item"] = init["item_id"].clone();
            init["expense_account"] = init["expense_account_id"].clone();
            bind_fields(
                line_fields(&ch, &format!("lines-{i}"), None, false)
                    .into_iter()
                    .filter(|f| {
                        [
                            "expense_account",
                            "item",
                            "description",
                            "quantity",
                            "rate",
                            "DELETE",
                        ]
                        .contains(&f.name.as_str())
                    })
                    .collect(),
                p,
                &init,
                e,
                true,
            )
        })
        .collect::<Vec<_>>();
    let mut vm = ctx(app, r)?;
    put(&mut vm, "confirmation", confirmation);
    put(
        &mut vm,
        "confirmation_json",
        confirmation.map(|preview| preview["token"].to_string()),
    );
    let mut fs = fields(app, c)?;
    if t.is_some() {
        fs.retain(|f| f.name != "active");
    }
    put(&mut vm, "template", t);
    put(&mut vm, "form", bind_fields(fs, p, &initial, e, true));
    put(&mut vm, "lines", forms);
    put(&mut vm, "count", count);
    put(&mut vm, "csrf_token", token(app, path));
    put(&mut vm, "form_action", path);
    put(
        &mut vm,
        "empty_line",
        bind_fields(
            line_fields(&ch, "lines-__prefix__", None, false)
                .into_iter()
                .filter(|f| {
                    [
                        "expense_account",
                        "item",
                        "description",
                        "quantity",
                        "rate",
                        "DELETE",
                    ]
                    .contains(&f.name.as_str())
                })
                .collect(),
            None,
            &json!({"quantity":"1"}),
            &Errors::new(),
            true,
        ),
    );
    draw(app, "payables/recurring/form.html", &vm)
}
pub(in crate::screens::d6) fn render(
    app: &mut App,
    r: &Route,
    p: Posted<'_>,
    path: &str,
) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    if ["recurring:new", "recurring:edit"].contains(&r.name.as_str()) {
        let prefill = get(p, "bill").map(|id| vec![("source_bill".into(), id.into())]);
        return Ok((
            form(app, r, prefill.as_deref(), path, &Errors::new(), None)?,
            200,
        ));
    }
    let mut vm = ctx(app, r)?;
    put(&mut vm, "messages", std::mem::take(&mut app.messages));
    put(&mut vm, "today", (app.today)().to_string());
    put(
        &mut vm,
        "can_write",
        app.authenticated_session()?.role != crate::identity::Role::Readonly,
    );
    if r.name == "recurring:list" {
        let o = app.open_mut()?;
        let mut ts = core::list(&mut o.book, o.owner, c)?;
        let today = (app.today)().to_string();
        for t in &mut ts {
            t["stalled"] = json!(core::stalled(t, &today)?);
        }
        put(&mut vm, "templates", ts);
        put(&mut vm, "vendor_labels", labels(app, c)?);
        Ok((draw(app, "payables/recurring/list.html", &vm)?, 200))
    } else {
        let mut t = load(app, r)?;
        let accounts = choices(app, c, None)?.accounts;
        let mut total = 0i128;
        if let Some(lines) = t["data"]["lines"].as_array_mut() {
            for line in lines {
                let amount = fb_core::calculations::line_amount(&line["quantity"], &line["rate"])?;
                total += amount.cents();
                line["amount"] = json!(amount.to_string());
                line["account_label"] = json!(
                        accounts
                            .iter()
                            .find(|a| Some(a.id.to_db().as_str())
                                == line["expense_account_id"].as_str())
                            .map(|a| a.label())
                            .unwrap_or_else(|| "No account selected".into())
                    );
            }
        }
        t["total"] = json!(cash(total));
        t["stalled"] = json!(core::stalled(&t, &(app.today)().to_string())?);
        put(&mut vm, "vendor_labels", labels(app, c)?);
        put(&mut vm, "template", t);
        put(&mut vm, "csrf_token", token(app, path));
        put(&mut vm, "form_action", path);
        Ok((draw(app, "payables/recurring/detail.html", &vm)?, 200))
    }
}
pub(in crate::screens::d6) fn submit(
    app: &mut App,
    r: &Route,
    path: &str,
    p: Posted<'_>,
) -> AppResult<Outcome> {
    let c = arg_company(r)?;
    let mut e = Errors::new();
    let rev = integer(p, "expected_revision", 0, None, true, &mut e).unwrap_or(0);
    if r.name == "recurring:detail" {
        let action = get(p, "action").unwrap_or("");
        let t = load(app, r)?;
        let resuming = ["resume", "resume_skip", "resume_create"].contains(&action);
        if resuming && (t["active"] != 0 || t["deleted"] == 1) {
            return Ok(Outcome::Redirect {
                location: path.into(),
            });
        }
        let preview = core::due_confirmation(&t, &(app.today)().to_string())?;
        let confirmed =
            get(p, "confirmation").and_then(|value| serde_json::from_str::<Value>(value).ok());
        if action == "delete" && !checked(p, "confirm_delete")
            || resuming
                && (action == "resume" && preview["count"] != 0
                    || action == "resume_create" && preview["more"] == true
                    || action != "resume"
                        && (confirmed.as_ref() != Some(&preview["token"]) || t["revision"] != rev))
        {
            let mut vm = ctx(app, r)?;
            put(&mut vm, "template", t);
            put(&mut vm, "deleting", action == "delete");
            put(&mut vm, "confirmation", &preview);
            put(&mut vm, "confirmation_json", preview["token"].to_string());
            put(&mut vm, "csrf_token", token(app, path));
            put(&mut vm, "form_action", path);
            return Ok(Outcome::Page {
                html: draw(app, "payables/recurring/confirm.html", &vm)?,
                status: 200,
            });
        }
        app.run(Command::ControlRecurringBill {
            company_id: c.to_db(),
            template_id: r.args["template_id"].clone(),
            expected_revision: rev,
            action: action.into(),
            confirmation: get(p, "confirmation").and_then(|v| serde_json::from_str(v).ok()),
            confirm_delete: checked(p, "confirm_delete"),
        })?;
        return Ok(Outcome::Redirect {
            location: if action == "delete" {
                url("recurring:list", &[&c.to_db()])
            } else {
                path.into()
            },
        });
    }
    if !["recurring:new", "recurring:edit"].contains(&r.name.as_str()) {
        return Err(AppError::NotFound(path.into()));
    }
    let fs = fields(app, c)?;
    let vendor = choice(p, named(&fs, "vendor"), &mut e, true);
    let frequency = choice(p, named(&fs, "frequency"), &mut e, false);
    let count = integer(p, "lines-TOTAL_FORMS", 1, Some(50), true, &mut e).unwrap_or(1);
    let mut lines = vec![];
    for i in 0..count {
        let prefix = format!("lines-{i}-");
        let row = p
            .iter()
            .filter_map(|(k, v)| k.strip_prefix(&prefix).map(|k| (k.to_owned(), v.clone())))
            .collect::<Vec<_>>();
        if checked(&row, "DELETE")
            || ["item", "expense_account", "description", "rate"]
                .iter()
                .all(|k| get(&row, k).unwrap_or("").is_empty())
        {
            continue;
        }
        let mut line_errors = Errors::new();
        lines.push(BillLineValues {
            origin_position: None,
            item_id: get(&row, "item")
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            expense_account_id: get(&row, "expense_account")
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            description: clean_text(&row, "description", 1000, true, true, &mut line_errors),
            quantity: get(&row, "quantity")
                .filter(|s| !s.is_empty())
                .unwrap_or("1")
                .into(),
            rate: clean_text(&row, "rate", 40, true, true, &mut line_errors),
            refresh_item: false,
            keep_archived: false,
        });
        for (key, errors) in line_errors {
            e.insert(format!("lines-{i}-{key}"), errors);
        }
    }
    let values = RecurringBillValues {
        source_bill_id: get(p, "source_bill_id")
            .filter(|v| !v.is_empty())
            .map(str::to_owned),
        confirmation: get(p, "confirmation").and_then(|v| serde_json::from_str(v).ok()),
        id: r.args.get("template_id").cloned(),
        request_key: get(p, "request_key").unwrap_or("").into(),
        expected_revision: rev,
        name: clean_text(p, "name", 120, true, true, &mut e),
        vendor_id: vendor,
        memo: clean_text(p, "memo", 2000, false, true, &mut e),
        terms_days: integer(p, "terms_days", 0, Some(36500), true, &mut e).unwrap_or(0),
        frequency,
        start_date: clean_date(p, "start_date", true, &mut e).unwrap_or_default(),
        end_date: clean_date(p, "end_date", false, &mut e),
        occurrence_limit: integer(p, "occurrence_limit", 1, Some(120000), false, &mut e),
        active: if r.args.contains_key("template_id") {
            load(app, r)?["active"] == 1
        } else {
            checked(p, "active")
        },
        auto_post: checked(p, "auto_post"),
        lines,
    };
    if e.is_empty() {
        let today = (app.today)().to_string();
        let o = app.open_mut()?;
        match core::save_confirmation(
            &mut o.book,
            o.owner,
            c,
            &serde_json::to_value(&values).unwrap(),
            &today,
        ) {
            Ok(preview) => {
                if (preview["count"] != 0 || !preview["source_warning"].is_null())
                    && values.confirmation.as_deref() != preview["token"].as_str()
                {
                    return Ok(Outcome::Page {
                        html: form(app, r, Some(p), path, &e, Some(&preview))?,
                        status: 200,
                    });
                }
            }
            // Limits such as the oldest allowed first bill date belong on the form, not a retry page.
            Err(FbError::Validation(messages)) => error(&mut e, "__all__", messages.join("; ")),
            Err(other) => return Err(other.into()),
        }
    }
    if e.is_empty()
        && let Some(d) = result_error(
            app.run(Command::SaveRecurringBill {
                company_id: c.to_db(),
                values,
            }),
            &mut e,
        )?
    {
        return Ok(Outcome::Redirect {
            location: url("recurring:detail", &[&c.to_db(), s(&d, "id")]),
        });
    }
    Ok(Outcome::Invalid {
        html: form(app, r, Some(p), path, &e, None)?,
    })
}
