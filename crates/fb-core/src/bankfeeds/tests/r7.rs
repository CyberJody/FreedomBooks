//! The reviewer's R7 tests (review 7 of `l4-bankfeeds` at 817f260), ported as written except: every
//! credential is at least 12 characters (`MIN_CREDENTIAL_CHARS`; the reviewer's `r7username` and
//! `r7ownsecret` were 10 and 11, and would now be refused), the short-password test asserts the
//! refusal instead of the masking, the entity-boundary sweep also tries 40, 100 and 1000 zeros, and
//! the raw-id HMAC check finds the account by its masked name (the raw-name lookup relied on the leak).
use super::*;
use base64::{Engine, engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD}};


fn r7_pct(s: &str) -> String { s.bytes().map(|b| format!("%{b:02X}")).collect() }
fn r7_plain(s: &str) -> String { s.chars().filter(|c| !c.is_whitespace() && !matches!(c,'\u{200b}'..='\u{200d}'|'\u{2060}'|'\u{feff}'|'\u{ad}')).collect() }
fn r7_texts(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => {out.push(s.clone()); if let Ok(v)=serde_json::from_str::<Value>(s) {r7_texts(&v,out);}},
        Value::Array(a) => for v in a {r7_texts(v,out)},
        Value::Object(o) => for (k,v) in o {out.push(k.clone());r7_texts(v,out)},
        _=>{}
    }
}
fn r7_stored(f: &Fx) -> Vec<String> {
    let mut out=vec![];
    for (_,rows) in fb_db::test_support::snapshot(f.book.connection()).unwrap() {
        for row in rows {for v in row {if let fb_db::rusqlite::types::Value::Text(s)=v {r7_texts(&json!(s),&mut out)}}}
    }
    out
}
fn r7_variants(v: &str) -> Vec<String> {
    let j=serde_json::to_string(v).unwrap();
    let u:String=v.encode_utf16().map(|c|format!("\\u{c:04x}")).collect();
    let h:String=v.chars().map(|c|format!("&#{};",c as u32)).collect();
    let hx:String=v.chars().map(|c|format!("&#x{:x};",c as u32)).collect();
    let p=r7_pct(v);
    let mut out=vec![v.into(),p.clone(),r7_pct(&p),j[1..j.len()-1].into(),u.clone(),h.clone(),hx,STANDARD.encode(v),URL_SAFE_NO_PAD.encode(v),STANDARD.encode(format!("a{v}")),URL_SAFE_NO_PAD.encode(format!("ab{v}")),u.replace('\\',"%5C"),h.replace('&',"&amp;")];
    let more:Vec<String>=out.iter().map(|x|x.chars().map(|c|format!("{c}\u{200b}")).collect()).collect();
    out.extend(more); out
}
fn r7_drive(f: &mut Fx, n:usize, value: &str, s:&SecretSet) -> Vec<String> {
    f.secrets = s.clone();
    let mut returned=vec![];
    let c=connect(&mut f.book,f.u,f.c,&json!("R7"),value,s).unwrap();
    returned.push(c.to_string());
    returned.push(forget(&mut f.book,f.u,f.c,c["id"].as_str().unwrap(), s).unwrap().to_string());
    record_sync_failure(&mut f.book,f.u,f.c,&f.conn.clone(),NOW,value,false,s).unwrap();
    let ar=format!("a{n} {value}"); let tr=format!("t{n} {value}"); let sr=format!("s{n} {value}");
    let mut a=acct(&ar,value,vec![tx(&tr,"2026-10-20",100,value)]);
    a.conn_key=value.into();a.org_name=value.into();a.skipped=vec![value.into()];
    let mut other=acct(&format!("other{n}"),"Other",vec![]);other.currency=value.into();
    returned.push(store_pull(&mut f.book,f.u,f.c,&f.conn.clone(),&StagedPull{accounts:vec![a,other],skipped:vec![value.into()],errors:vec![value.into()]},NOW,false,None,s).unwrap().to_string());
    returned.push(f.map(&ar,Some("100")).unwrap().to_string());
    returned.push(format!("{:?}",stage_bank_transactions(&mut f.book,f.u,f.c,&f.conn.clone(),&[(ar.clone(),tx(&sr,"2026-10-20",-100,value))],s).unwrap()));
    let item=f.item(&tr);ignore(&mut f.book,f.u,f.c,&item.id, s).unwrap();return_to_review(&mut f.book,f.u,f.c,&item.id, s).unwrap();
    // SEC-FIX1: a leftover invisible control cannot enter new accounting text.
    // Keep the leak checks on every provider field and require refusal rollback.
    for (remote,payload) in [(&tr,json!({"kind":"JOURNAL","account_id":f.acc("410")})),(&sr,json!({"kind":"CHECK","account_id":f.acc("500")}))] {
        let item=f.item(remote);
        let has_control=!fb_db::check_numbers::free_text_valid(&item.description);
        let before=has_control.then(||fb_db::test_support::snapshot(f.book.connection()).unwrap());
        match f.accept(remote,payload) {
            Ok(accepted)=>returned.push(format!("{accepted:?}")),
            Err(e)=>{
                assert_eq!(e.message(),"Control and formatting characters are not allowed.");
                assert_eq!(before.expect("the provider description contains a control"),fb_db::test_support::snapshot(f.book.connection()).unwrap());
                returned.push(e.message());
            }
        }
    }
    f.map(&ar,None).unwrap();
    returned
}

