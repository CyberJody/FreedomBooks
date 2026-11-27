mod support;
mod worker_support;

use fb_app::{App, Command, Outcome, commands::StarterChart, identity::Role};
use fb_core::directory::{self, Model};
use serde_json::json;
use worker_support::{account, fixture, key, page, redirect, submit, tax_on};

fn post_renderer(app: &mut App, path: &str, fields: &[(&str, &str)]) -> fb_app::AppResult<Outcome> {
    let token = app.tokens.mint(path);
    app.run_renderer(Command::SubmitForm {
        path: path.into(),
        token,
        fields: fields
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect(),
    })
}

fn invalid(outcome: Outcome) -> String {
    let Outcome::Invalid { html } = outcome else {
        panic!("expected invalid form, got {outcome:?}")
    };
    html
}

#[test]
fn periods_settings_are_scoped_and_confirm_reopening_by_date() {
    let (mut app, co) = fixture(StarterChart::Standard);
    let path = format!("/c/{co}/settings/accounting-periods/");
    for link in [
        format!("/c/{co}/settings/"),
        format!("/c/{co}/accounting-setup/"),
    ] {
        assert!(page(&mut app, &link).contains(&path));
    }
    assert!(page(&mut app, &path).contains("All dates open"));
    let values = [
        ("expected_revision", "0"),
        ("fiscal_year_start_month", "11"),
        ("closing_date", "2028-02-29"),
    ];
    assert_eq!(
        redirect(post_renderer(&mut app, &path, &values).unwrap()),
        path
    );
    assert!(page(&mut app, &path).contains("2028-02-29"));
    assert!(
        invalid(post_renderer(&mut app, &path, &values).unwrap())
            .contains("Accounting periods changed.")
    );
    let reopen = [
        ("expected_revision", "1"),
        ("fiscal_year_start_month", "11"),
        ("closing_date", "2028-02-27"),
    ];
    assert!(invalid(post_renderer(&mut app, &path, &reopen).unwrap()).contains("id_reason_error"));
    let mut reopen = reopen.to_vec();
    reopen.push(("reason", "Enter adjusting entries"));
    let Outcome::Page { html, status: 200 } = post_renderer(&mut app, &path, &reopen).unwrap()
    else {
        panic!("expected confirmation")
    };
    assert!(html.contains("2028-02-28 through 2028-02-29"));
    assert!(page(&mut app, &path).contains("2028-02-29"));
    reopen.push(("action", "confirm_reopen"));
    assert_eq!(
        redirect(post_renderer(&mut app, &path, &reopen).unwrap()),
        path
    );
    let history = page(&mut app, &format!("/c/{co}/history/"));
    assert!(history.contains("Accounting period reopened"));
    assert!(history.contains("Enter adjusting entries"));

    app.today = || chrono::NaiveDate::from_ymd_opt(2028, 2, 29).unwrap();
    let report = format!("/c/{co}/reports/profit-and-loss/?preset=this_year");
    let html = page(&mut app, &report);
    assert!(html.contains("2027-11-01") && html.contains("2028-10-31"));
    assert!(page(&mut app, &format!("{report}&output=csv")).contains("2027-11-01"));
    let audit = app.open_mut().unwrap().owner;
    let password = "Synthetic operator password";
    for (name, role) in [
        ("Bookkeeping operator", Role::Bookkeeper),
        ("Read-only operator", Role::Readonly),
    ] {
        let admin = "Pat Tester";
        if app.authenticated_session().unwrap().actor != audit {
            app.sign_out();
            app.sign_in(admin, "Individual synthetic fixture password")
                .unwrap();
        }
        app.save_desktop_user(
            None,
            name,
            role,
            true,
            Some(password),
            &[co.parse().unwrap()],
        )
        .unwrap();
        app.sign_out();
        app.sign_in(name, password).unwrap();
        let html = page(&mut app, &path);
        assert!(html.contains("2028-02-27") && !html.contains("name=\"closing_date\""));
        assert!(post_renderer(&mut app, &path, &reopen).is_err());
    }
}

fn selected(html: &str, field: &str, value: &str) -> bool {
    let marker = format!("name=\"{field}\"");
    let Some((_, options)) = html.split_once(&marker) else {
        return false;
    };
    let Some((options, _)) = options.split_once("</select>") else {
        return false;
    };
    options
        .split("<option ")
        .skip(1)
        .filter_map(|option| option.split_once('>'))
        .any(|(attrs, _)| {
            attrs.contains(&format!("value=\"{value}\"")) && attrs.contains("selected")
        })
}

