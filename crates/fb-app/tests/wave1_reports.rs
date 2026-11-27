//! Financial and tax reports through the app's normal routes and typed commands.
mod support;

use fb_app::{App, Command, Outcome, keys, screens};
use fb_core::{CompanyId, directory, documents, payables, sales, salestax, services};
use serde_json::{Value, json};

struct Fixture {
    _dir: tempfile::TempDir,
    app: App,
    company: CompanyId,
    customer: String,
    vendor: String,
    invoice: String,
    bill: String,
}

fn id(v: &Value) -> &str {
    v["id"].as_str().unwrap()
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new();
    app.today = || "2028-02-29".parse().unwrap();
    let company = app
        .create_book(
            &dir.path().join("book.fbbook"),
            &keys::new_master_key().unwrap(),
            "owner",
            "Synthetic Wave 1",
        )
        .unwrap()
        .company_id;
    support::sign_in(&mut app);
    let open = app.open_mut().unwrap();
    let (actor, book) = (open.owner, &mut open.book);
    let mut account = |code, kind, detail| {
        services::save_account(
            book,
            actor,
            company,
            services::AccountInput::new(code, code)
                .kind(kind)
                .detail(detail),
        )
        .unwrap()
        .id
        .to_db()
    };
    let bank = account("100", "ASSET", "BANK");
    let ar = account("110", "ASSET", "RECEIVABLE");
    let ap = account("200", "LIABILITY", "PAYABLE");
    let tax = account("210", "LIABILITY", "OTHER_CURRENT_LIABILITY");
    let income = account("400", "INCOME", "INCOME");
    let expense = account("600", "EXPENSE", "EXPENSE");
    documents::save_setup(
        book,
        actor,
        company,
        &json!({"receivables":ar,"payables":ap,"collected_tax":tax,"banks":[bank]}),
        Some(0),
    )
    .unwrap();
    let customer = directory::save_record(
        book,
        actor,
        company,
        directory::Model::Customer,
        &json!({"display_name":"Example <Customer>"}),
        None,
    )
    .unwrap();
    let vendor = directory::save_record(
        book,
        actor,
        company,
        directory::Model::Vendor,
        &json!({"display_name":"Example, Vendor","billing_address":"1 Example Road, Albany, NY"}),
        None,
    )
    .unwrap();
    let rate = salestax::save_rate(book, actor, company, &json!({
        "name":"Example tax","description":"Example tax","rate":"6","new_agency_name":"Example jurisdiction","expected_revision":0
    })).unwrap();
    salestax::save_settings(book, actor, company, &json!({
        "charge_tax":true,"default_item_id":rate["id"],"owed_basis":"ACCRUAL","filing_frequency":"QUARTERLY","expected_revision":0
    })).unwrap();
    let invoice = sales::save_invoice(book, actor, company, &json!({
        "request_key":uuid::Uuid::new_v4().to_string(),"customer_id":customer["id"],"date":"2027-12-01","due_date":"2028-01-01",
        "tax_mode":"ITEM","tax_item_id":rate["id"],"lines":[
            {"description":"Taxed","quantity":"1","rate":"100.00","taxable":true,"income_account_id":income},
            {"description":"Exempt","quantity":"1","rate":"20.00","taxable":false,"income_account_id":income}
        ]
    })).unwrap();
    let review = sales::review_invoice(book, actor, company, id(&invoice), Some(1)).unwrap();
    sales::post_invoice(book, actor, company, id(&invoice), Some(1), Some(&review)).unwrap();
    let bill = payables::save_bill(book, actor, company, &json!({
        "request_key":uuid::Uuid::new_v4().to_string(),"vendor_id":vendor["id"],"date":"2027-12-01","due_date":"2028-02-29",
        "lines":[{"description":"Supplies","quantity":"1","rate":"50.05","expense_account_id":expense}]
    })).unwrap();
    let review = payables::review_bill(book, actor, company, id(&bill), Some(1)).unwrap();
    payables::post_bill(book, actor, company, id(&bill), Some(1), &review).unwrap();
    let payment = json!({"request_key":uuid::Uuid::new_v4().to_string(),"vendor_id":vendor["id"],"date":"2027-12-31",
        "method":"CASH","amount":"20.01","bank_id":bank,"allocations":[{"bill_id":bill["id"],"amount":"20.01"}]});
    let review = payables::review_payment(book, actor, company, &payment).unwrap();
    payables::record_payment(book, actor, company, &payment, &review).unwrap();
    Fixture {
        _dir: dir,
        app,
        company,
        customer: id(&customer).into(),
        vendor: id(&vendor).into(),
        invoice: id(&invoice).into(),
        bill: id(&bill).into(),
    }
}

