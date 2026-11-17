//! The frame around every screen: the sidebar's "you are here", the breadcrumb, the company frame on
//! account pages, the title bar's sign-out, and the Settings entries each role sees.
use fb_app::{App, Command, Outcome, identity::{Role, SESSION, SIGN_IN}, keys::MasterKey, screens};
use fb_core::CompanyId;
use serde_json::json;

const OWNER: &str = "Synthetic owner";
const PASSWORD: &str = "Synthetic individual password";

struct Fixture {
    _dir: tempfile::TempDir,
    app: App,
    c: String,
    company: CompanyId,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("books/default.fbbook");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut app = App::new();
    app.backups.configure(dir.path()).unwrap();
    let company = app.create_book(&path, &MasterKey::new([41; 32]), OWNER, "Synthetic frame company").unwrap().company_id;
    app.enroll_owner(PASSWORD).unwrap();
    Fixture { _dir: dir, app, c: company.to_db(), company }
}

fn page(app: &mut App, path: &str) -> String {
    let r = screens::render(app, path).unwrap();
    assert_eq!(r.status, 200, "{path}: {}", r.html);
    r.html
}

/// The text of the highlighted sidebar entries.
fn highlighted(html: &str) -> Vec<String> {
    let nav = html.split_once(r#"<nav class="appnav""#).unwrap().1.split_once("</nav>").unwrap().0;
    nav.split(r#" aria-current="page""#).skip(1).map(|rest| rest.split("<span>").nth(1).unwrap().split("</span>").next().unwrap().to_owned()).collect()
}

fn crumbs(html: &str) -> String {
    html.split_once(r#"<nav class="crumbs" aria-label="Breadcrumb">"#).map(|(_, r)| r.split_once("</nav>").unwrap().0.to_owned()).unwrap_or_default()
}

fn account(app: &mut App, c: &str, code: &str, detail: &str) -> String {
    let Outcome::Done { data } = app.run(serde_json::from_value(json!({"type":"SaveAccount","company_id":c,"code":code,"name":format!("Account {code}"),"detail_type":detail})).unwrap()).unwrap() else { panic!() };
    data["id"].as_str().unwrap().to_owned()
}

#[test]
fn registers_highlight_bank_accounts_or_the_chart_by_account_type() {
    let mut f = fixture();
    let c = f.c.clone();
    let bank = account(&mut f.app, &c, "1010", "BANK");
    let income = account(&mut f.app, &c, "4010", "INCOME");
    let html = page(&mut f.app, &format!("/c/{c}/accounts/{bank}/register/"));
    assert_eq!(highlighted(&html), ["Bank accounts"]);
    let trail = crumbs(&html);
    assert!(trail.contains(&format!(r#"<a href="/c/{c}/registers/">Bank accounts</a>"#)) && trail.contains("1010 · Account 1010"), "{trail}");
    let html = page(&mut f.app, &format!("/c/{c}/accounts/{income}/register/"));
    assert_eq!(highlighted(&html), ["Chart of accounts"]);
    // Every list and form page below a sidebar entry highlights exactly that entry.
    for (path, entry) in [("invoices/new/", "Invoices"), ("estimates/", "Invoices"), ("customer-deposits/", "Payments received"),
        ("bills/calendar/", "Bills"), ("deposits/", "Checks &amp; expenses"), ("history/", "Reports"), ("", "Reports"),
        ("reminders/", "Home"), ("stripe/", "Settings"), ("edit/", "Settings"), ("sales-tax/rates/", "Settings")] {
        if path == "stripe/" && !f.app.invoice_delivery { continue; }
        let html = page(&mut f.app, &format!("/c/{c}/{path}"));
        assert_eq!(highlighted(&html), [entry], "{path}");
    }
}

#[test]
fn account_pages_keep_the_company_frame_and_settings_highlight() {
    let mut f = fixture();
    let c = f.c.clone();
    page(&mut f.app, &format!("/c/{c}/home/"));
    for path in ["/desktop/mfa/", "/desktop/users/", "/desktop/session/", "/settings/book-protection/"] {
        let html = page(&mut f.app, path);
        assert!(html.contains(r#"<span class="menu-name">Synthetic frame company</span>"#), "{path}: company menu");
        assert_eq!(highlighted(&html), ["Settings"], "{path}");
        assert!(html.contains(r#"<nav class="settings-menu" aria-label="Settings">"#), "{path}: settings menu");
    }
    let help = page(&mut f.app, "/help/report/");
    assert!(help.contains(&format!(r#"href="/c/{c}/home/""#)), "report a problem keeps the sidebar");
}

#[test]
fn trusted_company_selection_follows_the_frame_and_clears_with_the_named_session() {
    let mut f = fixture();
    assert_eq!(screens::frame_company(&mut f.app), None);
    page(&mut f.app, &format!("/c/{}/home/", f.c));
    for path in ["/help/report/", "/desktop/users/", "/desktop/mfa/", "/desktop/session/", "/settings/book-protection/"] {
        page(&mut f.app, path);
        assert_eq!(screens::frame_company(&mut f.app), Some(f.company), "{path}");
    }
    let other = {
        let open = f.app.open_mut().unwrap();
        fb_core::services::create_company(&mut open.book, open.owner, "Synthetic second company", "USD", false).unwrap().id
    };
    page(&mut f.app, &format!("/c/{}/home/", other.to_db()));
    assert_eq!(screens::frame_company(&mut f.app), Some(other));
    f.app.sign_out();
    assert_eq!(screens::frame_company(&mut f.app), None);
    f.app.sign_in(OWNER, PASSWORD).unwrap();
    assert_eq!(screens::frame_company(&mut f.app), None, "a new sign-in must select a company again");
}

#[test]
fn title_bar_sign_out_posts_one_reusable_session_token() {
    let mut f = fixture();
    let home = format!("/c/{}/home/", f.c);
    // A page without forms of its own renders identically twice: the sign-out token is reused.
    let reports = format!("/c/{}/reports/", f.c);
    assert_eq!(page(&mut f.app, &reports), page(&mut f.app, &reports), "a page renders the same twice");
    let first = page(&mut f.app, &home);
    let form = first.split_once(r#"<form id="fb-signout" method="post" action="/desktop/session/" hidden>"#).expect("sign-out form").1;
    let token = form.split_once(r#"name="fb_command_token" value=""#).unwrap().1.split('"').next().unwrap().to_owned();
    // The page's own forms come first, so a page's first command token is never the sign-out one.
    assert!(first.find(r#"id="fb-signout""#).unwrap() > first.find("</main>").unwrap());
    assert!(first.contains(r#"<button type="submit" form="fb-signout">Sign out</button>"#));
    let out = f.app.run_renderer(Command::SubmitForm { path: SESSION.into(), token, fields: vec![("action".into(), "signout".into())] }).unwrap();
    assert!(matches!(out, Outcome::Redirect { ref location } if location == SIGN_IN), "{out:?}");
    assert!(f.app.authenticated_session().is_err());
    assert_eq!(screens::render(&mut f.app, &home).unwrap().html, SIGN_IN);
}

#[test]
fn settings_shows_each_role_only_what_it_may_open() {
    let mut f = fixture();
    let c = f.c.clone();
    let admin = page(&mut f.app, &format!("/c/{c}/settings/"));
    for entry in ["Company profile", "Document numbering", "Accounting setup", "Sales tax and rates", "Invoices and email", "Users", "My sign-in and MFA", "Book protection", "Backups"] {
        assert!(admin.contains(&format!(">{entry}</a>")), "administrator: {entry}");
    }
    assert!(admin.contains(r#"<li><a href="/companies/new/">Create company</a></li>"#));
    f.app.save_desktop_user(None, "Synthetic bookkeeper", Role::Bookkeeper, true, Some(PASSWORD), &[f.company]).unwrap();
    f.app.sign_in("Synthetic bookkeeper", PASSWORD).unwrap();
    let keeper = page(&mut f.app, &format!("/c/{c}/settings/"));
    for entry in ["Accounting setup", "Sales tax and rates", "My sign-in and MFA"] {
        assert!(keeper.contains(&format!(">{entry}</a>")), "bookkeeper: {entry}");
    }
    for entry in ["Company profile", "Document numbering", "Invoices and email", "Users", "Book protection", "Backups", "AI access"] {
        assert!(!keeper.contains(&format!(">{entry}</a>")), "bookkeeper must not see {entry}");
    }
    assert!(!keeper.contains("Create company"));
    // Hiding an entry changes nothing on the server: the pages still refuse a bookkeeper.
    assert!(screens::render(&mut f.app, &format!("/c/{c}/edit/")).is_err());
    assert!(screens::render(&mut f.app, "/desktop/users/").is_err());
    assert!(screens::render(&mut f.app, "/settings/book-protection/").is_err());
}
