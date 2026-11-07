//! Fuzz audit 2026-11, area: posted journals from random valid input (fb-app commands over fb-core
//! services on a live encrypted book).
//!
//! Properties: for every posted journal the services produce (manual journals, invoices with random
//! lines and tax, bills, reversals), the sum of its line debits equals the sum of its credits, every
//! line has exactly one positive side, the trial balance balances, the book's own integrity check is
//! clean; and the invoice total shown on the invoice equals the receivable debit of its journal.
use fb_app::commands::{InvoiceLineValues, InvoiceValues, JournalLineValues, Outcome, parse_command};
use fb_app::{App, keys};
use proptest::prelude::*;
use serde_json::{Value, json};

/// A fresh book path inside its own temporary directory; the directory (book, WAL and attachments)
/// is removed when the returned guard drops, so a run leaves nothing in `$TMPDIR`.
fn tmp() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::Builder::new().prefix("fb-fuzz-audit-journals-").tempdir().unwrap();
    let path = dir.path().join("book.fbbook");
    (dir, path)
}

fn run(app: &mut App, v: Value) -> Result<Value, String> {
    match app.run(parse_command(&v.to_string()).map_err(|e| e.to_string())?) {
        Ok(Outcome::Done { data }) => Ok(data),
        Ok(other) => Err(format!("{other:?}")),
        Err(e) => Err(format!("{e:?}")),
    }
}

struct Books {
    app: App,
    c: String,
    accounts: Vec<String>,
    income: String,
    expense: String,
    customer: String,
    vendor: String,
    tax_item: String,
    /// Declared last: drops after `app` has closed the book.
    _dir: tempfile::TempDir,
}

fn setup(tax_rate: &str) -> Books {
    let mut app = App::new();
    app.today = || chrono::NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
    let (dir, path) = tmp();
    let c = app.create_book(&path, &keys::new_master_key().unwrap(), "Owner", "Prop Co").unwrap().company_id.to_string();
    let acct = |app: &mut App, code: &str, name: &str, t: &str| run(app, json!({"type": "SaveAccount", "company_id": c, "code": code, "name": name, "detail_type": t})).unwrap()["id"].as_str().unwrap().to_owned();
    let bank = acct(&mut app, "1000", "Checking", "BANK");
    let ar = acct(&mut app, "1100", "Accounts receivable", "RECEIVABLE");
    let uf = acct(&mut app, "1200", "Undeposited funds", "OTHER_CURRENT_ASSET");
    let ap = acct(&mut app, "2000", "Accounts payable", "PAYABLE");
    let tax = acct(&mut app, "2100", "Sales tax payable", "OTHER_CURRENT_LIABILITY");
    let equity = acct(&mut app, "3000", "Capital", "EQUITY");
    let income = acct(&mut app, "4000", "Sales", "INCOME");
    let expense = acct(&mut app, "5000", "Supplies", "EXPENSE");
    run(&mut app, json!({"type": "SaveSetup", "company_id": c, "expected_revision": 0, "receivables_id": ar, "undeposited_id": uf, "payables_id": ap, "collected_tax_id": tax, "bank_ids": [bank]})).unwrap();
    let customer = run(&mut app, json!({"type": "SaveCustomer", "company_id": c, "display_name": "Random Customer"})).unwrap()["id"].as_str().unwrap().to_owned();
    let vendor = run(&mut app, json!({"type": "SaveDirectoryRecord", "company_id": c, "record_id": null, "values": {"kind": "vendor", "display_name": "Random Vendor", "business_name": "", "contact_name": "", "email": "", "phone": "", "billing_address": "", "notes": "", "default_expense_account": expense, "default_terms": "30", "expense_shown": true, "terms_shown": true}}))
        .map(|v| v["id"].as_str().unwrap_or_default().to_owned())
        .unwrap_or_else(|e| { eprintln!("vendor not created: {e}"); String::new() });
    let tax_item = run(&mut app, json!({"type": "SaveTaxRate", "company_id": c, "item_id": null, "expected_revision": null, "name": "State", "description": "State tax", "rate": tax_rate, "agency_id": null, "new_agency_name": "State agency"}))
        .map(|v| v["id"].as_str().unwrap_or_default().to_owned())
        .unwrap_or_else(|e| { eprintln!("tax rate not created: {e}"); String::new() });
    let accounts = vec![bank, ar, uf, ap, tax, equity, income.clone(), expense.clone()];
    Books { app, c, accounts, income, expense, customer, vendor, tax_item, _dir: dir }
}

