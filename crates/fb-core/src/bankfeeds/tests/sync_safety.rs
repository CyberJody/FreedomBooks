//! Sync safety (review 2026-11-10, code-1 findings 2, 3 and 10): a re-downloaded transaction must not post
//! twice, the sync window must not skip days, and a bank revision of a decided row is surfaced, never applied.
use super::*;
use chrono::DateTime;

/// A feed account of one connection (the same provider id can exist under an old and a new connection).
fn feed_in(f: &mut Fx, conn: &str, remote: &str) -> String {
    let key = f.identity(ACCOUNT_DOMAIN, remote);
    f.book.connection().query_row("SELECT id FROM bankfeeds_account WHERE connection_id=?1 AND remote_identity=?2", [conn, &key], |r| r.get(0)).unwrap()
}
/// A downloaded transaction of one connection.
fn item_in(f: &mut Fx, conn: &str, remote: &str) -> Item {
    let key = f.identity(TRANSACTION_DOMAIN, remote);
    let id: String = f
        .book
        .connection()
        .query_row(
            "SELECT t.id FROM bankfeeds_transaction t JOIN bankfeeds_account a ON a.id=t.feed_account_id WHERE a.connection_id=?1 AND t.remote_identity=?2",
            [conn, &key],
            |r| r.get(0),
        )
        .unwrap();
    f.book.read(|q| item(q, f.c, &id)).unwrap()
}
/// The owner's reinstall: the old connection needs replacing, so its feeds are unmapped and it is forgotten; a
/// new connection downloads `pull` (its first sync) and is mapped to the same ledger accounts. Returns the old
/// connection; `f.conn` becomes the new one.
fn reconnect(f: &mut Fx, pull: &StagedPull) -> String {
    let old = f.conn.clone();
    for remote in ["ACT-checking", "ACT-savings", "ACT-card"] {
        let feed = feed_in(f, &old, remote);
        map_account(&mut f.book, f.u, f.c, &feed, &Value::Null, &SecretSet::none()).unwrap();
    }
    forget(&mut f.book, f.u, f.c, &old, &SecretSet::none()).unwrap();
    f.conn = connect(&mut f.book, f.u, f.c, &json!("SimpleFIN again"), "Bridge.SimpleFIN.org", &SecretSet::none()).unwrap()["id"].as_str().unwrap().to_owned();
    f.sync(pull);
    let new = f.conn.clone();
    for (remote, code) in [("ACT-checking", "100"), ("ACT-savings", "101"), ("ACT-card", "220")] {
        let feed = feed_in(f, &new, remote);
        let account = json!(f.acc(code));
        map_account(&mut f.book, f.u, f.c, &feed, &account, &SecretSet::none()).unwrap();
    }
    old
}
impl Fx {
    fn accept_in(&mut self, conn: &str, remote: &str, input: Value) -> FbResult<Item> {
        let i = item_in(self, conn, remote);
        accept(&mut self.book, self.u, self.c, &i.id, &input, &SecretSet::none())
    }
}
fn posted(f: &mut Fx) -> i64 {
    f.count("SELECT count(*) FROM ledger_journal WHERE status='POSTED'")
}
fn office(f: &Fx) -> Value {
    json!({"kind":"CHECK","account_id":f.acc("500")})
}

#[test]
fn a_transaction_downloaded_again_by_a_new_connection_is_not_posted_twice_without_confirmation() {
    let mut f = Fx::new();
    f.connected_books();
    let input = office(&f);
    let first = f.accept("TX-1001", input.clone()).unwrap();
    let interest = f.accept("TX-1006", json!({"kind":"JOURNAL","account_id":f.acc("410")})).unwrap();
    reconnect(&mut f, &demo());
    let conn = f.conn.clone();
    let again = item_in(&mut f, &conn, "TX-1001");
    assert_eq!(again.status, "NEW");
    let before = posted(&mut f);
    let refused = msg(accept(&mut f.book, f.u, f.c, &again.id, &input, &SecretSet::none()));
    assert!(refused.contains("already recorded") && refused.contains(&first.reference), "{refused}");
    assert_eq!(posted(&mut f), before, "nothing posts without the explicit confirmation");
    assert_eq!(f.count("SELECT count(*) FROM checks_check"), 1, "no draft check is left behind either");
    // A journal acceptance and a bulk-style retry are refused the same way.
    let interest_again = item_in(&mut f, &conn, "TX-1006");
    let journal = json!({"kind":"JOURNAL","account_id":f.acc("410")});
    let refused = msg(accept(&mut f.book, f.u, f.c, &interest_again.id, &journal, &SecretSet::none()));
    assert!(refused.contains(&interest.reference), "{refused}");
    assert_eq!(posted(&mut f), before);
    f.reopen_clean();
}

#[test]
fn no_candidate_guard_stops_earlier_recordings_after_reconnect() {
    let mut f = Fx::new();
    f.connected_books();
    let input = json!({"kind":"JOURNAL","account_id":f.acc("500"),"record_again":true});
    f.accept("TX-1001", input.clone()).unwrap();
    reconnect(&mut f, &demo());
    let connection = f.conn.clone();
    let id = item_in(&mut f, &connection, "TX-1001").id;
    assert_candidate_blocks_accept(&mut f, &id, json!({"match":0,"receipt":0,"recorded":true}), input);
}

#[test]
fn reissued_ids_on_a_new_connection_are_caught_by_the_entry_an_earlier_feed_row_produced() {
    let mut f = Fx::new();
    f.connected_books();
    let input = office(&f);
    f.accept("TX-1001", input.clone()).unwrap();
    let mut reissued = demo();
    for account in &mut reissued.accounts {
        for t in &mut account.transactions {
            t.remote_id = format!("NEW-{}", t.remote_id);
        }
    }
    // The bank dates the purchase a day later this time.
    reissued.accounts[0].transactions[0].posted_on = Some("2026-10-02".into());
    reconnect(&mut f, &reissued);
    let conn = f.conn.clone();
    let again = item_in(&mut f, &conn, "NEW-TX-1001");
    let before = posted(&mut f);
    assert!(msg(accept(&mut f.book, f.u, f.c, &again.id, &input, &SecretSet::none())).contains("already recorded"));
    assert_eq!(posted(&mut f), before);
}

#[test]
fn two_real_purchases_of_the_same_amount_on_one_feed_are_not_called_duplicates() {
    let mut f = Fx::new();
    f.connected_books();
    let input = office(&f);
    f.accept("TX-1001", input.clone()).unwrap();
    let mut more = demo();
    more.accounts[0].transactions.push(tx("TX-1099", "2026-10-02", -8419, "OFFICE DEPOT #221 PURCHASE"));
    f.sync(&more);
    let before = posted(&mut f);
    f.accept("TX-1099", input).unwrap();
    assert_eq!(posted(&mut f), before + 1);
}

