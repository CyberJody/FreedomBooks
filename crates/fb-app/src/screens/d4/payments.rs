use super::doc_forms::KindForm;
use super::*;
const METHODS: &[(&str, &str)] = &[
    ("CHECK", "Check"),
    ("CASH", "Cash"),
    ("CARD", "Card (record only)"),
    ("TRANSFER", "Transfer (record only)"),
    ("OTHER", "Other"),
];
const DESTS: &[(&str, &str)] = &[
    ("BANK", "Direct to designated bank"),
    ("UNDEPOSITED", "Undeposited funds"),
];
fn label(choices: &[(&str, &str)], v: &Value) -> String {
    choices
        .iter()
        .find(|(id, _)| v == id)
        .map(|(_, label)| label.to_string())
        .unwrap_or_default()
}
fn ns(sale: bool) -> &'static str {
    if sale { "sales_receipts" } else { "receipts" }
}
fn id_arg(sale: bool) -> &'static str {
    if sale {
        "sales_receipt_id"
    } else {
        "receipt_id"
    }
}
fn accounts(app: &mut App, c: CompanyId) -> AppResult<Vec<(String, String)>> {
    app.open_mut()?.book.read(|tx|->Result<_,FbError>{let mut st=tx.prepare("SELECT a.id,a.code,a.name FROM ledger_account a JOIN documents_designatedbank d ON d.account_id=a.id JOIN documents_accountingsetup s ON s.id=d.setup_id WHERE s.company_id=?1 ORDER BY a.code,a.id")?;Ok(st.query_map([c.to_db()],|r|Ok((r.get::<_,String>(0)?,format!("{} · {}",r.get::<_,String>(1)?,r.get::<_,String>(2)?))))?.collect::<Result<Vec<_>,_>>()?)}).map_err(AppError::from)
}
fn header(
    app: &mut App,
    c: CompanyId,
    sale: bool,
    selected: Option<&str>,
    p: Option<Posted<'_>>,
    e: &Errors,
) -> AppResult<Vec<Field>> {
    let mut customers = vec![("".into(), "Choose…".into())];
    let prior = choices::prior_receipt(app, c, sale, p)?;
    customers.extend(choices::customers(
        &mut app.open_mut()?.book,
        c,
        false,
        prior.as_ref().and_then(|v| v["customer_id"].as_str()),
        false,
    )?);
    let mut banks = vec![("".into(), "None".into())];
    banks.extend(accounts(app, c)?);
    if let Some(v) = &prior
        && v["destination"] == "BANK"
        && let Some(id) = v["destination_account_id"].as_str()
        && !banks.iter().any(|(key, _)| key == id)
        && let Some(a) = app
            .open_mut()?
            .book
            .read(|tx| fb_core::store::account(tx, AccountId::parse(id).unwrap()))?
    {
        banks.push((id.into(), a.label()));
    }
    let v = |k: &str, initial: Option<String>| super::super::d2::data_value(p, k, initial);
    let mut fs = vec![
        Field::new("request_key", "Request key", Widget::Hidden)
            .required()
            .value_opt(v("request_key", Some(uuid::Uuid::new_v4().to_string()))),
        Field::new("review_token", "Review token", Widget::Hidden)
            .value_opt(v("review_token", None)),
        Field::new(
            "customer",
            "Customer",
            Widget::Select { choices: customers },
        )
        .required()
        .value_opt(v("customer", selected.map(str::to_owned))),
        Field::new(
            "date",
            if sale { "Sale date" } else { "Payment date" },
            Widget::Date,
        )
        .required()
        .value_opt(v("date", Some((app.today)().to_string()))),
    ];
    if !sale {
        fs.push(
            Field::new("amount", "Receipt amount", Widget::Text)
                .required()
                .attr("inputmode", "decimal")
                .attr("maxlength", "32")
                .value_opt(v("amount", None)),
        );
    }
    fs.extend([
        Field::new(
            "method",
            if sale { "Payment method" } else { "Method" },
            Widget::Select {
                choices: METHODS
                    .iter()
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .collect(),
            },
        )
        .required()
        .value_opt(v("method", None)),
        Field::new(
            "reference",
            if sale {
                "Reference (check number or note)"
            } else {
                "Reference"
            },
            Widget::Text,
        )
        .attr("maxlength", "120")
        .help("Reference only. Never enter card numbers, credentials or secrets.")
        .value_opt(v("reference", None)),
        Field::new("memo", "Memo", Widget::Textarea { cols: 40, rows: 3 })
            .attr("maxlength", "2000")
            .value_opt(v("memo", None)),
        Field::new(
            "destination",
            if sale { "Deposit to" } else { "Destination" },
            Widget::Select {
                choices: DESTS
                    .iter()
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .collect(),
            },
        )
        .required()
        .value_opt(v("destination", sale.then(|| "UNDEPOSITED".into()))),
        Field::new(
            "bank",
            "Designated bank (direct-to-bank only)",
            Widget::Select { choices: banks },
        )
        .value_opt(v("bank", None)),
    ]);
    if sale {
        let ch = sale_choices(app, c, prior.as_ref())?;
        fs.push(
            Field::new("tax_mode", "Sales tax", Widget::Select { choices: ch.tax })
                .required()
                .value_opt(v("tax_mode", Some(ch.tax_initial))),
        );
    }
    if sale {
        for f in &mut fs {
            if ["reference", "memo"].contains(&f.name.as_str()) {
                f.attrs.retain(|(k, _)| k != "maxlength");
            }
        }
    }
    Ok(decorate(fs, e))
}
fn invoice_choices(
    app: &mut App,
    c: CompanyId,
    customer: Option<&str>,
    p: Option<Posted<'_>>,
) -> AppResult<Vec<(String, String)>> {
    let prior = choices::prior_receipt(app, c, false, p)?;
    let o = app.open_mut()?;
    let mut bs = fb_core::sales::invoice_balances(&mut o.book, o.owner, c)?;
    bs.retain(|b| {
        b["customer_id"].as_str() == customer
            && (["OPEN", "PARTIAL"].contains(&b["payment_state"].as_str().unwrap_or(""))
                || prior
                    .as_ref()
                    .and_then(|v| v["invoice_ids"].as_array())
                    .is_some_and(|ids| ids.contains(&b["invoice_id"])))
    });
    bs.sort_by(|a, b| {
        a["date"]
            .as_str()
            .cmp(&b["date"].as_str())
            .then(a["invoice_id"].as_str().cmp(&b["invoice_id"].as_str()))
    });
    let mut out = vec![("".into(), "Choose…".into())];
    for b in bs {
        let id = b["invoice_id"].as_str().unwrap().to_owned();
        let number:String=o.book.read(|tx|tx.query_row("SELECT d.number FROM documents_documentidentity d JOIN sales_invoice i ON i.identity_id=d.id WHERE i.id=?1",[&id],|r|r.get(0)))?;
        out.push((
            id,
            format!(
                "{number} · outstanding {}",
                b["outstanding"].as_str().unwrap_or("not open")
            ),
        ));
    }
    Ok(out)
}
fn allocation_fields(
    prefix: &str,
    choices: &[(String, String)],
    p: Option<Posted<'_>>,
    e: &Errors,
) -> Vec<Field> {
    let v = |k: &str| {
        p.and_then(|p| get(p, &format!("{prefix}-{k}")))
            .map(str::to_owned)
    };
    decorate(
        vec![
            Field::new(
                "invoice",
                "Invoice",
                Widget::Select {
                    choices: choices.to_vec(),
                },
            )
            .prefix(prefix)
            .value_opt(v("invoice")),
            Field::new("amount", "Amount", Widget::Text)
                .prefix(prefix)
                .attr("inputmode", "decimal")
                .attr("maxlength", "32")
                .value_opt(v("amount")),
            Field::new(
                "DELETE",
                "Delete",
                Widget::Checkbox {
                    checked: p
                        .is_some_and(|p| super::super::d2::checked(p, &format!("{prefix}-DELETE"))),
                },
            )
            .prefix(prefix),
        ],
        e,
    )
}
fn allocations(
    app: &mut App,
    c: CompanyId,
    customer: Option<&str>,
    p: Option<Posted<'_>>,
    e: &Errors,
    non: &[String],
) -> AppResult<FormsetVm> {
    let choices = invoice_choices(app, c, customer, p)?;
    let total = p
        .map(|p| {
            get(p, "allocations-TOTAL_FORMS")
                .and_then(clean_integer)
                .unwrap_or(0)
                .clamp(0, 50) as usize
        })
        .unwrap_or(3);
    let fs = (0..total)
        .map(|i| {
            forms::bind(
                &allocation_fields(&format!("allocations-{i}"), &choices, p, e),
                &[],
            )
        })
        .collect();
    let management = [
        ("TOTAL_FORMS", p.map(|p|get(p,"allocations-TOTAL_FORMS").unwrap_or("").to_owned()).unwrap_or(total.to_string())),
        ("INITIAL_FORMS", p.map(|p|get(p,"allocations-INITIAL_FORMS").unwrap_or("").to_owned()).unwrap_or("0".into())),
        ("MIN_NUM_FORMS", p.map(|p|get(p,"allocations-MIN_NUM_FORMS").unwrap_or("").to_owned()).unwrap_or("1".into())),
        ("MAX_NUM_FORMS", p.map(|p|get(p,"allocations-MAX_NUM_FORMS").unwrap_or("").to_owned()).unwrap_or("50".into())),
    ];
    let management_form = super::super::d2::management_html("allocations", &management, p.is_some());
    Ok(FormsetVm {
        management_form,
        forms: fs,
        empty_form: forms::bind(
            &allocation_fields("allocations-__prefix__", &choices, None, &Errors::new()),
            &[],
        ),
        non_form_errors: forms::error_list(non, None)
            .replace("errorlist nonfield", "errorlist nonform"),
        total_error_count: super::super::d2::formset_error_count(e, p, "allocations") + non.len(),
    })
}
#[allow(clippy::too_many_arguments)]
fn entry(
    app: &mut App,
    r: &Route,
    path: &str,
    selected: Option<&str>,
    p: Option<Posted<'_>>,
    e: &Errors,
    non: &[String],
    review: Option<&Value>,
) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let sale = r.name.starts_with("sales_receipts:");
    let mut vm = context(app, r)?;
    let fs = header(app, c, sale, selected, p, e)?;
    let customers = fs.iter().find(|f| f.name == "customer").unwrap();
    if let Widget::Select { choices } = &customers.widget {
        put(&mut vm, "customer_choices", choices);
    }
    put(&mut vm, "form", forms::bind(&fs, &nonfield(e, "")));
    put(&mut vm, "csrf_token", token(app, path));
    if sale {
        let prior = choices::prior_receipt(app, c, true, p)?;
        let ch = sale_choices(app, c, prior.as_ref())?;
        put(
            &mut vm,
            "formset",
            super::doc_forms::formset(KindForm::Sale, &ch, p, &[], e, non),
        );
        put(&mut vm, "catalog", ch.catalog);
        put(&mut vm, "customer_tax", ch.customer_tax);
    } else {
        put(
            &mut vm,
            "formset",
            allocations(app, c, selected, p, e, non)?,
        );
    }
    put(&mut vm, "review", review);
    Ok((
        draw(app, &format!("sales/{}/form.html", ns(sale)), &vm)?,
        if p.is_some() && review.is_none() {
            400
        } else {
            200
        },
    ))
}
pub(super) fn render(
    app: &mut App,
    r: &Route,
    p: Posted<'_>,
    path: &str,
) -> AppResult<(String, u16)> {
    let sale = r.name.starts_with("sales_receipts:");
    match r.name.rsplit(':').next().unwrap() {
        "new" => entry(
            app,
            r,
            path,
            get(p, "customer"),
            None,
            &Errors::new(),
            &[],
            None,
        ),
        "list" => listing(app, r, p, sale),
        "detail" => detail(app, r, sale),
        "void" | "reverse" => reverse_page(app, r, path, sale, None, &Errors::new()),
        _ => Err(AppError::NotFound(path.into())),
    }
}
fn load(app: &mut App, r: &Route, sale: bool) -> AppResult<Value> {
    let c = arg_company(r)?;
    let id = &r.args[id_arg(sale)];
    let o = app.open_mut()?;
    let actor = o.owner;
    o.book
        .read(|tx| {
            fb_core::services::access(tx, actor, c)?;
            let mut v = fb_core::sales::receipt(tx, c, id)?;
            if v["kind"] != if sale { "SR" } else { "RCP" } {
                return Err(FbError::missing("Receipt"));
            }
            v["pk"] = v["id"].clone();
            v["identity"] = identity(tx, v["identity_id"].as_str().unwrap())?;
            v["journal_label"] = json!(journal_label(tx, &v["journal_id"])?);
            v["get_method_display"] = json!(label(METHODS, &v["method"]));
            v["get_destination_display"] = json!(label(DESTS, &v["destination"]));
            let raw: Option<String> = tx
                .query_row(
                    "SELECT data FROM sales_receiptreversal WHERE receipt_id=?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()?;
            let mut reversal: Value = raw
                .map(|s| fb_db::parse_unique_json_as(&s).unwrap())
                .unwrap_or(Value::Null);
            if !reversal.is_null() {
                reversal["journal_label"] = json!(journal_label(tx, &reversal["journal_id"])?);
                reversal["created_at"] = json!(stamp(
                    tx,
                    id,
                    if sale {
                        "sales_receipt.voided"
                    } else {
                        "receipt.reversed"
                    }
                )?);
            }
            v["reversed"] = json!(!reversal.is_null());
            v["reversal"] = reversal;
            v["created_at"] = json!(stamp(
                tx,
                id,
                if sale {
                    "sales_receipt.recorded"
                } else {
                    "receipt.recorded"
                }
            )?);
            Ok(v)
        })
        .map_err(AppError::from)
}
fn stamp(tx: &(impl Query + ?Sized), id: &str, action: &str) -> Result<Option<String>, FbError> {
    Ok(tx.query_row("SELECT occurred_at FROM ledger_auditevent WHERE target='receipt:'||?1 AND action=?2 ORDER BY id LIMIT 1",params![id,action],|r|r.get(0)).optional()?)
}
fn deposit(app: &mut App, c: CompanyId, v: &Value) -> AppResult<Option<Value>> {
    if v["destination"] != "UNDEPOSITED" {
        return Ok(None);
    }
    let o = app.open_mut()?;
    let status = o
        .book
        .read(|tx| fb_core::banking::deposit_status(tx, c, &[v["id"].as_str().unwrap().into()]))?;
    let mut found = status.get(v["id"].as_str().unwrap()).cloned();
    if let Some(v) = &mut found {
        v["pk"] = v["id"].clone();
        v["identity"] = o
            .book
            .read(|tx| identity(tx, v["identity_id"].as_str().unwrap()))?;
    }
    Ok(found)
}
fn detail(app: &mut App, r: &Route, sale: bool) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let mut vm = context(app, r)?;
    let v = load(app, r, sale)?;
    put(&mut vm, "deposit", deposit(app, c, &v)?);
    let id = v["id"].as_str().unwrap();
    let (refs, blocked) = app.open_mut()?.book.read(|tx| -> Result<_, FbError> {
        let a = fb_core::store::account(
            tx,
            AccountId::parse(v["destination_account_id"].as_str().unwrap()).unwrap(),
        )?
        .unwrap();
        Ok((
            references(tx, v["identity_id"].as_str().unwrap())?,
            if v["destination"] == "UNDEPOSITED" && v["reversed"] == false && !a.active {
                serde_json::to_value(account_vm(&a)).unwrap()
            } else {
                Value::Null
            },
        ))
    })?;
    put(&mut vm, "references", refs);
    put(&mut vm, "blocked_account", blocked);
    if sale {
        let (mut sale, mut lines) = app.open_mut()?.book.read(|tx| -> Result<_, FbError> {
            let (saleid, raw): (String, String) = tx.query_row(
                "SELECT id,data FROM sales_salesreceipt WHERE receipt_id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let sale: Value = fb_db::parse_unique_json_as(&raw).unwrap();
            let mut st = tx.prepare(
                "SELECT data FROM sales_salesreceiptline WHERE sale_id=?1 ORDER BY position",
            )?;
            let lines = st
                .query_map([saleid], |r| r.get::<_, String>(0))?
                .map(|r| r.map(|s| fb_db::parse_unique_json_as::<Value>(&s).unwrap()))
                .collect::<Result<Vec<_>, _>>()?;
            Ok((sale, lines))
        })?;
        super::fixed_document(&mut sale, &mut lines);
        put(&mut vm, "sale", sale);
        put(&mut vm, "lines", lines);
        let events=app.open_mut()?.book.read(|tx|->Result<Vec<Value>,FbError>{let mut st=tx.prepare("SELECT occurred_at,action,actor_name,detail FROM ledger_auditevent WHERE target='receipt:'||?1 ORDER BY occurred_at,id")?;Ok(st.query_map([id],|r|{let action:String=r.get(1)?;let d:Value=fb_db::parse_unique_json_as(&r.get::<_,String>(3)?).unwrap();let detail=if action=="sales_receipt.recorded"{format!("{} · total {} · {} · {} line(s)",d["number"].as_str().unwrap(),d["amount"].as_str().unwrap(),d["destination"].as_str().unwrap(),d["lines"].as_array().unwrap().len())}else{format!("void dated {} · journal #{} · reason: {}",d["date"].as_str().unwrap(),journal_ordinal(tx,c,d["journal_id"].as_str().unwrap())?,d["reason"].as_str().unwrap())};Ok(json!({"occurred_at":r.get::<_,String>(0)?,"action":action,"actor_name":r.get::<_,String>(2)?,"detail":detail}))})?.collect::<Result<Vec<_>,_>>()?)})?;
        put(&mut vm, "events", events);
    } else {
        let (allocations,labels)=app.open_mut()?.book.read(|tx|->Result<_,FbError>{let mut st=tx.prepare("SELECT a.invoice_id,a.amount,a.receivables_id,i.identity_id,r.number FROM sales_receiptallocation a JOIN sales_invoice i ON i.id=a.invoice_id JOIN sales_invoicerevision r ON r.id=a.invoice_revision_id WHERE a.receipt_id=?1 ORDER BY a.id")?;let raw=st.query_map([id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?)))?.collect::<Result<Vec<_>,_>>()?;let mut out=vec![];let mut labels=vec![];for (id,n,ar,source,rev) in raw{let a=fb_core::store::account(tx,AccountId::parse(&ar).unwrap())?.unwrap();labels.push(json!({"id":ar,"code":a.code,"name":a.name}));out.push(json!({"invoice_id":id,"invoice":{"identity":identity(tx,&source)?},"invoice_revision":{"number":rev},"receivables_id":ar,"amount":cash(i128::from(n))}));}Ok((out,labels))})?;
        put(&mut vm, "allocations", allocations);
        put(&mut vm, "account_labels", labels);
    }
    super::attachments(app, c, v["identity_id"].as_str().unwrap(), &mut vm)?;
    put(&mut vm, "receipt", v);
    Ok((
        draw(app, &format!("sales/{}/detail.html", ns(sale)), &vm)?,
        200,
    ))
}
fn reverse_page(
    app: &mut App,
    r: &Route,
    path: &str,
    sale: bool,
    p: Option<Posted<'_>>,
    e: &Errors,
) -> AppResult<(String, u16)> {
    let mut vm = context(app, r)?;
    let v = load(app, r, sale)?;
    put(&mut vm, "deposit", deposit(app, arg_company(r)?, &v)?);
    put(&mut vm, "receipt", v);
    put(
        &mut vm,
        "form",
        forms::bind(
            &reversal_fields(p, 1, (app.today)(), sale, e),
            &nonfield(e, ""),
        ),
    );
    put(&mut vm, "csrf_token", token(app, path));
    Ok((
        draw(
            app,
            &format!(
                "sales/{}/{}.html",
                ns(sale),
                if sale { "void" } else { "reverse" }
            ),
            &vm,
        )?,
        if p.is_some() { 400 } else { 200 },
    ))
}
fn listing(app: &mut App, r: &Route, p: Posted<'_>, sale: bool) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let mut vm = context(app, r)?;
    let mut e = Errors::new();
    let q = clean_text(p, "q", 120, false, true, &mut e);
    let customer = get(p, "customer").unwrap_or("");
    let state = get(p, "state").unwrap_or("");
    let reversed = if sale { "VOID" } else { "REVERSED" };
    if !["", "LIVE", reversed].contains(&state) {
        error(
            &mut e,
            "state",
            format!("Select a valid choice. {state} is not one of the available choices."),
        );
    }
    let mut ch = vec![("".into(), "Choose…".into())];
    ch.extend(choices::customers(
        &mut app.open_mut()?.book,
        c,
        true,
        None,
        false,
    )?);
    if !ch.iter().any(|(id, _)| id == customer) {
        error(
            &mut e,
            "customer",
            "Select a valid choice. That choice is not one of the available choices.",
        );
    }
    let fs = decorate(
        vec![
            Field::new("q", "Number, reference or customer", Widget::Text)
                .attr("maxlength", "120")
                .value(get(p, "q").unwrap_or("")),
            Field::new("customer", "Customer", Widget::Select { choices: ch }).value(customer),
            Field::new(
                "state",
                "State",
                Widget::Select {
                    choices: vec![
                        (
                            "".into(),
                            format!("All {}", if sale { "sales receipts" } else { "receipts" }),
                        ),
                        ("LIVE".into(), "Recorded".into()),
                        (
                            reversed.into(),
                            if sale {
                                "Voided".into()
                            } else {
                                "Reversed".into()
                            },
                        ),
                    ],
                },
            )
            .value(state),
        ],
        &e,
    );
    let ids=app.open_mut()?.book.read(|tx|{let mut st=tx.prepare("SELECT id FROM sales_receipt WHERE company_id=?1 AND kind=?2 ORDER BY date DESC,id DESC")?;st.query_map(params![c.to_db(),if sale{"SR"}else{"RCP"}],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()})?;
    let mut rows = vec![];
    for id in ids {
        let mut route = Route {
            name: r.name.clone(),
            args: r.args.clone(),
        };
        route.args.insert(id_arg(sale).into(), id);
        let v = load(app, &route, sale)?;
        let hit = q.is_empty()
            || [
                v["identity"]["number"].as_str().unwrap(),
                v["reference"].as_str().unwrap(),
                v["customer_snapshot"]["display_name"].as_str().unwrap(),
            ]
            .iter()
            .any(|s| s.to_lowercase().contains(&q.to_lowercase()));
        if e.is_empty()
            && hit
            && (customer.is_empty() || v["customer_id"] == customer)
            && (state.is_empty() || (v["reversed"] == true) == (state == reversed))
        {
            rows.push(json!({"deposit":deposit(app,c,&v)?,"receipt":v}));
        }
    }
    let page = Page::of(&rows, 25, get(p, "page"));
    put(&mut vm, "rows", &page.object_list);
    put(&mut vm, "page", page);
    put(&mut vm, "filter_form", forms::bind(&fs, &[]));
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
    Ok((
        draw(app, &format!("sales/{}/list.html", ns(sale)), &vm)?,
        if e.is_empty() { 200 } else { 400 },
    ))
}

pub(super) fn submit(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    let sale = r.name.starts_with("sales_receipts:");
    let c = arg_company(r)?;
    let mut e = Errors::new();
    if r.name.ends_with(":void") || r.name.ends_with(":reverse") {
        let revision = clean_revision(p, 1, &mut e);
        let date = clean_date(p, "date", true, &mut e).unwrap_or_default();
        let reason = clean_text(p, "reason", 500, true, true, &mut e);
        if e.is_empty() {
            match app.run(Command::ReverseCustomerReceipt {
                company_id: c.to_db(),
                receipt_id: r.args[id_arg(sale)].clone(),
                sales_receipt: sale,
                expected_revision: revision,
                date,
                reason,
            }) {
                Ok(_) => {
                    return Ok(Outcome::Redirect {
                        location: url(
                            &format!("{}:detail", ns(sale)),
                            &[&c.to_db(), &r.args[id_arg(sale)]],
                        ),
                    });
                }
                Err(AppError::Core(FbError::Validation(messages))) => {
                    error(&mut e, "", messages.join("; "))
                }
                Err(e) => return Err(e),
            }
        }
        return Ok(Outcome::Invalid {
            html: reverse_page(app, r, path, sale, Some(p), &e)?.0,
        });
    }
    let header = header(app, c, sale, None, Some(p), &Errors::new())?;
    let mut values = json!({});
    for f in &header {
        let name = &f.name;
        let s = get(p, name).unwrap_or("");
        let v = if let Widget::Select { choices } = &f.widget {
            if s.is_empty() && f.required {
                error(&mut e, name, "This field is required.");
            } else if !choices.iter().any(|(id, _)| id == s) {
                error(
                    &mut e,
                    name,
                    if ["customer","bank"].contains(&name.as_str()) {"Select a valid choice. That choice is not one of the available choices.".into()}
                    else {format!("Select a valid choice. {s} is not one of the available choices.")},
                );
            }
            s.to_owned()
        } else if name == "date" {
            clean_date(p, name, true, &mut e).unwrap_or_default()
        } else {
            let max = match name.as_str() {
                "reference" => 120,
                "memo" => 2000,
                "review_token" => 1000,
                _ => f
                    .attrs
                    .iter()
                    .find(|(k, _)| k == "maxlength")
                    .and_then(|(_, v)| v.parse().ok())
                    .unwrap_or(usize::MAX),
            };
            let v = clean_text(p, name, max, f.required, true, &mut e);
            if name == "reference" || name == "memo" {
                super::super::d2::refuse_card_number(p, name, &mut e);
            }
            v
        };
        if name == "request_key" && !v.is_empty() && fb_core::py::py_uuid(&v).is_none() {
            error(&mut e, name, "Enter a valid UUID.");
        }
        let key = match name.as_str() {
            "customer" => "customer_id",
            "bank" => "bank_id",
            s => s,
        };
        values[key] = if key == "bank_id" && v.is_empty() {
            Value::Null
        } else {
            json!(v)
        };
    }
    let mut non = vec![];
    if sale {
        let prior = choices::prior_receipt(app, c, true, Some(p))?;
        let ch = sale_choices(app, c, prior.as_ref())?;
        let (lines, n) = super::doc_forms::clean_lines(p, KindForm::Sale, &ch, 0, &mut e);
        non = n;
        values["lines"] = json!(lines);
        let tax = values["tax_mode"].as_str().unwrap_or("").to_owned();
        if let Some(id) = tax.strip_prefix('T') {
            values["tax_item_id"] = json!(id);
            values["tax_mode"] = json!("ITEM");
        }
    } else {
        let choices = invoice_choices(app, c, values["customer_id"].as_str(), Some(p))?;
        let total = get(p, "allocations-TOTAL_FORMS").and_then(clean_integer);
        let initial = get(p, "allocations-INITIAL_FORMS").and_then(clean_integer);
        if total.is_none() || initial.is_none() {
            let names = [("TOTAL_FORMS", total), ("INITIAL_FORMS", initial)]
                .into_iter()
                .filter(|(_, v)| v.is_none())
                .map(|(n, _)| format!("allocations-{n}"))
                .collect::<Vec<_>>()
                .join(", ");
            non.push(format!("ManagementForm data is missing or has been tampered with. Missing fields: {names}. You may need to file a bug report if the issue persists."));
        }
        if total.is_some_and(|n| n > 50) {
            non.push("Please submit at most 50 forms.".into());
        }
        let mut allocations = vec![];
        for i in 0..total.unwrap_or(0).clamp(0, 50) {
            let prefix = format!("allocations-{i}");
            let local = p
                .iter()
                .filter_map(|(k, v)| {
                    k.strip_prefix(&format!("{prefix}-"))
                        .map(|k| (k.into(), v.clone()))
                })
                .collect::<Vec<_>>();
            let deleted = super::super::d2::checked(&local, "DELETE");
            if i >= initial.unwrap_or(0).max(1) && !super::super::d2::extra_row_changed(
                &local, &[("amount", "")], &["invoice"], &["DELETE"],
            ) {
                continue;
            }
            let mut le = Errors::new();
            let id = get(&local, "invoice").unwrap_or("");
            if id.is_empty() {
                error(&mut le, "invoice", "This field is required.");
            } else if !choices.iter().any(|(v, _)| v == id) {
                error(
                    &mut le,
                    "invoice",
                    "Select a valid choice. That choice is not one of the available choices.",
                );
            }
            let amount = clean_text(&local, "amount", 32, true, true, &mut le);
            for (k, v) in le {
                e.insert(format!("{prefix}-{k}"), v);
            }
            if deleted { continue; }
            allocations.push(json!({"invoice_id":id,"amount":amount}));
        }
        if allocations
            .iter()
            .all(|a| a["invoice_id"] == "" && a["amount"] == "")
            && non.is_empty()
        {
            non.push("Please submit at least 1 form.".into());
        }
        values["allocations"] = json!(allocations);
    }
    let review_token = values
        .as_object_mut()
        .unwrap()
        .remove("review_token")
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    let mut review = None;
    if super::super::d2::active_form_errors(&e, p) == 0 && non.is_empty() {
        let values: crate::commands::CustomerReceiptValues =
            serde_json::from_value(values).map_err(|e| AppError::Invalid(e.to_string()))?;
        let action = get(p, "action").unwrap_or("review");
        if action != "review" && action != "record" {
            error(&mut e, "", "Choose Review or explicitly Record.");
        } else {
            let cmd = if action == "review" {
                Command::ReviewCustomerReceipt {
                    company_id: c.to_db(),
                    sales_receipt: sale,
                    values,
                }
            } else {
                Command::RecordCustomerReceipt {
                    company_id: c.to_db(),
                    sales_receipt: sale,
                    values,
                    review_token,
                }
            };
            match app.run(cmd) {
                Ok(Outcome::Done { data }) => {
                    if action == "record" {
                        return Ok(Outcome::Redirect {
                            location: url(
                                &format!("{}:detail", ns(sale)),
                                &[&c.to_db(), data["id"].as_str().unwrap()],
                            ),
                        });
                    }
                    review = Some(data);
                }
                Err(AppError::Core(FbError::Validation(messages))) => {
                    error(&mut e, "", messages.join("; "))
                }
                Err(e) => return Err(e),
                _ => unreachable!(),
            }
        }
    }
    let mut posted = p.to_vec();
    if let Some(review) = &review {
        posted.retain(|(k, _)| k != "review_token");
        posted.push((
            "review_token".into(),
            review["token"].as_str().unwrap().into(),
        ));
    }
    let html = entry(
        app,
        r,
        path,
        get(p, "customer"),
        Some(&posted),
        &e,
        &non,
        review.as_ref().map(|v| &v["review"]),
    )?
    .0;
    Ok(if review.is_some() {
        Outcome::Page { html, status: 200 }
    } else {
        Outcome::Invalid { html }
    })
}

pub(in crate::screens) fn sale_choices(app: &mut App, c: CompanyId, prior: Option<&Value>) -> AppResult<InvoiceChoices> {
    let lines = prior
        .and_then(|v| v["lines"].as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut ch = doc_forms::choices(app, c, KindForm::Sale, None, lines)?;
    let include = prior.and_then(|v| v["sale"]["tax_item_id"].as_str());
    if let Some(include) = include {
        let (settings, items) = app.open_mut()?.book.read(|tx| -> Result<_, FbError> {
            Ok((
                fb_core::salestax::settings_for(tx, c)?,
                fb_core::salestax::list(tx, c)?,
            ))
        })?;
        ch.tax = if settings["charge_tax"] == true {
            vec![
                (String::new(), "Choose sales tax".into()),
                ("NONE".into(), "No tax".into()),
            ]
        } else {
            vec![("NONE".into(), "No tax (sales tax is turned off)".into())]
        };
        for item in items {
            if item["active"] == true || item["id"] == include {
                let label = app
                    .open_mut()?
                    .book
                    .read(|tx| fb_core::salestax::item_label(tx, c, &item))?;
                ch.tax
                    .push((format!("T{}", item["id"].as_str().unwrap()), label));
            }
        }
    }
    Ok(ch)
}

fn journal_ordinal(
    tx: &(impl Query + ?Sized),
    c: CompanyId,
    id: &str,
) -> fb_db::rusqlite::Result<i64> {
    tx.query_row(
        "SELECT count(*) FROM ledger_journal WHERE company_id=?1 AND id<=?2",
        params![c.to_db(), id],
        |r| r.get(0),
    )
}
