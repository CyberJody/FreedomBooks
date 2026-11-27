use super::*;
use fb_core::reports::{
    cash_flow::{self, Activity},
    comparative_profit_and_loss::{self as comparative, Periods},
    customer_statement,
    sales_by_customer_item::{self as sales, Group},
};
use serde::Serialize;

const SCREEN: &str = "ledger/reports/statement.html";
const BODY: &str = "ledger/reports/statement_body.html";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Report {
    Customer,
    CashFlow,
    Comparative,
    Sales,
    ScheduleC,
    Assets,
}

impl Report {
    pub(super) fn from_route(route: &str) -> Option<Self> {
        match route {
            "reports:customer_statements" => Some(Self::Customer),
            "reports:cash_flow" => Some(Self::CashFlow),
            "reports:comparative_profit_and_loss" => Some(Self::Comparative),
            "reports:sales_by_customer_item" => Some(Self::Sales),
            "reports:schedule_c_activity" => Some(Self::ScheduleC),
            "reports:asset_depreciation_schedule" => Some(Self::Assets),
            _ => None,
        }
    }

    pub(super) fn slug(self) -> &'static str {
        match self {
            Self::Customer => "customer-statements",
            Self::CashFlow => "cash-flow",
            Self::Comparative => "comparative-profit-and-loss",
            Self::Sales => "sales-by-customer-item",
            Self::ScheduleC => "schedule-c",
            Self::Assets => "asset-depreciation",
        }
    }

    fn basis_picker(self) -> bool {
        matches!(self, Self::Comparative | Self::ScheduleC)
    }

    fn worksheet_filter(self) -> Option<&'static str> {
        match self {
            Self::ScheduleC => Some("mappings"),
            Self::Assets => Some("assets"),
            _ => None,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Customer => "Customer Statements",
            Self::CashFlow => "Direct-Method Cash Flow",
            Self::Comparative => "Comparative Profit and Loss",
            Self::Sales => "Sales by Customer and Item",
            Self::ScheduleC => "Schedule C Activity Worksheet",
            Self::Assets => "Fixed-Asset Depreciation Schedule",
        }
    }
}

fn range(
    params: Posted<'_>,
    today: NaiveDate,
    month: u32,
) -> Result<(p::Range, NaiveDate), FbError> {
    let basis = match get(params, "basis") {
        Some("accrual") => Some(f::ACCRUAL),
        Some("cash") => Some(f::CASH),
        other => other,
    };
    let dates = p::range(
        &p::RangeInput {
            basis,
            preset: get(params, "preset"),
            start: get(params, "start"),
            end: get(params, "end"),
        },
        today,
        month,
    )?;
    let start = dates
        .start
        .ok_or_else(|| FbError::validation("Enter a \"From\" date for this report."))?;
    Ok((dates, start))
}

pub(super) fn end(params: Posted<'_>, today: NaiveDate, month: u32) -> Result<NaiveDate, FbError> {
    Ok(range(params, today, month)?.0.end)
}

#[derive(Serialize)]
pub(super) struct Cell {
    text: String,
    number: bool,
    money: bool,
    url: Option<String>,
}

impl Cell {
    pub(super) fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            number: false,
            money: false,
            url: None,
        }
    }

    pub(super) fn number(number: impl ToString) -> Self {
        Self {
            text: number.to_string(),
            number: true,
            money: false,
            url: None,
        }
    }

    pub(super) fn money(amount: fb_core::Money) -> Self {
        Self {
            money: true,
            ..Self::number(amount)
        }
    }

    pub(super) fn link(text: String, url: String) -> Self {
        Self {
            url: Some(url),
            ..Self::text(text)
        }
    }

    fn csv(&self) -> String {
        if self.number {
            self.text.clone()
        } else {
            p::csv_text(&fb_core::card_numbers::mask_card_numbers(&self.text))
        }
    }
}

#[derive(Serialize)]
struct Row {
    kind: &'static str,
    cells: Vec<Cell>,
}

#[derive(Default, Serialize)]
pub(super) struct Table {
    title: Option<&'static str>,
    headings: Vec<String>,
    rows: Vec<Row>,
    pub(super) notes: Vec<String>,
    metadata: Vec<(String, String)>,
    csv_journal_link: bool,
}

impl Table {
    fn new(headings: &[&str]) -> Self {
        Self {
            headings: headings.iter().map(|s| (*s).into()).collect(),
            ..Self::default()
        }
    }

