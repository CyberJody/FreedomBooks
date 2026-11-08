//! Deliberate desktop divergences from the Django reference (NORMALIZATION.md "N13", DESKTOP-WAVES.md
//! "Deliberate divergences"). Authority: Jody, via Dev Master, 11/03: "Report a problem" is an in-app
//! how-to for filing a GitHub issue (the "No report address is set" placeholder removed, no email
//! reporting), and the footer drops "No payroll processing" (copy removal only).
//!
//! Applied to the **Django side only**, after every other rule: each rewrites one exact, normalized
//! reference fragment into the exact approved desktop fragment. The desktop page is never rewritten, so
//! any other difference on those screens, or any change to either fragment, still shows as a difference.

/// The screen diff's fixed host details (main.rs sets them on the Rust app), so the approved page is exact.
pub const VERSION: &str = "0.0.0-screendiff";
pub const OS: &str = "Screen-diff OS 1";

const FOOTER_OLD: &str = r#" · Bank feeds are read-only · No payroll processing · <a class="report-problem" href="/help/report/">Report a problem</a></footer></body></html>"#;
const FOOTER_NEW: &str = r#" · Bank feeds are read-only · <a class="report-problem" href="/help/report/">Report a problem</a></footer></body></html>"#;

const REPORT_OLD: &str = r#"<main id="main"><h1>Report a problem</h1><p>FreedomBooks is an alpha. Keep your real books in your current software.</p><p>Include:</p><ul><li>The steps you took</li><li>What you expected</li><li>What actually happened</li><li>A screenshot that does not show private data</li></ul><p>Never send real financial data, passwords, or keys.</p><p>No report address is set on this install. Give these notes to the person who runs this copy. This page does not list an outside address.</p></main>"#;

fn report_new() -> String {
    let url = "https://github.com/CyberJody/FreedomBooks/issues/new/choose";
    format!(
        r#"<main id="main"><h1>Report a problem</h1><p>FreedomBooks is an alpha. Keep your real books in your current software.</p><p>Problems are reported as issues on GitHub, the website where the FreedomBooks project lives. You need a free GitHub account.</p><p class="notice" role="note"><strong>GitHub issues are public.</strong> Anyone can read them. Never paste real financial data, customer names, passwords, keys or bank details.</p><ol class="report-steps"><li><a class="report-issue-link" href="{url}">Open the GitHub issue page</a> (it opens in your web browser). You can also copy the address: <code>{url}</code></li><li>Write the steps you took, one at a time, starting from Home.</li><li>Say what you expected to happen.</li><li>Say what actually happened. Copy any error message word for word.</li><li>Copy your FreedomBooks version and operating system into the issue: <pre class="report-details">FreedomBooks version: {VERSION} Operating system: {OS}</pre></li><li>Attach a screenshot only after covering names, amounts and account numbers.</li></ol></main>"#
    )
}

