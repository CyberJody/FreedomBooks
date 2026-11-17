//! Company Home: the attention row, money cards, task row and recent activity. Every figure must agree
//! with the screen it links to, and a viewer sees only what those screens would show them.
use chrono::NaiveDate;
use fb_app::{App, identity::Role, keys::MasterKey, screens};
use fb_core::services::{AccountInput, DraftInput, line};
use fb_core::{AccountId, CompanyId, UserId, directory::{self, Model}, documents, payables, sales, services};
use serde_json::{Value, json};

const OWNER: &str = "Synthetic owner";
const PASSWORD: &str = "Synthetic individual password";

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
}

struct Fixture {
    _dir: tempfile::TempDir,
    app: App,
    c: String,
    company: CompanyId,
    owner: UserId,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("books/default.fbbook");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut app = App::new();
    app.today = today;
    app.backups.configure(dir.path()).unwrap();
    let company = app.create_book(&path, &MasterKey::new([43; 32]), OWNER, "Synthetic home company").unwrap().company_id;
    app.enroll_owner(PASSWORD).unwrap();
    let owner = app.authenticated_session().unwrap().actor;
    Fixture { _dir: dir, app, c: company.to_db(), company, owner }
}

fn page(app: &mut App, path: &str) -> String {
    let r = screens::render(app, path).unwrap();
    assert_eq!(r.status, 200, "{path}: {}", r.html);
    r.html
}

/// The text between `start` (exclusive) and the next `end`, after the first `marker`.
fn after<'a>(html: &'a str, marker: &str, start: &str, end: &str) -> &'a str {
    let rest = html.split_once(marker).unwrap_or_else(|| panic!("missing {marker}")).1;
    rest.split_once(start).unwrap().1.split_once(end).unwrap().0
}

