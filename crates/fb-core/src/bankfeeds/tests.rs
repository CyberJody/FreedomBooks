//! Bank-feed service tests (translated from bankfeeds/tests.py, test_races.py) and guard break-it tests.
use super::*;
use crate::{AccountId, checks, directory, services::AccountInput, transfers, users};
mod integration;
use std::sync::atomic::{AtomicUsize, Ordering};
// Synthetic service fixtures have no plugin vault. The explicit test-only
// capability is the same escape hatch as the suite's raw SQL connection.
fn restore_connection_credentials<S: AsRef<str>>(book: &mut Book, actor: UserId, co: CompanyId, connection_id: &str, access_url: &str, extra: impl IntoIterator<Item=S>) -> FbResult<SecretSet> {
    let restore = book.test_bankfeed_restore(connection_id, access_url);
    super::restore_connection_credentials(book, actor, co, connection_id, &restore, extra)
}
static NEXT: AtomicUsize = AtomicUsize::new(0);

mod fix6;
mod r7;
mod r8;
mod fix8;
mod r9;
mod fix9;
mod r10;
mod fix10;
mod dfix3;
mod mapping;

struct Fx {
    book: Book,
    path: std::path::PathBuf,
    u: UserId,
    c: CompanyId,
    outsider: UserId,
    other: CompanyId,
    other_expense: AccountId,
    a: std::collections::HashMap<&'static str, AccountId>,
    vendor: String,
    customer: String,
    conn: String,
    /// The connection's secrets, passed to every write path the helpers call (FIX8).
    secrets: SecretSet,
}
fn tx(remote: &str, date: &str, amount: i64, description: &str) -> StagedTransaction {
    StagedTransaction { remote_id: remote.into(), posted_on: Some(date.into()), transacted_on: None, amount, description: description.into(), pending: false }
}
fn acct(remote: &str, name: &str, txs: Vec<StagedTransaction>) -> StagedAccount {
    StagedAccount {
        conn_key: "CON-demo-001".into(),
        remote_id: remote.into(),
        name: name.into(),
        org_name: "Demo Bank".into(),
        currency: "USD".into(),
        balance: Some(1_248_055),
        available: None,
        balance_epoch: Some(1_793_188_800),
        skipped: vec![],
        transactions: txs,
    }
}
/// testing.demo_account_set, with the posted epochs already in America/New_York accounting dates.
fn demo() -> StagedPull {
    StagedPull {
        accounts: vec![
            acct(
                "ACT-checking",
                "Business Checking",
                vec![
                    tx("TX-1001", "2026-10-01", -8419, "OFFICE DEPOT #221 PURCHASE"),
                    tx("TX-1002", "2026-10-02", -120_000, "CHECK 1041"),
                    tx("TX-1003", "2026-10-05", 250_000, "DEPOSIT MOBILE CHECK"),
                    tx("TX-1004", "2026-10-08", -5900, "ACME INTERNET SERVICES AUTOPAY"),
                    tx("TX-1005", "2026-10-14", -50_000, "ONLINE TRANSFER TO SAVINGS"),
                    tx("TX-1006", "2026-10-19", 312, "INTEREST PAYMENT"),
                    StagedTransaction { transacted_on: Some("2026-10-22".into()), ..tx("TX-1007", "2026-10-23", -4275, "FUEL STOP 18 QUEENSBURY") },
                ],
            ),
            acct("ACT-savings", "Business Savings", vec![tx("TX-2001", "2026-10-14", 50_000, "ONLINE TRANSFER FROM CHECKING"), tx("TX-2002", "2026-10-30", 312, "INTEREST PAYMENT")]),
            acct("ACT-card", "Business Credit Card", vec![tx("TX-3001", "2026-10-10", -64_040, "HOSTING CO ANNUAL PLAN")]),
        ],
        skipped: vec![],
        errors: vec![],
    }
}
const NOW: &str = "2026-10-28T12:00:00Z";
impl Fx {
    fn new() -> Self {
        Self::new_in(&std::env::temp_dir())
    }
    fn new_in(dir: &std::path::Path) -> Self {
        let path = dir.join(format!("bankfeeds-{}-{}.fbbook", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
        let mut book = Book::create(&path, &[91; 32]).unwrap();
        let u = users::create_user(&mut book, "feeds-test", true).unwrap();
        let outsider = users::create_user(&mut book, "outsider", false).unwrap();
        let c = services::create_company(&mut book, u, "Synthetic feeds", "USD", false).unwrap().id;
        let other = services::create_company(&mut book, outsider, "Other books", "USD", false).unwrap().id;
        let mut a = std::collections::HashMap::new();
        for (code, name, kind, detail) in [
            ("100", "Checking", "ASSET", "BANK"),
            ("101", "Savings", "ASSET", "BANK"),
            ("105", "Undeposited", "ASSET", "OTHER_CURRENT_ASSET"),
            ("110", "Receivables", "ASSET", "RECEIVABLE"),
            ("200", "Payables", "LIABILITY", "PAYABLE"),
            ("220", "Synthetic card", "LIABILITY", "CREDIT_CARD"),
            ("400", "Sales", "INCOME", "INCOME"),
            ("410", "Interest income", "INCOME", "OTHER_INCOME"),
            ("500", "Office", "EXPENSE", "EXPENSE"),
        ] {
            let id = services::save_account(&mut book, u, c, AccountInput::new(code, name).kind(kind).detail(detail)).unwrap().id;
            a.insert(code, id);
        }
        let other_expense = services::save_account(&mut book, outsider, other, AccountInput::new("500", "Other office").kind("EXPENSE")).unwrap().id;
        documents::save_setup(
            &mut book,
            u,
            c,
            &json!({"receivables":a["110"].to_db(),"payables":a["200"].to_db(),"undeposited":a["105"].to_db(),"banks":[a["100"].to_db(),a["101"].to_db()]}),
            Some(0),
        )
        .unwrap();
        let vendor = directory::save_record(&mut book, u, c, directory::Model::Vendor, &json!({"display_name":"Office Depot"}), None).unwrap()["id"].as_str().unwrap().to_owned();
        let customer =
            directory::save_record(&mut book, u, c, directory::Model::Customer, &json!({"display_name":"Synthetic customer"}), None).unwrap()["id"].as_str().unwrap().to_owned();
        let conn = connect(&mut book, u, c, &json!("SimpleFIN"), "Bridge.SimpleFIN.org", &SecretSet::none()).unwrap()["id"].as_str().unwrap().to_owned();
        Self { book, path, u, c, outsider, other, other_expense, a, vendor, customer, conn, secrets: SecretSet::none() }
    }
    fn acc(&self, code: &str) -> String {
        self.a[code].to_db()
    }
    fn sync(&mut self, pull: &StagedPull) -> Value {
        store_pull(&mut self.book, self.u, self.c, &self.conn.clone(), pull, NOW, false, None, &SecretSet::none()).unwrap()
    }
    /// The identity key the book gives a raw provider id (FIX5).
    fn identity(&mut self, domain: &str, raw: &str) -> String {
        self.book.read(|q| identity_of(q, domain, raw)).unwrap().expect("the book has an identity secret")
    }
    /// Rows are found by the provider's raw id through its identity key, never by the displayed id (FIX5).
    fn feed(&mut self, remote: &str) -> String {
        let key = self.identity(ACCOUNT_DOMAIN, remote);
        self.book.connection().query_row("SELECT id FROM bankfeeds_account WHERE remote_identity=?1", [key], |r| r.get(0)).unwrap()
    }
    fn item(&mut self, remote: &str) -> Item {
        let identity = self.identity(TRANSACTION_DOMAIN, remote);
        let key: String = self.book.connection().query_row("SELECT id FROM bankfeeds_transaction WHERE remote_identity=?1", [identity], |r| r.get(0)).unwrap();
        self.book.read(|q| item(q, self.c, &key)).unwrap()
    }
    fn map(&mut self, remote: &str, code: Option<&str>) -> FbResult<Value> {
        let f = self.feed(remote);
        let target = code.map(|c| json!(self.acc(c))).unwrap_or(Value::Null);
        map_account(&mut self.book, self.u, self.c, &f, &target, &self.secrets)
    }
    fn connected_books(&mut self) {
        self.sync(&demo());
        self.map("ACT-checking", Some("100")).unwrap();
        self.map("ACT-savings", Some("101")).unwrap();
        self.map("ACT-card", Some("220")).unwrap();
    }
    fn accept(&mut self, remote: &str, input: Value) -> FbResult<Item> {
        let i = self.item(remote);
        accept(&mut self.book, self.u, self.c, &i.id, &input, &self.secrets)
    }
    fn lines(&mut self, journal: &str) -> Vec<(String, i64, i64)> {
        let mut st = self.book.connection().prepare("SELECT a.code,l.debit,l.credit FROM ledger_journalline l JOIN ledger_account a ON a.id=l.account_id WHERE l.journal_id=?1 ORDER BY a.code").unwrap();
        st.query_map([journal], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().collect::<Result<_, _>>().unwrap()
    }
    fn count(&mut self, sql: &str) -> i64 {
        self.book.connection().query_row(sql, [], |r| r.get(0)).unwrap()
    }
    fn reopen_clean(&mut self) {
        assert_eq!(self.book.check().unwrap(), Vec::<String>::new());
        assert!(Book::open(&self.path, &[91; 32]).unwrap().check().unwrap().is_empty());
    }
    fn journal(&mut self, date: &str, debit_code: &str, credit_code: &str, amount: &str) -> String {
        let lines = vec![json!({"account":self.acc(debit_code),"debit":amount}), json!({"account":self.acc(credit_code),"credit":amount})];
        let j = services::save_draft(&mut self.book, self.u, self.c, services::DraftInput::new(date, "Manual", lines)).unwrap();
        services::post_journal(&mut self.book, self.u, self.c, j.id, None).unwrap().id.to_db()
    }
    fn undeposited(&mut self, amount: &str, date: &str) -> String {
        let p = json!({"request_key":fb_ids::new_v7().to_string(),"customer_id":self.customer,"date":date,"destination":"UNDEPOSITED","method":"CASH","tax_mode":"NONE","lines":[{"item_id":null,"taxable":false,"description":"Sale","quantity":"1","rate":amount,"income_account_id":self.a["400"].to_db()}]});
        let r = crate::sales::review_sales_receipt(&mut self.book, self.u, self.c, &p).unwrap();
        crate::sales::record_sales_receipt(&mut self.book, self.u, self.c, &p, Some(&r)).unwrap()["id"].as_str().unwrap().to_owned()
    }
}
fn msg<T: std::fmt::Debug>(r: FbResult<T>) -> String {
    r.unwrap_err().message()
}

#[test]
fn sanitize_and_default_check_numbers_follow_python() {
    let mut f = Fx::new();
    f.sync(&demo());
    let i = f.item("TX-1002");
    assert_eq!(default_check_number(&i), "1041");
    let j = f.item("TX-1001");
    assert_eq!(default_check_number(&j), format!("EFT-{}", j.ordinal));
    assert_eq!(j.ordinal, 1);
}

#[test]
fn first_sync_downloads_every_account_and_transaction_once_and_resync_never_duplicates() {
    let mut f = Fx::new();
    let r = f.sync(&demo());
    assert_eq!((r["accounts"].as_i64(), r["new"].as_i64()), (Some(3), Some(10)));
    assert_eq!(r["summary"], "3 accounts, 10 new transactions");
    let again = f.sync(&demo());
    assert_eq!((again["new"].as_i64(), again["unchanged"].as_i64()), (Some(0), Some(10)));
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_transaction"), 10);
    let mut more = demo();
    more.accounts[0].transactions.push(tx("TX-1010", "2026-10-27", 20, "DEPOSIT"));
    assert_eq!(f.sync(&more)["new"], 1);
    assert_eq!(f.item("TX-1010").ordinal, 11);
    let synced = f.count("SELECT sync_count FROM bankfeeds_connection");
    assert_eq!(synced, 3);
    f.reopen_clean();
}

/// Found by the differential random run: Python's `_store` reads existing rows before its loop, so an id
/// listed twice as NEW in one pull is inserted twice and `full_clean` refuses the whole store. An id that
/// already existed may repeat (both copies update the same row).
#[test]
fn a_new_id_listed_twice_in_one_pull_refuses_the_store_like_python() {
    let mut f = Fx::new();
    f.sync(&demo());
    let mut twice = demo();
    twice.accounts[0].transactions.push(tx("TX-2000", "2026-10-27", 20, "NEW"));
    twice.accounts[0].transactions.push(tx("TX-2000", "2026-10-27", 20, "NEW"));
    let conn = f.conn.clone();
    let refused = msg(store_pull(&mut f.book, f.u, f.c, &conn, &twice, NOW, false, None, &SecretSet::none()));
    assert_eq!(refused, "Bank feed transaction with this Feed account and Remote id already exists.");
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_transaction WHERE remote_id='TX-2000'"), 0);
    assert_eq!(f.count("SELECT sync_count FROM bankfeeds_connection"), 1);
    let mut existing = demo();
    let first = existing.accounts[0].transactions[0].clone();
    existing.accounts[0].transactions.push(first);
    assert_eq!(f.sync(&existing)["unchanged"].as_i64(), Some(11));
    f.reopen_clean();
}

#[test]
fn pending_rows_post_update_and_withdraw_inside_the_window_only() {
    let mut f = Fx::new();
    let mut pull = demo();
    pull.accounts[0].transactions.push(StagedTransaction { pending: true, posted_on: None, transacted_on: Some("2026-10-27".into()), ..tx("TX-P1", "", -1000, "PENDING COFFEE") });
    pull.accounts[0].transactions.push(StagedTransaction { pending: true, posted_on: None, transacted_on: Some("2026-10-27".into()), ..tx("TX-P2", "", -2000, "PENDING LUNCH") });
    f.sync(&pull);
    let p1 = f.item("TX-P1");
    assert!(msg(accept(&mut f.book, f.u, f.c, &p1.id, &json!({"kind":"JOURNAL","account_id":f.a["500"].to_db()}), &SecretSet::none())).contains("pending"));
    // P1 posts under the same id; P2 is withdrawn (absent from the next pull).
    let mut next = demo();
    next.accounts[0].transactions.push(tx("TX-P1", "2026-10-28", -1000, "COFFEE"));
    let r = f.sync(&next);
    assert_eq!((r["posted"].as_i64(), r["pending_dropped"].as_i64()), (Some(1), Some(1)));
    assert!(r["summary"].as_str().unwrap().contains("1 now posted; 1 pending withdrawn"));
    assert!(!f.item("TX-P1").pending);
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_transaction WHERE remote_id='TX-P2'"), 0);
    // A window that starts after the row was first seen keeps it; an ignored pending row always stays.
    let mut p3 = demo();
    p3.accounts[1].transactions.push(StagedTransaction { pending: true, ..tx("TX-P3", "2026-10-28", -5, "HOLD") });
    f.sync(&p3);
    let conn = f.conn.clone();
    let r = store_pull(&mut f.book, f.u, f.c, &conn, &demo(), NOW, false, Some(4_102_444_800), &SecretSet::none()).unwrap();
    assert_eq!(r["pending_dropped"], 0);
    let p3 = f.item("TX-P3");
    ignore(&mut f.book, f.u, f.c, &p3.id, &SecretSet::none()).unwrap();
    assert_eq!(f.sync(&demo())["pending_dropped"], 0);
    // The ordinal counter never reuses the withdrawn row's number.
    let mut late = demo();
    late.accounts[0].transactions.push(tx("TX-LATE", "2026-10-28", 1, "LATE"));
    f.sync(&late);
    assert_eq!(f.item("TX-LATE").ordinal, 14);
    f.reopen_clean();
}

#[test]
fn decided_rows_keep_their_values_when_the_provider_changes_them_later() {
    let mut f = Fx::new();
    f.connected_books();
    f.accept("TX-1001", json!({"kind":"CHECK","account_id":f.a["500"].to_db()})).unwrap();
    let mut changed = demo();
    changed.accounts[0].transactions[0].amount = -9999;
    changed.accounts[0].transactions[0].description = "CHANGED".into();
    let r = f.sync(&changed);
    assert_eq!(r["updated"], 0);
    let i = f.item("TX-1001");
    assert_eq!((i.amount, i.description.as_str()), (-8419, "OFFICE DEPOT #221 PURCHASE"));
}

#[test]
fn mapping_rules() {
    let mut f = Fx::new();
    f.sync(&demo());
    for (code, phrase) in [("110", "receivables"), ("105", "undeposited"), ("500", "bank (asset) or credit card")] {
        let m = f.map("ACT-checking", Some(code));
        assert!(msg(m).contains(phrase), "{code}");
    }
    let checking = f.feed("ACT-checking");
    let other = json!(f.other_expense.to_db());
    assert!(msg(map_account(&mut f.book, f.u, f.c, &checking, &other, &SecretSet::none())).contains("from this company"));
    f.map("ACT-checking", Some("100")).unwrap();
    assert!(msg(f.map("ACT-savings", Some("100"))).contains("already mapped"));
    f.map("ACT-card", Some("220")).unwrap();
    f.map("ACT-checking", None).unwrap();
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_account WHERE ledger_account_id IS NOT NULL"), 1);
    assert_eq!(msg(map_account(&mut f.book, f.outsider, f.other, &checking, &other, &SecretSet::none())), "Choose a downloaded account of this company.");
    assert_eq!(f.count("SELECT count(*) FROM ledger_auditevent WHERE action='bankfeed.mapped'"), 3);
    let codes: Vec<String> = mappable_accounts(&mut f.book, f.u, f.c).unwrap().into_iter().map(|a| a.code).collect();
    assert_eq!(codes, ["100", "101", "220"]);
    f.reopen_clean();
}

#[test]
fn existing_entry_beats_receipt_beats_rule() {
    let mut f = Fx::new();
    f.connected_books();
    let interest = f.item("TX-1006");
    let rule = save_rule(&mut f.book, f.u, f.c, &json!({"name":"Interest","match_text":"interest","sign":"IN","account_id":f.a["410"].to_db()}), &SecretSet::none()).unwrap();
    let s = suggest(&mut f.book, f.u, f.c, &interest.id).unwrap();
    assert_eq!((s.kind.as_str(), s.rule.as_ref().map(|r| r.id.clone()), s.account_id.clone()), ("JOURNAL", Some(rule.id.clone()), Some(f.a["410"].to_db())));
    let receipt = f.undeposited("3.12", "2026-10-18");
    let s = suggest(&mut f.book, f.u, f.c, &interest.id).unwrap();
    assert_eq!((s.kind.as_str(), s.receipts.iter().map(|r| r.receipt_id.clone()).collect::<Vec<_>>()), ("DEPOSIT", vec![receipt]));
    let journal = f.journal("2026-10-20", "100", "410", "3.12");
    let s = suggest(&mut f.book, f.u, f.c, &interest.id).unwrap();
    assert_eq!((s.kind.as_str(), s.matches[0].journal_id.clone()), ("MATCH", journal.clone()));
    let far = f.journal("2026-10-01", "100", "410", "3.12");
    assert!(suggest(&mut f.book, f.u, f.c, &interest.id).unwrap().matches.iter().all(|m| m.journal_id != far));
    let line = s.matches[0].line_id.clone();
    let done = accept(&mut f.book, f.u, f.c, &interest.id, &json!({"kind":"MATCH","journal_line_id":line}), &SecretSet::none()).unwrap();
    assert_eq!((done.status.as_str(), done.reference.clone()), ("MATCHED", done.reference.clone()));
    assert!(done.reference.starts_with("JE "));
    let mut twin = demo();
    twin.accounts[0].transactions.push(tx("TX-1009", "2026-10-21", 312, "INTEREST PAYMENT AGAIN"));
    f.sync(&twin);
    let t = f.item("TX-1009");
    assert!(suggest(&mut f.book, f.u, f.c, &t.id).unwrap().matches.iter().all(|m| m.line_id != line));
    assert!(accept(&mut f.book, f.u, f.c, &t.id, &json!({"kind":"MATCH","journal_line_id":line}), &SecretSet::none()).is_err());
    f.reopen_clean();
}

#[test]
fn rule_precedence_sign_and_amount_range() {
    let mut f = Fx::new();
    f.connected_books();
    let fuel = f.item("TX-1007");
    let second = save_rule(&mut f.book, f.u, f.c, &json!({"name":"Any fuel","match_text":"FUEL","account_id":f.a["500"].to_db(),"payee_name":"Fuel vendor"}), &SecretSet::none()).unwrap();
    let first = save_rule(&mut f.book, f.u, f.c, &json!({"name":"Big fuel","match_text":"fuel stop","amount_min":"100","account_id":f.a["410"].to_db()}), &SecretSet::none()).unwrap();
    let pick = |f: &mut Fx| active_rules(&mut f.book, f.u, f.c).map(|r| rule_for(&r, &fuel).map(|r| r.id.clone())).unwrap();
    assert_eq!(pick(&mut f), Some(second.id.clone()));
    save_rule(&mut f.book, f.u, f.c, &json!({"rule_id":first.id,"name":"Big fuel","match_text":"fuel stop","amount_min":"10","amount_max":"50","account_id":f.a["410"].to_db()}), &SecretSet::none()).unwrap();
    assert_eq!(pick(&mut f), Some(second.id.clone()));
    move_rule(&mut f.book, f.u, f.c, &first.id, &json!("up"), &SecretSet::none()).unwrap();
    assert_eq!(pick(&mut f), Some(first.id.clone()));
    save_rule(&mut f.book, f.u, f.c, &json!({"rule_id":first.id,"name":"Big fuel","match_text":"fuel stop","sign":"IN","account_id":f.a["410"].to_db()}), &SecretSet::none()).unwrap();
    assert_eq!(pick(&mut f), Some(second.id.clone()));
    set_rule_active(&mut f.book, f.u, f.c, &second.id, &json!(false), &SecretSet::none()).unwrap();
    save_rule(&mut f.book, f.u, f.c, &json!({"rule_id":first.id,"name":"Big fuel","match_text":"fuel stop","sign":"OUT","account_id":f.a["410"].to_db()}), &SecretSet::none()).unwrap();
    assert_eq!(pick(&mut f), Some(first.id.clone()));
    set_rule_active(&mut f.book, f.u, f.c, &first.id, &json!(false), &SecretSet::none()).unwrap();
    assert_eq!(pick(&mut f), None);
    let s = suggest(&mut f.book, f.u, f.c, &fuel.id).unwrap();
    assert_eq!((s.kind.as_str(), s.account_id, s.payee_name.as_str()), ("", None, ""));
    move_rule(&mut f.book, f.u, f.c, &first.id, &json!("up"), &SecretSet::none()).unwrap();
    let names: Vec<String> = all_rules(&mut f.book, f.u, f.c).unwrap().into_iter().map(|r| r.name).collect();
    assert_eq!(names, ["Big fuel", "Any fuel"]);
}

#[test]
fn rule_validation_and_isolation() {
    let mut f = Fx::new();
    let other = f.other_expense.to_db();
    for (values, phrase) in [
        (json!({"match_text":"x","account_id":f.a["110"].to_db()}), "receivables"),
        (json!({"match_text":"","account_id":f.a["500"].to_db()}), "nonblank"),
        (json!({"match_text":"x","amount_min":"5","amount_max":"1","account_id":f.a["500"].to_db()}), "cannot exceed"),
        (json!({"match_text":"x","amount_min":"-1","account_id":f.a["500"].to_db()}), "negative"),
        (json!({"match_text":"x","sign":"UP","account_id":f.a["500"].to_db()}), "money in"),
        (json!({"match_text":"x","account_id":other}), "from this company"),
    ] {
        let mut v = values.clone();
        v["name"] = json!("r");
        let m = msg(save_rule(&mut f.book, f.u, f.c, &v, &SecretSet::none()));
        assert!(m.contains(phrase), "{values} -> {m}");
    }
    let rule = save_rule(&mut f.book, f.u, f.c, &json!({"name":"r","match_text":"x","account_id":f.a["500"].to_db()}), &SecretSet::none()).unwrap();
    assert!(save_rule(&mut f.book, f.outsider, f.other, &json!({"rule_id":rule.id,"name":"r","match_text":"x","account_id":other}), &SecretSet::none()).is_err());
    assert!(move_rule(&mut f.book, f.outsider, f.other, &rule.id, &json!("up"), &SecretSet::none()).is_err());
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_rule"), 1);
}

#[test]
fn money_out_becomes_a_recorded_check_through_the_check_service() {
    let mut f = Fx::new();
    f.connected_books();
    let item = f.item("TX-1001");
    let done = f.accept("TX-1001", json!({"kind":"CHECK","account_id":f.a["500"].to_db(),"payee_name":"Office Depot"})).unwrap();
    assert_eq!((done.status.as_str(), done.accepted_as.as_str(), &done.reference[..4]), ("ACCEPTED", "CHECK", "CHK-"));
    let chk: Value = checks::current_checks(&mut f.book, f.u, f.c).unwrap().into_iter().next().unwrap();
    assert_eq!((chk["current"]["state"].as_str(), chk["current"]["payee_name"].as_str(), chk["current"]["check_number"].clone(), chk["current"]["date"].as_str()), (Some("RECORDED"), Some("Office Depot"), json!(format!("EFT-{}", item.ordinal)), Some("2026-10-01")));
    let journal = done.journal_id.clone().unwrap();
    assert_eq!(f.lines(&journal), [("100".into(), 0, 8419), ("500".into(), 8419, 0)]);
    assert_eq!(f.count("SELECT count(*) FROM ledger_auditevent WHERE action='bankfeed.accepted'"), 1);
    let again = f.accept("TX-1001", json!({"kind":"CHECK","account_id":f.a["500"].to_db()})).unwrap();
    assert_eq!(again.journal_id, done.journal_id);
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'"), 1);
    let done2 = f.accept("TX-1002", json!({"kind":"CHECK","account_id":f.a["500"].to_db(),"vendor_id":f.vendor})).unwrap();
    let all = checks::current_checks(&mut f.book, f.u, f.c).unwrap();
    let second = all.iter().find(|c| c["number"] == done2.reference.as_str()).unwrap();
    assert_eq!((second["current"]["check_number"].as_str(), second["current"]["payee_kind"].as_str()), (Some("1041"), Some("VENDOR")));
    f.reopen_clean();
}

#[test]
fn check_refuses_money_in_control_accounts_unmapped_and_credit_card_feeds() {
    let mut f = Fx::new();
    f.connected_books();
    let m = msg(f.accept("TX-1003", json!({"kind":"CHECK","account_id":f.a["500"].to_db()})));
    assert!(m.contains("Money coming in"));
    let m = msg(f.accept("TX-1001", json!({"kind":"CHECK","account_id":f.a["200"].to_db()})));
    assert!(m.contains("control"), "{m}");
    let m = msg(f.accept("TX-3001", json!({"kind":"CHECK","account_id":f.a["500"].to_db()})));
    assert!(m.contains("designated bank"));
    f.map("ACT-checking", None).unwrap();
    let m = msg(f.accept("TX-1001", json!({"kind":"CHECK","account_id":f.a["500"].to_db()})));
    assert!(m.contains("Map"));
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal"), 0);
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_transaction WHERE status='NEW'"), 10);
}

#[test]
fn money_in_becomes_a_journal_entry_and_a_card_charge_posts_against_the_liability() {
    let mut f = Fx::new();
    f.connected_books();
    let interest = f.accept("TX-1006", json!({"kind":"JOURNAL","account_id":f.a["410"].to_db(),"payee_name":"Demo Bank"})).unwrap();
    let j = interest.journal_id.clone().unwrap();
    assert_eq!(f.lines(&j), [("100".into(), 312, 0), ("410".into(), 0, 312)]);
    let (status, memo, name): (String, String, String) = f.book.connection().query_row("SELECT j.status,j.memo,l.name FROM ledger_journal j JOIN ledger_journalline l ON l.journal_id=j.id WHERE j.id=?1 AND l.position=1", [&j], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    assert_eq!((status.as_str(), memo.as_str(), name.as_str(), &interest.reference[..3]), ("POSTED", "Bank feed: INTEREST PAYMENT", "Demo Bank", "JE "));
    let charge = f.accept("TX-3001", json!({"kind":"JOURNAL","account_id":f.a["500"].to_db()})).unwrap();
    assert_eq!(f.lines(&charge.journal_id.unwrap()), [("220".into(), 0, 64_040), ("500".into(), 64_040, 0)]);
    assert!(msg(f.accept("TX-1004", json!({"kind":"JOURNAL","account_id":f.a["105"].to_db()}))).contains("control"));
    assert!(f.accept("TX-1004", json!({"kind":"JOURNAL","account_id":f.a["100"].to_db()})).is_err());
    let other = f.other_expense.to_db();
    assert!(f.accept("TX-1004", json!({"kind":"JOURNAL","account_id":other})).is_err());
    f.reopen_clean();
}

#[test]
fn deposit_of_an_undeposited_receipt_goes_through_the_deposit_service() {
    let mut f = Fx::new();
    f.connected_books();
    let receipt = f.undeposited("2500.00", "2026-10-04");
    let item = f.item("TX-1003");
    assert_eq!(suggest(&mut f.book, f.u, f.c, &item.id).unwrap().kind, "DEPOSIT");
    let done = f.accept("TX-1003", json!({"kind":"DEPOSIT","receipt_ids":[receipt]})).unwrap();
    assert_eq!((done.accepted_as.as_str(), &done.reference[..4]), ("DEPOSIT", "DEP-"));
    assert_eq!(f.lines(&done.journal_id.unwrap()), [("100".into(), 250_000, 0), ("105".into(), 0, 250_000)]);
    let small = f.undeposited("0.10", "2026-10-04");
    let mut more = demo();
    more.accounts[0].transactions.push(tx("TX-1010", "2026-10-27", 20, "DEPOSIT"));
    f.sync(&more);
    let m = msg(f.accept("TX-1010", json!({"kind":"DEPOSIT","receipt_ids":[small]})));
    assert!(m.contains("not the bank's 0.20"), "{m}");
    assert!(msg(f.accept("TX-1001", json!({"kind":"DEPOSIT","receipt_ids":[]}))).contains("Money going out"));
    f.reopen_clean();
}

#[test]
fn transfer_posts_once_and_the_other_side_matches_it() {
    let mut f = Fx::new();
    f.connected_books();
    let done = f.accept("TX-1005", json!({"kind":"TRANSFER","transfer_account_id":f.a["101"].to_db()})).unwrap();
    assert_eq!((done.accepted_as.as_str(), &done.reference[..4]), ("TRANSFER", "TRF-"));
    let j = done.journal_id.clone().unwrap();
    assert_eq!(f.lines(&j), [("100".into(), 0, 50_000), ("101".into(), 50_000, 0)]);
    let mirror = f.item("TX-2001");
    let s = suggest(&mut f.book, f.u, f.c, &mirror.id).unwrap();
    assert_eq!((s.kind.as_str(), s.matches[0].journal_id.clone()), ("MATCH", j.clone()));
    let line = s.matches[0].line_id.clone();
    let matched = accept(&mut f.book, f.u, f.c, &mirror.id, &json!({"kind":"MATCH","journal_line_id":line}), &SecretSet::none()).unwrap();
    assert_eq!((matched.status.as_str(), matched.journal_id.clone()), ("MATCHED", Some(j)));
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal"), 1);
    assert!(f.accept("TX-1001", json!({"kind":"MATCH","journal_line_id":line})).is_err());
    assert!(f.accept("TX-3001", json!({"kind":"TRANSFER","transfer_account_id":f.a["100"].to_db()})).is_err());
    let _ = transfers::Review::clone;
    f.reopen_clean();
}

#[test]
fn ignore_return_and_undo_through_the_normal_void_path() {
    let mut f = Fx::new();
    f.connected_books();
    let item = f.item("TX-1004");
    ignore(&mut f.book, f.u, f.c, &item.id, &SecretSet::none()).unwrap();
    ignore(&mut f.book, f.u, f.c, &item.id, &SecretSet::none()).unwrap();
    assert!(f.accept("TX-1004", json!({"kind":"CHECK","account_id":f.a["500"].to_db()})).is_err());
    return_to_review(&mut f.book, f.u, f.c, &item.id, &SecretSet::none()).unwrap();
    let done = f.accept("TX-1004", json!({"kind":"CHECK","account_id":f.a["500"].to_db()})).unwrap();
    assert!(msg(return_to_review(&mut f.book, f.u, f.c, &item.id, &SecretSet::none())).contains("still posted"));
    let chk = checks::current_checks(&mut f.book, f.u, f.c).unwrap().into_iter().find(|c| c["number"] == done.reference.as_str()).unwrap();
    checks::void_check(&mut f.book, f.u, f.c, chk["id"].as_str().unwrap(), chk["revision"].as_i64(), &json!("2026-10-30"), &json!("Wrong category")).unwrap();
    let back = return_to_review(&mut f.book, f.u, f.c, &item.id, &SecretSet::none()).unwrap();
    assert_eq!((back.status.as_str(), back.journal_id.clone(), back.return_count), ("NEW", None, 2));
    let redo = f.accept("TX-1004", json!({"kind":"JOURNAL","account_id":f.a["410"].to_db()})).unwrap();
    assert_ne!(redo.journal_id, done.journal_id);
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'"), 3);
    let target = format!("bankfeedtransaction:{}", item.id);
    let mut st = f.book.connection().prepare("SELECT action FROM ledger_auditevent WHERE target=?1 ORDER BY id").unwrap();
    let actions: Vec<String> = st.query_map([&target], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
    drop(st);
    assert_eq!(actions, ["bankfeed.ignored", "bankfeed.returned", "bankfeed.accepted", "bankfeed.returned", "bankfeed.accepted"]);
    f.reopen_clean();
}

#[test]
fn cross_company_isolation_on_accept_and_review_data() {
    let mut f = Fx::new();
    f.connected_books();
    let item = f.item("TX-1001");
    let other = f.other_expense.to_db();
    assert!(accept(&mut f.book, f.outsider, f.other, &item.id, &json!({"kind":"CHECK","account_id":other}), &SecretSet::none()).is_err());
    assert!(ignore(&mut f.book, f.outsider, f.other, &item.id, &SecretSet::none()).is_err());
    let denied = accept(&mut f.book, f.outsider, f.c, &item.id, &json!({"kind":"CHECK","account_id":f.a["500"].to_db()}), &SecretSet::none()).unwrap_err();
    assert_eq!(denied.kind(), "PermissionDenied");
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal"), 0);
    let s = f.book.read(|q| suggest_for(q, f.other, &item, None)).unwrap();
    assert!(s.matches.is_empty());
}

#[test]
fn forget_keeps_history_and_blocks_sync_and_the_interval_is_respected() {
    let mut f = Fx::new();
    f.sync(&demo());
    let conn = f.conn.clone();
    let m = msg(begin_sync(&mut f.book, f.u, f.c, &conn, "2026-10-28T12:30:00Z", SCHEDULED_MIN_INTERVAL_SECS, false));
    assert_eq!(m, "Synced 30 min ago. SimpleFIN asks apps not to pull too often; try again in 331 min.");
    let start = begin_sync(&mut f.book, f.u, f.c, &conn, "2026-10-28T12:30:00Z", SCHEDULED_MIN_INTERVAL_SECS, true).unwrap();
    assert_eq!(start, 1_793_188_800 - 7 * 86_400);
    record_sync_failure(&mut f.book, f.u, f.c, &conn, "2026-10-28T13:00:00Z", "SimpleFIN said: https://u:p@x.example/a", false, &SecretSet::none()).unwrap();
    let start = begin_sync(&mut f.book, f.u, f.c, &conn, "2026-10-28T13:00:00Z", SCHEDULED_MIN_INTERVAL_SECS, false).unwrap();
    assert_eq!(start, 1_793_192_400 - 44 * 86_400);
    let summary: String = f.book.connection().query_row("SELECT last_sync_summary FROM bankfeeds_connection", [], |r| r.get(0)).unwrap();
    assert_eq!(summary, "SimpleFIN said: https://***@x.example/a");
    forget(&mut f.book, f.u, f.c, &conn, &SecretSet::none()).unwrap();
    forget(&mut f.book, f.u, f.c, &conn, &SecretSet::none()).unwrap();
    assert_eq!(f.count("SELECT count(*) FROM ledger_auditevent WHERE action='bankfeed.forgotten'"), 1);
    assert!(msg(begin_sync(&mut f.book, f.u, f.c, &conn, "2026-11-28T13:00:00Z", 0, true)).contains("forgotten"));
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_transaction"), 10);
    assert!(msg(forget(&mut f.book, f.outsider, f.other, &conn, &SecretSet::none())).contains("connection of this company"));
    f.reopen_clean();
}

#[test]
fn core_bridge_staging_dedupes_and_never_withdraws() {
    let mut f = Fx::new();
    f.sync(&demo());
    let conn = f.conn.clone();
    let batch = vec![("ACT-checking".to_owned(), tx("TX-1001", "2026-10-01", -8419, "OFFICE DEPOT #221 PURCHASE")), ("ACT-card".to_owned(), tx("TX-3002", "2026-10-27", -100, "NEW CHARGE"))];
    let ids = stage_bank_transactions(&mut f.book, f.u, f.c, &conn, &batch, &SecretSet::none()).unwrap();
    assert_eq!(ids[0], f.item("TX-1001").id);
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_transaction"), 11);
    assert!(stage_bank_transactions(&mut f.book, f.u, f.c, &conn, &[("ACT-nope".into(), tx("X", "2026-10-01", 1, ""))], &SecretSet::none()).is_err());
    assert_eq!(f.count("SELECT sync_count FROM bankfeeds_connection"), 1);
}

// --- guard break-it tests: each SQL refusal is exercised by direct SQL that bypasses the services ---

fn raw_err(f: &Fx, sql: &str) -> String {
    let e = f.book.connection().execute_batch(sql).unwrap_err();
    fb_db::constraint_of(&e).unwrap_or_else(|| e.to_string())
}
#[test]
fn guards_refuse_direct_sql_that_the_services_would_refuse() {
    let mut f = Fx::new();
    f.connected_books();
    let done = f.accept("TX-1006", json!({"kind":"JOURNAL","account_id":f.a["410"].to_db()})).unwrap();
    let i = f.item("TX-1001");
    let checking = f.feed("ACT-checking");
    let savings = f.feed("ACT-savings");
    let q = f.book.connection();
    q.execute_batch("PRAGMA recursive_triggers=ON").unwrap();
    let journal = done.journal_id.clone().unwrap();
    let line = done.journal_line_id.clone().unwrap();
    for recursive in [0, 1] {
        q.execute_batch(&format!("PRAGMA recursive_triggers={recursive}")).unwrap();
        let cases: Vec<(String, &str)> = vec![
            (format!("UPDATE bankfeeds_transaction SET id='{}' WHERE id='{}'", fb_ids::new_v7(), i.id), "primary_key_immutable"),
            (format!("UPDATE bankfeeds_account SET id='{}' WHERE id='{}'", fb_ids::new_v7(), checking), "primary_key_immutable"),
            (format!("UPDATE bankfeeds_connection SET id='{}'", fb_ids::new_v7()), "primary_key_immutable"),
            (format!("DELETE FROM bankfeeds_transaction WHERE id='{}'", i.id), "bankfeeds_retained"),
            ("DELETE FROM bankfeeds_account".into(), "bankfeeds_retained"),
            ("DELETE FROM bankfeeds_connection".into(), "bankfeeds_retained"),
            (format!("UPDATE bankfeeds_account SET ledger_account_id='{}' WHERE id='{}'", f.a["100"].to_db(), savings), "bankfeeds_mapping"),
            (format!("UPDATE bankfeeds_account SET ledger_account_id='{}' WHERE id='{}'", f.a["110"].to_db(), savings), "bankfeeds_mapping"),
            (format!("UPDATE bankfeeds_account SET ledger_account_id='{}' WHERE id='{}'", f.a["500"].to_db(), savings), "bankfeeds_mapping"),
            (format!("UPDATE bankfeeds_account SET ledger_account_id='{}' WHERE id='{}'", f.other_expense.to_db(), savings), "bankfeeds_mapping"),
            (format!("UPDATE bankfeeds_account SET remote_id='X' WHERE id='{}'", checking), "bankfeeds_account_fixed"),
            // FIX5: the displayed id may repeat (two raw ids can redact alike); identity keys may not.
            (format!("UPDATE bankfeeds_transaction SET remote_id='TX-1002' WHERE id='{}'", i.id), "bankfeeds_transaction_fixed"),
            (format!("UPDATE bankfeeds_transaction SET remote_identity=(SELECT remote_identity FROM bankfeeds_transaction WHERE remote_id='TX-1002') WHERE id='{}'", i.id), "bankfeeds_key"),
            (format!("UPDATE OR REPLACE bankfeeds_transaction SET remote_identity=(SELECT remote_identity FROM bankfeeds_transaction WHERE remote_id='TX-1002') WHERE id='{}'", i.id), "bankfeeds_key"),
            (format!("UPDATE bankfeeds_transaction SET ordinal=99 WHERE id='{}'", i.id), "bankfeeds_transaction_fixed"),
            (format!("UPDATE bankfeeds_transaction SET status='MATCHED',accepted_as='MATCH',journal_id='{journal}',journal_line_id='{line}',decided_at='2026-10-28T00:00:00.000000Z',decided_by_id='{}' WHERE id='{}'", f.u.to_db(), i.id), "bankfeeds_key|bankfeeds_decision"),
            (format!("UPDATE bankfeeds_transaction SET amount=1 WHERE id='{}'", done.id), "bankfeeds_decided_values"),
            (format!("UPDATE bankfeeds_transaction SET status='NEW',accepted_as='',journal_id=NULL,journal_line_id=NULL,reference='',reference_url='',decided_at=NULL,decided_by_id=NULL,return_count=1 WHERE id='{}'", done.id), "bankfeeds_return"),
            (format!("UPDATE bankfeeds_transaction SET return_count=5 WHERE id='{}'", i.id), "bankfeeds_return_count"),
            (format!("DELETE FROM ledger_journalline WHERE id='{line}'"), "bankfeeds_parent_kept"),
            (format!("DELETE FROM ledger_account WHERE id='{}'", f.a["220"].to_db()), "bankfeeds_parent_kept"),
            (format!("UPDATE bankfeeds_connection SET label='x' WHERE id='{}'", f.conn), "bankfeeds_connection_fixed"),
            (format!("UPDATE bankfeeds_connection SET sync_count=0 WHERE id='{}'", f.conn), "bankfeeds_connection_sync"),
            (format!("UPDATE bankfeeds_connection SET active=0 WHERE id='{}'", f.conn), "bankfeeds_connection_forgotten"),
            ("UPDATE bankfeeds_counter SET value=1".into(), "bankfeeds_ordinal"),
            ("DELETE FROM bankfeeds_counter".into(), "bankfeeds_ordinal"),
            (format!("INSERT INTO bankfeeds_transaction(id,ordinal,feed_account_id,remote_id,posted_on,transacted_on,amount,description,pending,first_seen_at,updated_at,status,accepted_as,journal_id,journal_line_id,reference,reference_url,decided_at,decided_by_id,return_count) VALUES('{}',3,'{checking}','NEWX','2026-10-01',NULL,1,'',0,'2026-10-28T00:00:00.000000Z','2026-10-28T00:00:00.000000Z','NEW','',NULL,NULL,'','',NULL,NULL,0)", fb_ids::new_v7()), "bankfeeds_key"),
            (format!("INSERT INTO bankfeeds_transaction(id,ordinal,feed_account_id,remote_id,posted_on,transacted_on,amount,description,pending,first_seen_at,updated_at,status,accepted_as,journal_id,journal_line_id,reference,reference_url,decided_at,decided_by_id,return_count) VALUES('{}',50,'{checking}','NEWX','2026-10-01',NULL,1,'',0,'2026-10-28T00:00:00.000000Z','2026-10-28T00:00:00.000000Z','NEW','',NULL,NULL,'','',NULL,NULL,0)", fb_ids::new_v7()), "bankfeeds_ordinal"),
            (format!("INSERT INTO bankfeeds_rule(id,company_id,position,name,match_text,sign,amount_min,amount_max,account_id,payee_name,active,created_at,updated_at) VALUES('{}','{}',1,'r','x','ANY',NULL,NULL,'{}','',1,'2026-10-28T00:00:00.000000Z','2026-10-28T00:00:00.000000Z')", fb_ids::new_v7(), f.c.to_db(), f.a["110"].to_db()), "bankfeeds_rule_account"),
            (format!("INSERT INTO bankfeeds_rule(id,company_id,position,name,match_text,sign,amount_min,amount_max,account_id,payee_name,active,created_at,updated_at) VALUES('{}','{}',1,'r','x','ANY',NULL,NULL,'{}','',1,'2026-10-28T00:00:00.000000Z','2026-10-28T00:00:00.000000Z')", fb_ids::new_v7(), f.c.to_db(), f.other_expense.to_db()), "bankfeeds_rule_account"),
            (format!("INSERT INTO bankfeeds_connection(id,company_id,label,host,active,created_by_id,created_at,forgotten_at,last_sync_at,last_sync_ok,last_sync_summary,sync_count) VALUES('{}','{}','x','h',1,'{}','2026-10-28T00:00:00.000000Z',NULL,NULL,NULL,'',0)", fb_ids::new_v7(), f.c.to_db(), f.outsider.to_db()), "bankfeeds_connection_scope"),
        ];
        for (sql, guard) in cases {
            q.execute_batch("BEGIN IMMEDIATE").unwrap();
            let got = raw_err(&f, &sql);
            q.execute_batch("ROLLBACK").unwrap();
            assert!(guard.split('|').any(|g| g == got), "{sql}: {got} not {guard}");
        }
    }
    q.execute_batch("PRAGMA recursive_triggers=ON").unwrap();
    f.reopen_clean();
}

#[test]
fn decision_guard_refuses_a_line_on_another_account_or_direction() {
    let mut f = Fx::new();
    f.connected_books();
    let j = f.journal("2026-10-01", "500", "100", "84.19");
    let i = f.item("TX-1001");
    let q = f.book.connection();
    let (bank_line, expense_line): (String, String) = q
        .query_row("SELECT (SELECT id FROM ledger_journalline WHERE journal_id=?1 AND credit>0),(SELECT id FROM ledger_journalline WHERE journal_id=?1 AND debit>0)", [&j], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    let decide = |line: &str, kind: &str| {
        format!("UPDATE bankfeeds_transaction SET status='{}',accepted_as='{kind}',journal_id='{j}',journal_line_id='{line}',decided_at='2026-10-28T00:00:00.000000Z',decided_by_id='{}' WHERE id='{}'", if kind == "MATCH" { "MATCHED" } else { "ACCEPTED" }, f.u.to_db(), i.id)
    };
    for (line, kind) in [(&expense_line, "MATCH"), (&bank_line, "CHECK"), (&bank_line, "DEPOSIT")] {
        q.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert_eq!(raw_err(&f, &decide(line, kind)), "bankfeeds_decision");
        q.execute_batch("ROLLBACK").unwrap();
    }
    q.execute_batch("BEGIN IMMEDIATE").unwrap();
    q.execute_batch(&decide(&bank_line, "MATCH")).unwrap();
    q.execute_batch("ROLLBACK").unwrap();
    // A pending item cannot be decided even through SQL.
    q.execute_batch(&format!("BEGIN IMMEDIATE;UPDATE bankfeeds_transaction SET pending=1 WHERE id='{}'", i.id)).unwrap();
    assert_eq!(raw_err(&f, &decide(&bank_line, "MATCH")), "bankfeeds_decision");
    q.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn open_refuses_externally_introduced_bad_decisions_and_missing_guards() {
    let mut f = Fx::new();
    f.connected_books();
    let done = f.accept("TX-1006", json!({"kind":"JOURNAL","account_id":f.a["410"].to_db()})).unwrap();
    drop(f.book);
    let book = Book::open(&f.path, &[91; 32]).unwrap();
    {
        let q = book.connection();
        q.execute_batch("PRAGMA foreign_keys=OFF;BEGIN;DROP TRIGGER fb_bankfeeds_transaction_update;").unwrap();
        q.execute(&format!("UPDATE bankfeeds_transaction SET amount=999 WHERE id='{}'", done.id), []).unwrap();
        q.execute_batch("COMMIT;PRAGMA foreign_keys=ON").unwrap();
    }
    let problems = book.check().unwrap();
    assert!(problems.iter().any(|p| p.contains("bank-feed")), "{problems:?}");
    assert!(problems.iter().any(|p| p.contains("fb_bankfeeds_transaction_update")), "{problems:?}");
    drop(book);
    assert!(Book::open(&f.path, &[91; 32]).is_err());
}

#[test]
fn late_failures_roll_back_every_bank_feed_write() {
    let mut f = Fx::new();
    f.connected_books();
    let before = fb_db::test_support::snapshot(f.book.connection()).unwrap();
    let item = f.item("TX-1006");
    // A composed caller that catches the failure and commits must not keep partial rows.
    let r: FbResult<()> = f.book.write(|tx| {
        let inner = tx.atomic(|| -> FbResult<()> {
            tx.execute("UPDATE bankfeeds_rule SET name=name", [])?;
            Err(err("late failure"))
        });
        assert!(inner.is_err());
        Ok(())
    });
    r.unwrap();
    assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(), before);
    // An outsider's accept is refused before any row is written.
    assert_eq!(accept(&mut f.book, f.outsider, f.c, &item.id, &json!({"kind":"JOURNAL","account_id":f.a["410"].to_db()}), &SecretSet::none()).unwrap_err().kind(), "PermissionDenied");
    assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(), before);
}

#[test]
fn populated_int_l3_book_upgrades_to_bank_feeds_without_changing_a_row() {
    // The integrated main prefix is every migration before 0133: a populated book with posted journals, a check,
    // a sales receipt, setup and audit history upgrades with every row and key intact, then accepts feeds.
    let path = std::env::temp_dir().join(format!("bankfeeds-prior-{}-{}.fbbook", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
    let mut book = Book::create(&path, &[91; 32]).unwrap();
    let u = users::create_user(&mut book, "prior", true).unwrap();
    let c = services::create_company(&mut book, u, "Prior books", "USD", false).unwrap().id;
    let mut make = |code: &str, kind: &str, detail: &str| services::save_account(&mut book, u, c, AccountInput::new(code, code).kind(kind).detail(detail)).unwrap().id;
    let (bank, uf, income, expense) = (make("100", "ASSET", "BANK"), make("105", "ASSET", "OTHER_CURRENT_ASSET"), make("400", "INCOME", "INCOME"), make("500", "EXPENSE", "EXPENSE"));
    documents::save_setup(&mut book, u, c, &json!({"undeposited":uf.to_db(),"banks":[bank.to_db()]}), Some(0)).unwrap();
    let j = services::save_draft(&mut book, u, c, services::DraftInput::new("2026-10-01", "Opening", vec![json!({"account":bank.to_db(),"debit":"10.00"}), json!({"account":income.to_db(),"credit":"10.00"})])).unwrap();
    services::post_journal(&mut book, u, c, j.id, None).unwrap();
    let chk = checks::save_check(&mut book, u, c, &json!({"bank_id":bank.to_db(),"payee_kind":"OTHER","payee_name":"Prior payee","date":"2026-10-02","check_number":"7","request_key":fb_ids::new_v7().to_string(),"lines":[{"tab":"EXPENSE","account_id":expense.to_db(),"rate":"1.00"}]})).unwrap();
    let rev = checks::review_check(&mut book, u, c, chk["id"].as_str().unwrap(), chk["revision"].as_i64()).unwrap();
    checks::record_check(&mut book, u, c, chk["id"].as_str().unwrap(), chk["revision"].as_i64(), &rev).unwrap();
    let copy_path = path.with_extension("prior-0133.fbbook");
    let copy = fb_db::test_support::populated_copy(book.connection(), &copy_path, &[91; 32], "0133_bankfeeds", false).unwrap();
    let mut upgraded = Book::open(&copy_path, &[91; 32]).unwrap();
    assert_eq!(fb_db::test_support::snapshot(upgraded.connection()).unwrap(), copy.expected);
    assert!(upgraded.check().unwrap().is_empty());
    let conn = connect(&mut upgraded, u, c, &json!(""), "bridge.simplefin.org", &SecretSet::none()).unwrap();
    let mut pull = demo();
    pull.accounts.truncate(1);
    store_pull(&mut upgraded, u, c, conn["id"].as_str().unwrap(), &pull, NOW, false, None, &SecretSet::none()).unwrap();
    assert!(upgraded.check().unwrap().is_empty());
    drop(upgraded);
    assert!(Book::open(&copy_path, &[91; 32]).unwrap().check().unwrap().is_empty());
}

#[test]
fn review_sql_rejects_non_uuid_primary_key() {
    let f = Fx::new();
    let result = f.book.connection().execute("INSERT INTO bankfeeds_connection(id,company_id,label,host,active,created_by_id,created_at,last_sync_summary,sync_count) VALUES(?1,?2,'invalid key','bridge.example',1,?3,'2026-10-28T12:00:00.000000Z','',0)", params!["x".repeat(36),f.c.to_db(),f.u.to_db()]);
    assert!(result.is_err(), "SQL accepted a 36-byte non-UUID connection identity");
}

#[test]
fn review_sql_rejects_nul_suffix_primary_key() {
    let f = Fx::new();
    let key = format!("{}\0suffix", fb_ids::new_v7());
    let result = f.book.connection().execute("INSERT INTO bankfeeds_connection(id,company_id,label,host,active,created_by_id,created_at,last_sync_summary,sync_count) VALUES(?1,?2,'invalid key','bridge.example',1,?3,'2026-10-28T12:00:00.000000Z','',0)", params![key,f.c.to_db(),f.u.to_db()]);
    assert!(result.is_err(), "SQL accepted a UUID with a NUL suffix");
}

#[test]
fn review_sql_rejects_invalid_timestamp() {
    let mut f = Fx::new();
    f.connected_books();
    let result = f.book.connection().execute("UPDATE bankfeeds_account SET last_seen_at='2026-02-30T12:00:00.000000Z'", []);
    if result.is_ok() { assert!(f.book.check().unwrap().is_empty(), "already detected at check"); }
    assert!(result.is_err(), "SQL and open-time integrity accepted an impossible timestamp");
}

#[test]
fn review_sql_rejects_year_zero_date() {
    let mut f = Fx::new();
    f.connected_books();
    assert!(fb_db::dates::parse_iso_date("0000-01-01").is_none());
    let result = f.book.connection().execute("UPDATE bankfeeds_transaction SET posted_on='0000-01-01' WHERE remote_id='TX-1001'", []);
    if result.is_ok() { assert!(f.book.check().unwrap().is_empty(), "already detected at check"); }
    assert!(result.is_err(), "SQL accepted a date the shared Python-exact parser rejects");
}

#[test]
fn review_sql_rejects_nul_hidden_description() {
    let mut f = Fx::new();
    f.connected_books();
    let value = format!("ok\0{}", "x".repeat(600));
    let result = f.book.connection().execute("UPDATE bankfeeds_transaction SET description=?1 WHERE remote_id='TX-1001'", [value]);
    assert!(result.is_err(), "SQL accepted more than 500 characters hidden after NUL");
}

#[test]
fn review_ignore_requires_eligible_decider() {
    let mut f = Fx::new();
    f.connected_books();
    let i = f.item("TX-1001");
    let result = f.book.connection().execute("UPDATE bankfeeds_transaction SET status='IGNORED',decided_at='2026-10-28T12:00:00.000000Z',decided_by_id=?1 WHERE id=?2", params![f.outsider.to_db(),i.id]);
    assert!(result.is_err(), "SQL accepted a nonmember as decision actor");
}

#[test]
fn review_reactivation_cannot_duplicate_mapping() {
    let mut f = Fx::new();
    f.connected_books();
    let checking = f.feed("ACT-checking");
    let savings = f.feed("ACT-savings");
    f.book.connection().execute("UPDATE bankfeeds_account SET active=0 WHERE id=?1", [&checking]).unwrap();
    map_account(&mut f.book,f.u,f.c,&savings,&json!(f.a["100"].to_db()), &SecretSet::none()).unwrap();
    let result = f.book.connection().execute("UPDATE bankfeeds_account SET active=1 WHERE id=?1", [&checking]);
    assert!(result.is_err(), "reactivation created two active mappings to one ledger account");
}

#[test]
fn review_begin_sync_does_not_panic_on_interval_overflow() {
    let mut f = Fx::new();
    f.connected_books();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| begin_sync(&mut f.book,f.u,f.c,&f.conn,"2026-10-28T12:30:00Z",i64::MAX,false)));
    assert!(result.is_ok(), "public begin_sync panicked on untrusted interval");
    assert!(result.unwrap().is_err(), "overflow must return a controlled error");
}

#[test]
fn review_check_accept_late_audit_failure_does_not_post() {
    let mut f = Fx::new();
    f.connected_books();
    f.book.connection().execute_batch("CREATE TRIGGER review_fail_decision_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='bankfeed.accepted' BEGIN SELECT RAISE(ABORT,'review_injected_audit_failure'); END;").unwrap();
    let result = f.accept("TX-1001", json!({"kind":"CHECK","account_id":f.a["500"].to_db()}));
    assert!(result.is_err());
    assert_eq!(f.item("TX-1001").status,"NEW");
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'"),0,"failed acceptance committed a posted check");
}

#[test]
fn review_check_number_uses_python_regex_grammar() {
    let mut f = Fx::new();
    f.connected_books();
    let mut item = f.item("TX-1001");
    item.ordinal = 42;
    let vectors: Vec<(String,String)> = serde_json::from_str(include_str!("testdata/check-number-vectors.json")).unwrap();
    for (input, expected) in vectors {
        item.description = input.clone();
        assert_eq!(default_check_number(&item),expected,"{input:?}");
    }
}

#[test]
fn review_redaction_matches_python_on_malformed_authority() {
    let vectors: Vec<(String,String)> = serde_json::from_str(include_str!("testdata/redact-vectors.json")).unwrap();
    for (input,expected) in vectors { assert_eq!(text::redact(&input),expected,"{input:?}"); }
}

#[test]
fn review_rule_lowercase_matches_frozen_python_unicode() {
    let vectors: Vec<(String,String)> = serde_json::from_str(include_str!("testdata/lower-vectors.json")).unwrap();
    for (input,expected) in vectors { assert_eq!(text::py_lower(&input),expected,"{input:?}"); }
}

#[test]
fn review_deposit_accept_late_audit_failure_does_not_post() {
    let mut f = Fx::new();
    f.connected_books();
    let receipt = f.undeposited("2500.00", "2026-10-04");
    let before = f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'");
    f.book.connection().execute_batch("CREATE TRIGGER review_fail_decision_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='bankfeed.accepted' BEGIN SELECT RAISE(ABORT,'review_injected_audit_failure'); END;").unwrap();
    let result = f.accept("TX-1003", json!({"kind":"DEPOSIT","receipt_ids":[receipt]}));
    assert!(result.is_err());
    assert_eq!(f.item("TX-1003").status,"NEW");
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'"),before,"failed acceptance committed a deposit");
}

#[test]
fn review_sql_match_obeys_same_date_window_as_service() {
    let mut f = Fx::new();
    f.connected_books();
    let journal = f.journal("2026-01-01","100","410","3.12");
    let item = f.item("TX-1006");
    let line: String = f.book.connection().query_row("SELECT id FROM ledger_journalline WHERE journal_id=?1 AND debit=312",[&journal],|r|r.get(0)).unwrap();
    assert!(msg(f.accept("TX-1006",json!({"kind":"MATCH","journal_line_id":line}))).contains("within 5 days"));
    let result = f.book.connection().execute("UPDATE bankfeeds_transaction SET status='MATCHED',accepted_as='MATCH',journal_id=?1,journal_line_id=?2,reference='JE 1',reference_url='journal:review',decided_at='2026-10-28T12:00:00.000000Z',decided_by_id=?3 WHERE id=?4",params![journal,line,f.u.to_db(),item.id]);
    assert!(result.is_err(),"SQL matched a transaction to an entry nine months away");
}

#[test]
fn review_sql_accepted_decision_cannot_name_reversed_journal() {
    let mut f = Fx::new();
    f.connected_books();
    let journal = f.journal("2026-10-19","100","410","3.12");
    let item = f.item("TX-1006");
    let line: String = f.book.connection().query_row("SELECT id FROM ledger_journalline WHERE journal_id=?1 AND debit=312",[&journal],|r|r.get(0)).unwrap();
    services::reverse_journal(&mut f.book,f.u,f.c,journal.parse().unwrap(),"2026-10-20","Review reversal").unwrap();
    let result = f.book.connection().execute("UPDATE bankfeeds_transaction SET status='ACCEPTED',accepted_as='JOURNAL',journal_id=?1,journal_line_id=?2,reference='JE 1',reference_url='journal:review',decided_at='2026-10-28T12:00:00.000000Z',decided_by_id=?3 WHERE id=?4",params![journal,line,f.u.to_db(),item.id]);
    assert!(result.is_err(),"SQL accepted an already reversed journal");
}

#[test]
fn review_rule_matching_keeps_python_unicode_version() {
    let mut f = Fx::new();
    f.connected_books();
    let mut item = f.item("TX-1001");
    item.description = "\u{1c8a}".into();
    let rule = save_rule(&mut f.book,f.u,f.c,&json!({"name":"New Unicode capital","match_text":"\u{1c89}","account_id":f.a["500"].to_db()}), &SecretSet::none()).unwrap();
    assert!(rule_for(&[rule],&item).is_none(),"Rust folds letters unknown to frozen Python 3.13 Unicode 15.1");
}

#[test]
fn review_redaction_never_keeps_credentials_after_combining_mark() {
    let input = "\u{301}https://review-user:review-pass@bridge.example/x";
    let clean = text::sanitize(input,500);
    let redaction_leaked = clean.contains("review-user:review-pass");
    let mut f = Fx::new();
    record_sync_failure(&mut f.book,f.u,f.c,&f.conn,NOW,input,false, &SecretSet::none()).unwrap();
    let leaks: i64 = f.book.connection().query_row("SELECT count(*) FROM bankfeeds_connection WHERE instr(last_sync_summary,'review-user:review-pass')>0",[],|r|r.get(0)).unwrap();
    assert_eq!(leaks,0,"sync summary persisted credential text");
    assert!(!redaction_leaked,"credential redaction was skipped after a combining mark");
}

#[test]
fn review_ordinary_account_edit_does_not_make_book_unopenable() {
    let mut f = Fx::new();
    f.sync(&demo());
    let account = services::save_account(&mut f.book,f.u,f.c,AccountInput::new("150","Unused mapped asset").kind("ASSET")).unwrap().id;
    let feed_id = f.feed("ACT-checking");
    map_account(&mut f.book,f.u,f.c,&feed_id,&json!(account.to_db()), &SecretSet::none()).unwrap();
    services::save_account(&mut f.book,f.u,f.c,AccountInput::new("150","Reclassified unused account").kind("INCOME").detail("INCOME").edit(account)).unwrap();
    let violations = f.book.check().unwrap();
    assert!(Book::open(&f.path,&[91;32]).is_ok(),"ordinary account edit made book unopenable");
    assert!(violations.is_empty(),"ordinary successful account edit broke bankfeeds integrity: {violations:?}");
}

#[test]
fn review_bridge_staging_writes_have_audit_evidence() {
    let mut f = Fx::new();
    f.sync(&demo());
    let audits = f.count("SELECT count(*) FROM ledger_auditevent");
    stage_bank_transactions(&mut f.book,f.u,f.c,&f.conn,&[("ACT-checking".into(),tx("REVIEW-STAGE","2026-10-28",100,"New staged transaction"))], &SecretSet::none()).unwrap();
    assert!(f.count("SELECT count(*) FROM ledger_auditevent")>audits,"public bridge staging changed the books without any audit evidence");
}

#[test]
fn review_new_tables_replacement_and_hidden_identity_matrix() {
    let mut f = Fx::new();
    f.connected_books();
    save_rule(&mut f.book,f.u,f.c,&json!({"name":"Review","match_text":"review","account_id":f.a["500"].to_db()}), &SecretSet::none()).unwrap();
    for recursion in [0,1] {
        f.book.connection().execute_batch(&format!("PRAGMA recursive_triggers={recursion}")).unwrap();
        for table in ["bankfeeds_connection","bankfeeds_account","bankfeeds_transaction","bankfeeds_rule","bankfeeds_counter"] {
            let before = fb_db::test_support::snapshot(f.book.connection()).unwrap();
            for alias in ["rowid","_rowid_","oid"] {
                assert!(f.book.connection().query_row(&format!("SELECT {alias} FROM {table}"),[],|r|r.get::<_,i64>(0)).is_err(),"{table} exposes {alias}");
            }
            assert!(f.book.connection().execute(&format!("INSERT OR REPLACE INTO {table} SELECT * FROM {table} LIMIT 1"),[]).is_err(),"{table} allowed replacement");
            let primary = if table=="bankfeeds_counter" {"name"} else {"id"};
            let key: String = f.book.connection().query_row(&format!("SELECT {primary} FROM {table} LIMIT 1"),[],|r|r.get(0)).unwrap();
            for verb in ["UPDATE","UPDATE OR REPLACE"] {
                assert!(f.book.connection().execute(&format!("{verb} {table} SET {primary}=?1 WHERE {primary}=?2"),params![fb_ids::new_v7().to_string(),key]).is_err(),"{table} allowed fresh primary key through {verb}");
            }
            assert!(f.book.connection().execute(&format!("DELETE FROM {table} WHERE {primary}=?1"),[key]).is_err(),"{table} allowed deleting retained evidence");
            assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(),before,"{table} changed after replacement refusal");
        }
    }
    f.book.connection().execute_batch("PRAGMA recursive_triggers=ON").unwrap();
    f.reopen_clean();
}

#[test]
fn review_remaining_mutations_roll_back_after_late_audit_failure() {
    for op in ["connect","forget","map","failure","store","journal","transfer","match","ignore","return","rule-create","rule-edit","rule-move","rule-active"] {
        let mut f = Fx::new();
        f.connected_books();
        let rule_input = json!({"name":"Atomic rule","match_text":"office","account_id":f.a["500"].to_db()});
        let rule = save_rule(&mut f.book,f.u,f.c,&rule_input, &SecretSet::none()).unwrap();
        save_rule(&mut f.book,f.u,f.c,&json!({"name":"Second rule","match_text":"acme","account_id":f.a["500"].to_db()}), &SecretSet::none()).unwrap();
        let target = f.item("TX-1006");
        if op=="return" { ignore(&mut f.book,f.u,f.c,&target.id, &SecretSet::none()).unwrap(); }
        let mut matching = String::new();
        if op=="match" {
            let j = f.journal("2026-10-19","100","410","3.12");
            matching = f.book.connection().query_row("SELECT id FROM ledger_journalline WHERE journal_id=?1 AND debit=312",[j],|r|r.get(0)).unwrap();
        }
        let feed_id = f.feed("ACT-checking");
        let before = fb_db::test_support::snapshot(f.book.connection()).unwrap();
        f.book.connection().execute_batch("CREATE TRIGGER review_fail_bank_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action LIKE 'bankfeed.%' BEGIN SELECT RAISE(ABORT,'review_injected_audit_failure'); END;").unwrap();
        let result = match op {
            "connect" => connect(&mut f.book,f.u,f.c,&json!("Second connection"),"bridge.example", &SecretSet::none()).map(|_|()),
            "forget" => forget(&mut f.book,f.u,f.c,&f.conn, &SecretSet::none()).map(|_|()),
            "map" => map_account(&mut f.book,f.u,f.c,&feed_id,&Value::Null, &SecretSet::none()).map(|_|()),
            "failure" => record_sync_failure(&mut f.book,f.u,f.c,&f.conn,NOW,"Synthetic failure",false, &SecretSet::none()),
            "store" => store_pull(&mut f.book,f.u,f.c,&f.conn,&demo(),NOW,false,None, &SecretSet::none()).map(|_|()),
            "journal" => f.accept("TX-1006",json!({"kind":"JOURNAL","account_id":f.a["410"].to_db()})).map(|_|()),
            "transfer" => f.accept("TX-1005",json!({"kind":"TRANSFER","transfer_account_id":f.a["101"].to_db()})).map(|_|()),
            "match" => f.accept("TX-1006",json!({"kind":"MATCH","journal_line_id":matching})).map(|_|()),
            "ignore" => ignore(&mut f.book,f.u,f.c,&target.id, &SecretSet::none()).map(|_|()),
            "return" => return_to_review(&mut f.book,f.u,f.c,&target.id, &SecretSet::none()).map(|_|()),
            "rule-create" => save_rule(&mut f.book,f.u,f.c,&rule_input, &SecretSet::none()).map(|_|()),
            "rule-edit" => {
                let mut edit = rule_input.clone(); edit["rule_id"]=json!(rule.id); edit["name"]=json!("Edited");
                save_rule(&mut f.book,f.u,f.c,&edit, &SecretSet::none()).map(|_|())
            },
            "rule-move" => move_rule(&mut f.book,f.u,f.c,&rule.id,&json!("down"), &SecretSet::none()).map(|_|()),
            "rule-active" => set_rule_active(&mut f.book,f.u,f.c,&rule.id,&json!(false), &SecretSet::none()).map(|_|()),
            _ => unreachable!()
        };
        let error = result.expect_err(op);
        assert!(error.to_string().contains("review_injected_audit_failure"),"{op} failed before injected audit: {error}");
        assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(),before,"{op} left partial rows");
        f.book.connection().execute_batch("DROP TRIGGER review_fail_bank_audit").unwrap();
        f.reopen_clean();
    }
}

#[test]
fn review_rule_absolute_amount_does_not_wrap() {
    let mut f = Fx::new();
    f.connected_books();
    let mut item = f.item("TX-1001");
    item.amount=i64::MIN;
    let rule=save_rule(&mut f.book,f.u,f.c,&json!({"name":"Only zero","match_text":"office","amount_max":"0","account_id":f.a["500"].to_db()}), &SecretSet::none()).unwrap();
    assert!(rule_for(&[rule],&item).is_none(),"absolute i64::MIN wrapped negative and matched a zero-maximum rule");
}

#[test]
fn review_review_list_keeps_python_null_date_order() {
    let mut f=Fx::new();
    let mut pull=demo();
    pull.accounts[0].transactions.push(StagedTransaction { posted_on:None,transacted_on:None,pending:true,..tx("REVIEW-UNDATED","2026-10-28",100,"Pending without date") });
    f.sync(&pull);
    let listed=items(&mut f.book,f.u,f.c).unwrap();
    assert_eq!(listed[0].remote_id,"REVIEW-UNDATED","Python/PostgreSQL DESC puts NULL posted_on first");
}

#[test]
fn review_withdrawn_audited_transaction_identity_cannot_be_reused() {
    let mut f=Fx::new();
    let mut pull=demo();
    pull.accounts[0].transactions.push(StagedTransaction { posted_on:None,pending:true,..tx("REVIEW-PENDING","2026-10-28",100,"Pending") });
    f.sync(&pull);
    let item=f.item("REVIEW-PENDING");
    ignore(&mut f.book,f.u,f.c,&item.id, &SecretSet::none()).unwrap();
    return_to_review(&mut f.book,f.u,f.c,&item.id, &SecretSet::none()).unwrap();
    f.sync(&demo());
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_transaction WHERE remote_id='REVIEW-PENDING'"),0);
    let target=format!("bankfeedtransaction:{}",item.id);
    let audits:i64=f.book.connection().query_row("SELECT count(*) FROM ledger_auditevent WHERE target=?1",[target],|r|r.get(0)).unwrap();
    assert_eq!(audits,2);
    let result=f.book.connection().execute("INSERT INTO bankfeeds_transaction(id,ordinal,feed_account_id,remote_id,posted_on,amount,description,pending,first_seen_at,updated_at,status,accepted_as,reference,reference_url,return_count) SELECT ?1,value+1,?2,'REPLACEMENT','2026-10-28',1,'Different transaction',0,'2026-10-28T12:00:00.000000Z','2026-10-28T12:00:00.000000Z','NEW','','','',0 FROM bankfeeds_counter WHERE name='transaction'",params![item.id,item.feed_account_id]);
    if result.is_ok() { f.reopen_clean(); }
    assert!(result.is_err(),"withdrawal allowed reusing an audited UUID for a different transaction");
}

// --- FIX1 (Sol's review of e2d7d51) -------------------------------------------------------------------

/// Run one acceptance on a fresh book and list the audit actions and tables it writes, so the injection
/// test below can fail each of those writes in turn.
fn fix1_accept_case(kind: &str) -> (Fx, &'static str, Value) {
    let mut f = Fx::new();
    f.connected_books();
    let (remote, input) = match kind {
        "JOURNAL" => ("TX-1006", json!({"kind":"JOURNAL","account_id":f.acc("410")})),
        "TRANSFER" => ("TX-1005", json!({"kind":"TRANSFER","transfer_account_id":f.acc("101")})),
        "CHECK" => ("TX-1001", json!({"kind":"CHECK","account_id":f.acc("500")})),
        "DEPOSIT" => {
            let receipt = f.undeposited("2500.00", "2026-10-04");
            ("TX-1003", json!({"kind":"DEPOSIT","receipt_ids":[receipt]}))
        }
        _ => {
            let j = f.journal("2026-10-19", "100", "410", "3.12");
            let line: String = f.book.connection().query_row("SELECT id FROM ledger_journalline WHERE journal_id=?1 AND debit=312", [j], |r| r.get(0)).unwrap();
            ("TX-1006", json!({"kind":"MATCH","journal_line_id":line}))
        }
    };
    (f, remote, input)
}

#[test]
fn fix1_acceptance_commits_nothing_when_any_later_write_fails() {
    for kind in ["JOURNAL", "TRANSFER", "CHECK", "DEPOSIT", "MATCH"] {
        // the writes a successful acceptance makes, in order
        let (mut f, remote, input) = fix1_accept_case(kind);
        let audits = f.count("SELECT count(*) FROM ledger_auditevent");
        f.accept(remote, input.clone()).unwrap();
        let actions: Vec<String> = {
            let mut st = f.book.connection().prepare("SELECT action FROM ledger_auditevent ORDER BY occurred_at,id").unwrap();
            st.query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
        };
        let mut steps: Vec<String> = actions[audits as usize..].iter().map(|a| format!("BEFORE INSERT ON ledger_auditevent WHEN NEW.action='{a}'")).collect();
        steps.extend(["BEFORE INSERT ON ledger_journal", "BEFORE INSERT ON ledger_journalline", "BEFORE UPDATE ON bankfeeds_transaction"].map(String::from));
        assert!(steps.len() >= 4, "{kind}: {steps:?}");
        for step in steps {
            let (mut f, remote, input) = fix1_accept_case(kind);
            let posted = f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'");
            let before = fb_db::test_support::snapshot(f.book.connection()).unwrap();
            f.book.connection().execute_batch(&format!("CREATE TEMP TRIGGER fix1_inject {step} BEGIN SELECT RAISE(ABORT,'fix1_injected'); END;")).unwrap();
            let result = f.accept(remote, input.clone());
            f.book.connection().execute_batch("DROP TRIGGER temp.fix1_inject").unwrap();
            let Err(e) = result else {
                // this write is not on the path for this kind (e.g. MATCH posts no journal)
                assert!(step.contains("ledger_journal"), "{kind} {step}: acceptance succeeded through an injected failure");
                continue;
            };
            assert!(e.to_string().contains("fix1_injected"), "{kind} {step}: {e}");
            assert_eq!(f.item(remote).status, "NEW", "{kind} {step}");
            assert_eq!(f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'"), posted, "{kind} {step}: money committed");
            if kind == "CHECK" {
                // Python's _check_draft commits the draft first; only the draft may remain, and a retry reuses it
                assert_eq!(f.count("SELECT count(*) FROM checks_check WHERE state<>'DRAFT'"), 0, "{kind} {step}");
                f.accept(remote, input.clone()).unwrap();
                assert_eq!(f.count("SELECT count(*) FROM checks_check"), 1, "{kind} {step}: retry made a second check");
            } else {
                assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(), before, "{kind} {step}: partial rows");
                f.accept(remote, input.clone()).unwrap();
            }
            assert_ne!(f.item(remote).status, "NEW", "{kind} {step}: retry did not decide");
            f.reopen_clean();
        }
    }
}

#[test]
fn fix1_every_service_reachable_account_and_setup_change_reopens() {
    let mut f = Fx::new();
    f.connected_books();
    let office = json!({"name":"Office","match_text":"office","account_id":f.acc("500")});
    save_rule(&mut f.book, f.u, f.c, &office, &SecretSet::none()).unwrap();
    let extra = services::save_account(&mut f.book, f.u, f.c, AccountInput::new("150", "Spare bank").kind("ASSET")).unwrap().id;
    let spare = f.feed("ACT-card");
    map_account(&mut f.book, f.u, f.c, &spare, &json!(extra.to_db()), &SecretSet::none()).unwrap();
    f.reopen_clean();
    let edits: Vec<(&str, AccountInput)> = vec![
        ("reclassify an unused mapped asset to income", AccountInput::new("150", "Now income").kind("INCOME").detail("INCOME").edit(extra)),
        ("reclassify it back", AccountInput::new("150", "Spare bank").kind("ASSET").edit(extra)),
        ("deactivate a mapped account", AccountInput::new("150", "Spare bank").kind("ASSET").active(false).edit(extra)),
        ("make a mapped account a header", AccountInput::new("101", "Savings").kind("ASSET").detail("BANK").postable(false).edit(f.a["101"])),
        ("reclassify the rule category", AccountInput::new("500", "Office asset").kind("ASSET").edit(f.a["500"])),
        ("deactivate the rule category", AccountInput::new("500", "Office asset").kind("ASSET").active(false).edit(f.a["500"])),
    ];
    for (why, edit) in edits {
        if services::save_account(&mut f.book, f.u, f.c, edit).is_ok() {
            assert!(Book::open(&f.path, &[91; 32]).is_ok(), "{why} made the book unopenable");
            f.reopen_clean();
        }
    }
    // control designations moving onto a mapped account afterwards
    let setup = json!({"receivables":f.acc("110"),"payables":f.acc("200"),"undeposited":f.acc("100"),"banks":[f.acc("101")]});
    let revision = f.count(&format!("SELECT revision FROM documents_accountingsetup WHERE company_id='{}'", f.c.to_db()));
    if documents::save_setup(&mut f.book, f.u, f.c, &setup, Some(revision)).is_ok() {
        f.reopen_clean();
    }
    // decisions, returns, reversal and forgetting
    let journal = f.accept("TX-1006", json!({"kind":"JOURNAL","account_id":f.acc("410")})).unwrap().journal_id.unwrap();
    services::reverse_journal(&mut f.book, f.u, f.c, journal.parse().unwrap(), "2026-10-20", "FIX1 reopen").unwrap();
    f.reopen_clean();
    let item = f.item("TX-1006");
    return_to_review(&mut f.book, f.u, f.c, &item.id, &SecretSet::none()).unwrap();
    ignore(&mut f.book, f.u, f.c, &item.id, &SecretSet::none()).unwrap();
    f.reopen_clean();
    forget(&mut f.book, f.u, f.c, &f.conn.clone(), &SecretSet::none()).unwrap();
    f.reopen_clean();
}

#[test]
fn fix1_retired_identities_are_permanent_and_backfilled_from_audit() {
    let mut f = Fx::new();
    let mut pull = demo();
    pull.accounts[0].transactions.push(StagedTransaction { posted_on: None, pending: true, ..tx("FIX1-PENDING", "2026-10-28", 100, "Pending") });
    f.sync(&pull);
    let item = f.item("FIX1-PENDING");
    f.sync(&demo());
    let retired: String = f.book.connection().query_row("SELECT id FROM bankfeeds_retired", [], |r| r.get(0)).unwrap();
    assert_eq!(retired, item.id);
    for sql in [
        format!("DELETE FROM bankfeeds_retired WHERE id='{retired}'"),
        format!("UPDATE bankfeeds_retired SET id='{}' WHERE id='{retired}'", fb_ids::new_v7()),
        format!("INSERT OR REPLACE INTO bankfeeds_retired(id,kind) VALUES('{retired}','transaction')"),
        format!("INSERT INTO bankfeeds_retired(id,kind) VALUES('{}','transaction')", f.item("TX-1001").id),
        format!("INSERT INTO bankfeeds_retired(id,kind) VALUES('{}'||char(0),'transaction')", fb_ids::new_v7()),
    ] {
        for recursion in [0, 1] {
            f.book.connection().execute_batch(&format!("PRAGMA recursive_triggers={recursion}")).unwrap();
            assert!(f.book.connection().execute(&sql, []).is_err(), "{sql}");
        }
    }
    f.book.connection().execute_batch("PRAGMA recursive_triggers=ON").unwrap();
    assert!(f.book.connection().query_row("SELECT rowid FROM bankfeeds_retired", [], |r| r.get::<_, i64>(0)).is_err());
    f.reopen_clean();
}

#[test]
fn fix1_open_refuses_live_duplicate_mappings_and_reused_identities() {
    for case in ["duplicate", "reused"] {
        let mut f = Fx::new();
        f.connected_books();
        let conn = f.book.connection();
        conn.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
        // simulate a writer without the guards: drop them only for this raw edit, then put the schema back
        let sql: Vec<(String, String)> = {
            let mut st = conn.prepare("SELECT name,sql FROM sqlite_master WHERE type='trigger' AND tbl_name IN('bankfeeds_account','bankfeeds_transaction','bankfeeds_retired')").unwrap();
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap()
        };
        for (name, _) in &sql {
            conn.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
        }
        if case == "duplicate" {
            conn.execute("UPDATE bankfeeds_account SET ledger_account_id=(SELECT ledger_account_id FROM bankfeeds_account WHERE remote_id='ACT-checking') WHERE remote_id='ACT-savings'", []).unwrap();
        } else {
            let id: String = conn.query_row("SELECT id FROM bankfeeds_transaction WHERE remote_id='TX-1001'", [], |r| r.get(0)).unwrap();
            conn.execute("INSERT INTO bankfeeds_retired(id,kind) VALUES(?1,'transaction')", [id]).ok();
        }
        for (_, create) in &sql {
            conn.execute_batch(create).unwrap();
        }
        assert!(!f.book.check().unwrap().is_empty(), "{case} passed the open-time check");
    }
}

#[test]
fn fix1_populated_0133_book_upgrades_and_retires_audited_withdrawals() {
    // A prior bank-feed layout (through 0133, after main's prefix) with feeds, mappings, a decision, a rule and an audited
    // pending row that was later withdrawn upgrades with every row intact; 0134 retires the withdrawn
    // identity from its audit trail, so the old UUID cannot come back.
    let mut f = Fx::new();
    let mut pull = demo();
    pull.accounts[0].transactions.push(StagedTransaction { posted_on: None, pending: true, ..tx("FIX1-UP", "2026-10-28", 100, "Pending") });
    f.sync(&pull);
    f.map("ACT-checking", Some("100")).unwrap();
    f.map("ACT-savings", Some("101")).unwrap();
    let withdrawn = f.item("FIX1-UP");
    ignore(&mut f.book, f.u, f.c, &withdrawn.id, &SecretSet::none()).unwrap();
    return_to_review(&mut f.book, f.u, f.c, &withdrawn.id, &SecretSet::none()).unwrap();
    f.sync(&demo());
    f.accept("TX-1006", json!({"kind":"JOURNAL","account_id":f.acc("410")})).unwrap();
    let office = json!({"name":"Office","match_text":"office","account_id":f.acc("500")});
    save_rule(&mut f.book, f.u, f.c, &office, &SecretSet::none()).unwrap();
    let copy_path = f.path.with_extension("prior-0134.fbbook");
    let copy = fb_db::test_support::populated_copy(f.book.connection(), &copy_path, &[91; 32], "0134_bankfeeds_hardening", false).unwrap();
    assert!(!copy.before.contains_key("bankfeeds_retired"), "the prior copy has the 0133 layout");
    let mut upgraded = Book::open(&copy_path, &[91; 32]).unwrap();
    let blind = |conn: &fb_db::rusqlite::Connection, snap| fb_db::test_support::identity_blind(conn, snap).unwrap();
    assert_eq!(blind(upgraded.connection(), fb_db::test_support::snapshot(upgraded.connection()).unwrap()), blind(f.book.connection(), copy.expected));
    assert!(upgraded.check().unwrap().is_empty());
    let retired: String = upgraded.connection().query_row("SELECT id FROM bankfeeds_retired", [], |r| r.get(0)).unwrap();
    assert_eq!(retired, withdrawn.id);
    let reuse = upgraded.connection().execute(
        "INSERT INTO bankfeeds_transaction(id,ordinal,feed_account_id,remote_id,posted_on,amount,description,pending,first_seen_at,updated_at,status,accepted_as,reference,reference_url,return_count) SELECT ?1,value+1,?2,'REPLACEMENT','2026-10-28',1,'Different',0,'2026-10-28T12:00:00.000000Z','2026-10-28T12:00:00.000000Z','NEW','','','',0 FROM bankfeeds_counter WHERE name='transaction'",
        params![withdrawn.id, withdrawn.feed_account_id],
    );
    assert!(reuse.is_err(), "an upgraded book let a withdrawn identity come back");
    restore_connection_credentials(&mut upgraded, f.u, f.c, &f.conn.clone(), "https://SafeUserName123:SafePassword123@fixture.test/access", Vec::<String>::new()).unwrap();
    store_pull(&mut upgraded, f.u, f.c, &f.conn.clone(), &demo(), NOW, false, None, &SecretSet::none()).unwrap();
    drop(upgraded);
    assert!(Book::open(&copy_path, &[91; 32]).unwrap().check().unwrap().is_empty());
}

// --- Sol review 2 (d393c70): nine further regressions, copied verbatim ---
#[test]
fn r2_word_adjacent_credentials_never_persist() {
    let mut f = Fx::new();
    let marker = "r2-synthetic-user:r2-synthetic-pass";
    let mut leaked = vec![];
    for prefix in ["x", "_", "9", "é", "\u{301}", "failed<b></b>"] {
        let previous: i64 = f.book.connection().query_row(
            "SELECT count(*) FROM ledger_auditevent WHERE instr(detail,?1)>0", [marker], |r| r.get(0)).unwrap();
        let input = format!("{prefix}https://{marker}@bridge.example/path");
        record_sync_failure(&mut f.book, f.u, f.c, &f.conn, NOW, &input, false, &SecretSet::none()).unwrap();
        let summary: i64 = f.book.connection().query_row(
            "SELECT count(*) FROM bankfeeds_connection WHERE instr(last_sync_summary,?1)>0", [marker], |r| r.get(0)).unwrap();
        let audit: i64 = f.book.connection().query_row(
            "SELECT count(*) FROM ledger_auditevent WHERE instr(detail,?1)>0", [marker], |r| r.get(0)).unwrap();
        if summary > 0 || audit > previous { leaked.push(prefix); }
    }
    f.reopen_clean();
    assert!(leaked.is_empty(), "credential marker persisted after prefixes {leaked:?}");
}

#[test]
fn r2_timestamp_validator_is_total_on_multibyte_input() {
    let value = format!("{}é{}", "x".repeat(9), "x".repeat(16));
    assert_eq!(value.len(),27);
    let got = std::panic::catch_unwind(|| fb_db::bankfeeds_text::timestamp(&value));
    assert!(got.is_ok(), "timestamp validator panicked on non-ASCII text");
    assert!(!got.unwrap());
}

#[test]
fn r2_postdecision_and_second_line_failures_roll_back_and_retry() {
    for kind in ["JOURNAL", "TRANSFER", "CHECK", "DEPOSIT", "MATCH"] {
        for point in ["decision", "second-line", "posted"] {
            if kind == "MATCH" && point != "decision" { continue; }
            let (mut f, remote, input) = fix1_accept_case(kind);
            // CHECK deliberately saves its draft first. Make that allowed draft the baseline.
            if kind == "CHECK" {
                f.book.connection().execute_batch("CREATE TEMP TRIGGER r2_seed BEFORE INSERT ON ledger_auditevent WHEN NEW.action='bankfeed.accepted' BEGIN SELECT RAISE(ABORT,'r2_seed'); END;").unwrap();
                assert!(f.accept(remote,input.clone()).is_err());
                f.book.connection().execute_batch("DROP TRIGGER temp.r2_seed").unwrap();
                assert_eq!(f.count("SELECT count(*) FROM checks_check WHERE state='DRAFT'"),1);
            }
            let before = fb_db::test_support::snapshot(f.book.connection()).unwrap();
            let at = match point {
                "decision" => "AFTER UPDATE ON bankfeeds_transaction WHEN NEW.status IN('ACCEPTED','MATCHED')",
                "second-line" => "AFTER INSERT ON ledger_journalline WHEN (SELECT count(*) FROM ledger_journalline WHERE journal_id=NEW.journal_id)=2",
                _ => "AFTER UPDATE ON ledger_journal WHEN NEW.status='POSTED'",
            };
            f.book.connection().execute_batch(&format!("CREATE TEMP TRIGGER r2_late {at} BEGIN SELECT RAISE(ABORT,'r2_late_failure'); END;")).unwrap();
            let error=f.accept(remote,input.clone()).expect_err("injected write must fail");
            assert!(error.to_string().contains("r2_late_failure"),"wrong failure for {kind}/{point}");
            f.book.connection().execute_batch("DROP TRIGGER temp.r2_late").unwrap();
            assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(),before,"partial rows for {kind}/{point}");
            f.reopen_clean();
            f.accept(remote,input.clone()).unwrap();
            let once = fb_db::test_support::snapshot(f.book.connection()).unwrap();
            f.accept(remote,input).unwrap();
            assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(),once,"retry duplicated rows for {kind}/{point}");
            f.reopen_clean();
        }
    }
}

#[test]
fn r2_every_mapping_and_rule_account_edit_can_reopen() {
    let mut accepted=0;
    for kind in ["ASSET","LIABILITY","EQUITY","INCOME","EXPENSE"] {
        let mut f=Fx::new();
        f.connected_books();
        let spare=services::save_account(&mut f.book,f.u,f.c,AccountInput::new("150","Spare").kind("ASSET")).unwrap().id;
        let feed=f.feed("ACT-card");
        map_account(&mut f.book,f.u,f.c,&feed,&json!(spare.to_db()), &SecretSet::none()).unwrap();
        let rule_input=json!({"name":"Historical category","match_text":"office","account_id":f.acc("500")});
        save_rule(&mut f.book,f.u,f.c,&rule_input, &SecretSet::none()).unwrap();
        for id in [spare,f.a["500"]] {
            let code=if id==spare {"150"} else {"500"};
            for (active,postable) in [(true,true),(false,true),(true,false)] {
                let result=services::save_account(&mut f.book,f.u,f.c,AccountInput::new(code,"Changed").kind(kind).active(active).postable(postable).edit(id));
                if result.is_ok() { accepted+=1; f.reopen_clean(); }
            }
        }
        forget(&mut f.book,f.u,f.c,&f.conn.clone(), &SecretSet::none()).unwrap();
        f.reopen_clean();
    }
    assert!(accepted>=20,"the edit matrix did not exercise enough successful edits: {accepted}");
}

#[test]
fn r2_reactivation_checks_current_mapping_eligibility() {
    let mut f=Fx::new();
    f.connected_books();
    let spare=services::save_account(&mut f.book,f.u,f.c,AccountInput::new("150","Spare").kind("ASSET")).unwrap().id;
    let feed=f.feed("ACT-card");
    map_account(&mut f.book,f.u,f.c,&feed,&json!(spare.to_db()), &SecretSet::none()).unwrap();
    f.book.write(|w| -> FbResult<()> { w.execute("UPDATE bankfeeds_account SET active=0 WHERE id=?1",[&feed])?; Ok(()) }).unwrap();
    services::save_account(&mut f.book,f.u,f.c,AccountInput::new("150","Income now").kind("INCOME").edit(spare)).unwrap();
    f.reopen_clean();
    let outcome=f.book.write(|w| -> FbResult<()> { w.execute("UPDATE bankfeeds_account SET active=1 WHERE id=?1",[&feed])?; Ok(()) });
    assert!(outcome.is_err(),"reactivation accepted a mapping to an income account");
}

#[test]
fn r2_company_setup_and_operator_edits_reopen() {
    let mut f=Fx::new();
    f.connected_books();
    for currency in ["EUR","USD"] {
        services::edit_company(&mut f.book,f.u,f.c,"Changed company",currency).unwrap();
        f.reopen_clean();
    }
    let setup=json!({"receivables":f.acc("110"),"payables":f.acc("200"),"undeposited":f.acc("100"),"banks":[f.acc("101")]});
    let rev=f.count(&format!("SELECT revision FROM documents_accountingsetup WHERE company_id='{}'",f.c.to_db()));
    documents::save_setup(&mut f.book,f.u,f.c,&setup,Some(rev)).unwrap();
    f.reopen_clean();
    users::set_active(&mut f.book,f.u,false).unwrap();
    f.reopen_clean();
    users::set_active(&mut f.book,f.u,true).unwrap();
    let item=f.item("TX-1001");
    ignore(&mut f.book,f.u,f.c,&item.id, &SecretSet::none()).unwrap();
    users::set_active(&mut f.book,f.u,false).unwrap();
    f.reopen_clean();
}

// --- FIX2: reactivation re-runs the whole mapping predicate (0135) ---
#[test]
fn fix2_reactivation_checks_every_mapping_condition_and_books_stay_openable() {
    let mut refused = vec![];
    for case in ["still eligible", "INCOME", "EQUITY", "EXPENSE", "inactive", "header", "control", "currency"] {
        let mut f = Fx::new();
        f.connected_books();
        let spare = services::save_account(&mut f.book, f.u, f.c, AccountInput::new("150", "Spare").kind("ASSET")).unwrap().id;
        let feed = f.feed("ACT-card");
        map_account(&mut f.book, f.u, f.c, &feed, &json!(spare.to_db()), &SecretSet::none()).unwrap();
        f.book.write(|w| -> FbResult<()> { w.execute("UPDATE bankfeeds_account SET active=0 WHERE id=?1", [&feed])?; Ok(()) }).unwrap();
        let edit = AccountInput::new("150", "Spare").edit(spare);
        match case {
            "INCOME" | "EQUITY" | "EXPENSE" => {
                services::save_account(&mut f.book, f.u, f.c, edit.kind(case)).unwrap();
            }
            "inactive" => {
                services::save_account(&mut f.book, f.u, f.c, edit.kind("ASSET").active(false)).unwrap();
            }
            "header" => {
                services::save_account(&mut f.book, f.u, f.c, edit.kind("ASSET").postable(false)).unwrap();
            }
            "control" => {
                let setup = json!({"receivables":f.acc("110"),"payables":f.acc("200"),"undeposited":spare.to_db(),"banks":[f.acc("101")]});
                let rev = f.count(&format!("SELECT revision FROM documents_accountingsetup WHERE company_id='{}'", f.c.to_db()));
                documents::save_setup(&mut f.book, f.u, f.c, &setup, Some(rev)).unwrap();
            }
            "currency" => {
                services::edit_company(&mut f.book, f.u, f.c, "Synthetic", "EUR").unwrap();
            }
            _ => {}
        }
        // The edit itself leaves an openable book; so does a refused reactivation.
        f.reopen_clean();
        let outcome = f.book.write(|w| -> FbResult<()> { w.execute("UPDATE bankfeeds_account SET active=1 WHERE id=?1", [&feed])?; Ok(()) });
        if outcome.is_err() {
            refused.push(case);
        }
        f.reopen_clean();
    }
    assert_eq!(refused, ["INCOME", "EQUITY", "EXPENSE", "inactive", "header", "control", "currency"], "reactivation must refuse exactly the ineligible mappings");
}

#[test]
fn fix2_book_from_0134_upgrades_to_0135_and_opens() {
    let mut f = Fx::new();
    f.connected_books();
    let copy_path = f.path.with_extension("from0134.fbbook");
    // Every migration before 0135: the 0134 layout, populated with this book's rows.
    let copy = fb_db::test_support::populated_copy(f.book.connection(), &copy_path, &[91; 32], "0135_bankfeeds_activation_eligibility", false).unwrap();
    let upgraded = Book::open(&copy_path, &[91; 32]).unwrap();
    let blind = |conn: &fb_db::rusqlite::Connection, snap| fb_db::test_support::identity_blind(conn, snap).unwrap();
    assert_eq!(blind(upgraded.connection(), fb_db::test_support::snapshot(upgraded.connection()).unwrap()), blind(f.book.connection(), copy.expected));
    assert!(upgraded.check().unwrap().is_empty());
    let sql: String = upgraded.connection().query_row("SELECT sql FROM sqlite_master WHERE name='fb_bankfeeds_account_activation'", [], |r| r.get(0)).unwrap();
    assert!(sql.contains("bankfeeds_mappable"), "the upgraded book carries the 0135 activation guard");
    drop(upgraded);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", copy_path.display()));
    }
}
#[test]
fn r3_credentials_with_legal_userinfo_delimiters_never_persist() {
    let mut f = Fx::new();
    let mut failures = vec![];
    // Apostrophe is a legal userinfo sub-delimiter. Also exercise errors/reasons on successful pulls.
    for (index, marker) in ["r3user:r3pa'ss", "r3us'er:r3pass", "r3user:r3pa%27ss", "r3user:r3pa!$&()*+,;=ss"].into_iter().enumerate() {
        for prefix in ["", "x", "<b>x</b>"] {
            let input = format!("{prefix}https://{marker}@bridge.example.test/path");
            record_sync_failure(&mut f.book, f.u, f.c, &f.conn, NOW, &input, false, &SecretSet::none()).unwrap();
            let mut pull = StagedPull::default();
            pull.errors.push(input.clone());
            pull.skipped.push(input.clone());
            let result = f.sync(&pull);
            let summaries: i64 = f.book.connection().query_row("SELECT count(*) FROM bankfeeds_connection WHERE instr(last_sync_summary,?1)>0", [marker], |r| r.get(0)).unwrap();
            let audits: i64 = f.book.connection().query_row("SELECT count(*) FROM ledger_auditevent WHERE instr(detail,?1)>0", [marker], |r| r.get(0)).unwrap();
            if summaries != 0 || audits != 0 || result["summary"].as_str().unwrap().contains(marker) {
                failures.push((index, prefix, summaries, audits));
            }
        }
    }
    f.reopen_clean();
    assert!(failures.is_empty(), "credential persisted: vector/prefix/summary-count/audit-count {failures:?}");
}

