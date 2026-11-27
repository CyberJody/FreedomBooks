//! Regression tests for FY2025 home-business simulation findings:
//! - Late-clearing checks (> 5 days) in Bank Review (`CHECK 1003`, 6 days)
//! - Matching one bank withdrawal to multiple posted entries (`$373.74` = `$277.49` + `$96.25`) + reconciliation
//! - Bulk Accept naming the uncategorized row in the top results banner
//! - Filtered Bank Review counts by bank account (full page and fragment POST)
//! - Direct-deposit receipt detail showing the deposit bank account (via Undeposited funds)
//! - Customer deposit apply defaulting to min(invoice outstanding, deposit remaining)
//! - Journal entries setup notice naming what is still open
mod support;
mod worker_support;
use fb_app::commands::StarterChart;
use fb_app::{App, Command, Outcome};
use fb_core::bankfeeds as bf;
use serde_json::{Value, json};
use worker_support::*;

fn url(name: &str, args: &[&str]) -> String {
    fb_templates::django::reverse(name, &args.iter().map(|a| a.to_string()).collect::<Vec<_>>()).unwrap()
}

fn staged(id: &str, date: &str, amount: i64, description: &str) -> bf::StagedTransaction {
    bf::StagedTransaction {
        remote_id: id.into(),
        posted_on: Some(date.into()),
        transacted_on: None,
        amount,
        description: description.into(),
        pending: false,
    }
}

fn feed_account(remote: &str, transactions: Vec<bf::StagedTransaction>) -> bf::StagedAccount {
    bf::StagedAccount {
        conn_key: "synthetic".into(),
        remote_id: remote.into(),
        name: remote.into(),
        org_name: "Synthetic Test Bank".into(),
        currency: "USD".into(),
        balance: None,
        available: None,
        balance_epoch: None,
        skipped: vec![],
        transactions,
    }
}

fn post_review(app: &mut App, path: &str, _html: &str, extra: &[(String, String)]) -> Value {
    let mut fields: Vec<(String, String)> = vec![("inline".into(), "1".into()), ("review_layout".into(), "new".into())];
    fields.extend(extra.iter().cloned());
    let token = app.tokens.mint(path);
    match app.run_renderer(Command::SubmitForm { path: path.into(), token, fields }).unwrap() {
        Outcome::Done { data } => data,
        other => panic!("expected inline fragments, got {other:?}"),
    }
}

