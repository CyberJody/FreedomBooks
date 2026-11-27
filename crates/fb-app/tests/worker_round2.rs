//! Bookkeeper test rounds 4 and 5 of 2026-11-23 (internal test notes, fix list items 22-36).
mod support;
mod worker_support;
use fb_app::commands::StarterChart;
use serde_json::json;
use worker_support::*;

/// A recorded check of `amount` from `bank`, numbered `number`.
fn check(app: &mut fb_app::App, c: &str, bank: &str, date: &str, number: &str, amount: &str) -> String {
    let expense = account(app, c, "6100");
    let id = run(app, json!({"type":"SaveCheck","company_id":c,"values":{"check_id":null,"expected_revision":0,"request_key":key(),"bank_id":bank,"date":date,"check_number":number,"print_later":false,"payee_kind":"OTHER","vendor_id":null,"payee_name":"Corner Hardware (synthetic)","address":"","memo":"Supplies","refresh_payee":false,"keep_archived":false,
        "lines":[{"tab":"EXPENSE","origin_position":null,"account_id":expense,"item_id":null,"description":"Supplies","quantity":"1","rate":amount,"refresh_item":false,"keep_archived":false}]}}))["id"].as_str().unwrap().to_owned();
    let review = run(app, json!({"type":"ReviewCheck","company_id":c,"check_id":id,"expected_revision":1}));
    run(app, json!({"type":"RecordCheck","company_id":c,"check_id":id,"expected_revision":1,"review_token":review["token"]}));
    id
}
/// The value of the option selected in the `<select name=…>`, if any (a selected blank "Choose…" is none).
fn selected(html: &str, name: &str) -> Option<String> {
    let select = html.split_once(&format!("name=\"{name}\"")).unwrap_or_else(|| panic!("no {name}")).1.split("</select>").next().unwrap();
    select.split("<option").find(|o| o.split('>').next().unwrap().contains(" selected")).map(|o| o.split_once("value=\"").unwrap().1.split('"').next().unwrap().to_owned()).filter(|v| !v.is_empty())
}
fn invalid(o: fb_app::Outcome) -> String {
    match o {
        fb_app::Outcome::Invalid { html } => html,
        fb_app::Outcome::Page { html, status } if status >= 400 => html,
        o => panic!("expected the form again: {o:?}"),
    }
}

/// Item 22: the Void check page said bank-register clearing and reconciliation did not exist.
#[test]
fn the_void_check_page_says_what_clearing_and_reconciliation_mean_for_a_void() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let bank = account(&mut app, &c, "1000");
    let id = check(&mut app, &c, &bank, "2026-05-20", "124", "57.00");
    let html = page(&mut app, &format!("/c/{c}/checks/{id}/void/"));
    assert!(!html.contains("do not exist yet"), "{html}");
    assert!(html.contains("A check marked cleared in its bank register must be marked not cleared there first. A check in a finished reconciliation can be voided only after that reconciliation is undone (Banking → Reconcile → Undo last reconciliation), then mark it not cleared."), "{html}");
}

fn directory_save(app: &mut fb_app::App, c: &str, kind: &str, id: Option<&str>, name: &str) -> Result<String, String> {
    let mut values = json!({"kind":kind,"display_name":name,"business_name":"","contact_name":"","email":"","phone":"","billing_address":"","notes":""});
    if kind == "customer" {
        values["tax_status"] = json!("TAXABLE");
        values["exemption_number"] = json!("");
    } else {
        values["default_expense_account"] = json!(null);
        values["default_terms"] = json!(null);
        values["expense_shown"] = json!(false);
        values["terms_shown"] = json!(false);
    }
    let command = json!({"type":"SaveDirectoryRecord","company_id":c,"record_id":id,"values":values});
    match app.run(fb_app::commands::parse_command(&command.to_string()).unwrap()) {
        Ok(fb_app::Outcome::Done { data }) => Ok(data["id"].as_str().unwrap().to_owned()),
        Err(e) => Err(e.to_string()),
        Ok(o) => panic!("{o:?}"),
    }
}
fn set_active(app: &mut fb_app::App, c: &str, kind: &str, id: &str, active: bool) {
    run(app, json!({"type":"SetDirectoryActive","company_id":c,"kind":kind,"record_id":id,"active":active}));
}

/// Item 23: a second "Juniper Print (TEST)" customer saved with no warning.
#[test]
fn a_new_or_renamed_customer_or_vendor_cannot_take_an_active_ones_name() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let juniper = directory_save(&mut app, &c, "customer", None, "Juniper Print (TEST)").unwrap();
    // The same name, in another case and with outer spaces, is refused for a new customer.
    let refused = directory_save(&mut app, &c, "customer", None, "  juniper print (test) ").unwrap_err();
    assert!(refused.contains("A customer named “Juniper Print (TEST)” already exists. Choose a different name."), "{refused}");
    // ... and for a renamed one.
    let cedar = directory_save(&mut app, &c, "customer", None, "Cedar Studio (TEST)").unwrap();
    assert!(directory_save(&mut app, &c, "customer", Some(&cedar), "JUNIPER PRINT (TEST)").unwrap_err().contains("already exists"));
    // A customer keeps its own name on an edit; a vendor may share a customer's name; vendors follow the same rule.
    directory_save(&mut app, &c, "customer", Some(&juniper), "Juniper Print (TEST)").unwrap();
    let vendor = directory_save(&mut app, &c, "vendor", None, "Juniper Print (TEST)").unwrap();
    let refused = directory_save(&mut app, &c, "vendor", None, "Juniper print (TEST)").unwrap_err();
    assert!(refused.contains("A vendor named “Juniper Print (TEST)” already exists. Choose a different name."), "{refused}");
    // An archived record does not hold its name.
    set_active(&mut app, &c, "vendor", &vendor, false);
    let second = directory_save(&mut app, &c, "vendor", None, "Juniper Print (TEST)").unwrap();
    // Review 2, N1: nor may the archived one be reactivated while the second holds the name. The form
    // says so, and nothing changes.
    let active = |app: &mut fb_app::App, id: &str| -> bool { app.open_mut().unwrap().book.read(|q| q.query_row("SELECT active FROM directory_vendor WHERE id=?1", [id], |r| r.get(0))).unwrap() };
    let audits = |app: &mut fb_app::App| -> i64 { app.open_mut().unwrap().book.read(|q| q.query_row("SELECT count(*) FROM ledger_auditevent", [], |r| r.get(0))).unwrap() };
    let before = audits(&mut app);
    let command = json!({"type":"SetDirectoryActive","company_id":c,"kind":"vendor","record_id":vendor,"active":true});
    let refused = app.run(fb_app::commands::parse_command(&command.to_string()).unwrap()).unwrap_err().to_string();
    assert!(refused.contains("An active vendor named “Juniper Print (TEST)” exists. Rename one of them first."), "{refused}");
    let html = invalid(submit(&mut app, &format!("/c/{c}/lists/vendors/{vendor}/status/"), &[("active", "1")]));
    assert!(html.contains("<li>An active vendor named “Juniper Print (TEST)” exists. Rename one of them first.</li>"), "{html}");
    assert!(!active(&mut app, &vendor) && audits(&mut app) == before, "nothing changed");
    // Renamed, it may come back.
    directory_save(&mut app, &c, "vendor", Some(&vendor), "Juniper Print West (TEST)").unwrap();
    set_active(&mut app, &c, "vendor", &vendor, true);
    // Duplicates made before the rule (a book from before it: made here by hand) stay, and stay editable
    // while they keep their name. No record is changed by the rule.
    app.open_mut().unwrap().book.connection().execute("UPDATE directory_vendor SET display_name='Juniper Print (TEST)' WHERE id=?1", [&vendor]).unwrap();
    directory_save(&mut app, &c, "vendor", Some(&vendor), "Juniper Print (TEST)").unwrap();
    directory_save(&mut app, &c, "vendor", Some(&second), "Juniper Print (TEST)").unwrap();
    let names: Vec<String> = app.open_mut().unwrap().book.read(|q| {
        let mut st = q.prepare("SELECT display_name FROM directory_vendor WHERE company_id=?1 AND active=1")?;
        let v = st.query_map([&c], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?;
        Ok::<_, fb_core::FbError>(v)
    }).unwrap();
    assert_eq!(names.iter().filter(|n| *n == "Juniper Print (TEST)").count(), 2);

    // The form says so at the name, and keeps what was typed.
    let path = format!("/c/{c}/lists/customers/new/");
    let html = page(&mut app, &path);
    assert!(html.contains("Required. No other active customer can have the same name."), "{html}");
    let html = invalid(submit(&mut app, &path, &[("display_name", "Juniper Print (TEST)"), ("email", "orders@juniper.example"), ("tax_status", "TAXABLE")]));
    let name_field = html.split_once("name=\"display_name\"").unwrap().0.rsplit("<div class=\"field").next().unwrap().to_owned();
    assert!(name_field.contains("A customer named “Juniper Print (TEST)” already exists. Choose a different name."), "{name_field}");
    assert!(html.contains("orders@juniper.example"));
    assert!(page(&mut app, &format!("/c/{c}/lists/vendors/new/")).contains("Required. No other active vendor can have the same name."));
}