#[test]
fn r3_after_final_audit_failure_rolls_back_all_accept_kinds() {
    for kind in ["JOURNAL", "TRANSFER", "CHECK", "DEPOSIT", "MATCH"] {
        let (mut f, remote, input) = fix1_accept_case(kind);
        if kind == "CHECK" {
            f.book.connection().execute_batch("CREATE TEMP TRIGGER r3_seed BEFORE INSERT ON ledger_auditevent WHEN NEW.action='bankfeed.accepted' BEGIN SELECT RAISE(ABORT,'r3_seed'); END;").unwrap();
            assert!(f.accept(remote, input.clone()).unwrap_err().to_string().contains("r3_seed"));
            f.book.connection().execute_batch("DROP TRIGGER temp.r3_seed").unwrap();
        }
        let baseline = fb_db::test_support::snapshot(f.book.connection()).unwrap();
        f.book.connection().execute_batch("CREATE TEMP TRIGGER r3_audit AFTER INSERT ON ledger_auditevent WHEN NEW.action='bankfeed.accepted' BEGIN SELECT RAISE(ABORT,'r3_final_audit'); END;").unwrap();
        assert!(f.accept(remote, input.clone()).unwrap_err().to_string().contains("r3_final_audit"));
        f.book.connection().execute_batch("DROP TRIGGER temp.r3_audit").unwrap();
        assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(), baseline, "{kind}");
        f.reopen_clean();
        f.accept(remote, input.clone()).unwrap();
        f.reopen_clean();
        let once = fb_db::test_support::snapshot(f.book.connection()).unwrap();
        f.accept(remote, input).unwrap();
        assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(), once);
    }
}