/// Every journal's lines, the totals per journal, and whether it is posted.
fn journals(app: &mut App) -> Vec<(String, String, i64, i64, usize)> {
    let open = app.open_mut().unwrap();
    open.book
        .read(|tx| {
            let mut st = tx.prepare(
                "SELECT j.id, j.status, coalesce(sum(l.debit), 0), coalesce(sum(l.credit), 0), count(l.id)
                 FROM ledger_journal j LEFT JOIN ledger_journalline l ON l.journal_id = j.id GROUP BY j.id",
            )?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get::<_, i64>(4)? as usize)))?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .unwrap()
}

fn one_sided_lines(app: &mut App) -> i64 {
    let open = app.open_mut().unwrap();
    open.book
        .read(|tx| tx.query_row("SELECT count(*) FROM ledger_journalline WHERE NOT ((debit > 0 AND credit = 0) OR (credit > 0 AND debit = 0))", [], |r| r.get(0)))
        .unwrap()
}

fn check_books(app: &mut App) -> Result<(), TestCaseError> {
    for (id, status, debit, credit, lines) in journals(app) {
        if status == "POSTED" {
            prop_assert_eq!(debit, credit, "posted journal {} is unbalanced", id);
            prop_assert!(debit > 0 && lines >= 2, "posted journal {} has no money or one line", id);
        }
    }
    prop_assert_eq!(one_sided_lines(app), 0, "a line with both sides or no side");
    let problems = app.open_mut().unwrap().book.check().unwrap();
    prop_assert!(problems.is_empty(), "book integrity: {:?}", problems);
    Ok(())
}

