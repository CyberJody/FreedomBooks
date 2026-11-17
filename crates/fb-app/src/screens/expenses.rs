//! The expense workspace is a filtered view of the existing financial ledger. No new posting path.
use super::*;
use fb_core::reports::{financial as f, params as p};
use fb_money::Money;

pub(super) fn render(
    app: &mut App,
    route: &Route,
    params: &[(String, String)],
    path: &str,
) -> AppResult<(String, u16)> {
    let c = arg_company(route)?;
    let mut vm = serde_json::to_value(shell(
        app,
        Some(c),
        &Current {
            route: &route.name,
            kind: None,
        },
    )?)
    .unwrap();
    vm["can_write"] = json!(app.authenticated_session()?.role != crate::identity::Role::Readonly);
    let today = (app.today)();
    vm["presets"] = json!(
        p::PRESETS
            .iter()
            .map(|(key, label)| {
                let (start, end) = p::preset_range(key, today)
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .unwrap_or_default();
                json!([key, label, start, end])
            })
            .collect::<Vec<_>>()
    );
    vm["bases"] = json!(f::BASES);
    let categories = app
        .open_mut()?
        .book
        .read(|tx| fb_core::store::accounts(tx, c))?
        .into_iter()
        .filter(|a| a.kind == Kind::Expense)
        .collect::<Vec<_>>();
    vm["categories"] = json!(categories.iter().map(account_vm).collect::<Vec<_>>());
    let raw_account = get(params, "account").unwrap_or("").trim();
    let search = get(params, "q").unwrap_or("").trim();
    vm["controls"] = json!({
        "preset": get(params, "preset").unwrap_or(""),
        "start": get(params, "start").unwrap_or(""),
        "end": get(params, "end").unwrap_or(""),
        "basis": get(params, "basis").unwrap_or(""),
        "account": raw_account, "q": search,
    });
    let range = p::range(
        &p::RangeInput {
            basis: get(params, "basis"),
            preset: get(params, "preset"),
            start: get(params, "start"),
            end: get(params, "end"),
        },
        today,
    );
    let mut errors = Vec::new();
    let account = if raw_account.is_empty() {
        None
    } else {
        let found = categories
            .iter()
            .find(|a| a.id.to_db() == raw_account)
            .map(|a| a.id);
        if found.is_none() {
            errors.push("Choose an expense category in this company.".to_owned());
        }
        found
    };
    if search.chars().count() > 200 {
        errors.push("Keep the expense search to 200 characters or fewer.".into());
    }
    let range = match range {
        Ok(range) => Some(range),
        Err(FbError::Validation(e)) => {
            errors.extend(e);
            None
        }
        Err(e) => return Err(e.into()),
    };
    if !errors.is_empty() {
        vm["errors"] = json!(errors);
        vm["invalid"] = json!(true);
        return Ok((app.screens.render("ledger/expenses.html", &vm)?, 400));
    }
    let range = range.expect("validated expense dates");
    vm["controls"]["preset"] = json!(range.preset);
    vm["controls"]["start"] = json!(range.start.map(|d| d.to_string()).unwrap_or_default());
    vm["controls"]["end"] = json!(range.end.to_string());
    vm["controls"]["basis"] = json!(range.basis.as_str());
    vm["start"] = json!(range.start.map(|d| d.to_string()));
    vm["end"] = json!(range.end.to_string());
    vm["basis"] = json!(range.basis.as_str());
    vm["basis_label"] = json!(range.basis.label());
    // Canonical resolved dates keep paging and drill-down on the displayed period.
    let pairs = vec![
        ("preset".into(), range.preset.clone()),
        (
            "start".into(),
            range.start.map(|d| d.to_string()).unwrap_or_default(),
        ),
        ("end".into(), range.end.to_string()),
        ("basis".into(), range.basis.as_str().into()),
        ("account".into(), raw_account.into()),
        ("q".into(), search.into()),
    ];
    vm["query_string"] = json!(urlencode(&pairs));
    let actor = app.open_mut()?.owner;
    let report = f::general_ledger(
        &mut app.open_mut()?.book,
        actor,
        c,
        range.basis,
        range.start,
        range.end,
        account,
        "expense",
    )?;
    let sources = d8::sources(app, c)?;
    let needle = search.to_lowercase();
    let mut rows = Vec::new();
    let mut summaries = Vec::new();
    let mut total = Money::ZERO;
    for section in &report.sections {
        let category = account_vm(&section.account);
        let mut subtotal = Money::ZERO;
        let mut count = 0;
        for row in &section.rows {
            let mut shown = d8::describe(c, row, &sources);
            let searchable = [
                category.label.as_str(),
                shown["party"].as_str().unwrap_or(""),
                shown["memo"].as_str().unwrap_or(""),
                shown["number"].as_str().unwrap_or(""),
                shown["type"].as_str().unwrap_or(""),
            ]
            .join(" ")
            .to_lowercase();
            if !searchable.contains(&needle) {
                continue;
            }
            let amount = row.debit - row.credit;
            subtotal += amount;
            count += 1;
            shown["category"] = json!(category.label);
            shown["amount"] = json!(amount.to_string());
            rows.push(shown);
        }
        if count == 0 {
            continue;
        }
        total += subtotal;
        let mut drill = pairs
            .iter()
            .filter(|(k, _)| k != "account")
            .cloned()
            .collect::<Vec<_>>();
        drill.push(("account".into(), section.account.id.to_db()));
        summaries.push(
            json!({"label": category.label, "amount": subtotal.to_string(),
            "count": count, "url": format!("{path}?{}", urlencode(&drill))}),
        );
    }
    // Stable newest-first ordering also keeps split lines together in a predictable order.
    rows.sort_by(|a, b| {
        b["date"]
            .as_str()
            .cmp(&a["date"].as_str())
            .then_with(|| b["number"].as_str().cmp(&a["number"].as_str()))
            .then_with(|| a["category"].as_str().cmp(&b["category"].as_str()))
    });
    vm["total"] = json!(total.to_string());
    vm["line_count"] = json!(rows.len());
    vm["category_count"] = json!(summaries.len());
    vm["summaries"] = json!(summaries);
    vm["unconverted"] = json!(report.unconverted);
    vm["page"] = serde_json::to_value(Page::of(&rows, 50, get(params, "page"))).unwrap();
    Ok((app.screens.render("ledger/expenses.html", &vm)?, 200))
}