#[test]
fn r3_service_edits_after_every_accept_kind_reopen() {
    let mut successful = 0;
    for kind in ["JOURNAL", "TRANSFER", "CHECK", "DEPOSIT", "MATCH"] {
        let (mut f, remote, input) = fix1_accept_case(kind);
        f.accept(remote, input).unwrap();
        f.reopen_clean();
        for code in ["100", "101", "220", "500", "410"] {
            let account = f.a[code];
            let original_kind = if ["500", "410"].contains(&code) { if code == "500" { "EXPENSE" } else { "INCOME" } } else if code == "220" { "LIABILITY" } else { "ASSET" };
            for (new_kind, active, postable) in [(original_kind, false, true), (original_kind, true, false), ("EQUITY", true, true), (original_kind, true, true)] {
                if services::save_account(&mut f.book, f.u, f.c, AccountInput::new(code, "Edited after accept").kind(new_kind).active(active).postable(postable).edit(account)).is_ok() {
                    successful += 1;
                }
                f.reopen_clean();
            }
        }
        for currency in ["EUR", "USD"] {
            let _ = services::edit_company(&mut f.book, f.u, f.c, "Renamed after acceptance", currency);
            f.reopen_clean();
        }
        let mut changed = demo();
        for a in &mut changed.accounts { a.currency = "EUR".into(); a.name = "Changed bank label".into(); }
        f.sync(&changed);
        f.reopen_clean();
        f.map("ACT-checking", None).unwrap();
        f.reopen_clean();
        forget(&mut f.book, f.u, f.c, &f.conn.clone(), &SecretSet::none()).unwrap();
        f.reopen_clean();
    }
    assert!(successful >= 30, "successful edits: {successful}");
}