#[test]
fn fresh_sales_forms_prefill_customer_terms_and_tax_and_preserve_explicit_values() {
    let (mut app, co) = fixture(StarterChart::Standard);
    let customer_form = format!("/c/{co}/lists/customers/new/");
    let html = page(&mut app, &customer_form);
    assert!(html.contains("Default payment terms") && html.contains("Net 15"));
    let person = redirect(submit(
        &mut app,
        &customer_form,
        &[
            ("display_name", "Local library"),
            ("tax_status", "EXEMPT"),
            ("exemption_number", "LIB-81"),
            ("default_terms", "15"),
        ],
    ));
    let customer = person.trim_end_matches('/').rsplit('/').next().unwrap();
    assert!(page(&mut app, &person).contains("Net 15"));
    let edit = format!("{person}edit/");
    redirect(submit(
        &mut app,
        &edit,
        &[("display_name", "Local library")],
    ));
    assert!(page(&mut app, &person).contains("Net 15"));
    let _ = tax_on(&mut app, &co);

    let invoice = format!("/c/{co}/invoices/new/");
    let html = page(&mut app, &format!("{invoice}?customer={customer}"));
    assert!(selected(&html, "customer", customer) && selected(&html, "terms", "15"));
    assert!(html.contains("name=\"due_date\" value=\"2026-12-08\""));
    assert!(selected(&html, "tax_mode", "NONE"));
    assert!(
        html.contains("LIB-81")
            && html.contains("application/json")
            && html.contains("customer-defaults.js")
    );

    let estimate = page(
        &mut app,
        &format!("/c/{co}/estimates/new/?customer={customer}"),
    );
    assert!(selected(&estimate, "tax_mode", "NONE"));
    assert!(estimate.contains("name=\"expiry\" value=\"2026-12-23\""));
    assert!(!estimate.contains("name=\"due_date\"") && !estimate.contains("name=\"terms\""));
    let receipt = page(
        &mut app,
        &format!("/c/{co}/sales-receipts/new/?customer={customer}"),
    );
    assert!(selected(&receipt, "tax_mode", "NONE"));
    assert!(receipt.contains("name=\"method\"") && !receipt.contains("name=\"terms\""));

    let request = key();
    let income = account(&mut app, &co, "4000");
    let mut values = vec![
        ("request_key", request.as_str()),
        ("expected_revision", "0"),
        ("customer", customer),
        ("date", "2028-02-29"),
        ("due_date", "2028-03-15"),
        ("terms", "0"),
        ("tax_mode", "NONE"),
        ("lines-TOTAL_FORMS", "1"),
        ("lines-INITIAL_FORMS", "0"),
        ("lines-0-description", "Library work"),
        ("lines-0-quantity", "1"),
        ("lines-0-rate", "50.00"),
        ("lines-0-income_account", income.as_str()),
        ("editor_action", "save"),
    ];
    let location = redirect(submit(&mut app, &invoice, &values));
    let draft = format!("{location}edit/");
    let html = page(&mut app, &draft);
    assert!(selected(&html, "terms", "0"));
    assert!(html.contains("name=\"due_date\" value=\"2028-02-29\""));
    let open = app.open_mut().unwrap();
    directory::save_record(
        &mut open.book,
        open.owner,
        co.parse().unwrap(),
        Model::Customer,
        &json!({"default_terms":"60","tax_status":"TAXABLE"}),
        Some(customer),
    )
    .unwrap();
    let html = page(&mut app, &draft);
    assert!(selected(&html, "terms", "0") && selected(&html, "tax_mode", "NONE"));
    let html = page(&mut app, &format!("{invoice}?customer={customer}"));
    assert!(selected(&html, "terms", "60") && !selected(&html, "tax_mode", "NONE"));

    let custom_key = key();
    values.extend([
        ("request_key", custom_key.as_str()),
        ("terms", ""),
        ("due_date", "2028-03-17"),
        ("lines-0-rate", "invalid"),
    ]);
    let html = invalid(submit(&mut app, &invoice, &values));
    assert!(html.contains("2028-03-17") && html.contains("Library work"));
    values.push(("lines-0-rate", "50.00"));
    let custom = redirect(submit(&mut app, &invoice, &values));
    let html = page(&mut app, &format!("{custom}edit/"));
    assert!(selected(&html, "terms", "") && html.contains("2028-03-17"));
    redirect(submit(
        &mut app,
        &edit,
        &[("display_name", "Local library"), ("default_terms", "")],
    ));
    assert!(selected(
        &page(&mut app, &format!("{invoice}?customer={customer}")),
        "terms",
        "30"
    ));
}

