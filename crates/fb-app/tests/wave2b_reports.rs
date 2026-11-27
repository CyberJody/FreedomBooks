//! Wave 2b reports through the desktop's actual routes and typed report exports.
mod support;

use fb_app::{App, Command, Outcome, keys, screens};
use fb_core::{CompanyId, credits, directory, documents, sales, services};
use serde_json::json;

struct Fixture {
    _scratch: tempfile::TempDir,
    app: App,
    company: CompanyId,
    customer: String,
    foreign: String,
}

fn fixture() -> Fixture {
    fixture_with("16.02", Some("6.01"))
}

fn fixture_with(rate: &str, partial: Option<&str>) -> Fixture {
    let scratch = tempfile::tempdir().unwrap();
    let mut app = App::new();
    app.today = || "2028-02-29".parse().unwrap();
    let company = app
        .create_book(
            &scratch.path().join("book.fbbook"),
            &keys::new_master_key().unwrap(),
            "owner",
            "=Example <Company>",
        )
        .unwrap()
        .company_id;
    support::sign_in(&mut app);
    let open = app.open_mut().unwrap();
    let (actor, book) = (open.owner, &mut open.book);
    let other = services::create_company(book, actor, "Separate company", "USD", false)
        .unwrap()
        .id;
    let mut seed = |company, name: &str, amount: &str, partial: Option<&str>| {
        let mut account = |code, name, kind, detail| {
            services::save_account(
                book,
                actor,
                company,
                services::AccountInput::new(code, name)
                    .kind(kind)
                    .detail(detail),
            )
            .unwrap()
            .id
        };
        let bank = account("100", "Example bank", "ASSET", "BANK");
        let ar = account("110", "Example receivables", "ASSET", "RECEIVABLE");
        let income = account("400", "Example income", "INCOME", "INCOME");
        documents::save_setup(
            book,
            actor,
            company,
            &json!({"receivables":ar.to_db(),"banks":[bank.to_db()]}),
            Some(0),
        )
        .unwrap();
        let customer = directory::save_record(
            book,
            actor,
            company,
            directory::Model::Customer,
            &json!({"display_name":name}),
            None,
        )
        .unwrap();
        let invoice = sales::save_invoice(book, actor, company, &json!({
            "request_key":uuid::Uuid::new_v4().to_string(),"customer_id":customer["id"],"date":"2028-02-01","tax_mode":"NONE",
            "lines":[{"description":"Work","quantity":"1.25","rate":amount,"taxable":false,"income_account_id":income.to_db()}]
        })).unwrap();
        let invoice_id = invoice["id"].as_str().unwrap();
        let review = sales::review_invoice(book, actor, company, invoice_id, Some(1)).unwrap();
        sales::post_invoice(book, actor, company, invoice_id, Some(1), Some(&review)).unwrap();
        if let Some(partial) = partial {
            let receipt = json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":customer["id"],"date":"2028-02-29","method":"CASH",
                "destination":"BANK","bank_id":bank.to_db(),"amount":partial,"allocations":[{"invoice_id":invoice_id,"amount":partial}]});
            let review = sales::review_receipt(book, actor, company, &receipt).unwrap();
            sales::record_receipt(book, actor, company, &receipt, Some(&review)).unwrap();
        }
        customer["id"].as_str().unwrap().to_owned()
    };
    let customer = seed(company, "=1+2 <Customer>", rate, partial);
    let foreign = seed(other, "Other company customer", "8000.00", Some("9000.00"));
    Fixture {
        _scratch: scratch,
        app,
        company,
        customer,
        foreign,
    }
}

const REPORTS: [(&str, &str, &str, &str, &str); 4] = [
    (
        "customer_statements",
        "customer-statements",
        "Customer Statements",
        "Closing Net A/R",
        "14.02",
    ),
    (
        "cash_flow",
        "cash-flow",
        "Direct-Method Cash Flow",
        "Closing cash",
        "6.01",
    ),
    (
        "comparative_profit_and_loss",
        "comparative-profit-and-loss",
        "Comparative Profit and Loss",
        "Period total",
        "20.03",
    ),
    (
        "sales_by_customer_item",
        "sales-by-customer-item",
        "Sales by Customer and Item",
        "Net sales",
        "20.03",
    ),
];

