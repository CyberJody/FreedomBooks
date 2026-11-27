use super::*;

fn desktop(f: &mut Fx, pull: &StagedPull, window: Option<i64>) -> Value {
    desktop_at(f, pull, NOW, window)
}

fn desktop_at(f: &mut Fx, pull: &StagedPull, at: &str, window: Option<i64>) -> Value {
    let proof: Vec<_> = pull
        .accounts
        .iter()
        .map(|a| ProviderIdEvidence {
            account_exact: true,
            transactions_exact: vec![true; a.transactions.len()],
        })
        .collect();
    store_pull_for_desktop(
        &mut f.book,
        f.u,
        f.c,
        &f.conn,
        pull,
        at,
        false,
        window,
        &f.secrets,
        &proof,
    )
    .unwrap()
}

fn settings(f: &mut Fx, changes: &[ImportSettings]) -> FbResult<()> {
    save_mappings_and_import_settings(&mut f.book, f.u, f.c, &f.conn, &[], changes, &f.secrets)
}

fn change(id: &str, state: &str, old: Option<&str>, date: Option<&str>) -> ImportSettings {
    ImportSettings {
        feed_account_id: id.into(),
        expected_state: state.into(),
        expected_date: old.map(str::to_owned),
        import_from_date: date.map(str::to_owned),
    }
}

#[test]
fn discovery_is_metadata_only_and_keeps_redacted_identities_distinct() {
    let mut f = Fx::new();
    let old = StagedPull {
        accounts: vec![acct(
            "old",
            "Old",
            vec![StagedTransaction {
                pending: true,
                ..tx("pending", "2026-10-01", -84, "Pending fixture")
            }],
        )],
        ..Default::default()
    };
    f.sync(&old);
    let cursor = |f: &Fx| {
        f.book.connection().query_row(
        "SELECT last_sync_at,last_sync_ok,sync_count,last_sync_summary FROM bankfeeds_connection WHERE id=?1",
        [&f.conn], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?)),
    ).unwrap()
    };
    let before = cursor(&f);
    let collision = [
        "https://first-mask:secret-one@bank.invalid/a",
        "https://other-mask:secret-two@bank.invalid/a",
    ];
    let mut pull = StagedPull {
        accounts: collision
            .iter()
            .map(|id| acct(id, "New", vec![tx("invalid", "bad-date", 0, "Discard me")]))
            .collect(),
        ..Default::default()
    };
    pull.accounts.push(acct("old", "Renamed", vec![]));
    begin_discovery(&mut f.book, f.u, f.c, &f.conn, NOW).unwrap();
    let result =
        store_discovery(&mut f.book, f.u, f.c, &f.conn, &pull, NOW, &f.secrets, &[]).unwrap();
    assert_eq!(result["accounts"], 3);
    assert!(begin_discovery(&mut f.book, f.u, f.c, &f.conn, NOW).is_err());
    // An audit written under a future clock cannot obscure recent discovery under the corrected clock.
    let earlier = "2026-10-27T12:00:00Z";
    begin_discovery(&mut f.book, f.u, f.c, &f.conn, earlier).unwrap();
    store_discovery(
        &mut f.book,
        f.u,
        f.c,
        &f.conn,
        &StagedPull::default(),
        earlier,
        &f.secrets,
        &[],
    )
    .unwrap();
    assert!(begin_discovery(&mut f.book, f.u, f.c, &f.conn, earlier).is_err());
    assert_eq!(cursor(&f), before);
    assert_eq!(
        f.count("SELECT count(*) FROM ledger_auditevent WHERE action='bankfeed.synced'"),
        1
    );
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_transaction"), 1);
    assert!(f.item("pending").pending);
    assert_eq!(
        f.count("SELECT count(*) FROM bankfeeds_account WHERE name='Renamed'"),
        1
    );
    let [a, b] = collision.map(|id| f.feed(id));
    assert_ne!(a, b);
    let a = feed(f.book.connection(), &a).unwrap().unwrap();
    let b = feed(f.book.connection(), &b).unwrap().unwrap();
    assert_eq!(a.remote_id, b.remote_id);
    assert_eq!(a.import_state, "PENDING");
    assert_eq!(b.import_state, "PENDING");
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_auto_queue"), 0);
    f.reopen_clean();
}