/// Rewrite the reference page's two known fragments: the footer only as the page's single footer at the
/// very end, the report body only as the one exact `<main>` of the old page.
pub fn django(text: String) -> String {
    let mut text = text;
    if text.matches("<footer>").count() == 1 && text.ends_with(FOOTER_OLD) {
        text.truncate(text.len() - FOOTER_OLD.len());
        text.push_str(FOOTER_NEW);
    }
    if text.matches(REPORT_OLD).count() == 1 && text.matches("<main").count() == 1 {
        text = text.replacen(REPORT_OLD, &report_new(), 1);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::super::{IdMap, first_difference, normalize};
    use super::*;
    use std::collections::BTreeMap;

    /// Django's actual report page (reference render, FIX3c screen diff), before normalization.
    const DJANGO_REPORT: &str = include_str!("fixtures/report-problem.django.html");

    fn rust_page(path: &str, host: fb_app::Host) -> String {
        let dir = std::env::temp_dir().join(format!("fb-screendiff-desktop-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = fb_app::App::new();
        // N13 against the reference page as Django renders it; plugins hidden is N14 (release.rs).
        app.plugins = true;
        app.create_book(&dir.join("b.fbbook"), &fb_app::keys::new_master_key().unwrap(), "owner", "Synthetic").unwrap();
        app.enroll_owner("Synthetic screen comparison password").unwrap();
        app.host = host;
        let html = fb_app::screens::render(&mut app, path).unwrap().html;
        let _ = std::fs::remove_dir_all(&dir);
        html
    }
    fn maps() -> (IdMap, IdMap) {
        (IdMap::new(&BTreeMap::new()).reference(), IdMap::new(&BTreeMap::new()))
    }
    fn screendiff_host() -> fb_app::Host {
        fb_app::Host::new(VERSION, OS)
    }

    // This rule owns report copy and footer copy. The frozen Django shell predates the
    // intentionally removed alpha banner and named-user controls; those have desktop tests.
    fn report_content(html: &str) -> &str {
        html.split_once("<main id=\"main\">").expect("report main content").1
    }

    #[test]
    fn reference_report_content_and_footer_match_the_approved_desktop_copy() {
        let (py, rs) = maps();
        let rust = rust_page("/help/report/", screendiff_host());
        let a = report_content(&normalize(DJANGO_REPORT, &py)).to_owned();
        let b = report_content(&normalize(&rust, &rs)).to_owned();
        assert_eq!(first_difference(&a, &b), None);
        assert!(b.contains("FreedomBooks version: 0.0.0-screendiff Operating system: Screen-diff OS 1"));
        // Without the rule (i.e. as the desktop side) the reference page is still the old page.
        assert!(normalize(DJANGO_REPORT, &rs).contains("No report address is set"));
    }

    #[test]
    fn any_other_difference_on_those_screens_still_differs() {
        let (py, rs) = maps();
        let rust = rust_page("/help/report/", screendiff_host());
        let a = report_content(&normalize(DJANGO_REPORT, &py)).to_owned();
        // Desktop-side changes: wording, the link, version/OS, the old footer phrase, the old placeholder.
        for (old, new) in [
            ("GitHub issues are public.", "GitHub issues are private."),
            ("issues/new/choose\">", "issues/new\">"),
            ("Report a problem</h1>", "Report problems</h1>"),
            ("account numbers.", "account numbers. Or email us."),
            (" · Bank feeds are read-only · <a", " · Bank feeds are read-only · No payroll processing · <a"),
            ("</ol>", "</ol><p>No report address is set on this install.</p>"),
        ] {
            assert!(rust.contains(old), "{old}");
            assert_ne!(report_content(&normalize(&rust.replacen(old, new, 1), &rs)), a, "{old} -> {new}");
        }
        let other_host = rust_page("/help/report/", fb_app::Host::new("0.1.0", OS));
        assert_ne!(report_content(&normalize(&other_host, &rs)), a, "the version must be the fixed screen-diff value");
        // Reference-side changes: the rule needs the exact old fragment, so a changed reference differs.
        for (old, new) in [
            ("This page does not list an outside address.", "This page lists an address."),
            ("<li>What you expected</li>", "<li>What you hoped</li>"),
            ("No payroll processing", "No payroll"),
            ("Bank feeds are read-only", "Bank feeds are live"),
        ] {
            assert!(DJANGO_REPORT.contains(old), "{old}");
            assert_ne!(report_content(&normalize(&DJANGO_REPORT.replacen(old, new, 1), &py)), report_content(&normalize(&rust, &rs)), "{old} -> {new}");
        }
    }

    #[test]
    fn footer_rule_is_only_the_phrase_at_the_end_of_the_single_footer() {
        let (py, rs) = maps();
        let page = |footer: &str| format!(r#"<html><body><main id="main"><h1>X</h1><p>No payroll processing · here</p></main><footer>FreedomBooks · USD{footer}"#);
        let django = page(FOOTER_OLD);
        let desktop = page(FOOTER_NEW);
        assert_eq!(normalize(&django, &py), normalize(&desktop, &rs));
        // The phrase in page content is untouched; a desktop page keeping the phrase differs.
        assert!(normalize(&django, &py).contains("<p>No payroll processing · here</p>"));
        assert_ne!(normalize(&django, &py), normalize(&django, &rs));
        // Text after the footer, or a second footer, disables the rule.
        assert_ne!(normalize(&format!("{django}<!-- x -->"), &py), normalize(&format!("{desktop}<!-- x -->"), &rs));
        let twice = format!("<footer>a</footer>{django}");
        assert_ne!(normalize(&twice, &py), normalize(&format!("<footer>a</footer>{desktop}"), &rs));
        // The Rust side is never rewritten.
        assert_eq!(normalize(&django, &rs), normalize(&django, &IdMap::new(&BTreeMap::new())));
    }

    #[test]
    fn a_real_shell_page_footer_matches_after_the_rule() {
        let (py, rs) = maps();
        let rust = normalize(&rust_page("/", screendiff_host()), &rs);
        let footer = rust.split_once("<footer>").unwrap().1;
        assert!(!footer.contains("payroll"));
        let django_footer = normalize(DJANGO_REPORT, &py);
        assert_eq!(footer, django_footer.split_once("<footer>").unwrap().1);
    }

    #[test]
    fn a_release_shell_page_footer_matches_after_n13_and_n14() {
        let (py, rs) = maps();
        let dir = std::env::temp_dir().join(format!("fb-screendiff-release-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = fb_app::App::new();
        app.plugins = false;
        app.create_book(&dir.join("b.fbbook"), &fb_app::keys::new_master_key().unwrap(), "owner", "Synthetic").unwrap();
        app.enroll_owner("Synthetic screen comparison password").unwrap();
        app.host = screendiff_host();
        let rust = normalize(&fb_app::screens::render(&mut app, "/").unwrap().html, &rs);
        let _ = std::fs::remove_dir_all(&dir);
        let footer = rust.split_once("<footer>").unwrap().1;
        assert!(!footer.contains("Bank feeds"));
        let django = super::super::release::django_after(&normalize(&super::super::release::django_before(DJANGO_REPORT), &py));
        assert_eq!(footer, django.split_once("<footer>").unwrap().1);
        // Without N14 the release footer differs from Django's.
        assert_ne!(footer, normalize(DJANGO_REPORT, &py).split_once("<footer>").unwrap().1);
    }
}
