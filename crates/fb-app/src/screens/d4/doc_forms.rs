//! Shared frozen EstimateForm/InvoiceForm/SalesLineForm construction and cleaning.
use super::*;
use fb_core::py;
#[derive(Clone, Copy, PartialEq)]
pub(in crate::screens) enum KindForm {
    Estimate,
    Invoice,
    Sale,
}
fn blank(list: &[(String, String)], label: &str) -> Vec<(String, String)> {
    let mut v = vec![(String::new(), label.into())];
    v.extend(list.iter().cloned());
    v
}
pub(in crate::screens) fn choices(
    app: &mut App,
    c: CompanyId,
    kind: KindForm,
    current: Option<&Value>,
    lines: &[Value],
) -> AppResult<InvoiceChoices> {
    let mut ch = super::super::invoice_choices(&mut app.open_mut()?.book, c)?;
    ch.customers = choices::customers(
        &mut app.open_mut()?.book,
        c,
        false,
        current.and_then(|h| h["customer_id"].as_str()),
        true,
    )?;
    if kind == KindForm::Estimate {
        for v in ch.catalog.as_object_mut().unwrap().values_mut() {
            v.as_object_mut().unwrap().remove("income_account");
        }
    }
    let retained = lines
        .iter()
        .filter_map(|l| l["item_id"].as_str())
        .collect::<Vec<_>>();
    let o = app.open_mut()?;
    o.book.read(|tx| -> Result<(), FbError> {
        let item_ord = super::super::d2::ordinals(tx, c, "directory_item")?;
        ch.items.clear();
        for it in fb_core::directory::list(tx, c, fb_core::directory::Model::Item)? {
            let id = it["id"].as_str().unwrap();
            if it["active"] == true || retained.contains(&id) {
                ch.items.push((
                    id.into(),
                    format!(
                        "{} · #{}{}",
                        it["code"].as_str().unwrap(),
                        item_ord[id],
                        if it["active"] == true {
                            ""
                        } else if kind == KindForm::Sale {
                            " (archived)"
                        } else {
                            " (archived — confirm retention or replace)"
                        }
                    ),
                ));
            }
        }
        if let Some(h) = current {
            let id = h["customer_id"].as_str().unwrap();
            let customer = fb_core::directory::get(tx, c, fb_core::directory::Model::Customer, id)?;
            if !ch.customers.iter().any(|(v, _)| v == id) {
                ch.customers.push((
                    id.into(),
                    format!(
                        "{} (archived — confirm retention or replace)",
                        customer["display_name"].as_str().unwrap()
                    ),
                ));
            }
            if h["tax_mode"] != "NONE" {
                let rate = h["tax_rate"]
                    .as_str()
                    .unwrap_or("0")
                    .trim_end_matches('0')
                    .trim_end_matches('.');
                ch.tax.insert(
                    if ch.tax.first().is_some_and(|(id, _)| id.is_empty()) {
                        2
                    } else {
                        1
                    },
                    (
                        "KEEP".into(),
                        format!(
                            "Keep saved: {} · {}%",
                            h["tax_label"].as_str().unwrap_or(""),
                            if rate.is_empty() { "0" } else { rate }
                        ),
                    ),
                );
                ch.tax_initial = "KEEP".into();
                if let Some(id) = h["tax_item_id"].as_str()
                    && let Ok(item) = fb_core::salestax::item(tx, c, id)
                    && item["active"] == true
                    && fb_core::salestax::same_snapshot(tx, c, &h["tax_snapshot"], &item)?
                {
                    ch.tax_initial = format!("T{id}");
                }
            } else {
                ch.tax_initial = "NONE".into();
            }
        }
        if kind == KindForm::Sale {
            let accounts = fb_core::store::accounts(tx, c)?;
            ch.accounts.retain(|(id, _)| {
                accounts
                    .iter()
                    .any(|a| a.id.to_db() == *id && a.kind == fb_core::model::Kind::Income)
            });
            for (_, label) in &mut ch.accounts {
                *label = label.replace(" (unavailable — replace before posting)", " (unavailable)");
            }
        }
        Ok(())
    })?;
    Ok(ch)
}
fn fields(
    app: &mut App,
    c: CompanyId,
    kind: KindForm,
    current: Option<&Value>,
    lines: &[Value],
    p: Option<Posted<'_>>,
    e: &Errors,
) -> AppResult<(Vec<Field>, InvoiceChoices)> {
    let retry = if current.is_none() {
        choices::retry_document(app, c, kind == KindForm::Estimate, p)?
    } else {
        None
    };
    let choice_current = retry.as_ref().map(|v| &v["current"]).or(current);
    let choice_lines = retry
        .as_ref()
        .and_then(|v| v["lines"].as_array())
        .map(Vec::as_slice)
        .unwrap_or(lines);
    let ch = choices(app, c, kind, choice_current, choice_lines)?;
    let today = (app.today)();
    let v = |k: &str, initial: Option<String>| super::super::d2::data_value(p, k, initial);
    let initial = |k: &str| current.and_then(|h| h[k].as_str()).map(str::to_owned);
    let on = |k: &str| p.is_some_and(|p| super::super::d2::checked(p, k));
    let mut header = vec![
        Field::new("request_key", "Request key", Widget::Hidden)
            .value_opt(v("request_key", Some(uuid::Uuid::new_v4().to_string()))),
        Field::new("expected_revision", "Expected revision", Widget::Hidden)
            .required()
            .value_opt(v(
                "expected_revision",
                Some(
                    current
                        .map(|h| h["number"].to_string())
                        .unwrap_or_else(|| "0".into()),
                ),
            )),
        Field::new(
            "customer",
            "Customer",
            Widget::Select {
                choices: blank(&ch.customers, "Choose…"),
            },
        )
        .required()
        .value_opt(v("customer", initial("customer_id"))),
        Field::new("date", "Date", Widget::Date)
            .required()
            .value_opt(v("date", initial("date").or(Some(today.to_string())))),
    ];
    if kind == KindForm::Estimate {
        header.push(
            Field::new("expiry", "Expiry", Widget::Date).value_opt(v("expiry", initial("expiry"))),
        );
    }
    header.extend([
        Field::new("memo", "Memo", Widget::Textarea { cols: 40, rows: 2 })
            .attr("maxlength", "2000")
            .value_opt(v("memo", initial("memo"))),
        Field::new(
            "tax_mode",
            "Sales tax",
            Widget::Select {
                choices: ch.tax.clone(),
            },
        )
        .required()
        .value_opt(v("tax_mode", Some(ch.tax_initial.clone()))),
        Field::new(
            "refresh_customer",
            "Update customer name and address from the customer list",
            Widget::Checkbox {
                checked: on("refresh_customer"),
            },
        ),
        Field::new(
            "keep_archived",
            "Keep archived customer details on this document",
            Widget::Checkbox {
                checked: on("keep_archived"),
            },
        ),
    ]);
    if kind == KindForm::Invoice {
        header.extend([
            Field::new("due_date", "Due date", Widget::Date)
                .required()
                .value_opt(v(
                    "due_date",
                    initial("due_date").or(Some((today + chrono::Duration::days(30)).to_string())),
                )),
            terms(p),
        ]);
    }
    Ok((decorate(header, e), ch))
}
pub(in crate::screens) fn terms(p: Option<Posted<'_>>) -> Field {
    Field::new(
        "terms",
        "Due-date convenience (select to fill)",
        Widget::Select {
            choices: vec![
                ("".into(), "Explicit due date".into()),
                ("0".into(), "Due on receipt".into()),
                ("15".into(), "Net 15".into()),
                ("30".into(), "Net 30".into()),
            ],
        },
    )
    .value_opt(super::super::d2::data_value(p, "terms", None))
}
fn rows(
    kind: KindForm,
    ch: &InvoiceChoices,
    prefix: &str,
    data: Option<&HashMap<String, String>>,
    e: &Errors,
) -> Vec<Field> {
    let mut fs = super::super::line_fields(prefix, ch, data, e, true);
    if kind == KindForm::Estimate {
        fs.retain(|f| f.name != "income_account");
    }
    if kind == KindForm::Sale {
        fs.retain(|f| {
            !["origin_position", "refresh_item", "keep_archived"].contains(&f.name.as_str())
        });
        for f in &mut fs {
            if f.name == "income_account" {
                f.label = "Income account".into();
            }
            if f.name == "description" {
                f.attrs.retain(|(k, _)| k != "maxlength");
            }
        }
    }
    fs
}
pub(in crate::screens) fn formset(
    kind: KindForm,
    ch: &InvoiceChoices,
    p: Option<Posted<'_>>,
    initial: &[Value],
    e: &Errors,
    non: &[String],
) -> FormsetVm {
    let extra = if kind == KindForm::Estimate { 2 } else { 1 };
    let initial_len = initial.len();
    let total = p
        .map(|p| {
            get(p, "lines-TOTAL_FORMS")
                .and_then(clean_integer)
                .unwrap_or(0)
                .clamp(0, 50) as usize
        })
        .unwrap_or(initial_len.max(1) + extra);
    let mut data: HashMap<String, String> = p.unwrap_or(&[]).iter().cloned().collect();
    if p.is_none() {
        for (i, l) in initial.iter().enumerate() {
            for (to, from) in [
                ("origin_position", "position"),
                ("item", "item_id"),
                ("income_account", "income_account_id"),
                ("description", "description"),
                ("quantity", "quantity"),
                ("rate", "rate"),
                ("taxable", "taxable"),
            ] {
                let v = &l[from];
                if !v.is_null() {
                    data.insert(
                        format!("lines-{i}-{to}"),
                        if v == true {
                            "on".into()
                        } else if v == false {
                            String::new()
                        } else {
                            v.as_str().map(str::to_owned).unwrap_or(v.to_string())
                        },
                    );
                }
            }
            data.insert(format!("lines-{i}-tax_auto"), String::new());
        }
    }
    let forms = (0..total)
        .map(|i| {
            forms::bind(
                &rows(
                    kind,
                    ch,
                    &format!("lines-{i}"),
                    if p.is_some() || i < initial_len {
                        Some(&data)
                    } else {
                        None
                    },
                    e,
                ),
                &nonfield(e, &format!("lines-{i}")),
            )
        })
        .collect();
    let management = [
        ("TOTAL_FORMS", p.map(|p|get(p,"lines-TOTAL_FORMS").unwrap_or("").to_owned()).unwrap_or(total.to_string())),
        ("INITIAL_FORMS", p.map(|p|get(p,"lines-INITIAL_FORMS").unwrap_or("").to_owned()).unwrap_or(initial_len.to_string())),
        ("MIN_NUM_FORMS", p.map(|p|get(p,"lines-MIN_NUM_FORMS").unwrap_or("").to_owned()).unwrap_or("1".into())),
        ("MAX_NUM_FORMS", p.map(|p|get(p,"lines-MAX_NUM_FORMS").unwrap_or("").to_owned()).unwrap_or("50".into())),
    ];
    let mgmt = super::super::d2::management_html("lines", &management, p.is_some());
    FormsetVm {
        management_form: mgmt,
        forms,
        empty_form: forms::bind(
            &rows(kind, ch, "lines-__prefix__", None, &Errors::new()),
            &[],
        ),
        non_form_errors: forms::error_list(non, None)
            .replace("errorlist nonfield", "errorlist nonform"),
        total_error_count: super::super::d2::formset_error_count(e, p, "lines") + non.len(),
    }
}
#[allow(clippy::too_many_arguments)]
pub(in crate::screens) fn page(
    app: &mut App,
    r: &Route,
    path: &str,
    kind: KindForm,
    root: Option<&Value>,
    p: Option<Posted<'_>>,
    e: &Errors,
    non: &[String],
) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let mut vm = context(app, r)?;
    let current = root.map(|v| &v["current"]);
    let lines = root
        .and_then(|v| v["lines"].as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let (header, ch) = fields(app, c, kind, current, lines, p, e)?;
    put(&mut vm, "csrf_token", token(app, path));
    put(&mut vm, "form", forms::bind(&header, &nonfield(e, "")));
    put(&mut vm, "formset", formset(kind, &ch, p, lines, e, non));
    put(&mut vm, "catalog", ch.catalog);
    put(&mut vm, "customer_tax", ch.customer_tax);
    put(&mut vm, "current", current);
    let (key, template) = if kind == KindForm::Estimate {
        ("estimate", "sales/form.html")
    } else {
        ("invoice", "sales/invoices/form.html")
    };
    put(&mut vm, key, root);
    Ok((
        draw(app, template, &vm)?,
        if p.is_some() { 400 } else { 200 },
    ))
}
pub(in crate::screens) fn clean_lines(
    p: Posted<'_>,
    kind: KindForm,
    ch: &InvoiceChoices,
    stored_initial: usize,
    e: &mut Errors,
) -> (Vec<InvoiceLineValues>, Vec<String>) {
    let total = get(p, "lines-TOTAL_FORMS").and_then(clean_integer);
    let init = get(p, "lines-INITIAL_FORMS").and_then(clean_integer);
    let mut non = vec![];
    if total.is_none() || init.is_none() {
        let names = [("TOTAL_FORMS", total), ("INITIAL_FORMS", init)]
            .into_iter()
            .filter(|(_, v)| v.is_none())
            .map(|(n, _)| format!("lines-{n}"))
            .collect::<Vec<_>>()
            .join(", ");
        non.push(format!("ManagementForm data is missing or has been tampered with. Missing fields: {names}. You may need to file a bug report if the issue persists."));
    }
    if total.is_some_and(|n| n > 50) {
        non.push("Please submit at most 50 forms.".into());
    }
    let n = total.unwrap_or(0).clamp(0, 50) as usize;
    let mut out = vec![];
    let mut present = 0usize;
    for i in 0..n {
        let prefix = format!("lines-{i}");
        let local = p
            .iter()
            .filter_map(|(k, v)| {
                k.strip_prefix(&format!("{prefix}-"))
                    .map(|k| (k.into(), v.clone()))
            })
            .collect::<Vec<_>>();
        let on = |k: &str| super::super::d2::checked(&local, k);
        let deleted = on("DELETE");
        let mut raw = vec!["origin_position", "item"];
        if kind != KindForm::Estimate { raw.push("income_account"); }
        // Existing quantity/rate initials are Python Decimals in CharFields. Django considers
        // those rows changed even if a posted INITIAL_FORMS count mislabels them as extra.
        let changed = i < stored_initial || super::super::d2::extra_row_changed(
            &local, &[("description", ""), ("quantity", ""), ("rate", "")],
            &raw, &["taxable", "refresh_item", "keep_archived", "DELETE"],
        );
        if !changed && i >= init.unwrap_or(0).max(1) as usize {
            continue;
        }
        present += usize::from(changed && !deleted);
        let mut le = Errors::new();
        let description = clean_text(&local, "description", 1000, true, true, &mut le);
        let mut decimal = |name: &str| {
            let v = clean_text(&local, name, 40, true, true, &mut le);
            if le.contains_key(name) {
                return v;
            }
            let result = if name == "quantity" {
                fb_core::calculations::quantity(&json!(v))
            } else {
                fb_core::calculations::unit_rate(&json!(v))
            };
            match result {
                Ok(n) => n.to_string(),
                Err(FbError::Validation(messages)) => {
                    error(&mut le, name, messages.join("; "));
                    v
                }
                Err(_) => v,
            }
        };
        let quantity = decimal("quantity");
        let rate = decimal("rate");
        let choice = |key: &str, list: &[(String, String)], required: bool, le: &mut Errors| {
            let v = get(&local, key).unwrap_or("").to_owned();
            if v.is_empty() {
                if required {
                    error(le, key, "This field is required.");
                }
            } else if !list.iter().any(|(id, _)| id == &v) {
                error(
                    le,
                    key,
                    "Select a valid choice. That choice is not one of the available choices.",
                );
            }
            v
        };
        let item = choice("item", &ch.items, false, &mut le);
        let account = if kind != KindForm::Estimate {
            choice(
                "income_account",
                &ch.accounts,
                kind == KindForm::Sale,
                &mut le,
            )
        } else {
            String::new()
        };
        let origin_raw = get(&local, "origin_position").unwrap_or("");
        let origin = if origin_raw.is_empty() {
            None
        } else {
            let value = clean_integer(origin_raw);
            match value {
                Some(n) if n < 1 => error(
                    &mut le,
                    "origin_position",
                    "Ensure this value is greater than or equal to 1.",
                ),
                Some(n) if n > 50 => error(
                    &mut le,
                    "origin_position",
                    "Ensure this value is less than or equal to 50.",
                ),
                None => error(&mut le, "origin_position", "Enter a whole number."),
                _ => {}
            }
            value
        };
        clean_text(&local, "tax_auto", 1, false, true, &mut le);
        for (k, v) in le {
            e.insert(format!("{prefix}-{k}"), v);
        }
        if deleted { continue; }
        out.push(InvoiceLineValues {
            origin_position: origin,
            item_id: (!item.is_empty()).then_some(item),
            description,
            quantity,
            rate,
            taxable: on("taxable"),
            refresh_item: on("refresh_item"),
            keep_archived: on("keep_archived"),
            income_account_id: (!account.is_empty()).then_some(account),
        });
    }
    if present < 1 && non.is_empty() {
        non.push("Please submit at least 1 form.".into());
    }
    (out, non)
}
pub(in crate::screens) fn submit(
    app: &mut App,
    r: &Route,
    path: &str,
    kind: KindForm,
    root: Option<&Value>,
    p: Posted<'_>,
) -> AppResult<Outcome> {
    let c = arg_company(r)?;
    let current = root.map(|v| &v["current"]);
    let lines = root
        .and_then(|v| v["lines"].as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let retry = if current.is_none() {
        choices::retry_document(app, c, kind == KindForm::Estimate, Some(p))?
    } else {
        None
    };
    let choice_current = retry.as_ref().map(|v| &v["current"]).or(current);
    let choice_lines = retry
        .as_ref()
        .and_then(|v| v["lines"].as_array())
        .map(Vec::as_slice)
        .unwrap_or(lines);
    let ch = choices(app, c, kind, choice_current, choice_lines)?;
    let mut e = Errors::new();
    let key = clean_text(p, "request_key", usize::MAX, root.is_none(), true, &mut e);
    if !key.is_empty() && py::py_uuid(&key).is_none() {
        error(&mut e, "request_key", "Enter a valid UUID.");
    }
    let rev = clean_revision(p, 0, &mut e);
    let customer = get(p, "customer").unwrap_or("").to_owned();
    if customer.is_empty() {
        error(&mut e, "customer", "This field is required.");
    } else if !ch.customers.iter().any(|(id, _)| id == &customer) {
        error(
            &mut e,
            "customer",
            "Select a valid choice. That choice is not one of the available choices.",
        );
    }
    let date = clean_date(p, "date", true, &mut e).unwrap_or_default();
    let second = clean_date(
        p,
        if kind == KindForm::Estimate {
            "expiry"
        } else {
            "due_date"
        },
        kind == KindForm::Invoice,
        &mut e,
    );
    let memo = clean_text(p, "memo", 2000, false, true, &mut e);
    let tax = get(p, "tax_mode").unwrap_or("").to_owned();
    if !ch.tax.iter().any(|(id, _)| id == &tax) {
        error(
            &mut e,
            "tax_mode",
            if tax.is_empty() {"This field is required.".to_owned()}
            else {format!("Select a valid choice. {tax} is not one of the available choices.")},
        );
    }
    if kind == KindForm::Invoice && !matches!(get(p, "terms").unwrap_or(""), "" | "0" | "15" | "30")
    {
        error(
            &mut e,
            "terms",
            format!("Select a valid choice. {} is not one of the available choices.", get(p, "terms").unwrap_or("")),
        );
    }
    let (lines, non) = clean_lines(p, kind, &ch, lines.len(), &mut e);
    if super::super::d2::active_form_errors(&e, p) == 0 && non.is_empty() {
        let tax_item_id = tax.strip_prefix('T').map(str::to_owned);
        let values = InvoiceValues {
            request_key: key,
            customer_id: customer,
            date,
            due_date: second.clone(),
            memo,
            tax_mode: if tax_item_id.is_some() {
                "ITEM".into()
            } else {
                tax
            },
            tax_item_id,
            tax_rate: None,
            tax_label: None,
            refresh_customer: super::super::d2::checked(p, "refresh_customer"),
            keep_archived: super::super::d2::checked(p, "keep_archived"),
            lines,
        };
        let cmd = if kind == KindForm::Invoice {
            Command::SaveInvoiceDraft {
                company_id: c.to_db(),
                invoice_id: root.and_then(|v| v["id"].as_str()).map(str::to_owned),
                expected_revision: rev,
                values,
            }
        } else {
            Command::SaveEstimate {
                company_id: c.to_db(),
                estimate_id: root.and_then(|v| v["id"].as_str()).map(str::to_owned),
                expected_revision: rev,
                expiry: second,
                values,
            }
        };
        match app.run(cmd) {
            Ok(Outcome::Done { data }) => {
                return Ok(Outcome::Redirect {
                    location: url(
                        if kind == KindForm::Estimate {
                            "sales:detail"
                        } else {
                            "invoices:detail"
                        },
                        &[&c.to_db(), data["id"].as_str().unwrap()],
                    ),
                });
            }
            Err(AppError::Core(FbError::Validation(messages))) => {
                error(&mut e, "", messages.join("; "))
            }
            Err(e) => return Err(e),
            _ => unreachable!(),
        }
    }
    Ok(Outcome::Invalid {
        html: page(app, r, path, kind, root, Some(p), &e, &non)?.0,
    })
}
