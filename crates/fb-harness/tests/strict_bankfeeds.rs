//! Identical-corruption self-tests: both "sides" carry the same corrupted bank-feed dump, so only the
//! strict shape and the audit-evidence checks can catch it. Every case must fail the comparer.
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
static NEXT: AtomicUsize = AtomicUsize::new(0);

fn dump(name: &str) -> Value {
    let scenario: Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{}/scenarios/{name}.json", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap();
    let path = std::env::temp_dir().join(format!("strict-bankfeeds-{}-{name}-{}.fbbook", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_file(&path);
    let mut runner = fb_harness::Runner::new(&scenario, &path);
    runner.run();
    let out = runner.dump();
    drop(runner);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    out
}
fn leaves(v: &Value, p: String, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => m.iter().for_each(|(k, v)| leaves(v, format!("{p}/{k}"), out)),
        Value::Array(a) => a.iter().enumerate().for_each(|(i, v)| leaves(v, format!("{p}/{i}"), out)),
        _ => out.push(p),
    }
}
fn rejects(bad: &Value, why: &str) {
    let report = fb_harness::compare(bad, bad);
    assert!(!report.ok(), "identical corruption accepted: {why}");
}
fn find<'a>(v: &'a mut Value, table: &str, f: impl Fn(&Value) -> bool) -> &'a mut Value {
    v["bankfeeds"][table].as_array_mut().unwrap().iter_mut().find(|r| f(r)).unwrap_or_else(|| panic!("no {table} row"))
}

#[test]
fn bankfeeds_shape_rejects_every_identical_type_corruption_and_missing_table() {
    let mut tested = 0;
    for name in ["bankfeeds-accept", "bankfeeds-pulls"] {
        let good = dump(name);
        let r = fb_harness::compare(&good, &good);
        assert!(r.ok(), "{name}: {:?}", r.failures);
        let mut pointers = vec![];
        for table in ["connections", "accounts", "transactions", "rules"] {
            leaves(&good["bankfeeds"][table], format!("/bankfeeds/{table}"), &mut pointers);
        }
        for pointer in pointers {
            let mut bad = good.clone();
            *bad.pointer_mut(&pointer).unwrap() = json!([]);
            rejects(&bad, &pointer);
            tested += 1;
        }
        for table in ["connections", "accounts", "transactions", "rules", "results"] {
            let mut bad = good.clone();
            bad["bankfeeds"].as_object_mut().unwrap().remove(table);
            rejects(&bad, &format!("missing {table}"));
            let mut bad = good.clone();
            bad["bankfeeds"][table] = json!(null);
            rejects(&bad, &format!("null {table}"));
        }
        let mut bad = good.clone();
        bad["bankfeeds"]["extra"] = json!([]);
        rejects(&bad, "unexpected table");
    }
    assert!(tested > 300, "only {tested} leaves");
}