#[test]
fn import_setup_widens_window_and_activates_only_after_a_complete_present_pull() {
    let mut f = Fx::new();
    f.sync(&StagedPull::default());
    let entry = |remote: &str, rows| acct(remote, remote, rows);
    let mut pull = StagedPull {
        accounts: vec![
            entry(
                "new-bank",
                vec![
                    tx("earlier", "2026-08-27", -1, "Prior"),
                    tx("at-start", "2026-08-28", -2, "On floor"),
                    StagedTransaction {
                        posted_on: None,
                        ..tx("undated", "2026-08-28", -3, "No date")
                    },
                    StagedTransaction {
                        transacted_on: Some("2026-08-26".into()),
                        ..tx("pending", "2026-08-28", -4, "Posted date wins")
                    },
                ],
            ),
            entry(
                "new-card",
                vec![
                    tx("card", "2026-10-10", 25, "All available"),
                    StagedTransaction {
                        posted_on: None,
                        pending: true,
                        ..tx("card-pending", "2026-10-10", 50, "No floor")
                    },
                ],
            ),
        ],
        ..Default::default()
    };
    pull.accounts[0].transactions[3].pending = true;
    let initial = desktop(&mut f, &pull, None);
    assert_eq!(initial["awaiting_setup"], 2);
    assert_eq!(initial["new"], 0);
    let (bank, card) = (f.feed("new-bank"), f.feed("new-card"));
    let bank_change = change(&bank, "PENDING", None, Some("2026-08-29"));
    settings(
        &mut f,
        &[bank_change.clone(), change(&card, "PENDING", None, None)],
    )
    .unwrap();
    let before = fb_db::test_support::snapshot(f.book.connection()).unwrap();
    assert!(settings(&mut f, &[bank_change]).is_err());
    assert_eq!(
        fb_db::test_support::snapshot(f.book.connection()).unwrap(),
        before
    );

    settings(
        &mut f,
        &[change(
            &bank,
            "READY",
            Some("2026-08-29"),
            Some("2026-08-28"),
        )],
    )
    .unwrap();
    let start = begin_sync(&mut f.book, f.u, f.c, &f.conn, NOW, 0, true).unwrap();
    let midnight = chrono::NaiveDate::from_ymd_opt(2026, 8, 28)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap();
    let zone = std::env::var("FB_TIME_ZONE")
        .ok()
        .and_then(|z| z.parse().ok())
        .unwrap_or(chrono_tz::America::New_York);
    let expected = chrono::TimeZone::from_local_datetime(&zone, &midnight)
        .earliest()
        .unwrap()
        .timestamp();
    assert_eq!(start, expected);

    pull.errors
        .push("Synthetic provider account not complete".into());
    let partial = desktop(&mut f, &pull, Some(start));
    assert_eq!(partial["awaiting_complete"], 2);
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_transaction"), 0);
    pull.errors.clear();
    let just_bank = StagedPull {
        accounts: vec![pull.accounts[0].clone()],
        ..Default::default()
    };
    let imported = desktop(&mut f, &just_bank, Some(start));
    assert_eq!(
        (
            imported["new"].as_i64(),
            imported["skipped_pre_start"].as_i64(),
            imported["skipped_undated"].as_i64()
        ),
        (Some(2), Some(1), Some(1))
    );
    assert_eq!(imported["activated"], 1);
    assert_eq!(
        feed(f.book.connection(), &card)
            .unwrap()
            .unwrap()
            .import_state,
        "READY"
    );
    assert!(settings(&mut f, &[change(&bank, "READY", Some("2026-08-28"), None)]).is_err());
    assert!(settings(&mut f, &[change(&bank, "ACTIVE", Some("2026-08-28"), None)]).is_err());

    let mut overlap = pull;
    overlap.accounts[0].transactions = vec![
        tx(
            "at-start",
            "2026-08-24",
            -6,
            "Existing provider correction dates below the floor",
        ),
        tx("undated", "2026-09-01", -3, "Now dated"),
        StagedTransaction {
            posted_on: None,
            transacted_on: Some("2026-09-03".into()),
            ..tx("transacted-only", "2026-09-03", -5, "Transaction date")
        },
    ];
    let result = desktop(&mut f, &overlap, Some(start));
    assert_eq!(result["new"], 4);
    assert_eq!(result["pending_dropped"], 1);
    assert_eq!(f.item("at-start").posted_on.as_deref(), Some("2026-08-24"));
    assert_eq!(f.item("undated").amount, -3);
    assert_eq!(f.item("transacted-only").date(), Some("2026-09-03"));
    assert!(f.item("card-pending").pending);
    assert_eq!(
        f.count("SELECT value FROM bankfeeds_counter WHERE name='transaction'"),
        6
    );
    let snapshot = fb_db::test_support::snapshot(f.book.connection()).unwrap();
    let checked = stage_bank_transactions(
        &mut f.book,
        f.u,
        f.c,
        &f.conn,
        &[
            (
                "new-bank".into(),
                tx("would-stage", "2026-09-01", -4, "First"),
            ),
            (
                "new-bank".into(),
                tx("cannot-stage", "2026-08-27", -4, "Too early"),
            ),
        ],
        &f.secrets,
    );
    assert!(msg(checked).contains("import start"));
    assert_eq!(
        fb_db::test_support::snapshot(f.book.connection()).unwrap(),
        snapshot
    );
    f.reopen_clean();
}

