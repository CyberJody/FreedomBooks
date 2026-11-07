//! Generic source boundary and nested-transaction guarantees from documents/tests.py.
mod common;
use common::*;
use fb_core::{
    FbError, FbResult, documents as d, services as l,
    services::{DraftInput, line},
};
use serde_json::json;
use std::collections::BTreeSet;

#[test]
fn source_post_retry_generic_guards_and_reversal() {
    let mut f = Fx::new("source");
    let (c, a, b) = f.chart();
    let source = f
        .book
        .write(|tx| d::create_identity(tx, f.user, c, "INV", "00000000-0000-0000-0000-000000000001"))
        .unwrap();
    let s = source["id"].as_str().unwrap();
    let lines = vec![line(a).debit("12").build(), line(b).credit("12").build()];
    let j = f
        .book
        .write(|tx| d::post_source(tx, f.user, c, s, "2026-10-30", "Source", lines.clone()))
        .unwrap();
    let retry = f
        .book
        .write(|tx| d::post_source(tx, f.user, c, s, "2026-10-30", "Source", lines.clone()))
        .unwrap();
    assert_eq!(j.id, retry.id);
    assert!(j.entry_number.is_empty());
    assert_eq!(j.source_id.as_deref(), Some(s));
    let message = "Use the owning source document lifecycle, not generic journal actions.";
    assert_eq!(err_text(l::post_journal(&mut f.book, f.user, c, j.id, None)), message);
    assert_eq!(
        err_text(l::reverse_journal(&mut f.book, f.user, c, j.id, "2026-10-30", "Bypass")),
        message
    );
    let mut input = DraftInput::new("2026-10-30", "Bypass", vec![]);
    input.journal_id = Some(j.id);
    assert_eq!(err_text(l::save_draft(&mut f.book, f.user, c, input)), message);
    let rev = f
        .book
        .write(|tx| d::reverse_source(tx, f.user, c, s, "2026-10-30", "Correction"))
        .unwrap();
    let again = f
        .book
        .write(|tx| d::reverse_source(tx, f.user, c, s, "2026-10-30", "Retry"))
        .unwrap();
    assert_eq!(rev.id, again.id);
    assert_eq!(rev.source_id, j.source_id);
    assert!(f.book.check().unwrap().is_empty());
}
#[test]
fn catching_failed_primitive_does_not_leak_its_counter_identity_or_audit() {
    let mut f = Fx::new("nested");
    let (c, a, b) = f.chart();
    let before = f.count("SELECT count(*) FROM ledger_auditevent");
    f.book.connection().execute_batch("CREATE TRIGGER fail_identified BEFORE INSERT ON ledger_auditevent WHEN NEW.action='document.identified' BEGIN SELECT RAISE(ABORT,'injected audit failure'); END;").unwrap();
    f.book
        .write(|tx| -> FbResult<()> {
            let result = d::create_identity(tx, f.user, c, "INV", "00000000-0000-0000-0000-000000000001");
            assert_eq!(result.unwrap_err().message(), "injected audit failure");
            Ok(())
        })
        .unwrap();
    assert_eq!(f.count("SELECT count(*) FROM documents_documentsequence"), 0);
    assert_eq!(f.count("SELECT count(*) FROM documents_documentidentity"), 0);
    assert_eq!(f.count("SELECT count(*) FROM ledger_auditevent"), before);
    f.book.connection().execute_batch("DROP TRIGGER fail_identified").unwrap();
    let source = f
        .book
        .write(|tx| d::create_identity(tx, f.user, c, "INV", "00000000-0000-0000-0000-000000000001"))
        .unwrap();
    assert_eq!(source["number"], "INV-000001");
    let s = source["id"].as_str().unwrap();
    let rows = vec![line(a).debit("10").build(), line(b).credit("9").build()];
    f.book
        .write(|tx| -> FbResult<()> {
            assert_eq!(d::post_source(tx, f.user, c, s, "2026-10-30", "Unbalanced", rows).unwrap_err().message(), "Posting needs at least two nonzero lines and equal debits and credits (debits 10.00, credits 9.00).");
            Ok(())
        })
        .unwrap();
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal"), 0);
}
#[test]
fn every_new_source_guard_is_attacked() {
    let mut f = Fx::new("source-break");
    let (c, a, b) = f.chart();
    let o = f.company("Other");
    let source = f
        .book
        .write(|tx| d::create_identity(tx, f.user, c, "INV", "00000000-0000-0000-0000-000000000001"))
        .unwrap();
    let s = source["id"].as_str().unwrap();
    let estimate = f
        .book
        .write(|tx| d::create_identity(tx, f.user, c, "EST", "00000000-0000-0000-0000-000000000002"))
        .unwrap();
    let est = estimate["id"].as_str().unwrap();
    let j = f
        .book
        .write(|tx| {
            d::post_source(
                tx,
                f.user,
                c,
                s,
                "2026-10-30",
                "Source",
                vec![line(a).debit("12").build(), line(b).credit("12").build()],
            )
        })
        .unwrap();
    let draft = f.draft(c, a, b, "1", "2026-10-30");
    let header = |extra: &str| {
        format!(
            "INSERT INTO ledger_journal(id,number,company_id,date,memo,created_by_id,created_at,source_id,reversal_of_id) VALUES('{}',999,'{c}','2026-10-30','raw','{}','2026-10-30T00:00:00Z',{extra})",
            fb_core::JournalId::new(),
            f.user
        )
    };
    let mut seen = BTreeSet::new();
    let attacks = vec![
        format!("UPDATE ledger_journal SET source_id='{s}',entry_number='' WHERE id='{}'", draft.id),
        header(&format!("'{est}',NULL")),
        header(&format!("'{}',NULL", fb_ids::new_v7())),
        header(&format!("NULL,'{}'", j.id)),
        format!("UPDATE ledger_account SET company_id='{o}' WHERE id='{a}'"),
    ];
    // Configure a used account: only company reassignment (not kind changes) is forbidden.
    d::save_setup(&mut f.book, f.user, c, &json!({"receivables":a.to_db()}), Some(0)).unwrap();
    for sql in attacks {
        seen.insert(legacy_l2a_attack_message(&f.book, &sql).unwrap_or_else(|| panic!("accepted: {sql}")));
    }
    let sql = format!(
        "UPDATE ledger_journal SET status='POSTED',posted_by_id='{}',posted_at='2026-10-30T00:00:00Z' WHERE id='{}'",
        f.user, draft.id
    );
    seen.insert(legacy_l2a_attack_message(&f.book, &sql).unwrap());
    // 0130 must not mask the source guards these attacks were written for.
    assert!(!seen.iter().any(|m| m.starts_with("fb_valid")), "refused by fb_valid: {seen:?}");
    let messages = |sql: &str| {
        sql.split("RAISE(ABORT, '")
            .skip(1)
            .map(|x| x.split("')").next().unwrap().to_string())
            .collect::<BTreeSet<_>>()
    };
    let old = messages(&fb_db::MIGRATIONS[..4].iter().map(|(_, s)| *s).collect::<String>());
    let current = messages(fb_db::MIGRATIONS[4].1);
    let new = current.difference(&old).collect::<Vec<_>>();
    assert_eq!(new.len(), 5);
    for message in new {
        assert!(seen.contains(message), "guard added without a refusing attack: {message}");
    }
    for msg in [
        "source_journal_ownership: Journal source ownership is immutable",
        "source_journal_scope: Invalid journal source company or type",
        "source_journal_reversal: Reversal must retain original source ownership",
        "account_configured_company: Configured accounts cannot change company",
        "journal_control_account: Manual journal entries cannot post to a receivables, payables, undeposited-funds or collected-tax control account (current setup or frozen by a document)",
    ] {
        assert!(seen.contains(msg), "not exercised: {msg}; seen: {seen:?}");
    }
}
#[test]
fn source_snapshots_and_setup_changes_preserve_historical_controls() {
    let mut f = Fx::new("controls");
    let (c, a, b) = f.chart();
    let src = f
        .book
        .write(|tx| d::create_identity(tx, f.user, c, "EST", "00000000-0000-0000-0000-000000000001"))
        .unwrap();
    let s = src["id"].as_str().unwrap();
    f.book
        .write(|tx| d::snapshot_source_accounts(tx, f.user, c, s, &json!({"receivables:example":a.to_db()})))
        .unwrap();
    assert_eq!(d::control_role(f.book.connection(), c, a).unwrap().as_deref(), Some("receivables"));
    let input = DraftInput::new(
        "2026-10-30",
        "No bypass",
        vec![line(a).debit("1").build(), line(b).credit("1").build()],
    );
    assert_eq!(err_text(l::save_draft(&mut f.book, f.user, c, input)), "1000 · Cash is a receivables (A/R) control account. Customer balances must tie out to it: record an invoice or a customer receipt instead (or a credit memo or refund). Journal entries cannot use control accounts.");
    let result: FbResult<()> = f.book.write(|tx| {
        d::create_identity(tx, f.user, c, "BILL", "00000000-0000-0000-0000-000000000002")?;
        Err(FbError::validation("module rollback"))
    });
    assert_eq!(result.unwrap_err().message(), "module rollback");
}

