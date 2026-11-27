//! Bookkeeper rounds 6 and 7 and the Neon tests of 2026-11-24 (internal test notes, fix list items 37-45).
//! Page behaviour is tested in `electron/tests/worker-fixes-3.test.cjs`; this file tests what the server draws
//! and decides. Item 41 and 44 tests are in `attach_flow.rs`.
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
/// The opening tag of the input named `name`.
fn input<'a>(html: &'a str, name: &str) -> &'a str {
    let at = html.find(&format!("name=\"{name}\"")).unwrap_or_else(|| panic!("no {name}: {html}"));
    let start = html[..at].rfind('<').unwrap();
    &html[start..at + html[at..].find('>').unwrap() + 1]
}
fn value(html: &str, name: &str) -> String {
    let tag = input(html, name);
    tag.split_once("value=\"").map(|(_, v)| v.split('"').next().unwrap().to_owned()).unwrap_or_default()
}
fn invalid(o: Outcome) -> String {
    match o {
        Outcome::Invalid { html } => html,
        o => panic!("expected the form again: {o:?}"),
    }
}
fn submit_page(app: &mut App, path: &str, fields: Vec<(String, String)>) -> Outcome {
    let token = app.tokens.mint(path);
    app.run(Command::SubmitForm { path: path.into(), token, fields }).unwrap()
}
fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
    p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}
fn count(app: &mut App, sql: &str) -> i64 {
    app.open_mut().unwrap().book.read(|q| q.query_row(sql, [], |r| r.get(0))).unwrap()
}

/// Item 38: a box that holds a suggested check or entry number (or the default connection name) is marked
/// `data-select-on-focus`, so the preload selects it on focus and typing replaces it (101 typed into the
/// suggested "1" became "1101"). The page behaviour is in worker-fixes-3.test.cjs.
#[test]
fn boxes_that_hold_a_suggested_number_are_marked_to_select_on_focus() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let marked = |html: &str, name: &str| input(html, name).contains("data-select-on-focus");
    let html = page(&mut app, &url("checks:new", &[&c]));
    assert!(marked(&html, "check_number"), "Write check: {}", input(&html, "check_number"));
    let html = page(&mut app, &url("payments:new", &[&c]));
    assert!(marked(&html, "reference"), "Pay bills, by check: {}", input(&html, "reference"));
    let html = page(&mut app, &url("journal_new", &[&c]));
    assert!(marked(&html, "entry_number"), "journal Entry no.: {}", input(&html, "entry_number"));
    // A free-text box stays as it was.
    assert!(!marked(&html, "memo"));
    let html = page(&mut app, &url("company_edit", &[&c]));
    assert!(marked(&html, "next_number"), "Document numbering: {}", input(&html, "next_number"));
    // Pay bills by another method: a free-text reference, not marked.
    let path = url("payments:new", &[&c]);
    let html = invalid(submit_page(&mut app, &path, pairs(&[("request_key", &key()), ("bank", ""), ("date", "2026-11-23"), ("method", "EFT"), ("reference", "ACH 5/3")])));
    assert!(!marked(&html, "reference"), "{}", input(&html, "reference"));
}

/// Item 38, the connection name: marked only while it holds the suggested name.
#[test]
fn the_connection_name_is_marked_only_while_it_is_the_suggestion() {
    let (mut app, c) = fixture(StarterChart::Standard);
    app.simplefin = true;
    let path = url("bankfeeds:connect", &[&c]);
    let html = page(&mut app, &path);
    assert_eq!(value(&html, "label"), bf::DEFAULT_LABEL);
    assert!(input(&html, "label").contains("data-select-on-focus"), "{}", input(&html, "label"));
    let html = invalid(submit_page(&mut app, &path, pairs(&[("label", "Fixture bank (TEST)"), ("setup_token", "")])));
    assert_eq!(value(&html, "label"), "Fixture bank (TEST)");
    assert!(!input(&html, "label").contains("data-select-on-focus"), "{}", input(&html, "label"));
}

/// A posted invoice for `customer` of `amount` dated `date`.
fn posted_invoice(app: &mut App, c: &str, customer: &str, date: &str, amount: &str) -> String {
    let income = account(app, c, "4000");
    let id = run(app, json!({"type":"SaveInvoice","company_id":c,"values":{"request_key":key(),"customer_id":customer,"date":date,"due_date":date,"memo":"","tax_mode":"NONE",
        "lines":[{"description":"Bakery shelving","quantity":"1","rate":amount,"taxable":false,"income_account_id":income}]}}))["id"].as_str().unwrap().to_owned();
    post_invoice(app, c, &id).unwrap();
    id
}
fn receipts(app: &mut App) -> i64 {
    count(app, "SELECT count(*) FROM sales_receipt")
}