#[test]
fn an_older_floor_is_capped_and_reported_even_with_no_transactions() {
    let mut f = Fx::new();
    let pull = StagedPull {
        accounts: vec![acct("old-window", "Old window", vec![])],
        ..Default::default()
    };
    desktop(&mut f, &pull, None);
    let id = f.feed("old-window");
    settings(&mut f, &[change(&id, "PENDING", None, Some("2025-01-01"))]).unwrap();
    let start = begin_sync(&mut f.book, f.u, f.c, &f.conn, NOW, 0, true).unwrap();
    let now = chrono::DateTime::parse_from_rfc3339(NOW)
        .unwrap()
        .timestamp();
    assert_eq!(start, now - MAX_WINDOW_DAYS * 86_400);
    let result = desktop(&mut f, &pull, Some(start));
    assert_eq!(result["activated"], 1);
    assert_eq!(result["missing"]["from"], "2025-01-01");
    assert_eq!(result["new"], 0);
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_counter"), 0);
    let gaps = f.book.read(|q| sync_gaps(q, f.c)).unwrap();
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].from, "2025-01-01");
}

#[test]
fn replacement_feed_pending_sync_does_not_shorten_first_all_available_import() {
    let mut f = Fx::new();
    // Old connection completed a sync on Sep 27 and was mapped to checking, then disconnected.
    let old_pull = StagedPull {
        accounts: vec![acct("old-chk", "Old checking", vec![tx("old-1", "2026-09-25", -10, "Old")])],
        ..Default::default()
    };
    let sep27 = "2026-09-27T12:00:00Z";
    let old_start = begin_sync(&mut f.book, f.u, f.c, &f.conn, sep27, 0, true).unwrap();
    store_pull(&mut f.book, f.u, f.c, &f.conn, &old_pull, sep27, false, Some(old_start), &f.secrets).unwrap();
    let old_feed = f.feed("old-chk");
    let checking = f.a["100"];
    save_mappings(&mut f.book, f.u, f.c, &f.conn, &[(old_feed.clone(), MappingTarget::Existing(Some(checking)))], &f.secrets).unwrap();
    save_mappings(&mut f.book, f.u, f.c, &f.conn, &[(old_feed, MappingTarget::Existing(None))], &f.secrets).unwrap();
    forget(&mut f.book, f.u, f.c, &f.conn, &f.secrets).unwrap();

    // Replacement connection runs a normal desktop sync on Nov 26 while its account is still PENDING.
    let new_conn = connect(&mut f.book, f.u, f.c, &json!("Replacement"), "bridge.example", &f.secrets).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let nov26 = "2026-11-26T12:00:00Z";
    let first_start = begin_sync(&mut f.book, f.u, f.c, &new_conn, nov26, 0, true).unwrap();
    let sep20 = chrono::DateTime::parse_from_rfc3339("2026-09-20T12:00:00Z").unwrap().timestamp();
    assert_eq!(first_start, sep20);
    let rep_pull = StagedPull {
        accounts: vec![acct("rep-chk", "Replacement checking", vec![tx("rep-1", "2026-10-01", -25, "Missed if truncated")])],
        ..Default::default()
    };
    let pending_res = store_pull_for_desktop(&mut f.book, f.u, f.c, &new_conn, &rep_pull, nov26, false, Some(first_start), &f.secrets, &[]).unwrap();
    assert_eq!(pending_res["awaiting_setup"], 1);
    assert_eq!(pending_res["new"], 0);

    // Operator maps replacement account and chooses All available (READY, import_from_date = None).
    let rep_feed = f.feed("rep-chk");
    save_mappings_and_import_settings(
        &mut f.book,
        f.u,
        f.c,
        &new_conn,
        &[(rep_feed.clone(), MappingTarget::Existing(Some(checking)))],
        &[change(&rep_feed, "PENDING", None, None)],
        &f.secrets,
    )
    .unwrap();

    // First real import must still reach back to Sep 20 (NOT Oct 13 / 44 days!).
    let import_start = begin_sync(&mut f.book, f.u, f.c, &new_conn, nov26, 0, true).unwrap();
    assert_eq!(import_start, sep20, "PENDING-skipped sync must not shrink replacement coverage");
    let imported = store_pull_for_desktop(&mut f.book, f.u, f.c, &new_conn, &rep_pull, nov26, false, Some(import_start), &f.secrets, &[]).unwrap();
    assert_eq!(imported["activated"], 1);
    assert_eq!(imported["new"], 1);
}