#[test]
fn manual_journal_controls_report_all_hits_sorted_once_with_python_suffix_and_no_writes() {
    let mut f = Fx::new("control-order");
    let (c, a, b) = f.chart();
    let source = f.book.write(|tx| d::create_identity(tx, f.user, c, "EST", "00000000-0000-0000-0000-000000000009")).unwrap();
    f.book.write(|tx| d::snapshot_source_accounts(tx, f.user, c, source["id"].as_str().unwrap(), &json!({"receivables:a":a.to_db(),"payables:b":b.to_db()}))).unwrap();
    let before = f.count("SELECT count(*) FROM ledger_auditevent");
    let input = DraftInput::new("2026-10-30", "No control journals", vec![line(b).debit("1").build(), line(a).credit("1").build(), line(b).debit("1").build(), line(a).credit("1").build()]);
    let error = l::save_draft(&mut f.book, f.user, c, input).unwrap_err();
    let mut ids = [a,b]; ids.sort();
    let expected: Vec<String> = ids.iter().map(|id| {
        let role = d::control_role(f.book.connection(), c, *id).unwrap().unwrap();
        let account = fb_core::store::account(f.book.connection(), *id).unwrap().unwrap();
        format!("{} Journal entries cannot use control accounts.", d::control_message(&account,&role))
    }).collect();
    assert_eq!(error.messages(), expected);
    assert_eq!(f.count("SELECT count(*) FROM ledger_auditevent"), before);
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal"), 0);
}
