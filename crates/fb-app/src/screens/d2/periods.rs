use super::*;
use crate::identity::{Permission, Role};
use fb_core::periods::{self, PeriodChange, PeriodPolicy};

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

fn current(app: &mut App, company: CompanyId) -> AppResult<PeriodPolicy> {
    Ok(app
        .open_mut()?
        .book
        .read(|query| periods::policy(query, company))?)
}

pub(super) fn page(app: &mut App, route: &Route, path: &str) -> AppResult<String> {
    let policy = current(app, arg_company(route)?)?;
    page_with(app, route, path, &policy, None, &Errors::new(), None)
}

fn page_with(
    app: &mut App,
    route: &Route,
    path: &str,
    policy: &PeriodPolicy,
    posted: Option<Posted<'_>>,
    errors: &Errors,
    reopened: Option<String>,
) -> AppResult<String> {
    let c = arg_company(route)?;
    let mut vm = ctx(app, route)?;
    let editable = app.ai.operation_actor(Some(c))?.is_none()
        && app.authenticated_session()?.role == Role::Administrator;
    put(&mut vm, "messages", std::mem::take(&mut app.messages));
    put(&mut vm, "editable", editable);
    put(
        &mut vm,
        "month_name",
        MONTHS[policy.values.fiscal_year_start_month as usize - 1],
    );
    put(&mut vm, "closing_date", &policy.values.closing_date);
    if editable {
        let month = data_value(
            posted,
            "fiscal_year_start_month",
            Some(policy.values.fiscal_year_start_month.to_string()),
        );
        let date = data_value(posted, "closing_date", policy.values.closing_date.clone());
        let reason = data_value(posted, "reason", None);
        let revision = policy.revision.to_string();
        let fields = if reopened.is_some() {
            vec![
                Field::new("fiscal_year_start_month", "", Widget::Hidden).value_opt(month),
                Field::new("closing_date", "", Widget::Hidden).value_opt(date),
                Field::new("reason", "", Widget::Hidden).value_opt(reason.clone()),
            ]
        } else {
            vec![
                Field::new("fiscal_year_start_month", "Fiscal year starts in", Widget::Select {
                    choices: MONTHS.iter().enumerate().map(|(index, name)| ((index + 1).to_string(), (*name).into())).collect(),
                }).required().value_opt(month),
                Field::new("closing_date", "Closing date", Widget::Date).value_opt(date)
                    .help("The books are closed through this date, inclusive. Leave blank to keep every date open."),
                Field::new("reason", "Reason for reopening dates", Widget::Text)
                    .attr("maxlength", "500").value_opt(reason.clone()),
            ]
        };
        put(&mut vm, "csrf_token", token(app, path));
        put(&mut vm, "revision", revision);
        put(
            &mut vm,
            "fields",
            forms::bind(&decorate(fields, errors), &[]).fields,
        );
        put(&mut vm, "errors", nonfield(errors, "__all__"));
        put(&mut vm, "reopened", reopened);
        put(&mut vm, "reason", reason);
    }
    draw(app, "ledger/accounting_periods.html", &vm)
}

pub(super) fn submit(
    app: &mut App,
    route: &Route,
    path: &str,
    fields: Posted<'_>,
) -> AppResult<Outcome> {
    let c = arg_company(route)?;
    if app.ai.operation_actor(Some(c))?.is_some() {
        return Err(AppError::Invalid(
            "Sign in as a company administrator to change accounting periods.".into(),
        ));
    }
    let actor = app.authorize(Some(c), Permission::Administer)?.actor;
    let policy = current(app, c)?;
    let mut errors = Errors::new();
    let revision = clean_revision(fields, &mut errors);
    let month = get(fields, "fiscal_year_start_month")
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|m| (1..=12).contains(m));
    if month.is_none() {
        error(&mut errors, "fiscal_year_start_month", periods::MONTH);
    }
    let closing_date = clean_date(fields, "closing_date", false, &mut errors);
    let reason = clean_text(fields, "reason", 500, false, true, &mut errors);
    let reopened =
        !errors.contains_key("closing_date") && closing_date < policy.values.closing_date;
    if reopened && reason.is_empty() && !errors.contains_key("reason") {
        error(&mut errors, "reason", periods::REASON);
    }
    if revision.is_some_and(|n| n != policy.revision) {
        return stale(app, route, path, &policy);
    }
    if !errors.is_empty() {
        if let Some(messages) = errors.remove("expected_revision") {
            errors.entry("__all__".into()).or_default().extend(messages);
        }
        return Ok(Outcome::Invalid {
            html: page_with(app, route, path, &policy, Some(fields), &errors, None)?,
        });
    }
    let action = get(fields, "action");
    if reopened && action != Some("confirm_reopen") {
        let old = policy
            .values
            .closing_date
            .as_deref()
            .expect("reopening a close");
        let dates = if let Some(date) = &closing_date {
            let first = py::form_date(date)
                .ok()
                .and_then(|d| d.succ_opt())
                .expect("a canonical date earlier than the close");
            if first.to_string() == old {
                old.to_owned()
            } else {
                format!("{first} through {old}")
            }
        } else {
            format!("all dates through {old}")
        };
        return Ok(Outcome::Page {
            html: page_with(
                app,
                route,
                path,
                &policy,
                Some(fields),
                &errors,
                Some(dates),
            )?,
            status: 200,
        });
    }
    let change = PeriodChange {
        fiscal_year_start_month: month.expect("validated month"),
        closing_date,
    };
    let result = periods::set_policy(
        &mut app.open_mut()?.book,
        actor,
        c,
        revision.expect("validated revision"),
        change,
        Some(&reason),
    );
    match result {
        Ok(_) => {
            app.messages.push("Accounting periods saved.".into());
            Ok(Outcome::Redirect {
                location: path.into(),
            })
        }
        Err(FbError::Validation(messages))
            if messages.iter().any(|message| message == periods::STALE) =>
        {
            let updated = current(app, c)?;
            stale(app, route, path, &updated)
        }
        Err(FbError::Validation(messages)) => {
            for message in messages {
                let field = match message.as_str() {
                    periods::MONTH => "fiscal_year_start_month",
                    periods::DATE => "closing_date",
                    periods::REASON => "reason",
                    _ => "__all__",
                };
                error(&mut errors, field, message);
            }
            Ok(Outcome::Invalid {
                html: page_with(app, route, path, &policy, Some(fields), &errors, None)?,
            })
        }
        Err(error) => Err(error.into()),
    }
}

fn stale(app: &mut App, route: &Route, path: &str, policy: &PeriodPolicy) -> AppResult<Outcome> {
    let errors = Errors::from([("__all__".into(), vec![periods::STALE.into()])]);
    Ok(Outcome::Invalid {
        html: page_with(app, route, path, policy, None, &errors, None)?,
    })
}
