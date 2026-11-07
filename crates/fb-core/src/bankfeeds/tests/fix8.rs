//! FIX8: masking at the persistence boundary (`bankfeeds/write.rs`) and two-character HTML references.
use super::*;

fn fix8_texts(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            out.push(s.clone());
            if let Ok(v) = serde_json::from_str::<Value>(s) {
                fix8_texts(&v, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|v| fix8_texts(v, out)),
        Value::Object(o) => o.iter().for_each(|(k, v)| {
            out.push(k.clone());
            fix8_texts(v, out)
        }),
        _ => {}
    }
}
/// Every text of every row of the book, JSON texts opened.
fn fix8_stored(f: &Fx) -> Vec<String> {
    let mut out = vec![];
    for (_, rows) in fb_db::test_support::snapshot(f.book.connection()).unwrap() {
        for v in rows.into_iter().flatten() {
            if let fb_db::rusqlite::types::Value::Text(s) = v {
                fix8_texts(&json!(s), &mut out);
            }
        }
    }
    out
}
fn fix8_squeeze(s: &str) -> String {
    s.chars().filter(|&c| !secrets::is_blank(c)).collect()
}
fn fix8_leaks(texts: &[String], needles: &[&str]) -> Vec<String> {
    let mut out = vec![];
    for t in texts {
        for n in needles {
            if fix8_squeeze(t).contains(&fix8_squeeze(n)) {
                out.push(format!("{n:?} in {t:?}"));
            }
        }
    }
    out
}

/// What every bank-feeds write does to a text (`Writes::text`).
fn fix8_written(set: &SecretSet, value: &str) -> String {
    staging::provider_text(set, value)
}

/// 12,000 vectors from tools/r8_vectors.py, every one confirmed with Python's own decoders. "single":
/// one text spelling the secret, with the two-character references among the spellings. "split": the
/// secret in two or three provider fields, each masked where it enters (`provider_text`), joined by the
/// core's own composers (`feed_label`, the skipped reason, `memo_text`, the sync summary), then masked as
/// every write masks it. The composers' own masking (`joined_names`, `staging::written`) is left out on
/// purpose (the summaries get plain `sanitize`), so every split case tests the write-time mask alone.
/// tools/r8_verify.py decodes the results.
#[test]
fn fix8_independent_decoder_vectors() {
    let vectors: Vec<Value> = serde_json::from_str(include_str!("r8_vectors.json")).unwrap();
    assert!(vectors.len() >= 10_000);
    let mut results = vec![];
    let mut missed = vec![];
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for (i, v) in vectors.iter().enumerate() {
        let secret = v["secret"].as_str().unwrap();
        let set = SecretSet::masking_new([secret]).unwrap();
        // (composed text, the plain join before the composer's `sanitize`)
        let (composed, joined) = if v["kind"] == "single" {
            let t = v["text"].as_str().unwrap().to_owned();
            (t.clone(), t)
        } else {
            let fields: Vec<String> = v["fields"].as_array().unwrap().iter().map(|f| staging::provider_text(&set, f.as_str().unwrap())).collect();
            match v["composer"].as_str().unwrap() {
                "label" => (feed_label(&fields[0], &fields[1]), feed_label(&fields[0], &fields[1])),
                "reason" => {
                    let j = format!("1 skipped ({}: {})", feed_label(&fields[0], &fields[1]), fields[2]);
                    (text::sanitize(&j, 500), j)
                }
                "memo" => (accept::memo_text(&fields[0]), format!("Bank feed: {}", fields[0])),
                _ => {
                    let j = [format!("SimpleFIN: {}", fields[0]), format!("SimpleFIN: {}", fields[1])].join("; ");
                    (text::sanitize(&j, 500), j)
                }
            }
        };
        let out = fix8_written(&set, &composed);
        *counts.entry(format!("{}-{}", v["kind"].as_str().unwrap(), v["composer"].as_str().unwrap_or("text"))).or_default() += 1;
        // the composer's `sanitize` removed part of the spelling (a `<…>` read as a tag): nothing whole is
        // left to mask; tools/r8_verify.py still decodes the result
        if composed != joined && !out.contains(MASK) {
            *counts.entry("composer-altered".into()).or_default() += 1;
        } else if out.contains(secret) || !out.contains(MASK) {
            missed.push(format!("vector {i}: {secret:?} in {composed:?} -> {out:?}"));
        }
        results.push(json!({"composed":composed,"masked":out}));
    }
    if let Ok(path) = std::env::var("FB_R8_FUZZ_OUT") {
        std::fs::write(path, serde_json::to_string(&results).unwrap()).unwrap();
    }
    eprintln!("FIX8 independent-decoder vectors={} by kind={counts:?} missed={}", results.len(), missed.len());
    assert!(missed.is_empty(), "{} vectors not masked; first: {:?}", missed.len(), &missed[..missed.len().min(3)]);
}

