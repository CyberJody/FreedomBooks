use super::super::d4::{
    doc_forms::{self, KindForm},
    payments::sale_choices,
};
use super::records::*;
use super::*;
const METHODS: &[(&str, &str)] = &[
    ("CHECK", "Check"),
    ("CASH", "Cash"),
    ("CARD", "Card (record only)"),
    ("TRANSFER", "Transfer (record only)"),
    ("OTHER", "Other"),
];
const REFUNDS: &[(&str, &str)] = &[
    ("CHECK", "Check"),
    ("CASH", "Cash"),
    ("OTHER", "Other (record only)"),
];
fn choices(items: &[(&str, &str)]) -> Widget {
    Widget::Select {
        choices: items
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect(),
    }
}
fn money_field(name: &str, label: &str) -> Field {
    Field::new(name, label, Widget::Text)
        .attr("inputmode", "decimal")
        .attr("maxlength", "32")
        .required()
}
fn keys() -> Vec<Field> {
    vec![
        Field::new("request_key", "Request key", Widget::Hidden).required(),
        Field::new("review_token", "Review token", Widget::Hidden),
    ]
}
fn date_field(label: &str) -> Field {
    Field::new("date", label, Widget::Date).required()
}
fn memo(rows: u32) -> Field {
    Field::new("memo", "Memo", Widget::Textarea { cols: 40, rows })
}
fn credit_line_data(p: Posted<'_>, ch: &InvoiceChoices) -> Vec<(String, String)> {
    let mut cleaned_lines = p.to_vec();
    // CreditLineForm has no editable revision/retention fields. Django ignores
    // those unknown POST keys; they must not make an unused extra row changed.
    cleaned_lines.retain(|(name, _)| {
        !name.starts_with("lines-")
            || !["-origin_position", "-refresh_item", "-keep_archived"]
                .iter()
                .any(|suffix| name.ends_with(suffix))
    });
    for (name, value) in &mut cleaned_lines {
        let opts = if name.starts_with("lines-") && name.ends_with("-item") {
            Some(&ch.items)
        } else if name.starts_with("lines-") && name.ends_with("-income_account") {
            Some(&ch.accounts)
        } else {
            None
        };
        if let Some(opts) = opts {
            let trimmed = py::py_strip(value);
            if !trimmed.is_empty() && opts.iter().any(|(id, _)| id == trimmed) {
                *value = trimmed.into();
            }
        }
    }
    cleaned_lines
}
fn credit_clean_lines(
    p: Posted<'_>,
    ch: &InvoiceChoices,
    e: &mut Errors,
) -> (Vec<InvoiceLineValues>, Vec<String>) {
    let result = doc_forms::clean_lines(p, KindForm::Sale, ch, 0, e);
    // Django's MaxLengthValidator chooses the singular using limit_value, not
    // the entered length. Keep this form-boundary wording out of core services.
    for (name, messages) in e {
        if name.starts_with("lines-") && name.ends_with("-tax_auto") {
            let value = py::py_strip(get(p, name).unwrap_or(""));
            let n = py::py_len(value);
            if n > 1 {
                let original = py::max_length_message(1, value);
                for message in messages {
                    if *message == original {
                        *message =
                            format!("Ensure this value has at most 1 character (it has {n}).");
                    }
                }
            }
        }
    }
    result
}
fn credit_formset(
    ch: &InvoiceChoices,
    p: Option<Posted<'_>>,
    e: &Errors,
    non: &[String],
) -> FormsetVm {
    // Django binds and displays deleted-row field errors but excludes those rows
    // from formset validity, the banner's error count, and the posting payload.
    let mut fs = doc_forms::formset(KindForm::Sale, ch, p, &[], e, non);
    fs.total_error_count = super::super::d2::formset_error_count(e, p, "lines") + non.len();
    fs
}

