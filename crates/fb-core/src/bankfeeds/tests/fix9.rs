//! Final serialized TEXT regressions, independent-decoder vectors, and writer bypass controls.
use super::*;

const PATHS: [&str; 12] = ["accept", "connect", "forget", "ignore", "map_account", "move_rule", "record_sync_failure", "return_to_review", "save_rule", "set_rule_active", "stage_bank_transactions", "store_pull"];
const ACTIONS: [&str; 11] = ["bankfeed.accepted", "bankfeed.connected", "bankfeed.forgotten", "bankfeed.ignored", "bankfeed.mapped", "bankfeed.returned", "bankfeed.rule_created", "bankfeed.rule_edited", "bankfeed.rule_moved", "bankfeed.staged", "bankfeed.synced"];

fn output(name: &str, data: &Value) {
    if let Ok(dir) = std::env::var("FB_R9_FUZZ_OUT") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(std::path::Path::new(&dir).join(name), serde_json::to_vec(data).unwrap()).unwrap();
    }
}

fn checkpoint(path: &str, seen: &mut std::collections::BTreeSet<String>, previous: &mut usize) {
    let count = write::FINAL_TEXTS.with_borrow(|log| log.as_ref().unwrap().len());
    assert!(count > *previous, "{path} bypassed the final SQL TEXT guard");
    *previous = count;
    seen.insert(path.to_owned());
}