/// The Home section with this heading id, or "" when Home does not show it.
fn section<'a>(html: &'a str, id: &str) -> &'a str {
    html.split_once(&format!(r#"aria-labelledby="{id}""#)).map(|(_, r)| r.split_once("</section>").unwrap().0).unwrap_or("")
}

struct Seeded {
    bank: AccountId,
    savings: AccountId,
}

/// Through the core services as the signed-in owner: overdue, partly paid, paid, current and draft invoices;
/// overdue, due-soon, later and draft bills; two banks with money and October activity.
fn seed(f: &mut Fixture) -> Seeded {
    let (co, actor) = (f.company, f.owner);
    let book = &mut f.app.open_mut().unwrap().book;
    let acct = |book: &mut fb_core::Book, code: &str, name: &str, kind: &str, detail: Option<&str>| {
        let mut input = AccountInput::new(code, name).kind(kind);
        if let Some(d) = detail {
            input = input.detail(d);
        }
        services::save_account(book, actor, co, input).unwrap().id
    };
    let ar = acct(book, "1100", "Receivables", "ASSET", Some("RECEIVABLE"));
    let ap = acct(book, "2000", "Payables", "LIABILITY", Some("PAYABLE"));
    let bank = acct(book, "1000", "Checking", "ASSET", Some("BANK"));
    let savings = acct(book, "1010", "Savings", "ASSET", Some("BANK"));
    let tax = acct(book, "2100", "Sales tax", "LIABILITY", None);
    let equity = acct(book, "3000", "Owner equity", "EQUITY", None);
    let income = acct(book, "4000", "Sales", "INCOME", None);
    let expense = acct(book, "5000", "Supplies", "EXPENSE", None);
    documents::save_setup(book, actor, co, &json!({"receivables":ar.to_db(),"payables":ap.to_db(),"collected_tax":tax.to_db(),"banks":[bank.to_db(),savings.to_db()]}), Some(0)).unwrap();
    fb_core::salestax::save_settings(book, actor, co, &json!({"expected_revision":0,"charge_tax":false,"default_item_id":null,"owed_basis":"ACCRUAL","filing_frequency":"MONTHLY"})).unwrap();
    let vendor = directory::save_record(book, actor, co, Model::Vendor, &json!({"display_name":"Valley Flour Supply","email":"vendor@example.invalid"}), None).unwrap()["id"].as_str().unwrap().to_owned();
    let customer = directory::save_record(book, actor, co, Model::Customer, &json!({"display_name":"Lakeside Cafe","email":"cafe@example.invalid"}), None).unwrap()["id"].as_str().unwrap().to_owned();
    let journal = |book: &mut fb_core::Book, date: &str, dr: AccountId, cr: AccountId, amount: &str| {
        let j = services::save_draft(book, actor, co, DraftInput::new(date, "Synthetic entry", vec![line(dr).debit(amount).credit("0").build(), line(cr).debit("0").credit(amount).build()])).unwrap();
        services::post_journal(book, actor, co, j.id, None).unwrap();
    };
    journal(book, "2026-09-20", savings, equity, "15000.00");
    journal(book, "2026-10-02", bank, equity, "1200.00");
    let invoice = |book: &mut fb_core::Book, date: &str, due: &str, rate: &str, post: bool| -> Value {
        let i = sales::save_invoice(book, actor, co, &json!({"customer_id":customer,"date":date,"due_date":due,"request_key":uuid::Uuid::new_v4().to_string(),"tax_mode":"NONE","lines":[{"description":"Bread","quantity":"1","rate":rate,"taxable":false,"income_account_id":income.to_db()}]})).unwrap();
        if !post {
            return i;
        }
        let id = i["id"].as_str().unwrap();
        let review = sales::review_invoice(book, actor, co, id, Some(1)).unwrap();
        sales::post_invoice(book, actor, co, id, Some(1), Some(&review)).unwrap()
    };
    let receive = |book: &mut fb_core::Book, inv: &Value, amount: &str| {
        let input = json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":customer,"date":"2026-10-10","amount":amount,"method":"CASH","reference":"","memo":"","destination":"BANK","bank_id":bank.to_db(),"allocations":[{"invoice_id":inv["id"],"amount":amount}]});
        let review = sales::review_receipt(book, actor, co, &input).unwrap();
        sales::record_receipt(book, actor, co, &input, Some(&review)).unwrap();
    };
    invoice(book, "2026-09-25", "2026-10-01", "1250.00", true);
    let partial = invoice(book, "2026-10-01", "2026-10-14", "300.00", true);
    receive(book, &partial, "100.00");
    invoice(book, "2026-10-05", "2026-11-04", "410.10", true);
    invoice(book, "2026-10-09", "2026-10-30", "999.00", false);
    let bill = |book: &mut fb_core::Book, date: &str, due: &str, rate: &str, post: bool| {
        let b = payables::save_bill(book, actor, co, &json!({"vendor_id":vendor,"date":date,"due_date":due,"request_key":uuid::Uuid::new_v4().to_string(),"lines":[{"description":"Flour","quantity":"1","rate":rate,"expense_account_id":expense.to_db()}]})).unwrap();
        if post {
            let id = b["id"].as_str().unwrap();
            let review = payables::review_bill(book, actor, co, id, Some(1)).unwrap();
            payables::post_bill(book, actor, co, id, Some(1), &review).unwrap();
        }
    };
    bill(book, "2026-09-30", "2026-10-10", "220.00", true);
    bill(book, "2026-10-01", "2026-10-20", "140.00", true);
    bill(book, "2026-10-01", "2026-11-30", "35.50", true);
    bill(book, "2026-10-01", "2026-10-21", "500.00", false);
    Seeded { bank, savings }
}

/// Write and verify a real encrypted synthetic backup, including its company binding.
fn backup_file(f: &mut Fixture, stamp: &str) -> std::path::PathBuf {
    use fb_core::backups::{self, BackupOptions, retention};
    let folder = f.app.backups.folder().unwrap();
    std::fs::create_dir_all(&folder).unwrap();
    let work = f._dir.path().join("test-work");
    std::fs::create_dir_all(&work).unwrap();
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&work, std::fs::Permissions::from_mode(0o700)).unwrap(); }
    let output = folder.join(retention::filename("default", stamp).unwrap());
    backups::create_backup(&f.app.open_mut().unwrap().book, &[43;32], &BackupOptions {
        output:&output,work_root:&work,receipts_root:None,verify_receipt:None,label:"default",app_version:"synthetic-test",created_at:stamp,
        passphrase:None,keep:None,allow_incomplete:false,
    }).unwrap();
    backups::verify_backup(&output, &[43;32], &work, None, true).unwrap();
    output
}