#[test]
fn estimate_conversion_resolves_terms_without_changing_the_accepted_tax_snapshot() {
    let (mut app, co) = fixture(StarterChart::Standard);
    let company = co.parse().unwrap();
    let _income = account(&mut app, &co, "4000");
    let open = app.open_mut().unwrap();
    let customer = directory::save_record(
        &mut open.book,
        open.owner,
        company,
        Model::Customer,
        &json!({"display_name":"Synthetic buyer","default_terms":"15","tax_status":"EXEMPT"}),
        None,
    )
    .unwrap();
    let customer = customer["id"].as_str().unwrap();
    let estimate = fb_core::sales::save_estimate(&mut open.book, open.owner, company, &json!({
        "request_key":key(),"date":"2026-11-01","expiry":"2026-12-01","customer_id":customer,"tax_mode":"NONE",
        "lines":[{"description":"Estimated work","quantity":"1","rate":"5.00","taxable":false}]
    })).unwrap();
    let estimate_id = estimate["id"].as_str().unwrap();
    let accepted = fb_core::sales::transition_estimate(
        &mut open.book,
        open.owner,
        company,
        estimate_id,
        "ACCEPTED",
        Some(1),
        "",
    )
    .unwrap();
    directory::save_record(
        &mut open.book,
        open.owner,
        company,
        Model::Customer,
        &json!({"default_terms":"60","tax_status":"TAXABLE"}),
        Some(customer),
    )
    .unwrap();
    let path = format!("/c/{co}/estimates/{estimate_id}/convert/2/");
    let html = page(&mut app, &path);
    assert!(selected(&html, "terms", "60"));
    assert!(html.contains("name=\"due_date\" value=\"2027-01-22\""));
    let input = [
        ("expected_revision", "2"),
        ("date", "2026-11-29"),
        ("terms", "15"),
        ("due_date", "2030-12-31"),
    ];
    let location = redirect(submit(&mut app, &path, &input));
    assert_eq!(redirect(submit(&mut app, &path, &input)), location);
    let invoice_id = location.trim_end_matches('/').rsplit('/').next().unwrap();
    let invoice = app
        .open_mut()
        .unwrap()
        .book
        .read(|q| fb_core::sales::invoice(q, company, invoice_id))
        .unwrap();
    assert_eq!(invoice["current"]["due_date"], "2026-12-14");
    assert_eq!(
        invoice["current"]["tax_snapshot"],
        accepted["current"]["tax_snapshot"]
    );
    assert_eq!(invoice["lines"][0]["taxable"], false);
}

#[test]
fn a_refused_sales_receipt_keeps_the_document_fields_and_marks_its_date() {
    let (mut app, co) = fixture(StarterChart::Standard);
    let customer = worker_support::customer(&mut app, &co, "Synthetic receipt customer");
    let income = account(&mut app, &co, "4000");
    let settings = format!("/c/{co}/settings/accounting-periods/");
    redirect(submit(
        &mut app,
        &settings,
        &[
            ("expected_revision", "0"),
            ("fiscal_year_start_month", "1"),
            ("closing_date", "2026-11-23"),
        ],
    ));
    let request = key();
    let path = format!("/c/{co}/sales-receipts/new/");
    let html = invalid(submit(
        &mut app,
        &path,
        &[
            ("request_key", &request),
            ("date", "2026-11-23"),
            ("customer", &customer),
            ("tax_mode", "NONE"),
            ("method", "CASH"),
            ("destination", "UNDEPOSITED"),
            ("lines-TOTAL_FORMS", "1"),
            ("lines-INITIAL_FORMS", "0"),
            ("lines-0-description", "Synthetic receipt line"),
            ("lines-0-quantity", "1"),
            ("lines-0-rate", "20.00"),
            ("lines-0-income_account", &income),
        ],
    ));
    assert!(
        html.contains("id_date_error") && html.contains("closed through 2026-11-23"),
        "{html}"
    );
    assert!(html.contains("Synthetic receipt line"));
}

