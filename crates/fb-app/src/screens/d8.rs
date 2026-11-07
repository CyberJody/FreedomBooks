//! Presentation-only port of ledger.report_views. Arithmetic, presets, rows and paging are L4 services.
use super::*;
use chrono::NaiveDate;
use fb_core::py;
use fb_core::reports::{financial as f, params as p};

pub(super) fn is_route(name: &str) -> bool {
    matches!(name, "profit_loss" | "balance_sheet" | "general_ledger")
}

enum Clean {
    Range(p::Range),
    AsOf(f::Basis, NaiveDate),
    Ledger(p::LedgerRange),
}
impl Clean {
    fn basis(&self) -> f::Basis {
        match self {
            Self::Range(r) => r.basis,
            Self::AsOf(b, _) => *b,
            Self::Ledger(r) => r.range.basis,
        }
    }
    fn end(&self) -> NaiveDate {
        match self {
            Self::Range(r) => r.end,
            Self::AsOf(_, d) => *d,
            Self::Ledger(r) => r.range.end,
        }
    }
    fn start(&self) -> Option<NaiveDate> {
        match self {
            Self::Range(r) => r.start,
            Self::AsOf(_, _) => None,
            Self::Ledger(r) => r.range.start,
        }
    }
}
fn choice_error(value: &str) -> String {
    format!("Select a valid choice. {value} is not one of the available choices.")
}
fn clean(name: &str, params: &[(String, String)], today: NaiveDate) -> Result<Clean, Vec<String>> {
    let input = p::RangeInput {
        basis: get(params, "basis"),
        preset: get(params, "preset"),
        start: get(params, "start"),
        end: get(params, "end"),
    };
    // LedgerForm's CharField validates length before clean_account. UUID identities necessarily
    // replace the decimal-pk length limit for valid ids; malformed text keeps Django's field error.
    let raw_account = get(params, "account").map(py::py_strip).unwrap_or("");
    let account_length = name == "general_ledger"
        && AccountId::parse(raw_account).is_none()
        && py::py_len(raw_account) > 12;
    let mut account_errors = Vec::new();
    if account_length {
        account_errors.push(py::max_length_message(12, raw_account));
    }
    if name == "general_ledger" && raw_account.contains('\0') {
        account_errors.push("Null characters are not allowed.".into());
    }
    let result = match name {
        "balance_sheet" => {
            p::as_of(input.basis, get(params, "as_of"), today).map(|(b, d)| Clean::AsOf(b, d))
        }
        "general_ledger" => p::ledger_range(
            &input,
            if !account_errors.is_empty() {
                None
            } else {
                get(params, "account")
            },
            get(params, "group"),
            today,
        )
        .map(Clean::Ledger),
        _ => p::range(&input, today).map(Clean::Range),
    };
    let mut errors = match &result {
        Err(FbError::Validation(e)) => e.clone(),
        Err(e) => vec![e.to_string()],
        _ => vec![],
    };
    if !account_errors.is_empty() {
        errors.retain(|e| {
            e != "Enter a \"To\" date for a custom range."
                && e != "The \"From\" date must not be after the \"To\" date."
        });
        let group = get(params, "group").unwrap_or("");
        let index = errors
            .len()
            .saturating_sub(usize::from(f::group_types(group).is_none()));
        errors.splice(index..index, account_errors);
    }
    // output is the second inherited Django field. It is not part of core RangeInput.
    let output = get(params, "output").unwrap_or("");
    if !["", "print", "csv"].contains(&output) {
        errors.retain(|e| {
            e != "Enter a \"To\" date for a custom range."
                && e != "The \"From\" date must not be after the \"To\" date."
        });
        let basis_error =
            get(params, "basis").is_some_and(|b| !["", "ACCRUAL", "CASH"].contains(&b));
        errors.insert(usize::from(basis_error), choice_error(output));
    }
    if errors.is_empty() {
        Ok(result.expect("validated report controls"))
    } else {
        Err(errors)
    }
}
fn fields(name: &str) -> Vec<&'static str> {
    let mut fields = vec!["basis", "output"];
    if name == "balance_sheet" {
        fields.push("as_of");
    } else {
        fields.extend(["preset", "start", "end"]);
    }
    if name == "general_ledger" {
        fields.extend(["account", "group"]);
    }
    fields
}
fn controls(name: &str, params: &[(String, String)], cleaned: Option<&Clean>) -> Value {
    let mut out = json!({});
    for field in fields(name) {
        out[field] = json!(get(params, field).unwrap_or(""));
    }
    if let Some(clean) = cleaned {
        out["basis"] = json!(clean.basis().as_str());
        out["output"] = json!(get(params, "output").unwrap_or(""));
        if let Clean::AsOf(_, d) = clean {
            out["as_of"] = json!(d.to_string());
        } else {
            let range = match clean {
                Clean::Range(r) => r,
                Clean::Ledger(r) => &r.range,
                _ => unreachable!(),
            };
            out["preset"] = json!(range.preset);
            out["start"] = json!(range.start.map(|d| d.to_string()).unwrap_or_default());
            out["end"] = json!(range.end.to_string());
            if let Clean::Ledger(r) = clean {
                out["account"] = json!(r.account.map(|id| id.to_db()).unwrap_or_default());
                out["group"] = json!(r.group);
            }
        }
    }
    out
}
/// SIM-FIX item 5: with a preset other than Custom, replace submitted `start`/`end` (which the preset
/// overrides) by the resolved dates, at the first date key's place. Queries without dates are unchanged.
fn canonical_dates(params: &[(String, String)], cleaned: &Clean) -> Vec<(String, String)> {
    let range = match cleaned {
        Clean::Range(r) => r,
        Clean::Ledger(r) => &r.range,
        Clean::AsOf(..) => return params.to_vec(),
    };
    let dated = |k: &str| k == "start" || k == "end";
    if range.preset == "custom" || !params.iter().any(|(k, _)| dated(k)) {
        return params.to_vec();
    }
    let mut out = Vec::with_capacity(params.len() + 1);
    let mut placed = false;
    for (k, v) in params {
        if !dated(k) {
            out.push((k.clone(), v.clone()));
        } else if !placed {
            placed = true;
            out.push(("start".into(), range.start.map(|d| d.to_string()).unwrap_or_default()));
            out.push(("end".into(), range.end.to_string()));
        }
    }
    out
}
/// QueryDict.urlencode groups duplicate keys at their first occurrence, preserving each key's values.
fn with(params: &[(String, String)], change: Option<(&str, &str)>) -> String {
    let mut keys = Vec::new();
    for (key, _) in params {
        if key != "page" && !keys.contains(key) {
            keys.push(key.clone());
        }
    }
    if let Some((key, value)) = change {
        if !value.is_empty() && !keys.iter().any(|k| k == key) {
            keys.push(key.into());
        }
        if value.is_empty() {
            keys.retain(|k| k != key);
        }
    }
    let mut pairs = Vec::new();
    for key in keys {
        if let Some((k, v)) = change
            && key == k
        {
            pairs.push((key, v.into()));
            continue;
        }
        pairs.extend(params.iter().filter(|(k, _)| *k == key).cloned());
    }
    urlencode(&pairs)
}
fn drill_url(c: CompanyId, drill: &p::Drill) -> String {
    let (route, basis, start, end, tail) = match drill {
        p::Drill::Ledger {
            basis,
            start,
            end,
            account,
            group,
        } => {
            let tail = if let Some(id) = account {
                ("account", id.to_db())
            } else {
                ("group", group.unwrap_or("").into())
            };
            ("general_ledger", *basis, *start, *end, Some(tail))
        }
        p::Drill::ProfitLoss { basis, start, end } => ("profit_loss", *basis, *start, *end, None),
    };
    let mut pairs = vec![
        ("basis".into(), basis.as_str().into()),
        ("preset".into(), "custom".into()),
        (
            "start".into(),
            start.map(|d| d.to_string()).unwrap_or_default(),
        ),
        ("end".into(), end.to_string()),
    ];
    if let Some((k, v)) = tail {
        pairs.push((k.into(), v));
    }
    format!("{}?{}", url(route, &[&c.to_db()]), urlencode(&pairs))
}
fn display_rows(c: CompanyId, rows: &[p::DisplayRow]) -> Vec<Value> {
    rows.iter().map(|row| json!({"kind":row.kind.as_str(), "label":row.label, "account":row.account.as_ref().map(report_account), "amount":row.amount.map(|a|a.to_string()), "depth":row.depth, "url":row.drill.as_ref().map(|d|drill_url(c,d)).unwrap_or_default(), "note":row.note, "final":row.final_row})).collect()
}
fn report_account(a: &Account) -> Value {
    let mut v = serde_json::to_value(account_vm(a)).unwrap();
    v["get_kind_display"] = json!(a.kind.label());
    v
}
fn accounts(app: &mut App, c: CompanyId) -> AppResult<Vec<Account>> {
    Ok(app
        .open_mut()?
        .book
        .read(|tx| fb_core::store::accounts(tx, c))?)
}

