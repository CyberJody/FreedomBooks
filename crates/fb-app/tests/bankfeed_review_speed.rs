//! Review transactions (review 2026-11-10): in-place row actions, "Already recorded" after a replaced
//! connection, Undo, the sync-gap and bank-change notices, and what each row shows. Synthetic data only.
use fb_app::{App, Command, Outcome, keys::MasterKey, screens};
use fb_core::{AccountId, CompanyId, bankfeeds as bf, services};
use serde_json::{Value, json};
mod support;

struct Fx {
    _dir: tempfile::TempDir,
    app: App,
    co: CompanyId,
    path: String,
    office: String,
    bank: String,
    conn: String,
}
fn tx(id: &str, date: &str, amount: i64, description: &str) -> bf::StagedTransaction {
    bf::StagedTransaction { remote_id: id.into(), posted_on: Some(date.into()), transacted_on: None, amount, description: description.into(), pending: false }
}
fn pull(transactions: Vec<bf::StagedTransaction>) -> bf::StagedPull {
    bf::StagedPull {
        accounts: vec![bf::StagedAccount {
            conn_key: "synthetic".into(),
            remote_id: "Checking".into(),
            name: "Checking".into(),
            org_name: "Synthetic bank".into(),
            currency: "USD".into(),
            balance: None,
            available: None,
            balance_epoch: None,
            skipped: vec![],
            transactions,
        }],
        ..Default::default()
    }
}
fn downloads() -> Vec<bf::StagedTransaction> {
    vec![
        tx("T-1", "2026-11-02", -8419, "OFFICE DEPOT #221"),
        tx("T-2", "2026-11-03", -120_000, "CHECK 1041"),
        tx("T-3", "2026-11-04", -5900, "ACME INTERNET AUTOPAY"),
        tx("T-4", "2026-11-05", 31_200, "CLIENT PAYMENT"),
    ]
}
impl Fx {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new();
        app.simplefin = true;
        let co = app.create_book(&dir.path().join("synthetic.fbbook"), &MasterKey::new([71; 32]), "Review actor", "Synthetic speed").unwrap().company_id;
        support::sign_in(&mut app);
        let o = app.open_mut().unwrap();
        let office = services::save_account(&mut o.book, o.owner, co, services::AccountInput::new("6070", "Office").detail("EXPENSE")).unwrap().id.to_db();
        services::save_account(&mut o.book, o.owner, co, services::AccountInput::new("4000", "Sales").kind("INCOME").detail("INCOME")).unwrap();
        let conn = bf::connect(&mut o.book, o.owner, co, &json!("Synthetic downloads"), "bridge.simplefin.org", &bf::SecretSet::none()).unwrap()["id"].as_str().unwrap().to_owned();
        bf::store_pull(&mut o.book, o.owner, co, &conn, &pull(downloads()), "2026-11-07T12:00:00Z", false, None, &bf::SecretSet::none()).unwrap();
        let feed = feed_of(&mut o.book, &conn);
        bf::save_mappings(&mut o.book, o.owner, co, &conn, &[(feed, bf::MappingTarget::New { name: "Ledger checking".into(), kind: bf::NewAccountKind::Checking })], &bf::SecretSet::none()).unwrap();
        let bank = o.book.read(|q| q.query_row("SELECT ledger_account_id FROM bankfeeds_account", [], |r| r.get(0))).unwrap();
        Self { _dir: dir, app, co, path: format!("/c/{co}/bank-feeds/review/"), office, bank, conn }
    }
    fn ordinal(&mut self, conn: &str, remote: &str) -> i64 {
        self.app
            .open_mut()
            .unwrap()
            .book
            .read(|q| q.query_row("SELECT t.ordinal FROM bankfeeds_transaction t JOIN bankfeeds_account f ON f.id=t.feed_account_id WHERE f.connection_id=?1 AND t.remote_id=?2", [conn, remote], |r| r.get(0)))
            .unwrap()
    }
    fn render(&mut self, query: &str) -> String {
        screens::render(&mut self.app, &format!("{}{query}", self.path)).unwrap().html
    }
    fn submit(&mut self, path: &str, fields: Vec<(&str, String)>) -> Outcome {
        let token = self.app.tokens.mint(path);
        self.app
            .run_renderer(Command::SubmitForm { path: path.into(), token, fields: fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect() })
            .unwrap()
    }
    /// A post as the page's script sends it: `inline=1`, the row's drafts, one button.
    fn inline(&mut self, pk: i64, button: &str, extra: Vec<(&str, String)>) -> Value {
        // The row's own fields first: `extra` replaces a default draft value.
        let mut fields = extra;
        for (k, v) in [("kind_PK", "CHECK".to_owned()), ("account_PK", self.office.clone())] {
            if !fields.iter().any(|(name, _)| *name == k) {
                fields.push((k, v));
            }
        }
        fields.extend([("inline", "1".to_owned()), ("review_layout", "new".to_owned()), (button, pk.to_string())]);
        let renamed: Vec<(String, String)> = fields.into_iter().map(|(k, v)| (k.replace("PK", &pk.to_string()), v)).collect();
        let token = self.app.tokens.mint(&self.path);
        match self.app.run_renderer(Command::SubmitForm { path: self.path.clone(), token, fields: renamed }).unwrap() {
            Outcome::Done { data } => data,
            other => panic!("expected an in-place reply, got {other:?}"),
        }
    }
    fn count(&mut self, sql: &str) -> i64 {
        self.app.open_mut().unwrap().book.read(|q| q.query_row(sql, [], |r| r.get(0))).unwrap()
    }
}
fn feed_of(book: &mut fb_core::Book, conn: &str) -> String {
    book.read(|q| q.query_row("SELECT id FROM bankfeeds_account WHERE connection_id=?1", [conn], |r| r.get(0))).unwrap()
}
fn fragment<'a>(data: &'a Value, id: &str) -> &'a str {
    data["fragments"].as_array().unwrap().iter().find(|f| f["id"] == id).unwrap_or_else(|| panic!("no fragment {id}: {data}"))["html"].as_str().unwrap()
}
fn row(html: &str, pk: i64) -> &str {
    html.split_once(&format!("id=\"review-row-{pk}\"")).unwrap().1.split_once("</tr>").unwrap().0
}
/// The earlier transaction a rendered row says it looks already recorded as (its `link_to_<pk>` field).
fn shown_as(html: &str, pk: i64) -> String {
    row(html, pk).split_once(&format!("name=\"link_to_{pk}\" value=\"")).unwrap().1.split_once('"').unwrap().0.to_owned()
}
/// A bulk post as the page sends it: each selected row's drafts and what it showed as already recorded.
fn bulk_fields(action: &str, rows: &[(i64, Option<String>)], office: &str, extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut fields: Vec<(String, String)> = [("inline", "1"), ("review_layout", "new"), ("action", action)].iter().chain(extra).map(|(k, v)| (k.to_string(), v.to_string())).collect();
    for (pk, shown) in rows {
        fields.extend([("sel".to_owned(), pk.to_string()), (format!("kind_{pk}"), "CHECK".to_owned()), (format!("account_{pk}"), office.to_owned())]);
        if let Some(earlier) = shown {
            fields.push((format!("link_to_{pk}"), earlier.clone()));
        }
    }
    fields
}