/// Item 24: the Customers list had no balance column (the Vendors list has one).
#[test]
fn the_customers_list_shows_what_each_customer_owes() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let birch = customer(&mut app, &c, "Birch Cafe (synthetic)");
    customer(&mut app, &c, "Aspen Dental (synthetic)");
    let income = account(&mut app, &c, "4000");
    let id = invoice(&mut app, &c, &birch, &income, None);
    assert!(matches!(post_invoice(&mut app, &c, &id).unwrap(), fb_app::Outcome::Done { .. }));
    // A draft owes nothing yet.
    invoice(&mut app, &c, &birch, &income, None);
    let html = page(&mut app, &format!("/c/{c}/lists/customers/"));
    assert!(html.contains("<th class=\"amount\">Open balance</th><th class=\"amount\">Overdue</th>"), "{html}");
    let row = |name: &str| html.split(name).nth(1).unwrap().split("</tr>").next().unwrap().to_owned();
    // 10 × 65.00, due 2026-04-01: overdue on 2026-11-23.
    assert!(row("Birch Cafe (synthetic)").contains("<td class=\"amount\">650.00</td><td class=\"amount overdue\">650.00</td>"), "{}", row("Birch Cafe"));
    assert!(row("Aspen Dental (synthetic)").contains("<td class=\"amount\">—</td><td class=\"amount\">—</td>"), "{}", row("Aspen Dental"));
}

/// A posted bill of `amount` from `vendor`, dated `date` and due `due`.
fn bill(app: &mut fb_app::App, c: &str, vendor: &str, date: &str, due: &str, amount: &str) -> String {
    let expense = account(app, c, "6100");
    let id = run(app, json!({"type":"SaveBill","company_id":c,"values":{"bill_id":null,"expected_revision":0,"request_key":key(),"vendor_id":vendor,"date":date,"due_date":due,"memo":"","reference":"","supersedes_id":null,"reentry_reason":"","refresh_vendor":false,"keep_archived":false,
        "lines":[{"origin_position":null,"item_id":null,"expense_account_id":expense,"description":"Lumber","quantity":"1","rate":amount,"refresh_item":false,"keep_archived":false}]}}))["id"].as_str().unwrap().to_owned();
    let review = run(app, json!({"type":"ReviewBill","company_id":c,"bill_id":id,"expected_revision":1}));
    run(app, json!({"type":"PostBill","company_id":c,"bill_id":id,"expected_revision":1,"review_token":review["token"]}));
    id
}
/// What the Pay bills page's script sends for one vendor's payment: its check number and entries.
fn shown(app: &mut fb_app::App, c: &str, check: &str, amount: &str) -> String {
    let (ap, bank) = (account(app, c, "2000"), account(app, c, "1000"));
    let mut entries = [format!("{ap}:{amount}:0.00"), format!("{bank}:0.00:{amount}")];
    entries.sort();
    format!("{check}||{}", entries.join("|"))
}
fn pay_run(html: &str, bank: &str, date: &str, reference: &str, picks: &[(&str, &str)], extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = [("request_key", field_value(html, "request_key").as_str()), ("bank", bank), ("date", date), ("method", "CHECK"), ("reference", reference), ("memo", "")]
        .iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    for (id, amount) in picks {
        v.push(("pay".into(), id.to_string()));
        v.push((format!("amount-{id}"), amount.to_string()));
    }
    v.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    v
}
fn submit_owned(app: &mut fb_app::App, path: &str, fields: &[(String, String)]) -> fb_app::Outcome {
    let token = app.tokens.mint(path);
    app.run(fb_app::Command::SubmitForm { path: path.into(), token, fields: fields.to_vec() }).unwrap()
}
fn field_value(html: &str, name: &str) -> String {
    html.split_once(&format!("name=\"{name}\"")).unwrap_or_else(|| panic!("no {name}")).1.split_once("value=\"").unwrap().1.split('"').next().unwrap().to_owned()
}
fn tr_open<'a>(html: &'a str, bill: &str) -> &'a str {
    html.split_once(&format!("data-bill=\"{bill}\"")).unwrap().1.split('>').next().unwrap()
}

/// Item 25: a refused pay run cleared its "Due on or before" filter, so a bill the filter had hidden
/// came back unticked (a "tick all" would then include it).
#[test]
fn a_refused_pay_run_keeps_its_filters() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let bank = account(&mut app, &c, "1000");
    let timber = directory_save(&mut app, &c, "vendor", None, "Timber Supply (synthetic)").unwrap();
    let valley = directory_save(&mut app, &c, "vendor", None, "Valley Electric (synthetic)").unwrap();
    let may = bill(&mut app, &c, &timber, "2026-05-10", "2026-05-25", "420.00");
    let june = bill(&mut app, &c, &valley, "2026-06-10", "2026-06-30", "88.20");
    let path = format!("/c/{c}/payments/new/");
    let html = page(&mut app, &path);
    assert!(html.contains("name=\"filter_due\" id=\"keep-filter-due\" value=\"\""), "the pay form carries the filters");
    // 500.00 on a 420.00 bill, sent while the page showed bills due on or before 05/31.
    let fields = pay_run(&html, &bank, "2026-05-28", "130", &[(&may, "500.00")], &[("filter_vendor", ""), ("filter_due", "2026-05-31")]);
    let html = invalid(submit_owned(&mut app, &path, &fields));
    assert!(html.contains("This is more than the open balance of 420.00."), "{html}");
    assert!(html.contains("id=\"filter-due\" value=\"2026-05-31\"") && html.contains("id=\"keep-filter-due\" value=\"2026-05-31\""), "{html}");
    assert!(tr_open(&html, &june).ends_with(" hidden"), "the June bill stays out of sight: {}", tr_open(&html, &june));
    assert!(!tr_open(&html, &may).ends_with(" hidden"), "the ticked bill with the problem is in sight");
    // A vendor filter is kept the same way; one that no longer has open bills is dropped.
    let fields = pay_run(&html, &bank, "2026-05-28", "130", &[(&may, "500.00")], &[("filter_vendor", &timber), ("filter_due", "")]);
    let html = invalid(submit_owned(&mut app, &path, &fields));
    assert!(html.contains(&format!("<option value=\"{timber}\" selected>")) && tr_open(&html, &june).ends_with(" hidden"), "{html}");
    let fields = pay_run(&html, &bank, "2026-05-28", "130", &[(&may, "500.00")], &[("filter_vendor", &key()), ("filter_due", "not a date")]);
    let html = invalid(submit_owned(&mut app, &path, &fields));
    assert!(!tr_open(&html, &june).ends_with(" hidden") && html.contains("id=\"filter-due\" value=\"\""), "{html}");
}