/// The metadata query is the existing core banking resolver, unchanged. URL construction is presentation.
pub(super) fn sources(app: &mut App, c: CompanyId) -> AppResult<BTreeMap<String, SourceVm>> {
    Ok(app.open_mut()?.book.read(|tx| -> Result<_, FbError> {
        let meta = fb_core::banking::source_metadata(tx, c)?;
        let mut out = BTreeMap::new();
        let mut st = tx
            .prepare("SELECT id,kind,number FROM documents_documentidentity WHERE company_id=?1")?;
        for row in st.query_map([c.to_db()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })? {
            let (identity, kind, mut number) = row?;
            let Some(meta) = meta.get(&identity) else {
                continue;
            };
            let route = match kind.as_str() {
                "INV" => "invoices:detail",
                "RCP" => "receipts:detail",
                "SR" => "sales_receipts:detail",
                "CM" => "credits:detail",
                "RFD" => "credits:refund_detail",
                "CDP" => "prepayments:detail",
                "CDA" => "prepayments:detail",
                "CDR" => "prepayments:refund_detail",
                "BILL" => "bills:detail",
                "BPAY" => "payments:detail",
                "CHK" => "checks:detail",
                "DEP" => "deposits:detail",
                "TRF" => "transfers:detail",
                _ => continue,
            };
            if kind == "CHK" {
                number = meta["ref"].as_str().unwrap_or("").into();
            }
            let mut document = meta["document_id"].as_str().unwrap_or("").to_owned();
            // A deposit application opens its parent deposit, as the frozen resolver does.
            if kind == "CDA" {
                document = tx.query_row(
                    "SELECT deposit_id FROM prepayments_depositapplication WHERE id=?1",
                    [&document],
                    |r| r.get(0),
                )?;
            }
            out.insert(
                identity,
                SourceVm {
                    label: meta["label"].as_str().unwrap_or("").into(),
                    number,
                    party: meta["party"].as_str().unwrap_or("").into(),
                    url: url(route, &[&c.to_db(), &document]),
                },
            );
        }
        Ok(out)
    })?)
}
fn describe(c: CompanyId, row: &f::LedgerRow, sources: &BTreeMap<String, SourceVm>) -> Value {
    let source = row.source_id.as_ref().and_then(|id| sources.get(id));
    let journal_url = row
        .journal
        .as_ref()
        .map(|(id, _)| url("journal", &[&c.to_db(), id]))
        .unwrap_or_default();
    let label = if row.synthetic.is_some() {
        format!(
            "{} (cash basis)",
            source.map(|s| s.label.as_str()).unwrap_or("Document")
        )
    } else {
        source
            .map(|s| s.label.clone())
            .unwrap_or("General journal".into())
    };
    let label = if row.reversal {
        format!("Reversal · {label}")
    } else {
        label
    };
    let number = source
        .filter(|s| !s.number.is_empty())
        .map(|s| s.number.clone())
        .unwrap_or_else(|| {
            if !row.entry_number.is_empty() {
                row.entry_number.clone()
            } else {
                row.journal
                    .as_ref()
                    .map(|(_, n)| format!("J{n:06}"))
                    .unwrap_or_default()
            }
        });
    json!({"date":row.date.to_string(), "type":label, "number":number, "party":source.filter(|s|!s.party.is_empty()).map(|s|s.party.as_str()).unwrap_or(&row.name), "memo":row.memo, "debit":row.debit.to_string(), "credit":row.credit.to_string(), "balance":row.balance.to_string(), "synthetic":row.synthetic.is_some(), "url":source.filter(|s|!s.url.is_empty()).map(|s|s.url.as_str()).unwrap_or(&journal_url), "journal_url":journal_url})
}