/// Item 39: 453.00 typed for the 435.00 invoice the person chose was saved with 18.00 on another invoice,
/// "applied in full", with no word. The page marks each row it filled (`allocations-N-chosen`: 1 chosen,
/// 0 filled by the page); money on an invoice nobody chose is saved only after a second Save.
#[test]
fn money_the_page_put_on_an_invoice_nobody_chose_is_saved_only_after_a_second_save() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let bluestem = customer(&mut app, &c, "Bluestem Bakery (TEST)");
    let older = posted_invoice(&mut app, &c, &bluestem, "2026-07-14", "435.00");
    let newer = posted_invoice(&mut app, &c, &bluestem, "2026-08-24", "190.00");
    let path = url("receipts:new", &[&c]);
    let html = page(&mut app, &format!("{path}?invoice={older}"));
    // Opened from an invoice: that invoice is the one chosen.
    let row = html.split_once(&format!("data-invoice=\"{older}\"")).unwrap().1.split('>').next().unwrap();
    assert!(row.contains("data-chosen=\"1\""), "{row}");
    assert!(html.contains(r#"<p class="notice amount-extra" id="amount-extra" role="status" hidden></p>"#), "{html}");
    let rk = value(&html, "request_key");
    let fields = |rk: &str, amount: &str, rows: &[(&str, &str, &str)], confirm: Option<&str>| {
        let mut p = pairs(&[("request_key", rk), ("customer", &bluestem), ("date", "2026-08-29"), ("amount", amount), ("method", "CHECK"), ("reference", "4412"), ("destination", "UNDEPOSITED"), ("action", "save")]);
        p.push(("allocations-TOTAL_FORMS".into(), rows.len().to_string()));
        for (i, (invoice, apply, chosen)) in rows.iter().enumerate() {
            p.push((format!("allocations-{i}-invoice"), invoice.to_string()));
            if !apply.is_empty() {
                p.push((format!("allocations-{i}-pay"), "on".into()));
            }
            p.push((format!("allocations-{i}-amount"), apply.to_string()));
            p.push((format!("allocations-{i}-chosen"), chosen.to_string()));
        }
        if let Some(confirm) = confirm {
            p.push(("extra_ok".into(), confirm.into()));
        }
        p
    };
    // 453.00 typed: the page filled 435.00 on the chosen invoice and 18.00 on the other one.
    let typo = [(older.as_str(), "435.00", "1"), (newer.as_str(), "18.00", "0")];
    let html = invalid(submit_page(&mut app, &path, fields(&rk, "453.00", &typo, None)));
    assert_eq!(receipts(&mut app), 0, "nothing saved");
    let note = html.split_once(r#"id="amount-extra""#).unwrap().1.split("</p>").next().unwrap();
    assert!(note.contains("<strong>Nothing was saved yet.</strong> 18.00 of the 453.00 received would go to INV-000002, which you did not choose (you chose INV-000001). To apply it there, choose Save payment again. Otherwise correct the amount received, or untick INV-000002."), "{note}");
    assert!(!html.contains("Payment not saved."), "a warning, not an error");
    assert_eq!(value(&html, "amount"), "453.00", "what was typed is kept");
    let confirm = value(&html, "extra_ok");
    assert!(!confirm.is_empty());
    let row = html.split_once(&format!("data-invoice=\"{newer}\"")).unwrap().1.split('>').next().unwrap();
    assert!(row.contains("data-chosen=\"0\""), "the marks come back: {row}");
    // Saved again with something changed (the amount): asked again, still nothing saved.
    let changed = [(older.as_str(), "435.00", "1"), (newer.as_str(), "20.00", "0")];
    let html = invalid(submit_page(&mut app, &path, fields(&rk, "455.00", &changed, Some(&confirm))));
    assert!(html.contains("20.00 of the 455.00 received would go to INV-000002"), "{html}");
    assert_eq!(receipts(&mut app), 0);
    // Corrected to 435.00 on the chosen invoice only: saved at once.
    let Outcome::Redirect { .. } = submit_page(&mut app, &path, fields(&rk, "435.00", &[(older.as_str(), "435.00", "1"), (newer.as_str(), "", "0")], None)) else { panic!("saved") };
    assert_eq!(receipts(&mut app), 1);
    // The same typo confirmed with the second Save is saved as shown (another form: a new request key).
    let other = posted_invoice(&mut app, &c, &bluestem, "2026-08-25", "435.00");
    let html = page(&mut app, &format!("{path}?invoice={other}"));
    let rk = value(&html, "request_key");
    let typo = [(newer.as_str(), "18.00", "0"), (other.as_str(), "435.00", "1")];
    let html = invalid(submit_page(&mut app, &path, fields(&rk, "453.00", &typo, None)));
    let confirm = value(&html, "extra_ok");
    let Outcome::Redirect { .. } = submit_page(&mut app, &path, fields(&rk, "453.00", &typo, Some(&confirm))) else { panic!("confirmed") };
    assert_eq!(receipts(&mut app), 2);
    // Without the page's script nothing is filled for the person and no row is marked: saved as sent.
    let html = page(&mut app, &format!("{path}?customer={bluestem}"));
    let rk = value(&html, "request_key");
    let mut p = pairs(&[("request_key", &rk), ("customer", &bluestem), ("date", "2026-08-29"), ("amount", "100.00"), ("method", "CHECK"), ("reference", "4413"), ("destination", "UNDEPOSITED"), ("action", "save"), ("allocations-TOTAL_FORMS", "1"), ("allocations-0-invoice", &newer), ("allocations-0-pay", "on"), ("allocations-0-amount", "100.00")]);
    p.retain(|(k, _)| !k.ends_with("-chosen"));
    let Outcome::Redirect { .. } = submit_page(&mut app, &path, p) else { panic!("no marks, no question") };
    assert_eq!(receipts(&mut app), 3);
}

// --- bank feeds: items 40, 42 and 43 ------------------------------------------------------------------

struct Feeds {
    app: App,
    c: String,
    path: String,
    card: String,
    checking: String,
    conn: String,
}
fn staged(id: &str, date: &str, amount: i64, description: &str) -> bf::StagedTransaction {
    bf::StagedTransaction { remote_id: id.into(), posted_on: Some(date.into()), transacted_on: None, amount, description: description.into(), pending: false }
}
fn feed_account(remote: &str, transactions: Vec<bf::StagedTransaction>) -> bf::StagedAccount {
    bf::StagedAccount { conn_key: "synthetic".into(), remote_id: remote.into(), name: remote.into(), org_name: "Synthetic Test Bank".into(), currency: "USD".into(), balance: None, available: None, balance_epoch: None, skipped: vec![], transactions }
}
impl Feeds {
    /// One connection, two downloaded accounts: checking (mapped to 1000) and a card (a new card account).
    /// The card payment appears on both: money out of checking, money in on the card.
    fn new() -> Self {
        let (mut app, c) = fixture(StarterChart::Standard);
        app.simplefin = true;
        let co = fb_core::CompanyId::parse(&c).unwrap();
        let checking = account(&mut app, &c, "1000");
        let o = app.open_mut().unwrap();
        let conn = bf::connect(&mut o.book, o.owner, co, &json!("Synthetic downloads"), "bridge.simplefin.org", &bf::SecretSet::none()).unwrap()["id"].as_str().unwrap().to_owned();
        let pull = bf::StagedPull {
            accounts: vec![
                feed_account("Checking", vec![staged("TX-0026", "2026-07-06", -118_284, "ONLINE PAYMENT TO CARD"), staged("TX-0027", "2026-07-07", -2_000, "COFFEE ROASTERS")]),
                feed_account("Card", vec![staged("TX-0028", "2026-07-06", 118_284, "PAYMENT THANK YOU"), staged("TX-0029", "2026-07-08", -4_900, "SOFTWARE SUBSCRIPTION")]),
            ],
            ..Default::default()
        };
        bf::store_pull(&mut o.book, o.owner, co, &conn, &pull, "2026-11-07T12:00:00Z", false, None, &bf::SecretSet::none()).unwrap();
        let feed = |o: &mut fb_app::session::Open, name: &str| -> String { o.book.read(|q| q.query_row("SELECT id FROM bankfeeds_account WHERE name=?1", [name], |r| r.get(0))).unwrap() };
        let (checking_feed, card_feed) = (feed(o, "Checking"), feed(o, "Card"));
        bf::save_mappings(&mut o.book, o.owner, co, &conn, &[
            (checking_feed, bf::MappingTarget::Existing(fb_core::AccountId::parse(&checking))),
            (card_feed.clone(), bf::MappingTarget::New { name: "Business card".into(), kind: bf::NewAccountKind::CreditCard }),
        ], &bf::SecretSet::none()).unwrap();
        let card = o.book.read(|q| bf::feed(q, &card_feed)).unwrap().unwrap().ledger_account_id.unwrap();
        let path = url("bankfeeds:review", &[&c]);
        Feeds { app, c, path, card, checking, conn }
    }
    fn pk(&mut self, remote: &str) -> i64 {
        let conn = self.conn.clone();
        self.app.open_mut().unwrap().book.read(|q| q.query_row("SELECT t.ordinal FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id WHERE f.connection_id=?1 AND t.remote_id=?2", [&conn, remote], |r| r.get(0))).unwrap()
    }
    fn page(&mut self) -> String {
        let path = self.path.clone();
        page(&mut self.app, &path)
    }
    /// What the page's form posts for its rows as drawn (every NEW row's fields), with `set` changing a
    /// row's drafts, plus the button and the script's own fields.
    fn post(&mut self, html: &str, set: &[(String, String)], extra: &[(&str, String)]) -> Value {
        let mut fields: Vec<(String, String)> = vec![("inline".into(), "1".into()), ("review_layout".into(), "new".into())];
        for name in ["kind_", "account_", "payee_", "number_", "line_", "receipt_", "to_", "link_to_"] {
            let mut rest = html;
            while let Some(at) = rest.find(&format!("name=\"{name}")) {
                let tag_start = rest[..at].rfind('<').unwrap();
                let tag = &rest[tag_start..];
                let field = tag.split_once("name=\"").unwrap().1.split('"').next().unwrap().to_owned();
                let v = if tag.starts_with("<select") {
                    let body = tag.split("</select>").next().unwrap();
                    body.split("<option").find(|o| o.split('>').next().unwrap().contains(" selected")).or_else(|| body.split("<option").nth(1)).and_then(|o| o.split_once("value=\"")).map(|(_, v)| v.split('"').next().unwrap().to_owned()).unwrap_or_default()
                } else {
                    tag.split('>').next().unwrap().split_once("value=\"").map(|(_, v)| v.split('"').next().unwrap().to_owned()).unwrap_or_default()
                };
                if !fields.iter().any(|(k, _)| *k == field) {
                    fields.push((field, v));
                }
                rest = &rest[at + name.len() + 6..];
            }
        }
        for (k, v) in set {
            fields.retain(|(name, _)| name != k);
            fields.push((k.clone(), v.clone()));
        }
        fields.extend(extra.iter().map(|(k, v)| (k.to_string(), v.clone())));
        let path = self.path.clone();
        let token = self.app.tokens.mint(&path);
        match self.app.run_renderer(Command::SubmitForm { path, token, fields }).unwrap() {
            Outcome::Done { data } => data,
            o => panic!("expected an in-place reply: {o:?}"),
        }
    }
    fn journals(&mut self) -> i64 {
        count(&mut self.app, "SELECT count(*) FROM ledger_journal WHERE status='POSTED'")
    }
    /// A journal entry made by hand and posted: `amount` debited to `debit`, credited to `credit`. Its id
    /// and its number (J000003).
    fn hand_journal(&mut self, date: &str, memo: &str, debit: &str, credit: &str, amount: &str) -> (String, String) {
        let co = fb_core::CompanyId::parse(&self.c).unwrap();
        let o = self.app.open_mut().unwrap();
        let draft = fb_core::services::DraftInput::new(date, memo, vec![json!({"account":debit,"debit":amount,"credit":"0.00"}), json!({"account":credit,"debit":"0.00","credit":amount})]);
        let j = fb_core::services::save_draft(&mut o.book, o.owner, co, draft).unwrap();
        let j = fb_core::services::post_journal(&mut o.book, o.owner, co, j.id, Some(j.revision)).unwrap();
        (j.id.to_db(), format!("{} · {}", j.entry_number, j.display()))
    }
    /// The line of `journal` on `account`.
    fn line_on(&mut self, journal: &str, account: &str) -> String {
        self.app.open_mut().unwrap().book.read(|q| q.query_row("SELECT id FROM ledger_journalline WHERE journal_id=?1 AND account_id=?2", [journal, account], |r| r.get(0))).unwrap()
    }
    fn matched_line(&mut self, pk: i64) -> Option<String> {
        self.app.open_mut().unwrap().book.read(|q| q.query_row("SELECT journal_line_id FROM bankfeeds_transaction WHERE ordinal=?1", [pk], |r| r.get(0))).unwrap()
    }
}
fn fragment<'a>(data: &'a Value, id: &str) -> Option<&'a str> {
    data["fragments"].as_array().unwrap().iter().find(|f| f["id"] == id).map(|f| f["html"].as_str().unwrap())
}
fn row_of(html: &str, pk: i64) -> &str {
    html.split_once(&format!("id=\"review-row-{pk}\"")).unwrap_or_else(|| panic!("no row {pk}: {html}")).1.split_once("</tr>").unwrap().0
}
fn set(pk: i64, pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| (format!("{k}_{pk}"), v.to_string())).collect()
}