/// Item 26: a pay run reused check number 103, from a reversed April payment, without a word. Using a
/// number again stays allowed (a voided number may be reused), but the first send now warns.
#[test]
fn reusing_a_voided_check_number_on_a_bill_payment_warns_first() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let bank = account(&mut app, &c, "1000");
    let timber = directory_save(&mut app, &c, "vendor", None, "Timber Supply (synthetic)").unwrap();
    let april = bill(&mut app, &c, &timber, "2026-04-02", "2026-04-30", "57.00");
    let values = json!({"request_key":key(),"vendor_id":timber,"date":"2026-04-20","amount":"57.00","method":"CHECK","reference":"103","memo":"","bank_id":bank,"allocations":[{"bill_id":april,"amount":"57.00"}]});
    let review = run(&mut app, json!({"type":"ReviewBillPayment","company_id":c,"values":values}));
    let paid = run(&mut app, json!({"type":"RecordBillPayment","company_id":c,"values":values,"review_token":review["token"]}));
    let number: String = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT i.number FROM payables_billpayment p JOIN documents_documentidentity i ON i.id=p.identity_id WHERE p.id=?1", [paid["id"].as_str().unwrap()], |r| r.get(0))).unwrap();
    run(&mut app, json!({"type":"ReverseBillPayment","company_id":c,"payment_id":paid["id"],"expected_revision":1,"date":"2026-04-21","reason":"Wrong amount"}));
    let may = bill(&mut app, &c, &timber, "2026-05-02", "2026-05-30", "75.00");
    let path = format!("/c/{c}/payments/new/");
    let html = page(&mut app, &path);
    let check = shown(&mut app, &c, "103", "75.00");
    let fields = pay_run(&html, &bank, "2026-05-26", "103", &[(&may, "75.00")], &[("filter_vendor", ""), ("filter_due", ""), (&format!("shown-{timber}"), &check)]);
    let html = invalid(submit_owned(&mut app, &path, &fields));
    let count = |app: &mut fb_app::App| -> i64 { app.open_mut().unwrap().book.read(|q| q.query_row("SELECT count(*) FROM payables_billpayment", [], |r| r.get(0))).unwrap() };
    assert_eq!(count(&mut app), 1, "nothing was paid on the first send");
    assert!(html.contains("<strong>Nothing was paid yet.</strong> This check number was used before on this bank account:"), "{html}");
    assert!(html.contains(&format!("<li>Check 103 for Timber Supply (synthetic): used before by {number}, which was reversed.</li>")), "{html}");
    assert!(html.contains("A voided number may be used again. To use it, choose Pay selected bills again."), "{html}");
    let confirm = field_value(&html, "reuse_ok");
    assert_eq!(confirm, format!("{bank}:103"));
    // Sent again as the page now is: paid with check 103.
    let mut again = pay_run(&html, &bank, "2026-05-26", "103", &[(&may, "75.00")], &[("filter_vendor", ""), ("filter_due", ""), (&format!("shown-{timber}"), &check)]);
    again.push(("reuse_ok".into(), confirm));
    let out = submit_owned(&mut app, &path, &again);
    assert!(matches!(out, fb_app::Outcome::Redirect { ref location } if location.contains("?paid=")), "{out:?}");
    let reference: String = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT reference FROM payables_billpayment WHERE date='2026-05-26'", [], |r| r.get(0))).unwrap();
    assert_eq!(reference, "103");
    // A number never used before pays at once.
    let june = bill(&mut app, &c, &timber, "2026-06-02", "2026-06-30", "20.00");
    let html = page(&mut app, &path);
    let check = shown(&mut app, &c, "140", "20.00");
    let fields = pay_run(&html, &bank, "2026-06-05", "140", &[(&june, "20.00")], &[("filter_vendor", ""), ("filter_due", ""), (&format!("shown-{timber}"), &check)]);
    assert!(matches!(submit_owned(&mut app, &path, &fields), fb_app::Outcome::Redirect { .. }));
}

/// The document number (CHK-…, CR-…) of the row `id` in `table`.
fn number_of(app: &mut fb_app::App, table: &str, id: &str) -> String {
    app.open_mut().unwrap().book.read(|q| q.query_row(&format!("SELECT i.number FROM {table} t JOIN documents_documentidentity i ON i.id=t.identity_id WHERE t.id=?1"), [id], |r| r.get(0))).unwrap()
}
fn void_check(app: &mut fb_app::App, c: &str, id: &str, date: &str) {
    let revision: i64 = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT revision FROM checks_check WHERE id=?1", [id], |r| r.get(0))).unwrap();
    run(app, json!({"type":"VoidCheck","company_id":c,"check_id":id,"expected_revision":revision,"date":date,"reason":"Spoiled check (synthetic)"}));
}

/// Item 26 in the fix list's own words (review 2, S2): void a check, then use its number in Pay bills.
/// The first send warns and pays nothing; the second pays with that number.
#[test]
fn void_a_check_then_reuse_its_number_in_pay_bills() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let bank = account(&mut app, &c, "1000");
    let spoiled = check(&mut app, &c, &bank, "2026-05-20", "118", "57.00");
    void_check(&mut app, &c, &spoiled, "2026-05-20");
    let number = number_of(&mut app, "checks_check", &spoiled);
    let timber = directory_save(&mut app, &c, "vendor", None, "Timber Supply (synthetic)").unwrap();
    let may = bill(&mut app, &c, &timber, "2026-05-02", "2026-05-30", "75.00");
    let path = format!("/c/{c}/payments/new/");
    let html = page(&mut app, &path);
    let check = shown(&mut app, &c, "118", "75.00");
    let fields = pay_run(&html, &bank, "2026-05-26", "118", &[(&may, "75.00")], &[("filter_vendor", ""), ("filter_due", ""), (&format!("shown-{timber}"), &check)]);
    let html = invalid(submit_owned(&mut app, &path, &fields));
    let paid = |app: &mut fb_app::App| -> i64 { app.open_mut().unwrap().book.read(|q| q.query_row("SELECT count(*) FROM payables_billpayment", [], |r| r.get(0))).unwrap() };
    assert_eq!(paid(&mut app), 0, "nothing was paid on the first send");
    assert!(html.contains(&format!("<li>Check 118 for Timber Supply (synthetic): used before by {number}, which was voided.</li>")), "{html}");
    assert!(html.contains("A voided number may be used again. To use it, choose Pay selected bills again. Otherwise change the first check number."), "{html}");
    assert!(!html.contains("sales tax payment"), "{html}");
    let mut again = fields.clone();
    again[0].1 = field_value(&html, "request_key");
    again.push(("reuse_ok".into(), field_value(&html, "reuse_ok")));
    assert!(matches!(submit_owned(&mut app, &path, &again), fb_app::Outcome::Redirect { ref location } if location.contains("?paid=")));
    assert_eq!(paid(&mut app), 1);
    let reference: String = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT reference FROM payables_billpayment", [], |r| r.get(0))).unwrap();
    assert_eq!(reference, "118");
}