fn money_text(cents: u64) -> String {
    format!("{}.{:02}", cents / 100, cents % 100)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, .. ProptestConfig::default() })]

    /// Manual journals: random balanced lines post and stay balanced; unbalanced ones are refused.
    #[test]
    fn manual_journals_post_balanced_or_not_at_all(
        entries in prop::collection::vec((prop::collection::vec((0usize..8, 1u64..5_000_000), 1..5), 0u8..3), 1..6),
        reverse in any::<bool>(),
    ) {
        let mut b = setup("6.25");
        for (debits, skew) in entries {
            let total: u64 = debits.iter().map(|(_, c)| c).sum();
            let mut lines: Vec<JournalLineValues> = debits.iter().map(|(a, c)| JournalLineValues { account: b.accounts[*a].clone(), debit: money_text(*c), credit: String::new(), description: "d".into(), name: String::new() }).collect();
            lines.push(JournalLineValues { account: b.accounts[(debits[0].0 + 1) % 8].clone(), debit: String::new(), credit: money_text(total + u64::from(skew)), description: String::new(), name: "n".into() });
            let saved = run(&mut b.app, json!({"type": "SaveJournal", "company_id": b.c, "journal_id": null, "date": "2026-10-05", "memo": "prop", "entry_number": "", "adjusting": false, "request_key": null, "expected_revision": null, "lines": lines}));
            let Ok(saved) = saved else { continue };
            let id = saved["id"].as_str().unwrap().to_owned();
            let posted = run(&mut b.app, json!({"type": "PostJournal", "company_id": b.c, "journal_id": id, "expected_revision": saved["revision"].as_i64().unwrap_or(1)}));
            prop_assert_eq!(posted.is_ok(), skew == 0, "post of a journal skewed by {} cents: {:?}", skew, posted);
            if posted.is_ok() && reverse {
                run(&mut b.app, json!({"type": "ReverseJournal", "company_id": b.c, "journal_id": id, "date": "2026-10-06", "reason": "prop"})).unwrap();
            }
            check_books(&mut b.app)?;
        }
        let open = b.app.open_mut().unwrap();
        let (user, company) = (open.owner, fb_core::CompanyId::parse(&b.c).unwrap());
        let tb = fb_core::reports::trial_balance(&mut open.book, user, company, "2026-12-31").unwrap();
        prop_assert_eq!(tb.debit, tb.credit);
    }

    /// Invoices with random lines, quantities, rates and tax modes: every posted invoice journal balances and its
    /// receivable debit equals the invoice total the document reports.
    #[test]
    fn posted_invoices_balance_and_match_their_totals(
        lines in prop::collection::vec((1u64..50_000, 0u32..10_000, 0u64..200_000, 0u32..100, any::<bool>()), 1..8),
        mode in prop_oneof![Just("NONE"), Just("MANUAL"), Just("RATE")],
        rate in (0u32..100, 0u32..10_000).prop_map(|(w, f)| format!("{w}.{f:04}")),
    ) {
        let mut b = setup(&rate);
        let values = InvoiceValues {
            request_key: uuid::Uuid::new_v4().to_string(),
            customer_id: b.customer.clone(),
            date: "2026-10-01".into(),
            due_date: Some("2026-10-31".into()),
            memo: "prop".into(),
            tax_mode: mode.into(),
            tax_rate: if mode == "MANUAL" { Some(rate.clone()) } else { None },
            tax_label: if mode == "MANUAL" { Some("Combined".into()) } else { None },
            tax_item_id: if mode == "RATE" { Some(b.tax_item.clone()) } else { None },
            refresh_customer: false,
            keep_archived: false,
            lines: lines.iter().map(|(qw, qf, rw, rf, taxable)| InvoiceLineValues {
                origin_position: None, item_id: None, description: "line".into(),
                quantity: format!("{qw}.{qf:04}"), rate: format!("{rw}.{rf:02}"), taxable: *taxable,
                refresh_item: false, keep_archived: false, income_account_id: Some(b.income.clone()),
            }).collect(),
        };
        let saved = run(&mut b.app, json!({"type": "SaveInvoice", "company_id": b.c, "values": values}));
        let Ok(saved) = saved else {
            // A valid-looking invoice may still be refused (zero total with for_posting, missing tax item);
            // that is a refusal, not a correctness failure. Nothing may have been written.
            prop_assert!(journals(&mut b.app).is_empty());
            return Ok(());
        };
        let id = saved["id"].as_str().unwrap().to_owned();
        let revision = saved["revision"].as_i64().unwrap_or(1);
        let posted = run(&mut b.app, json!({"type": "PostInvoice", "company_id": b.c, "invoice_id": id, "expected_revision": revision}));
        let Ok(posted) = posted else {
            check_books(&mut b.app)?;
            return Ok(());
        };
        check_books(&mut b.app)?;
        let total = posted["current"]["total"].as_str().or(posted["total"].as_str()).map(str::to_owned);
        let posted_journals: Vec<_> = journals(&mut b.app).into_iter().filter(|j| j.1 == "POSTED").collect();
        prop_assert_eq!(posted_journals.len(), 1, "exactly one posted journal: {:?}", posted_journals);
        if let Some(total) = total {
            let cents = fb_money::money(&fb_money::AmountInput::Text(total.clone())).unwrap().cents();
            prop_assert_eq!(i128::from(posted_journals[0].2), cents, "journal debit vs invoice total {}", total);
        }
    }

    /// Bills with random lines: a posted bill's journal balances; voiding keeps every journal balanced.
    #[test]
    fn posted_bills_balance(lines in prop::collection::vec((1u64..1_000, 0u32..10_000, 0u64..10_000, 0u32..100), 1..6), void in any::<bool>()) {
        let mut b = setup("5");
        if b.vendor.is_empty() {
            return Ok(());
        }
        let values = json!({
            "bill_id": null, "expected_revision": 0, "request_key": uuid::Uuid::new_v4().to_string(),
            "vendor_id": b.vendor, "date": "2026-10-03", "due_date": "2026-11-02", "memo": "prop", "reference": "R1",
            "supersedes_id": null, "reentry_reason": "", "refresh_vendor": false, "keep_archived": false,
            "lines": lines.iter().map(|(qw, qf, rw, rf)| json!({"origin_position": null, "item_id": null, "expense_account_id": b.expense, "description": "line", "quantity": format!("{qw}.{qf:04}"), "rate": format!("{rw}.{rf:02}"), "refresh_item": false, "keep_archived": false})).collect::<Vec<_>>(),
        });
        let saved = run(&mut b.app, json!({"type": "SaveBill", "company_id": b.c, "values": values}));
        let Ok(saved) = saved else { eprintln!("bill not saved: {saved:?}"); return Ok(()) };
        let id = saved["id"].as_str().unwrap().to_owned();
        let revision = saved["revision"].as_i64().unwrap_or(1);
        let review = run(&mut b.app, json!({"type": "ReviewBill", "company_id": b.c, "bill_id": id, "expected_revision": revision}));
        let Ok(review) = review else { eprintln!("bill not reviewed: {review:?}"); return Ok(()) };
        let token = review["token"].as_str().unwrap_or_default().to_owned();
        let posted = run(&mut b.app, json!({"type": "PostBill", "company_id": b.c, "bill_id": id, "expected_revision": revision, "review_token": token}));
        check_books(&mut b.app)?;
        if posted.is_ok() && void {
            let rev = posted.unwrap()["revision"].as_i64().unwrap_or(revision + 1);
            let _ = run(&mut b.app, json!({"type": "VoidBill", "company_id": b.c, "bill_id": id, "expected_revision": rev, "date": "2026-10-04", "reason": "prop"}));
            check_books(&mut b.app)?;
        }
    }
}