fn at_epoch(at: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(at).unwrap().timestamp()
}

fn pull_accounts(names: &[&str]) -> StagedPull {
    StagedPull {
        accounts: names.iter().map(|name| acct(name, name, vec![])).collect(),
        ..Default::default()
    }
}

fn discover_replacement(f: &mut Fx, old: &[(&str, &str, &str)], pull: &StagedPull) {
    for &(remote, code, at) in old {
        let start = at_epoch(at) - FIRST_SYNC_DAYS * 86_400;
        store_pull(&mut f.book, f.u, f.c, &f.conn, &pull_accounts(&[remote]), at, false, Some(start), &f.secrets).unwrap();
        f.map(remote, Some(code)).unwrap();
        f.map(remote, None).unwrap();
    }
    forget(&mut f.book, f.u, f.c, &f.conn, &f.secrets).unwrap();
    f.conn = connect(&mut f.book, f.u, f.c, &json!("Replacement"), "bridge.example", &f.secrets).unwrap()["id"]
        .as_str().unwrap().to_owned();
    store_discovery(&mut f.book, f.u, f.c, &f.conn, pull, NOW, &f.secrets, &[]).unwrap();
}

#[test]
fn a_checking_import_does_not_cover_pending_savings() {
    let mut f = Fx::new();
    let pull = pull_accounts(&["new-checking", "new-savings"]);
    discover_replacement(&mut f, &[("old-savings", "101", "2026-07-01T12:00:00Z")], &pull);
    let checking = f.feed("new-checking");
    let savings = f.feed("new-savings");
    f.map("new-savings", Some("101")).unwrap();
    settings(&mut f, &[change(&checking, "PENDING", None, Some("2025-01-01"))]).unwrap();
    let day = 86_400;
    let checking_start = begin_sync(&mut f.book, f.u, f.c, &f.conn, NOW, 0, true).unwrap();
    assert_eq!(checking_start, at_epoch(NOW) - MAX_WINDOW_DAYS * day);
    assert_eq!(desktop(&mut f, &pull, Some(checking_start))["awaiting_setup"], 1);
    let imported: String = f.book.connection().query_row(
        "SELECT json_extract(detail,'$.imported_feed_ids') FROM ledger_auditevent WHERE target=? AND action='bankfeed.synced'",
        [format!("bankfeedconnection:{}", f.conn)], |row| row.get(0),
    ).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&imported).unwrap(), json!([checking]));

    settings(&mut f, &[change(&savings, "PENDING", None, None)]).unwrap();
    let later = "2026-10-29T12:00:00Z";
    let savings_start = begin_sync(&mut f.book, f.u, f.c, &f.conn, later, 0, true).unwrap();
    assert_eq!(savings_start, at_epoch(later) - MAX_WINDOW_DAYS * day);
    let result = desktop_at(&mut f, &pull, later, Some(savings_start));
    assert_eq!(result["activated"], 1);
    assert_eq!(result["missing"], json!({"from":"2026-06-24","to":"2026-07-31"}));
    let later = "2026-10-30T12:00:00Z";
    let start = begin_sync(&mut f.book, f.u, f.c, &f.conn, later, 0, true).unwrap();
    assert_eq!(start, at_epoch("2026-10-29T12:00:00Z") - OVERLAP_DAYS * day);
    assert_eq!(desktop_at(&mut f, &pull, later, Some(start))["missing"], Value::Null);
}