/// Review 2, S1: check 127 had gone out on a sales tax payment whose Reference was "Check 127". Pay
/// bills said "A voided number may be used again", though nothing was voided. It now says what the
/// Reference means; a voided tax payment says "which was voided".
/// Sales tax on with one rate, a taxed May invoice, and a sales tax payment from `bank` whose Reference is
/// `reference` (that form has no check-number box). Returns the payment's id.
fn tax_paid_by_check(app: &mut fb_app::App, c: &str, bank: &str, reference: &str) -> String {
    redirect(tax_on(app, c));
    run(app, json!({"type":"SaveTaxRate","company_id":c,"item_id":null,"expected_revision":null,"name":"IL Sales Tax (TEST)","description":"Sales tax","rate":"5","agency_id":null,"new_agency_name":"Illinois Dept of Revenue (TEST)"}));
    let (rate, agency): (String, String) = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT id, agency_id FROM salestax_taxitem", [], |r| Ok((r.get(0)?, r.get(1)?)))).unwrap();
    let juniper = customer(app, c, "Juniper Print (synthetic)");
    let income = account(app, c, "4000");
    let id = run(app, json!({"type":"SaveInvoice","company_id":c,"values":{"request_key":key(),"customer_id":juniper,"date":"2026-05-08","due_date":"2026-05-08","memo":"","tax_mode":"ITEM","tax_item_id":rate,
        "lines":[{"description":"Banners","quantity":"1","rate":"1000.00","taxable":true,"income_account_id":income}]}}))["id"].as_str().unwrap().to_owned();
    assert!(matches!(post_invoice(app, c, &id).unwrap(), fb_app::Outcome::Done { .. }));
    let values = json!({"request_key":key(),"agency_id":agency,"date":"2026-06-10","bank_id":bank,"tax_amount":"50.00","penalty_amount":"0","expense_account_id":null,"period_start":"2026-05-01","period_end":"2026-05-31","reference":reference,"memo":""});
    let review = run(app, json!({"type":"ReviewSalesTaxPayment","company_id":c,"values":values}));
    run(app, json!({"type":"RecordSalesTaxPayment","company_id":c,"values":values,"review_token":review["token"]}))["id"].as_str().unwrap().to_owned()
}

#[test]
fn a_check_number_a_tax_payment_reference_names_is_not_called_reusable() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let bank = account(&mut app, &c, "1000");
    let tax_payment = tax_paid_by_check(&mut app, &c, &bank, "Check 127");
    let number = number_of(&mut app, "salestax_payment", &tax_payment);
    let timber = directory_save(&mut app, &c, "vendor", None, "Timber Supply (synthetic)").unwrap();
    let june = bill(&mut app, &c, &timber, "2026-06-02", "2026-06-30", "75.00");
    let path = format!("/c/{c}/payments/new/");
    let send = |app: &mut fb_app::App| {
        let html = page(app, &path);
        let check = shown(app, &c, "127", "75.00");
        let fields = pay_run(&html, &bank, "2026-06-26", "127", &[(&june, "75.00")], &[("filter_vendor", ""), ("filter_due", ""), (&format!("shown-{timber}"), &check)]);
        invalid(submit_owned(app, &path, &fields))
    };
    let html = send(&mut app);
    assert!(html.contains(&format!("<li>Check 127 for Timber Supply (synthetic): used before by sales tax payment {number} (reference “Check 127”).</li>")), "{html}");
    assert!(html.contains("A sales tax payment's Reference names this check number. If that check was written, choose another number. If not, choose Pay selected bills again to use it."), "{html}");
    assert!(!html.contains("A voided number may be used again"), "{html}");
    // Voided: the line says so; the program still cannot tell whether that check was written.
    run(&mut app, json!({"type":"VoidSalesTaxPayment","company_id":c,"payment_id":tax_payment,"expected_revision":1,"date":"2026-06-12","reason":"Entered twice (synthetic)"}));
    let html = send(&mut app);
    assert!(html.contains(&format!("<li>Check 127 for Timber Supply (synthetic): used before by sales tax payment {number} (reference “Check 127”), which was voided.</li>")), "{html}");
    assert!(!html.contains("A voided number may be used again"), "{html}");
    let paid: i64 = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT count(*) FROM payables_billpayment", [], |r| r.get(0))).unwrap();
    assert_eq!(paid, 0);
}

/// Review 2, S1: one pay run, two vendors, checks 118 and 119. 118 was a voided check and a sales tax
/// payment's Reference names 119. The warning says which of the two may be used again.
#[test]
fn a_pay_run_warns_about_a_voided_number_and_a_tax_payment_number_together() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let bank = account(&mut app, &c, "1000");
    let spoiled = check(&mut app, &c, &bank, "2026-05-20", "118", "57.00");
    void_check(&mut app, &c, &spoiled, "2026-05-20");
    let voided = number_of(&mut app, "checks_check", &spoiled);
    let tax_payment = tax_paid_by_check(&mut app, &c, &bank, "Check 119");
    let named = number_of(&mut app, "salestax_payment", &tax_payment);
    let alder = directory_save(&mut app, &c, "vendor", None, "Alder Hardware (synthetic)").unwrap();
    let birch = directory_save(&mut app, &c, "vendor", None, "Birch Lumber (synthetic)").unwrap();
    let first = bill(&mut app, &c, &alder, "2026-06-02", "2026-06-30", "75.00");
    let second = bill(&mut app, &c, &birch, "2026-06-03", "2026-06-30", "20.00");
    let path = format!("/c/{c}/payments/new/");
    let html = page(&mut app, &path);
    let (shown_a, shown_b) = (shown(&mut app, &c, "118", "75.00"), shown(&mut app, &c, "119", "20.00"));
    let fields = pay_run(&html, &bank, "2026-06-26", "118", &[(&first, "75.00"), (&second, "20.00")], &[("filter_vendor", ""), ("filter_due", ""), (&format!("shown-{alder}"), &shown_a), (&format!("shown-{birch}"), &shown_b)]);
    let html = invalid(submit_owned(&mut app, &path, &fields));
    assert!(html.contains("<strong>Nothing was paid yet.</strong> These check numbers were used before on this bank account:"), "{html}");
    assert!(html.contains(&format!("<li>Check 118 for Alder Hardware (synthetic): used before by {voided}, which was voided.</li>")), "{html}");
    assert!(html.contains(&format!("<li>Check 119 for Birch Lumber (synthetic): used before by sales tax payment {named} (reference “Check 119”).</li>")), "{html}");
    assert!(html.contains("<p>A voided number may be used again. A sales tax payment's Reference names one of these check numbers. If that check was written, choose another number. If not, choose Pay selected bills again to use them.</p>"), "{html}");
    let paid: i64 = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT count(*) FROM payables_billpayment", [], |r| r.get(0))).unwrap();
    assert_eq!(paid, 0);
    // Sent again as the page now is: both are paid, with checks 118 and 119.
    let mut again = fields.clone();
    again[0].1 = field_value(&html, "request_key");
    again.push(("reuse_ok".into(), field_value(&html, "reuse_ok")));
    assert!(matches!(submit_owned(&mut app, &path, &again), fb_app::Outcome::Redirect { .. }));
    let references: Vec<String> = app.open_mut().unwrap().book.read(|q| {
        let mut st = q.prepare("SELECT reference FROM payables_billpayment ORDER BY reference")?;
        let v = st.query_map([], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?;
        Ok::<_, fb_core::FbError>(v)
    }).unwrap();
    assert_eq!(references, ["118", "119"]);
}