/// FIX3 property test: random URL userinfo (every printable ASCII character but the authority's own
/// terminators `/ ? #` and space, plus non-ASCII, `@` spellings and marks) under every separator spelling,
/// random prefixes, suffixes and wrappers (sentences, JSON, quotes, tags). Over every path that writes
/// sync text (failed sync, successful sync with provider errors and skip reasons, bridge staging, and
/// acceptance as a journal entry or a check), neither the username nor the password, nor the marker at
/// either end of each, ever appears in a connection summary, an audit row, the posted entry's memo and
/// lines, the check, or the sync result's summary.
#[test]
fn fix3_userinfo_property_never_persists_on_any_sync_text_path() {
    // Respect the job's isolated TMPDIR, and remove scratch books once they reopen clean.
    let dir = std::env::temp_dir();
    let done = |f: &mut Fx| {
        f.book.connection().execute_batch("DROP TABLE temp.fix3_seen").unwrap();
        f.reopen_clean();
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", f.path.display()));
        }
    };
    let mut f = Fx::new_in(&dir);
    f.connected_books();
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut pool: Vec<char> = (0x21u8..0x7f).map(char::from).filter(|c| !matches!(c, '/' | '?' | '#')).collect();
    pool.extend(['é', 'ß', '中', '😀', 'İ', '\u{301}', '\u{200d}', '\u{ff20}', '\u{fe6b}', '\u{ff0f}', '\u{ff1a}', '𝟎']);
    let separators = ["://", "://", "://", "%3A%2F%2F", "%3a%2f%2f", ":%2F%2F", "\u{ff1a}\u{ff0f}\u{ff0f}", ":\\\\", "\u{fe55}//"];
    let ats = ["@", "@", "@", "%40", "%2540", "\u{ff20}", "\u{fe6b}"];
    let schemes = ["https", "HTTP", "hTtPs", "ftp", "s3", "git+ssh", "x", "", "xhttps", "_https"];
    let hosts = ["bridge.example.test", "[::1]", "h.example:8443", "", "[bad", "bridge.example.test:443"];
    let paths = ["", "/path", "/p?q=1#f", "?x=1", "#frag", "/a@b", " now"];
    let prefixes = ["", "x", "_", "9", "é", "\u{301}", "<b>", "</b>", "'", "\"", "(", "[", "=", ":", "%", "see ", "url=", "<i></i>"];
    let suffixes = ["", " now", "'", "\"", ")", ",", ";", ".", "<br>", "\"}", ">", " <a href=x>"];
    let pick = |r: u64, n: usize| (r % n as u64) as usize;
    let (mut cases, mut checked_texts, mut failures) = (0usize, 0usize, vec![]);
    let mut by_separator = [0usize; 9];
    // Audit rows already checked (the table has no rowid; UUIDv7 order within a millisecond is not promised).
    let seen = "CREATE TEMP TABLE fix3_seen(id TEXT PRIMARY KEY); INSERT INTO fix3_seen SELECT id FROM ledger_auditevent;";
    f.book.connection().execute_batch(seen).unwrap();
    for i in 0..2_500usize {
        // A fresh book every 250 cases keeps every query small; each finished book must reopen clean.
        if i > 0 && i % 250 == 0 {
            done(&mut f);
            f = Fx::new_in(&dir);
            f.connected_books();
            f.book.connection().execute_batch(seen).unwrap();
        }
        let mut random = |len: u64| -> String { (0..next() % len).map(|_| pool[pick(next(), pool.len())]).collect() };
        let user = format!("UA{i:05}q{}q{i:05}UZ", random(10));
        let pass = format!("PA{i:05}q{}q{i:05}PZ", random(14));
        let markers = [format!("UA{i:05}q"), format!("q{i:05}UZ"), format!("PA{i:05}q"), format!("q{i:05}PZ"), user.clone(), pass.clone()];
        let s = pick(next(), separators.len());
        by_separator[s] += 1;
        let encoded = separators[s].starts_with('%') || separators[s].starts_with(":%");
        let (u, p) = if encoded {
            let enc = |v: &str| v.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect::<String>();
            (enc(&user), enc(&pass))
        } else {
            (user.clone(), pass.clone())
        };
        let userinfo = match next() % 4 {
            0 => p.clone(),
            1 => format!("{u}:"),
            _ => format!("{u}:{p}"),
        };
        let url = format!(
            "{}{}{userinfo}{}{}{}",
            schemes[pick(next(), schemes.len())],
            separators[s],
            ats[pick(next(), ats.len())],
            hosts[pick(next(), hosts.len())],
            paths[pick(next(), paths.len())]
        );
        let embedded = format!("{}{url}{}", prefixes[pick(next(), prefixes.len())], suffixes[pick(next(), suffixes.len())]);
        let text = match next() % 5 {
            0 => format!("Sync failed for {embedded} at 10:00."),
            1 => json!({"url": embedded, "n": 1}).to_string(),
            2 => format!("'{embedded}'"),
            3 => format!("\"{embedded}\""),
            _ => embedded,
        };
        // Bounded like the provider fields: remote ids at 200, descriptions at 500 code points.
        let description: String = text.chars().take(480).collect();
        let money_in = i % 2 == 0;
        let remote = format!("TX-F{i:05}");
        // Failed sync.
        record_sync_failure(&mut f.book, f.u, f.c, &f.conn.clone(), NOW, &text, i % 3 == 0, &SecretSet::none()).unwrap();
        let mut texts = vec![f.book.connection().query_row("SELECT last_sync_summary FROM bankfeeds_connection WHERE id=?1", [&f.conn], |r| r.get::<_, String>(0)).unwrap()];
        // Successful sync: provider errors and skip reasons go into the summary and audit.
        let mut pull = demo();
        pull.errors.push(text.clone());
        pull.skipped.push(text.clone());
        pull.accounts[0].transactions.push(tx(&remote, "2026-10-20", if money_in { 1234 } else { -1234 }, &description));
        let result = f.sync(&pull);
        texts.push(result["summary"].as_str().unwrap().to_owned());
        texts.push(f.book.connection().query_row("SELECT last_sync_summary FROM bankfeeds_connection WHERE id=?1", [&f.conn], |r| r.get::<_, String>(0)).unwrap());
        // Bridge staging: account and transaction ids go into the audit.
        let staged_id: String = format!("ST{i:05}-{}", text.chars().take(180).collect::<String>());
        stage_bank_transactions(&mut f.book, f.u, f.c, &f.conn.clone(), &[("ACT-checking".into(), tx(&staged_id, "2026-10-21", 99, &description))], &SecretSet::none()).unwrap();
        // Acceptance: money in as a journal entry, money out as a check (payee from the description).
        let input = if money_in { json!({"kind":"JOURNAL","account_id":f.acc("410")}) } else { json!({"kind":"CHECK","account_id":f.acc("500")}) };
        let accepted = f.accept(&remote, input).unwrap();
        let journal = accepted.journal_id.clone().unwrap();
        texts.push(f.book.connection().query_row("SELECT memo FROM ledger_journal WHERE id=?1", [&journal], |r| r.get::<_, String>(0)).unwrap());
        let mut st = f.book.connection().prepare("SELECT description FROM ledger_journalline WHERE journal_id=?1").unwrap();
        texts.extend(st.query_map([&journal], |r| r.get::<_, String>(0)).unwrap().map(Result::unwrap));
        drop(st);
        let mut st = f.book.connection().prepare("SELECT target, detail FROM ledger_auditevent WHERE id NOT IN (SELECT id FROM temp.fix3_seen)").unwrap();
        for (target, detail) in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).unwrap().map(Result::unwrap) {
            // Details are JSON: compare the decoded strings too, so escaping cannot hide a marker.
            let decoded: Value = serde_json::from_str(&detail).unwrap();
            texts.extend([target, detail.clone(), decoded.to_string()]);
            fn strings(v: &Value, out: &mut Vec<String>) {
                match v {
                    Value::String(s) => out.push(s.clone()),
                    Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
                    Value::Object(m) => m.values().for_each(|x| strings(x, out)),
                    _ => {}
                }
            }
            strings(&decoded, &mut texts);
        }
        drop(st);
        f.book.connection().execute_batch("INSERT OR IGNORE INTO temp.fix3_seen SELECT id FROM ledger_auditevent").unwrap();
        if !money_in {
            let mut st = f
                .book
                .connection()
                .prepare(
                    "SELECT r2.payee_name||' '||r2.memo||' '||r2.payee_snapshot||' '||coalesce((SELECT group_concat(l.description,' ') FROM checks_checkline l WHERE l.revision_id=r2.id),'')
                     FROM checks_checkrevision r JOIN checks_checkrevision r2 ON r2.document_id=r.document_id WHERE r.journal_id=?1",
                )
                .unwrap();
            let revisions: Vec<String> = st.query_map([&journal], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
            assert!(!revisions.is_empty(), "case {i}: no check revision for the accepted check");
            texts.extend(revisions);
        }
        for t in &texts {
            checked_texts += 1;
            if let Some(m) = markers.iter().find(|m| t.contains(m.as_str())) {
                failures.push(format!("{text:?}: {m:?} in {t:?}"));
            }
        }
        cases += 1;
    }
    done(&mut f);
    eprintln!("fix3 userinfo property: {cases} cases, {checked_texts} persisted texts checked, per separator {by_separator:?}");
    assert!(failures.is_empty(), "{} leaks, first: {:#?}", failures.len(), &failures[..failures.len().min(6)]);
    assert_eq!(cases, 2_500);
}

