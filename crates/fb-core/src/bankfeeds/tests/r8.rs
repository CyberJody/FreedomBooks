//! The reviewer's R8 tests (review 8 of `l4-bankfeeds` at 4580245), ported as written except: the
//! evidence files go to the system temp directory instead of the reviewer's staging folder, the vectors
//! of every multi-character HTML named reference live in `r8_two.json` (from the reviewer's
//! `generate.py`), and the helpers carry an `r8_` prefix. FIX8 changed the contract: every write path
//! takes the connection's secrets, so the drive and the composed test hand the known set to `forget`,
//! `map_account`, `ignore`, `return_to_review` and `accept` too (through `Fx::secrets` for the helpers).
use super::*;

fn r8_pct(v: &str) -> String { v.bytes().map(|b| format!("%{b:02X}")).collect() }
fn r8_evidence(name: &str, v: &Value) {
    let _ = std::fs::write(std::env::temp_dir().join(name), serde_json::to_string_pretty(v).unwrap());
}
fn r8_drive(f: &mut Fx, n: usize, value: &str, s: &SecretSet) -> Vec<String> {
    f.secrets = s.clone();
    let mut returned = vec![];
    let c = connect(&mut f.book, f.u, f.c, &json!("R8"), value, s).unwrap();
    returned.push(c.to_string());
    returned.push(forget(&mut f.book, f.u, f.c, c["id"].as_str().unwrap(), s).unwrap().to_string());
    record_sync_failure(&mut f.book, f.u, f.c, &f.conn.clone(), NOW, value, false, s).unwrap();
    let ar = format!("a{n} {value}"); let tr = format!("t{n} {value}"); let sr = format!("s{n} {value}");
    let mut a = acct(&ar, value, vec![tx(&tr, "2026-10-20", 100, value)]);
    a.conn_key = value.into(); a.org_name = value.into(); a.skipped = vec![value.into()];
    let mut other = acct(&format!("other{n}"), "Other", vec![]); other.currency = value.into();
    returned.push(store_pull(&mut f.book, f.u, f.c, &f.conn.clone(), &StagedPull { accounts: vec![a, other], skipped: vec![value.into()], errors: vec![value.into()] }, NOW, false, None, s).unwrap().to_string());
    returned.push(f.map(&ar, Some("100")).unwrap().to_string());
    returned.push(format!("{:?}", stage_bank_transactions(&mut f.book, f.u, f.c, &f.conn.clone(), &[(ar.clone(), tx(&sr, "2026-10-20", -100, value))], s).unwrap()));
    let item = f.item(&tr); ignore(&mut f.book, f.u, f.c, &item.id, s).unwrap(); return_to_review(&mut f.book, f.u, f.c, &item.id, s).unwrap();
    returned.push(format!("{:?}", f.accept(&tr, json!({"kind":"JOURNAL","account_id":f.acc("410")})).unwrap()));
    returned.push(format!("{:?}", f.accept(&sr, json!({"kind":"CHECK","account_id":f.acc("500")})).unwrap()));
    f.map(&ar, None).unwrap();
    returned
}

#[test]
fn r8_multi_codepoint_html_entities_all_provider_paths() {
    let vectors: Vec<Value> = serde_json::from_str(include_str!("r8_two.json")).unwrap();
    let mut failures = vec![];
    for (i, v) in vectors.iter().enumerate() {
        let secret = v["secret"].as_str().unwrap(); let value = v["text"].as_str().unwrap();
        let secrets = SecretSet::masking_from_access_url(&format!("https://R8username-long:{}@r8.test/access", r8_pct(secret)), Vec::<String>::new()).unwrap();
        let mut f = Fx::new(); let returned = r8_drive(&mut f, i, value, &secrets);
        let mut locations = vec![];
        for (table, rows) in fb_db::test_support::snapshot(f.book.connection()).unwrap() { for row in rows { for v in row { if let fb_db::rusqlite::types::Value::Text(t) = v && t.contains(value) { locations.push(table.clone()); } } } }
        locations.sort(); locations.dedup();
        if !locations.is_empty() || returned.iter().any(|t| t.contains(value)) { failures.push(json!({"name":v["name"],"tables":locations,"returned":returned.iter().any(|t|t.contains(value))})); }
        f.reopen_clean(); r5_finish(&mut f);
    }
    r8_evidence("html-failures-r8.json", &json!(failures));
    eprintln!("R8 multi-codepoint named references={} leaked={}", vectors.len(), failures.len());
    assert!(failures.is_empty(), "multi-codepoint HTML reference credential persists: {} cases", failures.len());
}

#[test]
fn r8_html_fjlig_persisted_column_and_audit_evidence() {
    let mut f = Fx::new(); let secret = "R8prefixfjsecretlong"; let text = "R8prefix&fjlig;secretlong";
    let known = SecretSet::masking_from_credentials("R8username-long", secret, Vec::<String>::new()).unwrap();
    r8_drive(&mut f, 0, text, &known);
    let mut columns = vec![];
    for (table, cols) in [("bankfeeds_connection", vec!["host", "last_sync_summary"]), ("bankfeeds_account", vec!["conn_key", "remote_id", "name", "org_name", "currency"]), ("bankfeeds_transaction", vec!["remote_id", "description"]), ("ledger_journal", vec!["memo"]), ("ledger_journalline", vec!["description"])] {
        for col in cols { let n: i64 = f.book.connection().query_row(&format!("SELECT count(*) FROM {table} WHERE instr({col},?1)>0"), [text], |r| r.get(0)).unwrap(); if n > 0 { columns.push(format!("{table}.{col}")); } }
    }
    let mut st = f.book.connection().prepare("SELECT DISTINCT action FROM ledger_auditevent WHERE instr(detail,?1)>0 ORDER BY action").unwrap();
    let actions: Vec<String> = st.query_map([text], |r| r.get(0)).unwrap().map(Result::unwrap).collect(); drop(st);
    eprintln!("R8 fjlig leak columns={columns:?}; audit actions={actions:?}");
    r8_evidence("fjlig-evidence-r8.json", &json!({"columns":columns,"audit_actions":actions}));
    r5_finish(&mut f); assert!(columns.is_empty() && actions.is_empty(), "HTML-decoded own credential persists");
}

#[test]
fn r8_composed_provider_label_does_not_persist_raw_known_password() {
    let mut f = Fx::new(); let secret = "R8organization · R8accountname";
    let known = SecretSet::masking_from_access_url(&format!("https://R8username-long:{}@r8.test/access", r8_pct(secret)), Vec::<String>::new()).unwrap();
    let mut a = acct("composed-account", "R8accountname", vec![tx("composed-tx", "2026-10-20", 100, "Ordinary description")]); a.org_name = "R8organization".into();
    store_pull(&mut f.book, f.u, f.c, &f.conn.clone(), &StagedPull { accounts: vec![a], ..Default::default() }, NOW, false, None, &known).unwrap();
    f.secrets = known.clone();
    f.map("composed-account", Some("100")).unwrap();
    let mut st = f.book.connection().prepare("SELECT action FROM ledger_auditevent WHERE instr(detail,?1)>0 ORDER BY action").unwrap();
    let actions: Vec<String> = st.query_map([secret], |r| r.get(0)).unwrap().map(Result::unwrap).collect(); drop(st);
    eprintln!("R8 raw known password recomposed by audit writers={actions:?}");
    r8_evidence("composed-evidence-r8.json", &json!({"audit_actions":actions}));
    r5_finish(&mut f); assert!(actions.is_empty(), "raw known credential persists after composing provider fields");
}
