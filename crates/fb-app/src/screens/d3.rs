//! D3 ports of directory, salestax and reminders views/forms. Core services own every write.
use super::d2::{
    checked, clean_integer, clean_text, data_value, decorate, draw, error, nonfield, put, token,
};
use super::*;
use crate::{
    Command,
    commands::{DirectoryKind, DirectoryValues},
};
use fb_core::{
    directory::{self, Model},
    py, salestax,
};
use fb_db::{Query, rusqlite::params};
type Errors = HashMap<String, Vec<String>>;
const REQUIRED: &str = "This field is required.";
const MODEL_CHOICE: &str =
    "Select a valid choice. That choice is not one of the available choices.";
mod activity;
mod reminders;
mod tax;

pub(super) fn is_route(name: &str) -> bool {
    name.starts_with("directory:")
        || name.starts_with("salestax:")
        || name.starts_with("reminders:")
}
pub(crate) fn is_d3_command(c: &Command) -> bool {
    matches!(
        c,
        Command::SaveDirectoryRecord { .. }
            | Command::SetDirectoryActive { .. }
            | Command::SaveTaxRate { .. }
            | Command::SaveTaxGroup { .. }
            | Command::SaveTaxSettings { .. }
            | Command::SetTaxItemActive { .. }
            | Command::FinishTaxBulkStep { .. }
            | Command::SaveReminderSettings { .. }
    )
}
fn model(kind: DirectoryKind) -> Model {
    match kind {
        DirectoryKind::Customer => Model::Customer,
        DirectoryKind::Vendor => Model::Vendor,
        DirectoryKind::Item => Model::Item,
    }
}
pub(crate) fn run_d3_command(app: &mut App, cmd: Command) -> AppResult<Outcome> {
    let open = app.open_mut()?;
    let actor = open.owner;
    let book = &mut open.book;
    let c = |s: &str| crate::session::company(s);
    let data = match cmd {
        Command::SaveDirectoryRecord {
            company_id,
            record_id,
            values,
        } => {
            let mut data = serde_json::to_value(&values).expect("typed directory values");
            let m = match &values {
                DirectoryValues::Customer { .. } => Model::Customer,
                DirectoryValues::Vendor { .. } => Model::Vendor,
                DirectoryValues::Item { .. } => Model::Item,
            };
            let map = data.as_object_mut().unwrap();
            map.remove("kind");
            if m == Model::Item {
                let kind = map.remove("item_kind").unwrap();
                map.insert("kind".into(), kind);
                if map.get("taxable") == Some(&Value::Null) {
                    map.remove("taxable");
                }
            }
            if m == Model::Customer {
                for k in ["tax_status", "exemption_number"] {
                    if map.get(k) == Some(&Value::Null) {
                        map.remove(k);
                    }
                }
            }
            if let DirectoryValues::Vendor {
                expense_shown,
                terms_shown,
                ..
            } = values
            {
                map.remove("expense_shown");
                map.remove("terms_shown");
                if !expense_shown {
                    map.remove("default_expense_account");
                }
                if !terms_shown {
                    map.remove("default_terms");
                }
            }
            directory::save_record(book, actor, c(&company_id)?, m, &data, record_id.as_deref())?
        }
        Command::SetDirectoryActive {
            company_id,
            kind,
            record_id,
            active,
        } => directory::set_active(
            book,
            actor,
            c(&company_id)?,
            model(kind),
            &record_id,
            active,
        )?,
        Command::SaveTaxRate {
            company_id,
            item_id,
            expected_revision,
            name,
            description,
            rate,
            agency_id,
            new_agency_name,
        } => salestax::save_rate(
            book,
            actor,
            c(&company_id)?,
            &json!({"item_id":item_id,"expected_revision":expected_revision,"name":name,"description":description,"rate":rate,"agency_id":agency_id,"new_agency_name":new_agency_name}),
        )?,
        Command::SaveTaxGroup {
            company_id,
            item_id,
            expected_revision,
            name,
            description,
            member_ids,
        } => salestax::save_group(
            book,
            actor,
            c(&company_id)?,
            &json!({"item_id":item_id,"expected_revision":expected_revision,"name":name,"description":description,"member_ids":member_ids}),
        )?,
        Command::SaveTaxSettings {
            company_id,
            expected_revision,
            charge_tax,
            default_item_id,
            owed_basis,
            filing_frequency,
        } => {
            let (settings, offer) = salestax::save_settings(
                book,
                actor,
                c(&company_id)?,
                &json!({"expected_revision":expected_revision,"charge_tax":charge_tax,"default_item_id":default_item_id,"owed_basis":owed_basis,"filing_frequency":filing_frequency}),
            )?;
            json!({"settings":settings,"offer":offer})
        }
        Command::SetTaxItemActive {
            company_id,
            item_id,
            active,
            expected_revision,
        } => salestax::set_item_active(
            book,
            actor,
            c(&company_id)?,
            &item_id,
            active,
            Some(expected_revision),
        )?,
        Command::FinishTaxBulkStep {
            company_id,
            customers,
            items,
            digest,
        } => salestax::finish_bulk_step(book, actor, c(&company_id)?, customers, items, &digest)?,
        Command::SaveReminderSettings {
            company_id,
            days,
            expected,
        } => {
            let (co, changed) = fb_core::services::set_bill_reminder_days(
                book,
                actor,
                c(&company_id)?,
                days,
                Some(expected),
            )?;
            json!({"days":co.bill_reminder_days,"changed":changed})
        }
        _ => return Err(AppError::Invalid("Not a D3 command.".into())),
    };
    Ok(Outcome::Done { data })
}
fn ctx(app: &mut App, route: &Route) -> AppResult<Value> {
    let c = arg_company(route)?;
    Ok(serde_json::to_value(shell(
        app,
        Some(c),
        &Current {
            route: &route.name,
            kind: route.args.get("kind").map(String::as_str),
        },
    )?)
    .expect("shell"))
}
fn kind(route: &Route) -> AppResult<(Model, &'static str, &'static str)> {
    match route.args.get("kind").map(String::as_str) {
        Some("customers") => Ok((Model::Customer, "Customers", "customer")),
        Some("vendors") => Ok((Model::Vendor, "Vendors", "vendor")),
        Some("items") => Ok((Model::Item, "Items & services", "item")),
        _ => Err(AppError::NotFound("list".into())),
    }
}
fn record(app: &mut App, route: &Route) -> AppResult<Option<Value>> {
    let Some(id) = route.args.get("record_id") else {
        return Ok(None);
    };
    let c = arg_company(route)?;
    let (m, _, _) = kind(route)?;
    let r = app
        .open_mut()?
        .book
        .read(|tx| directory::get(tx, c, m, id))?;
    Ok(Some(r))
}
fn record_vm(mut r: Value, m: Model) -> Value {
    r["pk"] = r["id"].clone();
    r["label"] = if m == Model::Item {
        r["code"].clone()
    } else {
        r["display_name"].clone()
    };
    r["get_kind_display"] = json!(if r["kind"] == "SERVICE" {
        "Service"
    } else {
        "Non-stock"
    });
    r
}
fn context(app: &mut App, route: &Route, r: Option<&Value>) -> AppResult<Value> {
    let (m, title, singular) = kind(route)?;
    let mut vm = ctx(app, route)?;
    put(&mut vm, "kind", &route.args["kind"]);
    put(&mut vm, "title", title);
    put(&mut vm, "singular", singular);
    put(&mut vm, "is_item", m == Model::Item);
    put(&mut vm, "record", r.cloned().map(|r| record_vm(r, m)));
    Ok(vm)
}
fn choices(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}
fn choice_clean(
    fields: Posted<'_>,
    name: &str,
    opts: &[(String, String)],
    required: bool,
    errors: &mut Errors,
) -> String {
    let value = get(fields, name).unwrap_or("").to_owned();
    if value.is_empty() && required {
        error(errors, name, REQUIRED);
    } else if !value.is_empty() && !opts.iter().any(|(v, _)| v == &value) {
        error(
            errors,
            name,
            format!("Select a valid choice. {value} is not one of the available choices."),
        );
    }
    value
}
fn integer_clean(
    fields: Posted<'_>,
    name: &str,
    min: i64,
    max: Option<i64>,
    required: bool,
    errors: &mut Errors,
) -> Option<i64> {
    let s = get(fields, name).unwrap_or("");
    if py::py_strip(s).is_empty() && !required {
        return None;
    }
    match clean_integer(s) {
        Some(n) if n < min => {
            error(
                errors,
                name,
                format!("Ensure this value is greater than or equal to {min}."),
            );
            None
        }
        Some(n) if max.is_some_and(|m| n > m) => {
            error(
                errors,
                name,
                format!(
                    "Ensure this value is less than or equal to {}.",
                    max.unwrap()
                ),
            );
            None
        }
        Some(n) => Some(n),
        None => {
            error(
                errors,
                name,
                if py::py_strip(s).is_empty() {
                    REQUIRED
                } else {
                    "Enter a whole number."
                },
            );
            None
        }
    }
}
fn labels(tx: &(impl Query + ?Sized), c: CompanyId, m: Model) -> Result<Value, FbError> {
    let mut labels = fb_core::party_display::contact_label_map(tx, c, m, None)?;
    let ordinals = d2::ordinals(tx, c, &m.table())?;
    for (id, v) in labels.as_object_mut().unwrap() {
        if let Some(s) = v.as_str() {
            *v = json!(short_contact_label(s.into(), id, ordinals[id]));
        }
    }
    Ok(labels)
}
pub(super) fn render(
    app: &mut App,
    route: &Route,
    params: Posted<'_>,
    path: &str,
) -> AppResult<(String, u16)> {
    match route.name.as_str() {
        "directory:list" => listing(app, route, params),
        "directory:detail" => Ok((detail(app, route, params)?, 200)),
        "directory:new" | "directory:edit" => {
            Ok((form(app, route, params, path, None, &Errors::new())?, 200))
        }
        name if name.starts_with("salestax:") => tax::render(app, route, params, path),
        "reminders:page" => reminders::render(app, route, None, &Errors::new(), None, 200),
        _ => Err(AppError::NotFound(path.into())),
    }
}
pub(super) fn submit(
    app: &mut App,
    route: &Route,
    path: &str,
    fields: Posted<'_>,
) -> AppResult<Outcome> {
    match route.name.as_str() {
        "directory:new" | "directory:edit" => submit_record(app, route, path, fields),
        "directory:status" => submit_status(app, route, fields),
        name if name.starts_with("salestax:") => tax::submit(app, route, path, fields),
        "reminders:settings" => reminders::submit(app, route, fields),
        _ => Err(AppError::NotFound(path.into())),
    }
}
fn listing(app: &mut App, route: &Route, params: Posted<'_>) -> AppResult<(String, u16)> {
    let c = arg_company(route)?;
    let (m, _, _) = kind(route)?;
    let mut vm = context(app, route, None)?;
    let mut errors = Errors::new();
    let q = clean_text(params, "q", 200, false, true, &mut errors);
    let status = get(params, "status").unwrap_or("active");
    let opts = choices(&[
        ("active", "Active only"),
        ("all", "All (including inactive)"),
        ("inactive", "Inactive only"),
    ]);
    if !opts.iter().any(|(v, _)| v == status) {
        error(
            &mut errors,
            "status",
            if status.is_empty() {
                REQUIRED.into()
            } else {
                format!("Select a valid choice. {status} is not one of the available choices.")
            },
        );
    }
    let open = app.open_mut()?;
    let mut rows = open.book.read(|tx| -> Result<Vec<Value>, FbError> {
        let labels = if m == Model::Item {
            json!({})
        } else {
            labels(tx, c, m)?
        };
        let all = directory::list(tx, c, m)?;
        let query = fb_db::collation::upper(&q);
        Ok(all
            .into_iter()
            .filter(|r| {
                errors.is_empty()
                    && (status == "all" || r["active"] == (status == "active"))
                    && (query.is_empty()
                        || m.fields()
                            .iter()
                            .filter(|f| {
                                if m == Model::Item {
                                    ["code", "description"].contains(f)
                                } else {
                                    ![
                                        "tax_status",
                                        "exemption_number",
                                        "default_expense_account",
                                        "default_terms",
                                    ]
                                    .contains(f)
                                }
                            })
                            .any(|f| {
                                fb_db::collation::upper(r[*f].as_str().unwrap_or(""))
                                    .contains(&query)
                            }))
            })
            .map(|r| {
                let mut vm = record_vm(r, m);
                if m != Model::Item {
                    vm["party_label"] = labels[vm["id"].as_str().unwrap()].clone();
                    vm["town"] = json!(fb_core::party_display::town_from_address(
                        vm["billing_address"].as_str().unwrap_or("")
                    ));
                }
                vm
            })
            .collect())
    })?;
    // directory::list already uses the frozen model's name/code + creation ordering.
    let page = Page::of(&rows, 25, get(params, "page"));
    rows.clear();
    put(&mut vm, "page", page);
    let f = vec![
        Field::new("q", "Search", Widget::Text)
            .attr("maxlength", "200")
            .value(get(params, "q").unwrap_or("")),
        Field::new("status", "Show", Widget::Select { choices: opts })
            .required()
            .value(status),
    ];
    put(
        &mut vm,
        "filter_form",
        forms::bind(&decorate(f, &errors), &[]),
    );
    put(
        &mut vm,
        "query_string",
        if errors.is_empty() {
            urlencode(&[("q".into(), q), ("status".into(), status.into())])
        } else {
            String::new()
        },
    );
    Ok((
        draw(app, "directory/list.html", &vm)?,
        if errors.is_empty() { 200 } else { 400 },
    ))
}
fn contact_fields() -> Vec<Field> {
    let mut fields = vec![];
    for (name, label, max) in [
        ("display_name", "Display name", 120),
        ("business_name", "Business name", 120),
        ("contact_name", "Contact name", 120),
        ("email", "Email", 254),
        ("phone", "Phone", 50),
        ("billing_address", "Billing address", 1000),
        ("notes", "Notes", 4000),
    ] {
        let w = if ["billing_address", "notes"].contains(&name) {
            Widget::Textarea { cols: 40, rows: 3 }
        } else if name == "email" {
            Widget::Email
        } else {
            Widget::Text
        };
        let mut f = Field::new(name, label, w).attr("maxlength", max.to_string());
        if name == "display_name" {
            // SIM-FIX item 4 (deliberate divergence): v1 says "use town or email", but there is no Town
            // field. Lists take a town only from line 2 of a billing address (party_display), and new
            // text refuses line breaks (SEC-FIX1), so desktop records are told apart by email, then number.
            f=f.required().help("Required. Two customers or vendors may share a name; lists tell them apart by email, or else by a number.");
        }
        fields.push(f);
    }
    fields
}
fn account_options(
    app: &mut App,
    c: CompanyId,
    m: Model,
    name: &str,
) -> AppResult<Vec<(String, String)>> {
    Ok(app.open_mut()?.book.read(|tx| -> Result<Vec<_>, FbError> {
        let mut rows = fb_core::store::accounts(tx, c)?;
        rows.sort_by(|a, b| a.code.cmp(&b.code).then(a.id.to_db().cmp(&b.id.to_db())));

        let mut out = vec![];
        for a in rows {
            let eligible = if m == Model::Vendor {
                directory::purchase_line_allowed(&a)
                    && fb_core::documents::control_role(tx, c, a.id)?.is_none()
            } else {
                a.active
                    && a.postable
                    && a.kind
                        == if name == "income_account" {
                            Kind::Income
                        } else {
                            Kind::Expense
                        }
            };
            if eligible {
                out.push((a.id.to_db(), a.label()));
            }
        }
        Ok(out)
    })?)
}
fn fields_for(app: &mut App, c: CompanyId, m: Model) -> AppResult<Vec<Field>> {
    let mut f = if m == Model::Item {
        vec![Field::new("code","Item code / name",Widget::Text).required().attr("maxlength","120").help("Required; unique within this company, ignoring case and outer spaces, including archived items."),
        Field::new("description","Description",Widget::Textarea{cols:40,rows:3}).attr("maxlength","2000"),
        Field::new("kind","Type",Widget::Select{choices:choices(&[("SERVICE","Service"),("NONSTOCK","Non-stock")])}).required(),
        Field::new("sale_rate","Default sale rate",Widget::Text).required().attr("inputmode","decimal").attr("maxlength","100").help("Company currency; nonnegative, up to 99999999999999.99. Rounded half-up to 2 places; zero is allowed."),
        Field::new("purchase_cost","Default purchase cost",Widget::Text).attr("inputmode","decimal").attr("maxlength","100").help("Optional; same nonnegative limit and rounding as sale rate. Blank means no default, not zero.")]
    } else {
        contact_fields()
    };
    if m == Model::Customer {
        f.push(
            Field::new(
                "tax_status",
                "Sales tax",
                Widget::Select {
                    choices: choices(&[("TAXABLE", "Taxable"), ("EXEMPT", "Exempt")]),
                },
            )
            .value("TAXABLE")
            .help("Exempt: new sales lines start as not taxable. Each line can still be changed."),
        );
        f.push(
            Field::new(
                "exemption_number",
                "Exemption / resale certificate number",
                Widget::Text,
            )
            .attr("maxlength", "40"),
        );
    }
    if m == Model::Item || m == Model::Vendor {
        let names = if m == Model::Item {
            vec!["income_account", "expense_account"]
        } else {
            vec!["default_expense_account"]
        };
        for name in names {
            let opts = account_options(app, c, m, name)?;
            let widget = if m == Model::Vendor {
                let accounts = app
                    .open_mut()?
                    .book
                    .read(|tx| fb_core::store::accounts(tx, c))?;
                let mut groups = vec![(String::new(), vec![(String::new(), "No default".into())])];
                let mut accounts = accounts;
                accounts.sort_by(|a, b| {
                    a.type_order()
                        .cmp(&b.type_order())
                        .then(a.code.cmp(&b.code))
                        .then(a.id.to_db().cmp(&b.id.to_db()))
                });
                for a in accounts {
                    if opts.iter().any(|(id, _)| id == &a.id.to_db()) {
                        let label = a.detail_type.label();
                        if groups.last().is_none_or(|(s, _)| s != label) {
                            groups.push((label.into(), vec![]));
                        }
                        groups.last_mut().unwrap().1.push((a.id.to_db(), a.label()));
                    }
                }
                Widget::GroupedSelect { groups }
            } else {
                let mut choices = vec![(String::new(), "No default".into())];
                choices.extend(opts);
                Widget::Select { choices }
            };
            let (label, help) = match name {
                "income_account" => (
                    "Income account",
                    "Optional active posting account in this company. Defaults do not post transactions.",
                ),
                "expense_account" => (
                    "Expense account",
                    "Optional active posting account in this company. Defaults do not post transactions.",
                ),
                _ => (
                    "Default expense account",
                    "Fills the first blank expense line when you pick this vendor on a new bill or check. You can change it.",
                ),
            };
            f.push(Field::new(name, label, widget).help(help));
        }
        if m == Model::Vendor {
            f.push(Field::new("default_terms","Default payment terms",Widget::Select{choices:choices(&[("","No default"),("0","Due on receipt"),("15","Net 15"),("30","Net 30"),("60","Net 60")])}).help("Fills the due date on a new bill (bill date plus these terms). You can change it."));
        } else {
            f.push(Field::new("taxable","Taxable",Widget::Checkbox{checked:false}).help("Default tax flag for new sales lines. New services start unticked, non-stock items ticked."));
            f.push(Field::new("taxable_shown", "Taxable shown", Widget::Hidden).value("1"));
        }
    }
    Ok(f)
}
fn string(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::to_owned)
        .or_else(|| v.as_i64().map(|n| n.to_string()))
}
fn form(
    app: &mut App,
    route: &Route,
    params: Posted<'_>,
    path: &str,
    posted: Option<Posted<'_>>,
    errors: &Errors,
) -> AppResult<String> {
    let c = arg_company(route)?;
    let (m, _, _) = kind(route)?;
    let r = record(app, route)?;
    let mut vm = context(app, route, r.as_ref())?;
    let mut fs = fields_for(app, c, m)?;
    let mut stale = vec![];
    for f in &mut fs {
        let initial = r
            .as_ref()
            .and_then(|r| string(&r[&f.name]))
            .or_else(|| f.value.clone());
        let initial = if r.is_none()
            && posted.is_none()
            && ["display_name", "billing_address"].contains(&f.name.as_str())
        {
            get(params, &f.name)
                .map(|s| {
                    if f.name == "display_name" {
                        s.split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ")
                            .chars()
                            .take(120)
                            .collect()
                    } else {
                        py::py_strip(s).chars().take(1000).collect()
                    }
                })
                .or(initial)
        } else {
            initial
        };
        if let Some(row) = &r
            && f.name.ends_with("_account")
            && let Some(raw) = row[&f.name].as_str()
        {
            let opts = account_options(app, c, m, &f.name)?;
            if !opts.iter().any(|(id, _)| id == raw) {
                let a = app
                    .open_mut()?
                    .book
                    .read(|tx| fb_core::store::account(tx, AccountId::parse(raw).unwrap()))?
                    .unwrap();
                stale.push(format!("{}: {}. This default is no longer eligible; choose a replacement or clear it before saving/reactivating.",f.name.replace('_'," ").chars().enumerate().map(|(i,c)|if i==0{c.to_uppercase().next().unwrap()}else{c}).collect::<String>(),a.label()));
                let mut choices = vec![
                    (String::new(), "Clear default".into()),
                    (
                        raw.into(),
                        format!("{} (unavailable — replace or clear)", a.label()),
                    ),
                ];
                if let Widget::GroupedSelect { groups } = &f.widget {
                    let mut grouped = vec![(String::new(), choices)];
                    grouped.extend(groups.iter().skip(1).cloned());
                    f.widget = Widget::GroupedSelect { groups: grouped };
                } else {
                    choices.extend(opts);
                    f.widget = Widget::Select { choices };
                }
            }
        }
        f.value = data_value(posted, &f.name, initial);
        if matches!(f.widget, Widget::Checkbox { .. }) {
            f.widget = Widget::Checkbox {
                checked: posted
                    .map(|p| checked(p, &f.name))
                    .unwrap_or_else(|| r.as_ref().is_some_and(|r| r[&f.name] == true)),
            };
        }
    }
    put(&mut vm, "stale", stale);
    put(&mut vm, "csrf_token", token(app, path));
    put(
        &mut vm,
        "form",
        forms::bind(&decorate(fs, errors), &nonfield(errors, "__all__")),
    );
    draw(app, "directory/form.html", &vm)
}
fn submit_record(app: &mut App, route: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(route)?;
    let (m, _, _) = kind(route)?;
    let _ = record(app, route)?;
    let fs = fields_for(app, c, m)?;
    let mut errors = Errors::new();
    let mut data = json!({});
    for f in fs
        .iter()
        .filter(|f| !matches!(f.widget, Widget::Checkbox { .. }))
    {
        if f.name == "taxable_shown" {
            continue;
        }
        let v = match &f.widget {
            Widget::Select { choices } => {
                let raw = get(p, &f.name).unwrap_or("");
                let is_model = f.name.ends_with("_account");
                if is_model && !raw.is_empty() && !choices.iter().any(|(id, _)| id == raw) {
                    error(&mut errors, &f.name, MODEL_CHOICE);
                } else if !is_model {
                    choice_clean(p, &f.name, choices, f.required, &mut errors);
                }
                raw.to_owned()
            }
            Widget::GroupedSelect { groups } => {
                let raw = get(p, &f.name).unwrap_or("");
                if !raw.is_empty()
                    && !groups
                        .iter()
                        .any(|(_, rows)| rows.iter().any(|(id, _)| id == raw))
                {
                    error(&mut errors, &f.name, MODEL_CHOICE);
                }
                raw.into()
            }
            Widget::Email => {
                let value = py::py_strip(get(p, &f.name).unwrap_or(""));
                if !value.is_empty() && !fb_core::email_validation::valid_email(value) {
                    error(&mut errors, "email", "Enter a valid email address.");
                }
                if value.chars().count() > 254 {
                    error(&mut errors, "email", py::max_length_message(254, value));
                }
                if let Err(e) = fb_core::py::clean_free_text(get(p, &f.name).unwrap_or("")) {
                    error(&mut errors, "email", e.message());
                }
                value.into()
            }
            Widget::Textarea { .. } if f.name == "notes" => {
                let raw = get(p, "notes").unwrap_or("");
                if let Err(e) = py::clean_multiline_free_text(raw) {
                    error(&mut errors, "notes", e.message());
                }
                let value = py::py_strip(raw);
                if value.chars().count() > 4000 {
                    error(&mut errors, "notes", py::max_length_message(4000, value));
                }
                value.into()
            }
            _ => {
                let max = f
                    .attrs
                    .iter()
                    .find(|(k, _)| k == "maxlength")
                    .and_then(|(_, v)| v.parse().ok())
                    .unwrap_or(usize::MAX);
                clean_text(p, &f.name, max, f.required, true, &mut errors)
            }
        };
        if ["sale_rate", "purchase_cost"].contains(&f.name.as_str())
            && !v.is_empty()
            && !errors.contains_key(&f.name)
            && fb_core::services::money_json(&json!(v)).is_err()
        {
            error(&mut errors, &f.name, "Enter a valid monetary amount.");
        }
        data[&f.name] = json!(v);
    }
    // Django's per-field validation precedes form.clean. Tax compatibility markers distinguish
    // a deliberately cleared value from a page opened before these fields existed.
    if m == Model::Customer
        && get(p, "tax_status").is_some()
        && data["tax_status"] == ""
        && !errors.contains_key("tax_status")
    {
        error(&mut errors, "tax_status", "Choose Taxable or Exempt.");
    }
    if !errors.is_empty() {
        return Ok(Outcome::Invalid {
            html: form(app, route, &[], path, Some(p), &errors)?,
        });
    }
    let s = |k: &str| data[k].as_str().unwrap_or("").to_owned();
    let optional = |k: &str| {
        data[k]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let values = match m {
        Model::Customer => DirectoryValues::Customer {
            display_name: s("display_name"),
            business_name: s("business_name"),
            contact_name: s("contact_name"),
            email: s("email"),
            phone: s("phone"),
            billing_address: s("billing_address"),
            notes: s("notes"),
            tax_status: get(p, "tax_status").map(|_| s("tax_status")),
            exemption_number: get(p, "exemption_number").map(|_| s("exemption_number")),
        },
        Model::Vendor => DirectoryValues::Vendor {
            display_name: s("display_name"),
            business_name: s("business_name"),
            contact_name: s("contact_name"),
            email: s("email"),
            phone: s("phone"),
            billing_address: s("billing_address"),
            notes: s("notes"),
            default_expense_account: optional("default_expense_account"),
            default_terms: optional("default_terms"),
            expense_shown: get(p, "default_expense_account").is_some(),
            terms_shown: get(p, "default_terms").is_some(),
        },
        Model::Item => DirectoryValues::Item {
            code: s("code"),
            description: s("description"),
            item_kind: s("kind"),
            sale_rate: s("sale_rate"),
            purchase_cost: optional("purchase_cost"),
            income_account: optional("income_account"),
            expense_account: optional("expense_account"),
            taxable: if get(p, "taxable").is_some()
                || get(p, "taxable_shown").is_some_and(|s| !py::py_strip(s).is_empty())
            {
                Some(checked(p, "taxable"))
            } else {
                None
            },
        },
    };
    match app.run(Command::SaveDirectoryRecord {
        company_id: c.to_db(),
        record_id: route.args.get("record_id").cloned(),
        values,
    }) {
        Ok(Outcome::Done { data }) => Ok(Outcome::Redirect {
            location: url(
                "directory:detail",
                &[
                    &c.to_db(),
                    &route.args["kind"],
                    data["id"].as_str().unwrap(),
                ],
            ),
        }),
        Err(AppError::Core(FbError::Validation(e))) => {
            errors.insert("__all__".into(), e);
            Ok(Outcome::Invalid {
                html: form(app, route, &[], path, Some(p), &errors)?,
            })
        }
        Err(e) => Err(e),
        _ => unreachable!(),
    }
}
fn submit_status(app: &mut App, route: &Route, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(route)?;
    let (m, _, _) = kind(route)?;
    let r = record(app, route)?.unwrap();
    let errors = if ![Some("0"), Some("1")].contains(&get(p, "active")) {
        vec!["Choose archive or reactivate.".into()]
    } else {
        match app.run(Command::SetDirectoryActive {
            company_id: c.to_db(),
            kind: match m {
                Model::Customer => DirectoryKind::Customer,
                Model::Vendor => DirectoryKind::Vendor,
                Model::Item => DirectoryKind::Item,
            },
            record_id: route.args["record_id"].clone(),
            active: get(p, "active") == Some("1"),
        }) {
            Ok(_) => {
                return Ok(Outcome::Redirect {
                    location: url(
                        "directory:detail",
                        &[&c.to_db(), &route.args["kind"], &route.args["record_id"]],
                    ),
                });
            }
            Err(AppError::Core(FbError::Validation(e))) => e,
            Err(e) => return Err(e),
        }
    };
    let mut vm = context(app, route, Some(&r))?;
    put(&mut vm, "errors", errors);
    Ok(Outcome::Invalid {
        html: draw(app, "directory/status_error.html", &vm)?,
    })
}
fn detail(app: &mut App, route: &Route, p: Posted<'_>) -> AppResult<String> {
    let c = arg_company(route)?;
    let (m, _, _) = kind(route)?;
    let r = record(app, route)?.unwrap();
    let mut vm = context(app, route, Some(&r))?;
    let fs = fields_for(app, c, m)?;
    let mut fields = vec![];
    for name in m.fields() {
        let f = fs.iter().find(|f| f.name == name).unwrap();
        let mut v = r[name].clone();
        if name.ends_with("_account")
            && let Some(id) = v.as_str()
        {
            v = app
                .open_mut()?
                .book
                .read(|tx| fb_core::store::account(tx, AccountId::parse(id).unwrap()))?
                .map(|a| json!(a.label()))
                .unwrap_or(Value::Null);
        } else if name == "kind" {
            v = json!(if v == "SERVICE" {
                "Service"
            } else {
                "Non-stock"
            });
        } else if name == "tax_status" {
            v = json!(if v == "EXEMPT" { "Exempt" } else { "Taxable" });
        } else if name == "default_terms"
            && let Widget::Select { choices } = &f.widget
        {
            v = choices
                .iter()
                .find(|(id, _)| Some(id.as_str()) == v.as_str())
                .map(|(_, label)| json!(label))
                .unwrap_or(Value::Null);
        } else if let Some(b) = v.as_bool() {
            v = json!(if b { "Yes" } else { "No" });
        }
        let label = match name {
            "code" => "Item code / name",
            "kind" => "Type",
            "sale_rate" => "Default sale rate",
            "purchase_cost" => "Default purchase cost",
            "tax_status" => "Sales tax",
            "exemption_number" => "Exemption / resale certificate number",
            "default_terms" => "Default payment terms",
            _ => f.label.as_str(),
        };
        fields.push(json!([label, v]));
    }
    let target = format!("{}:{}", m.name(), r["id"].as_str().unwrap());
    let events=app.open_mut()?.book.read(|tx|->Result<Vec<Value>,FbError>{let mut st=tx.prepare("SELECT occurred_at,actor_name,action FROM ledger_auditevent WHERE company_id=?1 AND target=?2 ORDER BY occurred_at DESC,id DESC LIMIT 20")?;Ok(st.query_map(params![c.to_db(),target],|r|Ok(json!({"occurred_at":r.get::<_,String>(0)?,"actor_name":r.get::<_,String>(1)?,"action":r.get::<_,String>(2)?})))?.collect::<Result<_,_>>()?)})?;
    put(&mut vm, "fields", fields);
    put(&mut vm, "events", events);
    put(
        &mut vm,
        "csrf_token",
        token(
            app,
            &url(
                "directory:status",
                &[&c.to_db(), &route.args["kind"], &route.args["record_id"]],
            ),
        ),
    );
    activity::build(app, c, m, r["id"].as_str().unwrap(), p, &mut vm)?;
    draw(app, "directory/detail.html", &vm)
}