#[test]
fn an_omitted_feed_keeps_its_own_older_predecessor_and_sync_cursor() {
    let mut f = Fx::new();
    let pull = pull_accounts(&["new-checking", "new-savings"]);
    let old_good = "2026-08-14T12:00:00Z"; // 75 days before the first pull.
    discover_replacement(&mut f, &[
        ("old-savings", "101", old_good),
        ("old-checking", "100", "2026-10-18T12:00:00Z"),
    ], &pull);
    f.map("new-checking", Some("100")).unwrap();
    f.map("new-savings", Some("101")).unwrap();
    let checking = f.feed("new-checking");
    let savings = f.feed("new-savings");
    settings(&mut f, &[change(&checking, "PENDING", None, None), change(&savings, "PENDING", None, None)]).unwrap();
    let day = 86_400;
    let expected = at_epoch(old_good) - OVERLAP_DAYS * day;
    let start = begin_sync(&mut f.book, f.u, f.c, &f.conn, NOW, 0, true).unwrap();
    assert_eq!(start, expected);
    let checking_only = pull_accounts(&["new-checking"]);
    assert_eq!(desktop(&mut f, &checking_only, Some(start))["activated"], 1);

    let later = "2026-10-29T12:00:00Z";
    let start = begin_sync(&mut f.book, f.u, f.c, &f.conn, later, 0, true).unwrap();
    assert_eq!(start, expected);
    let result = desktop_at(&mut f, &pull, later, Some(start));
    assert_eq!(result["activated"], 1);
    assert_eq!(result["missing"], Value::Null);

    let recent = "2026-11-12T12:00:00Z";
    let start = begin_sync(&mut f.book, f.u, f.c, &f.conn, recent, 0, true).unwrap();
    desktop_at(&mut f, &checking_only, recent, Some(start));
    let next = begin_sync(&mut f.book, f.u, f.c, &f.conn, "2026-11-13T12:00:00Z", 0, true).unwrap();
    assert_eq!(next, at_epoch(later) - OVERLAP_DAYS * day);
}

#[test]
fn the_first_pull_after_discovery_reports_an_unreachable_predecessor() {
    let mut f = Fx::new();
    let pull = pull_accounts(&["new-checking"]);
    discover_replacement(&mut f, &[("old-checking", "100", "2026-07-01T12:00:00Z")], &pull);
    f.map("new-checking", Some("100")).unwrap();
    let checking = f.feed("new-checking");
    settings(&mut f, &[change(&checking, "PENDING", None, None)]).unwrap();
    let start = begin_sync(&mut f.book, f.u, f.c, &f.conn, NOW, 0, true).unwrap();
    assert_eq!(start, at_epoch(NOW) - MAX_WINDOW_DAYS * 86_400);
    let first = desktop(&mut f, &pull, Some(start));
    assert_eq!(first["missing"], json!({"from":"2026-06-24","to":"2026-07-30"}));
    assert_eq!(first["activated"], 1);
    assert_eq!(f.book.read(|q| sync_gaps(q, f.c)).unwrap().len(), 1);
}

#[test]
fn legacy_sync_audits_with_an_import_count_still_supply_coverage() {
    let mut f = Fx::new();
    let mut pull = pull_accounts(&["legacy-checking"]);
    pull.errors.push("Synthetic incomplete pull".into());
    store_pull(&mut f.book, f.u, f.c, &f.conn, &pull, NOW, false, None, &f.secrets).unwrap();
    let legacy = "2026-10-08T12:00:00Z";
    f.book.write(|tx| {
        let actor = services::access(tx, f.u, f.c)?;
        for (at, imports) in [(legacy, 1), (NOW, 0)] {
            services::audit(tx, &actor, f.c, "bankfeed.synced",
                format!("bankfeedconnection:{}", f.conn), json!({"ok":true,"at":at,"imported_feeds":imports}))?;
        }
        Ok::<_, FbError>(())
    }).unwrap();
    let start = begin_sync(&mut f.book, f.u, f.c, &f.conn, NOW, 0, true).unwrap();
    assert_eq!(start, at_epoch(legacy) - OVERLAP_DAYS * 86_400);
}