    pub(super) fn push(&mut self, kind: &'static str, cells: impl IntoIterator<Item = Cell>) {
        self.rows.push(Row {
            kind,
            cells: cells.into_iter().collect(),
        });
    }

    pub(super) fn titled(title: &'static str, headings: &[&str]) -> Self {
        Self {
            title: Some(title),
            ..Self::new(headings)
        }
    }

    fn csv(&self, output: &mut String) {
        if let Some(title) = self.title {
            csv_row(output, [p::csv_text(title)]);
        }
        for (label, value) in &self.metadata {
            csv_row(output, [p::csv_text(label), value.clone()]);
        }
        for note in &self.notes {
            csv_row(output, ["Note".into(), p::csv_text(note)]);
        }
        let mut headings = self
            .headings
            .iter()
            .map(|h| p::csv_text(h))
            .collect::<Vec<_>>();
        if self.csv_journal_link {
            headings.push("Journal link".into());
        }
        csv_row(output, headings);
        for row in &self.rows {
            let mut cells = row.cells.iter().map(Cell::csv).collect::<Vec<_>>();
            cells.resize(self.headings.len(), String::new());
            if self.csv_journal_link {
                let link = row
                    .cells
                    .iter()
                    .find_map(|cell| cell.url.as_deref())
                    .map(|path| p::csv_text(&format!("fb://app{path}")))
                    .unwrap_or_default();
                cells.push(link);
            }
            csv_row(output, cells);
        }
    }
}

fn invalid(app: &App, mut vm: Value, errors: Vec<String>) -> AppResult<(String, u16)> {
    vm["invalid"] = json!(true);
    vm["print_url"] = Value::Null;
    vm["csv_url"] = Value::Null;
    vm["errors"] = json!(errors);
    Ok((app.screens.render(SCREEN, &vm)?, 400))
}