#[test]
fn r7_printable_unicode_secret_encodings_every_provider_path() {
    let pool:Vec<char>=(32u8..=126).map(char::from).chain(['é','中','😀','\u{301}','\u{2003}','\u{200d}']).collect();
    let mut checked=0;
    for (i,c) in pool.into_iter().enumerate() {
        let secret=format!("r7p{i:03}{c}tail-long");
        let set=SecretSet::masking_from_access_url(&format!("https://r7username-long:{}@r7.test/access",r7_pct(&secret)),Vec::<String>::new()).unwrap();
        let variants=r7_variants(&secret);
        let mut f=Fx::new();
        let mut needles=vec![secret.clone(),r7_pct(&secret)];
        for (j,v) in variants.iter().enumerate() {
            let text=format!("v{j} {v}");
            let sanitized=staging::provider_text(&set,&text);
            assert!(!r7_plain(&sanitized).contains(&r7_plain(v)),"helper case {i}/{j}");
            let returned=r7_drive(&mut f,j,&text,&set);
            needles.push(v.clone());
            for t in returned {for n in &needles {assert!(!r7_plain(&t).contains(&r7_plain(n)),"returned case {i}/{j}");}}
            checked+=15;
        }
        for t in r7_stored(&f) {for n in &needles {assert!(!r7_plain(&t).contains(&r7_plain(n)),"stored case {i}");}}
        f.reopen_clean();r5_finish(&mut f);
    }
    eprintln!("R7 independent encoding/provider checks={checked}");
}

#[test]
fn r7_html_leading_zero_entity_never_persists() {
    let mut f=Fx::new();
    let value="&#0000000000000000000000000000000000000114;7ownsecret-long";
    let s=SecretSet::masking_from_access_url("https://r7username-long:r7ownsecret-long@r7.test/access",Vec::<String>::new()).unwrap();
    let returned=r7_drive(&mut f,0,value,&s);
    let mut leaked=vec![];
    for (path,text) in r7_stored(&f).into_iter().map(|t|("stored",t)).chain(returned.into_iter().map(|t|("returned",t))) {
        if text.contains(value) {leaked.push(path)}
    }
    f.reopen_clean();r5_finish(&mut f);
    assert!(leaked.is_empty(),"known credential in leading-zero HTML entity persisted/returned: {leaked:?}");
}

