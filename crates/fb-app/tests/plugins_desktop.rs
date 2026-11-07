//! WAVE4 item 3/5: the plugin host wiring through the real fb-net process (built with its
//! `fake-network` feature) against numeric-loopback fakes only: no real network, sign-in or bank.
//!
//! - SimpleFIN: connect (claim) → pull stages lines (the review list shows them) → accept one →
//!   lock and reopen: mask restored (writes allowed), no credential spelling in any stored text.
//! - ChatGPT: start → the browser approves at the fake provider and hits fb-net's loopback →
//!   poll → connected with the plan ("Using ChatGPT plan" state) → disconnect wipes the vault row.
//! - The R11 vectors through the same desktop connect → pull → accept path, every stored TEXT dumped
//!   for R11's independent stdlib decoders (scored by `staging/wave4-fix1/net/check_leak.py`).
use base64::{Engine, engine::general_purpose::STANDARD};
use fb_app::keys::MasterKey;
use fb_app::net::ProcessNet;
use fb_app::{App, plugins};
use fb_core::bankfeeds::{self, SecretSet};
use fb_core::services::{self, AccountInput};
use fb_core::{AccountId, CompanyId};
use fb_plugin_api::SecretString;
use fb_plugin_testkit::browser_get;
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

mod fakes;
use fakes::*;

/// R11 leak vectors (`FB_R11_VECTORS`) and where the scored output goes (`FB_R11_OUT`, default under
/// the test target dir). Nothing outside the build tree is written unless asked for.
fn r11_paths() -> Option<(PathBuf, PathBuf)> {
    let vectors = PathBuf::from(std::env::var_os("FB_R11_VECTORS")?);
    let out = std::env::var_os("FB_R11_OUT").map(PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_TARGET_TMPDIR")).join("desktop-leak"));
    Some((vectors, out))
}

/// The fake-network fb-net: `FB_NET_TEST_BIN`, else built once into its own target dir (the
/// production build of the workspace never gets the feature).
fn fb_net() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        if let Some(bin) = std::env::var_os("FB_NET_TEST_BIN") {
            return PathBuf::from(bin);
        }
        let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("fb-net-fake");
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let status = std::process::Command::new(cargo)
            .current_dir(&root)
            .args(["build", "--locked", "-p", "fb-net", "--features", "fake-network", "--target-dir"])
            .arg(&target)
            .status()
            .expect("cargo runs");
        assert!(status.success(), "building fb-net with fake-network failed");
        target.join("debug").join(if cfg!(windows) { "fb-net.exe" } else { "fb-net" })
    })
    .clone()
}

struct Desk {
    app: App,
    path: PathBuf,
    key: MasterKey,
    co: CompanyId,
    bank: AccountId,
    expense: AccountId,
    _dir: tempfile::TempDir,
}
fn desk(net: ProcessNet) -> Desk {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desk.fbbook");
    let key = MasterKey::new([0x5a; 32]);
    let mut app = App::new();
    app.plugins = true;
    app.os_store_usable = true;
    let co = app.create_book(&path, &key, "Synthetic owner", "Synthetic company").unwrap().company_id;
    let (bank, expense) = {
        let open = app.open_mut().unwrap();
        let bank = services::save_account(&mut open.book, open.owner, co, AccountInput::new("9100", "Feed checking").kind("ASSET").detail("BANK")).unwrap().id;
        let expense = services::save_account(&mut open.book, open.owner, co, AccountInput::new("9500", "Feed expenses").kind("EXPENSE")).unwrap().id;
        (bank, expense)
    };
    app.net = Some(Box::new(net));
    Desk { app, path, key, co, bank, expense, _dir: dir }
}