pub(super) fn render(
    app: &mut App,
    report: Report,
    params: Posted<'_>,
    path: &str,
    company: CompanyId,
    mut vm: Value,
) -> AppResult<(String, u16)> {
    let today = (app.today)();
    let month = fiscal_start_month(app, company)?;
    let raw = |key| get(params, key).unwrap_or("");
    let title = report.title();
    vm["title"] = json!(title);
    vm["path"] = json!(path);
    vm["comparative"] = json!(report == Report::Comparative);
    vm["customer_statement"] = json!(report == Report::Customer);
    vm["sales_report"] = json!(report == Report::Sales);
    vm["bases"] = json!(if report.basis_picker() {
        &f::BASES[..]
    } else {
        &[]
    });
    vm["period_choices"] = json!(Periods::CHOICES);
    vm["group_choices"] = json!(Group::CHOICES);
    vm["presets"] = json!(p::PRESETS.map(|(key, label)| {
        let (start, end) = p::preset_range(key, today, month)
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .unwrap_or_default();
        json!([key, label, start, end])
    }));
    let mut preset = raw("preset");
    if preset.is_empty() && (!raw("start").is_empty() || !raw("end").is_empty()) {
        preset = "custom";
    }
    let customer = raw("customer");
    let customer_q = raw("customer_q");
    vm["controls"] = json!({
        "preset":preset,"start":raw("start"),"end":raw("end"),"basis":raw("basis"),
        "customer":customer,"period":raw("period"),"group":raw("group"),
    });
    let worksheet = report
        .worksheet_filter()
        .map(|key| (key, raw(key).replace("\r\n", "\n").replace('\r', "\n")));
    if let Some((key, raw)) = worksheet.as_ref() {
        worksheets::editor(&mut vm, key, raw);
    }
    let worksheet_text = worksheet
        .as_ref()
        .map(|(_, raw)| raw.as_str())
        .unwrap_or_default();
    let output = raw("output");
    let print = raw("print");
    let printing = output == "print" || print == "1";
    let mut errors = Vec::new();
    if !matches!(output, "" | "print" | "csv") {
        errors.push(choice_error(output));
    }
    if !matches!(print, "" | "1") {
        errors.push("Select a valid print option.".into());
    }
    if !report.basis_picker() && !matches!(raw("basis"), "" | "ACCRUAL") {
        errors.push("This report has a fixed basis.".into());
    }
    let periods = Periods::parse(raw("period"));
    if report == Report::Comparative && periods.is_none() {
        errors.push("Choose Monthly, Quarterly, or Current FY and prior FY.".into());
    }
    let group = Group::parse(raw("group"));
    if report == Report::Sales && group.is_none() {
        errors.push("Choose a sales grouping from the list.".into());
    }
    let dates = range(params, today, month);
    if let Err(error) = &dates {
        errors.extend(error_messages(error));
    }
    if report == Report::Customer {
        if output != "csv" && !printing {
            let open = app.open_mut()?;
            let picker = customer_statement::customers(
                &mut open.book,
                open.owner,
                company,
                customer_q,
                customer,
            )?;
            vm["customers"] = json!(
                picker
                    .choices
                    .iter()
                    .map(|c| json!({
                        "id":c.id,"name":c.name,"active":c.active
                    }))
                    .collect::<Vec<_>>()
            );
            vm["customers_truncated"] = json!(picker.truncated);
            vm["customer_picker_limit"] = json!(customer_statement::CUSTOMER_PICKER_LIMIT);
            vm["customer_q"] = json!(customer_q);
        }
        if customer.is_empty() && (output == "csv" || printing) {
            errors.push("Select a customer to print or export a statement.".into());
        }
    }
    if !errors.is_empty() {
        return invalid(app, vm, errors);
    }

    let (dates, start) = dates.expect("validated dates");
    let end = dates.end;
    let periods = periods.unwrap_or(Periods::Monthly);
    let group = group.unwrap_or(Group::Customer);
    vm["controls"] = json!({
        "preset":dates.preset,"start":start.to_string(),"end":end.to_string(),"basis":dates.basis.as_str(),
        "customer":customer,"period":periods.key(),"group":group.key(),
    });
    if report == Report::Customer && customer.is_empty() {
        vm["pick_customer"] = json!(true);
        return Ok((app.screens.render(SCREEN, &vm)?, 200));
    }

    vm["start"] = json!(start.to_string());
    vm["end"] = json!(end.to_string());
    let basis = match report {
        Report::CashFlow => "Direct method",
        _ => dates.basis.label(),
    };
    vm["basis_label"] = json!(basis);

    let mut query = vec![
        ("preset".into(), "custom".into()),
        ("start".into(), start.to_string()),
        ("end".into(), end.to_string()),
    ];
    match report {
        Report::Customer => {
            query.push(("customer".into(), customer.into()));
            if !customer_q.is_empty() {
                query.push(("customer_q".into(), customer_q.into()));
            }
        }
        Report::Comparative => query.extend([
            ("basis".into(), dates.basis.as_str().into()),
            ("period".into(), periods.key().into()),
        ]),
        Report::Sales => query.push(("group".into(), group.key().into())),
        Report::ScheduleC => query.push(("basis".into(), dates.basis.as_str().into())),
        Report::CashFlow | Report::Assets => {}
    }
    if let Some((key, raw)) = worksheet.as_ref() {
        if !raw.is_empty() {
            query.push(((*key).into(), raw.clone()));
        }
        if path.len() + urlencode(&query).len() + 1 > worksheets::MAX_TARGET {
            return invalid(app, vm, vec![worksheets::TARGET_ERROR.into()]);
        }
    }
    vm["print_url"] = json!(format!("{path}?{}", with(&query, Some(("print", "1")))));
    vm["csv_url"] = json!(format!("{path}?{}", with(&query, Some(("output", "csv")))));
    vm["screen_url"] = json!(format!("{path}?{}", urlencode(&query)));

    let open = app.open_mut()?;
    let actor = open.owner;
    let book = &mut open.book;
    let result = match report {
        Report::Customer => customer_statement::customer_statement(
            book, actor, company, customer, start, end,
        )
        .map(|statement| {
            vm["subtitle"] = json!(statement.customer);
            vec![statement_table(company, statement)]
        }),
        Report::CashFlow => cash_flow::cash_flow(book, actor, company, start, end)
            .map(|flow| vec![cash_table(company, flow)]),
        Report::Comparative => comparative::comparative_profit_and_loss(
            book,
            actor,
            company,
            dates.basis,
            periods,
            start,
            end,
        )
        .map(|comparison| {
            let fiscal_years = periods == Periods::FiscalYears;
            vm["landscape"] = json!(!fiscal_years);
            if fiscal_years {
                vm["start"] = json!(comparison.columns[0].report.start.map(|d| d.to_string()));
                vm["end"] = json!(comparison.columns[0].report.end.to_string());
            }
            vec![comparative_table(comparison, fiscal_years)]
        }),
        Report::Sales => sales::sales_by_customer_item(book, actor, company, start, end, group)
            .map(|sales| vec![sales_table(sales, group)]),
        Report::ScheduleC => worksheets::schedule_c(
            book,
            actor,
            company,
            dates.basis,
            start,
            end,
            worksheet_text,
        )
        .map(|report| worksheets::schedule_c_tables(report, &mut vm)),
        Report::Assets => worksheets::assets(book, actor, company, start, end, worksheet_text)
            .map(|report| worksheets::asset_tables(report, company, start, end, &mut vm)),
    };
    let tables = match result {
        Ok(tables) => tables,
        Err(FbError::Validation(errors)) => return invalid(app, vm, errors),
        Err(error) => return Err(error.into()),
    };

    if output == "csv" {
        let mut csv = format!("\u{feff}{}", meta(&vm, title, Some(basis)));
        if let Some(customer) = vm["subtitle"].as_str() {
            csv_row(&mut csv, ["Customer".into(), p::csv_text(customer)]);
        }
        csv_row(
            &mut csv,
            ["From".into(), vm["start"].as_str().unwrap_or("").into()],
        );
        csv_row(
            &mut csv,
            ["Through".into(), vm["end"].as_str().unwrap_or("").into()],
        );
        for table in &tables {
            table.csv(&mut csv);
        }
        return Ok((csv, 200));
    }
    vm["tables"] = json!(tables);
    if printing {
        vm["body"] = json!(BODY);
        vm["generated"] = json!((app.now)().to_rfc3339());
        return super::super::printing::draw(
            app,
            "ledger/reports/print.html",
            &mut vm,
            path,
            "internal",
        );
    }
    Ok((app.screens.render(SCREEN, &vm)?, 200))
}