/// Review 2, S2: each earlier use the warning reads, one by one: a voided check, a voided credit refund
/// and a voided deposit refund ("…, which was voided"). A live check is not one (the review refuses its
/// number instead), and neither is a refund that was not voided.
#[test]
fn earlier_uses_name_voided_checks_and_refunds_but_not_live_ones() {
    let (mut app, c) = fixture(StarterChart::Empty);
    let bank = new_account(&mut app, &c, "1010", "Harbor checking (synthetic)", "BANK");
    let ar = new_account(&mut app, &c, "1100", "Receivables", "RECEIVABLE");
    let uf = new_account(&mut app, &c, "1200", "Undeposited funds", "OTHER_CURRENT_ASSET");
    let ap = new_account(&mut app, &c, "2000", "Payables", "PAYABLE");
    let deposits = new_account(&mut app, &c, "2100", "Customer deposits", "OTHER_CURRENT_LIABILITY");
    let income = new_account(&mut app, &c, "4000", "Sales", "INCOME");
    new_account(&mut app, &c, "6100", "Supplies", "EXPENSE");
    run(&mut app, json!({"type":"SaveSetup","company_id":c,"expected_revision":0,"receivables_id":ar,"undeposited_id":uf,"payables_id":ap,"customer_deposits_id":deposits,"bank_ids":[bank]}));
    let birch = customer(&mut app, &c, "Birch Cafe (synthetic)");
    // Checks 301 (voided) and 302 (live).
    let voided = check(&mut app, &c, &bank, "2026-05-04", "301", "40.00");
    void_check(&mut app, &c, &voided, "2026-05-05");
    check(&mut app, &c, &bank, "2026-05-06", "302", "41.00");
    // A credit memo refunded by checks 303 (voided) and 305 (live).
    let line = json!({"description":"Returned trays","quantity":"1","rate":"25.00","income_account_id":income});
    let memo = json!({"request_key":key(),"customer_id":birch,"date":"2026-05-07","memo":"","tax_mode":"NONE","tax_item_id":null,"lines":[line]});
    let review = run(&mut app, json!({"type":"ReviewCreditMemo","company_id":c,"values":memo}));
    let memo = run(&mut app, json!({"type":"RecordCreditMemo","company_id":c,"values":memo,"review_token":review["token"]}))["id"].as_str().unwrap().to_owned();
    // A customer deposit refunded by checks 304 (voided) and 306 (live).
    let deposit = json!({"request_key":key(),"customer_id":birch,"date":"2026-05-07","amount":"300.00","method":"CHECK","reference":"","destination":"BANK","bank_id":bank,"estimate_id":null,"order_reference":"Catering order (synthetic)","memo":""});
    let review = run(&mut app, json!({"type":"ReviewCustomerDeposit","company_id":c,"values":deposit}));
    let deposit = run(&mut app, json!({"type":"RecordCustomerDeposit","company_id":c,"values":deposit,"review_token":review["token"]}))["id"].as_str().unwrap().to_owned();
    let refund = |app: &mut fb_app::App, source: &str, deposit: bool, number: &str, void: bool| -> String {
        let values = json!({"request_key":key(),"date":"2026-05-08","amount":"10.00","bank_id":bank,"method":"CHECK","check_number":number,"memo":""});
        let review = run(app, json!({"type":"ReviewCustomerRefund","company_id":c,"source_id":source,"deposit":deposit,"values":values}));
        run(app, json!({"type":"RecordCustomerRefund","company_id":c,"source_id":source,"deposit":deposit,"values":values,"review_token":review["token"]}));
        let table = if deposit { "prepayments_depositrefund" } else { "credits_creditrefund" };
        let id: String = app.open_mut().unwrap().book.read(|q| q.query_row(&format!("SELECT id FROM {table} WHERE check_number=?1"), [number], |r| r.get(0))).unwrap();
        if void {
            run(app, json!({"type":"VoidCustomerRefund","company_id":c,"record_id":id,"deposit":deposit,"expected_revision":1,"date":"2026-05-09","reason":"Check spoiled (synthetic)"}));
        }
        number_of(app, table, &id)
    };
    let credit_refund = refund(&mut app, &memo, false, "303", true);
    let deposit_refund = refund(&mut app, &deposit, true, "304", true);
    refund(&mut app, &memo, false, "305", false);
    refund(&mut app, &deposit, true, "306", false);
    let check_number = number_of(&mut app, "checks_check", &voided);
    let co: fb_core::CompanyId = c.parse().unwrap();
    let earlier = |app: &mut fb_app::App, n: &str| app.open_mut().unwrap().book.read(|q| fb_core::checks::earlier_use(q, co, &bank, n)).unwrap();
    for (n, document) in [("301", &check_number), ("303", &credit_refund), ("304", &deposit_refund)] {
        let found = earlier(&mut app, n).unwrap_or_else(|| panic!("{n}: no earlier use"));
        assert!(!found.is_tax_payment(), "{n}");
        assert_eq!(found.text(), format!("{document}, which was voided"), "{n}");
    }
    for n in ["302", "305", "306", "307"] {
        assert_eq!(earlier(&mut app, n), None, "{n}");
    }
}

fn default_tax(app: &mut fb_app::App, c: &str) -> Option<String> {
    app.open_mut().unwrap().book.read(|q| fb_core::salestax::settings_for(q, c.parse().unwrap())).unwrap()["default_item_id"].as_str().map(str::to_owned)
}

/// Item 27: turning sales tax on and adding the one rate left every sales form at "Choose sales tax".
#[test]
fn the_only_tax_rate_becomes_the_default_for_new_sales_forms() {
    // Turned on first, then the rate added (as the worker did).
    let (mut app, c) = fixture(StarterChart::Standard);
    redirect(tax_on(&mut app, &c));
    assert_eq!(default_tax(&mut app, &c), None, "no rate yet");
    let path = format!("/c/{c}/sales-tax/rates/new/");
    page(&mut app, &path);
    redirect(submit(&mut app, &path, &[("name", "IL Sales Tax (TEST)"), ("description", "Sales tax"), ("rate", "6.25"), ("agency", ""), ("new_agency_name", "Illinois Dept of Revenue (TEST)")]));
    let rate: String = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT id FROM salestax_taxitem", [], |r| r.get(0))).unwrap();
    assert_eq!(default_tax(&mut app, &c).as_deref(), Some(rate.as_str()));
    let html = page(&mut app, &format!("/c/{c}/sales-tax/rates/"));
    assert!(html.contains("New sales forms start with the sales tax “IL Sales Tax (TEST)”. You can change this in Sales tax settings."), "{html}");
    let form = page(&mut app, &format!("/c/{c}/invoices/new/"));
    assert!(form.contains(&format!("value=\"T{rate}\" selected")), "the invoice starts with the rate");
    // A second rate changes nothing; the person's own choice is kept.
    run(&mut app, json!({"type":"SaveTaxRate","company_id":c,"item_id":null,"expected_revision":null,"name":"County (TEST)","description":"County tax","rate":"1","agency_id":null,"new_agency_name":"County Treasurer (TEST)"}));
    assert_eq!(default_tax(&mut app, &c).as_deref(), Some(rate.as_str()));

    // The rate first (tax still off), then turned on: the rate is made the default on turning on.
    let (mut app, c) = fixture(StarterChart::Standard);
    run(&mut app, json!({"type":"SaveTaxRate","company_id":c,"item_id":null,"expected_revision":null,"name":"IL Sales Tax (TEST)","description":"Sales tax","rate":"6.25","agency_id":null,"new_agency_name":"Illinois Dept of Revenue (TEST)"}));
    assert_eq!(default_tax(&mut app, &c), None, "sales tax is still off");
    redirect(tax_on(&mut app, &c));
    assert!(default_tax(&mut app, &c).is_some());
    // Two rates when turning on: none is guessed.
    let (mut app, c) = fixture(StarterChart::Standard);
    for (name, agency) in [("State (TEST)", "State Revenue (TEST)"), ("County (TEST)", "County Treasurer (TEST)")] {
        run(&mut app, json!({"type":"SaveTaxRate","company_id":c,"item_id":null,"expected_revision":null,"name":name,"description":"Sales tax","rate":"1","agency_id":null,"new_agency_name":agency}));
    }
    redirect(tax_on(&mut app, &c));
    assert_eq!(default_tax(&mut app, &c), None);
}

