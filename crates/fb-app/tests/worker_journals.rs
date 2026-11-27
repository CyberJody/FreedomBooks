//! Bookkeeper test rounds of 2026-11-23: journal entries show their date; a draft can be deleted (items 3, 19).
mod support;
mod worker_support;
use fb_app::Outcome;
use fb_app::commands::StarterChart;
use serde_json::json;
use worker_support::*;

/// Items 3 and 19: the journal page did not show the entry's date; an unbalanced draft could not be deleted.
#[test]
fn a_journal_page_shows_its_date_and_a_draft_can_be_deleted() {
    let (mut app, c) = fixture(StarterChart::Standard);
    let draw = account(&mut app, &c, "3200");
    let checking = account(&mut app, &c, "1000");
    let line = |a: &str, d: &str, cr: &str| json!({"account":a,"debit":d,"credit":cr,"description":"","name":""});
    let draft = run(&mut app, json!({"type":"SaveJournal","company_id":c,"journal_id":null,"date":"2026-05-15","memo":"Debit 10, credit 9","entry_number":"","adjusting":false,"request_key":null,"expected_revision":null,
        "lines":[line(&draw,"10.00","0.00"), line(&checking,"0.00","9.00")]}))["id"].as_str().unwrap().to_owned();
    let html = page(&mut app, &format!("/c/{c}/journals/{draft}/"));
    assert!(html.contains("Entry date <strong>2026-05-15</strong>"), "{html}");
    assert!(html.contains("Out of balance by 1.00") && html.contains(">Delete draft</button>"), "{html}");
    let edit = format!("/c/{c}/journals/{draft}/edit/");
    // A stale page is refused and keeps the draft.
    let Outcome::Invalid { html } = submit(&mut app, &edit, &[("action", "delete_draft"), ("expected_revision", "2")]) else { panic!("stale delete accepted") };
    assert!(html.contains("changed since you opened it"), "{html}");
    let list = redirect(submit(&mut app, &edit, &[("action", "delete_draft"), ("expected_revision", "1")]));
    assert_eq!(list, format!("/c/{c}/"));
    let html = page(&mut app, &list);
    assert!(html.contains("deleted. It had not changed your books."), "{html}");
    let left: i64 = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT count(*) FROM ledger_journal WHERE id=?1", [&draft], |r| r.get(0))).unwrap();
    assert_eq!(left, 0);
    assert!(page(&mut app, &format!("/c/{c}/history/")).contains("Journal draft deleted"));
    // A posted entry shows its date and offers no delete; its reversal starts on the first day of the
    // next month (review S1), marked for the earlier-month notice.
    let posted = run(&mut app, json!({"type":"SaveJournal","company_id":c,"journal_id":null,"date":"2026-03-31","memo":"Owner draw March","entry_number":"","adjusting":false,"request_key":null,"expected_revision":null,
        "lines":[line(&draw,"200.00","0.00"), line(&checking,"0.00","200.00")]}))["id"].as_str().unwrap().to_owned();
    run(&mut app, json!({"type":"PostJournal","company_id":c,"journal_id":posted,"expected_revision":1}));
    let html = page(&mut app, &format!("/c/{c}/journals/{posted}/"));
    assert!(html.contains("Entry date <strong>2026-03-31</strong>") && !html.contains("Delete draft"), "{html}");
    assert!(html.contains("name=\"date\" value=\"2026-04-01\" data-month-notice=\"2026-11-23\""), "reversal date: {html}");
    let Outcome::Invalid { html } = submit(&mut app, &format!("/c/{c}/journals/{posted}/edit/"), &[("action", "delete_draft"), ("expected_revision", "1")]) else {
        panic!("a posted entry was deleted")
    };
    assert!(html.contains("Only a draft journal entry can be deleted. Correct a posted entry by reversing it."), "{html}");
}
