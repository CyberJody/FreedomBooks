use super::*;

#[test]
fn retry_aliases_preserve_raw_journal_identity_and_every_dump_reference() {
    let scenario: Value = serde_json::from_str(include_str!(
        "../scenarios/attachments-reminders-retry-alias.json"
    ))
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut runner = Runner::new(&scenario, &dir.path().join("aliases.fbbook"));
    runner.run();
    assert!(runner.outcomes.iter().all(|o| o["ok"] == true));
    let journal = runner.journals["a-bill:journal"];
    assert_eq!(journal, runner.journals["z-bill:journal"]);
    assert_eq!(runner.l2_refs["a-bill"], runner.l2_refs["z-bill"]);
    runner
        .book
        .read::<_, fb_core::FbError>(|q| {
            let raw: String = q.query_row(
                "SELECT journal_id FROM payables_billrevision WHERE journal_id IS NOT NULL",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(raw, journal.to_db());
            let target: String = q.query_row(
                "SELECT target FROM ledger_auditevent WHERE target=?1 LIMIT 1",
                [format!("journal:{raw}")],
                |r| r.get(0),
            )?;
            assert_eq!(target, format!("journal:{raw}"));
            Ok(())
        })
        .unwrap();

    // More aliases expose consumers still choosing a HashMap entry independently.
    for i in 0..32 {
        runner.journals.insert(format!("alias-{i:02}"), journal);
    }
    let ids = runner.l2_audit_ids();
    let mut ids = runner.sales_audit_ids(ids);
    assert_eq!(
        ids["journal"][&journal.to_db()],
        "a-bill:journal",
        "sales audit map"
    );
    runner.extend_payables_audit_ids(&mut ids);
    assert_eq!(
        ids["journal"][&journal.to_db()],
        "a-bill:journal",
        "payables audit map"
    );
    runner.extend_checks_audit_ids(&mut ids);
    assert_eq!(
        ids["journal"][&journal.to_db()],
        "a-bill:journal",
        "checks audit map"
    );
    runner.extend_credits_audit_ids(&mut ids);
    assert_eq!(
        ids["journal"][&journal.to_db()],
        "a-bill:journal",
        "credits audit map"
    );
    runner.extend_banking_audit(&mut ids);
    assert_eq!(
        ids["journal"][&journal.to_db()],
        "a-bill:journal",
        "banking audit map"
    );
    let dump = runner.dump();
    let current = &dump["final"];
    assert_eq!(current["journals"][0]["ref"], "a-bill:journal");
    for row in current["payables"]["billrevision"].as_array().unwrap() {
        if !row["journal_id"].is_null() {
            assert_eq!(row["journal_id"], "a-bill:journal", "payables alias");
        }
    }
    let event = current["audit"]["co"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| {
            e["target"]
                .as_str()
                .is_some_and(|t| t.starts_with("journal:"))
        })
        .unwrap();
    assert_eq!(event["target"], "journal:a-bill:journal", "audit alias");
    assert_eq!(current["l2a"]["source_journals"][0][1], "a-bill:journal");
    assert!(
        current["registers"]
            .as_object()
            .unwrap()
            .values()
            .flat_map(|r| r["rows"].as_array().unwrap())
            .all(|r| r[0] == "a-bill:journal")
    );
    // Earlier checkpoints retain the aliases known then, rather than borrowing future names.
    assert_eq!(
        dump["checkpoints"][10]["dump"]["journals"][0]["ref"],
        "z-bill:journal"
    );
}

#[test]
fn journal_aliases_keep_distinct_ids_and_unicode_order() {
    let first = JournalId::new();
    let second = JournalId::new();
    for aliases in [
        vec![("z", first), ("é", first), ("a", first), ("other", second)],
        vec![("other", second), ("a", first), ("é", first), ("z", first)],
    ] {
        let refs = aliases
            .into_iter()
            .map(|(name, id)| (name.to_owned(), id))
            .collect();
        let names = journal_aliases::journal_aliases(&refs);
        assert_eq!(names.len(), 2);
        assert_eq!(names[&first], "a");
        assert_eq!(names[&second], "other");
    }
}
