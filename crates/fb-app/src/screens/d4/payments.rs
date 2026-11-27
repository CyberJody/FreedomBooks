use super::doc_forms::KindForm;
use super::*;
const METHODS: &[(&str, &str)] = &[
    ("CHECK", "Check"),
    ("CASH", "Cash"),
    ("CARD", "Card"),
    ("TRANSFER", "Bank transfer"),
    ("OTHER", "Other"),
];
const DESTS: &[(&str, &str)] = &[
    ("UNDEPOSITED", "Undeposited funds"),
    ("BANK", "Bank account"),
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
    (selected, amount): (Option<&str>, Option<&str>),
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
            Field::new("amount", "Amount received", Widget::Text)
                .required()
                .attr("inputmode", "decimal")
                .attr("maxlength", "32")
                .value_opt(v("amount", amount.map(str::to_owned))),
        );
    }
    fs.extend([
        Field::new(
            "method",
            "Payment method",
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
            "Check or reference no.",
            Widget::Text,
        )
        .attr("maxlength", "120")
        .value_opt(v("reference", None)),
        Field::new("memo", "Memo", Widget::Textarea { cols: 40, rows: 3 })
            .attr("maxlength", "2000")
            .value_opt(v("memo", None)),
        Field::new(
            "destination",
            "Deposit to",
            Widget::Select {
                choices: DESTS
                    .iter()
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .collect(),
            },
        )
        .required()
        .value_opt(v("destination", Some("UNDEPOSITED".into()))),
        Field::new(
            "bank",
            "Bank account",
            Widget::Select { choices: banks },
        )
        .value_opt(v("bank", None)),
    ]);
    if sale {
        let ch = sale_choices(app, c, prior.as_ref())?;
        let selected = selected.filter(|id| ch.customers.iter().any(|(choice, _)| choice == id));
        let (_, tax) = ch.customer_defaults(selected);
        let initial = tax.to_owned();
        fs.push(
            Field::new("tax_mode", "Sales tax", Widget::Select { choices: ch.tax })
                .required()
                .value_opt(v("tax_mode", Some(initial))),
        );
        fs.push(Field::new("tax_header_auto", "", Widget::Hidden)
            .value_opt(v("tax_header_auto", Some("1".into()))));
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
/// The open invoices a payment can be applied to, oldest first, for every customer. A retried
/// form also keeps the invoices its earlier, already recorded payment used.
fn open_invoices(app: &mut App, c: CompanyId, p: Option<Posted<'_>>) -> AppResult<Vec<Value>> {
    let prior = choices::prior_receipt(app, c, false, p)?;
    let kept = prior
        .as_ref()
        .and_then(|v| v["invoice_ids"].as_array())
        .cloned()
        .unwrap_or_default();
    let today = (app.today)().to_string();
    let o = app.open_mut()?;
    let mut bs = fb_core::sales::invoice_balances(&mut o.book, o.owner, c)?;
    bs.retain(|b| {
        ["OPEN", "PARTIAL"].contains(&b["payment_state"].as_str().unwrap_or(""))
            || kept.contains(&b["invoice_id"])
    });
    bs.sort_by(|a, b| {
        a["date"]
            .as_str()
            .cmp(&b["date"].as_str())
            .then(a["due_date"].as_str().cmp(&b["due_date"].as_str()))
            .then(a["invoice_id"].as_str().cmp(&b["invoice_id"].as_str()))
    });
    o.book
        .read(|tx| -> Result<Vec<Value>, FbError> {
            bs.into_iter()
                .map(|b| {
                    let id = b["invoice_id"].as_str().unwrap_or_default();
                    let (number, receivables): (String, Option<String>) = tx.query_row(
                        "SELECT d.number,(SELECT a.code||' · '||a.name FROM documents_documentaccount x JOIN ledger_account a ON a.id=x.account_id WHERE x.source_id=i.identity_id AND x.role='receivables') FROM sales_invoice i JOIN documents_displayidentity d ON d.id=i.identity_id WHERE i.id=?1",
                        [id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )?;
                    let overdue = b["due_date"].as_str().is_some_and(|d| d < today.as_str());
                    Ok(json!({"invoice_id":id,"customer_id":b["customer_id"],"number":number,"date":b["date"],"due_date":b["due_date"],"total":b["total"],"outstanding":b["outstanding"].as_str().unwrap_or("0.00"),"overdue":overdue,"receivables":receivables.unwrap_or_default()}))
                })
                .collect()
        })
        .map_err(AppError::from)
}
/// The posted pay box and amount for each invoice row, keyed by invoice.
fn posted_rows(p: Posted<'_>) -> Vec<(String, bool, String, usize)> {
    let total = get(p, "allocations-TOTAL_FORMS")
        .and_then(clean_integer)
        .unwrap_or(0)
        .clamp(0, MAX_ROWS);
    (0..total as usize)
        .map(|i| {
            let k = |name: &str| format!("allocations-{i}-{name}");
            (
                get(p, &k("invoice")).unwrap_or("").to_owned(),
                super::super::d2::checked(p, &k("pay")),
                fb_core::py::py_strip(get(p, &k("amount")).unwrap_or("")).to_owned(),
                i,
            )
        })
        .collect()
}
/// Worker item 39: row `i`'s `allocations-i-chosen` mark, posted by the page's script: "1" when the person
/// chose that invoice, "0" when the page filled it. Absent without the script, which fills nothing itself.
fn chosen_mark(p: Posted<'_>, i: usize) -> Option<&'static str> {
    match get(p, &format!("allocations-{i}-chosen")) {
        Some("1") => Some("1"),
        Some("0") => Some("0"),
        _ => None,
    }
}
/// "A", "A and B", "A, B and C".
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}
/// Rows past this are refused as tampering; the core separately limits one payment to 50 invoices.
const MAX_ROWS: i64 = 1000;
/// Core refusals for a payment, said the way a bookkeeper would. Unknown messages pass through.
fn plain(message: &str) -> String {
    match message {
        "Explicit allocations must equal the receipt amount exactly; no on-account credit." => "The amounts applied to invoices must add up to the amount received. FreedomBooks does not keep part of a payment as unapplied credit; if the customer paid before you invoiced, record a customer deposit instead.".into(),
        "Allocation exceeds the actual outstanding invoice amount. Review again." => "An amount is more than that invoice's open balance. Lower it to the open balance or less.".into(),
        "Payment date cannot precede any allocated invoice date." => "The payment date is before the date of an invoice you are applying it to. Change the payment date or untick that invoice.".into(),
        "Receipt and allocation amounts must be positive." => "Enter amounts greater than zero.".into(),
        "Enter 1–50 explicit allocations." => "Apply the payment to between 1 and 50 invoices. For more, record it as two payments.".into(),
        "Allocate to each invoice only once." => "Each invoice can appear only once in a payment.".into(),
        "Allocate only to this customer’s posted, unreversed invoices." => "One of the invoices is no longer open for this customer. Reload the page to see the current open invoices.".into(),
        "Configure an explicit undeposited funds destination first." => "Choose your undeposited funds account in Accounting setup first, or deposit this payment straight to a bank account.".into(),
        "Choose a designated bank account in Accounting setup." => "Choose a bank account. Bank accounts are listed in Accounting setup.".into(),
        "Do not select a bank for undeposited funds." => "Clear the bank account, or choose Deposit to: Bank account.".into(),
        "Choose an active customer in this company." => "This customer is archived. Reactivate the customer before recording a payment.".into(),
        "Destination must differ from the invoices’ original receivables accounts." => "The deposit account cannot be the invoices' receivables account. Choose a different account.".into(),
        m => m.into(),
    }
}
/// The sales receipt form: the invoice editor's header, line grid and totals, with the entered
/// values and any problems when a Save was refused.
#[allow(clippy::too_many_arguments)]
fn entry(
    app: &mut App,
    r: &Route,
    path: &str,
    selected: Option<&str>,
    p: Option<Posted<'_>>,
    e: &Errors,
    non: &[String],
    problems: &[String],
) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let mut vm = context(app, r)?;
    let fs = header(app, c, true, (selected, None), p, e)?;
    put(&mut vm, "form", forms::bind(&fs, &nonfield(e, "")));
    put(&mut vm, "csrf_token", token(app, path));
    let prior = choices::prior_receipt(app, c, true, p)?;
    let ch = sale_choices(app, c, prior.as_ref())?;
    put(
        &mut vm,
        "formset",
        super::doc_forms::formset(KindForm::Sale, &ch, p, &[], e, non),
    );
    let (customers, rates) = super::doc_forms::editor_data(app, c, None)?;
    put(&mut vm, "invoice_customers", customers);
    put(&mut vm, "invoice_tax_rates", rates);
    put(&mut vm, "posting_accounts", super::doc_forms::posting_accounts(app, c, "SR", &ch)?);
    put(&mut vm, "item_count", ch.catalog.as_object().map_or(0, |m| m.len()));
    put(&mut vm, "catalog", ch.catalog);
    put(&mut vm, "company_tax_default", &ch.company_tax_default);
    put(&mut vm, "customer_tax", ch.customer_tax);
    put(&mut vm, "problems", problems);
    put(&mut vm, "setup_hint", problems.iter().any(|m| m.contains("Accounting setup")));
    put(&mut vm, "has_errors", super::super::d2::active_form_errors(e, p.unwrap_or(&[])) > 0 || !non.is_empty() || !problems.is_empty());
    put(&mut vm, "saved_notices", std::mem::take(&mut app.messages));
    Ok((
        draw(app, "sales/sales_receipts/form.html", &vm)?,
        if p.is_some() { 400 } else { 200 },
    ))
}
/// Receive payment: one customer field, that customer's open invoices with a Pay box and an amount
/// each, and one Save. `query` is the GET query (`customer`, `invoice`); `p` a refused submission;
/// `extra` the warning and its confirmation for money the page put on invoices nobody chose (item 39);
/// `fee_warning` the same for a positive fee line on an expense account (worker review 3b M1).
#[allow(clippy::too_many_arguments)]
fn receive_page(
    app: &mut App,
    r: &Route,
    path: &str,
    query: Posted<'_>,
    p: Option<Posted<'_>>,
    e: &Errors,
    non: &[String],
    extra: Option<(String, String)>,
    fee_warning: Option<(String, String)>,
) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let mut vm = context(app, r)?;
    let rows = open_invoices(app, c, p)?;
    // Arriving from an invoice preselects its customer, ticks it and fills its balance.
    let from_invoice = if p.is_none() {
        get(query, "invoice").and_then(|id| rows.iter().find(|r| r["invoice_id"] == id))
    } else {
        None
    };
    let customer = match (p, from_invoice) {
        (Some(p), _) => get(p, "customer").map(str::to_owned),
        (None, Some(row)) => row["customer_id"].as_str().map(str::to_owned),
        (None, None) => get(query, "customer").map(str::to_owned),
    };
    let amount = from_invoice.and_then(|row| row["outstanding"].as_str());
    let fs = header(app, c, false, (customer.as_deref(), amount), p, e)?;
    let posted = p.map(posted_rows).unwrap_or_default();
    let mut shown = vec![];
    for row in rows.iter().filter(|r| r["customer_id"].as_str() == customer.as_deref()) {
        let id = row["invoice_id"].as_str().unwrap_or_default();
        let mut row = row.clone();
        // Worker item 39: whether the person chose this invoice ("1": ticked, typed or the invoice the page
        // was opened from; "0": the page filled or the person unticked it), kept through a refused save.
        let (pay, apply, chosen) = match posted.iter().find(|(invoice, ..)| invoice == id) {
            Some((_, pay, amount, i)) => {
                (*pay || !amount.is_empty(), amount.clone(), p.and_then(|p| chosen_mark(p, *i)).unwrap_or(""))
            }
            None if from_invoice.is_some_and(|f| f["invoice_id"] == id) => {
                (true, row["outstanding"].as_str().unwrap_or_default().to_owned(), "1")
            }
            None => (false, String::new(), ""),
        };
        row["index"] = json!(shown.len());
        row["pay"] = json!(pay);
        row["apply"] = json!(apply);
        row["chosen"] = json!(chosen);
        row["errors"] = json!(forms::error_list(
            e.get(&format!("invoice-{id}")).map(Vec::as_slice).unwrap_or(&[]),
            None
        ));
        shown.push(row);
    }
    let accounts = app.open_mut()?.book.read(|tx| -> Result<Value, FbError> {
        let setup = fb_core::documents::setup_snapshot(tx, c)?;
        let undeposited = match setup["undeposited"].as_str().and_then(AccountId::parse) {
            Some(id) => fb_core::store::account(tx, id)?.map(|a| a.label()),
            None => None,
        };
        Ok(json!({"undeposited": undeposited}))
    })?;
    let mut accounts = accounts;
    accounts["banks"] = json!(accounts_map(app, c)?);
    put(&mut vm, "customer_selected", customer.is_some_and(|c| !c.is_empty()));
    put(&mut vm, "invoice_rows", &shown);
    put(&mut vm, "open_invoices", &rows);
    put(&mut vm, "payment_accounts", accounts);
    put(&mut vm, "form", forms::bind(&fs, &nonfield(e, "")));
    put(&mut vm, "problems", non);
    put(&mut vm, "deposit_hint", non.iter().any(|m| m.contains("customer deposit")));
    put(&mut vm, "setup_hint", non.iter().any(|m| m.contains("Accounting setup")));
    put(&mut vm, "has_errors", !e.is_empty() || !non.is_empty());
    if let Some((warning, confirm)) = extra {
        put(&mut vm, "extra_warning", warning);
        put(&mut vm, "extra_ok", confirm);
    }
    // Worker item 47: fee and adjustment lines, shown while the payment goes straight to a bank (or when a
    // refused save carried some).
    let (fee_accounts, fee_default) = crate::screens::fees::choices(app, c)?;
    let fee_lines = p.map(|p| crate::screens::fees::posted(p, "fee_")).unwrap_or_default();
    put(&mut vm, "show_fees", p.and_then(|p| get(p, "destination")) == Some("BANK") || !fee_lines.is_empty());
    put(&mut vm, "fee_lines", crate::screens::fees::drawn(&fee_lines, fee_default.as_deref()));
    put(&mut vm, "fee_accounts", fee_accounts);
    put(&mut vm, "fee_default", fee_default);
    put(&mut vm, "fee_errors", e.get("fees").cloned().unwrap_or_default());
    if let Some((warning, confirm)) = fee_warning {
        put(&mut vm, "fee_warning", warning);
        put(&mut vm, "fee_ok", confirm);
    }
    put(&mut vm, "csrf_token", token(app, path));
    Ok((
        draw(app, "sales/receipts/form.html", &vm)?,
        if p.is_some() { 400 } else { 200 },
    ))
}
fn accounts_map(app: &mut App, c: CompanyId) -> AppResult<serde_json::Map<String, Value>> {
    Ok(accounts(app, c)?.into_iter().map(|(id, label)| (id, json!(label))).collect())
}
pub(super) fn render(
    app: &mut App,
    r: &Route,
    p: Posted<'_>,
    path: &str,
) -> AppResult<(String, u16)> {
    let sale = r.name.starts_with("sales_receipts:");
    match r.name.rsplit(':').next().unwrap() {
        "new" if sale => entry(app, r, path, get(p, "customer"), None, &Errors::new(), &[], &[]),
        "new" => receive_page(app, r, path, p, None, &Errors::new(), &[], None, None),
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
        if let Some(bank_id) = v["bank_id"].as_str() {
            let bank_label: Option<String> = o.book.read(|tx| {
                tx.query_row(
                    "SELECT code||' · '||name FROM ledger_account WHERE id=?1 AND company_id=?2",
                    params![bank_id, c.to_db()],
                    |r| r.get(0),
                )
                .optional()
                .map_err(FbError::from)
            })?;
            if let Some(label) = bank_label {
                v["bank_label"] = json!(label);
            }
        }
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
        put(&mut vm, "posted_entries", posted_entries(app, c, v["journal_id"].as_str().unwrap())?);
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
        let (allocations,labels)=app.open_mut()?.book.read(|tx|->Result<_,FbError>{let mut st=tx.prepare("SELECT a.invoice_id,a.amount,a.receivables_id,i.identity_id,r.number FROM sales_receiptallocation a JOIN sales_invoice i ON i.id=a.invoice_id JOIN sales_invoicerevision r ON r.id=a.invoice_revision_id WHERE a.receipt_id=?1 ORDER BY a.id")?;let raw=st.query_map([id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?)))?.collect::<Result<Vec<_>,_>>()?;let mut out=vec![];let mut labels=vec![];for (id,n,ar,source,rev) in raw{let a=fb_core::store::account(tx,AccountId::parse(&ar).unwrap())?.unwrap();labels.push(json!({"id":ar,"code":a.code,"name":a.name}));out.push(json!({"invoice_id":id,"invoice":{"identity":identity(tx,&source)?},"invoice_revision":{"number":rev},"receivables_id":ar,"amount":cash(i128::from(n)),"outstanding":fb_core::sales::invoice_balance(tx,c,&id)?["outstanding"]}));}Ok((out,labels))})?;
        put(&mut vm, "allocations", allocations);
        put(&mut vm, "account_labels", labels);
    }
    super::attachments(app, c, v["identity_id"].as_str().unwrap(), &mut vm)?;
    put(&mut vm, "receipt_notices", std::mem::take(&mut app.messages));
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
    let date = crate::screens::document_date(app, &v);
    put(&mut vm, "deposit", deposit(app, arg_company(r)?, &v)?);
    put(&mut vm, "receipt", v);
    put(
        &mut vm,
        "form",
        forms::bind(
            &crate::screens::month_notice(app, reversal_fields(p, 1, date, sale, e)),
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
    let mut ch = vec![("".into(), if sale { "All customers" } else { "Choose…" }.into())];
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
                if sale { "Status" } else { "State" },
                Widget::Select {
                    choices: vec![
                        (
                            "".into(),
                            format!("All {}", if sale { "sales receipts" } else { "payments" }),
                        ),
                        ("LIVE".into(), if sale { "Paid" } else { "Recorded" }.into()),
                        (
                            reversed.into(),
                            "Voided".into(),
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
    if !sale && r.name.ends_with(":new") {
        return receive_submit(app, r, path, p);
    }
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
                    super::super::d2::posting_errors(&mut e, "", messages);
                }
                Err(e) => return Err(e),
            }
        }
        return Ok(Outcome::Invalid {
            html: reverse_page(app, r, path, sale, Some(p), &e)?.0,
        });
    }
    sale_submit(app, r, path, p)
}

