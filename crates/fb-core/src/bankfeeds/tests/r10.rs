// Review 10: independent end-to-end driver. Vectors come from gen.py (Python standard encoders);
// every stored TEXT, audit detail, returned value and error message is dumped for verify.py, which
// decodes with Python's standard decoders. Nothing here reuses the builder's generator or checker.
use super::*;

fn input(name: &str) -> String {
    let data = match name {
        "baseline.json" => include_str!("r10_baseline.json"),
        "vectors2.json" => include_str!("r10_vectors2.json"),
        "vectors_accepted.json" => include_str!("r10_accepted.json"),
        "joins.json" => include_str!("r10_joins.json"),
        "provider.json" => include_str!("r10_provider.json"),
        _ => panic!("unknown R10 input"),
    };
    data.to_owned()
}

fn texts(f: &Fx) -> Vec<(String, String)> {
    fb_db::test_support::snapshot(f.book.connection()).unwrap().into_iter().flat_map(|(table, rows)| {
        rows.into_iter().flatten().filter_map(move |v| match v {
            fb_db::rusqlite::types::Value::Text(s) => Some((table.clone(), s)),
            fb_db::rusqlite::types::Value::Blob(b) => Some((format!("{table}#blob"), String::from_utf8_lossy(&b).into_owned())),
            _ => None,
        })
    }).collect()
}

fn audits(f: &Fx) -> Vec<(String, String)> {
    let mut st = f.book.connection().prepare("SELECT action,target,detail FROM ledger_auditevent ORDER BY id").unwrap();
    st.query_map([], |r| Ok((r.get::<_, String>(0)?, format!("{}\u{1}{}", r.get::<_, String>(1)?, r.get::<_, String>(2)?)))).unwrap().map(Result::unwrap).collect()
}

fn s(v: &Value, k: &str) -> String { v[k].as_str().unwrap_or_default().to_owned() }
fn sv(v: &Value, k: &str, i: usize) -> String { v[k][i].as_str().unwrap_or_default().to_owned() }

