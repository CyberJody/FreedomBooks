//! Shared frozen EstimateForm/InvoiceForm/SalesLineForm construction and cleaning.
use super::*;
use fb_core::py;
#[derive(Clone, Copy, PartialEq)]
pub(in crate::screens) enum KindForm {
    Estimate,
    Invoice,
    Sale,
}
fn invoice_text(p: Posted<'_>, name: &str, max: usize, required: bool, errors: &mut Errors) -> String {
    let raw = get(p, name).unwrap_or("");
    let value = py::py_strip(raw);
    if required && value.is_empty() { error(errors, name, "This field is required."); }
    if value.chars().count() > max { error(errors, name, py::max_length_message(max, value)); }
    if let Err(e) = py::clean_multiline_free_text(raw) { error(errors, name, e.message()); }
    value.to_owned()
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
        ch.items.clear();
        for it in fb_core::directory::list(tx, c, fb_core::directory::Model::Item)? {
            let id = it["id"].as_str().unwrap();
            if it["active"] == true || retained.contains(&id) {
                // Item codes are unique in a company, so the code alone names the item.
                ch.items.push((
                    id.into(),
                    format!("{}{}", it["code"].as_str().unwrap(), if it["active"] == true { "" } else { " (archived)" }),
                ));
            }
        }
        for (_, label) in &mut ch.customers { *label = label.replace(" (archived — confirm retention or replace)", " (archived)"); }
        if let Some(h) = current {
            let id = h["customer_id"].as_str().unwrap();
            let customer = fb_core::directory::get(tx, c, fb_core::directory::Model::Customer, id)?;
            if !ch.customers.iter().any(|(v, _)| v == id) {
                ch.customers.push((
                    id.into(),
                    format!("{} (archived)", customer["display_name"].as_str().unwrap()),
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
            // Shown as "4000 · Sales"; the ordinal only matters where codes repeat, and they cannot.
            ch.accounts = ch
                .accounts
                .iter()
                .filter_map(|(id, label)| {
                    let a = accounts.iter().find(|a| a.id.to_db() == *id)?;
                    let unavailable = label.ends_with(" (unavailable — replace before posting)");
                    Some((id.clone(), format!("{} · {}{}", a.code, a.name, if unavailable { " (unavailable)" } else { "" })))
                })
                .collect();
        }
        Ok(())
    })?;
    Ok(ch)
}
#[allow(clippy::too_many_arguments)]
fn fields(
    app: &mut App,
    c: CompanyId,
    kind: KindForm,
    current: Option<&Value>,
    lines: &[Value],
    p: Option<Posted<'_>>,
    e: &Errors,
    preset: Option<&str>,
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
        .value_opt(v("customer", initial("customer_id").or(preset.map(str::to_owned)))),
        Field::new("date", "Date", Widget::Date)
            .required()
            .value_opt(v("date", initial("date").or(Some(today.to_string())))),
    ];
    if kind == KindForm::Estimate {
        // A new estimate is good for 30 days unless the person changes it.
        let expiry = if current.is_some() { initial("expiry") } else { Some((today + chrono::Duration::days(30)).to_string()) };
        header.push(
            Field::new("expiry", "Expires", Widget::Date).value_opt(v("expiry", expiry)),
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
            "Update this estimate with the customer's current name and address",
            Widget::Checkbox {
                checked: on("refresh_customer"),
            },
        ),
        Field::new(
            "keep_archived",
            "Keep this archived customer on the estimate",
            Widget::Checkbox {
                checked: on("keep_archived"),
            },
        ),
    ]);
    if kind == KindForm::Estimate {
        // The two customer choices matter only when the saved customer is archived (keep it, or
        // choose another) or has changed in the customer list since this estimate copied it.
        let (archived, changed) = customer_flags(app, c, current)?;
        for field in &mut header {
            let shown = match field.name.as_str() {
                "keep_archived" => archived,
                "refresh_customer" => changed && !archived,
                _ => continue,
            };
            if !shown {
                field.widget = Widget::Hidden;
                field.value = Some(String::new());
            }
        }
    }
    if kind == KindForm::Invoice {
        // Historical copies belong to the document. Editing an unchanged selection keeps
        // them automatically; the core still rejects newly selected archived records.
        for field in &mut header {
            if matches!(field.name.as_str(), "refresh_customer" | "keep_archived") {
                field.widget = Widget::Hidden;
                field.value = Some(if field.name == "keep_archived" && choice_current.is_some() { "1" } else { "" }.into());
            }
        }
        // A new invoice is Net 30; a saved one shows the term its dates match, else Custom.
        let due = initial("due_date").or(Some((today + chrono::Duration::days(30)).to_string()));
        let shown = if current.is_some() { term_for(initial("date").as_deref(), due.as_deref()) } else { "30" };
        header.extend([
            Field::new("due_date", "Due date", Widget::Date)
                .required()
                .value_opt(v("due_date", due)),
            terms_with(p, Some(shown)),
        ]);
    }
    Ok((decorate(header, e), ch))
}
/// Whether a saved document's customer is now archived, and whether the customer list now holds
/// different name, address or contact details than the copy saved on the document.
fn customer_flags(app: &mut App, c: CompanyId, current: Option<&Value>) -> AppResult<(bool, bool)> {
    let Some(h) = current else { return Ok((false, false)) };
    let Some(id) = h["customer_id"].as_str() else { return Ok((false, false)) };
    let customer = app.open_mut()?.book.read(|tx| fb_core::directory::get(tx, c, fb_core::directory::Model::Customer, id))?;
    let snapshot = &h["customer_snapshot"];
    let changed = ["display_name", "business_name", "contact_name", "billing_address", "email", "phone"]
        .iter()
        .any(|k| text_of(&customer[*k]) != text_of(&snapshot[*k]));
    Ok((customer["active"] != true, changed))
}
fn text_of(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}
/// For each saved line (by position) whose item is still the one on the line: is that item now
/// archived, and does the item list now hold different details or account defaults than the
/// copy saved on the line? These decide when a line shows its two item choices.
fn item_flags(app: &mut App, c: CompanyId, lines: &[Value]) -> AppResult<HashMap<i64, (bool, bool)>> {
    let mut out = HashMap::new();
    let o = app.open_mut()?;
    o.book.read(|tx| -> Result<(), FbError> {
        for line in lines {
            let (Some(position), Some(id)) = (line["position"].as_i64(), line["item_id"].as_str()) else { continue };
            let Ok(item) = fb_core::directory::get(tx, c, fb_core::directory::Model::Item, id) else { continue };
            let snapshot = &line["item_snapshot"];
            let details = ["code", "description", "kind", "sale_rate"].iter().any(|k| snapshot[*k] != item[*k]);
            let accounts = [("income_account_id", "income_account"), ("expense_account_id", "expense_account")]
                .iter()
                .any(|(on_line, on_item)| text_of(&line[*on_line]) != item[*on_item].as_str().or(item[format!("{on_item}_id")].as_str()).unwrap_or(""));
            out.insert(position, (item["active"] != true, details || accounts));
        }
        Ok(())
    })?;
    Ok(out)
}
/// The payment terms a person can pick; each sets the due date from the invoice date.
pub(in crate::screens) const TERMS: &[(&str, &str)] = &[
    ("", "Custom"),
    ("0", "Due on receipt"),
    ("15", "Net 15"),
    ("30", "Net 30"),
    ("60", "Net 60"),
];
/// The term whose days match the gap between two ISO dates, if one does ("" = custom).
pub(in crate::screens) fn term_for(date: Option<&str>, due: Option<&str>) -> &'static str {
    let parse = |d: Option<&str>| d.and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
    match (parse(date), parse(due)) {
        (Some(a), Some(b)) => {
            let days = (b - a).num_days().to_string();
            TERMS.iter().find(|(v, _)| !v.is_empty() && *v == days).map(|(v, _)| *v).unwrap_or("")
        }
        _ => "",
    }
}
pub(in crate::screens) fn terms_valid(p: Posted<'_>) -> bool {
    TERMS.iter().any(|(v, _)| Some(*v) == Some(get(p, "terms").unwrap_or("")))
}
/// Terms with the initial choice shown when the form first opens.
pub(in crate::screens) fn terms_with(p: Option<Posted<'_>>, initial: Option<&str>) -> Field {
    Field::new(
        "terms",
        "Terms",
        Widget::Select {
            choices: TERMS.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect(),
        },
    )
    .value_opt(super::super::d2::data_value(p, "terms", initial.map(str::to_owned)))
}
fn rows(
    kind: KindForm,
    ch: &InvoiceChoices,
    prefix: &str,
    data: Option<&HashMap<String, String>>,
    e: &Errors,
    flags: (bool, bool),
) -> Vec<Field> {
    let mut fs = super::super::line_fields(prefix, ch, data, e, true);
    if kind == KindForm::Invoice {
        let value = data.and_then(|d| d.get(&format!("{prefix}-line_kind"))).cloned().unwrap_or("CHARGE".into());
        fs.push(Field::new("line_kind", "Line type", Widget::Select { choices: vec![
            ("CHARGE".into(), "Charge".into()), ("DISCOUNT".into(), "Discount / credit".into()), ("NOTE".into(), "Note (no amount)".into())
        ] }).prefix(prefix).value(value).errors(e.get(&format!("{prefix}-line_kind")).cloned().unwrap_or_default()));
        for field in &mut fs { if field.name == "description" {
            field.attrs.retain(|(k,_)| k != "maxlength");
            field.attrs.push(("maxlength".into(), "4096".into()));
        } }
        for field in &mut fs {
            match field.name.as_str() {
                "income_account" | "line_kind" => field.widget = Widget::Hidden,
                "refresh_item" => { field.widget = Widget::Hidden; field.value = data.and_then(|d| d.get(&format!("{prefix}-refresh_item"))).cloned(); }
                "keep_archived" => {
                    field.widget = Widget::Hidden;
                    field.value = Some(if data.is_some_and(|d| d.get(&format!("{prefix}-origin_position")).is_some_and(|v| !v.is_empty())) { "1" } else { "" }.into());
                }
                "item" => field.label = "Product / service".into(),
                "quantity" => field.label = "Qty".into(),
                "DELETE" => field.label = "Remove".into(),
                _ => {}
            }
        }
    }
    if kind == KindForm::Estimate {
        fs.retain(|f| f.name != "income_account");
        // Keep an archived item, or refresh an item the item list has since changed: shown only on
        // a saved line where that is the case. Elsewhere the core's default (no) applies.
        let (archived, changed) = flags;
        for field in &mut fs {
            let shown = match field.name.as_str() {
                "keep_archived" => {
                    field.label = "Keep this archived item on the line".into();
                    archived
                }
                "refresh_item" => {
                    field.label = "Use the item's current details from the item list".into();
                    changed && !archived
                }
                "item" => { field.label = "Product / service".into(); continue }
                "quantity" => { field.label = "Qty".into(); continue }
                "DELETE" => { field.label = "Remove".into(); continue }
                _ => continue,
            };
            if !shown {
                field.widget = Widget::Hidden;
                field.value = Some(String::new());
            }
        }
    }
    if kind == KindForm::Sale {
        fs.retain(|f| {
            !["origin_position", "refresh_item", "keep_archived"].contains(&f.name.as_str())
        });
        for f in &mut fs {
            match f.name.as_str() {
                "item" => f.label = "Product / service".into(),
                "quantity" => f.label = "Qty".into(),
                "DELETE" => f.label = "Remove".into(),
                _ => {}
            }
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
    formset_with(kind, ch, p, initial, e, non, &HashMap::new())
}
/// The line formset; `flags` holds, by saved line position, whether that line's item is archived
/// or changed in the item list (estimates only).
fn formset_with(
    kind: KindForm,
    ch: &InvoiceChoices,
    p: Option<Posted<'_>>,
    initial: &[Value],
    e: &Errors,
    non: &[String],
    flags: &HashMap<i64, (bool, bool)>,
) -> FormsetVm {
    let initial_len = initial.len();
    let total = p
        .map(|p| {
            get(p, "lines-TOTAL_FORMS")
                .and_then(clean_integer)
                .unwrap_or(0)
                .clamp(0, 50) as usize
        })
        // Every document starts with one line; Alt + Enter or Add line adds the next.
        .unwrap_or(initial_len.max(1));
    let mut data: HashMap<String, String> = p.unwrap_or(&[]).iter().cloned().collect();
    if p.is_none() {
        for (i, l) in initial.iter().enumerate() {
            for (to, from) in [
                ("origin_position", "position"),
                ("item", "item_id"),
                ("income_account", "income_account_id"),
                ("line_kind", "line_kind"),
                ("description", "description"),
                ("quantity", "quantity"),
                ("rate", "rate"),
                ("taxable", "taxable"),
            ] {
                let v = &l[from];
                if to == "quantity" && kind == KindForm::Invoice && let Some(q) = v.as_str().filter(|q| q.contains('.')) {
                    // Shown as typed: 5, not 5.0000.
                    data.insert(format!("lines-{i}-{to}"), q.trim_end_matches('0').trim_end_matches('.').to_owned());
                    continue;
                }
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
    let line_flags = |i: usize| {
        let prefix = format!("lines-{i}");
        let origin = data.get(&format!("{prefix}-origin_position")).and_then(|v| clean_integer(v));
        let stored = origin.and_then(|n| initial.iter().find(|l| l["position"].as_i64() == Some(n)));
        // Only while the line still holds the item it was saved with.
        match (origin, stored) {
            (Some(n), Some(l)) if l["item_id"].as_str().is_some_and(|id| data.get(&format!("{prefix}-item")).map(String::as_str) == Some(id)) => {
                flags.get(&n).copied().unwrap_or_default()
            }
            _ => (false, false),
        }
    };
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
                    line_flags(i),
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
            &rows(kind, ch, "lines-__prefix__", None, &Errors::new(), (false, false)),
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
    page_for(app, r, path, kind, root, p, e, non, None)
}
/// A new document started from a customer's page (`?customer=`) opens with that customer chosen.
#[allow(clippy::too_many_arguments)]
pub(in crate::screens) fn page_for(
    app: &mut App,
    r: &Route,
    path: &str,
    kind: KindForm,
    root: Option<&Value>,
    p: Option<Posted<'_>>,
    e: &Errors,
    non: &[String],
    preset: Option<&str>,
) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let mut vm = context(app, r)?;
    let current = root.map(|v| &v["current"]);
    let lines = root
        .and_then(|v| v["lines"].as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let (header, ch) = fields(app, c, kind, current, lines, p, e, preset)?;
    let (customers, rates) = editor_data(app, c, current)?;
    put(&mut vm, "invoice_customers", customers);
    put(&mut vm, "invoice_tax_rates", rates);
    put(&mut vm, "item_count", ch.catalog.as_object().unwrap().len());
    if kind == KindForm::Invoice {
        let (preferences,stripe_connected)=app.open_mut()?.book.read(|tx| -> Result<_,FbError> {
            let settings=fb_core::invoice_preferences::get(tx,c)?.settings;
            let connection=fb_core::invoice_stripe::current_connection(tx,c,&settings.stripe_mode)?;
            Ok((settings,connection.is_some_and(|v|v["status"]=="CONNECTED")))
        })?;
        put(&mut vm,"payment_preferences",preferences);put(&mut vm,"stripe_connected",stripe_connected);
    }
    put(&mut vm, "csrf_token", token(app, path));
    put(&mut vm, "form", forms::bind(&header, &nonfield(e, "")));
    let flags = if kind == KindForm::Estimate { item_flags(app, c, lines)? } else { HashMap::new() };
    put(&mut vm, "formset", formset_with(kind, &ch, p, lines, e, non, &flags));
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
/// What the line editor needs to show the bill-to address and calculate tax as the person types:
/// each customer's address and email, and the component rates of every active tax choice (and of
/// the tax already saved on a document, as "KEEP"), so tax rounds per component as the core does.
pub(in crate::screens) fn editor_data(app: &mut App, c: CompanyId, current: Option<&Value>) -> AppResult<(serde_json::Map<String, Value>, Value)> {
    Ok(app.open_mut()?.book.read(|tx| -> Result<_, FbError> {
        let mut customers = serde_json::Map::new();
        for customer in fb_core::directory::list(tx, c, fb_core::directory::Model::Customer)? {
            customers.insert(customer["id"].as_str().unwrap().into(), json!({"billing_address":customer["billing_address"], "email":customer["email"]}));
        }
        let mut rates = json!({"NONE":[]});
        for item in fb_core::salestax::list(tx, c)? {
            if item["active"] == true {
                let snapshot = fb_core::salestax::tax_snapshot(tx, c, &item)?;
                rates[format!("T{}", item["id"].as_str().unwrap())] = json!(snapshot["members"].as_array().unwrap().iter().map(|m| m["rate"].clone()).collect::<Vec<_>>());
            }
        }
        if let Some(h) = current {
            customers.insert(h["customer_id"].as_str().unwrap().into(), json!({"billing_address":h["customer_snapshot"]["billing_address"], "email":h["customer_snapshot"]["email"]}));
            rates["KEEP"] = if let Some(members) = h["tax_snapshot"]["members"].as_array() { json!(members.iter().map(|m| m["rate"].clone()).collect::<Vec<_>>()) } else { json!([h["tax_rate"]]) };
        }
        Ok((customers, rates))
    })?)
}
/// The account names a sales receipt ("SR") or credit memo ("CM") posts to, for the form's
/// "Show accounting entries" preview: the setup's undeposited funds, receivables and collected tax
/// accounts, and each income account a line can use.
pub(in crate::screens) fn posting_accounts(app: &mut App, c: CompanyId, kind: &str, ch: &InvoiceChoices) -> AppResult<Value> {
    let mut v = app.open_mut()?.book.read(|tx| -> Result<Value, FbError> {
        let setup = fb_core::documents::setup_snapshot(tx, c)?;
        let label = |role: &str| -> Result<Value, FbError> {
            Ok(match setup[role].as_str().and_then(AccountId::parse) {
                Some(id) => json!(fb_core::store::account(tx, id)?.map(|a| format!("{} · {}", a.code, a.name))),
                None => Value::Null,
            })
        };
        Ok(json!({"kind": kind, "undeposited": label("undeposited")?, "receivables": label("receivables")?, "collected_tax": label("collected_tax")?}))
    })?;
    v["income"] = json!(ch.accounts.iter().map(|(id, label)| (id.clone(), json!(label))).collect::<serde_json::Map<_, _>>());
    Ok(v)
}
/// A core refusal on a sales receipt or credit memo, said plainly; unknown messages pass through.
pub(in crate::screens) fn plain_sale(message: &str) -> String {
    match message {
        "Every line requires an active postable INCOME account." => "Each line needs an active income account. Choose one for every line.".into(),
        "Every posting account must be active and postable." => "One of the accounts this would post to is inactive. Reactivate it in the chart of accounts, or choose another.".into(),
        "Every selected item must be active before recording." => "An item on this form is archived. Reactivate it in Items, or choose another item.".into(),
        "Choose an active customer in this company." => "This customer is archived. Reactivate the customer before saving.".into(),
        "Configure an explicit undeposited funds destination first." => "Choose your undeposited funds account in Accounting setup first, or deposit this sale straight to a bank account.".into(),
        "Choose a designated bank account in Accounting setup." => "Choose a bank account. Bank accounts are listed in Accounting setup.".into(),
        "Do not select a bank for undeposited funds." => "Clear the bank account, or choose Deposit to: Bank account.".into(),
        "The destination must be an active postable ASSET account." => "The account this sale is deposited to must be an active bank or asset account.".into(),
        "Configure collected tax in Accounting setup before recording a taxed sale." | "Configure collected tax in Accounting setup before recording a taxed credit memo." => "Choose a sales tax payable account in Accounting setup before saving a taxed sale or credit.".into(),
        "Collected tax must be an active postable LIABILITY account." => "The sales tax payable account in Accounting setup must be an active liability account.".into(),
        "Configure a receivables (A/R) account in Accounting setup before recording a credit memo." => "Choose an accounts receivable account in Accounting setup before saving a credit memo.".into(),
        "Enter 1–50 meaningful lines." => "Enter between 1 and 50 lines.".into(),
        m => m.into(),
    }
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
        let line_kind = if kind == KindForm::Invoice { get(&local, "line_kind").unwrap_or("CHARGE") } else { "CHARGE" };
        let changed = i < stored_initial || line_kind != "CHARGE" || super::super::d2::extra_row_changed(
            &local, &[("description", ""), ("quantity", ""), ("rate", "")],
            &raw, &["taxable", "refresh_item", "keep_archived", "DELETE"],
        );
        if !changed && i >= init.unwrap_or(0).max(1) as usize {
            continue;
        }
        present += usize::from(changed && !deleted);
        let mut le = Errors::new();
        if !["CHARGE", "DISCOUNT", "NOTE"].contains(&line_kind) { error(&mut le, "line_kind", "Choose a line type."); }
        let description = if kind == KindForm::Invoice { invoice_text(&local, "description", 4096, true, &mut le) } else { clean_text(&local, "description", 1000, true, true, &mut le) };
        let mut decimal = |name: &str| {
            let v = clean_text(&local, name, 40, true, true, &mut le);
            if le.contains_key(name) {
                return v;
            }
            let result = if name == "quantity" && line_kind == "NOTE" {
                fb_core::calculations::unit_rate(&json!(v))
            } else if name == "quantity" {
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
            // A sales receipt or credit memo line with an item uses that item's income account,
            // as choosing the item on the form does; a line without one must name an account.
            let inferred = (kind == KindForm::Sale
                && get(&local, "income_account").unwrap_or("").is_empty()
                && !item.is_empty()
                && !le.contains_key("item"))
            .then(|| ch.catalog[item.as_str()]["income_account"].as_str())
            .flatten()
            .filter(|a| ch.accounts.iter().any(|(id, _)| id == a));
            match inferred {
                Some(a) => a.to_owned(),
                None => {
                    let v = choice("income_account", &ch.accounts, kind == KindForm::Sale, &mut le);
                    if kind == KindForm::Sale && let Some(m) = le.get_mut("income_account") {
                        for m in m.iter_mut().filter(|m| *m == "This field is required.") {
                            *m = if item.is_empty() { "Choose the income account for this line.".into() } else { "This item has no income account. Choose one for this line, or set one on the item.".into() };
                        }
                    }
                    v
                }
            }
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
            line_kind: (line_kind != "CHARGE").then(|| line_kind.to_owned()),
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
    let memo = if kind == KindForm::Invoice { invoice_text(p, "memo", 2000, false, &mut e) } else { clean_text(p, "memo", 2000, false, true, &mut e) };
    let tax = get(p, "tax_mode").unwrap_or("").to_owned();
    if !ch.tax.iter().any(|(id, _)| id == &tax) {
        error(
            &mut e,
            "tax_mode",
            if tax.is_empty() {"This field is required.".to_owned()}
            else {format!("Select a valid choice. {tax} is not one of the available choices.")},
        );
    }
    if kind == KindForm::Invoice && !terms_valid(p)
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
            Ok(Outcome::Done { data }) if kind == KindForm::Invoice && get(p, "editor_action") == Some("post") => {
                return save_and_post(app, c, &data, rev + 1);
            }
            Ok(Outcome::Done { data }) => {
                return Ok(Outcome::Redirect {
                    location: url(
                        if get(p,"editor_action")==Some("preview") {
                            if kind==KindForm::Estimate {"sales:print"} else {"invoices:print"}
                        } else if kind == KindForm::Estimate {"sales:detail"} else {"invoices:detail"},
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

/// "Save and post": the draft was just saved as `saved`; post exactly that revision. The core
/// reviews and posts it, refusing if the invoice changed after this save. A refusal leaves the
/// saved draft and explains why on its page.
fn save_and_post(app: &mut App, c: CompanyId, saved: &Value, fresh: i64) -> AppResult<Outcome> {
    let id = saved["id"].as_str().unwrap_or_default().to_owned();
    let detail = url("invoices:detail", &[&c.to_db(), &id]);
    if saved["current"]["state"] != "DRAFT" {
        return Ok(Outcome::Redirect { location: detail });
    }
    if saved["revision"].as_i64() != Some(fresh) {
        app.messages.push("Not posted: this draft was changed after that page was shown. Check it and choose Save and post again.".into());
        return Ok(Outcome::Redirect { location: detail });
    }
    match app.run(Command::PostInvoice {
        company_id: c.to_db(),
        invoice_id: id.clone(),
        expected_revision: saved["revision"].as_i64().unwrap_or(0),
    }) {
        Ok(_) => app.messages.push("Invoice saved and posted.".into()),
        Err(AppError::Core(FbError::Validation(messages))) => app.messages.push(format!(
            "Saved as a draft. Not posted. {}",
            messages.iter().map(|m| super::invoices::plain_post(m)).collect::<Vec<_>>().join(" ")
        )),
        Err(e) => return Err(e),
    }
    Ok(Outcome::Redirect { location: url("invoices:detail", &[&c.to_db(), &id]) })
}
