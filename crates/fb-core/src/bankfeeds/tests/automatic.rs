use super::*;
use crate::bankfeeds::qbo::{self, Selection};
fn source(auto: bool, amount: Option<&str>) -> qbo::Import {
    let mut cs = vec![
        json!({"ruleType":1,"value":"ACME"}),
        json!({"ruleType":10,"value":"-1"}),
    ];
    if let Some(amount) = amount {
        cs.push(json!({"ruleType":3,"value":amount}));
    }
    qbo::Import{hash:"a".repeat(64),rows:vec![qbo::parse_row(2,"Synthetic imported rule",&json!({"ruleConditions":cs,"isAndRule":true}).to_string(),&json!({"ruleActions":[{"actionType":0,"value":"Office"},{"actionType":8,"value":auto}]}).to_string())]}
}
fn import(f: &mut Fx, auto: bool, amount: Option<&str>) -> FbResult<Value> {
    let scope = f.feed("ACT-checking");
    let selection = Selection {
        row: 2,
        preserve_auto_post: true,
        account_id: f.acc("500"),
        scope,
        kind: "EXPENSE".into(),
        payee_id: String::new(),
        payee_table: String::new(),
    };
    qbo::import_rules(
        &mut f.book,
        f.u,
        f.c,
        &source(auto, amount),
        &[selection],
        true,
        &SecretSet::none(),
    )
}
fn download(f: &mut Fx) -> Item {
    let pull = StagedPull {
        accounts: vec![acct(
            "ACT-checking",
            "Business Checking",
            vec![tx("AUTO-1", "2026-11-05", -5900, "ACME INTERNET")],
        )],
        ..StagedPull::default()
    };
    store_pull_for_desktop(
        &mut f.book,
        f.u,
        f.c,
        &f.conn,
        &pull,
        NOW,
        false,
        None,
        &SecretSet::none(), &[],
    )
    .unwrap();
    f.item("AUTO-1")
}
#[test]
fn import_never_posts_and_automatic_is_balanced_idempotent_and_retained() {
    let mut f = Fx::new();
    f.connected_books();
    import(&mut f, true, None).unwrap();
    let old = f.item("TX-1004");
    assert_eq!(old.status, "NEW");
    assert!(!auto_post(&mut f.book, f.u, f.c, &old.id, &SecretSet::none()).unwrap());
    let i = download(&mut f);
    assert!(auto_post(&mut f.book, f.u, f.c, &i.id, &SecretSet::none()).unwrap());
    assert!(!auto_post(&mut f.book, f.u, f.c, &i.id, &SecretSet::none()).unwrap());
    let done = f.item("AUTO-1");
    assert_eq!(done.status, "ACCEPTED");
    assert_eq!(
        f.lines(done.journal_id.as_ref().unwrap()),
        vec![("100".into(), 0, 5900), ("500".into(), 5900, 0)]
    );
    assert!(import(&mut f, true, None).is_err());
    f.reopen_clean();
}
#[test]
fn missing_foreign_scope_category_and_duplicate_import_fail_atomically() {
    let mut f = Fx::new();
    f.connected_books();
    let base = Selection {
        row: 2,
        preserve_auto_post: true,
        account_id: f.acc("500"),
        scope: String::new(),
        kind: "EXPENSE".into(),
        payee_id: String::new(),
        payee_table: String::new(),
    };
    assert!(
        qbo::import_rules(
            &mut f.book,
            f.u,
            f.c,
            &source(true, None),
            std::slice::from_ref(&base),
            true,
            &SecretSet::none()
        )
        .is_err()
    );
    let scope = f.feed("ACT-checking");
    let other = Selection {
        scope,
        account_id: f.other_expense.to_db(),
        ..base
    };
    assert!(
        qbo::import_rules(
            &mut f.book,
            f.u,
            f.c,
            &source(true, None),
            &[other],
            true,
            &SecretSet::none()
        )
        .is_err()
    );
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_rule"), 0);
}
#[test]
fn priority_signed_boundaries_and_reviewed_states_remain_manual() {
    let mut f = Fx::new();
    f.connected_books();
    save_rule(
        &mut f.book,
        f.u,
        f.c,
        &json!({"name":"Earlier manual","match_text":"ACME","account_id":f.a["500"].to_db()}),
        &SecretSet::none(),
    )
    .unwrap();
    import(&mut f, true, Some("-59.01")).unwrap();
    let i = download(&mut f);
    assert!(!auto_post(&mut f.book, f.u, f.c, &i.id, &SecretSet::none()).unwrap());
    let rules = all_rules(&mut f.book, f.u, f.c).unwrap();
    set_rule_active(
        &mut f.book,
        f.u,
        f.c,
        &rules[0].id,
        &json!(false),
        &SecretSet::none(),
    )
    .unwrap();
    let imported = &rules[1];
    let mut exact = i.clone();
    exact.amount = -5901;
    assert!(rule_for(std::slice::from_ref(imported), &exact).is_none());
    assert!(rule_for(std::slice::from_ref(imported), &i).is_some());
    let mut foreign = i.clone();
    foreign.feed_account_id = f.feed("ACT-savings");
    assert!(rule_for(std::slice::from_ref(imported), &foreign).is_none());
    ignore(&mut f.book, f.u, f.c, &i.id, &SecretSet::none()).unwrap();
    assert!(!auto_post(&mut f.book, f.u, f.c, &i.id, &SecretSet::none()).unwrap());
    return_to_review(&mut f.book, f.u, f.c, &i.id, &SecretSet::none()).unwrap();
    assert!(!auto_post(&mut f.book, f.u, f.c, &i.id, &SecretSet::none()).unwrap());
}
#[test]
fn existing_entry_and_repeated_provider_downloads_do_not_double_post() {
    let mut f = Fx::new();
    f.connected_books();
    import(&mut f, true, None).unwrap();
    let i = download(&mut f);
    f.journal("2026-11-05", "500", "100", "59.00");
    assert!(!auto_post(&mut f.book, f.u, f.c, &i.id, &SecretSet::none()).unwrap());
    assert_eq!(f.item("AUTO-1").status, "NEW");
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_rule_review"), 1);
    let repeat = f.sync(&demo());
    assert!(
        repeat["auto_post_candidates"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn durable_queue_survives_reopen_and_remapped_destinations_require_review() {
    let mut f = Fx::new();
    f.connected_books();
    import(&mut f, true, None).unwrap();
    let i = download(&mut f);
    f.reopen_clean();
    assert_eq!(
        pending_automatic(&mut f.book, f.u, f.c, &f.conn).unwrap(),
        vec![i.id.clone()]
    );
    f.map("ACT-savings", None).unwrap();
    f.map("ACT-checking", Some("101")).unwrap();
    assert!(!auto_post(&mut f.book, f.u, f.c, &i.id, &SecretSet::none()).unwrap());
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal"), 0);
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_auto_queue"), 0);
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_rule_review"), 1);
    f.reopen_clean();
}
#[test]
fn late_audit_failure_rolls_back_post_and_preserves_queue_for_retry() {
    let mut f = Fx::new();
    f.connected_books();
    import(&mut f, true, None).unwrap();
    let i = download(&mut f);
    f.book.connection().execute_batch("CREATE TRIGGER auto_fail_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='bankfeed.rule_auto_posted' BEGIN SELECT RAISE(ABORT,'injected_auto_audit'); END;").unwrap();
    assert!(auto_post(&mut f.book, f.u, f.c, &i.id, &SecretSet::none()).is_err());
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal"), 0);
    assert_eq!(f.item("AUTO-1").status, "NEW");
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_auto_queue"), 1);
    f.book
        .connection()
        .execute_batch("DROP TRIGGER auto_fail_audit")
        .unwrap();
    f.book = Book::open(&f.path, &[91; 32]).unwrap();
    restore_connection_credentials(
        &mut f.book,
        f.u,
        f.c,
        &f.conn,
        "https://synthetic-user:synthetic-password@bridge.simplefin.org/simplefin",
        [] as [&str; 0],
    )
    .unwrap();
    assert!(auto_post(&mut f.book, f.u, f.c, &i.id, &SecretSet::none()).unwrap());
    f.reopen_clean();
}
#[test]
fn credit_card_payment_posts_balanced_transfer_and_counterpart_is_held() {
    let mut f = Fx::new();
    f.connected_books();
    let selection = Selection {
        row: 2,
        preserve_auto_post: true,
        account_id: f.acc("220"),
        scope: f.feed("ACT-checking"),
        kind: "TRANSFER".into(),
        payee_id: String::new(),
        payee_table: String::new(),
    };
    qbo::import_rules(
        &mut f.book,
        f.u,
        f.c,
        &source(true, None),
        &[selection],
        true,
        &SecretSet::none(),
    )
    .unwrap();
    let i = download(&mut f);
    assert!(auto_post(&mut f.book, f.u, f.c, &i.id, &SecretSet::none()).unwrap());
    let done = f.item("AUTO-1");
    assert_eq!(
        f.lines(done.journal_id.as_ref().unwrap()),
        vec![("100".into(), 0, 5900), ("220".into(), 5900, 0)]
    );
    let mut counterpart = source(true, None);
    counterpart.rows[0].name = "Other side".into();
    counterpart.rows[0].direction = "IN".into();
    counterpart.rows[0].source_key = "c".repeat(64);
    let selection = Selection {
        row: 2,
        preserve_auto_post: true,
        account_id: f.acc("100"),
        scope: f.feed("ACT-card"),
        kind: "TRANSFER".into(),
        payee_id: String::new(),
        payee_table: String::new(),
    };
    qbo::import_rules(
        &mut f.book,
        f.u,
        f.c,
        &counterpart,
        &[selection],
        true,
        &SecretSet::none(),
    )
    .unwrap();
    let pull = StagedPull {
        accounts: vec![acct(
            "ACT-card",
            "Card",
            vec![tx("CARD-SIDE", "2026-11-05", 5900, "ACME PAYMENT")],
        )],
        ..StagedPull::default()
    };
    store_pull_for_desktop(
        &mut f.book,
        f.u,
        f.c,
        &f.conn,
        &pull,
        NOW,
        false,
        None,
        &SecretSet::none(), &[],
    )
    .unwrap();
    let side = f.item("CARD-SIDE");
    assert!(!auto_post(&mut f.book, f.u, f.c, &side.id, &SecretSet::none()).unwrap());
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal"), 1);
    f.reopen_clean();
}
#[test]
fn generic_staging_pending_items_and_cross_company_calls_cannot_auto_post() {
    let mut f = Fx::new();
    f.connected_books();
    import(&mut f, true, None).unwrap();
    let mut pull = StagedPull {
        accounts: vec![acct(
            "ACT-checking",
            "Checking",
            vec![tx("GENERIC", "2026-11-06", -4501, "ACME")],
        )],
        ..StagedPull::default()
    };
    f.sync(&pull);
    let generic = f.item("GENERIC");
    assert!(!auto_post(&mut f.book, f.u, f.c, &generic.id, &SecretSet::none()).unwrap());
    pull.accounts[0].transactions[0].remote_id = "PENDING-AUTO".into();
    pull.accounts[0].transactions[0].amount = -4502;
    pull.accounts[0].transactions[0].pending = true;
    store_pull_for_desktop(
        &mut f.book,
        f.u,
        f.c,
        &f.conn,
        &pull,
        NOW,
        false,
        None,
        &SecretSet::none(), &[],
    )
    .unwrap();
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_auto_queue"), 0);
    pull.accounts[0].transactions[0].pending = false;
    store_pull_for_desktop(
        &mut f.book,
        f.u,
        f.c,
        &f.conn,
        &pull,
        NOW,
        false,
        None,
        &SecretSet::none(), &[],
    )
    .unwrap();
    let i = f.item("PENDING-AUTO");
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_auto_queue"), 1);
    assert!(auto_post(&mut f.book, f.u, f.other, &i.id, &SecretSet::none()).is_err());
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_auto_queue"), 1);
    assert!(auto_post(&mut f.book, f.u, f.c, &i.id, &SecretSet::none()).unwrap());
    f.reopen_clean();
}

#[test]
fn inactive_connection_is_held_and_pending_withdrawal_cleans_derived_state() {
    let mut f = Fx::new();
    f.connected_books();
    import(&mut f, true, None).unwrap();
    let i = download(&mut f);
    forget(&mut f.book, f.u, f.c, &f.conn, &SecretSet::none()).unwrap();
    assert!(!auto_post(&mut f.book, f.u, f.c, &i.id, &SecretSet::none()).unwrap());
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal"), 0);
    f.reopen_clean();
    let mut f = Fx::new();
    f.connected_books();
    import(&mut f, true, None).unwrap();
    let mut pending = tx("WITHDRAW-PENDING", "2026-10-27", -8989, "ACME");
    pending.pending = true;
    let pull = StagedPull {
        accounts: vec![acct("ACT-checking", "Checking", vec![pending])],
        ..StagedPull::default()
    };
    f.sync(&pull);
    let i = f.item("WITHDRAW-PENDING");
    // Derived review state must not prevent a provider withdrawal of a pending row.
    f.book.connection().execute("INSERT INTO bankfeeds_rule_review(item_id,reason) VALUES(?1,'Synthetic pending review')",[i.id]).unwrap();
    let pull = StagedPull {
        accounts: vec![acct("ACT-checking", "Checking", vec![])],
        ..StagedPull::default()
    };
    store_pull_for_desktop(
        &mut f.book,
        f.u,
        f.c,
        &f.conn,
        &pull,
        "2026-10-29T12:00:00Z",
        false,
        Some(1793059200),
        &SecretSet::none(), &[],
    )
    .unwrap();
    assert_eq!(
        f.count("SELECT count(*) FROM bankfeeds_transaction WHERE remote_id='WITHDRAW-PENDING'"),
        0
    );
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_rule_review"), 0);
    f.reopen_clean();
}

#[test]
fn changed_download_invalidates_queue_and_any_does_not_override_direction_or_scope() {
    let mut f = Fx::new();
    f.connected_books();
    import(&mut f, true, None).unwrap();
    let i = download(&mut f);
    let mut policy = all_rules(&mut f.book, f.u, f.c).unwrap()[0].policy.clone().unwrap();
    policy.conditions.push(qbo::Condition { code: 2, value: "-1.00".into() });
    policy.all = false;
    assert!(policy.matches(&i));
    policy.all = true;
    assert!(!policy.matches(&i));
    policy.all = false;
    let mut other = i.clone();
    other.amount = 5900;
    assert!(!policy.matches(&other));
    other = i.clone();
    other.feed_account_id = f.feed("ACT-savings");
    assert!(!policy.matches(&other));
    let changed = StagedPull { accounts: vec![acct("ACT-checking", "Business Checking", vec![tx("AUTO-1", "2026-11-05", -5901, "ACME INTERNET")])], ..Default::default() };
    store_pull_for_desktop(&mut f.book, f.u, f.c, &f.conn, &changed, NOW, false, None, &SecretSet::none(), &[]).unwrap();
    assert!(!auto_post(&mut f.book, f.u, f.c, &i.id, &SecretSet::none()).unwrap());
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal"), 0);
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_rule_review"), 1);
    assert!(!auto_post(&mut f.book, f.u, f.c, &i.id, &SecretSet::none()).unwrap());
    f.reopen_clean();
}

#[test]
fn imported_auto_post_can_be_declined_without_losing_suggestions_or_source_evidence() {
    let mut f = Fx::new();
    f.connected_books();
    let selection = Selection {
        row: 2, preserve_auto_post: false, account_id: f.acc("500"),
        scope: f.feed("ACT-checking"), kind: "EXPENSE".into(),
        payee_id: String::new(), payee_table: String::new(),
    };
    let result = qbo::import_rules(&mut f.book, f.u, f.c, &source(true, None), &[selection], true, &SecretSet::none()).unwrap();
    assert_eq!(result["auto_post"], 0);
    let rule = all_rules(&mut f.book, f.u, f.c).unwrap().remove(0);
    assert!(rule.active && !rule.policy.as_ref().unwrap().auto_post);
    assert_eq!(f.count("SELECT count(*) FROM ledger_auditevent WHERE action='bankfeed.rule_imported' AND json_extract(detail,'$.source_auto_post')=1 AND json_extract(detail,'$.policy.auto_post')=0"), 1);
    let item = download(&mut f);
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_auto_queue"), 0);
    assert!(!auto_post(&mut f.book, f.u, f.c, &item.id, &SecretSet::none()).unwrap());
    let suggestion = suggest(&mut f.book, f.u, f.c, &item.id).unwrap();
    assert_eq!(suggestion.account_id, Some(f.acc("500")));
    assert_eq!(suggestion.rule.unwrap().id, rule.id);
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal"), 0);
    f.reopen_clean();
}

#[test]
fn turning_off_imported_auto_post_is_scoped_atomic_idempotent_and_keeps_suggestions() {
    let mut f = Fx::new();
    f.connected_books();
    import(&mut f, true, None).unwrap();
    let item = download(&mut f);
    let before = all_rules(&mut f.book, f.u, f.c).unwrap().remove(0);
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_auto_queue"), 1);
    assert!(turn_off_imported_auto_post(&mut f.book, f.outsider, f.c, None, &SecretSet::none()).is_err());
    assert!(turn_off_imported_auto_post(&mut f.book, f.u, f.other, Some(&before.id), &SecretSet::none()).is_err());
    f.book.connection().execute_batch("CREATE TRIGGER off_fail_audit BEFORE INSERT ON ledger_auditevent WHEN NEW.action='bankfeed.rule_auto_post_disabled' BEGIN SELECT RAISE(ABORT,'injected_off_audit'); END;").unwrap();
    assert!(turn_off_imported_auto_post(&mut f.book, f.u, f.c, None, &SecretSet::none()).is_err());
    assert!(all_rules(&mut f.book, f.u, f.c).unwrap()[0].policy.as_ref().unwrap().auto_post);
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_auto_queue"), 1);
    f.book.connection().execute_batch("DROP TRIGGER off_fail_audit").unwrap();
    assert_eq!(turn_off_imported_auto_post(&mut f.book, f.u, f.c, Some(&before.id), &SecretSet::none()).unwrap(), 1);
    assert_eq!(turn_off_imported_auto_post(&mut f.book, f.u, f.c, None, &SecretSet::none()).unwrap(), 0);
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_auto_queue"), 0);
    assert_eq!(f.count("SELECT count(*) FROM ledger_auditevent WHERE action='bankfeed.rule_auto_post_disabled'"), 1);
    let after = all_rules(&mut f.book, f.u, f.c).unwrap().remove(0);
    assert!(after.active && !after.policy.as_ref().unwrap().auto_post);
    let mut old_policy = serde_json::to_value(before.policy.unwrap()).unwrap();
    old_policy["auto_post"] = json!(false);
    assert_eq!(serde_json::to_value(after.policy.unwrap()).unwrap(), old_policy);
    assert_eq!(before.account_id, after.account_id);
    assert_eq!(before.position, after.position);
    assert_eq!(before.payee_name, after.payee_name);
    assert!(!auto_post(&mut f.book, f.u, f.c, &item.id, &SecretSet::none()).unwrap());
    assert_eq!(suggest(&mut f.book, f.u, f.c, &item.id).unwrap().account_id, Some(f.acc("500")));
    assert_eq!(f.count("SELECT count(*) FROM ledger_journal"), 0);
    // Reloading the same provider input cannot requeue a suggestions-only rule.
    download(&mut f);
    assert_eq!(f.count("SELECT count(*) FROM bankfeeds_auto_queue"), 0);
    f.reopen_clean();
    assert!(!all_rules(&mut f.book, f.u, f.c).unwrap()[0].policy.as_ref().unwrap().auto_post);
}

#[test]
fn turning_off_auto_post_preserves_inactive_rules_and_legacy_rules() {
    let mut f = Fx::new();
    f.connected_books();
    import(&mut f, true, None).unwrap();
    let id = all_rules(&mut f.book, f.u, f.c).unwrap()[0].id.clone();
    set_rule_active(&mut f.book, f.u, f.c, &id, &json!(false), &SecretSet::none()).unwrap();
    let account = f.acc("500");
    let legacy = save_rule(&mut f.book, f.u, f.c, &json!({"name":"Legacy suggestion","match_text":"Legacy","account_id":account}), &SecretSet::none()).unwrap();
    let foreign = Selection { row:2, preserve_auto_post:true, account_id:f.other_expense.to_db(), scope:"ALL".into(), kind:"EXPENSE".into(), payee_id:String::new(), payee_table:String::new() };
    qbo::import_rules(&mut f.book, f.outsider, f.other, &source(true,None), &[foreign], true, &SecretSet::none()).unwrap();
    assert!(turn_off_imported_auto_post(&mut f.book, f.u, f.c, Some(&legacy.id), &SecretSet::none()).is_err());
    assert_eq!(turn_off_imported_auto_post(&mut f.book, f.u, f.c, None, &SecretSet::none()).unwrap(), 1);
    let rules = all_rules(&mut f.book, f.u, f.c).unwrap();
    let imported = rules.iter().find(|r|r.id==id).unwrap();
    assert!(!imported.active && !imported.policy.as_ref().unwrap().auto_post);
    let retained = rules.iter().find(|r|r.id==legacy.id).unwrap();
    assert!(retained.active && retained.policy.is_none());
    let foreign = all_rules(&mut f.book, f.outsider, f.other).unwrap().remove(0);
    assert!(foreign.active && foreign.policy.unwrap().auto_post, "bulk disable cannot change another company");
    f.reopen_clean();
}

#[test]
fn turning_off_retained_policy_preserves_noncanonical_key_order() {
    let mut f = Fx::new();
    f.connected_books();
    let account = f.acc("500");
    let rule = save_rule(&mut f.book, f.u, f.c, &json!({"name":"Retained imported policy","match_text":"ACME","account_id":account}), &SecretSet::none()).unwrap();
    // A valid retained policy need not use the Rust struct's serialization order.
    let data = r#"{"auto_post":true,"all":true,"conditions":[{"value":"ACME","code":1}],"scope":"ALL","direction":"OUT","kind":"EXPENSE","payee_table":"","payee_id":""}"#;
    f.book.write(|w| w.execute("INSERT INTO bankfeeds_rule_policy(rule_id,company_id,source_key,source_hash,source_row,data) VALUES(?1,?2,?3,?4,2,?5)",params![rule.id,f.c.to_db(),"c".repeat(64),"d".repeat(64),data])).unwrap();
    assert_eq!(turn_off_imported_auto_post(&mut f.book, f.u, f.c, Some(&rule.id), &SecretSet::none()).unwrap(), 1);
    let stored:String=f.book.connection().query_row("SELECT data FROM bankfeeds_rule_policy WHERE rule_id=?1",[rule.id],|r|r.get(0)).unwrap();
    assert_eq!(stored,data.replacen("\"auto_post\":true","\"auto_post\":false",1));
    f.reopen_clean();
}

/// Populated identity matrix for the 0191 tables (registered in fb-core/tests/identity_permanence.rs):
/// an imported rule's policy and a queued automatic posting cannot move to another key, by UPDATE or
/// UPDATE OR REPLACE. bankfeeds_rule_review is not permanent by design: it is a per-transaction note the
/// service rewrites (ON CONFLICT DO UPDATE) and deletes when the item is decided, so moving its key is
/// the same as the delete-and-insert the product does; only its presence is checked here.
#[test]
fn imported_policy_and_queue_identities_are_permanent() {
    let mut f = Fx::new();
    f.connected_books();
    import(&mut f, true, None).unwrap();
    // An existing entry holds the first download for review; a second download stays queued.
    let held = download(&mut f);
    f.journal("2026-11-05", "500", "100", "59.00");
    assert!(!auto_post(&mut f.book, f.u, f.c, &held.id, &SecretSet::none()).unwrap());
    let pull = StagedPull { accounts: vec![acct("ACT-checking", "Business Checking", vec![tx("AUTO-2", "2026-11-06", -6100, "ACME INTERNET")])], ..StagedPull::default() };
    store_pull_for_desktop(&mut f.book, f.u, f.c, &f.conn, &pull, NOW, false, None, &SecretSet::none(), &[]).unwrap();
    for table in ["bankfeeds_rule_policy", "bankfeeds_auto_queue", "bankfeeds_rule_review"] {
        assert_eq!(f.count(&format!("SELECT count(*) FROM {table}")), 1, "{table}");
    }
    let tables = ["bankfeeds_rule_policy", "bankfeeds_auto_queue"];
    let accepted = fb_db::test_support::primary_key_changes_accepted(f.book.connection(), &tables);
    assert!(accepted.is_empty(), "accepted primary-key changes: {accepted:#?}");
    f.reopen_clean();
}
