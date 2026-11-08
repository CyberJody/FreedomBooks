//! D9 bank-feed screens on an encrypted synthetic book: the Rust-only reconnect state and desktop
//! messages, invalid POSTs that write nothing, and a rule saved through the narrowed SubmitForm path.
mod support;
use fb_app::{App, Command, Outcome, keys::MasterKey, screens};
use fb_core::{CompanyId, bankfeeds, services::{self, AccountInput}};
use serde_json::json;

fn account(app: &mut App, co: CompanyId, code: &str, name: &str, detail: &str) -> String {
    let o = app.open_mut().unwrap();
    services::save_account(&mut o.book, o.owner, co, AccountInput {
        code: code.into(), name: name.into(), kind: None, detail_type: Some(detail.into()), is_contra: None,
        description: None, active: true, postable: true, account: None,
    }).unwrap().id.to_db()
}
fn audit(app: &mut App) -> i64 {
    app.open_mut().unwrap().book.read(|q| q.query_row("SELECT count(*) FROM ledger_auditevent", [], |r| r.get(0))).unwrap()
}
fn submit(app: &mut App, path: &str, fields: &[(&str, &str)]) -> Outcome {
    let _ = screens::render(app, path).unwrap();
    let token = app.tokens.mint(path);
    app.run(Command::SubmitForm { path: path.into(), token, fields: fields.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }).unwrap()
}
fn html(o: Outcome) -> (String, u16) {
    match o {
        Outcome::Invalid { html } => (html, 400),
        Outcome::Page { html, status } => (html, status),
        other => panic!("expected a page, got {other:?}"),
    }
}

fn book() -> (tempfile::TempDir, std::path::PathBuf, MasterKey, App, CompanyId) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("d9.fbbook");
    let key = MasterKey::new([29; 32]);
    let mut app = App::new();
    app.plugins = true;
    app.os_store_usable = true;
    let co = app.create_book(&path, &key, "D9 operator", "D9 synthetic feeds").unwrap().company_id;
    support::sign_in(&mut app);
    (dir, path, key, app, co)
}