pub(super) fn method_not_allowed(app: &mut App, route: &Route) -> AppResult<Outcome> {
    // Django checks membership before refusing POST. No report has a POST action or command token.
    let _ = shell(
        app,
        Some(arg_company(route)?),
        &Current {
            route: &route.name,
            kind: None,
        },
    )?;
    Ok(Outcome::Page {
        html: String::new(),
        status: 405,
    })
}

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
    let template = format!("ledger/reports/{}.html", route.name);
    // SIM-FIX item 5: each preset carries the dates it stands for today (same clock as the report),
    // so the desktop fills From/To as soon as a preset is chosen, before Run (electron/preload).
    let today = (app.today)();
    vm["presets"] = json!(p::PRESETS.iter().map(|(key, label)| {
        let (start, end) = p::preset_range(key, today).map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or_default();
        json!([key, label, start, end])
    }).collect::<Vec<_>>());
    vm["bases"] = json!(f::BASES);
    vm["group_choices"] = json!(f::GL_GROUPS);
    if route.name == "general_ledger" {
        vm["accounts"] = json!(
            accounts(app, c)?
                .iter()
                .map(report_account)
                .collect::<Vec<_>>()
        );
    }
    let cleaned = match clean(&route.name, params, (app.today)()) {
        Ok(v) => v,
        Err(errors) => {
            vm["invalid"] = json!(true);
            vm["errors"] = json!(errors);
            vm["controls"] = controls(&route.name, params, None);
            return Ok((app.screens.render(&template, &vm)?, 400));
        }
    };
    vm["controls"] = controls(&route.name, params, Some(&cleaned));
    vm["basis"] = json!(cleaned.basis().as_str());
    vm["basis_label"] = json!(cleaned.basis().label());
    vm["start"] = json!(cleaned.start().map(|d| d.to_string()));
    vm["end"] = json!(cleaned.end().to_string());
    // SIM-FIX item 5: Print, CSV and paging links carry the dates the preset resolved to, not stale
    // submitted ones (the preset overrides them), so every copy of the query agrees with the report.
    let canonical = canonical_dates(params, &cleaned);
    let params = canonical.as_slice();
    vm["print_url"] = json!(format!(
        "{path}?{}",
        with(params, Some(("output", "print")))
    ));
    vm["csv_url"] = json!(format!("{path}?{}", with(params, Some(("output", "csv")))));
    let output = get(params, "output").unwrap_or("");
    let actor = app.open_mut()?.owner;
    match &cleaned {
        Clean::Range(r) => {
            let report =
                f::profit_and_loss(&mut app.open_mut()?.book, actor, c, r.basis, r.start, r.end)?;
            let rows = p::pl_rows(&report);
            if output == "csv" {
                return Ok((financial_csv(&vm, &cleaned, "Profit and loss", &rows), 200));
            }
            vm["report"] = json!({"unconverted":report.unconverted});
            vm["rows"] = json!(display_rows(c, &rows));
        }
        Clean::AsOf(basis, date) => {
            let report = f::balance_sheet(&mut app.open_mut()?.book, actor, c, *basis, *date)?;
            let rows = p::bs_rows(&report);
            vm["as_of"] = json!(date.to_string());
            vm["report"] = json!({"unconverted":report.unconverted,"balanced":report.balanced,"fiscal_year_start":report.fiscal_year_start.to_string()});
            if output == "csv" {
                return Ok((financial_csv(&vm, &cleaned, "Balance sheet", &rows), 200));
            }
            vm["rows"] = json!(display_rows(c, &rows));
        }
        Clean::Ledger(r) => {
            let report = f::general_ledger(
                &mut app.open_mut()?.book,
                actor,
                c,
                r.range.basis,
                r.range.start,
                r.range.end,
                r.account,
                &r.group,
            )?;
            let sources = sources(app, c)?;
            if output == "csv" {
                return Ok((ledger_csv(c, &vm, &cleaned, &report, &sources), 200));
            }
            let count = report
                .sections
                .iter()
                .map(|s| s.rows.len().max(1))
                .sum::<usize>();
            let per_page = if output == "print" {
                count.max(1)
            } else {
                p::GL_PAGE_ROWS
            };
            let (number, pages, grouped) =
                p::ledger_page(&report.sections, get(params, "page"), per_page);
            let groups = grouped.iter().map(|g| {
                let section=&report.sections[g.index];
                json!({"section":{"account":report_account(&section.account),"debit":section.debit.to_string(),"credit":section.credit.to_string(),"closing":section.closing.to_string()},"first":g.first,"forward":g.forward.to_string(),"last":g.last,"rows":g.rows.iter().map(|i|describe(c,&section.rows[*i],&sources)).collect::<Vec<_>>()})
            }).collect::<Vec<_>>();
            vm["account"] = json!(
                accounts(app, c)?
                    .iter()
                    .find(|a| Some(a.id) == r.account)
                    .map(report_account)
            );
            vm["groups"] = json!(groups);
            vm["report"] = json!({"unconverted":report.unconverted,"debit":report.debit.to_string(),"credit":report.credit.to_string()});
            vm["page"] = json!({"number":number,"paginator":{"num_pages":pages,"count":count},"has_previous":number>1,"has_next":number<pages,"previous_page_number":number.saturating_sub(1),"next_page_number":number+1});
            vm["query_string"] = json!(with(params, None));
        }
    }
    if output == "print" {
        vm["title"] = json!(match route.name.as_str() { "profit_loss"=>"Profit and loss", "balance_sheet"=>"Balance sheet", _=>"General ledger" });
        vm["body"] = json!(template.replace(".html", "_body.html"));
        vm["generated"] = json!((app.now)().to_rfc3339());
        vm["screen_url"] = json!(format!("{path}?{}",with(params,Some(("output","")))));
        let target = format!("{path}?{}",urlencode(params));
        return super::printing::draw(app,"ledger/reports/print.html",&mut vm,&target,"internal");
    }
    Ok((app.screens.render(&template, &vm)?, 200))
}