/// A sales receipt's one Save. The core reviews exactly the entered sale and records it with that
/// review in this same request, so what is recorded is what passed review. A form whose sale is
/// already recorded (a second click, or Back and Save again) shows that sale instead.
fn sale_submit(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(r)?;
    if let Some(prior) = choices::prior_receipt(app, c, true, Some(p))? {
        let id = prior["id"].as_str().unwrap_or_default().to_owned();
        let number = app.open_mut()?.book.read(|tx| {
            tx.query_row("SELECT d.number FROM sales_receipt r JOIN documents_displayidentity d ON d.id=r.identity_id WHERE r.id=?1", [&id], |r| r.get::<_, String>(0))
        })?;
        app.messages.push(format!("This sales receipt was already saved as {number}. Nothing new was recorded."));
        return Ok(Outcome::Redirect { location: url("sales_receipts:detail", &[&c.to_db(), &id]) });
    }
    let mut e = Errors::new();
    let mut problems = vec![];
    let mut values = header_values(app, c, true, p, &mut e)?;
    // The bank account applies only to a sale deposited straight to the bank; the field is hidden
    // otherwise and anything left in it is not part of the sale.
    if values["destination"] != "BANK" {
        values["bank_id"] = Value::Null;
        e.remove("bank");
    }
    let ch = sale_choices(app, c, None)?;
    let (lines, non) = super::doc_forms::clean_lines(p, KindForm::Sale, &ch, 0, &mut e);
    values["lines"] = json!(lines);
    let tax = values["tax_mode"].as_str().unwrap_or("").to_owned();
    if let Some(id) = tax.strip_prefix('T') {
        values["tax_item_id"] = json!(id);
        values["tax_mode"] = json!("ITEM");
    }
    if super::super::d2::active_form_errors(&e, p) == 0 && non.is_empty() {
        let values: crate::commands::CustomerReceiptValues =
            serde_json::from_value(values).map_err(|e| AppError::Invalid(e.to_string()))?;
        let reviewed = app.run(Command::ReviewCustomerReceipt {
            company_id: c.to_db(),
            sales_receipt: true,
            values: values.clone(),
            include_settlement: false,
        });
        let recorded = match reviewed {
            Ok(Outcome::Done { data }) => app.run(Command::RecordCustomerReceipt {
                company_id: c.to_db(),
                sales_receipt: true,
                values,
                review_token: data["token"].as_str().unwrap_or_default().to_owned(),
                require_same_settlement: false,
            }),
            other => other,
        };
        match recorded {
            Ok(Outcome::Done { data }) => {
                return Ok(Outcome::Redirect {
                    location: url("sales_receipts:detail", &[&c.to_db(), data["id"].as_str().unwrap()]),
                });
            }
            Err(AppError::Core(FbError::Validation(messages))) => {
                for message in messages {
                    if fb_core::periods::is_closed_message(&message) {
                        error(&mut e, "date", message);
                    } else {
                        problems.push(super::doc_forms::plain_sale(&message));
                    }
                }
            }
            Err(err) => return Err(err),
            _ => unreachable!(),
        }
    }
    Ok(Outcome::Invalid {
        html: entry(app, r, path, get(p, "customer"), Some(p), &e, &non, &problems)?.0,
    })
}