fn fragment(data: &Value, id: &str) -> String {
    data["fragments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == id)
        .and_then(|f| f["html"].as_str())
        .unwrap_or("")
        .to_owned()
}

#[test]
fn review_late_check_multi_match_bulk_error_and_filtered_account_counts() {
    let (mut app, c) = fixture(StarterChart::Standard);
    app.simplefin = true;
    let co = fb_core::CompanyId::parse(&c).unwrap();
    let checking = account(&mut app, &c, "1000");
    let expense = account(&mut app, &c, "6060");
    let reserve = {
        let o = app.open_mut().unwrap();
        fb_core::services::save_account(
            &mut o.book,
            o.owner,
            co,
            fb_core::services::AccountInput::new("1030", "Tax Reserve")
                .kind("ASSET")
                .detail("BANK"),
        )
        .unwrap()
        .id
        .to_db()
    };

    // 1. Write Check 1003 on 2026-03-11 for $350.00 (will clear 6 days later on 2026-03-17).
    let (chk_id, chk_line) = {
        let o = app.open_mut().unwrap();
        let saved = fb_core::checks::save_check(
            &mut o.book,
            o.owner,
            co,
            &json!({
                "request_key": key(),
                "bank_id": checking,
                "payee_kind": "OTHER",
                "payee_name": "Valley Tax Prep",
                "date": "2026-03-11",
                "check_number": "1003",
                "memo": "Tax prep",
                "lines": [{"tab": "EXPENSE", "account_id": expense, "description": "Tax prep", "rate": "350.00"}],
            }),
        )
        .unwrap();
        let id = saved["id"].as_str().unwrap().to_owned();
        let rev = fb_core::checks::review_check(&mut o.book, o.owner, co, &id, Some(1)).unwrap();
        let rec = fb_core::checks::record_check(&mut o.book, o.owner, co, &id, Some(1), &rev).unwrap();
        let jid = rec["current"]["journal_id"].as_str().unwrap().to_owned();
        let lid: String = o
            .book
            .read(|q| q.query_row("SELECT id FROM ledger_journalline WHERE journal_id=?1 AND account_id=?2", [&jid, &checking], |r| r.get(0)))
            .unwrap();
        (id, lid)
    };
    let _ = chk_id;

    // 2. Post two outflows on Tax Reserve on 2026-03-20 totaling $373.74 ($277.49 + $96.25).
    let post_journal = |app: &mut App, date: &str, amount: &str, memo: &str| -> String {
        let o = app.open_mut().unwrap();
        let draft = fb_core::services::DraftInput::new(
            date,
            memo,
            vec![
                json!({"account": expense, "debit": amount, "credit": "0.00", "description": memo, "name": ""}),
                json!({"account": reserve, "debit": "0.00", "credit": amount, "description": memo, "name": ""}),
            ],
        );
        let saved = fb_core::services::save_draft(&mut o.book, o.owner, co, draft).unwrap();
        let posted = fb_core::services::post_journal(&mut o.book, o.owner, co, saved.id, Some(saved.revision)).unwrap();
        o.book
            .read(|q| q.query_row("SELECT id FROM ledger_journalline WHERE journal_id=?1 AND account_id=?2", [&posted.id.to_db(), &reserve], |r| r.get(0)))
            .unwrap()
    };
    let line_277 = post_journal(&mut app, "2026-03-20", "277.49", "NY Dec-Feb sales tax");
    let line_96 = post_journal(&mut app, "2026-03-20", "96.25", "NY Dec 2024 sales tax");

    // 3. Stage bank feed rows: Check 1003 (6 days later), uncategorized fee on Checking, and $373.74 NY tax on Tax Reserve.
    let (conn, checking_feed, reserve_feed) = {
        let o = app.open_mut().unwrap();
        let conn = bf::connect(&mut o.book, o.owner, co, &json!("Synthetic bank"), "bridge.simplefin.org", &bf::SecretSet::none())
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let pull = bf::StagedPull {
            accounts: vec![
                feed_account("Checking", vec![
                    staged("TX-CHK-1003", "2026-03-17", -35000, "CHECK 1003"),
                    staged("TX-UNCAT", "2026-03-22", -400, "MONTHLY FEE"),
                ]),
                feed_account("Reserve", vec![
                    staged("TX-NY-373", "2026-03-20", -37374, "NYS DTF SALES TAX PAYMNT"),
                ]),
            ],
            ..Default::default()
        };
        bf::store_pull(&mut o.book, o.owner, co, &conn, &pull, "2026-03-25T12:00:00Z", false, None, &bf::SecretSet::none()).unwrap();
        let feed_id = |o: &mut fb_app::session::Open, name: &str| -> String {
            o.book.read(|q| q.query_row("SELECT id FROM bankfeeds_account WHERE name=?1", [name], |r| r.get(0))).unwrap()
        };
        let (cf, rf) = (feed_id(o, "Checking"), feed_id(o, "Reserve"));
        bf::save_mappings(
            &mut o.book,
            o.owner,
            co,
            &conn,
            &[
                (cf.clone(), bf::MappingTarget::Existing(fb_core::AccountId::parse(&checking))),
                (rf.clone(), bf::MappingTarget::Existing(fb_core::AccountId::parse(&reserve))),
            ],
            &bf::SecretSet::none(),
        )
        .unwrap();
        (conn, cf, rf)
    };
    let pk_of = |app: &mut App, remote: &str| -> i64 {
        app.open_mut()
            .unwrap()
            .book
            .read(|q| q.query_row("SELECT t.ordinal FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id WHERE f.connection_id=?1 AND t.remote_id=?2", [&conn, remote], |r| r.get(0)))
            .unwrap()
    };
    let (pk_chk, pk_uncat, pk_ny) = (pk_of(&mut app, "TX-CHK-1003"), pk_of(&mut app, "TX-UNCAT"), pk_of(&mut app, "TX-NY-373"));

    let review_path = url("bankfeeds:review", &[&c]);
    let html = page(&mut app, &review_path);
    assert!(html.contains("3 to review"), "all accounts count: {html}");
    // Late-clearing Check 1003 is suggested as Match existing entry.
    assert!(html.contains(&format!("value=\"{chk_line}\" selected")), "Check 1003 should be preselected for match");
    // Multi-entry $373.74 row offers "Match existing entries" with both checkboxes.
    assert!(html.contains("Match existing entries"));
    assert!(html.contains(&format!("name=\"match_line_{pk_ny}\" value=\"{line_277}\"")));
    assert!(html.contains(&format!("name=\"match_line_{pk_ny}\" value=\"{line_96}\"")));

    // Filtered count on Reserve shows 1 to review (not 3).
    let reserve_html = page(&mut app, &format!("{review_path}?account={reserve_feed}"));
    assert!(reserve_html.contains("1 to review"), "filtered count must reflect account: {reserve_html}");

    // Match the $373.74 row to BOTH entries via inline POST while filtered to Reserve -> filtered count becomes 0 to review!
    let matched_ny = post_review(
        &mut app,
        &review_path,
        &reserve_html,
        &[
            ("review_account".into(), reserve_feed.clone()),
            (format!("kind_{pk_ny}"), "MATCH".into()),
            (format!("match_line_{pk_ny}"), line_277.clone()),
            (format!("match_line_{pk_ny}"), line_96.clone()),
            ("accept_one".into(), pk_ny.to_string()),
        ],
    );
    let status_frag = fragment(&matched_ny, "review-status");
    assert!(status_frag.contains("0 to review"), "filtered fragment count must update to 0: {status_frag}");
    assert!(status_frag.contains("matched to"), "matched banner wording: {status_frag}");
    let row_frag = fragment(&matched_ny, &format!("review-row-{pk_ny}"));
    assert!(row_frag.contains("Matched:"), "{row_frag}");
    assert!(row_frag.matches("/journals/").count() >= 2, "both matched entries must be linked: {row_frag}");

    // Match late-clearing Check 1003.
    let checking_html = page(&mut app, &format!("{review_path}?account={checking_feed}"));
    assert!(checking_html.contains("2 to review"), "{checking_html}");
    let matched_chk = post_review(
        &mut app,
        &review_path,
        &checking_html,
        &[
            ("review_account".into(), checking_feed.clone()),
            (format!("kind_{pk_chk}"), "MATCH".into()),
            (format!("line_{pk_chk}"), chk_line),
            ("accept_one".into(), pk_chk.to_string()),
        ],
    );
    assert!(fragment(&matched_chk, "review-status").contains("1 to review"));

    // Bulk Accept on uncategorized row names the row and missing category in the top results banner.
    let checking_html2 = page(&mut app, &format!("{review_path}?account={checking_feed}"));
    let bulk_fail = post_review(
        &mut app,
        &review_path,
        &checking_html2,
        &[
            ("review_account".into(), checking_feed),
            ("sel".into(), pk_uncat.to_string()),
            (format!("kind_{pk_uncat}"), "CHECK".into()),
            ("action".into(), "accept".into()),
        ],
    );
    let fail_status = fragment(&bulk_fail, "review-status");
    assert!(fail_status.contains("MONTHLY FEE") && fail_status.contains("Choose a category for this transaction."), "{fail_status}");

    // Bank register marks BOTH lines of the grouped match ($277.49 and $96.25) as seen in the bank feed.
    let reg_html = page(&mut app, &url("register", &[&c, &reserve]));
    assert_eq!(
        reg_html.matches("Seen in a bank feed download").count(),
        2,
        "both grouped lines should show feed badge in register: {reg_html}"
    );

    // Bank Review uses the same MULTI_MATCH_POOL_LIMIT (200) as auto-post and accept: 50 same-day $1.00
    // entries cannot crowd out $40.00 + $60.00 from the previous day when matching a $100.00 withdrawal.
    for _ in 0..50 {
        let _ = post_journal(&mut app, "2026-04-10", "1.00", "Tiny fee");
    }
    let line_40 = post_journal(&mut app, "2026-04-09", "40.00", "Held group part A");
    let line_60 = post_journal(&mut app, "2026-04-09", "60.00", "Held group part B");
    {
        let o = app.open_mut().unwrap();
        let pull2 = bf::StagedPull {
            accounts: vec![feed_account("Reserve", vec![staged("TX-HELD-100", "2026-04-10", -10000, "HELD GROUP 100")])],
            ..Default::default()
        };
        bf::store_pull(&mut o.book, o.owner, co, &conn, &pull2, "2026-04-11T12:00:00Z", false, None, &bf::SecretSet::none()).unwrap();
    }
    let pk_held = pk_of(&mut app, "TX-HELD-100");
    let held_html = page(&mut app, &format!("{review_path}?account={reserve_feed}"));
    assert!(held_html.contains("Match existing entries"), "Review must offer Match existing entries past rank 50");
    assert!(held_html.contains(&format!("name=\"match_line_{pk_held}\" value=\"{line_40}\"")));
    assert!(held_html.contains(&format!("name=\"match_line_{pk_held}\" value=\"{line_60}\"")));
    let journals_before: i64 = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT count(*) FROM ledger_journal", [], |r| r.get(0))).unwrap();
    let held_matched = post_review(
        &mut app,
        &review_path,
        &held_html,
        &[
            ("review_account".into(), reserve_feed.clone()),
            (format!("kind_{pk_held}"), "MATCH".into()),
            (format!("match_line_{pk_held}"), line_40.clone()),
            (format!("match_line_{pk_held}"), line_60.clone()),
            ("accept_one".into(), pk_held.to_string()),
        ],
    );
    assert!(fragment(&held_matched, &format!("review-row-{pk_held}")).contains("Matched:"));
    let journals_after: i64 = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT count(*) FROM ledger_journal", [], |r| r.get(0))).unwrap();
    assert_eq!(journals_before, journals_after, "matching grouped entries past rank 50 must create no new journal");

    // Both multi-matched entries clear cleanly during Tax Reserve reconciliation with $0.00 difference.
    {
        let o = app.open_mut().unwrap();
        let rec = fb_core::banking::begin(
            &mut o.book,
            o.owner,
            co,
            &reserve,
            &json!({
                "request_key": key(),
                "statement_date": "2026-03-31",
                "ending_balance": "-373.74",
            }),
        )
        .unwrap();
        let rec_id = rec["id"].as_str().unwrap().to_owned();
        let ticked = fb_core::banking::save_ticks(
            &mut o.book,
            o.owner,
            co,
            &rec_id,
            &reserve,
            &json!({
                "expected_revision": rec["revision"],
                "shown": { &line_277: 0, &line_96: 0 },
                "ticked": [&line_277, &line_96],
            }),
        )
        .unwrap();
        let figs = o.book.read(|q| fb_core::banking::figures(q, co, &ticked)).unwrap();
        assert_eq!(figs["difference"], "0.00", "{figs}");
        let done = fb_core::banking::reconcile_now(
            &mut o.book,
            o.owner,
            co,
            &rec_id,
            &reserve,
            ticked["revision"].as_i64(),
            &json!(key()),
        )
        .unwrap();
        assert_eq!(done["status"], "DONE");
    }

    assert!(app.open_mut().unwrap().book.check().unwrap().is_empty());
}

#[test]
fn a_populated_format_96_book_upgrades_and_supports_late_checks_and_multi_match() {
    use fb_db::{Book, Opened, SnapshotSource, test_support};
    let (mut app, c) = fixture(StarterChart::Standard);
    app.simplefin = true;
    let co = fb_core::CompanyId::parse(&c).unwrap();
    let checking = account(&mut app, &c, "1000");
    let expense = account(&mut app, &c, "6160");
    let (conn_id, chk_line, l1, l2) = {
        let o = app.open_mut().unwrap();
        let saved = fb_core::checks::save_check(
            &mut o.book,
            o.owner,
            co,
            &json!({
                "request_key": key(),
                "bank_id": checking,
                "payee_kind": "OTHER",
                "payee_name": "Late Payee",
                "date": "2026-03-11",
                "check_number": "1003",
                "memo": "Late check",
                "lines": [{"tab": "EXPENSE", "account_id": expense, "description": "Late check", "rate": "350.00"}],
            }),
        )
        .unwrap();
        let cid = saved["id"].as_str().unwrap().to_owned();
        let rev = fb_core::checks::review_check(&mut o.book, o.owner, co, &cid, Some(1)).unwrap();
        let rec = fb_core::checks::record_check(&mut o.book, o.owner, co, &cid, Some(1), &rev).unwrap();
        let jid = rec["current"]["journal_id"].as_str().unwrap().to_owned();
        let line_of = |book: &mut fb_db::Book, j: &str| -> String {
            book.read(|q| q.query_row("SELECT id FROM ledger_journalline WHERE journal_id=?1 AND account_id=?2", [j, checking.as_str()], |r| r.get(0))).unwrap()
        };
        let chk_line = line_of(&mut o.book, &jid);
        let post_j = |book: &mut fb_db::Book, amt: &str| -> String {
            let d = fb_core::services::DraftInput::new(
                "2026-03-20",
                "Split tax",
                vec![
                    json!({"account": expense, "debit": amt, "credit": "0.00", "description": "Tax", "name": ""}),
                    json!({"account": checking, "debit": "0.00", "credit": amt, "description": "Tax", "name": ""}),
                ],
            );
            let s = fb_core::services::save_draft(book, o.owner, co, d).unwrap();
            let p = fb_core::services::post_journal(book, o.owner, co, s.id, Some(s.revision)).unwrap();
            line_of(book, &p.id.to_db())
        };
        let l1 = post_j(&mut o.book, "277.49");
        let l2 = post_j(&mut o.book, "96.25");
        let l_single = post_j(&mut o.book, "45.00");
        let conn = bf::connect(&mut o.book, o.owner, co, &json!("Format 96 bank"), "bridge.simplefin.org", &bf::SecretSet::none()).unwrap()["id"].as_str().unwrap().to_owned();
        let pull = bf::StagedPull {
            accounts: vec![feed_account("Checking", vec![
                staged("TX-SINGLE-96", "2026-03-20", -4500, "PRIOR SINGLE MATCH"),
                staged("TX-CHK", "2026-03-25", -35000, "CHECK 1003"),
                staged("TX-MULTI", "2026-03-20", -37374, "NYS DTF"),
            ])],
            ..Default::default()
        };
        bf::store_pull(&mut o.book, o.owner, co, &conn, &pull, "2026-03-26T12:00:00Z", false, None, &bf::SecretSet::none()).unwrap();
        let feed: String = o.book.read(|q| q.query_row("SELECT id FROM bankfeeds_account WHERE connection_id=?1", [&conn], |r| r.get(0))).unwrap();
        bf::save_mappings(&mut o.book, o.owner, co, &conn, &[(feed, bf::MappingTarget::Existing(fb_core::AccountId::parse(&checking)))], &bf::SecretSet::none()).unwrap();
        // Record a single-entry match BEFORE snapshotting format 96 so the upgrade preserves an existing MATCHED row.
        let single_tx_id: String = o.book.read(|q| q.query_row("SELECT id FROM bankfeeds_transaction WHERE remote_id='TX-SINGLE-96'", [], |r| r.get(0))).unwrap();
        let single_done = bf::accept(&mut o.book, o.owner, co, &single_tx_id, &json!({"kind": "MATCH", "journal_line_id": l_single}), &bf::SecretSet::none()).unwrap();
        assert_eq!(single_done.status, "MATCHED");
        (conn, chk_line, l1, l2)
    };
    let owner = app.open_mut().unwrap().owner;
    let dir = temp_dir();
    let key_bytes = [96; 32];
    let old_path = dir.join("format-96.fbbook");
    let copy = test_support::populated_copy(app.open_mut().unwrap().book.connection(), &old_path, &key_bytes, "0209_deposit_adjustments", true).unwrap();
    let Opened::Behind(old) = Book::open_staged(&old_path, &key_bytes).unwrap() else { panic!("expected a format-96 book") };
    assert_eq!((old.from(), old.to()), (96, 98));
    let backup = dir.join("verified-format-96.fbbook");
    old.snapshot(&backup, &key_bytes).unwrap();
    assert_eq!(test_support::snapshot_file(&backup, &key_bytes).unwrap(), copy.before);
    let mut upgraded = old.upgrade().unwrap();
    assert_eq!(test_support::snapshot(upgraded.connection()).unwrap(), copy.expected);
    assert!(upgraded.check().unwrap().is_empty());
    upgraded.release_unreadable_bankfeed(&conn_id, &key_bytes).unwrap();

    // After upgrade, both late-clearing Check 1003 (14 days) and the 2-entry $373.74 match work cleanly.
    let tx_id = |book: &mut Book, remote: &str| -> String {
        book.read(|q| q.query_row("SELECT id FROM bankfeeds_transaction WHERE remote_id=?1", [remote], |r| r.get(0))).unwrap()
    };
    let (id_chk, id_multi) = (tx_id(&mut upgraded, "TX-CHK"), tx_id(&mut upgraded, "TX-MULTI"));
    let m1 = bf::accept(&mut upgraded, owner, co, &id_chk, &json!({"kind": "MATCH", "journal_line_id": chk_line}), &bf::SecretSet::none()).unwrap();
    assert_eq!(m1.status, "MATCHED");
    let m2 = bf::accept(&mut upgraded, owner, co, &id_multi, &json!({"kind": "MATCH", "journal_line_ids": [l1, l2]}), &bf::SecretSet::none()).unwrap();
    assert_eq!(m2.status, "MATCHED");
    assert_eq!(upgraded.read(|q| bf::matched_entries(q, co, &m2)).unwrap().len(), 2);
    assert!(upgraded.check().unwrap().is_empty());
    drop(upgraded);
    let reopened = Book::open(&old_path, &key_bytes).unwrap();
    assert!(reopened.check().unwrap().is_empty());
}

#[test]
fn receipt_detail_shows_direct_deposit_bank_and_deposit_apply_caps_to_remaining() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let co = fb_core::CompanyId::parse(&c).unwrap();
    let checking = account(&mut app, &c, "1000");
    let fee_acct = account(&mut app, &c, "6160");
    let income = account(&mut app, &c, "4000");
    let cust_dep_acct = new_account(&mut app, &c, "2250", "Customer Deposits", "OTHER_CURRENT_LIABILITY");
    // Configure customer_deposits in Accounting setup.
    {
        let o = app.open_mut().unwrap();
        let s = o.book.read(|q| fb_core::documents::setup_snapshot(q, co)).unwrap();
        let rev = s["revision"].as_i64();
        fb_core::documents::save_setup(
            &mut o.book,
            o.owner,
            co,
            &json!({
                "receivables": s["receivables"],
                "payables": s["payables"],
                "undeposited": s["undeposited"],
                "collected_tax": s["collected_tax"],
                "customer_deposits": cust_dep_acct,
                "banks": s["banks"],
            }),
            rev,
        )
        .unwrap();
    }
    let customer = customer(&mut app, &c, "Pine Hollow");
    let post_inv = |app: &mut App, date: &str, rate: &str| -> String {
        let o = app.open_mut().unwrap();
        let inv = fb_core::sales::save_invoice(
            &mut o.book,
            o.owner,
            co,
            &json!({
                "request_key": key(),
                "customer_id": customer,
                "date": date,
                "due_date": date,
                "tax_mode": "NONE",
                "lines": [{"description": "Service", "income_account_id": income, "quantity": "1", "rate": rate, "taxable": false}],
            }),
        )
        .unwrap();
        let id = inv["id"].as_str().unwrap().to_owned();
        let rev = fb_core::sales::review_invoice(&mut o.book, o.owner, co, &id, Some(1)).unwrap();
        fb_core::sales::post_invoice(&mut o.book, o.owner, co, &id, Some(1), Some(&rev)).unwrap();
        id
    };

    // 1. Direct-deposit card payment with fee (record_receipt_net) shows "Deposited to 1000 · Checking (via Undeposited funds)".
    let inv1 = post_inv(&mut app, "2026-06-01", "865.63");
    let rcp_id = {
        let o = app.open_mut().unwrap();
        let done = fb_core::banking::record_receipt_net(
            &mut o.book,
            o.owner,
            co,
            &json!({
                "request_key": key(),
                "customer_id": customer,
                "date": "2026-06-08",
                "method": "CARD",
                "amount": "865.63",
                "reference": "TP-1",
                "memo": "Card payout",
                "allocations": [{"invoice_id": inv1, "amount": "865.63"}],
            }),
            &checking,
            &json!([{"account_id": fee_acct, "amount": "-22.66"}]),
        )
        .unwrap();
        done["receipt"]["id"].as_str().unwrap().to_owned()
    };
    let rcp_html = page(&mut app, &url("receipts:detail", &[&c, &rcp_id]));
    assert!(rcp_html.contains("Deposited to 1000 · Checking (via Undeposited funds)"), "{rcp_html}");

    // 2. Customer deposit of $500.00 applied to a $1,835.74 invoice defaults the amount box to 500.00 (not 1835.74).
    let inv2 = post_inv(&mut app, "2026-06-15", "1835.74");
    let dep_id = {
        let o = app.open_mut().unwrap();
        let payload = json!({
            "request_key": key(),
            "customer_id": customer,
            "date": "2026-06-10",
            "method": "CHECK",
            "destination": "UNDEPOSITED",
            "amount": "500.00",
            "reference": "DEP-CHK",
            "memo": "Retainer",
        });
        let rev = fb_core::prepayments::review_deposit(&mut o.book, o.owner, co, &payload).unwrap();
        fb_core::prepayments::record_deposit(&mut o.book, o.owner, co, Some(&rev), &payload).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let apply_html = page(&mut app, &url("prepayments:invoice_apply", &[&c, &inv2]));
    assert!(apply_html.contains("value=\"500.00\""), "deposit apply should default to remaining 500.00: {apply_html}");
    assert!(apply_html.contains(&format!("value=\"{dep_id}\" selected")), "single eligible deposit should be preselected: {apply_html}");

    // 3. When a second eligible deposit exists ($200.00), invoice_apply leaves amount blank rather than
    //    defaulting to the larger deposit's balance and failing if the user picks the smaller one.
    {
        let o = app.open_mut().unwrap();
        let payload = json!({
            "request_key": key(),
            "customer_id": customer,
            "date": "2026-06-11",
            "method": "CHECK",
            "destination": "UNDEPOSITED",
            "amount": "200.00",
            "reference": "DEP-CHK-2",
            "memo": "Second retainer",
        });
        let rev = fb_core::prepayments::review_deposit(&mut o.book, o.owner, co, &payload).unwrap();
        fb_core::prepayments::record_deposit(&mut o.book, o.owner, co, Some(&rev), &payload).unwrap();
    }
    let multi_apply_html = page(&mut app, &url("prepayments:invoice_apply", &[&c, &inv2]));
    assert!(
        multi_apply_html.contains("<input type=\"text\" name=\"amount\" inputmode=\"decimal\""),
        "multi-deposit apply should leave amount blank until a deposit is chosen: {multi_apply_html}"
    );
    assert!(!multi_apply_html.contains("value=\"500.00\"") && !multi_apply_html.contains("value=\"1835.74\""));
}

#[test]
fn journal_list_setup_notice_names_remaining_checklist_items() {
    let (mut app, c) = fixture(StarterChart::Standard);
    // Standard fixture starts with first_run_open = true before setup_done.
    let html = page(&mut app, &url("company", &[&c]));
    assert!(html.contains("Setup checklist still open"), "{html}");
    assert!(html.contains("Finish setting up"), "{html}");
}

#[test]
fn visa_payments_in_bank_review_match_both_sides_of_three_exact_transfers() {
    let (mut app, company) = fixture(StarterChart::Standard);
    app.simplefin = true;
    let co = company.parse().unwrap();
    let bank = new_account(&mut app, &company, "1010", "FY2025 Business Checking", "BANK");
    let visa = new_account(&mut app, &company, "2210", "FY2025 Business Visa", "CREDIT_CARD");
    let second_card = new_account(&mut app, &company, "2220", "FY2025 Mastercard", "CREDIT_CARD");
    let equity = new_account(&mut app, &company, "3999", "FY2025 balance offset", "EQUITY");
    let payments = [("2025-10-15", 53_234), ("2025-11-15", 110_058), ("2025-12-15", 46_220)];
    let mut bank_rows = Vec::new();
    let mut card_rows = Vec::new();
    for (i, (date, amount)) in payments.into_iter().enumerate() {
        bank_rows.push(staged(&format!("bank-{i}"), date, -amount, "SUMMIT CARD ONLINE PMT"));
        card_rows.push(staged(&format!("card-{i}"), date, amount, "PAYMENT THANK YOU"));
    }

    {
        let o = app.open_mut().unwrap();
        // September statement balances and the net of all Q4 activity outside the three payments.
        for (date, bank_change, card_change, offset) in [
            ("2025-09-30", "4250.36", "554.81", "3695.55"),
            ("2025-12-31", "2388.47", "2116.56", "271.91"),
        ] {
            let draft = fb_core::services::save_draft(&mut o.book, o.owner, co,
                fb_core::services::DraftInput::new(date, "FY2025 statement activity", vec![
                    json!({"account":bank,"debit":bank_change}),
                    json!({"account":visa,"credit":card_change}),
                    json!({"account":equity,"credit":offset}),
                ])).unwrap();
            fb_core::services::post_journal(&mut o.book, o.owner, co, draft.id, None).unwrap();
        }
        let conn = bf::connect(&mut o.book, o.owner, co, &json!("FY2025 SimpleFIN"), "bank.example", &bf::SecretSet::none()).unwrap();
        bf::store_pull(&mut o.book, o.owner, co, conn["id"].as_str().unwrap(), &bf::StagedPull {
            accounts: vec![feed_account("Bank Checking", bank_rows), feed_account("Visa Feed", card_rows)],
            ..Default::default()
        }, "2025-12-31T19:00:00Z", false, None, &bf::SecretSet::none()).unwrap();
        for (label, account) in [("Bank Checking", &bank), ("Visa Feed", &visa)] {
            let id: String = o.book.read(|q| q.query_row("SELECT id FROM bankfeeds_account WHERE name=?1", [label], |r| r.get(0))).unwrap();
            bf::map_account(&mut o.book, o.owner, co, &id, &json!(account), &bf::SecretSet::none()).unwrap();
        }
    }

    let review = url("bankfeeds:review", &[&company]);
    let transfer = url("transfers:new", &[&company]);
    let review_html = fb_app::screens::render(&mut app, &review).unwrap().html;
    assert!(review_html.contains("Card payment"));
    assert!(review_html.contains("Balance transfer"));
    assert!(fb_app::screens::render(&mut app, &transfer).unwrap().html.contains("FY2025 Business Visa"));
    assert!(fb_app::screens::render(&mut app, &transfer).unwrap().html.contains(&second_card));
    let item = |app: &mut App, remote: &str| {
        let o = app.open_mut().unwrap();
        o.book.read(|q| {
            let id: String = q.query_row("SELECT id FROM bankfeeds_transaction WHERE remote_id=?1", [remote], |r| r.get(0))?;
            bf::item(q, co, &id)
        }).unwrap()
    };
    for (i, (_, amount)) in payments.into_iter().enumerate() {
        let bank_remote = format!("bank-{i}");
        let card_remote = format!("card-{i}");
        let bank_row = item(&mut app, &bank_remote);
        let card_row = item(&mut app, &card_remote);
        let (first, last, other, last_other) = if i == 1 {
            (bank_row, card_row, &visa, &bank)
        } else {
            (card_row, bank_row, &bank, &visa)
        };
        let p = first.ordinal;
        let n = last.ordinal;
        let accepted = post_review(&mut app, &review, &review_html, &[
            ("action".into(), "accept".into()),
            ("sel".into(), p.to_string()), ("sel".into(), n.to_string()),
            (format!("kind_{p}"), "TRANSFER".into()), (format!("to_{p}"), other.clone()),
            (format!("kind_{n}"), "TRANSFER".into()), (format!("to_{n}"), last_other.clone()),
        ]);
        assert!(fragment(&accepted, &format!("review-row-{n}")).contains("Choose Match or Record again"), "{accepted}");
        let first = item(&mut app, &first.remote_id);
        assert_eq!(first.accepted_as, "TRANSFER");
        let entry = item(&mut app, &last.remote_id);
        assert_eq!(entry.status, "NEW");
        let journal = first.journal_id.as_deref().unwrap();
        let (line, rows) = app.open_mut().unwrap().book.read(|q| {
            let line: String = q.query_row("SELECT id FROM ledger_journalline WHERE journal_id=?1 AND account_id=?2", [journal, other], |r| r.get(0))?;
            let mut s = q.prepare("SELECT account_id,debit,credit FROM ledger_journalline WHERE journal_id=?1 ORDER BY position")?;
            let rows = s.query_map([journal], |r| Ok((r.get::<_,String>(0)?, r.get::<_,i64>(1)?, r.get::<_,i64>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
            Ok::<_, fb_core::FbError>((line, rows))
        }).unwrap();
        assert_eq!(rows, [(bank.clone(), 0, amount), (visa.clone(), amount, 0)]);
        post_review(&mut app, &review, &review_html, &[("accept_one".into(), n.to_string()),
            (format!("kind_{n}"), "MATCH".into()), (format!("line_{n}"), line.clone())]);
        let matched = item(&mut app, &last.remote_id);
        assert_eq!(matched.status, "MATCHED");
        assert_eq!(matched.journal_id, first.journal_id);
        assert_eq!(matched.journal_line_id.as_deref(), Some(line.as_str()));
        assert_ne!(matched.journal_line_id, first.journal_line_id);
    }
    let o = app.open_mut().unwrap();
    let balance = |o: &mut fb_app::session::Open, id: &str| {
        fb_core::transfers::posted_balance(&mut o.book, o.owner, co, id.parse().unwrap()).unwrap()
    };
    assert_eq!(balance(o, &bank), "4543.71");
    assert_eq!(balance(o, &visa), "576.25");
    assert!(o.book.check().unwrap().is_empty());
}

#[test]
#[ignore = "requires the existing FY2025 generated synthetic feed fixtures"]
fn replay_actual_fy2025_q3_to_q4_cumulative_download_adds_157_ids() {
    let root = std::path::PathBuf::from(std::env::var_os("FB_FY2025_DATASETS").expect("synthetic Q3 and Q4 dataset directory"));
    let (mut app, company) = fixture(StarterChart::Standard);
    let co = company.parse().unwrap();
    let o = app.open_mut().unwrap();
    let conn = bf::connect(&mut o.book, o.owner, co, &json!("Synthetic replay"), "bank.example", &bf::SecretSet::none()).unwrap();
    let id = conn["id"].as_str().unwrap();
    let read = |quarter| {
        let data: Value = serde_json::from_slice(&std::fs::read(root.join(format!("fy2025-{quarter}/feed.json"))).unwrap()).unwrap();
        let mut accounts = vec![];
        for a in data["accounts"].as_array().unwrap() {
            let as_str = |k: &str| a[k].as_str().unwrap();
            let transactions = a["transactions"].as_array().unwrap().iter().map(|t| {
                // The fixture's timestamps are at local midday, so their UTC day is also the Review day.
                let date = chrono::DateTime::from_timestamp(t["posted"].as_i64().unwrap(), 0).unwrap().date_naive().to_string();
                let text = t["amount"].as_str().unwrap();
                let amount = fb_core::services::money_json(&json!(text.trim_start_matches('-'))).unwrap().to_db().unwrap();
                let amount = if text.starts_with('-') { -amount } else { amount };
                staged(t["id"].as_str().unwrap(), &date, amount, t["description"].as_str().unwrap())
            }).collect();
            let mut entry = feed_account(as_str("id"), transactions);
            entry.name = as_str("name").into();
            entry.conn_key = as_str("conn_id").into();
            accounts.push(entry);
        }
        bf::StagedPull { accounts, ..Default::default() }
    };
    let q3 = bf::store_pull(&mut o.book, o.owner, co, id, &read("q3"),
        "2025-09-30T19:00:00Z", false, None, &bf::SecretSet::none()).unwrap();
    let q4 = bf::store_pull(&mut o.book, o.owner, co, id, &read("q4"),
        "2025-12-31T19:00:00Z", false, None, &bf::SecretSet::none()).unwrap();
    assert_eq!(q3["new"], 462);
    assert_eq!(q4["unchanged"], 462);
    assert_eq!(q4["new"], 157);
    let boundaries: (String, String, i64) = o.book.read(|q| q.query_row(
        "SELECT min(posted_on),max(posted_on),count(*) FROM bankfeeds_transaction WHERE ordinal>462", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    )).unwrap();
    assert_eq!(boundaries, ("2025-10-01".into(), "2025-12-31".into(), 157));
    assert!(o.book.check().unwrap().is_empty());
}