/// The source of every bank-feeds module but `write.rs` and the tests: the choke point is the only writer.
fn fix8_sources() -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bankfeeds");
    let mut out = vec![];
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.extension().is_some_and(|e| e == "rs") && name != "write.rs" && name != "tests.rs" {
            out.push((name, std::fs::read_to_string(&path).unwrap()));
        }
    }
    out.sort();
    // Decoder tables moved beside the sealed Book policy; every service module
    // still participates, and every newly added module is scanned automatically.
    for required in ["accept.rs", "credentials.rs", "mod.rs", "rules.rs", "secrets.rs", "staging.rs", "text.rs"] {
        assert!(out.iter().any(|(name, _)| name == required), "missing {required}");
    }
    out
}
/// Count of `needle` in `text` not preceded by one of `allowed` prefixes.
fn fix8_bare(text: &str, needle: &str, allowed: &[&str]) -> Vec<usize> {
    text.match_indices(needle).map(|(i, _)| i).filter(|&i| !allowed.iter().any(|a| text[..i].ends_with(a))).collect()
}

/// Every write path and audit action, enumerated from the source: no SQL write, audit or write transaction
/// outside `write.rs`; every write transaction is `command(book, secrets, |w| …)`; every public write path
/// takes the connection's secrets; every payload handed to another module's service is masked where it is
/// bound. A new write path or writer that bypasses the choke point fails here.
#[test]
fn fix8_every_write_goes_through_the_choke_point() {
    let boundary = include_str!("../mod.rs");
    assert!(boundary.contains("write::retain_secrets(book, secrets)?;"));
    assert!(boundary.contains("book.write(|tx| tx.atomic(|| f(&Writes::new(tx, secrets))))"));
    let write = include_str!("../write.rs");
    assert!(write.contains("book.retain_bankfeed_mask(&secrets.0)"));
    let policy = include_str!("../../../../fb-db/src/credential_mask/mod.rs");
    assert!(policy.contains("self.retain_text_guard(\"bankfeeds\".into(), move |text|"));
    assert!(policy.contains("final_text(&locked.combined, text)"));
    assert!(policy.contains("locked.combined.merge(secrets)"));
    let db = include_str!("../../../../fb-db/src/lib.rs");
    assert!(db.contains("*text = self.guarded_text(text)?;"));
    assert!(db.contains("statement.execute(rusqlite::params_from_iter(values))"));
    assert!(db.contains("text_guards: std::cell::RefCell::new(self.retained_text_guards.values().cloned().collect())"));
    // Writer::execute is shared by the owning modules: audit serializes here, check snapshots and
    // deposit data serialize in their own insert helpers, all under the same scoped guard.
    for (source, serializer) in [
        (include_str!("../../services.rs"), "serde_json::to_string(&detail)"),
        (include_str!("../../checks/support.rs"), "SqlValue::Text(x.to_string())"),
        (include_str!("../../banking/deposits.rs"), "out.to_string()"),
    ] {
        assert!(source.contains(serializer) && (source.contains("tx.execute(") || source.contains("conn.execute(")));
    }
    // These run after the feed command ends. Runtime coverage is in fix10 and R10's complete
    // join corpus; source inventory pins their shared writer and audit/snapshot serializers.
    for (source, functions, actions) in [
        (include_str!("../../checks/lifecycle.rs"), &["pub fn void_check("][..], &["voided"][..]),
        (include_str!("../../checks/mod.rs"), &["pub fn save_check(", "pub fn save_check_tx("][..], &["revised"][..]),
        (include_str!("../../services.rs"), &["pub fn reverse_journal(", "pub fn edit_company("][..], &["journal.reversed", "company.updated"][..]),
    ] {
        for function in functions { assert!(source.contains(function), "lifecycle inventory changed: {function}"); }
        for action in actions { assert!(source.contains(action), "lifecycle audit inventory changed: {action}"); }
        assert!(source.contains("audit(") && (source.contains("book.write(") || source.contains("write(book,")));
    }
    assert!(include_str!("../../checks/mod.rs").contains("format!(\"check.{}\", s(&r, \"action\"))"));
    let mut write_paths = vec![];
    let mut credential_paths = vec![];
    let mut actions = std::collections::BTreeSet::new();
    for (name, src) in fix8_sources() {
        let line = |i: usize| src[..i].lines().count();
        for (needle, allowed) in [("execute(", &["w."][..]), ("execute_batch", &[][..]), ("audit(", &["w."][..]), (".write(", &[][..]), ("Writer<'_>", &[][..])] {
            // mod.rs's `command` is the one write transaction; staging's `active_connection` only reads
            let exempt = |i: usize| {
                (name == "mod.rs" && src[i..].starts_with(".write(|tx| tx.atomic(|| f(&Writes::new(tx, secrets))))"))
                    || (name == "staging.rs" && src[..i].ends_with("fn active_connection(tx: &"))
            };
            let bare: Vec<usize> = fix8_bare(&src, needle, allowed).into_iter().filter(|&i| !exempt(i)).collect();
            assert!(bare.is_empty(), "{name}: `{needle}` outside the choke point at lines {:?}", bare.iter().map(|&i| line(i)).collect::<Vec<_>>());
        }
        for (i, _) in src.match_indices("command(book") {
            assert!(src[i..].starts_with("command(book, secrets, |w| {"), "{name}:{}: a write transaction without the connection's secrets", line(i));
        }
        for (i, _) in src.match_indices("let payload = ") {
            assert!(src[i..].starts_with("let payload = w.value(&json!("), "{name}:{}: a payload for another module not masked where it is bound", line(i));
        }
        for (i, _) in src.match_indices("DraftInput::new(") {
            assert!(src[i..].starts_with("DraftInput::new(&date, &w.text("), "{name}:{}: a journal memo not masked", line(i));
        }
        // public write paths: a `pub fn` whose body opens a write transaction
        let starts: Vec<usize> = src.match_indices("\npub fn ").map(|(i, _)| i + 1).collect();
        for (k, &start) in starts.iter().enumerate() {
            let end = starts.get(k + 1).copied().unwrap_or(src.len());
            let body = &src[start..end];
            if name == "credentials.rs" {
                let fname = body["pub fn ".len()..].split(['(', '<']).next().unwrap();
                credential_paths.push(fname.to_owned());
                if fname == "flag_unreadable_credential" {
                    // Unlocks nothing but the reconnect flag: a Book-issued proof, no secrets, no SecretSet back.
                    assert!(body.contains("unreadable: &fb_db::credential_mask::UnreadableBankfeed") && body.contains("FbResult<String>"));
                    assert!(body.contains("flag(book, actor, co, connection_id, &SecretSet::none(), \"unreadable\")"));
                } else {
                    assert!(body.contains("restore: &fb_db::credential_mask::BankfeedRestore") && body.contains("FbResult<SecretSet>"));
                }
            }
            if body.contains("command(book") && name != "credentials.rs" {
                let fname = body["pub fn ".len()..].split('(').next().unwrap().to_owned();
                let sig = &body[..body.find('{').unwrap()];
                assert!(sig.contains("secrets: &SecretSet"), "{name}: {fname} writes without the connection's secrets");
                write_paths.push(fname);
            }
        }
        let mut rest = src.as_str();
        while let Some(i) = rest.find("\"bankfeed.") {
            let tail = &rest[i + 1..];
            let action: String = tail.chars().take_while(|c| *c != '"').collect();
            if action.len() > "bankfeed.".len() {
                actions.insert(action);
            }
            rest = &tail[1..];
        }
    }
    write_paths.sort();
    let expected = [
        "accept", "connect", "forget", "ignore", "map_account", "move_rule", "record_sync_failure", "return_to_review", "save_rule", "set_rule_active", "stage_bank_transactions",
        "store_pull",
    ];
    assert_eq!(write_paths, expected, "the write paths changed: drive the new one in fix8_split_secrets_every_write_path_and_audit_action and list it here");
    assert_eq!(credential_paths, ["restore_connection_credentials", "flag_unreadable_credential"], "credential load paths changed: add public-admission, quarantine and later-writer regressions");
    let mut expected_actions = FIX8_ACTIONS.iter().map(|s| (*s).to_owned()).collect::<std::collections::BTreeSet<_>>();
    expected_actions.insert("bankfeed.reconnect_required".into());
    assert_eq!(actions, expected_actions, "the audit actions changed: drive the new one and list it");
    let credentials = include_str!("../credentials.rs");
    assert!(credentials.contains("pub fn restore_connection_credentials<"));
    assert!(credentials.contains("command(book, secrets, |w| {"));
    assert!(credentials.contains("SecretSet::from_access_url(access_url, &extra)"));
    // No public unchecked constructor may let a plugin admit a legacy credential.
    let secrets = include_str!("../secrets.rs");
    assert!(!secrets.contains("pub fn masking_"));
}
const FIX8_ACTIONS: [&str; 11] = [
    "bankfeed.accepted",
    "bankfeed.connected",
    "bankfeed.forgotten",
    "bankfeed.ignored",
    "bankfeed.mapped",
    "bankfeed.returned",
    "bankfeed.rule_created",
    "bankfeed.rule_edited",
    "bankfeed.rule_moved",
    "bankfeed.staged",
    "bankfeed.synced",
];

