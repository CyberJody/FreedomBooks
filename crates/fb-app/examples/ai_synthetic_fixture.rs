//! Isolated integration fixture generator, never used by the product. Fixed key, synthetic data only.
#[allow(dead_code)]
#[path = "../tests/ai_support/mod.rs"]
mod ai_support;
fn main() {
    let destination =
        std::path::PathBuf::from(std::env::args().nth(1).expect("isolated output directory"));
    std::fs::create_dir_all(&destination).unwrap();
    let builtin = std::env::args().nth(2).as_deref() == Some("--builtin");
    let mut f = ai_support::Fixture::with_builtin_book(builtin);
    let key_hex = if builtin { fb_app::keys::to_hex(&fb_core::backups::portable::BUILTIN_KEY) } else { "2a".repeat(32) };
    let mut manifest = serde_json::json!({"company_id":f.company.to_db(),"other_company_id":f.other.to_db(),"bank":f.bank,"income":f.income,"expense":f.expense,"username":"Synthetic operator","user_password":ai_support::PASSWORD,"key_hex":key_hex,"builtin":builtin,"book":destination.join("synthetic.fbbook")});
    manifest["device_hex"] = serde_json::json!(fb_app::keys::to_hex(&ai_support::DEVICE_KEY));
    manifest["bank_secret"] = serde_json::json!(ai_support::BANK_SECRET);
    if std::env::args().any(|a| a == "--reconnect") {
        let (old, savings) = reconnect(&mut f);
        manifest["reconnect_old"] = serde_json::json!(old);
        manifest["savings"] = serde_json::json!(savings);
    }
    if let Some(path) = std::env::args().skip_while(|a| a != "--sync-feed").nth(1) {
        manifest["sync"] = sync_fixture(&mut f, &path);
    }
    f.app.lock();
    std::fs::copy(&f.path, destination.join("synthetic.fbbook")).unwrap();
    if builtin {
        std::fs::copy(fb_app::protection::record_path(&f.path),fb_app::protection::record_path(&destination.join("synthetic.fbbook"))).unwrap();
    }
    std::fs::write(destination.join("fixture.json"), manifest.to_string()).unwrap();
    println!("Created isolated synthetic AI fixture.");
}

/// `--reconnect`: a replaced bank connection on a second (synthetic savings) account. The old connection's row
/// was accepted; the account was then reconnected and the new connection's copy linked to it ("Link to
/// existing"). Returns the old row's number, whose Undo the review smoke checks is refused, and the savings
/// account.
fn reconnect(f: &mut ai_support::Fixture) -> (i64, String) {
    use fb_core::{bankfeeds as feeds, services};
    use serde_json::json;
    let (company, expense) = (f.company, f.expense.clone());
    let o = f.app.open_mut().unwrap();
    let none = feeds::SecretSet::none();
    let savings = services::save_account(&mut o.book, o.owner, company, services::AccountInput::new("101", "Savings").kind("ASSET").detail("BANK")).unwrap().id.to_db();
    let pull: feeds::StagedPull = serde_json::from_value(json!({"accounts":[{"conn_key":"synthetic-savings","remote_id":"synthetic-savings","name":"Synthetic Savings","org_name":"Synthetic bank","currency":"USD","balance":5000,"available":null,"balance_epoch":1793534400,"transactions":[{"remote_id":"synthetic-s1","posted_on":"2026-10-30","transacted_on":null,"amount":-2500,"description":"Synthetic reconnect fee","pending":false}]}]})).unwrap();
    let connect = |o: &mut fb_app::session::Open, label: &str| -> (String, String, fb_core::bankfeeds::Item) {
        let conn = feeds::connect(&mut o.book, o.owner, company, &json!(label), "bridge.simplefin.org", &none).unwrap()["id"].as_str().unwrap().to_owned();
        feeds::store_pull(&mut o.book, o.owner, company, &conn, &pull, "2026-11-01T12:00:00Z", false, None, &none).unwrap();
        let item = feeds::items(&mut o.book, o.owner, company).unwrap().into_iter().find(|i| i.description == "Synthetic reconnect fee" && i.status == "NEW").unwrap();
        let feed = item.feed_account_id.clone();
        feeds::map_account(&mut o.book, o.owner, company, &feed, &json!(savings), &none).unwrap();
        (conn, feed, item)
    };
    let (old, old_feed, earlier) = connect(o, "Old savings feed");
    feeds::accept(&mut o.book, o.owner, company, &earlier.id, &json!({"kind":"JOURNAL","account_id":expense}), &none).unwrap();
    feeds::map_account(&mut o.book, o.owner, company, &old_feed, &serde_json::Value::Null, &none).unwrap();
    feeds::forget(&mut o.book, o.owner, company, &old, &none).unwrap();
    let (_, _, again) = connect(o, "New savings feed");
    feeds::link_existing(&mut o.book, o.owner, company, &again.id, Some(&earlier.id), &none).unwrap();
    (earlier.ordinal, savings)
}

/// Additional accounting setup and downloaded lines for the CyberBrain integration check.
fn sync_fixture(f: &mut ai_support::Fixture, path: &str) -> serde_json::Value {
    use fb_core::{bankfeeds as feeds, services};
    use serde_json::json;

    let co = f.company;
    let o = f.app.open_mut().unwrap();
    let mut account = |code, name, kind, detail| {
        services::save_account(
            &mut o.book,
            o.owner,
            co,
            services::AccountInput::new(code, name).kind(kind).detail(detail),
        )
        .unwrap()
        .id
        .to_db()
    };
    let checking = account("102", "Synthetic Checking 1234", "ASSET", "BANK");
    let savings = account("103", "Synthetic Savings 5678", "ASSET", "BANK");
    let draws = account("3200", "Owner Draw", "EQUITY", "EQUITY");
    let tax = account("2100", "Collected Sales Tax", "LIABILITY", "OTHER_CURRENT_LIABILITY");
    let mut setup = o.book.read(|q| fb_core::documents::setup_snapshot(q, co)).unwrap();
    setup["collected_tax"] = json!(tax);
    setup["banks"] = json!([f.bank, checking, savings]);
    fb_core::documents::save_setup(&mut o.book, o.owner, co, &setup, setup["revision"].as_i64())
        .unwrap();

    let none = feeds::SecretSet::none();
    let pull: feeds::StagedPull = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let conn = feeds::connect(
        &mut o.book, o.owner, co, &json!("Synthetic sync feed"), "bridge.simplefin.org", &none,
    )
    .unwrap();
    feeds::store_pull(
        &mut o.book, o.owner, co, conn["id"].as_str().unwrap(), &pull,
        "2026-11-20T12:00:00Z", false, None, &none,
    )
    .unwrap();
    let items = feeds::items(&mut o.book, o.owner, co).unwrap();
    for (remote, account) in [("sync-checking", &checking), ("sync-savings", &savings)] {
        let row = pull.accounts.iter().find(|a| a.remote_id == remote).unwrap();
        let item = items.iter().find(|i| i.remote_id == row.transactions[0].remote_id).unwrap();
        feeds::map_account(&mut o.book, o.owner, co, &item.feed_account_id, &json!(account), &none)
            .unwrap();
    }
    let ids: std::collections::BTreeMap<_, _> =
        items.iter().map(|i| (&i.remote_id, &i.id)).collect();
    json!({"checking":checking,"savings":savings,"tax":tax,"draws":draws,"feed_ids":ids})
}
