use super::*;
use fb_core::reports::subledgers as reports;

#[derive(Clone, Copy)]
pub(super) enum Report {
    Receivables,
    Payables,
    Vendor1099,
}

impl Report {
    pub(super) fn from_route(route: &str) -> Option<Self> {
        match route {
            "ar_aging" => Some(Self::Receivables),
            "ap_aging" => Some(Self::Payables),
            "vendor_1099" => Some(Self::Vendor1099),
            _ => None,
        }
    }

    pub(super) fn slug(self) -> &'static str {
        match self {
            Self::Receivables => "ar-aging",
            Self::Payables => "ap-aging",
            Self::Vendor1099 => "vendor-1099",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Receivables => "A/R Aging Summary",
            Self::Payables => "A/P Aging Summary",
            Self::Vendor1099 => "Vendor 1099 Summary",
        }
    }

    fn document_routes(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Receivables => ("Customer", "customers", "invoices:detail"),
            Self::Payables => ("Vendor", "vendors", "bills:detail"),
            Self::Vendor1099 => ("Vendor", "vendors", ""),
        }
    }

    fn is_vendor_1099(self) -> bool {
        matches!(self, Self::Vendor1099)
    }

    pub(super) fn end(self, params: Posted<'_>, today: NaiveDate) -> Result<NaiveDate, FbError> {
        Ok(if self.is_vendor_1099() {
            vendor_dates(params, today)?.2
        } else {
            p::as_of(None, get(params, "as_of"), today)?.1
        })
    }
}

fn vendor_dates(
    params: Posted<'_>,
    today: NaiveDate,
) -> Result<(&'static str, NaiveDate, NaiveDate), FbError> {
    p::tax_year_range(
        &p::RangeInput {
            preset: get(params, "preset"),
            start: get(params, "start"),
            end: get(params, "end"),
            ..Default::default()
        },
        today,
    )
}

fn aging_amounts(amounts: &reports::AgingAmounts, receivables: bool) -> Vec<String> {
    let mut cells = amounts.buckets.map(|m| m.to_string()).to_vec();
    if receivables {
        cells.extend([
            amounts.receivables.open_invoices.to_string(),
            amounts.receivables.unapplied_credits.to_string(),
        ]);
    }
    cells.push(amounts.total.to_string());
    cells
}