/// R7's short password `q7!` persisted verbatim. FIX7 (lead decision): a credential component shorter
/// than MIN_CREDENTIAL_CHARS is refused when the set is built, so no entry point ever runs with it, and
/// the book is left as it was.
#[test]
fn r7_short_own_password_threat_model_limit() {
    let mut f=Fx::new();
    let before=fb_db::test_support::snapshot(f.book.connection()).unwrap();
    for url in ["https://r7username-long:q7!@r7.test/access","https://q7!:r7password-long@r7.test/access","https://r7username-long:%71%37%21%20%20%20%20%20%20%20%20%20@r7.test/access"] {
        let e=SecretSet::masking_from_access_url(url,Vec::<String>::new()).unwrap_err().to_string();
        assert!(e.contains(TOO_SHORT),"{e}");
    }
    let e=SecretSet::masking_from_access_url("https://r7username-long:r7password-long@r7.test/access",["q7!"]).unwrap_err().to_string();
    assert!(e.contains(TOO_SHORT),"{e}");
    let e=SecretSet::masking_from_credentials("r7username-long","q7!",Vec::<String>::new()).unwrap_err().to_string();
    assert!(e.contains(TOO_SHORT),"{e}");
    assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(),before);
    assert!(!r7_stored(&f).iter().any(|t|t.contains("q7!")),"book's known short password persisted in summary/audit");
    r5_finish(&mut f);
}

#[test]
fn r7_atomic_accept_fail_at_every_changed_write() {
    let mut reached=0;
    for kind in ["JOURNAL","CHECK","DEPOSIT","TRANSFER","MATCH"] {
        let (mut f,remote,input)=fix1_accept_case(kind);
        if kind=="CHECK" {r5_seed_check(&mut f,remote,&input);}
        let before=fb_db::test_support::snapshot(f.book.connection()).unwrap();
        f.accept(remote,input).unwrap();
        let after=fb_db::test_support::snapshot(f.book.connection()).unwrap();
        let mut boundaries=vec![];
        for (table,rows) in after {if before.get(&table)!=Some(&rows) {for op in ["INSERT","UPDATE","DELETE"] {boundaries.push(format!("AFTER {op} ON {table}"));}}}
        r5_finish(&mut f);
        for boundary in boundaries {
            let (mut f,remote,input)=fix1_accept_case(kind);
            if kind=="CHECK" {r5_seed_check(&mut f,remote,&input);}
            let before=fb_db::test_support::snapshot(f.book.connection()).unwrap();
            f.book.connection().execute_batch(&format!("CREATE TEMP TRIGGER r7_fail {boundary} BEGIN SELECT RAISE(FAIL,'r7_failure'); END;")).unwrap();
            let result=f.accept(remote,input.clone());
            f.book.connection().execute_batch("DROP TRIGGER temp.r7_fail").unwrap();
            if let Err(e)=result {assert!(e.to_string().contains("r7_failure"));assert_eq!(fb_db::test_support::snapshot(f.book.connection()).unwrap(),before,"{kind}/{boundary}");f.reopen_clean();f.accept(remote,input).unwrap();reached+=1;}
            f.reopen_clean();r5_finish(&mut f);
        }
    }
    assert!(reached>=25);eprintln!("R7 FAIL rollback boundaries={reached}");
}

#[test]
fn r7_reopen_after_feed_name_and_ledger_edits() {
    let mut f=Fx::new();f.connected_books();let mut count=0;
    for active in [false,true] {for postable in [false,true] {for kind in ["ASSET","LIABILITY","INCOME","EXPENSE","EQUITY"] {
        let code=format!("R7{count:03}");
        let id=services::save_account(&mut f.book,f.u,f.c,AccountInput::new(&code,"Unused mapped").kind("ASSET")).unwrap().id;
        let feed=f.feed("ACT-savings");map_account(&mut f.book,f.u,f.c,&feed,&json!(id.to_db()), &SecretSet::none()).unwrap();
        services::save_account(&mut f.book,f.u,f.c,AccountInput::new(&code,"Changed mapped").kind(kind).active(active).postable(postable).edit(id)).unwrap();
        f.reopen_clean();map_account(&mut f.book,f.u,f.c,&feed,&Value::Null, &SecretSet::none()).unwrap();f.reopen_clean();
        services::save_account(&mut f.book,f.u,f.c,AccountInput::new(&code,"Restored").kind("ASSET").edit(id)).unwrap();count+=1;
    }}}
    r5_finish(&mut f);eprintln!("R7 {count} account-edit combinations reopen");
}