// Independent R4 attacks. Synthetic credential markers only.
fn r4_no_markers(value: &Value, markers: &[String]) {
    match value {
        Value::String(s) => assert!(!markers.iter().any(|m| s.contains(m)), "R4 credential marker leaked in persisted text"),
        Value::Array(a) => a.iter().for_each(|v| r4_no_markers(v, markers)),
        Value::Object(o) => o.iter().for_each(|(k,v)| { r4_no_markers(&json!(k), markers); r4_no_markers(v, markers); }),
        _ => {}
    }
}

#[test]
fn r4_independent_ascii_unicode_encoded_credentials_all_paths() {
    let mut seed = 0x82f6_a1d3_930c_6eb7u64;
    let mut next = || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; seed };
    let mut pool: Vec<char> = (32u8..=126).map(char::from).collect();
    pool.extend(['é','中','😀','\u{301}','\u{200d}','ı','\u{ff20}','\u{fe6b}']);
    let encode = |s: &str| s.bytes().map(|b| format!("%{b:02X}")).collect::<String>();
    let mut inputs = vec![];
    for i in 0..4096usize {
        let salt = if i < pool.len() { pool[i].to_string() } else { (0..next()%15).map(|_| pool[next() as usize % pool.len()]).collect() };
        // Authority terminators are percent-encoded when they are credential contents.
        let safe = salt.replace(' ', "%20").replace('/', "%2F").replace('?', "%3F").replace('#', "%23");
        let user = format!("R4U{i:04}X{safe}R4UE{i:04}");
        let pass = format!("R4P{i:04}Y{safe}R4PE{i:04}");
        let marker = vec![format!("R4U{i:04}X"),format!("R4UE{i:04}"),format!("R4P{i:04}Y"),format!("R4PE{i:04}")];
        let scheme = ["hTTps://","xHTTPS://","ftp://","s3://", "https%3a%2f%2f", "https:\\\\"][i%6];
        let (u,p,at) = if i%6==4 { (encode(&user),encode(&pass),"%40") } else { (user,pass,"@") };
        let url = format!("{scheme}{u}:{p}{at}r4.example.test/p");
        let input = match i%4 { 0=>format!("Error: '{url}' now"), 1=>json!({"endpoint":url}).to_string(), 2=>format!("<b>x</b>{url}"), _=>format!("\"{url}\"") };
        let mut markers = marker;
        markers.extend(markers.clone().iter().map(|m| encode(m)));
        for output in [text::redact(&input),text::sanitize(&input,500),text::strip_url_userinfo(&input)] {
            r4_no_markers(&json!(output), &markers);
        }
        if i < 384 { inputs.push((input,markers)); }
    }
    let temp_root = std::env::temp_dir();
    let dir = temp_root.as_path();
    let mut f = Fx::new_in(dir);
    f.connected_books();
    let mut markers = vec![];
    for (i,(input,case_markers)) in inputs.into_iter().enumerate() {
        markers.extend(case_markers);
        record_sync_failure(&mut f.book, f.u, f.c, &f.conn.clone(), NOW, &input, i%2==0, &SecretSet::none()).unwrap();
        let remote = format!("R4TX{i}");
        let mut pull = StagedPull { accounts: vec![acct("ACT-checking","Business Checking",vec![tx(&remote,"2026-10-20",if i%2==0 { 100 } else { -100 },&input)])], skipped:vec![input.clone()], errors:vec![input.clone()] };
        pull.accounts[0].skipped.push(input.clone());
        let result = f.sync(&pull);
        r4_no_markers(&result["summary"], &markers);
        stage_bank_transactions(&mut f.book,f.u,f.c,&f.conn.clone(), &[("ACT-checking".into(), tx(&format!("https://R4U{i:04}X:R4P{i:04}Y@r4.example.test/p"),"2026-10-20", 100,&input))], &SecretSet::none()).unwrap();
        let item = f.item(&remote);
        ignore(&mut f.book,f.u,f.c,&item.id, &SecretSet::none()).unwrap();
        return_to_review(&mut f.book,f.u,f.c,&item.id, &SecretSet::none()).unwrap();
        let payload = if i%2==0 { json!({"kind":"JOURNAL","account_id":f.acc("410")}) } else {json!({"kind":"CHECK","account_id":f.acc("500")})};
        f.accept(&remote,payload).unwrap();
    }
    let mut texts = vec![];
    for query in ["SELECT last_sync_summary FROM bankfeeds_connection", "SELECT detail FROM ledger_auditevent", "SELECT target FROM ledger_auditevent"] {
        let mut st = f.book.connection().prepare(query).unwrap();
        for s in st.query_map([], |r| r.get::<_,String>(0)).unwrap().map(Result::unwrap) {
            texts.push(serde_json::from_str::<Value>(&s).unwrap_or(json!(s)));
        }
    }
    r4_no_markers(&json!(texts),&markers);
    f.reopen_clean();
    for suffix in ["","-wal","-shm"] { let _ = std::fs::remove_file(format!("{}{suffix}",f.path.display())); }
    eprintln!("R4: 4096 generated credential cases; 384 through failure, success, staging, ignore, return, and acceptance paths");
}