/// Items 28 and 32: a transfer opened on 1020 → 1000 (the first account in the chart, which this
/// company does not use); a deposit refund and a sales tax payment chose nothing. Every form that picks
/// a bank account now follows the deposit's rule (item 8): the only bank account; else the one the
/// last document of its kind used; else none.
#[test]
fn bank_accounts_start_where_the_last_document_of_the_kind_was() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let checking = new_account(&mut app, &c, "1010", "Test Checking", "BANK");
    let savings = new_account(&mut app, &c, "1020", "Test Savings", "BANK");
    // Transfers: before the first, neither account is chosen.
    let path = format!("/c/{c}/transfers/new/");
    let html = page(&mut app, &path);
    assert_eq!((selected(&html, "from_account"), selected(&html, "to_account")), (None, None), "{html}");
    let transfer = |app: &mut fb_app::App, from: &str, to: &str| {
        let values = json!({"request_key":key(),"date":"2026-07-20","from_account_id":from,"to_account_id":to,"amount":"500.00","memo":"Move (synthetic)"});
        let review = run(app, json!({"type":"ReviewTransfer","company_id":c,"values":values}));
        run(app, json!({"type":"RecordTransfer","company_id":c,"values":values,"review_token":review["token"]}));
    };
    transfer(&mut app, &checking, &savings);
    let html = page(&mut app, &path);
    assert_eq!((selected(&html, "from_account"), selected(&html, "to_account")), (Some(checking.clone()), Some(savings.clone())));
    transfer(&mut app, &savings, &checking);
    let html = page(&mut app, &path);
    assert_eq!((selected(&html, "from_account"), selected(&html, "to_account")), (Some(savings.clone()), Some(checking.clone())), "not 1000 Checking");

    // Checks and bill payments: none before the first, then the last one's account.
    let html = page(&mut app, &format!("/c/{c}/checks/new/"));
    assert_eq!(selected(&html, "bank"), None);
    check(&mut app, &c, &checking, "2026-07-21", "501", "12.00");
    assert_eq!(selected(&page(&mut app, &format!("/c/{c}/checks/new/")), "bank"), Some(checking.clone()));
    let pay = format!("/c/{c}/payments/new/");
    let timber = directory_save(&mut app, &c, "vendor", None, "Timber Supply (synthetic)").unwrap();
    let id = bill(&mut app, &c, &timber, "2026-07-01", "2026-07-31", "40.00");
    assert_eq!(selected(&page(&mut app, &pay), "bank"), None);
    let values = json!({"request_key":key(),"vendor_id":timber,"date":"2026-07-22","amount":"40.00","method":"CASH","reference":"","memo":"","bank_id":savings,"allocations":[{"bill_id":id,"amount":"40.00"}]});
    let review = run(&mut app, json!({"type":"ReviewBillPayment","company_id":c,"values":values}));
    run(&mut app, json!({"type":"RecordBillPayment","company_id":c,"values":values,"review_token":review["token"]}));
    bill(&mut app, &c, &timber, "2026-07-02", "2026-07-31", "41.00");
    assert_eq!(selected(&page(&mut app, &pay), "bank"), Some(savings.clone()));

    // A credit refund: none, then the last refund's account.
    let lakeside = customer(&mut app, &c, "Lakeside Bakery (synthetic)");
    let income = account(&mut app, &c, "4000");
    let memo = json!({"request_key":key(),"customer_id":lakeside,"date":"2026-07-05","memo":"Returned (synthetic)","tax_mode":"NONE","tax_item_id":null,"lines":[{"description":"Returned shelf","quantity":"1","rate":"60.00","income_account_id":income}]});
    let review = run(&mut app, json!({"type":"ReviewCreditMemo","company_id":c,"values":memo}));
    let memo = run(&mut app, json!({"type":"RecordCreditMemo","company_id":c,"values":memo,"review_token":review["token"]}))["id"].as_str().unwrap().to_owned();
    let refund_path = format!("/c/{c}/credit-memos/{memo}/refund/");
    assert_eq!(selected(&page(&mut app, &refund_path), "bank"), None);
    let values = json!({"request_key":key(),"date":"2026-07-06","amount":"10.00","bank_id":checking,"method":"CASH","check_number":"","memo":""});
    let review = run(&mut app, json!({"type":"ReviewCustomerRefund","company_id":c,"source_id":memo,"deposit":false,"values":values}));
    run(&mut app, json!({"type":"RecordCustomerRefund","company_id":c,"source_id":memo,"deposit":false,"values":values,"review_token":review["token"]}));
    assert_eq!(selected(&page(&mut app, &refund_path), "bank"), Some(checking.clone()));

    // A sales tax payment: none, then the last payment's account.
    redirect(tax_on(&mut app, &c));
    run(&mut app, json!({"type":"SaveTaxRate","company_id":c,"item_id":null,"expected_revision":null,"name":"IL Sales Tax (TEST)","description":"Sales tax","rate":"5","agency_id":null,"new_agency_name":"Illinois Dept of Revenue (TEST)"}));
    let rate: String = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT id FROM salestax_taxitem", [], |r| r.get(0))).unwrap();
    let mut values = json!({"request_key":key(),"customer_id":lakeside,"date":"2026-07-08","due_date":"2026-07-08","memo":"","tax_mode":"ITEM","tax_item_id":rate,
        "lines":[{"description":"Shelf","quantity":"1","rate":"100.00","taxable":true,"income_account_id":income}]});
    let id = run(&mut app, json!({"type":"SaveInvoice","company_id":c,"values":values.take()}))["id"].as_str().unwrap().to_owned();
    assert!(matches!(post_invoice(&mut app, &c, &id).unwrap(), fb_app::Outcome::Done { .. }));
    let tax_path = format!("/c/{c}/sales-tax/payments/new/");
    assert_eq!(selected(&page(&mut app, &tax_path), "bank"), None);
    let agency: String = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT agency_id FROM salestax_taxitem", [], |r| r.get(0))).unwrap();
    let values = json!({"request_key":key(),"agency_id":agency,"date":"2026-08-10","bank_id":savings,"tax_amount":"5.00","penalty_amount":"0","expense_account_id":null,"period_start":"2026-07-01","period_end":"2026-07-31","reference":"Online payment","memo":""});
    let review = run(&mut app, json!({"type":"ReviewSalesTaxPayment","company_id":c,"values":values}));
    run(&mut app, json!({"type":"RecordSalesTaxPayment","company_id":c,"values":values,"review_token":review["token"]}));
    assert_eq!(selected(&page(&mut app, &tax_path), "bank"), Some(savings.clone()));
}

/// Item 29: "Payments made" on What you owe listed the latest 200 payments whatever the From and
/// Through dates above it said.
#[test]
fn what_you_owe_lists_the_payments_made_in_the_chosen_dates() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let bank = account(&mut app, &c, "1000");
    redirect(tax_on(&mut app, &c));
    run(&mut app, json!({"type":"SaveTaxRate","company_id":c,"item_id":null,"expected_revision":null,"name":"IL Sales Tax (TEST)","description":"Sales tax","rate":"5","agency_id":null,"new_agency_name":"Illinois Dept of Revenue (TEST)"}));
    let (rate, agency): (String, String) = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT id, agency_id FROM salestax_taxitem", [], |r| Ok((r.get(0)?, r.get(1)?)))).unwrap();
    let juniper = customer(&mut app, &c, "Juniper Print (synthetic)");
    let income = account(&mut app, &c, "4000");
    let id = run(&mut app, json!({"type":"SaveInvoice","company_id":c,"values":{"request_key":key(),"customer_id":juniper,"date":"2026-05-08","due_date":"2026-05-08","memo":"","tax_mode":"ITEM","tax_item_id":rate,
        "lines":[{"description":"Banners","quantity":"1","rate":"1000.00","taxable":true,"income_account_id":income}]}}))["id"].as_str().unwrap().to_owned();
    assert!(matches!(post_invoice(&mut app, &c, &id).unwrap(), fb_app::Outcome::Done { .. }));
    for (date, amount, reference) in [("2026-06-10", "20.00", "June return (synthetic)"), ("2026-07-10", "30.00", "July return (synthetic)")] {
        let values = json!({"request_key":key(),"agency_id":agency,"date":date,"bank_id":bank,"tax_amount":amount,"penalty_amount":"0","expense_account_id":null,"period_start":"2026-05-01","period_end":"2026-05-31","reference":reference,"memo":""});
        let review = run(&mut app, json!({"type":"ReviewSalesTaxPayment","company_id":c,"values":values}));
        run(&mut app, json!({"type":"RecordSalesTaxPayment","company_id":c,"values":values,"review_token":review["token"]}));
    }
    let owed = |app: &mut fb_app::App, query: &str| page(app, &format!("/c/{c}/sales-tax/owed/{query}")).split("id=\"payments-title\"").nth(1).unwrap().to_owned();
    let june = owed(&mut app, "?start=2026-06-01&end=2026-06-30");
    assert!(june.starts_with(">Payments made from 2026-06-01 to 2026-06-30</h2>"), "{june}");
    assert!(june.contains("STP-000001") && !june.contains("STP-000002"), "{june}");
    let both = owed(&mut app, "?start=2026-06-01&end=2026-07-31");
    assert!(both.contains("STP-000001") && both.contains("STP-000002"));
    assert!(owed(&mut app, "?start=2026-08-01&end=2026-08-31").contains("No payments in these dates."));
    // Dates that cannot be read show the latest payments, as before.
    let bad = fb_app::screens::render(&mut app, &format!("/c/{c}/sales-tax/owed/?start=2026-08-31&end=2026-08-01")).unwrap();
    assert_eq!(bad.status, 400);
    let bad = bad.html.split("id=\"payments-title\"").nth(1).unwrap().to_owned();
    assert!(bad.starts_with(">Payments made</h2>") && bad.contains("STP-000001") && bad.contains("STP-000002"), "{bad}");
}