fn csv_row(out: &mut String, cells: impl IntoIterator<Item = String>) {
    let cells = cells
        .into_iter()
        .map(|s| p::csv_quote(&s))
        .collect::<Vec<_>>();
    out.push_str(&cells.join(","));
    out.push_str("\r\n");
}
fn meta(vm: &Value, clean: &Clean, title: &str) -> String {
    let mut out = String::new();
    for (k, v) in [
        ("Report", title),
        ("Company", vm["company"]["name"].as_str().unwrap()),
        ("Currency", vm["company"]["currency"].as_str().unwrap()),
        ("Basis", clean.basis().label()),
    ] {
        csv_row(&mut out, [p::csv_text(k), p::csv_text(v)]);
    }
    out
}
fn csv_tail_meta(out: &mut String) {
    csv_row(out,["Note".into(),"Alpha: keep your real books in your current software. Posted transactions only; drafts excluded.".into()]);
    out.push_str("\r\n");
}
fn financial_csv(vm: &Value, clean: &Clean, title: &str, rows: &[p::DisplayRow]) -> String {
    let mut out = meta(vm, clean, title);
    if title == "Balance sheet" {
        csv_row(&mut out, ["As of".into(), clean.end().to_string()]);
        csv_row(
            &mut out,
            [
                "Fiscal year start".into(),
                vm["report"]["fiscal_year_start"].as_str().unwrap().into(),
            ],
        );
    } else {
        csv_row(
            &mut out,
            [
                "From".into(),
                clean
                    .start()
                    .map(|d| d.to_string())
                    .unwrap_or("Beginning".into()),
            ],
        );
        csv_row(&mut out, ["Through".into(), clean.end().to_string()]);
    }
    csv_tail_meta(&mut out);
    csv_row(
        &mut out,
        [
            "Row type",
            "Section",
            "Group",
            "Account code",
            "Account",
            "Account ID",
            "Amount",
        ]
        .map(str::to_owned),
    );
    for row in p::csv_rows(rows) {
        csv_row(
            &mut out,
            row.into_iter()
                .enumerate()
                .map(|(i, s)| if i == 6 { s } else { p::csv_text(&s) }),
        );
    }
    out
}
fn ledger_csv(
    c: CompanyId,
    vm: &Value,
    clean: &Clean,
    report: &f::GeneralLedger,
    sources: &BTreeMap<String, SourceVm>,
) -> String {
    let mut out = meta(vm, clean, "General ledger");
    csv_row(
        &mut out,
        [
            "From".into(),
            clean
                .start()
                .map(|d| d.to_string())
                .unwrap_or("Beginning".into()),
        ],
    );
    csv_row(&mut out, ["Through".into(), clean.end().to_string()]);
    csv_row(&mut out,["Balances".into(),"Each account on its section side (debit for assets and expenses, credit otherwise); contra accounts run negative".into()]);
    csv_tail_meta(&mut out);
    csv_row(
        &mut out,
        [
            "Row type",
            "Account code",
            "Account",
            "Account ID",
            "Date",
            "Type",
            "Number",
            "Party",
            "Memo",
            "Debit",
            "Credit",
            "Balance",
            "Link",
        ]
        .map(str::to_owned),
    );
    for section in &report.sections {
        let a = &section.account;
        let prefix = |kind: &str, date: String| {
            vec![
                kind.into(),
                p::csv_text(&a.code),
                p::csv_text(&a.name),
                a.id.to_db(),
                date,
            ]
        };
        let mut opening = prefix(
            "opening",
            clean.start().map(|d| d.to_string()).unwrap_or_default(),
        );
        opening.extend([
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            section.opening.to_string(),
            String::new(),
        ]);
        csv_row(&mut out, opening);
        for row in &section.rows {
            let d = describe(c, row, sources);
            let mut cells = prefix("line", row.date.to_string());
            cells.extend(
                ["type", "number", "party", "memo"].map(|k| p::csv_text(&fb_core::card_numbers::mask_card_numbers(d[k].as_str().unwrap()))),
            );
            cells.extend([row.debit, row.credit].map(|m| {
                if m.is_zero() {
                    String::new()
                } else {
                    m.to_string()
                }
            }));
            cells.push(row.balance.to_string());
            cells.push(p::csv_text(&format!(
                "fb://app{}",
                d["url"].as_str().unwrap()
            )));
            csv_row(&mut out, cells);
        }
        let mut closing = prefix("closing", clean.end().to_string());
        closing.extend([
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            section.debit.to_string(),
            section.credit.to_string(),
            section.closing.to_string(),
            String::new(),
        ]);
        csv_row(&mut out, closing);
    }
    out
}
pub(crate) fn export_report(
    app: &mut App,
    c: CompanyId,
    report: &str,
    filters: &[(String, String)],
) -> AppResult<Outcome> {
    if !is_route(report) {
        return Err(AppError::Invalid("Unknown report.".into()));
    }
    let path = url(report, &[&c.to_db()]);
    let mut filters = filters
        .iter()
        .filter(|(k, _)| k != "output")
        .cloned()
        .collect::<Vec<_>>();
    filters.push(("output".into(), "csv".into()));
    let route = resolve(&path).unwrap();
    let (contents, status) = render(app, &route, &filters, &path)?;
    if status != 200 {
        return Err(AppError::Invalid(
            "Correct the report options before exporting.".into(),
        ));
    }
    let clean =
        clean(report, &filters, (app.today)()).map_err(|e| AppError::Invalid(e.join("; ")))?;
    let name = match report {
        "profit_loss" => "profit-and-loss",
        "balance_sheet" => "balance-sheet",
        _ => "general-ledger",
    };
    // The Save dialog is visible UI too: use the book's stable company creation ordinal, not a UUID.
    let ordinal = app.open_mut()?.book.read(|tx| -> Result<_, FbError> {
        let mut st = tx.prepare("SELECT id FROM ledger_company ORDER BY id")?;
        let companies = st
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        companies
            .iter()
            .position(|id| *id == c.to_db())
            .map(|i| i + 1)
            .ok_or_else(|| FbError::missing("Company"))
    })?;
    Ok(Outcome::Export {
        filename: format!("{name}-{ordinal}-{}.csv", clean.end()),
        contents,
    })
}