fn texts(app: &mut App) -> Vec<(String, String)> {
    let book = &app.open_mut().unwrap().book;
    fb_db::test_support::snapshot(book.connection())
        .unwrap()
        .into_iter()
        .flat_map(|(table, rows)| {
            rows.into_iter().flatten().filter_map(move |v| match v {
                fb_db::rusqlite::types::Value::Text(s) => Some((table.clone(), s)),
                _ => None,
            })
        })
        .collect()
}
fn envelope(app: &mut App, co: CompanyId, plugin: &str, connection: &str) -> bool {
    let book = &mut app.open_mut().unwrap().book;
    book.credential_envelope(&co.to_db(), plugin, connection).unwrap().is_some()
}
fn map_and_accept(d: &mut Desk, payee: &str) -> Result<bankfeeds::Item, String> {
    let open = d.app.open_mut().unwrap();
    let (owner, co) = (open.owner, d.co);
    let items = bankfeeds::items(&mut open.book, owner, co).map_err(|e| e.message())?;
    let feed = items.first().ok_or("no staged lines")?.feed_account_id.clone();
    bankfeeds::map_account(&mut open.book, owner, co, &feed, &json!(d.bank.to_db()), &SecretSet::none()).map_err(|e| e.message())?;
    let line = items.iter().find(|i| i.status == "NEW" && !i.pending).ok_or("no line to accept")?.id.clone();
    bankfeeds::accept(&mut open.book, owner, co, &line, &json!({"kind":"JOURNAL","account_id":d.expense.to_db(),"payee_name":payee}), &SecretSet::none())
        .map_err(|e| e.message())
}

const USER: &str = "SynthUserQ7x2Kd9Lm4Rt";
const PASS: &str = "SynthPassZ8vN3bW6yHc1Jd5Fs";

#[test]
fn simplefin_connect_pull_accept_reopen_through_fb_net() {
    let bridge = FakeBridge::start(USER, PASS, sample_accounts());
    let mut d = desk(ProcessNet::new(fb_net()).with_fake("FB_NET_FAKE_SIMPLEFIN_PORT", &bridge.port()));
    let co = d.co;

    // A label the core would refuse never spends the one-time token.
    let long = "L".repeat(121);
    assert_eq!(plugins::bankfeed_connect(&mut d.app, co, &bridge.setup_token(), &long).unwrap(), Err("Enter text up to 120 characters.".into()));
    assert!(bridge.state.lock().unwrap().requests.is_empty());

    let connection = plugins::bankfeed_connect(&mut d.app, co, &bridge.setup_token(), "Synthetic feed").unwrap().expect("connected");
    assert!(envelope(&mut d.app, co, "simplefin", &connection), "the vault holds the claimed credential");
    // The same token again is refused by the bridge (already claimed): the plugin's fixed sentence.
    let again = plugins::bankfeed_connect(&mut d.app, co, &bridge.setup_token(), "Again").unwrap();
    assert_eq!(again, Err("The setup token was refused. Obtain a new token from the provider.".into()));

    let summary = plugins::bankfeed_sync(&mut d.app, co, &connection).unwrap().expect("pulled");
    assert_eq!(summary, "1 accounts, 2 new transactions");
    // The manual minimum interval (Python MANUAL_MIN_INTERVAL) refuses an immediate second pull.
    let soon = plugins::bankfeed_sync(&mut d.app, co, &connection).unwrap().unwrap_err();
    assert!(soon.starts_with("Synced 0 min ago. SimpleFIN asks apps not to pull too often; try again in 10 min."), "{soon}");
    let requests = bridge.state.lock().unwrap().requests.clone();
    assert_eq!(requests, ["POST /claim/SYNTHETIC-ONE-TIME-CLAIM", "POST /claim/SYNTHETIC-ONE-TIME-CLAIM", "GET /simplefin/accounts"]);

    let accepted = map_and_accept(&mut d, "Fuel stop").expect("accepted");
    assert_eq!(accepted.status, "ACCEPTED");
    let items = {
        let open = d.app.open_mut().unwrap();
        bankfeeds::items(&mut open.book, open.owner, co).unwrap()
    };
    assert_eq!(items.len(), 2, "the review list shows both pulled lines");
    assert!(items.iter().any(|i| i.posted_on.is_some() && i.description == "FUEL STOP"));

    d.app.lock();
    d.app.open_book(&d.path, &d.key).unwrap();
    assert!(d.app.messages.is_empty(), "no reconnect notice: {:?}", d.app.messages);
    // Mask restored: a write through the bank-feed services is allowed after reopen.
    let other = items.iter().find(|i| i.status == "NEW").unwrap().id.clone();
    {
        let open = d.app.open_mut().unwrap();
        bankfeeds::ignore(&mut open.book, open.owner, co, &other, &SecretSet::none()).expect("write after reopen");
    }
    let enc = STANDARD.encode(format!("{USER}:{PASS}"));
    for (table, text) in texts(&mut d.app) {
        for needle in [USER, PASS, enc.as_str(), "SYNTHETIC-ONE-TIME-CLAIM"] {
            assert!(!text.contains(needle), "{table} stores a credential spelling");
        }
    }
    let raw = std::fs::read(&d.path).unwrap();
    assert!(!raw.windows(PASS.len()).any(|w| w == PASS.as_bytes()));

    // Forget: the core deactivates and the vault row is wiped; a pull is then refused locally.
    plugins::bankfeed_forget(&mut d.app, co, &connection).unwrap();
    assert!(!envelope(&mut d.app, co, "simplefin", &connection));
    let refused = plugins::bankfeed_sync(&mut d.app, co, &connection).unwrap().unwrap_err();
    assert_eq!(refused, "This connection was forgotten. Connect again with a new setup token.");
    println!("SIMPLEFIN DESKTOP OK: connect, pull ({summary}), accept, reopen with mask, forget");
}

