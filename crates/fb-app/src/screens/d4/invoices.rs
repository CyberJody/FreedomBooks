use super::*;

fn load(app: &mut App, r: &Route) -> AppResult<Value> {
    let c = arg_company(r)?;
    let id = &r.args["invoice_id"];
    let o = app.open_mut()?;
    let actor = o.owner;
    o.book.read(|tx|{fb_core::services::access(tx,actor,c)?;let mut inv=fb_core::sales::invoice(tx,c,id)?;
        inv["pk"]=inv["id"].clone();inv["identity"]=identity(tx,inv["identity_id"].as_str().unwrap())?;
        let mut h=inv["current"].clone();let mut lines=inv["lines"].as_array().unwrap().clone();fixed_document(&mut h,&mut lines);inv["lines"]=json!(lines);display_revision(&mut h);
        h["journal_label"]=json!(journal_label(tx,&h["journal_id"])?);h["reversal_label"]=json!(journal_label(tx,&h["reversal_id"])?);
        let mut st=tx.prepare("SELECT r.data,(SELECT occurred_at FROM ledger_auditevent a WHERE a.target='invoice:'||i.id AND json_extract(a.detail,'$.revision')=r.number ORDER BY a.id LIMIT 1) FROM sales_invoicerevision r JOIN sales_invoice i ON i.id=r.invoice_id WHERE i.id=?1 ORDER BY r.number DESC")?;
        let rows=st.query_map([id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
        let history=rows.into_iter().map(|(raw,stamp)|{let mut v:Value=fb_db::parse_unique_json_as(&raw).map_err(|_|FbError::validation("Invalid stored invoice revision."))?;fixed_document(&mut v,&mut []);display_revision(&mut v);v["created_at"]=json!(stamp);Ok(v)}).collect::<Result<Vec<_>,FbError>>()?;
        h["created_at"]=history.iter().find(|v|v["number"]==h["number"]).map(|v|v["created_at"].clone()).unwrap_or(Value::Null);
        inv["current"]=h;inv["history"]=json!(history);
        inv["identity"]["account_references"]=json!({"all":references(tx,inv["identity_id"].as_str().unwrap())?});
        let link:Option<(String,i64,String)>=tx.query_row("SELECT x.estimate_id,r.number,e.identity_id FROM sales_estimateconversion x JOIN sales_estimaterevision r ON r.id=x.revision_id JOIN sales_estimate e ON e.id=x.estimate_id WHERE x.invoice_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        inv["conversion"]=if let Some((estimate,number,source))=link{json!({"estimate_id":estimate,"estimate":{"identity":identity(tx,&source)?},"revision":{"number":number}})}else{Value::Null};
        Ok::<_,FbError>(inv)
    }).map_err(AppError::from)
}
fn base(app: &mut App, r: &Route) -> AppResult<Value> {
    let mut vm = context(app, r)?;
    let inv = load(app, r)?;
    if app.invoice_delivery {
        let c=arg_company(r)?;let path=format!("/c/{c}/invoices/{}/",r.args["invoice_id"]);
        vm["stripe_csrf"]=json!(token(app,&path));vm["invoice_delivery"]=json!(true);
        let o=app.open_mut()?;vm["stripe_links"]=json!(o.book.read(|q|fb_core::invoice_stripe::links(q,o.owner,c))?.into_iter().filter(|l|l["invoice_id"]==r.args["invoice_id"]).collect::<Vec<_>>());
        vm["stripe_mode"]=json!(o.book.read(|q|fb_core::invoice_preferences::get(q,c))?.settings.stripe_mode);
    }
    let mut current = inv["current"].clone();
    current["terms_label"] = json!(terms_label(&current));
    put(&mut vm, "current", current);
    put(&mut vm, "lines", &inv["lines"]);
    super::attachments(app, arg_company(r)?, inv["identity_id"].as_str().unwrap(), &mut vm)?;
    put(&mut vm, "invoice", inv);
    Ok(vm)
}
/// "Net 30" and the like when the invoice's dates match a standard term; empty otherwise.
fn terms_label(h: &Value) -> &'static str {
    let term = doc_forms::term_for(h["date"].as_str(), h["due_date"].as_str());
    doc_forms::TERMS.iter().find(|(v, _)| !term.is_empty() && *v == term).map(|(_, l)| *l).unwrap_or("")
}
pub(super) fn render(
    app: &mut App,
    r: &Route,
    p: Posted<'_>,
    path: &str,
) -> AppResult<(String, u16)> {
    match r.name.as_str() {
        "invoices:convert" => conversion(app, r, path, None, &Errors::new()),
        "invoices:detail" => detail(app, r, p),
        "invoices:revision" => {
            let mut vm = base(app, r)?;
            let n =
                clean_integer(&r.args["number"]).ok_or_else(|| AppError::NotFound(path.into()))?;
            let mut rev = vm["invoice"]["history"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["number"] == n)
                .cloned()
                .ok_or_else(|| AppError::NotFound(path.into()))?;
            let id = rev["id"].as_str().unwrap().to_owned();
            let mut lines = app.open_mut()?.book.read(|tx| {
                let mut st = tx.prepare(
                    "SELECT data FROM sales_invoiceline WHERE revision_id=?1 ORDER BY position",
                )?;
                st.query_map([id], |r| r.get::<_, String>(0))?
                    .map(|r| r.map(|s| fb_db::parse_unique_json_as::<Value>(&s).unwrap()))
                    .collect::<Result<Vec<_>, _>>()
            })?;
            // Historical revisions have no current balance annotations.
            fixed_document(&mut rev, &mut lines);
            display_revision(&mut rev);
            rev["terms_label"] = json!(terms_label(&rev));
            put(&mut vm, "current", rev);
            put(&mut vm, "lines", lines);
            Ok((draw(app, "sales/invoices/revision.html", &vm)?, 200))
        }
        "invoices:review" => review(app, r, p, path),
        "invoices:void" => void_page(app, r, path, None, &Errors::new()),
        "invoices:edit" => {
            let root = load(app, r)?;
            doc_forms::page(
                app,
                r,
                path,
                doc_forms::KindForm::Invoice,
                Some(&root),
                None,
                &Errors::new(),
                &[],
            )
        }
        "invoices:new" => doc_forms::page_for(
            app,
            r,
            path,
            doc_forms::KindForm::Invoice,
            None,
            None,
            &Errors::new(),
            &[],
            get(p, "customer"),
        ),
        _ => Err(AppError::NotFound(path.into())),
    }
}
fn detail(app: &mut App, r: &Route, p: Posted<'_>) -> AppResult<(String, u16)> {
    let mut vm = base(app, r)?;
    let c = arg_company(r)?;
    let id = &r.args["invoice_id"];
    let today = (app.today)().to_string();
    if let Some(journal) = vm["current"]["journal_id"].as_str() {
        vm["posted_entries"] = json!(posted_entries(app, c, journal)?);
    }
    vm["invoice_notices"] = json!(std::mem::take(&mut app.messages));
    let o = app.open_mut()?;
    let balances = fb_core::sales::invoice_balances(&mut o.book, o.owner, c)?;
    let b = balances.iter().find(|v| v["invoice_id"] == *id).unwrap();
    for (k, v) in b.as_object().unwrap() {
        vm["current"][k] = v.clone();
    }
    vm["invoice_open"] = json!(b["state"] == "OPEN" && money(&b["outstanding"]) > 0);
    vm["invoice_id"] = json!(id);
    let overdue = b["state"] == "OPEN"
        && money(&b["outstanding"]) > 0
        && b["due_date"].as_str().is_some_and(|d| d < today.as_str());
    vm["overdue"] = json!(overdue);
    vm["status"] = status(&vm["current"], overdue);
    // Payments, credits and customer deposits applied to this invoice, newest first, in one list.
    let mut applied = o.book.read(|tx| -> Result<Vec<Value>, FbError> {
        let mut out = vec![];
        let mut st = tx.prepare("SELECT a.amount,r.date,r.id,r.identity_id,EXISTS(SELECT 1 FROM sales_receiptreversal z WHERE z.receipt_id=r.id) FROM sales_receiptallocation a JOIN sales_receipt r ON r.id=a.receipt_id WHERE a.invoice_id=?1 ORDER BY a.id DESC LIMIT 50")?;
        let rows = st.query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, bool>(4)?)))?.collect::<Result<Vec<_>, _>>()?;
        for (n, date, receipt, source, reversed) in rows {
            out.push(json!({"date":date,"kind":"Payment","number":identity(tx,&source)?["number"],"route":"receipts:detail","target":receipt,"live":!reversed,"inactive":"Voided","amount":cash(i128::from(n))}));
        }
        let mut st = tx.prepare("SELECT l.amount,a.date,a.credit_memo_id,m.identity_id,NOT EXISTS(SELECT 1 FROM credits_creditunapplication u WHERE u.application_id=a.id) FROM credits_creditapplicationline l JOIN credits_creditapplication a ON a.id=l.application_id JOIN credits_creditmemo m ON m.id=a.credit_memo_id WHERE l.invoice_id=?1 ORDER BY l.id DESC")?;
        let rows = st.query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, bool>(4)?)))?.collect::<Result<Vec<_>, _>>()?;
        for (n, date, memo, source, live) in rows {
            out.push(json!({"date":date,"kind":"Credit memo","number":identity(tx,&source)?["number"],"route":"credits:detail","target":memo,"live":live,"inactive":"Unapplied","amount":cash(i128::from(n))}));
        }
        let mut st = tx.prepare("SELECT a.amount,a.date,a.deposit_id,r.identity_id,NOT EXISTS(SELECT 1 FROM prepayments_depositunapplication u WHERE u.application_id=a.id) FROM prepayments_depositapplication a JOIN sales_receipt r ON r.id=a.deposit_id WHERE a.invoice_id=?1 ORDER BY a.id DESC LIMIT 50")?;
        let rows = st.query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, bool>(4)?)))?.collect::<Result<Vec<_>, _>>()?;
        for (n, date, deposit, source, live) in rows {
            out.push(json!({"date":date,"kind":"Customer deposit","number":identity(tx,&source)?["number"],"route":"prepayments:detail","target":deposit,"live":live,"inactive":"Unapplied","amount":cash(i128::from(n))}));
        }
        Ok(out)
    })?;
    applied.sort_by(|a, b| b["date"].as_str().cmp(&a["date"].as_str()));
    put(&mut vm, "applied", applied);
    let history = Page::of(
        vm["invoice"]["history"].as_array().unwrap(),
        20,
        get(p, "page"),
    );
    put(&mut vm, "history_page", history);
    if vm["current"]["state"] == "DRAFT" {
        posting_panel(app, r, &mut vm)?;
    }
    vm["email_ready"] = json!(crate::invoice_email::gmail_ready(app, c)? && vm["current"]["customer_snapshot"]["email"].as_str().is_some_and(|e| !e.is_empty()));
    Ok((draw(app, "sales/invoices/detail.html", &vm)?, 200))
}
/// The chip beside an invoice's title.
fn status(h: &Value, overdue: bool) -> Value {
    let (label, class) = match (h["state"].as_str().unwrap_or(""), h["payment_state"].as_str().unwrap_or("")) {
        ("DRAFT", _) => ("Draft", "draft"),
        ("VOID", _) if h["journal_id"].is_null() => ("Deleted draft", "void"),
        ("VOID", _) => ("Voided", "void"),
        (_, "PAID") => ("Paid", "paid"),
        (_, "PARTIAL") if overdue => ("Partly paid · Overdue", "overdue"),
        (_, "PARTIAL") => ("Partly paid", "partial"),
        _ if overdue => ("Overdue", "overdue"),
        _ => ("Open", "open"),
    };
    json!({"label": label, "class": class})
}
/// A draft's Post button and its accounting entries. The page shows the core's review of this exact
/// revision; its opaque token goes in the Post form, so posting refuses anything that changed since.
fn posting_panel(app: &mut App, r: &Route, vm: &mut Value) -> AppResult<()> {
    let c = arg_company(r)?;
    let id = &r.args["invoice_id"];
    let rev = vm["invoice"]["revision"].as_i64().unwrap_or(0);
    let o = app.open_mut()?;
    match fb_core::sales::review_invoice(&mut o.book, o.owner, c, id, Some(rev)) {
        Ok(review) => {
            vm["entries"] = review.display["entries"].clone();
            vm["mapping"] = review.display["mapping"].clone();
            let key = remember(app, review);
            put(vm, "post_form", forms::bind(&post_fields(None, rev, &key, &Errors::new()), &[]));
            put(vm, "post_csrf", token(app, &url("invoices:post", &[&c.to_db(), id])));
        }
        Err(FbError::Validation(messages)) => {
            // Worker items 5 and 17: link straight to the one Accounting setup field a missing
            // default names ("Configure receivables in Accounting setup first.").
            let field = fb_core::documents::ROLES.iter().find(|role| {
                messages.iter().any(|m| *m == format!("Configure {} in Accounting setup first.", role.replace('_', " ")))
            });
            if let Some(role) = field {
                let label = role.replace('_', " ");
                put(vm, "setup_field", json!({"id": format!("id_{role}"), "label": format!("{}{}", label[..1].to_uppercase(), &label[1..])}));
            }
            put(vm, "post_problems", messages)
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
pub(super) fn remember(app: &mut App, review: fb_core::sales::Review) -> String {
    if app.sales_reviews.len() >= 1024 {
        app.sales_reviews.clear();
    }
    let key = uuid::Uuid::new_v4().to_string();
    app.sales_reviews.insert(key.clone(), review);
    key
}
fn post_fields(p: Option<Posted<'_>>, revision: i64, key: &str, e: &Errors) -> Vec<Field> {
    decorate(
        vec![
            Field::new("expected_revision", "Expected revision", Widget::Hidden)
                .required()
                .value_opt(super::super::d2::data_value(
                    p,
                    "expected_revision",
                    Some(revision.to_string()),
                )),
            Field::new("review_token", "Review token", Widget::Hidden)
                .required()
                .value_opt(super::super::d2::data_value(
                    p,
                    "review_token",
                    Some(key.into()),
                )),
        ],
        e,
    )
}
fn review(app: &mut App, r: &Route, p: Posted<'_>, _path: &str) -> AppResult<(String, u16)> {
    let mut vm = base(app, r)?;
    let c = arg_company(r)?;
    let id = &r.args["invoice_id"];
    let rev = get(p, "revision")
        .filter(|s| !s.is_empty() && s.chars().all(|c| fb_money::unicode_decimal(c).is_some()))
        .and_then(clean_integer);
    let result = if let Some(rev) = rev {
        let o = app.open_mut()?;
        fb_core::sales::review_invoice(&mut o.book, o.owner, c, id, Some(rev))
    } else {
        Err(FbError::validation(
            "Open review from the current invoice revision.",
        ))
    };
    match result {
        Ok(review) => {
            for (k, v) in review.display.as_object().unwrap() {
                vm[k] = v.clone();
            }
            let key = remember(app, review);
            put(
                &mut vm,
                "form",
                forms::bind(&post_fields(None, rev.unwrap(), &key, &Errors::new()), &[]),
            );
            put(
                &mut vm,
                "csrf_token",
                token(app, &url("invoices:post", &[&c.to_db(), id])),
            );
            Ok((draw(app, "sales/invoices/review.html", &vm)?, 200))
        }
        Err(FbError::Validation(e)) => {
            put(&mut vm, "errors", e);
            Ok((draw(app, "sales/invoices/review.html", &vm)?, 400))
        }
        Err(e) => Err(e.into()),
    }
}
fn void_page(
    app: &mut App,
    r: &Route,
    path: &str,
    p: Option<Posted<'_>>,
    e: &Errors,
) -> AppResult<(String, u16)> {
    let mut vm = base(app, r)?;
    let rev = vm["invoice"]["revision"].as_i64().unwrap();
    let date = crate::screens::document_date(app, &vm["current"]);
    let mut fields = crate::screens::month_notice(app, reversal_fields(p, rev, date, false, e));
    // A draft never posted, so deleting it needs no date and the reason is optional.
    if vm["current"]["state"] == "DRAFT" {
        fields.retain(|f| f.name != "date");
        for f in &mut fields {
            if f.name == "reason" {
                f.label = "Reason (optional)".into();
                f.required = false;
            }
        }
    }
    put(&mut vm, "form", forms::bind(&fields, &nonfield(e, "")));
    put(&mut vm, "csrf_token", token(app, path));
    Ok((
        draw(app, "sales/invoices/void.html", &vm)?,
        if p.is_some() { 400 } else { 200 },
    ))
}
pub(super) fn submit(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    if r.name == "invoices:convert" {
        let mut e = Errors::new();
        let revision = clean_revision(p, 1, &mut e);
        let date = clean_date(p, "date", true, &mut e).unwrap_or_default();
        let due_date = clean_date(p, "due_date", true, &mut e).unwrap_or_default();
        if !doc_forms::terms_valid(p) {
            error(
                &mut e,
                "terms",
                format!("Select a valid choice. {} is not one of the available choices.",get(p,"terms").unwrap_or("")),
            );
        }
        if e.is_empty() && clean_integer(&r.args["number"]) != Some(revision) {
            error(
                &mut e,
                "",
                "Selected revision does not match this conversion form.",
            );
        }
        if e.is_empty() {
            match app.run(Command::ConvertEstimate {
                company_id: arg_company(r)?.to_db(),
                estimate_id: r.args["estimate_id"].clone(),
                expected_revision: revision,
                date,
                due_date,
            }) {
                Ok(Outcome::Done { data }) => {
                    return Ok(Outcome::Redirect {
                        location: url(
                            "invoices:detail",
                            &[&arg_company(r)?.to_db(), data["id"].as_str().unwrap()],
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
        return Ok(Outcome::Invalid {
            html: conversion(app, r, path, Some(p), &e)?.0,
        });
    }
    if r.name == "invoices:new" || r.name == "invoices:edit" {
        let root = if r.name == "invoices:edit" {
            Some(load(app, r)?)
        } else {
            None
        };
        return doc_forms::submit(app, r, path, doc_forms::KindForm::Invoice, root.as_ref(), p);
    }
    let c = arg_company(r)?;
    let id = &r.args["invoice_id"];
    let mut e = Errors::new();
    let revision = clean_revision(p, 1, &mut e);
    let cmd = match r.name.as_str() {
        "invoices:void" => {
            let current = load(app, r)?["current"].clone();
            let (date, reason) = if current["state"] == "DRAFT" {
                // Deleting a draft records the day it was deleted (never before the invoice date).
                let today = (app.today)().to_string();
                let dated = current["date"].as_str().unwrap_or_default().to_owned();
                let reason = clean_text(p, "reason", 500, false, true, &mut e);
                (today.max(dated), if reason.is_empty() { "No reason given.".into() } else { reason })
            } else {
                (clean_date(p, "date", true, &mut e).unwrap_or_default(), clean_text(p, "reason", 500, true, true, &mut e))
            };
            Command::VoidInvoice {
                company_id: c.to_db(),
                invoice_id: id.clone(),
                expected_revision: revision,
                date,
                reason,
            }
        }
        "invoices:post" => {
            let review_token = clean_text(p, "review_token", 1000, true, true, &mut e);
            Command::PostReviewedInvoice {
                company_id: c.to_db(),
                invoice_id: id.clone(),
                expected_revision: revision,
                review_token,
            }
        }
        _ => return Err(AppError::NotFound(path.into())),
    };
    if e.is_empty() {
        match app.run(cmd) {
            Ok(_) => {
                if r.name == "invoices:post" { app.messages.push("Invoice posted.".into()); }
                // "Post and email" continues to the email preview, where sending is confirmed.
                let next = if r.name == "invoices:post" && get(p, "then") == Some("email") {
                    "invoices:email_preview"
                } else {
                    "invoices:detail"
                };
                return Ok(Outcome::Redirect {
                    location: url(next, &[&c.to_db(), id]),
                });
            }
            Err(AppError::Core(FbError::Validation(messages))) => {
                error(&mut e, "", messages.join("; "))
            }
            Err(err) => return Err(err),
        }
    }
    if r.name == "invoices:void" {
        return Ok(Outcome::Invalid {
            html: void_page(app, r, path, Some(p), &e)?.0,
        });
    }
    // A refused post is explained on the invoice page, which shows a fresh review to post from.
    let reasons = e.values().flatten().map(|m| plain_post(m)).collect::<Vec<_>>();
    app.messages.push(format!("Not posted. {}", reasons.join(" ")));
    let page = Route { name: "invoices:detail".into(), args: r.args.clone() };
    Ok(Outcome::Invalid {
        html: detail(app, &page, &[])?.0,
    })
}
/// Posting refusals in plain words. Unknown messages pass through unchanged.
pub(super) fn plain_post(message: &str) -> String {
    match message {
        "Posting mapping changed. Open a new review before posting." => "The accounts this invoice posts to changed after this page was opened. Check the accounting entries and post again.".into(),
        "Invalid review. Open a new posting review." => "This page is out of date. Check the invoice and post again.".into(),
        "This record changed since you opened it. Reload and review before saving." => "This invoice changed after this page was opened. Check it and post again.".into(),
        m => m.into(),
    }
}

fn conversion(
    app: &mut App,
    r: &Route,
    path: &str,
    p: Option<Posted<'_>>,
    e: &Errors,
) -> AppResult<(String, u16)> {
    let mut vm = context(app, r)?;
    let root = super::estimates::load(app, r)?;
    let number = clean_integer(&r.args["number"]).unwrap_or(0);
    let selected = root["history"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["number"] == number)
        .ok_or_else(|| AppError::NotFound(path.into()))?
        .clone();
    put(&mut vm, "estimate", root);
    put(&mut vm, "selected", selected);
    let today = (app.today)();
    let v = |k: &str, initial: String| super::super::d2::data_value(p, k, Some(initial));
    let fs = decorate(
        vec![
            Field::new("expected_revision", "Expected revision", Widget::Hidden)
                .required()
                .value_opt(v("expected_revision", number.to_string())),
            Field::new("date", "Date", Widget::Date)
                .required()
                .value_opt(v("date", today.to_string())),
            Field::new("due_date", "Due date", Widget::Date)
                .required()
                .value_opt(v(
                    "due_date",
                    (today + chrono::Duration::days(30)).to_string(),
                )),
            doc_forms::terms_with(p, Some("30")),
        ],
        e,
    );
    put(&mut vm, "form", forms::bind(&fs, &nonfield(e, "")));
    put(&mut vm, "csrf_token", token(app, path));
    Ok((
        draw(app, "sales/invoices/convert.html", &vm)?,
        if p.is_some() { 400 } else { 200 },
    ))
}
