//! The bill and Write check editors: header order, compact lines, the one-step "Save and post" /
//! "Save and record" that posts only the entries the page showed, snapshot checkboxes shown only
//! when they decide something, vendor preselection, and the bills list Pay actions.
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
fn field(html: &str, name: &str) -> String {
    html.split_once(&format!("name=\"{name}\" value=\"")).unwrap().1.split('"').next().unwrap().into()
}
fn submit(app: &mut App, path: &str, values: Vec<(&str, String)>) -> Outcome {
    let html = screens::render(app, path).unwrap().html;
    app.run(Command::SubmitForm {
        path: path.into(),
        token: field(&html, "fb_command_token"),
        fields: values.into_iter().map(|(k, v)| (k.into(), v)).collect(),
    })
    .unwrap()
}
fn account(app: &mut App, c: &str, code: &str, detail: &str) -> String {
    command(app, json!({"type":"SaveAccount","company_id":c,"code":code,"name":format!("Account {code}"),"detail_type":detail}))["id"]
        .as_str()
        .unwrap()
        .into()
}
fn count(app: &mut App, table: &str) -> i64 {
    app.open_mut().unwrap().book.read(|tx| tx.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))).unwrap()
}
fn vendor(app: &mut App, c: &str, name: &str) -> String {
    let o = app.open_mut().unwrap();
    fb_core::directory::save_record(&mut o.book, o.owner, c.parse().unwrap(), fb_core::directory::Model::Vendor, &json!({"display_name":name,"billing_address":"1 Mill Road"}), None)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}
