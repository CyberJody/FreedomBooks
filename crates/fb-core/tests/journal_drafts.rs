//! Bookkeeper test rounds of 2026-11-23: a hand-made journal entry draft can be deleted (item 19).
mod common;
use common::*;
use fb_core::FbError;
use fb_core::services;
use serde_json::Value;

fn messages(e: FbError) -> String {
    match e {
        FbError::Validation(m) => m.join("; "),
        other => panic!("expected a validation error, got {other:?}"),
    }
}

/// Item 19: an unbalanced journal entry saved as a draft could not be deleted.
#[test]
fn a_hand_made_draft_journal_can_be_deleted_and_a_posted_one_cannot() {
    let mut f = Fx::new("delete-draft-journal");
    let (c, cash, equity) = f.chart();
    let unbalanced = services::save_draft(
        &mut f.book,
        f.user,
        c,
        services::DraftInput::new(
            "2026-05-15",
            "Debit 10, credit 9",
            vec![services::line(cash).debit("10.00").build(), services::line(equity).credit("9.00").build()],
        ),
    )
    .unwrap();
    assert!(services::post_journal(&mut f.book, f.user, c, unbalanced.id, Some(unbalanced.revision)).is_err());
    // A stale page is refused; the current revision deletes it, its lines and nothing else.
    let e = services::delete_draft_journal(&mut f.book, f.user, c, unbalanced.id, Some(unbalanced.revision + 1)).unwrap_err();
    assert!(messages(e).contains("changed since you opened it"));
    let other = f.company("Other");
    assert!(services::delete_draft_journal(&mut f.book, f.user, other, unbalanced.id, Some(unbalanced.revision)).is_err());
    let kept = f.draft(c, cash, equity, "25.00", "2026-05-16");
    services::delete_draft_journal(&mut f.book, f.user, c, unbalanced.id, Some(unbalanced.revision)).unwrap();
    assert!(fb_core::store::journal(f.book.connection(), unbalanced.id).unwrap().is_none());
    assert_eq!(f.count(&format!("SELECT count(*) FROM ledger_journalline WHERE journal_id='{}'", unbalanced.id.to_db())), 0);
    assert_eq!(f.journal(kept.id).memo, "Synthetic contribution");
    // The audit keeps what the draft held.
    let detail: String = f
        .book
        .connection()
        .query_row("SELECT detail FROM ledger_auditevent WHERE action='journal.deleted' AND target=?1", [format!("journal:{}", unbalanced.id)], |r| r.get(0))
        .unwrap();
    let detail: Value = serde_json::from_str(&detail).unwrap();
    assert_eq!(detail["before"]["memo"], "Debit 10, credit 9");
    assert_eq!(detail["before"]["lines"].as_array().unwrap().len(), 2);
    // Posted entries are corrected by reversal, never deleted; nor is a reversal.
    let posted = f.post(c, kept.id);
    let e = services::delete_draft_journal(&mut f.book, f.user, c, posted.id, None).unwrap_err();
    assert_eq!(messages(e), "Only a draft journal entry can be deleted. Correct a posted entry by reversing it.");
    let reversal = services::reverse_journal(&mut f.book, f.user, c, posted.id, "2026-05-20", "Synthetic reversal").unwrap();
    assert!(services::delete_draft_journal(&mut f.book, f.user, c, reversal.id, None).is_err());
    assert_eq!(f.audit_count("journal.deleted"), 1);
}

/// Review N2 (item 19): a draft that has had a file attached is kept, even after the file is removed
/// (the attachment row stays, so the database would refuse the delete too).
#[test]
fn a_draft_that_has_had_a_file_attached_is_kept() {
    use fb_core::attachments::{self as a, Limits, Owner, storage::Store};
    const PDF: &[u8] = b"%PDF-1.4\n1 0 obj<</Type/Catalog>>endobj\n%%EOF\n";
    let path = book_path("delete-draft-attached");
    let mut book = fb_db::test_support::template_book(&path, &KEY).unwrap();
    let user = fb_core::users::create_user(&mut book, "tester", false).unwrap();
    let mut f = Fx { book, user };
    let (c, cash, equity) = f.chart();
    let draft = f.draft(c, cash, equity, "40.00", "2026-05-15");
    let store = Store::for_book(&path, &KEY).unwrap();
    let (row, _) = a::add(&mut f.book, &store, f.user, c, &Owner::Journal(draft.id), PDF, "receipt", &uuid::Uuid::new_v4().to_string(), &Limits::default()).unwrap();
    let refusal = "This draft has had files attached, so it is kept. A draft does not change your reports.";
    let e = services::delete_draft_journal(&mut f.book, f.user, c, draft.id, Some(draft.revision)).unwrap_err();
    assert_eq!(messages(e), refusal);
    a::remove(&mut f.book, f.user, c, row["id"].as_str().unwrap(), "Wrong receipt").unwrap();
    let e = services::delete_draft_journal(&mut f.book, f.user, c, draft.id, Some(draft.revision)).unwrap_err();
    assert_eq!(messages(e), refusal);
    assert_eq!(f.journal(draft.id).memo, "Synthetic contribution");
    assert_eq!(f.audit_count("journal.deleted"), 0);
}