#[test]
fn a_failed_attempt_does_not_move_the_next_window_past_the_last_good_sync() {
    let mut f = Fx::new();
    let conn = f.conn.clone();
    // Last good sync 2026-01-01; a failed attempt 2026-02-20; the next good sync 2026-03-01.
    store_pull(&mut f.book, f.u, f.c, &conn, &demo(), "2026-01-01T12:00:00Z", false, None, &SecretSet::none()).unwrap();
    record_sync_failure(&mut f.book, f.u, f.c, &conn, "2026-02-20T12:00:00Z", "SimpleFIN is unavailable", false, &SecretSet::none()).unwrap();
    let start = begin_sync(&mut f.book, f.u, f.c, &conn, "2026-03-01T12:00:00Z", MANUAL_MIN_INTERVAL_SECS, false).unwrap();
    let good = DateTime::parse_from_rfc3339("2026-01-01T12:00:00Z").unwrap().timestamp();
    assert_eq!(start, good - OVERLAP_DAYS * 86_400, "the window reaches back to the last good sync, less the overlap");
}

#[test]
fn a_pull_with_provider_errors_does_not_count_as_a_good_sync_for_the_window() {
    let mut f = Fx::new();
    let conn = f.conn.clone();
    store_pull(&mut f.book, f.u, f.c, &conn, &demo(), "2026-10-01T12:00:00Z", false, None, &SecretSet::none()).unwrap();
    let mut partial = demo();
    partial.errors = vec!["Business Checking: the bank asked to sign in again".into()];
    store_pull(&mut f.book, f.u, f.c, &conn, &partial, "2026-10-20T12:00:00Z", false, None, &SecretSet::none()).unwrap();
    let start = begin_sync(&mut f.book, f.u, f.c, &conn, "2026-10-28T12:00:00Z", MANUAL_MIN_INTERVAL_SECS, true).unwrap();
    let good = DateTime::parse_from_rfc3339("2026-10-01T12:00:00Z").unwrap().timestamp();
    assert_eq!(start, good - OVERLAP_DAYS * 86_400);
}

#[test]
fn an_accepted_transaction_the_bank_revised_is_reported_and_left_alone() {
    let mut f = Fx::new();
    f.connected_books();
    let accepted = f.accept("TX-1001", office(&f)).unwrap();
    let journals = posted(&mut f);
    let mut changed = demo();
    changed.accounts[0].transactions[0].amount = -9419;
    let r = f.sync(&changed);
    assert_eq!(r["unchanged"], 9, "{r}");
    assert_eq!(r["bank_changed"], 1, "{r}");
    let summary = r["summary"].as_str().unwrap();
    assert!(summary.contains("1 accepted transaction was changed by the bank"), "{summary}");
    let i = f.item("TX-1001");
    assert_eq!((i.amount, i.status.as_str(), i.journal_id.clone()), (-8419, "ACCEPTED", accepted.journal_id));
    assert_eq!(posted(&mut f), journals, "nothing changes in the books");
}