#[test]
fn manual_rules_follow_feed_identity_and_share_position_order() {
    let mut f = Fx::new();
    f.connected_books();
    let checking = f.feed("ACT-checking");
    let savings = f.feed("ACT-savings");
    let mut bank_item = f.item("TX-1001");
    bank_item.description = "Scoped text".into();
    let mut savings_item = bank_item.clone();
    savings_item.feed_account_id = savings.clone();
    let create = |f: &mut Fx, name: &str, scope: Option<&str>| {
        let values = json!({"name":name,"match_text":"scoped text","account_id":f.acc("500"),"feed_account_id":scope});
        save_rule(&mut f.book, f.u, f.c, &values, &f.secrets).unwrap()
    };
    let global = create(&mut f, "Global first", None);
    let scoped = create(&mut f, "Scoped second", Some(&checking));
    let rules = active_rules(&mut f.book, f.u, f.c).unwrap();
    assert_eq!(rule_for(&rules, &bank_item).unwrap().id, global.id);
    move_rule(&mut f.book, f.u, f.c, &scoped.id, &json!("up"), &f.secrets).unwrap();
    let rules = active_rules(&mut f.book, f.u, f.c).unwrap();
    assert_eq!(rule_for(&rules, &bank_item).unwrap().id, scoped.id);
    assert_eq!(rule_for(&rules, &savings_item).unwrap().id, global.id);

    f.map("ACT-checking", None).unwrap();
    f.map("ACT-checking", Some("100")).unwrap();
    assert_eq!(
        rule_for(&active_rules(&mut f.book, f.u, f.c).unwrap(), &bank_item)
            .unwrap()
            .id,
        scoped.id
    );

    forget(&mut f.book, f.u, f.c, &f.conn, &f.secrets).unwrap();
    let rules = active_rules(&mut f.book, f.u, f.c).unwrap();
    assert_eq!(rule_for(&rules, &bank_item).unwrap().id, global.id);
    let edit = json!({"rule_id":scoped.id,"name":"Retained","match_text":"scoped text","account_id":f.acc("500")});
    assert_eq!(
        save_rule(&mut f.book, f.u, f.c, &edit, &f.secrets)
            .unwrap()
            .feed_account_id,
        Some(checking)
    );
    let mut edit = edit;
    edit["feed_account_id"] = json!(savings);
    assert!(
        msg(save_rule(&mut f.book, f.u, f.c, &edit, &f.secrets)).contains("active feed account")
    );
    edit["feed_account_id"] = Value::Null;
    assert_eq!(
        save_rule(&mut f.book, f.u, f.c, &edit, &f.secrets)
            .unwrap()
            .feed_account_id,
        None
    );
    f.reopen_clean();
}

#[test]
fn card_cash_and_balance_transfer_keep_exact_direction_checks_and_undo() {
    let mut f = Fx::new();
    f.connected_books();
    let new_card = services::save_account(
        &mut f.book,
        f.u,
        f.c,
        AccountInput::new("221", "Other card").detail("CREDIT_CARD"),
    )
    .unwrap()
    .id
    .to_db();
    let checking = f.acc("100");
    let card = f.acc("220");
    let pull = StagedPull {
        accounts: vec![acct(
            "ACT-card",
            "Card",
            vec![
                tx("card-cash", "2026-10-20", -10_015, "Cash advance"),
                tx("card-balance", "2026-10-21", 50_012, "Balance transfer"),
            ],
        )],
        ..Default::default()
    };
    f.sync(&pull);
    let input = json!({"kind":"TRANSFER","transfer_account_id":checking});
    let mut split = input.clone();
    split["lines"] = json!([{"account_id":checking,"amount":"100.15"}]);
    assert!(msg(f.accept("card-cash", split)).contains("exactly two accounts"));
    for account in [card, f.acc("105"), f.acc("200"), f.other_expense.to_db()] {
        assert!(
            f.accept(
                "card-cash",
                json!({"kind":"TRANSFER","transfer_account_id":account})
            )
            .is_err()
        );
    }
    let cash = f.accept("card-cash", input.clone()).unwrap();
    assert_eq!(f.accept("card-cash", input).unwrap(), cash);
    assert_eq!(
        f.lines(cash.journal_id.as_deref().unwrap()),
        [("100".into(), 10_015, 0), ("220".into(), 0, 10_015)]
    );
    let balance = f
        .accept(
            "card-balance",
            json!({"kind":"TRANSFER","transfer_account_id":new_card}),
        )
        .unwrap();
    assert_eq!(
        f.lines(balance.journal_id.as_deref().unwrap()),
        [("220".into(), 50_012, 0), ("221".into(), 0, 50_012)]
    );
    let undone = undo_accept(&mut f.book, f.u, f.c, &cash.id, &f.secrets).unwrap();
    assert_eq!(undone.status, "NEW");
    assert_eq!(f.count("SELECT count(*) FROM transfers_transfervoid"), 1);
    f.reopen_clean();
}