#[test]
fn editing_taxed_draft_with_sales_tax_off_keeps_saved_tax_and_legacy_retries_survive() {
    let (mut app, co) = fixture(StarterChart::Standard);
    let company: fb_core::CompanyId = co.parse().unwrap();
    let customer = worker_support::customer(&mut app, &co, "Taxed draft customer");
    let income = account(&mut app, &co, "4000");
    let rate = {
        let open = app.open_mut().unwrap();
        let rate = fb_core::salestax::save_rate(
            &mut open.book,
            open.owner,
            company,
            &json!({"name":"State","description":"State tax","rate":"6","new_agency_name":"State Agency","expected_revision":0}),
        )
        .unwrap();
        fb_core::salestax::save_settings(
            &mut open.book,
            open.owner,
            company,
            &json!({"charge_tax":true,"default_item_id":rate["id"],"owed_basis":"ACCRUAL","filing_frequency":"QUARTERLY","expected_revision":0}),
        )
        .unwrap();
        rate
    };
    let rate_id = rate["id"].as_str().unwrap();
    let inv = {
        let open = app.open_mut().unwrap();
        fb_core::sales::save_invoice(
            &mut open.book,
            open.owner,
            company,
            &json!({
                "request_key": key(),
                "customer_id": customer,
                "date": "2026-11-01",
                "due_date": "2026-11-20",
                "tax_mode": "ITEM",
                "tax_item_id": rate_id,
                "lines": [{"description":"Taxed service","quantity":"1","rate":"100.00","taxable":true,"income_account_id":income}]
            }),
        )
        .unwrap()
    };
    let est = {
        let open = app.open_mut().unwrap();
        fb_core::sales::save_estimate(
            &mut open.book,
            open.owner,
            company,
            &json!({
                "request_key": key(),
                "customer_id": customer,
                "date": "2026-11-01",
                "expiry": "2026-12-01",
                "tax_mode": "ITEM",
                "tax_item_id": rate_id,
                "lines": [{"description":"Taxed quote","quantity":"1","rate":"100.00","taxable":true}]
            }),
        )
        .unwrap()
    };
    // Turn company sales tax OFF while the live rate itself remains active.
    {
        let open = app.open_mut().unwrap();
        let current = fb_core::salestax::settings_for(open.book.connection(), company).unwrap();
        fb_core::salestax::save_settings(
            &mut open.book,
            open.owner,
            company,
            &json!({"charge_tax":false,"default_item_id":null,"owed_basis":"ACCRUAL","filing_frequency":"QUARTERLY","expected_revision":current["revision"]}),
        )
        .unwrap();
    }
    let inv_edit = format!("/c/{co}/invoices/{}/edit/", inv["id"].as_str().unwrap());
    let est_edit = format!("/c/{co}/estimates/{}/edit/", est["id"].as_str().unwrap());
    let inv_html = page(&mut app, &inv_edit);
    let est_html = page(&mut app, &est_edit);
    assert!(selected(&inv_html, "tax_mode", "KEEP"), "{inv_html}");
    assert!(selected(&est_html, "tax_mode", "KEEP"), "{est_html}");

    // Legacy retry: invoice saved in format-97 style with custom due_date 2026-11-20;
    // retrying the POST with terms=30 and due_date=2026-11-20 must succeed as an exact retry.
    let legacy_key = key();
    let legacy_inv = {
        let open = app.open_mut().unwrap();
        fb_core::sales::save_invoice(
            &mut open.book,
            open.owner,
            company,
            &json!({
                "request_key": legacy_key,
                "customer_id": customer,
                "date": "2026-11-01",
                "due_date": "2026-11-20",
                "tax_mode": "NONE",
                "lines": [{"description":"Legacy retry","quantity":"1","rate":"50.00","taxable":false,"income_account_id":income}]
            }),
        )
        .unwrap()
    };
    let invoice_new = format!("/c/{co}/invoices/new/");
    let retried = redirect(submit(
        &mut app,
        &invoice_new,
        &[
            ("request_key", legacy_key.as_str()),
            ("expected_revision", "0"),
            ("customer", customer.as_str()),
            ("date", "2026-11-01"),
            ("due_date", "2026-11-20"),
            ("terms", "30"),
            ("tax_mode", "NONE"),
            ("lines-TOTAL_FORMS", "1"),
            ("lines-INITIAL_FORMS", "0"),
            ("lines-0-description", "Legacy retry"),
            ("lines-0-quantity", "1"),
            ("lines-0-rate", "50.00"),
            ("lines-0-income_account", income.as_str()),
            ("editor_action", "save"),
        ],
    ));
    assert!(retried.contains(legacy_inv["id"].as_str().unwrap()), "{retried}");

    // Legacy estimate conversion retry with terms=30 and custom due_date=2026-11-20.
    let est_id = est["id"].as_str().unwrap();
    let converted = {
        let open = app.open_mut().unwrap();
        fb_core::sales::transition_estimate(&mut open.book, open.owner, company, est_id, "ACCEPTED", Some(1), "").unwrap();
        fb_core::sales::convert_estimate(&mut open.book, open.owner, company, est_id, Some(2), "2026-11-01", "2026-11-20").unwrap()
    };
    let convert_path = format!("/c/{co}/estimates/{est_id}/convert/2/");
    let retried_convert = redirect(submit(
        &mut app,
        &convert_path,
        &[
            ("expected_revision", "2"),
            ("date", "2026-11-01"),
            ("due_date", "2026-11-20"),
            ("terms", "30"),
        ],
    ));
    assert!(retried_convert.contains(converted["id"].as_str().unwrap()), "{retried_convert}");
}