#[test]
fn link_to_existing_posts_nothing_and_a_confirmed_second_recording_is_audited() {
    let mut f = Fx::new();
    f.connected_books();
    let input = office(&f);
    let first = f.accept("TX-1001", input.clone()).unwrap();
    f.accept("TX-1004", input.clone()).unwrap();
    let old = f.conn.clone();
    reconnect(&mut f, &demo());
    let conn = f.conn.clone();
    // The first sync of a new connection comes before its mapping; once mapped, the rows are counted.
    assert_eq!(f.book.read(|q| already_recorded_count(q, f.c)).unwrap(), 2);
    // A later download of a row an earlier connection recorded is counted when it is staged.
    let mut later = demo();
    later.accounts[0].transactions.push(tx("NEW-TX-1004", "2026-10-08", -5900, "ACME INTERNET SERVICES AUTOPAY"));
    let summary = f.sync(&later)["summary"].as_str().unwrap().to_owned();
    assert!(summary.contains("1 looks already recorded from an earlier download"), "{summary}");
    let again = item_in(&mut f, &conn, "TX-1001");
    let earlier = item_in(&mut f, &old, "TX-1001");
    let found = f.book.read(|q| already_recorded(q, f.c, &again)).unwrap().unwrap();
    assert_eq!((found.earlier_id.as_str(), found.reference.as_str(), found.same_identity), (earlier.id.as_str(), first.reference.as_str(), true));
    let before = posted(&mut f);
    // The screen showed another entry: nothing is linked.
    assert!(msg(link_existing(&mut f.book, f.u, f.c, &again.id, Some(&again.id), &SecretSet::none())).contains("books changed"));
    let linked = link_existing(&mut f.book, f.u, f.c, &again.id, Some(&earlier.id), &SecretSet::none()).unwrap();
    assert_eq!((linked.status.as_str(), linked.journal_id.clone()), ("IGNORED", None));
    assert_eq!(posted(&mut f), before);
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_link"), 1);
    // A linked row comes back to review in one step, and its link goes with it.
    let back = return_to_review(&mut f.book, f.u, f.c, &again.id, &SecretSet::none()).unwrap();
    assert_eq!(back.status, "NEW");
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_link"), 0);
    // Recording it again needs the explicit confirmation, and the audit names the earlier entry.
    let mut confirmed = input.clone();
    confirmed["record_again"] = json!(true);
    let twice = accept(&mut f.book, f.u, f.c, &again.id, &confirmed, &SecretSet::none()).unwrap();
    assert_eq!(posted(&mut f), before + 1);
    let detail: String = f.book.connection().query_row("SELECT detail FROM ledger_auditevent WHERE action='bankfeed.recorded_again'", [], |r| r.get(0)).unwrap();
    let detail: Value = serde_json::from_str(&detail).unwrap();
    assert_eq!((detail["earlier_reference"].as_str(), detail["reference"].as_str()), (Some(first.reference.as_str()), Some(twice.reference.as_str())));
    // The other re-download still pairs with its own entry, not the one already used.
    let other = item_in(&mut f, &conn, "TX-1004");
    assert_eq!(f.book.read(|q| already_recorded(q, f.c, &other)).unwrap().unwrap().earlier_ordinal, item_in(&mut f, &old, "TX-1004").ordinal);
    // A row never accepted on the old connection is not "already recorded".
    let fresh = item_in(&mut f, &conn, "TX-1003");
    assert!(f.book.read(|q| already_recorded(q, f.c, &fresh)).unwrap().is_none());
    let actions: Vec<String> = {
        let mut st = f.book.connection().prepare("SELECT action FROM ledger_auditevent WHERE target=?1 ORDER BY id").unwrap();
        st.query_map([format!("bankfeedtransaction:{}", again.id)], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
    };
    assert_eq!(actions, ["bankfeed.linked", "bankfeed.returned", "bankfeed.accepted", "bankfeed.recorded_again"]);
    f.reopen_clean();
}

#[test]
fn a_reversed_earlier_entry_is_not_offered_and_matching_is_never_refused() {
    let mut f = Fx::new();
    f.connected_books();
    let input = office(&f);
    let first = f.accept("TX-1001", input.clone()).unwrap();
    reconnect(&mut f, &demo());
    let conn = f.conn.clone();
    let chk = checks::current_checks(&mut f.book, f.u, f.c).unwrap().into_iter().find(|c| c["number"] == first.reference.as_str()).unwrap();
    checks::void_check(&mut f.book, f.u, f.c, chk["id"].as_str().unwrap(), chk["revision"].as_i64(), &json!("2026-10-30"), &json!("Recorded twice")).unwrap();
    let again = item_in(&mut f, &conn, "TX-1001");
    assert!(f.book.read(|q| already_recorded(q, f.c, &again)).unwrap().is_none());
    f.accept_in(&conn, "TX-1001", input).unwrap();
}

#[test]
fn a_capped_window_names_the_dates_that_may_be_missing_until_the_user_reviews_them() {
    let mut f = Fx::new();
    let conn = f.conn.clone();
    store_pull(&mut f.book, f.u, f.c, &conn, &demo(), "2026-01-01T12:00:00Z", false, None, &SecretSet::none()).unwrap();
    // 150 days later the window is capped at 90 days.
    let now = "2026-05-31T12:00:00Z";
    let start = begin_sync(&mut f.book, f.u, f.c, &conn, now, MANUAL_MIN_INTERVAL_SECS, false).unwrap();
    assert_eq!(start, DateTime::parse_from_rfc3339(now).unwrap().timestamp() - MAX_WINDOW_DAYS * 86_400);
    let r = store_pull(&mut f.book, f.u, f.c, &conn, &demo(), now, false, Some(start), &SecretSet::none()).unwrap();
    assert_eq!(r["missing"], json!({"from":"2025-12-25","to":"2026-03-02"}), "{r}");
    assert!(r["summary"].as_str().unwrap().contains("transactions dated 2025-12-25 to 2026-03-02 may be missing"));
    // A later normal sync does not hide the notice.
    let later = "2026-06-01T12:00:00Z";
    let start = begin_sync(&mut f.book, f.u, f.c, &conn, later, MANUAL_MIN_INTERVAL_SECS, false).unwrap();
    let r = store_pull(&mut f.book, f.u, f.c, &conn, &demo(), later, false, Some(start), &SecretSet::none()).unwrap();
    assert_eq!(r["missing"], Value::Null);
    let gaps = f.book.read(|q| sync_gaps(q, f.c)).unwrap();
    assert_eq!(gaps, vec![SyncGap { connection_id: conn.clone(), from: "2025-12-25".into(), to: "2026-03-02".into() }]);
    review_sync_gap(&mut f.book, f.u, f.c, &conn, &SecretSet::none()).unwrap();
    assert!(f.book.read(|q| sync_gaps(q, f.c)).unwrap().is_empty());
    assert!(review_sync_gap(&mut f.book, f.outsider, f.other, &conn, &SecretSet::none()).is_err());
    f.reopen_clean();
}

#[test]
fn a_bank_change_can_be_kept_and_is_reported_again_only_for_a_new_amount() {
    let mut f = Fx::new();
    f.connected_books();
    f.accept("TX-1001", office(&f)).unwrap();
    let mut changed = demo();
    changed.accounts[0].transactions[0].amount = -9419;
    f.sync(&changed);
    let i = f.item("TX-1001");
    let list = f.book.read(|q| bank_changes(q, f.c)).unwrap();
    assert_eq!((list.len(), list[0].recorded, list[0].bank, list[0].ordinal), (1, -8419, -9419, i.ordinal));
    keep_recorded(&mut f.book, f.u, f.c, &i.id, &SecretSet::none()).unwrap();
    assert!(f.book.read(|q| bank_changes(q, f.c)).unwrap().is_empty());
    assert_eq!(f.sync(&changed)["bank_changed"], 0);
    changed.accounts[0].transactions[0].amount = -9519;
    assert_eq!(f.sync(&changed)["bank_changed"], 1);
    // The bank going back to the recorded amount clears it.
    assert_eq!(f.sync(&demo())["bank_changed"], 0);
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_bank_change"), 0);
    // Returned to review (after voiding), the row takes the bank's amount on the next sync.
    f.sync(&changed);
    let chk = checks::current_checks(&mut f.book, f.u, f.c).unwrap().into_iter().find(|c| c["number"] == i.reference.as_str()).unwrap();
    checks::void_check(&mut f.book, f.u, f.c, chk["id"].as_str().unwrap(), chk["revision"].as_i64(), &json!("2026-10-30"), &json!("Bank changed it")).unwrap();
    return_to_review(&mut f.book, f.u, f.c, &i.id, &SecretSet::none()).unwrap();
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_bank_change"), 0);
    f.sync(&changed);
    assert_eq!(f.item("TX-1001").amount, -9519);
    f.reopen_clean();
}

#[test]
fn link_and_bank_change_guards_refuse_direct_sql_the_services_would_refuse() {
    let mut f = Fx::new();
    f.connected_books();
    f.accept("TX-1001", office(&f)).unwrap();
    let accepted = f.item("TX-1001").ordinal;
    let new = f.item("TX-1004").ordinal;
    let other = f.item("TX-3001").ordinal;
    let q = f.book.connection();
    let fails = |sql: String| q.execute_batch(&sql).unwrap_err().to_string();
    // A NEW row cannot carry a link; a row cannot link to an entry of its own downloaded account.
    assert!(fails(format!("INSERT INTO bankfeeds_link VALUES({new},{accepted})")).contains("bankfeeds_link"));
    q.execute_batch(&format!("UPDATE bankfeeds_transaction SET status='IGNORED',decided_at='2026-10-28T12:00:00.000000Z',decided_by_id='{}' WHERE ordinal={new}", f.u.to_db())).unwrap();
    assert!(fails(format!("INSERT INTO bankfeeds_link VALUES({new},{accepted})")).contains("bankfeeds_link"));
    assert!(fails(format!("INSERT INTO bankfeeds_link VALUES({other},{accepted})")).contains("bankfeeds_link"));
    // A bank change needs a decided row and a different amount.
    assert!(fails(format!("INSERT INTO bankfeeds_bank_change VALUES({new},-1,0)")).contains("bankfeeds_bank_change"));
    assert!(fails(format!("INSERT INTO bankfeeds_bank_change VALUES({accepted},-8419,0)")).contains("bankfeeds_bank_change"));
    q.execute_batch(&format!("INSERT INTO bankfeeds_bank_change VALUES({accepted},-1,0)")).unwrap();
    assert!(fails(format!("UPDATE bankfeeds_bank_change SET amount=-8419 WHERE item_ordinal={accepted}")).contains("bankfeeds_bank_change"));
    // Written around the guards, open refuses the books.
    q.execute_batch("DROP TRIGGER fb_bankfeeds_bank_change_update").unwrap();
    q.execute_batch(&format!("UPDATE bankfeeds_bank_change SET amount=-8419 WHERE item_ordinal={accepted}")).unwrap();
    let problems = f.book.check().unwrap();
    assert!(problems.iter().any(|p| p.contains("bank-feed links or bank changes") || p.contains("fb_bankfeeds_bank_change_update")), "{problems:?}");
}

fn reversal_of(f: &mut Fx, journal: &str) -> (String, String) {
    f.book
        .connection()
        .query_row("SELECT r.date,j.date FROM ledger_journal r JOIN ledger_journal j ON j.id=r.reversal_of_id WHERE j.id=?1", [journal], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
}

#[test]
fn undo_voids_the_document_the_acceptance_created_and_returns_the_row_in_one_step() {
    let mut f = Fx::new();
    f.connected_books();
    let receipt = f.undeposited("2500.00", "2026-10-04");
    let cases = [
        ("TX-1001", json!({"kind":"CHECK","account_id":f.acc("500")}), "check.voided"),
        ("TX-1006", json!({"kind":"JOURNAL","account_id":f.acc("410")}), "journal.reversed"),
        ("TX-1005", json!({"kind":"TRANSFER","transfer_account_id":f.acc("101")}), "transfer.voided"),
        ("TX-1003", json!({"kind":"DEPOSIT","receipt_ids":[receipt]}), "deposit.reversed"),
    ];
    for (remote, input, voided) in cases {
        let done = f.accept(remote, input.clone()).unwrap();
        let journal = done.journal_id.clone().unwrap();
        let plan = f.book.read(|q| undo_plan(q, f.u, f.c, &done)).unwrap().unwrap_or_else(|| panic!("{remote} offers Undo"));
        assert_eq!(plan.reference, done.reference);
        let back = undo_accept(&mut f.book, f.u, f.c, &done.id, &SecretSet::none()).unwrap();
        assert_eq!((back.status.as_str(), back.journal_id.clone(), back.return_count), ("NEW", None, 1), "{remote}");
        let (reversal_date, original_date) = reversal_of(&mut f, &journal);
        assert_eq!(reversal_date, original_date, "{remote}: Undo nets the original date to zero");
        let actions: Vec<String> = {
            let mut st = f.book.connection().prepare("SELECT action FROM ledger_auditevent WHERE action IN(?1,'bankfeed.undone','bankfeed.returned') ORDER BY id DESC LIMIT 3").unwrap();
            st.query_map([voided], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
        };
        assert_eq!(actions, ["bankfeed.returned", "bankfeed.undone", voided], "{remote}");
        // Accepting again, differently, works as after a manual void.
        f.accept(remote, input).unwrap();
    }
    f.reopen_clean();
}

#[test]
fn undo_is_not_offered_for_a_match_a_voided_document_or_a_cleared_line() {
    let mut f = Fx::new();
    f.connected_books();
    let transfer = f.accept("TX-1005", json!({"kind":"TRANSFER","transfer_account_id":f.acc("101")})).unwrap();
    let mirror = f.item("TX-2001");
    let s = suggest(&mut f.book, f.u, f.c, &mirror.id).unwrap();
    let matched = accept(&mut f.book, f.u, f.c, &mirror.id, &json!({"kind":"MATCH","journal_line_id":s.matches[0].line_id}), &SecretSet::none()).unwrap();
    assert!(f.book.read(|q| undo_plan(q, f.u, f.c, &matched)).unwrap().is_none());
    // The transfer's savings line is cleared in the register: the reversal would be refused, so no Undo.
    let line: String = f.book.connection().query_row("SELECT l.id FROM ledger_journalline l WHERE l.journal_id=?1 AND l.account_id=?2", [transfer.journal_id.clone().unwrap(), f.acc("101")], |r| r.get(0)).unwrap();
    crate::banking::set_cleared(&mut f.book, f.u, f.c, &line, "C", Some(0), None).unwrap();
    assert!(f.book.read(|q| undo_plan(q, f.u, f.c, &transfer)).unwrap().is_none());
    let before = posted(&mut f);
    assert!(msg(undo_accept(&mut f.book, f.u, f.c, &transfer.id, &SecretSet::none())).contains("cannot be undone here"));
    assert_eq!(posted(&mut f), before);
    // Voided by hand from its page: the manual path (Return to review) applies, not Undo.
    let check = f.accept("TX-1001", office(&f)).unwrap();
    let chk = checks::current_checks(&mut f.book, f.u, f.c).unwrap().into_iter().find(|c| c["number"] == check.reference.as_str()).unwrap();
    checks::void_check(&mut f.book, f.u, f.c, chk["id"].as_str().unwrap(), chk["revision"].as_i64(), &json!("2026-10-30"), &json!("By hand")).unwrap();
    assert!(f.book.read(|q| undo_plan(q, f.u, f.c, &check)).unwrap().is_none());
    assert!(undo_accept(&mut f.book, f.outsider, f.other, &check.id, &SecretSet::none()).is_err());
    f.reopen_clean();
}

// --- review 2026-11-11 (findings 1 to 5, 7): Undo dependents, retired connections, the replacement gap,
// clock skew, reissued ids on the same account ---------------------------------------------------------

/// Live (posted, unreversed, not a reversal) lines on one ledger account with this debit and credit.
fn live_lines(f: &mut Fx, code: &str, debit: i64, credit: i64) -> i64 {
    let acc = f.acc(code);
    f.book
        .connection()
        .query_row(
            "SELECT count(*) FROM ledger_journalline l JOIN ledger_journal j ON j.id=l.journal_id WHERE l.account_id=?1 AND l.debit=?2 AND l.credit=?3 AND j.status='POSTED' AND j.reversal_of_id IS NULL AND NOT EXISTS(SELECT 1 FROM ledger_journal r WHERE r.reversal_of_id=j.id)",
            params![acc, debit, credit],
            |r| r.get(0),
        )
        .unwrap()
}
fn undo_reason(f: &mut Fx, i: &Item) -> Option<String> {
    let i = f.book.read(|q| item(q, f.c, &i.id)).unwrap();
    match f.book.read(|q| undo_status(q, f.u, f.c, &i)).unwrap() {
        Some(Err(reason)) => Some(reason),
        Some(Ok(_)) => None,
        None => Some("<Undo does not apply>".into()),
    }
}
fn match_first(f: &mut Fx, i: &Item) -> Item {
    let s = suggest(&mut f.book, f.u, f.c, &i.id).unwrap();
    accept(&mut f.book, f.u, f.c, &i.id, &json!({"kind":"MATCH","journal_line_id":s.matches[0].line_id}), &SecretSet::none()).unwrap()
}
fn epoch(t: &str) -> i64 {
    DateTime::parse_from_rfc3339(t).unwrap().timestamp()
}

/// Q1: clearing mappings without forgetting the old connection must not allow an unrecoverable Undo.
#[test]
fn followup_q1_an_unforgotten_replaced_connection_cannot_undo_or_return() {
    for forgotten in [false, true] {
        let mut f = Fx::new();
        f.connected_books();
        let input = office(&f);
        let earlier = f.accept("TX-1001", input.clone()).unwrap();
        let old = f.conn.clone();
        let old_feed = feed_in(&mut f, &old, "ACT-checking");
        f.map("ACT-checking", None).unwrap();
        if forgotten {
            forget(&mut f.book, f.u, f.c, &old, &SecretSet::none()).unwrap();
        }
        f.conn = connect(&mut f.book, f.u, f.c, &json!("Replacement"), "Bridge.SimpleFIN.org", &SecretSet::none()).unwrap()["id"].as_str().unwrap().to_owned();
        f.sync(&demo());
        let new = f.conn.clone();
        let new_feed = feed_in(&mut f, &new, "ACT-checking");
        map_account(&mut f.book, f.u, f.c, &new_feed, &json!(f.a["100"].to_db()), &SecretSet::none()).unwrap();
        let reason = undo_reason(&mut f, &earlier).expect("replacement must block Undo");
        assert!(reason.contains("bank connection that has been replaced"), "{reason}");
        let before = fb_db::test_support::snapshot(f.book.connection()).unwrap();
        let refused = msg(undo_accept(&mut f.book, f.u, f.c, &earlier.id, &SecretSet::none()));
        assert!(refused.contains("bank connection that has been replaced"), "{refused}");
        let refused = msg(return_to_review(&mut f.book, f.u, f.c, &earlier.id, &SecretSet::none()));
        assert!(refused.contains("bank connection that has been replaced") && !refused.contains("Void or reverse"), "{refused}");
        assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(), before);
        // Even after a manual void, plain Return must not strand the old accepted row in review.
        let id = earlier.reference_url.rsplit_once(':').unwrap().1;
        let check = checks::get_check(&mut f.book, f.u, f.c, id).unwrap();
        checks::void_check(&mut f.book, f.u, f.c, id, check["revision"].as_i64(), &json!("2026-10-30"), &json!("By hand")).unwrap();
        assert!(msg(return_to_review(&mut f.book, f.u, f.c, &earlier.id, &SecretSet::none())).contains("bank connection that has been replaced"));
        assert_eq!(item_in(&mut f, &old, "TX-1001").status, "ACCEPTED");
        // Freeing the ledger mapping makes this row recoverable again.
        map_account(&mut f.book, f.u, f.c, &new_feed, &Value::Null, &SecretSet::none()).unwrap();
        map_account(&mut f.book, f.u, f.c, &old_feed, &json!(f.a["100"].to_db()), &SecretSet::none()).unwrap();
        return_to_review(&mut f.book, f.u, f.c, &earlier.id, &SecretSet::none()).unwrap();
        f.reopen_clean();
    }
}

#[test]
fn followup_q1_temporarily_unmapped_account_can_be_undone_and_mapped_again() {
    let mut f = Fx::new();
    f.connected_books();
    let input = office(&f);
    let accepted = f.accept("TX-1001", input.clone()).unwrap();
    f.map("ACT-checking", None).unwrap();
    assert!(f.book.read(|q| undo_plan(q, f.u, f.c, &accepted)).unwrap().is_some());
    undo_accept(&mut f.book, f.u, f.c, &accepted.id, &SecretSet::none()).unwrap();
    f.map("ACT-checking", Some("100")).unwrap();
    f.accept("TX-1001", input).unwrap();
    assert_eq!(live_lines(&mut f, "100", 0, 8419), 1);
    f.reopen_clean();
}

/// Q2: distinct ids delivered together can be genuine identical charges.
#[test]
fn followup_q2_identical_same_sync_fees_record_without_confirmation() {
    let mut f = Fx::new();
    let mut pull = demo();
    for n in 0..4 {
        pull.accounts[0].transactions.push(tx(&format!("FEE-{n}"), "2026-10-20", -300, "SERVICE FEE"));
    }
    f.sync(&pull);
    f.map("ACT-checking", Some("100")).unwrap();
    let input = json!({"kind":"JOURNAL","account_id":f.acc("500")});
    for n in 0..4 {
        let i = f.item(&format!("FEE-{n}"));
        assert!(f.book.read(|q| already_recorded(q, f.c, &i)).unwrap().is_none());
        f.accept(&format!("FEE-{n}"), input.clone()).unwrap();
    }
    assert_eq!(f.book.read(|q| already_recorded_count(q, f.c)).unwrap(), 0);
    assert_eq!(live_lines(&mut f, "100", 0, 300), 4);
    assert_eq!(f.count("SELECT count(*) FROM ledger_auditevent WHERE action='bankfeed.recorded_again'"), 0);
    f.reopen_clean();
}

#[test]
fn followup_q2_a_later_download_cannot_make_an_earlier_row_look_recorded() {
    let mut f = Fx::new();
    let mut pull = demo();
    pull.accounts[0].transactions.push(tx("EARLIER-FEE", "2026-10-20", -300, "SERVICE FEE"));
    f.sync(&pull);
    f.map("ACT-checking", Some("100")).unwrap();
    pull.accounts[0].transactions.push(tx("LATER-FEE", "2026-10-20", -300, "SERVICE FEE"));
    f.sync(&pull);
    let input = json!({"kind":"JOURNAL","account_id":f.acc("500")});
    f.accept("LATER-FEE", input.clone()).unwrap();
    let earlier = f.item("EARLIER-FEE");
    assert!(f.book.read(|q| already_recorded(q, f.c, &earlier)).unwrap().is_none());
    f.accept("EARLIER-FEE", input).unwrap();
    assert_eq!(live_lines(&mut f, "100", 0, 300), 2);
    f.reopen_clean();
}

/// Q4: a forgotten connection cannot keep a clock notice alive.
#[test]
fn followup_q4_clock_notice_ignores_forgotten_connections() {
    let mut f = Fx::new();
    let old = f.conn.clone();
    store_pull(&mut f.book, f.u, f.c, &old, &demo(), "2027-03-01T12:00:00Z", false, None, &SecretSet::none()).unwrap();
    let now = "2026-11-11T12:00:00Z";
    assert!(f.book.read(|q| clock_warning(q, f.c, now)).unwrap().is_some());
    forget(&mut f.book, f.u, f.c, &old, &SecretSet::none()).unwrap();
    let new = connect(&mut f.book, f.u, f.c, &json!("Replacement"), "Bridge.SimpleFIN.org", &SecretSet::none()).unwrap()["id"].as_str().unwrap().to_owned();
    store_pull(&mut f.book, f.u, f.c, &new, &demo(), now, false, None, &SecretSet::none()).unwrap();
    assert!(f.book.read(|q| clock_warning(q, f.c, now)).unwrap().is_none());
    f.reopen_clean();
}

/// Review probe R1: a transfer whose other side another feed row matched, and a card payment whose card side
/// was matched, keep their entries: Undo is refused with the reason, until that row is returned.
#[test]
fn undo_is_refused_while_another_downloaded_row_is_matched_to_the_entry() {
    let mut f = Fx::new();
    let mut pull = demo();
    // A card payment from checking: recorded from the card row as a journal against checking, whose checking
    // line the checking row then matches.
    pull.accounts[0].transactions.push(tx("TX-1010", "2026-10-10", 64_040, "CARD PAYMENT REFUND"));
    f.sync(&pull);
    f.map("ACT-checking", Some("100")).unwrap();
    f.map("ACT-savings", Some("101")).unwrap();
    f.map("ACT-card", Some("220")).unwrap();
    let transfer = f.accept("TX-1005", json!({"kind":"TRANSFER","transfer_account_id":f.acc("101")})).unwrap();
    let mirror = f.item("TX-2001");
    let mirror = match_first(&mut f, &mirror);
    assert_eq!(mirror.status, "MATCHED");
    let reason = undo_reason(&mut f, &transfer).unwrap();
    assert!(reason.contains("matched to this entry") && reason.contains("Business Savings"), "{reason}");
    let before = posted(&mut f);
    let refused = msg(undo_accept(&mut f.book, f.u, f.c, &transfer.id, &SecretSet::none()));
    assert!(refused.contains("cannot be undone here") && refused.contains("Return that one to review first"), "{refused}");
    assert_eq!((posted(&mut f), live_lines(&mut f, "101", 50_000, 0)), (before, 1), "the savings side stays in the books");
    // Returned first, the mirror no longer depends on the transfer: Undo is offered and works.
    return_to_review(&mut f.book, f.u, f.c, &mirror.id, &SecretSet::none()).unwrap();
    assert!(f.book.read(|q| undo_plan(q, f.u, f.c, &transfer)).unwrap().is_some());
    undo_accept(&mut f.book, f.u, f.c, &transfer.id, &SecretSet::none()).unwrap();
    assert_eq!((f.item("TX-1005").status.as_str(), f.item("TX-2001").status.as_str()), ("NEW", "NEW"));

    let card = f.accept("TX-3001", json!({"kind":"JOURNAL","account_id":f.acc("100")})).unwrap();
    let checking = f.item("TX-1010");
    let checking = match_first(&mut f, &checking);
    assert_eq!(checking.journal_id, card.journal_id, "the checking row matched the card payment's checking line");
    let reason = undo_reason(&mut f, &card).unwrap();
    assert!(reason.contains("matched to this entry"), "{reason}");
    assert!(undo_accept(&mut f.book, f.u, f.c, &card.id, &SecretSet::none()).is_err());
    assert_eq!(live_lines(&mut f, "100", 64_040, 0), 1);
    f.reopen_clean();
}

/// Review probe R2: after a reconnect, "Link to existing" on the new copy points at the old connection's
/// accepted row. Undo and Return to review of that old row are refused while the link stands, so the
/// transaction can never end up in the books zero times; the old row of a disconnected connection is never
/// undone into a review queue it cannot leave.
#[test]
fn an_earlier_row_a_re_download_is_linked_to_keeps_its_entry() {
    let mut f = Fx::new();
    f.connected_books();
    let input = office(&f);
    f.accept("TX-1001", input.clone()).unwrap();
    let old = reconnect(&mut f, &demo());
    let conn = f.conn.clone();
    let again = item_in(&mut f, &conn, "TX-1001");
    let earlier = item_in(&mut f, &old, "TX-1001");
    link_existing(&mut f.book, f.u, f.c, &again.id, Some(&earlier.id), &SecretSet::none()).unwrap();
    let reason = undo_reason(&mut f, &earlier).unwrap();
    assert!(reason.contains("linked to it") && reason.contains("Return that copy to review first"), "{reason}");
    let before = posted(&mut f);
    assert!(msg(undo_accept(&mut f.book, f.u, f.c, &earlier.id, &SecretSet::none())).contains("cannot be undone here"));
    assert_eq!((posted(&mut f), live_lines(&mut f, "100", 0, 8419)), (before, 1));
    // The copy comes back first; the old row's Undo is still refused: it belongs to a disconnected connection
    // whose account no longer maps, so back in review it could never be recorded again.
    return_to_review(&mut f.book, f.u, f.c, &again.id, &SecretSet::none()).unwrap();
    let reason = undo_reason(&mut f, &earlier).unwrap();
    assert!(reason.contains("bank connection that has been replaced"), "{reason}");
    assert!(undo_accept(&mut f.book, f.u, f.c, &earlier.id, &SecretSet::none()).is_err());
    assert_eq!(live_lines(&mut f, "100", 0, 8419), 1);
    // Linked again, a manual void leaves both rows saying so. Only the new copy can return to review:
    // the replacement still holds the old row's ledger mapping.
    link_existing(&mut f.book, f.u, f.c, &again.id, Some(&earlier.id), &SecretSet::none()).unwrap();
    let chk = checks::current_checks(&mut f.book, f.u, f.c).unwrap().into_iter().find(|c| c["number"] == earlier.reference.as_str()).unwrap();
    checks::void_check(&mut f.book, f.u, f.c, chk["id"].as_str().unwrap(), chk["revision"].as_i64(), &json!("2026-10-30"), &json!("By hand")).unwrap();
    let stale: Vec<i64> = f.book.read(|q| stale_decisions(q, f.c)).unwrap().iter().map(|i| i.ordinal).collect();
    assert_eq!(stale, vec![earlier.ordinal, again.ordinal]);
    let refused = msg(return_to_review(&mut f.book, f.u, f.c, &earlier.id, &SecretSet::none()));
    assert!(refused.contains("cannot return to review yet") && refused.contains("linked to it"), "{refused}");
    let back = return_to_review(&mut f.book, f.u, f.c, &again.id, &SecretSet::none()).unwrap();
    assert!(f.book.read(|q| already_recorded(q, f.c, &back)).unwrap().is_none(), "the voided entry no longer counts");
    let refused = msg(return_to_review(&mut f.book, f.u, f.c, &earlier.id, &SecretSet::none()));
    assert!(refused.contains("bank connection that has been replaced"), "{refused}");
    assert_eq!(f.book.read(|q| stale_decisions(q, f.c)).unwrap().iter().map(|i| i.ordinal).collect::<Vec<_>>(), vec![earlier.ordinal]);
    assert_eq!(f.book.read(|q| downloaded_again(q, f.c, &earlier)).unwrap().map(|i| i.id), Some(back.id.clone()));
    f.accept_in(&conn, "TX-1001", input).unwrap();
    assert_eq!(live_lines(&mut f, "100", 0, 8419), 1);
    f.reopen_clean();
}

/// The manual void of a transfer whose other side is matched leaves both rows flagged with the way forward.
#[test]
fn a_manual_void_leaves_no_row_silently_pointing_at_a_dead_entry() {
    let mut f = Fx::new();
    f.connected_books();
    let transfer = f.accept("TX-1005", json!({"kind":"TRANSFER","transfer_account_id":f.acc("101")})).unwrap();
    let mirror = f.item("TX-2001");
    let mirror = match_first(&mut f, &mirror);
    assert!(f.book.read(|q| stale_decisions(q, f.c)).unwrap().is_empty());
    let id = transfer.reference_url.rsplit_once(':').unwrap().1.to_owned();
    crate::transfers::void_transfer(&mut f.book, f.u, f.c, &id, Some(1), &json!("2026-10-30"), &json!("By hand")).unwrap();
    let stale: Vec<(i64, &str)> =
        f.book.read(|q| stale_decisions(q, f.c)).unwrap().iter().map(|i| (i.ordinal, f.book.read(|q| stale_decision(q, i)).unwrap().unwrap())).collect();
    assert_eq!(stale, vec![(transfer.ordinal, "Its entry was voided or reversed."), (mirror.ordinal, "The entry this was matched to was voided or reversed.")]);
    for i in [&transfer, &mirror] {
        assert_eq!(return_to_review(&mut f.book, f.u, f.c, &i.id, &SecretSet::none()).unwrap().status, "NEW");
    }
    assert!(f.book.read(|q| stale_decisions(q, f.c)).unwrap().is_empty());
    f.reopen_clean();
}

/// Review probe R3: a bank reissuing ids on the same downloaded account. The row looks already recorded by
/// the auto-post path's exact test (same date, amount and description); it cannot be linked (one account),
/// it can be ignored, and a real second identical charge is still recordable with the confirmation.
#[test]
fn a_reissued_id_on_the_same_account_looks_already_recorded() {
    let mut f = Fx::new();
    f.connected_books();
    let input = office(&f);
    let first = f.accept("TX-1001", input.clone()).unwrap();
    let mut again = demo();
    again.accounts[0].transactions[0].remote_id = "REISSUED-TX-1001".into();
    // Same amount and description a day later: a different purchase, never flagged.
    again.accounts[0].transactions.push(tx("TX-1098", "2026-10-02", -8419, "OFFICE DEPOT #221 PURCHASE"));
    let r = f.sync(&again);
    assert!(r["summary"].as_str().unwrap().contains("1 looks already recorded"), "{r}");
    let dup = f.item("REISSUED-TX-1001");
    let found = f.book.read(|q| already_recorded(q, f.c, &dup)).unwrap().unwrap();
    assert_eq!((found.reference.as_str(), found.same_account, found.same_identity), (first.reference.as_str(), true, false));
    let other_day = f.item("TX-1098");
    assert!(f.book.read(|q| already_recorded(q, f.c, &other_day)).unwrap().is_none());
    let before = posted(&mut f);
    assert!(msg(accept(&mut f.book, f.u, f.c, &dup.id, &input, &SecretSet::none())).contains("already recorded"));
    assert!(msg(link_existing(&mut f.book, f.u, f.c, &dup.id, Some(&found.earlier_id), &SecretSet::none())).contains("second copy"));
    assert_eq!((posted(&mut f), live_lines(&mut f, "100", 0, 8419)), (before, 1));
    // Two identical charges on one day are rare but real: the confirmation records it.
    let mut confirmed = input.clone();
    confirmed["record_again"] = json!(true);
    confirmed["recorded_as"] = json!(found.earlier_id);
    accept(&mut f.book, f.u, f.c, &dup.id, &confirmed, &SecretSet::none()).unwrap();
    assert_eq!(live_lines(&mut f, "100", 0, 8419), 2);
    f.reopen_clean();
}

/// Review finding 6, core half: a confirmation names the earlier entry the user was shown, and holds for
/// that one only.
#[test]
fn a_confirmation_for_one_earlier_entry_does_not_record_against_another() {
    let mut f = Fx::new();
    f.connected_books();
    let input = office(&f);
    f.accept("TX-1001", input.clone()).unwrap();
    reconnect(&mut f, &demo());
    let conn = f.conn.clone();
    let again = item_in(&mut f, &conn, "TX-1001");
    let before = posted(&mut f);
    let mut confirmed = input.clone();
    confirmed["record_again"] = json!(true);
    confirmed["recorded_as"] = json!(again.id);
    let refused = msg(accept(&mut f.book, f.u, f.c, &again.id, &confirmed, &SecretSet::none()));
    assert!(refused.contains("books changed since this page was shown"), "{refused}");
    assert_eq!(posted(&mut f), before);
    assert_eq!(f.count("SELECT count(*) FROM checks_check"), 1, "no draft check is left behind");
}

/// Review probe R4: a complete sync recorded under a clock set ahead never moves the window forward, in
/// either order, never holds the next sync back, and the clock notice says so until a sync is recorded at a
/// believable time.
#[test]
fn a_sync_recorded_under_a_clock_set_ahead_never_moves_the_window_forward() {
    for future_last in [false, true] {
        let mut f = Fx::new();
        let conn = f.conn.clone();
        let (future, good) = ("2027-10-28T12:00:00Z", "2026-10-28T12:00:00Z");
        let order = if future_last { [good, future] } else { [future, good] };
        for at in order {
            store_pull(&mut f.book, f.u, f.c, &conn, &demo(), at, false, None, &SecretSet::none()).unwrap();
        }
        let now = "2026-10-29T12:00:00Z";
        let start = begin_sync(&mut f.book, f.u, f.c, &conn, now, MANUAL_MIN_INTERVAL_SECS, false).unwrap();
        assert_eq!(start, epoch(good) - OVERLAP_DAYS * 86_400, "future_last={future_last}");
        let warning = f.book.read(|q| clock_warning(q, f.c, now)).unwrap();
        assert_eq!(warning.is_some(), future_last, "the last recorded sync is later than this clock");
        if let Some(w) = warning {
            assert_eq!((w.recorded.as_str(), w.now.as_str()), ("2027-10-28 08:00", "2026-10-29 08:00"));
        }
        store_pull(&mut f.book, f.u, f.c, &conn, &demo(), now, false, Some(start), &SecretSet::none()).unwrap();
        assert!(f.book.read(|q| clock_warning(q, f.c, now)).unwrap().is_none());
    }
    // Only a future complete sync: none is trusted, so the window reaches as far as the provider allows.
    let mut f = Fx::new();
    let conn = f.conn.clone();
    store_pull(&mut f.book, f.u, f.c, &conn, &demo(), "2027-10-28T12:00:00Z", false, None, &SecretSet::none()).unwrap();
    let now = "2026-10-29T12:00:00Z";
    assert_eq!(begin_sync(&mut f.book, f.u, f.c, &conn, now, MANUAL_MIN_INTERVAL_SECS, false).unwrap(), epoch(now) - MAX_WINDOW_DAYS * 86_400);
}

/// The owner's reconnect: the old connection's accounts were mapped, it is disconnected, and a new connection
/// is made. Old connection (`old_good`), map, unmap, forget; the new connection's first sync, its mapping, its
/// second sync. Returns (first window start, second window start, second sync's `missing`).
fn replace_after(old_good: &str, forget_old: bool) -> (Fx, i64, i64, Value) {
    let mut f = Fx::new();
    let old = f.conn.clone();
    store_pull(&mut f.book, f.u, f.c, &old, &demo(), old_good, false, Some(epoch(old_good) - 30 * 86_400), &SecretSet::none()).unwrap();
    for (remote, code) in [("ACT-checking", Some("100")), ("ACT-checking", None)] {
        let feed = feed_in(&mut f, &old, remote);
        let target = code.map(|c| json!(f.acc(c))).unwrap_or(Value::Null);
        map_account(&mut f.book, f.u, f.c, &feed, &target, &SecretSet::none()).unwrap();
    }
    if forget_old {
        forget(&mut f.book, f.u, f.c, &old, &SecretSet::none()).unwrap();
    }
    let new = connect(&mut f.book, f.u, f.c, &json!("SimpleFIN again"), "Bridge.SimpleFIN.org", &SecretSet::none()).unwrap()["id"].as_str().unwrap().to_owned();
    let now = "2026-10-28T12:00:00Z";
    let first = begin_sync(&mut f.book, f.u, f.c, &new, now, MANUAL_MIN_INTERVAL_SECS, false).unwrap();
    let r = store_pull(&mut f.book, f.u, f.c, &new, &demo(), now, false, Some(first), &SecretSet::none()).unwrap();
    assert_eq!(r["missing"], Value::Null, "before its accounts are mapped, which connection it replaces is unknown");
    let feed = feed_in(&mut f, &new, "ACT-checking");
    let checking = json!(f.acc("100"));
    map_account(&mut f.book, f.u, f.c, &feed, &checking, &SecretSet::none()).unwrap();
    let later = "2026-10-29T12:00:00Z";
    let second = begin_sync(&mut f.book, f.u, f.c, &new, later, MANUAL_MIN_INTERVAL_SECS, false).unwrap();
    let r = store_pull(&mut f.book, f.u, f.c, &new, &demo(), later, false, Some(second), &SecretSet::none()).unwrap();
    f.conn = new;
    (f, first, second, r["missing"].clone())
}

/// Review probe R5: a replacement connection reaches back to the replaced connection's last complete sync
/// within the 90-day cap, and names the dates the cap kept it from; a brand-new company keeps the first-sync
/// window and no notice.
#[test]
fn a_replacement_connection_reaches_back_to_the_connection_it_replaced() {
    let now = epoch("2026-10-28T12:00:00Z");
    let day = 86_400;
    // Disconnected 49 days before: the first sync reaches back to it (beyond the 44-day default).
    let (mut f, first, second, missing) = replace_after("2026-09-10T12:00:00Z", true);
    assert_eq!(first, epoch("2026-09-10T12:00:00Z") - OVERLAP_DAYS * day);
    assert_eq!((second, missing), (now - OVERLAP_DAYS * day, Value::Null));
    assert!(f.book.read(|q| sync_gaps(q, f.c)).unwrap().is_empty());
    // Not yet disconnected (so not known at the first sync): once mapped, the next sync reaches back.
    let (_, first, second, missing) = replace_after("2026-08-20T12:00:00Z", false);
    assert_eq!(first, now - FIRST_SYNC_DAYS * day);
    assert_eq!((second, missing), (epoch("2026-08-20T12:00:00Z") - OVERLAP_DAYS * day, Value::Null));
    // Beyond the cap: the first sync reaches as far as it can, and once the mapping shows what it replaced the
    // dates nobody downloaded are named, once.
    let (mut f, first, second, missing) = replace_after("2026-07-01T12:00:00Z", true);
    assert_eq!(first, now - MAX_WINDOW_DAYS * day);
    assert_eq!(second, now - OVERLAP_DAYS * day, "nothing more can be reached: no repeated 90-day pulls");
    assert_eq!(missing, json!({"from":"2026-06-24","to":"2026-07-30"}));
    let gaps = f.book.read(|q| sync_gaps(q, f.c)).unwrap();
    assert_eq!(gaps, vec![SyncGap { connection_id: f.conn.clone(), from: "2026-06-24".into(), to: "2026-07-30".into() }]);
    let conn = f.conn.clone();
    let later = "2026-10-30T12:00:00Z";
    let start = begin_sync(&mut f.book, f.u, f.c, &conn, later, MANUAL_MIN_INTERVAL_SECS, false).unwrap();
    let r = store_pull(&mut f.book, f.u, f.c, &conn, &demo(), later, false, Some(start), &SecretSet::none()).unwrap();
    assert_eq!(r["missing"], Value::Null, "named once");
    // A brand-new company: the first-sync window and no notice.
    let mut f = Fx::new();
    let conn = f.conn.clone();
    let start = begin_sync(&mut f.book, f.u, f.c, &conn, "2026-10-28T12:00:00Z", MANUAL_MIN_INTERVAL_SECS, false).unwrap();
    assert_eq!(start, now - FIRST_SYNC_DAYS * day);
    let r = store_pull(&mut f.book, f.u, f.c, &conn, &demo(), "2026-10-28T12:00:00Z", false, Some(start), &SecretSet::none()).unwrap();
    assert_eq!(r["missing"], Value::Null);
    f.map("ACT-checking", Some("100")).unwrap();
    let start = begin_sync(&mut f.book, f.u, f.c, &conn, "2026-10-29T12:00:00Z", MANUAL_MIN_INTERVAL_SECS, false).unwrap();
    assert_eq!(start, now - OVERLAP_DAYS * day);
    assert_eq!(store_pull(&mut f.book, f.u, f.c, &conn, &demo(), "2026-10-29T12:00:00Z", false, Some(start), &SecretSet::none()).unwrap()["missing"], Value::Null);
}

/// Review finding 7: missing-date labels use the display zone's day boundaries, like transaction dates.
#[test]
fn missing_dates_are_named_in_the_display_zone_like_transaction_dates() {
    if std::env::var("FB_TIME_ZONE").is_ok_and(|z| z != "America/New_York") {
        return; // the expected dates below are New York's
    }
    let mut f = Fx::new();
    let conn = f.conn.clone();
    // 03:00 UTC is still the previous evening in New York.
    store_pull(&mut f.book, f.u, f.c, &conn, &demo(), "2026-01-01T03:00:00Z", false, None, &SecretSet::none()).unwrap();
    let now = "2026-05-31T03:00:00Z";
    let start = begin_sync(&mut f.book, f.u, f.c, &conn, now, MANUAL_MIN_INTERVAL_SECS, false).unwrap();
    let r = store_pull(&mut f.book, f.u, f.c, &conn, &demo(), now, false, Some(start), &SecretSet::none()).unwrap();
    assert_eq!(r["missing"], json!({"from":"2025-12-24","to":"2026-03-01"}), "{r}");
}

/// Review probe R6 (found sound; kept as a regression): the owner's mix on the old connection is flagged after
/// a reconnect, and a never-recorded row is not.
#[test]
fn every_previously_recorded_kind_is_flagged_after_a_reconnect() {
    let mut f = Fx::new();
    f.connected_books();
    let receipt = f.undeposited("2500.00", "2026-10-04");
    f.accept("TX-1001", office(&f)).unwrap();
    f.accept("TX-1003", json!({"kind":"DEPOSIT","receipt_ids":[receipt]})).unwrap();
    f.accept("TX-1005", json!({"kind":"TRANSFER","transfer_account_id":f.acc("101")})).unwrap();
    let mirror = f.item("TX-2001");
    match_first(&mut f, &mirror);
    f.accept("TX-3001", json!({"kind":"JOURNAL","account_id":f.acc("500")})).unwrap();
    f.journal("2026-10-02", "500", "100", "1200.00");
    let hand = f.item("TX-1002");
    ignore(&mut f.book, f.u, f.c, &hand.id, &SecretSet::none()).unwrap();
    reconnect(&mut f, &demo());
    let conn = f.conn.clone();
    for (remote, flagged) in [("TX-1001", true), ("TX-1003", true), ("TX-1005", true), ("TX-2001", true), ("TX-3001", true), ("TX-1004", false)] {
        let i = item_in(&mut f, &conn, remote);
        assert_eq!(f.book.read(|q| already_recorded(q, f.c, &i)).unwrap().is_some(), flagged, "{remote}");
    }
    // Entered by hand: offered as a match to that entry, not flagged.
    let i = item_in(&mut f, &conn, "TX-1002");
    assert_eq!(suggest(&mut f.book, f.u, f.c, &i.id).unwrap().kind, "MATCH");
}