/// Review 8's finding, for every write path: secrets split across provider fields that the core's
/// composers join (`org · name`, `name: reason`, `Bank feed: description`, `…; SimpleFIN: error`) and a
/// secret typed into a rule never reach a stored row, an audit (of bank feeds or of the check, deposit,
/// transfer and journal services) or a returned value. Every public write path and every bank-feeds audit
/// action listed in `fix8_every_write_goes_through_the_choke_point` runs here. After `forget` the host
/// holds no credential any more: the joined names were checked where they entered, so a later mapping
/// still writes no secret.
#[test]
fn fix8_split_secrets_every_write_path_and_audit_action() {
    let label = "R8orgsplit · R8namesplit";
    let reason = "R8saving-name: R8why-skipped";
    let memo = "Bank feed: R8memo-split-desc";
    let summary = "R8err-first; SimpleFIN: R8err-second";
    let password = "R8password-typed-into-rule";
    let known = SecretSet::masking_from_credentials("R8username-long", password, [label, reason, memo, summary]).unwrap();
    let needles = [label, reason, memo, summary, password];
    let mut f = Fx::new();
    f.secrets = known.clone();
    let mut returned: Vec<String> = vec![];
    let mut pull = demo();
    pull.accounts[0].org_name = "R8orgsplit".into();
    pull.accounts[0].name = "R8namesplit".into();
    pull.accounts[1].name = "R8saving-name".into();
    pull.accounts[1].skipped = vec!["R8why-skipped".into()];
    pull.errors = vec!["R8err-first".into(), "R8err-second".into()];
    for t in pull.accounts[0].transactions.iter_mut().filter(|t| ["TX-1001", "TX-1003", "TX-1005", "TX-1006"].contains(&t.remote_id.as_str())) {
        t.description = "R8memo-split-desc".into();
    }
    let second = connect(&mut f.book, f.u, f.c, &json!("R8orgsplit"), "R8namesplit.test", &known).unwrap();
    returned.push(second.to_string());
    returned.push(record_sync_failure(&mut f.book, f.u, f.c, &f.conn.clone(), NOW, "R8err-first; SimpleFIN: R8err-second", false, &known).map(|_| String::new()).unwrap());
    returned.push(store_pull(&mut f.book, f.u, f.c, &f.conn.clone(), &pull, NOW, false, None, &known).unwrap().to_string());
    // an error that quotes the label, before the account is mapped
    returned.push(f.accept("TX-1006", json!({"kind":"JOURNAL","account_id":f.acc("410")})).unwrap_err().message());
    for (remote, code) in [("ACT-checking", "100"), ("ACT-savings", "101"), ("ACT-card", "220")] {
        returned.push(f.map(remote, Some(code)).unwrap().to_string());
    }
    returned.push(format!("{:?}", stage_bank_transactions(&mut f.book, f.u, f.c, &f.conn.clone(), &[("ACT-checking".into(), tx("TX-1100", "2026-10-21", -700, "R8memo-split-desc"))], &known).unwrap()));
    let receipt = f.undeposited("2500.00", "2026-10-04");
    let j = f.journal("2026-10-30", "101", "410", "3.12");
    let line: String = f.book.connection().query_row("SELECT id FROM ledger_journalline WHERE journal_id=?1 AND debit=312", [j], |r| r.get(0)).unwrap();
    for (remote, input) in [
        ("TX-1006", json!({"kind":"JOURNAL","account_id":f.acc("410")})),
        ("TX-1001", json!({"kind":"CHECK","account_id":f.acc("500")})),
        ("TX-1003", json!({"kind":"DEPOSIT","receipt_ids":[receipt]})),
        ("TX-1005", json!({"kind":"TRANSFER","transfer_account_id":f.acc("101")})),
        ("TX-2002", json!({"kind":"MATCH","journal_line_id":line})),
    ] {
        returned.push(format!("{:?}", f.accept(remote, input).unwrap_or_else(|e| panic!("{remote}: {e}"))));
    }
    let item = f.item("TX-1100");
    returned.push(format!("{:?}", ignore(&mut f.book, f.u, f.c, &item.id, &known).unwrap()));
    returned.push(format!("{:?}", return_to_review(&mut f.book, f.u, f.c, &item.id, &known).unwrap()));
    // a secret typed into a rule: nothing masks it where it enters, the write does
    let office = f.acc("500");
    let rule = save_rule(&mut f.book, f.u, f.c, &json!({"name":password,"match_text":format!("x {password}"),"payee_name":password,"account_id":office}), &known).unwrap();
    let other = save_rule(&mut f.book, f.u, f.c, &json!({"name":"Other","match_text":"other","account_id":office}), &known).unwrap();
    returned.push(format!("{rule:?} {other:?}"));
    returned.push(format!("{:?}", save_rule(&mut f.book, f.u, f.c, &json!({"rule_id":rule.id,"name":password,"match_text":"edited","account_id":office}), &known).unwrap()));
    returned.push(format!("{:?}", move_rule(&mut f.book, f.u, f.c, &other.id, &json!("up"), &known).unwrap()));
    returned.push(format!("{:?}", set_rule_active(&mut f.book, f.u, f.c, &rule.id, &json!(false), &known).unwrap()));
    returned.push(forget(&mut f.book, f.u, f.c, second["id"].as_str().unwrap(), &known).unwrap().to_string());
    // the credential is gone with the forgotten connection: the host has no set to pass
    f.secrets = SecretSet::none();
    returned.push(f.map("ACT-checking", None).unwrap().to_string());
    returned.push(f.map("ACT-checking", Some("100")).unwrap().to_string());

    let mut st = f.book.connection().prepare("SELECT DISTINCT action FROM ledger_auditevent ORDER BY action").unwrap();
    let seen: Vec<String> = st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    drop(st);
    for action in FIX8_ACTIONS {
        assert!(seen.iter().any(|a| a == action), "{action} was not driven; seen {seen:?}");
    }
    for other in ["check.", "deposit", "transfer", "journal.posted"] {
        assert!(seen.iter().any(|a| a.starts_with(other)), "{other} was not driven; seen {seen:?}");
    }
    let stored = fix8_stored(&f);
    let leaks = fix8_leaks(&stored, &needles);
    assert!(leaks.is_empty(), "stored: {:?}", &leaks[..leaks.len().min(5)]);
    let leaks = fix8_leaks(&returned, &needles);
    assert!(leaks.is_empty(), "returned: {:?}", &leaks[..leaks.len().min(5)]);
    // the composers did run: the masks are where the joined fields were
    let memos: Vec<String> = {
        let mut st = f.book.connection().prepare("SELECT memo FROM ledger_journal ORDER BY memo").unwrap();
        st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    // journal, check, deposit and transfer memos: `Bank feed: …` joined, then masked whole
    assert!(memos.iter().filter(|m| m.ends_with("***")).count() >= 4, "memo: {memos:?}");
    assert!(stored.iter().any(|t| t.contains("skipped (Demo Bank · ***)")), "reason");
    eprintln!("FIX8 write paths driven with split secrets; audit actions seen={}", seen.len());
    f.reopen_clean();
    r5_finish(&mut f);
}

/// The choke point masks after composition even where entry masking can't see the secret: a label joined
/// from two names neither of which spells it, written through `Writes`.
#[test]
fn fix8_writes_masks_composed_text_and_every_parameter() {
    let mut f = Fx::new();
    let known = SecretSet::masking_new(["R8orgsplit · R8namesplit"]).unwrap();
    let composed = feed_label("R8orgsplit", "R8namesplit");
    f.sync(&demo());
    let feed = f.feed("ACT-checking");
    let (stored, detail): (String, String) = f
        .book
        .write(|tx| {
            let w = write::Writes::new(tx, &known);
            w.execute("UPDATE bankfeeds_account SET name=?2 WHERE id=?1", fb_db::rusqlite::params![feed, composed])?;
            let user = services::access(tx, f.u, f.c)?;
            w.audit(&user, f.c, "bankfeed.mapped", format!("bankfeedaccount:{composed}"), json!({"feed_account":composed,"nested":[{"x":composed}],"n":1}))?;
            let stored: String = tx.query_row("SELECT name FROM bankfeeds_account WHERE id=?1", [&feed], |r| r.get(0))?;
            let detail: String = tx.query_row("SELECT target||detail FROM ledger_auditevent WHERE action='bankfeed.mapped'", [], |r| r.get(0))?;
            Ok::<_, FbError>((stored, detail))
        })
        .unwrap();
    assert_eq!(stored, "***");
    assert!(!detail.contains("R8orgsplit · R8namesplit") && detail.contains("bankfeedaccount:***") && detail.contains("\"n\":1"), "{detail}");
    r5_finish(&mut f);
}
