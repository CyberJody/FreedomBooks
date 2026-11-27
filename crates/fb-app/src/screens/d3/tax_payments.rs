//! Sales tax pages: what you owe, record a payment to an agency, the payment's page and its void.
//! Every write is a typed command; the review is kept in the session and spent by the record.
use super::*;
use crate::commands::SalesTaxPaymentValues;
use chrono::{Datelike, NaiveDate};
use fb_core::salestax_payments as stp;
use crate::screens::d2::clean_date;

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}

pub(super) fn is_payment_route(name: &str) -> bool {
    matches!(name, "salestax:what_you_owe" | "salestax:record_payment" | "salestax:tax_payment" | "salestax:void_payment")
}

fn page_vm(app: &mut App, route: &Route) -> AppResult<Value> {
    let mut vm = ctx(app, route)?;
    put(&mut vm, "tab", "owed");
    put(&mut vm, "messages", std::mem::take(&mut app.messages));
    Ok(vm)
}
fn read<T>(app: &mut App, c: CompanyId, f: impl FnOnce(&fb_db::Reader<'_>) -> Result<T, FbError>) -> AppResult<T> {
    let open = app.open_mut()?;
    let actor = open.owner;
    Ok(open.book.read(|q| {
        fb_core::services::access(q, actor, c)?;
        f(q)
    })?)
}
fn payment(app: &mut App, c: CompanyId, id: &str) -> AppResult<Value> {
    read(app, c, |q| stp::payment(q, c, id)).map_err(|e| match e {
        AppError::Core(FbError::DoesNotExist(_)) => AppError::NotFound("sales tax payment".into()),
        e => e,
    })
}

/// The filing period containing `today`.
pub(super) fn current_period(today: NaiveDate, frequency: &str) -> (NaiveDate, NaiveDate) {
    let first = |y: i32, m: u32| NaiveDate::from_ymd_opt(y, m, 1).unwrap();
    let months = match frequency {
        "MONTHLY" => 1,
        "ANNUALLY" => 12,
        _ => 3,
    };
    let index = (today.year() * 12 + today.month0() as i32) / months * months;
    let start = first(index.div_euclid(12), index.rem_euclid(12) as u32 + 1);
    let next = index + months;
    let end = first(next.div_euclid(12), next.rem_euclid(12) as u32 + 1).pred_opt().unwrap();
    (start, end)
}

fn owed_page(app: &mut App, route: &Route, p: Posted<'_>) -> AppResult<(String, u16)> {
    let c = arg_company(route)?;
    let mut vm = page_vm(app, route)?;
    let settings = read(app, c, |q| salestax::settings_for(q, c))?;
    let (default_start, default_end) = current_period((app.today)(), s(&settings, "filing_frequency"));
    let mut errors = Errors::new();
    let start = if get(p, "start").is_some() { clean_date(p, "start", true, &mut errors) } else { Some(default_start.to_string()) };
    let end = if get(p, "end").is_some() { clean_date(p, "end", true, &mut errors) } else { Some(default_end.to_string()) };
    if let (Some(a), Some(b)) = (&start, &end)
        && a > b
    {
        error(&mut errors, "end", "The end date must not be before the start date.");
    }
    let fields = vec![
        Field::new("start", "From", Widget::Date).required().value(start.clone().unwrap_or_else(|| get(p, "start").unwrap_or("").into())),
        Field::new("end", "Through", Widget::Date).required().value(end.clone().unwrap_or_else(|| get(p, "end").unwrap_or("").into())),
    ];
    put(&mut vm, "period_form", forms::bind(&decorate(fields, &errors), &nonfield(&errors, "__all__")));
    let status = if errors.is_empty() { 200 } else { 400 };
    let dated = if errors.is_empty() { start.clone().zip(end.clone()) } else { None };
    if errors.is_empty() {
        let (start, end) = (start.unwrap(), end.unwrap());
        let mut report = read(app, c, |q| stp::owed(q, c, Some(&start), &end))?;
        for row in report["agencies"].as_array_mut().unwrap() {
            let mut query = vec![("agency".to_owned(), s(row, "agency_id").to_owned()), ("start".into(), start.clone()), ("end".into(), end.clone())];
            if stp::report_cents(&row["balance_cents"]).unwrap_or(0) > 0 {
                query.push(("amount".into(), s(row, "balance").to_owned()));
            }
            row["pay_url"] = json!(format!("{}?{}", url("salestax:record_payment", &[&c.to_db()]), crate::screens::urlencode(&query)));
        }
        put(&mut vm, "report", report);
        put(&mut vm, "start", &start);
        put(&mut vm, "end", &end);
    }
    let mut payments = read(app, c, |q| stp::payments(q, c))?;
    // Worker item 29: the list under the dates shows the payments made in those dates (dates of the
    // form as shown; the latest 200 when they cannot be read).
    if let Some((start, end)) = &dated {
        payments.retain(|v| (start.as_str()..=end.as_str()).contains(&s(v, "date")));
        put(&mut vm, "payments_dated", true);
    }
    payments.truncate(200);
    put(&mut vm, "payments", payments);
    put(&mut vm, "frequency", match s(&settings, "filing_frequency") { "MONTHLY" => "monthly", "ANNUALLY" => "yearly", _ => "quarterly" });
    put(&mut vm, "has_agencies", !read(app, c, |q| stp::agencies(q, c))?.is_empty());
    Ok((draw(app, "salestax/owed.html", &vm)?, status))
}

type Choices = Vec<(String, String)>;
fn options(app: &mut App, c: CompanyId, keep: &[&str]) -> AppResult<(Choices, Choices, Choices)> {
    read(app, c, |q| {
        let agencies = stp::agencies(q, c)?
            .into_iter()
            .filter(|a| a["active"] == true || keep.contains(&s(a, "id")))
            .map(|a| (s(&a, "id").to_owned(), format!("{}{}", s(&a, "display_name"), if a["active"] == true { "" } else { " (inactive)" })))
            .collect();
        let setup = fb_core::documents::setup_snapshot(q, c)?;
        let mut banks = vec![];
        let mut expenses = vec![];
        for a in fb_core::store::accounts(q, c)? {
            let id = a.id.to_db();
            let designated = setup["banks"].as_array().is_some_and(|b| b.contains(&json!(id)));
            if designated && a.active && a.postable {
                banks.push((id.clone(), a.label()));
            }
            if a.kind.as_str() == "EXPENSE" && a.active && a.postable && fb_core::documents::control_role(q, c, a.id)?.is_none() {
                expenses.push((id, a.label()));
            }
        }
        Ok((agencies, banks, expenses))
    })
}
fn choose(placeholder: &str, opts: Vec<(String, String)>) -> Widget {
    let mut choices = vec![(String::new(), placeholder.to_owned())];
    choices.extend(opts);
    Widget::Select { choices }
}
fn payment_fields(app: &mut App, c: CompanyId, keep: &[&str]) -> AppResult<Vec<Field>> {
    let (agencies, banks, expenses) = options(app, c, keep)?;
    Ok(vec![
        Field::new("request_key", "Request key", Widget::Hidden).required(),
        Field::new("review_token", "Review token", Widget::Hidden),
        Field::new("agency", "Tax agency", choose("Choose the agency…", agencies)).required(),
        Field::new("date", "Payment date", Widget::Date).required(),
        Field::new("bank", "Paid from", choose("Choose a bank account…", banks)).required(),
        Field::new("tax_amount", "Sales tax paid", Widget::Text).required().attr("inputmode", "decimal").attr("placeholder", "0.00").attr("maxlength", "32"),
        Field::new("period_start", "Period covered: from", Widget::Date),
        Field::new("period_end", "Period covered: through", Widget::Date),
        Field::new("reference", "Reference", Widget::Text).attr("maxlength", "40").help("For example, the return or confirmation number."),
        Field::new("penalty_amount", "Penalty and interest", Widget::Text).attr("inputmode", "decimal").attr("placeholder", "0.00").attr("maxlength", "32").help("Only if the payment includes a late penalty or interest. It is recorded as an expense."),
        Field::new("expense_account", "Penalty and interest account", choose("Choose an expense account…", expenses)),
        Field::new("memo", "Memo", Widget::Textarea { cols: 40, rows: 2 }).attr("maxlength", "2000"),
    ])
}
fn payment_form(app: &mut App, route: &Route, path: &str, posted: Option<Posted<'_>>, query: Posted<'_>, errors: &Errors, review: Option<Value>) -> AppResult<String> {
    let c = arg_company(route)?;
    let keep = posted.and_then(|p| get(p, "agency")).map(str::to_owned);
    let mut fs = payment_fields(app, c, &keep.iter().map(String::as_str).collect::<Vec<_>>())?;
    let mut initial = json!({"request_key":uuid::Uuid::new_v4().to_string(),"date":(app.today)().to_string()});
    for (field, param) in [("agency", "agency"), ("tax_amount", "amount"), ("period_start", "start"), ("period_end", "end")] {
        if let Some(v) = get(query, param) {
            initial[field] = json!(v);
        }
    }
    // Worker item 28: the bank account starts as the one the last sales tax payment was paid from (the
    // deposit's rule; with one bank account, that one, as below).
    if let Some(Widget::Select { choices }) = fs.iter().find(|f| f.name == "bank").map(|f| f.widget.clone())
        && let Some(bank) = super::super::d6::default_bank_choice(app, c, &choices, "SELECT bank_id FROM salestax_payment WHERE company_id=? ORDER BY id DESC LIMIT 1")?
    {
        initial["bank"] = json!(bank);
    }
    for f in &mut fs {
        if let Widget::Select { choices } = &f.widget {
            // Preselect when there is only one choice, or keep a valid query choice.
            let wanted = initial[&f.name].as_str().filter(|v| choices.iter().any(|(id, _)| id == v)).map(str::to_owned);
            let only = (choices.len() == 2 && f.required).then(|| choices[1].0.clone());
            initial[&f.name] = json!(wanted.or(only).unwrap_or_default());
        }
        let v = initial[&f.name].as_str().map(str::to_owned);
        f.value = data_value(posted, &f.name, v);
    }
    let mut vm = page_vm(app, route)?;
    let banks = fs[4].clone();
    put(&mut vm, "no_banks", matches!(&banks.widget, Widget::Select { choices } if choices.len() < 2));
    put(&mut vm, "no_agencies", matches!(&fs[2].widget, Widget::Select { choices } if choices.len() < 2));
    put(&mut vm, "form", forms::bind(&decorate(fs, errors), &nonfield(errors, "__all__")));
    put(&mut vm, "csrf_token", token(app, path));
    put(&mut vm, "review", review);
    draw(app, "salestax/payment_form.html", &vm)
}
fn detail(app: &mut App, route: &Route) -> AppResult<String> {
    let c = arg_company(route)?;
    let v = payment(app, c, &route.args["payment_id"])?;
    let mut vm = page_vm(app, route)?;
    put(&mut vm, "payment", v);
    draw(app, "salestax/payment_detail.html", &vm)
}
fn void_form(app: &mut App, route: &Route, path: &str, posted: Option<Posted<'_>>, errors: &Errors) -> AppResult<String> {
    let c = arg_company(route)?;
    let v = payment(app, c, &route.args["payment_id"])?;
    let mut fs = vec![
        Field::new("expected_revision", "Expected revision", Widget::Hidden).required().value("1"),
        // Worker item 10: starts on the payment's own date, like every void.
        Field::new("date", "Void date", Widget::Date).required().value(crate::screens::document_date(app, &v)).help("On or after the payment date. The payment's entry is reversed on this date."),
        Field::new("reason", "Reason", Widget::Textarea { cols: 40, rows: 2 }).required().attr("maxlength", "500"),
    ];
    for f in &mut fs {
        f.value = data_value(posted, &f.name, f.value.clone());
    }
    let mut vm = page_vm(app, route)?;
    put(&mut vm, "payment", v);
    put(&mut vm, "form", forms::bind(&decorate(crate::screens::month_notice(app, fs), errors), &nonfield(errors, "__all__")));
    put(&mut vm, "csrf_token", token(app, path));
    draw(app, "salestax/payment_void.html", &vm)
}

pub(super) fn render(app: &mut App, route: &Route, p: Posted<'_>, path: &str) -> AppResult<(String, u16)> {
    match route.name.as_str() {
        "salestax:what_you_owe" => owed_page(app, route, p),
        "salestax:record_payment" => Ok((payment_form(app, route, path, None, p, &Errors::new(), None)?, 200)),
        "salestax:tax_payment" => Ok((detail(app, route)?, 200)),
        "salestax:void_payment" => {
            let c = arg_company(route)?;
            if payment(app, c, &route.args["payment_id"])?["voided"] == true {
                return Ok((url("salestax:tax_payment", &[&c.to_db(), &route.args["payment_id"]]), 302));
            }
            Ok((void_form(app, route, path, None, &Errors::new())?, 200))
        }
        _ => Err(AppError::NotFound(path.into())),
    }
}

fn core_errors(result: AppResult<Outcome>, errors: &mut Errors) -> AppResult<Option<Value>> {
    match result {
        Ok(Outcome::Done { data }) => Ok(Some(data)),
        Err(AppError::Core(FbError::Validation(messages))) => {
            for m in messages {
                error(errors, "__all__", m);
            }
            Ok(None)
        }
        Err(e) => Err(e),
        Ok(_) => Err(AppError::Invalid("Unexpected result.".into())),
    }
}
fn chosen(p: Posted<'_>, f: &Field, errors: &mut Errors) -> String {
    let v = get(p, &f.name).unwrap_or("").to_owned();
    let Widget::Select { choices } = &f.widget else { return v };
    if v.is_empty() {
        if f.required {
            error(errors, &f.name, REQUIRED);
        }
    } else if !choices.iter().any(|(id, _)| *id == v) {
        error(errors, &f.name, MODEL_CHOICE);
    }
    v
}
pub(super) fn submit(app: &mut App, route: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(route)?;
    let mut errors = Errors::new();
    if route.name == "salestax:void_payment" {
        let id = route.args["payment_id"].clone();
        payment(app, c, &id)?;
        let revision = clean_integer(get(p, "expected_revision").unwrap_or("")).unwrap_or(-1);
        let date = clean_date(p, "date", true, &mut errors).unwrap_or_default();
        let reason = clean_text(p, "reason", 500, true, true, &mut errors);
        if errors.is_empty()
            && core_errors(app.run(Command::VoidSalesTaxPayment { company_id: c.to_db(), payment_id: id.clone(), expected_revision: revision, date, reason }), &mut errors)?.is_some()
        {
            app.messages.push("Payment voided. Its entry was reversed; the amount is owed again.".into());
            return Ok(Outcome::Redirect { location: url("salestax:tax_payment", &[&c.to_db(), &id]) });
        }
        return Ok(Outcome::Invalid { html: void_form(app, route, path, Some(p), &errors)? });
    }
    if route.name != "salestax:record_payment" {
        return Err(AppError::NotFound(path.into()));
    }
    let keep = get(p, "agency").map(str::to_owned);
    let fs = payment_fields(app, c, &keep.iter().map(String::as_str).collect::<Vec<_>>())?;
    let key = match get(p, "request_key").and_then(py::py_uuid) {
        Some(k) => k.0,
        None => {
            error(&mut errors, "__all__", "This form is missing its request key. Open Record payment again.");
            String::new()
        }
    };
    let review_token = clean_text(p, "review_token", 100, false, true, &mut errors);
    let agency = chosen(p, &fs[2], &mut errors);
    let date = clean_date(p, "date", true, &mut errors).unwrap_or_default();
    let bank = chosen(p, &fs[4], &mut errors);
    let tax_amount = clean_text(p, "tax_amount", 32, true, true, &mut errors);
    let period_start = clean_date(p, "period_start", false, &mut errors).unwrap_or_default();
    let period_end = clean_date(p, "period_end", false, &mut errors).unwrap_or_default();
    let reference = clean_text(p, "reference", 40, false, true, &mut errors);
    let penalty_amount = clean_text(p, "penalty_amount", 32, false, true, &mut errors);
    let expense = chosen(p, &fs[10], &mut errors);
    let memo = clean_text(p, "memo", 2000, false, true, &mut errors);
    crate::screens::d2::refuse_card_number(p, "memo", &mut errors);
    crate::screens::d2::refuse_card_number(p, "reference", &mut errors);
    let penalty_given = !penalty_amount.is_empty() && fb_core::services::money_json(&json!(penalty_amount)).is_ok_and(|m| !m.is_zero());
    if penalty_given && expense.is_empty() {
        error(&mut errors, "expense_account", "Choose the expense account for the penalty and interest.");
    }
    let mut review = None;
    if errors.is_empty() {
        let values = SalesTaxPaymentValues {
            request_key: key,
            agency_id: agency,
            date,
            bank_id: bank,
            tax_amount,
            penalty_amount: if penalty_amount.is_empty() { "0".into() } else { penalty_amount },
            expense_account_id: (penalty_given && !expense.is_empty()).then_some(expense),
            period_start,
            period_end,
            reference,
            memo,
        };
        match get(p, "action").unwrap_or("review") {
            "record" => {
                if let Some(d) = core_errors(app.run(Command::RecordSalesTaxPayment { company_id: c.to_db(), values, review_token }), &mut errors)? {
                    app.messages.push(format!("Payment {} recorded.", s(&d, "number")));
                    return Ok(Outcome::Redirect { location: url("salestax:tax_payment", &[&c.to_db(), s(&d, "id")]) });
                }
            }
            "review" => review = core_errors(app.run(Command::ReviewSalesTaxPayment { company_id: c.to_db(), values }), &mut errors)?,
            _ => error(&mut errors, "__all__", "Choose Review or Record payment."),
        }
    }
    let mut fields = p.to_vec();
    fields.retain(|(k, _)| k != "review_token");
    if let Some(d) = &review {
        fields.push(("review_token".into(), s(d, "token").into()));
    }
    let status = if review.is_some() { 200 } else { 400 };
    let html = payment_form(app, route, path, Some(&fields), &[], &errors, review)?;
    Ok(Outcome::Page { html, status })
}

pub(crate) fn run_payment_command(app: &mut App, cmd: Command) -> AppResult<Outcome> {
    let actor = app.open_mut()?.owner;
    let data = match cmd {
        Command::ReviewSalesTaxPayment { company_id, values } => {
            let p = serde_json::to_value(values).expect("typed payment values");
            let r = stp::review_payment(&mut app.open_mut()?.book, actor, crate::session::company(&company_id)?, &p)?;
            let mut d = r.display.clone();
            if app.salestax_reviews.len() >= 256 {
                app.salestax_reviews.clear();
            }
            let token = uuid::Uuid::new_v4().to_string();
            app.salestax_reviews.insert(token.clone(), r);
            d["token"] = json!(token);
            d
        }
        Command::RecordSalesTaxPayment { company_id, values, review_token } => {
            let r = app.salestax_reviews.remove(&review_token).ok_or_else(|| AppError::from(FbError::validation("This review is no longer open. Review the payment again before recording it.")))?;
            let p = serde_json::to_value(values).expect("typed payment values");
            stp::record_payment(&mut app.open_mut()?.book, actor, crate::session::company(&company_id)?, &p, &r)?
        }
        Command::VoidSalesTaxPayment { company_id, payment_id, expected_revision, date, reason } => {
            stp::void_payment(&mut app.open_mut()?.book, actor, crate::session::company(&company_id)?, &payment_id, Some(expected_revision), &json!(date), &json!(reason))?
        }
        _ => return Err(AppError::Invalid("Not a sales tax payment command.".into())),
    };
    Ok(Outcome::Done { data })
}

#[cfg(test)]
mod tests {
    use super::current_period;
    use chrono::NaiveDate;
    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }
    #[test]
    fn the_default_period_is_the_current_filing_period() {
        assert_eq!(current_period(d("2026-11-11"), "QUARTERLY"), (d("2026-10-01"), d("2026-12-31")));
        assert_eq!(current_period(d("2026-01-15"), "QUARTERLY"), (d("2026-01-01"), d("2026-03-31")));
        assert_eq!(current_period(d("2028-02-01"), "MONTHLY"), (d("2028-02-01"), d("2028-02-29")));
        assert_eq!(current_period(d("2026-01-01"), "MONTHLY"), (d("2026-01-01"), d("2026-01-31")));
        assert_eq!(current_period(d("2026-11-11"), "ANNUALLY"), (d("2026-01-01"), d("2026-12-31")));
    }
}
