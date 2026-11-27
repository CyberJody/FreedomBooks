//! Bookkeeper round 7, the missing bank-feed features (fix list items 46-48):
//! a row split over several accounts in Review, fee and adjustment lines on deposits and payments, and a
//! deposit of several received payments from one Review row. Page behaviour is tested in
//! `electron/tests/worker-fixes-3b.test.cjs`; this file tests what is posted and what the server draws.
//! Every posting is checked line by line (account, debit, credit) and against the trial balance.
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
fn count(app: &mut App, sql: &str) -> i64 {
    app.open_mut().unwrap().book.read(|q| q.query_row(sql, [], |r| r.get(0))).unwrap()
}
/// Every line of a journal as (account code, debit cents, credit cents), in line order.
fn lines(app: &mut App, journal: &str) -> Vec<(String, i64, i64)> {
    app.open_mut()
        .unwrap()
        .book
        .read(|q| -> Result<_, fb_core::FbError> {
            let mut st = q.prepare("SELECT a.code,l.debit,l.credit FROM ledger_journalline l JOIN ledger_account a ON a.id=l.account_id WHERE l.journal_id=?1 ORDER BY l.position,l.id")?;
            Ok(st.query_map([journal], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<Vec<_>, _>>()?)
        })
        .unwrap()
}
fn owned(rows: &[(&str, i64, i64)]) -> Vec<(String, i64, i64)> {
    rows.iter().map(|(a, d, c)| (a.to_string(), *d, *c)).collect()
}
/// The trial balance at the end of the year: it balances, the book passes its integrity checks, and the
/// net of each account named (debit positive, credit negative) is as given.
fn trial_balance(app: &mut App, c: &str, nets: &[(&str, i64)]) {
    let co = fb_core::CompanyId::parse(c).unwrap();
    let o = app.open_mut().unwrap();
    assert!(o.book.check().unwrap().is_empty(), "{:?}", o.book.check().unwrap());
    let tb = fb_core::reports::trial_balance(&mut o.book, o.owner, co, "2026-12-31").unwrap();
    assert_eq!(tb.debit, tb.credit, "the trial balance balances");
    for (code, net) in nets {
        let row = tb.rows.iter().find(|r| r.account.code == *code).unwrap_or_else(|| panic!("no {code}"));
        assert_eq!((row.debit - row.credit).cents(), i128::from(*net), "{code}");
    }
}

// --- bank feeds ---------------------------------------------------------------------------------------

struct Feeds {
    app: App,
    c: String,
    path: String,
    card: String,
    conn: String,
}
fn staged(id: &str, date: &str, amount: i64, description: &str) -> bf::StagedTransaction {
    bf::StagedTransaction { remote_id: id.into(), posted_on: Some(date.into()), transacted_on: None, amount, description: description.into(), pending: false }
}
fn feed_account(remote: &str, transactions: Vec<bf::StagedTransaction>) -> bf::StagedAccount {
    bf::StagedAccount { conn_key: "synthetic".into(), remote_id: remote.into(), name: remote.into(), org_name: "Synthetic Test Bank".into(), currency: "USD".into(), balance: None, available: None, balance_epoch: None, skipped: vec![], transactions }
}
impl Feeds {
    /// Round 7's kinds of row, made up: checking (mapped to 1000) with a split bill, a split deposit, a
    /// deposit of two customer checks and a card-processor payout; a card (a new card account) with a split
    /// charge.
    fn new() -> Self {
        let (mut app, c) = fixture(StarterChart::Standard);
        app.simplefin = true;
        let co = fb_core::CompanyId::parse(&c).unwrap();
        let checking = account(&mut app, &c, "1000");
        let o = app.open_mut().unwrap();
        let conn = bf::connect(&mut o.book, o.owner, co, &json!("Synthetic downloads"), "bridge.simplefin.org", &bf::SecretSet::none()).unwrap()["id"].as_str().unwrap().to_owned();
        let pull = bf::StagedPull {
            accounts: vec![
                feed_account("Checking", vec![
                    staged("TX-0002", "2026-06-05", -11_309, "FERNVALE FIBER ACH"),
                    staged("TX-0057", "2026-08-12", 82_505, "CARD PROCESSOR PAYOUT"),
                    staged("TX-0064", "2026-08-26", 57_550, "DEPOSIT"),
                    staged("TX-0090", "2026-09-30", 12_000, "MOBILE DEPOSIT"),
                ]),
                feed_account("Card", vec![staged("TX-0001", "2026-06-03", -16_368, "BENCHMART ELECTRONICS")]),
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
        Feeds { app, c, path, card, conn }
    }
    fn pk(&mut self, remote: &str) -> i64 {
        let conn = self.conn.clone();
        self.app.open_mut().unwrap().book.read(|q| q.query_row("SELECT t.ordinal FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id WHERE f.connection_id=?1 AND t.remote_id=?2", [&conn, remote], |r| r.get(0))).unwrap()
    }
    /// The transaction id (`bankfeeds_transaction.id`) of a downloaded row, as the core's `accept` takes it.
    fn tx_id(&mut self, remote: &str) -> String {
        let conn = self.conn.clone();
        self.app.open_mut().unwrap().book.read(|q| q.query_row("SELECT t.id FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id WHERE f.connection_id=?1 AND t.remote_id=?2", [&conn, remote], |r| r.get(0))).unwrap()
    }
    fn page(&mut self) -> String {
        let path = self.path.clone();
        page(&mut self.app, &path)
    }
    fn account(&mut self, code: &str) -> String {
        let c = self.c.clone();
        account(&mut self.app, &c, code)
    }
    /// What the page's form posts for its rows as drawn (every NEW row's single-valued fields), with `set`
    /// replacing a row's drafts, then `extra` (repeated fields such as split lines, and the button).
    fn post(&mut self, html: &str, set: &[(String, String)], extra: &[(String, String)]) -> Value {
        let mut fields: Vec<(String, String)> = vec![("inline".into(), "1".into()), ("review_layout".into(), "new".into())];
        for name in ["kind_", "account_", "payee_", "number_", "line_", "to_", "link_to_"] {
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
        fields.extend(extra.iter().cloned());
        let path = self.path.clone();
        let token = self.app.tokens.mint(&path);
        match self.app.run_renderer(Command::SubmitForm { path, token, fields }).unwrap() {
            Outcome::Done { data } => data,
            o => panic!("expected an in-place reply: {o:?}"),
        }
    }
    fn journals(&mut self) -> i64 {
        count(&mut self.app, "SELECT count(*) FROM ledger_journal")
    }
    fn row(&mut self, pk: i64) -> (String, Option<String>, String) {
        self.app.open_mut().unwrap().book.read(|q| q.query_row("SELECT status,journal_id,reference FROM bankfeeds_transaction WHERE ordinal=?1", [pk], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))).unwrap()
    }
}
/// A fragment of an in-place reply, with the template's escaped apostrophes read back.
fn fragment(data: &Value, id: &str) -> String {
    data["fragments"].as_array().unwrap().iter().find(|f| f["id"] == id).and_then(|f| f["html"].as_str()).unwrap_or_else(|| panic!("no fragment {id}: {data}")).replace("&#x27;", "'")
}
fn row_of(html: &str, pk: i64) -> &str {
    html.split_once(&format!("id=\"review-row-{pk}\"")).unwrap_or_else(|| panic!("no row {pk}: {html}")).1.split_once("</tr>").unwrap().0
}
fn set(pk: i64, pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| (format!("{k}_{pk}"), v.to_string())).collect()
}
/// "Split" ticked and its lines (account id, amount, memo), as the page posts them.
fn split(pk: i64, lines: &[(&str, &str, &str)]) -> Vec<(String, String)> {
    let mut out = vec![(format!("split_{pk}"), "1".to_owned())];
    for (account, amount, memo) in lines {
        out.push((format!("split_account_{pk}"), account.to_string()));
        out.push((format!("split_amount_{pk}"), amount.to_string()));
        out.push((format!("split_memo_{pk}"), memo.to_string()));
    }
    out
}
fn button(name: &str, pk: i64) -> (String, String) {
    (name.to_owned(), pk.to_string())
}

// --- item 46: split a row in Review --------------------------------------------------------------------

/// Item 46: a checking withdrawal split over Utilities and Owner's Draw (round 7's TX-0002) is one check with
/// the row's EFT number and two lines that add up to the bank amount. Undo voids it as for a one-line row.
#[test]
fn a_split_withdrawal_is_one_check_with_a_line_per_account_and_undo_voids_it() {
    let mut f = Feeds::new();
    let pk = f.pk("TX-0002");
    let page = f.page();
    let row = row_of(&page, pk).to_owned();
    assert!(row.contains(&format!(r#"<input type="checkbox" name="split_{pk}" value="1" data-split-toggle> Split</label>"#)), "{row}");
    assert!(row.contains(r#"data-split data-amount="113.09""#) && row.contains("data-split-lines hidden"), "no lines until Split is ticked: {row}");
    assert!(!row.contains(&format!("name=\"split_amount_{pk}\"")), "{row}");
    let (utilities, draw) = (f.account("6140"), f.account("3200"));
    let lines_in = [(utilities.as_str(), "93.10", "Internet and business voice"), (draw.as_str(), "19.99", "Home TV add-on (personal)")];
    let mut extra = split(pk, &lines_in);
    extra.push(button("accept_one", pk));
    let data = f.post(&page, &set(pk, &[("kind", "CHECK"), ("payee", "Fernvale Fiber")]), &extra);
    let (status, journal, reference) = f.row(pk);
    assert_eq!(status, "ACCEPTED", "{data}");
    assert!(fragment(&data, "review-status").contains(&format!(": recorded as {reference}, split over 2 lines")), "{data}");
    // One check, numbered EFT-<row>, paid from 1000: two expense lines with their memos.
    let check: Value = {
        let co = fb_core::CompanyId::parse(&f.c).unwrap();
        let id = reference.clone();
        let o = f.app.open_mut().unwrap();
        let check_id: String = o.book.read(|q| q.query_row("SELECT c.id FROM checks_check c JOIN documents_documentidentity i ON i.id=c.identity_id WHERE i.number=?1", [&id], |r| r.get(0))).unwrap();
        fb_core::checks::get_check(&mut o.book, o.owner, co, &check_id).unwrap()
    };
    assert_eq!(check["current"]["check_number"], format!("EFT-{pk}"));
    assert_eq!(check["current"]["payee_name"], "Fernvale Fiber");
    let descriptions: Vec<&str> = check["current"]["lines"].as_array().unwrap().iter().map(|l| l["description"].as_str().unwrap()).collect();
    assert_eq!(descriptions, ["Internet and business voice", "Home TV add-on (personal)"]);
    let journal = journal.unwrap();
    let mut posted = lines(&mut f.app, &journal);
    posted.sort();
    assert_eq!(posted, owned(&[("1000", 0, 11_309), ("3200", 1_999, 0), ("6140", 9_310, 0)]));
    let c = f.c.clone();
    trial_balance(&mut f.app, &c, &[("1000", -11_309), ("6140", 9_310), ("3200", 1_999)]);
    // Undo works as for any accepted row: it voids the check and the row is back in review.
    let page = f.page();
    f.post(&page, &[], &[button("undo_one", pk)]);
    let page = f.page();
    let data = f.post(&page, &[], &[button("undo_confirm_one", pk)]);
    assert_eq!(f.row(pk).0, "NEW", "{data}");
    trial_balance(&mut f.app, &c, &[("1000", 0), ("6140", 0), ("3200", 0)]);
}

/// Item 46: a split whose lines do not add up to the row is refused with the message at the lines; nothing
/// is posted and no check draft is left; the lines come back as typed. The same for a line with no amount and
/// a negative line on a check.
#[test]
fn a_split_that_does_not_add_up_is_refused_at_the_lines_and_nothing_is_posted() {
    let mut f = Feeds::new();
    let pk = f.pk("TX-0002");
    let (utilities, draw) = (f.account("6140"), f.account("3200"));
    let before = (f.journals(), count(&mut f.app, "SELECT count(*) FROM checks_check"));
    let attempt = |f: &mut Feeds, lines_in: &[(&str, &str, &str)]| -> String {
        let page = f.page();
        let mut extra = split(pk, lines_in);
        extra.push(button("accept_one", pk));
        let data = f.post(&page, &set(pk, &[("kind", "CHECK")]), &extra);
        fragment(&data, &format!("review-row-{pk}"))
    };
    let row = attempt(&mut f, &[(&utilities, "93.10", "Internet"), (&draw, "10.00", "")]);
    let message = "The split lines add up to 103.10, not the bank's 113.09: 9.99 is left to assign. Nothing was recorded.";
    let at_lines = row.split_once("data-split-lines>").expect("the lines are shown").1;
    assert!(at_lines.contains(&format!(r#"<span class="row-error-text split-error" role="alert" data-split-error>{message}</span>"#)), "{row}");
    assert_eq!(row.matches(message).count(), 1, "said once, at the lines: {row}");
    assert!(row.contains(&format!(r#"name="split_{pk}" value="1" data-split-toggle checked>"#)) && row.contains(r#"<span class="single-cat" hidden>"#), "{row}");
    assert!(row.contains(r#"value="93.10""#) && row.contains(r#"value="10.00""#) && row.contains(r#"value="Internet""#), "kept as typed: {row}");
    assert!(row.contains(&format!(r#"<option value="{draw}" selected>3200 · Owner's Draw</option>"#)), "{row}");
    assert!(row.contains("data-status=\"NEW\""));
    let over = attempt(&mut f, &[(&utilities, "93.10", ""), (&draw, "21.00", "")]);
    assert!(over.contains("The split lines add up to 114.10, not the bank's 113.09: they are 1.01 over. Nothing was recorded."), "{over}");
    let blank = attempt(&mut f, &[(&utilities, "113.09", ""), (&draw, "", "")]);
    assert!(blank.contains("Enter an amount such as 19.99 on each split line."), "{blank}");
    let negative = attempt(&mut f, &[(&utilities, "133.09", ""), (&draw, "-20.00", "")]);
    assert!(negative.contains("A check's split lines must each be more than 0.00. Record it as a journal entry to put a line the other way."), "{negative}");
    let none = attempt(&mut f, &[]);
    assert!(none.contains("Enter the split lines, or untick Split."), "{none}");
    assert_eq!((f.journals(), count(&mut f.app, "SELECT count(*) FROM checks_check")), before, "nothing posted, no draft");
    assert_eq!(f.row(pk).0, "NEW");
}

/// Item 46: a split card charge (round 7's TX-0001) and a split deposit with a line the other way are
/// journal entries with one line per account against the one bank or card line.
#[test]
fn a_split_card_charge_and_a_split_deposit_are_journal_entries_with_a_line_per_account() {
    let mut f = Feeds::new();
    let (charge, deposit) = (f.pk("TX-0001"), f.pk("TX-0090"));
    let (cogs, supplies, sales, fees) = (f.account("5000"), f.account("6100"), f.account("4000"), f.account("6160"));
    let page = f.page();
    let mut extra = split(charge, &[(&cogs, "149.78", "Resale SSDs"), (&supplies, "13.90", "Shop cable")]);
    extra.push(button("accept_one", charge));
    f.post(&page, &set(charge, &[("kind", "JOURNAL")]), &extra);
    let (status, journal, _) = f.row(charge);
    assert_eq!(status, "ACCEPTED");
    let card_code: String = { let card = f.card.clone(); f.app.open_mut().unwrap().book.read(|q| q.query_row("SELECT code FROM ledger_account WHERE id=?1", [&card], |r| r.get(0))).unwrap() };
    assert_eq!(lines(&mut f.app, &journal.unwrap()), vec![(card_code.clone(), 0, 16_368), ("5000".into(), 14_978, 0), ("6100".into(), 1_390, 0)]);
    // A deposit split with a line the other way: 140.00 of sales less a 20.00 fee kept back.
    let page = f.page();
    let mut extra = split(deposit, &[(&sales, "140.00", "Market stall"), (&fees, "-20.00", "Terminal fee")]);
    extra.push(button("accept_one", deposit));
    f.post(&page, &set(deposit, &[("kind", "JOURNAL")]), &extra);
    let (status, journal, _) = f.row(deposit);
    assert_eq!(status, "ACCEPTED");
    let journal = journal.unwrap();
    assert_eq!(lines(&mut f.app, &journal), owned(&[("1000", 12_000, 0), ("4000", 0, 14_000), ("6160", 2_000, 0)]));
    let memos: Vec<String> = f.app.open_mut().unwrap().book.read(|q| -> Result<_, fb_core::FbError> {
        let mut st = q.prepare("SELECT description FROM ledger_journalline WHERE journal_id=?1 ORDER BY position")?;
        Ok(st.query_map([&journal], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?)
    }).unwrap();
    assert_eq!(memos, ["MOBILE DEPOSIT", "Market stall", "Terminal fee"]);
    let c = f.c.clone();
    trial_balance(&mut f.app, &c, &[(&card_code, -16_368), ("5000", 14_978), ("6100", 1_390), ("1000", 12_000), ("4000", -14_000), ("6160", 2_000)]);
}

// --- item 47: fee and adjustment lines on deposits and payments -----------------------------------------

/// A posted invoice for `customer` of `amount` dated `date`.
fn posted_invoice(app: &mut App, c: &str, customer: &str, date: &str, amount: &str) -> String {
    let income = account(app, c, "4000");
    let id = run(app, json!({"type":"SaveInvoice","company_id":c,"values":{"request_key":key(),"customer_id":customer,"date":date,"due_date":date,"memo":"","tax_mode":"NONE",
        "lines":[{"description":"Annual service plan","quantity":"1","rate":amount,"taxable":false,"income_account_id":income}]}}))["id"].as_str().unwrap().to_owned();
    post_invoice(app, c, &id).unwrap();
    id
}
fn value_of(html: &str, name: &str) -> String {
    let at = html.find(&format!("name=\"{name}\"")).unwrap_or_else(|| panic!("no {name}"));
    let tag = &html[html[..at].rfind('<').unwrap()..];
    tag.split('>').next().unwrap().split_once("value=\"").map(|(_, v)| v.split('"').next().unwrap().to_owned()).unwrap_or_default()
}
fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
    p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}
fn submit_page(app: &mut App, path: &str, fields: Vec<(String, String)>) -> Outcome {
    let token = app.tokens.mint(path);
    app.run(Command::SubmitForm { path: path.into(), token, fields }).unwrap()
}
/// Receive payment's form for one invoice paid in full, as the page posts it.
#[allow(clippy::too_many_arguments)]
fn payment_form(app: &mut App, c: &str, customer: &str, invoice: &str, amount: &str, date: &str, bank: Option<&str>, fees: &[(&str, &str)]) -> (String, Vec<(String, String)>) {
    let path = url("receipts:new", &[c]);
    let html = page(app, &format!("{path}?invoice={invoice}"));
    let mut p = pairs(&[("request_key", &value_of(&html, "request_key")), ("customer", customer), ("date", date), ("amount", amount), ("method", "CARD"), ("reference", "Processor batch 0812"),
        ("destination", if bank.is_some() { "BANK" } else { "UNDEPOSITED" }), ("bank", bank.unwrap_or("")), ("action", "save"),
        ("allocations-TOTAL_FORMS", "1"), ("allocations-0-invoice", invoice), ("allocations-0-pay", "on"), ("allocations-0-amount", amount), ("allocations-0-chosen", "1")]);
    for (account, amount) in fees {
        p.push(("fee_account".into(), account.to_string()));
        p.push(("fee_amount".into(), amount.to_string()));
    }
    (path, p)
}
fn receipt_ids(app: &mut App) -> Vec<String> {
    app.open_mut().unwrap().book.read(|q| -> Result<_, fb_core::FbError> {
        let mut st = q.prepare("SELECT id FROM sales_receipt ORDER BY date,id")?;
        Ok(st.query_map([], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?)
    }).unwrap()
}
fn deposits(app: &mut App) -> Vec<(String, String, i64)> {
    app.open_mut().unwrap().book.read(|q| -> Result<_, fb_core::FbError> {
        let mut st = q.prepare("SELECT id,journal_id,amount FROM banking_deposit ORDER BY id")?;
        Ok(st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<Vec<_>, _>>()?)
    }).unwrap()
}

/// Item 47, Record deposit: round 7's processor payout. The 850.00 payment was received into Undeposited
/// Funds; the deposit has a fee line of -24.95 on 6160 (the form starts with it): the bank gets 825.05, the
/// fee account 24.95, and Undeposited Funds is credited 850.00. Reversing the deposit undoes all three.
#[test]
fn a_deposit_with_a_fee_line_posts_the_net_to_the_bank_and_the_fee_to_its_account() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let old_mill = customer(&mut app, &c, "Old Mill Insurance Agency (TEST)");
    let invoice = posted_invoice(&mut app, &c, &old_mill, "2026-08-04", "850.00");
    let (path, p) = payment_form(&mut app, &c, &old_mill, &invoice, "850.00", "2026-08-12", None, &[]);
    redirect(submit_page(&mut app, &path, p));
    let receipt = receipt_ids(&mut app).pop().unwrap();
    let (bank, fees_account) = (account(&mut app, &c, "1000"), account(&mut app, &c, "6160"));
    let path = url("deposits:new", &[&c]);
    let html = page(&mut app, &path);
    // The form offers fee lines, starting with the bank-charges account.
    let fees_box = html.split_once("<fieldset class=\"fee-lines\"").expect("the fee lines").1.split("</fieldset>").next().unwrap().to_owned();
    assert!(fees_box.contains(&format!(r#"<option value="{fees_account}" data-expense selected>6160 · Bank Service Charges</option>"#)), "{fees_box}");
    assert!(fees_box.contains(r#"data-base-from="receipts""#) && fees_box.contains("A fee is negative, such as -24.95"), "{fees_box}");
    assert!(html.contains(&format!(r#"id="receipt-{receipt}" data-amount="850.00""#)), "the receipt's amount for the live total");
    let rk = value_of(&html, "request_key");
    let fields = |action: &str, token: &str| pairs(&[("request_key", &rk), ("bank", &bank), ("date", "2026-08-12"), ("reference", "Payout 0812"), ("memo", ""), ("receipts", &receipt),
        ("fee_account", &fees_account), ("fee_amount", "-24.95"), ("action", action), ("review_token", token)]);
    let Outcome::Page { html, status: 200 } = submit_page(&mut app, &path, fields("review", "")) else { panic!("reviewed") };
    let review = html.split_once("class=\"deposit-review\"").unwrap().1.to_owned();
    assert!(review.contains(r#"total <strong id="review-total">825.05</strong> USD from 1 payment (850.00) and 1 fee or adjustment line"#), "{review}");
    assert!(review.contains("<td colspan=\"3\">Fee or adjustment</td><td>6160 · Bank Service Charges</td><td class=\"num\">-24.95</td>"), "{review}");
    assert!(review.contains("<td>Fee or adjustment</td><td>6160 · Bank Service Charges</td>"), "the posting role: {review}");
    let token = value_of(&html, "review_token");
    let location = redirect(submit_page(&mut app, &path, fields("record", &token)));
    let (deposit, journal, amount) = deposits(&mut app).pop().unwrap();
    assert_eq!(amount, 82_505);
    let mut posted = lines(&mut app, &journal);
    posted.sort();
    assert_eq!(posted, owned(&[("1000", 82_505, 0), ("1200", 0, 85_000), ("6160", 2_495, 0)]));
    trial_balance(&mut app, &c, &[("1000", 82_505), ("1200", 0), ("1100", 0), ("6160", 2_495), ("4000", -85_000)]);
    // The audit trail records the fee line like the receipts.
    let audit: String = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT detail FROM ledger_auditevent WHERE action='deposit.recorded' ORDER BY occurred_at DESC LIMIT 1", [], |r| r.get(0))).unwrap();
    let audit: Value = serde_json::from_str(&audit).unwrap();
    assert_eq!(audit["adjustments"], json!([{"account_id": fees_account, "amount": "-24.95"}]));
    assert_eq!((audit["amount"].as_str(), audit["receipts_total"].as_str()), (Some("825.05"), Some("850.00")));
    // The deposit page shows the payments, the fee line and the total.
    let detail = page(&mut app, &location);
    assert!(detail.contains(r#"<th colspan="5">Payments</th><td class="num" id="deposit-receipts-total">850.00</td>"#), "{detail}");
    assert!(detail.contains(r#"<th colspan="4">Fee or adjustment</th><td>6160 · Bank Service Charges</td><td class="num">-24.95</td>"#), "{detail}");
    assert!(detail.contains(r#"<th class="num" id="deposit-total">825.05</th>"#));
    // Reversed: every line is undone.
    let co = fb_core::CompanyId::parse(&c).unwrap();
    let o = app.open_mut().unwrap();
    fb_core::banking::reverse_deposit(&mut o.book, o.owner, co, &deposit, Some(1), &json!("2026-08-13"), &json!("Synthetic test: wrong payout")).unwrap();
    trial_balance(&mut app, &c, &[("1000", 0), ("1200", 85_000), ("6160", 0)]);
}

/// Item 47: what a fee or adjustment line may not do. Nothing is recorded for any of these.
#[test]
fn fee_lines_are_refused_on_the_bank_on_undeposited_funds_on_control_accounts_and_when_nothing_is_left() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let old_mill = customer(&mut app, &c, "Old Mill Insurance Agency (TEST)");
    let invoice = posted_invoice(&mut app, &c, &old_mill, "2026-08-04", "850.00");
    let (path, p) = payment_form(&mut app, &c, &old_mill, &invoice, "850.00", "2026-08-12", None, &[]);
    redirect(submit_page(&mut app, &path, p));
    let receipt = receipt_ids(&mut app).pop().unwrap();
    let co = fb_core::CompanyId::parse(&c).unwrap();
    let (bank, undeposited, receivables, fees_account, sales) = (account(&mut app, &c, "1000"), account(&mut app, &c, "1200"), account(&mut app, &c, "1100"), account(&mut app, &c, "6160"), account(&mut app, &c, "4000"));
    let o = app.open_mut().unwrap();
    let review = |o: &mut fb_app::session::Open, adjustments: Value| -> String {
        let input = json!({"request_key":key(),"bank_id":bank,"date":"2026-08-12","reference":"","memo":"","receipt_ids":[receipt],"adjustments":adjustments});
        match fb_core::banking::review_deposit(&mut o.book, o.owner, co, &input) { Ok(r) => format!("reviewed {}", r.display["amount"]), Err(e) => e.to_string() }
    };
    assert_eq!(review(o, json!([{"account_id":fees_account,"amount":"-24.95"},{"account_id":sales,"amount":"5.00"}])), "reviewed \"830.05\"", "a positive line adds");
    for (adjustments, message) in [
        (json!([{"account_id":bank,"amount":"-24.95"}]), "A fee or adjustment cannot use the bank account the deposit goes to."),
        (json!([{"account_id":undeposited,"amount":"-24.95"}]), "A fee or adjustment cannot use the undeposited funds account the payments are held in."),
        (json!([{"account_id":receivables,"amount":"-24.95"}]), "Fees and adjustments cannot use control accounts."),
        (json!([{"account_id":fees_account,"amount":"-850.00"}]), "The fees and adjustments (-850.00) would leave nothing to deposit from payments of 850.00."),
        (json!([{"account_id":fees_account,"amount":"0.00"}]), "A fee or adjustment line cannot be 0.00."),
        (json!([{"account_id":fees_account,"amount":"-24.955"}]), "Enter each fee or adjustment as an amount such as -24.95."),
        (json!([{"account_id":fees_account,"amount":"-1.00"},{"account_id":fees_account,"amount":"-2.00"}]), "Use one fee or adjustment line per account."),
    ] {
        let got = review(o, adjustments.clone());
        assert!(got.contains(message), "{adjustments}: {got}");
    }
    assert!(deposits(&mut app).is_empty());
}

/// Item 47, Receive payment straight to the bank with a fee line: the invoice is paid in full (850.00), the
/// bank gets the net (825.05) and the fee account 24.95. The payment is held in Undeposited Funds and
/// deposited net of the fee in the same step, so the bank has one line the feed row can match.
#[test]
fn receive_payment_to_a_bank_with_a_fee_pays_the_invoice_in_full_and_the_bank_gets_the_net() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let old_mill = customer(&mut app, &c, "Old Mill Insurance Agency (TEST)");
    let invoice = posted_invoice(&mut app, &c, &old_mill, "2026-08-04", "850.00");
    let (bank, fees_account) = (account(&mut app, &c, "1000"), account(&mut app, &c, "6160"));
    // The form has the fee lines, hidden until the payment goes straight to a bank.
    let html = page(&mut app, &format!("{}?invoice={invoice}", url("receipts:new", &[&c])));
    assert!(html.contains(r#"data-base-from="amount" data-base-label="Received" data-total-label="the bank gets""#) && html.contains("data-default=\"") , "{html}");
    let fieldset = html.split_once("<fieldset class=\"fee-lines\"").unwrap().1.split('>').next().unwrap().to_owned();
    // Review 3b M7: hidden, the lines are disabled too, so they are not sent.
    assert!(fieldset.ends_with(" hidden disabled"), "{fieldset}");
    // A fee on a payment kept in Undeposited Funds is refused: it belongs on the deposit.
    let (path, p) = payment_form(&mut app, &c, &old_mill, &invoice, "850.00", "2026-08-12", None, &[(&fees_account, "-24.95")]);
    let Outcome::Invalid { html } = submit_page(&mut app, &path, p) else { panic!("refused") };
    assert!(html.contains("Fees and adjustments are taken off when the money reaches the bank. Choose the bank under Deposit to, or put the fee on the deposit when you deposit this payment."), "{html}");
    assert!(!html.split_once("<fieldset class=\"fee-lines\"").unwrap().1.split('>').next().unwrap().contains("hidden"), "shown with its message");
    assert!(receipt_ids(&mut app).is_empty());
    // Straight to 1000 with the fee: saved, and said.
    let (path, p) = payment_form(&mut app, &c, &old_mill, &invoice, "850.00", "2026-08-12", Some(&bank), &[(&fees_account, "-24.95")]);
    let location = redirect(submit_page(&mut app, &path, p));
    let shown = page(&mut app, &location);
    assert!(shown.contains("Saved RCP-000001: 850.00 applied to the invoices. Deposited to 1000 · Checking as DEP-000001: 825.05 (850.00 with fees and adjustments of -24.95)."), "{shown}");
    let co = fb_core::CompanyId::parse(&c).unwrap();
    let balances = { let o = app.open_mut().unwrap(); fb_core::sales::invoice_balances(&mut o.book, o.owner, co).unwrap() };
    let paid = balances.iter().find(|b| b["invoice_id"] == invoice.as_str()).unwrap();
    assert_eq!((paid["payment_state"].as_str(), paid["allocated"].as_str(), paid["outstanding"].as_str()), (Some("PAID"), Some("850.00"), Some("0.00")), "paid in full: {paid}");
    let receipt_journal: String = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT journal_id FROM sales_receipt", [], |r| r.get(0))).unwrap();
    assert_eq!(lines(&mut app, &receipt_journal), owned(&[("1200", 85_000, 0), ("1100", 0, 85_000)]));
    let (_, journal, amount) = deposits(&mut app).pop().unwrap();
    assert_eq!(amount, 82_505);
    let mut posted = lines(&mut app, &journal);
    posted.sort();
    assert_eq!(posted, owned(&[("1000", 82_505, 0), ("1200", 0, 85_000), ("6160", 2_495, 0)]));
    trial_balance(&mut app, &c, &[("1000", 82_505), ("1100", 0), ("1200", 0), ("6160", 2_495), ("4000", -85_000)]);
}

/// Item 47, book format 96 (migration 0209): a populated format-95 book, with a deposit recorded the old way,
/// is backed up, upgraded with every row unchanged, still passes every integrity check, and then takes a
/// deposit with a fee line.
#[test]
fn a_populated_format_95_book_upgrades_and_then_takes_a_deposit_with_a_fee() {
    use fb_db::{Book, Opened, SnapshotSource, test_support};
    let (mut app, c) = fixture(StarterChart::Standard);
    let old_mill = customer(&mut app, &c, "Old Mill Insurance Agency (TEST)");
    let first = posted_invoice(&mut app, &c, &old_mill, "2026-08-04", "850.00");
    let second = posted_invoice(&mut app, &c, &old_mill, "2026-09-09", "850.00");
    for (invoice, date) in [(&first, "2026-08-12"), (&second, "2026-09-16")] {
        let (path, p) = payment_form(&mut app, &c, &old_mill, invoice, "850.00", date, None, &[]);
        redirect(submit_page(&mut app, &path, p));
    }
    let receipts = receipt_ids(&mut app);
    let (bank, fees_account) = (account(&mut app, &c, "1000"), account(&mut app, &c, "6160"));
    let co = fb_core::CompanyId::parse(&c).unwrap();
    let deposit = |book: &mut Book, owner, receipt: &str, adjustments: Value| -> Result<Value, fb_core::FbError> {
        let mut input = json!({"request_key":key(),"bank_id":bank,"date":"2026-09-16","reference":"","memo":"","receipt_ids":[receipt]});
        if !adjustments.is_null() {
            input["adjustments"] = adjustments;
        }
        let review = fb_core::banking::review_deposit(book, owner, co, &input)?;
        fb_core::banking::record_deposit(book, owner, co, &input, &review)
    };
    {
        let o = app.open_mut().unwrap();
        deposit(&mut o.book, o.owner, &receipts[0], Value::Null).unwrap();
    }
    let owner = app.open_mut().unwrap().owner;
    let dir = temp_dir();
    let key_bytes = [95; 32];
    let old_path = dir.join("format-95.fbbook");
    let copy = test_support::populated_copy(app.open_mut().unwrap().book.connection(), &old_path, &key_bytes, "0208_ai_unattended", true).unwrap();
    let Opened::Behind(old) = Book::open_staged(&old_path, &key_bytes).unwrap() else { panic!("expected a format-95 book") };
    assert_eq!((old.from(), old.to()), (95, 97));
    let backup = dir.join("verified-format-95.fbbook");
    old.snapshot(&backup, &key_bytes).unwrap();
    assert_eq!(test_support::snapshot_file(&backup, &key_bytes).unwrap(), copy.before, "the backup is the book as it was");
    let mut upgraded = old.upgrade().unwrap();
    assert_eq!(test_support::snapshot(upgraded.connection()).unwrap(), copy.expected, "every earlier value is unchanged");
    assert!(upgraded.check().unwrap().is_empty(), "{:?}", upgraded.check().unwrap());
    // The upgraded book takes a deposit net of a fee.
    let recorded = deposit(&mut upgraded, owner, &receipts[1], json!([{"account_id":fees_account,"amount":"-24.95"}])).unwrap();
    assert_eq!(recorded["amount"], "825.05");
    assert_eq!(recorded["adjustments"], json!([{"account_id":fees_account,"amount":"-24.95"}]));
    assert!(upgraded.check().unwrap().is_empty());
    drop(upgraded);
    let reopened = Book::open(&old_path, &key_bytes).unwrap();
    assert!(reopened.check().unwrap().is_empty());
}

// --- item 48: deposit several received payments from one Review row --------------------------------------

/// Received payments for the Feeds fixture: an invoice per (customer, amount, payment date), paid into
/// Undeposited Funds. Returns the receipt ids in that order.
fn received(f: &mut Feeds, payments: &[(&str, &str, &str)]) -> Vec<String> {
    let c = f.c.clone();
    let mut out = vec![];
    for (name, amount, date) in payments {
        let who = customer(&mut f.app, &c, name);
        let invoice = posted_invoice(&mut f.app, &c, &who, "2026-08-01", amount);
        let (path, p) = payment_form(&mut f.app, &c, &who, &invoice, amount, date, None, &[]);
        redirect(submit_page(&mut f.app, &path, p));
        out.push(receipt_ids(&mut f.app).into_iter().find(|id| !out.contains(id)).unwrap());
    }
    out
}
fn deposit_panel(row: &str) -> &str {
    row.split_once("data-deposit ").expect("the deposit panel").1
}

/// Item 48 with item 47: round 7's processor payout (TX-0057, 825.05). The 850.00 payment was received into
/// Undeposited Funds first; on the bank row it is ticked under "Deposit received payments" with a fee line of
/// -24.95. Accept records the deposit and matches the row to it in one step: the bank has one line of
/// 825.05. Undo reverses the deposit.
#[test]
fn a_processor_payout_is_deposited_from_its_review_row_with_the_fee_in_one_step() {
    let mut f = Feeds::new();
    let receipts = received(&mut f, &[("Old Mill Insurance Agency (TEST)", "850.00", "2026-08-12")]);
    let pk = f.pk("TX-0057");
    let fees_account = f.account("6160");
    let page = f.page();
    let row = row_of(&page, pk).replace("&#x27;", "'");
    assert!(row.contains(r#"<option value="DEPOSIT">Deposit received payments</option>"#), "{row}");
    let panel = deposit_panel(&row);
    assert!(panel.starts_with(r#"data-amount="825.05""#), "{panel}");
    assert!(panel.contains(&format!(r#"<input type="checkbox" name="receipt_{pk}" value="{}" data-amount="850.00">"#, receipts[0])), "listed, not ticked (another amount): {panel}");
    assert!(panel.contains("RCP-000001 Old Mill Insurance Agency (TEST)<span class=\"muted\"> · 2026-08-12 · 850.00</span>"), "{panel}");
    assert!(panel.contains(&format!(r#"<option value="{fees_account}" selected>6160 · Bank Service Charges</option>"#)), "the fee line starts on bank charges: {panel}");
    let before = f.journals();
    let mut extra = vec![(format!("receipt_{pk}"), receipts[0].clone()), (format!("fee_account_{pk}"), fees_account.clone()), (format!("fee_amount_{pk}"), "-24.95".into())];
    extra.push(button("accept_one", pk));
    let data = f.post(&page, &set(pk, &[("kind", "DEPOSIT")]), &extra);
    let (status, journal, reference) = f.row(pk);
    assert_eq!((status.as_str(), reference.as_str()), ("ACCEPTED", "DEP-000001"), "{data}");
    assert_eq!(f.journals(), before + 1, "one deposit entry, nothing else");
    let journal = journal.unwrap();
    let mut posted = lines(&mut f.app, &journal);
    posted.sort();
    assert_eq!(posted, owned(&[("1000", 82_505, 0), ("1200", 0, 85_000), ("6160", 2_495, 0)]));
    // The row points at the bank line of the deposit: the one 825.05 line the reconciliation will clear.
    let bank_lines = count(&mut f.app, &format!("SELECT count(*) FROM ledger_journalline l JOIN bankfeeds_transaction t ON t.journal_line_id=l.id WHERE t.ordinal={pk} AND l.debit=82505"));
    assert_eq!(bank_lines, 1);
    let c = f.c.clone();
    trial_balance(&mut f.app, &c, &[("1000", 82_505), ("1100", 0), ("1200", 0), ("6160", 2_495), ("4000", -85_000)]);
    // Undo reverses the deposit; the payment is back in Undeposited Funds and the row back in review.
    let page = f.page();
    f.post(&page, &[], &[button("undo_one", pk)]);
    let page = f.page();
    f.post(&page, &[], &[button("undo_confirm_one", pk)]);
    assert_eq!(f.row(pk).0, "NEW");
    trial_balance(&mut f.app, &c, &[("1000", 0), ("1200", 85_000), ("6160", 0)]);
}

/// Item 48: round 7's deposit of two customer checks (TX-0064, 575.50 = 300.00 + 275.50): both are ticked on
/// the bank row and deposited together.
#[test]
fn two_customer_checks_in_one_bank_deposit_are_ticked_on_its_review_row() {
    let mut f = Feeds::new();
    let receipts = received(&mut f, &[("Copperleaf Dental Studio (TEST)", "300.00", "2026-08-20"), ("Hollow Pine Veterinary (TEST)", "275.50", "2026-08-24")]);
    let pk = f.pk("TX-0064");
    let page = f.page();
    let panel = deposit_panel(row_of(&page, pk)).to_owned();
    for (id, amount) in receipts.iter().zip(["300.00", "275.50"]) {
        assert!(panel.contains(&format!(r#"value="{id}" data-amount="{amount}">"#)), "{panel}");
    }
    let mut extra: Vec<(String, String)> = receipts.iter().map(|id| (format!("receipt_{pk}"), id.clone())).collect();
    extra.push(button("accept_one", pk));
    f.post(&page, &set(pk, &[("kind", "DEPOSIT")]), &extra);
    let (status, journal, _) = f.row(pk);
    assert_eq!(status, "ACCEPTED");
    let mut posted = lines(&mut f.app, &journal.unwrap());
    posted.sort();
    assert_eq!(posted, owned(&[("1000", 57_550, 0), ("1200", 0, 57_550)]));
    let c = f.c.clone();
    trial_balance(&mut f.app, &c, &[("1000", 57_550), ("1200", 0), ("1100", 0), ("4000", -57_550)]);
}

/// Item 48: a deposit whose ticked payments (and fees) do not equal the bank amount is refused with the
/// message at the deposit lines; nothing is posted and the ticks and fees come back as they were.
#[test]
fn a_deposit_whose_ticks_do_not_equal_the_bank_amount_is_refused_and_nothing_is_posted() {
    let mut f = Feeds::new();
    let receipts = received(&mut f, &[("Copperleaf Dental Studio (TEST)", "300.00", "2026-08-20"), ("Hollow Pine Veterinary (TEST)", "275.50", "2026-08-24"), ("Old Mill Insurance Agency (TEST)", "850.00", "2026-08-12")]);
    let (two_checks, payout) = (f.pk("TX-0064"), f.pk("TX-0057"));
    let fees_account = f.account("6160");
    let before = (f.journals(), deposits(&mut f.app).len());
    let attempt = |f: &mut Feeds, pk: i64, extra: Vec<(String, String)>| -> String {
        let page = f.page();
        let mut extra = extra;
        extra.push(button("accept_one", pk));
        let data = f.post(&page, &set(pk, &[("kind", "DEPOSIT")]), &extra);
        fragment(&data, &format!("review-row-{pk}"))
    };
    let row = attempt(&mut f, two_checks, vec![(format!("receipt_{two_checks}"), receipts[0].clone())]);
    let message = "The selected receipts total 300.00, not the bank's 575.50.";
    assert!(deposit_panel(&row).contains(&format!(r#"<span class="row-error-text split-error" role="alert" data-deposit-error>{message}</span>"#)), "{row}");
    assert_eq!(row.matches(message).count(), 1, "said once, at the deposit lines");
    assert!(row.contains(&format!(r#"value="{}" data-amount="300.00" checked>"#, receipts[0])) && row.contains(&format!(r#"value="{}" data-amount="275.50">"#, receipts[1])), "ticks kept: {row}");
    let row = attempt(&mut f, payout, vec![(format!("receipt_{payout}"), receipts[2].clone()), (format!("fee_account_{payout}"), fees_account.clone()), (format!("fee_amount_{payout}"), "-20.00".into())]);
    assert!(row.contains("The selected receipts and fees total 830.00, not the bank's 825.05."), "{row}");
    assert!(row.contains(r#"name="fee_amount_"#) && row.contains(r#"value="-20.00""#), "the fee comes back as typed: {row}");
    let row = attempt(&mut f, payout, vec![(format!("receipt_{payout}"), receipts[2].clone()), (format!("fee_account_{payout}"), String::new()), (format!("fee_amount_{payout}"), "-24.95".into())]);
    assert!(row.contains("Choose an account for each fee or adjustment."), "{row}");
    let row = attempt(&mut f, payout, vec![]);
    assert!(row.contains("Choose the undeposited receipt(s) this deposit covers."), "{row}");
    assert_eq!((f.journals(), deposits(&mut f.app).len()), before, "nothing posted");
    assert_eq!((f.row(two_checks).0, f.row(payout).0), ("NEW".to_owned(), "NEW".to_owned()));
}

/// Items 47 and 48: the other route for a payout, Receive payment straight to the bank with the fee line,
/// leaves one 825.05 line on the bank, which the payout row offers as its match; matching posts nothing.
#[test]
fn a_payment_received_net_of_its_fee_is_what_the_payout_row_matches() {
    let mut f = Feeds::new();
    let c = f.c.clone();
    let who = customer(&mut f.app, &c, "Old Mill Insurance Agency (TEST)");
    let invoice = posted_invoice(&mut f.app, &c, &who, "2026-08-04", "850.00");
    let (bank, fees_account) = (f.account("1000"), f.account("6160"));
    let (path, p) = payment_form(&mut f.app, &c, &who, &invoice, "850.00", "2026-08-12", Some(&bank), &[(&fees_account, "-24.95")]);
    redirect(submit_page(&mut f.app, &path, p));
    let pk = f.pk("TX-0057");
    let page = f.page();
    assert!(row_of(&page, pk).contains(r#"<option value="MATCH" selected>Match existing entry</option>"#), "{}", row_of(&page, pk));
    let before = f.journals();
    f.post(&page, &[], &[button("accept_one", pk)]);
    let (status, _, reference) = f.row(pk);
    assert_eq!((status.as_str(), reference.as_str()), ("MATCHED", "DEP-000001"));
    assert_eq!(f.journals(), before, "a match posts nothing");
}

// --- review fixes ----------------------------------------

/// Record deposit as the form posts it: Review deposit, then Record deposit with that review's token and,
/// when the review carried one, its `fee_ok`. Returns the review page and the recorded deposit's address.
fn record_deposit_page(app: &mut App, c: &str, bank: &str, date: &str, receipts: &[&str], fees: &[(&str, &str)]) -> (String, String) {
    let path = url("deposits:new", &[c]);
    let rk = value_of(&page(app, &path), "request_key");
    let fields = |action: &str, extra: &[(&str, &str)]| {
        let mut p = pairs(&[("request_key", &rk), ("bank", bank), ("date", date), ("reference", ""), ("memo", ""), ("action", action)]);
        p.extend(receipts.iter().map(|r| ("receipts".to_owned(), r.to_string())));
        for (account, amount) in fees {
            p.extend(pairs(&[("fee_account", account), ("fee_amount", amount)]));
        }
        p.extend(pairs(extra));
        p
    };
    let Outcome::Page { html: review, status: 200 } = submit_page(app, &path, fields("review", &[])) else { panic!("reviewed") };
    let token = value_of(&review, "review_token");
    let ok = if review.contains("name=\"fee_ok\"") { value_of(&review, "fee_ok") } else { String::new() };
    let mut extra = vec![("review_token", token.as_str())];
    if !ok.is_empty() {
        extra.push(("fee_ok", ok.as_str()));
    }
    let location = redirect(submit_page(app, &path, fields("record", &extra)));
    (review, location)
}
fn reversal_journal(app: &mut App, table: &str, key_column: &str, key: &str) -> String {
    app.open_mut().unwrap().book.read(|q| q.query_row(&format!("SELECT journal_id FROM {table} WHERE {key_column}=?1"), [key], |r| r.get(0))).unwrap()
}
fn sorted(mut rows: Vec<(String, i64, i64)>) -> Vec<(String, i64, i64)> {
    rows.sort();
    rows
}
const REASON: [(&str, &str); 3] = [("expected_revision", "1"), ("date", "2026-08-14"), ("reason", "Synthetic test: the payout was charged back")];

/// Review 3b S1 (1): a deposit with a positive adjustment line, which credits its account. Round 7's payout
/// with its -24.95 fee and a 5.00 promotional credit from the processor on 4900 Other Income: the bank gets
/// 830.05. A positive line on an income account is not warned about (M1). Reversing undoes every line.
#[test]
fn a_deposit_with_a_positive_adjustment_line_credits_its_account_and_reverses() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let old_mill = customer(&mut app, &c, "Old Mill Insurance Agency (TEST)");
    let invoice = posted_invoice(&mut app, &c, &old_mill, "2026-08-04", "850.00");
    let (path, p) = payment_form(&mut app, &c, &old_mill, &invoice, "850.00", "2026-08-12", None, &[]);
    redirect(submit_page(&mut app, &path, p));
    let receipt = receipt_ids(&mut app).pop().unwrap();
    let (bank, fees_account, other_income) = (account(&mut app, &c, "1000"), account(&mut app, &c, "6160"), account(&mut app, &c, "4900"));
    let (review, location) = record_deposit_page(&mut app, &c, &bank, "2026-08-12", &[&receipt], &[(&fees_account, "-24.95"), (&other_income, "5.00")]);
    assert!(!review.contains("data-fee-warning"), "a positive line on an income account is not warned about: {review}");
    assert!(review.contains(r#"total <strong id="review-total">830.05</strong> USD from 1 payment (850.00) and 2 fee or adjustment lines"#), "{review}");
    let (deposit, journal, amount) = deposits(&mut app).pop().unwrap();
    assert_eq!(amount, 83_005);
    // The bank is debited the net, the fee debits 6160, the positive line credits 4900, the payment 1200.
    assert_eq!(sorted(lines(&mut app, &journal)), owned(&[("1000", 83_005, 0), ("1200", 0, 85_000), ("4900", 0, 500), ("6160", 2_495, 0)]));
    trial_balance(&mut app, &c, &[("1000", 83_005), ("1100", 0), ("1200", 0), ("4900", -500), ("6160", 2_495), ("4000", -85_000)]);
    let detail = page(&mut app, &location);
    assert!(detail.contains(r#"<th colspan="4">Fee or adjustment</th><td>4900 · Other Income</td><td class="num">5.00</td>"#), "{detail}");
    assert!(detail.contains(r#"<th class="num" id="deposit-total">830.05</th>"#), "{detail}");
    // Reversed from its page: every line is undone, and the payment is back in Undeposited Funds.
    redirect(submit_page(&mut app, &url("deposits:reverse", &[&c, &deposit]), pairs(&REASON)));
    let reversal = reversal_journal(&mut app, "banking_depositreversal", "deposit_id", &deposit);
    assert_eq!(sorted(lines(&mut app, &reversal)), owned(&[("1000", 0, 83_005), ("1200", 85_000, 0), ("4900", 500, 0), ("6160", 0, 2_495)]));
    trial_balance(&mut app, &c, &[("1000", 0), ("1100", 0), ("1200", 85_000), ("4900", 0), ("6160", 0), ("4000", -85_000)]);
}

/// Review 3b S1 (2): a split card charge recorded as a journal entry with a negative line. Round 7's TX-0001,
/// -163.68 on the card: 183.68 of resale SSDs on 5000, less a 20.00 store credit for a returned cable on 6100.
/// The card is credited 163.68, the positive line debits 5000, and the negative line credits 6100.
#[test]
fn a_split_card_charge_with_a_negative_line_credits_that_lines_account() {
    let mut f = Feeds::new();
    let charge = f.pk("TX-0001");
    let (cogs, supplies) = (f.account("5000"), f.account("6100"));
    let page = f.page();
    let mut extra = split(charge, &[(&cogs, "183.68", "Resale SSDs"), (&supplies, "-20.00", "Store credit, returned cable")]);
    extra.push(button("accept_one", charge));
    let data = f.post(&page, &set(charge, &[("kind", "JOURNAL")]), &extra);
    let (status, journal, _) = f.row(charge);
    assert_eq!(status, "ACCEPTED", "{data}");
    let card_code: String = { let card = f.card.clone(); f.app.open_mut().unwrap().book.read(|q| q.query_row("SELECT code FROM ledger_account WHERE id=?1", [&card], |r| r.get(0))).unwrap() };
    let journal = journal.unwrap();
    assert_eq!(lines(&mut f.app, &journal), vec![(card_code.clone(), 0, 16_368), ("5000".into(), 18_368, 0), ("6100".into(), 0, 2_000)]);
    let memos: Vec<String> = f.app.open_mut().unwrap().book.read(|q| -> Result<_, fb_core::FbError> {
        let mut st = q.prepare("SELECT description FROM ledger_journalline WHERE journal_id=?1 ORDER BY position")?;
        Ok(st.query_map([&journal], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?)
    }).unwrap();
    assert_eq!(memos, ["BENCHMART ELECTRONICS", "Resale SSDs", "Store credit, returned cable"]);
    let c = f.c.clone();
    trial_balance(&mut f.app, &c, &[(&card_code, -16_368), ("5000", 18_368), ("6100", -2_000)]);
}

/// Review 3b S1 (3): undoing a payment received net of its fee. Receive payment straight to the bank with a
/// -24.95 fee saves RCP-000001 (into Undeposited Funds) and DEP-000001 (to the bank). The receipt cannot be
/// reversed first: its page says to reverse the deposit, and nothing changes. Reversing the deposit, then the
/// receipt, opens the invoice again and returns 1000, 1100, 1200 and 6160 to their balances before the payment.
#[test]
fn a_payment_received_net_of_fees_is_reversed_deposit_first_and_the_invoice_opens_again() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let old_mill = customer(&mut app, &c, "Old Mill Insurance Agency (TEST)");
    let invoice = posted_invoice(&mut app, &c, &old_mill, "2026-08-04", "850.00");
    let before = [("1000", 0), ("1100", 85_000), ("1200", 0), ("6160", 0), ("4000", -85_000)];
    trial_balance(&mut app, &c, &before);
    let (bank, fees_account) = (account(&mut app, &c, "1000"), account(&mut app, &c, "6160"));
    let (path, p) = payment_form(&mut app, &c, &old_mill, &invoice, "850.00", "2026-08-12", Some(&bank), &[(&fees_account, "-24.95")]);
    redirect(submit_page(&mut app, &path, p));
    let paid = [("1000", 82_505), ("1100", 0), ("1200", 0), ("6160", 2_495), ("4000", -85_000)];
    trial_balance(&mut app, &c, &paid);
    let receipt = receipt_ids(&mut app).pop().unwrap();
    let (deposit, ..) = deposits(&mut app).pop().unwrap();
    let receipt_page = page(&mut app, &url("receipts:detail", &[&c, &receipt]));
    assert!(receipt_page.contains("DEP-000001"), "the receipt page names its deposit: {receipt_page}");
    // The receipt first: refused with the existing message; nothing is written.
    let reverse_receipt = url("receipts:reverse", &[&c, &receipt]);
    let Outcome::Invalid { html } = submit_page(&mut app, &reverse_receipt, pairs(&REASON)) else { panic!("refused") };
    assert!(html.contains("Reverse deposit DEP-000001 before reversing this receipt."), "{html}");
    assert_eq!((count(&mut app, "SELECT count(*) FROM sales_receiptreversal"), count(&mut app, "SELECT count(*) FROM banking_depositreversal")), (0, 0));
    trial_balance(&mut app, &c, &paid);
    // The deposit: the bank, the fee and Undeposited Funds are undone; the payment is held again.
    redirect(submit_page(&mut app, &url("deposits:reverse", &[&c, &deposit]), pairs(&REASON)));
    let undone = reversal_journal(&mut app, "banking_depositreversal", "deposit_id", &deposit);
    assert_eq!(sorted(lines(&mut app, &undone)), owned(&[("1000", 0, 82_505), ("1200", 85_000, 0), ("6160", 0, 2_495)]));
    trial_balance(&mut app, &c, &[("1000", 0), ("1100", 0), ("1200", 85_000), ("6160", 0), ("4000", -85_000)]);
    // Then the receipt: the invoice is open again, and every account is back where it was.
    redirect(submit_page(&mut app, &reverse_receipt, pairs(&REASON)));
    let undone = reversal_journal(&mut app, "sales_receiptreversal", "receipt_id", &receipt);
    assert_eq!(sorted(lines(&mut app, &undone)), owned(&[("1100", 85_000, 0), ("1200", 0, 85_000)]));
    trial_balance(&mut app, &c, &before);
    let co = fb_core::CompanyId::parse(&c).unwrap();
    let balances = { let o = app.open_mut().unwrap(); fb_core::sales::invoice_balances(&mut o.book, o.owner, co).unwrap() };
    let open = balances.iter().find(|b| b["invoice_id"] == invoice.as_str()).unwrap();
    assert_eq!((open["payment_state"].as_str(), open["allocated"].as_str(), open["outstanding"].as_str()), (Some("OPEN"), Some("0.00"), Some("850.00")), "{open}");
}

/// Review 3b M1: a fee typed without its minus sign. On Record deposit and on Receive payment, a positive line
/// on an expense account (6160) is said at the fee lines before anything is saved, with what it will do, and
/// recorded as typed only when confirmed: Record deposit from that review, or Save payment again. A changed
/// line is checked again. A positive line on an income account is not warned about.
#[test]
fn a_positive_fee_on_an_expense_account_is_said_before_it_is_saved() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let old_mill = customer(&mut app, &c, "Old Mill Insurance Agency (TEST)");
    let invoices: Vec<String> = ["2026-08-04", "2026-08-05", "2026-08-06"].iter().map(|d| posted_invoice(&mut app, &c, &old_mill, d, "850.00")).collect();
    let (path, p) = payment_form(&mut app, &c, &old_mill, &invoices[0], "850.00", "2026-08-12", None, &[]);
    redirect(submit_page(&mut app, &path, p));
    let receipt = receipt_ids(&mut app).pop().unwrap();
    let (bank, fees_account, other_income) = (account(&mut app, &c, "1000"), account(&mut app, &c, "6160"), account(&mut app, &c, "4900"));
    let said = |again: &str, amount: &str| format!(
        "{amount} on 6160 · Bank Service Charges is positive, so it will increase the deposit by {amount} and credit 6160 · Bank Service Charges, an expense account. A fee the bank or card processor kept is negative, such as -{amount}. If the amount is right, {again}; otherwise put a minus sign before it."
    );
    // Record deposit: the review says it at the fee lines. Record deposit without the review's confirmation
    // only shows the review again; with it, the line is recorded as typed.
    let path = url("deposits:new", &[&c]);
    let rk = value_of(&page(&mut app, &path), "request_key");
    let fields = |action: &str, extra: &[(&str, &str)]| {
        let mut p = pairs(&[("request_key", &rk), ("bank", &bank), ("date", "2026-08-12"), ("reference", ""), ("memo", ""), ("receipts", &receipt), ("fee_account", &fees_account), ("fee_amount", "24.95"), ("action", action)]);
        p.extend(pairs(extra));
        p
    };
    let Outcome::Page { html: review, status: 200 } = submit_page(&mut app, &path, fields("review", &[])) else { panic!("reviewed") };
    let at_lines = review.split_once("<fieldset class=\"fee-lines\"").unwrap().1.split("</fieldset>").next().unwrap().to_owned();
    let warning = said("choose Record deposit", "24.95");
    assert!(at_lines.contains(&format!(r#"<p class="notice fee-warning" role="alert" id="fee-warning" data-fee-warning><strong>Nothing was saved yet.</strong> {warning}</p>"#)), "{at_lines}");
    assert!(review.contains(r#"total <strong id="review-total">874.95</strong>"#), "the review shows what would be recorded: {review}");
    let ok = value_of(&review, "fee_ok");
    assert_eq!(ok, format!("{fees_account}=24.95"));
    let Outcome::Page { html: again, status: 200 } = submit_page(&mut app, &path, fields("record", &[("review_token", &value_of(&review, "review_token"))])) else { panic!("reviewed again") };
    assert!(again.contains(&warning) && deposits(&mut app).is_empty(), "not recorded without the confirmation: {again}");
    redirect(submit_page(&mut app, &path, fields("record", &[("review_token", &value_of(&again, "review_token")), ("fee_ok", &ok)])));
    let (_, journal, amount) = deposits(&mut app).pop().unwrap();
    assert_eq!(amount, 87_495);
    assert_eq!(sorted(lines(&mut app, &journal)), owned(&[("1000", 87_495, 0), ("1200", 0, 85_000), ("6160", 0, 2_495)]));
    // Receive payment straight to the bank: the first Save says it and saves nothing.
    let (path, p) = payment_form(&mut app, &c, &old_mill, &invoices[1], "850.00", "2026-08-13", Some(&bank), &[(&fees_account, "24.95")]);
    let Outcome::Invalid { html } = submit_page(&mut app, &path, p.clone()) else { panic!("warned") };
    let warning = said("choose Save payment again", "24.95");
    let at_lines = html.split_once("<fieldset class=\"fee-lines\"").unwrap().1.split("</fieldset>").next().unwrap().to_owned();
    assert!(at_lines.contains(&format!("<strong>Nothing was saved yet.</strong> {warning}</p>")), "{at_lines}");
    assert_eq!(receipt_ids(&mut app).len(), 1, "nothing saved");
    let ok = value_of(&html, "fee_ok");
    // Another amount with the old confirmation is checked again.
    let mut changed: Vec<(String, String)> = p.iter().map(|(k, v)| (k.clone(), if k == "fee_amount" { "30.00".into() } else { v.clone() })).collect();
    changed.push(("fee_ok".into(), ok.clone()));
    let Outcome::Invalid { html } = submit_page(&mut app, &path, changed) else { panic!("warned again") };
    assert!(html.contains(&said("choose Save payment again", "30.00")), "{html}");
    assert_eq!(receipt_ids(&mut app).len(), 1, "nothing saved");
    // Save payment again with the same lines: recorded as typed, 6160 credited.
    let mut confirmed = p.clone();
    confirmed.push(("fee_ok".into(), ok));
    let location = redirect(submit_page(&mut app, &path, confirmed));
    let shown = page(&mut app, &location);
    assert!(shown.contains("Saved RCP-000002: 850.00 applied to the invoices. Deposited to 1000 · Checking as DEP-000002: 874.95 (850.00 with fees and adjustments of 24.95)."), "{shown}");
    let journal: String = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT d.journal_id FROM banking_deposit d JOIN documents_documentidentity i ON i.id=d.identity_id WHERE i.number='DEP-000002'", [], |r| r.get(0))).unwrap();
    assert_eq!(sorted(lines(&mut app, &journal)), owned(&[("1000", 87_495, 0), ("1200", 0, 85_000), ("6160", 0, 2_495)]));
    // A positive line on an income account is saved at once.
    let (path, p) = payment_form(&mut app, &c, &old_mill, &invoices[2], "850.00", "2026-08-14", Some(&bank), &[(&other_income, "5.00")]);
    redirect(submit_page(&mut app, &path, p));
    trial_balance(&mut app, &c, &[("1000", 87_495 + 87_495 + 85_500), ("1100", 0), ("1200", 0), ("6160", -4_990), ("4900", -500), ("4000", -255_000)]);
}

/// Review 3b M3: account ids are compared in their standard form. On the card's split charge, a line or the
/// one category on the card itself spelled in upper case is refused like the card itself, and nothing is
/// posted; another account spelled so is recorded on that account. On a deposit, a fee line on the bank
/// spelled so is refused, and a fee account spelled so is recorded in its standard form.
#[test]
fn account_ids_are_compared_in_their_standard_form() {
    let mut f = Feeds::new();
    let co = fb_core::CompanyId::parse(&f.c).unwrap();
    let charge = f.tx_id("TX-0001");
    let (card, cogs) = (f.card.to_uppercase(), f.account("5000"));
    assert_ne!(card, f.card, "the spelling differs");
    let before = f.journals();
    let accept = |f: &mut Feeds, input: Value| -> Result<String, String> {
        let o = f.app.open_mut().unwrap();
        bf::accept(&mut o.book, o.owner, co, &charge, &input, &bf::SecretSet::none()).map(|i| i.status).map_err(|e| e.to_string())
    };
    let split_on_card = accept(&mut f, json!({"kind":"JOURNAL","lines":[{"account_id":cogs,"amount":"183.68","memo":""},{"account_id":card,"amount":"-20.00","memo":""}]}));
    assert!(split_on_card.as_ref().is_err_and(|e| e.contains("Choose an account other than the feed account itself on each split line.")), "{split_on_card:?}");
    let one_on_card = accept(&mut f, json!({"kind":"JOURNAL","account_id":card}));
    assert!(one_on_card.as_ref().is_err_and(|e| e.contains("Choose a category other than the feed account itself.")), "{one_on_card:?}");
    assert_eq!(f.journals(), before, "nothing posted");
    let pk = f.pk("TX-0001");
    assert_eq!(f.row(pk).0, "NEW");
    // 5000 spelled in upper case is 5000.
    assert_eq!(accept(&mut f, json!({"kind":"JOURNAL","account_id":cogs.to_uppercase()})), Ok("ACCEPTED".to_owned()));
    let card_code: String = { let card = f.card.clone(); f.app.open_mut().unwrap().book.read(|q| q.query_row("SELECT code FROM ledger_account WHERE id=?1", [&card], |r| r.get(0))).unwrap() };
    let journal = f.row(pk).1.unwrap();
    assert_eq!(lines(&mut f.app, &journal), vec![(card_code, 0, 16_368), ("5000".into(), 16_368, 0)]);
    // Deposits: a fee line on the bank spelled in upper case is the bank.
    let receipts = received(&mut f, &[("Old Mill Insurance Agency (TEST)", "850.00", "2026-08-12")]);
    let (bank, fees_account) = (f.account("1000"), f.account("6160"));
    let o = f.app.open_mut().unwrap();
    let input = |account: &str| json!({"request_key":key(),"bank_id":bank,"date":"2026-08-12","reference":"","memo":"","receipt_ids":[receipts[0]],"adjustments":[{"account_id":account,"amount":"-24.95"}]});
    let on_bank = fb_core::banking::review_deposit(&mut o.book, o.owner, co, &input(&bank.to_uppercase())).map(|_| ()).map_err(|e| e.to_string());
    assert!(on_bank.as_ref().is_err_and(|e| e.contains("A fee or adjustment cannot use the bank account the deposit goes to.")), "{on_bank:?}");
    // 6160 spelled so is recorded as 6160, in its standard form.
    let request = input(&fees_account.to_uppercase());
    let review = fb_core::banking::review_deposit(&mut o.book, o.owner, co, &request).unwrap();
    let recorded = fb_core::banking::record_deposit(&mut o.book, o.owner, co, &request, &review).unwrap();
    assert_eq!(recorded["adjustments"], json!([{"account_id":fees_account,"amount":"-24.95"}]));
    assert!(o.book.check().unwrap().is_empty());
}

/// Review 3b M8: a populated format-95 book with a reversed deposit, a payment deposited again after that
/// reversal, and that deposit in a finished reconciliation, is backed up and upgraded with every value
/// unchanged, passes every integrity check, still shows the reconciliation, and then takes a deposit with a fee.
#[test]
fn a_format_95_book_with_a_reversed_and_a_reconciled_deposit_upgrades_and_passes_its_checks() {
    use fb_db::{Book, Opened, SnapshotSource, test_support};
    let (mut app, c) = fixture(StarterChart::Standard);
    let old_mill = customer(&mut app, &c, "Old Mill Insurance Agency (TEST)");
    for (date, paid) in [("2026-08-04", "2026-08-12"), ("2026-09-09", "2026-09-16"), ("2026-10-01", "2026-10-05")] {
        let invoice = posted_invoice(&mut app, &c, &old_mill, date, "850.00");
        let (path, p) = payment_form(&mut app, &c, &old_mill, &invoice, "850.00", paid, None, &[]);
        redirect(submit_page(&mut app, &path, p));
    }
    let receipts = receipt_ids(&mut app);
    let (bank, fees_account) = (account(&mut app, &c, "1000"), account(&mut app, &c, "6160"));
    let co = fb_core::CompanyId::parse(&c).unwrap();
    let deposit = |book: &mut Book, owner, ids: &[&String], date: &str, adjustments: Value| -> Value {
        let mut input = json!({"request_key":key(),"bank_id":bank,"date":date,"reference":"","memo":"","receipt_ids":ids});
        if !adjustments.is_null() {
            input["adjustments"] = adjustments;
        }
        let review = fb_core::banking::review_deposit(book, owner, co, &input).unwrap();
        fb_core::banking::record_deposit(book, owner, co, &input, &review).unwrap()
    };
    let owner = app.open_mut().unwrap().owner;
    {
        let o = app.open_mut().unwrap();
        // The first payment is deposited and the deposit reversed, then deposited again with the second.
        let first = deposit(&mut o.book, owner, &[&receipts[0]], "2026-08-12", Value::Null);
        fb_core::banking::reverse_deposit(&mut o.book, owner, co, first["id"].as_str().unwrap(), Some(1), &json!("2026-08-13"), &json!("Synthetic test: slip lost")).unwrap();
        let second = deposit(&mut o.book, owner, &[&receipts[0], &receipts[1]], "2026-09-16", Value::Null);
        assert_eq!(second["amount"], "1700.00");
        // The second deposit clears and is reconciled on the September statement.
        let line: String = o.book.read(|q| q.query_row("SELECT id FROM ledger_journalline WHERE journal_id=?1 AND account_id=?2", [second["journal_id"].as_str().unwrap(), &bank], |r| r.get(0))).unwrap();
        fb_core::banking::set_cleared(&mut o.book, owner, co, &line, "C", Some(0), None).unwrap();
        let rec = fb_core::banking::begin(&mut o.book, owner, co, &bank, &json!({"request_key":key(),"statement_date":"2026-09-30","ending_balance":"1700.00"})).unwrap();
        let done = fb_core::banking::reconcile_now(&mut o.book, owner, co, rec["id"].as_str().unwrap(), &bank, rec["revision"].as_i64(), &json!(key())).unwrap();
        assert_ne!(done["status"], "OPEN", "{done}");
        assert!(o.book.check().unwrap().is_empty());
    }
    assert_eq!(count(&mut app, "SELECT count(*) FROM banking_depositreversal"), 1);
    assert_eq!(count(&mut app, "SELECT count(*) FROM banking_reconciliation WHERE status<>'OPEN'"), 1);
    assert_eq!(count(&mut app, "SELECT count(*) FROM banking_banklinestatus WHERE state='R'"), 1);
    let dir = temp_dir();
    let key_bytes = [95; 32];
    let old_path = dir.join("format-95-reconciled.fbbook");
    let copy = test_support::populated_copy(app.open_mut().unwrap().book.connection(), &old_path, &key_bytes, "0208_ai_unattended", true).unwrap();
    let Opened::Behind(old) = Book::open_staged(&old_path, &key_bytes).unwrap() else { panic!("expected a format-95 book") };
    assert_eq!((old.from(), old.to()), (95, 97));
    let backup = dir.join("verified-format-95-reconciled.fbbook");
    old.snapshot(&backup, &key_bytes).unwrap();
    assert_eq!(test_support::snapshot_file(&backup, &key_bytes).unwrap(), copy.before, "the backup is the book as it was");
    let mut upgraded = old.upgrade().unwrap();
    assert_eq!(test_support::snapshot(upgraded.connection()).unwrap(), copy.expected, "every earlier value is unchanged");
    assert!(upgraded.check().unwrap().is_empty(), "{:?}", upgraded.check().unwrap());
    let reconciled: i64 = upgraded.read(|q| q.query_row("SELECT count(*) FROM banking_banklinestatus WHERE state='R'", [], |r| r.get(0))).unwrap();
    assert_eq!(reconciled, 1, "the reconciled line is still reconciled");
    // The upgraded book takes a deposit net of a fee, and still passes its checks after it and on reopening.
    let recorded = deposit(&mut upgraded, owner, &[&receipts[2]], "2026-10-05", json!([{"account_id":fees_account,"amount":"-24.95"}]));
    assert_eq!(recorded["amount"], "825.05");
    assert!(upgraded.check().unwrap().is_empty());
    drop(upgraded);
    let reopened = Book::open(&old_path, &key_bytes).unwrap();
    assert!(reopened.check().unwrap().is_empty());
}

/// Review 3b M8: one payment cannot be deposited from two Review rows. The 850.00 payment is in the window of
/// both the payout row (825.05) and the next deposit row (575.50), so both list it. Ticked on the payout row
/// it is deposited; ticked again on the other row from the same page (with fee lines that make the total),
/// it is refused and nothing is posted. That row comes back with the refusal at "Record as", which no longer
/// offers "Deposit received payments": no payment is left to deposit.
#[test]
fn one_payment_cannot_be_deposited_from_two_review_rows() {
    let mut f = Feeds::new();
    let receipts = received(&mut f, &[("Old Mill Insurance Agency (TEST)", "850.00", "2026-08-11")]);
    let (payout, other) = (f.pk("TX-0057"), f.pk("TX-0064"));
    let fees_account = f.account("6160");
    let page = f.page();
    for pk in [payout, other] {
        let row = row_of(&page, pk);
        assert!(row.split_once("data-deposit ").is_some_and(|(_, panel)| panel.contains(&format!(r#"value="{}" data-amount="850.00""#, receipts[0]))), "listed on both rows: {row}");
    }
    let tick = |pk: i64, fee: &str| vec![(format!("receipt_{pk}"), receipts[0].clone()), (format!("fee_account_{pk}"), fees_account.clone()), (format!("fee_amount_{pk}"), fee.to_owned()), button("accept_one", pk)];
    f.post(&page, &set(payout, &[("kind", "DEPOSIT")]), &tick(payout, "-24.95"));
    assert_eq!(f.row(payout).0, "ACCEPTED");
    let before = (f.journals(), deposits(&mut f.app).len());
    // The same page, the other row: refused.
    let data = f.post(&page, &set(other, &[("kind", "DEPOSIT")]), &tick(other, "-274.50"));
    let row = fragment(&data, &format!("review-row-{other}"));
    assert!(row.contains(r#"<option value="DEPOSIT" selected>Unavailable choice — choose again</option></select><span class="row-error-text" role="alert">RCP-000001 already belongs to a live deposit.</span>"#), "{row}");
    assert_eq!(row.matches("already belongs to a live deposit").count(), 1, "said once: {row}");
    assert!(!row.contains("data-deposit "), "nothing left to deposit on this row: {row}");
    assert_eq!((f.journals(), deposits(&mut f.app).len()), before, "nothing posted");
    assert_eq!(f.row(other).0, "NEW");
    let c = f.c.clone();
    trial_balance(&mut f.app, &c, &[("1000", 82_505), ("1200", 0), ("6160", 2_495)]);
}
