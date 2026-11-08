//! DESKTOP-JODY1 (Jody, 11/03): "Report a problem" is a how-to for filing a public GitHub issue, and the
//! footer carries no disclaimer filler. Synthetic encrypted book; no network.
mod support;
use fb_app::{App, Host, ISSUE_URL, keys, screens};

fn app() -> (App, String) {
    let dir = std::env::temp_dir().join(format!("fb-app-report-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new();
    app.simplefin = false;
    let c = app.create_book(&dir.join("books.fbbook"), &keys::new_master_key().unwrap(), "Owner", "Synthetic company").unwrap().company_id.to_db();
    support::sign_in(&mut app);
    (app, c)
}

#[test]
fn report_page_is_a_github_issue_how_to_with_version_and_os() {
    let (mut app, _) = app();
    app.host = Host::new("0.1.0-test", "Debian GNU/Linux 13 (trixie)");
    let page = screens::render(&mut app, "/help/report/").unwrap();
    assert_eq!(page.status, 200);
    let html = page.html;
    assert_eq!(ISSUE_URL, "https://github.com/CyberJody/FreedomBooks/issues/new/choose");
    // The link (opened by the main process) and the same address as copyable text.
    assert!(html.contains(&format!(r#"<a class="report-issue-link" href="{ISSUE_URL}">Open the GitHub issue page</a>"#)), "{html}");
    assert!(html.contains(&format!("<code>{ISSUE_URL}</code>")));
    assert!(html.contains("FreedomBooks version: 0.1.0-test\nOperating system: Debian GNU/Linux 13 (trixie)</pre>"), "{html}");
    for step in ["Write the steps you took", "Say what you expected", "Say what actually happened", "Attach a screenshot only after covering names, amounts and account numbers"] {
        assert!(html.contains(step), "{step}");
    }
    assert!(html.contains("<strong>GitHub issues are public.</strong>"));
    assert!(html.contains("Never paste real financial data, customer names, passwords, keys or bank details."));
    // The old placeholder and any other outside address or email path are gone.
    for gone in ["No report address", "person who runs this copy", "does not list an outside address", "mailto:", "target=\"_blank\"", "QuickBooks", "Intuit"] {
        assert!(!html.contains(gone), "{gone}");
    }
    assert_eq!(html.matches("https://").count(), 2, "only the issue page, as link and text");
}

#[test]
fn host_text_is_escaped_plain_and_bounded() {
    let (mut app, _) = app();
    app.host = Host::new(" 1.2.3\u{202e}\n", "<b>Evil</b>\u{0}\tOS");
    assert_eq!(app.host, Host { version: "1.2.3".into(), os: "<b>Evil</b> OS".into() });
    let html = screens::render(&mut app, "/help/report/").unwrap().html;
    assert!(html.contains("Operating system: &lt;b&gt;Evil&lt;/b&gt; OS</pre>"), "{html}");
    assert_eq!(Host::new("", "\u{200b}").version, "Unknown");
    assert_eq!(Host::new("", "\u{200b}").os, "Unknown");
    assert_eq!(Host::new(&"x".repeat(500), "y").version.chars().count(), 100);
    // Before the shell reports in, the page still renders with the core's own version.
    assert_eq!(Host::default().version, env!("CARGO_PKG_VERSION"));
}

#[test]
fn shell_has_no_obsolete_alpha_or_record_only_notices() {
    let (mut app, c) = app();
    // Release builds hide the bank-feed phrase with the plugins; a plugins build keeps Django's footer.
    for (plugins, tail) in [(false, r#" · <a class="report-problem" href="/help/report/">Report a problem</a></footer>"#),
                            (true, r#" · Bank feeds are read-only · <a class="report-problem" href="/help/report/">Report a problem</a></footer>"#)] {
        app.plugins = plugins;
        for path in ["/".to_owned(), "/help/report/".to_owned(), format!("/c/{c}/home/"), format!("/c/{c}/accounts/new/"), format!("/c/{c}/trial-balance/"), format!("/c/{c}/history/")] {
            let html = screens::render(&mut app, &path).unwrap().html;
            assert!(!html.contains("Alpha: keep your real books"), "{path}: obsolete alpha banner");
            assert!(!html.contains("Record-only books."), "{path}: obsolete Home disclaimer");
            assert!(!html.contains("Nothing here moves money, contacts a bank, prints or sends anything."), "{path}: obsolete capability claim");
            let footer = html.split_once("<footer>").map(|(_, f)| f).unwrap_or_else(|| panic!("{path}: no footer"));
            assert!(!footer.to_lowercase().contains("payroll"), "{path}: {footer}");
            assert!(footer.starts_with("FreedomBooks · "), "{path}: {footer}");
            assert!(footer.contains(tail), "{path} plugins={plugins}: {footer}");
            assert_eq!(footer.contains("Bank feeds"), plugins, "{path}: {footer}");
        }
    }
}
