use super::*;
use crate::commands::BillAllocationValues;
fn record(app: &mut App, c: CompanyId, id: &str) -> AppResult<Value> {
    let mut v=scoped_rows(app,c,"SELECT p.*,i.number FROM payables_billpayment p JOIN documents_documentidentity i ON i.id=p.identity_id WHERE p.company_id=? AND p.id=?",&[&c.to_db(),&id])?.pop().ok_or_else(||AppError::NotFound("payment".into()))?;
    json_columns(&mut v);
    v["pk"] = v["id"].clone();
    v["identity"] = json!({"number":v["number"]});
    v["amount"] = json!(cash(v["amount"].as_i64().unwrap() as i128));
    let bank = app.open_mut()?.book.read(|tx| {
        fb_core::store::account(tx, AccountId::parse(s(&v, "bank_id")).unwrap())
            .map(|a| a.expect("payment bank"))
    })?;
    v["bank"] = json!({"code":bank.code,"name":bank.name});
    let rev = scoped_rows(
        app,
        c,
        "SELECT * FROM payables_paymentreversal WHERE payment_id=?",
        &[&id],
    )?
    .pop();
    v["reversed"] = json!(rev.is_some());
    v["reversal"] = json!(rev);
    v["get_method_display"] = json!(match s(&v, "method") {
        "CHECK" => "Check",
        "CASH" => "Cash",
        "CARD" => "Card (record only)",
        "TRANSFER" => "Transfer (record only)",
        "OTHER" => "Other",
        s => s,
    });
    Ok(v)
}
fn prior(app: &mut App, c: CompanyId, p: Option<Posted<'_>>) -> AppResult<Option<Value>> {
    let key = p
        .and_then(|p| get(p, "request_key"))
        .and_then(py::py_uuid)
        .map(|v| v.0)
        .unwrap_or_default();
    Ok(scoped_rows(app,c,"SELECT p.* FROM payables_billpayment p JOIN documents_documentidentity i ON i.id=p.identity_id WHERE p.company_id=? AND i.request_key=?",&[&c.to_db(),&key])?.pop())
}
fn fields(app: &mut App, c: CompanyId, prior: Option<&Value>) -> AppResult<Vec<Field>> {
    let extra = prior.map(|p| vec![s(p, "bank_id")]).unwrap_or_default();
    let opts = banks(app, c, &extra)?
        .iter()
        .map(|a| (a.id.to_db(), a.label()))
        .collect();
    Ok(vec![Field::new("request_key","Request key",Widget::Hidden).required(),Field::new("review_token","Review token",Widget::Hidden),select("vendor","Vendor","Choose…",contacts(app,c,true,prior.map(|p|s(p,"vendor_id")),"short")?,true),Field::new("date","Payment date",Widget::Date).required(),Field::new("amount","Payment amount",Widget::Text).attr("inputmode","decimal").attr("maxlength","32").required(),Field::new("method","Method",Widget::Select{choices:vec![("CHECK".into(),"Check".into()),("CASH".into(),"Cash".into()),("CARD".into(),"Card (record only)".into()),("TRANSFER".into(),"Transfer (record only)".into()),("OTHER".into(),"Other".into())]}).required(),Field::new("reference","Check no. / reference",Widget::Text).help("When paying by check, this number shares this bank’s check numbers with Write checks and refunds. Do not enter card numbers or secrets."),Field::new("memo","Memo",Widget::Textarea{cols:40,rows:3}),select("bank","Designated bank","Choose a bank",opts,true)])
}
fn allocation_choices(
    app: &mut App,
    c: CompanyId,
    vendor: &str,
    prior: Option<&Value>,
) -> AppResult<Vec<(String, String)>> {
    let actor = app.open_mut()?.owner;
    let mut bills = payables::current_bills(&mut app.open_mut()?.book, actor, c)?;
    let retained = if let Some(p) = prior {
        scoped_rows(
            app,
            c,
            "SELECT bill_id FROM payables_paymentallocation WHERE payment_id=?",
            &[&s(p, "id")],
        )?
    } else {
        vec![]
    };
    bills.retain(|b| {
        b["vendor_id"] == vendor
            && (["OPEN", "PARTIAL"].contains(&s(b, "payment_state"))
                || retained.iter().any(|a| a["bill_id"] == b["id"]))
    });
    bills.sort_by(|a, b| {
        s(&a["current"], "date")
            .cmp(s(&b["current"], "date"))
            .then(s(a, "id").cmp(s(b, "id")))
    });
    Ok(bills
        .iter()
        .map(|b| {
            (
                s(b, "id").into(),
                format!(
                    "{} · outstanding {}",
                    s(b, "number"),
                    b["outstanding"].as_str().unwrap_or("not open")
                ),
            )
        })
        .collect())
}
fn allocation_fields(prefix: &str, opts: &[(String, String)], required: bool) -> Vec<Field> {
    let mut fs = vec![
        select("bill", "Bill", "Choose…", opts.to_vec(), required),
        Field::new("amount", "Amount", Widget::Text)
            .attr("inputmode", "decimal")
            .attr("maxlength", "32"),
        Field::new("DELETE", "Delete", Widget::Checkbox { checked: false }),
    ];
    for f in &mut fs {
        f.prefix = prefix.into();
        if required && f.name == "amount" {
            f.required = true;
        }
    }
    fs
}
#[allow(clippy::too_many_arguments)]
fn form(
    app: &mut App,
    r: &Route,
    query: Posted<'_>,
    path: &str,
    p: Option<Posted<'_>>,
    e: &Errors,
    non_form: &[String],
    review: Option<Value>,
) -> AppResult<String> {
    let c = arg_company(r)?;
    let mut vm = ctx(app, r)?;
    let prior = prior(app, c, p)?;
    let fs = fields(app, c, prior.as_ref())?;
    let bs = banks(
        app,
        c,
        &prior
            .as_ref()
            .map(|p| vec![s(p, "bank_id")])
            .unwrap_or_default(),
    )?;
    let mut next = json!({});
    for b in &bs {
        let actor = app.open_mut()?.owner;
        next[b.id.to_db()] = json!(fb_core::checks::next_number(
            &mut app.open_mut()?.book,
            actor,
            c,
            b.id
        )?);
    }
    let vendor = get(p.unwrap_or(query), "vendor").unwrap_or("");
    let mut initial = json!({"request_key":uuid::Uuid::new_v4().to_string(),"date":(app.today)().to_string(),"method":"CHECK","vendor":vendor});
    if let Some(b) = bs.first() {
        initial["bank"] = json!(b.id.to_db());
        initial["reference"] = next[b.id.to_db()].clone();
    }
    let choices = if let Widget::Select { choices } = &fs[2].widget {
        choices.clone()
    } else {
        vec![]
    };
    put(&mut vm, "vendor_choices", choices);
    put(&mut vm, "vendor_value", vendor);
    put(&mut vm, "form", bind_fields(fs, p, &initial, e, true));
    put(&mut vm, "csrf_token", token(app, path));
    let opts = allocation_choices(app, c, vendor, prior.as_ref())?;
    let total = formset_count(p, "allocations", 0);
    let forms = (0..total)
        .map(|i| {
            bind_fields(
                allocation_fields(&format!("allocations-{i}"), &opts, false),
                p,
                &Value::Null,
                e,
                true,
            )
        })
        .collect();
    put(
        &mut vm,
        "formset",
        FormsetVm {
            management_form: management(p, "allocations", total, 0),
            non_form_errors: formset_errors(non_form),
            total_error_count: super::super::d2::formset_error_count(e, p, "allocations") + non_form.len(),
            forms,
            empty_form: bind_fields(
                allocation_fields("allocations-__prefix__", &opts, false),
                None,
                &Value::Null,
                &Errors::new(),
                true,
            ),
        },
    );
    put(&mut vm, "review", review);
    put(&mut vm, "next_numbers", next);
    draw(app, "payables/payments/form.html", &vm)
}
fn listing(app: &mut App, r: &Route, p: Posted<'_>) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let mut vm = ctx(app, r)?;
    let fs = vec![
        Field::new("q", "Number, reference or vendor", Widget::Text).attr("maxlength", "120"),
        select(
            "vendor",
            "Vendor",
            "Choose…",
            contacts(app, c, false, None, "short")?,
            false,
        ),
        select(
            "state",
            "State",
            "All payments",
            vec![
                ("LIVE".into(), "Recorded".into()),
                ("REVERSED".into(), "Reversed".into()),
            ],
            false,
        ),
    ];
    let mut e = Errors::new();
    let q = clean_text(p, "q", 120, false, true, &mut e);
    let vendor = choice(p, &fs[1], &mut e, true);
    let state = choice(p, &fs[2], &mut e, false);
    let mut records = vec![];
    if e.is_empty() {
        let ids = scoped_rows(
            app,
            c,
            "SELECT id FROM payables_billpayment WHERE company_id=? ORDER BY date DESC,id DESC",
            &[&c.to_db()],
        )?;
        let q = fb_db::collation::upper(&q);
        for id in ids {
            let v = record(app, c, s(&id, "id"))?;
            if (!vendor.is_empty() && v["vendor_id"] != vendor)
                || (!state.is_empty() && (v["reversed"] == true) != (state == "REVERSED"))
                || (!q.is_empty()
                    && ![
                        s(&v, "number"),
                        s(&v, "reference"),
                        s(&v["vendor_snapshot"], "display_name"),
                    ]
                    .iter()
                    .any(|v| fb_db::collation::upper(v).contains(&q)))
            {
                continue;
            }
            records.push(v);
        }
    }
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
    put(&mut vm, "vendor_labels", labels(app, c)?);
    Ok((
        draw(app, "payables/payments/list.html", &vm)?,
        if e.is_empty() { 200 } else { 400 },
    ))
}
fn detail(app: &mut App, r: &Route) -> AppResult<String> {
    let c = arg_company(r)?;
    let v = record(app, c, &r.args["payment_id"])?;
    let mut vm = ctx(app, r)?;
    put(&mut vm, "payment", &v);
    put(
        &mut vm,
        "references",
        references(app, c, s(&v, "identity_id"))?,
    );
    let mut allocations = scoped_rows(
        app,
        c,
        "SELECT a.*,r.number AS revision_number,i.number AS bill_number FROM payables_paymentallocation a JOIN payables_bill b ON b.id=a.bill_id JOIN documents_documentidentity i ON i.id=b.identity_id JOIN payables_billrevision r ON r.id=a.bill_revision_id WHERE a.payment_id=? ORDER BY a.id",
        &[&s(&v, "id")],
    )?;
    for row in &mut allocations {
        row["amount"] = json!(cash(row["amount"].as_i64().unwrap() as i128));
        row["bill"] = json!({"identity":{"number":row["bill_number"]}});
        row["bill_revision"] = json!({"number":row["revision_number"]});
    }
    put(&mut vm, "allocations", allocations);
    let accounts = app
        .open_mut()?
        .book
        .read(|tx| fb_core::store::accounts(tx, c))?;
    put(
        &mut vm,
        "account_labels",
        accounts
            .iter()
            .map(|a| json!({"id":a.id.to_db(),"code":a.code,"name":a.name}))
            .collect::<Vec<_>>(),
    );
    evidence(app, c, &mut vm)?;
    attachments(app, c, s(&v, "identity_id"), &mut vm)?;
    draw(app, "payables/payments/detail.html", &vm)
}
fn reverse_form(
    app: &mut App,
    r: &Route,
    path: &str,
    p: Option<Posted<'_>>,
    e: &Errors,
) -> AppResult<String> {
    let c = arg_company(r)?;
    let v = record(app, c, &r.args["payment_id"])?;
    let mut vm = ctx(app, r)?;
    put(&mut vm, "payment", v);
    put(
        &mut vm,
        "form",
        bind_fields(
            reversal_fields(),
            p,
            &json!({"expected_revision":1,"date":(app.today)().to_string()}),
            e,
            true,
        ),
    );
    put(&mut vm, "csrf_token", token(app, path));
    draw(app, "payables/payments/reverse.html", &vm)
}
pub(super) fn render(
    app: &mut App,
    r: &Route,
    p: Posted<'_>,
    path: &str,
) -> AppResult<(String, u16)> {
    match r.name.as_str() {
        "payments:list" => listing(app, r, p),
        "payments:new" => Ok((form(app, r, p, path, None, &Errors::new(), &[], None)?, 200)),
        "payments:detail" => Ok((detail(app, r)?, 200)),
        "payments:reverse" => Ok((reverse_form(app, r, path, None, &Errors::new())?, 200)),
        _ => Err(AppError::NotFound(r.name.clone())),
    }
}
pub(super) fn submit(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(r)?;
    let mut e = Errors::new();
    if r.name == "payments:reverse" {
        let _ = record(app, c, &r.args["payment_id"])?;
        let (rev, date, reason) = reverse_values(p, &mut e);
        if e.is_empty()
            && result_error(
                app.run(Command::ReverseBillPayment {
                    company_id: c.to_db(),
                    payment_id: r.args["payment_id"].clone(),
                    expected_revision: rev,
                    date,
                    reason,
                }),
                &mut e,
            )?
            .is_some()
        {
            return Ok(Outcome::Redirect {
                location: url("payments:detail", &[&c.to_db(), &r.args["payment_id"]]),
            });
        }
        return Ok(Outcome::Invalid {
            html: reverse_form(app, r, path, Some(p), &e)?,
        });
    }
    if r.name != "payments:new" {
        return Err(AppError::NotFound(path.into()));
    }
    let prior = prior(app, c, Some(p))?;
    let fs = fields(app, c, prior.as_ref())?;
    let key = key(p, true, &mut e);
    let rt = clean_text(p, "review_token", 1000, false, true, &mut e);
    let vendor = choice(p, &fs[2], &mut e, true);
    let date = clean_date(p, "date", true, &mut e).unwrap_or_default();
    let amount = clean_text(p, "amount", 32, true, true, &mut e);
    let method = choice(p, &fs[5], &mut e, false);
    let reference = clean_text(p, "reference", 120, false, true, &mut e);
    super::super::d2::refuse_card_number(p, "reference", &mut e);
    let memo = clean_text(p, "memo", 2000, false, true, &mut e);
    super::super::d2::refuse_card_number(p, "memo", &mut e);
    let bank = choice(p, &fs[8], &mut e, true);
    let opts = allocation_choices(app, c, get(p, "vendor").unwrap_or(""), prior.as_ref())?;
    let count = formset_count(Some(p), "allocations", 0);
    let initial = get(p, "allocations-INITIAL_FORMS")
        .and_then(clean_integer)
        .unwrap_or(0);
    let mut allocations = vec![];
    let mut any_error = false;
    let mut meaningful=0;
    for i in 0..count {
        let prefix = format!("allocations-{i}");
        let row = p
            .iter()
            .filter_map(|(k, v)| {
                k.strip_prefix(&(prefix.clone() + "-"))
                    .map(|k| (k.into(), v.clone()))
            })
            .collect::<Vec<_>>();
        let changed = row.iter().any(|(k, v)| k != "DELETE" && !v.is_empty());
        let deleted = checked(&row, "DELETE");
        if !changed && i >= initial as usize && i >= 1 {
            continue;
        }
        if !deleted && (changed || i < initial.max(0) as usize) {meaningful+=1;}
        let mut le = Errors::new();
        let bill = choice(&row, &allocation_fields("", &opts, true)[0], &mut le, true);
        let amount = clean_text(&row, "amount", 32, true, true, &mut le);
        any_error |= !deleted && !le.is_empty();
        for (k, msg) in le {
            e.insert(format!("{prefix}-{k}"), msg);
        }
        if deleted { continue; }
        allocations.push(BillAllocationValues {
            bill_id: bill,
            amount,
        });
    }
    let non_form = set_errors(p, "allocations", meaningful, any_error);
    let mut review = None;
    if super::super::d2::active_form_errors(&e, p) == 0 && non_form.is_empty() {
        let values = BillPaymentValues {
            request_key: key,
            vendor_id: vendor,
            date,
            amount,
            method,
            reference,
            memo,
            bank_id: bank,
            allocations,
        };
        let action = get(p, "action").unwrap_or("review");
        if action == "record" {
            if let Some(d) = result_error(
                app.run(Command::RecordBillPayment {
                    company_id: c.to_db(),
                    values,
                    review_token: rt,
                }),
                &mut e,
            )? {
                return Ok(Outcome::Redirect {
                    location: url("payments:detail", &[&c.to_db(), s(&d, "id")]),
                });
            }
        } else if action == "review" {
            review = result_error(
                app.run(Command::ReviewBillPayment {
                    company_id: c.to_db(),
                    values,
                }),
                &mut e,
            )?;
        } else {
            error(&mut e, "__all__", "Choose Review or explicitly Record.");
        }
    }
    let mut fields = p.to_vec();
    if let Some(d) = &review {
        fields.retain(|(k, _)| k != "review_token");
        fields.push(("review_token".into(), s(d, "token").into()));
    }
    let status = if review.is_some() { 200 } else { 400 };
    Ok(Outcome::Page {
        html: form(app, r, &[], path, Some(&fields), &e, &non_form, review)?,
        status,
    })
}