struct Ids {
    c: String,
    bank: String,
    ap: String,
    expense: String,
    supplies: String,
    vendor: String,
}
struct Books {
    app: App,
    k: Ids,
}
fn fixture(payables: bool) -> Books {
    let path = std::env::temp_dir().join(format!("fb-purchase-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    std::fs::create_dir_all(&path).unwrap();
    let mut app = App::new();
    app.today = || chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
    let c = app.create_book(&path.join("book.fbbook"), &keys::new_master_key().unwrap(), "owner", "Purchase books").unwrap().company_id.to_db();
    support::sign_in(&mut app);
    let bank = account(&mut app, &c, "1000", "BANK");
    let ap = account(&mut app, &c, "2000", "PAYABLE");
    let expense = account(&mut app, &c, "5000", "EXPENSE");
    let supplies = account(&mut app, &c, "5100", "EXPENSE");
    command(&mut app, json!({"type":"SaveSetup","company_id":c,"expected_revision":0,"receivables_id":null,"undeposited_id":null,"payables_id":if payables {json!(ap)} else {Value::Null},"bank_ids":[bank]}));
    let vendor = vendor(&mut app, &c, "Valley Flour");
    Books { app, k: Ids { c, bank, ap, expense, supplies, vendor } }
}
/// The canonical entries string the page sends (entries.js `signature`).
fn signature(entries: &[(&str, &str, &str)]) -> String {
    let mut rows = entries.iter().map(|(a, d, c)| format!("{a}:{d}:{c}")).collect::<Vec<_>>();
    rows.sort();
    rows.join("|")
}
/// A three-line bill as the compact editor posts it: Account, Description, Amount (quantity blank).
fn bill_values(b: &Ids, key: &str, action: &str, shown: &str) -> Vec<(&'static str, String)> {
    vec![
        ("request_key", key.into()),
        ("expected_revision", "0".into()),
        ("vendor", b.vendor.clone()),
        ("date", "2026-10-01".into()),
        ("terms", "30".into()),
        ("due_date", "2026-10-31".into()),
        ("reference", "INV-77".into()),
        ("lines-TOTAL_FORMS", "4".into()),
        ("lines-INITIAL_FORMS", "0".into()),
        ("lines-0-expense_account", b.expense.clone()),
        ("lines-0-description", "Flour".into()),
        ("lines-0-rate", "120.00".into()),
        ("lines-1-expense_account", b.supplies.clone()),
        ("lines-1-description", "Boxes".into()),
        ("lines-1-rate", "30.50".into()),
        ("lines-2-expense_account", b.expense.clone()),
        ("lines-2-description", "Sugar".into()),
        ("lines-2-rate", "9.50".into()),
        ("editor_action", action.into()),
        ("shown_entries", shown.into()),
    ]
}
fn shown_bill(b: &Ids) -> String {
    signature(&[(&b.ap, "0.00", "160.00"), (&b.expense, "129.50", "0.00"), (&b.supplies, "30.50", "0.00")])
}
fn bill_state(app: &mut App) -> Vec<String> {
    app.open_mut().unwrap().book.read(|tx| {
        let mut st = tx.prepare("SELECT state FROM payables_bill ORDER BY id").unwrap();
        Ok::<_, fb_core::FbError>(st.query_map([], |r| r.get::<_, String>(0)).unwrap().map(Result::unwrap).collect())
    })
    .unwrap()
}

#[test]
fn new_bill_page_reads_in_order_with_one_compact_line_and_no_rare_fields_open() {
    let mut b = fixture(true);
    let html = screens::render(&mut b.app, &format!("/c/{}/bills/new/", b.k.c)).unwrap().html;
    let at = |needle: &str| html.find(needle).unwrap_or_else(|| panic!("missing {needle}"));
    let order = ["name=\"vendor\"", "name=\"date\"", "name=\"terms\"", "name=\"due_date\"", "name=\"reference\"", "name=\"memo\"", "name=\"supersedes\""];
    for w in order.windows(2) {
        assert!(at(w[0]) < at(w[1]), "{} before {}", w[0], w[1]);
    }
    assert!(html.contains(">Bill date:</label>") && html.contains(">Ref no.</label>"));
    assert!(html.contains("<details class=\"purchase-correction\"><summary>"), "correction folded away");
    assert!(html.contains("name=\"lines-0-expense_account\"") && !html.contains("name=\"lines-1-expense_account\""), "one line to start");
    assert!(html.contains("data-picker=\"account\""));
    assert!(!html.contains("name=\"refresh_vendor\"") && !html.contains("name=\"keep_archived\"") && !html.contains("lines-0-refresh_item\""));
    assert!(html.contains("value=\"post\">Save and post</button>") && html.contains("value=\"save\">Save draft</button>"));
    assert!(html.contains("Accounting entries") && html.contains("name=\"shown_entries\""));
    for gone in ["RECORD ONLY", "test company", "Synthetic", "round half-up", "Due-date convenience"] {
        assert!(!html.contains(gone), "{gone}");
    }
    assert!(html.contains(&format!("\"payables\": \"{}\"", b.k.ap)), "the page knows the payables account it shows");
}

#[test]
fn save_and_post_posts_only_the_entries_the_page_showed() {
    let mut b = fixture(true);
    let path = format!("/c/{}/bills/new/", b.k.c);
    let key = field(&screens::render(&mut b.app, &path).unwrap().html, "request_key");
    // A page that showed different entries (here: a wrong total) saves the draft but does not post.
    let wrong = signature(&[(&b.k.ap, "0.00", "150.00"), (&b.k.expense, "129.50", "0.00"), (&b.k.supplies, "20.50", "0.00")]);
    let out = submit(&mut b.app, &path, bill_values(&b.k, &key, "post", &wrong));
    let Outcome::Invalid { html } = out else { panic!("{out:?}") };
    assert!(html.contains("The entries changed since the page was shown"), "{html}");
    assert!(html.contains("Entries the books would post") && html.contains("160.00"));
    assert_eq!(bill_state(&mut b.app), ["DRAFT"]);
    assert_eq!(count(&mut b.app, "ledger_journal"), 0);
    // The page now edits the saved draft; posting from it with the entries it shows posts once.
    let bill: String = b.app.open_mut().unwrap().book.read(|tx| tx.query_row("SELECT id FROM payables_bill", [], |r| r.get(0))).unwrap();
    assert!(html.contains(&format!("action=\"/c/{}/bills/{bill}/edit/\"", b.k.c)), "{html}");
    let edit = format!("/c/{}/bills/{bill}/edit/", b.k.c);
    let mut values = bill_values(&b.k, &key, "post", &shown_bill(&b.k));
    values.retain(|(k, _)| *k != "request_key" && *k != "expected_revision" && *k != "lines-INITIAL_FORMS" && *k != "lines-TOTAL_FORMS");
    values.extend([("expected_revision", "1".into()), ("lines-TOTAL_FORMS", "4".into()), ("lines-INITIAL_FORMS", "3".into()), ("lines-0-origin_position", "1".into()), ("lines-1-origin_position", "2".into()), ("lines-2-origin_position", "3".into())]);
    let out = submit(&mut b.app, &edit, values);
    assert!(matches!(&out, Outcome::Redirect { location } if location.ends_with(&format!("/bills/{bill}/"))), "{out:?}");
    assert_eq!(bill_state(&mut b.app), ["OPEN"]);
    assert_eq!(count(&mut b.app, "ledger_journal"), 1);
    let total: i64 = b.app.open_mut().unwrap().book.read(|tx| tx.query_row("SELECT sum(credit) FROM ledger_journalline", [], |r| r.get(0))).unwrap();
    assert_eq!(total, 16000);
}

#[test]
fn save_and_post_in_one_step_and_a_repeated_submit_posts_once() {
    let mut b = fixture(true);
    let path = format!("/c/{}/bills/new/", b.k.c);
    let key = field(&screens::render(&mut b.app, &path).unwrap().html, "request_key");
    let out = submit(&mut b.app, &path, bill_values(&b.k, &key, "post", &shown_bill(&b.k)));
    let Outcome::Redirect { location } = out else { panic!("{out:?}") };
    assert_eq!(bill_state(&mut b.app), ["OPEN"]);
    let audit = count(&mut b.app, "ledger_auditevent");
    // The same form submitted again (same request key) opens the posted bill; nothing new posts.
    let again = submit(&mut b.app, &path, bill_values(&b.k, &key, "post", &shown_bill(&b.k)));
    assert!(matches!(&again, Outcome::Redirect { location: l } if *l == location), "{again:?}");
    assert_eq!(count(&mut b.app, "ledger_journal"), 1);
    assert_eq!(count(&mut b.app, "payables_bill"), 1);
    assert_eq!(count(&mut b.app, "ledger_auditevent"), audit);
    // Quantity left blank on the compact line is one: each line's amount is what was typed.
    let amounts: Vec<i64> = b.app.open_mut().unwrap().book.read(|tx| {
        let mut st = tx.prepare("SELECT amount FROM payables_billline l JOIN payables_billrevision r ON r.id=l.revision_id WHERE r.number=1 ORDER BY l.position").unwrap();
        Ok::<_, fb_core::FbError>(st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect())
    })
    .unwrap();
    assert_eq!(amounts, [12000, 3050, 950]);
}

#[test]
fn save_and_post_refused_by_the_books_keeps_the_saved_draft_with_the_reason() {
    let mut b = fixture(false);
    let path = format!("/c/{}/bills/new/", b.k.c);
    let key = field(&screens::render(&mut b.app, &path).unwrap().html, "request_key");
    let out = submit(&mut b.app, &path, bill_values(&b.k, &key, "post", ""));
    let Outcome::Invalid { html } = out else { panic!("{out:?}") };
    assert!(html.contains("Saved as a draft, not posted:"), "{html}");
    assert!(html.contains("payables"), "the reason names what is missing");
    assert_eq!(bill_state(&mut b.app), ["DRAFT"]);
    assert_eq!(count(&mut b.app, "ledger_journal"), 0);
    // Save draft alone still opens the bill, as before.
    let key = field(&screens::render(&mut b.app, &path).unwrap().html, "request_key");
    let mut values = bill_values(&b.k, &key, "save", "");
    for v in values.iter_mut().filter(|(k, _)| *k == "reference") {
        v.1 = "INV-78".into();
    }
    assert!(matches!(submit(&mut b.app, &path, values), Outcome::Redirect { .. }));
    // A line with no description gets a plain message.
    let key = field(&screens::render(&mut b.app, &path).unwrap().html, "request_key");
    let mut values = bill_values(&b.k, &key, "save", "");
    values.retain(|(k, _)| *k != "lines-1-description" && *k != "reference");
    let Outcome::Invalid { html } = submit(&mut b.app, &path, values) else { panic!() };
    assert!(html.contains("Enter a description for this line."));
}

#[test]
fn snapshot_checkboxes_appear_only_when_the_vendor_is_archived_or_changed() {
    let mut b = fixture(true);
    let path = format!("/c/{}/bills/new/", b.k.c);
    let key = field(&screens::render(&mut b.app, &path).unwrap().html, "request_key");
    let Outcome::Redirect { location } = submit(&mut b.app, &path, bill_values(&b.k, &key, "save", "")) else { panic!() };
    let edit = format!("{location}edit/");
    let html = screens::render(&mut b.app, &edit).unwrap().html;
    assert!(!html.contains("name=\"keep_archived\"") && !html.contains("name=\"refresh_vendor\""), "nothing to decide yet");
    // The vendor's address changed in the vendor list: offer to update the bill's copy.
    {
        let o = b.app.open_mut().unwrap();
        fb_core::directory::save_record(&mut o.book, o.owner, b.k.c.parse().unwrap(), fb_core::directory::Model::Vendor, &json!({"display_name":"Valley Flour","billing_address":"9 New Street"}), Some(&b.k.vendor)).unwrap();
    }
    let html = screens::render(&mut b.app, &edit).unwrap().html;
    assert!(html.contains("name=\"refresh_vendor\"") && !html.contains("name=\"keep_archived\""));
    // Archived: the core asks to keep it explicitly, so that box shows instead.
    {
        let o = b.app.open_mut().unwrap();
        fb_core::directory::set_active(&mut o.book, o.owner, b.k.c.parse().unwrap(), fb_core::directory::Model::Vendor, &b.k.vendor, false).unwrap();
    }
    let html = screens::render(&mut b.app, &edit).unwrap().html;
    assert!(html.contains("name=\"keep_archived\"") && !html.contains("name=\"refresh_vendor\""));
}

#[test]
fn vendor_query_preselects_an_active_vendor_on_new_bill_and_check_only() {
    let mut b = fixture(true);
    for doc in ["bills", "checks"] {
        let html = screens::render(&mut b.app, &format!("/c/{}/{doc}/new/?vendor={}", b.k.c, b.k.vendor)).unwrap().html;
        assert!(html.contains(&format!("<option value=\"{}\" selected>", b.k.vendor)), "{doc}");
        assert!(html.contains("data-vendor-preset=\"1\""), "{doc}");
        let html = screens::render(&mut b.app, &format!("/c/{}/{doc}/new/?vendor=00000000-0000-7000-8000-000000000001", b.k.c)).unwrap().html;
        assert!(html.contains("data-vendor-preset=\"0\"") && !html.contains("00000000-0000-7000-8000-000000000001"), "{doc}: unknown vendor ignored");
    }
    {
        let o = b.app.open_mut().unwrap();
        fb_core::directory::set_active(&mut o.book, o.owner, b.k.c.parse().unwrap(), fb_core::directory::Model::Vendor, &b.k.vendor, false).unwrap();
    }
    let html = screens::render(&mut b.app, &format!("/c/{}/bills/new/?vendor={}", b.k.c, b.k.vendor)).unwrap().html;
    assert!(html.contains("data-vendor-preset=\"0\""), "archived vendor ignored");
}

#[test]
fn bills_list_offers_pay_on_payable_rows_and_pay_selected() {
    let mut b = fixture(true);
    let path = format!("/c/{}/bills/new/", b.k.c);
    let key = field(&screens::render(&mut b.app, &path).unwrap().html, "request_key");
    let Outcome::Redirect { location } = submit(&mut b.app, &path, bill_values(&b.k, &key, "post", &shown_bill(&b.k))) else { panic!() };
    let open = location.trim_end_matches('/').rsplit('/').next().unwrap().to_owned();
    let key = field(&screens::render(&mut b.app, &path).unwrap().html, "request_key");
    let mut draft = bill_values(&b.k, &key, "save", "");
    for v in draft.iter_mut().filter(|(k, _)| *k == "reference") {
        v.1 = "INV-99".into();
    }
    let Outcome::Redirect { location } = submit(&mut b.app, &path, draft) else { panic!() };
    let draft = location.trim_end_matches('/').rsplit('/').next().unwrap().to_owned();
    let html = screens::render(&mut b.app, &format!("/c/{}/bills/", b.k.c)).unwrap().html;
    assert!(html.contains(&format!("action=\"/c/{}/payments/new/\"", b.k.c)) && html.contains(">Pay selected</button>"));
    assert!(html.contains(&format!("name=\"bill\" value=\"{open}\"")) && html.contains(&format!("/payments/new/?bill={open}\"")));
    assert!(!html.contains(&format!("name=\"bill\" value=\"{draft}\"")) && !html.contains(&format!("?bill={draft}\"")), "drafts are not payable");
    assert!(html.contains("<strong class=\"overdue\">Overdue</strong>"), "due 2026-10-31, today 2026-11-01");
    let detail = screens::render(&mut b.app, &format!("/c/{}/bills/{open}/", b.k.c)).unwrap().html;
    assert!(detail.contains(&format!("/payments/new/?bill={open}\">Pay bill</a>")));
}

fn check_values(b: &Ids, key: &str, action: &str, shown: &str) -> Vec<(&'static str, String)> {
    vec![
        ("request_key", key.into()),
        ("expected_revision", "0".into()),
        ("bank", b.bank.clone()),
        ("date", "2026-10-15".into()),
        ("check_number", "101".into()),
        ("payee_kind", "VENDOR".into()),
        ("vendor", b.vendor.clone()),
        ("lines-TOTAL_FORMS", "3".into()),
        ("lines-INITIAL_FORMS", "0".into()),
        ("lines-0-tab", "EXPENSE".into()),
        ("lines-0-account", b.expense.clone()),
        ("lines-0-description", "Delivery".into()),
        ("lines-0-quantity", "1".into()),
        ("lines-0-rate", "45.00".into()),
        ("lines-1-tab", "EXPENSE".into()),
        ("lines-1-account", b.supplies.clone()),
        ("lines-1-quantity", "1".into()),
        ("lines-1-rate", "5.25".into()),
        ("lines-2-tab", "EXPENSE".into()),
        ("lines-2-quantity", "1".into()),
        ("editor_action", action.into()),
        ("shown_entries", shown.into()),
    ]
}

#[test]
fn write_check_saves_and_records_only_what_the_page_showed() {
    let mut b = fixture(true);
    let path = format!("/c/{}/checks/new/", b.k.c);
    let html = screens::render(&mut b.app, &path).unwrap().html;
    assert!(html.contains("<h1>Write check</h1>") && html.contains("value=\"record\">Save and record</button>"));
    assert!(html.contains("name=\"lines-0-account\"") && !html.contains("name=\"lines-1-account\""), "one line to start");
    for gone in ["RECORD ONLY", "designated", "Designate"] {
        assert!(!html.contains(gone), "{gone}");
    }
    let key = field(&html, "request_key");
    let wrong = signature(&[(&b.k.bank, "0.00", "50.00"), (&b.k.expense, "45.00", "0.00"), (&b.k.supplies, "5.00", "0.00")]);
    let Outcome::Invalid { html } = submit(&mut b.app, &path, check_values(&b.k, &key, "record", &wrong)) else { panic!() };
    assert!(html.contains("Saved as a draft, not recorded. The entries changed"), "{html}");
    assert_eq!(count(&mut b.app, "ledger_journal"), 0);
    let key = field(&screens::render(&mut b.app, &path).unwrap().html, "request_key");
    let mut values = check_values(&b.k, &key, "record", &signature(&[(&b.k.bank, "0.00", "50.25"), (&b.k.expense, "45.00", "0.00"), (&b.k.supplies, "5.25", "0.00")]));
    for v in values.iter_mut().filter(|(k, _)| *k == "check_number") {
        v.1 = "102".into();
    }
    let out = submit(&mut b.app, &path, values.clone());
    assert!(matches!(out, Outcome::Redirect { .. }), "{out:?}");
    assert_eq!(count(&mut b.app, "ledger_journal"), 1);
    // Repeated submit of the same form: the recorded check opens again, nothing new is recorded.
    assert!(matches!(submit(&mut b.app, &path, values), Outcome::Redirect { .. }));
    assert_eq!(count(&mut b.app, "ledger_journal"), 1);
}

#[test]
fn a_resent_copy_of_the_new_bill_page_never_posts_a_draft_edited_since() {
    let mut b = fixture(true);
    let path = format!("/c/{}/bills/new/", b.k.c);
    let key = field(&screens::render(&mut b.app, &path).unwrap().html, "request_key");
    // Save draft from the new page.
    let out = submit(&mut b.app, &path, bill_values(&b.k, &key, "save", ""));
    assert!(matches!(out, Outcome::Redirect { .. }), "{out:?}");
    let bill: String = b.app.open_mut().unwrap().book.read(|tx| tx.query_row("SELECT id FROM payables_bill", [], |r| r.get(0))).unwrap();
    // The draft is edited (another date and memo) from its own edit page.
    let edit = format!("/c/{}/bills/{bill}/edit/", b.k.c);
    let mut values = bill_values(&b.k, &key, "save", "");
    values.retain(|(k, _)| !["request_key", "expected_revision", "lines-INITIAL_FORMS", "lines-TOTAL_FORMS", "date"].contains(k));
    values.extend([("expected_revision", "1".into()), ("date", "2026-09-01".into()), ("memo", "Edited later".into()), ("lines-TOTAL_FORMS", "4".into()), ("lines-INITIAL_FORMS", "3".into()), ("lines-0-origin_position", "1".into()), ("lines-1-origin_position", "2".into()), ("lines-2-origin_position", "3".into())]);
    let out = submit(&mut b.app, &edit, values);
    assert!(matches!(out, Outcome::Redirect { .. }), "{out:?}");
    // The old new-bill page is sent again with Save and post and the entries it showed: the
    // edited draft (another date) is not posted.
    let out = submit(&mut b.app, &path, bill_values(&b.k, &key, "post", &shown_bill(&b.k)));
    match out {
        Outcome::Invalid { html } => assert!(html.contains("this draft was changed after that page was shown"), "{html}"),
        Outcome::Redirect { location } => panic!("posted or redirected: {location}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(bill_state(&mut b.app), ["DRAFT"]);
    assert_eq!(count(&mut b.app, "ledger_journal"), 0);
}