#[test]
fn r4_accept_abort_on_second_line_and_after_decision_is_atomic() {
    for kind in ["JOURNAL","TRANSFER","CHECK","DEPOSIT","MATCH"] {
        for boundary in ["AFTER INSERT ON ledger_journalline WHEN (SELECT count(*) FROM ledger_journalline WHERE journal_id=NEW.journal_id)>1", "AFTER UPDATE ON bankfeeds_transaction WHEN NEW.status IN ('ACCEPTED','MATCHED')"] {
            let (mut f,remote,payload)=fix1_accept_case(kind);
            if kind=="CHECK" {
                f.book.connection().execute_batch("CREATE TEMP TRIGGER r4_seed BEFORE INSERT ON ledger_auditevent WHEN NEW.action='bankfeed.accepted' BEGIN SELECT RAISE(ABORT,'r4_seed'); END;").unwrap();
                assert!(f.accept(remote,payload.clone()).unwrap_err().to_string().contains("r4_seed"));
                f.book.connection().execute_batch("DROP TRIGGER temp.r4_seed").unwrap();
            }
            let before=fb_db::test_support::snapshot(f.book.connection()).unwrap();
            f.book.connection().execute_batch(&format!("CREATE TEMP TRIGGER r4_abort {boundary} BEGIN SELECT RAISE(ABORT,'r4_abort'); END;")).unwrap();
            let result=f.accept(remote,payload.clone());
            f.book.connection().execute_batch("DROP TRIGGER temp.r4_abort").unwrap();
            if kind=="MATCH" && boundary.contains("journalline") { assert!(result.is_ok()); continue; }
            assert!(result.unwrap_err().to_string().contains("r4_abort"));
            assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(),before,"{kind} {boundary}");
            f.reopen_clean();
            f.accept(remote,payload).unwrap();
            f.reopen_clean();
        }
    }
}

#[test]
fn r4_rule_mapping_operator_edits_reopen_after_decisions() {
    for kind in ["JOURNAL","TRANSFER","CHECK","DEPOSIT","MATCH"] {
        let (mut f,remote,payload)=fix1_accept_case(kind);
        f.accept(remote,payload).unwrap();
        let rule_input=json!({"name":"R4","match_text":"office","account_id":f.acc("500")});
        let rule=save_rule(&mut f.book,f.u,f.c,&rule_input, &SecretSet::none()).unwrap();
        f.reopen_clean();
        let id=rule.id.as_str();
        set_rule_active(&mut f.book,f.u,f.c,id,&json!(false), &SecretSet::none()).unwrap();
        f.reopen_clean();
        set_rule_active(&mut f.book,f.u,f.c,id,&json!(true), &SecretSet::none()).unwrap();
        f.reopen_clean();
        let mut edit=json!({"rule_id":id,"name":"R4 edited","match_text":"new","account_id":f.acc("410")});
        save_rule(&mut f.book,f.u,f.c,&edit, &SecretSet::none()).unwrap();
        f.reopen_clean();
        edit["account_id"]=Value::Null;
        let _=save_rule(&mut f.book,f.u,f.c,&edit, &SecretSet::none());
        f.reopen_clean();
        users::set_active(&mut f.book,f.u,false).unwrap();
        f.reopen_clean();
        users::set_active(&mut f.book,f.u,true).unwrap();
        f.reopen_clean();
        f.map("ACT-card",None).unwrap();
        f.reopen_clean();
        forget(&mut f.book,f.u,f.c,&f.conn.clone(), &SecretSet::none()).unwrap();
        f.reopen_clean();
    }
}

#[test]
fn r4_provider_account_text_never_leaks_when_mapping() {
    let mut f=Fx::new();
    let mut pool: Vec<char>=(32u8..=126).map(char::from).collect();
    pool.extend(['é','中','😀','\u{301}','\u{200d}','\u{ff20}']);
    let mut leaks=vec![];
    for field in ["remote_id","name","org_name"] {
        for (i,ch) in pool.iter().enumerate() {
            let salt=match ch { ' '=>"%20".into(), '/'=>"%2F".into(), '?'=>"%3F".into(), '#'=>"%23".into(), _=>ch.to_string() };
            let user=format!("R4MAPU{i:03}");
            let pass=format!("R4MAPP{i:03}");
            let text=format!("https://{user}:{pass}{salt}@r4.example.test/path");
            let mut account=acct(&format!("map-{field}-{i}"),"Synthetic name",vec![]);
            match field { "remote_id"=>account.remote_id=text.clone(), "name"=>account.name=text.clone(), _=>account.org_name=text.clone() }
            let remote=account.remote_id.clone();
            f.sync(&StagedPull {accounts:vec![account],..Default::default()});
            f.map(&remote,Some("100")).unwrap();
            let target=format!("bankfeedaccount:{}",f.feed(&remote));
            let found:i64=f.book.connection().query_row("SELECT count(*) FROM ledger_auditevent WHERE action='bankfeed.mapped' AND target=?3 AND (instr(detail,?1)>0 OR instr(detail,?2)>0)",params![user,pass,target],|r|r.get(0)).unwrap();
            if found>0 { leaks.push((field,i)); }
            f.map(&remote,None).unwrap();
        }
    }
    f.reopen_clean();
    let count=leaks.len();
    if let Some(path) = std::env::var_os("FB_R4_MAPPING_FUZZ_OUT") {
        std::fs::write(path,serde_json::to_vec_pretty(&leaks).unwrap()).unwrap();
    }
    for suffix in ["","-wal","-shm"] { let _=std::fs::remove_file(format!("{}{suffix}",f.path.display())); }
    assert_eq!(count,0,"credential markers persisted in bankfeed.mapped audit for {count}/303 account-field cases; field/vector indexes: {leaks:?}");
}

// FIX4: provider text is redacted once where it enters the core, so no audit writer can store it raw.