/// Amounts as the core reads them: up to two decimal places, no grouping.
fn cents_of(text: &str) -> Option<i128> {
    let d = fb_core::calculations::unit_rate(&json!(text)).ok()?;
    fb_money::Money::from_decimal_exact(d).map(|m| m.cents())
}
/// Receive payment's one Save. The page proposes allocations; the core validates the same plan in
/// a review and records it with that review in this one request, so what is recorded is exactly
/// what passed review. Anything refused comes back on the form with the entered values.
fn receive_submit(app: &mut App, r: &Route, path: &str, p: Posted<'_>) -> AppResult<Outcome> {
    let c = arg_company(r)?;
    // A form whose payment is already recorded (a second click, or Back and Save again) shows that
    // payment instead of recording anything new.
    if let Some(prior) = choices::prior_receipt(app, c, false, Some(p))? {
        let id = prior["id"].as_str().unwrap_or_default().to_owned();
        let number = app.open_mut()?.book.read(|tx| {
            tx.query_row("SELECT d.number FROM sales_receipt r JOIN documents_displayidentity d ON d.id=r.identity_id WHERE r.id=?1", [&id], |r| r.get::<_, String>(0))
        })?;
        app.messages.push(format!("This payment was already saved as {number}. Nothing new was recorded."));
        return Ok(Outcome::Redirect { location: url("receipts:detail", &[&c.to_db(), &id]) });
    }
    let mut e = Errors::new();
    let mut non = vec![];
    let mut values = header_values(app, c, false, p, &mut e)?;
    let customer = values["customer_id"].as_str().unwrap_or_default().to_owned();
    let open = open_invoices(app, c, Some(p))?;
    let mine = open.iter().filter(|r| r["customer_id"] == customer.as_str()).collect::<Vec<_>>();
    if get(p, "allocations-TOTAL_FORMS").and_then(clean_integer).is_some_and(|n| n > MAX_ROWS) {
        non.push("This form has more invoice rows than it can accept. Reload the page and try again.".to_owned());
    }
    let mut allocations = vec![];
    let mut applied = Some(0i128);
    // Worker item 39: allocations the page made to invoices the person did not choose, and those chosen.
    let (mut extras, mut chosen) = (vec![], vec![]);
    for (id, pay, amount, i) in posted_rows(p) {
        if !pay && amount.is_empty() {
            continue;
        }
        if let Some(row) = mine.iter().find(|r| r["invoice_id"] == id.as_str()) {
            let number = row["number"].as_str().unwrap_or_default().to_owned();
            match chosen_mark(p, i) {
                Some("0") => extras.push((id.clone(), number, cents_of(&amount))),
                Some(_) => chosen.push(number),
                None => {}
            }
        }
        let key = format!("invoice-{id}");
        let Some(row) = mine.iter().find(|r| r["invoice_id"] == id.as_str()) else {
            error(&mut e, &key, "This invoice is no longer open for this customer.");
            non.push("One of the invoices is no longer open for this customer. Reload the page to see the current open invoices.".to_owned());
            continue;
        };
        if amount.is_empty() {
            error(&mut e, &key, "Enter the amount to apply, or untick Pay.");
            continue;
        }
        match cents_of(&amount) {
            Some(n) if n <= 0 => error(&mut e, &key, "Enter an amount greater than zero."),
            Some(n) if n > money(&row["outstanding"]) => error(
                &mut e,
                &key,
                format!("More than the open balance of {}.", row["outstanding"].as_str().unwrap_or("")),
            ),
            Some(n) => applied = applied.and_then(|sum| sum.checked_add(n)),
            None => {
                error(&mut e, &key, "Enter an amount such as 125.00.");
                applied = None;
            }
        }
        allocations.push(json!({"invoice_id": id, "amount": amount}));
    }
    if !customer.is_empty() && !e.contains_key("customer") && non.is_empty() {
        if mine.is_empty() {
            non.push("This customer has no open invoices. To record money received before you invoice, use Receive customer deposit.".to_owned());
        } else if allocations.is_empty() {
            non.push("Tick at least one invoice to apply this payment to.".to_owned());
        } else if let (Some(received), Some(applied)) = (values["amount"].as_str().and_then(cents_of), applied)
            && received != applied
            && e.is_empty()
        {
            non.push(format!(
                "You applied {} but received {}. The amounts applied to invoices must add up to the amount received; FreedomBooks does not keep part of a payment as unapplied credit.",
                cash(applied),
                cash(received)
            ));
        }
    }
    values["allocations"] = json!(allocations);
    values.as_object_mut().unwrap().remove("review_token");
    // Worker item 47: a payment that reaches the bank net of a processor's fee. The fee lines are taken off
    // on the way to the bank, so they need Deposit to: a bank account.
    let fees = crate::screens::fees::posted(p, "fee_");
    let adjustments = match crate::screens::fees::adjustments(&fees) {
        Ok(a) => a,
        Err(message) => {
            error(&mut e, "fees", message);
            vec![]
        }
    };
    if !fees.is_empty() && values["destination"] != "BANK" {
        error(&mut e, "fees", "Fees and adjustments are taken off when the money reaches the bank. Choose the bank under Deposit to, or put the fee on the deposit when you deposit this payment.");
    }
    // Worker item 39: typing an amount spreads it over the open invoices oldest first. Money the page put on
    // an invoice the person did not choose is never saved without saying so: the first Save shows how much
    // and where, at the amount box, and saves nothing; saving again with the same amounts goes ahead.
    let extra = if e.is_empty() && non.is_empty() && !extras.is_empty() {
        let received = values["amount"].as_str().and_then(cents_of).unwrap_or(0);
        let confirm = format!(
            "{}|{}",
            cash(received),
            extras.iter().map(|(id, _, n)| format!("{id}={}", n.map(cash).unwrap_or_default())).collect::<Vec<_>>().join(",")
        );
        let extra: i128 = extras.iter().filter_map(|(.., n)| *n).sum();
        let places = if extras.len() == 1 {
            extras[0].1.clone()
        } else {
            and_list(&extras.iter().map(|(_, number, n)| format!("{number} ({})", n.map(cash).unwrap_or_default())).collect::<Vec<_>>())
        };
        let yours = if chosen.is_empty() { String::new() } else { format!(" (you chose {})", and_list(&chosen)) };
        let untick = if extras.len() == 1 { format!("untick {}", extras[0].1) } else { "untick those invoices".to_owned() };
        let warning = format!(
            "{} of the {} received would go to {places}, which you did not choose{yours}. To apply it there, choose Save payment again. Otherwise correct the amount received, or {untick}.",
            cash(extra),
            cash(received)
        );
        Some((warning, confirm))
    } else {
        None
    };
    // Worker review 3b M1: a fee typed without its minus sign on an expense account would increase the deposit
    // and credit that expense. The first Save says so at the fee lines and saves nothing; saving again with the
    // same lines records them as typed. Shown with item 39's warning when both apply, so one Save confirms both.
    let fee = if e.is_empty() && non.is_empty() && !adjustments.is_empty() {
        crate::screens::fees::positive_expense(app, c, &fees, "choose Save payment again")?
    } else {
        None
    };
    let unconfirmed = |warning: &Option<(String, String)>, field: &str| warning.as_ref().is_some_and(|(_, ok)| get(p, field) != Some(ok.as_str()));
    if unconfirmed(&extra, "extra_ok") || unconfirmed(&fee, "fee_ok") {
        return Ok(Outcome::Invalid { html: receive_page(app, r, path, &[], Some(p), &e, &non, extra, fee)?.0 });
    }
    if e.is_empty() && non.is_empty() && !adjustments.is_empty() {
        // Held in Undeposited Funds and deposited to the bank at once, net of the fees, in one step.
        let bank = values["bank_id"].as_str().unwrap_or_default().to_owned();
        let mut input = values.clone();
        input.as_object_mut().unwrap().retain(|k, _| !["tax_mode", "tax_item_id", "lines"].contains(&k.as_str()));
        let o = app.open_mut()?;
        match fb_core::banking::record_receipt_net(&mut o.book, o.owner, c, &input, &bank, &json!(adjustments)) {
            Ok(done) => {
                let (number, deposit, bank_label): (String, String, String) = app.open_mut()?.book.read(|tx| {
                    let n = |id: &str| tx.query_row("SELECT number FROM documents_displayidentity WHERE id=?1", [id], |r| r.get::<_, String>(0));
                    let bank_label = tx.query_row("SELECT code||' · '||name FROM ledger_account WHERE id=?1", [&bank], |r| r.get::<_, String>(0))?;
                    Ok::<_, FbError>((n(done["receipt"]["identity_id"].as_str().unwrap_or_default())?, n(done["deposit"]["identity_id"].as_str().unwrap_or_default())?, bank_label))
                })?;
                let received = done["receipt"]["amount"].as_str().and_then(cents_of).unwrap_or(0);
                let net = done["deposit"]["amount"].as_str().and_then(cents_of).unwrap_or(0);
                app.messages.push(format!(
                    "Saved {number}: {} applied to the invoices. Deposited to {bank_label} as {deposit}: {} ({} with fees and adjustments of {}).",
                    cash(received), cash(net), cash(received), cash(net - received)
                ));
                return Ok(Outcome::Redirect { location: url("receipts:detail", &[&c.to_db(), done["receipt"]["id"].as_str().unwrap_or_default()]) });
            }
            Err(FbError::Validation(messages)) => non.extend(messages.iter().map(|m| plain(m))),
            Err(err) => return Err(err.into()),
        }
    }
    if e.is_empty() && non.is_empty() {
        let values: crate::commands::CustomerReceiptValues =
            serde_json::from_value(values).map_err(|e| AppError::Invalid(e.to_string()))?;
        let reviewed = app.run(Command::ReviewCustomerReceipt {
            company_id: c.to_db(),
            sales_receipt: false,
            values: values.clone(),
            include_settlement: false,
        });
        let recorded = match reviewed {
            Ok(Outcome::Done { data }) => app.run(Command::RecordCustomerReceipt {
                company_id: c.to_db(),
                sales_receipt: false,
                values,
                review_token: data["token"].as_str().unwrap_or_default().to_owned(),
                require_same_settlement: false,
            }),
            other => other,
        };
        match recorded {
            Ok(Outcome::Done { data }) => {
                return Ok(Outcome::Redirect {
                    location: url("receipts:detail", &[&c.to_db(), data["id"].as_str().unwrap()]),
                });
            }
            Err(AppError::Core(FbError::Validation(messages))) => {
                for message in messages {
                    if fb_core::periods::is_closed_message(&message) {
                        error(&mut e, "date", message);
                    } else {
                        non.push(plain(&message));
                    }
                }
            }
            Err(err) => return Err(err),
            _ => unreachable!(),
        }
    }
    Ok(Outcome::Invalid {
        html: receive_page(app, r, path, &[], Some(p), &e, &non, None, None)?.0,
    })
}
/// The cleaned header values of a receipt form, with errors recorded against each field.
fn header_values(app: &mut App, c: CompanyId, sale: bool, p: Posted<'_>, e: &mut Errors) -> AppResult<Value> {
    let header = header(app, c, sale, (None, None), Some(p), &Errors::new())?;
    let mut values = json!({});
    for f in &header {
        let name = &f.name;
        if name == "tax_header_auto" { continue; }
        let s = get(p, name).unwrap_or("");
        let v = if let Widget::Select { choices } = &f.widget {
            if s.is_empty() && f.required {
                error(e, name, "This field is required.");
            } else if !choices.iter().any(|(id, _)| id == s) {
                error(
                    e,
                    name,
                    if ["customer","bank"].contains(&name.as_str()) {"Select a valid choice. That choice is not one of the available choices.".into()}
                    else {format!("Select a valid choice. {s} is not one of the available choices.")},
                );
            }
            s.to_owned()
        } else if name == "date" {
            clean_date(p, name, true, e).unwrap_or_default()
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
            let v = clean_text(p, name, max, f.required, true, e);
            if name == "reference" || name == "memo" {
                super::super::d2::refuse_card_number(p, name, e);
            }
            v
        };
        if name == "request_key" && !v.is_empty() && fb_core::py::py_uuid(&v).is_none() {
            error(e, name, "Enter a valid UUID.");
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
    Ok(values)
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
