//! D2: literal ports of ledger.views, ledger.forms and documents.views/forms.
use super::*;
use crate::Command;
use crate::commands::JournalLineValues;
use fb_core::{
    JournalId,
    model::{Journal, Status},
    py, store,
};
use fb_db::{
    Query,
    rusqlite::{OptionalExtension, params},
};

type Errors = HashMap<String, Vec<String>>;
mod company_documents;
const REQUIRED: &str = "This field is required.";
const CHOICE: &str = "Select a valid choice. That choice is not one of the available choices.";

pub(super) fn ordinals(
    tx: &(impl Query + ?Sized),
    c: CompanyId,
    table: &str,
) -> Result<HashMap<String, usize>, FbError> {
    assert!(
        [
            "ledger_account",
            "directory_item",
            "directory_customer",
            "directory_vendor"
        ]
        .contains(&table)
    );
    let mut st = tx.prepare(&format!(
        "SELECT id FROM {table} WHERE company_id=?1 ORDER BY id"
    ))?;
    Ok(st
        .query_map([c.to_db()], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .enumerate()
        .map(|(n, id)| (id, n + 1))
        .collect())
}

fn ctx(app: &mut App, route: &Route) -> AppResult<Value> {
    let c = if route.args.contains_key("company_id") {
        Some(arg_company(route)?)
    } else {
        None
    };
    serde_json::to_value(shell(
        app,
        c,
        &Current {
            route: &route.name,
            kind: None,
        },
    )?)
    .map_err(|e| AppError::Invalid(e.to_string()))
}
pub(super) fn put(vm: &mut Value, name: &str, value: impl serde::Serialize) {
    vm[name] = serde_json::to_value(value).expect("view model");
}
pub(super) fn draw(app: &App, template: &str, vm: &Value) -> AppResult<String> {
    Ok(app.screens.render(template, vm)?)
}
pub(super) fn token(app: &mut App, path: &str) -> String {
    format!(
        r#"<input type="hidden" name="fb_command_token" value="{}">"#,
        app.tokens.mint(path)
    )
}
pub(super) fn data_value(posted: Option<Posted<'_>>, name: &str, initial: Option<String>) -> Option<String> {
    match posted {
        Some(p) => get(p, name).map(str::to_owned),
        None => initial,
    }
}
// Django cleans every deleted form for display, then excludes its errors from
// formset validity and total_error_count. Keep those two uses explicit.
pub(in crate::screens) fn deleted_row_error(p: Posted<'_>, name: &str) -> bool {
    let mut parts = name.splitn(3, '-');
    let prefix = parts.next().unwrap_or("");
    let index = parts.next().unwrap_or("");
    ["lines", "allocations"].contains(&prefix)
        && parts.next().is_some() && index.parse::<usize>().is_ok()
        && checked(p, &format!("{prefix}-{index}-DELETE"))
}
pub(in crate::screens) fn active_form_errors(e: &Errors, p: Posted<'_>) -> usize {
    e.keys().filter(|name| !deleted_row_error(p, name)).count()
}
pub(in crate::screens) fn formset_error_count(e: &Errors, p: Option<Posted<'_>>, prefix: &str) -> usize {
    e.keys().filter(|name| name.starts_with(&format!("{prefix}-")) && !p.is_some_and(|p| deleted_row_error(p, name))).count()
}

pub(super) fn error(errors: &mut Errors, key: &str, message: impl Into<String>) {
    errors.entry(key.into()).or_default().push(message.into());
}
pub(super) fn decorate(fields: Vec<Field>, errors: &Errors) -> Vec<Field> {
    fields
        .into_iter()
        .map(|f| {
            let key = f.html_name();
            f.errors(errors.get(&key).cloned().unwrap_or_default())
        })
        .collect()
}
pub(super) fn nonfield(errors: &Errors, prefix: &str) -> Vec<String> {
    errors.get(prefix).cloned().unwrap_or_default()
}
pub(super) fn clean_text(
    fields: Posted<'_>,
    name: &str,
    max: usize,
    required: bool,
    strip: bool,
    errors: &mut Errors,
) -> String {
    let value = get(fields, name).unwrap_or("");
    let value = if strip { py::py_strip(value) } else { value };
    if value.is_empty() && required {
        error(errors, name, REQUIRED);
    }
    if value.chars().count() > max {
        error(errors, name, py::max_length_message(max, value));
    }
    let free_text = matches!(name.rsplit('-').next().unwrap_or(name),
        "name" | "code" | "display_name" | "business_name" | "contact_name" |
        "email" | "phone" | "billing_address" | "notes" | "description" | "memo" |
        "reference" | "reentry_reason" | "reason" | "payee_name" | "address" |
        "tax_label" | "order_reference");
    if free_text {
        if let Err(e) = fb_core::py::clean_free_text(get(fields, name).unwrap_or("")) {
            error(errors, name, e.message());
        }
    } else if value.contains('\0') {
        error(errors, name, "Null characters are not allowed.");
    }
    value.to_owned()
}
/// A card number in a payment document's reference or memo is refused on the field (a v2 rule; v1 only
/// asks in help text). Runs after the field's own clean, and only when that left no error.
pub(super) fn refuse_card_number(fields: Posted<'_>, name: &str, errors: &mut Errors) {
    if !errors.contains_key(name) && fb_core::card_numbers::has_card_number(get(fields, name).unwrap_or("")) {
        error(errors, name, fb_core::card_numbers::CARD_NUMBER);
    }
}
pub(super) fn clean_date(
    fields: Posted<'_>,
    name: &str,
    required: bool,
    errors: &mut Errors,
) -> Option<String> {
    let value = get(fields, name).unwrap_or("");
    if !required && py::py_strip(value).is_empty() {
        return None;
    }
    match py::form_date(value) {
        Ok(d) => Some(d.to_string()),
        Err(e) => {
            error(errors, name, e);
            None
        }
    }
}
pub(super) fn clean_integer(value: &str) -> Option<i64> {
    let value = py::py_strip(value);
    let value = if let Some((left, right)) = value.rsplit_once('.') {
        if right.chars().all(|c| c == '0') {
            left
        } else {
            value
        }
    } else {
        value
    };
    forms::python_integer(value)
}
pub(super) fn clean_revision(fields: Posted<'_>, errors: &mut Errors) -> Option<i64> {
    let v = get(fields, "expected_revision").unwrap_or("");
    match clean_integer(v) {
        Some(n) if n >= 0 => Some(n),
        Some(_) => {
            error(
                errors,
                "expected_revision",
                "Ensure this value is greater than or equal to 0.",
            );
            None
        }
        None => {
            error(
                errors,
                "expected_revision",
                if py::py_strip(v).is_empty() {
                    REQUIRED
                } else {
                    "Enter a whole number."
                },
            );
            None
        }
    }
}
pub(super) fn management_html(prefix: &str, fields: &[(&str, String)], bound: bool) -> String {
    let mut errors = vec![];
    let hidden = fields.iter().map(|(name, value)| {
        if bound {
            let message = if value.is_empty() && ["TOTAL_FORMS", "INITIAL_FORMS"].contains(name) { Some(REQUIRED) }
                else if !value.is_empty() && clean_integer(value).is_none() {Some("Enter a whole number.")}
                else {None};
            if let Some(message) = message { errors.push(format!("(Hidden field {name}) {message}")); }
        }
        Field::new(name,name,Widget::Hidden).prefix(prefix).value(value).widget_html()
    }).collect::<String>();
    if errors.is_empty() {hidden} else {format!("{}\n<div>{hidden}</div>",forms::error_list(&errors,None))}
}

/// Django extra-form has_changed: CharFields strip; choices/integers keep raw input;
/// BooleanFields use CheckboxInput's false-string rule. Only declared fields participate.
pub(super) fn extra_row_changed(
    p: Posted<'_>, chars: &[(&str, &str)], raw: &[&str], checks: &[&str],
) -> bool {
    chars.iter().any(|(name, initial)| py::py_strip(get(p, name).unwrap_or("")) != *initial)
        || raw.iter().any(|name| !get(p, name).unwrap_or("").is_empty())
        || checks.iter().any(|name| checked(p, name))
}

pub(super) fn checked(posted: Posted<'_>, name: &str) -> bool {
    get(posted, name).is_some_and(|s| !s.is_empty() && !s.eq_ignore_ascii_case("false"))
}

pub(super) fn render(
    app: &mut App,
    route: &Route,
    params: Posted<'_>,
    path: &str,
) -> AppResult<(String, u16)> {
    match route.name.as_str() {
        "home" => {
            let mut vm = ctx(app, route)?;
            let rows = app.query(crate::Query::Companies)?;
            put(
                &mut vm,
                "companies",
                rows.as_array()
                    .unwrap()
                    .iter()
                    .map(|r| json!({"pk":r["id"],"name":r["name"],"currency":r["currency"]}))
                    .collect::<Vec<_>>(),
            );
            Ok((draw(app, "ledger/home.html", &vm)?, 200))
        }
        "company_new" | "company_edit" | "account_new" | "account_edit" | "documents:setup" => {
            Ok((simple_form(app, route, path, None, &Errors::new())?, 200))
        }
        "journal_new" | "journal_edit" => journal_form(app, route, path, None, &Errors::new()),
        "journal" => Ok((journal_page(app, route)?, 200)),
        "company" => journal_list(app, route, params),
        "trial_balance" => trial_balance(app, route, params),
        "setup_start" => Ok((setup_start(app, route)?, 200)),
        "history" => history(app, route, params),
        "report_problem" => {
            let mut vm = ctx(app, route)?;
            vm["issue_url"] = json!(crate::session::ISSUE_URL);
            vm["about"] = json!({"version": app.host.version, "os": app.host.os});
            Ok((draw(app, "ledger/report_problem.html", &vm)?, 200))
        }
        _ => Err(AppError::NotFound(path.into())),
    }
}

fn account_at(app: &mut App, route: &Route) -> AppResult<Option<Account>> {
    let Some(raw) = route.args.get("account_id") else {
        return Ok(None);
    };
    let c = arg_company(route)?;
    let id = AccountId::parse(raw).ok_or_else(|| AppError::NotFound("account".into()))?;
    let a = app
        .open_mut()?
        .book
        .read(|tx| store::account(tx, id))?
        .filter(|a| a.company_id == c)
        .ok_or_else(|| AppError::NotFound("account".into()))?;
    Ok(Some(a))
}

fn locked_kind(app: &mut App, a: Option<&Account>) -> AppResult<Option<Kind>> {
    let Some(a) = a else { return Ok(None) };
    let used = app.open_mut()?.book.read(|tx| -> Result<bool, FbError> {
        Ok(store::account_has_lines(tx, a.id)?
            || tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM documents_documentaccount WHERE account_id=?1)",
                [a.id.to_db()],
                |r| r.get(0),
            )?)
    })?;
    Ok(used.then_some(a.kind))
}