#[test]
fn a_connection_whose_sign_in_is_unreadable_offers_forget_and_connect_again_never_sync() {
    let (_dir, path, key, mut app, co) = book();
    {
        let o = app.open_mut().unwrap();
        // No saved credential envelope: the next open flags it "reconnect needed".
        bankfeeds::connect(&mut o.book, o.owner, co, &json!("Checking feed"), "fixture.test", &bankfeeds::SecretSet::none()).unwrap();
    }
    app.lock();
    app.open_book(&path, &key).unwrap();
    support::sign_in(&mut app);
    assert_eq!(app.messages.len(), 1);
    let list = format!("/c/{}/bank-feeds/", co.to_db());
    let page = screens::render(&mut app, &list).unwrap();
    assert_eq!(page.status, 200);
    assert!(page.html.contains("Reconnect needed"), "{}", page.html);
    assert!(page.html.contains(&format!(r#"<a href="/c/{}/bank-feeds/connect/">Connect again</a>"#, co.to_db())));
    assert!(page.html.contains(">Forget</a>"));
    assert!(!page.html.contains("Sync now"));
    // The desktop message shows once, as a plain notice above the page content.
    assert!(page.html.contains(r#"<p class="notice">The saved sign-in for Checking feed couldn&#x27;t be read."#), "{}", page.html);
    assert!(app.messages.is_empty());
    let again = screens::render(&mut app, &list).unwrap();
    assert!(!again.html.contains(r#"<p class="notice">"#));
    // Nor does its accounts page offer Sync.
    let id: String = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT id FROM bankfeeds_connection", [], |r| r.get(0))).unwrap();
    let accounts = screens::render(&mut app, &format!("{list}{id}/accounts/")).unwrap();
    assert!(accounts.html.contains("Bank feed accounts") && !accounts.html.contains("/sync/") && !accounts.html.contains(">Sync now</button>"));
}

#[test]
fn the_sync_form_carries_a_token_for_its_own_action_and_shows_the_plugin_refusal_as_a_notice() {
    let (_dir, _path, _key, mut app, co) = book();
    let id = {
        let o = app.open_mut().unwrap();
        bankfeeds::connect(&mut o.book, o.owner, co, &json!("Synthetic feed"), "fixture.test", &bankfeeds::SecretSet::none()).unwrap()["id"].as_str().unwrap().to_owned()
    };
    let sync = format!("/c/{}/bank-feeds/{id}/sync/", co.to_db());
    let page = screens::render(&mut app, &format!("/c/{}/bank-feeds/", co.to_db())).unwrap().html;
    let marker = format!(r#"<form method="post" action="{sync}"><input type="hidden" name="fb_command_token" value=""#);
    let start = page.find(&marker).unwrap_or_else(|| panic!("{page}")) + marker.len();
    let token = page[start..].split('"').next().unwrap().to_owned();
    let outcome = app.run(Command::SubmitForm { path: sync, token, fields: vec![] }).unwrap();
    let (html, status) = html(outcome);
    assert_eq!(status, 400, "this build's plugin host refuses; the list shows it");
    assert!(html.contains(r#"<p class="notice error" role="alert">Synthetic feed: "#), "{html}");
}

#[test]
fn the_assistant_page_is_the_loopback_case_with_the_desktop_note_and_its_switch_works() {
    let (_dir, _path, _key, mut app, co) = book();
    let c = co.to_db();
    let page = screens::render(&mut app, &format!("/c/{c}/ai/")).unwrap();
    assert_eq!(page.status, 200);
    assert!(page.html.contains(r#"<p class="ai-desktop-note">For powering your personal finance.</p>"#));
    assert!(page.html.contains(">Continue with ChatGPT</button>"));
    assert!(!page.html.contains("ai-tunnel"), "the SSH tunnel branch is server-only");
    assert!(page.html.contains(&format!(r#"<input type="hidden" name="company" value="{c}">"#)));
    let (html, status) = html(submit(&mut app, &format!("/c/{c}/ai/read-attachments/"), &[("enabled", "1")]));
    assert_eq!(status, 200);
    assert!(html.contains("AI may now read attachments for this company.") && html.contains("<strong>On.</strong>"), "{html}");
    // Sign in with ChatGPT goes only through the plugin host; this build's refusal shows as an error.
    let token = app.tokens.mint("/auth/openai/start");
    let outcome = app.run(Command::SubmitForm { path: "/auth/openai/start".into(), token, fields: vec![("company".into(), c.clone())] }).unwrap();
    match outcome {
        Outcome::External { location, .. } => assert_eq!(location, format!("/c/{c}/ai/")),
        Outcome::Page { html, .. } => assert!(html.contains(r#"<p class="errorlist" role="status">"#), "{html}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn removing_an_attachment_posts_with_its_own_token_and_returns_to_the_owner_with_a_note() {
    let (dir, path, _key, mut app, co) = book();
    let c = co.to_db();
    let bank = account(&mut app, co, "1000", "Checking", "BANK");
    let equity = account(&mut app, co, "3000", "Equity", "EQUITY");
    let journal = {
        let o = app.open_mut().unwrap();
        let (b, e) = (fb_core::AccountId::parse(&bank).unwrap(), fb_core::AccountId::parse(&equity).unwrap());
        let j = services::save_draft(&mut o.book, o.owner, co, services::DraftInput::new("2026-10-01", "Opening", vec![services::line(b).debit("10").build(), services::line(e).credit("10").build()])).unwrap();
        services::post_journal(&mut o.book, o.owner, co, j.id, None).unwrap().id
    };
    let store = fb_core::attachments::storage::Store::for_book(&path, &[7; 32]).unwrap();
    let pdf = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n1 0 obj<</Type/Catalog>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n";
    let limits = fb_core::attachments::Limits { max_mb: 10, uploads_per_window: 60, company_max_mb: 2048 };
    let id = {
        let o = app.open_mut().unwrap();
        let owner = fb_core::attachments::Owner::Journal(journal);
        fb_core::attachments::add(&mut o.book, &store, o.owner, co, &owner, pdf, "scan.pdf", "00000000-0000-0000-0000-0000000d9f01", &limits).unwrap().0["id"].as_str().unwrap().to_owned()
    };
    let remove = format!("/c/{c}/attachments/{id}/remove/");
    // A blank reason is refused by the service and comes back as a panel note; nothing is removed.
    let token = app.tokens.mint(&remove);
    let outcome = app.run(Command::SubmitForm { path: remove.clone(), token, fields: vec![("reason".into(), "   ".into())] }).unwrap();
    assert!(matches!(&outcome, Outcome::Redirect { location } if *location == format!("/c/{c}/journals/{}/#attachments", journal.to_db())), "{outcome:?}");
    let token = app.tokens.mint(&remove);
    app.run(Command::SubmitForm { path: remove, token, fields: vec![("reason".into(), "Wrong file".into())] }).unwrap();
    let deleted: bool = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT deleted FROM attachments_attachment WHERE id=?1", [&id], |r| r.get(0))).unwrap();
    assert!(deleted);
    drop(dir);
}

/// COMMON rule 10: the review -> record path end to end through the database guards.
#[test]
fn accepting_a_downloaded_transaction_from_the_review_page_posts_it_and_shows_the_reference() {
    let (_dir, _path, _key, mut app, co) = book();
    let c = co.to_db();
    let bank = account(&mut app, co, "1000", "Checking", "BANK");
    let fees = account(&mut app, co, "6100", "Bank fees", "EXPENSE");
    let conn = {
        let o = app.open_mut().unwrap();
        bankfeeds::connect(&mut o.book, o.owner, co, &json!("Synthetic feed"), "fixture.test", &bankfeeds::SecretSet::none()).unwrap()["id"].as_str().unwrap().to_owned()
    };
    let pull: bankfeeds::StagedPull = serde_json::from_value(json!({"accounts": [{"conn_key": "org1", "remote_id": "chk", "name": "Everyday", "org_name": "Synthetic Bank",
        "currency": "USD", "balance": 100000, "available": 100000, "balance_epoch": 1790000000, "transactions": [
            {"remote_id": "j1", "posted_on": "2026-10-08", "transacted_on": null, "amount": -310, "description": "Service charge", "pending": false}]}]})).unwrap();
    let feed = {
        let o = app.open_mut().unwrap();
        bankfeeds::store_pull(&mut o.book, o.owner, co, &conn, &pull, "2026-10-10T15:00:00Z", false, None, &bankfeeds::SecretSet::none()).unwrap();
        o.book.read(|q| q.query_row("SELECT id FROM bankfeeds_account", [], |r| r.get::<_, String>(0))).unwrap()
    };
    let review = format!("/c/{c}/bank-feeds/review/");
    // Mapping through the accounts form redirects to the list, like Django.
    let outcome = submit(&mut app, &format!("/c/{c}/bank-feeds/{conn}/accounts/"), &[(&format!("map_{feed}"), &bank)]);
    assert!(matches!(&outcome, Outcome::Redirect { location } if *location == format!("/c/{c}/bank-feeds/")), "{outcome:?}");
    let (page, status) = html(submit(&mut app, &review, &[("accept_one", "1"), ("kind_1", "JOURNAL"), ("account_1", &fees), ("payee_1", " Bank ")]));
    assert_eq!(status, 200, "{page}");
    assert!(page.contains("2026-10-08 Service charge -3.10: recorded as JE 1"), "{page}");
    let posted: i64 = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT count(*) FROM ledger_journal WHERE status='POSTED'", [], |r| r.get(0))).unwrap();
    assert_eq!(posted, 1);
    let status: String = app.open_mut().unwrap().book.read(|q| q.query_row("SELECT status FROM bankfeeds_transaction", [], |r| r.get(0))).unwrap();
    assert_eq!(status, "ACCEPTED");
}

#[test]
fn invalid_posts_render_django_errors_with_status_400_and_write_nothing() {
    let (_dir, _path, _key, mut app, co) = book();
    let c = co.to_db();
    account(&mut app, co, "6000", "Expense", "EXPENSE");
    let before = audit(&mut app);
    let (page, status) = html(submit(&mut app, &format!("/c/{c}/bank-feeds/connect/"), &[("label", ""), ("setup_token", "secret-token-text")]));
    assert_eq!(status, 400);
    assert!(page.contains(r#"<ul class="errorlist" id="id_label_error"><li>This field is required.</li></ul>"#), "{page}");
    assert!(!page.contains("secret-token-text"), "a pasted token is never echoed");
    // A clean form reaches the plugin host; its refusal is Django's non-field error, token not echoed.
    let (page, status) = html(submit(&mut app, &format!("/c/{c}/bank-feeds/connect/"), &[("label", "Feed"), ("setup_token", "secret-token-text")]));
    assert_eq!(status, 400);
    assert!(page.contains(r#"<ul class="errorlist nonfield"><li>"#) && !page.contains("secret-token-text"), "{page}");
    let (page, status) = html(submit(&mut app, &format!("/c/{c}/bank-feeds/rules/"), &[]));
    assert_eq!(status, 400);
    assert!(page.contains("Choose a rule action."));
    let (page, status) = html(submit(&mut app, &format!("/c/{c}/bank-feeds/rules/new/"), &[("name", ""), ("match_text", "x"), ("sign", "BOTH"), ("account", "")]));
    assert_eq!(status, 400);
    // Field order: every field reports its own error, none hides another.
    let name = page.find("id_name_error").unwrap();
    let sign = page.find("Select a valid choice. BOTH is not one of the available choices.").unwrap();
    let acct = page.find("id_account_error").unwrap();
    assert!(name < sign && sign < acct);
    let (page, status) = html(submit(&mut app, &format!("/c/{c}/bank-feeds/review/"), &[]));
    assert_eq!(status, 400);
    assert!(page.contains("Tick the rows to accept or ignore, or use a row button."));
    assert_eq!(audit(&mut app), before);
}

#[test]
fn a_rule_saves_through_submit_form_and_lists_in_order() {
    let (_dir, _path, _key, mut app, co) = book();
    let c = co.to_db();
    let expense = account(&mut app, co, "6000", "Expense", "EXPENSE");
    let outcome = submit(&mut app, &format!("/c/{c}/bank-feeds/rules/new/"), &[("name", " Office "), ("match_text", "office depot"), ("sign", "OUT"),
        ("amount_min", ""), ("amount_max", "50.00"), ("account", &expense), ("payee_name", "Office Depot"), ("active", "on")]);
    assert!(matches!(&outcome, Outcome::Redirect { location } if *location == format!("/c/{c}/bank-feeds/rules/")), "{outcome:?}");
    let page = screens::render(&mut app, &format!("/c/{c}/bank-feeds/rules/")).unwrap();
    assert!(page.html.contains(">Office</a>") && page.html.contains("– 50.00") && page.html.contains("6000 · Expense"), "{}", page.html);
    // The min > max refusal comes from the core service, as a non-field error.
    let (page, status) = html(submit(&mut app, &format!("/c/{c}/bank-feeds/rules/new/"), &[("name", "Range"), ("match_text", "r"), ("sign", "ANY"),
        ("amount_min", "9"), ("amount_max", "1"), ("account", &expense)]));
    assert_eq!(status, 400);
    assert!(page.contains(r#"<ul class="errorlist nonfield"><li>The minimum amount cannot exceed the maximum.</li></ul>"#), "{page}");
}

#[test]
fn bank_feed_pages_explain_missing_os_keyring_without_disabling_review() {
    let (_dir, _path, _key, mut app, co) = book();
    app.os_store_usable = false;
    let list = screens::render(&mut app, &format!("/c/{}/bank-feeds/", co.to_db())).unwrap().html;
    assert!(list.contains("OS keyring"));
    assert!(list.contains("Review downloaded transactions"));
    assert!(!list.contains("FB_SECRETS_KEY") && !list.contains("manage.py"));
    let connect = screens::render(&mut app, &format!("/c/{}/bank-feeds/connect/", co.to_db())).unwrap().html;
    assert!(connect.contains("OS keyring"));
    assert!(connect.contains("disabled>Connect bank</button>"));
    assert!(connect.contains("/settings/book-protection/"));
}