/// Item 33: a refused journal reversal opened a separate "Action not completed" page; the date and
/// reason typed were lost, and the only way back was the breadcrumb.
#[test]
fn a_refused_journal_reversal_stays_on_the_entry_with_what_was_typed() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let accrued = new_account(&mut app, &c, "2150", "Accrued Expenses (synthetic)", "OTHER_CURRENT_LIABILITY");
    let expense = account(&mut app, &c, "6140");
    let line = |a: &str, d: &str, cr: &str| json!({"account":a,"debit":d,"credit":cr,"description":"","name":""});
    let id = run(&mut app, json!({"type":"SaveJournal","company_id":c,"journal_id":null,"date":"2026-07-31","memo":"July accrual (synthetic)","entry_number":"","adjusting":false,"request_key":null,"expected_revision":null,
        "lines":[line(&expense,"45.00","0.00"), line(&accrued,"0.00","45.00")]}))["id"].as_str().unwrap().to_owned();
    run(&mut app, json!({"type":"PostJournal","company_id":c,"journal_id":id,"expected_revision":1}));
    let reverse = format!("/c/{c}/journals/{id}/reverse/");
    // A date before the entry: refused by the books, shown on the entry's own page.
    let html = invalid(submit(&mut app, &reverse, &[("date", "2026-07-30"), ("reason", "Accrual reversal (synthetic)")]));
    assert!(html.contains("July accrual (synthetic)") && html.contains("Entry date <strong>2026-07-31</strong>"), "{html}");
    assert!(html.contains("<details open id=\"reverse-journal\"><summary>Reverse this journal</summary>"), "{html}");
    assert!(html.contains("Supply a reason and a reversal date on or after the original date."), "{html}");
    assert!(html.contains("name=\"date\" value=\"2026-07-30\"") && html.contains("value=\"Accrual reversal (synthetic)\""), "{html}");
    assert!(!html.contains("Action not completed"), "{html}");
    // A missing reason: said at the reason, the date kept.
    let html = invalid(submit(&mut app, &reverse, &[("date", "2026-08-01"), ("reason", "")]));
    let before_reason = html.split_once("name=\"reason\"").unwrap().0;
    let reason = before_reason.split_once("name=\"date\"").unwrap().1;
    assert!(reason.contains("This field is required."), "{reason}");
    assert!(html.contains("name=\"date\" value=\"2026-08-01\""), "{html}");
    // Corrected, it posts as before.
    let location = redirect(submit(&mut app, &reverse, &[("date", "2026-08-01"), ("reason", "Accrual reversal (synthetic)")]));
    assert!(location.starts_with(&format!("/c/{c}/journals/")) && !location.contains(&id), "{location}");
    // The page as first shown is unchanged: closed, starting on the first of the next month.
    let html = page(&mut app, &format!("/c/{c}/journals/{id}/"));
    assert!(!html.contains("<details open id=\"reverse-journal\">"));
}

/// Item 34: a negative quantity said "Enter a finite positive value with at most 4 decimal places within
/// the supported limit.", across the line (item 12) and without naming the Qty box.
#[test]
fn a_line_number_error_names_its_box_in_plain_words() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let birch = customer(&mut app, &c, "Birch Street Bakery (synthetic)");
    let sales = account(&mut app, &c, "4100");
    let path = format!("/c/{c}/invoices/new/");
    let html = page(&mut app, &path);
    let html = invalid(submit(&mut app, &path, &[("request_key", &field_value(&html, "request_key")), ("customer", &birch), ("date", "2026-11-23"), ("terms", "30"), ("due_date", "2026-12-23"), ("tax_mode", "NONE"),
        ("lines-TOTAL_FORMS", "2"), ("lines-INITIAL_FORMS", "0"),
        ("lines-0-description", "Negative qty test"), ("lines-0-quantity", "-1"), ("lines-0-rate", "150.00"), ("lines-0-income_account", &sales),
        ("lines-1-description", "Odd rate test"), ("lines-1-quantity", "1"), ("lines-1-rate", "12.345"), ("lines-1-income_account", &sales),
        ("editor_action", "save")]));
    let error = |id: &str| html.split_once(&format!("id=\"{id}\"")).unwrap_or_else(|| panic!("no {id}: {html}")).1.split("</ul>").next().unwrap().to_owned();
    assert!(error("id_lines-0-quantity_error").contains("<li>Qty: enter a number more than 0, with at most 4 decimal places.</li>"), "{}", error("id_lines-0-quantity_error"));
    assert!(error("id_lines-1-rate_error").contains("<li>Rate: enter an amount of 0 or more, with at most 2 decimal places.</li>"), "{}", error("id_lines-1-rate_error"));
    assert!(!html.contains("within the supported limit"), "{html}");
    // Review 2, N5: a number in the right form but over the limit says the limit.
    let html = page(&mut app, &path);
    let html = invalid(submit(&mut app, &path, &[("request_key", &field_value(&html, "request_key")), ("customer", &birch), ("date", "2026-11-23"), ("terms", "30"), ("due_date", "2026-12-23"), ("tax_mode", "NONE"),
        ("lines-TOTAL_FORMS", "2"), ("lines-INITIAL_FORMS", "0"),
        ("lines-0-description", "Large qty test"), ("lines-0-quantity", "99999999999999999"), ("lines-0-rate", "1.00"), ("lines-0-income_account", &sales),
        ("lines-1-description", "Large rate test"), ("lines-1-quantity", "1"), ("lines-1-rate", "100000000000000.00"), ("lines-1-income_account", &sales),
        ("editor_action", "save")]));
    let error = |id: &str| html.split_once(&format!("id=\"{id}\"")).unwrap_or_else(|| panic!("no {id}: {html}")).1.split("</ul>").next().unwrap().to_owned();
    assert!(error("id_lines-0-quantity_error").contains("<li>Qty: enter a number no larger than 99,999,999,999,999.9999.</li>"), "{}", error("id_lines-0-quantity_error"));
    assert!(error("id_lines-1-rate_error").contains("<li>Rate: enter an amount no larger than 99,999,999,999,999.99.</li>"), "{}", error("id_lines-1-rate_error"));
}

