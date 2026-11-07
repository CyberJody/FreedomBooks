use super::*;

fn new(name: &str) -> MappingTarget {
    MappingTarget::New {
        name: name.into(),
        kind: NewAccountKind::Checking,
    }
}

#[test]
fn new_bank_accounts_are_ready_for_checks_and_preserve_existing_setup() {
    let mut f = Fx::new();
    f.sync(&demo());
    let before = f.book.read(|q| documents::setup_snapshot(q, f.c)).unwrap();
    let checking = f.feed("ACT-checking");
    let savings = f.feed("ACT-savings");
    let card = f.feed("ACT-card");
    save_mappings(
        &mut f.book,
        f.u,
        f.c,
        &f.conn,
        &[
            (checking.clone(), new("Ready checking")),
            (
                savings.clone(),
                MappingTarget::New {
                    name: "Ready savings".into(),
                    kind: NewAccountKind::Savings,
                },
            ),
            (
                card.clone(),
                MappingTarget::New {
                    name: "Ready card".into(),
                    kind: NewAccountKind::CreditCard,
                },
            ),
        ],
        &f.secrets,
    )
    .unwrap();
    let after = f.book.read(|q| documents::setup_snapshot(q, f.c)).unwrap();
    for key in [
        "receivables",
        "payables",
        "undeposited",
        "collected_tax",
        "customer_deposits",
    ] {
        assert_eq!(before[key], after[key]);
    }
    for bank in before["banks"].as_array().unwrap() {
        assert!(after["banks"].as_array().unwrap().contains(bank));
    }
    for (id, designated) in [(checking, true), (savings, true), (card, false)] {
        let target = f
            .book
            .read(|q| feed(q, &id))
            .unwrap()
            .unwrap()
            .ledger_account_id
            .unwrap();
        assert_eq!(
            after["banks"].as_array().unwrap().contains(&json!(target)),
            designated
        );
    }
    let account = f.acc("500");
    f.accept("TX-1001",json!({"kind":"CHECK","account_id":account,"payee_name":"Synthetic supplier","check_number":"501"})).unwrap();
}

#[test]
fn setup_audit_failure_rolls_back_new_accounts_mappings_and_designation() {
    let mut f = Fx::new();
    f.sync(&demo());
    let before = f.book.read(|q| documents::setup_snapshot(q, f.c)).unwrap();
    let counts = (
        f.count("SELECT count(*) FROM ledger_account"),
        f.count("SELECT count(*) FROM ledger_auditevent"),
    );
    let id = f.feed("ACT-checking");
    f.book.connection().execute_batch("CREATE TEMP TRIGGER reject_test_setup_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='accounting.setup' BEGIN SELECT RAISE(ABORT,'synthetic late setup failure'); END;").unwrap();
    assert!(
        save_mappings(
            &mut f.book,
            f.u,
            f.c,
            &f.conn,
            &[(id.clone(), new("Rolled back"))],
            &f.secrets
        )
        .is_err()
    );
    assert_eq!(
        before,
        f.book.read(|q| documents::setup_snapshot(q, f.c)).unwrap()
    );
    assert_eq!(
        counts,
        (
            f.count("SELECT count(*) FROM ledger_account"),
            f.count("SELECT count(*) FROM ledger_auditevent")
        )
    );
    assert!(
        f.book
            .read(|q| feed(q, &id))
            .unwrap()
            .unwrap()
            .ledger_account_id
            .is_none()
    );
}

#[test]
fn create_mapping_batch_is_atomic_on_later_currency_failure() {
    let mut f = Fx::new();
    let mut pull = demo();
    pull.accounts[1].currency = "EUR".into();
    f.sync(&pull);
    let checking = f.feed("ACT-checking");
    let savings = f.feed("ACT-savings");
    let before = (
        f.count("SELECT count(*) FROM ledger_account"),
        f.count("SELECT count(*) FROM ledger_auditevent"),
    );
    assert!(
        save_mappings(
            &mut f.book,
            f.u,
            f.c,
            &f.conn,
            &[
                (checking.clone(), new("Created first")),
                (savings, new("Rejected second"))
            ],
            &f.secrets
        )
        .is_err()
    );
    assert_eq!(
        before,
        (
            f.count("SELECT count(*) FROM ledger_account"),
            f.count("SELECT count(*) FROM ledger_auditevent")
        )
    );
    assert!(
        f.book
            .read(|q| feed(q, &checking))
            .unwrap()
            .unwrap()
            .ledger_account_id
            .is_none()
    );
}