#[test]
fn bankfeeds_completeness_rejects_identical_semantic_corruptions() {
    let good = dump("bankfeeds-accept");
    let rules = dump("bankfeeds-rules");
    assert!(fb_harness::compare(&rules, &rules).ok());
    type Mutation = (&'static str, Box<dyn Fn(&mut Value)>);
    let cases: Vec<Mutation> = vec![
        ("drop a downloaded row", Box::new(|v| {
            v["bankfeeds"]["transactions"].as_array_mut().unwrap().remove(0);
        })),
        ("duplicate a downloaded row", Box::new(|v| {
            let row = v["bankfeeds"]["transactions"][0].clone();
            v["bankfeeds"]["transactions"].as_array_mut().unwrap().push(row);
        })),
        ("unnamed row", Box::new(|v| v["bankfeeds"]["transactions"][0]["id"] = json!("?"))),
        ("status without its audit", Box::new(|v| {
            let t = find(v, "transactions", |t| t["status"] == "NEW" && t["return_count"] == 0);
            t["status"] = json!("IGNORED");
            t["decided"] = json!(true);
            t["decided_by"] = json!("feeds");
        })),
        ("accepted row turned back to review", Box::new(|v| {
            let t = find(v, "transactions", |t| t["status"] == "ACCEPTED");
            for (k, x) in [("status", json!("NEW")), ("accepted_as", json!("")), ("journal_id", json!(null)), ("journal_line_id", json!(null)),
                           ("reference", json!("")), ("reference_url", json!("")), ("decided", json!(false)), ("decided_by", json!(null))] {
                t[k] = x;
            }
        })),
        ("return count", Box::new(|v| {
            let t = find(v, "transactions", |t| t["return_count"] == 1);
            t["return_count"] = json!(2);
        })),
        ("decided amount", Box::new(|v| {
            let t = find(v, "transactions", |t| t["status"] == "ACCEPTED");
            t["amount"] = json!(t["amount"].as_i64().unwrap() * 2);
        })),
        ("decision reference", Box::new(|v| find(v, "transactions", |t| t["status"] == "ACCEPTED")["reference"] = json!("JE 999"))),
        ("decision kind", Box::new(|v| find(v, "transactions", |t| t["accepted_as"] == "JOURNAL")["accepted_as"] = json!("TRANSFER"))),
        ("decision line moved", Box::new(|v| {
            let t = find(v, "transactions", |t| t["accepted_as"] == "JOURNAL");
            let line = t["journal_line_id"].as_str().unwrap().to_owned();
            let (j, p) = line.rsplit_once(':').unwrap();
            t["journal_line_id"] = json!(format!("{j}:{}", if p == "1" { 2 } else { 1 }));
        })),
        ("decision journal", Box::new(|v| {
            let other = v["journals"][0]["ref"].clone();
            find(v, "transactions", |t| t["accepted_as"] == "JOURNAL" && t["journal_id"] != other)["journal_id"] = other.clone();
        })),
        ("decided date", Box::new(|v| find(v, "transactions", |t| t["status"] == "ACCEPTED")["posted_on"] = json!("2026-02-30"))),
        ("sync count", Box::new(|v| v["bankfeeds"]["connections"][0]["sync_count"] = json!(9))),
        ("last summary", Box::new(|v| find(v, "connections", |c| c["sync_count"] != 0)["last_sync_summary"] = json!("0 accounts"))),
        ("forgotten", Box::new(|v| {
            let c = &mut v["bankfeeds"]["connections"][0];
            c["active"] = json!(false);
            c["forgotten"] = json!(true);
        })),
        ("mapping", Box::new(|v| find(v, "accounts", |f| f["ledger_account_id"] == "bank")["ledger_account_id"] = json!("savings"))),
        ("mapping cleared", Box::new(|v| {
            let f = find(v, "accounts", |f| f["ledger_account_id"] == "card");
            f["ledger_account_id"] = json!(null);
            f["mapped"] = json!(false);
        })),
        ("feed key", Box::new(|v| v["bankfeeds"]["accounts"][0]["remote_id"] = json!("renamed"))),
        ("connected audit dropped", Box::new(|v| {
            for events in v["audit"].as_object_mut().unwrap().values_mut() {
                let list = events.as_array_mut().unwrap();
                if let Some(i) = list.iter().position(|e| e["action"] == "bankfeed.connected") {
                    list.remove(i);
                    return;
                }
            }
        })),
        ("accepted audit dropped", Box::new(|v| {
            for events in v["audit"].as_object_mut().unwrap().values_mut() {
                let list = events.as_array_mut().unwrap();
                if let Some(i) = list.iter().rposition(|e| e["action"] == "bankfeed.accepted") {
                    list.remove(i);
                    return;
                }
            }
        })),
        ("connection row dropped", Box::new(|v| {
            v["bankfeeds"]["connections"].as_array_mut().unwrap().remove(0);
        })),
    ];
    for (why, mutate) in &cases {
        let mut bad = good.clone();
        mutate(&mut bad);
        assert_ne!(bad, good, "{why} changed nothing");
        rejects(&bad, why);
    }
    let rule_cases: Vec<Mutation> = vec![
        ("rule name", Box::new(|v| v["bankfeeds"]["rules"][0]["name"] = json!("Renamed"))),
        ("rule minimum", Box::new(|v| find(v, "rules", |r| r["amount_min"].is_number())["amount_min"] = json!(1))),
        ("rule active", Box::new(|v| {
            let r = find(v, "rules", |r| r["active"] == true);
            r["active"] = json!(false);
        })),
        ("rule account", Box::new(|v| find(v, "rules", |r| r["account_id"] == "fees")["account_id"] = json!("expense"))),
        ("rule position collision", Box::new(|v| {
            let p = v["bankfeeds"]["rules"][0]["position"].clone();
            v["bankfeeds"]["rules"][1]["position"] = p;
        })),
        ("rule dropped", Box::new(|v| {
            v["bankfeeds"]["rules"].as_array_mut().unwrap().remove(0);
        })),
        ("rule sign", Box::new(|v| v["bankfeeds"]["rules"][0]["sign"] = json!("BOTH"))),
    ];
    for (why, mutate) in &rule_cases {
        let mut bad = rules.clone();
        mutate(&mut bad);
        assert_ne!(bad, rules, "{why} changed nothing");
        rejects(&bad, why);
    }
}

#[test]
fn bankfeeds_key_normalization_leaves_no_raw_derived_keys() {
    let good = dump("bankfeeds-accept");
    let text = good.to_string();
    assert!(text.contains("bankfeed:CHECK:conn/org1/chk/c1:1:"), "request keys are labelled after a return");
    assert!(text.contains("EFT#T"), "EFT numbers are labelled");
    assert!(!regex_like_eft(&text), "a raw EFT-<n> survived");
}
fn regex_like_eft(text: &str) -> bool {
    text.match_indices("EFT-").any(|(i, _)| text[i + 4..].chars().next().is_some_and(|c| c.is_ascii_digit()))
}

#[test]
fn review_comparer_requires_query_result_inventory() {
    let mut bad = dump("bankfeeds-connect-map");
    assert!(!bad["bankfeeds"]["results"].as_object().unwrap().is_empty());
    bad["bankfeeds"]["results"] = json!({});
    rejects(&bad,"all bank-feed query results erased identically");
}

#[test]
fn review_comparer_requires_empty_downloaded_accounts() {
    let mut bad = dump("bankfeeds-connect-map");
    let tables = &bad["bankfeeds"];
    let index = tables["accounts"].as_array().unwrap().iter().position(|a| {
        a["ledger_account_id"].is_null() && !tables["transactions"].as_array().unwrap().iter().any(|t| t["feed_account_id"] == a["id"]) &&
        !bad["audit"].as_object().unwrap().values().any(|events| events.as_array().unwrap().iter().any(|e| e["target"] == format!("bankfeedaccount:{}",a["id"].as_str().unwrap())))
    }).expect("scenario includes an unmapped account without transactions or mapping audits");
    bad["bankfeeds"]["accounts"].as_array_mut().unwrap().remove(index);
    rejects(&bad,"empty unmapped downloaded account omitted identically");
}

#[test]
fn review_comparer_rejects_wrong_rule_priority() {
    let mut bad = dump("bankfeeds-rules");
    let rules = bad["bankfeeds"]["rules"].as_array_mut().unwrap();
    let n = rules.len() as i64;
    for r in rules { r["position"] = json!(r["position"].as_i64().unwrap() + n); }
    rejects(&bad,"all rule positions shifted beyond their audit evidence");
}

#[test]
fn review_comparer_requires_success_on_each_side() {
    let scenario = json!({"users":[{"username":"review","is_staff":true}],"steps":[
        {"op":"create_company","actor":"review","ref":"co","name":"Review","currency":"USD"},
        {"op":"bankfeeds_forget","actor":"review","company":"co","connection":"absent","expect_error":false}
    ]});
    let path = std::env::temp_dir().join(format!("review-outcome-{}.fbbook",std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut runner = fb_harness::Runner::new(&scenario,&path);
    runner.run();
    let out = runner.dump();
    assert_eq!(out["outcomes"][1]["ok"],false);
    rejects(&out,"expected-success step fails identically on both sides");
}

fn review_named_company_dump(name: &str) -> Value {
    let mut scenario: Value = serde_json::from_str(&std::fs::read_to_string(format!("{}/scenarios/bankfeeds-connect-map.json",env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap();
    scenario["steps"][0]["name"] = json!(name);
    let path = std::env::temp_dir().join(format!("normalize-bankfeeds-{}-{}.fbbook",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
    let mut runner = fb_harness::Runner::new(&scenario,&path);
    runner.run();
    runner.dump()
}

#[test]
fn review_normalization_preserves_ordinary_company_name() {
    let out = review_named_company_dump("Ordinary EFT-1");
    let company = out["companies"].as_array().unwrap().iter().find(|c|c["ref"]=="co").unwrap();
    assert_eq!(company["name"],"Ordinary EFT-1","generated-number normalization changed an ordinary company name");
}

#[test]
fn review_normalization_does_not_mask_real_business_name_difference() {
    // FIX1 adaptation: the original asserted the overbroad rewrite as a precondition (the name had to come
    // back changed). With normalization limited to generated fields the name stays exact, so compare it
    // with every label the old rewrite could have produced instead.
    let first = review_named_company_dump("Ordinary EFT-1");
    for name in ["Ordinary EFT#T1", "Ordinary EFT#T2"] {
        let second = review_named_company_dump(name);
        assert!(!fb_harness::compare(&first,&second).ok(),"different real company names became MATCH after normalization: {name}");
    }
}

#[test]
fn review_comparer_rejects_overflowing_counts_without_panicking() {
    let mut bad=dump("bankfeeds-connect-map");
    for events in bad["audit"].as_object_mut().unwrap().values_mut() {
        if let Some(event)=events.as_array_mut().unwrap().iter_mut().find(|e|e["action"]=="bankfeed.synced" && e["detail"]["ok"]==true) {
            event["detail"]["new"]=json!(i64::MAX);
            event["detail"]["pending_dropped"]=json!(-1);
            break;
        }
    }
    let result=std::panic::catch_unwind(||fb_harness::compare(&bad,&bad));
    assert!(result.is_ok(),"comparer panicked on overflowing stored sync counts");
    assert!(!result.unwrap().ok(),"overflowing sync counts were accepted");
}

#[test]
fn review_comparer_validates_query_result_fields() {
    let mut bad=dump("bankfeeds-connect-map");
    assert!(bad["bankfeeds"]["results"].as_object().unwrap().contains_key("mappable"));
    bad["bankfeeds"]["results"]["mappable"]=json!({"accounts":7});
    rejects(&bad,"mappable account list has numeric scalar type");
}

// --- Sol review 2 (d393c70): three further regressions, copied verbatim ---
#[test]
fn r2_comparer_requires_query_list_members() {
    let mut bad=dump("bankfeeds-connect-map");
    assert!(fb_harness::compare(&bad,&bad).ok());
    assert!(!bad["bankfeeds"]["results"]["mappable"]["accounts"].as_array().unwrap().is_empty());
    bad["bankfeeds"]["results"]["mappable"]["accounts"]=json!([]);
    rejects(&bad,"all mappable account results erased while retaining the result key");
}

#[test]
fn r2_comparer_requires_review_list_members() {
    let mut bad=dump("bankfeeds-pulls");
    assert!(fb_harness::compare(&bad,&bad).ok());
    let mut changed=0;
    for result in bad["bankfeeds"]["results"].as_object_mut().unwrap().values_mut() {
        // clippy::collapsible_if adaptation of the reviewer's nested `if let` / `if`; same behaviour.
        if let Some(items)=result.get_mut("items") && !items.as_array().unwrap().is_empty() { *items=json!([]); changed+=1; }
    }
    assert!(changed>0);
    rejects(&bad,"review lists erased identically while retaining result keys");
}

#[test]
fn r2_normalization_preserves_explicit_check_number() {
    let mut scenario: Value=serde_json::from_str(&std::fs::read_to_string(format!("{}/scenarios/bankfeeds-accept.json",env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap();
    let steps=scenario["steps"].as_array_mut().unwrap();
    let check=steps.iter_mut().find(|s| s["op"]=="bankfeeds_accept" && s["values"]["kind"]=="CHECK").unwrap();
    check["values"]["check_number"]=json!("EFT-1");
    let path=std::env::temp_dir().join(format!("r2-explicit-check-{}.fbbook",std::process::id()));
    let mut runner=fb_harness::Runner::new(&scenario,&path);
    runner.run();
    let out=runner.dump();
    // Inspect the output for a check number explicitly chosen by the scenario.
    fn contains(v:&Value)->bool {
        match v {
            Value::Object(m)=>m.iter().any(|(k,x)| k=="check_number" && x=="EFT-1" || contains(x)),
            Value::Array(a)=>a.iter().any(contains),
            _=>false,
        }
    }
    assert!(contains(&out),"explicit check number was rewritten as a generated number");
}

// --- FIX2: credentials never persist; the Python reference's known leaks compare as v2 redacts them ---
fn feed_audit_strings(v: &Value) -> Vec<String> {
    let mut out = vec![];
    for events in v["audit"].as_object().unwrap().values() {
        for e in events.as_array().unwrap().iter().filter(|e| e["action"].as_str().unwrap_or("").starts_with("bankfeed.")) {
            out.push(e.to_string());
        }
    }
    out
}
#[test]
fn fix2_glued_credentials_never_reach_the_rust_summary_audit_or_messages() {
    let good = dump("bankfeeds-pulls");
    let r = fb_harness::compare(&good, &good);
    assert!(r.ok(), "{:?}", r.failures);
    let text = good.to_string();
    for secret in ["user:pw", "tok3n"] {
        assert!(!text.contains(secret), "credential {secret} persisted in the Rust dump");
    }
    // The scenario's glued failures did reach the audit trail, redacted.
    let audit = feed_audit_strings(&good).join("\n");
    for redacted in ["failedhttps://***@bridge.example.test/x", "9https://***@bridge.example.test", "ftp://***@files.example.test/x"] {
        assert!(audit.contains(redacted), "missing redacted audit text {redacted}");
    }
}
#[test]
fn fix2_comparer_holds_rust_to_credential_absence_and_redacts_only_python_leaks() {
    let good = dump("bankfeeds-pulls");
    // Python's frozen redact keeps the glued credential: that alone still MATCHes.
    let mut py = good.to_string().replace("failedhttps://***@bridge", "failedhttps://user:pw@bridge").replace("ftp://***@files", "ftp://user:pw@files");
    assert_ne!(py, good.to_string());
    let py: Value = serde_json::from_str(&std::mem::take(&mut py)).unwrap();
    let r = fb_harness::compare(&py, &good);
    assert!(r.ok() && r.warnings.is_empty(), "{:?} {:?}", r.failures, r.warnings);
    // A Rust side that keeps it fails, even identical to Python.
    let leaked: Value = serde_json::from_str(&good.to_string().replace("failedhttps://***@bridge", "failedhttps://user:pw@bridge")).unwrap();
    rejects(&leaked, "rust kept a glued credential");
    assert!(!fb_harness::compare(&good, &leaked).ok(), "rust kept a glued credential against a clean reference");
    // Any other Python difference in the same strings is still a difference.
    let other: Value = serde_json::from_str(&good.to_string().replace("failedhttps://***@bridge", "failedhttps://***@bridgE")).unwrap();
    assert!(!fb_harness::compare(&other, &good).ok(), "a non-credential difference was masked");
}

// --- FIX2: normalization by recorded provenance, never by field name alone ---
fn numbers(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => m.iter().for_each(|(k, x)| match (k.as_str(), x) {
            ("check_number" | "number_key", Value::String(s)) => out.push(s.clone()),
            _ => numbers(x, out),
        }),
        Value::Array(a) => a.iter().for_each(|x| numbers(x, out)),
        _ => {}
    }
}
#[test]
fn fix2_literal_eft_numbers_stay_while_generated_ones_are_normalized() {
    let mut scenario: Value = serde_json::from_str(&std::fs::read_to_string(format!("{}/scenarios/bankfeeds-accept.json", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap();
    let steps = scenario["steps"].as_array_mut().unwrap();
    // The first CHECK accept would generate EFT-1 itself; the scenario now writes that literal.
    let first = steps.iter_mut().find(|s| s["op"] == "bankfeeds_accept" && s["values"]["kind"] == "CHECK").unwrap();
    first["values"]["check_number"] = json!("EFT-1");
    let path = std::env::temp_dir().join(format!("fix2-literal-eft-{}.fbbook", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut runner = fb_harness::Runner::new(&scenario, &path);
    runner.run();
    let out = runner.dump();
    drop(runner);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    let mut found = vec![];
    numbers(&out, &mut found);
    assert!(found.iter().any(|n| n == "EFT-1"), "the literal EFT-1 was rewritten: {found:?}");
    assert!(found.iter().any(|n| n.starts_with("EFT#T")), "generated numbers are no longer normalized: {found:?}");
    let raw_generated: Vec<&String> = found.iter().filter(|n| n.starts_with("EFT-") && *n != "EFT-1").collect();
    assert!(raw_generated.is_empty(), "a generated number kept its raw ordinal: {raw_generated:?}");
}

// --- FIX2: query lists checked member by member and in order against per-step evidence ---
#[test]
fn fix2_query_lists_reject_identical_member_order_and_evidence_corruption() {
    let mut tested = 0;
    for name in ["bankfeeds-connect-map", "bankfeeds-pulls", "bankfeeds-rules", "bankfeeds-accept"] {
        let good = dump(name);
        let r = fb_harness::compare(&good, &good);
        assert!(r.ok(), "{name}: {:?}", r.failures);
        let lists: Vec<(String, &str)> = good["bankfeeds"]["results"]
            .as_object()
            .unwrap()
            .iter()
            .filter_map(|(k, v)| ["accounts", "items"].into_iter().find(|f| v[*f].is_array()).map(|f| (k.clone(), f)))
            .collect();
        for (result, field) in lists {
            let list = good["bankfeeds"]["results"][&result][field].as_array().unwrap().clone();
            let step = good["outcomes"].as_array().unwrap().iter().position(|o| o["manifest"]["result"] == json!(result)).unwrap();
            let mut cases: Vec<(String, Value)> = vec![];
            let mut push = |why: &str, f: &dyn Fn(&mut Value)| {
                let mut bad = good.clone();
                f(&mut bad);
                cases.push((format!("{name} {result}: {why}"), bad));
            };
            if !list.is_empty() {
                push("list emptied", &|b| b["bankfeeds"]["results"][&result][field] = json!([]));
                push("last member dropped", &|b| drop(b["bankfeeds"]["results"][&result][field].as_array_mut().unwrap().pop()));
                let evidence_key = if field == "items" { "transactions" } else { "accounts" };
                push("list and evidence emptied together", &|b| {
                    b["bankfeeds"]["results"][&result][field] = json!([]);
                    b["outcomes"][step]["evidence"][evidence_key] = json!([]);
                });
            }
            if list.len() > 1 {
                push("order reversed", &|b| b["bankfeeds"]["results"][&result][field].as_array_mut().unwrap().reverse());
            }
            push("evidence removed", &|b| drop(b["outcomes"][step].as_object_mut().unwrap().remove("evidence")));
            for (why, bad) in cases {
                rejects(&bad, &why);
                tested += 1;
            }
        }
        // An account step's manifest is what bounds the account lists from below.
        let account = good["outcomes"].as_array().unwrap().iter().position(|o| o["op"] == "save_account").unwrap();
        let mut bad = good.clone();
        bad["outcomes"][account].as_object_mut().unwrap().remove("manifest");
        rejects(&bad, &format!("{name}: account step manifest removed"));
        tested += 1;
    }
    assert!(tested >= 29, "{tested}");
}
#[test]
fn r3_comparer_rejects_each_single_review_member_omission_with_evidence() {
    let good = dump("bankfeeds-pulls");
    assert!(fb_harness::compare(&good, &good).ok());
    let mut accepted = vec![];
    let mut tried = 0;
    for (step, outcome) in good["outcomes"].as_array().unwrap().iter().enumerate() {
        if outcome["op"] != "bankfeeds_items" || outcome["ok"] != true { continue; }
        let key = outcome["manifest"]["result"].as_str().unwrap();
        for member in good["bankfeeds"]["results"][key]["items"].as_array().unwrap() {
            let mut bad = good.clone();
            bad["bankfeeds"]["results"][key]["items"].as_array_mut().unwrap().retain(|v| v != member);
            bad["outcomes"][step]["evidence"]["transactions"].as_array_mut().unwrap().retain(|v| &v["tx"] != member);
            tried += 1;
            if fb_harness::compare(&bad, &bad).ok() { accepted.push((key.to_owned(), member.clone())); }
        }
    }
    assert!(tried > 5);
    assert!(accepted.is_empty(), "identical member/evidence omissions MATCH: {accepted:?}");
}

#[test]
fn r3_normalization_preserves_literal_from_ordinary_check_service() {
    let mut scenario: Value = serde_json::from_str(&std::fs::read_to_string(format!("{}/scenarios/bankfeeds-accept.json", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap();
    scenario["steps"].as_array_mut().unwrap().push(json!({
        "op":"checks_save", "company":"co", "ref":"r3-manual-check",
        "values": {"bank_id":"savings", "payee_kind":"OTHER", "payee_name":"Manual supplier", "date":"2026-10-23", "check_number":"EFT-1", "memo":"Explicit manual check", "request_key":"a4df60d9-49bd-4327-9644-ab28fc9f6d20", "lines":[{"tab":"EXPENSE","account_id":"expense","rate":"12.34"}]}
    }));
    let path = std::env::temp_dir().join(format!("r3-explicit-manual-check-{}.fbbook",std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut runner = fb_harness::Runner::new(&scenario, &path);
    runner.run();
    let out = runner.dump();
    assert_eq!(out["outcomes"].as_array().unwrap().last().unwrap()["ok"], true, "manual check creation refused");
    let mut found = vec![];
    numbers(&out, &mut found);
    scenario["steps"].as_array_mut().unwrap().last_mut().unwrap()["values"]["check_number"] = json!("EFT#T3");
    let path2 = std::env::temp_dir().join(format!("r3-explicit-manual-check-other-{}.fbbook",std::process::id()));
    let _ = std::fs::remove_file(&path2);
    let mut other = fb_harness::Runner::new(&scenario, &path2);
    other.run();
    let changed = other.dump();
    assert_eq!(changed["outcomes"].as_array().unwrap().last().unwrap()["ok"], true);
    assert!(!fb_harness::compare(&out, &changed).ok(), "different explicit ordinary check numbers EFT-1 and EFT#T3 MATCH");
    assert!(found.iter().any(|n| n == "EFT-1"), "explicit ordinary check number lost: {found:?}");
}

#[test]
fn r3_comparer_rejects_quoted_credentials_even_if_both_sides_leak() {
    let good = dump("bankfeeds-pulls");
    let encoded = good.to_string();
    let changed = encoded.replace("failedhttps://***@bridge", "failedhttps://r3user:r3pa'ss@bridge");
    assert_ne!(encoded, changed);
    let bad: Value = serde_json::from_str(&changed).unwrap();
    rejects(&bad, "both sides persisted credentials containing an apostrophe");
}

#[test]
fn r3_new_credential_scenarios_require_the_intended_failure_reason() {
    let mut scenario: Value = serde_json::from_str(&std::fs::read_to_string(format!("{}/scenarios/bankfeeds-pulls.json", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap();
    let mut changed = vec![];
    for (index, step) in scenario["steps"].as_array_mut().unwrap().iter_mut().enumerate() {
        let fail = step["fail"].as_str().unwrap_or("");
        if fail.starts_with("failed") || fail.starts_with("bridge 9") || fail.starts_with("again:") {
            step["actor"] = json!("outsider");
            changed.push(index);
        }
    }
    assert_eq!(changed.len(), 4);
    let path = std::env::temp_dir().join(format!("r3-wrong-failure-reason-{}.fbbook", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut runner = fb_harness::Runner::new(&scenario, &path);
    runner.run();
    let mut out = runner.dump();
    for index in changed {
        assert_eq!(out["outcomes"][index]["kind"], "PermissionDenied");
        // Main's stricter driver flags a wrong exception kind immediately. The comparer must also
        // detect the pinned-reason violation independently, even if that driver flag is erased.
        assert_eq!(out["outcomes"][index]["unexpected"], true);
        out["outcomes"][index].as_object_mut().unwrap().remove("unexpected");
    }
    assert!(!fb_harness::compare(&out, &out).ok(), "all four new credential failures were permission refusals instead of sync failures, yet MATCH");
}

#[test]
fn scenario_pins_apply_the_reviewed_python_redaction_but_still_refuse_wrong_messages() {
    let scenario: Value = serde_json::from_str(include_str!("../scenarios/bankfeeds-pulls.json")).unwrap();
    let good = dump("bankfeeds-pulls");
    let mut python = good.clone();
    let index = python["outcomes"].as_array().unwrap().iter().position(|o| o["message"].as_str().is_some_and(|s| s.contains("failedhttps://***@"))).unwrap();
    python["outcomes"][index]["message"] = json!(python["outcomes"][index]["message"].as_str().unwrap().replace("***@", "user:pw@"));
    let report = fb_harness::compare_scenario(Some(&scenario), &python, &good);
    assert!(report.ok(), "{:?}", report.failures);
    let mut wrong = good.clone();
    wrong["outcomes"][index]["message"] = json!("another refusal reason");
    wrong["outcomes"][index].as_object_mut().unwrap().remove("unexpected");
    assert!(!fb_harness::compare_scenario(Some(&scenario), &wrong, &wrong).ok());
}

/// FIX3: the review-list inventory is replayed over time, so a row a pull withdrew (named in that pull's
/// `pending_dropped_ids`) cannot come back into a later list and its evidence, in its correct place.
#[test]
fn fix3_comparer_rejects_a_withdrawn_row_back_in_a_review_list() {
    let good = dump("bankfeeds-pulls");
    let mut tried = 0;
    for (step, outcome) in good["outcomes"].as_array().unwrap().iter().enumerate() {
        if outcome["op"] != "bankfeeds_items" || outcome["ok"] != true {
            continue;
        }
        let key = outcome["manifest"]["result"].as_str().unwrap();
        let listed: Vec<Value> = good["bankfeeds"]["results"][key]["items"].as_array().unwrap().clone();
        // Every transaction an earlier pull listed that this list does not hold: withdrawn rows.
        let earlier: Vec<String> = good["outcomes"].as_array().unwrap()[..step]
            .iter()
            .filter(|o| (o["op"] == "bankfeeds_sync" || o["op"] == "bankfeeds_store") && o["ok"] == true)
            .flat_map(|o| o["manifest"]["transactions"].as_array().unwrap().iter().map(|t| t.as_str().unwrap().to_owned()))
            .filter(|t| !listed.contains(&json!(t)))
            .collect();
        for gone in earlier {
            let mut bad = good.clone();
            let rows = bad["outcomes"][step]["evidence"]["transactions"].as_array_mut().unwrap();
            let seq = rows.iter().filter_map(|r| r["seq"].as_u64()).max().unwrap_or(0) + 1;
            rows.push(json!({"tx": gone, "posted_on": null, "seq": seq}));
            rows.sort_by(|a, b| a["tx"].as_str().cmp(&b["tx"].as_str()));
            // Undated, highest sequence: first in `-posted_on, -pk` order.
            bad["bankfeeds"]["results"][key]["items"].as_array_mut().unwrap().insert(0, json!(gone));
            rejects(&bad, &format!("withdrawn {gone} back in review list {key}"));
            tried += 1;
        }
    }
    assert!(tried >= 1, "no withdrawn row to bring back");
}

/// FIX3 (main's rule): every refusal in every bank-feed scenario pins a non-empty kind and message.
#[test]
fn fix3_every_bank_feed_refusal_pins_kind_and_message() {
    let mut refusals = 0;
    for entry in std::fs::read_dir(format!("{}/scenarios", env!("CARGO_MANIFEST_DIR"))).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !name.starts_with("bankfeeds-") || !name.ends_with(".json") {
            continue;
        }
        let scenario: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for (index, step) in scenario["steps"].as_array().unwrap().iter().enumerate() {
            if step["expect_error"] == true {
                refusals += 1;
                for pin in ["expect_kind", "expect_message"] {
                    assert!(step[pin].as_str().is_some_and(|s| !s.is_empty()), "{name} step {index}: no {pin}");
                }
            }
        }
    }
    assert!(refusals >= 300, "refusals checked: {refusals}");
}

/// FIX3: a pinned refusal refused for another reason fails on the side that did it, even when only one
/// side does, and when both do it identically.
#[test]
fn fix3_comparer_rejects_a_refusal_for_another_reason_on_either_side() {
    let good = dump("bankfeeds-pulls");
    assert!(fb_harness::compare(&good, &good).ok());
    let steps: Vec<usize> = good["outcomes"].as_array().unwrap().iter().enumerate().filter(|(_, o)| o["ok"] == false).map(|(i, _)| i).collect();
    assert_eq!(steps.len(), 12);
    for step in steps {
        for (field, wrong) in [("message", json!("Company access denied.")), ("kind", json!("PermissionDenied"))] {
            if good["outcomes"][step][field] == wrong {
                continue;
            }
            for bad_side in ["python", "rust", "both"] {
                let (mut py, mut rs) = (good.clone(), good.clone());
                if bad_side != "rust" {
                    py["outcomes"][step][field] = wrong.clone();
                }
                if bad_side != "python" {
                    rs["outcomes"][step][field] = wrong.clone();
                }
                let report = fb_harness::compare(&py, &rs);
                for side in ["python", "rust"] {
                    let flagged = report.failures.iter().any(|f| f.contains(&format!("{side} refused for another reason")));
                    assert_eq!(flagged, bad_side == side || bad_side == "both", "step {step} {field} {bad_side}/{side}: {:?}", report.failures);
                }
            }
        }
    }
}

#[test]
fn r4_comparer_refuses_impossible_withdrawal_identity() {
    let good = dump("bankfeeds-pulls");
    assert!(fb_harness::compare(&good,&good).ok());
    let mut tested=0;
    for (company,events) in good["audit"].as_object().unwrap() {
        for (index,event) in events.as_array().unwrap().iter().enumerate() {
            if event["action"]=="bankfeed.synced" && event["detail"]["ok"]==true && event["detail"]["pending_dropped_ids"].as_array().is_some_and(|a|!a.is_empty()) {
                let mut bad=good.clone();
                bad["audit"][company][index]["detail"]["pending_dropped_ids"][0]=json!("R4-never-downloaded-remote-id");
                if let Some(path) = std::env::var_os("FB_R4_IMPOSSIBLE_WITHDRAWAL_OUT") { std::fs::write(path,serde_json::to_vec_pretty(&bad).unwrap()).unwrap(); }
                let report=fb_harness::compare(&bad,&bad);
                assert!(!report.ok(),"impossible pending withdrawal identity MATCHed at {company} audit {index}");
                tested+=1;
            }
        }
    }
    assert!(tested>0);
}

#[test]
fn r4_comparer_refuses_complete_review_erasure_after_forged_withdrawal() {
    let good = dump("bankfeeds-pulls");
    let mut attempted=0;
    for (step,outcome) in good["outcomes"].as_array().unwrap().iter().enumerate() {
        if outcome["op"]!="bankfeeds_items" || outcome["ok"]!=true { continue; }
        let key=outcome["manifest"]["result"].as_str().unwrap();
        let list=good["bankfeeds"]["results"][key]["items"].as_array().unwrap();
        for item in list {
            let mut bad=good.clone();
            bad["bankfeeds"]["results"][key]["items"].as_array_mut().unwrap().retain(|r|r!=item);
            bad["outcomes"][step]["evidence"]["transactions"].as_array_mut().unwrap().retain(|r|r["tx"]!=*item);
            for events in bad["audit"].as_object_mut().unwrap().values_mut() {
                for event in events.as_array_mut().unwrap() {
                    if event["action"]=="bankfeed.synced" && event["detail"]["pending_dropped_ids"].as_array().is_some_and(|a|!a.is_empty()) {
                        event["detail"]["pending_dropped_ids"][0]=json!("R4-never-downloaded-remote-id");
                    }
                }
            }
            if let (true, Some(path)) = (fb_harness::compare(&bad,&bad).ok(), std::env::var_os("FB_R4_OMITTED_REVIEW_OUT")) { std::fs::write(path,serde_json::to_vec_pretty(&bad).unwrap()).unwrap(); }
            assert!(!fb_harness::compare(&bad,&bad).ok(),"omission of {item} plus impossible withdrawal MATCHed in {key}");
            attempted+=1;
        }
    }
    assert!(attempted>5);
}

/// FIX4: withdrawal evidence that does not line up fails the comparer, whatever else the dump says.
#[test]
fn fix4_comparer_refuses_withdrawal_counts_that_disagree_with_the_named_ids() {
    let good = dump("bankfeeds-pulls");
    assert!(fb_harness::compare(&good, &good).ok());
    let mut tested = 0;
    for (company, events) in good["audit"].as_object().unwrap() {
        for (index, event) in events.as_array().unwrap().iter().enumerate() {
            if event["action"] != "bankfeed.synced" || event["detail"]["ok"] != true || !event["detail"]["pending_dropped_ids"].as_array().is_some_and(|a| !a.is_empty()) {
                continue;
            }
            // Fewer ids than the count (as if truncated), and a count above the ids.
            let mut fewer = good.clone();
            fewer["audit"][company][index]["detail"]["pending_dropped_ids"].as_array_mut().unwrap().pop();
            rejects(&fewer, &format!("{company} audit {index}: a withdrawn id removed"));
            let mut more = good.clone();
            let n = more["audit"][company][index]["detail"]["pending_dropped"].as_u64().unwrap();
            more["audit"][company][index]["detail"]["pending_dropped"] = json!(n + 1);
            rejects(&more, &format!("{company} audit {index}: withdrawal count raised"));
            // An id repeated: no live candidate withdraws twice.
            let mut twice = good.clone();
            let first = twice["audit"][company][index]["detail"]["pending_dropped_ids"][0].clone();
            twice["audit"][company][index]["detail"]["pending_dropped_ids"].as_array_mut().unwrap().push(first);
            twice["audit"][company][index]["detail"]["pending_dropped"] = json!(n + 1);
            rejects(&twice, &format!("{company} audit {index}: a withdrawn id repeated"));
            tested += 1;
        }
    }
    assert!(tested > 0);
}

/// FIX4: the replay's final inventory is every company's dumped rows. Erasing a downloaded row from the
/// dump, every list and its evidence, with the sync counts forged to agree, still fails.
#[test]
fn fix4_comparer_refuses_row_erasure_even_with_forged_sync_counts() {
    let good = dump("bankfeeds-pulls");
    let ids: Vec<String> = good["bankfeeds"]["transactions"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap().to_owned()).collect();
    assert!(ids.len() > 5);
    for id in &ids {
        let mut bad = good.clone();
        bad["bankfeeds"]["transactions"].as_array_mut().unwrap().retain(|t| t["id"] != json!(id));
        for result in bad["bankfeeds"]["results"].as_object_mut().unwrap().values_mut() {
            if let Some(items) = result["items"].as_array_mut() {
                items.retain(|t| t != &json!(id));
            }
        }
        for o in bad["outcomes"].as_array_mut().unwrap() {
            if let Some(rows) = o["evidence"]["transactions"].as_array_mut() {
                rows.retain(|r| r["tx"] != json!(id));
            }
        }
        // Lower the last ok sync's `new` count on the row's connection, so the count check agrees.
        let connection = id.split('/').next().unwrap().to_owned();
        let target = json!(format!("bankfeedconnection:{connection}"));
        let mut forged = false;
        for events in bad["audit"].as_object_mut().unwrap().values_mut() {
            for e in events.as_array_mut().unwrap().iter_mut().rev() {
                if !forged && e["action"] == "bankfeed.synced" && e["target"] == target && e["detail"]["ok"] == true && e["detail"]["new"].as_u64().is_some_and(|n| n > 0) {
                    e["detail"]["new"] = json!(e["detail"]["new"].as_u64().unwrap() - 1);
                    forged = true;
                }
            }
        }
        assert!(forged, "{id}: no sync count to forge");
        // A row with its own audit trail is also caught by the audit-target check; drop that trail too.
        let audit_target = json!(format!("bankfeedtransaction:{id}"));
        for events in bad["audit"].as_object_mut().unwrap().values_mut() {
            events.as_array_mut().unwrap().retain(|e| e["target"] != audit_target);
        }
        rejects(&bad, &format!("{id} erased with forged counts"));
    }
}

#[test]
fn r5_withdrawal_replay_checks_complete_run_without_review_queries() {
    let mut scenario: Value=serde_json::from_str(&std::fs::read_to_string(format!("{}/scenarios/bankfeeds-pulls.json",env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap();
    scenario["steps"].as_array_mut().unwrap().retain(|s|s["op"]!="bankfeeds_items");
    let dir=tempfile::tempdir().unwrap();
    let mut runner=fb_harness::Runner::new(&scenario,&dir.path().join("r5.fbbook"));
    runner.run();let good=runner.dump();
    let report=fb_harness::compare(&good,&good);
    assert!(report.ok(),"valid scenario without review queries: {:?}",report.failures);
    assert!(good["outcomes"].as_array().unwrap().iter().all(|o|o["op"]!="bankfeeds_items"));
    let mut mutations=0;
    for (company,events) in good["audit"].as_object().unwrap() {
        for (index,event) in events.as_array().unwrap().iter().enumerate() {
            if event["action"]!="bankfeed.synced" || event["detail"]["ok"]!=true {continue;}
            let ids=event["detail"]["pending_dropped_ids"].as_array().unwrap();
            // Impossible identities still fail if the claimed count is changed to agree.
            for replacement in [json!(["R5-impossible"]),json!(["R5-impossible","R5-impossible"])] {
                let mut bad=good.clone();
                bad["audit"][company][index]["detail"]["pending_dropped"]=json!(replacement.as_array().unwrap().len());
                bad["audit"][company][index]["detail"]["pending_dropped_ids"]=replacement;
                rejects(&bad,"forged count and identities, no review queries");mutations+=1;
            }
            if !ids.is_empty() {
                for ty in [json!(null),json!(7),json!({}),json!(true),json!([null]),json!([7])] {
                    let mut bad=good.clone();bad["audit"][company][index]["detail"]["pending_dropped_ids"]=ty;
                    rejects(&bad,"malformed withdrawal evidence, no review queries");mutations+=1;
                }
            }
            // A missing paired audit and a duplicated paired audit cannot weaken the replay.
            let mut missing=good.clone();missing["audit"][company].as_array_mut().unwrap().remove(index);
            rejects(&missing,"missing successful sync audit");mutations+=1;
            let mut extra=good.clone();extra["audit"][company].as_array_mut().unwrap().push(event.clone());
            rejects(&extra,"extra successful sync audit");mutations+=1;
        }
    }
    assert!(mutations>=20);
    eprintln!("R5: {mutations} contradictory evidence mutations refused without any review-list query");
}

/// FIX5: ids that differ only in URL userinfo are named by scenario refs (runner-recorded provenance) and stay
/// two rows; the Rust dump carries no credential anywhere.
#[test]
fn fix5_identity_scenario_keeps_userinfo_twins_apart_without_credentials() {
    let good = dump("bankfeeds-identity");
    let report = fb_harness::compare(&good, &good);
    assert!(report.ok(), "{:?}", report.failures);
    let ids = |table: &str| good["bankfeeds"][table].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
    assert_eq!(ids("accounts"), ["conn/~/acct-a", "conn/~/acct-b"]);
    assert_eq!(ids("transactions"), ["conn/~/acct-a/hold-second", "conn/~/acct-a/tx-a", "conn/~/acct-b/tx-b"]);
    let remotes: Vec<&str> = good["bankfeeds"]["accounts"].as_array().unwrap().iter().map(|r| r["remote_id"].as_str().unwrap()).collect();
    assert_eq!(remotes, ["https://***@ids.example.test/acct", "https://***@ids.example.test/acct"], "displayed alike, kept apart");
    let text = good.to_string();
    assert!(!text.contains("pw-first") && !text.contains("pw-second") && !text.contains("first:") && !text.contains("second:"), "a credential reached the Rust dump");
}

/// FIX5: the comparer refuses a merged identity (the FIX4 regression's two shapes), and an alias on an
/// id that carries no userinfo (aliases exist only to keep credentials out of names).
#[test]
fn fix5_comparer_rejects_merged_identities_and_needless_aliases() {
    let good = dump("bankfeeds-identity");
    // The second account's transaction silently merged into the first's accepted row.
    let mut bad = good.clone();
    bad["bankfeeds"]["transactions"].as_array_mut().unwrap().retain(|t| t["id"] != "conn/~/acct-b/tx-b");
    rejects(&bad, "a distinct transaction vanished");
    // The second account folded into the first (it would inherit the first one's mapping).
    let mut bad = good.clone();
    bad["bankfeeds"]["accounts"].as_array_mut().unwrap().retain(|a| a["id"] != "conn/~/acct-b");
    for t in bad["bankfeeds"]["transactions"].as_array_mut().unwrap() {
        if t["feed_account_id"] == "conn/~/acct-b" {
            t["feed_account_id"] = json!("conn/~/acct-a");
            t["id"] = json!("conn/~/acct-a/tx-b");
        }
    }
    rejects(&bad, "a distinct account merged into its twin");
    // An alias may stand only for an id that carries userinfo.
    let mut bad = good.clone();
    find(&mut bad, "accounts", |a| a["id"] == "conn/~/acct-a")["remote_id"] = json!("plain-account");
    rejects(&bad, "an alias for a plain id");
}