fn optional(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}
fn prior(
    app: &mut App,
    c: CompanyId,
    r: &Route,
    p: Option<Posted<'_>>,
) -> AppResult<Option<Value>> {
    let k = p
        .and_then(|p| get(p, "request_key"))
        .and_then(py::py_uuid)
        .map(|u| u.0)
        .unwrap_or_default();
    let table = match action(r) {
        "new" => {
            if deposit(r) {
                "sales_receipt"
            } else {
                "credits_creditmemo"
            }
        }
        "apply" | "invoice_apply" => {
            if deposit(r) {
                "prepayments_depositapplication"
            } else {
                "credits_creditapplication"
            }
        }
        "refund" => {
            if deposit(r) {
                "prepayments_depositrefund"
            } else {
                "credits_creditrefund"
            }
        }
        _ => return Ok(None),
    };
    let q = if table == "credits_creditapplication" {
        format!(
            "SELECT r.id FROM {table} r WHERE r.company_id=? AND r.request_key=? AND r.credit_memo_id='{}'",
            r.args[source_arg(r)]
        )
    } else {
        format!(
            "SELECT r.id FROM {table} r JOIN documents_documentidentity i ON i.id=r.identity_id WHERE r.company_id=? AND i.request_key=?"
        )
    };
    let q = if action(r) == "new" && deposit(r) {
        format!("{q} AND r.kind='CDP'")
    } else {
        q
    };
    let found = scoped_rows(app, c, &q, &[&c.to_db(), &k])?.pop();
    if let Some(v) = found {
        let v = match action(r) {
            "new" => source(app, c, s(&v, "id"), deposit(r))?,
            "refund" => refund(app, c, s(&v, "id"), deposit(r))?,
            _ => application(app, c, s(&v, "id"), deposit(r))?,
        };
        if action(r) == "refund"
            && v[if deposit(r) {
                "deposit_id"
            } else {
                "credit_memo_id"
            }] != r.args[source_arg(r)]
        {
            return Ok(None);
        }
        if action(r) == "new" && deposit(r) && v["kind"] != "CDP" {
            return Ok(None);
        }
        Ok(Some(v))
    } else {
        Ok(None)
    }
}
fn bank_options(
    app: &mut App,
    c: CompanyId,
    retained: Option<&str>,
) -> AppResult<Vec<(String, String)>> {
    Ok(banks(app, c, &retained.into_iter().collect::<Vec<_>>())?
        .iter()
        .map(|a| (a.id.to_db(), a.label()))
        .collect())
}
fn credit_choices(app: &mut App, c: CompanyId, prior: Option<&Value>) -> AppResult<InvoiceChoices> {
    let prior = prior.map(|v| json!({"customer_id":v["customer_id"],"lines":v["lines"],"sale":v}));
    sale_choices(app, c, prior.as_ref())
}
fn header(app: &mut App, c: CompanyId, r: &Route, prior: Option<&Value>) -> AppResult<Vec<Field>> {
    let mut fs = keys();
    if action(r) == "new" {
        fs.push(select(
            "customer",
            "Customer",
            "Choose…",
            super::super::d4::customer_choices(
                &mut app.open_mut()?.book,
                c,
                false,
                prior.and_then(|v| v["customer_id"].as_str()),
                false,
            )?,
            true,
        ));
        fs.push(date_field(if deposit(r) {
            "Deposit date"
        } else {
            "Credit date"
        }));
        if !deposit(r) {
            fs.push(memo(3));
            let ch = credit_choices(app, c, prior)?;
            fs.push(
                Field::new("tax_mode", "Sales tax", Widget::Select { choices: ch.tax }).required(),
            );
        } else {
            fs.extend([
                money_field("amount", "Deposit amount"),
                Field::new("method", "Payment method", choices(METHODS)).required(),
                Field::new("reference", "Reference", Widget::Text)
                    .help("Check or reference number only. Never enter card numbers or secrets."),
                Field::new(
                    "destination",
                    "Deposit to",
                    choices(&[
                        ("BANK", "Direct to designated bank"),
                        ("UNDEPOSITED", "Undeposited funds"),
                    ]),
                )
                .required(),
                select(
                    "bank",
                    "Designated bank (direct-to-bank only)",
                    "None",
                    bank_options(
                        app,
                        c,
                        prior
                            .filter(|v| v["destination"] == "BANK")
                            .and_then(|v| v["destination_account_id"].as_str()),
                    )?,
                    false,
                ),
            ]);
            let mut estimates = vec![];
            for e in scoped_rows(
                app,
                c,
                "SELECT e.id,i.number,json_extract(r.data,'$.customer_snapshot.display_name') AS customer FROM sales_estimate e JOIN documents_documentidentity i ON i.id=e.identity_id JOIN sales_estimaterevision r ON r.estimate_id=e.id AND r.number=e.revision WHERE e.company_id=? ORDER BY i.ordinal DESC",
                &[&c.to_db()],
            )? {
                estimates.push((
                    s(&e, "id").into(),
                    format!("{} · {}", s(&e, "number"), s(&e, "customer")),
                ));
            }
            fs.extend([
                select(
                    "estimate",
                    "For estimate (optional)",
                    "No estimate",
                    estimates,
                    false,
                ),
                Field::new(
                    "order_reference",
                    "Order reference (optional)",
                    Widget::Text,
                )
                .help("For example a sales order or job number."),
                memo(2),
            ]);
        }
    } else if action(r) == "refund" {
        fs.extend([date_field("Refund date"),money_field("amount","Refund amount"),select("bank","Pay from (designated bank)","Choose a bank",bank_options(app,c,prior.and_then(|v|v["bank_id"].as_str()))?,true),Field::new("method","Refund method",choices(REFUNDS)).required(),
            Field::new("check_number","Check number (optional, checks only)",Widget::Text).help("Shares this bank’s check numbers with Write checks and bill payments by check. Leave blank for a handwritten or unnumbered check."),memo(2)]);
    }
    Ok(fs)
}
fn eligible(
    app: &mut App,
    c: CompanyId,
    src: &Value,
    prior: Option<&Value>,
    dep: bool,
) -> AppResult<(Vec<Value>, usize)> {
    let o = app.open_mut()?;
    let mut rows = fb_core::sales::invoice_balances(&mut o.book, o.owner, c)?;
    rows.retain(|v| v["customer_id"] == src["customer_id"]);
    rows.sort_by(|a, b| {
        s(a, "date")
            .cmp(s(b, "date"))
            .then(s(a, "invoice_id").cmp(s(b, "invoice_id")))
    });
    let mut result = vec![];
    let mut other = 0;
    for v in rows {
        let old = prior.is_some_and(|p| {
            if dep {
                p["invoice_id"] == v["invoice_id"]
            } else {
                p["lines"]
                    .as_array()
                    .is_some_and(|ls| ls.iter().any(|l| l["invoice_id"] == v["invoice_id"]))
            }
        });
        let mut same = true;
        if !dep {
            same = !scoped_rows(app,c,"SELECT a.account_id FROM documents_documentaccount a JOIN sales_invoice i ON i.identity_id=a.source_id WHERE i.company_id=? AND i.id=? AND a.role='receivables' AND a.account_id=?",&[&c.to_db(),&s(&v,"invoice_id"),&s(src,"receivables_id")])?.is_empty();
            if open_q(&v) && !same {
                other += 1;
            }
        }
        if (open_q(&v) && same) || old {
            result.push(invoice(app, c, s(&v, "invoice_id"))?);
        }
    }
    if !dep {
        result.truncate(50);
    }
    Ok((result, other))
}
fn application_fields(
    app: &mut App,
    c: CompanyId,
    r: &Route,
    src: Option<&Value>,
    inv: Option<&Value>,
    prior: Option<&Value>,
) -> AppResult<(Vec<Field>, Vec<Value>, usize)> {
    let mut fs = keys();
    let mut rows = vec![];
    let mut other = 0;
    if !deposit(r) {
        (rows, other) = eligible(app, c, src.unwrap(), prior, false)?;
        fs.push(date_field("Application date"));
        for v in &rows {
            let mut f = money_field(
                &format!("amount_{}", s(v, "invoice_id")),
                &format!(
                    "Credit to apply to {}",
                    s(&v["invoice"]["identity"], "number")
                ),
            );
            f.required = false;
            fs.push(f);
        }
    } else {
        if let Some(src) = src {
            (rows, other) = eligible(app, c, src, prior, true)?;
            fs.push(select(
                "invoice",
                "Apply to invoice",
                "Choose…",
                rows.iter()
                    .map(|v| {
                        (
                            s(v, "invoice_id").into(),
                            format!(
                                "{} · {} · open balance {}",
                                s(&v["invoice"]["identity"], "number"),
                                s(v, "date"),
                                s(v, "outstanding")
                            ),
                        )
                    })
                    .collect(),
                true,
            ));
        } else {
            let o = app.open_mut()?;
            let mut ds = prepayments::deposit_balances(&mut o.book, o.owner, c)?;
            ds.retain(|v| {
                v["customer_id"] == inv.unwrap()["customer_id"]
                    && (money(&v["remaining"]) > 0
                        || prior.is_some_and(|p| p["deposit_id"] == v["id"]))
            });
            ds.sort_by(|a, b| {
                s(a, "date")
                    .cmp(s(b, "date"))
                    .then(s(a, "id").cmp(s(b, "id")))
            });
            let mut opts = vec![];
            for v in ds {
                let d = source(app, c, s(&v, "id"), true)?;
                opts.push((
                    s(&d, "id").into(),
                    format!(
                        "{} · {} · remaining {}",
                        s(&d["identity"], "number"),
                        s(&d, "date"),
                        s(&d, "remaining")
                    ),
                ));
            }
            fs.push(select("deposit", "Customer deposit", "Choose…", opts, true));
        }
        fs.extend([
            date_field("Application date"),
            money_field("amount", "Amount to apply"),
        ]);
    }
    Ok((fs, rows, other))
}
#[allow(clippy::too_many_arguments)]
pub(super) fn page(
    app: &mut App,
    r: &Route,
    path: &str,
    query: Posted<'_>,
    p: Option<Posted<'_>>,
    e: &Errors,
    non: &[String],
    review: Option<Value>,
) -> AppResult<String> {
    let c = arg_company(r)?;
    let dep = deposit(r);
    let a = action(r);
    let mut vm = ctx(app, r)?;
    let src = if a != "new" && a != "invoice_apply" {
        Some(source(app, c, &r.args[source_arg(r)], dep)?)
    } else {
        None
    };
    let inv = if a == "invoice_apply" {
        Some(invoice(app, c, &r.args["invoice_id"])?)
    } else {
        None
    };
    let pr = prior(app, c, r, p)?;
    let mut initial =
        json!({"request_key":uuid::Uuid::new_v4().to_string(),"date":(app.today)().to_string()});
    if let Some(src) = &src {
        initial["date"] = json!(std::cmp::max(
            (app.today)().to_string(),
            s(src, "date").to_string()
        ));
        initial["amount"] = src[if dep { "remaining" } else { "available" }].clone();
    }
    if let Some(inv) = &inv {
        initial["date"] = json!(std::cmp::max(
            (app.today)().to_string(),
            s(inv, "date").to_string()
        ));
        initial["amount"] = inv["outstanding"].clone();
    }
    initial["method"] = json!("CHECK");
    initial["destination"] = json!("UNDEPOSITED");
    for k in ["customer", "estimate"] {
        if let Some(v) = get(query, k) {
            initial[k] = json!(v);
        }
    }
    let (fs, rows, other) = if a == "apply" || a == "invoice_apply" {
        application_fields(app, c, r, src.as_ref(), inv.as_ref(), pr.as_ref())?
    } else {
        (header(app, c, r, pr.as_ref())?, vec![], 0)
    };
    if a == "new" && !dep {
        let ch = credit_choices(app, c, pr.as_ref())?;
        initial["tax_mode"] = json!(ch.tax_initial);
        put(&mut vm, "formset", credit_formset(&ch, p, e, non));
        put(&mut vm, "catalog", ch.catalog);
        put(&mut vm, "customer_tax", ch.customer_tax);
    }
    let mut form = json!(bind_fields(fs.clone(), p, &initial, e, true));
    if !dep && a == "apply" {
        form["rows"] = json!(
            rows.into_iter()
                .map(
                    |v| json!({"field":form["f"][format!("amount_{}",s(&v,"invoice_id"))],"rev":v})
                )
                .collect::<Vec<_>>()
        );
        put(&mut vm, "other_ar", other);
        put(
            &mut vm,
            "capped",
            form["rows"].as_array().unwrap().len() >= 50,
        );
    }
    put(&mut vm, "form", form);
    put(&mut vm, "csrf_token", token(app, path));
    put(&mut vm, if dep { "deposit" } else { "memo" }, &src);
    put(&mut vm, "invoice", &inv);
    put(
        &mut vm,
        "invoice_choice",
        fs.iter().any(|f| f.name == "invoice"),
    );
    put(
        &mut vm,
        "deposit_choice",
        fs.iter().any(|f| f.name == "deposit"),
    );
    for (name, field) in [
        ("invoice_choices_exist", "invoice"),
        ("deposit_choices_exist", "deposit"),
    ] {
        put(
            &mut vm,
            name,
            fs.iter()
                .find(|f| f.name == field)
                .is_some_and(|f| matches!(&f.widget,Widget::Select{choices} if choices.len()>1)),
        );
    }
    let setup = app
        .open_mut()?
        .book
        .read(|tx| fb_core::documents::setup_snapshot(tx, c))?;
    put(
        &mut vm,
        "setup_ready",
        !setup["customer_deposits"].is_null(),
    );
    put(&mut vm, "review", review);
    draw(
        app,
        &format!(
            "{}/{}.html",
            ns(r),
            match a {
                "new" => "form",
                "refund" => "refund_form",
                _ => "apply",
            }
        ),
        &vm,
    )
}
pub(super) fn reverse_page(
    app: &mut App,
    r: &Route,
    path: &str,
    p: Option<Posted<'_>>,
    e: &Errors,
) -> AppResult<String> {
    let c = arg_company(r)?;
    let a = action(r);
    let mut vm = ctx(app, r)?;
    let value = match a {
        "void" => source(app, c, &r.args[source_arg(r)], deposit(r))?,
        "unapply" => application(app, c, &r.args[application_arg(r)], deposit(r))?,
        _ => refund(app, c, &r.args[refund_arg(r)], deposit(r))?,
    };
    put(
        &mut vm,
        match a {
            "void" => {
                if deposit(r) {
                    "deposit"
                } else {
                    "memo"
                }
            }
            "unapply" => "application",
            _ => "refund",
        },
        value,
    );
    let mut fs = reversal_fields();
    fs[1].label = if a == "unapply" {
        "Unapply date"
    } else if deposit(r) {
        "Void date"
    } else {
        "Reversal date"
    }
    .into();
    put(
        &mut vm,
        "form",
        bind_fields(
            fs,
            p,
            &json!({"expected_revision":1,"date":(app.today)().to_string()}),
            e,
            true,
        ),
    );
    put(&mut vm, "csrf_token", token(app, path));
    draw(app, &format!("{}/{a}.html", ns(r)), &vm)
}
fn clean(p: Posted<'_>, fs: &[Field], e: &mut Errors) -> Value {
    let mut out = json!({});
    for f in fs {
        let k = f.name.as_str();
        let value = match k {
            "request_key" => key(p, true, e),
            "date" => clean_date(p, k, true, e).unwrap_or_default(),
            _ => {
                if matches!(f.widget, Widget::Select { .. }) {
                    choice(
                        p,
                        f,
                        e,
                        k != "method" && k != "destination" && k != "tax_mode",
                    )
                } else {
                    clean_text(
                        p,
                        k,
                        match k {
                            "review_token" => 1000,
                            "memo" => 2000,
                            "reference" | "order_reference" => 120,
                            "check_number" => 20,
                            _ => 32,
                        },
                        f.required,
                        true,
                        e,
                    )
                }
            }
        };
        out[k] = json!(value);
    }
    out
}
pub(super) fn submit(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(r)?;
    let dep = deposit(r);
    let a = action(r);
    let mut e = Errors::new();
    if ["void", "unapply", "refund_void"].contains(&a) {
        let id = &r.args[match a {
            "void" => source_arg(r),
            "unapply" => application_arg(r),
            _ => refund_arg(r),
        }];
        let v = match a {
            "void" => source(app, c, id, dep)?,
            "unapply" => application(app, c, id, dep)?,
            _ => refund(app, c, id, dep)?,
        };
        let (expected_revision, date, reason) = reverse_values(p, &mut e);
        if e.is_empty() {
            let company_id = c.to_db();
            let record_id = id.clone();
            let cmd = match (a, dep) {
                ("void", false) => Command::VoidCreditMemo {
                    company_id,
                    record_id,
                    expected_revision,
                    date,
                    reason,
                },
                ("void", true) => Command::VoidCustomerDeposit {
                    company_id,
                    record_id,
                    expected_revision,
                    date,
                    reason,
                },
                ("unapply", false) => Command::UnapplyCredit {
                    company_id,
                    record_id,
                    expected_revision,
                    date,
                    reason,
                },
                ("unapply", true) => Command::UnapplyDeposit {
                    company_id,
                    record_id,
                    expected_revision,
                    date,
                    reason,
                },
                _ => Command::VoidCustomerRefund {
                    company_id,
                    record_id,
                    deposit: dep,
                    expected_revision,
                    date,
                    reason,
                },
            };
            if result_error(app.run(cmd), &mut e)?.is_some() {
                let id = if a == "unapply" {
                    s(&v, if dep { "deposit_id" } else { "credit_memo_id" })
                } else {
                    id
                };
                return Ok(Outcome::Redirect {
                    location: url(
                        &format!(
                            "{}:{}",
                            ns(r),
                            if a == "refund_void" {
                                "refund_detail"
                            } else {
                                "detail"
                            }
                        ),
                        &[&c.to_db(), id],
                    ),
                });
            }
        }
        return Ok(Outcome::Invalid {
            html: reverse_page(app, r, path, Some(p), &e)?,
        });
    }
    if !["new", "apply", "invoice_apply", "refund"].contains(&a) {
        return Err(AppError::NotFound(path.into()));
    }
    let pr = prior(app, c, r, Some(p))?;
    let src = if a != "new" && a != "invoice_apply" {
        Some(source(app, c, &r.args[source_arg(r)], dep)?)
    } else {
        None
    };
    let inv = if a == "invoice_apply" {
        Some(invoice(app, c, &r.args["invoice_id"])?)
    } else {
        None
    };
    let (fs, rows, _) = if a == "apply" || a == "invoice_apply" {
        application_fields(app, c, r, src.as_ref(), inv.as_ref(), pr.as_ref())?
    } else {
        (header(app, c, r, pr.as_ref())?, vec![], 0)
    };
    let mut values = clean(p, &fs, &mut e);
    if (a == "new" && dep) || a == "refund" {
        for f in fs.iter().filter(|f| ["reference", "memo", "order_reference"].contains(&f.name.as_str())) {
            super::super::d2::refuse_card_number(p, &f.name, &mut e);
        }
    }
    let rt = s(&values, "review_token").to_owned();
    values.as_object_mut().unwrap().remove("review_token");
    let mut non = vec![];
    if a == "new" && !dep {
        let ch = credit_choices(app, c, pr.as_ref())?;
        let cleaned_lines = credit_line_data(p, &ch);
        let (lines, n) = credit_clean_lines(&cleaned_lines, &ch, &mut e);
        non = n;
        // Django validates min_num after the management form and replaces its nonform error.
        if get(p, "lines-TOTAL_FORMS")
            .and_then(super::super::d2::clean_integer)
            .unwrap_or(0)
            <= 0
        {
            non = vec!["Please submit at least 1 form.".into()];
        }
        values["lines"] = json!(lines);
        let mode = s(&values, "tax_mode").to_owned();
        values["tax_item_id"] = json!(mode.strip_prefix('T'));
        values["tax_mode"] = json!(if mode.starts_with('T') {
            "ITEM"
        } else {
            "NONE"
        });
    }
    if a == "new" && dep {
        for name in ["bank", "estimate"] {
            let v = optional(s(&values, name).to_owned());
            values.as_object_mut().unwrap().remove(name);
            values[format!("{name}_id")] = json!(v);
        }
        values["customer_id"] = values["customer"].clone();
        values.as_object_mut().unwrap().remove("customer");
    }
    if a == "new" && !dep {
        values["customer_id"] = values["customer"].clone();
        values.as_object_mut().unwrap().remove("customer");
    }
    if a == "refund" {
        values["bank_id"] = values["bank"].clone();
        values.as_object_mut().unwrap().remove("bank");
    }
    if !dep && a == "apply" {
        let alloc = rows
            .iter()
            .filter_map(|v| {
                let k = format!("amount_{}", s(v, "invoice_id"));
                let amount = s(&values, &k);
                (!amount.is_empty() && !e.contains_key(&k))
                    .then(|| json!({"invoice_id":v["invoice_id"],"amount":amount}))
            })
            .collect::<Vec<_>>();
        // Django Form.clean runs even when a field failed; only cleaned fields participate.
        if alloc.is_empty() {
            error(
                &mut e,
                "__all__",
                "Enter the credit to apply to at least one invoice.",
            );
        }
        let mut d =
            json!({"request_key":values["request_key"],"date":values["date"],"allocations":alloc});
        std::mem::swap(&mut values, &mut d);
    }
    let source_id = if a == "invoice_apply" {
        s(&values, "deposit").to_owned()
    } else {
        src.as_ref()
            .map(|s| s["id"].as_str().unwrap().to_owned())
            .unwrap_or_default()
    };
    if dep && (a == "apply" || a == "invoice_apply") {
        values["invoice_id"] = if let Some(inv) = &inv {
            inv["invoice_id"].clone()
        } else {
            values["invoice"].clone()
        };
        values.as_object_mut().unwrap().remove("invoice");
        values.as_object_mut().unwrap().remove("deposit");
    }
    let mut review = None;
    if super::super::d2::active_form_errors(&e, p) == 0 && non.is_empty() {
        let record = if a == "apply" || a == "invoice_apply" {
            "apply"
        } else {
            "record"
        };
        let selected = get(p, "action").unwrap_or("review");
        if selected != "review" && selected != record {
            error(
                &mut e,
                "__all__",
                if record == "apply" {
                    "Choose Review or explicitly Apply."
                } else {
                    "Choose Review or explicitly Record."
                },
            );
        } else {
            let company_id = c.to_db();
            let review_token = rt;
            let recording = selected == record;
            macro_rules! decode {
                ($t:ty) => {
                    serde_json::from_value::<$t>(values)
                        .map_err(|e| AppError::Invalid(e.to_string()))?
                };
            }
            let cmd = match (a, dep, recording) {
                ("new", false, false) => Command::ReviewCreditMemo {
                    company_id,
                    values: decode!(CreditMemoValues),
                },
                ("new", false, true) => Command::RecordCreditMemo {
                    company_id,
                    values: decode!(CreditMemoValues),
                    review_token,
                },
                ("new", true, false) => Command::ReviewCustomerDeposit {
                    company_id,
                    values: decode!(CustomerDepositValues),
                },
                ("new", true, true) => Command::RecordCustomerDeposit {
                    company_id,
                    values: decode!(CustomerDepositValues),
                    review_token,
                },
                ("apply", false, false) => Command::ReviewCreditApplication {
                    company_id,
                    source_id: source_id.clone(),
                    values: decode!(CreditApplicationValues),
                },
                ("apply", false, true) => Command::RecordCreditApplication {
                    company_id,
                    source_id: source_id.clone(),
                    values: decode!(CreditApplicationValues),
                    review_token,
                },
                ("apply" | "invoice_apply", true, false) => Command::ReviewDepositApplication {
                    company_id,
                    source_id: source_id.clone(),
                    values: decode!(DepositApplicationValues),
                },
                ("apply" | "invoice_apply", true, true) => Command::RecordDepositApplication {
                    company_id,
                    source_id: source_id.clone(),
                    values: decode!(DepositApplicationValues),
                    review_token,
                },
                (_, _, false) => Command::ReviewCustomerRefund {
                    company_id,
                    source_id: source_id.clone(),
                    deposit: dep,
                    values: decode!(CustomerRefundValues),
                },
                (_, _, true) => Command::RecordCustomerRefund {
                    company_id,
                    source_id: source_id.clone(),
                    deposit: dep,
                    values: decode!(CustomerRefundValues),
                    review_token,
                },
            };
            if let Some(mut data) = result_error(app.run(cmd), &mut e)? {
                if recording {
                    let id = if a == "apply" || a == "invoice_apply" {
                        &source_id
                    } else {
                        s(&data, "id")
                    };
                    return Ok(Outcome::Redirect {
                        location: if a == "invoice_apply" {
                            url("invoices:detail", &[&c.to_db(), &r.args["invoice_id"]])
                        } else {
                            url(
                                &format!(
                                    "{}:{}",
                                    ns(r),
                                    if a == "refund" {
                                        "refund_detail"
                                    } else {
                                        "detail"
                                    }
                                ),
                                &[&c.to_db(), id],
                            )
                        },
                    });
                }
                if dep && (a == "apply" || a == "invoice_apply") {
                    data["deposit_number"] =
                        source(app, c, &source_id, true)?["identity"]["number"].clone();
                }
                if dep
                    && a == "new"
                    && let Some(id) = data["data"]["estimate_id"].as_str().map(str::to_owned)
                {
                    let v=scoped_rows(app,c,"SELECT i.number FROM sales_estimate e JOIN documents_documentidentity i ON i.id=e.identity_id WHERE e.company_id=? AND e.id=?",&[&c.to_db(),&id])?.pop();
                    if let Some(v) = v {
                        data["estimate_number"] = v["number"].clone();
                    }
                }
                review = Some(data);
            }
        }
    }
    let mut posted = p.to_vec();
    if let Some(d) = &review {
        posted.retain(|(k, _)| k != "review_token");
        posted.push(("review_token".into(), s(d, "token").into()));
    }
    let html = page(app, r, path, &[], Some(&posted), &e, &non, review.clone())?;
    Ok(if review.is_some() {
        Outcome::Page { html, status: 200 }
    } else {
        Outcome::Invalid { html }
    })
}
