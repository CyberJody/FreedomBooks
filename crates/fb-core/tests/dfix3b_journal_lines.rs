mod common;
use common::*;
use fb_core::{services::{self, DraftInput, line}, store};

#[test]
fn legacy_line_text_can_move_but_only_within_its_draft_and_field() {
    let mut f=Fx::new("dfix3b-journal-lines");
    let (c,bank,equity)=f.chart();
    let original=DraftInput::new("2026-10-01","Draft",vec![
        line(bank).debit("5").description("plain").name("plain").build(),
        line(bank).debit("5").description("old description").name("old name").build(),
        line(equity).credit("10").build(),
    ]);
    let journal=services::save_draft(&mut f.book,f.user,c,original.clone()).unwrap();
    let other=services::save_draft(&mut f.book,f.user,c,original).unwrap();
    // Synthetic legacy-shaped test state; the example/driver separately creates
    // this exact three-line draft with genuine 98100e0 services and upgrades it.
    f.book.connection().execute("UPDATE ledger_journalline SET description=?1,name=?2 WHERE journal_id=?3 AND position=2",
        fb_db::rusqlite::params!["Legacy\nline","Legacy\nname",journal.id.to_db()]).unwrap();
    let mut input=DraftInput::new("2026-10-01","Draft",vec![
        line(bank).debit("5").description("Legacy\nline").name("Legacy\nname").build(),
        line(equity).credit("5").build(),
    ]);
    input.journal_id=Some(journal.id);
    input.expected_revision=Some(1);
    let saved=services::save_draft(&mut f.book,f.user,c,input.clone()).unwrap();
    assert_eq!(saved.revision,2);
    let rows=store::lines(f.book.connection(),saved.id).unwrap();
    assert_eq!(rows.len(),2);
    assert_eq!(rows[0].description,"Legacy\nline");
    assert_eq!(rows[0].name,"Legacy\nname");
    input.expected_revision=Some(2);
    let before=fb_db::test_support::snapshot(f.book.connection()).unwrap();
    for (field,value) in [("description","New\nline"),("name","New\nname"),("description","Legacy\nname"),("name","Legacy\nline")] {
        let mut changed=input.clone();
        changed.lines[0][field]=serde_json::json!(value);
        assert_eq!(services::save_draft(&mut f.book,f.user,c,changed).unwrap_err().message(),"Control and formatting characters are not allowed.");
        assert_eq!(before,fb_db::test_support::snapshot(f.book.connection()).unwrap());
    }
    input.journal_id=Some(other.id);
    input.expected_revision=Some(1);
    assert_eq!(services::save_draft(&mut f.book,f.user,c,input.clone()).unwrap_err().message(),"Control and formatting characters are not allowed.");
    input.journal_id=None;
    input.expected_revision=None;
    assert_eq!(services::save_draft(&mut f.book,f.user,c,input).unwrap_err().message(),"Control and formatting characters are not allowed.");
    assert_eq!(before,fb_db::test_support::snapshot(f.book.connection()).unwrap());
    assert!(f.book.check().unwrap().is_empty());
}