#[test]
fn r7_supplement_entity_digit_boundaries_and_c1() {
    let known=SecretSet::masking_new(["r7ownsecret-long"]).unwrap();
    let mut failures=vec![];
    for zeros in (0..24).chain([40,100,1000]) {
        for radix in [10,16] {
            let first=if radix==10 {format!("&#{}114;","0".repeat(zeros))} else {format!("&#x{}72;","0".repeat(zeros))};
            let value=format!("{first}7ownsecret-long");
            if known.mask(&value)==value {failures.push(format!("radix{radix}/zeros{zeros}"));}
        }
    }
    let euro=SecretSet::masking_new(["r7€password-long"]).unwrap();
    let c1="r7&#128;password-long";
    eprintln!("R7 entity limits={failures:?}; HTML C1 remap leak={}",euro.mask(c1)==c1);
    assert!(failures.is_empty(),"documented leading-zero entities leak at {} boundaries",failures.len());
    assert_ne!(euro.mask(c1),c1,"HTML numeric C1 reference decodes to euro");
}

#[test]
fn r7_supplement_persisted_locations_and_hmac_raw_id() {
    let mut f=Fx::new();
    let value="&#0000000000000000000000000000000000000114;7ownsecret-long";
    let secrets=SecretSet::masking_from_access_url("https://r7username-long:r7ownsecret-long@r7.test/access",Vec::<String>::new()).unwrap();
    r7_drive(&mut f,0,value,&secrets);
    let mut locations=vec![];
    for (table,cols) in [
        ("bankfeeds_connection",vec!["host","last_sync_summary"]),
        ("bankfeeds_account",vec!["conn_key","remote_id","name","org_name","currency"]),
        ("bankfeeds_transaction",vec!["remote_id","description"]),
        ("ledger_journal",vec!["memo"]),
        ("ledger_journalline",vec!["description"]),
    ] {
        for col in cols {
            let n:i64=f.book.connection().query_row(&format!("SELECT count(*) FROM {table} WHERE instr({col},?1)>0"),[value],|r|r.get(0)).unwrap();
            if n>0 {locations.push(format!("{table}.{col}={n}"));}
        }
    }
    let mut st=f.book.connection().prepare("SELECT DISTINCT action FROM ledger_auditevent WHERE instr(detail,?1)>0 ORDER BY action").unwrap();
    let actions:Vec<String>=st.query_map([value],|r|r.get(0)).unwrap().map(Result::unwrap).collect();drop(st);
    let raw=format!("a0 {value}");
    let key:Vec<u8>=f.book.connection().query_row("SELECT secret FROM bankfeeds_identity_secret",[],|r|r.get(0)).unwrap();
    // FIX7: the name is stored masked now; the reviewer's lookup by the raw name relied on the leak
    let shown=staging::provider_text(&secrets,value);
    assert_eq!(shown,MASK);
    let stored:String=f.book.connection().query_row("SELECT remote_identity FROM bankfeeds_account WHERE name=?1",[&shown],|r|r.get(0)).unwrap();
    assert_eq!(stored,fb_db::bankfeeds_text::identity_key(&key,ACCOUNT_DOMAIN,&raw));
    assert_ne!(stored,fb_db::bankfeeds_text::identity_key(&key,ACCOUNT_DOMAIN,"a0 ***"));
    eprintln!("R7 known-credential leak locations={locations:?}; audit actions={actions:?}; HMAC uses raw ID=true");
    r5_finish(&mut f);
    assert!(locations.is_empty()&&actions.is_empty(),"known credential persists in {} columns and {} audit writers",locations.len(),actions.len());
}