#[test]
fn fix9_final_serialized_vectors() {
    let mut vectors: Vec<Value> = serde_json::from_str(include_str!("r8_vectors.json")).unwrap();
    let more: Vec<Value> = if let Ok(path) = std::env::var("FB_R9_VECTORS") {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    } else { serde_json::from_str(include_str!("r9_vectors.json")).unwrap() };
    vectors.extend(more);
    let mut results = vec![];
    let mut missed = vec![];
    for (i, vector) in vectors.iter().enumerate() {
        let secret = vector["secret"].as_str().unwrap();
        let known = SecretSet::masking_new([secret]).unwrap();
        // No ingress masking here: exercise the final serialized boundary in isolation, including
        // the R8 split-field composers, and nested string values with typed siblings.
        let data = json!({"data":vector["text"],"nested":[vector["text"],{"keep":true}],"n":23,"none":null});
        let before = data.to_string();
        let masked = write::final_text(&known, &before).unwrap();
        let after: Value = serde_json::from_str(&masked).unwrap();
        assert_eq!(after["n"], 23);
        assert_eq!(after["nested"][1]["keep"], true);
        assert!(after["none"].is_null());
        if masked.contains(secret) || masked == before { missed.push(i); }
        results.push(json!({"secret":secret,"layers":vector["layers"],"before":before,"masked":masked}));
    }
    // Passwords made by the serializer itself: quotes, backslashes, and a cross-field JSON join.
    for (secret, data) in [
        (r#"R9alpha\"omega-secret"#, json!({"data":r#"R9alpha"omega-secret"#})),
        (r#"R9alpha\\omega-secret"#, json!({"data":r#"R9alpha\omega-secret"#})),
        (r#"R9alpha","remote_id":"R9omega"#, json!({"feed_account":"R9alpha","remote_id":"R9omega"})),
    ] {
        let before = data.to_string();
        assert!(before.contains(secret));
        let masked = write::final_text(&SecretSet::masking_new([secret]).unwrap(), &before).unwrap();
        if masked.contains(secret) { missed.push(results.len()); }
        results.push(json!({"secret":secret,"layers":[],"before":before,"masked":masked}));
    }
    output("vectors.json", &json!(results));
    eprintln!("FIX9 final serialized vectors={} (standard decoder output enabled by FB_R9_FUZZ_OUT)", results.len());
    assert!(missed.is_empty(), "final mask missed {} vectors; first {:?}", missed.len(), missed.first());
}

#[test]
fn fix9_guard_covers_downstream_and_refuses_schema_changes_atomically() {
    let mut f = Fx::new();
    let secret = r#"R9alpha\"omega-secret"#;
    let known = SecretSet::masking_new([secret]).unwrap();
    // services::audit does its own serialization. No Writes::value/text ingress can help here.
    command(&mut f.book, &known, |w| {
        let user = services::access(w.tx, f.u, f.c)?;
        services::audit(w.tx, &user, f.c, "fix9.downstream", "fixture".into(), json!({"data":r#"R9alpha"omega-secret"#,"nested":[true,32,null]}))
    }).unwrap();
    let stored: String = f.book.connection().query_row("SELECT detail FROM ledger_auditevent WHERE action='fix9.downstream'", [], |r| r.get(0)).unwrap();
    assert!(!stored.contains(secret));
    assert_eq!(serde_json::from_str::<Value>(&stored).unwrap()["nested"], json!([true,32,null]));

    for (secret, data) in [
        ("a-secret-key-name", json!({"a-secret-key-name":27})),
        ("123456789012345678", json!({"amount":123456789012345678i64})),
    ] {
        let known = SecretSet::masking_new([secret]).unwrap();
        let before = fb_db::test_support::snapshot(f.book.connection()).unwrap();
        let error = command(&mut f.book, &known, |w| {
            let user = services::access(w.tx, f.u, f.c)?;
            services::audit(w.tx, &user, f.c, "fix9.before_refusal", "fixture".into(), json!({}))?;
            services::audit(w.tx, &user, f.c, "fix9.refused", "fixture".into(), data.clone())
        }).unwrap_err();
        assert!(error.message().contains("preserving its JSON schema"), "{error}");
        assert!(!error.message().contains(secret));
        assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(), before);
    }
    r5_finish(&mut f);
}

#[test]
fn fix9_writer_guard_is_scoped_and_cannot_be_skipped() {
    let mut f = Fx::new();
    f.book.write(|tx| {
        tx.with_text_guard(|s| Ok(s.replace("secret-value", "***")), || {
            for result in [
                tx.prepare("UPDATE bankfeeds_rule SET name='secret-value'").map(|_| ()),
                fb_db::Query::prepare(tx, "UPDATE bankfeeds_rule SET name='secret-value'").map(|_| ()),
                tx.query_row("UPDATE bankfeeds_rule SET name='secret-value' RETURNING name", [], |r| r.get::<_, String>(0)).map(|_| ()),
                tx.execute_batch("UPDATE bankfeeds_rule SET name='secret-value'"),
                tx.execute("UPDATE bankfeeds_rule SET name='secret-value'", []).map(|_| ()),
            ] { assert!(result.unwrap_err().to_string().contains("guarded write")); }
            // Numbered holes, repeated and named bindings, and non-text values keep their types.
            tx.execute("UPDATE bankfeeds_rule SET name=?3 WHERE name=?3 AND match_text=?1 AND payee_name IS ?2", fb_db::rusqlite::params!["secret-value", Option::<String>::None, "secret-value"])?;
            tx.execute("UPDATE bankfeeds_rule SET name=:name WHERE amount_min=:n AND match_text=:b", fb_db::rusqlite::named_params!{":name":"secret-value",":n":3.5,":b":vec![1u8,2,3]})?;
            Ok::<_, FbError>(())
        })?;
        // Guard removed on normal return, refusal, and panic, without altering another command.
        assert!(tx.prepare("UPDATE bankfeeds_rule SET name='secret-value'").is_ok());
        let _ = tx.with_text_guard(|s| Ok(s.into()), || Err::<(), _>(FbError::validation("fixture refusal")));
        assert!(tx.prepare("UPDATE bankfeeds_rule SET name='secret-value'").is_ok());
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _: FbResult<()> = tx.with_text_guard(|s| Ok(s.into()), || panic!("fixture panic"));
        }));
        assert!(panic.is_err());
        assert!(tx.prepare("UPDATE bankfeeds_rule SET name='secret-value'").is_ok());
        Ok::<_, FbError>(())
    }).unwrap();
    r5_finish(&mut f);
}

/// Every write path, all 11 bank-feed audit actions, and the check/deposit/transfer/journal writers.
/// Capture the actual final SQL TEXT parameters, not an earlier Value or a reconstructed snapshot.
#[test]
fn fix9_final_text_every_write_path_and_audit_action_fuzz() {
    let r9: Vec<Value> = serde_json::from_str(include_str!("r9_vectors.json")).unwrap();
    let r8: Vec<Value> = serde_json::from_str(include_str!("r8_vectors.json")).unwrap();
    let mut scenarios = vec![];
    let mut missed = vec![];
    for i in 0..128 {
        let vector = if i % 2 == 0 { &r9[i / 2] } else { &r8[i / 2] };
        let (secret, text, layers) = match i % 8 {
            0 => (format!(r#"R9alpha{i}\"omega-secret"#), format!("R9alpha{i}\"omega-secret"), json!([])),
            2 => (format!(r#"R9alpha{i}\\omega-secret"#), format!("R9alpha{i}\\omega-secret"), json!([])),
            _ => (vector["secret"].as_str().unwrap().into(), vector["text"].as_str().unwrap().into(), vector["layers"].clone()),
        };
        // Every scenario also has a password that exists ONLY after JSON joins two fields. Entry
        // masking and scalar serialization preflight cannot see it: the final writer is essential.
        let join_secret = format!(r#"R9join{i}","remote_id":"ACT-checking"#);
        let known = SecretSet::masking_new([secret.as_str(), join_secret.as_str()]).unwrap();
        let mut f = Fx::new();
        f.secrets = known.clone();
        write::FINAL_TEXTS.with_borrow_mut(|log| *log = Some(vec![]));
        let mut driven = std::collections::BTreeSet::new();
        let mut previous = 0;
        let second = connect(&mut f.book, f.u, f.c, &json!("Fuzz connection"), "fixture.test", &known).unwrap();
        checkpoint("connect", &mut driven, &mut previous);
        record_sync_failure(&mut f.book, f.u, f.c, &f.conn.clone(), NOW, &text, false, &known).unwrap();
        checkpoint("record_sync_failure", &mut driven, &mut previous);
        let mut pull = demo();
        for a in &mut pull.accounts {
            a.name = text.clone(); a.org_name = text.clone(); a.skipped = vec![text.clone()];
            for t in &mut a.transactions { t.description = text.clone(); }
        }
        pull.accounts[0].name = format!("R9join{i}");
        pull.accounts[0].org_name.clear();
        pull.errors = vec![text.clone()];
        store_pull(&mut f.book, f.u, f.c, &f.conn.clone(), &pull, NOW, false, None, &known).unwrap();
        checkpoint("store_pull", &mut driven, &mut previous);
        for (remote, code) in [("ACT-checking", "100"), ("ACT-savings", "101"), ("ACT-card", "220")] { f.map(remote, Some(code)).unwrap(); }
        checkpoint("map_account", &mut driven, &mut previous);
        stage_bank_transactions(&mut f.book, f.u, f.c, &f.conn.clone(), &[("ACT-checking".into(), tx("fuzz-staged", "2026-10-20", -100, &text))], &known).unwrap();
        checkpoint("stage_bank_transactions", &mut driven, &mut previous);
        let staged = f.item("fuzz-staged");
        ignore(&mut f.book, f.u, f.c, &staged.id, &known).unwrap();
        checkpoint("ignore", &mut driven, &mut previous);
        return_to_review(&mut f.book, f.u, f.c, &staged.id, &known).unwrap();
        checkpoint("return_to_review", &mut driven, &mut previous);
        let receipt = f.undeposited("2500.00", "2026-10-04");
        for (remote, payload) in [
            ("TX-1006", json!({"kind":"JOURNAL","account_id":f.acc("410")})),
            ("TX-1001", json!({"kind":"CHECK","account_id":f.acc("500")})),
            ("TX-1003", json!({"kind":"DEPOSIT","receipt_ids":[receipt]})),
            ("TX-1005", json!({"kind":"TRANSFER","transfer_account_id":f.acc("101")})),
        ] { f.accept(remote, payload).unwrap(); }
        checkpoint("accept", &mut driven, &mut previous);
        let office = f.acc("500");
        let rule_text = if fb_db::check_numbers::free_text_valid(&secret) { secret.clone() } else {
            let before = fb_db::test_support::snapshot(f.book.connection()).unwrap();
            let refused = save_rule(&mut f.book, f.u, f.c, &json!({"name":secret,"match_text":secret,"payee_name":secret,"account_id":office}), &known).unwrap_err();
            assert_eq!(refused.message(), "Control and formatting characters are not allowed.");
            assert_eq!(before, fb_db::test_support::snapshot(f.book.connection()).unwrap());
            // Keep testing every writer with the same secret in a permitted
            // spelling: escape only its forbidden characters, rather than
            // removing the secret from this path's security coverage.
            let encoded = secret.chars().map(|c| if fb_db::check_numbers::free_text_valid(&c.to_string()) {
                c.to_string()
            } else { c.encode_utf16(&mut [0;2]).iter().map(|u| format!("\\u{u:04x}")).collect() }).collect::<String>();
            assert_eq!(known.mask(&encoded), known.mask(&secret), "the escaped spelling must carry the same masked secret");
            encoded
        };
        let rule = save_rule(&mut f.book, f.u, f.c, &json!({"name":rule_text,"match_text":rule_text,"payee_name":rule_text,"account_id":office}), &known).unwrap();
        let other = save_rule(&mut f.book, f.u, f.c, &json!({"name":"Other","match_text":"other","account_id":office}), &known).unwrap();
        save_rule(&mut f.book, f.u, f.c, &json!({"rule_id":rule.id,"name":rule_text,"match_text":rule_text,"account_id":office}), &known).unwrap();
        checkpoint("save_rule", &mut driven, &mut previous);
        move_rule(&mut f.book, f.u, f.c, &other.id, &json!("up"), &known).unwrap();
        checkpoint("move_rule", &mut driven, &mut previous);
        set_rule_active(&mut f.book, f.u, f.c, &rule.id, &json!(false), &known).unwrap();
        checkpoint("set_rule_active", &mut driven, &mut previous);
        forget(&mut f.book, f.u, f.c, second["id"].as_str().unwrap(), &known).unwrap();
        checkpoint("forget", &mut driven, &mut previous);
        assert_eq!(driven.iter().map(String::as_str).collect::<Vec<_>>(), PATHS);
        let final_texts = write::FINAL_TEXTS.with_borrow_mut(|log| log.take().unwrap());
        let actions: Vec<String> = {
            let mut st = f.book.connection().prepare("SELECT DISTINCT action FROM ledger_auditevent ORDER BY action").unwrap();
            st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
        };
        for action in ACTIONS { assert!(actions.iter().any(|a| a == action), "missing {action}"); }
        for action in ["check.created", "deposit.recorded", "transfer.recorded", "journal.posted"] { assert!(actions.iter().any(|a| a == action), "missing downstream {action}"); }
        let stored = r9::snapshot_texts(&f);
        if stored.iter().any(|(_, text)| text.contains(&secret) || text.contains(&join_secret)) { missed.push(i); }
        scenarios.push(json!({"secret":secret,"join_secret":join_secret,"layers":layers,"paths":driven,"actions":actions,"final_texts":final_texts,"stored":stored}));
        r5_finish(&mut f);
    }
    output("persisted.json", &json!(scenarios));
    eprintln!("FIX9 final-text fuzz: {} scenarios x {} write paths; all {} bank-feed audit actions and downstream snapshots", scenarios.len(), PATHS.len(), ACTIONS.len());
    assert!(missed.is_empty(), "final mask leaked in scenarios {missed:?}");
}