/// Item 35 (and round 4's note on references): a row found by its tax, a line's amount or rate, or its
/// reference showed only the document's total and memo, not what matched.
#[test]
fn find_says_which_amount_or_reference_matched() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let juniper = customer(&mut app, &c, "Juniper Print (synthetic)");
    let income = account(&mut app, &c, "4000");
    let line = |d: &str, quantity: &str, rate: &str, taxable: bool| json!({"description":d,"quantity":quantity,"rate":rate,"taxable":taxable,"income_account_id":income});
    // 850.00 taxed at 6.25% (53.13) plus 3 × 33.33 = 99.99, untaxed: total 1,003.12.
    let id = run(&mut app, json!({"type":"SaveInvoice","company_id":c,"values":{"request_key":key(),"customer_id":juniper,"date":"2026-11-06","due_date":"2026-11-06","memo":"Spring banners","tax_mode":"MANUAL","tax_label":"State tax","tax_rate":"6.25",
        "lines":[line("Banners","1","850.00",true), line("Posters","3","33.33",false)]}}))["id"].as_str().unwrap().to_owned();
    assert!(matches!(post_invoice(&mut app, &c, &id).unwrap(), fb_app::Outcome::Done { .. }));
    let receipt = json!({"request_key":key(),"customer_id":juniper,"date":"2026-11-10","method":"CHECK","reference":"5150","memo":"Paid at the counter","destination":"UNDEPOSITED","bank_id":null,"amount":"100.00","allocations":[{"invoice_id":id,"amount":"100.00"}]});
    let review = run(&mut app, json!({"type":"ReviewCustomerReceipt","company_id":c,"sales_receipt":false,"values":receipt}));
    run(&mut app, json!({"type":"RecordCustomerReceipt","company_id":c,"sales_receipt":false,"values":receipt,"review_token":review["token"]}));
    let find = |app: &mut fb_app::App, q: &str| {
        let html = page(app, &format!("/c/{c}/find/?q={}&all=1", fb_app::screens::urlencode(&[("x".into(), q.into())]).trim_start_matches("x=")));
        html.split("<td class=\"find-detail\">").skip(1).map(|cell| cell.split("</td>").next().unwrap().to_owned()).collect::<Vec<_>>()
    };
    for (q, matched) in [("53.13", "Tax 53.13"), ("99.99", "Line amount 99.99"), ("33.33", "Rate 33.33"), ("949.99", "Subtotal 949.99"), ("850", "Line amount 850.00")] {
        let cells = find(&mut app, q);
        assert!(cells.iter().any(|c| c == &format!("Spring banners<span class=\"find-matched\">Matched: {matched}</span>")), "{q}: {cells:?}");
    }
    // The total, or the invoice's own number, is in the row already: nothing more is said.
    assert!(find(&mut app, "1,003.12").iter().all(|c| !c.contains("Matched")));
    // A reference the row does not show is named.
    let cells = find(&mut app, "5150");
    assert!(cells.contains(&"Paid at the counter<span class=\"find-matched\">Matched: Reference 5150</span>".to_owned()), "{cells:?}");
}

/// Item 36: saving an account said nothing; the account form had no Cancel; the bill form could not add
/// a vendor (item 13 covered the sales receipt only).
#[test]
fn account_saves_say_so_and_a_vendor_can_be_added_from_a_new_bill() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let path = format!("/c/{c}/accounts/new/");
    let html = page(&mut app, &path);
    assert!(html.contains(&format!("<button type=\"submit\">Save</button> <a href=\"/c/{c}/accounts/\">Cancel</a>")), "{html}");
    let list = redirect(submit(&mut app, &path, &[("code", "6108"), ("name", " Shop Supplies (synthetic) "), ("detail_type", "EXPENSE"), ("active", "on"), ("postable", "on")]));
    assert_eq!(list, format!("/c/{c}/accounts/"));
    let html = page(&mut app, &list);
    assert!(html.contains("<p class=\"notice\" role=\"status\">Account 6108 · Shop Supplies (synthetic) saved.</p>"), "{html}");
    assert!(!page(&mut app, &list).contains("saved."), "said once");
    let id = account(&mut app, &c, "6108");
    let edit = format!("/c/{c}/accounts/{id}/edit/");
    assert!(page(&mut app, &edit).contains(&format!("<a href=\"/c/{c}/accounts/\">Cancel</a>")));
    redirect(submit(&mut app, &edit, &[("code", "6108"), ("name", "Shop Supplies (synthetic)"), ("detail_type", "EXPENSE"), ("description", "Glue and sandpaper"), ("active", "on"), ("postable", "on")]));
    assert!(page(&mut app, &list).contains("Account 6108 · Shop Supplies (synthetic) saved."));

    // A new vendor from a new bill comes back to the bill with that vendor chosen.
    let html = page(&mut app, &format!("/c/{c}/bills/new/"));
    let new = format!("/c/{c}/lists/vendors/new/");
    assert!(html.contains(&format!("<a href=\"{new}?then=bill\">New vendor</a>")), "{html}");
    let html = page(&mut app, &format!("{new}?then=bill"));
    assert!(html.contains("<input type=\"hidden\" name=\"then\" value=\"bill\">") && html.contains("After you save, you go back to the new bill with this vendor chosen."), "{html}");
    let back = redirect(submit(&mut app, &new, &[("display_name", "Valley Electric (synthetic)"), ("then", "bill")]));
    let vendor: String = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT id FROM directory_vendor WHERE display_name='Valley Electric (synthetic)'", [], |r| r.get(0))).unwrap();
    assert_eq!(back, format!("/c/{c}/bills/new/?vendor={vendor}"));
    assert!(page(&mut app, &back).contains(&format!("value=\"{vendor}\" selected")));
    // The sales receipt's link still works the same way; a "then" that does not fit the kind is ignored.
    let plain = redirect(submit(&mut app, &new, &[("display_name", "Harbor Supply (synthetic)"), ("then", "sales_receipt")]));
    assert!(plain.starts_with(&format!("/c/{c}/lists/vendors/")), "{plain}");
}

/// Item 23, on the tax rate form: a new tax authority named like an active vendor points at that vendor.
#[test]
fn a_new_tax_authority_named_like_a_vendor_points_at_it() {
    let (mut app, c) = fixture(StarterChart::Standard);
    directory_save(&mut app, &c, "vendor", None, "Illinois Dept of Revenue (TEST)").unwrap();
    let path = format!("/c/{c}/sales-tax/rates/new/");
    page(&mut app, &path);
    let html = invalid(submit(&mut app, &path, &[("name", "IL Sales Tax (TEST)"), ("description", "Sales tax"), ("rate", "6.25"), ("agency", ""), ("new_agency_name", "illinois dept of revenue (test)")]));
    let field = html.split_once("name=\"new_agency_name\"").unwrap().0.rsplit("<p>").next().unwrap().to_owned();
    assert!(field.contains("A vendor named “Illinois Dept of Revenue (TEST)” already exists. Choose it under Existing tax authority instead."), "{field}");
}

/// Item 31: "Please fill out this field" showed on the Transfer form's empty Amount box right after an
/// account was picked. Cause (reproduced under Xvfb with the worker's own input, then with the pointer
/// alone): the browser's hover tooltip for an empty required field in a form without `novalidate`; the
/// pointer came to rest on the Amount box when the account list closed. Nothing was sent. The form now
/// has `novalidate`, like the app's other forms, and the app's own message covers an empty amount.
#[test]
fn the_transfer_form_uses_the_apps_own_messages() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let checking = new_account(&mut app, &c, "1010", "Test Checking", "BANK");
    let savings = new_account(&mut app, &c, "1020", "Test Savings", "BANK");
    let path = format!("/c/{c}/transfers/new/");
    let html = page(&mut app, &path);
    assert!(html.contains("<form method=\"post\" id=\"transfer-entry\" novalidate>"), "{html}");
    assert!(!html.contains(">Funds transfer</textarea>"));
    let html = invalid(submit(&mut app, &path, &[("request_key", &field_value(&html, "request_key")), ("date", "2026-07-27"), ("from_account", &checking), ("to_account", &savings), ("amount", ""), ("memo", "Funds transfer"), ("action", "review")]));
    let amount = html.split_once("<div class=\"field-amount\">").unwrap().1.split("</div>").next().unwrap().to_owned();
    assert!(amount.contains("This field is required."), "{amount}");
    assert_eq!(selected(&html, "from_account"), Some(checking));
}