fn token(html: &str) -> &str {
    html.split_once("name=\"fb_command_token\" value=\"")
        .unwrap()
        .1
        .split('"')
        .next()
        .unwrap()
}

#[test]
fn aging_and_1099_routes_drills_print_and_typed_csv() {
    let mut f = fixture();
    let c = f.company.to_db();
    let audit = |app: &mut App| {
        app.open_mut()
            .unwrap()
            .book
            .read(|q| {
                q.query_row("SELECT count(*) FROM ledger_auditevent", [], |r| {
                    r.get::<_, i64>(0)
                })
            })
            .unwrap()
    };
    let before = audit(&mut f.app);
    let report_path = format!("/c/{c}/reports/");
    let hub = screens::render(&mut f.app, &report_path).unwrap().html;
    for (slug, route, title, query, expected, filename) in [
        (
            "ar-aging",
            "ar_aging",
            "A/R Aging Summary",
            "as_of=2028-02-29",
            "Example <Customer>,0.00,0.00,126.00,0.00,0.00,126.00,0.00,126.00",
            "ar-aging-1-2028-02-29.csv",
        ),
        (
            "ap-aging",
            "ap_aging",
            "A/P Aging Summary",
            "as_of=2028-02-29",
            "\"Example, Vendor\",30.04,0.00,0.00,0.00,0.00,30.04",
            "ap-aging-1-2028-02-29.csv",
        ),
        (
            "vendor-1099",
            "vendor_1099",
            "Vendor 1099 Summary",
            "preset=last_year",
            "\"Example, Vendor\",\"1 Example Road, Albany, NY\",1,20.01",
            "vendor-1099-1-2027-12-31.csv",
        ),
    ] {
        let path = format!("{report_path}{slug}/");
        assert!(hub.contains(&path) && hub.contains(title), "{hub}");
        let target = format!("{path}?{query}");
        let rendered = screens::render(&mut f.app, &target).unwrap();
        assert_eq!(rendered.status, 200);
        assert!(rendered.html.contains(title) && rendered.html.contains("Download CSV"));
        let print_target = format!("{target}&output=print");
        let print = screens::render(&mut f.app, &print_target).unwrap();
        assert_eq!(print.status, 200);
        assert!(print.html.contains("report-paper") && print.html.contains(title));
        let outcome = f
            .app
            .run(Command::SubmitForm {
                path: print_target,
                token: token(&print.html).into(),
                fields: vec![("action".into(), "desktop_print".into())],
            })
            .unwrap();
        assert!(matches!(outcome, Outcome::Done { data } if data["desktop_print"] == true));
        let direct = screens::render(&mut f.app, &format!("{target}&output=csv")).unwrap();
        let filters = query
            .split_once('=')
            .map(|(k, v)| vec![(k.into(), v.into())])
            .unwrap();
        let export = f
            .app
            .run(Command::ExportReport {
                company_id: c.clone(),
                report: route.into(),
                filters,
            })
            .unwrap();
        let Outcome::Export {
            filename: actual,
            contents,
        } = export
        else {
            panic!("{export:?}")
        };
        assert_eq!(actual, filename);
        assert_eq!(contents, direct.html);
        assert!(contents.contains(expected), "{contents}");
    }
    for (slug, party, party_id, documents, document_id) in [
        ("ar-aging", "customers", &f.customer, "invoices", &f.invoice),
        ("ap-aging", "vendors", &f.vendor, "bills", &f.bill),
    ] {
        let path = format!("{report_path}{slug}/");
        let screen = screens::render(&mut f.app, &path).unwrap();
        assert!(screen.html.contains("value=\"2028-02-29\""));
        assert!(
            screen
                .html
                .contains(&format!("/c/{c}/lists/{party}/{party_id}/"))
        );
        assert!(
            screen
                .html
                .contains(&format!("/c/{c}/{documents}/{document_id}/"))
        );
        let historical = screens::render(&mut f.app, &format!("{path}?as_of=2027-11-30")).unwrap();
        assert!(historical.html.contains("No open"));
        let invalid = screens::render(&mut f.app, &format!("{path}?as_of=bad")).unwrap();
        assert_eq!(invalid.status, 400);
        assert!(invalid.html.contains("Enter a valid date"));
        assert!(!invalid.html.contains("Download CSV"));
    }
    let path = format!("{report_path}vendor-1099/");
    let current = screens::render(&mut f.app, &path).unwrap();
    assert!(current.html.contains("This Year") && current.html.contains("Last Year"));
    assert!(current.html.contains("No vendor payments"));
    let custom = screens::render(
        &mut f.app,
        &format!("{path}?preset=custom&start=2027-12-31&end=2027-12-31"),
    )
    .unwrap();
    assert!(custom.html.contains("20.01") && custom.html.contains("1 Example Road"));
    assert_eq!(audit(&mut f.app), before);
}