pub(super) fn render(
    app: &mut App,
    report: Report,
    params: Posted<'_>,
    path: &str,
    c: CompanyId,
    mut vm: Value,
) -> AppResult<(String, u16)> {
    const SCREEN: &str = "ledger/reports/subledger.html";
    let today = (app.today)();
    let title = report.title();
    let (party_label, party_kind, document_route) = report.document_routes();
    let vendor_1099 = report.is_vendor_1099();
    let receivables = matches!(report, Report::Receivables);
    vm["title"] = json!(title);
    vm["party_label"] = json!(party_label);
    vm["vendor_1099"] = json!(vendor_1099);
    vm["receivables"] = json!(receivables);
    vm["document_label"] = json!(if receivables {
        "Invoices and credits"
    } else {
        "Bills"
    });
    if vendor_1099 {
        vm["presets"] = json!(p::TAX_YEAR_PRESETS.map(|(key, label)| {
            let (start, end) = p::preset_range(key, today, 1)
                .map(|(start, end)| (start.to_string(), end.to_string()))
                .unwrap_or_default();
            json!([key, label, start, end])
        }));
    }
    let raw = |key| get(params, key).unwrap_or("");
    let output = raw("output");
    let dates = if vendor_1099 {
        vendor_dates(params, today).map(|(preset, start, end)| (preset, Some(start), end))
    } else {
        report.end(params, today).map(|end| ("", None, end))
    };
    let mut errors = if ["", "csv", "print"].contains(&output) {
        vec![]
    } else {
        vec![choice_error(output)]
    };
    if let Err(e) = &dates {
        match e {
            FbError::Validation(messages) => errors.extend(messages.iter().cloned()),
            other => errors.push(other.to_string()),
        }
    }
    if !errors.is_empty() {
        vm["invalid"] = json!(true);
        vm["errors"] = json!(errors);
        vm["controls"] = if vendor_1099 {
            let preset = raw("preset");
            let preset =
                if preset.is_empty() && (!raw("start").is_empty() || !raw("end").is_empty()) {
                    "custom"
                } else {
                    preset
                };
            json!({"preset":preset,"start":raw("start"),"end":raw("end")})
        } else {
            json!({"as_of":raw("as_of")})
        };
        return Ok((app.screens.render(SCREEN, &vm)?, 400));
    }
    let (preset, start, end) = dates.expect("validated report dates");
    vm["end"] = json!(end.to_string());
    let mut query = if let Some(start) = start {
        vm["start"] = json!(start.to_string());
        vm["controls"] = json!({"preset":preset,"start":start.to_string(),"end":end.to_string()});
        vec![
            ("preset".into(), preset.into()),
            ("start".into(), start.to_string()),
            ("end".into(), end.to_string()),
        ]
    } else {
        vm["controls"] = json!({"as_of":end.to_string()});
        vec![("as_of".into(), end.to_string())]
    };
    vm["print_url"] = json!(format!(
        "{path}?{}",
        with(&query, Some(("output", "print")))
    ));
    vm["csv_url"] = json!(format!("{path}?{}", with(&query, Some(("output", "csv")))));
    let open = app.open_mut()?;
    let actor = open.owner;
    let mut csv = String::new();
    if output == "csv" {
        csv = meta(&vm, title, None);
        if let Some(start) = start {
            csv_row(&mut csv, ["From".into(), start.to_string()]);
            csv_row(&mut csv, ["Through".into(), end.to_string()]);
        } else {
            csv_row(&mut csv, ["As of".into(), end.to_string()]);
        }
        csv_tail_meta(&mut csv);
    }

    if let Some(start) = start {
        let data = reports::vendor_1099(&mut open.book, actor, c, start, end)?;
        if output == "csv" {
            csv_row(
                &mut csv,
                ["Vendor", "Billing address", "Payments", "Total paid"].map(str::to_owned),
            );
            for vendor in &data.vendors {
                csv_row(
                    &mut csv,
                    [
                        p::csv_text(&vendor.name),
                        p::csv_text(&vendor.address),
                        vendor.count.to_string(),
                        vendor.total.to_string(),
                    ],
                );
            }
            csv_row(
                &mut csv,
                [
                    "Total".into(),
                    String::new(),
                    data.count.to_string(),
                    data.total.to_string(),
                ],
            );
        } else {
            vm["rows"] = json!(data.vendors.iter().map(|v| json!({"name":v.name,"address":v.address,"count":v.count,
                "total":v.total.to_string(),"url":url("directory:detail", &[&c.to_db(), party_kind, &v.id])})).collect::<Vec<_>>());
            vm["count"] = json!(data.count);
            vm["total"] = json!(data.total.to_string());
        }
    } else {
        let data = match report {
            Report::Receivables => reports::ar_aging(&mut open.book, actor, c, end)?,
            Report::Payables => reports::ap_aging(&mut open.book, actor, c, end)?,
            Report::Vendor1099 => unreachable!(),
        };
        let mut headings = reports::AGING_BUCKETS.to_vec();
        if receivables {
            headings.extend(["Open Invoice A/R", "Unapplied Credits", "Net A/R"]);
        } else {
            headings.push("Total");
        }
        if output == "csv" {
            csv_row(
                &mut csv,
                [party_label]
                    .into_iter()
                    .chain(headings)
                    .map(str::to_owned),
            );
            for party in &data.parties {
                csv_row(
                    &mut csv,
                    [p::csv_text(&party.name)]
                        .into_iter()
                        .chain(aging_amounts(&party.amounts, receivables)),
                );
            }
            csv_row(
                &mut csv,
                ["Total".into()]
                    .into_iter()
                    .chain(aging_amounts(&data.amounts, receivables)),
            );
        } else {
            let company = c.to_db();
            vm["headings"] = json!(headings);
            vm["rows"] = json!(data.parties.iter().map(|party| json!({
                "name":party.name,"url":url("directory:detail", &[&company, party_kind, &party.id]),
                "amounts":aging_amounts(&party.amounts, receivables),
                "documents":party.documents.iter().map(|d| json!({"number":d.number,"due":d.due_date.to_string(),
                    "balance":d.balance.to_string(),"credit":d.credit_memo,"url":url(if d.credit_memo { "credits:detail" } else { document_route }, &[&company, &d.id])})).collect::<Vec<_>>()
            })).collect::<Vec<_>>());
            vm["totals"] = json!(aging_amounts(&data.amounts, receivables));
        }
    }

    match output {
        "csv" => Ok((csv, 200)),
        "print" => {
            query.push(("output".into(), "print".into()));
            draw_print(
                app,
                &mut vm,
                "ledger/reports/subledger_body.html",
                path,
                &query,
            )
        }
        _ => Ok((app.screens.render(SCREEN, &vm)?, 200)),
    }
}