fn error_messages(error: &FbError) -> Vec<String> {
    match error {
        FbError::Validation(messages) => messages.clone(),
        other => vec![other.to_string()],
    }
}

fn statement_table(company: CompanyId, report: customer_statement::CustomerStatement) -> Table {
    let mut table = Table::new(&[
        "Activity",
        "Date",
        "Reference",
        "A/R change",
        "Credit allocated",
        "Net A/R balance",
    ]);
    let balance = |label, amount| {
        [
            Cell::text(label),
            Cell::text(""),
            Cell::text(""),
            Cell::text(""),
            Cell::text(""),
            Cell::money(amount),
        ]
    };
    table.push(
        "detail",
        balance(
            "Open Invoice A/R at opening",
            report.opening_receivables.open_invoices,
        ),
    );
    table.push(
        "detail",
        balance(
            "Unapplied Credits at opening",
            report.opening_receivables.unapplied_credits,
        ),
    );
    table.push("total", balance("Opening Net A/R", report.opening_ar));
    if report.lines.is_empty() {
        table.push(
            "heading",
            [Cell::text("No receivables activity in this date range.")],
        );
    }
    for row in report.lines {
        let reference = if let Some(journal) = row.journal {
            Cell::link(row.number, url("journal", &[&company.to_db(), &journal]))
        } else {
            Cell::text(row.number)
        };
        table.push(
            "detail",
            [
                Cell::text(row.activity),
                Cell::text(row.date.to_string()),
                reference,
                Cell::money(row.ar_delta),
                Cell::money(row.credit_allocation),
                Cell::money(row.ar_balance),
            ],
        );
    }
    table.push(
        "detail",
        balance(
            "Open Invoice A/R at closing",
            report.closing_receivables.open_invoices,
        ),
    );
    table.push(
        "detail",
        balance(
            "Unapplied Credits at closing",
            report.closing_receivables.unapplied_credits,
        ),
    );
    table.push(
        "grand fin-final",
        balance("Closing Net A/R", report.closing_ar),
    );
    table.csv_journal_link = true;
    table.metadata.extend([
        (
            "Opening unapplied customer deposits (liability)".into(),
            report.opening_deposits.to_string(),
        ),
        (
            "Closing unapplied customer deposits (liability)".into(),
            report.closing_deposits.to_string(),
        ),
    ]);
    table
}