#[test]
fn closed_period_refusal_is_shown_on_bill_post_and_check_record_screens() {
    let (mut app, co) = fixture(StarterChart::Standard);
    let company: fb_core::CompanyId = co.parse().unwrap();
    let vendor = {
        let open = app.open_mut().unwrap();
        directory::save_record(
            &mut open.book,
            open.owner,
            company,
            Model::Vendor,
            &json!({"display_name":"Closed Period Vendor"}),
            None,
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let bank = account(&mut app, &co, "1000");
    let expense = account(&mut app, &co, "6000");
    let (bill_id, check_id) = {
        let open = app.open_mut().unwrap();
        let bill = fb_core::payables::save_bill(
            &mut open.book,
            open.owner,
            company,
            &json!({
                "request_key": key(),
                "vendor_id": vendor,
                "date": "2026-11-20",
                "due_date": "2026-12-20",
                "lines": [{"description":"Supplies","quantity":"1","rate":"25.00","expense_account_id":expense}]
            }),
        )
        .unwrap();
        let check = fb_core::checks::save_check(
            &mut open.book,
            open.owner,
            company,
            &json!({
                "request_key": key(),
                "bank_id": bank,
                "payee_kind": "VENDOR",
                "vendor_id": vendor,
                "date": "2026-11-20",
                "lines": [{"tab":"EXPENSE","account_id":expense,"description":"Supplies","rate":"15.00"}]
            }),
        )
        .unwrap();
        (
            bill["id"].as_str().unwrap().to_owned(),
            check["id"].as_str().unwrap().to_owned(),
        )
    };
    // Close books through 2026-11-23.
    let settings = format!("/c/{co}/settings/accounting-periods/");
    redirect(submit(
        &mut app,
        &settings,
        &[
            ("expected_revision", "0"),
            ("fiscal_year_start_month", "1"),
            ("closing_date", "2026-11-23"),
        ],
    ));
    let bill_token = match app
        .run(fb_app::Command::ReviewBill {
            company_id: co.clone(),
            bill_id: bill_id.clone(),
            expected_revision: 1,
        })
        .unwrap()
    {
        fb_app::Outcome::Done { data } => data["token"].as_str().unwrap().to_owned(),
        other => panic!("{other:?}"),
    };
    let bill_post_html = invalid(submit(
        &mut app,
        &format!("/c/{co}/bills/{bill_id}/post/"),
        &[
            ("expected_revision", "1"),
            ("review_token", bill_token.as_str()),
        ],
    ));
    assert!(bill_post_html.contains("closed through 2026-11-23"), "{bill_post_html}");

    let check_token = match app
        .run(fb_app::Command::ReviewCheck {
            company_id: co.clone(),
            check_id: check_id.clone(),
            expected_revision: 1,
        })
        .unwrap()
    {
        fb_app::Outcome::Done { data } => data["token"].as_str().unwrap().to_owned(),
        other => panic!("{other:?}"),
    };
    let check_record_html = invalid(submit(
        &mut app,
        &format!("/c/{co}/checks/{check_id}/record/"),
        &[
            ("expected_revision", "1"),
            ("review_token", check_token.as_str()),
        ],
    ));
    assert!(check_record_html.contains("closed through 2026-11-23"), "{check_record_html}");
}