#[test]
fn transfers_between_credit_cards_keep_standalone_retries_and_frozen_accounts() {
    let mut f = Fx::new();
    let from = f.acc("220");
    let to = services::save_account(
        &mut f.book,
        f.u,
        f.c,
        AccountInput::new("221", "Second card").detail("CREDIT_CARD"),
    )
    .unwrap()
    .id
    .to_db();
    let input = json!({"request_key":fb_ids::new_v7().to_string(),"date":"2026-10-21",
        "from_account_id":from,"to_account_id":to,"amount":"500.12","memo":"Balance transfer"});
    let review = transfers::review_transfer(&mut f.book, f.u, f.c, &input).unwrap();
    assert_eq!(review.display["purpose"], "Balance transfer");
    let record = transfers::record_transfer(&mut f.book, f.u, f.c, &input, &review).unwrap();
    assert_eq!(
        f.lines(record["journal_id"].as_str().unwrap()),
        [("220".into(), 0, 50_012), ("221".into(), 50_012, 0)]
    );
    assert_eq!(
        transfers::record_transfer(&mut f.book, f.u, f.c, &input, &review).unwrap()["id"],
        record["id"]
    );
    let mut st = f.book.connection().prepare("SELECT role,account_id FROM documents_documentaccount WHERE source_id=?1 ORDER BY role").unwrap();
    let frozen: Vec<(String, String)> = st
        .query_map([record["identity_id"].as_str().unwrap()], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(frozen, [("from".into(), from), ("to".into(), to)]);
    drop(st);
    f.reopen_clean();
}

#[test]
fn raw_transfers_require_eligible_endpoints_the_exact_posting_and_frozen_roles() {
    let mut f = Fx::new();
    let from = f.acc("100");
    let card = f.acc("220");
    let expense = f.acc("500");
    let start = fb_db::test_support::snapshot(f.book.connection()).unwrap();
    for (to, reverse, swap, date, error) in [
        (
            &expense,
            false,
            false,
            "2026-10-21",
            "Choose two eligible bank or credit card accounts",
        ),
        (
            &card,
            false,
            false,
            "2026-10-22",
            "Invalid transfer ownership",
        ),
        (
            &card,
            true,
            false,
            "2026-10-21",
            "Transfer journal must be exactly one credit",
        ),
        (
            &card,
            false,
            true,
            "2026-10-21",
            "Transfer account evidence must be exactly from and to",
        ),
    ] {
        let attempt: FbResult<()> = f.book.write(|w| {
            let identity = documents::create_identity(w, f.u, f.c, "TRF", &fb_ids::new_v7().to_string())?;
            let identity = identity["id"].as_str().unwrap();
            let (locked_from, locked_to) = if swap { (to, &from) } else { (&from, to) };
            documents::snapshot_source_accounts(w, f.u, f.c, identity, &json!({"from": locked_from, "to": locked_to}))?;
            let (credit, debit) = if reverse { (to, &from) } else { (&from, to) };
            let journal = documents::post_source(w, f.u, f.c, identity, "2026-10-21", "Synthetic transfer", vec![
                json!({"account": credit, "credit": "100.00"}), json!({"account": debit, "debit": "100.00"}),
            ])?.id.to_db();
            w.execute(
                "INSERT INTO transfers_transfer(id,company_id,identity_id,creation_digest,review_digest,
                 from_account_id,to_account_id,from_snapshot,to_snapshot,company_snapshot,date,amount,memo,journal_id,actor_id,actor_name,created_at)
                 VALUES (?1,?2,?3,?4,?4,?5,?6,'{}','{}','{}',?7,10000,'Synthetic transfer',?8,?9,'Synthetic operator',?10)",
                params![fb_ids::new_v7().to_string(),f.c.to_db(),identity,"f".repeat(64),from,to,date,journal,f.u.to_db(),py::now_utc()],
            )?;
            Ok(())
        });
        let message = attempt.unwrap_err().to_string();
        assert!(message.contains(error), "{message}");
        assert_eq!(
            fb_db::test_support::snapshot(f.book.connection()).unwrap(),
            start
        );
    }
    f.reopen_clean();
}

#[test]
fn transfer_undo_requires_returning_the_matched_card_or_bank_row_first() {
    let mut f = Fx::new();
    f.connected_books();
    let pull = StagedPull {
        accounts: vec![
            acct(
                "ACT-card",
                "Card",
                vec![tx("visa-payment", "2026-10-15", 75_000, "Payment")],
            ),
            acct(
                "ACT-checking",
                "Checking",
                vec![tx("bank-withdrawal", "2026-10-15", -75_000, "Card payment")],
            ),
        ],
        ..Default::default()
    };
    f.sync(&pull);
    let posted = f
        .accept(
            "visa-payment",
            json!({"kind":"TRANSFER","transfer_account_id":f.acc("100")}),
        )
        .unwrap();
    let other = f.item("bank-withdrawal");
    let suggestion = suggest(&mut f.book, f.u, f.c, &other.id).unwrap();
    assert_eq!(suggestion.kind, "MATCH");
    let line = &suggestion.matches[0].line_id;
    assert_eq!(
        f.accept(
            "bank-withdrawal",
            json!({"kind":"MATCH","journal_line_id":line})
        )
        .unwrap()
        .journal_id,
        posted.journal_id
    );
    let error = msg(undo_accept(&mut f.book, f.u, f.c, &posted.id, &f.secrets));
    assert!(error.contains("Return that one to review first"), "{error}");
    return_to_review(&mut f.book, f.u, f.c, &other.id, &f.secrets).unwrap();
    let undone = undo_accept(&mut f.book, f.u, f.c, &posted.id, &f.secrets).unwrap();
    assert_eq!(undone.status, "NEW");
    assert_eq!(f.count("SELECT count(*) FROM transfers_transfervoid"), 1);
    f.reopen_clean();
}