#[test]
fn sales_tax_owed_uses_ny_presets_and_preserves_custom_date_validation() {
    let mut f = fixture();
    let c = f.company.to_db();
    let path = format!("/c/{c}/sales-tax/owed/");
    let hub = screens::render(&mut f.app, &format!("/c/{c}/reports/"))
        .unwrap()
        .html;
    assert!(hub.contains("Sales tax owed") && hub.contains(&path));
    for (preset, start, end) in [
        ("ny_dec_feb", "2027-12-01", "2028-02-29"),
        ("ny_mar_may", "2027-03-01", "2027-05-31"),
        ("ny_jun_aug", "2027-06-01", "2027-08-31"),
        ("ny_sep_nov", "2027-09-01", "2027-11-30"),
        ("calendar_quarter", "2028-01-01", "2028-03-31"),
        ("month", "2028-02-01", "2028-02-29"),
        ("year", "2028-01-01", "2028-12-31"),
    ] {
        let page = screens::render(
            &mut f.app,
            &format!("{path}?preset={preset}&start=2000-01-01&end=2000-02-01"),
        )
        .unwrap();
        assert_eq!(page.status, 200);
        for date in [start, end] {
            assert!(
                page.html.contains(&format!("value=\"{date}\"")),
                "{preset} {date}"
            );
        }
        if preset == "ny_dec_feb" {
            let breakdown = page
                .html
                .split_once("Sales by tax rate and jurisdiction")
                .unwrap()
                .1
                .split("</section>")
                .next()
                .unwrap();
            for value in [
                "Taxable sales",
                "Non-taxable / exempt sales",
                "Tax collected",
                "Example tax (6%)",
                "Example jurisdiction",
                "100.00",
                "20.00",
                "6.00",
            ] {
                assert!(breakdown.contains(value), "{value}: {breakdown}");
            }
        }
    }
    let custom = screens::render(
        &mut f.app,
        &format!("{path}?preset=custom&start=2027-12-01&end=2027-12-01"),
    )
    .unwrap();
    assert_eq!(custom.status, 200);
    assert!(custom.html.contains("2027-12-01 to 2027-12-01"));
    let invalid = screens::render(
        &mut f.app,
        &format!("{path}?preset=custom&start=2027-12-01"),
    )
    .unwrap();
    assert_eq!(invalid.status, 400);
    assert!(invalid.html.contains("This field is required."));
    assert!(!invalid.html.contains("Sales by tax rate and jurisdiction"));
}