/// Items 40 and 42: after an in-place Accept every row the page shows is drawn again, so the card side of a
/// card payment offers the entry the checking side just recorded; a typed draft on another row is kept and
/// only that row stays a draft.
#[test]
fn an_in_place_accept_redraws_the_shown_rows_and_keeps_only_typed_drafts() {
    let mut f = Feeds::new();
    let (pay, coffee, card_pay, software) = (f.pk("TX-0026"), f.pk("TX-0027"), f.pk("TX-0028"), f.pk("TX-0029"));
    let page = f.page();
    assert!(!row_of(&page, card_pay).contains("Match existing entry"), "nothing to match yet");
    let shown = [pay, coffee, card_pay, software].iter().map(|n| n.to_string()).collect::<Vec<_>>().join(",");
    let office = account(&mut f.app, &f.c, "6100");
    // The checking side recorded as a check to the card account; the person typed a payee on the software row.
    let mut drafts = set(pay, &[("kind", "CHECK"), ("account", &f.card.clone())]);
    drafts.extend(set(software, &[("kind", "JOURNAL"), ("account", &office), ("payee", "Synthetic Software Co")]));
    let data = f.post(&page, &drafts, &[("accept_one", pay.to_string()), ("shown", shown.clone()), ("edited", software.to_string())]);
    assert!(fragment(&data, &format!("review-row-{pay}")).unwrap().contains("data-status=\"ACCEPTED\""), "{data}");
    // The card side, untouched by the person, is drawn as a new page draws it: Match, already chosen.
    let card_row = fragment(&data, &format!("review-row-{card_pay}")).expect("the card side is drawn again");
    assert!(card_row.contains(r#"<option value="MATCH" selected>Match existing entry</option>"#), "{card_row}");
    assert!(card_row.contains("data-draft-retained=\"false\""), "{card_row}");
    // The row with a typed draft keeps it and is still a draft; the untouched coffee row is not.
    let soft = fragment(&data, &format!("review-row-{software}")).unwrap();
    assert!(soft.contains("data-draft-retained=\"true\"") && soft.contains("value=\"Synthetic Software Co\""), "{soft}");
    assert!(fragment(&data, &format!("review-row-{coffee}")).unwrap().contains("data-draft-retained=\"false\""));
    // Without the page's lists (a whole-page post, or an old page) only the acted-on row comes back.
    let page = f.page();
    let data = f.post(&page, &set(coffee, &[("kind", "CHECK"), ("account", &office)]), &[("accept_one", coffee.to_string())]);
    assert!(fragment(&data, &format!("review-row-{coffee}")).is_some() && fragment(&data, &format!("review-row-{software}")).is_none(), "{data}");
}

/// Item 40: recording the card side again as a new entry while the checking side's check is already on the
/// card account is refused with that entry named, and offers Match; Record again… goes ahead after it.
#[test]
fn a_new_entry_for_money_already_on_the_account_warns_and_offers_match() {
    let mut f = Feeds::new();
    let (pay, card_pay) = (f.pk("TX-0026"), f.pk("TX-0028"));
    let page = f.page();
    let card = f.card.clone();
    f.post(&page, &set(pay, &[("kind", "CHECK"), ("account", &card)]), &[("accept_one", pay.to_string())]);
    let check: String = f.app.open_mut().unwrap().book.read(|q| q.query_row("SELECT reference FROM bankfeeds_transaction WHERE ordinal=?1", [pay], |r| r.get(0))).unwrap();
    // The card side as "Payment (journal)" to checking, as the worker chose it.
    let page = f.page();
    let row = row_of(&page, card_pay);
    assert!(row.contains("Already recorded?") && row.contains(" hidden><strong>"), "a note for other kinds, hidden while Match is chosen: {row}");
    let before = f.journals();
    let checking = f.checking.clone();
    let as_journal = set(card_pay, &[("kind", "JOURNAL"), ("account", &checking)]);
    let data = f.post(&page, &as_journal, &[("accept_one", card_pay.to_string()), ("edited", card_pay.to_string())]);
    assert_eq!(f.journals(), before, "nothing recorded");
    let row = fragment(&data, &format!("review-row-{card_pay}")).unwrap();
    assert!(row.contains("data-status=\"NEW\""), "{row}");
    assert!(row.contains(&format!("This transaction looks already recorded as {check}")) && row.contains("Match it to that entry, or confirm that you want to record it again."), "{row}");
    assert!(row.contains("<strong>Already recorded?</strong>") && row.contains("is already on this account with the same amount. Match it, or choose Record again… to record a new entry as well."), "{row}");
    assert!(row.contains("name=\"match_one\"") && row.contains("name=\"accept_again_one\"") && !row.contains("name=\"accept_one\""), "{row}");
    // Record again… (after the page's confirmation) records it, audited as recorded again.
    let mut again = as_journal.clone();
    let line = row.split_once(&format!("name=\"link_to_{card_pay}\" value=\"")).unwrap().1.split('"').next().unwrap().to_owned();
    again.push((format!("link_to_{card_pay}"), line.clone()));
    let page = f.page();
    let data = f.post(&page, &again, &[("accept_again_one", card_pay.to_string())]);
    assert!(fragment(&data, &format!("review-row-{card_pay}")).unwrap().contains("data-status=\"ACCEPTED\""), "{data}");
    assert_eq!(f.journals(), before + 1);
    assert_eq!(count(&mut f.app, "SELECT count(*) FROM ledger_auditevent WHERE action='bankfeed.recorded_again'"), 1);
}

/// Item 40: "Match" on the warning matches the entry it named; nothing new is posted.
#[test]
fn match_on_the_warning_matches_the_entry_it_named() {
    let mut f = Feeds::new();
    let (pay, card_pay) = (f.pk("TX-0026"), f.pk("TX-0028"));
    let page = f.page();
    let card = f.card.clone();
    f.post(&page, &set(pay, &[("kind", "CHECK"), ("account", &card)]), &[("accept_one", pay.to_string())]);
    let page = f.page();
    let checking = f.checking.clone();
    let as_journal = set(card_pay, &[("kind", "JOURNAL"), ("account", &checking)]);
    let data = f.post(&page, &as_journal, &[("accept_one", card_pay.to_string())]);
    let row = fragment(&data, &format!("review-row-{card_pay}")).unwrap().to_owned();
    let line = row.split_once(&format!("name=\"link_to_{card_pay}\" value=\"")).unwrap().1.split('"').next().unwrap().to_owned();
    let before = f.journals();
    let mut matched = as_journal.clone();
    matched.push((format!("link_to_{card_pay}"), line));
    let page = f.page();
    let data = f.post(&page, &matched, &[("match_one", card_pay.to_string())]);
    assert!(fragment(&data, &format!("review-row-{card_pay}")).unwrap().contains("data-status=\"MATCHED\""), "{data}");
    assert_eq!(f.journals(), before, "a match posts nothing");
    assert!(fragment(&data, "review-status").unwrap().contains(": matched to "), "{data}");
}

/// Item 40, the core: a new entry is refused while an entry Match could take is on the account, unless the
/// person confirmed it, and a confirmation for another entry does not count.
#[test]
fn the_core_refuses_a_new_entry_for_money_already_on_the_account_without_confirmation() {
    let mut f = Feeds::new();
    let co = fb_core::CompanyId::parse(&f.c).unwrap();
    let (card, checking) = (f.card.clone(), f.checking.clone());
    let o = f.app.open_mut().unwrap();
    let id = |o: &mut fb_app::session::Open, remote: &str| -> String { o.book.read(|q| q.query_row("SELECT id FROM bankfeeds_transaction WHERE remote_id=?1", [remote], |r| r.get(0))).unwrap() };
    let (pay, card_pay) = (id(o, "TX-0026"), id(o, "TX-0028"));
    bf::accept(&mut o.book, o.owner, co, &pay, &json!({"kind":"CHECK","account_id":card}), &bf::SecretSet::none()).unwrap();
    let journal = json!({"kind":"JOURNAL","account_id":checking});
    let refused = bf::accept(&mut o.book, o.owner, co, &card_pay, &journal, &bf::SecretSet::none()).unwrap_err().to_string();
    assert!(refused.contains("looks already recorded as") && refused.contains("5 days or less apart"), "{refused}");
    let mut other = journal.clone();
    other["record_again"] = json!(true);
    other["recorded_as"] = json!("00000000-0000-0000-0000-000000000000");
    assert!(bf::accept(&mut o.book, o.owner, co, &card_pay, &other, &bf::SecretSet::none()).unwrap_err().to_string().contains("The books changed since this page was shown"));
    let item = o.book.read(|q| bf::item(q, co, &card_pay)).unwrap();
    let entry = o.book.read(|q| bf::entry_on_account(q, co, &item)).unwrap().expect("the check's card line");
    let mut confirmed = journal.clone();
    confirmed["record_again"] = json!(true);
    confirmed["recorded_as"] = json!(entry.line_id);
    assert_eq!(bf::accept(&mut o.book, o.owner, co, &card_pay, &confirmed, &bf::SecretSet::none()).unwrap().status, "ACCEPTED");
}

/// Item 43: the status area of every reply carries the count the left menu shows, and the menu's Review
/// link has the id the page script updates.
#[test]
fn the_in_place_reply_carries_the_menu_count() {
    let mut f = Feeds::new();
    let pay = f.pk("TX-0026");
    let page = f.page();
    assert!(page.contains("id=\"nav-review\""), "the menu link");
    assert!(page.contains("data-to-review=\"4\""), "{}", page.split_once("id=\"review-status\"").unwrap().1.split('>').next().unwrap());
    assert!(page.split_once("id=\"nav-review\"").unwrap().1.split("</a>").next().unwrap().contains("<span class=\"nav-badge\">4</span>"));
    let card = f.card.clone();
    let data = f.post(&page, &set(pay, &[("kind", "CHECK"), ("account", &card)]), &[("accept_one", pay.to_string())]);
    assert!(fragment(&data, "review-status").unwrap().contains("data-to-review=\"3\""), "{data}");
}

/// Item 45: the journal entry's Memo is marked required, the form uses the app's own messages, and an
/// empty Memo comes back with the message at the box, marked to be brought into view and focused.
#[test]
fn the_journal_memo_is_marked_required_and_its_message_is_the_apps_own() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let path = url("journal_new", &[&c]);
    let html = page(&mut app, &path);
    assert!(html.contains(r#"<form method="post" class="journal-entry" data-line-max="50" novalidate>"#), "{html}");
    let memo = html.split_once(r#"<div class="field-memo">"#).unwrap().1.split("</div>").next().unwrap();
    assert!(memo.contains(r#"<span class="helptext" id="id_memo_helptext">Required.</span>"#) && memo.contains(r#"aria-describedby="id_memo_helptext""#), "{memo}");
    let (bank, equity) = (account(&mut app, &c, "1000"), account(&mut app, &c, "3000"));
    let fields = pairs(&[("request_key", &value(&html, "request_key")), ("expected_revision", "0"), ("date", "2026-01-01"), ("entry_number", "1"), ("memo", ""),
        ("lines-TOTAL_FORMS", "2"), ("lines-INITIAL_FORMS", "0"), ("lines-0-account", &bank), ("lines-0-debit", "5000.00"), ("lines-1-account", &equity), ("lines-1-credit", "5000.00")]);
    let html = invalid(submit_page(&mut app, &path, fields));
    let memo = html.split_once(r#"<div class="field-memo" data-reveal>"#).expect("the Memo box is revealed").1.split("</div>").next().unwrap();
    assert!(memo.contains("This field is required.") && memo.contains(r#"aria-invalid="true""#), "{memo}");
    assert_eq!(count(&mut app, "SELECT count(*) FROM ledger_journal"), 0, "nothing saved");
}

// --- review fixes of round 3, part A (S1, M1, M2, M3, M5) ------------------------------------------------

/// Review S1: "Match" on a row that warned of an entry already on its account, with "Match existing entry"
/// chosen on the row, matches the entry the person chose in its "Existing entry" list, not the closest-dated
/// one the warning named. The list names each entry by number.
#[test]
fn match_on_a_warning_row_uses_the_entry_chosen_in_its_list() {
    let mut f = Feeds::new();
    let card_pay = f.pk("TX-0028");
    let (card, checking) = (f.card.clone(), f.checking.clone());
    // Two card payments of the same amount entered by hand, 2 days before and 1 day after the bank's.
    let (early, early_no) = f.hand_journal("2026-07-04", "Card payment by hand", &card, &checking, "1182.84");
    let (late, late_no) = f.hand_journal("2026-07-07", "Card payment again", &card, &checking, "1182.84");
    let (early_line, late_line) = (f.line_on(&early, &card), f.line_on(&late, &card));
    // Recorded as a journal entry: refused, and the warning names the closer one, 07-07.
    let page = f.page();
    let as_journal = set(card_pay, &[("kind", "JOURNAL"), ("account", &checking)]);
    let data = f.post(&page, &as_journal, &[("accept_one", card_pay.to_string()), ("edited", card_pay.to_string())]);
    let row = fragment(&data, &format!("review-row-{card_pay}")).unwrap().to_owned();
    assert!(row.contains(&format!("<strong>Already recorded?</strong> {late_no} (2026-07-07, “Card payment again”) is already on this account")), "{row}");
    assert!(row.contains(&format!("name=\"link_to_{card_pay}\" value=\"{late_line}\"")), "{row}");
    assert!(row.contains("name=\"match_one\""), "{row}");
    // Both entries are in the row's Existing entry list, by number and date.
    assert!(row.contains(&format!("<option value=\"{early_line}\">{early_no} · 2026-07-04 · Card payment by hand</option>")), "{row}");
    assert!(row.contains(&format!("{late_no} · 2026-07-07 · Card payment again</option>")), "{row}");
    // The person chooses Match existing entry, picks the 07-04 entry, and presses the row's Match (or Enter).
    let before = f.journals();
    let mut chosen = set(card_pay, &[("kind", "MATCH"), ("line", &early_line), ("account", &checking)]);
    chosen.push((format!("link_to_{card_pay}"), late_line.clone()));
    let page = f.page();
    let data = f.post(&page, &chosen, &[("match_one", card_pay.to_string())]);
    assert!(fragment(&data, &format!("review-row-{card_pay}")).unwrap().contains("data-status=\"MATCHED\""), "{data}");
    assert_eq!(f.matched_line(card_pay).as_deref(), Some(early_line.as_str()), "the entry the person chose, not the closest");
    assert_eq!(f.journals(), before, "a match posts nothing");
    // (The decided row keeps the reference Review has always stored for a journal entry, "JE <entry no.>".)
    assert!(fragment(&data, "review-status").unwrap().contains(&format!(": matched to {early_no} ")), "{data}");
    // With the row still set to a new entry, Match is the entry the warning named.
    let mut g = Feeds::new();
    let card_pay = g.pk("TX-0028");
    let (card, checking) = (g.card.clone(), g.checking.clone());
    g.hand_journal("2026-07-04", "Card payment by hand", &card, &checking, "1182.84");
    let (late, _) = g.hand_journal("2026-07-07", "Card payment again", &card, &checking, "1182.84");
    let late_line = g.line_on(&late, &card);
    let mut warned = set(card_pay, &[("kind", "JOURNAL"), ("account", &checking)]);
    warned.push((format!("link_to_{card_pay}"), late_line.clone()));
    let page = g.page();
    g.post(&page, &warned, &[("match_one", card_pay.to_string())]);
    assert_eq!(g.matched_line(card_pay).as_deref(), Some(late_line.as_str()));
}

/// Review M2: the warning, the core's refusal and the `bankfeed.recorded_again` audit name the entry by its
/// number and date, with the memo only after them: a journal entry by its J-number, a check by its CHK number.
#[test]
fn the_warning_and_the_audit_name_the_entry_by_its_number() {
    let mut f = Feeds::new();
    let card_pay = f.pk("TX-0028");
    let (card, checking) = (f.card.clone(), f.checking.clone());
    let (journal, number) = f.hand_journal("2026-07-07", "Card payment by hand", &card, &checking, "1182.84");
    let line = f.line_on(&journal, &card);
    let page = f.page();
    let row = row_of(&page, card_pay);
    assert!(row.contains(&format!("<strong>Already recorded?</strong> {number} (2026-07-07, “Card payment by hand”) is already on this account with the same amount. Accept asks you")), "the note shown when another kind is chosen: {row}");
    assert!(row.contains(&format!("Looks like {number} · Card payment by hand (2026-07-07)")), "{row}");
    let as_journal = set(card_pay, &[("kind", "JOURNAL"), ("account", &checking)]);
    let data = f.post(&page, &as_journal, &[("accept_one", card_pay.to_string()), ("edited", card_pay.to_string())]);
    let row = fragment(&data, &format!("review-row-{card_pay}")).unwrap().to_owned();
    assert!(row.contains(&format!("This transaction looks already recorded as {number} (2026-07-07, “Card payment by hand”): an entry on this account with the same amount, 5 days or less apart.")), "{row}");
    assert!(row.contains(&format!("<strong>Already recorded?</strong> {number} (2026-07-07, “Card payment by hand”) is already on this account with the same amount. Match it,")), "{row}");
    assert!(row.contains(&format!("data-confirm=\"Record this transaction as a new entry? {number} (2026-07-07) is already on this account with the same amount.\"")), "{row}");
    // Record again…: the audit names the earlier entry by its number, its date and memo beside it.
    let mut again = as_journal.clone();
    again.push((format!("link_to_{card_pay}"), line.clone()));
    let page = f.page();
    let data = f.post(&page, &again, &[("accept_again_one", card_pay.to_string())]);
    assert!(fragment(&data, &format!("review-row-{card_pay}")).unwrap().contains("data-status=\"ACCEPTED\""), "{data}");
    let detail: String = f.app.open_mut().unwrap().book.read(|q| q.query_row("SELECT detail FROM ledger_auditevent WHERE action='bankfeed.recorded_again'", [], |r| r.get(0))).unwrap();
    let detail: Value = serde_json::from_str(&detail).unwrap();
    assert_eq!((detail["earlier_reference"].as_str(), detail["earlier_date"].as_str(), detail["earlier_memo"].as_str()), (Some("J000001"), Some("2026-07-07"), Some("Card payment by hand")), "{detail}");
    assert_eq!((detail["earlier_journal_id"].as_str(), detail["earlier_line_id"].as_str()), (Some(journal.as_str()), Some(line.as_str())));
    // A check is named by its number first, then its date, its memo without the number again.
    let mut f = Feeds::new();
    let (pay, card_pay) = (f.pk("TX-0026"), f.pk("TX-0028"));
    let page = f.page();
    let card = f.card.clone();
    f.post(&page, &set(pay, &[("kind", "CHECK"), ("account", &card)]), &[("accept_one", pay.to_string())]);
    let check: String = f.app.open_mut().unwrap().book.read(|q| q.query_row("SELECT reference FROM bankfeeds_transaction WHERE ordinal=?1", [pay], |r| r.get(0))).unwrap();
    assert!(check.starts_with("CHK-"), "{check}");
    let page = f.page();
    let checking = f.checking.clone();
    let data = f.post(&page, &set(card_pay, &[("kind", "JOURNAL"), ("account", &checking)]), &[("accept_one", card_pay.to_string()), ("edited", card_pay.to_string())]);
    let row = fragment(&data, &format!("review-row-{card_pay}")).unwrap();
    assert!(row.contains(&format!("looks already recorded as {check} (2026-07-06")), "{row}");
    assert!(!row.contains(&format!("“{check}")), "the memo does not repeat the number: {row}");
}

/// Review M1 and M3: a bulk Accept says what is true of each row near an entry already on its account.
#[test]
fn bulk_accept_on_rows_with_an_entry_on_their_account_says_what_is_true() {
    let mut f = Feeds::new();
    let (pay, card_pay) = (f.pk("TX-0026"), f.pk("TX-0028"));
    let (card, checking) = (f.card.clone(), f.checking.clone());
    // One card payment entered by hand: on the card account for the card row, on checking for the checking row.
    let (journal, number) = f.hand_journal("2026-07-07", "Card payment by hand", &card, &checking, "1182.84");
    let (card_line, checking_line) = (f.line_on(&journal, &card), f.line_on(&journal, &checking));
    let before = f.journals();
    // M1, case 1: the card row was shown in the warning state (its link_to is the entry); the person chose
    // Match existing entry on it, ticked it and used bulk Accept. It is matched.
    let page = f.page();
    let mut fields = set(card_pay, &[("kind", "MATCH"), ("line", &card_line)]);
    fields.push((format!("link_to_{card_pay}"), card_line.clone()));
    let data = f.post(&page, &fields, &[("sel", card_pay.to_string()), ("action", "accept".into())]);
    let status = fragment(&data, "review-status").unwrap();
    assert!(fragment(&data, &format!("review-row-{card_pay}")).unwrap().contains("data-status=\"MATCHED\""), "{data}");
    assert!(!status.contains("changed since this page was shown") && !status.contains("no longer looks already recorded"), "{status}");
    assert_eq!(f.matched_line(card_pay).as_deref(), Some(card_line.as_str()));
    // M1, case 2: the checking row was drawn with Match chosen (no warning, so no link_to); the person changed
    // it to a check and used bulk Accept. Nothing is recorded, and the row says to decide on it.
    let page = f.page();
    let data = f.post(&page, &set(pay, &[("kind", "CHECK"), ("account", &card)]), &[("sel", pay.to_string()), ("action", "accept".into()), ("edited", pay.to_string())]);
    let status = fragment(&data, "review-status").unwrap();
    let row = fragment(&data, &format!("review-row-{pay}")).unwrap();
    assert!(row.contains(&format!("{number} (2026-07-07, “Card payment by hand”) is already on this account with the same amount. Choose Match or Record again… on this row. Nothing was recorded.")), "{row}");
    assert!(row.contains("name=\"match_one\"") && row.contains("name=\"accept_again_one\""), "{row}");
    assert!(status.contains("1 selected row has an entry already on its account with the same amount, so nothing was recorded for it. Choose Match or Record again… on that row."), "{status}");
    assert!(!status.contains("changed since this page was shown") && !status.contains("now looks already recorded"), "{status}");
    assert_eq!(f.journals(), before, "nothing posted");
    // M3: the same row as the page now shows it (the warning, with its link_to), bulk-accepted without "Also
    // record…": skipped, and told to Match, which is what the row offers.
    let mut warned = set(pay, &[("kind", "CHECK"), ("account", &card)]);
    warned.push((format!("link_to_{pay}"), checking_line.clone()));
    let page = f.page();
    let data = f.post(&page, &warned, &[("sel", pay.to_string()), ("action", "accept".into()), ("edited", pay.to_string())]);
    let status = fragment(&data, "review-status").unwrap();
    assert!(status.contains("Skipped 1 selected row that looks already recorded. Match it to the existing entry, or tick “Also record rows that look already recorded”."), "{status}");
    assert!(!status.contains("Link it"), "{status}");
    assert_eq!(f.journals(), before);
    // Ticked "Also record…": recorded again, as that entry.
    let page = f.page();
    let data = f.post(&page, &warned, &[("sel", pay.to_string()), ("action", "accept".into()), ("include_recorded", "on".into())]);
    assert!(fragment(&data, &format!("review-row-{pay}")).unwrap().contains("data-status=\"ACCEPTED\""), "{data}");
    assert_eq!(f.journals(), before + 1);
}

/// Review M5: after the warning, unticking the invoice the page filled keeps the amount received as typed
/// (the page script, tested in worker-fixes-3.test.cjs); the save is then refused until the amounts applied
/// add up to it, as before, and nothing is saved.
#[test]
fn unticking_after_the_warning_keeps_the_amount_and_the_save_is_refused_until_it_is_applied() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let bluestem = customer(&mut app, &c, "Bluestem Bakery (TEST)");
    let older = posted_invoice(&mut app, &c, &bluestem, "2026-07-14", "435.00");
    let newer = posted_invoice(&mut app, &c, &bluestem, "2026-08-24", "190.00");
    let path = url("receipts:new", &[&c]);
    let html = page(&mut app, &format!("{path}?invoice={older}"));
    let rk = value(&html, "request_key");
    let fields = |amount: &str, rows: &[(&str, &str, &str)], confirm: Option<&str>| {
        let mut p = pairs(&[("request_key", &rk), ("customer", &bluestem), ("date", "2026-08-29"), ("amount", amount), ("method", "CHECK"), ("reference", "4412"), ("destination", "UNDEPOSITED"), ("action", "save")]);
        p.push(("allocations-TOTAL_FORMS".into(), rows.len().to_string()));
        for (i, (invoice, apply, chosen)) in rows.iter().enumerate() {
            p.push((format!("allocations-{i}-invoice"), invoice.to_string()));
            if !apply.is_empty() {
                p.push((format!("allocations-{i}-pay"), "on".into()));
            }
            p.push((format!("allocations-{i}-amount"), apply.to_string()));
            p.push((format!("allocations-{i}-chosen"), chosen.to_string()));
        }
        if let Some(confirm) = confirm {
            p.push(("extra_ok".into(), confirm.into()));
        }
        p
    };
    let html = invalid(submit_page(&mut app, &path, fields("453.00", &[(older.as_str(), "435.00", "1"), (newer.as_str(), "18.00", "0")], None)));
    let confirm = value(&html, "extra_ok");
    // Following "…or untick INV-000002": the amount received stays 453.00, so 18.00 is applied to nothing.
    let html = invalid(submit_page(&mut app, &path, fields("453.00", &[(older.as_str(), "435.00", "1"), (newer.as_str(), "", "0")], Some(&confirm))));
    assert!(html.contains("You applied 435.00 but received 453.00. The amounts applied to invoices must add up to the amount received; FreedomBooks does not keep part of a payment as unapplied credit."), "{html}");
    assert_eq!(receipts(&mut app), 0, "nothing saved");
    assert_eq!(value(&html, "amount"), "453.00", "the amount received is kept as typed");
    let row = html.split_once(&format!("data-invoice=\"{newer}\"")).unwrap().1.split('>').next().unwrap();
    assert!(row.contains("data-chosen=\"0\""), "unticked, and drawn so: {row}");
}