#[test]
fn create_mapping_checks_actor_company_connection_and_control_accounts() {
    let mut f = Fx::new();
    f.sync(&demo());
    let id = f.feed("ACT-checking");
    let conn2 = connect(
        &mut f.book,
        f.u,
        f.c,
        &json!("Other connection"),
        "bridge.simplefin.org",
        &f.secrets,
    )
    .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let count = f.count("SELECT count(*) FROM ledger_account");
    for (actor, co, conn) in [
        (f.outsider, f.c, f.conn.clone()),
        (f.u, f.other, f.conn.clone()),
        (f.u, f.c, conn2),
    ] {
        assert!(
            save_mappings(
                &mut f.book,
                actor,
                co,
                &conn,
                &[(id.clone(), new("Refused"))],
                &f.secrets
            )
            .is_err()
        );
    }
    for account in [f.a["110"], f.other_expense, f.a["500"]] {
        assert!(
            save_mappings(
                &mut f.book,
                f.u,
                f.c,
                &f.conn,
                &[(id.clone(), MappingTarget::Existing(Some(account)))],
                &f.secrets
            )
            .is_err()
        );
    }
    assert_eq!(count, f.count("SELECT count(*) FROM ledger_account"));
}

#[test]
fn automatic_codes_skip_collisions_and_accounts_and_audits_mask_credentials() {
    let mut f = Fx::new();
    f.sync(&demo());
    services::save_account(
        &mut f.book,
        f.u,
        f.c,
        AccountInput::new("1000", "Occupied code").detail("BANK"),
    )
    .unwrap();
    let id = f.feed("ACT-checking");
    let secret = "SyntheticSecretForMapping12345";
    let secrets = restore_connection_credentials(
        &mut f.book,
        f.u,
        f.c,
        &f.conn,
        &format!("https://SyntheticUsernameLong:{secret}@bridge.simplefin.org/access"),
        std::iter::empty::<&str>(),
    )
    .unwrap();
    save_mappings(
        &mut f.book,
        f.u,
        f.c,
        &f.conn,
        &[(id.clone(), new(&format!("Edited {secret}")))],
        &secrets,
    )
    .unwrap();
    let feed = f.book.read(|q| feed(q, &id)).unwrap().unwrap();
    let account = f
        .book
        .read(|q| {
            store::account(
                q,
                AccountId::parse(feed.ledger_account_id.as_ref().unwrap()).unwrap(),
            )
        })
        .unwrap()
        .unwrap();
    assert_eq!(account.code, "1001");
    assert!(!account.name.contains(secret));
    assert!(account.name.contains("***"));
    let saved = f.book.read(|q| q.query_row("SELECT count(*) FROM ledger_auditevent WHERE action='account.saved' AND actor_id=?1 AND target=?2", params![f.u.to_db(),format!("account:{}",account.id)],|r|r.get::<_,i64>(0))).unwrap();
    assert_eq!(saved, 1);
    let audits: String = f
        .book
        .read(|q| {
            q.query_row(
                "SELECT group_concat(detail) FROM ledger_auditevent",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert!(!audits.contains(secret));
}

#[test]
fn duplicate_feed_or_account_mapping_rolls_back_the_batch() {
    let mut f = Fx::new();
    f.sync(&demo());
    let checking = f.feed("ACT-checking");
    let savings = f.feed("ACT-savings");
    let count = f.count("SELECT count(*) FROM ledger_account");
    assert!(
        save_mappings(
            &mut f.book,
            f.u,
            f.c,
            &f.conn,
            &[
                (checking.clone(), new("First")),
                (checking.clone(), new("Duplicate"))
            ],
            &f.secrets
        )
        .is_err()
    );
    assert_eq!(count, f.count("SELECT count(*) FROM ledger_account"));
    let target = MappingTarget::Existing(Some(f.a["100"]));
    assert!(
        save_mappings(
            &mut f.book,
            f.u,
            f.c,
            &f.conn,
            &[(checking, target.clone()), (savings, target)],
            &f.secrets
        )
        .is_err()
    );
    assert_eq!(
        f.count("SELECT count(*) FROM bankfeeds_account WHERE ledger_account_id IS NOT NULL"),
        0
    );
}