fn simple_form(
    app: &mut App,
    route: &Route,
    path: &str,
    posted: Option<Posted<'_>>,
    errors: &Errors,
) -> AppResult<String> {
    simple_form_numbering(app,route,path,posted,errors,None)
}
fn simple_form_numbering(
    app: &mut App,
    route: &Route,
    path: &str,
    posted: Option<Posted<'_>>,
    errors: &Errors,
    numbering_error: Option<(Posted<'_>, String)>,
) -> AppResult<String> {
    let mut vm = ctx(app, route)?;
    let d = |n: &str, i: Option<String>| data_value(posted, n, i);
    let route_name = route.name.as_str();
    if route_name == "documents:setup" {
        put(&mut vm, "messages", std::mem::take(&mut app.messages));
    }
    let (title, fields) = match route_name {
        "company_new" | "company_edit" => {
            let company = vm["company"].clone();
            let editing = route_name == "company_edit";
            if editing {
                let mut rows=app.query(crate::Query::NumberingSettings { company_id:arg_company(route)?.to_db() })?;
                if let Some((fields,message))=numbering_error {
                    let row=rows.as_array_mut().unwrap().iter_mut().find(|r|r["kind"]==get(fields,"kind").unwrap_or("") && r["scope"]==get(fields,"bank_id").unwrap_or(""))
                        .ok_or_else(||AppError::Invalid(message.clone()))?;
                    row["error"]=json!(message);
                    row["entered"]=json!(get(fields,"next_number").unwrap_or(""));
                }
                if !app.messages.is_empty() {put(&mut vm,"numbering_message",std::mem::take(&mut app.messages).join(" "));}
                put(&mut vm,"numbering_has_bank",rows.as_array().unwrap().iter().any(|r|r["kind"]=="CHECK"));
                put(&mut vm,"numbering",rows);
                put(
                    &mut vm,
                    "setup_start_url",
                    url("setup_start", &[&arg_company(route)?.to_db()]),
                );
            }
            let document_fields = if editing {
                company_documents::fields(app, arg_company(route)?, &mut vm, posted)?
            } else { Vec::new() };
            (
                if editing {
                    "Edit company"
                } else {
                    "Create company"
                },
                {
                    let mut fields = vec![
                        Field::new("name", "Name", Widget::Text)
                            .attr("maxlength", "120")
                            .required()
                            .value_opt(d("name", company["name"].as_str().map(str::to_owned))),
                        Field::new("currency", "Currency", Widget::Text)
                            .attr("maxlength", "3")
                            .required()
                            .value_opt(d(
                                "currency",
                                Some(company["currency"].as_str().unwrap_or("USD").into()),
                            )),
                    ];
                    if !editing {
                        fields.push(
                            Field::new("starter_chart", "Chart of accounts", Widget::Select { choices: vec![
                                ("standard".into(), "Standard small-business chart (recommended)".into()),
                                ("empty".into(), "Start with an empty chart (for accountants)".into()),
                            ] })
                            .required()
                            .value(d("starter_chart", Some("standard".into())).unwrap_or_else(|| "standard".into())),
                        );
                    }
                    fields.extend(document_fields);
                    fields
                },
            )
        }
        "account_new" | "account_edit" => {
            let a = account_at(app, route)?;
            let locked = locked_kind(app, a.as_ref())?;
            let mut groups = vec![];
            if locked.is_none() {
                groups.push((String::new(), vec![(String::new(), "Choose a type".into())]));
            }
            for kind in Kind::ALL {
                if locked.is_none_or(|k| k == kind) {
                    groups.push((
                        kind.label().to_owned(),
                        DetailType::ALL
                            .iter()
                            .filter(|t| t.kind() == kind)
                            .map(|t| (t.as_str().into(), t.label().into()))
                            .collect(),
                    ));
                }
            }
            let initial = |f: fn(&Account) -> String| a.as_ref().map(f);
            let on = |n: &str, default: bool| posted.map(|p| checked(p, n)).unwrap_or(default);
            let mut ty = Field::new("detail_type", "Type", Widget::GroupedSelect { groups })
                .required()
                .value(
                    d("detail_type", initial(|a| a.detail_type.as_str().into()))
                        .unwrap_or_default(),
                );
            if let Some(k) = locked {
                ty=ty.help(format!("This account already has activity (journals or documents), so only {} types are offered.",k.label().to_lowercase()));
            }
            (if a.is_some(){"Edit account"}else{"Create account"},vec![
                Field::new("code","Code",Widget::Text).attr("maxlength","20").required().value_opt(d("code",initial(|a|a.code.clone()))),
                Field::new("name","Name",Widget::Text).attr("maxlength","120").required().value_opt(d("name",initial(|a|a.name.clone()))),
                ty,
                Field::new("is_contra","Contra account",Widget::Checkbox{checked:on("is_contra",a.as_ref().is_some_and(|a|a.is_contra))}).help("Balance sits opposite its type, like accumulated depreciation or owner's draw. Reports show it as a negative in its section."),
                Field::new("description","Description",Widget::Text).attr("maxlength","200").value_opt(d("description",initial(|a|a.description.clone()))).help("Optional. Shown on the chart of accounts."),
                Field::new("active","Active",Widget::Checkbox{checked:on("active",a.as_ref().is_none_or(|a|a.active))}),
                Field::new("postable","Postable",Widget::Checkbox{checked:on("postable",a.as_ref().is_none_or(|a|a.postable))}).help("Turn off for summary-only accounts."),
            ])
        }
        "documents:setup" => {
            let c = arg_company(route)?;
            let (setup, accounts) = app.open_mut()?.book.read(|tx| -> Result<_, FbError> {
                Ok((
                    fb_core::documents::setup_snapshot(tx, c)?,
                    store::accounts(tx, c)?,
                ))
            })?;
            vm["setup"] = if setup["revision"] == 0 {
                Value::Null
            } else {
                setup.clone()
            };
            let mut fields = vec![
                Field::new("expected_revision", "Expected revision", Widget::Hidden)
                    .required()
                    .value(
                        d("expected_revision", Some(setup["revision"].to_string()))
                            .unwrap_or_default(),
                    ),
            ];
            for role in fb_core::documents::ROLES {
                let kind = if ["receivables", "undeposited"].contains(role) {
                    Kind::Asset
                } else {
                    Kind::Liability
                };
                let available = |a: &Account| {
                    a.active
                        && a.postable
                        && a.kind == kind
                        && (*role != "customer_deposits"
                            || a.detail_type == DetailType::OtherCurrentLiability)
                };
                let mut choices = vec![(String::new(), "Unconfigured — select when needed".into())];
                choices.extend(
                    accounts
                        .iter()
                        .filter(|a| available(a) || setup[*role] == a.id.to_db())
                        .map(|a| {
                            (
                                a.id.to_db(),
                                format!(
                                    "{}{}",
                                    a.label(),
                                    if available(a) {
                                        ""
                                    } else {
                                        " (unavailable — replace or clear)"
                                    }
                                ),
                            )
                        }),
                );
                let label = role.replace('_', " ");
                let label = format!("{}{}", label[..1].to_uppercase(), &label[1..]);
                fields.push(Field::new(role,&label,Widget::Select{choices}).value(d(role,setup[*role].as_str().map(str::to_owned)).unwrap_or_default()).help(if *role=="customer_deposits"{"Other current liability account that holds customer prepayments until they are applied to invoices; optional until you receive a customer deposit.".into()}else{format!("{} account; optional until used by a document.",kind.label())}));
            }
            let selected = posted
                .map(|p| {
                    p.iter()
                        .filter(|(k, _)| k == "banks")
                        .map(|(_, v)| v.clone())
                        .collect()
                })
                .unwrap_or_else(|| {
                    setup["banks"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                });
            let choices = accounts
                .iter()
                .filter(|a| {
                    (a.active && a.postable && a.kind == Kind::Asset)
                        || setup["banks"]
                            .as_array()
                            .unwrap()
                            .contains(&json!(a.id.to_db()))
                })
                .map(|a| {
                    (
                        a.id.to_db(),
                        format!(
                            "{}{}",
                            a.label(),
                            if a.active && a.postable && a.kind == Kind::Asset {
                                ""
                            } else {
                                " (unavailable — replace or clear)"
                            }
                        ),
                    )
                })
                .collect();
            fields.push(Field::new("banks","Designated bank accounts",Widget::MultipleSelect{choices,selected}).attr("size","5").help("Select all bank accounts to designate. Control/Command-click to select multiple or clear a selection."));
            ("Accounting setup", fields)
        }
        _ => return Err(AppError::NotFound(path.into())),
    };
    put(
        &mut vm,
        "form",
        forms::bind(&decorate(fields, errors), &nonfield(errors, "__all__")),
    );
    put(&mut vm, "title", title);
    put(&mut vm, "csrf_token", token(app, path));
    draw(
        app,
        if route_name == "documents:setup" {
            "documents/setup.html"
        } else if route_name == "company_edit" {
            "ledger/company_form.html"
        } else {
            "ledger/form.html"
        },
        &vm,
    )
}

fn journal_at(app: &mut App, route: &Route) -> AppResult<Option<Journal>> {
    let Some(id) = route.args.get("journal_id") else {
        return Ok(None);
    };
    let c = arg_company(route)?;
    let id = JournalId::parse(id).ok_or_else(|| AppError::NotFound("journal".into()))?;
    Ok(Some(
        app.open_mut()?
            .book
            .read(|tx| store::journal_in(tx, id, c))?,
    ))
}

fn journal_vm(tx: &(impl Query + ?Sized), j: &Journal) -> Result<Value, FbError> {
    let mut v = json!({"pk":j.id.to_db(),"label":j.display(),"display_number":j.display_number(),"entry_number":j.entry_number,"date":j.date,"memo":j.memo,"status":j.status.as_str(),"get_status_display":if j.status==Status::Draft{"Draft"}else{"Posted"},"adjusting":j.adjusting,"source_id":j.source_id,"reversal_of_id":j.reversal_of_id.map(|id|id.to_db()),"manual":j.manual(),"revision":j.revision,"posted_at":j.posted_at});
    if let Some(id) = &j.source_id {
        v["source"] = fb_core::documents::identity(tx, j.company_id, id)?;
    }
    if let Some(id) = j.posted_by_id {
        v["posted_by"] =
            json!({"username":store::user(tx,id)?.map(|u|u.username).unwrap_or_default()});
    }
    if let Some(id) = j.reversal_of_id
        && let Some(original) = store::journal(tx, id)?
    {
        v["reversal_of"] = json!({"pk":id.to_db(),"display_number":original.display_number(),"entry_number":original.entry_number,"label":original.display()});
    }
    let reversal = tx
        .query_row(
            "SELECT id FROM ledger_journal WHERE reversal_of_id=?1 AND company_id=?2",
            params![j.id.to_db(), j.company_id.to_db()],
            |r| r.get::<_, String>(0),
        )
        .optional()?;
    if let Some(id) = reversal {
        let r = store::journal(tx, JournalId::parse(&id).unwrap())?.unwrap();
        v["reversal"] = json!({"pk":id,"label":r.display(),"date":r.date});
    }
    Ok(v)
}

fn controls_and_choices(app: &mut App, c: CompanyId) -> AppResult<(bool, Vec<(String, String)>)> {
    Ok(app.open_mut()?.book.read(|tx| -> Result<_, FbError> {
        let mut present = false;
        let mut choices = vec![(String::new(), "Choose…".into())];
        for a in store::accounts(tx, c)? {
            let control = fb_core::documents::control_role(tx, c, a.id)?.is_some();
            present |= control;
            if a.active && a.postable {
                choices.push((
                    a.id.to_db(),
                    format!(
                        "{}{}",
                        a.label(),
                        if control {
                            " — control account, not allowed"
                        } else {
                            ""
                        }
                    ),
                ));
            }
        }
        Ok((present, choices))
    })?)
}

fn journal_line(
    prefix: &str,
    choices: &[(String, String)],
    posted: Option<Posted<'_>>,
    initial: Option<&fb_core::model::JournalLine>,
    errors: &Errors,
) -> forms::BoundForm {
    let d = |n: &str, i: Option<String>| data_value(posted, &format!("{prefix}-{n}"), i);
    let fields = vec![
        Field::new(
            "account",
            "Account",
            Widget::Select {
                choices: choices.to_vec(),
            },
        )
        .prefix(prefix)
        .value(d("account", initial.map(|l| l.account_id.to_db())).unwrap_or_default()),
        Field::new("description", "Memo", Widget::Text)
            .prefix(prefix)
            .value_opt(d("description", initial.map(|l| l.description.clone()))),
        Field::new("debit", "Debit", Widget::Text)
            .prefix(prefix)
            .attr("inputmode", "decimal")
            .attr("placeholder", "0.00")
            .attr("maxlength", "40")
            .value_opt(d("debit", initial.map(|l| l.debit.to_string()))),
        Field::new("credit", "Credit", Widget::Text)
            .prefix(prefix)
            .attr("inputmode", "decimal")
            .attr("placeholder", "0.00")
            .attr("maxlength", "40")
            .value_opt(d("credit", initial.map(|l| l.credit.to_string()))),
        Field::new("name", "Name", Widget::Text)
            .prefix(prefix)
            .attr("list", "journal-names")
            .attr("autocomplete", "off")
            .value_opt(d("name", initial.map(|l| l.name.clone()))),
        Field::new(
            "DELETE",
            "Delete",
            Widget::Checkbox {
                checked: posted.is_some_and(|p| checked(p, &format!("{prefix}-DELETE"))),
            },
        )
        .prefix(prefix),
    ];
    forms::bind(
        &decorate(fields, errors),
        &nonfield(errors, &format!("{prefix}-__all__")),
    )
}

fn failure(app: &mut App, route: &Route, errors: Vec<String>) -> AppResult<String> {
    let mut vm = ctx(app, route)?;
    put(&mut vm, "errors", errors);
    draw(app, "ledger/error.html", &vm)
}

fn journal_form(
    app: &mut App,
    route: &Route,
    path: &str,
    posted: Option<Posted<'_>>,
    errors: &Errors,
) -> AppResult<(String, u16)> {
    let mut vm = ctx(app, route)?;
    let c = arg_company(route)?;
    let journal = journal_at(app, route)?;
    if let Some(j) = &journal {
        let refusal = if j.source_id.is_some() {
            Some("Use the owning source document lifecycle, not generic journal actions.")
        } else if j.status != Status::Draft {
            Some("Posted journals are immutable. Use a reversal instead.")
        } else {
            None
        };
        if let Some(m) = refusal {
            return Ok((failure(app, route, vec![m.into()])?, 400));
        }
    }
    let (controls_present, choices) = controls_and_choices(app, c)?;
    let today = (app.today)().to_string();
    let (initial,names,next)=app.open_mut()?.book.read(|tx|->Result<_,FbError>{
        let lines=journal.as_ref().map(|j|store::lines(tx,j.id)).transpose()?.unwrap_or_default();
        let mut names=std::collections::BTreeSet::new();
        for table in ["directory_customer","directory_vendor"] {
            let mut st=tx.prepare(&format!("SELECT display_name FROM {table} WHERE company_id=?1 AND active=1 ORDER BY display_name COLLATE fb_text,id LIMIT 500"))?;
            names.extend(st.query_map([c.to_db()],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?);
        }
        if let Some(j)=&journal{put(&mut vm,"journal",journal_vm(tx,j)?);}
        Ok((lines,names.into_iter().collect::<Vec<_>>(),fb_core::services::next_entry_number_tx(tx,c)?))
    })?;
    let d = |n: &str, i: Option<String>| data_value(posted, n, i);
    let header = vec![
        Field::new("request_key", "Request key", Widget::Hidden).value_opt(d(
            "request_key",
            journal.is_none().then(|| uuid::Uuid::new_v4().to_string()),
        )),
        Field::new("expected_revision", "Expected revision", Widget::Hidden)
            .required()
            .value(
                d(
                    "expected_revision",
                    Some(
                        journal
                            .as_ref()
                            .map(|j| j.revision)
                            .unwrap_or(0)
                            .to_string(),
                    ),
                )
                .unwrap_or_default(),
            ),
        Field::new("date", "Date", Widget::Date)
            .required()
            .value_opt(d(
                "date",
                Some(journal.as_ref().map(|j| j.date.clone()).unwrap_or(today)),
            )),
        Field::new("entry_number", "Entry no.", Widget::Text)
            .value_opt(d(
                "entry_number",
                Some(
                    journal
                        .as_ref()
                        .filter(|j| !j.entry_number.is_empty())
                        .map(|j| j.entry_number.clone())
                        .unwrap_or(next),
                ),
            ))
            .help("Next number filled in for you; any unique text up to 20 characters."),
        Field::new(
            "adjusting",
            "Adjusting entry",
            Widget::Checkbox {
                checked: posted
                    .map(|p| checked(p, "adjusting"))
                    .unwrap_or(journal.as_ref().is_some_and(|j| j.adjusting)),
            },
        ),
        Field::new("memo", "Memo", Widget::Text)
            .required()
            .value_opt(d("memo", journal.as_ref().map(|j| j.memo.clone()))),
    ];
    let total = posted
        .map(|p| {
            get(p, "lines-TOTAL_FORMS")
                .and_then(clean_integer)
                .unwrap_or(0)
                .clamp(0, 50) as usize
        })
        .unwrap_or((initial.len() + 8).min(50));
    let forms = (0..total)
        .map(|i| {
            journal_line(
                &format!("lines-{i}"),
                &choices,
                posted,
                initial.get(i),
                errors,
            )
        })
        .collect();
    let management: [(&str, String); 4] = [
        (
            "TOTAL_FORMS",
            posted
                .map(|p| get(p, "lines-TOTAL_FORMS").unwrap_or("").to_owned())
                .unwrap_or(total.to_string()),
        ),
        (
            "INITIAL_FORMS",
            posted
                .map(|p| get(p, "lines-INITIAL_FORMS").unwrap_or("").to_owned())
                .unwrap_or(initial.len().to_string()),
        ),
        (
            "MIN_NUM_FORMS",
            posted
                .map(|p| get(p, "lines-MIN_NUM_FORMS").unwrap_or("").to_owned())
                .unwrap_or("0".into()),
        ),
        (
            "MAX_NUM_FORMS",
            posted
                .map(|p| get(p, "lines-MAX_NUM_FORMS").unwrap_or("").to_owned())
                .unwrap_or("50".into()),
        ),
    ];
    let management_form = management_html("lines", &management, posted.is_some());
    put(
        &mut vm,
        "form",
        forms::bind(&decorate(header, errors), &nonfield(errors, "__all__")),
    );
    put(
        &mut vm,
        "lines",
        FormsetVm {
            forms,
            management_form,
            non_form_errors: forms::error_list(&nonfield(errors, "lines-__all__"), None),
            empty_form: journal_line("lines-__prefix__", &choices, None, None, &Errors::new()),
            total_error_count: formset_error_count(errors, posted, "lines"),
        },
    );
    put(&mut vm, "names", names);
    put(&mut vm, "line_max", 50);
    put(&mut vm, "controls_present", controls_present);
    put(&mut vm, "csrf_token", token(app, path));
    Ok((
        draw(app, "ledger/journal_form.html", &vm)?,
        if posted.is_some() { 400 } else { 200 },
    ))
}

fn setup_start(app: &mut App, route: &Route) -> AppResult<String> {
    let mut vm = ctx(app, route)?;
    let c = arg_company(route)?;
    put(&mut vm, "messages", std::mem::take(&mut app.messages));
    let state = app
        .open_mut()?
        .book
        .read(|tx| fb_core::first_run::checklist(tx, c))?;
    let rows = [
        (
            "Chart of accounts",
            "chart",
            false,
            "Add at least one income account and one expense account.",
            "accounts",
            None,
            "Open the chart",
        ),
        (
            "Accounting setup",
            "special",
            false,
            "Choose the accounts for money customers owe you, money you owe vendors, undeposited funds, sales tax you have collected, and at least one bank.",
            "documents:setup",
            None,
            "Open Accounting setup",
        ),
        (
            "Sales tax",
            "tax",
            false,
            "Say whether this company charges sales tax.",
            "salestax:settings",
            None,
            "Open sales tax settings",
        ),
        (
            "Customers (optional)",
            "customer",
            true,
            "Add a customer when you are ready to invoice.",
            "directory:new",
            Some("customers"),
            "Add a customer",
        ),
        (
            "Vendors (optional)",
            "vendor",
            true,
            "Add a vendor when you are ready to enter bills.",
            "directory:new",
            Some("vendors"),
            "Add a vendor",
        ),
        (
            "Items (optional)",
            "item",
            true,
            "Add an item or service you sell.",
            "directory:new",
            Some("items"),
            "Add an item",
        ),
    ];
    put(&mut vm,"items",rows.iter().map(|(label,key,optional,detail,route,kind,_)|json!({"label":label,"done":state[*key],"optional":optional,"detail":detail,"url":if let Some(k)=kind{url(route,&[&c.to_db(),k])}else{url(route,&[&c.to_db()])}})).collect::<Vec<_>>());
    // The link text is part of the frozen checklist, not generated from the route.
    for (row, (_, _, _, _, _, _, link)) in vm["items"].as_array_mut().unwrap().iter_mut().zip(rows)
    {
        row["link"] = json!(link);
    }
    put(&mut vm, "state", state);
    put(
        &mut vm,
        "fill_token",
        token(app, &url("setup_fill", &[&c.to_db()])),
    );
    put(
        &mut vm,
        "done_token",
        token(app, &url("setup_done", &[&c.to_db()])),
    );
    draw(app, "ledger/setup_start.html", &vm)
}

fn setup_fill(app: &mut App, route: &Route, errors: &[String]) -> AppResult<String> {
    let mut vm = ctx(app, route)?;
    let c = arg_company(route)?;
    let proposal = app
        .open_mut()?
        .book
        .read(|tx| fb_core::first_run::propose(tx, c))?;
    put(&mut vm, "proposal", proposal);
    put(&mut vm, "errors", errors);
    put(
        &mut vm,
        "csrf_token",
        token(app, &url("setup_fill", &[&c.to_db()])),
    );
    draw(app, "ledger/setup_fill.html", &vm)
}

fn trial_balance(app: &mut App, route: &Route, params: Posted<'_>) -> AppResult<(String, u16)> {
    let mut vm = ctx(app, route)?;
    let c = arg_company(route)?;
    let today = (app.today)().to_string();
    let open = app.open_mut()?;
    let result = fb_core::reports::trial_balance(
        &mut open.book,
        open.owner,
        c,
        get(params, "as_of")
            .filter(|s| !s.is_empty())
            .unwrap_or(&today),
    );
    match result {
        Ok(r) => {
            put(&mut vm, "as_of", r.as_of);
            put(&mut vm, "debit", r.debit.to_string());
            put(&mut vm, "credit", r.credit.to_string());
            put(&mut vm,"rows",r.rows.iter().map(|r|json!({"account":account_vm(&r.account),"debit":r.debit.to_string(),"credit":r.credit.to_string()})).collect::<Vec<_>>());
            Ok((draw(app, "ledger/trial_balance.html", &vm)?, 200))
        }
        Err(FbError::Validation(messages)) => Ok((failure(app, route, messages)?, 400)),
        Err(e) => Err(e.into()),
    }
}

fn journal_list(app: &mut App, route: &Route, params: Posted<'_>) -> AppResult<(String, u16)> {
    let mut vm = ctx(app, route)?;
    let c = arg_company(route)?;
    let mut errors = Errors::new();
    // A 3xx Rendered carries its Location path in the existing body slot; main follows it locally.
    let first = app
        .open_mut()?
        .book
        .read(|tx| store::get_company(tx, c))?
        .first_run_open;
    if first {
        return Ok((url("setup_start", &[&c.to_db()]), 302));
    }
    let q = clean_text(params, "q", 120, false, true, &mut errors);
    let start = clean_date(params, "start", false, &mut errors);
    let end = clean_date(params, "end", false, &mut errors);
    let choices = [
        (
            "origin",
            "Origin",
            vec![
                ("", "All journals"),
                ("MANUAL", "General journal entries"),
                ("DOCUMENT", "From documents"),
            ],
        ),
        (
            "adjusting",
            "Adjusting",
            vec![
                ("", "Adjusting or not"),
                ("1", "Adjusting only"),
                ("0", "Not adjusting"),
            ],
        ),
        (
            "status",
            "Status",
            vec![("", "Any status"), ("DRAFT", "Draft"), ("POSTED", "Posted")],
        ),
    ];
    let mut fields = vec![
        Field::new("q", "Entry no., J-number or memo", Widget::Text)
            .attr("maxlength", "120")
            .value_opt(get(params, "q").map(str::to_owned)),
    ];
    for (name, label, list) in choices {
        let value = get(params, name).unwrap_or("");
        if !list.iter().any(|(v, _)| *v == value) {
            error(
                &mut errors,
                name,
                format!("Select a valid choice. {value} is not one of the available choices."),
            );
        }
        fields.push(
            Field::new(
                name,
                label,
                Widget::Select {
                    choices: list
                        .into_iter()
                        .map(|(a, b)| (a.into(), b.into()))
                        .collect(),
                },
            )
            .value(value),
        );
    }
    fields.extend([
        Field::new("start", "From", Widget::Date)
            .value_opt(get(params, "start").map(str::to_owned)),
        Field::new("end", "Through", Widget::Date).value_opt(get(params, "end").map(str::to_owned)),
    ]);
    if start.as_ref().zip(end.as_ref()).is_some_and(|(s, e)| s > e) {
        error(
            &mut errors,
            "__all__",
            "The start date must not be after the end date.",
        );
    }
    let rows = if errors.is_empty() {
        app.open_mut()?
            .book
            .read(|tx| -> Result<Vec<Value>, FbError> {
                let mut st = tx.prepare(&format!(
                    "SELECT {} FROM ledger_journal WHERE company_id=?1 ORDER BY date DESC,id DESC",
                    store::JOURNAL_COLS
                ))?;
                let all = st
                    .query_map([c.to_db()], store::journal_row)?
                    .collect::<Result<Vec<_>, _>>()?;
                let digits = q.strip_prefix(['J', 'j']).unwrap_or(&q);
                let journal_number = if digits.chars().count() <= 12 {
                    digits.chars().try_fold(0i64, |n, ch| {
                        n.checked_mul(10)?
                            .checked_add(fb_money::unicode_decimal(ch)? as i64)
                    })
                } else {
                    None
                };
                all.iter()
                    .filter(|j| {
                        (q.is_empty()
                            || fb_db::collation::upper(&j.entry_number)
                                .contains(&fb_db::collation::upper(&q))
                            || fb_db::collation::upper(&j.memo)
                                .contains(&fb_db::collation::upper(&q))
                            || journal_number == Some(j.number))
                            && match get(params, "origin").unwrap_or("") {
                                "MANUAL" => j.source_id.is_none(),
                                "DOCUMENT" => j.source_id.is_some(),
                                _ => true,
                            }
                            && match get(params, "adjusting").unwrap_or("") {
                                "1" => j.adjusting,
                                "0" => !j.adjusting,
                                _ => true,
                            }
                            && get(params, "status")
                                .filter(|s| !s.is_empty())
                                .is_none_or(|s| j.status.as_str() == s)
                            && start.as_ref().is_none_or(|s| j.date >= *s)
                            && end.as_ref().is_none_or(|s| j.date <= *s)
                    })
                    .map(|j| journal_vm(tx, j))
                    .collect()
            })?
    } else {
        vec![]
    };
    let page = Page::of(&rows, 25, get(params, "page"));
    put(&mut vm, "journals", &page.object_list);
    put(&mut vm, "page", page);
    put(
        &mut vm,
        "filter_form",
        forms::bind(&decorate(fields, &errors), &nonfield(&errors, "__all__")),
    );
    put(&mut vm, "filtered", !params.is_empty());
    put(
        &mut vm,
        "query_string",
        urlencode(
            &params
                .iter()
                .filter(|(k, _)| k != "page")
                .cloned()
                .collect::<Vec<_>>(),
        ),
    );
    Ok((
        draw(app, "ledger/company.html", &vm)?,
        if errors.is_empty() { 200 } else { 400 },
    ))
}

pub(super) fn is_form(name: &str) -> bool {
    matches!(
        name,
        "company_new"
            | "company_edit"
            | "account_new"
            | "account_edit"
            | "journal_new"
            | "journal_edit"
            | "journal_post"
            | "journal_reverse"
            | "documents:setup"
            | "setup_fill"
            | "setup_done"
    )
}

pub(super) fn submit(
    app: &mut App,
    route: &Route,
    path: &str,
    fields: Posted<'_>,
) -> AppResult<Outcome> {
    // Check membership and row ownership before parsing any field, just as the Django views do.
    let _ = ctx(app, route)?;
    match route.name.as_str() {
        "company_edit" if get(fields,"action")==Some("set_next_number") => submit_numbering(app,route,path,fields),
        "journal_new" | "journal_edit" => submit_journal(app, route, path, fields),
        "journal_post" | "journal_reverse" => submit_journal_action(app, route, fields),
        "setup_fill" => {
            let c = arg_company(route)?;
            match get(fields, "action") {
                Some("preview") => Ok(Outcome::Page {
                    html: setup_fill(app, route, &[])?,
                    status: 200,
                }),
                Some("apply") => match app.run(Command::FillAccountingSetup {
                    company_id: c.to_db(),
                    digest: get(fields, "digest").unwrap_or("").into(),
                    expected_revision: get(fields, "expected_revision").and_then(forms::python_integer),
                }) {
                    Ok(_) => {
                        app.messages.push("Accounting setup filled from your chart. You can still change every choice.".into());
                        Ok(Outcome::Redirect {
                            location: url("setup_start", &[&c.to_db()]),
                        })
                    }
                    Err(AppError::Core(FbError::Validation(e))) => Ok(Outcome::Invalid {
                        html: setup_fill(app, route, &e)?,
                    }),
                    Err(e) => Err(e),
                },
                _ => Ok(Outcome::Invalid {
                    html: failure(app, route, vec!["Review the chart before saving.".into()])?,
                }),
            }
        }
        "setup_done" => {
            let c = arg_company(route)?;
            app.run(Command::DismissSetup {
                company_id: c.to_db(),
            })?;
            Ok(Outcome::Redirect {
                location: url("company_home", &[&c.to_db()]),
            })
        }
        _ => submit_simple(app, route, path, fields),
    }
}

fn submit_numbering(app: &mut App, route: &Route, path: &str, fields: Posted<'_>) -> AppResult<Outcome> {
    let co=arg_company(route)?;
    let kind=get(fields,"kind").unwrap_or("");
    let scope=get(fields,"bank_id").unwrap_or("");
    let bank=if scope.is_empty(){None}else{Some(scope.to_owned())};
    let revision=get(fields,"expected_revision").and_then(forms::python_integer).unwrap_or(-1);
    let result=app.run(Command::SetNextNumber {company_id:co.to_db(),kind:kind.into(),bank_id:bank,next:get(fields,"next_number").unwrap_or("").into(),expected_next:get(fields,"expected_next").unwrap_or("").into(),expected_revision:revision});
    match result {
        Ok(Outcome::Done {..})=>{
            app.messages.push("Next number saved.".into());
            Ok(Outcome::Redirect {location:format!("{path}#document-numbering")})
        }
        Err(AppError::Core(FbError::Validation(messages)))=>Ok(Outcome::Invalid {
            html:simple_form_numbering(app,route,path,None,&Errors::new(),Some((fields,messages.join("; "))))?,
        }),
        Err(e)=>Err(e),
        other=>other,
    }
}

fn submit_simple(
    app: &mut App,
    route: &Route,
    path: &str,
    fields: Posted<'_>,
) -> AppResult<Outcome> {
    let mut errors = Errors::new();
    let command = match route.name.as_str() {
        "company_new" | "company_edit" => {
            let name = clean_text(fields, "name", 120, true, true, &mut errors);
            let currency =
                clean_text(fields, "currency", 3, true, true, &mut errors).to_uppercase();
            if !errors.contains_key("currency")
                && (currency.len() != 3 || !currency.bytes().all(|b| b.is_ascii_alphabetic()))
            {
                error(
                    &mut errors,
                    "currency",
                    "Use a three-letter currency code, e.g. USD.",
                );
            }
            if route.name == "company_edit" {
                Command::SaveCompanyDetails { company_id: arg_company(route)?.to_db(), name, currency,
                    values: company_documents::clean_post(fields, &mut errors) }
            } else {
                let starter_chart = match get(fields, "starter_chart").unwrap_or("standard") {
                    "standard" => crate::commands::StarterChart::Standard,
                    "empty" => crate::commands::StarterChart::Empty,
                    _ => { error(&mut errors, "starter_chart", CHOICE); crate::commands::StarterChart::Empty }
                };
                Command::SaveCompany { company_id: None, name, currency, starter_chart }
            }
        }
        "account_new" | "account_edit" => {
            let c = arg_company(route)?;
            let a = account_at(app, route)?;
            let locked = locked_kind(app, a.as_ref())?;
            let code = clean_text(fields, "code", 20, true, true, &mut errors);
            let name = clean_text(fields, "name", 120, true, true, &mut errors);
            let description = clean_text(fields, "description", 200, false, true, &mut errors);
            let detail_type = get(fields, "detail_type").unwrap_or("").to_owned();
            let detail = DetailType::parse(&detail_type);
            let is_contra = checked(fields, "is_contra");
            if detail.is_none() {
                error(
                    &mut errors,
                    "detail_type",
                    if detail_type.is_empty() {
                        REQUIRED.into()
                    } else {
                        format!(
                            "Select a valid choice. {detail_type} is not one of the available choices."
                        )
                    },
                );
            }
            if let Some(detail) = detail {
                if let Some(k) = locked.filter(|k| *k != detail.kind()) {
                    error(
                        &mut errors,
                        "detail_type",
                        format!(
                            "An account used in journals or source document snapshots cannot change its type to another kind; choose another {} type.",
                            k.label().to_lowercase()
                        ),
                    );
                }
                if is_contra && detail.refuses_contra() {
                    error(
                        &mut errors,
                        "is_contra",
                        format!(
                            "A {} account cannot be a contra account.",
                            detail.label().to_lowercase()
                        ),
                    );
                }
            }
            Command::SaveAccount {
                company_id: c.to_db(),
                account_id: a.map(|a| a.id.to_db()),
                code,
                name,
                detail_type,
                is_contra,
                description,
                active: checked(fields, "active"),
                postable: checked(fields, "postable"),
            }
        }
        "documents:setup" => {
            let c = arg_company(route)?;
            let revision = clean_revision(fields, &mut errors);
            // Resolve choices from the same scoped queryset the GET form uses, including retained unavailable accounts.
            let (snapshot, accounts) = app.open_mut()?.book.read(|tx| -> Result<_, FbError> {
                Ok((
                    fb_core::documents::setup_snapshot(tx, c)?,
                    store::accounts(tx, c)?,
                ))
            })?;
            let mut ids = HashMap::new();
            for role in fb_core::documents::ROLES {
                let value = get(fields, role).unwrap_or("");
                let kind = if ["receivables", "undeposited"].contains(role) {
                    Kind::Asset
                } else {
                    Kind::Liability
                };
                if !value.is_empty()
                    && !accounts.iter().any(|a| {
                        a.id.to_db() == value
                            && ((a.active
                                && a.postable
                                && a.kind == kind
                                && (*role != "customer_deposits"
                                    || a.detail_type == DetailType::OtherCurrentLiability))
                                || snapshot[*role] == value)
                    })
                {
                    error(&mut errors, role, CHOICE);
                }
                ids.insert(*role, (!value.is_empty()).then(|| value.to_owned()));
            }
            let mut bank_ids: Vec<String> = fields
                .iter()
                .filter(|(k, _)| k == "banks")
                .map(|(_, v)| v.clone())
                .collect();
            bank_ids.sort();
            bank_ids.dedup();
            for id in &bank_ids {
                if !accounts.iter().any(|a| {
                    a.id.to_db() == *id
                        && ((a.active && a.postable && a.kind == Kind::Asset)
                            || snapshot["banks"].as_array().unwrap().contains(&json!(id)))
                }) {
                    error(
                        &mut errors,
                        "banks",
                        format!("Select a valid choice. {id} is not one of the available choices."),
                    );
                }
            }
            Command::SaveSetup {
                company_id: c.to_db(),
                expected_revision: revision.unwrap_or(0),
                receivables_id: ids.remove("receivables").flatten(),
                payables_id: ids.remove("payables").flatten(),
                undeposited_id: ids.remove("undeposited").flatten(),
                collected_tax_id: ids.remove("collected_tax").flatten(),
                customer_deposits_id: ids.remove("customer_deposits").flatten(),
                bank_ids,
            }
        }
        _ => return Err(AppError::NotFound(path.into())),
    };
    if errors.is_empty() {
        match app.run(command) {
            Ok(Outcome::Done { data }) => {
                let c = route
                    .args
                    .get("company_id")
                    .cloned()
                    .or_else(|| data["id"].as_str().map(str::to_owned))
                    .unwrap();
                if route.name == "documents:setup" {
                    app.messages.push(format!(
                        "Accounting setup saved (revision {}).",
                        data["revision"]
                    ));
                }
                let name = match route.name.as_str() {
                    "company_new" => "setup_start",
                    "company_edit" => "company",
                    "documents:setup" => "documents:setup",
                    _ => "accounts",
                };
                return Ok(Outcome::Redirect {
                    location: url(name, &[&c]),
                });
            }
            Err(AppError::Core(FbError::Validation(messages))) => {
                error(&mut errors, "__all__", messages.join("; "))
            }
            Err(e) => return Err(e),
            other => return Ok(other.unwrap()),
        }
    }
    Ok(Outcome::Invalid {
        html: simple_form(app, route, path, Some(fields), &errors)?,
    })
}

fn submit_journal(
    app: &mut App,
    route: &Route,
    path: &str,
    fields: Posted<'_>,
) -> AppResult<Outcome> {
    let c = arg_company(route)?;
    let j = journal_at(app, route)?;
    if j.as_ref()
        .is_some_and(|j| j.source_id.is_some() || j.status != Status::Draft)
    {
        return Ok(Outcome::Invalid {
            html: journal_form(app, route, path, Some(fields), &Errors::new())?.0,
        });
    }
    let mut errors = Errors::new();
    let revision = clean_revision(fields, &mut errors);
    // Django `forms.UUIDField`: strip (CharField), then required, then `uuid.UUID`.
    let raw_key = py::py_strip(get(fields, "request_key").unwrap_or(""));
    let key = if raw_key.is_empty() {
        if j.is_none() {
            error(&mut errors, "request_key", REQUIRED);
        }
        None
    } else {
        match py::py_uuid(raw_key) {
            Some(k) => Some(k.0),
            None => {
                error(&mut errors, "request_key", "Enter a valid UUID.");
                None
            }
        }
    };
    let date = clean_date(fields, "date", true, &mut errors).unwrap_or_default();
    let entry_number = clean_text(fields, "entry_number", 20, false, false, &mut errors);
    let memo = clean_text(fields, "memo", 240, true, true, &mut errors);
    let (_, choices) = controls_and_choices(app, c)?;
    let total = get(fields, "lines-TOTAL_FORMS").and_then(clean_integer);
    let initial = get(fields, "lines-INITIAL_FORMS").and_then(clean_integer);
    if total.is_none() || initial.is_none() {
        let missing = [
            ("lines-TOTAL_FORMS", total),
            ("lines-INITIAL_FORMS", initial),
        ]
        .iter()
        .filter(|(_, v)| v.is_none())
        .map(|(k, _)| *k)
        .collect::<Vec<_>>()
        .join(", ");
        error(
            &mut errors,
            "lines-__all__",
            format!(
                "ManagementForm data is missing or has been tampered with. Missing fields: {missing}. You may need to file a bug report if the issue persists."
            ),
        );
    }
    let total = total.unwrap_or(0).max(0);
    if total > 50 {
        error(
            &mut errors,
            "lines-__all__",
            "A journal entry has at most 50 lines.",
        );
    }
    let mut lines = vec![];
    for i in 0..total.min(50) {
        let prefix = format!("lines-{i}");
        let f = |n: &str| get(fields, &format!("{prefix}-{n}")).unwrap_or("");
        let deleted = checked(fields, &format!("{prefix}-DELETE"));
        let changed = ["account", "description", "debit", "credit", "name"]
            .iter()
            .any(|n| !f(n).is_empty());
        if i >= initial.unwrap_or(0) && !changed {
            continue;
        }
        let mut line_errors = Errors::new();
        let account = f("account");
        if !account.is_empty() && !choices.iter().any(|(id, _)| id == account) {
            error(&mut line_errors, "account", CHOICE);
        }
        let description = clean_text(
            fields,
            &format!("{prefix}-description"),
            160,
            false,
            true,
            &mut errors,
        );
        let name = clean_text(
            fields,
            &format!("{prefix}-name"),
            120,
            false,
            true,
            &mut errors,
        );
        let mut amounts = vec![];
        for field in ["debit", "credit"] {
            let text = clean_text(
                fields,
                &format!("{prefix}-{field}"),
                40,
                false,
                true,
                &mut errors,
            );
            if errors.contains_key(&format!("{prefix}-{field}")) {
                amounts.push(None);
                continue;
            }
            match fb_core::services::money_json(&json!(if text.is_empty() { "0" } else { &text })) {
                Ok(m) => amounts.push(Some(m)),
                Err(FbError::Validation(e)) => {
                    for e in e {
                        error(&mut line_errors, field, e);
                    }
                    amounts.push(None)
                }
                Err(e) => return Err(e.into()),
            }
        }
        let field_prefix = format!("{prefix}-");
        if line_errors.is_empty()
            && !errors.keys().any(|k| k.starts_with(&field_prefix))
            && let (Some(debit), Some(credit)) = (amounts[0], amounts[1])
        {
            if account.is_empty() && debit.cents() == 0 && credit.cents() == 0 {
                continue;
            }
            if account.is_empty() {
                error(&mut line_errors, "account", "Choose an account.");
            } else if (debit.cents() > 0) == (credit.cents() > 0) {
                error(
                    &mut line_errors,
                    "__all__",
                    "Enter a positive debit OR credit, not both.",
                );
            } else {
                if !deleted { lines.push(JournalLineValues {
                    account: account.into(),
                    debit: debit.to_string(),
                    credit: credit.to_string(),
                    description,
                    name,
                }); }
            }
        }
        for (key, messages) in line_errors {
            errors.insert(format!("{prefix}-{key}"), messages);
        }
    }
    if active_form_errors(&errors, fields) == 0 {
        let command = Command::SaveJournal {
            company_id: c.to_db(),
            journal_id: j.as_ref().map(|j| j.id.to_db()),
            date,
            memo,
            entry_number,
            adjusting: checked(fields, "adjusting"),
            request_key: if j.is_none() { key } else { None },
            expected_revision: if j.is_some() { revision } else { None },
            lines,
        };
        match app.run(command) {
            Ok(Outcome::Done { data }) => {
                return Ok(Outcome::Redirect {
                    location: url("journal", &[&c.to_db(), data["id"].as_str().unwrap()]),
                });
            }
            Err(AppError::Core(FbError::Validation(e))) => {
                error(&mut errors, "__all__", e.join("; "))
            }
            Err(e) => return Err(e),
            other => return Ok(other.unwrap()),
        }
    }
    Ok(Outcome::Invalid {
        html: journal_form(app, route, path, Some(fields), &errors)?.0,
    })
}

fn submit_journal_action(app: &mut App, route: &Route, fields: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(route)?;
    let j = journal_at(app, route)?.unwrap();
    let mut errors = Errors::new();
    let command = if route.name == "journal_post" {
        if j.source_id.is_some() {
            return Ok(Outcome::Invalid {
                html: failure(
                    app,
                    route,
                    vec![
                        "Use the owning source document lifecycle, not generic journal actions."
                            .into(),
                    ],
                )?,
            });
        }
        let revision = clean_revision(fields, &mut errors);
        if revision.is_none() {
            return Ok(Outcome::Invalid{html:failure(app,route,vec!["Open the journal entry again and post it from its page (the revision token is missing).".into()])?});
        }
        Command::PostJournal {
            company_id: c.to_db(),
            journal_id: j.id.to_db(),
            expected_revision: revision.unwrap(),
        }
    } else {
        let date = clean_date(fields, "date", true, &mut errors).unwrap_or_default();
        let reason = clean_text(fields, "reason", 180, true, true, &mut errors);
        if !errors.is_empty() {
            return Ok(Outcome::Invalid {
                html: failure(
                    app,
                    route,
                    vec!["Enter a valid reversal date and reason.".into()],
                )?,
            });
        }
        Command::ReverseJournal {
            company_id: c.to_db(),
            journal_id: j.id.to_db(),
            date,
            reason,
        }
    };
    match app.run(command) {
        Ok(Outcome::Done { data }) => Ok(Outcome::Redirect {
            location: url("journal", &[&c.to_db(), data["id"].as_str().unwrap()]),
        }),
        Err(AppError::Core(FbError::Validation(e))) => Ok(Outcome::Invalid {
            html: failure(app, route, e)?,
        }),
        Err(e) => Err(e),
        other => other,
    }
}

mod audit;
pub use audit::SIM_FIX_ACTION_LABELS;
pub(crate) use audit::export_history;
use audit::{history, journal_page};

#[cfg(test)]
mod deletion_tests {
    use super::*;

    #[test]
    fn deletion_only_excludes_real_formset_rows_not_dynamic_allocation_fields() {
        let p = vec![
            ("lines-0-DELETE".into(), "on".into()),
            ("allocations-1-DELETE".into(), "on".into()),
            ("amount_00000000-0000-DELETE".into(), "on".into()),
        ];
        assert!(deleted_row_error(&p, "lines-0-rate"));
        assert!(deleted_row_error(&p, "allocations-1-amount"));
        let dynamic = "amount_00000000-0000-4000-8000-000000000001";
        assert!(!deleted_row_error(&p, dynamic));
        let e = Errors::from([(dynamic.into(), vec!["Enter a valid amount.".into()])]);
        assert_eq!(active_form_errors(&e, &p), 1);
    }
}