fn cash_table(company: CompanyId, report: cash_flow::CashFlow) -> Table {
    let mut table = Table::new(&["Activity", "Date", "Journal", "Memo", "Cash change"]);
    table.csv_journal_link = true;
    let summary = |label, amount| {
        [
            Cell::text(label),
            Cell::text(""),
            Cell::text(""),
            Cell::text(""),
            Cell::money(amount),
        ]
    };
    table.push("total", summary("Opening cash", report.opening));
    if report.rows.is_empty() {
        table.push(
            "heading",
            [Cell::text("No external cash movements in this date range.")],
        );
    }
    for (activity, total) in Activity::ALL.into_iter().zip(report.net) {
        table.push("heading", [Cell::text(activity.label())]);
        for row in report.rows.iter().filter(|r| r.activity == activity) {
            table.push(
                "detail",
                [
                    Cell::text(row.category),
                    Cell::text(row.date.to_string()),
                    Cell::link(
                        format!("J{:06}", row.number),
                        url("journal", &[&company.to_db(), &row.journal]),
                    ),
                    Cell::text(fb_core::card_numbers::mask_card_numbers(&row.memo)),
                    Cell::money(row.amount),
                ],
            );
        }
        table.push("total", summary("Net cash for activity", total));
    }
    table.push(
        "total",
        summary("Net change in cash", report.closing - report.opening),
    );
    table.push("grand fin-final", summary("Closing cash", report.closing));
    table.notes.push("Cash includes posted balances of bank accounts, including archived bank accounts. Transfers within these accounts offset. Review unclassified movements using their journal links.".into());
    table
}

fn comparative_table(report: comparative::ComparativeProfitAndLoss, fiscal_years: bool) -> Table {
    let mut table = Table::new(&["Account"]);
    for column in &report.columns {
        table.headings.push(column.label.clone());
        table.notes.push(format!(
            "{} · {} through {}",
            column.label,
            column.report.start.expect("comparison start"),
            column.report.end
        ));
    }
    if fiscal_years {
        table
            .headings
            .extend(["Variance".into(), "Variance %".into()]);
    }
    if report.unconverted > 0 {
        table.notes.push(format!("Cash basis includes {} unconverted receivables or payables lines through the latest period.", report.unconverted));
    }
    for row in report.rows {
        let mut cells = vec![Cell::text(row.label)];
        cells.extend(row.amounts.iter().copied().map(Cell::money));
        if fiscal_years && let [current, prior] = row.amounts.as_slice() {
            cells.push(Cell::money(*current - *prior));
            cells.push(Cell::number(
                comparative::variance_percent(*current, *prior).unwrap_or_default(),
            ));
        }
        table.push(row.kind, cells);
    }
    table
}

fn sales_table(report: sales::SalesByCustomerItem, group: Group) -> Table {
    let mut table = Table::new(&[]);
    if group != Group::Item {
        table.headings.push("Customer".into());
    }
    if group != Group::Customer {
        table.headings.push("Item".into());
    }
    table.headings.extend(
        [
            "Gross sales",
            "Discounts and credits",
            "Net sales",
            "Net quantity",
            "Activity lines",
        ]
        .map(str::to_owned),
    );
    let totals = |amounts: sales::SalesAmounts| {
        [
            Cell::money(amounts.gross),
            Cell::money(amounts.adjustments),
            Cell::money(amounts.net()),
            Cell::number(amounts.quantity_text()),
            Cell::number(amounts.activity_lines),
        ]
    };
    if report.rows.is_empty() {
        table.push(
            "heading",
            [Cell::text("No posted sales activity in this date range.")],
        );
    }
    for row in report.rows {
        let mut cells = Vec::new();
        if group != Group::Item {
            cells.push(Cell::text(row.customer));
        }
        if group != Group::Customer {
            cells.push(Cell::text(row.item));
        }
        cells.extend(totals(row.amounts));
        table.push("detail", cells);
    }
    let mut total = vec![Cell::text("Total")];
    if group == Group::CustomerItem {
        total.push(Cell::text(""));
    }
    total.extend(totals(report.totals));
    table.push("grand fin-final", total);
    table.notes.push(format!("Net sales exclude collected sales tax. Posted sales documents reconcile to {} across {} postings and reversals.", report.document_net, report.document_events));
    if report.manual_income_lines > 0 {
        table.notes.push(format!(
            "Manual journal income of {} across {} lines is excluded from sales documents.",
            report.manual_income, report.manual_income_lines
        ));
    }
    table
}
