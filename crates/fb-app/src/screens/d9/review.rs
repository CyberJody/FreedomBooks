//! Downloaded transaction review. Draft edits never write to the ledger.
use super::*;

const PER_PAGE: usize = 50;
const MAX_BULK: usize = 100;

// --- review ----------------------------------------------------------------------------------------

/// `views._int`: a positive integer below 10**12, else None.
fn int(v: &str) -> Option<i64> {
    forms::python_integer(v).filter(|n| *n > 0 && *n < 1_000_000_000_000)
}
fn transaction_by_ordinal(
    tx: &(impl Query + ?Sized),
    c: CompanyId,
    ordinal: i64,
) -> Result<Option<bf::Item>, FbError> {
    let id: Option<String> = tx
        .query_row(
            "SELECT t.id FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE t.ordinal=?1 AND c.company_id=?2",
            params![ordinal, c.to_db()],
            |r| r.get(0),
        )
        .optional()?;
    id.map(|id| bf::item(tx, c, &id)).transpose()
}
/// The transaction id a posted pk names, or a value no transaction has (the service then refuses it
/// with its own "this company" message, as Python's does for an unknown pk).
fn target_id(app: &mut App, c: CompanyId, ordinal: i64) -> AppResult<String> {
    Ok(app
        .open_mut()?
        .book
        .read(|tx| transaction_by_ordinal(tx, c, ordinal))?
        .map(|i| i.id)
        .unwrap_or_default())
}
fn status_display(s: &str) -> &'static str {
    match s {
        "NEW" => "To review",
        "ACCEPTED" => "Accepted",
        "MATCHED" => "Matched",
        _ => "Ignored",
    }
}
fn kind_display(s: &str) -> &'static str {
    match s {
        "CHECK" => "Check",
        "DEPOSIT" => "Deposit",
        "TRANSFER" => "Transfer",
        "JOURNAL" => "Journal entry",
        "MATCH" => "Existing entry",
        _ => "",
    }
}
/// fb-core's `reference_url` route name (`checks:detail:<id>`) as the URL Django stored.
fn reference_url(c: CompanyId, stored: &str) -> String {
    let Some((route, id)) = stored.rsplit_once(':') else {
        return String::new();
    };
    fb_templates::django::reverse(route, &[c.to_db(), id.to_owned()]).unwrap_or_default()
}
fn designated(tx: &(impl Query + ?Sized), c: CompanyId) -> Result<Vec<String>, FbError> {
    let setup = documents::setup_snapshot(tx, c)?;
    Ok(setup["banks"]
        .as_array()
        .map(|b| {
            b.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default())
}

struct Posted1 {
    kind: Option<String>,
    account_id: Option<String>,
    payee: String,
    number: String,
    other_bank: String,
    line: String,
    receipt: String,
    selected: bool,
    error: String,
}

#[derive(Default)]
pub(super) struct ReviewPost {
    posted: HashMap<i64, Posted1>,
    results: Vec<Value>,
}

fn accept_from_post(
    app: &mut App,
    c: CompanyId,
    pk: i64,
    p: Posted<'_>,
    vendors: &HashMap<String, String>,
) -> AppResult<Result<bf::Item, FbError>> {
    let owner = actor(app)?;
    let id = target_id(app, c, pk)?;
    let field = |k: &str| get(p, &format!("{k}_{pk}")).unwrap_or("");
    let posted_id = |k: &str| {
        int(field(k)).map(|_| field(k).to_owned()).or_else(|| {
            // Rust ids are UUIDs where Python's are integers; `_int` refuses anything else.
            let v = py::py_strip(field(k));
            fb_ids::parse_uuid(v)
                .filter(|u| u.to_string() == v)
                .map(|_| v.to_owned())
        })
    };
    let payee = py::py_truncate(py::py_strip(field("payee")), 120);
    let vendor_id = if payee.is_empty() {
        None
    } else {
        vendors.get(&bf::text::py_lower(&payee)).cloned()
    };
    let input = json!({"kind": get(p, &format!("kind_{pk}")).unwrap_or(""), "account_id": posted_id("account"), "payee_name": payee,
        "vendor_id": vendor_id, "check_number": py::py_truncate(py::py_strip(field("number")), 20), "journal_line_id": posted_id("line"),
        "receipt_ids": posted_id("receipt").into_iter().collect::<Vec<_>>(), "transfer_account_id": posted_id("to")});
    Ok(bf::accept(
        &mut app.open_mut()?.book,
        owner,
        c,
        &id,
        &input,
        &bf::SecretSet::none(),
    ))
}

pub(super) fn run_review_post(app: &mut App, c: CompanyId, p: Posted<'_>) -> AppResult<ReviewPost> {
    let owner = actor(app)?;
    let mut out = ReviewPost::default();
    // Retain every visible draft, including unselected rows, when one row fails or succeeds.
    for (key, _) in p
        .iter()
        .filter(|(k, _)| k.starts_with("kind_"))
        .take(PER_PAGE)
    {
        let Some(pk) = key.strip_prefix("kind_").and_then(int) else {
            continue;
        };
        let field = |k: &str| get(p, &format!("{k}_{pk}")).unwrap_or("").to_owned();
        out.posted.insert(
            pk,
            Posted1 {
                kind: Some(field("kind")),
                account_id: Some(field("account")),
                payee: field("payee"),
                number: field("number"),
                other_bank: field("to"),
                line: field("line"),
                receipt: field("receipt"),
                selected: p.iter().any(|(k, v)| k == "sel" && int(v) == Some(pk)),
                error: String::new(),
            },
        );
    }
    let vendors: HashMap<String, String> = app.open_mut()?.book.read(|tx| -> Result<_, FbError> {
        let mut st = tx.prepare("SELECT display_name,id FROM directory_vendor WHERE company_id=?1 AND active=1 ORDER BY id")?;
        let rows = st.query_map([c.to_db()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().map(|(n, id)| (bf::text::py_lower(&n), id)).collect())
    })?;
    let truthy = |k: &str| get(p, k).filter(|v| !v.is_empty());
    let mut targets: Vec<(&str, Option<i64>)> = vec![];
    if let Some(v) = truthy("accept_one") {
        targets.push(("accept", int(v)));
    } else if let Some(v) = truthy("ignore_one") {
        targets.push(("ignore", int(v)));
    } else if let Some(v) = truthy("return_one") {
        targets.push(("return", int(v)));
    } else if let Some(action) = get(p, "action").filter(|a| ["accept", "ignore"].contains(a)) {
        let mut found = vec![];
        for (_, v) in p.iter().filter(|(k, _)| k == "sel").take(MAX_BULK + 1) {
            if let Some(n) = int(v)
                && !found.contains(&n)
            {
                found.push(n);
            }
        }
        targets = found
            .into_iter()
            .map(|n| {
                (
                    if action == "accept" {
                        "accept"
                    } else {
                        "ignore"
                    },
                    Some(n),
                )
            })
            .collect();
    }
    if targets.len() > MAX_BULK {
        out.results.push(
            json!({"ok": false, "text": format!("Choose at most {MAX_BULK} rows at a time.")}),
        );
        targets.clear();
    }
    if targets.is_empty() && out.results.is_empty() {
        out.results.push(
            json!({"ok": false, "text": "Tick the rows to accept or ignore, or use a row button."}),
        );
    }
    for (action, pk) in targets {
        let Some(pk) = pk else { continue };
        let current = app
            .open_mut()?
            .book
            .read(|tx| transaction_by_ordinal(tx, c, pk))?;
        let result = if action != "return" && current.is_some_and(|i| i.status != "NEW") {
            Err(FbError::validation(
                "This transaction is no longer new. No review edits were applied; refresh the review list.",
            ))
        } else {
            match action {
                "accept" => accept_from_post(app, c, pk, p, &vendors)?.map(|i| (i, "accept")),
                "ignore" => {
                    let id = target_id(app, c, pk)?;
                    bf::ignore(
                        &mut app.open_mut()?.book,
                        owner,
                        c,
                        &id,
                        &bf::SecretSet::none(),
                    )
                    .map(|i| (i, "ignore"))
                }
                _ => {
                    let id = target_id(app, c, pk)?;
                    bf::return_to_review(
                        &mut app.open_mut()?.book,
                        owner,
                        c,
                        &id,
                        &bf::SecretSet::none(),
                    )
                    .map(|i| (i, "return"))
                }
            }
        };
        match result {
            Ok((item, what)) => {
                let head = format!(
                    "{} {} {}",
                    item.date().unwrap_or("None"),
                    py::py_truncate(&item.description, 60),
                    cents(item.amount)
                );
                out.results.push(match what {
                    "accept" => json!({"ok": true, "text": format!("{head}: recorded as {}", item.reference), "url": reference_url(c, &item.reference_url)}),
                    "ignore" => json!({"ok": true, "text": format!("{head}: ignored")}),
                    _ => json!({"ok": true, "text": format!("{head}: back in review")}),
                });
            }
            Err(e) => {
                let message = messages(e)?;
                if let Some(row) = out.posted.get_mut(&pk) {
                    row.error = message.clone();
                }
                out.results
                    .push(json!({"ok": false, "text": message, "pk": pk}));
            }
        }
    }
    Ok(out)
}

struct Filter {
    status: String,
    account: Option<String>,
    q: String,
}

pub(super) fn review_page(
    app: &mut App,
    r: &Route,
    path: &str,
    full_path: &str,
    params: Posted<'_>,
    post: Option<ReviewPost>,
) -> AppResult<(String, u16)> {
    let c = arg_company(r)?;
    let owner = actor(app)?;
    let mut status = 200;
    let post = post.unwrap_or_default();
    if post.results.iter().any(|x| x["ok"] == false) {
        status = 400;
    }
    // ReviewFilterForm(request.GET): always bound.
    let feed_choices: Vec<(String, String)> = app.open_mut()?.book.read(|tx| -> Result<_, FbError> {
        let mut st = tx.prepare("SELECT f.id,f.org_name,f.name,c.label FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE c.company_id=?1 ORDER BY f.org_name COLLATE fb_text,f.name COLLATE fb_text,f.first_seen_at,f.id")?;
        let rows = st
            .query_map([c.to_db()], |r| {
                let (org, name, label): (String, String, String) = (r.get(1)?, r.get(2)?, r.get(3)?);
                Ok((r.get::<_, String>(0)?, format!("{} ({label})", if org.is_empty() { name } else { format!("{org} · {name}") })))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })?;
    let status_choices: Vec<(String, String)> = [
        ("", "To review"),
        ("ACCEPTED", "Accepted"),
        ("MATCHED", "Matched"),
        ("IGNORED", "Ignored"),
        ("ALL", "Everything"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    let mut e = Errors::new();
    let raw_status = get(params, "status").unwrap_or("").to_owned();
    if !raw_status.is_empty() && !status_choices.iter().any(|(v, _)| *v == raw_status) {
        error(
            &mut e,
            "status",
            format!("Select a valid choice. {raw_status} is not one of the available choices."),
        );
    }
    let account = model_choice(params, "account", &feed_choices, false, &mut e);
    let q = char_field(params, "q", 120, false, &mut e);
    let valid = e.is_empty();
    let filter = valid.then(|| Filter {
        status: raw_status.clone(),
        account: account.clone(),
        q: q.clone(),
    });
    if !valid {
        status = 400;
    }
    let mut account_choices = vec![(String::new(), "All accounts".to_owned())];
    account_choices.extend(feed_choices);
    let fields = vec![
        Field::new(
            "status",
            "Show",
            Widget::Select {
                choices: status_choices,
            },
        )
        .value_opt(get(params, "status").map(str::to_owned)),
        Field::new(
            "account",
            "Bank account",
            Widget::Select {
                choices: account_choices,
            },
        )
        .value_opt(get(params, "account").map(str::to_owned)),
        Field::new("q", "Description contains", Widget::Text)
            .attr("maxlength", "120")
            .value_opt(get(params, "q").map(str::to_owned)),
    ];
    let filter_form = forms::bind(&bind_errors(fields, &e), &[]);

    let rules = bf::active_rules(&mut app.open_mut()?.book, owner, c)?;
    let categories = bf::rule_accounts(&mut app.open_mut()?.book, owner, c)?;
    let mut vm = base(app, r, path)?;
    let (rows, page, unmapped, pending, vendor_names) = app.open_mut()?.book.read(|tx| -> Result<_, FbError> {
        let mut items: Vec<bf::Item> = vec![];
        if let Some(f) = &filter {
            let mut st = tx.prepare("SELECT t.id,t.feed_account_id,t.status,t.description FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE c.company_id=?1 ORDER BY t.posted_on IS NOT NULL,t.posted_on DESC,t.ordinal DESC")?;
            let ids = st.query_map([c.to_db()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)))?.collect::<Result<Vec<_>, _>>()?;
            let wanted = if f.status.is_empty() { "NEW" } else { f.status.as_str() };
            let needle = bf::text::py_lower(py::py_strip(&f.q));
            for (id, feed, st, description) in ids {
                if wanted != "ALL" && st != wanted {
                    continue;
                }
                if f.account.as_ref().is_some_and(|a| *a != feed) {
                    continue;
                }
                if !f.q.is_empty() && !bf::text::py_lower(&description).contains(&needle) {
                    continue;
                }
                items.push(bf::item(tx, c, &id)?);
            }
        }
        // Paginator(PER_PAGE).get_page(request.GET.get('page')).
        let count = items.len();
        let pages = count.div_ceil(PER_PAGE).max(1);
        let number = match get(params, "page").and_then(forms::python_integer) {
            Some(n) if n >= 1 => (n as usize).min(pages),
            Some(_) => pages,
            None => 1,
        };
        let shown: Vec<bf::Item> = items.into_iter().skip((number - 1) * PER_PAGE).take(PER_PAGE).collect();
        let banks = designated(tx, c)?;
        let bank_accounts: Vec<fb_core::model::Account> = store::accounts(tx, c)?.into_iter().filter(|a| banks.contains(&a.id.to_db())).collect();
        let currency: String = tx.query_row("SELECT currency FROM ledger_company WHERE id=?1", [c.to_db()], |r| r.get(0))?;
        let mut rows = vec![];
        for item in shown {
            let feed = bf::feed(tx, &item.feed_account_id)?.ok_or_else(|| FbError::validation("Choose a downloaded transaction of this company."))?;
            let ledger = feed.ledger_account_id.as_deref().and_then(AccountId::parse).map(|id| store::account(tx, id)).transpose()?.flatten();
            let mut iv = json!({"pk": item.ordinal, "status": item.status, "status_display": status_display(&item.status), "pending": item.pending,
                "date": item.date().unwrap_or(""), "description": item.description, "amount": cents(item.amount), "money_in": item.money_in(),
                "reference": item.reference, "reference_url": reference_url(c, &item.reference_url), "accepted_as": item.accepted_as,
                "accepted_as_display": kind_display(&item.accepted_as),
                "feed_account": {"name": feed.name, "ledger_account": ledger.as_ref().map(|a| json!({"code": a.code, "name": a.name}))}});
            if item.status != "NEW" {
                let reversed = match &item.journal_id {
                    Some(j) => tx.query_row("SELECT EXISTS(SELECT 1 FROM ledger_journal WHERE reversal_of_id=?1)", [j], |r| r.get::<_, bool>(0))?,
                    None => false,
                };
                rows.push(json!({"item": iv, "suggestion": null, "reversed": reversed}));
                continue;
            }
            let s = bf::suggest_for(tx, c, &item, Some(&rules))?;
            let is_designated = feed.ledger_account_id.as_ref().is_some_and(|a| banks.contains(a));
            let other: Vec<&fb_core::model::Account> = bank_accounts.iter().filter(|b| Some(b.id.to_db()) != feed.ledger_account_id && b.active && b.postable).collect();
            let mut options: Vec<(&str, &str)> = vec![];
            if !s.matches.is_empty() {
                options.push(("MATCH", "Match existing entry"));
            }
            if !s.receipts.is_empty() {
                options.push(("DEPOSIT", "Deposit receipt"));
            }
            if item.money_in() {
                options.push(("JOURNAL", if is_designated { "Deposit (journal)" } else { "Payment (journal)" }));
            } else {
                if is_designated {
                    options.push(("CHECK", "Check or debit"));
                }
                options.push(("JOURNAL", if !is_designated { "Charge (journal)" } else { "Journal entry" }));
            }
            if is_designated && !other.is_empty() {
                options.push(("TRANSFER", "Transfer"));
            }
            let default_kind = if !item.money_in() && is_designated { "CHECK" } else { "JOURNAL" };
            let preferred = if s.kind.is_empty() { default_kind } else { s.kind.as_str() };
            options.sort_by_key(|o| o.0 != preferred);
            let posted = post.posted.get(&item.ordinal);
            let kind = posted.and_then(|x| x.kind.clone()).unwrap_or_else(|| options.first().map(|o| o.0.to_owned()).unwrap_or_default());
            let account_id = posted.and_then(|x| x.account_id.clone()).or_else(|| s.account_id.clone());
            let payee = posted.map(|x| x.payee.clone()).unwrap_or_else(|| s.payee_name.clone());
            let number = posted.map(|x| x.number.clone()).unwrap_or_else(|| if item.money_in() { String::new() } else { bf::default_check_number(&item) });
            // Keep unavailable submitted choices visible instead of silently selecting a new value.
            if !options.iter().any(|(value, _)| *value == kind) {
                options.push((&kind, "Unavailable choice — choose again"));
            }
            let category_valid = account_id.as_ref().is_none_or(|id| id.is_empty() || categories.iter().any(|a| a.id.to_db() == *id && Some(id) != feed.ledger_account_id.as_ref()));
            let bulk_block = if !feed.active || ledger.as_ref().is_none_or(|a| !a.active || !a.postable) {
                "Map to an active posting account first"
            } else if item.pending { "Still pending at the bank" }
            else if item.date().is_none() { "Missing transaction date" }
            else if item.amount == 0 { "Zero amount cannot be accepted" }
            else if feed.currency.to_uppercase() != currency { "Account currency differs from the company" }
            else { "" };
            iv["pk"] = json!(item.ordinal);
            rows.push(json!({
                "item": iv,
                "suggestion": {
                    "matches": s.matches.iter().map(|m| json!({"label": m.label, "date": m.date, "line": {"pk": m.line_id}})).collect::<Vec<_>>(),
                    "receipts": s.receipts.iter().map(|x| json!({"label": x.label, "date": x.date, "receipt": {"pk": x.receipt_id}})).collect::<Vec<_>>(),
                    "rule": s.rule.as_ref().map(|x| json!({"name": x.name, "pk": x.id})),
                },
                "options": options,
                "kind": kind,
                "category_valid": category_valid,
                "draft_retained": if posted.is_some() { "true" } else { "false" },
                "selected": posted.is_some_and(|x| x.selected),
                "other_bank_valid": posted.is_none_or(|x| x.other_bank.is_empty() || other.iter().any(|a| a.id.to_db() == x.other_bank)),
                "line_valid": posted.is_none_or(|x| x.line.is_empty() || s.matches.iter().any(|m| m.line_id == x.line)),
                "receipt_valid": posted.is_none_or(|x| x.receipt.is_empty() || s.receipts.iter().any(|r| r.receipt_id == x.receipt)),
                "other_bank": posted.map(|x| x.other_bank.clone()).unwrap_or_default(),
                "line": posted.map(|x| x.line.clone()).unwrap_or_else(|| s.matches.first().map(|m| m.line_id.clone()).unwrap_or_default()),
                "receipt": posted.map(|x| x.receipt.clone()).unwrap_or_else(|| s.receipts.first().map(|r| r.receipt_id.clone()).unwrap_or_default()),
                "bulk_block": bulk_block,
                "matching_rules": rules.iter().filter(|rule| bf::rule_for(std::slice::from_ref(*rule), &item).is_some()).map(|r| r.id.clone()).collect::<Vec<_>>().join(" "),
                "account_id": account_id,
                "payee": payee,
                "number": number,
                "categories": categories.iter().filter(|a| Some(a.id.to_db()) != feed.ledger_account_id).map(|a| json!({"pk": a.id.to_db(), "code": a.code, "name": a.name})).collect::<Vec<_>>(),
                "other_banks": other.iter().map(|a| json!({"pk": a.id.to_db(), "code": a.code, "name": a.name})).collect::<Vec<_>>(),
                "error": posted.map(|x| x.error.clone()),
            }));
        }
        let page = json!({"number": number, "has_previous": number > 1, "has_next": number < pages, "previous_page_number": number.saturating_sub(1),
            "next_page_number": number + 1, "paginator": {"num_pages": pages, "count": count}});
        let unmapped: i64 = tx.query_row("SELECT count(DISTINCT f.id) FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id JOIN bankfeeds_transaction t ON t.feed_account_id=f.id WHERE c.company_id=?1 AND f.active=1 AND f.ledger_account_id IS NULL AND t.status='NEW'", [c.to_db()], |r| r.get(0))?;
        let (_, pending, _) = overview(tx, c)?;
        let mut st = tx.prepare("SELECT display_name FROM directory_vendor WHERE company_id=?1 AND active=1 ORDER BY display_name COLLATE fb_text,id LIMIT 500")?;
        let vendor_names = st.query_map([c.to_db()], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        Ok((rows, page, unmapped, pending, vendor_names))
    })?;
    let query: Vec<(String, String)> = params
        .iter()
        .filter(|(k, _)| k != "page")
        .cloned()
        .collect();
    put(
        &mut vm,
        "review_filters",
        ["status", "account", "q", "page"].map(
            |key| json!({"name": format!("review_{key}"), "value": get(params, key).unwrap_or("")}),
        ),
    );
    put(
        &mut vm,
        "bulk_categories",
        categories
            .iter()
            .map(|a| json!({"pk": a.id.to_db(), "label": account_label(a)}))
            .collect::<Vec<_>>(),
    );
    put(&mut vm, "bulk_rules", rules.iter().map(|r| json!({"pk": r.id, "name": r.name, "account": r.account_id, "payee": r.payee_name})).collect::<Vec<_>>());
    put(&mut vm, "filter_form", filter_form);
    put(&mut vm, "page", page);
    put(
        &mut vm,
        "review_editable",
        rows.iter().any(|r| r["item"]["status"] == "NEW"),
    );
    put(&mut vm, "rows", rows);
    put(&mut vm, "results", post.results);
    put(&mut vm, "query_string", urlencode(&query));
    put(&mut vm, "vendor_names", vendor_names);
    put(
        &mut vm,
        "showing",
        if valid && !raw_status.is_empty() {
            raw_status
        } else {
            "NEW".into()
        },
    );
    put(&mut vm, "unmapped", unmapped);
    put(&mut vm, "pending", pending);
    put(&mut vm, "full_path", full_path);
    Ok((draw(app, "bankfeeds/review.html", &vm)?, status))
}

/// Prefill the existing rule form from a company-owned review row or its reviewed values.
pub(super) fn rule_initial(
    app: &mut App,
    c: CompanyId,
    params: Posted<'_>,
    choices: &[(String, String)],
) -> AppResult<HashMap<&'static str, Option<String>>> {
    let mut initial = HashMap::new();
    if let Some(from) = get(params, "from").filter(|v| !v.is_empty()) {
        let ordinal = int(from).unwrap_or(0);
        let source = app
            .open_mut()?
            .book
            .read(|tx| transaction_by_ordinal(tx, c, ordinal))?;
        if let Some(item) = source {
            let d = &item.description;
            initial.insert(
                "name",
                Some(if d.is_empty() {
                    "New rule".into()
                } else {
                    py::py_truncate(d, 60)
                }),
            );
            initial.insert("match_text", Some(py::py_truncate(d, 200)));
            initial.insert(
                "sign",
                Some(if item.money_in() { "IN" } else { "OUT" }.into()),
            );
            initial.insert(
                "payee_name",
                Some(py::py_truncate(get(params, "payee").unwrap_or(d), 120)),
            );
            if let Some(account) =
                get(params, "category").filter(|id| choices.iter().any(|(key, _)| key == id))
            {
                initial.insert("account", Some(account.to_owned()));
            } else if let Some(journal) = &item.journal_id {
                // Accepted journal/check: reuse its counterpart account and reviewed payee.
                let reviewed: Option<(String, String)> = app.open_mut()?.book.read(|tx| {
                    tx.query_row("SELECT l.account_id,l.name FROM ledger_journalline l JOIN bankfeeds_account f ON f.id=?2 WHERE l.journal_id=?1 AND l.account_id != f.ledger_account_id AND (SELECT count(*) FROM ledger_journalline c WHERE c.journal_id=l.journal_id AND c.account_id != f.ledger_account_id)=1 ORDER BY l.id LIMIT 1", params![journal, item.feed_account_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()
                })?;
                if let Some((account, payee)) =
                    reviewed.filter(|(id, _)| choices.iter().any(|(key, _)| key == id))
                {
                    initial.insert("account", Some(account));
                    // Check journal lines carry accounting amounts, not the reviewed payee.
                    // Read it through the owning document service and verify the journal link.
                    let payee = if item.accepted_as == "CHECK" {
                        let check_id = item
                            .reference_url
                            .strip_prefix("checks:detail:")
                            .ok_or_else(|| {
                                AppError::Invalid(
                                    "This transaction's check reference is damaged.".into(),
                                )
                            })?;
                        let owner = actor(app)?;
                        let check = fb_core::checks::get_check(
                            &mut app.open_mut()?.book,
                            owner,
                            c,
                            check_id,
                        )?;
                        if check["current"]["journal_id"].as_str() != Some(journal.as_str()) {
                            return Err(AppError::Invalid(
                                "This transaction's check does not match its journal.".into(),
                            ));
                        }
                        check["current"]["payee_name"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned()
                    } else {
                        payee
                    };
                    initial.insert("payee_name", Some(payee));
                }
            } else {
                let owner = actor(app)?;
                let rules = bf::active_rules(&mut app.open_mut()?.book, owner, c)?;
                if let Some(rule) = bf::rule_for(&rules, &item) {
                    initial.insert("account", Some(rule.account_id.clone()));
                    if get(params, "payee").is_none() {
                        initial.insert("payee_name", Some(rule.payee_name.clone()));
                    }
                }
            }
        }
    }
    Ok(initial)
}