#[test]
fn legacy_count_only_audit_does_not_shorten_a_ready_feeds_first_import() {
    let mut f = Fx::new();
    // Discover both checking and savings as PENDING, then activate only checking.
    let pull = StagedPull {
        accounts: vec![
            acct("chk", "Checking", vec![tx("c1", "2026-10-20", -100, "Coffee")]),
            acct("sav", "Savings", vec![tx("s1", "2026-10-01", 500, "Deposit")]),
        ],
        ..Default::default()
    };
    desktop(&mut f, &pull, None);
    let chk = f.feed("chk");
    let sav = f.feed("sav");
    settings(&mut f, &[change(&chk, "PENDING", None, None)]).unwrap();
    let oct28 = "2026-10-28T12:00:00Z";
    // Simulate a legacy count-only sync audit (no imported_feed_ids) written while savings was PENDING.
    f.book
        .write(|tx| {
            let actor = services::access(tx, f.u, f.c)?;
            services::audit(
                tx,
                &actor,
                f.c,
                "bankfeed.synced",
                format!("bankfeedconnection:{}", f.conn),
                json!({"ok":true,"at":oct28,"imported_feeds":1}),
            )?;
            Ok::<_, FbError>(())
        })
        .unwrap();
    // Now activate savings as READY ("All available") with no predecessor or explicit date.
    settings(&mut f, &[change(&sav, "PENDING", None, None)]).unwrap();
    let sav_start = begin_sync(&mut f.book, f.u, f.c, &f.conn, "2026-10-29T12:00:00Z", 0, true).unwrap();
    let expected_first =
        chrono::DateTime::parse_from_rfc3339("2026-10-29T12:00:00Z").unwrap().timestamp()
            - FIRST_SYNC_DAYS * 86_400;
    assert_eq!(
        sav_start, expected_first,
        "ambiguous legacy audit must not advance a READY feed's first-import window"
    );
}

#[test]
fn duplicate_account_entries_in_one_pull_emit_unique_imported_feed_ids_and_predecessor_gaps() {
    let mut f = Fx::new();
    let discovery = pull_accounts(&["new-savings"]);
    discover_replacement(
        &mut f,
        &[("old-savings", "101", "2026-07-01T12:00:00Z")],
        &discovery,
    );
    let savings = f.feed("new-savings");
    f.map("new-savings", Some("101")).unwrap();
    settings(&mut f, &[change(&savings, "PENDING", None, None)]).unwrap();
    let start = begin_sync(&mut f.book, f.u, f.c, &f.conn, NOW, 0, true).unwrap();
    let repeated = StagedPull {
        accounts: vec![
            acct("new-savings", "Savings", vec![tx("s1", "2026-10-20", -100, "Coffee")]),
            acct("new-savings", "Savings", vec![tx("s2", "2026-10-21", -250, "Lunch")]),
        ],
        ..Default::default()
    };
    desktop(&mut f, &repeated, Some(start));
    let detail: Value = f
        .book
        .read(|tx| {
            let raw: String = tx.query_row(
                "SELECT detail FROM ledger_auditevent WHERE company_id=?1 AND target=?2 AND action='bankfeed.synced' ORDER BY id DESC LIMIT 1",
                params![f.c.to_db(), format!("bankfeedconnection:{}", f.conn)],
                |r| r.get(0),
            )?;
            Ok::<_, FbError>(serde_json::from_str(&raw).unwrap())
        })
        .unwrap();
    assert_eq!(
        detail["imported_feed_ids"],
        json!([savings]),
        "duplicate pull entries must not repeat imported_feed_ids: {detail}"
    );
    assert_eq!(
        detail["predecessor_gaps"],
        json!([{"feed_id": savings, "from": "2026-06-24"}]),
        "duplicate pull entries must emit exactly one predecessor gap for the repeated feed: {detail}"
    );
}