#[test]
fn every_home_figure_equals_the_screen_it_links_to() {
    let mut f = fixture();
    let s = seed(&mut f);
    let c = f.c.clone();
    let home = page(&mut f.app, &format!("/c/{c}/home/"));
    let attention = section(&home, "attention-title");
    // Overdue invoices: 1250 + 200 (300 less the 100 received).
    assert!(attention.contains("2 overdue invoices") && attention.contains("1,450.00"), "{attention}");
    assert!(attention.contains(&format!(r#"href="/c/{c}/invoices/?state=OVERDUE""#)));
    let invoices = page(&mut f.app, &format!("/c/{c}/invoices/?state=OVERDUE"));
    assert_eq!(after(&invoices, "Overdue</span>", r#"<span class="tracker-value">"#, "</span>"), "1450.00 USD");
    assert_eq!(after(&invoices, "Open unpaid</span>", r#"<span class="tracker-value">"#, "</span>"), "1860.10 USD");
    let reminders = page(&mut f.app, &format!("/c/{c}/reminders/"));
    assert!(reminders.contains(r#"2 invoices</span> <span class="reminder-total"><span class="amount">1,450.00</span>"#), "reminders agree");
    // Bills: 220 overdue; 140 due within the reminder window; 35.50 later; one draft.
    assert!(attention.contains("1 overdue bill") && attention.contains("220.00"), "{attention}");
    assert!(attention.contains("1 bill due in the next 10 days") && attention.contains("140.00"), "{attention}");
    assert!(attention.contains("1 draft invoice not posted") && attention.contains("1 draft bill not posted"), "{attention}");
    let bills = page(&mut f.app, &format!("/c/{c}/bills/?state=OVERDUE"));
    assert_eq!(after(&bills, "Overdue</span>", r#"<span class="tracker-value">"#, "</span>"), "220.00 USD");
    assert_eq!(after(&bills, "Open unpaid</span>", r#"<span class="tracker-value">"#, "</span>"), "395.50 USD");
    assert_eq!(after(&bills, "Drafts</span>", r#"<span class="tracker-value">"#, "</span>"), "1 draft");
    let soon = page(&mut f.app, &format!("/c/{c}/bills/?state=OUTSTANDING&start=2026-10-15&end=2026-10-25"));
    assert!(soon.contains(">140.00<") && !soon.contains(">220.00<") && !soon.contains(">35.50<"), "the due-soon link lists that bill only");
    // Money cards.
    let money = section(&home, "money-title");
    let receivables = after(money, r#"id="card-receivables""#, r#"<p class="card-figure">"#, "</p>");
    assert!(receivables.contains("1,860.10") && receivables.contains("USD"), "{receivables}");
    let payables = after(money, r#"id="card-payables""#, r#"<p class="card-figure">"#, "</p>");
    assert!(payables.contains("395.50"), "{payables}");
    assert!(money.contains(r#"Next due <time datetime="2026-10-20">20 Oct</time>"#), "{money}");
    // Bank balances: Checking 1200 + 100 received; Savings 15000. Each equals the Bank accounts list.
    assert!(money.contains(&format!(r#"href="/c/{c}/accounts/{}/register/">1000 · Checking</a><span class="amount">1,300.00</span>"#, s.bank)), "{money}");
    assert!(money.contains(&format!(r#"href="/c/{c}/accounts/{}/register/">1010 · Savings</a><span class="amount">15,000.00</span>"#, s.savings)), "{money}");
    let registers = page(&mut f.app, &format!("/c/{c}/registers/"));
    assert!(registers.contains(r#"<td class="amount">1300.00</td>"#) && registers.contains(r#"<td class="amount">15000.00</td>"#), "registers list agrees");
    // This month (October, accrual): income 300 + 410.10; expenses the two October bills (140 + 35.50).
    let month = after(money, r#"id="card-month""#, r#"<dl class="month-figures">"#, "</dl>");
    assert!(month.contains("<dt>Income</dt><dd class=\"amount\">710.10</dd>"), "{month}");
    assert!(month.contains("<dt>Expenses</dt><dd class=\"amount\">175.50</dd>"), "{month}");
    assert!(month.contains("<dt>Net income</dt><dd class=\"amount\">534.60</dd>"), "{month}");
    let pl_href = after(money, r#"id="card-month""#, r#"<a href=""#, "\"");
    assert_eq!(pl_href, format!("/c/{c}/reports/profit-and-loss/?preset=this_month&amp;basis=ACCRUAL"));
    let pl = page(&mut f.app, &format!("/c/{c}/reports/profit-and-loss/?preset=this_month&basis=ACCRUAL"));
    assert!(pl.contains("534.60") && pl.contains("710.10") && pl.contains("175.50"), "the P&L shows the same October figures");
    // Start a task and recent activity.
    let tasks = section(&home, "tasks-title");
    for label in ["New invoice", "Receive payment", "Enter bill", "Pay bills", "Write check", "Record deposit"] {
        assert!(tasks.contains(&format!(">{label}</a>")), "{label}");
    }
    let recent = section(&home, "recent-title");
    assert!(recent.contains("Bill <span class=\"doc-state\">Draft</span>"), "{recent}");
    assert!(recent.contains("Valley Flour Supply") && recent.contains("Lakeside Cafe") && recent.contains("Payment received"), "{recent}");
    // Four invoices, one payment and four bills, newest first, plus the header row.
    assert_eq!(recent.matches("<tr>").count(), 9 + 1, "{recent}");
    // The old link copies are gone and the diagrams are closed once something is posted.
    assert!(!home.contains("home-shelf") && !home.contains(">Transactions</a>"));
    assert!(home.contains(r#"<details class="home-flows"><summary>Show how the pieces fit</summary>"#));
}

#[test]
fn the_attention_row_is_absent_when_nothing_needs_attention_and_backups_are_admin_only() {
    let mut f = fixture();
    let c = f.c.clone();
    let co = f.company;
    let owner = f.owner;
    // Set up and post one current invoice: nothing is overdue, due soon or in draft.
    let book = &mut f.app.open_mut().unwrap().book;
    let ar = services::save_account(book, owner, co, AccountInput::new("1100", "Receivables").kind("ASSET").detail("RECEIVABLE")).unwrap().id;
    let bank = services::save_account(book, owner, co, AccountInput::new("1000", "Checking").kind("ASSET").detail("BANK")).unwrap().id;
    let ap = services::save_account(book, owner, co, AccountInput::new("2000", "Payables").kind("LIABILITY").detail("PAYABLE")).unwrap().id;
    let tax = services::save_account(book, owner, co, AccountInput::new("2100", "Tax").kind("LIABILITY")).unwrap().id;
    let undeposited = services::save_account(book, owner, co, AccountInput::new("1200", "Undeposited funds").kind("ASSET")).unwrap().id;
    let income = services::save_account(book, owner, co, AccountInput::new("4000", "Sales").kind("INCOME")).unwrap().id;
    services::save_account(book, owner, co, AccountInput::new("5000", "Supplies").kind("EXPENSE")).unwrap();
    documents::save_setup(book, owner, co, &json!({"receivables":ar.to_db(),"payables":ap.to_db(),"collected_tax":tax.to_db(),"undeposited":undeposited.to_db(),"banks":[bank.to_db()]}), Some(0)).unwrap();
    fb_core::salestax::save_settings(book, owner, co, &json!({"expected_revision":0,"charge_tax":false,"default_item_id":null,"owed_basis":"ACCRUAL","filing_frequency":"MONTHLY"})).unwrap();
    let customer = directory::save_record(book, owner, co, Model::Customer, &json!({"display_name":"Current customer"}), None).unwrap()["id"].as_str().unwrap().to_owned();
    let i = sales::save_invoice(book, owner, co, &json!({"customer_id":customer,"date":"2026-10-10","due_date":"2026-11-09","request_key":uuid::Uuid::new_v4().to_string(),"tax_mode":"NONE","lines":[{"description":"Bread","quantity":"1","rate":"20.00","taxable":false,"income_account_id":income.to_db()}]})).unwrap();
    let id = i["id"].as_str().unwrap();
    let review = sales::review_invoice(book, owner, co, id, Some(1)).unwrap();
    sales::post_invoice(book, owner, co, id, Some(1), Some(&review)).unwrap();

    // No verified complete backup yet: the administrator is told, with a Back up now link to the Backups page.
    let home = page(&mut f.app, &format!("/c/{c}/home/"));
    let attention = section(&home, "attention-title");
    assert!(attention.contains("No verified complete backup yet") && attention.contains(&format!(r#"href="/c/{c}/backups/""#)) && attention.contains("Back up now"), "{attention}");
    // An old backup is flagged with its age; a recent one clears the row entirely.
    backup_file(&mut f, "2026-09-01T12:00:00Z");
    let home = page(&mut f.app, &format!("/c/{c}/home/"));
    assert!(section(&home, "attention-title").contains("Last verified backup 44 days ago"));
    backup_file(&mut f, "2026-10-14T12:00:00Z");
    let home = page(&mut f.app, &format!("/c/{c}/home/"));
    assert!(!home.contains("Needs attention"), "nothing to say: {}", section(&home, "attention-title"));
    assert!(home.contains(&format!(r#"<a class="home-reminders" href="/c/{c}/reminders/">See all reminders</a>"#)));
    assert!(home.contains("Customers owe you"));

    // A bookkeeper cannot open Backups, so Home never mentions backups to them.
    std::fs::remove_dir_all(f.app.backups.folder().unwrap()).unwrap();
    f.app.save_desktop_user(None, "Synthetic bookkeeper", Role::Bookkeeper, true, Some(PASSWORD), &[f.company]).unwrap();
    f.app.sign_in("Synthetic bookkeeper", PASSWORD).unwrap();
    let home = page(&mut f.app, &format!("/c/{c}/home/"));
    assert!(!home.contains("backup") && !home.contains("/backups/"), "bookkeeper: no backup status");
    assert!(section(&home, "tasks-title").contains("New invoice"));
    assert!(home.contains("Customers owe you") && home.contains("20.00"));
}

#[test]
fn read_only_users_see_figures_but_no_task_row_and_strangers_see_nothing() {
    let mut f = fixture();
    seed(&mut f);
    let c = f.c.clone();
    let other = services::create_company(&mut f.app.open_mut().unwrap().book, f.owner, "Other synthetic company", "USD", false).unwrap().id;
    f.app.save_desktop_user(None, "Synthetic reader", Role::Readonly, true, Some(PASSWORD), &[f.company]).unwrap();
    f.app.sign_in("Synthetic reader", PASSWORD).unwrap();
    let home = page(&mut f.app, &format!("/c/{c}/home/"));
    // The lists and reports these figures come from are open to read-only users, so the figures are too.
    assert!(section(&home, "attention-title").contains("2 overdue invoices"));
    assert!(section(&home, "money-title").contains("1,860.10"));
    assert!(!home.contains("No verified complete backup yet") && !home.contains("/backups/"));
    // They cannot save, so Home offers no tasks.
    assert!(!home.contains("Start a task"));
    for route in ["invoices/new/", "bills/new/", "payments/new/", "receipts/new/", "checks/new/"] {
        assert!(!home.contains(&format!("href=\"/c/{c}/{route}\"")), "readonly action {route}");
    }
    assert!(!home.contains("Save sales tax setup"));
    // A company the person is not a member of renders nothing at all.
    assert!(screens::render(&mut f.app, &format!("/c/{}/home/", other.to_db())).is_err());
}

#[test]
fn a_new_company_gets_setup_steps_and_tasks_instead_of_zeros() {
    let mut f = fixture();
    let c = f.c.clone();
    let home = page(&mut f.app, &format!("/c/{c}/home/"));
    let setup = section(&home, "setup-title");
    assert!(setup.contains("Finish setting up"), "{setup}");
    assert!(setup.contains(&format!(r#"<span class="step-state">To do</span> <a href="/c/{c}/accounting-setup/">Accounting setup</a>"#)), "{setup}");
    assert!(home.contains("Do you collect sales tax?"));
    assert!(section(&home, "tasks-title").contains("New invoice"));
    for absent in ["Customers owe you", "Bank and card balances", "This month", "Recent activity"] {
        assert!(!home.contains(absent), "{absent}");
    }
    // Only true items: no zero counts. The owner still hears about the missing backup.
    let attention = section(&home, "attention-title");
    assert!(!attention.contains(" 0 ") && attention.contains("No verified complete backup yet"), "{attention}");
    assert!(home.contains(r#"<details class="home-flows" open>"#), "a new company sees how the pieces fit");
}

#[test]
fn paid_history_does_not_change_the_open_or_monthly_figures() {
    let mut f = fixture();
    seed(&mut f);
    let c = f.c.clone();
    // A year of paid invoices changes no open figure and no figure for this month.
    let (co, actor) = (f.company, f.owner);
    let book = &mut f.app.open_mut().unwrap().book;
    let customer = directory::list(book.connection(), co, Model::Customer).unwrap()[0]["id"].as_str().unwrap().to_owned();
    let income = fb_core::store::accounts(book.connection(), co).unwrap().into_iter().find(|a| a.code == "4000").unwrap().id;
    let bank = fb_core::store::accounts(book.connection(), co).unwrap().into_iter().find(|a| a.code == "1000").unwrap().id;
    for n in 0..60 {
        let date = format!("2025-{:02}-{:02}", n % 12 + 1, n % 28 + 1);
        let i = sales::save_invoice(book, actor, co, &json!({"customer_id":customer,"date":date,"due_date":date,"request_key":uuid::Uuid::new_v4().to_string(),"tax_mode":"NONE","lines":[{"description":"History","quantity":"1","rate":"10.00","taxable":false,"income_account_id":income.to_db()}]})).unwrap();
        let id = i["id"].as_str().unwrap();
        let review = sales::review_invoice(book, actor, co, id, Some(1)).unwrap();
        sales::post_invoice(book, actor, co, id, Some(1), Some(&review)).unwrap();
        let input = json!({"request_key":uuid::Uuid::new_v4().to_string(),"customer_id":customer,"date":date,"amount":"10.00","method":"CASH","reference":"","memo":"","destination":"BANK","bank_id":bank.to_db(),"allocations":[{"invoice_id":id,"amount":"10.00"}]});
        let review = sales::review_receipt(book, actor, co, &input).unwrap();
        sales::record_receipt(book, actor, co, &input, Some(&review)).unwrap();
    }
    let home = page(&mut f.app, &format!("/c/{c}/home/"));
    assert!(section(&home, "attention-title").contains("1,450.00"));
    assert!(section(&home, "money-title").contains("1,860.10"));
    assert!(section(&home, "money-title").contains("<dt>Income</dt><dd class=\"amount\">710.10</dd>"));
}


#[test]
fn home_never_counts_corrupt_incomplete_other_company_or_future_backups_as_current() {
    use fb_core::backups::retention;
    let mut f = fixture();
    let home = format!("/c/{}/home/", f.c);
    let backup = backup_file(&mut f, "2026-10-14T12:00:00Z");
    assert!(!page(&mut f.app, &home).contains("No verified complete backup yet"));
    let sidecar = retention::sidecar(&backup);
    let original = std::fs::read(&sidecar).unwrap();
    for (field, value) in [("complete", json!(false)), ("company_tags", json!(["another-company"])), ("backup_created_at", json!("2099-10-14T12:00:00Z"))] {
        let mut note: Value = serde_json::from_slice(&original).unwrap();
        note[field] = value;
        std::fs::write(&sidecar, serde_json::to_vec(&note).unwrap()).unwrap();
        assert!(page(&mut f.app, &home).contains("No verified complete backup yet"), "{field}");
    }
    std::fs::write(&sidecar, original).unwrap();
    std::fs::write(&backup, b"corrupt archive").unwrap();
    assert!(page(&mut f.app, &home).contains("No verified complete backup yet"));
    // A recent file with a plausible name and no verified note also cannot clear the warning.
    std::fs::write(backup.parent().unwrap().join(retention::filename("other-book", "2026-10-15T12:00:00Z").unwrap()), b"not a backup").unwrap();
    assert!(page(&mut f.app, &home).contains("No verified complete backup yet"));
}