/// One full provider flow over every write path (and the downstream services) with the case's fields.
pub(super) fn drive(case: &Value) -> Value {
    let known = SecretSet::from_credentials(&s(case, "user"), &s(case, "pass"), Vec::<String>::new())
        .unwrap_or_else(|_| SecretSet::masking_from_credentials(&s(case, "user"), &s(case, "pass"), Vec::<String>::new()).unwrap());
    let fv = &case["fields"];
    let mut f = Fx::new();
    f.secrets = known.clone();
    let mut steps = vec![];
    let mut returned = vec![];
    let mut refused_rollback = vec![];
    macro_rules! step {
        ($name:expr, $e:expr) => {{
            let before = texts(&f);
            let r = $e;
            match &r {
                Ok(v) => { returned.push(format!("{v:?}")); steps.push(json!([$name, "ok"])); }
                Err(e) => {
                    returned.push(e.message());
                    steps.push(json!([$name, "err", e.message()]));
                    // a refused command leaves every row as it was
                    refused_rollback.push(json!([$name, before == texts(&f)]));
                }
            }
            r.ok()
        }};
    }
    let c = step!("connect", connect(&mut f.book, f.u, f.c, &json!(s(fv, "conn_label")), &s(fv, "host"), &known));
    if let Some(c) = c { step!("forget", forget(&mut f.book, f.u, f.c, c["id"].as_str().unwrap(), &known)); }
    step!("record_sync_failure", record_sync_failure(&mut f.book, f.u, f.c, &f.conn.clone(), NOW, &s(fv, "sync_error"), false, &known));
    let mut pull = demo();
    for (i, a) in pull.accounts.iter_mut().enumerate() {
        a.name = sv(fv, "name", i);
        a.org_name = sv(fv, "org", i);
        a.skipped = vec![sv(fv, "acct_skipped", i)];
    }
    let mut d = 0;
    for a in &mut pull.accounts { for t in &mut a.transactions { t.description = sv(fv, "desc", d); d += 1; } }
    pull.accounts.push(StagedAccount { remote_id: s(fv, "remote"), name: sv(fv, "name", 3), org_name: sv(fv, "org", 3), ..acct("x", "x", vec![]) });
    pull.errors = vec![s(fv, "errors")];
    pull.skipped = vec![s(fv, "skipped")];
    let pulled = step!("store_pull", store_pull(&mut f.book, f.u, f.c, &f.conn.clone(), &pull, NOW, false, None, &known)).is_some();
    if pulled {
        for (a, code) in [("ACT-checking", "100"), ("ACT-savings", "101"), ("ACT-card", "220")] { step!("map_account", f.map(a, Some(code))); }
        let extra = s(fv, "remote");
        step!("map_account_extra", f.map(&extra, Some("105")).or_else(|_| f.map(&extra, None)));
        let staged = step!("stage", stage_bank_transactions(&mut f.book, f.u, f.c, &f.conn.clone(), &[("ACT-checking".into(), tx("TX-R10", "2026-10-20", -100, &s(fv, "staged")))], &known)).is_some();
        if staged {
            let i = f.item("TX-R10");
            step!("ignore", ignore(&mut f.book, f.u, f.c, &i.id, &known));
            step!("return_to_review", return_to_review(&mut f.book, f.u, f.c, &i.id, &known));
        }
        let receipt = f.undeposited("2500.00", "2026-10-04");
        for (remote, input) in [
            ("TX-1006", json!({"kind":"JOURNAL","account_id":f.acc("410"),"payee_name":s(fv,"payee_j")})),
            ("TX-1001", json!({"kind":"CHECK","account_id":f.acc("500"),"payee_name":s(fv,"payee_c"),"check_number":s(fv,"check_no")})),
            ("TX-1003", json!({"kind":"DEPOSIT","receipt_ids":[receipt]})),
            ("TX-1005", json!({"kind":"TRANSFER","transfer_account_id":f.acc("101")})),
        ] { step!(format!("accept {remote}"), f.accept(remote, input)); }
        let accepted = f.item("TX-1001");
        if accepted.status == "ACCEPTED" {
            if let Some(chk) = checks::current_checks(&mut f.book, f.u, f.c).unwrap().into_iter().find(|c| c["number"].as_str() == Some(accepted.reference.as_str())) {
                // later void outside any bank-feed command: re-snapshots stored fields with no secrets in scope
                step!("void_check(unguarded)", checks::void_check(&mut f.book, f.u, f.c, chk["id"].as_str().unwrap(), chk["revision"].as_i64(), &json!("2026-10-30"), &json!("Review correction")));
            }
            step!("return_to_review(check)", return_to_review(&mut f.book, f.u, f.c, &accepted.id, &known));
        }
    }
    let cat = f.acc("500");
    let rule = step!("save_rule", save_rule(&mut f.book, f.u, f.c, &json!({"name":s(fv,"rule_name"),"match_text":s(fv,"match_text"),"payee_name":s(fv,"rule_payee"),"account_id":cat}), &known));
    if let Some(rule) = rule {
        step!("edit_rule", save_rule(&mut f.book, f.u, f.c, &json!({"rule_id":rule.id,"name":s(fv,"match_text"),"match_text":s(fv,"rule_name"),"payee_name":s(fv,"rule_payee"),"account_id":cat}), &known));
        let r2 = step!("save_rule2", save_rule(&mut f.book, f.u, f.c, &json!({"name":s(fv,"rule_payee"),"match_text":s(fv,"rule_payee"),"payee_name":s(fv,"rule_name"),"account_id":cat}), &known));
        if r2.is_some() { step!("move_rule", move_rule(&mut f.book, f.u, f.c, &rule.id, &json!("down"), &known)); }
        step!("set_rule_active", set_rule_active(&mut f.book, f.u, f.c, &rule.id, &json!(false), &known));
    }
    let out = json!({"id": case["id"], "steps": steps, "refused_rollback": refused_rollback, "texts": texts(&f), "audits": audits(&f), "returned": returned});
    f.reopen_clean();
    r5_finish(&mut f);
    out
}

fn run(name: &str, output: &str) {
    let cases: Vec<Value> = serde_json::from_str(&input(name)).unwrap();
    let mut results = vec![];
    for case in cases {
        if name == "vectors_accepted.json" {
            SecretSet::from_credentials(&s(&case, "user"), &s(&case, "pass"), Vec::<String>::new()).unwrap();
        }
        let result = drive(&case);
        let password = case["pass"].as_str().unwrap();
        assert!(result["texts"].as_array().unwrap().iter().all(|t| !t[1].as_str().unwrap().contains(password)), "R10 {} persisted a password", case["id"]);
        results.push(result);
    }
    if let Ok(dir) = std::env::var("FB_R10_FUZZ_OUT") {
        use std::io::Write;
        std::fs::create_dir_all(&dir).unwrap();
        let mut out = std::fs::File::create(std::path::Path::new(&dir).join(output)).unwrap();
        for result in results { writeln!(out, "{result}").unwrap(); }
    }
}

#[test]
fn r10_baseline_markers() { run("baseline.json", "baseline.out.jsonl"); }
#[test]
fn r10_joins() { run("joins.json", "joins.out.jsonl"); }
#[test]
fn r10_provider() { run("provider.json", "provider.out.jsonl"); }
#[test]
fn r10_vectors2() { run("vectors2.json", "vectors2.out.jsonl"); }
#[test]
fn r10_vectors_accepted() { run("vectors_accepted.json", "vectors_accepted.out.jsonl"); }