#[test]
fn report_hub_screen_csv_print_and_typed_download_share_exact_company_scoped_data() {
    let mut fixture = fixture();
    let company = fixture.company.to_db();
    let root = format!("/c/{company}/reports/");
    let hub = screens::render(&mut fixture.app, &root).unwrap();
    for (route, slug, title, label, exact) in REPORTS {
        let path = format!("{root}{slug}/");
        assert!(hub.html.contains(&path) && hub.html.contains(title));
        let route = format!("reports:{route}");
        let mut filters = vec![
            ("start".into(), "2028-02-01".into()),
            ("end".into(), "2028-02-29".into()),
        ];
        if slug == "customer-statements" {
            filters.push(("customer".into(), fixture.customer.clone()));
        }
        if slug == "sales-by-customer-item" {
            filters.push(("group".into(), "customer_item".into()));
        }
        let query = filters
            .iter()
            .map(|(key, value): &(String, String)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("&");
        let target = format!("{path}?{query}");
        let screen = screens::render(&mut fixture.app, &target).unwrap();
        assert_eq!(
            (screen.route.as_str(), screen.status),
            (route.as_str(), 200),
            "{}",
            screen.html
        );
        for expected in [
            title,
            label,
            exact,
            "Download CSV",
            "print=1",
            "reports.css",
        ] {
            assert!(screen.html.contains(expected), "{slug} missing {expected}");
        }
        assert!(screen.html.contains("=Example &lt;Company&gt;"));
        assert!(
            !screen.html.contains("Other company customer") && !screen.html.contains("9000.00")
        );

        let print_target = format!("{target}&print=1");
        let print = screens::render(&mut fixture.app, &print_target).unwrap();
        assert_eq!(print.status, 200);
        for expected in [
            "report-paper",
            "reports.css",
            "print-now",
            title,
            exact,
            "Back to the report",
        ] {
            assert!(print.html.contains(expected), "{slug} missing {expected}");
        }
        assert!(!print.html.contains("Download CSV"));
        let token = print
            .html
            .split_once("name=\"fb_command_token\" value=\"")
            .unwrap()
            .1
            .split('"')
            .next()
            .unwrap()
            .to_owned();
        let printed = fixture
            .app
            .run(Command::SubmitForm {
                path: print_target,
                token,
                fields: vec![("action".into(), "desktop_print".into())],
            })
            .unwrap();
        assert!(matches!(printed, Outcome::Done { data } if data["desktop_print"] == true));
        let csv = screens::render(
            &mut fixture.app,
            &format!("{target}&output=print&output=csv"),
        )
        .unwrap();
        assert_eq!(csv.status, 200);
        assert_eq!(csv.route, route);
        assert!(csv.html.starts_with(&format!(
            "\u{feff}Report,{title}\r\nCompany,'=Example <Company>\r\n"
        )));
        assert!(
            csv.html.contains(label) && csv.html.contains(exact),
            "{}",
            csv.html
        );
        assert!(!csv.html.contains("Other company customer") && !csv.html.contains("9000.00"));
        if matches!(slug, "customer-statements" | "sales-by-customer-item") {
            assert!(csv.html.contains("'=1+2 <Customer>"), "{}", csv.html);
        }
        let export = fixture
            .app
            .run(Command::ExportReport {
                company_id: company.clone(),
                report: route,
                filters,
            })
            .unwrap();
        assert!(matches!(export, Outcome::Export { filename, contents } if
            filename == format!("{slug}-1-2028-02-29.csv") && contents == csv.html));
    }
}

#[test]
fn invalid_filters_and_foreign_customer_do_not_produce_a_download() {
    let mut fixture = fixture();
    let root = format!("/c/{}/reports/", fixture.company);
    let picker = screens::render(&mut fixture.app, &format!("{root}customer-statements/")).unwrap();
    assert_eq!(picker.status, 200);
    assert!(picker.html.contains("Select a customer") && !picker.html.contains("Download CSV"));
    for (slug, filter, expected) in [
        (
            "customer-statements",
            "output=csv",
            "Select a customer to print or export",
        ),
        (
            "cash-flow",
            "start=2028-02-30&end=2028-03-01",
            "Enter a valid date",
        ),
        (
            "comparative-profit-and-loss",
            "period=monthly&start=2027-01-01&end=2028-02-29",
            "no more than 12 periods",
        ),
        (
            "sales-by-customer-item",
            "group=unrecognized",
            "Choose a sales grouping",
        ),
    ] {
        let screen = screens::render(&mut fixture.app, &format!("{root}{slug}/?{filter}")).unwrap();
        assert_eq!(screen.status, 400, "{slug}");
        assert!(screen.html.contains(expected));
        assert!(!screen.html.contains("Download CSV") && !screen.html.starts_with('\u{feff}'));
    }
    let foreign = screens::render(
        &mut fixture.app,
        &format!(
            "{root}customer-statements/?customer={}&output=csv",
            fixture.foreign
        ),
    );
    assert!(foreign.is_err());
}

#[test]
fn statements_and_aging_show_invoice_credit_and_net_ar_in_every_output() {
    let mut f = fixture_with("80.00", None);
    let open = f.app.open_mut().unwrap();
    let income: String = open
        .book
        .read(|tx| {
            tx.query_row(
                "SELECT id FROM ledger_account WHERE company_id=?1 AND code='400'",
                [f.company.to_db()],
                |r| r.get(0),
            )
        })
        .unwrap();
    let memo = json!({
        "request_key":uuid::Uuid::new_v4().to_string(),"customer_id":f.customer,
        "date":"2028-02-20","tax_mode":"NONE","lines":[
            {"item_id":null,"description":"Credit","quantity":"1","rate":"25.00","taxable":false,"income_account_id":income}
        ]
    });
    let review = credits::review_credit_memo(&mut open.book, open.owner, f.company, &memo).unwrap();
    credits::record_credit_memo(&mut open.book, open.owner, f.company, &memo, Some(&review))
        .unwrap();
    let root = format!("/c/{}/reports/", f.company);
    let statement = format!(
        "{root}customer-statements/?customer={}&start=2028-03-01&end=2028-03-05",
        f.customer
    );
    let aging = format!("{root}ar-aging/?as_of=2028-03-05");
    let statement_balances = [
        ("Open Invoice A/R at opening", "100.00"),
        ("Unapplied Credits at opening", "-25.00"),
        ("Opening Net A/R", "75.00"),
        ("Open Invoice A/R at closing", "100.00"),
        ("Unapplied Credits at closing", "-25.00"),
        ("Closing Net A/R", "75.00"),
    ];
    for output in ["", "&output=print"] {
        let report = screens::render(&mut f.app, &format!("{statement}{output}")).unwrap();
        assert_eq!(report.status, 200);
        for (label, amount) in statement_balances {
            let row = report
                .html
                .split_once(&format!("{label}</th>"))
                .unwrap()
                .1
                .split_once("</tr>")
                .unwrap()
                .0;
            assert!(row.contains(&format!(">{amount}</td>")), "{label}: {row}");
        }

        let report = screens::render(&mut f.app, &format!("{aging}{output}")).unwrap();
        assert_eq!(report.status, 200);
        let headings = ["Open Invoice A/R", "Unapplied Credits", "Net A/R"]
            .map(|label| format!(r#"<th scope="col" class="amount">{label}</th>"#))
            .concat();
        let table = report
            .html
            .split_once(r#"<table class="fin-report">"#)
            .unwrap()
            .1;
        assert!(table.contains(&headings));
        for section in ["</details>", "<tfoot>"] {
            let row = table
                .split_once(section)
                .unwrap()
                .1
                .split_once("</tr>")
                .unwrap()
                .0;
            let balances = ["100.00", "-25.00", "75.00"]
                .map(|amount| format!(r#"<td class="amount">{amount}</td>"#))
                .concat();
            assert!(row.ends_with(&balances), "{row}");
        }
    }

    let csv = screens::render(&mut f.app, &format!("{statement}&output=csv")).unwrap();
    assert_eq!(csv.status, 200);
    for (label, amount) in statement_balances {
        assert!(
            csv.html
                .lines()
                .any(|line| line == format!("{label},,,,,{amount},")),
            "{}",
            csv.html
        );
    }
    let csv = screens::render(&mut f.app, &format!("{aging}&output=csv")).unwrap();
    assert_eq!(csv.status, 200);
    assert!(
        csv.html
            .contains("90+,Open Invoice A/R,Unapplied Credits,Net A/R")
    );
    for label in ["'=1+2 <Customer>", "Total"] {
        assert!(
            csv.html
                .lines()
                .any(|line| line.starts_with(&format!("{label},"))
                    && line.ends_with(",100.00,-25.00,75.00")),
            "{}",
            csv.html
        );
    }
}

#[test]
fn an_explicit_archived_customer_statement_works_beyond_the_picker_bound() {
    let mut f = fixture();
    let company = f.company.to_db();
    f.app.open_mut().unwrap().book.write::<_, fb_core::FbError>(|tx| {
        tx.execute(
            "WITH RECURSIVE seq(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM seq WHERE n<5000)
             INSERT INTO directory_customer(id,company_id,display_name,email,active)
             SELECT printf('00000000-0000-4000-8000-%012d',n),?1,printf('Bulk customer %05d',n),'',n%2
             FROM seq",
            [&company],
        )?;
        tx.execute("UPDATE directory_customer SET active=0 WHERE id=?1", [&f.customer])?;
        Ok(())
    }).unwrap();

    let path = format!("/c/{company}/reports/customer-statements/");
    let target = format!(
        "{path}?customer={}&start=2028-02-01&end=2028-02-29",
        f.customer
    );
    for (output, expected) in [
        ("", "Download CSV"),
        ("&output=print", "print-now"),
        ("&output=csv", "\u{feff}Report,Customer Statements"),
    ] {
        let report = screens::render(&mut f.app, &format!("{target}{output}")).unwrap();
        assert_eq!(report.status, 200);
        assert!(report.html.contains("Closing Net A/R") && report.html.contains("14.02"));
        assert!(report.html.contains(expected));
        if output.is_empty() {
            assert!(
                report
                    .html
                    .contains("Showing the first 100 matching customers")
            );
            assert!(
                report
                    .html
                    .contains(&format!(r#"value="{}" selected>"#, f.customer))
            );
            assert!(report.html.contains("(archived)"));
            assert_eq!(
                report.html.matches(r#"<option value="00000000-"#).count(),
                100
            );
        }
        let foreign = format!("{path}?customer={}{output}", f.foreign);
        assert!(screens::render(&mut f.app, &foreign).is_err());
    }
    let search = screens::render(
        &mut f.app,
        &format!("{path}?customer_q=bulk+customer+05000"),
    )
    .unwrap();
    assert_eq!(search.status, 200);
    assert!(search.html.contains("Bulk customer 05000 (archived)"));
    assert!(!search.html.contains("Bulk customer 04999"));
    let export = f
        .app
        .run(Command::ExportReport {
            company_id: company,
            report: "reports:customer_statements".into(),
            filters: vec![
                ("customer".into(), f.customer),
                ("start".into(), "2028-02-01".into()),
                ("end".into(), "2028-02-29".into()),
            ],
        })
        .unwrap();
    assert!(
        matches!(export, Outcome::Export { contents, .. } if contents.contains("Closing Net A/R,,,,,14.02"))
    );
}

#[test]
fn worksheets_preserve_inputs_across_screen_print_csv_and_typed_download() {
    let mut f = fixture();
    let company = f.company;
    let open = f.app.open_mut().unwrap();
    let (book, actor) = (&mut open.book, open.owner);
    fb_core::periods::set_policy(
        book,
        actor,
        company,
        0,
        fb_core::periods::PeriodChange {
            fiscal_year_start_month: 11,
            closing_date: None,
        },
        None,
    )
    .unwrap();
    let accounts = book
        .read(|tx| fb_core::store::accounts(tx, company))
        .unwrap();
    let get = |code| accounts.iter().find(|a| a.code == code).unwrap().id;
    let income = get("400");
    let bank = get("100");
    let asset = services::save_account(
        book,
        actor,
        company,
        services::AccountInput::new("150", "Synthetic equipment")
            .kind("ASSET")
            .detail("FIXED_ASSET"),
    )
    .unwrap()
    .id;
    let draft = services::save_draft(
        book,
        actor,
        company,
        services::DraftInput::new(
            "2028-02-01",
            "Synthetic asset purchase",
            vec![
                services::line(asset).debit("100.00").build(),
                services::line(bank).credit("100.00").build(),
            ],
        ),
    )
    .unwrap();
    let j = services::post_journal(book, actor, company, draft.id, Some(1))
        .unwrap()
        .id;
    let cases = [
        (
            "schedule-c",
            "reports:schedule_c_activity",
            "Schedule C Activity Worksheet",
            "mappings",
            format!("account|{income}|＝Consulting <activity>|gross_receipts|1w"),
            vec![("basis".into(), "cash".into())],
            "Reconciliation to Book P&L",
            "Worksheet total,6.01,0.00,6.01",
            "'＝Consulting <activity>",
        ),
        (
            "asset-depreciation",
            "reports:asset_depreciation_schedule",
            "Fixed-Asset Depreciation Schedule",
            "assets",
            format!("＋Equipment <asset>|{asset}|{j}|||9000|0|||0"),
            Vec::new(),
            "Ledger Reconciliation",
            "\"Variance (worksheet less ledger, period depreciation provisional)\",-10.00,0.00,0.00,-100.00",
            "'＋Equipment <asset>",
        ),
    ];
    let root = format!("/c/{company}/reports/");
    let hub = screens::render(&mut f.app, &root).unwrap();
    for (slug, route, title, key, input, mut filters, label, totals, escaped) in cases {
        let path = format!("{root}{slug}/");
        assert!(hub.html.contains(&path) && hub.html.contains(title));
        let fiscal =
            screens::render(&mut f.app, &format!("{path}?preset=this_year&output=csv")).unwrap();
        assert_eq!(fiscal.status, 200);
        assert!(
            fiscal
                .html
                .contains("From,2027-11-01\r\nThrough,2028-10-31")
        );
        filters.extend([
            ("start".into(), "2028-02-01".into()),
            ("end".into(), "2028-02-29".into()),
            (key.into(), input),
        ]);
        let query = screens::urlencode(&filters);
        let target = format!("{path}?{query}");
        let screen = screens::render(&mut f.app, &target).unwrap();
        assert_eq!((screen.status, screen.route.as_str()), (200, route));
        assert!(screen.html.contains(&format!("name=\"{key}\"")));
        assert!(
            screen.html.contains("report-worksheet.js") && screen.html.contains("Download CSV")
        );
        assert!(screen.html.contains("print=1"));
        if key == "assets" {
            assert!(screen.html.contains(&format!("account={asset}")));
            assert!(
                screen
                    .html
                    .contains("Incomplete · missing in-service date, method, useful life")
            );
        } else {
            assert!(screen.html.contains("Tax-preparation worksheet"));
            assert!(screen.html.contains("Unassigned"));
        }

        let print_target = format!("{target}&print=1");
        let print = screens::render(&mut f.app, &print_target).unwrap();
        assert_eq!(print.status, 200);
        assert!(print.html.contains("print-now") && print.html.contains("reports.css"));
        assert!(
            !print.html.contains("report-worksheet.js") && !print.html.contains("Download CSV")
        );
        assert!(print.html.contains(&format!("{key}=")));
        let label_html = label.replace('&', "&amp;");
        assert!(screen.html.contains(&label_html) && print.html.contains(&label_html));
        let token = print
            .html
            .split_once("name=\"fb_command_token\" value=\"")
            .unwrap()
            .1
            .split('"')
            .next()
            .unwrap();
        assert!(matches!(f.app.run(Command::SubmitForm {
            path: print_target, token: token.into(), fields: vec![("action".into(), "desktop_print".into())],
        }).unwrap(), Outcome::Done { data } if data["desktop_print"] == true));

        let csv =
            screens::render(&mut f.app, &format!("{target}&output=print&output=csv")).unwrap();
        assert_eq!((csv.status, csv.route.as_str()), (200, route));
        assert!(csv.html.starts_with(&format!("\u{feff}Report,{title}\r\n")));
        assert!(
            csv.html.contains(escaped) && csv.html.contains(totals),
            "{}",
            csv.html
        );
        assert!(!csv.html.contains("9000.00") && !csv.html.contains("Other company"));
        let export = f
            .app
            .run(Command::ExportReport {
                company_id: company.to_db(),
                report: route.into(),
                filters,
            })
            .unwrap();
        assert!(matches!(export, Outcome::Export { filename, contents }
            if filename == format!("{slug}-1-2028-02-29.csv") && contents == csv.html));
    }
}

#[test]
fn worksheets_reject_cross_company_and_oversize_inputs_before_export() {
    let mut f = fixture();
    let open = f.app.open_mut().unwrap();
    let book = &mut open.book;
    let (foreign, income): (String, String) = book
        .read(|tx| {
            Ok::<_, fb_core::FbError>(tx.query_row(
                "SELECT company_id,id FROM ledger_account WHERE company_id<>?1 AND code='400'",
                [f.company.to_db()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .unwrap();
    let foreign = CompanyId::parse(&foreign).unwrap();
    let asset = services::save_account(
        book,
        open.owner,
        foreign,
        services::AccountInput::new("155", "Foreign equipment")
            .kind("ASSET")
            .detail("FIXED_ASSET"),
    )
    .unwrap()
    .id;
    for (slug, key, input, expected) in [
        (
            "schedule-c",
            "mappings",
            format!("account|{income}|Other|gross_receipts|1w"),
            "Choose a P&amp;L account in this company",
        ),
        (
            "asset-depreciation",
            "assets",
            format!("Asset|{asset}||||10000|0|straight_line|12|0"),
            "Choose a fixed-asset cost account in this company",
        ),
        (
            "schedule-c",
            "mappings",
            "＠".repeat(425),
            "Worksheet inputs are too long",
        ),
    ] {
        let query = screens::urlencode(&[(key.into(), input)]);
        let url = format!("/c/{}/reports/{slug}/?{query}&output=csv", f.company);
        let page = screens::render(&mut f.app, &url).unwrap();
        assert_eq!(page.status, 400);
        assert!(page.html.contains(expected), "Missing {expected}");
        assert!(!page.html.starts_with('\u{feff}') && !page.html.contains("Download CSV"));
    }
}

#[test]
fn asset_outputs_show_disposal_and_qualify_unresolved_depreciation() {
    let mut f = fixture();
    let open = f.app.open_mut().unwrap();
    let (book, actor, company) = (&mut open.book, open.owner, f.company);
    let bank = book
        .read(|tx| {
            Ok::<_, fb_core::FbError>(
                fb_core::store::accounts(tx, company)?
                    .into_iter()
                    .find(|account| account.code == "100")
                    .unwrap()
                    .id,
            )
        })
        .unwrap();
    let mut account = |code, name, kind, detail, contra| {
        services::save_account(
            book,
            actor,
            company,
            services::AccountInput::new(code, name)
                .kind(kind)
                .detail(detail)
                .contra(contra),
        )
        .unwrap()
        .id
    };
    let accumulated = account(
        "159",
        "Accumulated depreciation",
        "ASSET",
        "FIXED_ASSET",
        true,
    );
    let cogs = account(
        "510",
        "Manufacturing depreciation",
        "EXPENSE",
        "COST_OF_GOODS_SOLD",
        false,
    );
    let supplies = account("610", "Supplies", "EXPENSE", "EXPENSE", false);
    let draft = services::save_draft(
        book,
        actor,
        company,
        services::DraftInput::new(
            "2028-02-10",
            "Compound depreciation",
            vec![
                services::line(cogs).debit("50").build(),
                services::line(supplies).debit("20").build(),
                services::line(bank).credit("20").build(),
                services::line(accumulated).credit("50").build(),
            ],
        ),
    )
    .unwrap();
    services::post_journal(book, actor, company, draft.id, Some(1)).unwrap();
    let path = format!("/c/{company}/reports/asset-depreciation/");
    let target = |disposal| {
        let query = screens::urlencode(&[
            ("preset".into(), "custom".into()),
            ("start".into(), "2028-02-01".into()),
            ("end".into(), "2028-02-29".into()),
            (
                "assets".into(),
                format!("Machinery||||2028-02-01|29000|14500|straight_line|1|0|{disposal}"),
            ),
        ]);
        format!("{path}?{query}")
    };
    for output in ["", "&print=1", "&output=csv"] {
        let page =
            screens::render(&mut f.app, &format!("{}{output}", target("2028-02-10"))).unwrap();
        assert_eq!(page.status, 200, "{}", page.html);
        for visible in [
            "Disposal date",
            "2028-02-10",
            "Disposed by To date",
            "Unresolved credits",
            "period depreciation provisional",
            "Mixed journals contain unresolved contra movements",
        ] {
            assert!(
                page.html.contains(visible),
                "{visible} missing for {output}"
            );
        }
        if output == "&output=csv" {
            assert!(page.html.contains("Machinery,2028-02-01,2028-02-10,Straight-line / 1 months,290.00,145.00,0.00,50.00,0.00,0.00,0.00,Complete · Disposed by To date"));
            assert!(page.html.contains(
                "Total accumulated depreciation,0.00,0.00,0.00,0.00,50.00,0.00,50.00,50.00,-50.00"
            ));
        }
    }
    for (invalid, expected) in [
        ("2028-02-30", "disposal date in YYYY-MM-DD format"),
        ("2028-2-10", "disposal date in YYYY-MM-DD format"),
        ("2028-01-31", "on or after the in-service date"),
    ] {
        let page = screens::render(&mut f.app, &target(invalid)).unwrap();
        assert_eq!(page.status, 400);
        assert!(page.html.contains(expected), "{}", page.html);
    }
}

#[test]
fn worksheet_builder_uses_rendered_inputs_and_matches_the_server_url_limit() {
    let mut f = fixture();
    let assets = format!("/c/{}/reports/asset-depreciation/", f.company);
    let mappings = format!("/c/{}/reports/schedule-c/", f.company);
    let dates = [
        ("preset".into(), "custom".into()),
        ("start".into(), "2028-02-01".into()),
        ("end".into(), "2028-02-29".into()),
    ];
    let target = |rows: &str| {
        let mut pairs = dates.to_vec();
        if !rows.is_empty() {
            pairs.push(("assets".into(), rows.into()));
        }
        format!("{assets}?{}", screens::urlencode(&pairs))
    };
    let mut labels: Vec<_> = (1..=20).map(|i| format!("Tool {i:02} *~!'()")).collect();
    let rows = |labels: &[String]| {
        labels
            .iter()
            .map(|label| format!("{label}||||2028-02-01|29000|14500|straight_line|1|0|2028-02-10"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut remaining = 3800 - target(&rows(&labels)).len();
    for label in &mut labels {
        let stars = (remaining / 3).min(80 - label.len());
        label.push_str(&"*".repeat(stars));
        remaining -= 3 * stars;
    }
    assert!(remaining <= 2);
    labels.last_mut().unwrap().push_str(&"~".repeat(remaining));
    let rows = rows(&labels);
    let limit = target(&rows);
    assert_eq!(limit.len(), 3800);
    assert!(limit.contains("+%2A~%21%27%28%29"));
    assert!(limit.contains("%0A") && !limit.contains("%0D"));
    let page = screens::render(&mut f.app, &limit).unwrap();
    assert_eq!(page.status, 200, "{}", page.html);
    let escaped = limit.replace('&', "&amp;");
    assert!(
        page.html
            .contains(&format!("href=\"{escaped}&amp;print=1\""))
    );
    assert!(
        page.html
            .contains(&format!("href=\"{escaped}&amp;output=csv\""))
    );
    for suffix in ["&print=1", "&output=csv"] {
        let output = screens::render(&mut f.app, &format!("{limit}{suffix}")).unwrap();
        assert_eq!(output.status, 200);
        assert!(output.html.contains("Tool 20"));
    }
    let (prefix, last) = rows.rsplit_once('\n').unwrap();
    let over_rows = format!("{prefix}\n{}", last.replacen("|", "~|", 1));
    let over = target(&over_rows);
    assert_eq!(over.len(), 3801);
    let refused = screens::render(&mut f.app, &over).unwrap();
    assert_eq!(refused.status, 400);
    assert!(refused.html.contains("Worksheet inputs are too long"));

    let mapping_page = screens::render(&mut f.app, &mappings).unwrap();
    let asset_page = screens::render(&mut f.app, &target("")).unwrap();
    assert!(
        mapping_page
            .html
            .contains("data-worksheet-posted-id data-worksheet-field")
    );
    assert!(
        asset_page
            .html
            .contains("Disposal date <input data-worksheet-field type=\"date\"")
    );

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../electron");
    let electron = root.join("node_modules/electron/dist/electron");
    if !electron.is_file() {
        eprintln!("Electron is unavailable. Rendered HTML and server URL assertions passed.");
        return;
    }
    let scratch = tempfile::tempdir().unwrap();
    std::fs::write(scratch.path().join("worksheet.json"), json!({
        "pages": {(&mappings):mapping_page.html, (&assets):asset_page.html},
        "mappings":mappings, "assets":target(""), "target":limit, "rows":rows, "overRows":over_rows,
    }).to_string()).unwrap();
    let result = std::process::Command::new(electron)
        .arg("--ozone-platform=headless")
        .arg(root.join("tests/support/worksheet-builder.cjs"))
        .arg(scratch.path())
        .env("FB_DESKTOP_USER_DATA", scratch.path().join("profile"))
        .env_remove("ELECTRON_RUN_AS_NODE")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