/// B1: accepting one same-sync fee must not make its identical peers look changed or recorded.
#[test]
fn followup_b1_identical_same_sync_fees_all_accept_in_one_bulk_pass() {
    for include_recorded in [false, true] {
        let mut f = Fx::new();
        let conn = f.conn.clone();
        let o = f.app.open_mut().unwrap();
        let mut d = downloads();
        for n in 0..4 {
            d.push(tx(&format!("FEE-{n}"), "2026-11-06", -300, "MONTHLY SERVICE FEE"));
        }
        bf::store_pull(&mut o.book, o.owner, f.co, &conn, &pull(d), "2026-11-08T12:00:00Z", false, None, &bf::SecretSet::none()).unwrap();
        let fees: Vec<_> = (0..4).map(|n| f.ordinal(&conn, &format!("FEE-{n}"))).collect();
        let page = f.render("");
        for pk in &fees {
            assert!(!row(&page, *pk).contains("Already recorded"));
        }
        let posted = f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'");
        let rows: Vec<_> = fees.iter().map(|pk| (*pk, None)).collect();
        let extra = if include_recorded { vec![("include_recorded", "1")] } else { vec![] };
        let fields = bulk_fields("accept", &rows, &f.office, &extra);
        let token = f.app.tokens.mint(&f.path);
        let Outcome::Done { data } = f.app.run_renderer(Command::SubmitForm { path: f.path.clone(), token, fields }).unwrap() else { panic!() };
        assert_eq!(f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'"), posted + 4, "{data}");
        let status = fragment(&data, "review-status");
        assert!(!status.contains("changed since this page was shown") && !status.contains("already recorded"), "{status}");
        for pk in &fees {
            assert!(fragment(&data, &format!("review-row-{pk}")).contains("data-status=\"ACCEPTED\""));
        }
        assert_eq!(f.count("SELECT count(*) FROM ledger_auditevent WHERE action='bankfeed.recorded_again'"), 0);
    }
}

#[test]
fn followup_same_account_later_sync_keeps_ignore_secondary() {
    let mut f = Fx::new();
    let conn = f.conn.clone();
    let first = f.ordinal(&conn, "T-1");
    f.inline(first, "accept_one", vec![]);
    let o = f.app.open_mut().unwrap();
    bf::store_pull(&mut o.book, o.owner, f.co, &conn, &pull(vec![tx("T-1-AGAIN", "2026-11-02", -8419, "OFFICE DEPOT #221")]), "2026-11-08T12:00:00Z", false, None, &bf::SecretSet::none()).unwrap();
    let copy = f.ordinal(&conn, "T-1-AGAIN");
    let page = f.render("");
    let html = row(&page, copy);
    assert!(html.contains("Already recorded") && html.contains("name=\"ignore_one\"") && html.contains("name=\"accept_again_one\""), "{html}");
    assert!(html.contains("class=\"primary\" data-primary data-confirm="), "{html}");
    assert!(!html.contains("data-primary data-ignore"), "{html}");
}

#[test]
fn followup_n3_matched_transfer_names_the_other_row_without_a_manual_void_hint() {
    let mut f = Fx::new();
    let conn = f.conn.clone();
    let from = f.ordinal(&conn, "T-2");
    let o = f.app.open_mut().unwrap();
    let mut p = pull(downloads());
    let mut savings = p.accounts[0].clone();
    savings.remote_id = "Savings".into();
    savings.name = "Savings".into();
    savings.transactions = vec![tx("SAVINGS-IN", "2026-11-03", 120_000, "TRANSFER IN")];
    p.accounts.push(savings);
    bf::store_pull(&mut o.book, o.owner, f.co, &conn, &p, "2026-11-08T12:00:00Z", false, None, &bf::SecretSet::none()).unwrap();
    let feed: String = o.book.read(|q| q.query_row("SELECT id FROM bankfeeds_account WHERE name='Savings'", [], |r| r.get(0))).unwrap();
    bf::save_mappings(&mut o.book, o.owner, f.co, &conn, &[(feed.clone(), bf::MappingTarget::New { name: "Ledger savings".into(), kind: bf::NewAccountKind::Savings })], &bf::SecretSet::none()).unwrap();
    let bank = o.book.read(|q| bf::feed(q, &feed)).unwrap().unwrap().ledger_account_id.unwrap();
    let id: String = o.book.read(|q| q.query_row("SELECT id FROM bankfeeds_transaction WHERE ordinal=?1", [from], |r| r.get(0))).unwrap();
    bf::accept(&mut o.book, o.owner, f.co, &id, &json!({"kind":"TRANSFER","transfer_account_id":bank}), &bf::SecretSet::none()).unwrap();
    let id: String = o.book.read(|q| q.query_row("SELECT id FROM bankfeeds_transaction WHERE remote_id='SAVINGS-IN'", [], |r| r.get(0))).unwrap();
    let suggestion = bf::suggest(&mut o.book, o.owner, f.co, &id).unwrap();
    bf::accept(&mut o.book, o.owner, f.co, &id, &json!({"kind":"MATCH","journal_line_id":suggestion.matches[0].line_id}), &bf::SecretSet::none()).unwrap();
    let page = f.render("?status=ACCEPTED");
    let html = row(&page, from);
    assert!(html.contains("Savings 2026-11-03 TRANSFER IN") && html.contains("Return that one to review first"), "{html}");
    assert!(!html.contains("void or reverse") && !html.contains(">Undo</button>"), "{html}");
    let posted = f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'");
    let data = f.inline(from, "undo_confirm_one", vec![]);
    assert!(fragment(&data, "review-status").contains("Return that one to review first"));
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'"), posted);
    let matched = f.ordinal(&conn, "SAVINGS-IN");
    f.inline(matched, "return_one", vec![]);
    assert!(row(&f.render("?status=ACCEPTED"), from).contains(">Undo</button>"));
}
/// The owner's reconnect: the old connection's feed is unmapped and it is forgotten; a new connection downloads
/// `again` and is mapped to the same ledger account through the accounts page. Returns the new connection.
fn reconnect(f: &mut Fx, again: Vec<bf::StagedTransaction>) -> String {
    let old = f.conn.clone();
    let o = f.app.open_mut().unwrap();
    let old_feed = feed_of(&mut o.book, &old);
    bf::map_account(&mut o.book, o.owner, f.co, &old_feed, &Value::Null, &bf::SecretSet::none()).unwrap();
    bf::forget(&mut o.book, o.owner, f.co, &old, &bf::SecretSet::none()).unwrap();
    let new = bf::connect(&mut o.book, o.owner, f.co, &json!("Again"), "bridge.simplefin.org", &bf::SecretSet::none()).unwrap()["id"].as_str().unwrap().to_owned();
    bf::store_pull(&mut o.book, o.owner, f.co, &new, &pull(again), "2026-11-10T12:00:00Z", false, None, &bf::SecretSet::none()).unwrap();
    let new_feed = feed_of(&mut o.book, &new);
    let bank = AccountId::parse(&f.bank).unwrap();
    let accounts = format!("/c/{}/bank-feeds/{new}/accounts/", f.co);
    assert!(matches!(f.submit(&accounts, vec![(&format!("map_{new_feed}"), bank.to_db())]), Outcome::Redirect { .. }));
    new
}

#[test]
fn accept_and_ignore_reply_in_place_with_the_row_outcome_and_a_fresh_token() {
    let mut f = Fx::new();
    let conn = f.conn.clone();
    let pk = f.ordinal(&conn, "T-1");
    let data = f.inline(pk, "accept_one", vec![]);
    let html = fragment(&data, &format!("review-row-{pk}"));
    assert!(html.starts_with(&format!("<tr id=\"review-row-{pk}\" data-fragment")), "{html}");
    assert!(html.contains("data-status=\"ACCEPTED\"") && html.contains("Accepted: <a href=") && html.contains(">Undo</button>"), "{html}");
    assert!(fragment(&data, "review-status").contains("3 to review"));
    assert!(fragment(&data, "review-status").contains("recorded as CHK-"));
    // The next token is bound to the review path and works once.
    let token = data["next_token"].as_str().unwrap().to_owned();
    assert!(f.app.tokens.valid(&token, &f.path));
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'"), 1);
    // A refusal shows on the row and posts nothing.
    let other = f.ordinal(&conn, "T-3");
    let data = f.inline(other, "accept_one", vec![("account_PK", String::new())]);
    let html = fragment(&data, &format!("review-row-{other}"));
    assert!(html.contains("row-error") && html.contains("data-status=\"NEW\""), "{html}");
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'"), 1);
    let data = f.inline(other, "ignore_one", vec![]);
    assert!(fragment(&data, &format!("review-row-{other}")).contains("Ignored"));
}

#[test]
fn undo_asks_once_then_voids_and_returns_the_row_in_one_step() {
    let mut f = Fx::new();
    let conn = f.conn.clone();
    let pk = f.ordinal(&conn, "T-1");
    f.inline(pk, "accept_one", vec![]);
    let ask = f.inline(pk, "undo_one", vec![]);
    let html = fragment(&ask, &format!("review-row-{pk}"));
    assert!(html.contains("Undo voids CHK-") && html.contains("Confirm undo") && html.contains("Keep it"), "{html}");
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal WHERE reversal_of_id IS NOT NULL"), 0, "asking writes nothing");
    let done = f.inline(pk, "undo_confirm_one", vec![]);
    let html = fragment(&done, &format!("review-row-{pk}"));
    assert!(html.contains("data-status=\"NEW\""), "{html}");
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal WHERE reversal_of_id IS NOT NULL"), 1);
    assert_eq!(f.count("SELECT count(*) FROM ledger_auditevent WHERE action='check.voided'"), 1);
    // The Accepted filter offers Undo too; a voided-by-hand document keeps the manual path.
    f.inline(pk, "accept_one", vec![]);
    let accepted = f.render("?status=ACCEPTED");
    assert!(row(&accepted, pk).contains(">Undo</button>"));
}

#[test]
fn a_replaced_connection_shows_already_recorded_rows_links_them_and_bulk_accept_skips_them() {
    let mut f = Fx::new();
    let old = f.conn.clone();
    let first = f.ordinal(&old, "T-1");
    let second = f.ordinal(&old, "T-3");
    f.inline(first, "accept_one", vec![]);
    f.inline(second, "accept_one", vec![]);
    // Reinstall: the old connection is unmapped and forgotten; a new one downloads the same days again.
    let o = f.app.open_mut().unwrap();
    let old_feed = feed_of(&mut o.book, &old);
    bf::map_account(&mut o.book, o.owner, f.co, &old_feed, &Value::Null, &bf::SecretSet::none()).unwrap();
    bf::forget(&mut o.book, o.owner, f.co, &old, &bf::SecretSet::none()).unwrap();
    let new = bf::connect(&mut o.book, o.owner, f.co, &json!("Again"), "bridge.simplefin.org", &bf::SecretSet::none()).unwrap()["id"].as_str().unwrap().to_owned();
    bf::store_pull(&mut o.book, o.owner, f.co, &new, &pull(downloads()), "2026-11-10T12:00:00Z", false, None, &bf::SecretSet::none()).unwrap();
    let new_feed = feed_of(&mut o.book, &new);
    let bank = AccountId::parse(&f.bank).unwrap();
    // Mapping through the accounts page says how many look already recorded.
    let accounts = format!("/c/{}/bank-feeds/{new}/accounts/", f.co);
    let outcome = f.submit(&accounts, vec![(&format!("map_{new_feed}"), bank.to_db())]);
    assert!(matches!(outcome, Outcome::Redirect { .. }), "{outcome:?}");
    let list = screens::render(&mut f.app, &format!("/c/{}/bank-feeds/", f.co)).unwrap().html;
    assert!(list.contains("2 downloaded transactions look already recorded from an earlier bank connection"), "{list}");
    let page = f.render("");
    let again = f.ordinal(&new, "T-1");
    let also = f.ordinal(&new, "T-3");
    let fresh = f.ordinal(&new, "T-4");
    assert!(page.contains("2 look already recorded"));
    let html = row(&page, again);
    assert!(html.contains("Already recorded</strong> as <a href=") && html.contains("same bank transaction"), "{html}");
    assert!(html.contains("name=\"link_one\"") && html.contains("name=\"accept_again_one\"") && !html.contains("name=\"accept_one\""), "{html}");
    assert!(!row(&page, fresh).contains("Already recorded"));
    let posted = f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'");
    // Bulk accept skips them by default and says how many.
    let token = f.app.tokens.mint(&f.path);
    let fields = bulk_fields("accept", &[(again, Some(shown_as(&page, again))), (also, Some(shown_as(&page, also)))], &f.office, &[]);
    let Outcome::Done { data } = f.app.run_renderer(Command::SubmitForm { path: f.path.clone(), token, fields }).unwrap() else { panic!() };
    assert!(fragment(&data, "review-status").contains("Skipped 2 selected rows that look already recorded"), "{data}");
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'"), posted);
    // Link to existing marks one handled without posting; it shows with the matched rows.
    let data = f.inline(again, "link_one", vec![]);
    let html = fragment(&data, &format!("review-row-{again}"));
    assert!(html.contains("Already recorded: <a href=") && html.contains("linked; nothing posted"), "{html}");
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'"), posted);
    assert!(f.render("?status=MATCHED").contains(&format!("id=\"review-row-{again}\"")));
    assert!(!f.render("?status=IGNORED").contains(&format!("id=\"review-row-{again}\"")));
    // Recording one again is its own explicit action.
    let shown = shown_as(&page, also);
    let data = f.inline(also, "accept_again_one", vec![("link_to_PK", shown)]);
    assert!(fragment(&data, &format!("review-row-{also}")).contains("data-status=\"ACCEPTED\""));
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'"), posted + 1);
    assert_eq!(f.count("SELECT count(*) FROM ledger_auditevent WHERE action='bankfeed.recorded_again'"), 1);
}

#[test]
fn rows_show_only_the_controls_their_record_as_needs_and_no_invented_check_numbers() {
    let mut f = Fx::new();
    let conn = f.conn.clone();
    let page = f.render("");
    let debit = row(&page, f.ordinal(&conn, "T-1"));
    assert!(debit.contains("placeholder=\"Check no. (optional)\"") && !debit.contains("EFT-"), "{debit}");
    assert!(debit.contains("data-combobox"));
    let check = row(&page, f.ordinal(&conn, "T-2"));
    assert!(check.contains("value=\"1041\""), "a real check number from the bank's text is kept");
    // Money in records as a journal: no check number at all, payee and category shown.
    let deposit = row(&page, f.ordinal(&conn, "T-4"));
    assert!(!deposit.contains("number_"));
    assert!(deposit.contains("data-kinds=\"CHECK JOURNAL\">"), "{deposit}");
    // The key legend, the hidden selection bar and the corrected empty state.
    assert!(page.contains("id=\"review-keys\"") && page.contains("<kbd>Enter</kbd> accept"));
    assert!(page.contains("id=\"review-bulk\" class=\"bulk-bar\" aria-label=\"Selected transactions\" hidden>"));
    let rules = screens::render(&mut f.app, &format!("/c/{}/bank-feeds/rules/", f.co)).unwrap().html;
    assert!(rules.contains("Use “Create rule…” on a transaction in Review transactions"));
    // A debit accepted with the number left blank still gets the core's EFT number, as before.
    let pk = f.ordinal(&conn, "T-1");
    f.inline(pk, "accept_one", vec![("number_PK", String::new())]);
    assert!(f.count("SELECT count(*) FROM checks_checkrevision WHERE check_number LIKE 'EFT-%'") >= 1);
}

#[test]
fn the_bank_feeds_page_names_missing_dates_until_reviewed_and_bank_changes_can_be_kept() {
    let mut f = Fx::new();
    let conn = f.conn.clone();
    let pk = f.ordinal(&conn, "T-1");
    f.inline(pk, "accept_one", vec![]);
    let o = f.app.open_mut().unwrap();
    let start = bf::begin_sync(&mut o.book, o.owner, f.co, &conn, "2027-03-01T12:00:00Z", 0, true).unwrap();
    let mut revised = downloads();
    revised[0].amount = -9419;
    bf::store_pull(&mut o.book, o.owner, f.co, &conn, &pull(revised), "2027-03-01T12:00:00Z", false, Some(start), &bf::SecretSet::none()).unwrap();
    let list = screens::render(&mut f.app, &format!("/c/{}/bank-feeds/", f.co)).unwrap().html;
    assert!(list.contains("Some bank transactions may be missing from Synthetic downloads") && list.contains("Transactions dated 2026-10-31 to 2026-12-01"), "{list}");
    let sync = format!("/c/{}/bank-feeds/{conn}/sync/", f.co);
    assert!(matches!(f.submit(&sync, vec![("action", "gap_reviewed".into())]), Outcome::Redirect { .. }));
    assert!(!screens::render(&mut f.app, &format!("/c/{}/bank-feeds/", f.co)).unwrap().html.contains("Some bank transactions may be missing"));
    let page = f.render("");
    assert!(page.contains("1 reviewed transaction was changed by the bank") && page.contains("Undo it (or void the entry from its page) and accept it again") && page.contains("recorded <span class=\"num\">-84.19</span>, the bank now says <span class=\"num\">-94.19</span>"), "{page}");
    let data = f.inline(pk, "keep_one", vec![]);
    assert!(!fragment(&data, "review-status").contains("changed by the bank"));
    assert_eq!(f.count("SELECT count(*) FROM ledger_auditevent WHERE action='bankfeed.bank_change_kept'"), 1);
}

/// Review probe A1 (finding 6): "Also record rows that look already recorded" records exactly the selected rows
/// the page showed as already recorded, and its confirmation never reaches a row that became "already
/// recorded" after the page was drawn.
#[test]
fn bulk_also_record_confirms_only_the_rows_the_page_showed_as_already_recorded() {
    let mut f = Fx::new();
    let old = f.conn.clone();
    let first = f.ordinal(&old, "T-1");
    f.inline(first, "accept_one", vec![]);
    let new = reconnect(&mut f, downloads());
    // A separate sync reissues T-3. Its original is still unreviewed when this page is drawn.
    let o = f.app.open_mut().unwrap();
    bf::store_pull(&mut o.book, o.owner, f.co, &new, &pull(vec![tx("T-3-REISSUED", "2026-11-04", -5900, "ACME INTERNET AUTOPAY")]), "2026-11-11T12:00:00Z", false, None, &bf::SecretSet::none()).unwrap();
    let page = f.render("");
    let (t1, t3, t3b) = (f.ordinal(&new, "T-1"), f.ordinal(&new, "T-3"), f.ordinal(&new, "T-3-REISSUED"));
    assert!(row(&page, t1).contains("Already recorded</strong>"));
    assert!(!row(&page, t3b).contains("Already recorded"), "nothing of T-3 is recorded when the page is drawn");
    // After the page was drawn, T-3 is accepted elsewhere: its reissued copy now looks already recorded.
    f.inline(t3, "accept_one", vec![]);
    let posted = f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'");
    let token = f.app.tokens.mint(&f.path);
    let fields = bulk_fields("accept", &[(t1, Some(shown_as(&page, t1))), (t3b, None)], &f.office, &[("include_recorded", "1")]);
    let Outcome::Done { data } = f.app.run_renderer(Command::SubmitForm { path: f.path.clone(), token, fields }).unwrap() else { panic!() };
    let status = fragment(&data, "review-status");
    assert!(status.contains("recorded as CHK-"), "{status}");
    assert!(status.contains("1 selected row changed since this page was shown"), "{status}");
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'"), posted + 1, "only the row shown as already recorded");
    assert_eq!(f.count("SELECT count(*) FROM ledger_auditevent WHERE action='bankfeed.recorded_again'"), 1);
    let html = fragment(&data, &format!("review-row-{t3b}"));
    assert!(html.contains("data-status=\"NEW\"") && html.contains("now looks already recorded"), "{html}");
    // Shown now as a second copy from a later sync: Record again first, Ignore secondary, no Link.
    let page = f.render("");
    let html = row(&page, t3b);
    assert!(html.contains("on this bank account") && html.contains("name=\"accept_again_one\"") && !html.contains("name=\"link_one\""), "{html}");
}

/// Review probes A2 and A3 (found sound; regressions): bank text in a fragment is escaped, and the next
/// token is bound to the review path only.
#[test]
fn fragments_escape_bank_text_and_the_next_token_is_bound_to_the_review_path() {
    let mut f = Fx::new();
    let conn = f.conn.clone();
    let o = f.app.open_mut().unwrap();
    let mut d = downloads();
    d.push(tx("T-9", "2026-11-06", -777, "<img src=x onerror=alert(1)></td></tr><tr id=\"review-status\" data-fragment><form>"));
    bf::store_pull(&mut o.book, o.owner, f.co, &conn, &pull(d), "2026-11-08T12:00:00Z", false, None, &bf::SecretSet::none()).unwrap();
    let evil = f.ordinal(&conn, "T-9");
    let data = f.inline(evil, "ignore_one", vec![]);
    let html = fragment(&data, &format!("review-row-{evil}"));
    assert!(!html.contains("<img src=x") && html.contains("&lt;img") && html.matches("<tr ").count() == 1, "{html}");
    let next = data["next_token"].as_str().unwrap().to_owned();
    assert!(!f.app.tokens.valid(&next, &format!("/c/{}/bank-feeds/", f.co)));
    assert!(f.app.tokens.valid(&next, &f.path));
}

/// Findings 1 and 7: after a reconnect, the old connection's accepted row a new copy is linked to offers no
/// Undo and says why; its never-reviewed rows say they cannot be recorded, offer only Ignore, and the copies
/// the new connection downloaded again can be ignored in one press.
#[test]
fn after_a_reconnect_the_old_rows_say_what_can_be_done_with_them() {
    let mut f = Fx::new();
    let old = f.conn.clone();
    let first = f.ordinal(&old, "T-1");
    f.inline(first, "accept_one", vec![]);
    // The new connection downloads T-1 to T-3 again, not T-4.
    let new = reconnect(&mut f, downloads().into_iter().take(3).collect());
    let again = f.ordinal(&new, "T-1");
    f.inline(again, "link_one", vec![]);
    let accepted = f.render("?status=ACCEPTED");
    let html = row(&accepted, first);
    assert!(!html.contains(">Undo</button>") && html.contains("Undo not available: the copy downloaded again") && html.contains("Return that copy to review first"), "{html}");
    assert!(!html.contains("void or reverse"), "{html}");
    // Pressing Undo anyway (a stale page) is refused on the row; nothing is voided.
    let data = f.inline(first, "undo_confirm_one", vec![]);
    assert!(fragment(&data, "review-status").contains("cannot be undone here"), "{data}");
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal WHERE reversal_of_id IS NOT NULL"), 0);
    // Never-reviewed old rows: T-2 and T-3 were downloaded again, T-4 was not.
    let page = f.render("");
    assert!(page.contains("(3 from a disconnected bank connection)") && page.contains("Ignore 2 old copies downloaded again"), "{page}");
    let (t2, t4) = (f.ordinal(&old, "T-2"), f.ordinal(&old, "T-4"));
    assert!(row(&page, t2).contains("The current connection downloaded it again") && !row(&page, t2).contains("name=\"accept_one\""));
    assert!(row(&page, t4).contains("Not downloaded again: if the books need it, enter it by hand"));
    let posted = f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'");
    let token = f.app.tokens.mint(&f.path);
    let fields = vec![("inline", "1"), ("review_layout", "new"), ("ignore_copies", "all")].into_iter().map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
    let Outcome::Done { data } = f.app.run_renderer(Command::SubmitForm { path: f.path.clone(), token, fields }).unwrap() else { panic!() };
    assert!(fragment(&data, "review-status").contains("2 old copies from a disconnected bank connection ignored"), "{data}");
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'"), posted);
    let page = f.render("");
    assert!(page.contains("(1 from a disconnected bank connection)") && !page.contains("old copies downloaded again"), "{page}");
    // Voided by hand, the old entry leaves the linked copy saying so, reachable from the notice.
    let check: String = f.app.open_mut().unwrap().book.read(|q| q.query_row("SELECT reference_url FROM bankfeeds_transaction WHERE ordinal=?1", [first], |r| r.get(0))).unwrap();
    let id = check.rsplit_once(':').unwrap().1.to_owned();
    let o = f.app.open_mut().unwrap();
    let chk = fb_core::checks::get_check(&mut o.book, o.owner, f.co, &id).unwrap();
    fb_core::checks::void_check(&mut o.book, o.owner, f.co, &id, chk["revision"].as_i64(), &json!("2026-11-10"), &json!("By hand")).unwrap();
    let page = f.render("");
    assert!(page.contains("2 reviewed transactions point at an entry that was voided or reversed"), "{page}");
    let attention = f.render("?status=ATTENTION");
    let html = row(&attention, again);
    assert!(html.contains("The entry it was linked to was voided") && html.contains("name=\"return_one\""), "{html}");
    assert!(row(&attention, first).contains("Return to review not available: the copy downloaded again"));
}
