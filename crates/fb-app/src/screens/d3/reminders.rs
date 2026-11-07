use super::*;
pub(super) fn render(
    app: &mut App,
    route: &Route,
    posted: Option<Posted<'_>>,
    errors: &Errors,
    conflict: Option<&str>,
    status: u16,
) -> AppResult<(String, u16)> {
    let c = arg_company(route)?;
    let today = (app.today)().to_string();
    let mut vm = ctx(app, route)?;
    let open = app.open_mut()?;
    let staff = open.staff;
    let actor = open.owner;
    let co = open.book.read(|tx| fb_core::store::get_company(tx, c))?;
    let mut built = fb_core::reminders::build(&mut open.book, actor, c, &today, None)?;
    // The service carries stable ids; the desktop displays the approved short ordinal labels.
    for (key, m) in [("bills", Model::Vendor), ("invoices", Model::Customer)] {
        let map = open.book.read(|tx| labels(tx, c, m))?;

        let all = open.book.read(|tx| directory::list(tx, c, m))?;
        for row in built[key]["rows"].as_array_mut().unwrap() {
            let label = row["party"].as_str().unwrap_or("");
            for r in &all {
                let id = r["id"].as_str().unwrap();
                if label.ends_with(&format!("(#{id})")) {
                    row["party"] = map[id].clone();
                    break;
                }
            }
        }
    }
    let through = built["through"].as_str().unwrap();
    let fs = vec![
        Field::new(
            "days",
            "Show bills this many days before they're due",
            Widget::Number,
        )
        .required()
        .attr("min", "0")
        .attr("max", "90")
        .attr("step", "1")
        .attr("class", "reminder-days")
        .value(
            data_value(posted, "days", Some(co.bill_reminder_days.to_string())).unwrap_or_default(),
        ),
        Field::new("expected", "Expected", Widget::Hidden)
            .required()
            .value(
                data_value(posted, "expected", Some(co.bill_reminder_days.to_string()))
                    .unwrap_or_default(),
            ),
    ];
    put(
        &mut vm,
        "links",
        json!({"bills":format!("{}?state=OUTSTANDING&end={through}",url("bills:list",&[&c.to_db()])),"invoices":format!("{}?state=OVERDUE",url("invoices:list",&[&c.to_db()])),"calendar":url("bills:calendar",&[&c.to_db()])}),
    );
    put(&mut vm, "built", built);
    put(&mut vm, "staff", staff);
    put(
        &mut vm,
        "settings_form",
        forms::bind(&decorate(fs, errors), &[]),
    );
    put(
        &mut vm,
        "settings_errors",
        ["days", "expected"]
            .iter()
            .flat_map(|k| errors.get(*k).cloned().unwrap_or_default())
            .collect::<Vec<_>>(),
    );
    put(&mut vm, "conflict", conflict);
    put(
        &mut vm,
        "csrf_token",
        token(app, &url("reminders:settings", &[&c.to_db()])),
    );
    vm["company"]["bill_reminder_days"] = json!(co.bill_reminder_days);
    Ok((draw(app, "reminders/page.html", &vm)?, status))
}
pub(super) fn submit(app: &mut App, route: &Route, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(route)?;
    let mut errors = Errors::new();
    if !app.open_mut()?.staff {
        return Err(AppError::Core(FbError::denied(
            "Only a staff operator can change reminder settings.",
        )));
    }
    let days = integer_clean(p, "days", 0, Some(90), true, &mut errors);
    let expected = integer_clean(p, "expected", 0, Some(90), true, &mut errors);
    if errors.is_empty() {
        match app.run(Command::SaveReminderSettings {
            company_id: c.to_db(),
            days: days.unwrap(),
            expected: expected.unwrap(),
        }) {
            Ok(_) => {
                return Ok(Outcome::Redirect {
                    location: url("reminders:page", &[&c.to_db()]),
                });
            }
            Err(AppError::Core(FbError::Validation(e))) => {
                let (html, status) =
                    render(app, route, None, &Errors::new(), Some(&e.join("; ")), 409)?;
                return Ok(Outcome::Page { html, status });
            }
            Err(e) => return Err(e),
        }
    }
    Ok(Outcome::Invalid {
        html: render(app, route, Some(p), &errors, None, 400)?.0,
    })
}