#[test]
fn needs_reconnect_connection_is_never_pulled() {
    let bridge = FakeBridge::start(USER, PASS, sample_accounts());
    let mut d = desk(ProcessNet::new(fb_net()).with_fake("FB_NET_FAKE_SIMPLEFIN_PORT", &bridge.port()));
    let co = d.co;
    let connection = plugins::bankfeed_connect(&mut d.app, co, &bridge.setup_token(), "Feed").unwrap().unwrap();
    // Damage the stored envelope: the next open flags the connection "reconnect needed".
    {
        let book = &mut d.app.open_mut().unwrap().book;
        book.delete_credential_envelope(&co.to_db(), "simplefin", &connection).unwrap();
    }
    d.app.lock();
    d.app.open_book(&d.path, &d.key).unwrap();
    assert_eq!(d.app.messages.len(), 1, "{:?}", d.app.messages);
    let refused = plugins::bankfeed_sync(&mut d.app, co, &connection).unwrap().unwrap_err();
    assert_eq!(refused, bankfeeds::UNSAFE_CREDENTIAL);
    assert_eq!(bridge.state.lock().unwrap().requests, ["POST /claim/SYNTHETIC-ONE-TIME-CLAIM"], "no pull reached the bridge");
}

#[test]
fn chatgpt_failed_callback_is_audited_and_connects_nothing() {
    let (server, _provider) = fake_openai();
    let mut d = desk(ProcessNet::new(fb_net()).with_fake("FB_NET_FAKE_OPENAI", &server.url("")));
    let co = d.co;
    let url = plugins::chatgpt_start(&mut d.app, co).unwrap().expect("authorization url");
    let callback = browser_get(&SecretString::from(url)).unwrap().location.unwrap();
    // A reply whose state differs from the one started here aborts the sign-in (F-L8 keeps only
    // stray no-state / wrong-path requests from consuming the listener).
    let mut tampered = url::Url::parse(fb_plugin_api::ExposeSecret::expose_secret(&callback)).unwrap();
    let pairs: Vec<(String, String)> = tampered.query_pairs().map(|(k, v)| (k.to_string(), if k == "state" { "forged-state".into() } else { v.into_owned() })).collect();
    tampered.query_pairs_mut().clear().extend_pairs(pairs);
    let _ = browser_get(&SecretString::from(tampered.to_string()));
    let deadline = Instant::now() + Duration::from_secs(30);
    let finished = loop {
        if let Some(done) = plugins::chatgpt_poll(&mut d.app, co).unwrap() {
            break done;
        }
        assert!(Instant::now() < deadline, "the sign-in never finished");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(finished, Err("The sign-in response did not match this sign-in attempt.".into()));
    assert!(!envelope(&mut d.app, co, "openai", &co.to_db()));
    let open = d.app.open_mut().unwrap();
    let audits: Vec<(String, String)> = open.book.read(|q| {
        let mut st = q.prepare("SELECT action, detail FROM ledger_auditevent WHERE action LIKE 'ai.%' ORDER BY occurred_at, id")?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()
    }).unwrap();
    assert_eq!(audits.iter().map(|a| a.0.as_str()).collect::<Vec<_>>(), ["ai.signin_started", "ai.signin_failed"]);
    assert!(audits[1].1.contains("state_mismatch"), "{}", audits[1].1);
    assert!(fb_core::assistant::connection_for(&mut open.book, open.owner, co).unwrap().is_none_or(|r| r["status"] != "connected"));
}

#[test]
fn without_the_helper_plugins_say_so() {
    let dir = tempfile::tempdir().unwrap();
    let key = MasterKey::new([0x11; 32]);
    let mut app = App::new();
    app.plugins = true;
    app.os_store_usable = true;
    let co = app.create_book(&dir.path().join("b.fbbook"), &key, "Owner", "Co").unwrap().company_id;
    let unavailable = Err("Bank feeds and ChatGPT aren't available in this build.".to_owned());
    assert_eq!(plugins::bankfeed_connect(&mut app, co, "token", "Feed").unwrap(), unavailable);
    assert_eq!(plugins::chatgpt_start(&mut app, co).unwrap(), unavailable);
    assert_eq!(plugins::chatgpt_poll(&mut app, co).unwrap(), None);
    // A configured but missing helper says the same, and spends nothing.
    app.net = Some(Box::new(ProcessNet::new(dir.path().join("no-such-fb-net"))));
    assert_eq!(plugins::bankfeed_connect(&mut app, co, "token", "Feed").unwrap(), unavailable);
    assert_eq!(plugins::chatgpt_start(&mut app, co).unwrap(), unavailable);
    let open = app.open_mut().unwrap();
    let feeds: i64 = open.book.read(|q| q.query_row("SELECT count(*) FROM bankfeeds_connection", [], |r| r.get(0))).unwrap();
    assert_eq!(feeds, 0);
}

#[test]
fn chatgpt_sign_in_and_disconnect_through_fb_net() {
    let (server, provider) = fake_openai();
    let mut d = desk(ProcessNet::new(fb_net()).with_fake("FB_NET_FAKE_OPENAI", &server.url("")));
    let co = d.co;
    let company = co.to_db();
    let url = plugins::chatgpt_start(&mut d.app, co).unwrap().expect("authorization url");
    assert!(url.starts_with(&server.url("/authorize?")), "the system browser goes to the provider");
    assert!(!url.contains("id_token_hint"));
    assert_eq!(plugins::chatgpt_poll(&mut d.app, co).unwrap(), None, "still waiting for the browser");

    // The system browser: approve at the provider, follow its redirect to fb-net's loopback.
    let callback = browser_get(&SecretString::from(url)).unwrap().location.expect("redirect to the loopback");
    let landed = browser_get(&callback).unwrap();
    assert_eq!(landed.status, 200);
    let deadline = Instant::now() + Duration::from_secs(30);
    let finished = loop {
        if let Some(done) = plugins::chatgpt_poll(&mut d.app, co).unwrap() {
            break done;
        }
        assert!(Instant::now() < deadline, "the sign-in never finished");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(finished, Ok(()));
    let row = {
        let open = d.app.open_mut().unwrap();
        fb_core::assistant::connection_for(&mut open.book, open.owner, co).unwrap().unwrap()
    };
    assert_eq!((row["mode"].as_str(), row["status"].as_str(), row["plan_usage"].as_bool()), (Some("siwc"), Some("connected"), Some(true)), "Using ChatGPT plan");
    assert_eq!(row["account_email"], "synthetic@example.invalid");
    assert_eq!(row["model"], "synthetic-model");
    assert!(envelope(&mut d.app, co, "openai", &company));
    for (table, text) in texts(&mut d.app) {
        assert!(!text.contains("FAKE_REFRESH_TOKEN") && !text.contains("FAKE_ACCESS_TOKEN"), "{table} stores a token");
    }

    // A reopen keeps the sign-in (vault + row); disconnect revokes and wipes the vault row.
    d.app.lock();
    d.app.open_book(&d.path, &d.key).unwrap();
    assert_eq!(plugins::chatgpt_disconnect(&mut d.app, co).unwrap(), Ok(()));
    assert!(!envelope(&mut d.app, co, "openai", &company), "disconnect wipes the vault row");
    assert_eq!(provider.lock().unwrap().revoked, 1);
    let row = {
        let open = d.app.open_mut().unwrap();
        fb_core::assistant::connection_for(&mut open.book, open.owner, co).unwrap().unwrap()
    };
    assert_eq!((row["status"].as_str(), row["credential_present"].as_bool(), row["revoke_confirmed"].as_bool()), (Some("disconnected"), Some(false), Some(true)));
    assert_eq!(plugins::chatgpt_disconnect(&mut d.app, co).unwrap(), Err("Nothing is connected.".into()));
    let actions: Vec<String> = {
        let book = &mut d.app.open_mut().unwrap().book;
        book.read(|q| {
            let mut st = q.prepare("SELECT action FROM ledger_auditevent WHERE action LIKE 'ai.%' ORDER BY occurred_at, id")?;
            st.query_map([], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()
        })
        .unwrap()
    };
    assert_eq!(actions, ["ai.signin_started", "ai.connected", "ai.disconnected"]);
    println!("CHATGPT DESKTOP OK: start, browser callback, poll -> connected (plan), reopen, disconnect wiped the vault");
}

// --- R11 vectors through the desktop path ----------------------------------------------------------

fn leaves(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| leaves(x, out)),
        Value::Object(o) => o.values().for_each(|x| leaves(x, out)),
        _ => {}
    }
}

/// Provider text embedding every encoding of the case's fields: names, ids, descriptions, the
/// errlist message and a skipped transaction id.
fn vector_accounts(fields: &Value) -> Value {
    let f = |k: &str| fields[k].as_str().unwrap_or("").to_owned();
    let posted = now() - 2 * 86_400;
    json!({"errlist":[{"code":"con.synthetic","msg":f("errors"),"conn_id":f("host")}],
        "connections":[{"conn_id":"CON-V","org_name":f("host"),"name":f("co_name")}],
        "accounts":[{"id":f("remote"),"name":f("conn_label"),"conn_id":"CON-V","currency":"USD","balance":"100.00","balance-date":posted,
            "transactions":[
                {"id":f("sync_error"),"posted":posted,"amount":"-12.34","description":f("staged")},
                {"id":"TX-V-2","posted":posted,"amount":"5.00","description":f("payee_j")},
                {"id":f("skipped"),"posted":posted,"description":f("vendor_name")}]}]})
}

#[test]
fn r11_vectors_through_desktop_connect_pull_accept() {
    let Some((vectors, scratch)) = r11_paths() else {
        eprintln!("SKIPPED: set FB_R11_VECTORS to run the R11 desktop leak vectors");
        return;
    };
    let raw = std::fs::read_to_string(&vectors).expect("FB_R11_VECTORS is readable");
    let cases: Vec<Value> = serde_json::from_str(&raw).unwrap();
    std::fs::create_dir_all(&scratch).unwrap();
    let mut out = std::fs::File::create(scratch.join("desktop-leak.jsonl")).unwrap();
    let bridge = FakeBridge::start("placeholder-user-0000", "placeholder-pass-0000", json!({"accounts":[]}));
    let mut skipped = vec![];
    let mut runs = 0;
    for case in &cases {
        let id = case["id"].as_str().unwrap();
        let (user, pass) = (case["user"].as_str().unwrap(), case["pass"].as_str().unwrap());
        if SecretSet::from_access_url(&format!("https://{}:{}@bridge.example/simplefin", user.replace('/', "%2F"), pass.replace('/', "%2F")), Vec::<String>::new()).is_err() {
            skipped.push(id.to_owned());
            continue;
        }
        bridge.reset(user, pass, vector_accounts(&case["fields"]));
        let mut d = desk(ProcessNet::new(fb_net()).with_fake("FB_NET_FAKE_SIMPLEFIN_PORT", &bridge.port()));
        let co = d.co;
        let mut steps = vec![];
        let mut returned = vec![];
        let label = case["fields"]["conn_label"].as_str().unwrap_or("");
        let connection = match plugins::bankfeed_connect(&mut d.app, co, &bridge.setup_token(), label).unwrap() {
            Ok(c) => {
                steps.push(json!(["connect", "ok"]));
                c
            }
            Err(message) => {
                steps.push(json!(["connect", "invalid"]));
                returned.push(message);
                bridge.reset(user, pass, vector_accounts(&case["fields"]));
                let c = plugins::bankfeed_connect(&mut d.app, co, &bridge.setup_token(), "Vector feed").unwrap().expect("connect with a plain label");
                steps.push(json!(["connect_plain", "ok"]));
                c
            }
        };
        match plugins::bankfeed_sync(&mut d.app, co, &connection).unwrap() {
            Ok(summary) => {
                steps.push(json!(["pull", "ok"]));
                returned.push(summary);
            }
            Err(message) => {
                steps.push(json!(["pull", "invalid"]));
                returned.push(message);
            }
        }
        let payee = case["fields"]["payee_c"].as_str().unwrap_or("");
        match map_and_accept(&mut d, payee) {
            Ok(item) => {
                steps.push(json!(["accept", "ok"]));
                returned.push(item.description);
            }
            Err(message) => {
                steps.push(json!(["accept", "invalid"]));
                returned.push(message);
            }
        }
        d.app.lock();
        d.app.open_book(&d.path, &d.key).unwrap();
        let reconnect = !d.app.messages.is_empty();
        let stored = texts(&mut d.app);
        d.app.lock();
        let raw = std::fs::read(&d.path).unwrap();
        let raw_hit = raw.windows(pass.len()).any(|w| w == pass.as_bytes()) || raw.windows(user.len()).any(|w| w == user.as_bytes());
        let mut values = vec![];
        leaves(&case["fields"], &mut values);
        writeln!(out, "{}", json!({"id": id, "mode": "desktop-net", "texts": stored, "returned": returned, "refused_rollback": [], "steps": steps, "raw_file_hit": raw_hit, "reconnect_on_reopen": reconnect})).unwrap();
        runs += 1;
    }
    std::fs::write(scratch.join("desktop-leak-skipped.json"), json!(skipped).to_string()).unwrap();
    println!("R11 DESKTOP-NET RUNS: {runs} run, {} skipped (credential refused by SecretSet)", skipped.len());
    assert!(runs > 0);
}

#[test]
fn simplefin_requires_os_sealing_before_claim_or_sync_and_preserves_saved_credentials() {
    let bridge = FakeBridge::start(USER, PASS, sample_accounts());
    let mut d = desk(ProcessNet::new(fb_net()).with_fake("FB_NET_FAKE_SIMPLEFIN_PORT", &bridge.port()));
    let co = d.co;
    d.app.os_store_usable = false;
    let before = texts(&mut d.app);
    let error = plugins::bankfeed_connect(&mut d.app, co, &bridge.setup_token(), "Synthetic feed").unwrap().unwrap_err();
    assert!(error.contains("OS keyring"), "{error}");
    assert!(bridge.state.lock().unwrap().requests.is_empty(), "must not spend the setup token");
    assert_eq!(texts(&mut d.app), before, "refusal must not change accounting or credential metadata");
    let count: i64 = d.app.open_mut().unwrap().book.read(|q| q.query_row("SELECT count(*) FROM plugin_credential", [], |r| r.get(0))).unwrap();
    assert_eq!(count, 0);

    d.app.os_store_usable = true;
    let connection = plugins::bankfeed_connect(&mut d.app, co, &bridge.setup_token(), "Synthetic feed").unwrap().unwrap();
    assert!(envelope(&mut d.app, co, "simplefin", &connection));
    let requests = bridge.state.lock().unwrap().requests.clone();
    d.app.os_store_usable = false;
    d.app.lock();
    d.app.open_book(&d.path, &d.key).unwrap();
    let before = texts(&mut d.app);
    let error = plugins::bankfeed_sync(&mut d.app, co, &connection).unwrap().unwrap_err();
    assert!(error.contains("OS keyring"), "{error}");
    assert_eq!(bridge.state.lock().unwrap().requests, requests);
    assert_eq!(texts(&mut d.app), before);
    assert!(envelope(&mut d.app, co, "simplefin", &connection), "keep the saved connection intact");
    d.app.os_store_usable = true;
    assert_eq!(plugins::bankfeed_sync(&mut d.app, co, &connection).unwrap().unwrap(), "1 accounts, 2 new transactions");
}
