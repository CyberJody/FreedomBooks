//! Pay bills (one payment per vendor, all or none) and bank accounts by account type, against a
//! real encrypted book through the screen handlers.
mod support;
use fb_app::{App, Command, Outcome, commands::parse_command, keys, screens};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
static N: AtomicUsize = AtomicUsize::new(0);

fn command(app: &mut App, v: Value) -> Value {
    match app.run(parse_command(&v.to_string()).unwrap()).unwrap() {
        Outcome::Done { data } => data,
        v => panic!("{v:?}"),
    }
}
fn try_command(app: &mut App, v: Value) -> Result<Value, String> {
    match app.run(parse_command(&v.to_string()).unwrap()) {
        Ok(Outcome::Done { data }) => Ok(data),
        Ok(v) => panic!("{v:?}"),
        Err(e) => Err(e.to_string()),
    }
}
fn field(html: &str, name: &str) -> String {
    html.split_once(&format!("name=\"{name}\" value=\"")).unwrap().1.split('"').next().unwrap().into()
}
fn submit(app: &mut App, path: &str, values: Vec<(String, String)>) -> Outcome {
    let html = screens::render(app, path).unwrap().html;
    app.run(Command::SubmitForm { path: path.into(), token: field(&html, "fb_command_token"), fields: values }).unwrap()
}
fn count(app: &mut App, sql: &str) -> i64 {
    app.open_mut().unwrap().book.read(|tx| tx.query_row(sql, [], |r| r.get(0))).unwrap()
}
fn account(app: &mut App, c: &str, code: &str, detail: &str) -> String {
    command(app, json!({"type":"SaveAccount","company_id":c,"code":code,"name":format!("Account {code}"),"detail_type":detail}))["id"].as_str().unwrap().into()
}
fn vendor(app: &mut App, c: &str, name: &str) -> String {
    let o = app.open_mut().unwrap();
    fb_core::directory::save_record(&mut o.book, o.owner, c.parse().unwrap(), fb_core::directory::Model::Vendor, &json!({"display_name":name}), None).unwrap()["id"].as_str().unwrap().to_owned()
}
fn bill(app: &mut App, c: &str, vendor: &str, expense: &str, rate: &str, due: &str) -> (String, String) {
    let saved = command(app, json!({"type":"SaveBill","company_id":c,"values":{"bill_id":null,"expected_revision":0,"request_key":uuid(),"vendor_id":vendor,"date":"2026-10-01","due_date":due,"memo":"","reference":"","supersedes_id":null,"reentry_reason":"","refresh_vendor":false,"keep_archived":false,"lines":[{"origin_position":null,"item_id":null,"expense_account_id":expense,"description":"Supplies","quantity":"1","rate":rate,"refresh_item":false,"keep_archived":false}]}}));
    let id = saved["id"].as_str().unwrap().to_owned();
    let review = command(app, json!({"type":"ReviewBill","company_id":c,"bill_id":id,"expected_revision":1}));
    let posted = command(app, json!({"type":"PostBill","company_id":c,"bill_id":id,"expected_revision":1,"review_token":review["token"]}));
    (id, posted["number"].as_str().unwrap().to_owned())
}
fn uuid() -> String {
    format!("00000000-0000-4000-8000-{:012x}", N.fetch_add(1, Ordering::SeqCst) + 0x9000)
}
struct Books { app: App, c: String, bank: String, savings: String, ap: String, expense: String, a: String, b: String }
/// Two Bank-type accounts and a setup that lists no bank: Bank-type accounts are bank accounts.
fn books() -> Books {
    let path = std::env::temp_dir().join(format!("fb-paybills-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    std::fs::create_dir_all(&path).unwrap();
    let mut app = App::new();
    app.today = || chrono::NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();
    let c = app.create_book(&path.join("book.fbbook"), &keys::new_master_key().unwrap(), "owner", "Pay bills test books").unwrap().company_id.to_db();
    support::sign_in(&mut app);
    let ap = account(&mut app, &c, "2000", "PAYABLE");
    let expense = account(&mut app, &c, "5000", "EXPENSE");
    let rev0 = current_revision(&mut app, &c);
    let revision = command(&mut app, json!({"type":"SaveSetup","company_id":c,"expected_revision":rev0,"receivables_id":null,"undeposited_id":null,"payables_id":ap,"bank_ids":[]}))["revision"].as_i64().unwrap();
    assert!(revision >= 1);
    let bank = account(&mut app, &c, "1000", "BANK");
    let savings = account(&mut app, &c, "1010", "BANK");
    let a = vendor(&mut app, &c, "Avery Flour");
    let b = vendor(&mut app, &c, "Blake Power");
    Books { app, c, bank, savings, ap, expense, a, b }
}
fn current_revision(app: &mut App, c: &str) -> i64 {
    let o = app.open_mut().unwrap();
    o.book.read(|tx| fb_core::documents::setup_snapshot(tx, c.parse().unwrap())).unwrap()["revision"].as_i64().unwrap()
}
fn banks(app: &mut App, c: &str) -> Vec<String> {
    let o = app.open_mut().unwrap();
    o.book.read(|tx| fb_core::documents::setup_snapshot(tx, c.parse().unwrap())).unwrap()["banks"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect()
}
fn hidden_shown(html: &str) -> Vec<(String, String)> {
    html.split("name=\"shown-").skip(1).map(|rest| {
        let (vendor, rest) = rest.split_once('"').unwrap();
        let value = rest.split_once("value=\"").unwrap().1.split('"').next().unwrap();
        (format!("shown-{vendor}"), value.replace("&#x7c;", "|").replace("&#124;", "|"))
    }).collect()
}
fn html_of(out: &Outcome) -> (String, u16) {
    match out {
        Outcome::Invalid { html } => (html.clone(), 400),
        Outcome::Page { html, status } => (html.clone(), *status),
        o => panic!("{o:?}"),
    }
}

#[test]
fn bank_type_accounts_are_bank_accounts_without_a_designation_step() {
    let mut f = books();
    // Created after the setup, both Bank-type accounts are listed; saving the setup with no
    // banks keeps them.
    let listed = banks(&mut f.app, &f.c);
    assert!(listed.contains(&f.bank) && listed.contains(&f.savings), "{listed:?}");
    let rev = current_revision(&mut f.app, &f.c);
    command(&mut f.app, json!({"type":"SaveSetup","company_id":f.c,"expected_revision":rev,"receivables_id":null,"undeposited_id":null,"payables_id":f.ap,"bank_ids":[]}));
    assert_eq!(banks(&mut f.app, &f.c).len(), 2);
    // Every form offers them, worded "Bank account".
    for path in ["payments/new/", "transfers/new/", "checks/new/", "deposits/new/"] {
        let html = screens::render(&mut f.app, &format!("/c/{}/{path}", f.c)).unwrap().html;
        assert!(html.contains(&f.bank) && html.contains(&f.savings), "{path}");
        assert!(!html.contains("designated") && !html.contains("Designated"), "{path}");
    }
    // A transfer between them records with no setup step.
    let values = json!({"request_key":uuid(),"date":"2026-11-02","from_account_id":f.bank,"to_account_id":f.savings,"amount":"20.00","memo":"Reserve"});
    let review = command(&mut f.app, json!({"type":"ReviewTransfer","company_id":f.c,"values":values}));
    command(&mut f.app, json!({"type":"RecordTransfer","company_id":f.c,"values":values,"review_token":review["token"]}));
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM transfers_transfer"), 1);
    // The guard is kept: an asset account that is not of type Bank is not a bank account.
    let other = account(&mut f.app, &f.c, "1200", "OTHER_CURRENT_ASSET");
    let values = json!({"request_key":uuid(),"date":"2026-11-02","from_account_id":f.bank,"to_account_id":other,"amount":"5.00","memo":""});
    let err = try_command(&mut f.app, json!({"type":"ReviewTransfer","company_id":f.c,"values":values})).unwrap_err();
    assert!(err.contains("Choose two eligible bank or credit card accounts"), "{err}");
    // A new designation is refused at both entry points, even when supplied directly by ID.
    let rev = current_revision(&mut f.app, &f.c);
    let audits = count(&mut f.app, "SELECT count(*) FROM ledger_auditevent");
    let err = try_command(&mut f.app, json!({"type":"SaveSetup","company_id":f.c,"expected_revision":rev,"payables_id":f.ap,"bank_ids":[other]})).unwrap_err();
    assert!(err.contains("1200 · Account 1200 is not a Bank-type account."), "{err}");
    let setup_path = format!("/c/{}/accounting-setup/", f.c);
    let out = submit(&mut f.app, &setup_path, vec![("expected_revision".into(),rev.to_string()),("banks".into(),other)]);
    let Outcome::Invalid { html } = out else { panic!("{out:?}") };
    assert!(html.contains("1200 · Account 1200 is not a Bank-type account."));
    assert_eq!(current_revision(&mut f.app, &f.c), rev);
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM ledger_auditevent"), audits);
    // An inactive Bank-type account stops being offered and comes back when reactivated; each
    // change is in the audit history as an accounting setup change.
    let audits = count(&mut f.app, "SELECT count(*) FROM ledger_auditevent WHERE action='accounting.setup'");
    command(&mut f.app, json!({"type":"SaveAccount","company_id":f.c,"account_id":f.savings,"code":"1010","name":"Account 1010","detail_type":"BANK","active":false}));
    assert!(!banks(&mut f.app, &f.c).contains(&f.savings));
    let html = screens::render(&mut f.app, &format!("/c/{}/payments/new/", f.c)).unwrap().html;
    assert!(!html.contains(&f.savings));
    command(&mut f.app, json!({"type":"SaveAccount","company_id":f.c,"account_id":f.savings,"code":"1010","name":"Account 1010","detail_type":"BANK","active":true}));
    assert!(banks(&mut f.app, &f.c).contains(&f.savings));
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM ledger_auditevent WHERE action='accounting.setup'"), audits + 2);
    // Choosing a bank account for a role takes it off the bank list (as unticking it did).
    let rev = current_revision(&mut f.app, &f.c);
    command(&mut f.app, json!({"type":"SaveSetup","company_id":f.c,"expected_revision":rev,"receivables_id":null,"undeposited_id":f.savings,"payables_id":f.ap,"bank_ids":[]}));
    assert!(!banks(&mut f.app, &f.c).contains(&f.savings));
    let rev = current_revision(&mut f.app, &f.c);
    command(&mut f.app, json!({"type":"SaveSetup","company_id":f.c,"expected_revision":rev,"receivables_id":null,"undeposited_id":null,"payables_id":f.ap,"bank_ids":[]}));
    // Retyped to another asset type, a bank account stays listed (as one chosen in setup).
    let petty = account(&mut f.app, &f.c, "1030", "BANK");
    command(&mut f.app, json!({"type":"SaveAccount","company_id":f.c,"account_id":petty,"code":"1030","name":"Account 1030","detail_type":"OTHER_CURRENT_ASSET"}));
    assert!(banks(&mut f.app, &f.c).contains(&petty));
    // A Bank-type account that was never used can still change type.
    let spare = account(&mut f.app, &f.c, "1020", "BANK");
    assert!(banks(&mut f.app, &f.c).contains(&spare));
    command(&mut f.app, json!({"type":"SaveAccount","company_id":f.c,"account_id":spare,"code":"1020","name":"Account 1020","detail_type":"EXPENSE"}));
    assert!(!banks(&mut f.app, &f.c).contains(&spare));
    // Accounting setup explains that Bank-type accounts are automatic and offers only others.
    let html = screens::render(&mut f.app, &format!("/c/{}/accounting-setup/", f.c)).unwrap().html;
    assert!(html.contains("Every active account of type Bank is a bank account"), "{html}");
    assert!(html.contains("Previously designated bank accounts"));
    let non_banks = html.split("name=\"banks\"").nth(1).unwrap().split("</select>").next().unwrap();
    assert!(non_banks.contains(&petty));
    assert!(!non_banks.contains(&f.ap));
    let rev = current_revision(&mut f.app, &f.c);
    let audit_count = count(&mut f.app, "SELECT count(*) FROM ledger_auditevent");
    let current_banks = banks(&mut f.app, &f.c);
    command(&mut f.app, json!({"type":"SaveSetup","company_id":f.c,"expected_revision":rev,"payables_id":f.ap,"bank_ids":current_banks}));
    assert_eq!(current_revision(&mut f.app, &f.c), rev);
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM ledger_auditevent"), audit_count);

    // An older designation can outlive the active account. The form identifies it on display
    // and on save, and the owner can remove it by clearing the selection.
    command(&mut f.app, json!({"type":"SaveAccount","company_id":f.c,"account_id":petty,"code":"1030","name":"Account 1030","detail_type":"OTHER_CURRENT_ASSET","active":false}));
    let html = screens::render(&mut f.app, &setup_path).unwrap().html;
    assert!(html.contains("1030 · Account 1030 (unavailable — replace or clear)"));
    let rev = current_revision(&mut f.app, &f.c);
    let fields = vec![("expected_revision".into(),rev.to_string()),("payables".into(),f.ap.clone())];
    let mut keep = fields.clone();
    keep.push(("banks".into(),petty.clone()));
    let Outcome::Invalid {html} = submit(&mut f.app, &setup_path, keep) else { panic!("invalid bank retained") };
    assert!(html.contains("1030 · Account 1030 is unavailable as a bank account. Replace or clear it."));
    assert_eq!(current_revision(&mut f.app, &f.c), rev);
    assert!(matches!(submit(&mut f.app, &setup_path, fields), Outcome::Redirect {..}));
    assert!(!banks(&mut f.app, &f.c).contains(&petty));
}

#[test]
fn backfill_migration_lists_existing_bank_type_accounts() {
    let mut f = books();
    // An older book: Bank-type accounts that were never designated.
    let conn = f.app.open_mut().unwrap().book.connection();
    conn.execute("DELETE FROM documents_designatedbank", []).unwrap();
    conn.execute_batch(include_str!("../../fb-db/sql/0196_bank_accounts_by_type.sql")).unwrap();
    let mut listed = banks(&mut f.app, &f.c);
    listed.sort();
    let mut want = vec![f.bank.clone(), f.savings.clone()];
    want.sort();
    assert_eq!(listed, want);
    let ids: Vec<String> = f.app.open_mut().unwrap().book.read(|tx| tx.prepare("SELECT id FROM documents_designatedbank")?.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()).unwrap();
    assert!(ids.iter().all(|id| fb_db::validators::uuid(id)), "{ids:?}");
    // Running it again adds nothing.
    let conn = f.app.open_mut().unwrap().book.connection();
    conn.execute_batch(include_str!("../../fb-db/sql/0196_bank_accounts_by_type.sql")).unwrap();
    assert_eq!(banks(&mut f.app, &f.c).len(), 2);
}

fn pay_fields(f: &Books, html: &str, picks: &[(&str, &str)], reference: &str) -> Vec<(String, String)> {
    let mut v = vec![
        ("request_key".to_owned(), field(html, "request_key")),
        ("bank".into(), f.bank.clone()),
        ("date".into(), "2026-11-10".into()),
        ("method".into(), "CHECK".into()),
        ("reference".into(), reference.into()),
        ("memo".into(), "November run".into()),
    ];
    for (id, amount) in picks {
        v.push(("pay".into(), (*id).into()));
        v.push((format!("amount-{id}"), (*amount).into()));
    }
    v
}

#[test]
fn pay_bills_pays_several_vendors_with_partial_amounts_in_one_run() {
    let mut f = books();
    let (a1, a1n) = bill(&mut f.app, &f.c, &f.a, &f.expense, "100.00", "2026-10-31");
    let (a2, a2n) = bill(&mut f.app, &f.c, &f.a, &f.expense, "50.00", "2026-11-20");
    let (b1, b1n) = bill(&mut f.app, &f.c, &f.b, &f.expense, "200.00", "2026-11-10");
    let (b2, _) = bill(&mut f.app, &f.c, &f.b, &f.expense, "30.00", "2026-12-01");
    let path = format!("/c/{}/payments/new/", f.c);
    let html = screens::render(&mut f.app, &path).unwrap().html;
    for n in [&a1n, &a2n, &b1n] {
        assert!(html.contains(n.as_str()));
    }
    assert!(html.contains("Overdue 10 days") && html.contains("Due on payment date") && html.contains("Due in 10 days"), "{html}");
    assert!(!html.contains("Synthetic") && !html.contains("RECORD ONLY") && !html.contains("Choose vendor to load"));
    let mut backdated = pay_fields(&f, &html, &[], "1001");
    backdated.iter_mut().find(|(key, _)| key == "date").unwrap().1 = "2026-11-01".into();
    let (backdated, _) = html_of(&submit(&mut f.app, &path, backdated));
    assert!(backdated.contains("Overdue 1 day<") && backdated.contains("Due in 9 days"));
    // Arriving from a bill ticks it with its full balance and shows its vendor.
    let from_bill = screens::render(&mut f.app, &format!("{path}?bill={a1}")).unwrap().html;
    assert!(from_bill.contains(&format!("id=\"pay-{a1}\" checked")) && from_bill.contains(&format!("name=\"amount-{a1}\" value=\"100.00\"")));
    assert!(from_bill.contains(&format!("<option value=\"{}\" selected>", f.a)));
    // The first send carries no proposal from the page: nothing is paid; the page shows the
    // payments it would make (with their entries) to confirm.
    let journals = count(&mut f.app, "SELECT count(*) FROM ledger_journal");
    let fields = pay_fields(&f, &html, &[(&a1, "100.00"), (&a2, "20.00"), (&b1, ""), (&b2, "30")], "1001");
    let (page, status) = html_of(&submit(&mut f.app, &path, fields.clone()));
    assert_eq!(status, 400);
    assert!(page.contains("changed since the page was shown"), "{page}");
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM payables_billpayment"), 0);
    // The server never fills in what the page showed: only the page's own script does.
    assert!(hidden_shown(&page).is_empty());
    // What the page shows: Avery first, check 1001 for 120.00 to payables; Blake check 1002, 230.00.
    let shown_for = |check: &str, amount: &str| {
        let mut want = [format!("{}:{amount}:0.00", f.ap), format!("{}:0.00:{amount}", f.bank)];
        want.sort();
        format!("{check}||{}", want.join("|"))
    };
    let shown = vec![(format!("shown-{}", f.a), shown_for("1001", "120.00")), (format!("shown-{}", f.b), shown_for("1002", "230.00"))];
    // Showing Blake's check as 1001 (wrong order) is refused too.
    let mut wrong = fields.clone();
    wrong.extend([(format!("shown-{}", f.a), shown_for("1002", "120.00")), (format!("shown-{}", f.b), shown_for("1001", "230.00"))]);
    let (page, _) = html_of(&submit(&mut f.app, &path, wrong));
    assert!(page.contains("changed since the page was shown"));
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM payables_billpayment"), 0);
    // The selections are kept.
    assert!(page.contains(&format!("name=\"amount-{a2}\" value=\"20.00\"")) && page.contains(&format!("id=\"pay-{b2}\" checked")));
    // Sent with what the page showed: two payments, four allocations, one transaction.
    let mut confirmed = fields.clone();
    confirmed.extend(shown.clone());
    let out = submit(&mut f.app, &path, confirmed.clone());
    let Outcome::Redirect { location } = out else { panic!("{out:?}") };
    assert!(location.contains("?paid="));
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM payables_billpayment"), 2);
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM payables_paymentallocation"), 4);
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM ledger_journal"), journals + 2);
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM ledger_auditevent WHERE action='payment.recorded'"), 2);
    let refs: Vec<(String, i64)> = f.app.open_mut().unwrap().book.read(|tx| tx.prepare("SELECT reference,amount FROM payables_billpayment ORDER BY reference")?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()).unwrap();
    assert_eq!(refs, vec![("1001".to_owned(), 12000), ("1002".to_owned(), 23000)]);
    let paid = screens::render(&mut f.app, &format!("/c/{}/bills/{a1}/", f.c)).unwrap().html;
    assert!(paid.contains(&format!("{a1n} · Paid · USD")) && paid.contains("Current bill state: Paid"));
    // A2 keeps 30.00 open; the others are paid.
    let html = screens::render(&mut f.app, &path).unwrap().html;
    assert!(html.contains(&a2n) && !html.contains(&a1n) && !html.contains(&b1n));
    assert!(html.contains("data-open=\"30.00\""));
    let done = screens::render(&mut f.app, &format!("{path}{}", &location[location.find('?').unwrap()..])).unwrap().html;
    assert!(done.contains("Paid 4 bills with 2 payments, total 350.00"), "{done}");
    // Sending the same page again pays nothing more.
    let again = submit(&mut f.app, &path, confirmed);
    assert!(matches!(again, Outcome::Redirect { ref location } if location.contains("?paid=")), "{again:?}");
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM payables_billpayment"), 2);
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM ledger_journal"), journals + 2);
}

#[test]
fn one_refused_vendor_pays_nothing_and_keeps_the_choices() {
    let mut f = books();
    let (a1, _) = bill(&mut f.app, &f.c, &f.a, &f.expense, "100.00", "2026-10-31");
    let (b1, _) = bill(&mut f.app, &f.c, &f.b, &f.expense, "200.00", "2026-11-10");
    let path = format!("/c/{}/payments/new/", f.c);
    let html = screens::render(&mut f.app, &path).unwrap().html;
    // More than the open balance is refused on the row, in words.
    let (page, status) = { let fields = pay_fields(&f, &html, &[(&a1, "100.00"), (&b1, "250.00")], "2001"); html_of(&submit(&mut f.app, &path, fields)) };
    assert_eq!(status, 400);
    assert!(page.contains("more than the open balance of 200.00"), "{page}");
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM payables_billpayment"), 0);
    // The core refuses Blake (archived since the page was shown): Avery is not paid either.
    let o = f.app.open_mut().unwrap();
    fb_core::directory::set_active(&mut o.book, o.owner, f.c.parse().unwrap(), fb_core::directory::Model::Vendor, &f.b, false).unwrap();
    let (page, status) = { let fields = pay_fields(&f, &html, &[(&a1, "100.00"), (&b1, "200.00")], "2001"); html_of(&submit(&mut f.app, &path, fields)) };
    assert_eq!(status, 400);
    assert!(page.contains("Nothing was paid") && page.contains("Blake Power: Choose an active vendor"), "{page}");
    assert!(page.contains(&format!("id=\"pay-{a1}\" checked")) && page.contains(&format!("id=\"pay-{b1}\" checked")));
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM payables_billpayment"), 0);
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM ledger_auditevent WHERE action='payment.recorded'"), 0);
    // Nothing ticked.
    let (page, _) = { let fields = pay_fields(&f, &html, &[], ""); html_of(&submit(&mut f.app, &path, fields)) };
    assert!(page.contains("Tick at least one bill to pay."));
}

#[test]
fn a_run_records_all_payments_or_none() {
    let mut f = books();
    let (a1, _) = bill(&mut f.app, &f.c, &f.a, &f.expense, "100.00", "2026-10-31");
    let (b1, _) = bill(&mut f.app, &f.c, &f.b, &f.expense, "200.00", "2026-11-10");
    let pay = |key: String, vendor: &str, bill: &str, amount: &str, reference: &str, bank: &str| json!({"request_key":key,"vendor_id":vendor,"date":"2026-11-10","amount":amount,"method":"CHECK","reference":reference,"memo":"","bank_id":bank,"allocations":[{"bill_id":bill,"amount":amount}]});
    let first = pay(uuid(), &f.a, &a1, "100.00", "3001", &f.bank);
    let second = pay(uuid(), &f.b, &b1, "200.00", "3002", &f.bank);
    let r1 = command(&mut f.app, json!({"type":"ReviewBillPayment","company_id":f.c,"values":first}));
    let r2 = command(&mut f.app, json!({"type":"ReviewBillPayment","company_id":f.c,"values":second}));
    // Blake's bill is paid elsewhere after the review: the run is refused as a whole.
    let elsewhere = pay(uuid(), &f.b, &b1, "50.00", "", &f.savings);
    let mut elsewhere = elsewhere;
    elsewhere["method"] = json!("OTHER");
    let r = command(&mut f.app, json!({"type":"ReviewBillPayment","company_id":f.c,"values":elsewhere}));
    command(&mut f.app, json!({"type":"RecordBillPayment","company_id":f.c,"values":elsewhere,"review_token":r["token"]}));
    let before = count(&mut f.app, "SELECT count(*) FROM ledger_auditevent");
    let err = try_command(&mut f.app, json!({"type":"RecordBillPayments","company_id":f.c,"payments":[first,second],"review_tokens":[r1["token"],r2["token"]]})).unwrap_err();
    assert!(err.contains("outstanding") || err.contains("changed"), "{err}");
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM payables_billpayment"), 1);
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM ledger_auditevent"), before);
    // A forged or reused review refuses the run too.
    let r1 = command(&mut f.app, json!({"type":"ReviewBillPayment","company_id":f.c,"values":first}));
    let err = try_command(&mut f.app, json!({"type":"RecordBillPayments","company_id":f.c,"payments":[first,second],"review_tokens":[r1["token"],"forged"]})).unwrap_err();
    assert!(err.contains("Invalid review"), "{err}");
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM payables_billpayment"), 1);
}

#[test]
fn vendor_pages_show_what_is_owed() {
    let mut f = books();
    let (a1, a1n) = bill(&mut f.app, &f.c, &f.a, &f.expense, "100.00", "2026-10-31");
    bill(&mut f.app, &f.c, &f.a, &f.expense, "50.00", "2026-11-20");
    let list = screens::render(&mut f.app, &format!("/c/{}/lists/vendors/?sort=balance", f.c)).unwrap().html;
    assert!(list.contains("Open balance") && list.contains("Overdue") && list.contains("150.00") && list.contains("100.00"));
    assert!(list.find("Avery Flour").unwrap() < list.find("Blake Power").unwrap());
    assert!(list.contains(&format!("payments/new/?vendor={}", f.a)));
    let page = screens::render(&mut f.app, &format!("/c/{}/lists/vendors/{}/", f.c, f.a)).unwrap().html;
    for want in ["New bill", "Pay bills", "Write check", "Transactions", "150.00", &a1n] {
        assert!(page.contains(want), "{want}");
    }
    assert!(page.contains(&format!("bills/new/?vendor={}", f.a)) && page.contains(&format!("checks/new/?vendor={}", f.a)) && page.contains(&format!("payments/new/?bill={a1}")));
    assert!(!page.contains("No balances or transactions are changed"));
}