/// Every `"bankfeed.<action>"` literal the module's sources pass to the audit writer.
fn fix4_emitted_actions() -> std::collections::BTreeSet<String> {
    let sources = [include_str!("mod.rs"), include_str!("staging.rs"), include_str!("accept.rs"), include_str!("rules.rs")];
    let found: std::collections::BTreeSet<String> = sources
        .iter()
        .flat_map(|s| s.split("\"bankfeed.").skip(1).map(|rest| rest.chars().take_while(|c| c.is_ascii_lowercase() || *c == '_').collect::<String>()))
        .filter(|a| !a.is_empty())
        .map(|a| format!("bankfeed.{a}"))
        .collect();
    assert!(found.len() >= 11, "action scan found too few actions: {found:?}");
    found
}

#[test]
fn fix4_every_audit_writer_is_credential_fuzzed_on_every_provider_field() {
    // Field codes: each provider-derived input gets its own credential, so a leak names its field.
    const FIELDS: [&str; 15] = ["host", "fail", "ck", "ar", "nm", "og", "cu", "as", "ps", "pe", "tr", "td", "sr", "sd", "sa"];
    let mut seed = 0x5eed_f1c4_0bad_cafeu64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut pool: Vec<char> = (32u8..=126).map(char::from).collect();
    pool.extend(['é', '中', '😀', '\u{301}', '\u{200d}', 'ı', '\u{ff20}', '\u{fe6b}']);
    let cases = pool.len() * 3;
    let mut f = Fx::new();
    let (office, interest) = (f.acc("500"), f.acc("410"));
    let rule = save_rule(&mut f.book, f.u, f.c, &json!({"name":"F4","match_text":"office","account_id":office}), &SecretSet::none()).unwrap();
    save_rule(&mut f.book, f.u, f.c, &json!({"name":"F4 second","match_text":"fuel","account_id":office}), &SecretSet::none()).unwrap();
    save_rule(&mut f.book, f.u, f.c, &json!({"rule_id":rule.id,"name":"F4 edited","match_text":"office","account_id":interest}), &SecretSet::none()).unwrap();
    move_rule(&mut f.book, f.u, f.c, &rule.id, &json!("down"), &SecretSet::none()).unwrap();
    set_rule_active(&mut f.book, f.u, f.c, &rule.id, &json!(false), &SecretSet::none()).unwrap();
    let mut markers: Vec<(String, String)> = vec![];
    let mut returned: Vec<String> = vec![];
    for i in 0..cases {
        let salt: String = if i < pool.len() { pool[i].to_string() } else { (0..next() % 7).map(|_| pool[next() as usize % pool.len()]).collect() };
        let safe = salt.replace(' ', "%20").replace('/', "%2F").replace('?', "%3F").replace('#', "%23");
        let scheme = ["hTTps://", "xHTTPS://", "ftp://", "s3://", "https%3a%2f%2f", "https:\\\\"][i % 6];
        let at = if i % 6 == 4 { "%40" } else { "@" };
        let mut cred = |field: &str| {
            let (user, pass) = (format!("F4{field}U{i:03}X"), format!("F4{field}P{i:03}Y"));
            let (user_end, pass_end) = (format!("F4{field}UE{i:03}"), format!("F4{field}PE{i:03}"));
            for m in [&user, &pass, &user_end, &pass_end] {
                markers.push((field.to_owned(), m.clone()));
            }
            let url = format!("{scheme}{user}{safe}{user_end}:{pass}{safe}{pass_end}{at}f4.example.test/c{i:03}{field}");
            // The leading word keeps ids unique when redaction drops a whole authority and path.
            let wrapped = match i % 4 {
                0 => format!("Error: '{url}' now"),
                1 => json!({"endpoint":url}).to_string(),
                2 => format!("<b>x</b>{url}"),
                _ => format!("\"{url}\""),
            };
            format!("c{i:03}{field} {wrapped}")
        };
        let creds: std::collections::HashMap<&str, String> = FIELDS.iter().map(|k| (*k, cred(k))).collect();
        let connection = connect(&mut f.book, f.u, f.c, &json!("F4"), &creds["host"], &SecretSet::none()).unwrap();
        forget(&mut f.book, f.u, f.c, connection["id"].as_str().unwrap(), &SecretSet::none()).unwrap();
        returned.push(connection.to_string());
        record_sync_failure(&mut f.book, f.u, f.c, &f.conn.clone(), NOW, &creds["fail"], i % 2 == 0, &SecretSet::none()).unwrap();
        let money_in = i % 2 == 0;
        let mut account = acct(&creds["ar"], &creds["nm"], vec![tx(&creds["tr"], "2026-10-20", if money_in { 100 } else { -100 }, &creds["td"])]);
        account.conn_key = creds["ck"].clone();
        account.org_name = creds["og"].clone();
        account.skipped.push(creds["as"].clone());
        let mut other = acct(&format!("F4-other-{i}"), "Other", vec![]);
        other.currency = creds["cu"].clone();
        let pull = StagedPull { accounts: vec![account, other], skipped: vec![creds["ps"].clone()], errors: vec![creds["pe"].clone()] };
        returned.push(f.sync(&pull).to_string());
        f.map(&creds["ar"], Some("100")).unwrap();
        let staged = StagedTransaction { posted_on: Some("2026-10-21".into()), ..tx(&creds["sr"], "2026-10-21", if money_in { -100 } else { 100 }, &creds["sd"]) };
        let staged_account = creds["ar"].clone();
        returned.push(format!("{:?}", stage_bank_transactions(&mut f.book, f.u, f.c, &f.conn.clone(), &[(staged_account, staged)], &SecretSet::none()).unwrap()));
        // A stage naming another credential-bearing account is refused, unless redaction collapsed both ids to
        // one (an `@` spelling Python's parser rejects redacts the whole authority and path: `hTTps://***`).
        if staging::provider_text(&SecretSet::none(), &creds["sa"]) != staging::provider_text(&SecretSet::none(), &creds["ar"]) {
            assert!(stage_bank_transactions(&mut f.book, f.u, f.c, &f.conn.clone(), &[(creds["sa"].clone(), tx("F4-x", "2026-10-21", 1, "x"))], &SecretSet::none()).is_err(), "case {i}");
        }
        let item = f.item(&creds["tr"]);
        returned.push(format!("{:?}", ignore(&mut f.book, f.u, f.c, &item.id, &SecretSet::none()).unwrap()));
        returned.push(format!("{:?}", return_to_review(&mut f.book, f.u, f.c, &item.id, &SecretSet::none()).unwrap()));
        let journal = |f: &Fx| json!({"kind":"JOURNAL","account_id":f.acc("410")});
        let check = |f: &Fx| json!({"kind":"CHECK","account_id":f.acc("500")});
        let (first, second) = if money_in { (journal(&f), check(&f)) } else { (check(&f), journal(&f)) };
        returned.push(format!("{:?}", f.accept(&creds["tr"], first).unwrap()));
        returned.push(format!("{:?}", f.accept(&creds["sr"], second).unwrap()));
        f.map(&creds["ar"], None).unwrap();
    }
    // Every action the sources emit was driven by this fuzz: a new audit writer fails here until it is.
    let mut st = f.book.connection().prepare("SELECT DISTINCT action FROM ledger_auditevent WHERE action GLOB 'bankfeed.*'").unwrap();
    let seen: std::collections::BTreeSet<String> = st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    drop(st);
    let missing: Vec<_> = fix4_emitted_actions().difference(&seen).cloned().collect();
    assert!(missing.is_empty(), "audit actions without credential-fuzz coverage: {missing:?}");
    // No marker anywhere in the book (every table, audit details and targets included) or in any return value.
    let book = format!("{:?}", fb_db::test_support::snapshot(f.book.connection()).unwrap());
    let encode = |s: &str| s.bytes().map(|b| format!("%{b:02X}")).collect::<String>();
    // Case-blind: `connect` lowercases the host.
    let haystack = format!("{book}\n{}", returned.join("\n")).to_lowercase();
    let found = |m: &str| haystack.contains(&m.to_lowercase()) || haystack.contains(&encode(m).to_lowercase());
    let mut leaks: Vec<(String, String)> = markers.iter().filter(|(_, m)| found(m)).cloned().collect();
    leaks.sort();
    leaks.dedup();
    let fields: std::collections::BTreeSet<&str> = leaks.iter().map(|(field, _)| field.as_str()).collect();
    f.reopen_clean();
    assert!(leaks.is_empty(), "{} credential markers persisted or returned, fields {fields:?}; first {:?}", leaks.len(), leaks.first());
    // The provider text did reach each writer, redacted rather than dropped.
    for action in ["bankfeed.connected", "bankfeed.forgotten", "bankfeed.mapped", "bankfeed.synced", "bankfeed.staged", "bankfeed.accepted", "bankfeed.ignored"] {
        let redacted: i64 = f.book.connection().query_row("SELECT count(*) FROM ledger_auditevent WHERE action=?1 AND instr(detail,'***')>0", [action], |r| r.get(0)).unwrap();
        assert!(redacted >= cases as i64, "{action}: only {redacted} audits carry redacted provider text");
    }
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", f.path.display()));
    }
    eprintln!("FIX4: {cases} cases x {} provider fields through {} audit actions", FIELDS.len(), seen.len());
}

#[test]
fn fix4_redaction_at_the_boundary_is_idempotent_on_python_output_and_keeps_plain_ids() {
    for plain in ["ACT-checking", "TX-1001", "CON-demo-001", "Business Checking", "Demo Bank", "USD", "OFFICE DEPOT #221 PURCHASE", "https://***@host/path", "café 中 😀"] {
        assert_eq!(staging::provider_text(&SecretSet::none(), plain), plain);
    }
    // FIX5: two provider ids that differ only in userinfo display alike but stay two rows (identity keys).
    let mut f = Fx::new();
    let pull = |user: &str| StagedPull { accounts: vec![acct(&format!("https://{user}:pw@ids.example.test/a"), "Checking", vec![])], ..Default::default() };
    f.sync(&pull("first"));
    f.sync(&pull("second"));
    f.sync(&pull("first"));
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_account WHERE remote_id='https://***@ids.example.test/a'"), 2);
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_account WHERE instr(remote_id,'first')>0 OR instr(remote_id,'second')>0"), 0);
    f.reopen_clean();
}

// Reviewer R5 (independent): credential fuzz over every provider field and audit, rollback and reopen sweeps.
fn r5_encode(s: &str) -> String {
    s.bytes().map(|b| format!("%{b:02X}")).collect()
}
fn r5_assert_clean(s: &str, context: &str) {
    let lower = s.to_lowercase();
    for marker in ["R5SECUSER", "R5SECPASS", "R5SECENDU", "R5SECENDP"] {
        assert!(!lower.contains(&marker.to_lowercase()), "raw credential leaked: {context}");
        assert!(!lower.contains(&r5_encode(marker).to_lowercase()), "encoded credential leaked: {context}");
    }
}
fn r5_vector(i: usize, field: &str, seed: &mut u64) -> String {
    let mut next = || { *seed ^= *seed << 13; *seed ^= *seed >> 7; *seed ^= *seed << 17; *seed };
    let mut pool: Vec<char> = (32u8..=126).map(char::from).collect();
    pool.extend(['é', '中', '😀', '\u{301}', '\u{200d}', '\u{ff20}', '\u{fe6b}', 'ß', 'İ']);
    let salt = if i < pool.len() { pool[i].to_string() } else { (0..1+next()%18).map(|_| pool[next() as usize%pool.len()]).collect() };
    let safe = salt.replace(' ', "%20").replace('/', "%2F").replace('?', "%3F").replace('#', "%23");
    let u = format!("R5SECUSER{field}{i}{safe}R5SECENDU");
    let p = format!("R5SECPASS{field}{i}{safe}R5SECENDP");
    let forms = ["://", "%3A%2f%2F", ":%2f/", ":\\\\", "\u{ff1a}\u{ff0f}\u{ff0f}", "\u{fe55}//"];
    let ats = ["@", "%40", "%2540", "\u{ff20}", "\u{fe6b}"];
    let (u,p) = if i.is_multiple_of(3) { (r5_encode(&u),r5_encode(&p)) } else { (u,p) };
    let url = format!("{}{}{u}:{p}{}r5.test/{i}/{field}", ["hTTps", "xHTTPS", "s3", "git+ssh"][i%4], forms[i%forms.len()], ats[(i/6)%ats.len()]);
    let text = match i%5 { 0=>format!("Failed: '{url}' now"), 1=>json!({"endpoint":url}).to_string(), 2=>format!("<b>x</b>{url}"), 3=>format!("\"{url}\""), _=>format!("x{url}") };
    format!("v{i}{field} {text}")
}
fn r5_finish(f: &mut Fx) {
    f.reopen_clean();
    for suffix in ["", "-wal", "-shm"] { let _ = std::fs::remove_file(format!("{}{suffix}", f.path.display())); }
}

#[test]
fn r5_independent_provider_field_fuzz_and_all_audits() {
    let mut seed=0x6b7a_1289_abcd_359du64;
    // Pure text paths first: random multi-character userinfo, not just one character per case.
    for i in 0..8192 {
        let value=r5_vector(i,"text",&mut seed);
        for out in [text::redact(&value), text::sanitize(&value,500), text::strip_url_userinfo(&value)] {
            r5_assert_clean(&out,"pure text");
        }
    }
    let mut seen=std::collections::BTreeSet::<String>::new();
    const FIELDS: [&str;15]=["host","failure","connkey","accountid","accountname","org","currency","accountskip","pullskip","error","txid","description","stageid","stagedesc","stageaccount"];
    for chunk in 0..12 {
        let mut f=Fx::new();
        let category=f.acc("500");
        let rule=save_rule(&mut f.book,f.u,f.c,&json!({"name":"R5 first","match_text":"office","account_id":category}), &SecretSet::none()).unwrap();
        save_rule(&mut f.book,f.u,f.c,&json!({"name":"R5 second","match_text":"fuel","account_id":category}), &SecretSet::none()).unwrap();
        save_rule(&mut f.book,f.u,f.c,&json!({"rule_id":rule.id,"name":"R5 edited","match_text":"office","account_id":category}), &SecretSet::none()).unwrap();
        move_rule(&mut f.book,f.u,f.c,&rule.id,&json!("down"), &SecretSet::none()).unwrap();
        set_rule_active(&mut f.book,f.u,f.c,&rule.id,&json!(false), &SecretSet::none()).unwrap();
        let mut returned=String::new();
        for i in chunk*100..(chunk+1)*100 {
            let values: std::collections::HashMap<_,_>=FIELDS.iter().map(|k|(*k,r5_vector(i,k,&mut seed))).collect();
            let connection=connect(&mut f.book,f.u,f.c,&json!("R5"),&values["host"], &SecretSet::none()).unwrap();
            returned.push_str(&connection.to_string());
            returned.push_str(&forget(&mut f.book,f.u,f.c,connection["id"].as_str().unwrap(), &SecretSet::none()).unwrap().to_string());
            record_sync_failure(&mut f.book,f.u,f.c,&f.conn.clone(),NOW,&values["failure"],i%2==0, &SecretSet::none()).unwrap();
            let amount=if i%2==0 {100} else {-100};
            let mut a=acct(&values["accountid"],&values["accountname"],vec![tx(&values["txid"],"2026-10-20",amount,&values["description"])]);
            a.conn_key=values["connkey"].clone(); a.org_name=values["org"].clone(); a.skipped.push(values["accountskip"].clone());
            let mut other=acct(&format!("other-{i}"),"Other",vec![]); other.currency=values["currency"].clone();
            returned.push_str(&f.sync(&StagedPull{accounts:vec![a,other],skipped:vec![values["pullskip"].clone()],errors:vec![values["error"].clone()]}).to_string());
            returned.push_str(&f.map(&values["accountid"],Some("100")).unwrap().to_string());
            returned.push_str(&format!("{:?}",stage_bank_transactions(&mut f.book,f.u,f.c,&f.conn.clone(),&[(values["accountid"].clone(),tx(&values["stageid"],"2026-10-20",-amount,&values["stagedesc"]))], &SecretSet::none()).unwrap()));
            let rejected=stage_bank_transactions(&mut f.book,f.u,f.c,&f.conn.clone(),&[(values["stageaccount"].clone(),tx("unused","2026-10-20",1,"x"))], &SecretSet::none());
            assert!(rejected.is_err());
            returned.push_str(&format!("{rejected:?}"));
            let item=f.item(&values["txid"]);
            returned.push_str(&format!("{:?}",ignore(&mut f.book,f.u,f.c,&item.id, &SecretSet::none()).unwrap()));
            returned.push_str(&format!("{:?}",return_to_review(&mut f.book,f.u,f.c,&item.id, &SecretSet::none()).unwrap()));
            let journal=json!({"kind":"JOURNAL","account_id":f.acc("410")});
            let check=json!({"kind":"CHECK","account_id":f.acc("500")});
            let (one,two)=if amount>0 {(journal,check)} else {(check,journal)};
            returned.push_str(&format!("{:?}",f.accept(&values["txid"],one).unwrap()));
            returned.push_str(&format!("{:?}",f.accept(&values["stageid"],two).unwrap()));
            returned.push_str(&f.map(&values["accountid"],None).unwrap().to_string());
        }
        r5_assert_clean(&returned,"service returns");
        let snapshot=fb_db::test_support::snapshot(f.book.connection()).unwrap();
        // Snapshot serializes typed SQLite strings; scan every table, including derived payees/memos.
        r5_assert_clean(&format!("{snapshot:?}"),"whole book");
        let mut st=f.book.connection().prepare("SELECT DISTINCT action FROM ledger_auditevent WHERE action GLOB 'bankfeed.*'").unwrap();
        seen.extend(st.query_map([],|r|r.get::<_,String>(0)).unwrap().map(Result::unwrap));
        drop(st);
        r5_finish(&mut f);
    }
    assert_eq!(seen,fix4_emitted_actions());
    eprintln!("R5: 8192 pure-text cases; 1200 persisted cases x 15 provider fields; {} audit actions",seen.len());
}

fn r5_seed_check(f:&mut Fx,remote:&str,input:&Value) {
    f.book.connection().execute_batch("CREATE TEMP TRIGGER r5_seed BEFORE INSERT ON ledger_auditevent WHEN NEW.action='bankfeed.accepted' BEGIN SELECT RAISE(ABORT,'r5_seed'); END;").unwrap();
    assert!(f.accept(remote,input.clone()).unwrap_err().to_string().contains("r5_seed"));
    f.book.connection().execute_batch("DROP TRIGGER temp.r5_seed").unwrap();
}

#[test]
fn r5_atomic_accept_after_each_changed_table_and_audit() {
    let mut points=0;
    for kind in ["JOURNAL","CHECK","DEPOSIT","TRANSFER","MATCH"] {
        let (mut f,remote,input)=fix1_accept_case(kind);
        if kind=="CHECK" {r5_seed_check(&mut f,remote,&input);}
        let before=fb_db::test_support::snapshot(f.book.connection()).unwrap();
        let old_audits:std::collections::BTreeSet<String>={let mut st=f.book.connection().prepare("SELECT id FROM ledger_auditevent").unwrap();st.query_map([],|r|r.get(0)).unwrap().map(Result::unwrap).collect()};
        f.accept(remote,input).unwrap();
        let after=fb_db::test_support::snapshot(f.book.connection()).unwrap();
        let mut boundaries=vec![];
        for (table,rows) in &after {
            if table=="ledger_auditevent" || before.get(table)==Some(rows) {continue;}
            for op in ["INSERT","UPDATE","DELETE"] {boundaries.push(format!("AFTER {op} ON {table}"));}
        }
        let mut st=f.book.connection().prepare("SELECT id,action FROM ledger_auditevent").unwrap();
        for (id,action) in st.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).unwrap().map(Result::unwrap) {
            if !old_audits.contains(&id) {boundaries.push(format!("AFTER INSERT ON ledger_auditevent WHEN NEW.action='{action}'"));}
        }
        drop(st);
        for boundary in boundaries {
            let (mut f,remote,input)=fix1_accept_case(kind);
            if kind=="CHECK" {r5_seed_check(&mut f,remote,&input);}
            let before=fb_db::test_support::snapshot(f.book.connection()).unwrap();
            f.book.connection().execute_batch(&format!("CREATE TEMP TRIGGER r5_abort {boundary} BEGIN SELECT RAISE(ABORT,'r5_late'); END;")).unwrap();
            let result=f.accept(remote,input.clone());
            f.book.connection().execute_batch("DROP TRIGGER temp.r5_abort").unwrap();
            if let Err(e)=result {
                assert!(e.to_string().contains("r5_late"),"{kind}/{boundary}: wrong error");
                assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(),before,"{kind}/{boundary}: partial write");
                f.reopen_clean();
                f.accept(remote,input.clone()).unwrap();
                let once=fb_db::test_support::snapshot(f.book.connection()).unwrap();
                f.accept(remote,input).unwrap();
                assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(),once);
                points+=1;
            }
            f.reopen_clean();
        }
    }
    assert!(points>=25,"only {points} reached boundaries");
    eprintln!("R5: {points} reached AFTER-write boundaries rollback exactly, reopen, retry once");
}