#[test]
fn r7_supplement_basic_auth_and_mixed_encoding_fuzz() {
    let mut seed=0x71b4_8a2f_61d3_82acu64;
    let mut next=|| {seed^=seed<<13;seed^=seed>>7;seed^=seed<<17;seed};
    let pool:Vec<char>=(32u8..=126).map(char::from).chain(['é','€','中','😀','\u{301}','\u{2003}']).collect();
    let mut checked=0;
    for case in 0..64 {
        let user=format!("r7user{case:03}{}",(0..12).map(|_|pool[next() as usize%pool.len()]).collect::<String>());
        let pass=format!("r7pass{case:03}{}",(0..12).map(|_|pool[next() as usize%pool.len()]).collect::<String>());
        let pair=format!("{user}:{pass}");
        let set=SecretSet::masking_from_access_url(&format!("https://{}:{}@r7.test/access",r7_pct(&user),r7_pct(&pass)),Vec::<String>::new()).unwrap();
        let mut f=Fx::new();
        let mut values=vec![format!("Authorization: Basic {}",STANDARD.encode(&pair)),format!("Basic {}",URL_SAFE_NO_PAD.encode(&pair))];
        let mixed:String=pass.chars().map(|c|match next()%4 {
            0=>c.to_string(),1=>r7_pct(&c.to_string()),2=>c.encode_utf16(&mut [0;2]).iter().map(|u|format!("\\u{u:04x}")).collect(),_=>format!("&#{};",c as u32)
        }).collect();
        values.push(mixed);
        let with_gaps:Vec<String>=values.iter().map(|v|v.chars().map(|c|format!("{c}\u{2003}")).collect()).collect();
        values.extend(with_gaps);
        for (j,value) in values.iter().enumerate() {
            let masked=staging::provider_text(&set,value);
            assert!(!r7_plain(&masked).contains(&r7_plain(value)),"mixed helper case{case}/{j}");
            r7_drive(&mut f,j,value,&set);checked+=15;
        }
        for text in r7_stored(&f) {for value in &values {assert!(!r7_plain(&text).contains(&r7_plain(value)),"mixed stored case{case}");}}
        r5_finish(&mut f);
    }
    eprintln!("R7 independent Basic/mixed provider checks={checked}");
}

/// FIX7: 12,000 spellings from tools/r7_vectors.py, each confirmed by Python's own decoders
/// (html.unescape, urllib.parse.unquote, json.loads, base64) before it was kept: padded numeric
/// references, Windows-1252 C1 references, named references, percent-encoding, JSON escapes, base64
/// and base64url, alone and nested two deep. Each is masked here; with `FB_R7_FUZZ_OUT` set the masked
/// texts are written there, and tools/r7_verify.py decodes them with the same Python decoders to confirm
/// no secret is left.
#[test]
fn r7_independent_decoder_vectors() {
    let vectors: Vec<Value> = serde_json::from_str(include_str!("r7_vectors.json")).unwrap();
    assert!(vectors.len() >= 10_000);
    let mut masked = vec![];
    let mut missed = vec![];
    for (i, v) in vectors.iter().enumerate() {
        let (secret, text) = (v["secret"].as_str().unwrap(), v["text"].as_str().unwrap());
        let out = SecretSet::masking_new([secret]).unwrap().mask(text);
        if out == text || !out.contains(MASK) || out.contains(secret) {
            missed.push(format!("vector {i} {}: {secret:?} in {text:?} -> {out:?}", v["layers"]));
        }
        masked.push(out);
    }
    if let Ok(path) = std::env::var("FB_R7_FUZZ_OUT") {
        std::fs::write(path, serde_json::to_string(&masked).unwrap()).unwrap();
    }
    eprintln!("R7 independent-decoder vectors masked={}, missed={}", masked.len(), missed.len());
    assert!(missed.is_empty(), "{} vectors not masked; first: {:?}", missed.len(), &missed[..missed.len().min(3)]);
}
