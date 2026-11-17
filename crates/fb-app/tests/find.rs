//! Find: the title-bar search across a company's documents and names. One synthetic book with a
//! document of every kind, a second company, and users of each role.
use fb_app::{App, Outcome, commands::parse_command, identity::Role, keys, screens};
use fb_core::CompanyId;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};

static N: AtomicUsize = AtomicUsize::new(0);
const PASSWORD: &str = "Synthetic individual password";

fn run(app: &mut App, v: Value) -> Value {
    match app.run(parse_command(&v.to_string()).unwrap()).unwrap_or_else(|e| panic!("{v}: {e}")) {
        Outcome::Done { data } => data,
        o => panic!("{v}: {o:?}"),
    }
}
fn key() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn page(app: &mut App, path: &str) -> String {
    let r = screens::render(app, path).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert_eq!(r.status, 200, "{path}: {}", r.html);
    r.html
}
/// The text of the results page's tables: `[heading, [row texts]]`.
fn results(html: &str) -> Vec<(String, Vec<String>)> {
    html.split(r#"<section class="find-group""#)
        .skip(1)
        .map(|s| {
            let heading = text(s.split_once("<h2").unwrap().1.split_once('>').unwrap().1.split(" <span").next().unwrap());
            let rows = s.split("<tr").skip(2).map(|r| text(&format!("<tr{}", r.split("</tr>").next().unwrap()))).collect();
            (heading, rows)
        })
        .collect()
}
fn text(html: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for ch in html.chars() {
        match ch {
            '<' => { tag = true; out.push(' '); }
            '>' => tag = false,
            c if !tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ").replace("&amp;", "&").replace("&#x2f;", "/")
}
fn hrefs(html: &str) -> Vec<String> {
    let main = html.split_once("<main").unwrap().1.split_once("</main>").unwrap().0;
    main.split(r#"<a href=""#).skip(1).map(|s| s.split('"').next().unwrap().to_owned()).collect()
}

struct Book {
    _dir: std::path::PathBuf,
    app: App,
    c: String,
    company: CompanyId,
    other: String,
    docs: std::collections::BTreeMap<&'static str, String>,
}

fn book() -> Book {
    let dir = std::env::temp_dir().join(format!("fb-find-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new();
    app.today = || chrono::NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();
    let company = app.create_book(&dir.join("book.fbbook"), &keys::new_master_key().unwrap(), "Synthetic owner", "Synthetic find books").unwrap().company_id;
    app.enroll_owner(PASSWORD).unwrap();
    let c = company.to_db();
    let account = |app: &mut App, code: &str, name: &str, detail: &str| {
        run(app, json!({"type":"SaveAccount","company_id":c,"code":code,"name":name,"detail_type":detail}))["id"].as_str().unwrap().to_owned()
    };
    let bank = account(&mut app, "1010", "Harbor checking", "BANK");
    let savings = account(&mut app, "1020", "Harbor savings", "BANK");
    let ar = account(&mut app, "1100", "Receivables", "RECEIVABLE");
    let uf = account(&mut app, "1200", "Undeposited funds", "OTHER_CURRENT_ASSET");
    let ap = account(&mut app, "2000", "Payables", "PAYABLE");
    let prepaid = account(&mut app, "2100", "Customer prepayments", "OTHER_CURRENT_LIABILITY");
    let accrued = account(&mut app, "2200", "Accrued expenses", "OTHER_CURRENT_LIABILITY");
    let income = account(&mut app, "4000", "Catering income", "INCOME");
    let expense = account(&mut app, "6000", "Flour and supplies", "EXPENSE");
    run(&mut app, json!({"type":"SaveSetup","company_id":c,"expected_revision":0,"receivables_id":ar,"undeposited_id":uf,"payables_id":ap,"customer_deposits_id":prepaid,"bank_ids":[bank,savings]}));
    let party = |app: &mut App, kind: &str, name: &str, email: &str, phone: &str| {
        let mut values = json!({"kind":kind,"display_name":name,"business_name":name,"contact_name":"Casey Sample","email":email,"phone":phone,"billing_address":"","notes":""});
        if kind == "customer" {
            values["tax_status"] = json!("TAXABLE");
            values["exemption_number"] = json!(null);
        } else {
            for (k, v) in [("default_expense_account", json!(null)), ("default_terms", json!(null)), ("expense_shown", json!(false)), ("terms_shown", json!(false))] {
                values[k] = v;
            }
        }
        run(app, json!({"type":"SaveDirectoryRecord","company_id":c,"record_id":null,"values":values}))["id"].as_str().unwrap().to_owned()
    };
    let lakeside = party(&mut app, "customer", "Lakeside Cafe", "orders@lakeside.example", "(555) 010-2233");
    let maple = party(&mut app, "customer", "Maple Events", "events@maple.example", "555-010-4455");
    let flour = party(&mut app, "vendor", "Valley Flour Supply", "billing@valley.example", "555 010 6677");
    run(&mut app, json!({"type":"SaveItem","company_id":c,"code":"Sourdough loaf","description":"Daily bread, whole loaf","kind":"SERVICE","sale_rate":"6.50","income_account_id":income}));
    let line = |d: &str, rate: &str| json!({"description":d,"quantity":"1","rate":rate,"income_account_id":income});
    let mut docs = std::collections::BTreeMap::new();

    // Invoices: two posted (one paid below), one draft.
    let invoice = |app: &mut App, customer: &str, date: &str, memo: &str, rate: &str, post: bool| {
        let id = run(app, json!({"type":"SaveInvoice","company_id":c,"values":{"request_key":key(),"customer_id":customer,"date":date,"due_date":date,"memo":memo,"tax_mode":"NONE","lines":[line("Catering",rate)]}}))["id"].as_str().unwrap().to_owned();
        if post { run(app, json!({"type":"PostInvoice","company_id":c,"invoice_id":id,"expected_revision":1})); }
        id
    };
    docs.insert("invoice_wedding", invoice(&mut app, &maple, "2026-10-19", "Wedding cake, three tier", "725.00", true));
    docs.insert("invoice_bread", invoice(&mut app, &lakeside, "2026-10-05", "Bread delivery week 40", "84.00", true));
    docs.insert("invoice_draft", invoice(&mut app, &lakeside, "2026-11-09", "Bread delivery week 45", "84.00", false));
    // A payment received in full for the bread invoice.
    let receipt = json!({"request_key":key(),"customer_id":lakeside,"date":"2026-10-20","method":"CHECK","reference":"Check 5521","memo":"Paid at the counter","destination":"UNDEPOSITED","bank_id":null,"amount":"84.00","allocations":[{"invoice_id":docs["invoice_bread"],"amount":"84.00"}]});
    let review = run(&mut app, json!({"type":"ReviewCustomerReceipt","company_id":c,"sales_receipt":false,"values":receipt}));
    let paid = run(&mut app, json!({"type":"RecordCustomerReceipt","company_id":c,"sales_receipt":false,"values":receipt,"review_token":review["token"]}));
    docs.insert("payment", paid["id"].as_str().unwrap().to_owned());
    // A sales receipt.
    let sale = json!({"request_key":key(),"customer_id":maple,"date":"2026-10-22","method":"CASH","reference":"","memo":"Farmers market stall","destination":"BANK","bank_id":bank,"tax_mode":"NONE","lines":[line("Pastries","42.75")]});
    let review = run(&mut app, json!({"type":"ReviewCustomerReceipt","company_id":c,"sales_receipt":true,"values":sale}));
    docs.insert("sales_receipt", run(&mut app, json!({"type":"RecordCustomerReceipt","company_id":c,"sales_receipt":true,"values":sale,"review_token":review["token"]}))["id"].as_str().unwrap().to_owned());
    // An estimate.
    docs.insert("estimate", run(&mut app, json!({"type":"SaveEstimate","company_id":c,"estimate_id":null,"expected_revision":0,"expiry":null,"values":{"request_key":key(),"customer_id":maple,"date":"2026-11-02","memo":"Anniversary brunch","tax_mode":"NONE","lines":[line("Brunch for 40","960.00")]}}))["id"].as_str().unwrap().to_owned());
    // A credit memo.
    let memo = json!({"request_key":key(),"customer_id":lakeside,"date":"2026-10-25","memo":"Stale rolls returned","tax_mode":"NONE","tax_item_id":null,"lines":[line("Returned rolls","12.00")]});
    let review = run(&mut app, json!({"type":"ReviewCreditMemo","company_id":c,"values":memo}));
    docs.insert("credit_memo", run(&mut app, json!({"type":"RecordCreditMemo","company_id":c,"values":memo,"review_token":review["token"]}))["id"].as_str().unwrap().to_owned());
    // A customer deposit (a prepayment received before invoicing).
    let deposit = json!({"request_key":key(),"customer_id":maple,"date":"2026-10-28","amount":"300.00","method":"CHECK","reference":"","destination":"BANK","bank_id":bank,"estimate_id":null,"order_reference":"Holiday order 12","memo":"Deposit for holiday trays"});
    let review = run(&mut app, json!({"type":"ReviewCustomerDeposit","company_id":c,"values":deposit}));
    docs.insert("customer_deposit", run(&mut app, json!({"type":"RecordCustomerDeposit","company_id":c,"values":deposit,"review_token":review["token"]}))["id"].as_str().unwrap().to_owned());
    // A bill, posted, and a bill payment for part of it.
    let bill = run(&mut app, json!({"type":"SaveBill","company_id":c,"values":{"bill_id":null,"expected_revision":0,"request_key":key(),"vendor_id":flour,"date":"2026-10-12","due_date":"2026-11-11","memo":"Flour and sugar, October","reference":"VF-88123","supersedes_id":null,"reentry_reason":"","refresh_vendor":false,"keep_archived":false,
        "lines":[{"origin_position":null,"item_id":null,"expense_account_id":expense,"description":"Flour","quantity":"1","rate":"412.37","refresh_item":false,"keep_archived":false}]}}))["id"].as_str().unwrap().to_owned();
    let review = run(&mut app, json!({"type":"ReviewBill","company_id":c,"bill_id":bill,"expected_revision":1}));
    run(&mut app, json!({"type":"PostBill","company_id":c,"bill_id":bill,"expected_revision":1,"review_token":review["token"]}));
    docs.insert("bill", bill.clone());
    let pay = json!({"request_key":key(),"vendor_id":flour,"date":"2026-10-30","amount":"200.00","method":"CHECK","reference":"1045","memo":"Part payment","bank_id":bank,"allocations":[{"bill_id":bill,"amount":"200.00"}]});
    let review = run(&mut app, json!({"type":"ReviewBillPayment","company_id":c,"values":pay}));
    docs.insert("bill_payment", run(&mut app, json!({"type":"RecordBillPayment","company_id":c,"values":pay,"review_token":review["token"]}))["id"].as_str().unwrap().to_owned());
    // A check.
    let check = run(&mut app, json!({"type":"SaveCheck","company_id":c,"values":{"check_id":null,"expected_revision":0,"request_key":key(),"bank_id":bank,"date":"2026-10-28","check_number":"1042","print_later":false,"payee_kind":"OTHER","vendor_id":null,"payee_name":"City Power and Light","address":"","memo":"Electric bill","refresh_payee":false,"keep_archived":false,
        "lines":[{"tab":"EXPENSE","origin_position":null,"account_id":expense,"item_id":null,"description":"Power","quantity":"1","rate":"188.90","refresh_item":false,"keep_archived":false}]}}))["id"].as_str().unwrap().to_owned();
    let review = run(&mut app, json!({"type":"ReviewCheck","company_id":c,"check_id":check,"expected_revision":1}));
    run(&mut app, json!({"type":"RecordCheck","company_id":c,"check_id":check,"expected_revision":1,"review_token":review["token"]}));
    docs.insert("check", check);
    // A bank deposit of the undeposited payment.
    let dep = json!({"request_key":key(),"bank_id":bank,"date":"2026-10-31","reference":"Slip 77","memo":"Counter takings","receipt_ids":[docs["payment"]]});
    let review = run(&mut app, json!({"type":"ReviewDeposit","company_id":c,"values":dep}));
    docs.insert("deposit", run(&mut app, json!({"type":"RecordDeposit","company_id":c,"values":dep,"review_token":review["token"]}))["id"].as_str().unwrap().to_owned());
    // A transfer.
    let transfer = json!({"request_key":key(),"date":"2026-11-03","from_account_id":bank,"to_account_id":savings,"amount":"500.00","memo":"Tax reserve"});
    let review = run(&mut app, json!({"type":"ReviewTransfer","company_id":c,"values":transfer}));
    docs.insert("transfer", run(&mut app, json!({"type":"RecordTransfer","company_id":c,"values":transfer,"review_token":review["token"]}))["id"].as_str().unwrap().to_owned());
    // A journal entry.
    let journal = run(&mut app, json!({"type":"SaveJournal","company_id":c,"journal_id":null,"date":"2026-11-04","memo":"Packaging accrual","entry_number":"ADJ-7","adjusting":false,"request_key":null,"expected_revision":null,
        "lines":[{"account":expense,"debit":"96.15","credit":"0.00","description":"Boxes","name":"Paper goods"},{"account":accrued,"debit":"0.00","credit":"96.15","description":"","name":""}]}));
    let journal = journal["id"].as_str().unwrap().to_owned();
    run(&mut app, json!({"type":"PostJournal","company_id":c,"journal_id":journal,"expected_revision":1}));
    let number: i64 = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT number FROM ledger_journal WHERE id=?1", [&journal], |r| r.get(0))).unwrap();
    docs.insert("journal", journal);
    docs.insert("journal_number", format!("J{number:06}"));

    // A second company in the same book, with look-alike names and amounts.
    let other = run(&mut app, json!({"type":"SaveCompany","company_id":null,"name":"Other synthetic company","currency":"USD"}))["id"].as_str().unwrap().to_owned();
    run(&mut app, json!({"type":"SaveCustomer","company_id":other,"display_name":"Lakeside Other Cafe"}));
    run(&mut app, json!({"type":"SaveAccount","company_id":other,"code":"6000","name":"Flour and supplies elsewhere","detail_type":"EXPENSE"}));
    Book { _dir: dir, app, c, company, other, docs }
}

fn find(b: &mut Book, q: &str) -> String {
    page(&mut b.app, &format!("/c/{}/find/?q={}&all=1", b.c, screens::urlencode(&[("x".into(), q.into())]).trim_start_matches("x=")))
}

#[test]
fn finds_documents_by_number_name_amount_and_memo_most_recent_first() {
    let mut b = book();
    // A partial customer name finds the customer and every document for them, newest first.
    let html = find(&mut b, "lakesi");
    let r = results(&html);
    let kinds: Vec<&str> = r.iter().map(|(h, _)| h.as_str()).collect();
    assert_eq!(kinds, ["Invoices", "Credit memos", "Payments received", "Customers"], "{r:#?}");
    assert_eq!(r[0].1, ["INV-000003 Lakeside Cafe 2026-11-09 84.00 Draft Bread delivery week 45", "INV-000002 Lakeside Cafe 2026-10-05 84.00 Paid Bread delivery week 40"]);
    assert_eq!(r[3].1, ["Lakeside Cafe orders@lakeside.example (555) 010-2233"]);
    assert!(!html.contains("Lakeside Other"), "another company's records never appear");
    // An exact amount, typed with or without cents, finds documents of every kind with that amount.
    for q in ["84", "84.00", "$84.00"] {
        let r = results(&find(&mut b, q));
        assert_eq!(r.iter().find(|(h, _)| h == "Invoices").unwrap().1.len(), 2, "{q}");
        assert!(r.iter().any(|(h, rows)| h == "Payments received" && rows.len() == 1), "{q}: {r:#?}");
    }
    let r = results(&find(&mut b, "96.15"));
    let jn = b.docs["journal_number"].clone();
    assert_eq!(r, [("Journal entries".to_owned(), vec![format!("{jn} Entry no. ADJ-7 2026-11-04 96.15 Posted Packaging accrual")])], "{r:#?}");
    // Memo and reference words.
    for (q, kind, row) in [
        ("wedding", "Invoices", "INV-000001 Maple Events 2026-10-19 725.00 Open Wedding cake, three tier"),
        ("market", "Sales receipts", "SR-000001 Maple Events 2026-10-22 42.75 Recorded Farmers market stall"),
        ("brunch", "Estimates", "EST-000001 Maple Events 2026-11-02 960.00 Draft Anniversary brunch"),
        ("stale", "Credit memos", "CM-000001 Lakeside Cafe 2026-10-25 12.00 Recorded Stale rolls returned"),
        ("holiday order", "Customer deposits", "CDP-000001 Maple Events 2026-10-28 300.00 Recorded Deposit for holiday trays"),
        ("VF-88123", "Bills", "BILL-000001 Ref. VF-88123 Valley Flour Supply 2026-10-12 412.37 Partly paid Flour and sugar, October"),
        ("part payment", "Bill payments", "BPAY-000001 Ref. 1045 Valley Flour Supply 2026-10-30 200.00 Recorded Part payment"),
        ("electric", "Checks & expenses", "CHK-000001 Check no. 1042 City Power and Light 2026-10-28 188.90 Recorded Electric bill"),
        ("slip 77", "Deposits", "DEP-000001 Harbor checking 2026-10-31 84.00 Recorded Counter takings"),
        ("reserve", "Transfers", "TRF-000001 Harbor checking → Harbor savings 2026-11-03 500.00 Recorded Tax reserve"),
        ("paper goods", "Journal entries", format!("{jn} Entry no. ADJ-7 2026-11-04 96.15 Posted Packaging accrual").as_str()),
    ] {
        let r = results(&find(&mut b, q));
        let group = r.iter().find(|(h, _)| h == kind).unwrap_or_else(|| panic!("{q}: {r:#?}"));
        assert_eq!(group.1, [row], "{q}");
    }
    // Names, accounts and items.
    assert_eq!(results(&find(&mut b, "valley")).last().unwrap().1, ["Valley Flour Supply billing@valley.example 555 010 6677"]);
    assert_eq!(results(&find(&mut b, "5550104455")), [("Customers".to_owned(), vec!["Maple Events events@maple.example 555-010-4455".to_owned()])]);
    assert_eq!(results(&find(&mut b, "harbor sav")).last().unwrap(), &("Accounts".to_owned(), vec!["1020 · Harbor savings Asset".to_owned()]));
    assert_eq!(results(&find(&mut b, "sourdough")), [("Items & services".to_owned(), vec!["Sourdough loaf Daily bread, whole loaf 6.50".to_owned()])]);
    // Every result links to the record it names.
    let html = find(&mut b, "maple");
    for id in [&b.docs["invoice_wedding"], &b.docs["sales_receipt"], &b.docs["estimate"], &b.docs["customer_deposit"]] {
        assert!(hrefs(&html).iter().any(|h| h.ends_with(&format!("/{id}/"))), "{id}");
    }
    for href in hrefs(&html) {
        page(&mut b.app, &href);
    }
}

#[test]
fn an_exact_document_number_opens_the_document() {
    let mut b = book();
    let c = b.c.clone();
    let jn: &str = b.docs["journal_number"].clone().leak();
    let go = |b: &mut Book, q: &str| screens::render(&mut b.app, &format!("/c/{c}/find/?q={q}")).unwrap();
    for (q, doc, path) in [("INV-000001", "invoice_wedding", "invoices"), ("inv-000001", "invoice_wedding", "invoices"), ("BILL-000001", "bill", "bills"),
        ("VF-88123", "bill", "bills"), ("CHK-000001", "check", "checks"), ("ADJ-7", "journal", "journals"), (jn, "journal", "journals")] {
        let r = go(&mut b, q);
        assert_eq!((r.status, r.html.as_str()), (303, format!("/c/{c}/{path}/{}/", b.docs[doc]).as_str()), "{q}");
    }
    // `1` is the number of every first document of each kind: a results page, not a jump.
    assert_eq!(go(&mut b, "1").status, 200);
    // `1042` is check number 1042 only (no document numbered 1042, no amount of 1,042.00).
    let r = go(&mut b, "1042");
    assert_eq!((r.status, r.html.as_str()), (303, format!("/c/{c}/checks/{}/", b.docs["check"]).as_str()));
    // A name is never a jump, even when it is exact.
    assert_eq!(go(&mut b, "Maple+Events").status, 200);
    // Too short, too long and empty queries search nothing and say why.
    assert!(find(&mut b, "a").contains("Type at least 2 characters"));
    assert!(find(&mut b, &"x".repeat(121)).contains("Use at most 120 characters"));
    assert!(find(&mut b, "").contains("Use the Find field at the top of the window"));
    assert!(find(&mut b, "zzzz-nothing").contains("Nothing in Synthetic find books matches"));
}

#[test]
fn search_shows_only_what_the_person_may_open() {
    let mut b = book();
    let (c, other) = (b.c.clone(), b.other.clone());
    // The core query refuses a company the actor is not a member of.
    let terms = fb_core::find::Terms::parse("lakeside").unwrap();
    // A bookkeeper of the first company only, and a read-only user of both.
    b.app.save_desktop_user(None, "Synthetic bookkeeper", Role::Bookkeeper, true, Some(PASSWORD), &[b.company]).unwrap();
    let other_id: CompanyId = other.parse().unwrap();
    b.app.sign_in("Synthetic owner", PASSWORD).unwrap();
    b.app.save_desktop_user(None, "Synthetic reader", Role::Readonly, true, Some(PASSWORD), &[b.company, other_id]).unwrap();
    b.app.sign_out();
    b.app.sign_in("Synthetic bookkeeper", PASSWORD).unwrap();
    let keeper = b.app.authenticated_session().unwrap().actor;
    {
        let open = b.app.open_mut().unwrap();
        assert!(fb_core::find::find(&mut open.book, keeper, other_id, &terms, 5).is_err(), "not a member of the other company");
        assert!(!fb_core::find::find(&mut open.book, keeper, b.company, &terms, 5).unwrap().is_empty());
    }
    // The search page is refused exactly where the company's own lists are.
    for path in [format!("/c/{other}/find/?q=lakeside"), format!("/c/{other}/invoices/")] {
        assert!(screens::render(&mut b.app, &path).is_err(), "{path}");
    }
    let html = page(&mut b.app, &format!("/c/{c}/find/?q=lakeside&all=1"));
    assert!(!html.contains("Lakeside Other"));
    for href in hrefs(&html) {
        page(&mut b.app, &href);
    }
    // A read-only user may search and open what they find, and sees no "+ New" menu.
    b.app.sign_out();
    b.app.sign_in("Synthetic reader", PASSWORD).unwrap();
    let html = page(&mut b.app, &format!("/c/{other}/find/?q=lakeside&all=1"));
    assert_eq!(results(&html), [("Customers".to_owned(), vec!["Lakeside Other Cafe".to_owned()])]);
    assert!(!html.contains("titlebar-new"), "read-only users cannot make records");
    let html = page(&mut b.app, &format!("/c/{c}/find/?q=84&all=1"));
    assert!(!hrefs(&html).is_empty());
    for href in hrefs(&html) {
        page(&mut b.app, &href);
    }
    // Signed out, the page is the sign-in redirect like every other page.
    b.app.sign_out();
    let r = screens::render(&mut b.app, &format!("/c/{c}/find/?q=lakeside")).unwrap();
    assert_eq!((r.status, r.html.as_str()), (303, fb_app::identity::SIGN_IN));
}

#[test]
fn the_title_bar_has_find_and_new_on_every_company_page() {
    let mut b = book();
    let c = b.c.clone();
    for path in [format!("/c/{c}/home/"), format!("/c/{c}/invoices/"), format!("/c/{c}/settings/"), "/desktop/mfa/".into()] {
        let html = page(&mut b.app, &path);
        let bar = html.split_once(r#"<header class="titlebar">"#).unwrap().1.split_once("</header>").unwrap().0;
        assert!(bar.contains(&format!(r#"<form class="find" role="search" method="get" action="/c/{c}/find/">"#)), "{path}");
        assert!(bar.contains(r#"<label class="sr-only" for="fb-find">"#) && bar.contains(r#"placeholder="Find invoice, bill, customer, amount""#), "{path}");
        let new = bar.split_once(r#"<details class="menu titlebar-new">"#).unwrap_or_else(|| panic!("{path}: no New menu")).1.split_once("</details>").unwrap().0;
        let links: Vec<String> = new.split(r#"<li><a href=""#).skip(1).map(|s| text(&format!("<{}", s.split("</a>").next().unwrap()))).collect();
        assert_eq!(links, ["Invoice", "Receive payment", "Sales receipt", "Estimate", "Credit memo", "Job ticket", "Enter time", "Bill", "Pay bills", "Write check", "Deposit", "Transfer", "Journal entry"], "{path}");
        assert!(bar.contains(r#"data-fb-nav="back""#) && bar.contains(r#"data-fb-nav="forward""#), "{path}: Back and Forward");
    }
    // Each New entry opens its "new" page for an administrator and a bookkeeper.
    let html = page(&mut b.app, &format!("/c/{c}/home/"));
    let new = html.split_once(r#"<details class="menu titlebar-new">"#).unwrap().1.split_once("</details>").unwrap().0.to_owned();
    let targets: Vec<String> = new.split(r#"<a href=""#).skip(1).map(|s| s.split('"').next().unwrap().to_owned()).collect();
    assert_eq!(targets.len(), 13);
    for t in &targets {
        page(&mut b.app, t);
    }
    b.app.save_desktop_user(None, "Synthetic bookkeeper", Role::Bookkeeper, true, Some(PASSWORD), &[b.company]).unwrap();
    b.app.sign_out();
    b.app.sign_in("Synthetic bookkeeper", PASSWORD).unwrap();
    let html = page(&mut b.app, &format!("/c/{c}/home/"));
    assert!(html.contains("titlebar-new"));
    for t in &targets {
        page(&mut b.app, t);
    }
    // The Companies page has no company to search.
    assert!(!page(&mut b.app, "/").contains(r#"id="fb-find""#));
}