#[test]
fn r5_provider_edits_and_reopen_across_all_accept_kinds() {
    let mut edits=0;
    let mut seed=0x8257_6afe_8323_1200;
    for kind in ["JOURNAL","CHECK","DEPOSIT","TRANSFER","MATCH"] {
        let (mut f,remote,input)=fix1_accept_case(kind);
        for i in 0..24 {
            let desc=r5_vector(i,"allkinds",&mut seed);
            let mut pull=demo();
            for a in &mut pull.accounts {
                a.name=r5_vector(i,"name",&mut seed); a.org_name=r5_vector(i,"org",&mut seed);
                for t in &mut a.transactions {t.description=desc.clone();}
            }
            f.sync(&pull);f.reopen_clean();edits+=1;
        }
        f.accept(remote,input).unwrap();f.reopen_clean();
        r5_assert_clean(&format!("{:?}",fb_db::test_support::snapshot(f.book.connection()).unwrap()),"all accept kinds");
        for i in 24..36 {
            let mut pull=demo();
            for a in &mut pull.accounts {a.currency=if i%2==0 {"EUR".into()} else {"USD".into()}; a.name=r5_vector(i,"postaccept",&mut seed);}
            f.sync(&pull);f.reopen_clean();edits+=1;
            let _=services::edit_company(&mut f.book,f.u,f.c,"R5 company",if i%2==0 {"EUR"} else {"USD"});f.reopen_clean();edits+=1;
        }
        let vendor=directory::save_record(&mut f.book,f.u,f.c,directory::Model::Vendor,&json!({"display_name":"Edited supplier"}),Some(&f.vendor.clone()));
        assert!(vendor.is_ok());f.reopen_clean();edits+=1;
        let customer=directory::save_record(&mut f.book,f.u,f.c,directory::Model::Customer,&json!({"display_name":"Edited customer"}),Some(&f.customer.clone()));
        assert!(customer.is_ok());f.reopen_clean();edits+=1;
    }
    eprintln!("R5: {edits} additional service edits reopen across all accept kinds");
}

// Reviewer R5 (HIGH, FIX4 regression): distinct provider ids that differ only in userinfo stay distinct.
#[test]
fn r5_distinct_provider_transaction_ids_do_not_merge_after_redaction() {
    let mut f=Fx::new();
    let first="https://R5SECUSERfirst:R5SECPASSfirst@provider.test/transaction";
    let second="https://R5SECUSERsecond:R5SECPASSsecond@provider.test/transaction";
    let pull=|id:&str,amount:i64|StagedPull{accounts:vec![acct("collision-account","Synthetic bank",vec![tx(id,"2026-10-20",amount,"Different provider transaction")])],..Default::default()};
    f.sync(&pull(first,100));f.map("collision-account",Some("100")).unwrap();
    f.accept(first,json!({"kind":"JOURNAL","account_id":f.acc("410")})).unwrap();
    let second_pull=f.sync(&pull(second,200));
    let count=f.count("SELECT count(*) FROM bankfeeds_transaction");
    let pending=f.count("SELECT count(*) FROM bankfeeds_transaction WHERE status='NEW' AND amount=200");
    r5_assert_clean(&format!("{:?}",fb_db::test_support::snapshot(f.book.connection()).unwrap()),"collision book");
    r5_finish(&mut f);
    assert_eq!((count,pending,second_pull["new"].as_i64()),(2,1,Some(1)),"distinct provider transaction was silently merged into the accepted row; counts={second_pull}");
}

#[test]
fn r5_distinct_provider_account_ids_do_not_merge_after_redaction() {
    let mut f=Fx::new();
    let first="https://R5SECUSERfirst:R5SECPASSfirst@provider.test/account";
    let second="https://R5SECUSERsecond:R5SECPASSsecond@provider.test/account";
    f.sync(&StagedPull{accounts:vec![acct(first,"First bank",vec![tx("first-bank-tx","2026-10-20",100,"First bank")])],..Default::default()});
    f.map(first,Some("100")).unwrap();
    f.sync(&StagedPull{accounts:vec![acct(second,"Second bank",vec![tx("second-bank-tx","2026-10-20",200,"Second bank")])],..Default::default()});
    let count=f.count("SELECT count(*) FROM bankfeeds_account");
    let wrong_mapping=f.count("SELECT count(*) FROM bankfeeds_transaction t JOIN bankfeeds_account a ON a.id=t.feed_account_id WHERE t.remote_id='second-bank-tx' AND a.ledger_account_id IS NOT NULL");
    let accepted=f.accept("second-bank-tx",json!({"kind":"JOURNAL","account_id":f.acc("410")})).is_ok();
    r5_finish(&mut f);
    assert_eq!((count,wrong_mapping,accepted),(2,0,false),"second provider bank account inherited the first account's ledger mapping and posted through it");
}

// --- FIX5 (Sol's review 5 of b92b4ea): identity apart from display -----------------------------------

/// Provider ids that differ only in URL userinfo (the credential markers are R5SEC*, checked by
/// `r5_assert_clean`).
fn fix5_id(user: &str, path: &str) -> String {
    format!("https://R5SECUSER{user}:R5SECPASS{user}@ids.example.test/{path}")
}
fn fix5_pending(remote: &str, amount: i64) -> StagedTransaction {
    StagedTransaction { pending: true, posted_on: None, transacted_on: Some("2026-10-27".into()), ..tx(remote, "", amount, "PENDING HOLD") }
}

#[test]
fn fix5_userinfo_only_ids_stay_distinct_through_pull_sync_accept_mapping_and_staging() {
    let mut f = Fx::new();
    let (acct_a, acct_b) = (fix5_id("a", "account"), fix5_id("b", "account"));
    let (tx_a, tx_b) = (fix5_id("a", "tx-a"), fix5_id("b", "tx-a"));
    let (tx_a2, tx_b2) = (fix5_id("a", "tx-b"), fix5_id("b", "tx-b"));
    let pull = || StagedPull {
        accounts: vec![
            acct(&acct_a, "Checking A", vec![tx(&tx_a, "2026-10-20", 100, "Deposit A"), tx(&tx_a2, "2026-10-21", 300, "Deposit A2")]),
            acct(&acct_b, "Checking B", vec![tx(&tx_b, "2026-10-20", 200, "Deposit B"), tx(&tx_b2, "2026-10-21", 400, "Deposit B2")]),
        ],
        ..Default::default()
    };
    // Pull: two accounts and four transactions, although each pair displays the same redacted id.
    let first = f.sync(&pull());
    assert_eq!((first["accounts"].as_i64(), first["new"].as_i64()), (Some(2), Some(4)), "{first}");
    assert_eq!(f.count("SELECT count(DISTINCT remote_id) FROM bankfeeds_account"), 1, "both accounts display one redacted id");
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_account"), 2);
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_transaction"), 4);
    // Mapping: each account keeps its own ledger account; the second never inherits the first's.
    f.map(&acct_a, Some("100")).unwrap();
    let feed_b = f.feed(&acct_b);
    assert_eq!(f.count(&format!("SELECT count(*) FROM bankfeeds_account WHERE id='{feed_b}' AND ledger_account_id IS NULL")), 1);
    assert!(f.accept(&tx_b, json!({"kind":"JOURNAL","account_id":f.acc("410")})).is_err(), "an unmapped account posted through its twin's mapping");
    f.map(&acct_b, Some("101")).unwrap();
    // Accept: each posts to its own bank account and leaves its twin to review.
    let a = f.accept(&tx_a, json!({"kind":"JOURNAL","account_id":f.acc("410")})).unwrap();
    assert_eq!(f.item(&tx_b).status, "NEW");
    let b = f.accept(&tx_b, json!({"kind":"JOURNAL","account_id":f.acc("410")})).unwrap();
    assert_eq!(f.lines(a.journal_id.as_deref().unwrap()), vec![("100".to_owned(), 100, 0), ("410".to_owned(), 0, 100)]);
    assert_eq!(f.lines(b.journal_id.as_deref().unwrap()), vec![("101".to_owned(), 200, 0), ("410".to_owned(), 0, 200)]);
    // Sync again: the same raw ids find their own rows; nothing new, nothing lost.
    let again = f.sync(&pull());
    assert_eq!((again["new"].as_i64(), again["unchanged"].as_i64()), (Some(0), Some(4)), "{again}");
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_transaction WHERE status='ACCEPTED'"), 2);
    // Bridge staging names the account by its raw id too: B's new transaction lands in B, not A.
    let tx_b3 = fix5_id("b", "tx-c");
    let staged = stage_bank_transactions(&mut f.book, f.u, f.c, &f.conn.clone(), &[(acct_b.clone(), tx(&tx_b3, "2026-10-22", 500, "Deposit B3"))], &SecretSet::none()).unwrap();
    assert_eq!(f.item(&tx_b3).feed_account_id, f.feed(&acct_b));
    assert_eq!(staged, vec![f.item(&tx_b3).id]);
    // Withdrawal: a pending id is withdrawn only when its own raw id is absent, not its display twin.
    let (hold_a, hold_b) = (fix5_id("a", "hold"), fix5_id("b", "hold"));
    let mut with_holds = pull();
    with_holds.accounts[0].transactions.push(fix5_pending(&hold_a, -10));
    f.sync(&with_holds);
    let mut swapped = pull();
    swapped.accounts[0].transactions.push(fix5_pending(&hold_b, -10));
    let result = f.sync(&swapped);
    assert_eq!((result["new"].as_i64(), result["pending_dropped"].as_i64()), (Some(1), Some(1)), "{result}");
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_transaction WHERE pending=1"), 1);
    assert_eq!(f.item(&hold_b).feed_account_id, f.feed(&acct_a));
    r5_assert_clean(&format!("{:?}", fb_db::test_support::snapshot(f.book.connection()).unwrap()), "fix5 identity book");
    r5_assert_clean(&format!("{first}{again}{result}"), "fix5 sync results");
    r5_finish(&mut f);
}

#[test]
fn fix5_the_same_raw_id_dedupes_across_pulls_and_staging() {
    let mut f = Fx::new();
    let (account, remote) = (fix5_id("same", "account"), fix5_id("same", "tx"));
    let pull = |amount: i64| StagedPull { accounts: vec![acct(&account, "Checking", vec![tx(&remote, "2026-10-20", amount, "Deposit")])], ..Default::default() };
    assert_eq!(f.sync(&pull(100))["new"].as_i64(), Some(1));
    let unchanged = f.sync(&pull(100));
    assert_eq!((unchanged["new"].as_i64(), unchanged["unchanged"].as_i64()), (Some(0), Some(1)), "{unchanged}");
    let updated = f.sync(&pull(150));
    assert_eq!((updated["new"].as_i64(), updated["updated"].as_i64()), (Some(0), Some(1)), "{updated}");
    stage_bank_transactions(&mut f.book, f.u, f.c, &f.conn.clone(), &[(account.clone(), tx(&remote, "2026-10-20", 150, "Deposit"))], &SecretSet::none()).unwrap();
    assert_eq!((f.count("SELECT count(*) FROM bankfeeds_account"), f.count("SELECT count(*) FROM bankfeeds_transaction")), (1, 1));
    assert_eq!(f.item(&remote).amount, 150);
    // A raw id listed twice as new in one pull is still refused, like Python; its userinfo twin is not a repeat.
    let twice = StagedPull { accounts: vec![acct(&account, "Checking", vec![tx("TWICE", "2026-10-21", 1, "x"), tx("TWICE", "2026-10-21", 1, "x")])], ..Default::default() };
    let e = store_pull(&mut f.book, f.u, f.c, &f.conn.clone(), &twice, NOW, false, None, &SecretSet::none()).unwrap_err();
    assert_eq!(e.message(), "Bank feed transaction with this Feed account and Remote id already exists.");
    let twins = StagedPull { accounts: vec![acct(&account, "Checking", vec![tx(&fix5_id("x", "t"), "2026-10-21", 1, "x"), tx(&fix5_id("y", "t"), "2026-10-21", 1, "x")])], ..Default::default() };
    assert_eq!(f.sync(&twins)["new"].as_i64(), Some(2));
    r5_finish(&mut f);
}

#[test]
fn fix5_identity_keys_are_keyed_per_book_and_never_hold_the_credential() {
    let mut f = Fx::new();
    let mut g = Fx::new();
    let remote = fix5_id("k", "account");
    let pull = StagedPull { accounts: vec![acct(&remote, "Checking", vec![tx(&fix5_id("k", "tx"), "2026-10-20", 1, "x")])], ..Default::default() };
    assert_eq!(f.book.read(|q| identity_of(q, ACCOUNT_DOMAIN, &remote)).unwrap(), None, "no secret before the first download");
    f.sync(&pull);
    g.sync(&pull);
    let stored = |x: &mut Fx| -> String { x.book.connection().query_row("SELECT remote_identity FROM bankfeeds_account", [], |r| r.get(0)).unwrap() };
    let (kf, kg) = (stored(&mut f), stored(&mut g));
    assert_eq!(kf, f.identity(ACCOUNT_DOMAIN, &remote));
    assert_ne!(kf, kg, "the key is under a per-book secret");
    use sha2::{Digest, Sha256};
    for bare in [Sha256::digest(remote.as_bytes()), Sha256::digest(format!("account\0{remote}").as_bytes())] {
        let bare: String = bare.iter().map(|b| format!("{b:02x}")).collect();
        assert_ne!(kf, bare, "a bare hash could be guessed back to a short password");
    }
    let secret: Vec<u8> = f.book.connection().query_row("SELECT secret FROM bankfeeds_identity_secret", [], |r| r.get(0)).unwrap();
    assert_eq!(secret.len(), 32);
    // The secret is permanent: no replace, change or delete.
    for sql in ["UPDATE bankfeeds_identity_secret SET secret=randomblob(32)", "DELETE FROM bankfeeds_identity_secret",
        "INSERT OR REPLACE INTO bankfeeds_identity_secret(name,secret) VALUES('identity',randomblob(32))", "INSERT INTO bankfeeds_identity_secret(name,secret) VALUES('identity',randomblob(32))"] {
        assert_eq!(raw_err(&f, sql), "bankfeeds_identity", "{sql}");
    }
    for x in [&mut f, &mut g] {
        r5_assert_clean(&format!("{:?}", fb_db::test_support::snapshot(x.book.connection()).unwrap()), "fix5 keyed book");
        r5_finish(x);
    }
}

#[test]
fn fix5_no_sql_path_makes_two_rows_with_one_identity() {
    let mut f = Fx::new();
    // Before any download there is no secret, so a row cannot be written with keys from nowhere.
    let key = "0".repeat(64);
    let account_sql = |id: String, conn: &str, key: &str, remote: &str| format!("INSERT INTO bankfeeds_account(id,connection_id,conn_key,remote_id,name,org_name,currency,balance,available_balance,balance_at,ledger_account_id,mapped_at,first_seen_at,last_seen_at,active,conn_key_identity,remote_identity) VALUES('{id}','{conn}','','{remote}','n','','USD',NULL,NULL,NULL,NULL,NULL,'2026-10-28T00:00:00.000000Z',NULL,1,'{key}','{key}')");
    assert_eq!(raw_err(&f, &account_sql(fb_ids::new_v7().to_string(), &f.conn.clone(), &key, "x")), "bankfeeds_identity");
    f.sync(&StagedPull { accounts: vec![acct(&fix5_id("a", "account"), "Checking", vec![tx("TX-1", "2026-10-20", 1, "x"), tx("TX-2", "2026-10-20", 2, "y")])], ..Default::default() });
    let (feed, ck, rk): (String, String, String) =
        f.book.connection().query_row("SELECT id,conn_key_identity,remote_identity FROM bankfeeds_account", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    let tx_key = |n: &str| -> String { f.book.connection().query_row("SELECT remote_identity FROM bankfeeds_transaction WHERE remote_id=?1", [n], |r| r.get(0)).unwrap() };
    let (k1, k2) = (tx_key("TX-1"), tx_key("TX-2"));
    let tx_sql = |verb: &str, id: String, key: &str| format!("{verb} INTO bankfeeds_transaction(id,ordinal,feed_account_id,remote_id,posted_on,transacted_on,amount,description,pending,first_seen_at,updated_at,status,accepted_as,journal_id,journal_line_id,reference,reference_url,decided_at,decided_by_id,return_count,remote_identity) SELECT '{id}',value+1,'{feed}','other','2026-10-01',NULL,1,'',0,'2026-10-28T00:00:00.000000Z','2026-10-28T00:00:00.000000Z','NEW','',NULL,NULL,'','',NULL,NULL,0,'{key}' FROM bankfeeds_counter");
    let conn = f.conn.clone();
    let mut cases = vec![
        (account_sql(fb_ids::new_v7().to_string(), &conn, &ck, "different display").replace(&format!("'{ck}','{ck}'"), &format!("'{ck}','{rk}'")), "bankfeeds_key"),
        (account_sql(fb_ids::new_v7().to_string(), &conn, &ck, "x").replace("INSERT INTO", "INSERT OR REPLACE INTO").replace(&format!("'{ck}','{ck}'"), &format!("'{ck}','{rk}'")), "bankfeeds_key"),
        (tx_sql("INSERT", fb_ids::new_v7().to_string(), &k1), "bankfeeds_key"),
        (tx_sql("INSERT OR REPLACE", fb_ids::new_v7().to_string(), &k1), "bankfeeds_key"),
        (format!("UPDATE bankfeeds_transaction SET remote_identity='{k1}' WHERE remote_identity='{k2}'"), "bankfeeds_key"),
        (format!("UPDATE OR REPLACE bankfeeds_transaction SET remote_identity='{k1}' WHERE remote_identity='{k2}'"), "bankfeeds_key"),
        (format!("UPDATE bankfeeds_transaction SET remote_identity='{}' WHERE remote_identity='{k2}'", "f".repeat(64)), "bankfeeds_transaction_fixed"),
        (format!("UPDATE bankfeeds_account SET remote_identity='{}'", "f".repeat(64)), "bankfeeds_account_fixed"),
        (tx_sql("INSERT", fb_ids::new_v7().to_string(), "NOT-HEX"), "fb_bankfeeds_identity_form(remote_identity)"),
    ];
    // A second account under the same keys, then one under a fresh remote key (allowed shape, other identity).
    cases.push((account_sql(fb_ids::new_v7().to_string(), &conn, &ck, "x").replace(&format!("'{ck}','{ck}'"), &format!("'{ck}','{rk}'")).replace("INSERT INTO", "INSERT OR IGNORE INTO"), "ignored"));
    for (sql, guard) in cases {
        if guard == "ignored" {
            // OR IGNORE does not skip a RAISE(ABORT): the guard still refuses.
            assert_eq!(raw_err(&f, &sql), "bankfeeds_key", "{sql}");
            continue;
        }
        let got = raw_err(&f, &sql);
        assert!(got.contains(guard), "{sql}: {got} not {guard}");
    }
    // With every bank-feed trigger gone, the table's UNIQUE identity keys still refuse the duplicate.
    let triggers: Vec<String> = {
        let mut st = f.book.connection().prepare("SELECT name FROM sqlite_master WHERE type='trigger' AND tbl_name IN('bankfeeds_account','bankfeeds_transaction')").unwrap();
        st.query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
    };
    let q = f.book.connection();
    q.execute_batch("BEGIN IMMEDIATE").unwrap();
    for t in &triggers {
        q.execute_batch(&format!("DROP TRIGGER {t}")).unwrap();
    }
    let e = q.execute_batch(&tx_sql("INSERT", fb_ids::new_v7().to_string(), &k1)).unwrap_err().to_string();
    assert!(e.contains("UNIQUE constraint failed: bankfeeds_transaction.feed_account_id, bankfeeds_transaction.remote_identity"), "{e}");
    let e = q.execute_batch(&account_sql(fb_ids::new_v7().to_string(), &conn, &ck, "x").replace(&format!("'{ck}','{ck}'"), &format!("'{ck}','{rk}'"))).unwrap_err().to_string();
    assert!(e.contains("UNIQUE constraint failed: bankfeeds_account.connection_id, bankfeeds_account.conn_key_identity, bankfeeds_account.remote_identity"), "{e}");
    q.execute_batch("ROLLBACK").unwrap();
    r5_finish(&mut f);
}
